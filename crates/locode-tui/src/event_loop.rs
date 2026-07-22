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

    let exit_code = loop {
        if app.should_quit {
            break ExitCode::SUCCESS;
        }

        // Print finalized blocks once into native scrollback, then paint the
        // live region (rate-capped; deferred paint instead of spinning).
        if !app.outbox.is_empty() {
            flush_outbox(&mut terminal, &mut app)?;
        }
        if app.dirty {
            let now = Instant::now();
            if now.duration_since(last_draw) >= MIN_DRAW_INTERVAL {
                // Grow/shrink the inline live region to fit the composer before
                // painting (ADR-0019 amendment — dynamic height).
                let desired = ui::desired_live_rows(&app, terminal.size()?.width);
                terminal = term::resize_live_region(terminal, desired)?;
                terminal.draw(|frame| ui::draw(frame, &app))?;
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
                    terminal.autoresize()?;
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

/// Render queued blocks once above the viewport (print-once transcript).
fn flush_outbox(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> std::io::Result<()> {
    let width = terminal.size()?.width;
    let lines: Vec<Line<'static>> = app
        .outbox
        .drain(..)
        .flat_map(|block| block.render(width))
        .collect();
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    if height == 0 {
        return Ok(());
    }
    terminal.insert_before(height, |buf| {
        use ratatui::widgets::Widget;
        ratatui::widgets::Paragraph::new(lines).render(buf.area, buf);
    })?;
    Ok(())
}

/// The `LOCODE_TUI_DEBUG_LOG` path, resolved once at first use.
static DEBUG_LOG_PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Append one line to the debug log when `LOCODE_TUI_DEBUG_LOG` is set — the
/// `insert_before` transcript leaves nothing greppable in a captured pty, so
/// this is the reusable smoke/bug-report instrumentation. No-op otherwise.
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
