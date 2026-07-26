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
#[allow(clippy::too_many_lines)] // the one cohesive select! loop — splitting hurts clarity
pub async fn run(cli: Cli, registry: ProviderRegistry) -> Result<ExitCode, RunError> {
    term::install_panic_hook();
    // The guard, not the tail of this function, owns the teardown: every `?`
    // below (paint, size, clear) used to return past `term::restore()` and
    // leave the terminal raw with the keyboard enhancement still on.
    let (mut terminal, _restore_guard) = term::init()?;
    let mut input_rx = term::spawn_input_reader();
    let mut signal_rx = spawn_signal_task();
    // Pre-fill from a positional prompt before `cli` moves into the engine.
    let initial_draft = cli.prompt.clone();
    let restricted = cli.restricted;
    let show_hidden = cli.debug_show_hidden_context;
    let (engine_tx, mut engine_rx) = engine::spawn(cli, registry);

    let mut app = match &initial_draft {
        Some(prompt) => App::with_draft(prompt),
        None => App::new(),
    };
    if show_hidden {
        app = app.showing_hidden_context();
    }
    // The permission posture is stated up front (ADR-0008 amendment 2026-07-24):
    // unrestricted is the default, and restricted is knowingly incomplete. The TUI
    // has no stderr surface, so it rides the same notice channel as other advisories.
    app.outbox.push(crate::ui::blocks::Block::Notice(
        if restricted {
            locode_exec::RESTRICTED_MODE_NOTICE
        } else {
            locode_exec::UNRESTRICTED_MODE_NOTICE
        }
        .to_string(),
    ));
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
        // Commit completed streaming blocks into the tail (→ scrollback), so a
        // long in-progress reply is scrollable mid-stream (ADR-0021). This runs
        // BEFORE `flush_outbox`: the last assistant `Message` (which signals the
        // finalize) and `RunFinished` (which pushes the `completed · … · Ns`
        // turn-end summary to the outbox) are drained in the SAME batch, so the
        // final block must be committed to the tail before the summary — else the
        // summary interleaves into the middle of the reply (ADR-0022).
        fold_streaming(&terminal, &mut app, &mut tail)?;
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
                run_reducer(&mut app, Msg::SignalQuit, &mut io).await;
            }
            // Engine arm gated on an empty input queue so a busy engine can
            // never starve keystrokes; bounded batch drain (grok's rule).
            maybe_msg = engine_rx.recv(), if input_rx.is_empty() => {
                let Some(first) = maybe_msg else {
                    // Engine task gone (BuildFailed already surfaced); the
                    // app stays usable for quit keys.
                    continue;
                };
                route_engine(&mut app, first, &engine_tx, &mut current_cancel, &mut approvals)
                    .await;
                for _ in 1..ENGINE_DRAIN_MAX {
                    if !input_rx.is_empty() {
                        break;
                    }
                    match engine_rx.try_recv() {
                        Ok(msg) => {
                            route_engine(
                                &mut app,
                                msg,
                                &engine_tx,
                                &mut current_cancel,
                                &mut approvals,
                            )
                            .await;
                        }
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
                run_reducer(&mut app, Msg::Input(Box::new(event)), &mut io).await;
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
                    run_reducer(&mut app, Msg::Tick, &mut io).await;
                }
                // A due deferred draw is handled by the top-of-loop paint.
            }
        }
    };

    Ok(exit_code) // `_restore_guard` tears the terminal down on the way out
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

