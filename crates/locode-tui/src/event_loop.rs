//! The main loop: one biased `select!` over signals, engine messages
//! (gated on empty input, batch-drained), input, and timers; draw-on-change
//! with a ~16 ms cap and zero idle wakeups (study §6.2/6.3).

use std::process::ExitCode;
use std::time::{Duration, Instant};

use ratatui::text::Line;

use crate::app::{App, Cmd, Msg};
use crate::approval::ApprovalOutcome;
use crate::cli::Cli;
use crate::engine::{self, EngineMsg, UiCommand};
use crate::{term, ui};
use locode_core::ProviderRegistry;

/// Minimum interval between paints while events stream in.
const MIN_DRAW_INTERVAL: Duration = Duration::from_millis(16);
/// Resize storms collapse into one relayout (grok: 16 ms debounce).
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(16);
/// Spinner cadence while a run is active (only timer that exists when busy).
const TICK_INTERVAL: Duration = Duration::from_millis(100);
/// Max engine messages drained per loop iteration (grok's ACP batch bound).
const ENGINE_DRAIN_MAX: usize = 32;

/// A setup failure before or during the UI run.
pub struct RunError(pub String);

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<E: std::error::Error> From<E> for RunError {
    fn from(e: E) -> Self {
        RunError(e.to_string())
    }
}

