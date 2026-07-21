//! App state and the sans-IO reducer: `Msg → update(&mut App, now) → Vec<Cmd>`
//! (grok's dispatch discipline, `src/app/actions.rs:1-8` — "dispatch stays
//! sans-IO"). All interaction semantics live here so they are table-testable
//! without a terminal.

use std::time::{Duration, Instant};

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};

use crate::ui::composer::Composer;

/// Double-press window for Esc-clear and the Ctrl+C quit arm (grok uses
/// 800 ms for double-Esc; codex's quit arm is the same order of magnitude).
pub const ARM_WINDOW: Duration = Duration::from_millis(800);

/// Everything the reducer consumes.
#[derive(Debug)]
pub enum Msg {
    /// A terminal event from the input-reader thread.
    Input(CrosstermEvent),
    /// SIGINT/SIGTERM arrived (graceful quit path).
    SignalQuit,
}

/// Everything the reducer asks the loop to do (the loop owns all IO).
#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Submit the composed prompt (slice 1: recorded only; slice 2 wires the
    /// engine).
    Submit(String),
    /// Tear down and exit.
    Quit,
}

/// A transient one-line hint shown in the footer (quit arming, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// "press ctrl+c again to quit"
    QuitArmed,
    /// "press esc again to clear"
    ClearArmed,
}

