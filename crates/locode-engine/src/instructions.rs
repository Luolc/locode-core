//! Rendering discovered project instructions into a conversation message (ADR-0023 §2).
//!
//! The loader in `locode-host` produces a neutral [`ProjectInstructions`]; the engine turns
//! it into **one `User`-role `<system-reminder>` message** and injects it (Slice 3). This is
//! the "engine renders / injects the neutral value" half of ADR-0023 — a single format for
//! every pack (fidelity is bounded to tools + prompt, ADR-0023 §1).
//!
//! Why `User` and not `Developer`: injected framing is authored as `User` from the start so
//! the conversation ⇄ payload conversion stays losslessly bidirectional (ADR-0013 amendment
//! 2026-07-23) — a `Developer`-rendered-to-user fallback cannot be reversed.

use std::hash::{Hash, Hasher};

use locode_host::ProjectInstructions;
use locode_protocol::{ContentBlock, Message, Role};

/// The authority preamble + relevance disclaimer (Claude's framing —
/// `claude-code: api.ts:461-473`).
const PREAMBLE: &str = "As you answer the user's questions, you can use the project \
instructions below (deeper directories take precedence on conflict). They are context, \
not a message to answer.";

/// Leads the envelope when the instructions changed mid-session (ADR-0023 Refresh, the
/// Codex/opencode replace-banner pattern — `codex: context/world_state/agents_md.rs:9-11`).
const REPLACE_BANNER: &str =
    "These instructions replace all previously provided project instructions.";

/// The whole body when the instructions vanished mid-session.
const REMOVAL_NOTICE: &str = "The previously provided project instructions no longer apply.";

/// Appended when the body is truncated to the byte budget.
const TRUNCATION_MARKER: &str = "\n\n…[project instructions truncated]…";

/// Render project instructions into one `User` `<system-reminder>` message, or `None` when
/// there is nothing to inject.
///
/// Each entry becomes a `## From: <source_path>` section (root→cwd order, so the deepest
/// file is last). The **sections body** is capped at `byte_budget` bytes (UTF-8-boundary
/// safe) with a truncation marker; `byte_budget == 0` means unbounded. When `replace` is set
/// (the content changed mid-session), a replace banner leads the envelope.
pub(crate) fn render_instructions(
    instructions: &ProjectInstructions,
    byte_budget: usize,
    replace: bool,
) -> Option<Message> {
    if instructions.entries.is_empty() {
        return None;
    }

    let sections: Vec<String> = instructions
        .entries
        .iter()
        .map(|entry| {
            format!(
                "## From: {}\n{}",
                entry.source_path.display(),
                entry.content.trim_end()
            )
        })
        .collect();
    let body = truncate_on_char_boundary(sections.join("\n\n"), byte_budget);

    let lead = if replace {
        format!("{REPLACE_BANNER}\n{PREAMBLE}")
    } else {
        PREAMBLE.to_string()
    };
    let text = format!("<system-reminder>\n{lead}\n\n{body}\n</system-reminder>");
    Some(user_reminder(text))
}

/// The removal banner message, emitted when previously-applied instructions vanish.
pub(crate) fn removal_message() -> Message {
    user_reminder(format!(
        "<system-reminder>\n{REMOVAL_NOTICE}\n</system-reminder>"
    ))
}

/// A stable hash of the discovered instructions, or `None` when nothing was discovered.
/// Drives the per-turn diff: an unchanged hash means "don't re-inject" (ADR-0023 Refresh).
pub(crate) fn instructions_hash(instructions: &ProjectInstructions) -> Option<u64> {
    if instructions.entries.is_empty() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for entry in &instructions.entries {
        entry.source_path.hash(&mut hasher);
        entry.content.hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// A `User`-role `<system-reminder>` message wrapping `text`.
fn user_reminder(text: String) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text }],
    }
}

