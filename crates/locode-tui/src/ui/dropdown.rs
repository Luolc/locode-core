//! The slash-command dropdown, drawn directly above the composer.
//!
//! Ported from grok's `views/slash_dropdown.rs`: an aligned label column, the
//! description in a dimmed second column, and a selected row carrying a background
//! band, bold text and a `❯` prefix — with two spaces in the prefix slot otherwise, so
//! the labels never shift as the selection moves.
//!
//! **One deviation, deliberate.** Grok word-wraps a long description across extra
//! rows; we truncate to one row per item. Its dropdown caps at six *rows*, so a wrapped
//! description eats the menu — and a skill's description is routing text written for a
//! model, routinely a full sentence. One row per item keeps six *commands* visible,
//! which is what a command menu is for. (This is our own UI, not a ported pack: the
//! faithfulness rule governs packs, and best-of applies here.)

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::commands::{MAX_VISIBLE_ROWS, SlashState, SuggestionRow};

/// Left and right margins, matching the transcript blocks and the composer rules.
const MARGIN: usize = 2;

/// Width of the `❯ ` / `  ` selection prefix. With the margin this puts labels at
/// column 4 — the composer's text column.
const PREFIX_W: usize = 2;

/// Columns between the label column and the description.
const GAP: usize = 2;

/// Hard cap on the label column, so one very long skill name cannot squeeze every
/// description off the row (grok's `LABEL_CAP`).
const LABEL_CAP: usize = 40;

/// Background band on the selected row. `DarkGray` is the ANSI bright-black palette
/// slot — "one step above the background" under whatever theme the user runs, the same
/// palette-relative choice the user-prompt band makes.
const SELECTED_BG: Color = Color::DarkGray;

/// The matched letters. Bright blue reads against the band and the normal background
/// alike (grok's `theme.fuzzy_accent`).
const MATCH_FG: Color = Color::LightBlue;

/// Rows the dropdown wants: one per suggestion, capped.
#[must_use]
pub fn desired_rows(state: &SlashState) -> u16 {
    if !state.open {
        return 0;
    }
    u16::try_from(state.matches.len().min(MAX_VISIBLE_ROWS)).unwrap_or(0)
}

/// Draw the visible slice of the menu into `area`.
pub fn render(frame: &mut crate::frame_terminal::Frame<'_>, area: Rect, state: &SlashState) {
    if !state.open || area.height == 0 {
        return;
    }
    let content_w = usize::from(area.width).saturating_sub(2 * MARGIN);
    if content_w <= PREFIX_W + GAP {
        return;
    }
    let label_w = label_column_width(&state.matches, content_w - PREFIX_W);
    let height = usize::from(area.height);
    let offset = state.scroll_offset(height);
    let selected = state.selected.min(state.matches.len().saturating_sub(1));

    let lines: Vec<Line<'static>> = state
        .matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, row)| row_line(row, i == selected, label_w, content_w))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The aligned label column: the widest label, bounded by 60% of the space and by
/// [`LABEL_CAP`] — full command names matter more than long descriptions.
fn label_column_width(rows: &[SuggestionRow], available: usize) -> usize {
    let budget = (available * 3 / 5).min(LABEL_CAP);
    rows.iter()
        .map(|r| span_width(&r.display))
        .filter(|&w| w <= LABEL_CAP)
        .max()
        .unwrap_or(0)
        .min(budget)
}

