//! Transcript blocks: typed entries that render once into native scrollback
//! (SPEC-TUI rendering model; grok's `RenderBlock` enum + codex's
//! `HistoryCell` at v1 scale). Each block owns its source text so the
//! reflow-from-source extension stays possible.

use locode_core::{Report, Status};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Terminal display width of a char in cells (CJK/emoji = 2, zero-width = 0). Wrapping
/// must measure cells, not char count, or CJK overflows and is truncated on the right.
fn ch_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

/// Rows of tool-result body kept before head/tail truncation kicks in
/// (codex keeps 5 for agent commands; `exec_cell/render.rs:33`).
const TOOL_BODY_MAX_LINES: usize = 6;

/// Left gutter width for the message bullet (`● `) and hanging indent.
const GUTTER: usize = 2;

/// Uniform left/right margin for transcript content (columns of blank space so
/// nothing hugs the terminal edge). User request 2026-07-22.
const MARGIN: u16 = 2;

/// Background of the user-prompt band (grok's `RenderBlock::UserPrompt`
/// full-pane fill; codex's `user_message_bg`). We follow the terminal theme
/// rather than a hard RGB: `Color::DarkGray` is the ANSI bright-black palette
/// slot, so the band renders as "one step above the background" under whatever
/// theme the user runs — the same palette-relative trick the code highlighter
/// uses (`ui/highlight.rs`). When a real color-theme system lands we can swap
/// this for a dedicated band color or codex's terminal-bg blend.
const BAND_BG: Color = Color::DarkGray;

/// Prepend the left margin to a rendered line, preserving its line-level style.
fn with_left_margin(line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(" ".repeat(MARGIN as usize)));
    spans.extend(line.spans);
    let mut out = Line::from(spans);
    out.style = line.style;
    out
}

/// One finalized transcript entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// The user's submitted prompt (echoed by the UI at submit time).
    UserPrompt(String),
    /// A complete assistant text message.
    AssistantText(String),
    /// A finalized tool call (result paired).
    ToolCall {
        /// Client-facing tool name.
        name: String,
        /// One-line argument summary.
        args: String,
        /// Whether the paired result was an error.
        is_error: bool,
        /// The model-facing result text (truncated at render).
        body: String,
    },
    /// The per-run separator, from the run's `Report`.
    TurnEnd {
        /// Terminal status (`completed`, `cancelled`, …).
        status: Status,
        /// Turns this run took.
        turns: u32,
        /// Input+output tokens this run.
        tokens: u64,
        /// Wall-clock seconds (measured UI-side).
        elapsed_secs: u64,
    },
    /// A non-terminal note (engine retry notes, build failures, app info).
    Notice(String),
}