/// Commit *completed* markdown blocks of the in-progress streaming message into
/// the transcript `tail` (which `paint` then commits to native scrollback), so a
/// long reply is scrollable mid-stream (ADR-0021). Only the current in-progress
/// block stays in `app.streaming` (rendered as the live cell). On finalize, the
/// remaining block is committed and the streaming state cleared. Completed blocks
/// are stable (fence-aware boundary) so they never reflow after committing.
fn fold_streaming<B: ratatui::backend::Backend>(
    terminal: &crate::frame_terminal::FrameTerminal<B>,
    app: &mut App,
    tail: &mut Vec<Line<'static>>,
) -> std::io::Result<()> {
    let width = terminal.size()?.width;
    if app.streaming_finalize {
        if let Some(text) = app.streaming.take()
            && !text.trim().is_empty()
        {
            tail.extend(ui::blocks::render_assistant_chunk(
                &text,
                width,
                !app.streaming_committed_any,
            ));
        }
        app.streaming = None;
        app.streaming_committed_any = false;
        app.streaming_finalize = false;
        app.dirty = true;
        return Ok(());
    }
    let Some(buffer) = app.streaming.as_deref() else {
        return Ok(());
    };
    let stable_end = ui::blocks::stable_prefix_end(buffer);
    if stable_end > 0 {
        let stable = buffer[..stable_end].to_string();
        let remainder = buffer[stable_end..].to_string();
        tail.extend(ui::blocks::render_assistant_chunk(
            &stable,
            width,
            !app.streaming_committed_any,
        ));
        app.streaming = Some(remainder);
        app.streaming_committed_any = true;
        app.dirty = true;
    }
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
    // (The composer self-scrolls to keep the caret on its bottom row while the
    // draft overflows — computed inside `Composer::render`, not here.)

    // Transcript rows that fit on screen; the rest is the oldest tail (the top
    // rows of the current full frame) and is committed to scrollback.
    // Retain up to a *minimum-composer* screen of transcript in memory, so
    // shrinking the composer can refill the freed rows from memory (no gap) —
    // the transcript "comes back down". Only commit lines older than that to
    // native scrollback (permanent). `non_tail - composer_rows` is the status +
    // queue + footer rows; the min frame keeps `MIN_COMPOSER_ROWS` for the input.
    let min_non_tail = non_tail
        .saturating_sub(composer_rows)
        .saturating_add(term::MIN_COMPOSER_ROWS);
    let max_tail = usize::from(v.saturating_sub(min_non_tail));
    let overflow = tail.len().saturating_sub(max_tail);
    if overflow > 0 {
        // Commit the actual oldest lines (chunked by screen height inside
        // `commit_scrollback`), so the right transcript lands in scrollback even
        // when those lines aren't currently on screen — then drop exactly those.
        let commit_lines: Vec<Line<'static>> = tail[..overflow].to_vec();
        // Protect the bottom `non_tail` rows (status + queue + composer + footer)
        // from the scroll so the composer is never smeared into the transcript.
        terminal.commit_scrollback(&commit_lines, non_tail)?;
        tail.drain(..overflow);
        *committed = true;
    }

    // Show as many recent tail lines as fit under the *current* composer; the
    // rest stay in memory (revealed again if the composer shrinks). Own the
    // slice before the draw so `tail`/`app` borrows don't clash with the closure.
    let tail_cap = usize::from(v.saturating_sub(non_tail));
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
async fn route_engine(
    app: &mut App,
    msg: EngineMsg,
    engine_tx: &tokio::sync::mpsc::UnboundedSender<UiCommand>,
    current_cancel: &mut Option<locode_core::CancellationToken>,
    approvals: &mut PendingApprovals,
) {
    match msg {
        EngineMsg::RunStarted {
            cancel,
            input_queue,
        } => {
            *current_cancel = Some(cancel.clone());
            dispatch_engine(
                app,
                EngineMsg::RunStarted {
                    cancel,
                    input_queue,
                },
                engine_tx,
                current_cancel.as_ref(),
                approvals,
            )
            .await;
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
            )
            .await;
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
            run_reducer(app, Msg::Approval(view), &mut io).await;
        }
        other => dispatch_engine(app, other, engine_tx, current_cancel.as_ref(), approvals).await,
    }
}

async fn dispatch_engine(
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
    run_reducer(app, Msg::Engine(Box::new(msg)), &mut io).await;
}

