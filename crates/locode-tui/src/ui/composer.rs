//! The multiline prompt editor — a thin wrapper over `tui-textarea` so the
//! widget choice stays local (SPEC-TUI: replacing it later is one module).

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use tui_textarea::TextArea;

/// Hard ceiling on the composer's editor rows — a safety bound only. The real,
/// screen-relative cap (~50% of the screen, Claude Code's dynamic composer) is
/// applied by the caller via `term::max_composer_rows`.
const MAX_ROWS: u16 = 100;

/// Extra rows for the top and bottom framing rules.
const FRAME_ROWS: u16 = 2;

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

    /// Replace the contents with `text` (draft restore).
    pub fn set_text(&mut self, text: &str) {
        self.clear();
        self.textarea.insert_str(text);
    }

    /// Rows needed at `width`: content lines clamped to `[1, MAX_ROWS]`, plus
    /// the two framing rules (top + bottom). This is *dynamic* — it grows and
    /// shrinks with the draft; the screen-relative ceiling is applied by the
    /// caller (`term::max_composer_rows`). Soft-wrap is deferred with the
    /// widget; long lines scroll.
    #[must_use]
    pub fn desired_height(&self, _width: u16) -> u16 {
        let lines = u16::try_from(self.textarea.lines().len()).unwrap_or(u16::MAX);
        lines.clamp(1, MAX_ROWS) + FRAME_ROWS
    }

    /// Glue the caret to the composer's bottom line while the draft overflows
    /// `editor_height` (the caret-follows-bottom behavior of ADR-0022): when the
    /// content fits, scroll to the top; when the caret is on the last line,
    /// scroll to the bottom so it stays visible.
    pub fn sync_scroll(&mut self, editor_height: u16) {
        let h = usize::from(editor_height);
        let content = self.textarea.lines().len();
        if h == 0 {
            return;
        }
        // NB: `-i16::MAX`, not `i16::MIN` — tui-textarea negates the delta
        // internally and negating `i16::MIN` overflows (debug panic).
        if content <= h {
            self.textarea.scroll((-i16::MAX, 0)); // fits → top
        } else if self.textarea.cursor().0 + 1 >= content {
            self.textarea.scroll((i16::MAX, 0)); // caret at end → bottom
        }
    }

    /// Render into `area`: a dim rule, the `❯ ` gutter + editor, then a dim
    /// rule — the input's top/bottom frame (user choice, 2026-07-22). A 2-col
    /// left/right margin insets the rules and squeezes the input, and the
    /// `  ❯ ` gutter puts the input text at column 4 (aligning with the
    /// transcript's bulleted text and the status line).
    pub fn render(&self, frame: &mut crate::frame_terminal::Frame<'_>, area: Rect) {
        use ratatui::layout::{Constraint, Layout};
        use ratatui::style::Modifier;
        use ratatui::text::Line;
        use ratatui::widgets::Paragraph;

        let dim = Style::default().add_modifier(Modifier::DIM);
        let margin = 2usize;
        let rule_width = usize::from(area.width).saturating_sub(2 * margin);
        let rule = format!("{}{}", " ".repeat(margin), "─".repeat(rule_width));
        let [top, mid, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(Line::styled(rule.clone(), dim)), top);
        // gutter "  ❯ " (margin + prompt) · editor · right margin.
        let [gutter, editor, _right] = Layout::horizontal([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(mid);
        frame.render_widget(Paragraph::new(Line::from("  ❯ ")), gutter);
        frame.render_widget(&self.textarea, editor);
        frame.render_widget(Paragraph::new(Line::styled(rule, dim)), bottom);
    }
}

#[cfg(test)]
mod tests {
    use super::{Composer, FRAME_ROWS};

    /// `desired_height` tracks the draft: one row when empty/short, grows a row
    /// per content line, and shrinks back when the draft is replaced.
    #[test]
    fn desired_height_grows_then_shrinks_with_content() {
        let mut c = Composer::new();
        let base = c.desired_height(40);
        assert_eq!(
            base,
            1 + FRAME_ROWS,
            "empty composer is one text row + frame"
        );

        c.insert_text("a\nb\nc");
        let grown = c.desired_height(40);
        assert_eq!(
            grown,
            3 + FRAME_ROWS,
            "three lines → three text rows + frame"
        );
        assert!(grown > base, "grew with content");

        c.set_text("only one line");
        let shrunk = c.desired_height(40);
        assert_eq!(shrunk, 1 + FRAME_ROWS, "back to one text row + frame");
        assert!(shrunk < grown, "shrank with content");
    }
}