impl Block {
    /// Render to pre-wrapped lines at `width` for the transcript tail, with a
    /// uniform left/right margin so content doesn't hug the terminal edge. Every
    /// block's `● `/`❯ ` prefix then sits at the margin, so text aligns at
    /// `MARGIN + 2` (the same column as the composer's input text).
    #[must_use]
    pub fn render(&self, width: u16) -> Vec<Line<'static>> {
        // The user prompt is a full-bleed shaded band (edge-to-edge bg fill), so
        // it bypasses the inset-margin path the other blocks share.
        if let Block::UserPrompt(text) = self {
            return render_user_prompt(text, width);
        }
        let inner = width.saturating_sub(2 * MARGIN);
        self.render_inner(inner)
            .into_iter()
            .map(with_left_margin)
            .collect()
    }

    fn render_inner(&self, width: u16) -> Vec<Line<'static>> {
        let dim = Style::default().add_modifier(Modifier::DIM);
        match self {
            // Rendered by `render` as a full-width band (never reaches here).
            Block::UserPrompt(_) => unreachable!("UserPrompt renders as a band"),
            Block::AssistantText(text) => {
                // A leading ● bullet on the first line, the rest hanging-indented
                // under it (Claude Code's message hierarchy). The 2-col gutter is
                // the left margin; keep a right margin too so prose doesn't hug
                // either edge.
                let content_width = usize::from(width).saturating_sub(GUTTER).max(4);
                let mut lines = vec![Line::from("")];
                for (i, line) in crate::ui::markdown::render(text, content_width)
                    .into_iter()
                    .enumerate()
                {
                    let lead = if i == 0 {
                        Span::styled("● ", Style::default().fg(Color::White))
                    } else {
                        Span::raw("  ")
                    };
                    let mut spans = Vec::with_capacity(line.spans.len() + 1);
                    spans.push(lead);
                    spans.extend(line.spans);
                    lines.push(Line::from(spans));
                }
                lines
            }
            Block::ToolCall {
                name,
                args,
                is_error,
                body,
            } => {
                let bullet_style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                };
                // Leading blank line so blocks don't squeeze together (matches
                // UserPrompt/AssistantText — one blank before every block).
                let mut lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("● ", bullet_style),
                        Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(" "),
                        Span::styled(args.clone(), dim),
                    ]),
                ];
                lines.extend(truncated_body(body, width));
                lines
            }
            Block::TurnEnd {
                status,
                turns,
                tokens,
                elapsed_secs,
            } => {
                // Subtle dim text, NOT a full-width rule — a `──…──` bar stacked
                // with the composer's top rule read as a redundant "extra rule"
                // (user vibe-check, 2026-07-22). Aligned to the message gutter.
                let label = format!(
                    "{} · {turns} turn{} · {tokens} tokens · {}",
                    status_str(*status),
                    if *turns == 1 { "" } else { "s" },
                    fmt_elapsed(*elapsed_secs),
                );
                vec![Line::from(""), Line::styled(format!("  {label}"), dim)]
            }
            Block::Notice(text) => {
                vec![Line::from(""), Line::styled(format!("● {text}"), dim)]
            }
        }
    }
}

/// Render the user prompt as a full-width shaded band (grok's
/// `RenderBlock::UserPrompt`; codex's `UserHistoryCell`): a leading unshaded
/// separator, one blank shaded row (vpad), the `❯ `-prefixed wrapped text, and
/// one closing blank shaded row. Every band row is padded with spaces to the
/// full `width` so the [`BAND_BG`] fill spans edge-to-edge (ratatui only styles
/// the cells a span covers — the pad is what carries the bg to the right edge).
/// Text stays at column 4 (2-col margin + `❯ `), aligning with the assistant
/// bullet and the composer input; a 2-col right margin keeps prose off the
/// band's right edge. No timestamp in v1 (grok's is off by default).
fn render_user_prompt(text: &str, width: u16) -> Vec<Line<'static>> {
    let band = Style::default().bg(BAND_BG);
    let total = usize::from(width);
    // Left inset = MARGIN + `❯ ` (= col 4); right inset = MARGIN. What's left is
    // the wrap width for the prompt text.
    let content_width = total
        .saturating_sub(usize::from(MARGIN) * 2 + GUTTER)
        .max(4);

    let band_row = |body: String| -> Line<'static> {
        let pad = total.saturating_sub(body.chars().count());
        let mut spans = vec![Span::styled(body, band)];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), band));
        }
        Line::from(spans)
    };

    // Leading unshaded separator + top vpad, then content, then bottom vpad.
    let mut lines = vec![Line::from(""), band_row(String::new())];
    for (i, physical) in wrap_plain(text, content_width).into_iter().enumerate() {
        let lead = if i == 0 { "  ❯ " } else { "    " };
        lines.push(band_row(format!("{lead}{physical}")));
    }
    lines.push(band_row(String::new()));
    lines
}

