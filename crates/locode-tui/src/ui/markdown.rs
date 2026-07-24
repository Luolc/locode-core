//! Markdown → styled `Line`s for assistant text.
//!
//! A pulldown-cmark pass covering the constructs an agent actually emits:
//! headings (bold), lists (bulleted/ordered, nested, hanging-indented),
//! inline code (cyan), fenced code (syntect-highlighted via `super::highlight`),
//! block quotes, inline bold/italic/strikethrough, and GFM tables (aligned
//! columns, bold header + dim rule; `render_table`). The pattern follows codex's
//! `markdown_render.rs`: parse to events, collect inline content **preserving
//! exact whitespace**, and width-wrap per block with a first-line prefix + a
//! hanging continuation indent.
//!
//! Whitespace correctness is the load-bearing property here: inline styling
//! (code, bold) must not add or drop spaces around a span. We therefore never
//! reconstruct spacing from `split_whitespace`; we keep the source text verbatim
//! in styled segments and only collapse whitespace at the word-wrap boundary.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Terminal display width of a char in cells: CJK/fullwidth = 2, emoji = 2,
/// zero-width/control = 0. All wrapping math measures cells, never char/byte count —
/// counting characters truncates CJK on the right (each han char is 2 cells).
fn ch_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

/// Display width of a run of style-tagged chars.
fn word_width(word: &[(char, Style)]) -> usize {
    word.iter().map(|&(ch, _)| ch_width(ch)).sum()
}

/// How many leading chars of `chars` fit in `max_cols` display columns — always at
/// least 1, so a lone wide char in a too-narrow column still makes progress (no
/// infinite loop; codex's `take_prefix_by_width` guard).
fn prefix_by_width(chars: &[(char, Style)], max_cols: usize) -> usize {
    let mut cols = 0usize;
    let mut n = 0usize;
    for &(ch, _) in chars {
        let w = ch_width(ch);
        if n >= 1 && cols + w > max_cols {
            break;
        }
        cols += w;
        n += 1;
    }
    n.max(1)
}

/// Render markdown `text` to word-wrapped styled lines at `width`.
#[must_use]
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut w = Writer::new(width);
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES);
    for event in parser {
        w.event(event);
    }
    w.finish()
}

/// One run of inline text with a single style (whitespace preserved verbatim).
#[derive(Clone)]
struct Seg {
    text: String,
    style: Style,
}

/// A styled word: a run of non-space chars, each carrying its own style.
type Word = Vec<(char, Style)>;
/// A wrap unit + whether whitespace preceded it in the source (so joins are spaced or
/// glued faithfully). A narrow run is one unit; each wide (CJK/emoji) char is its own
/// unit, so a space-less CJK run has per-character break opportunities.
type Unit = (Word, bool);
/// One table cell's content, tokenized into styled units.
type CellWords = Vec<Unit>;

/// Accumulates inline segments per block, then wraps them into styled lines.
struct Writer {
    width: usize,
    out: Vec<Line<'static>>,
    /// Inline content of the block currently being built.
    inline: Vec<Seg>,
    /// Inline style nesting counters.
    bold: u32,
    italic: u32,
    strike: u32,
    heading: bool,
    in_code_block: bool,
    /// Language token of the current fenced code block (empty = none/unknown).
    code_lang: String,
    /// Buffered code-block text, highlighted as a whole at the closing fence.
    code_buf: String,
    /// List item markers by nesting depth (`None` = bullet, `Some(n)` = ordered).
    list_stack: Vec<Option<u64>>,
    /// Marker to render before the first line of the current list item.
    item_marker: Option<String>,
    quote_depth: u32,
    /// Table accumulation (pulldown emits cells as inline runs; we lay them out
    /// as aligned columns at the closing `Table`). Column alignments, the rows
    /// collected so far (each cell = its inline segments), the row being built,
    /// and how many leading rows are the header.
    table_aligns: Vec<Alignment>,
    table_rows: Vec<Vec<Vec<Seg>>>,
    table_row: Vec<Vec<Seg>>,
    table_head_rows: usize,
    in_table_cell: bool,
}

