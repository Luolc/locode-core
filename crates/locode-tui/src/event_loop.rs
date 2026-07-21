//! The main loop: one biased `select!` over input, signals, and timers;
//! draw-on-change with a ~16 ms cap and zero idle wakeups (study §6.2/6.3).

use std::process::ExitCode;
use std::time::{Duration, Instant};

use crate::app::{App, Cmd, Msg};
use crate::cli::Cli;
use crate::{term, ui};
use locode_core::ProviderRegistry;

/// Minimum interval between paints while events stream in.
const MIN_DRAW_INTERVAL: Duration = Duration::from_millis(16);
/// Resize storms collapse into one relayout (grok: 16 ms debounce).
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(16);

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

/// Run the app to completion. The `registry` is unused until slice 2 wires
/// the engine task; it is threaded now so the public entry is stable.
pub async fn run(cli: Cli, _registry: &ProviderRegistry) -> Result<ExitCode, RunError> {
    let _ = &cli; // engine wiring lands in slice 2

    term::install_panic_hook();
    let mut terminal = term::init()?;
    let mut input_rx = term::spawn_input_reader();
    let mut signal_rx = spawn_signal_task();

    let mut app = App::new();
    let mut last_draw = Instant::now()
        .checked_sub(MIN_DRAW_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut deferred_draw: Option<Instant> = None;
    let mut resize_at: Option<Instant> = None;

    let exit_code = loop {
        if app.should_quit {
            break ExitCode::SUCCESS;
        }

        // Draw when dirty, rate-capped; when capped, schedule one deferred
        // paint instead of spinning (grok's deferred-draw arm).
        if app.dirty {
            let now = Instant::now();
            if now.duration_since(last_draw) >= MIN_DRAW_INTERVAL {
                terminal.draw(|frame| ui::draw(frame, &app))?;
                app.dirty = false;
                last_draw = now;
                deferred_draw = None;
            } else if deferred_draw.is_none() {
                deferred_draw = Some(last_draw + MIN_DRAW_INTERVAL);
            }
        }

        let timer = next_deadline(deferred_draw, resize_at);
        tokio::select! {
            biased;
            _ = signal_rx.recv() => {
                let _ = dispatch(&mut app, Msg::SignalQuit);
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
                let _ = dispatch(&mut app, Msg::Input(event));
            }
            () = sleep_until(timer), if timer.is_some() => {
                let now = Instant::now();
                if resize_at.is_some_and(|at| now >= at) {
                    resize_at = None;
                    terminal.autoresize()?;
                    app.dirty = true;
                }
                // A due deferred draw is handled by the top-of-loop paint.
            }
        }
    };

    term::restore();
    Ok(exit_code)
}

/// Run the reducer and execute the returned commands (all IO lives here).
fn dispatch(app: &mut App, msg: Msg) -> Vec<Cmd> {
    let cmds = app.update(msg, Instant::now());
    for cmd in &cmds {
        match cmd {
            Cmd::Quit => app.should_quit = true,
            // Slice 2 forwards this to the engine task; until then a submit
            // simply clears the composer (recorded by the reducer).
            Cmd::Submit(_) => {}
        }
    }
    cmds
}

fn next_deadline(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
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
