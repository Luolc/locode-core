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

use crossterm::event::{
    Event as CrosstermEvent, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::{TerminalOptions, Viewport};

/// Initial inline live-region height at startup (a 1-line composer: top rule +
/// editor + bottom rule + footer). The region is then **dynamic** — it grows and
/// shrinks with the composer via [`resize_live_region`] (ADR-0019 amendment
/// 2026-07-22), up to [`max_live_rows`].
pub const INITIAL_LIVE_ROWS: u16 = 4;

/// Floor for the live region (a framed 1-line composer + footer).
pub const MIN_LIVE_ROWS: u16 = 4;

/// Cap the live region at ~half the terminal so the transcript stays visible
/// (Claude Code's `maxHeight="50%"`). Never below [`MIN_LIVE_ROWS`].
#[must_use]
pub fn max_live_rows(terminal_height: u16) -> u16 {
    (terminal_height / 2).max(MIN_LIVE_ROWS)
}

/// Set once the teardown sequence has run; makes restore idempotent so the
/// panic hook, signal path, and normal exit can all call it safely.
static RESTORED: AtomicBool = AtomicBool::new(false);

/// Whether the kitty keyboard enhancement was pushed (so `restore` only pops
/// what it pushed — popping an empty stack confuses some terminals).
static KITTY_PUSHED: AtomicBool = AtomicBool::new(false);

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
    // Kitty keyboard protocol (DISAMBIGUATE_ESCAPE_CODES only): makes modifiers
    // on keys like Enter reportable, so **Shift+Enter** is distinguishable from
    // a bare Enter (iTerm2/Ghostty/kitty support this; plain xterm does not).
    // We deliberately do NOT set REPORT_EVENT_TYPES — that would add Release
    // events and double every keypress, forcing Press-filtering everywhere.
    // Best-effort + capability-gated: unsupported terminals simply keep the old
    // behavior (Shift+Enter == Enter), never an error.
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && crossterm::execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KITTY_PUSHED.store(true, Ordering::SeqCst);
    }
    RESTORED.store(false, Ordering::SeqCst);
    match Terminal::with_options(
        CrosstermBackend::new(std::io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(INITIAL_LIVE_ROWS),
        },
    ) {
        Ok(terminal) => Ok(terminal),
        Err(e) => {
            restore();
            Err(e)
        }
    }
}

/// Resize the inline live region to `target` rows, keeping it bottom-anchored
/// (ADR-0019 amendment). Stock ratatui can't change an inline viewport's height,
/// so we recreate the terminal at the new height (iteration 1 — see the ADR):
///
/// - **Grow**: park the cursor at the transcript's end and recreate; ratatui's
///   `compute_inline_size` appends lines (scrolls the transcript up) so the
///   taller region is anchored to the bottom.
/// - **Shrink**: clear the old region, then anchor the smaller region to the
///   bottom (a transient blank gap above is possible — no worse than the former
///   fixed-height gap).
///
/// Returns the (possibly recreated) terminal. A no-op when the height matches.
///
/// # Errors
/// Propagates terminal write/setup failures.
pub fn resize_live_region(
    mut terminal: Terminal<CrosstermBackend<Stdout>>,
    target: u16,
) -> std::io::Result<Terminal<CrosstermBackend<Stdout>>> {
    use crossterm::cursor::MoveTo;
    use crossterm::terminal::{Clear, ClearType};

    let full = terminal.size()?;
    let target = target.clamp(MIN_LIVE_ROWS, max_live_rows(full.height));
    let current = terminal.get_frame().area();
    if current.height == target {
        return Ok(terminal);
    }

    let mut stdout = std::io::stdout();
    if target > current.height {
        // Grow: anchor at the transcript's end; recreation scrolls it up.
        crossterm::execute!(
            stdout,
            MoveTo(0, current.y),
            Clear(ClearType::FromCursorDown)
        )?;
    } else {
        // Shrink: clear the old region, then anchor the smaller region at the
        // bottom of the screen.
        let top = full.height.saturating_sub(target);
        crossterm::execute!(
            stdout,
            MoveTo(0, current.y),
            Clear(ClearType::FromCursorDown),
            MoveTo(0, top)
        )?;
    }
    drop(terminal);
    Terminal::with_options(
        CrosstermBackend::new(std::io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(target),
        },
    )
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
    // Pop the kitty enhancement first (only if we pushed it), then the rest.
    if KITTY_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = crossterm::execute!(stdout, PopKeyboardEnhancementFlags);
    }
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