/// One rendered row: `  ❯ /name    description`, background-filled to `content_w`
/// when selected.
fn row_line(
    row: &SuggestionRow,
    selected: bool,
    label_w: usize,
    content_w: usize,
) -> Line<'static> {
    let bold = if selected {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let base = if selected {
        Style::default().bg(SELECTED_BG)
    } else {
        Style::default()
    };
    let normal = base.add_modifier(bold);
    let matched = normal.fg(MATCH_FG);
    let dim = base.add_modifier(Modifier::DIM);

    let label = truncate(&row.display, label_w);
    let label_used = span_width(&label);
    let desc_w = content_w.saturating_sub(PREFIX_W + label_w + GAP);
    let desc = truncate(&row.description, desc_w);

    let mut spans = vec![
        // The margin is outside the band: the band starts at the prefix, like the
        // composer's text column.
        Span::raw(" ".repeat(MARGIN)),
        Span::styled(if selected { "❯ " } else { "  " }, normal),
    ];
    spans.extend(highlight(&label, &row.indices, normal, matched));
    spans.push(Span::styled(" ".repeat(label_w - label_used + GAP), base));
    let desc_used = span_width(&desc);
    spans.push(Span::styled(desc, dim));
    // Fill the rest so the band spans the full content width.
    spans.push(Span::styled(
        " ".repeat(desc_w.saturating_sub(desc_used)),
        base,
    ));
    Line::from(spans)
}

/// Split `text` into runs of matched / unmatched characters, one span per run.
///
/// Runs rather than one span per character (grok's `build_highlighted_spans`): fewer
/// spans, and no visible seams between identically-styled cells.
fn highlight(text: &str, indices: &[u32], normal: Style, matched: Style) -> Vec<Span<'static>> {
    if indices.is_empty() {
        return vec![Span::styled(text.to_string(), normal)];
    }
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_is_match = false;
    for (i, ch) in text.chars().enumerate() {
        let is_match = indices.contains(&u32::try_from(i).unwrap_or(u32::MAX));
        if run.is_empty() {
            run_is_match = is_match;
        } else if is_match != run_is_match {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_is_match { matched } else { normal },
            ));
            run_is_match = is_match;
        }
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(
            run,
            if run_is_match { matched } else { normal },
        ));
    }
    spans
}