/// Greedy word-wrap of plain text to `width` **display columns** (CJK = 2 cells each),
/// preserving the user's hard line breaks (`\n`) and hard-splitting any run wider than
/// `width` on the display-width boundary. Runs of intra-line whitespace collapse to
/// single spaces (this is an echo, not an editor).
///
/// TODO(wide-char upgrade): surgical display-width fix; the comprehensive
/// textwrap/UAX#14/grapheme upgrade is tracked in `docs/research/tui-text-wrapping-cjk.md`.
fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    // Display width of a slice of chars (CJK = 2 cells each).
    let run_width = |cs: &[char]| -> usize { cs.iter().map(|&c| ch_width(c)).sum() };
    let mut out = Vec::new();
    for src in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0usize; // display width of `cur`
        let flush = |cur: &mut String, cur_w: &mut usize, out: &mut Vec<String>| {
            out.push(std::mem::take(cur));
            *cur_w = 0;
        };
        for word in src.split_whitespace() {
            let mut word: Vec<char> = word.chars().collect();
            // Hard-split a run wider than the whole width (e.g. a space-less CJK
            // sentence) on the display-width boundary — at least one char per slice.
            while run_width(&word) > width {
                if cur_w > 0 {
                    flush(&mut cur, &mut cur_w, &mut out);
                }
                let mut take = 0usize;
                let mut cols = 0usize;
                for &c in &word {
                    let w = ch_width(c);
                    if take >= 1 && cols + w > width {
                        break;
                    }
                    cols += w;
                    take += 1;
                }
                out.push(word[..take].iter().collect());
                word.drain(..take);
            }
            let wlen = run_width(&word);
            if cur_w == 0 {
                cur.extend(&word);
                cur_w = wlen;
            } else if cur_w + 1 + wlen <= width {
                cur.push(' ');
                cur.extend(&word);
                cur_w += 1 + wlen;
            } else {
                flush(&mut cur, &mut cur_w, &mut out);
                cur.extend(&word);
                cur_w = wlen;
            }
        }
        out.push(cur);
    }
    out
}

/// Render one chunk of an assistant message as markdown (ADR-0021 progressive
/// streaming): a leading gap line, then the markdown lines with a `●` bullet on
/// the first line **iff `first`** (else a 2-col hanging indent) — identical to how
/// [`Block::AssistantText`] renders when `first` is true, so a chunk committed
/// mid-stream is pixel-identical to the same text rendered whole. Streaming
/// commits completed blocks as `first=true` (the message's first block) then
/// `first=false` continuations; the finalized whole message renders the same.
#[must_use]
pub fn render_assistant_chunk(text: &str, width: u16, first: bool) -> Vec<Line<'static>> {
    let content_width = usize::from(width.saturating_sub(2 * MARGIN))
        .saturating_sub(GUTTER)
        .max(4);
    let mut lines = vec![Line::from("")];
    for (i, line) in crate::ui::markdown::render(text, content_width)
        .into_iter()
        .enumerate()
    {
        let lead = if i == 0 && first {
            Span::styled("● ", Style::default().fg(Color::White))
        } else {
            Span::raw("  ")
        };
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(lead);
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
    lines.into_iter().map(with_left_margin).collect()
}

/// The byte offset up to which `buffer` can be **safely committed** to scrollback
/// as completed markdown blocks (ADR-0021): the position after the last blank
/// line that is **not inside an open code fence**. A blank line inside a fenced
/// code block must *not* split it — committing a half-fence renders broken (an
/// unclosed fence) and can't be un-committed. Everything before the offset is
/// stable (won't reflow as more text streams); after it is the in-progress block.
#[must_use]
pub fn stable_prefix_end(buffer: &str) -> usize {
    let mut in_fence = false;
    let mut safe = 0usize;
    let mut offset = 0usize;
    for line in buffer.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if content.trim().is_empty() && !in_fence {
            safe = offset + line.len();
        }
        offset += line.len();
    }
    safe
}

/// Render the **in-progress** (uncommitted) block of a streaming message as a
/// live cell — [`render_assistant_chunk`] capped to the last `max_rows` rows so a
/// single huge block (e.g. a long open code fence) fills the screen showing its
/// tail rather than overflowing. `first` places the message bullet iff no earlier
/// block has committed yet. Completed blocks are committed to scrollback by the
/// loop (`fold_streaming`), so they stay scrollable mid-stream.
#[must_use]
pub fn render_streaming(
    buffer: &str,
    width: u16,
    max_rows: usize,
    first: bool,
) -> Vec<Line<'static>> {
    if buffer.is_empty() || max_rows == 0 {
        return Vec::new();
    }
    let lines = render_assistant_chunk(buffer, width, first);
    let start = lines.len().saturating_sub(max_rows);
    lines[start..].to_vec()
}

