//! The multiline prompt editor — a thin wrapper over `tui-textarea` for the
//! text/cursor *model* (editing, key handling), with our own **soft-wrapping**
//! renderer on top: `tui-textarea` 0.7 has no soft-wrap, so a long line without
//! spaces would scroll off the right edge. We wrap logical lines to visual rows
//! at the editor width (display-width aware, so CJK wraps correctly) and draw a
//! block caret at the wrapped position (SPEC-TUI: the widget stays behind this
//! wrapper, so replacing it later is one module).

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use tui_textarea::TextArea;

/// Hard ceiling on the composer's editor rows — a safety bound only. The real,
/// screen-relative cap (~50% of the screen, Claude Code's dynamic composer) is
/// applied by the caller via `term::max_composer_rows`.
const MAX_ROWS: u16 = 100;

/// Extra rows for the top and bottom framing rules.
const FRAME_ROWS: u16 = 2;

/// Left gutter width (`  ❯ ` = 2-col margin + `❯ ` prompt) — puts the input text
/// at column 4, aligned with the transcript bullet and the status line.
const GUTTER: u16 = 4;

/// Right margin — insets the input off the right edge (matches the rules).
const RIGHT_MARGIN: u16 = 2;

/// Placeholder shown when the composer is empty.
const PLACEHOLDER: &str = "type a prompt…";

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
    /// An empty composer.
    #[must_use]
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
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

    /// The cursor's character offset within a **single-line** draft; `None` once the
    /// draft has more than one line. Only the slash menu asks, and it does not run on
    /// multiline drafts, so a whole-buffer offset would be answering a question nobody
    /// has.
    #[must_use]
    pub fn cursor_offset(&self) -> Option<usize> {
        let (row, col) = self.textarea.cursor();
        (self.textarea.lines().len() == 1 && row == 0).then_some(col)
    }

    /// Replace the character range `range` with `text`, leaving the cursor just past
    /// what was inserted (the slash-completion accept path).
    pub fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        let current = self.text();
        let chars: Vec<char> = current.chars().collect();
        let end = range.end.min(chars.len());
        let start = range.start.min(end);
        let head: String = chars[..start].iter().collect();
        let tail: String = chars[end..].iter().collect();
        let cursor = start + text.chars().count();
        self.set_text(&format!("{head}{text}{tail}"));
        self.textarea.move_cursor(tui_textarea::CursorMove::Jump(
            0,
            u16::try_from(cursor).unwrap_or(u16::MAX),
        ));
    }

    /// The editor width available for text at composer area `width` (the full
    /// width minus the gutter and right margin).
    fn editor_width(width: u16) -> u16 {
        width.saturating_sub(GUTTER + RIGHT_MARGIN)
    }

    /// Rows needed at `width`: the **soft-wrapped** visual-row count (a long
    /// unbroken line now costs several rows instead of scrolling off-screen),
    /// clamped to `[1, MAX_ROWS]`, plus the two framing rules. Dynamic — grows
    /// and shrinks with the draft; the screen-relative ceiling is applied by the
    /// caller (`term::max_composer_rows`).
    #[must_use]
    pub fn desired_height(&self, width: u16) -> u16 {
        let rows = self.layout(Self::editor_width(width)).content_height();
        let rows = u16::try_from(rows).unwrap_or(MAX_ROWS);
        rows.clamp(1, MAX_ROWS) + FRAME_ROWS
    }

    /// Compute the wrapped visual layout for the current text at `editor_width`.
    fn layout(&self, editor_width: u16) -> VisualLayout {
        let w = usize::from(editor_width);
        let lines = self.textarea.lines();
        let (crow, ccol) = self.textarea.cursor();
        let mut visual: Vec<String> = Vec::new();
        let mut cursor_row = 0usize;
        let mut cursor_col = 0usize;
        for (li, line) in lines.iter().enumerate() {
            let base = visual.len();
            if li == crow && w > 0 {
                let (vr, vc) = cursor_in_line(line, ccol, w);
                cursor_row = base + vr;
                cursor_col = vc;
            }
            if w == 0 {
                visual.push(line.clone());
            } else {
                visual.extend(wrap_line(line, w));
            }
        }
        if visual.is_empty() {
            visual.push(String::new());
        }
        VisualLayout {
            visual,
            cursor_row,
            cursor_col,
        }
    }

    /// Render into `area`: a dim rule, the `❯ ` gutter + soft-wrapped editor, then
    /// a dim rule — the input's top/bottom frame (user choice, 2026-07-22). The
    /// editor is rendered by us (not the widget) so long lines **wrap** to the
    /// editor width instead of overflowing; a block caret marks the cursor, and
    /// the view scrolls to keep the caret on the bottom row once the draft
    /// overflows the (capped) editor height (ADR-0022 caret-follows-bottom).
    pub fn render(&self, frame: &mut crate::frame_terminal::Frame<'_>, area: Rect) {
        use ratatui::layout::{Constraint, Layout};
        use ratatui::widgets::Paragraph;

        let dim = Style::default().add_modifier(Modifier::DIM);
        let margin = usize::from(RIGHT_MARGIN);
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
            Constraint::Length(GUTTER),
            Constraint::Fill(1),
            Constraint::Length(RIGHT_MARGIN),
        ])
        .areas(mid);
        frame.render_widget(Paragraph::new(Line::from("  ❯ ")), gutter);
        frame.render_widget(
            Paragraph::new(self.editor_lines(editor.width, editor.height)),
            editor,
        );
        frame.render_widget(Paragraph::new(Line::styled(rule, dim)), bottom);
    }

    /// The visible editor rows for an `editor_width` × `editor_height` editor: the
    /// soft-wrapped text scrolled so the caret stays visible (glued to the bottom
    /// row on overflow), with a block caret on the cursor row and the dim
    /// placeholder when empty.
    fn editor_lines(&self, editor_width: u16, editor_height: u16) -> Vec<Line<'static>> {
        let eh = usize::from(editor_height);
        if eh == 0 {
            return Vec::new();
        }
        let dim = Style::default().add_modifier(Modifier::DIM);

        if self.is_empty() {
            // Block caret over the first placeholder glyph; the rest stays dim.
            let mut chars = PLACEHOLDER.chars();
            let first = chars.next().unwrap_or(' ').to_string();
            let rest: String = chars.collect();
            let mut out = vec![Line::from(vec![
                Span::styled(first, dim.add_modifier(Modifier::REVERSED)),
                Span::styled(rest, dim),
            ])];
            out.extend(std::iter::repeat_n(Line::default(), eh.saturating_sub(1)));
            return out;
        }

        let caret = Style::default().add_modifier(Modifier::REVERSED);
        let layout = self.layout(editor_width);
        let content_h = layout.content_height();
        let offset = if content_h <= eh {
            0
        } else {
            layout.cursor_row.saturating_sub(eh - 1).min(content_h - eh)
        };
        let mut out = Vec::with_capacity(eh);
        for i in 0..eh {
            let vrow = offset + i;
            let text = layout.visual.get(vrow).map_or("", String::as_str);
            if vrow == layout.cursor_row {
                out.push(caret_line(text, layout.cursor_col, caret));
            } else {
                out.push(Line::raw(text.to_string()));
            }
        }
        out
    }
}