/// The whole UI state (one struct owned by the event loop — the ratatui
/// answer to Claude Code's ref-mirror epidemic; study §5).
pub struct App {
    /// The multiline prompt editor.
    pub composer: Composer,
    /// Set when the loop should exit after the current iteration.
    pub should_quit: bool,
    /// Redraw needed.
    pub dirty: bool,
    /// Ctrl+C quit arm: armed until this instant.
    quit_armed_until: Option<Instant>,
    /// Esc clear-draft arm: armed until this instant.
    esc_armed_until: Option<Instant>,
    /// The active footer hint, if any.
    pub hint: Option<Hint>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Fresh state with an empty composer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            composer: Composer::new(),
            should_quit: false,
            dirty: true,
            quit_armed_until: None,
            esc_armed_until: None,
            hint: None,
        }
    }

    /// The reducer. Pure over (`self`, `msg`, `now`): no IO, no clock reads —
    /// the loop injects `now` so tests control time.
    pub fn update(&mut self, msg: Msg, now: Instant) -> Vec<Cmd> {
        self.dirty = true;
        match msg {
            Msg::SignalQuit => {
                self.should_quit = true;
                vec![Cmd::Quit]
            }
            Msg::Input(CrosstermEvent::Key(key)) => self.on_key(key, now),
            Msg::Input(CrosstermEvent::Paste(text)) => {
                // Normalize CR pastes (Windows/legacy terminals) to LF.
                self.composer
                    .insert_text(&text.replace("\r\n", "\n").replace('\r', "\n"));
                vec![]
            }
            Msg::Input(CrosstermEvent::Resize(..)) => vec![], // redraw via dirty
            Msg::Input(_) => {
                self.dirty = false; // focus/mouse events: nothing to do
                vec![]
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            // Ctrl+C: clear a non-empty draft first (grok's two-step,
            // `agent_view/mod.rs:22-26`), else arm-then-quit (codex's
            // arm/confirm, `interaction.rs:360-414`).
            (KeyCode::Char('c'), true) => {
                if !self.composer.is_empty() {
                    self.composer.clear();
                    self.disarm();
                    return vec![];
                }
                if Self::is_armed(self.quit_armed_until, now) {
                    self.should_quit = true;
                    return vec![Cmd::Quit];
                }
                self.quit_armed_until = Some(now + ARM_WINDOW);
                self.hint = Some(Hint::QuitArmed);
                vec![]
            }
            // Ctrl+D: quit only on an empty composer (codex,
            // `interaction.rs:420-445`); otherwise ignored in v1.
            (KeyCode::Char('d'), true) => {
                if self.composer.is_empty() {
                    self.should_quit = true;
                    vec![Cmd::Quit]
                } else {
                    vec![]
                }
            }
            // Esc at idle: double-press clears a non-empty draft (grok's
            // 800 ms TTL, `agent_view/prompt.rs:751-830`).
            (KeyCode::Esc, _) => {
                if self.composer.is_empty() {
                    self.disarm();
                    return vec![];
                }
                if Self::is_armed(self.esc_armed_until, now) {
                    self.composer.clear();
                    self.disarm();
                    return vec![];
                }
                self.esc_armed_until = Some(now + ARM_WINDOW);
                self.hint = Some(Hint::ClearArmed);
                vec![]
            }
            // Enter submits; Alt+Enter inserts a newline (works without the
            // kitty protocol — deferred).
            (KeyCode::Enter, _) => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.insert_newline();
                    return vec![];
                }
                let text = self.composer.take_text();
                self.disarm();
                if text.trim().is_empty() {
                    return vec![];
                }
                vec![Cmd::Submit(text)]
            }
            // Everything else goes to the editor; any keypress disarms the
            // pending quit/clear arms.
            _ => {
                self.disarm();
                self.composer.input(key);
                vec![]
            }
        }
    }

    fn is_armed(armed_until: Option<Instant>, now: Instant) -> bool {
        armed_until.is_some_and(|until| now <= until)
    }

    fn disarm(&mut self) {
        self.quit_armed_until = None;
        self.esc_armed_until = None;
        self.hint = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Msg {
        Msg::Input(CrosstermEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }
    fn ctrl(c: char) -> Msg {
        Msg::Input(CrosstermEvent::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        )))
    }
    fn alt_enter() -> Msg {
        Msg::Input(CrosstermEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::ALT,
        )))
    }
    fn type_str(app: &mut App, s: &str, now: Instant) {
        for ch in s.chars() {
            let _ = app.update(key(KeyCode::Char(ch)), now);
        }
    }

    #[test]
    fn ctrl_c_clears_draft_then_arms_then_quits() {
        let mut app = App::new();
        let t0 = Instant::now();
        type_str(&mut app, "draft", t0);

        // Draft present: first Ctrl+C clears it, nothing armed.
        assert_eq!(app.update(ctrl('c'), t0), vec![]);
        assert!(app.composer.is_empty());
        assert_eq!(app.hint, None);

        // Empty: first press arms, second within the window quits.
        assert_eq!(app.update(ctrl('c'), t0), vec![]);
        assert_eq!(app.hint, Some(Hint::QuitArmed));
        assert_eq!(
            app.update(ctrl('c'), t0 + Duration::from_millis(300)),
            vec![Cmd::Quit]
        );
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_arm_expires() {
        let mut app = App::new();
        let t0 = Instant::now();
        assert_eq!(app.update(ctrl('c'), t0), vec![]);
        // Past the window: re-arms instead of quitting.
        assert_eq!(
            app.update(ctrl('c'), t0 + ARM_WINDOW + Duration::from_millis(1)),
            vec![]
        );
        assert!(!app.should_quit);
        assert_eq!(app.hint, Some(Hint::QuitArmed));
    }

    #[test]
    fn ctrl_d_quits_only_on_empty_composer() {
        let mut app = App::new();
        let t0 = Instant::now();
        type_str(&mut app, "x", t0);
        assert_eq!(app.update(ctrl('d'), t0), vec![]);
        assert!(!app.should_quit);

        app.composer.clear();
        assert_eq!(app.update(ctrl('d'), t0), vec![Cmd::Quit]);
        assert!(app.should_quit);
    }

    #[test]
    fn esc_double_press_clears_draft_within_window() {
        let mut app = App::new();
        let t0 = Instant::now();
        type_str(&mut app, "keep me", t0);

        assert_eq!(app.update(key(KeyCode::Esc), t0), vec![]);
        assert!(!app.composer.is_empty(), "single Esc must not clear");
        assert_eq!(app.hint, Some(Hint::ClearArmed));

        assert_eq!(
            app.update(key(KeyCode::Esc), t0 + Duration::from_millis(500)),
            vec![]
        );
        assert!(app.composer.is_empty(), "double Esc clears");
        assert_eq!(app.hint, None);
    }

    #[test]
    fn esc_arm_expires_and_typing_disarms() {
        let mut app = App::new();
        let t0 = Instant::now();
        type_str(&mut app, "draft", t0);
        let _ = app.update(key(KeyCode::Esc), t0);

        // Expired second press only re-arms.
        let late = t0 + ARM_WINDOW + Duration::from_millis(1);
        let _ = app.update(key(KeyCode::Esc), late);
        assert!(!app.composer.is_empty());

        // Typing disarms: Esc-Esc with a keypress in between never clears.
        let _ = app.update(key(KeyCode::Char('!')), late);
        assert_eq!(app.hint, None);
    }

    #[test]
    fn enter_submits_and_alt_enter_inserts_newline() {
        let mut app = App::new();
        let t0 = Instant::now();
        type_str(&mut app, "line one", t0);
        let _ = app.update(alt_enter(), t0);
        type_str(&mut app, "line two", t0);

        let cmds = app.update(key(KeyCode::Enter), t0);
        assert_eq!(cmds, vec![Cmd::Submit("line one\nline two".into())]);
        assert!(app.composer.is_empty(), "composer cleared on submit");

        // Empty/whitespace submit is a no-op.
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
    }

    #[test]
    fn paste_normalizes_carriage_returns() {
        let mut app = App::new();
        let t0 = Instant::now();
        let _ = app.update(Msg::Input(CrosstermEvent::Paste("a\r\nb\rc".into())), t0);
        let cmds = app.update(key(KeyCode::Enter), t0);
        assert_eq!(cmds, vec![Cmd::Submit("a\nb\nc".into())]);
    }

    #[test]
    fn signal_quit_is_immediate() {
        let mut app = App::new();
        assert_eq!(app.update(Msg::SignalQuit, Instant::now()), vec![Cmd::Quit]);
        assert!(app.should_quit);
    }
}
