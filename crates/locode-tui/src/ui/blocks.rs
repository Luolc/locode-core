//! Transcript blocks: typed entries that render once into native scrollback
//! (SPEC-TUI rendering model; grok's `RenderBlock` enum + codex's
//! `HistoryCell` at v1 scale). Each block owns its source text so the
//! reflow-from-source extension stays possible.

use locode_core::{Report, Status};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Rows of tool-result body kept before head/tail truncation kicks in
/// (codex keeps 5 for agent commands; `exec_cell/render.rs:33`).
const TOOL_BODY_MAX_LINES: usize = 6;

/// Left gutter width for the message bullet (`● `) and hanging indent.
const GUTTER: usize = 2;

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
    /// Render to pre-wrapped lines at `width` for `insert_before`.
    #[must_use]
    pub fn render(&self, width: u16) -> Vec<Line<'static>> {
        let dim = Style::default().add_modifier(Modifier::DIM);
        match self {
            Block::UserPrompt(text) => {
                let mut lines = vec![Line::from("")];
                for (i, l) in text.lines().enumerate() {
                    let prefix = if i == 0 { "❯ " } else { "  " };
                    lines.push(Line::styled(
                        format!("{prefix}{l}"),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                }
                lines
            }
            Block::AssistantText(text) => {
                // A leading ● bullet on the first line, the rest hanging-indented
                // under it (Claude Code's message hierarchy). The 2-col gutter is
                // the left margin; keep a right margin too so prose doesn't hug
                // either edge.
                let content_width = usize::from(width).saturating_sub(GUTTER + 2).max(4);
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
                let mut lines = vec![Line::from(vec![
                    Span::styled("● ", bullet_style),
                    Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(args.clone(), dim),
                ])];
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
                    "{} · {turns} turn{} · {tokens} tok · {elapsed_secs}s",
                    status_str(*status),
                    if *turns == 1 { "" } else { "s" },
                );
                vec![Line::styled(format!("  {label}"), dim)]
            }
            Block::Notice(text) => vec![Line::styled(format!("● {text}"), dim)],
        }
    }
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

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn user_prompt_renders_with_prefix_and_multiline_indent() {
        let lines = text_of(&Block::UserPrompt("a\nb".into()).render(40));
        assert_eq!(lines, vec!["", "❯ a", "  b"]);
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
        assert!(lines[0].contains("run_terminal_cmd"));
        assert!(lines.iter().any(|l| l.contains("line-1")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("+15 lines")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("line-20")), "{lines:?}");
        // 1 header + 6 body rows (3 head + marker + 2 tail).
        assert_eq!(lines.len(), 7, "{lines:?}");
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
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("completed · 3 turns · 1234 tok · 41s"),
            "{lines:?}"
        );
        // No longer a full-width `──…──` rule (it read as an extra rule).
        assert!(!lines[0].contains("──"), "should not be a rule: {lines:?}");
    }

    #[test]
    fn assistant_text_has_bullet_and_hanging_indent() {
        let lines = Block::AssistantText("first line here".into()).render(40);
        // Leading blank, then the bulleted first content line.
        let first = lines
            .iter()
            .find(|l| l.to_string().contains("first"))
            .expect("content line");
        assert!(first.to_string().starts_with("● "), "{:?}", text_of(&lines));
    }

    #[test]
    fn assistant_text_wraps_to_width() {
        let lines = text_of(&Block::AssistantText("one two three four five".into()).render(16));
        assert!(lines.len() > 2, "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 16), "{lines:?}");
    }
}