impl Writer {
    fn new(width: usize) -> Self {
        Self {
            width: width.max(4),
            out: Vec::new(),
            inline: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            heading: false,
            in_code_block: false,
            code_lang: String::new(),
            code_buf: String::new(),
            list_stack: Vec::new(),
            item_marker: None,
            quote_depth: 0,
            table_aligns: Vec::new(),
            table_rows: Vec::new(),
            table_row: Vec::new(),
            table_head_rows: 0,
            in_table_cell: false,
        }
    }

    fn inline_style(&self) -> Style {
        let mut s = Style::default();
        if self.bold > 0 || self.heading {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        s
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(&tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(c) => {
                let style = self.inline_style().fg(Color::Cyan);
                self.inline.push(Seg {
                    text: c.into_string(),
                    style,
                });
            }
            // A soft break is inter-word whitespace (reflow); a hard break is
            // treated the same at v1 (agents rarely emit hard breaks mid-prose).
            Event::SoftBreak | Event::HardBreak => self.inline.push(Seg {
                text: " ".to_string(),
                style: Style::default(),
            }),
            Event::Rule => {
                self.flush_inline();
                self.gap();
                self.out.push(Line::styled(
                    "─".repeat(self.width),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: &Tag<'_>) {
        match tag {
            Tag::Heading { .. } => {
                self.gap();
                self.heading = true;
            }
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                self.gap();
                self.in_code_block = true;
                // The info string may carry attributes (`rust,ignore`); the
                // language is its first whitespace/comma-delimited token.
                self.code_lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split([' ', '\t', ','])
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code_buf.clear();
            }
            Tag::List(first) => {
                // Emit any pending item text before a nested list opens (tight
                // lists put the item's own text directly before the sublist).
                self.flush_inline();
                if self.list_stack.is_empty() {
                    self.gap();
                }
                self.list_stack.push(*first);
            }
            Tag::Item => {
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{indent}{n}. ");
                        *n += 1;
                        m
                    }
                    _ => format!("{indent}• "),
                };
                self.item_marker = Some(marker);
            }
            Tag::BlockQuote(_) => {
                self.gap();
                self.quote_depth += 1;
            }
            Tag::Table(aligns) => {
                self.flush_inline();
                self.gap();
                self.table_aligns.clone_from(aligns);
                self.table_rows.clear();
                self.table_row.clear();
                self.table_head_rows = 0;
            }
            Tag::TableHead | Tag::TableRow => self.table_row = Vec::new(),
            Tag::TableCell => {
                self.in_table_cell = true;
                self.inline.clear();
            }
            Tag::Paragraph if self.list_stack.is_empty() && self.quote_depth == 0 => {
                self.gap();
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.heading = false;
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::CodeBlock => {
                self.emit_code_block();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item | TagEnd::Paragraph => self.flush_inline(),
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::TableCell => {
                self.in_table_cell = false;
                let cell = std::mem::take(&mut self.inline);
                self.table_row.push(cell);
            }
            TagEnd::TableHead => {
                self.table_rows.push(std::mem::take(&mut self.table_row));
                self.table_head_rows = self.table_rows.len();
            }
            TagEnd::TableRow => self.table_rows.push(std::mem::take(&mut self.table_row)),
            TagEnd::Table => self.render_table(),
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if self.in_code_block {
            // Buffer the whole block; highlight it as one unit at the closing
            // fence (a syntax highlighter needs full lines, not fragments).
            self.code_buf.push_str(t);
            return;
        }
        self.inline.push(Seg {
            text: t.to_string(),
            style: self.inline_style(),
        });
    }

    /// Emit the buffered code block: syntax-highlighted when the language is
    /// known (indented, no wrap — preserve lines for copy/paste, as codex
    /// does), else plain dim lines.
    fn emit_code_block(&mut self) {
        let code = std::mem::take(&mut self.code_buf);
        let lang = std::mem::take(&mut self.code_lang);
        let code = code.strip_suffix('\n').unwrap_or(&code);
        if let Some(highlighted) = crate::ui::highlight::highlight_lines(code, &lang) {
            for spans in highlighted {
                let mut line = vec![Span::raw("    ")];
                line.extend(spans);
                self.out.push(Line::from(line));
            }
        } else {
            let dim = Style::default().add_modifier(Modifier::DIM);
            for raw in code.split('\n') {
                self.out
                    .push(Line::from(Span::styled(format!("    {raw}"), dim)));
            }
        }
    }

    /// Lay out the accumulated table with box-drawing borders (Claude Code /
    /// Grok style): natural column widths shrunk proportionally to fit, cells
    /// wrapped, a bold header separated by a `├─┼─┤` rule. One space of padding
    /// inside each cell; borders are dim.
    fn render_table(&mut self) {
        let rows = std::mem::take(&mut self.table_rows);
        let aligns = std::mem::take(&mut self.table_aligns);
        let head_rows = std::mem::take(&mut self.table_head_rows);
        if rows.is_empty() {
            return;
        }
        let n_cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);

        // Words per cell + each column's natural (unwrapped) width.
        let mut cells: Vec<Vec<CellWords>> = Vec::with_capacity(rows.len());
        let mut col_natural = vec![1usize; n_cols];
        for row in &rows {
            let mut rw = Vec::with_capacity(n_cols);
            for (c, natural_w) in col_natural.iter_mut().enumerate() {
                let units = row.get(c).map(|s| segs_to_units(s)).unwrap_or_default();
                // Natural (unwrapped) width: unit widths + one space per space-joined unit.
                let natural = units.iter().map(|(w, _)| word_width(w)).sum::<usize>()
                    + units.iter().skip(1).filter(|(_, sp)| *sp).count();
                *natural_w = (*natural_w).max(natural);
                rw.push(units);
            }
            cells.push(rw);
        }

        // Borders + 1-space cell padding: `│ c │ c │` → 3 cols of chrome per
        // column plus one closing border. Shrink to fit if the naturals overflow.
        let overhead = 3 * n_cols + 1;
        let natural_sum: usize = col_natural.iter().sum();
        let col_width: Vec<usize> = if natural_sum + overhead <= self.width || natural_sum == 0 {
            col_natural.clone()
        } else {
            let target = self.width.saturating_sub(overhead).max(n_cols * 3);
            col_natural
                .iter()
                .map(|&w| (w * target / natural_sum).max(3))
                .collect()
        };

        // Borders render in the normal foreground (NOT dim): terminals render
        // the DIM modifier on box-drawing as a muddy/tinted color that reads as
        // "a different color" and can hide the header rule (user, 2026-07-22).
        let hrule = |left: &str, mid: &str, right: &str| -> Line<'static> {
            let mut s = String::from(left);
            for (i, w) in col_width.iter().enumerate() {
                if i > 0 {
                    s.push_str(mid);
                }
                s.push_str(&"─".repeat(w + 2));
            }
            s.push_str(right);
            Line::from(s)
        };

        self.out.push(hrule("┌", "┬", "┐"));
        let empty = Line::from("");
        for (ri, row) in cells.iter().enumerate() {
            let is_head = ri < head_rows;
            let cell_lines: Vec<Vec<Line<'static>>> = (0..n_cols)
                .map(|c| {
                    let mut lines = wrap_words(&row[c], &[], &[], col_width[c]);
                    if is_head {
                        for line in &mut lines {
                            for span in &mut line.spans {
                                span.style = span.style.add_modifier(Modifier::BOLD);
                            }
                        }
                    }
                    lines
                })
                .collect();
            let height = cell_lines.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for r in 0..height {
                let mut spans: Vec<Span<'static>> = vec![Span::raw("│")];
                for c in 0..n_cols {
                    spans.push(Span::raw(" "));
                    let line = cell_lines[c].get(r).unwrap_or(&empty);
                    let align = aligns.get(c).copied().unwrap_or(Alignment::None);
                    pad_into(&mut spans, line, col_width[c], align);
                    spans.push(Span::raw(" "));
                    spans.push(Span::raw("│"));
                }
                self.out.push(Line::from(spans));
            }
            // A `├─┼─┤` rule between every row (full grid, Grok-style) so the
            // header AND each inner row are separated; the last row is followed
            // by the bottom border instead.
            if ri + 1 < cells.len() {
                self.out.push(hrule("├", "┼", "┤"));
            }
        }
        self.out.push(hrule("└", "┴", "┘"));
    }

    /// The first-line and continuation prefixes for the current block context
    /// (quote bar + list indent/marker). Consumes the pending item marker.
    fn leads(&mut self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut first: Vec<Span<'static>> = Vec::new();
        let mut cont: Vec<Span<'static>> = Vec::new();
        if self.quote_depth > 0 {
            let bar = "┃ ".repeat(self.quote_depth as usize);
            first.push(Span::styled(bar.clone(), dim));
            cont.push(Span::styled(bar, dim));
        }
        if !self.list_stack.is_empty() {
            let depth = self.list_stack.len() - 1;
            let indent = "  ".repeat(depth);
            if let Some(marker) = self.item_marker.take() {
                let width = marker.width();
                first.push(Span::raw(marker));
                cont.push(Span::raw(" ".repeat(width)));
            } else {
                // A continuation paragraph inside an item aligns under the text.
                let width = indent.width() + 2;
                first.push(Span::raw(" ".repeat(width)));
                cont.push(Span::raw(" ".repeat(width)));
            }
        }
        (first, cont)
    }

    /// Wrap the accumulated inline segments into `out`, then clear them.
    fn flush_inline(&mut self) {
        let words = segs_to_units(&self.inline);
        self.inline.clear();
        if words.is_empty() {
            // No content: drop any dangling marker so it can't leak downward.
            self.item_marker = None;
            return;
        }
        let (first, cont) = self.leads();
        let lines = wrap_words(&words, &first, &cont, self.width);
        self.out.extend(lines);
    }

    /// Ensure a single blank line separates the previous block from the next.
    fn gap(&mut self) {
        if !self.out.is_empty() && !last_is_blank(&self.out) {
            self.out.push(Line::from(""));
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_inline();
        while last_is_blank(&self.out) {
            self.out.pop();
        }
        self.out
    }
}

/// Split inline segments into wrap **units** (see [`Unit`]): a run of narrow chars is
/// one unit, and **each wide (CJK/emoji, 2-cell) char is its own unit** — so a space-less
/// CJK run has per-character break opportunities and can *fill a line's tail* instead of
/// jumping whole to the next line (the over-aggressive raggedness a plain word model
/// causes). Each unit records whether whitespace preceded it, so joins are spaced or
/// glued exactly as in the source. This approximates UAX#14 for CJK — the full
/// textwrap/UAX#14 upgrade (punctuation-aware breaks, grapheme clusters) is tracked in
/// `docs/research/tui-text-wrapping-cjk.md`.
fn segs_to_units(segs: &[Seg]) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    let mut cur: Word = Vec::new();
    let mut space_before = false; // whitespace seen since the last emitted unit
    for seg in segs {
        for ch in seg.text.chars() {
            if ch.is_whitespace() {
                if !cur.is_empty() {
                    units.push((std::mem::take(&mut cur), space_before));
                }
                space_before = true;
            } else if ch_width(ch) >= 2 {
                // A wide char is its own break unit; a preceding narrow run stays whole
                // and the wide char glues onto it (no source whitespace between them).
                if cur.is_empty() {
                    units.push((vec![(ch, seg.style)], space_before));
                } else {
                    units.push((std::mem::take(&mut cur), space_before));
                    units.push((vec![(ch, seg.style)], false));
                }
                space_before = false;
            } else {
                cur.push((ch, seg.style));
            }
        }
    }
    if !cur.is_empty() {
        units.push((cur, space_before));
    }
    units
}

/// Greedy wrap over style-tagged [`Unit`]s with distinct first-line and continuation
/// prefixes. Units join with a space or glued per their `space_before` flag; a unit
/// wider than the whole line (an over-long narrow word) hard-splits on the
/// **display-width** boundary (CJK = 2 cells) so wide text is never truncated.
///
/// TODO(wide-char upgrade): a bespoke wrapper with per-CJK-char break opportunities and
/// display-width math — enough for CJK + common text. The mature codex/grok `textwrap`
/// 0.16 + UAX#14 + grapheme stack (punctuation-aware breaks, ZWJ/emoji, optimal-fit) is
/// planned in `docs/research/tui-text-wrapping-cjk.md`.
fn wrap_words(
    units: &[Unit],
    first_lead: &[Span<'static>],
    cont_lead: &[Span<'static>],
    width: usize,
) -> Vec<Line<'static>> {
    let lead_width =
        |lead: &[Span<'static>]| -> usize { lead.iter().map(|s| s.content.width()).sum() };
    let first_w = lead_width(first_lead);
    let cont_w = lead_width(cont_lead);
    let avail_for = |out: &[Line<'static>]| -> usize {
        width
            .saturating_sub(if out.is_empty() { first_w } else { cont_w })
            .max(1)
    };

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut line: Word = Vec::new();
    let mut line_w = 0usize; // display width of `line`
    for (chars, space_before) in units {
        // Try to append to the current line (with a joining space when the source had
        // whitespace here); if it doesn't fit, flush and continue on a fresh line.
        if !line.is_empty() {
            let avail = avail_for(&out);
            let sep = usize::from(*space_before);
            let unit_w = word_width(chars);
            if line_w + sep + unit_w <= avail {
                if sep == 1 {
                    line.push((' ', Style::default()));
                    line_w += 1;
                }
                line.extend_from_slice(chars);
                line_w += unit_w;
                continue;
            }
            let lead = if out.is_empty() {
                first_lead
            } else {
                cont_lead
            };
            out.push(build_line(lead, &std::mem::take(&mut line)));
            // `line` is now empty; the placement loop below resets `line_w`.
        }
        // The line is empty: place the unit, hard-splitting one wider than the whole
        // line (an over-long narrow word; a wide char is a single unit and always fits).
        let mut rest: &[(char, Style)] = chars;
        loop {
            let avail = avail_for(&out);
            let rest_w = word_width(rest);
            if rest_w <= avail {
                line.extend_from_slice(rest);
                line_w = rest_w;
                break;
            }
            let take = prefix_by_width(rest, avail);
            line.extend_from_slice(&rest[..take]);
            let lead = if out.is_empty() {
                first_lead
            } else {
                cont_lead
            };
            out.push(build_line(lead, &std::mem::take(&mut line)));
            rest = &rest[take..];
        }
    }
    if !line.is_empty() || out.is_empty() {
        let lead = if out.is_empty() {
            first_lead
        } else {
            cont_lead
        };
        out.push(build_line(lead, &line));
    }
    out
}

/// Coalesce adjacent same-style chars into spans, prefixed by `lead`.
fn build_line(lead: &[Span<'static>], chars: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = lead.to_vec();
    let mut cur = String::new();
    let mut cur_style: Option<Style> = None;
    for &(ch, st) in chars {
        if cur_style == Some(st) {
            cur.push(ch);
        } else {
            if let Some(s) = cur_style {
                spans.push(Span::styled(std::mem::take(&mut cur), s));
            }
            cur.push(ch);
            cur_style = Some(st);
        }
    }
    if let Some(s) = cur_style {
        spans.push(Span::styled(cur, s));
    }
    Line::from(spans)
}

/// Push a table cell's `line` spans into `out`, padded to `width` per `align`.
fn pad_into(out: &mut Vec<Span<'static>>, line: &Line<'static>, width: usize, align: Alignment) {
    let content: usize = line.spans.iter().map(|s| s.content.width()).sum();
    let pad = width.saturating_sub(content);
    let (left, right) = match align {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        Alignment::Left | Alignment::None => (0, pad),
    };
    if left > 0 {
        out.push(Span::raw(" ".repeat(left)));
    }
    out.extend(line.spans.iter().cloned());
    if right > 0 {
        out.push(Span::raw(" ".repeat(right)));
    }
}

/// A line with no visible content (used as a block separator).
fn last_is_blank(out: &[Line<'_>]) -> bool {
    out.last()
        .is_some_and(|l| l.spans.iter().all(|s| s.content.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }
    fn joined(lines: &[Line<'_>]) -> String {
        texts(lines).join("\n")
    }
    fn has_bold(line: &Line<'_>, needle: &str) -> bool {
        line.spans
            .iter()
            .any(|s| s.content.contains(needle) && s.style.add_modifier.contains(Modifier::BOLD))
    }

    #[test]
    fn heading_is_bold() {
        let lines = render("# Title", 40);
        assert!(
            lines.iter().any(|l| has_bold(l, "Title")),
            "{:?}",
            texts(&lines)
        );
    }

    #[test]
    fn list_items_are_bulleted_and_nested() {
        let md = "- one\n- two\n  - nested";
        let out = texts(&render(md, 40));
        assert!(out.iter().any(|l| l == "• one"), "{out:?}");
        assert!(out.iter().any(|l| l == "• two"), "{out:?}");
        assert!(out.iter().any(|l| l == "  • nested"), "{out:?}");
    }

    #[test]
    fn ordered_list_numbers() {
        let out = texts(&render("1. first\n2. second", 40));
        assert!(out.iter().any(|l| l == "1. first"), "{out:?}");
        assert!(out.iter().any(|l| l == "2. second"), "{out:?}");
    }

    #[test]
    fn fenced_code_block_is_indented_dim() {
        let md = "text\n\n```\nlet x = 1;\n```\n";
        let lines = render(md, 40);
        let code = lines
            .iter()
            .find(|l| l.to_string().contains("let x = 1;"))
            .expect("code line present");
        assert!(code.to_string().starts_with("    "), "indented: {code:?}");
        assert!(
            code.spans
                .iter()
                .all(|s| s.style.add_modifier.contains(Modifier::DIM)),
            "dim code: {code:?}"
        );
    }

    #[test]
    fn fenced_code_with_language_is_highlighted() {
        let md = "```rust\nfn main() { let x = 1; }\n```";
        let lines = render(md, 80);
        let code = lines
            .iter()
            .find(|l| l.to_string().contains("fn main"))
            .expect("code line present");
        assert!(code.to_string().starts_with("    "), "indented: {code:?}");
        // Highlighted: at least one span carries a color, and it is not the
        // all-dim plain fallback.
        assert!(
            code.spans.iter().any(|s| s.style.fg.is_some()),
            "colored: {code:?}"
        );
        assert!(
            !code
                .spans
                .iter()
                .all(|s| s.style.add_modifier.contains(Modifier::DIM)),
            "not the dim fallback: {code:?}"
        );
    }

    #[test]
    fn inline_code_and_bold_styled() {
        let lines = render("use `cargo` and **run** it", 40);
        assert!(joined(&lines).contains("use cargo and run it"));
        assert!(
            lines.iter().any(|l| has_bold(l, "run")),
            "{:?}",
            texts(&lines)
        );
    }

    #[test]
    fn paragraphs_word_wrap_to_width() {
        let lines = render("one two three four five six seven", 12);
        assert!(lines.len() > 2, "wrapped: {:?}", texts(&lines));
        assert!(
            lines.iter().all(|l| l.to_string().chars().count() <= 12),
            "{:?}",
            texts(&lines)
        );
    }

    #[test]
    fn cjk_paragraph_wraps_by_display_width_not_char_count() {
        // A space-less Chinese run: each han char is 2 cells, so width 10 holds at most
        // 5 chars. Char-count wrapping packed 10 (= 20 cells) → overflow → right-edge
        // truncation (the reported bug). Assert every line's DISPLAY width fits.
        let md = "这是一段没有空格的中文文本用来测试换行是否按显示宽度处理";
        let lines = render(md, 10);
        assert!(
            lines.len() > 1,
            "must wrap onto several lines: {:?}",
            texts(&lines)
        );
        for l in &lines {
            let w = l.to_string().width();
            assert!(
                w <= 10,
                "line over display width ({w}): {:?}",
                l.to_string()
            );
        }
        // Nothing dropped: every source char survives somewhere in the output.
        let joined = joined(&lines);
        for ch in md.chars() {
            assert!(joined.contains(ch), "char {ch:?} lost in wrapping");
        }
    }

    #[test]
    fn mixed_cjk_ascii_wraps_by_display_width() {
        // ASCII words break at spaces; the space-less CJK run hard-splits by width.
        let md = "sample dispatch append 的循环类型化的工具注册表以及统一契约";
        let lines = render(md, 16);
        for l in &lines {
            let w = l.to_string().width();
            assert!(
                w <= 16,
                "line over display width ({w}): {:?}",
                l.to_string()
            );
        }
    }

    #[test]
    fn cjk_fills_line_tails_no_early_break() {
        // A long space-less CJK run must FILL each line before wrapping (per-char break
        // opportunities) — not jump the whole run to the next line leaving a big gap (the
        // over-aggressive raggedness). At width 40, each line holds 20 han chars = 40 cells.
        let md = "这是一段没有任何空格的长中文文本用来验证换行会填满每一行的行尾而不是提前\
                  断开留下大片空白区域继续继续继续继续继续继续";
        let width = 40;
        let lines = render(md, width);
        assert!(lines.len() >= 2, "must wrap: {:?}", texts(&lines));
        for l in &lines[..lines.len() - 1] {
            let w = l.to_string().width();
            assert!(w <= width, "over width ({w}): {:?}", l.to_string());
            // Filled to within one wide char (2 cells) of the edge — no early break.
            assert!(
                w >= width - 1,
                "line under-filled ({w}/{width}): {:?}",
                l.to_string()
            );
        }
    }

    #[test]
    fn cjk_table_cells_align_by_display_width() {
        // A CJK-content table: columns are sized and padded by display width, so the
        // border glyphs line up (char-count padding would desync by a cell per han char).
        let md = "| 名称 | 说明 |\n| --- | --- |\n| 循环 | 采样调度追加 |";
        let lines = texts(&render(md, 40));
        // Every rendered table row has the same display width (borders aligned).
        let widths: Vec<usize> = lines
            .iter()
            .filter(|l| l.contains('│') || l.contains('├') || l.contains('┌') || l.contains('└'))
            .map(|l| l.width())
            .collect();
        assert!(!widths.is_empty(), "table rendered: {lines:?}");
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "all border rows equal display width: {widths:?}\n{lines:#?}"
        );
    }

    #[test]
    fn plain_text_is_unchanged() {
        let out = texts(&render("just a sentence", 40));
        assert_eq!(out, vec!["just a sentence"]);
    }

    // --- Regression tests for the bugs fixed in this slice ---

    #[test]
    fn inline_code_preserves_surrounding_spaces() {
        // The old renderer dropped the space *before* a code span and added a
        // spurious one *after* it (`from README.md` → `fromREADME.md `).
        let out = joined(&render("from `README.md` to `SPEC.md` now", 80));
        assert_eq!(out, "from README.md to SPEC.md now", "got: {out:?}");
    }

    #[test]
    fn code_span_then_suffix_has_no_gap() {
        // A suffix touching a code span (`` `Event`s ``) must not gain a space:
        // the old renderer produced `Event s`. The space before the span is
        // real (from `streaming `) and must be kept.
        let out = joined(&render("streaming `Event`s here", 80));
        assert_eq!(out, "streaming Events here", "got: {out:?}");
    }

    #[test]
    fn list_item_starting_with_code_keeps_marker_first() {
        // A code span at the item start must not defer the marker mid-line.
        let out = texts(&render("1. `run.rs` is the loop", 80));
        let item = out
            .iter()
            .find(|l| l.contains("run.rs"))
            .expect("item present");
        assert!(item.starts_with("1. run.rs"), "got: {item:?}");
    }

    #[test]
    fn wrapped_list_item_has_hanging_indent() {
        let out = texts(&render("- alpha beta gamma delta epsilon", 14));
        // First line carries the bullet; continuation aligns under the text.
        assert!(out[0].starts_with("• alpha"), "{out:?}");
        assert!(
            out.iter().skip(1).any(|l| l.starts_with("  ")),
            "continuation indented: {out:?}"
        );
    }

    #[test]
    fn blocks_separated_by_blank_line() {
        let out = texts(&render("# Heading\n\npara one\n\npara two", 80));
        assert!(
            out.iter().any(String::is_empty),
            "has a blank line: {out:?}"
        );
        // No leading/trailing blank.
        assert!(!out.first().unwrap().is_empty(), "{out:?}");
        assert!(!out.last().unwrap().is_empty(), "{out:?}");
    }

    #[test]
    fn table_renders_bordered_columns_with_header_rule() {
        let md = "| Crate | Role |\n|---|---|\n| proto | pure types |\n| tools | the registry |";
        let out = texts(&render(md, 40));
        // No raw ASCII pipes survive (we draw box-drawing borders instead).
        assert!(
            !out.iter().any(|l| l.contains('|')),
            "no raw pipes: {out:?}"
        );
        // Top border, header row, a ├─┼─┤ rule, a bottom border.
        assert!(out.iter().any(|l| l.starts_with('┌')), "top: {out:?}");
        let header = out
            .iter()
            .position(|l| l.contains("Crate"))
            .expect("header");
        assert!(
            out[header].contains("Role") && out[header].starts_with('│'),
            "{out:?}"
        );
        assert!(out[header + 1].starts_with('├'), "header rule: {out:?}");
        assert!(out.iter().any(|l| l.starts_with('└')), "bottom: {out:?}");
        assert!(
            out.iter()
                .any(|l| l.contains("proto") && l.contains("pure types")),
            "{out:?}"
        );
    }

    #[test]
    fn table_header_cells_are_bold() {
        let lines = render("| A | B |\n|---|---|\n| x | y |", 40);
        assert!(
            lines.iter().any(|l| has_bold(l, "A")),
            "{:?}",
            texts(&lines)
        );
    }

    #[test]
    fn narrow_table_wraps_without_panic_or_overflow() {
        let md = "| Column one heading | Column two heading |\n|---|---|\n| some longish value | another long value |";
        let out = render(md, 20);
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|l| l.to_string().chars().count() <= 20),
            "fits width: {:?}",
            texts(&out)
        );
    }

    #[test]
    fn strikethrough_is_styled() {
        let lines = render("this is ~~gone~~ text", 80);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("gone")
                    && s.style.add_modifier.contains(Modifier::CROSSED_OUT))),
            "{:?}",
            texts(&lines)
        );
    }
}