/// Tool body, head/tail-kept with a middle marker past the cap (codex's
/// shape: `… +N lines`), each line dimmed and indented.
fn truncated_body(body: &str, width: u16) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let all: Vec<&str> = body.lines().collect();
    let mut out = Vec::new();
    let indent = "  ";
    let push = |out: &mut Vec<Line<'static>>, l: &str| {
        let max = usize::from(width).saturating_sub(indent.len()).max(4);
        let mut line: String = l.to_string();
        if line.chars().count() > max {
            line = line.chars().take(max.saturating_sub(1)).collect::<String>() + "…";
        }
        out.push(Line::styled(format!("{indent}{line}"), dim));
    };
    if all.len() <= TOOL_BODY_MAX_LINES {
        for l in &all {
            push(&mut out, l);
        }
        return out;
    }
    let head = TOOL_BODY_MAX_LINES / 2;
    let tail = TOOL_BODY_MAX_LINES - head - 1;
    for l in &all[..head] {
        push(&mut out, l);
    }
    out.push(Line::styled(
        format!("{indent}… +{} lines", all.len() - head - tail),
        dim,
    ));
    for l in &all[all.len() - tail..] {
        push(&mut out, l);
    }
    out
}

/// The wire string for a status (mirrors the envelope's `snake_case` values).
fn status_str(status: Status) -> &'static str {
    match status {
        Status::Completed => "completed",
        Status::MaxTurns => "max_turns",
        Status::ModelError => "model_error",
        Status::Error => "error",
        Status::Cancelled => "cancelled",
        _ => "unknown",
    }
}

