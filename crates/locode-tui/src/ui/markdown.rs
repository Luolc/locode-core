//! Minimal markdown → styled `Line`s for assistant text (slice 5b).
//!
//! A pulldown-cmark pass covering the constructs an agent actually emits:
//! headings (bold), lists (bulleted, nested-indented), fenced/inline code
//! (dim + code-block indent), block quotes, and inline bold/italic. **No
//! syntect** (SPEC-TUI non-goal) — code is styled dim, not highlighted. The
//! pattern (not the styling depth) follows codex's `markdown_render.rs`.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render markdown `text` to word-wrapped styled lines at `width`.
#[must_use]
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut w = Writer::new(width);
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    for event in parser {
        w.event(event);
    }
    w.finish()
}

/// Accumulates styled spans into wrapped lines, tracking inline style and the
/// current block context (list depth, code block, quote).
struct Writer {
    width: usize,
    out: Vec<Line<'static>>,
    /// Spans of the line currently being built.
    current: Vec<Span<'static>>,
    /// Inline style modifiers to apply to text spans.
    bold: u32,
    italic: u32,
    /// `Some(prefix)` while inside a code block (dim, no wrap).
    in_code_block: bool,
    /// List item markers by nesting depth (`None` = bullet, `Some(n)` = ordered).
    list_stack: Vec<Option<u64>>,
    /// Pending list-item marker to emit at the next text.
    pending_marker: Option<String>,
    quote_depth: u32,
}

impl Writer {
    fn new(width: usize) -> Self {
        Self {
            width: width.max(4),
            out: Vec::new(),
            current: Vec::new(),
            bold: 0,
            italic: 0,
            in_code_block: false,
            list_stack: Vec::new(),
            pending_marker: None,
            quote_depth: 0,
        }
    }

    fn inline_style(&self) -> Style {
        let mut s = Style::default();
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        s
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(&tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(c) => {
                self.push_span(Span::styled(
                    c.into_string(),
                    Style::default().fg(Color::Cyan),
                ));
            }
            Event::SoftBreak | Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_line();
                self.out.push(Line::styled(
                    "───",
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: &Tag<'_>) {
        match tag {
            Tag::Heading { .. } => {
                self.flush_line();
                self.bold += 1;
            }
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::CodeBlock(_) => {
                self.flush_line();
                self.in_code_block = true;
            }
            Tag::List(first) => self.list_stack.push(*first),
            Tag::Item => {
                self.flush_line();
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
                self.pending_marker = Some(marker);
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.quote_depth += 1;
            }
            Tag::Paragraph => self.flush_line(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(level) => {
                self.flush_line();
                self.bold = self.bold.saturating_sub(1);
                // A blank line after H1/H2 for breathing room.
                if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                    self.out.push(Line::from(""));
                }
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::CodeBlock => {
                self.flush_line();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item | TagEnd::Paragraph => self.flush_line(),
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if self.in_code_block {
            // Code blocks: dim, indented, no word-wrap (preserve lines). Emit
            // one dim SPAN per source line so styling lives on the span
            // (consistent with inline styling; line-level styles are avoided).
            let dim = Style::default().add_modifier(Modifier::DIM);
            for raw in t.split_inclusive('\n') {
                let line = raw.strip_suffix('\n').unwrap_or(raw);
                self.push_span(Span::styled(format!("    {line}"), dim));
                if raw.ends_with('\n') {
                    self.flush_line();
                }
            }
            return;
        }
        // Word-wrap normal text, applying the current inline style.
        let style = self.inline_style();
        for word in t.split_whitespace() {
            let cur_len: usize = self.line_len();
            if cur_len > 0 && cur_len + 1 + word.chars().count() > self.width {
                self.flush_line();
            }
            if self.line_len() > 0 {
                self.push_span(Span::raw(" "));
            } else {
                self.emit_line_prefix();
            }
            self.push_span(Span::styled(word.to_owned(), style));
        }
    }

    /// Marker/quote prefix at the start of a fresh line.
    fn emit_line_prefix(&mut self) {
        if let Some(marker) = self.pending_marker.take() {
            self.current
                .push(Span::styled(marker, Style::default().fg(Color::Yellow)));
        }
        for _ in 0..self.quote_depth {
            self.current.push(Span::styled(
                "┃ ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
    }

    fn push_span(&mut self, span: Span<'static>) {
        self.current.push(span);
    }

    fn line_len(&self) -> usize {
        self.current.iter().map(|s| s.content.chars().count()).sum()
    }

    fn flush_line(&mut self) {
        if !self.current.is_empty() {
            self.out.push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        // Drop a trailing empty line if the source ended with a block break.
        while matches!(self.out.last(), Some(l) if l.spans.is_empty()) {
            self.out.pop();
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
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
        assert!(out.iter().any(|l| l.contains("• one")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("• two")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("  • nested")), "{out:?}");
    }

    #[test]
    fn ordered_list_numbers() {
        let out = texts(&render("1. first\n2. second", 40));
        assert!(out.iter().any(|l| l.contains("1. first")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("2. second")), "{out:?}");
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
    fn inline_code_and_bold_styled() {
        let lines = render("use `cargo` and **run** it", 40);
        let joined: String = texts(&lines).join(" ");
        assert!(joined.contains("cargo"));
        assert!(joined.contains("run"));
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
}