/// Truncate `s` to at most `budget` bytes at a UTF-8 char boundary, appending a marker when
/// anything was dropped. `budget == 0` ⇒ unbounded (returned unchanged).
fn truncate_on_char_boundary(mut s: String, budget: usize) -> String {
    if budget == 0 || s.len() <= budget {
        return s;
    }
    let mut end = budget;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s.push_str(TRUNCATION_MARKER);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use locode_host::InstructionEntry;
    use std::path::PathBuf;

    fn instr(entries: &[(&str, &str)]) -> ProjectInstructions {
        ProjectInstructions {
            entries: entries
                .iter()
                .map(|(p, c)| InstructionEntry {
                    source_path: PathBuf::from(p),
                    content: (*c).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn renders_exact_envelope() {
        let msg = render_instructions(&instr(&[("/p1", "a\n"), ("/p2", "b")]), 0, false).unwrap();
        let ContentBlock::Text { text } = &msg.content[0] else {
            panic!("expected text block");
        };
        let expected = "<system-reminder>\n\
            As you answer the user's questions, you can use the project instructions below \
            (deeper directories take precedence on conflict). They are context, not a \
            message to answer.\n\
            \n\
            ## From: /p1\n\
            a\n\
            \n\
            ## From: /p2\n\
            b\n\
            </system-reminder>";
        assert_eq!(text, expected);
    }

    #[test]
    fn empty_renders_none() {
        assert!(render_instructions(&instr(&[]), 0, false).is_none());
    }

    #[test]
    fn injected_message_is_user_text() {
        let msg = render_instructions(&instr(&[("/p", "x")]), 0, false).unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        assert!(matches!(msg.content[0], ContentBlock::Text { .. }));
    }

    #[test]
    fn truncates_at_budget_on_char_boundary_with_marker() {
        // A body well over a tiny budget, containing multi-byte chars near the cut.
        let big = "é".repeat(200); // 400 bytes
        let msg = render_instructions(&instr(&[("/p", &big)]), 64, false).unwrap();
        let ContentBlock::Text { text } = &msg.content[0] else {
            panic!();
        };
        assert!(
            text.contains("…[project instructions truncated]…"),
            "marker present"
        );
        // The section body was capped at 64 bytes before the marker; the envelope adds the
        // preamble + tags, but the point is the body did not blow past the budget.
        let body_start = text.find("## From:").unwrap();
        let body_end = text.find(TRUNCATION_MARKER).unwrap();
        assert!(body_end - body_start <= 64, "body ≤ budget before marker");
        // The cut landed on a char boundary — else `String::truncate` in the renderer
        // would have panicked before we got here, splitting an `é` mid-sequence.
    }

    #[test]
    fn zero_budget_is_unbounded() {
        let big = "x".repeat(10_000);
        let msg = render_instructions(&instr(&[("/p", &big)]), 0, false).unwrap();
        let ContentBlock::Text { text } = &msg.content[0] else {
            panic!();
        };
        assert!(!text.contains("truncated"));
        assert!(text.len() > 10_000);
    }

    #[test]
    fn replace_flag_leads_with_banner() {
        let msg = render_instructions(&instr(&[("/p", "x")]), 0, true).unwrap();
        let ContentBlock::Text { text } = &msg.content[0] else {
            panic!();
        };
        assert!(
            text.starts_with(
                "<system-reminder>\nThese instructions replace all previously provided \
                 project instructions.\n"
            ),
            "{text}"
        );
        assert!(text.contains("## From: /p"));
    }

    #[test]
    fn removal_message_is_user_notice() {
        let msg = removal_message();
        assert_eq!(msg.role, Role::User);
        let ContentBlock::Text { text } = &msg.content[0] else {
            panic!();
        };
        assert_eq!(
            text,
            "<system-reminder>\nThe previously provided project instructions no longer \
             apply.\n</system-reminder>"
        );
    }

    #[test]
    fn hash_is_none_empty_stable_and_content_sensitive() {
        assert_eq!(instructions_hash(&instr(&[])), None);
        let a = instructions_hash(&instr(&[("/p", "x")]));
        let a2 = instructions_hash(&instr(&[("/p", "x")]));
        let b = instructions_hash(&instr(&[("/p", "y")]));
        assert!(a.is_some());
        assert_eq!(a, a2, "stable for identical content");
        assert_ne!(a, b, "changes when content changes");
    }
}