/// Human-readable elapsed time for the turn-end summary: `20s`, `1m 20s`,
/// `1h 2m 3s` — never a bare `1234s`. Minutes/hours are shown only once the
/// coarser unit is non-zero.
fn fmt_elapsed(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Build the `TurnEnd` block from a run's report + UI-measured elapsed time.
#[must_use]
pub fn turn_end(report: &Report, elapsed_secs: u64) -> Block {
    Block::TurnEnd {
        status: report.status,
        turns: report.turns,
        tokens: report.usage.input_tokens + report.usage.output_tokens,
        elapsed_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn wrap_plain_cjk_respects_display_width() {
        // A space-less Chinese prompt: han chars are 2 cells. Char-count wrapping
        // overflowed and the frame clipped the right side (the reported bug).
        let out = wrap_plain("这是一段用来测试用户输入换行的中文提示没有空格", 8);
        assert!(out.len() > 1, "must wrap: {out:?}");
        for line in &out {
            assert!(
                line.width() <= 8,
                "line over display width: {line:?} ({})",
                line.width()
            );
        }
    }

    #[test]
    fn wrap_plain_ascii_unchanged() {
        // Regression: ASCII behavior (char count == display width) is identical.
        let out = wrap_plain("one two three four five", 9);
        assert!(out.iter().all(|l| l.width() <= 9), "{out:?}");
    }

    #[test]
    fn user_prompt_renders_as_full_width_shaded_band() {
        let block = Block::UserPrompt("a\nb".into());
        let lines = block.render(40);
        let s = text_of(&lines);
        // Unshaded separator, top vpad, two content rows, bottom vpad.
        assert_eq!(s.len(), 5, "{s:?}");
        assert_eq!(s[0], "", "leading separator is unshaded");
        assert!(s[1].trim().is_empty(), "top vpad row: {:?}", s[1]);
        // `❯` sits at col 2, text at col 4 (2-col margin + prefix).
        assert!(s[2].starts_with("  ❯ a"), "{:?}", s[2]);
        assert!(
            s[3].starts_with("    b"),
            "continuation indents to col 4: {:?}",
            s[3]
        );
        assert!(s[4].trim().is_empty(), "bottom vpad row: {:?}", s[4]);
        // Every band row is padded to the full width so the fill spans edge-to-edge.
        for row in &s[1..] {
            assert_eq!(row.chars().count(), 40, "band row full width: {row:?}");
        }
        // The band carries the theme-relative background on its shaded rows.
        assert_eq!(lines[2].spans[0].style.bg, Some(BAND_BG), "content row bg");
        assert_eq!(lines[1].spans[0].style.bg, Some(BAND_BG), "vpad row bg");
        assert_eq!(
            lines[0].spans.first().and_then(|s| s.style.bg),
            None,
            "separator unshaded"
        );
    }

    #[test]
    fn user_prompt_wraps_long_lines_within_the_band() {
        let block = Block::UserPrompt("one two three four five six seven".into());
        let lines = block.render(20);
        let s = text_of(&lines);
        // Content wraps to width; every band row stays exactly 20 wide.
        for row in &s[1..] {
            assert_eq!(row.chars().count(), 20, "band row full width: {row:?}");
        }
        // More than one content row was produced (it wrapped).
        let content_rows = s[2..s.len() - 1].len();
        assert!(content_rows > 1, "should wrap: {s:?}");
        assert!(
            s[2].starts_with("  ❯ "),
            "first row keeps the prompt prefix: {:?}",
            s[2]
        );
    }

    #[test]
    fn tool_call_truncates_long_bodies_head_tail() {
        let body = (1..=20)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = text_of(
            &Block::ToolCall {
                name: "run_terminal_cmd".into(),
                args: "seq 20".into(),
                is_error: false,
                body,
            }
            .render(60),
        );
        assert_eq!(lines[0].trim(), "", "leading blank between blocks");
        assert!(lines[1].contains("run_terminal_cmd"));
        assert!(lines.iter().any(|l| l.contains("line-1")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("+15 lines")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("line-20")), "{lines:?}");
        // 1 blank + 1 header + 6 body rows (3 head + marker + 2 tail).
        assert_eq!(lines.len(), 8, "{lines:?}");
    }

    #[test]
    fn turn_end_is_subtle_dim_text_not_a_rule() {
        let lines = text_of(
            &Block::TurnEnd {
                status: Status::Completed,
                turns: 3,
                tokens: 1234,
                elapsed_secs: 41,
            }
            .render(60),
        );
        // Leading blank between blocks, then the one dim summary line.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].trim(), "");
        assert!(
            lines[1].contains("completed · 3 turns · 1234 tokens · 41s"),
            "{lines:?}"
        );
        // No longer a full-width `──…──` rule (it read as an extra rule).
        assert!(!lines[1].contains("──"), "should not be a rule: {lines:?}");
    }

    #[test]
    fn elapsed_reads_as_minutes_and_hours_never_a_bare_seconds_count() {
        assert_eq!(fmt_elapsed(20), "20s");
        assert_eq!(fmt_elapsed(59), "59s");
        assert_eq!(fmt_elapsed(60), "1m 0s");
        assert_eq!(fmt_elapsed(80), "1m 20s");
        assert_eq!(fmt_elapsed(1234), "20m 34s");
        assert_eq!(fmt_elapsed(3661), "1h 1m 1s");
    }

    #[test]
    fn assistant_text_has_bullet_and_hanging_indent() {
        let lines = Block::AssistantText("first line here".into()).render(40);
        // Leading blank, then the bulleted first content line.
        let first = lines
            .iter()
            .find(|l| l.to_string().contains("first"))
            .expect("content line");
        // 2-col global margin, then the ● bullet.
        assert!(
            first.to_string().starts_with("  ● "),
            "{:?}",
            text_of(&lines)
        );
    }

    #[test]
    fn assistant_text_wraps_to_width() {
        let lines = text_of(&Block::AssistantText("one two three four five".into()).render(16));
        assert!(lines.len() > 2, "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 16), "{lines:?}");
    }

    #[test]
    fn render_streaming_empty_or_zero_rows_is_no_rows() {
        assert!(render_streaming("", 40, 20, true).is_empty());
        // max_rows == 0 (a too-short screen) also renders nothing.
        assert!(render_streaming("something", 40, 0, true).is_empty());
    }

    #[test]
    fn render_streaming_bullet_only_when_first() {
        // A leading gap line, then the bulleted first content row (`first = true`).
        let first = text_of(&render_streaming("first line here", 40, 20, true));
        assert_eq!(first[0].trim(), "", "leading gap line");
        assert!(first[1].starts_with("  ● first"), "{first:?}");
        // A continuation chunk (`first = false`) hangs under, no bullet.
        let cont = text_of(&render_streaming("more text", 40, 20, false));
        assert!(!cont.iter().any(|l| l.contains('●')), "no bullet: {cont:?}");
        // 2-col margin + 2-col hanging indent = text at col 4, no bullet.
        assert!(cont[1].starts_with("    more"), "hanging indent: {cont:?}");
    }

    #[test]
    fn render_streaming_wraps_to_width() {
        let lines = render_streaming("one two three four five six seven", 18, 20, true);
        assert!(lines.len() > 1, "should wrap");
        assert!(
            text_of(&lines).iter().all(|l| l.chars().count() <= 18),
            "{:?}",
            text_of(&lines)
        );
    }

    #[test]
    fn render_streaming_caps_to_max_rows_and_scrolls_the_tail() {
        // A markdown list → one rendered line per item + a leading gap line.
        let buffer = (0..40)
            .map(|i| format!("- item{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let full = text_of(&render_streaming(&buffer, 40, 100, true));
        assert_eq!(full.len(), 41, "leading gap + 40 items, uncapped");
        assert!(full.last().unwrap().contains("item39"), "{full:?}");
        // Capped to 8 → the last 8 rows (the tail), no bullet (scrolled past it).
        let capped = text_of(&render_streaming(&buffer, 40, 8, true));
        assert_eq!(capped.len(), 8, "capped to max_rows");
        assert!(
            !capped[0].contains('●'),
            "no bullet once scrolled: {capped:?}"
        );
        assert!(
            capped.last().unwrap().contains("item39"),
            "tail: {capped:?}"
        );
    }

    #[test]
    fn render_streaming_applies_markdown_formatting() {
        // A bold span proves the live cell renders markdown (not plain text).
        let lines = render_streaming("this is **bold** now", 40, 20, true);
        let has_bold = lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.content.contains("bold") && s.style.add_modifier.contains(Modifier::BOLD));
        assert!(has_bold, "streamed markdown should bold **bold**");
    }

    #[test]
    fn stable_prefix_end_commits_completed_blocks() {
        // Everything up to the last blank line (block boundary).
        assert_eq!(stable_prefix_end("block1\n\nblock2"), "block1\n\n".len());
        // No blank line yet → nothing stable.
        assert_eq!(stable_prefix_end("still typing the first line"), 0);
        // Multiple blocks: commit up to the last blank.
        let s = "a\n\nb\n\nc";
        assert_eq!(stable_prefix_end(s), "a\n\nb\n\n".len());
    }

    #[test]
    fn stable_prefix_end_never_splits_an_open_code_fence() {
        // A blank line INSIDE an open fence must not be a commit boundary.
        let open = "para\n\n```rust\nfn a() {}\n\nfn b() {}";
        assert_eq!(
            stable_prefix_end(open),
            "para\n\n".len(),
            "commit only the paragraph, keep the open fence live"
        );
        // Once the fence CLOSES and a blank follows, the whole fence is stable.
        let closed = "```rust\nfn a() {}\n\nfn b() {}\n```\n\ndone";
        let end = stable_prefix_end(closed);
        assert_eq!(end, "```rust\nfn a() {}\n\nfn b() {}\n```\n\n".len());
        assert_eq!(&closed[end..], "done");
    }

    #[test]
    fn assistant_chunk_matches_assistant_text_when_first() {
        // A committed first chunk is pixel-identical to the same text as a block.
        let chunk = render_assistant_chunk("hello **world**", 40, true);
        let block = Block::AssistantText("hello **world**".into()).render(40);
        assert_eq!(
            text_of(&chunk),
            text_of(&block),
            "first chunk == AssistantText"
        );
    }
}