/// A wrapped visual layout: the display rows plus the cursor's row/column within
/// them (both 0-based; `cursor_col` is a display column, not a char index).
struct VisualLayout {
    visual: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

impl VisualLayout {
    /// Rows the content occupies, including the phantom row the caret sits on
    /// when it is parked just past a full-width wrap boundary.
    fn content_height(&self) -> usize {
        self.visual.len().max(self.cursor_row + 1)
    }
}

/// Display width of a string (unicode-aware, via ratatui's own measurement, so
/// CJK/wide glyphs count as two columns).
fn disp_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Display width of a single char.
fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    disp_width(c.encode_utf8(&mut buf))
}

/// Split one logical line into display-width-bounded visual segments (character
/// wrap, no word breaking). An empty line yields one empty segment; a segment is
/// never empty except that lone empty-line case.
fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![line.to_string()];
    }
    let mut rows: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in line.chars() {
        let cw = char_width(ch);
        if cur_w + cw > width && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += cw;
    }
    rows.push(cur);
    rows
}

/// Map a char offset `col` into `line` to its (visual-row-within-line, display-col)
/// at `width`. When the caret is parked exactly at a full-width wrap boundary it
/// sits at the start of the next visual row.
fn cursor_in_line(line: &str, col: usize, width: usize) -> (usize, usize) {
    if width == 0 {
        return (0, 0);
    }
    let prefix: String = line.chars().take(col).collect();
    let segs = wrap_line(&prefix, width);
    let last_w = segs.last().map_or(0, |s| disp_width(s));
    if last_w >= width {
        (segs.len(), 0)
    } else {
        (segs.len() - 1, last_w)
    }
}

/// Build a visual row with a block caret (reversed cell) at display column
/// `vcol`. If the caret is past the last glyph (end of line), it reverses a
/// trailing space.
fn caret_line(row: &str, vcol: usize, caret: Style) -> Line<'static> {
    let mut before = String::new();
    let mut w = 0usize;
    let mut iter = row.chars();
    while w < vcol {
        match iter.next() {
            Some(c) => {
                w += char_width(c);
                before.push(c);
            }
            None => break,
        }
    }
    let (caret_txt, after) = match iter.next() {
        Some(c) => (c.to_string(), iter.collect::<String>()),
        None => (" ".to_string(), String::new()),
    };
    Line::from(vec![
        Span::raw(before),
        Span::styled(caret_txt, caret),
        Span::raw(after),
    ])
}