/// Run the app to completion.
pub async fn run(cli: Cli, registry: ProviderRegistry) -> Result<ExitCode, RunError> {
    term::install_panic_hook();
    let mut terminal = term::init()?;
    let mut input_rx = term::spawn_input_reader();
    let mut signal_rx = spawn_signal_task();
    // Pre-fill from a positional prompt before `cli` moves into the engine.
    let initial_draft = cli.prompt.clone();
    let (engine_tx, mut engine_rx) = engine::spawn(cli, registry);

    let mut app = match &initial_draft {
        Some(prompt) => App::with_draft(prompt),
        None => App::new(),
    };
    let mut last_draw = Instant::now()
        .checked_sub(MIN_DRAW_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut deferred_draw: Option<Instant> = None;
    let mut resize_at: Option<Instant> = None;
    let mut next_tick: Option<Instant> = None;
    // The current run's cancel handle (ADR-0018): captured at RunStarted,
    // fired on Cmd::CancelRun, cleared at RunFinished. Loop-owned so the
    // reducer stays sans-IO.
    let mut current_cancel: Option<locode_core::CancellationToken> = None;
    // Pending approval oneshots keyed by tool_use id (ADR-0017). Bounded by
    // the engine's serial dispatch (one in flight) but a map for generality.
    let mut approvals: PendingApprovals = std::collections::HashMap::new();
    // The recent transcript tail rendered inside the bottom-anchored frame
    // (ADR-0022). Rows that overflow the screen top are committed to native
    // scrollback via `scroll_up` and drained here; there is no `insert_before`.
    let mut tail: Vec<Line<'static>> = Vec::new();
    // Set once any tail rows have been committed to native scrollback (the
    // `viewportY` moved); reserved for the shrink-below-scrollback guard.
    let mut committed: bool = false;

    let exit_code = loop {
        if app.should_quit {
            break ExitCode::SUCCESS;
        }

        // Fold finalized blocks into the transcript tail, then paint the whole
        // bottom-anchored frame (rate-capped; deferred paint instead of
        // spinning). Overflow leaves the frame via `scroll_up`, not
        // `insert_before` (ADR-0022).
        if !app.outbox.is_empty() {
            flush_outbox(&terminal, &mut app, &mut tail)?;
        }
        if app.dirty {
            let now = Instant::now();
            if now.duration_since(last_draw) >= MIN_DRAW_INTERVAL {
                paint(&mut terminal, &mut app, &mut tail, &mut committed)?;
                app.dirty = false;
                last_draw = now;
                deferred_draw = None;
            } else if deferred_draw.is_none() {
                deferred_draw = Some(last_draw + MIN_DRAW_INTERVAL);
            }
        }

        // Animation tick exists only while a run is active (zero idle wakeups).
        if app.is_running() {
            if next_tick.is_none() {
                next_tick = Some(Instant::now() + TICK_INTERVAL);
            }
        } else {
            next_tick = None;
        }

        let timer = [deferred_draw, resize_at, next_tick]
            .into_iter()
            .flatten()
            .min();
        tokio::select! {
            biased;
            _ = signal_rx.recv() => {
                let mut io = LoopIo { engine_tx: &engine_tx, current_cancel: current_cancel.as_ref(), approvals: &mut approvals };
                run_reducer(&mut app, Msg::SignalQuit, &mut io);
            }
            // Engine arm gated on an empty input queue so a busy engine can
            // never starve keystrokes; bounded batch drain (grok's rule).
            maybe_msg = engine_rx.recv(), if input_rx.is_empty() => {
                let Some(first) = maybe_msg else {
                    // Engine task gone (BuildFailed already surfaced); the
                    // app stays usable for quit keys.
                    continue;
                };
                route_engine(&mut app, first, &engine_tx, &mut current_cancel, &mut approvals);
                for _ in 1..ENGINE_DRAIN_MAX {
                    if !input_rx.is_empty() {
                        break;
                    }
                    match engine_rx.try_recv() {
                        Ok(msg) => route_engine(&mut app, msg, &engine_tx, &mut current_cancel, &mut approvals),
                        Err(_) => break,
                    }
                }
            }
            maybe_event = input_rx.recv() => {
                let Some(event) = maybe_event else {
                    // Input thread died: nothing can reach us — exit cleanly.
                    break ExitCode::from(1);
                };
                if matches!(event, crossterm::event::Event::Resize(..)) {
                    resize_at = Some(Instant::now() + RESIZE_DEBOUNCE);
                    continue;
                }
                let mut io = LoopIo { engine_tx: &engine_tx, current_cancel: current_cancel.as_ref(), approvals: &mut approvals };
                run_reducer(&mut app, Msg::Input(Box::new(event)), &mut io);
            }
            () = sleep_until(timer), if timer.is_some() => {
                let now = Instant::now();
                if resize_at.is_some_and(|at| now >= at) {
                    resize_at = None;
                    // ADR-0022: a resize is a full reset — clear the screen and
                    // drop the diff baseline so the next paint fully repaints at
                    // the new size (the debounce collapsed the storm).
                    terminal.clear()?;
                    app.dirty = true;
                }
                if next_tick.is_some_and(|at| now >= at) {
                    next_tick = None; // rescheduled at loop top while running
                    let mut io = LoopIo { engine_tx: &engine_tx, current_cancel: current_cancel.as_ref(), approvals: &mut approvals };
                    run_reducer(&mut app, Msg::Tick, &mut io);
                }
                // A due deferred draw is handled by the top-of-loop paint.
            }
        }
    };

    term::restore();
    Ok(exit_code)
}

/// Render finalized blocks into the transcript `tail` (ADR-0022): no
/// `insert_before` — the tail is part of the one bottom-anchored frame, and
/// rows that overflow the screen top are committed to native scrollback by
/// [`paint`]. Marks the app dirty so the next iteration repaints.
fn flush_outbox<B: ratatui::backend::Backend>(
    terminal: &crate::frame_terminal::FrameTerminal<B>,
    app: &mut App,
    tail: &mut Vec<Line<'static>>,
) -> std::io::Result<()> {
    let width = terminal.size()?.width;
    let rendered: Vec<Line<'static>> = app
        .outbox
        .drain(..)
        .flat_map(|block| block.render(width))
        .collect();
    tail.extend(rendered);
    app.dirty = true;
    Ok(())
}

/// Paint one bottom-anchored frame (ADR-0022): commit the transcript overflow
/// to native scrollback via `scroll_up`, then diff-render the visible tail plus
/// the pinned status/composer/footer as a single buffer update.
fn paint<B: ratatui::backend::Backend>(
    terminal: &mut crate::frame_terminal::FrameTerminal<B>,
    app: &mut App,
    tail: &mut Vec<Line<'static>>,
    committed: &mut bool,
) -> std::io::Result<()> {
    let size = terminal.size()?;
    let v = size.height;
    let (composer_rows, non_tail) = ui::live_rows(app, size.width, v);
    // Glue the caret to the composer's bottom line (editor rows = composer rows
    // minus the two framing rules).
    app.composer.sync_scroll(composer_rows.saturating_sub(2));

    // Transcript rows that fit on screen; the rest is the oldest tail (the top
    // rows of the current full frame) and is committed to scrollback.
    let tail_cap = usize::from(v.saturating_sub(non_tail));
    let overflow = tail.len().saturating_sub(tail_cap);
    if overflow > 0 {
        // Commit the actual oldest lines (chunked by screen height inside
        // `commit_scrollback`), so the right transcript lands in scrollback even
        // when the frame wasn't full — then drop exactly those from the tail.
        let commit_lines: Vec<Line<'static>> = tail[..overflow].to_vec();
        terminal.commit_scrollback(&commit_lines)?;
        tail.drain(..overflow);
        *committed = true;
    }

    // Own the visible slice before the draw so `tail`/`app` borrows don't clash
    // with the closure's `&mut terminal`.
    let shown = tail.len().min(tail_cap);
    let visible: Vec<Line<'static>> = tail[tail.len() - shown..].to_vec();
    terminal.draw(|frame| ui::draw(frame, app, &visible, composer_rows))?;
    Ok(())
}

/// The `LOCODE_TUI_DEBUG_LOG` path, resolved once at first use.
static DEBUG_LOG_PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Append one line to the debug log when `LOCODE_TUI_DEBUG_LOG` is set — the
/// diff-painted frame leaves nothing greppable in a captured pty, so this is the
/// reusable smoke/bug-report instrumentation. No-op otherwise.
fn debug_log(line: &str) {
    let path = DEBUG_LOG_PATH.get_or_init(|| std::env::var("LOCODE_TUI_DEBUG_LOG").ok());
    if let Some(path) = path {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// The map of pending approval oneshots, keyed by `tool_use` id (the loop
/// holds these; the reducer holds the display queue).
type PendingApprovals =
    std::collections::HashMap<String, tokio::sync::oneshot::Sender<ApprovalOutcome>>;

/// The loop-owned IO the reducer's commands drive.
struct LoopIo<'a> {
    engine_tx: &'a tokio::sync::mpsc::UnboundedSender<UiCommand>,
    current_cancel: Option<&'a locode_core::CancellationToken>,
    approvals: &'a mut PendingApprovals,
}

/// Route an engine message: manage the loop-owned cancel handle + approval
/// oneshots around the run lifecycle, then dispatch a reducer-visible message.
fn route_engine(
    app: &mut App,
    msg: EngineMsg,
    engine_tx: &tokio::sync::mpsc::UnboundedSender<UiCommand>,
    current_cancel: &mut Option<locode_core::CancellationToken>,
    approvals: &mut PendingApprovals,
) {
    match msg {
        EngineMsg::RunStarted { cancel } => {
            *current_cancel = Some(cancel.clone());
            dispatch_engine(
                app,
                EngineMsg::RunStarted { cancel },
                engine_tx,
                current_cancel.as_ref(),
                approvals,
            );
        }
        EngineMsg::RunFinished(report) => {
            *current_cancel = None;
            approvals.clear(); // defensive: senders should already be resolved
            dispatch_engine(
                app,
                EngineMsg::RunFinished(report),
                engine_tx,
                current_cancel.as_ref(),
                approvals,
            );
        }
        // Take the responder into the loop's map; forward the display view.
        EngineMsg::Approval(ask) => {
            let crate::approval::ApprovalAsk { view, respond } = ask;
            approvals.insert(view.tool_use_id.clone(), respond);
            let mut io = LoopIo {
                engine_tx,
                current_cancel: current_cancel.as_ref(),
                approvals,
            };
            run_reducer(app, Msg::Approval(view), &mut io);
        }
        other => dispatch_engine(app, other, engine_tx, current_cancel.as_ref(), approvals),
    }
}

fn dispatch_engine(
    app: &mut App,
    msg: EngineMsg,
    engine_tx: &tokio::sync::mpsc::UnboundedSender<UiCommand>,
    current_cancel: Option<&locode_core::CancellationToken>,
    approvals: &mut PendingApprovals,
) {
    let mut io = LoopIo {
        engine_tx,
        current_cancel,
        approvals,
    };
    run_reducer(app, Msg::Engine(Box::new(msg)), &mut io);
}

/// Run the reducer and execute the returned commands (all IO lives here).
fn run_reducer(app: &mut App, msg: Msg, io: &mut LoopIo<'_>) {
    debug_log(&format!("msg: {msg:?}"));
    let cmds = app.update(msg, Instant::now());
    for cmd in cmds {
        match cmd {
            Cmd::Quit => app.should_quit = true,
            Cmd::Submit(text) => {
                let _ = io.engine_tx.send(UiCommand::Submit(text));
            }
            Cmd::NewSession => {
                let _ = io.engine_tx.send(UiCommand::NewSession);
            }
            // Fire the run's cancel handle (idempotent — ADR-0018) AND drain
            // every pending approval with Deny, so a run parked in an
            // approval await (the ADR-0017 gap) unblocks and settles as
            // cancelled.
            Cmd::CancelRun => {
                if let Some(cancel) = io.current_cancel {
                    cancel.cancel();
                }
                for (_, tx) in io.approvals.drain() {
                    let _ = tx.send(ApprovalOutcome::Deny {
                        reason: "run cancelled".to_string(),
                    });
                }
            }
            Cmd::ResolveApproval { id, outcome } => {
                if let Some(tx) = io.approvals.remove(&id) {
                    let _ = tx.send(outcome);
                }
            }
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
        // The `if timer.is_some()` guard keeps this arm disabled when idle —
        // zero wakeups; pending() documents the intent.
        None => std::future::pending().await,
    }
}

/// SIGINT/SIGTERM → graceful quit through the same teardown as /quit.
fn spawn_signal_task() -> tokio::sync::mpsc::UnboundedReceiver<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(unix)]
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut int), Ok(mut term)) = (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) else {
            return;
        };
        loop {
            tokio::select! {
                _ = int.recv() => {}
                _ = term.recv() => {}
            }
            if tx.send(()).is_err() {
                return;
            }
        }
    });
    #[cfg(not(unix))]
    drop(tx);
    rx
}