/// Run the reducer and execute the returned commands (all IO lives here).
async fn run_reducer(app: &mut App, msg: Msg, io: &mut LoopIo<'_>) {
    debug_log(&format!("msg: {msg:?}"));
    // A worklist rather than a `for`: running a command produces further commands, and
    // the reducer cannot run one itself (`execute` is async and a skill-backed command
    // reads its file from disk).
    let mut work: std::collections::VecDeque<Cmd> = app.update(msg, Instant::now()).into();
    while let Some(cmd) = work.pop_front() {
        match cmd {
            Cmd::RunCommand { line } => {
                let ctx = app.command_ctx();
                let result = crate::commands::execute(&app.registry, &ctx, &line).await;
                work.extend(app.apply_command_result(result));
            }
            Cmd::Quit => app.should_quit = true,
            Cmd::Submit(text) => {
                let _ = io.engine_tx.send(UiCommand::Submit(text));
            }
            Cmd::NewSession => {
                let _ = io.engine_tx.send(UiCommand::NewSession);
            }
            Cmd::SetModel(model) => {
                let _ = io.engine_tx.send(UiCommand::SetModel(model));
            }
            Cmd::SetEffort(effort) => {
                let _ = io.engine_tx.send(UiCommand::SetEffort(effort));
            }
            Cmd::AddDir(dir) => {
                let _ = io.engine_tx.send(UiCommand::AddDir(dir));
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
    use super::{fold_streaming, paint};
    use crate::app::App;
    use crate::frame_terminal::FrameTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::text::Line;

    #[test]
    fn fold_streaming_commits_completed_blocks_and_finalizes_remainder() {
        let t = FrameTerminal::new(TestBackend::new(40, 20)).unwrap();
        let mut app = App::new();
        let mut tail: Vec<Line<'static>> = Vec::new();

        // Two completed blocks + an in-progress third (no trailing blank line).
        app.streaming = Some("First block.\n\nSecond block.\n\nThird in prog".into());
        fold_streaming(&t, &mut app, &mut tail).unwrap();
        // The two completed blocks committed to the tail; only the in-progress
        // block remains live.
        assert_eq!(app.streaming.as_deref(), Some("Third in prog"));
        assert!(app.streaming_committed_any, "a block committed");
        let committed = tail
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(committed.contains("First block."), "{committed}");
        assert!(committed.contains("Second block."), "{committed}");
        assert!(
            !committed.contains("Third in prog"),
            "in-progress not committed"
        );
        // Exactly one assistant bullet across the committed blocks (one message).
        assert_eq!(
            committed.matches('●').count(),
            1,
            "single bullet: {committed}"
        );

        // Finalize: the remaining block commits, streaming state clears.
        app.streaming_finalize = true;
        fold_streaming(&t, &mut app, &mut tail).unwrap();
        assert_eq!(app.streaming, None);
        assert!(!app.streaming_committed_any);
        assert!(!app.streaming_finalize);
        let all = tail
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all.contains("Third in prog"),
            "final block committed: {all}"
        );
        assert_eq!(all.matches('●').count(), 1, "still one bullet total");
    }

    #[test]
    fn finalize_commits_the_last_block_before_the_run_summary() {
        // Regression: the final assistant `Message` (finalize signal) and
        // `RunFinished` (which pushes the `completed · … · Ns` summary to the
        // outbox) drain in the SAME batch. The loop must fold the streaming
        // finalize into the tail BEFORE flushing the outbox, or the summary
        // interleaves into the middle of the reply.
        use crate::ui::blocks::Block;
        let t = FrameTerminal::new(TestBackend::new(60, 20)).unwrap();
        let mut app = App::new();
        let mut tail: Vec<Line<'static>> = Vec::new();

        // A streaming turn whose last in-progress block is pending finalize, and
        // the run-summary already sitting in the outbox (same-batch order).
        app.streaming = Some("The final paragraph of the reply.".into());
        app.streaming_committed_any = true;
        app.streaming_finalize = true;
        app.outbox
            .push(Block::Notice("completed · 13 turns · 82s".into()));

        // Mirror the loop's top-of-iteration order exactly.
        super::fold_streaming(&t, &mut app, &mut tail).unwrap();
        super::flush_outbox(&t, &mut app, &mut tail).unwrap();

        let joined = tail
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let block_at = joined
            .find("The final paragraph")
            .expect("final block committed");
        let summary_at = joined.find("completed ·").expect("summary committed");
        assert!(
            block_at < summary_at,
            "final assistant block must precede the run summary:\n{joined}"
        );
    }

    #[test]
    fn fold_streaming_never_commits_an_open_fence() {
        let t = FrameTerminal::new(TestBackend::new(40, 20)).unwrap();
        let mut app = App::new();
        let mut tail: Vec<Line<'static>> = Vec::new();
        // A paragraph, then an OPEN code fence with a blank line inside it.
        app.streaming = Some("intro\n\n```rust\nlet a = 1;\n\nlet b = 2;".into());
        fold_streaming(&t, &mut app, &mut tail).unwrap();
        // Only the paragraph commits; the open fence stays live (uncommitted).
        assert_eq!(
            app.streaming.as_deref(),
            Some("```rust\nlet a = 1;\n\nlet b = 2;")
        );
        let committed = tail
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(committed.contains("intro"), "{committed}");
        assert!(!committed.contains("let a = 1"), "open fence not committed");
    }

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
        // 30 wide, 9 tall. Empty composer = 3 rows + 2 footer = non_tail 5, so
        // tail_cap = 4 transcript rows fit.
        let mut t = FrameTerminal::new(TestBackend::new(30, 9)).unwrap();
        let mut app = App::new();
        let mut committed = false;

        // A single burst of 10 transcript lines (taller than the 4-row cap and
        // the 8-row screen) — the jump-from-not-full case.
        let mut tail: Vec<Line<'static>> =
            (0..10).map(|i| Line::from(format!("L{i:02}"))).collect();
        paint(&mut t, &mut app, &mut tail, &mut committed).unwrap();

        let r = rows(&t);
        // Composer prompt sits on the last rows (bottom-pinned) — above the
        // 2-row footer, so within the last 4 rows.
        assert!(
            r.iter().rev().take(4).any(|l| l.contains('❯')),
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

    /// Growing the composer hides recent tail lines (kept in memory, NOT
    /// committed); shrinking it back reveals them again — the transcript "comes
    /// back down" with no gap and no extra scrollback commits (the common,
    /// screen-filling case the earlier attempt got wrong).
    #[test]
    fn shrinking_the_composer_refills_the_transcript_from_memory() {
        let mut t = FrameTerminal::new(TestBackend::new(30, 13)).unwrap();
        let mut app = App::new();
        let mut committed = false;

        // 8 transcript lines; with an empty composer (3 rows) + 2-row footer,
        // tail_cap is 8 — they all fit, nothing committed.
        let mut tail: Vec<Line<'static>> = (0..8).map(|i| Line::from(format!("T{i:02}"))).collect();
        paint(&mut t, &mut app, &mut tail, &mut committed).unwrap();
        assert!(rows(&t).iter().any(|l| l == "T00"), "T00 shown initially");
        let sb0 = scrollback(&t).iter().filter(|l| !l.is_empty()).count();

        // Grow the composer to 4 lines → fewer transcript rows fit; the oldest
        // shown lines are hidden (in memory, not scrollback).
        app.composer.insert_text("a\nb\nc\nd");
        paint(&mut t, &mut app, &mut tail, &mut committed).unwrap();
        assert!(
            !rows(&t).iter().any(|l| l == "T00"),
            "T00 hidden while the composer is tall: {:?}",
            rows(&t)
        );
        assert_eq!(
            scrollback(&t).iter().filter(|l| !l.is_empty()).count(),
            sb0,
            "growing the composer must NOT commit to scrollback"
        );

        // Shrink back to empty → the hidden lines come back from memory.
        app.composer.clear();
        paint(&mut t, &mut app, &mut tail, &mut committed).unwrap();
        assert!(
            rows(&t).iter().any(|l| l == "T00"),
            "T00 refilled after the composer shrinks: {:?}",
            rows(&t)
        );
        assert_eq!(
            scrollback(&t).iter().filter(|l| !l.is_empty()).count(),
            sb0,
            "the whole grow/shrink cycle never touched scrollback"
        );
    }
}