/// Display width of a string (wide glyphs count as two columns).
fn span_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Truncate to `width` display columns, marking the cut with `…`.
fn truncate(text: &str, width: usize) -> String {
    if span_width(text) <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = span_width(ch.encode_utf8(&mut [0u8; 4]));
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_terminal::FrameTerminal;
    use ratatui::backend::TestBackend;

    fn row(display: &str, description: &str, indices: Vec<u32>) -> SuggestionRow {
        SuggestionRow {
            display: display.into(),
            description: description.into(),
            insert_text: display.into(),
            indices,
        }
    }

    fn state(rows: Vec<SuggestionRow>, selected: usize) -> SlashState {
        SlashState::open_with(rows, selected)
    }

    /// Render and return `(text_rows, cells)` for assertions on style.
    fn draw(state: &SlashState, width: u16, height: u16) -> FrameTerminal<TestBackend> {
        let mut t = FrameTerminal::new(TestBackend::new(width, height)).unwrap();
        t.draw(|f| render(f, Rect::new(0, 0, width, height), state))
            .unwrap();
        t
    }

    fn text_rows(t: &FrameTerminal<TestBackend>) -> Vec<String> {
        let buf = t.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn rows_show_the_label_and_description_in_aligned_columns() {
        let s = state(
            vec![
                row("/new", "start a fresh session", vec![]),
                row("/quit", "exit locode", vec![]),
            ],
            0,
        );
        let rows = text_rows(&draw(&s, 50, 2));
        // The label column is the widest label ("/quit" = 5) plus the two-column gap.
        assert_eq!(rows[0], "  ❯ /new   start a fresh session");
        assert_eq!(rows[1], "    /quit  exit locode");
        // Both descriptions begin in the same *column* — the aligned label column.
        // (Byte offsets differ: `❯` is three bytes.)
        let column = |row: &str, needle: &str| row[..row.find(needle).unwrap()].chars().count();
        assert_eq!(
            column(&rows[0], "start"),
            column(&rows[1], "exit"),
            "description column aligned: {rows:?}"
        );
    }

    /// The selected row is the only one carrying the band, the bold, and the arrow;
    /// unselected rows pay two spaces so nothing shifts as the selection moves.
    #[test]
    fn the_selected_row_gets_the_band_bold_and_arrow() {
        let s = state(vec![row("/new", "a", vec![]), row("/quit", "b", vec![])], 1);
        let t = draw(&s, 40, 2);
        let buf = t.backend().buffer();
        let rows = text_rows(&t);
        assert!(rows[1].starts_with("  ❯ /quit"), "arrow on row 1: {rows:?}");
        assert!(
            rows[0].starts_with("    /new"),
            "no arrow on row 0: {rows:?}"
        );
        // The band covers the selected row from the prefix to the content edge…
        for x in 2..38 {
            assert_eq!(
                buf[(x, 1)].style().bg,
                Some(SELECTED_BG),
                "selected row banded at column {x}"
            );
            assert_ne!(
                buf[(x, 0)].style().bg,
                Some(SELECTED_BG),
                "unselected row unbanded at column {x}"
            );
        }
        // …and the margin stays outside it.
        assert_ne!(
            buf[(0, 1)].style().bg,
            Some(SELECTED_BG),
            "left margin outside the band"
        );
        assert!(
            buf[(4, 1)].style().add_modifier.contains(Modifier::BOLD),
            "selected label bold"
        );
    }

    #[test]
    fn matched_letters_are_styled_as_runs_not_characters() {
        // "/model" with "od" matched → three runs: "/m", "od", "el".
        let spans = highlight(
            "/model",
            &[2, 3],
            Style::default(),
            Style::default().fg(MATCH_FG),
        );
        let rendered: Vec<(&str, Option<Color>)> = spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style.fg))
            .collect();
        assert_eq!(
            rendered,
            vec![("/m", None), ("od", Some(MATCH_FG)), ("el", None),]
        );
    }

    #[test]
    fn a_long_description_is_truncated_to_one_row() {
        let s = state(
            vec![row(
                "/commit",
                "use when the user asks to commit staged work and write a message",
                vec![],
            )],
            0,
        );
        let rows = text_rows(&draw(&s, 40, 1));
        assert_eq!(rows.len(), 1, "one row per item, never wrapped");
        assert!(rows[0].ends_with('…'), "cut is marked: {rows:?}");
        assert!(
            rows[0].chars().count() <= 38,
            "inside the margins: {rows:?}"
        );
    }

    /// More items than rows: the viewport scrolls to keep the selection visible.
    #[test]
    fn the_viewport_follows_the_selection() {
        let rows: Vec<SuggestionRow> = (0..8)
            .map(|i| row(&format!("/cmd{i}"), "", vec![]))
            .collect();
        let mut s = state(rows, 0);
        assert_eq!(desired_rows(&s), 6, "capped at six rows");

        let shown = text_rows(&draw(&s, 30, 3));
        assert!(shown[0].contains("/cmd0") && shown[2].contains("/cmd2"));

        s.selected = 7;
        let shown = text_rows(&draw(&s, 30, 3));
        assert!(
            shown.iter().any(|r| r.contains("/cmd7")),
            "the selection stays visible: {shown:?}"
        );
        assert!(
            !shown.iter().any(|r| r.contains("/cmd0")),
            "and the list scrolled: {shown:?}"
        );
    }

    #[test]
    fn a_closed_menu_draws_nothing() {
        let mut s = state(vec![row("/new", "x", vec![])], 0);
        s.open = false;
        assert_eq!(desired_rows(&s), 0);
        assert_eq!(text_rows(&draw(&s, 30, 1)), vec![String::new()]);
    }

    /// A pathologically narrow terminal must not panic or spill.
    #[test]
    fn very_narrow_areas_are_survivable() {
        let s = state(vec![row("/new", "start a fresh session", vec![])], 0);
        for width in 1..12u16 {
            let rows = text_rows(&draw(&s, width, 1));
            assert!(
                rows[0].chars().count() <= usize::from(width),
                "width {width}: {rows:?}"
            );
        }
    }
}
