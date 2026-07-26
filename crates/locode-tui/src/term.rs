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
    Event as CrosstermEvent, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::backend::CrosstermBackend;

use crate::frame_terminal::FrameTerminal;

/// The vendored full-screen terminal the interactive app renders into
/// (ADR-0022): a bottom-anchored relative frame diffed one buffer per paint.
pub type Term = FrameTerminal<CrosstermBackend<Stdout>>;

/// Minimum composer height: one text row plus the two framing rules.
pub const MIN_COMPOSER_ROWS: u16 = 3;

/// The composer's height cap — ~50% of the screen (Claude Code's dynamic
/// composer), never below [`MIN_COMPOSER_ROWS`]. The caller clamps
/// `composer::desired_height` to this.
#[must_use]
pub fn max_composer_rows(screen_h: u16) -> u16 {
    (screen_h / 2).max(MIN_COMPOSER_ROWS)
}

/// Set once the teardown sequence has run; makes restore idempotent so the
/// panic hook, signal path, and normal exit can all call it safely.
static RESTORED: AtomicBool = AtomicBool::new(false);

/// Turn the kitty keyboard enhancement off, in wire order, unconditionally:
///
/// - `CSI < 1 u` pops the entry [`init`] pushed. Popping an *empty* stack is a
///   no-op per the kitty protocol, so this is safe even when we never pushed —
///   which is why it is not gated on a "did we push?" flag any more.
/// - `CSI = 0 ; 1 u` then clears every flag on whatever entry is left. This is
///   the healing half: a pop only balances *our own* push, so a stack entry
///   leaked by anyone else (an earlier locode killed with SIGKILL before its
///   teardown ran, a full-screen editor a tool spawned and killed) survives it
///   and leaves the shell in CSI-u mode — where Ctrl+C arrives as the literal
///   `ESC [ 9 9 ; 5 u` instead of interrupting, and Esc as `ESC [ 2 7 u`.
///
/// Order is load-bearing: zeroing *before* the pop would zero the entry we are
/// about to discard and leave the leaked one active. Terminals without the
/// protocol ignore both sequences (unknown CSI).
///
/// Claude Code's ink ships the same unconditional pop on exit and documents the
/// same failure — an unbalanced stack leaving "the shell in CSI u mode where
/// Ctrl+C/Ctrl+D leak as escape sequences" (`src/ink/ink.tsx:883-887,1492`,
/// `src/ink/termio/csi.ts:303-307`).
const KEYBOARD_ENHANCEMENT_OFF: &str = "\x1b[<1u\x1b[=0;1u";

/// The one place diagnostics go (stderr; stdout belongs to the TUI frames).
#[allow(clippy::print_stderr)]
pub fn error_line(message: &str) {
    eprintln!("error: {message}");
}

/// Enter TUI modes and build the vendored full-screen terminal (ADR-0022).
///
/// Returns the terminal **and** a [`RestoreGuard`]: hold the guard for as long
/// as the terminal is in use and the teardown runs on every exit from that
/// scope, including the `?` paths that would otherwise return past it.
///
/// # Errors
/// Propagates terminal setup failures; on partial failure the teardown
/// sequence is emitted so the terminal is never left raw.
pub fn init() -> std::io::Result<(Term, RestoreGuard)> {
    crossterm::terminal::enable_raw_mode()?;
    RESTORED.store(false, Ordering::SeqCst);
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
    // behavior (Shift+Enter == Enter), never an error. The *teardown* is not
    // gated the same way — see [`KEYBOARD_ENHANCEMENT_OFF`].
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        let _ = crossterm::execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    // Clear the visible screen AND purge scrollback, then home the cursor
    // (Claude Code / Grok Build's start; grok's `resize_purge_rerender`
    // sequence). The session begins on a *fresh* scrollback, so its transcript —
    // which commits scrolled-off rows into native scrollback — never overlaps or
    // scrolls into whatever the terminal held before locode started. Like the
    // `clear` command, this drops the pre-session scrollback. Best-effort: a
    // terminal that ignores `ESC[3J` simply keeps its scrollback.
    let _ = crossterm::execute!(
        stdout,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::Purge),
        crossterm::cursor::MoveTo(0, 0),
    );
    match FrameTerminal::new(CrosstermBackend::new(std::io::stdout())) {
        Ok(terminal) => Ok((terminal, RestoreGuard)),
        Err(e) => {
            restore();
            Err(e)
        }
    }
}

/// Runs [`restore`] when it goes out of scope — the teardown the `?` paths
/// used to skip. Handed out by [`init`]; keep it alive for the whole session.
#[derive(Debug)]
pub struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// The teardown sequence, defined once (idempotent): keyboard enhancement off,
/// bracketed paste off, raw mode off, cursor shown, and a trailing newline so
/// the shell prompt lands below the parked viewport. Best-effort throughout —
/// this must succeed as far as possible even mid-panic.
pub fn restore() {
    if RESTORED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut stdout = std::io::stdout();
    // Keyboard enhancement off first — it is what the *shell* inherits, and a
    // shell left in CSI-u mode is unusable (see `KEYBOARD_ENHANCEMENT_OFF`).
    let _ = write!(stdout, "{KEYBOARD_ENHANCEMENT_OFF}");
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

    /// The teardown pops the stack entry we pushed **and then** clears the
    /// flags on whatever is left. Both halves, in that order: the pop alone
    /// balances only our own push, so an entry leaked by another program (or
    /// by a locode that died before its teardown) would keep the shell in
    /// CSI-u mode — Ctrl+C arriving as `ESC [ 9 9 ; 5 u` instead of
    /// interrupting.
    #[test]
    fn teardown_pops_then_clears_the_keyboard_flags() {
        let pop = KEYBOARD_ENHANCEMENT_OFF
            .find("\x1b[<1u")
            .expect("teardown pops the entry init pushed");
        let clear = KEYBOARD_ENHANCEMENT_OFF
            .find("\x1b[=0;1u")
            .expect("teardown clears the flags on the entry left behind");
        assert!(
            pop < clear,
            "clearing before popping zeroes the wrong entry"
        );
    }

    /// The hand-written pop stays byte-identical to crossterm's command, so a
    /// change on their side can't leave us emitting a stale sequence.
    #[test]
    fn the_pop_matches_crossterms_command() {
        let mut expected = String::new();
        crossterm::Command::write_ansi(
            &crossterm::event::PopKeyboardEnhancementFlags,
            &mut expected,
        )
        .expect("render the pop");
        assert!(
            KEYBOARD_ENHANCEMENT_OFF.starts_with(&expected),
            "teardown must start with crossterm's pop ({expected:?})"
        );
    }

    /// The guard exists so the `?` paths in the run loop can't return past the
    /// teardown; dropping it runs `restore` exactly like the clean exit does.
    #[test]
    fn dropping_the_guard_restores() {
        RESTORED.store(false, Ordering::SeqCst);
        drop(RestoreGuard);
        assert!(RESTORED.load(Ordering::SeqCst));
    }
}
