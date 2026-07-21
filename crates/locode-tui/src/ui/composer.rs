//! The multiline prompt editor — a thin wrapper over `tui-textarea` so the
//! widget choice stays local (SPEC-TUI: replacing it later is one module).

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use tui_textarea::TextArea;

/// Maximum rows the composer may occupy inside the live region.
const MAX_ROWS: u16 = 5;

/// The prompt editor.
pub struct Composer {
    textarea: TextArea<'static>,
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

impl Composer {
    /// An empty composer with the `❯ ` prompt prefix rendered via the
    /// textarea's line prefix styling.
    #[must_use]
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        textarea.set_placeholder_text("type a prompt…");
        textarea.set_placeholder_style(Style::default().add_modifier(Modifier::DIM));
        Self { textarea }
    }

    /// Feed one key event into the editor.
    pub fn input(&mut self, key: KeyEvent) {
        self.textarea.input(key);
    }

    /// Insert literal text at the cursor (paste path — already normalized).
    pub fn insert_text(&mut self, text: &str) {
        self.textarea.insert_str(text);
    }

    /// Insert a newline (Alt+Enter path).
    pub fn insert_newline(&mut self) {
        self.textarea.insert_newline();
    }

    /// Whether the editor holds no text at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.textarea
            .lines()
            .iter()
            .all(std::string::String::is_empty)
    }

    /// The current text, joined with newlines.
    #[must_use]
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Take the text out and reset the editor (submit path).
    pub fn take_text(&mut self) -> String {
        let text = self.text();
        self.clear();
        text
    }

    /// Reset to empty.
    pub fn clear(&mut self) {
        self.textarea = Self::new().textarea;
    }

    /// Rows needed at `width`: content lines clamped to `[1, MAX_ROWS]`
    /// (soft-wrap is deferred with the widget; long lines scroll).
    #[must_use]
    pub fn desired_height(&self, _width: u16) -> u16 {
        let lines = u16::try_from(self.textarea.lines().len()).unwrap_or(u16::MAX);
        lines.clamp(1, MAX_ROWS)
    }

    /// Render into `area`, with the `❯ ` gutter column.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        use ratatui::layout::{Constraint, Layout};
        use ratatui::text::Line;
        use ratatui::widgets::Paragraph;

        let [gutter, editor] =
            Layout::horizontal([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
        frame.render_widget(Paragraph::new(Line::from("❯ ")), gutter);
        frame.render_widget(&self.textarea, editor);
    }
}