#[cfg(test)]
mod tests {
    use super::{Composer, FRAME_ROWS};
    use crate::frame_terminal::FrameTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    /// Row of the block caret (the reversed cell) in the rendered composer.
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

    /// The visible editor text rows (trimmed) of the rendered composer.
    fn rendered_rows(c: &Composer, area: Rect, screen_h: u16) -> Vec<String> {
        let mut t = FrameTerminal::new(TestBackend::new(area.width, screen_h)).unwrap();
        t.draw(|f| c.render(f, area)).unwrap();
        let buf = t.backend().buffer();
        (0..screen_h)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// The caret sits on the row of the *last* content line — not one above it.
    #[test]
    fn caret_sits_on_the_last_content_row() {
        let mut c = Composer::new();
        c.insert_text("a\nb\nc"); // 3 lines; caret at end of line 3 (index 2)
        // Editor occupies rows 1..4; the last content row ("c") is screen row 3.
        assert_eq!(
            caret_row(&c, Rect::new(0, 0, 30, 5), 5),
            Some(3),
            "caret on the last content row (screen row 3)"
        );
    }

    /// When the draft overflows the editor, the caret is glued to the bottom
    /// editor row — and stays there after a Backspace.
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
        let area = Rect::new(0, 0, 30, 7);
        assert_eq!(
            caret_row(&c, area, 7),
            Some(5),
            "caret glued to the bottom editor row (screen row 5)"
        );
        c.input(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Backspace,
        ));
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

    /// A single long line with no spaces **wraps** to multiple visual rows
    /// instead of overflowing off the right edge — the reported bug.
    #[test]
    fn long_unbroken_line_wraps_to_multiple_rows() {
        let mut c = Composer::new();
        // area width 20 → editor width 20 - 4 (gutter) - 2 (right) = 14.
        c.insert_text("abcdefghijklmnopqrstuvwxyz0123"); // 30 chars → ceil(30/14) = 3 rows
        assert_eq!(
            c.desired_height(20),
            3 + FRAME_ROWS,
            "30 chars at editor-width 14 wrap to 3 rows"
        );

        // Render tall enough to show all rows. The wrap is proven by the 3-row
        // height (a truncated single line would be 1 row); assert the wrapped
        // segments appear and nothing spills past the editor's right edge.
        let area = Rect::new(0, 0, 20, 3 + FRAME_ROWS);
        let rows = rendered_rows(&c, area, 3 + FRAME_ROWS);
        assert!(
            rows.iter().any(|r| r.contains("abcdefghijklmn")),
            "first wrapped segment present: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("opqrstuvwxyz01")),
            "second wrapped segment present: {rows:?}"
        );
        // Editor text lives in columns 4..18 (width 14); nothing spills past 18.
        for r in &rows {
            assert!(
                r.chars().count() <= 18,
                "row within the right margin (no overflow): {r:?}"
            );
        }
    }

    /// The slash menu needs a single-line character offset, and explicitly nothing on
    /// a multiline draft (where it does not run).
    #[test]
    fn cursor_offset_tracks_a_single_line_and_vanishes_on_a_second() {
        let mut c = Composer::new();
        assert_eq!(c.cursor_offset(), Some(0), "empty composer, caret at 0");
        c.insert_text("/mod");
        assert_eq!(c.cursor_offset(), Some(4));
        c.insert_newline();
        assert_eq!(c.cursor_offset(), None, "multiline draft has no offset");
    }

    /// Accepting a completion replaces the command token in place and leaves the caret
    /// just past it — not at the end of the line, which would jump over trailing args.
    #[test]
    fn replace_range_swaps_the_token_and_parks_the_caret_after_it() {
        let mut c = Composer::new();
        c.insert_text("/mod gpt-5");
        c.replace_range(0..4, "/model");
        assert_eq!(c.text(), "/model gpt-5");
        assert_eq!(c.cursor_offset(), Some(6), "caret after the inserted name");

        // Character offsets, not bytes: a wide-glyph draft must not be sliced apart.
        let mut c = Composer::new();
        c.insert_text("/新 尾");
        c.replace_range(0..2, "/new");
        assert_eq!(c.text(), "/new 尾");
        assert_eq!(c.cursor_offset(), Some(4));
    }

    /// Wide (CJK) glyphs wrap by *display* width — two columns each — so a run of
    /// double-width characters wraps at half the column count.
    #[test]
    fn wide_glyphs_wrap_by_display_width() {
        let mut c = Composer::new();
        // editor width 14 → 7 double-width glyphs per row. 16 glyphs → 3 rows.
        c.insert_text("测试一二三四五六七八九十甲乙丙丁");
        assert_eq!(
            c.desired_height(20),
            3 + FRAME_ROWS,
            "16 wide glyphs at width 14 wrap to 3 rows"
        );
    }
}
