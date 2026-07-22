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
        if editor_height == 0 {
            return;
        }
        // Reset the viewport deterministically, then restore the caret and let
        // tui-textarea's render place the scroll: it homes the viewport to the
        // top, then (at render) scrolls just enough to keep the caret visible —
        // which puts the caret on the BOTTOM row whenever the draft overflows and
        // the caret is at the end (the Shift+Enter-past-the-cap / Backspace case),
        // and on its own line when the draft fits.
        //
        // `scroll` drags the caret into the scrolled viewport, so we save the
        // caret first and jump it back. `-i16::MAX` (not `i16::MIN`, which
        // overflows when tui-textarea negates the delta).
        let (row, col) = self.textarea.cursor();
        self.textarea.scroll((-i16::MAX, 0));
        self.textarea.move_cursor(tui_textarea::CursorMove::Jump(
            u16::try_from(row).unwrap_or(u16::MAX),
            u16::try_from(col).unwrap_or(u16::MAX),
        ));
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
    use crate::frame_terminal::FrameTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    /// Row of the tui-textarea caret cell (the reversed cell) in the rendered
    /// composer, or None.
    fn caret_row(c: &Composer, area: Rect, screen_h: u16) -> Option<u16> {
        let mut t = FrameTerminal::new(TestBackend::new(area.width, screen_h)).unwrap();
        t.draw(|f| c.render(f, area)).unwrap();
        let buf = t.backend().buffer();
        for y in 0..screen_h {
            for x in 0..area.width {
                if buf[(x, y)].modifier.contains(Modifier::REVERSED) {
                    return Some(y);
                }
            }
        }
        None
    }

    /// The caret sits on the row of the *last* content line — not one above it.
    /// (Regression: after a few Shift+Enters the caret rendered a row too high.)
    #[test]
    fn caret_sits_on_the_last_content_row() {
        let mut c = Composer::new();
        c.insert_text("a\nb\nc"); // 3 lines; caret at end of line 3 (index 2)
        // Composer area = 5 rows (top rule, 3-row editor, bottom rule) at screen top.
        let editor_h = 3;
        c.sync_scroll(editor_h);
        // Editor occupies rows 1..4; the last content row ("c") is screen row 3.
        assert_eq!(
            caret_row(&c, Rect::new(0, 0, 30, 5), 5),
            Some(3),
            "caret on the last content row (screen row 3)"
        );
    }

    /// When the draft overflows the editor, the caret is glued to the bottom
    /// editor row — and stays there after a Backspace (the earlier "caret drifts
    /// up on Backspace past the cap" bug).
    #[test]
    fn caret_glued_to_bottom_on_overflow_and_after_backspace() {
        let mut c = Composer::new();
        for i in 0..10 {
            c.insert_text(&format!("line{i}"));
            if i < 9 {
                c.insert_newline();
            }
        }
        // Editor is 5 rows; composer area = 7 (rule, 5-row editor, rule).
        let editor_h = 5;
        let area = Rect::new(0, 0, 30, 7);
        c.sync_scroll(editor_h);
        assert_eq!(
            caret_row(&c, area, 7),
            Some(5),
            "caret glued to the bottom editor row (screen row 5)"
        );
        // Backspace a char (still overflowing): caret must stay on the bottom row.
        c.input(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Backspace,
        ));
        c.sync_scroll(editor_h);
        assert_eq!(
            caret_row(&c, area, 7),
            Some(5),
            "caret stays on the bottom row after Backspace"
        );
    }

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
