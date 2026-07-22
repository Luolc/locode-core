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
/// One table cell's content, tokenized into styled words.
type CellWords = Vec<Word>;

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

    /// Lay out the accumulated table as aligned columns: natural column widths
    /// shrunk proportionally to fit, cells wrapped, header bold with a dim rule
    /// under it. No box borders (Claude-Code-style clean columns).
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
                let words = row.get(c).map(|s| segs_to_words(s)).unwrap_or_default();
                let natural =
                    words.iter().map(Vec::len).sum::<usize>() + words.len().saturating_sub(1);
                *natural_w = (*natural_w).max(natural);
                rw.push(words);
            }
            cells.push(rw);
        }

        // Fit: use natural widths if they fit, else shrink proportionally (min 3).
        let sep = 2usize;
        let overhead = sep * (n_cols - 1);
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
        let total: usize = col_width.iter().sum::<usize>() + overhead;

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
            let empty = Line::from("");
            for r in 0..height {
                let mut spans: Vec<Span<'static>> = Vec::new();
                for c in 0..n_cols {
                    if c > 0 {
                        spans.push(Span::raw("  "));
                    }
                    let line = cell_lines[c].get(r).unwrap_or(&empty);
                    let align = aligns.get(c).copied().unwrap_or(Alignment::None);
                    pad_into(&mut spans, line, col_width[c], align);
                }
                self.out.push(Line::from(spans));
            }
            if is_head && ri + 1 == head_rows {
                self.out.push(Line::styled(
                    "─".repeat(total),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        }
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
                let width = marker.chars().count();
                first.push(Span::raw(marker));
                cont.push(Span::raw(" ".repeat(width)));
            } else {
                // A continuation paragraph inside an item aligns under the text.
                let width = indent.chars().count() + 2;
                first.push(Span::raw(" ".repeat(width)));
                cont.push(Span::raw(" ".repeat(width)));
            }
        }
        (first, cont)
    }

    /// Wrap the accumulated inline segments into `out`, then clear them.
    fn flush_inline(&mut self) {
        let words = segs_to_words(&self.inline);
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

/// Split inline segments into style-tagged words, collapsing runs of whitespace
/// and trimming block leading/trailing space. Each word is a run of non-space
/// chars that may carry mixed styles (e.g. `` streaming`Event` `` with no space).
fn segs_to_words(segs: &[Seg]) -> Vec<Vec<(char, Style)>> {
    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    for seg in segs {
        for ch in seg.text.chars() {
            if ch.is_whitespace() {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push((ch, seg.style));
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Greedy word-wrap over style-tagged words with distinct first-line and
/// continuation prefixes. Words wider than the available width hard-split.
fn wrap_words(
    words: &[Vec<(char, Style)>],
    first_lead: &[Span<'static>],
    cont_lead: &[Span<'static>],
    width: usize,
) -> Vec<Line<'static>> {
    let lead_width =
        |lead: &[Span<'static>]| -> usize { lead.iter().map(|s| s.content.chars().count()).sum() };
    let first_w = lead_width(first_lead);
    let cont_w = lead_width(cont_lead);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut line: Vec<(char, Style)> = Vec::new();
    for word in words {
        let mut rest: &[(char, Style)] = word;
        loop {
            let is_first = out.is_empty();
            let avail = width
                .saturating_sub(if is_first { first_w } else { cont_w })
                .max(1);
            if line.is_empty() {
                if rest.len() <= avail {
                    line.extend_from_slice(rest);
                    break;
                }
                // Over-long word: hard-split at the width boundary.
                line.extend_from_slice(&rest[..avail]);
                let lead = if is_first { first_lead } else { cont_lead };
                out.push(build_line(lead, &std::mem::take(&mut line)));
                rest = &rest[avail..];
            } else if line.len() + 1 + rest.len() <= avail {
                line.push((' ', Style::default()));
                line.extend_from_slice(rest);
                break;
            } else {
                let lead = if is_first { first_lead } else { cont_lead };
                out.push(build_line(lead, &std::mem::take(&mut line)));
                // Retry placing the whole word on a fresh line.
            }
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
    let content: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
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
    fn table_renders_aligned_columns_with_header_rule() {
        let md = "| Crate | Role |\n|---|---|\n| proto | pure types |\n| tools | the registry |";
        let out = texts(&render(md, 40));
        // No raw pipe rows survive.
        assert!(
            !out.iter().any(|l| l.contains('|')),
            "no raw pipes: {out:?}"
        );
        // Header cells present and a dim rule under them.
        let header = out
            .iter()
            .position(|l| l.contains("Crate"))
            .expect("header");
        assert!(out[header].contains("Role"), "{out:?}");
        assert!(out[header + 1].starts_with("──"), "header rule: {out:?}");
        // Body cells laid out in columns (aligned: "proto" then padding then "pure").
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