#[cfg(test)]
mod tests {
    use super::paint;
    use crate::app::App;
    use crate::frame_terminal::FrameTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::text::Line;

    fn rows(t: &FrameTerminal<TestBackend>) -> Vec<String> {
        let b = t.backend().buffer();
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn scrollback(t: &FrameTerminal<TestBackend>) -> Vec<String> {
        let b = t.backend().scrollback();
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// End-to-end geometry through `paint`: the composer is pinned to the bottom;
    /// the transcript tail renders above it; a burst that overflows the screen
    /// (the first-big-response case) commits the *correct* oldest lines to
    /// scrollback with NO blank rows injected (the regression that killed the
    /// earlier attempts); and a following short frame stays clean.
    #[test]
    fn paint_pins_composer_commits_overflow_and_never_pollutes_scrollback() {
        // 30 wide, 8 tall. Empty composer = 3 rows + 1 footer = non_tail 4, so
        // tail_cap = 4 transcript rows fit.
        let mut t = FrameTerminal::new(TestBackend::new(30, 8)).unwrap();
        let mut app = App::new();
        let mut committed = false;

        // A single burst of 10 transcript lines (taller than the 4-row cap and
        // the 8-row screen) — the jump-from-not-full case.
        let mut tail: Vec<Line<'static>> =
            (0..10).map(|i| Line::from(format!("L{i:02}"))).collect();
        paint(&mut t, &mut app, &mut tail, &mut committed).unwrap();

        let r = rows(&t);
        // Composer prompt sits on the last rows (bottom-pinned).
        assert!(
            r.iter().rev().take(3).any(|l| l.contains('❯')),
            "composer pinned to bottom: {r:?}"
        );
        // The last 4 transcript lines are the visible tail (L06..L09).
        for id in ["L06", "L07", "L08", "L09"] {
            assert!(r.iter().any(|l| l == id), "{id} visible: {r:?}");
        }
        // The oldest 6 (L00..L05) went to scrollback — the RIGHT lines, in order.
        let sb = scrollback(&t);
        for id in ["L00", "L01", "L02", "L03", "L04", "L05"] {
            assert!(sb.iter().any(|l| l == id), "{id} committed: {sb:?}");
        }
        // The crux: scrollback contains NO blank rows between committed lines
        // (no scroll_region_down blank injection).
        let first = sb.iter().position(|l| l == "L00").unwrap();
        let last = sb.iter().position(|l| l == "L05").unwrap();
        assert!(
            sb[first..=last].iter().all(|l| !l.is_empty()),
            "no blank rows injected into scrollback: {sb:?}"
        );

        // Now type into the composer (grow) then a shorter frame: no new commits,
        // scrollback untouched.
        let sb_len_before = scrollback(&t).iter().filter(|l| !l.is_empty()).count();
        app.composer.insert_text("hi");
        paint(&mut t, &mut app, &mut tail, &mut committed).unwrap();
        let r2 = rows(&t);
        assert!(r2.iter().any(|l| l.contains("❯ hi")), "typed text: {r2:?}");
        let sb_len_after = scrollback(&t).iter().filter(|l| !l.is_empty()).count();
        assert_eq!(
            sb_len_before, sb_len_after,
            "editing the composer must not touch scrollback"
        );
    }
}
