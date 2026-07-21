//! Terminal lifecycle: init, the teardown sequence defined ONCE, panic hook,
//! and the input-reader thread (SPEC-TUI robustness floor).
//!
//! The teardown byte-order lives in exactly one function, shared by clean
//! exit, the error path, the panic hook, and the signal path — grok's rule
//! (`xai-grok-pager/src/app/mod.rs:1185-1245`); the panic hook chains the
//! previous hook after restoring — codex's rule (`tui/src/tui.rs:504-510`).

use std::io::{Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::Event as CrosstermEvent;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::{TerminalOptions, Viewport};

/// Rows the inline live region occupies (status + composer + footer; the
/// approval overlay swaps in). Fixed in v1 — stock ratatui's `Inline`
/// viewport has a fixed height; dynamic growth is a named extension.
pub const LIVE_REGION_ROWS: u16 = 10;

/// Set once the teardown sequence has run; makes restore idempotent so the
/// panic hook, signal path, and normal exit can all call it safely.
static RESTORED: AtomicBool = AtomicBool::new(false);

/// The one place diagnostics go (stderr; stdout belongs to the TUI frames).
#[allow(clippy::print_stderr)]
pub fn error_line(message: &str) {
    eprintln!("error: {message}");
}

/// Enter TUI modes and build the inline-viewport terminal.
///
/// # Errors
/// Propagates terminal setup failures; on partial failure the teardown
/// sequence is emitted so the terminal is never left raw.
pub fn init() -> std::io::Result<Terminal<CrosstermBackend<Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    if let Err(e) = crossterm::execute!(stdout, crossterm::event::EnableBracketedPaste) {
        restore();
        return Err(e);
    }
    RESTORED.store(false, Ordering::SeqCst);
    match Terminal::with_options(
        CrosstermBackend::new(std::io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(LIVE_REGION_ROWS),
        },
    ) {
        Ok(terminal) => Ok(terminal),
        Err(e) => {
            restore();
            Err(e)
        }
    }
}

/// The teardown sequence, defined once (idempotent): bracketed paste off,
/// raw mode off, cursor shown, and a trailing newline so the shell prompt
/// lands below the parked viewport. Best-effort throughout — this must
/// succeed as far as possible even mid-panic.
pub fn restore() {
    if RESTORED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut stdout = std::io::stdout();
    let _ = crossterm::execute!(
        stdout,
        crossterm::event::DisableBracketedPaste,
        crossterm::cursor::Show
    );
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = writeln!(stdout);
    let _ = stdout.flush();
}

/// Install a panic hook that restores the terminal, then chains the previous
/// hook (so the panic message prints onto a sane terminal).
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
}

/// Spawn the dedicated input-reader thread: `poll(100ms)+read()` forwarding
/// into an mpsc — never crossterm's `EventStream` inside `select!` (the
/// waker-strand bug grok documents at `event_loop.rs:1084-1092`). The thread
/// exits within one poll cycle of the receiver dropping.
#[must_use]
pub fn spawn_input_reader() -> tokio::sync::mpsc::UnboundedReceiver<CrosstermEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        const POLL_TIMEOUT: Duration = Duration::from_millis(100);
        // Tolerate transient parse errors (VTE/SSH garbage) up to a cap,
        // grok's rule (`event_loop.rs:1141-1154`).
        let mut consecutive_errors: u32 = 0;
        loop {
            if tx.is_closed() {
                break;
            }
            match crossterm::event::poll(POLL_TIMEOUT) {
                Ok(false) => {}
                Ok(true) => {
                    if let Ok(event) = crossterm::event::read() {
                        consecutive_errors = 0;
                        if tx.send(event).is_err() {
                            break;
                        }
                    } else {
                        consecutive_errors += 1;
                        if consecutive_errors > 50 {
                            break;
                        }
                    }
                }
                Err(_) => {
                    consecutive_errors += 1;
                    if consecutive_errors > 50 {
                        break;
                    }
                }
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `restore()` is idempotent: the second call is a no-op (the guard flag
    /// flips exactly once) — required because exit, panic hook, and the
    /// signal path may all invoke it.
    #[test]
    fn restore_is_idempotent() {
        RESTORED.store(false, Ordering::SeqCst);
        restore();
        assert!(RESTORED.load(Ordering::SeqCst));
        restore(); // second call must not panic or double-run
        assert!(RESTORED.load(Ordering::SeqCst));
    }
}
