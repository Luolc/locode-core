//! Pre-send transcript hygiene: pairing repair + duplicate-result dedup (ADR-0004).
//!
//! Providers reject the **entire** request if a `tool_use` has no matching
//! `tool_result`, or if a `tool_result` is duplicated. ADR-0004 makes this a single
//! function the provider layer runs unconditionally before every send, rather than
//! scattering checks. It lives here (not in `locode-protocol`, which is types-only)
//! because it is a provider-layer concern: the engine — which depends on
//! `locode-provider` — calls it before each sample, and each wire calls it before
//! serializing (Task 12). Adapted from Grok Build's `repair_dangling_tool_calls` +
//! `dedup_duplicate_tool_results`.
//!
//! Our transcript nests `ToolUse` inside an `Assistant` message and `ToolResult`
//! inside the following `User` message(s), so the port scans messages rather than a
//! flat item list.

use std::collections::{HashMap, HashSet};

use locode_protocol::{ContentBlock, Message, ResultChunk, Role};

/// What a [`repair_pairing`] pass changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RepairStats {
    /// Synthetic `is_error` results added for dangling `tool_use` blocks.
    pub synthesized: usize,
    /// Duplicate `tool_result` blocks removed (kept the last per id).
    pub deduped: usize,
}

impl RepairStats {
    /// Whether the pass left the transcript unchanged.
    #[must_use]
    pub fn is_noop(self) -> bool {
        self.synthesized == 0 && self.deduped == 0
    }
}

/// The text a synthesized result carries for a `tool_use` that never got a result.
const DANGLING_RESULT_TEXT: &str =
    "tool result missing: this call was not completed (synthesized to keep the transcript valid)";

/// Make `messages` valid to send: dedup duplicate `tool_result`s, then synthesize
/// `is_error` results for any dangling `tool_use`. Idempotent — a valid transcript
/// passes through unchanged.
pub fn repair_pairing(messages: &mut Vec<Message>) -> RepairStats {
    let deduped = dedup_duplicate_results(messages);
    let synthesized = repair_dangling(messages);
    RepairStats {
        synthesized,
        deduped,
    }
}

/// Remove duplicate `tool_result` blocks, keeping the **last** per `tool_use_id`
/// (Grok's `dedup_duplicate_tool_results` semantics). Drops any message left empty.
fn dedup_duplicate_results(messages: &mut Vec<Message>) -> usize {
    // The winning (message, block) position for each id is its last occurrence.
    let mut last: HashMap<&str, (usize, usize)> = HashMap::new();
    for (mi, message) in messages.iter().enumerate() {
        for (bi, block) in message.content.iter().enumerate() {
            if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                last.insert(tool_use_id.as_str(), (mi, bi));
            }
        }
    }
    // Nothing repeats if every id's count is 1; cheap early exit.
    if last.len()
        == messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .count()
    {
        return 0;
    }

    let winners: HashSet<(usize, usize)> = last.into_values().collect();
    let mut removed = 0;
    for (mi, message) in messages.iter_mut().enumerate() {
        let mut bi = 0;
        message.content.retain(|block| {
            let here = bi;
            bi += 1;
            let is_result = matches!(block, ContentBlock::ToolResult { .. });
            let keep = !is_result || winners.contains(&(mi, here));
            if !keep {
                removed += 1;
            }
            keep
        });
    }
    // A message emptied by dedup would itself be rejected by the wire — drop it.
    messages.retain(|m| !m.content.is_empty());
    removed
}

/// Synthesize an `is_error` `tool_result` for every `tool_use` with no result,
/// placing it in the message immediately after the emitting assistant turn.
fn repair_dangling(messages: &mut Vec<Message>) -> usize {
    let answered: HashSet<String> = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect();

    let mut synthesized = 0;
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role == Role::Assistant {
            let dangling: Vec<String> = messages[i]
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } if !answered.contains(id) => Some(id.clone()),
                    _ => None,
                })
                .collect();

            if !dangling.is_empty() {
                synthesized += dangling.len();
                let synth: Vec<ContentBlock> = dangling
                    .into_iter()
                    .map(|id| ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: vec![ResultChunk::Text {
                            text: DANGLING_RESULT_TEXT.to_owned(),
                        }],
                        is_error: true,
                    })
                    .collect();

                // Anthropic wants the results in the User turn right after the
                // assistant turn: reuse it if present, else insert a fresh one.
                if messages.get(i + 1).is_some_and(|m| m.role == Role::User) {
                    let existing = std::mem::take(&mut messages[i + 1].content);
                    let mut merged = synth;
                    merged.extend(existing);
                    messages[i + 1].content = merged;
                } else {
                    messages.insert(
                        i + 1,
                        Message {
                            role: Role::User,
                            content: synth,
                        },
                    );
                }
            }
        }
        i += 1;
    }
    synthesized
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant_tool_use(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "echo".into(),
                input: json!({}),
            }],
        }
    }

    fn user_result(id: &str, text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: vec![ResultChunk::Text { text: text.into() }],
                is_error: false,
            }],
        }
    }

    #[test]
    fn dangling_tool_use_gets_synthetic_result() {
        let mut messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text { text: "go".into() }],
            },
            assistant_tool_use("c1"), // no following result
        ];
        let stats = repair_pairing(&mut messages);
        assert_eq!(stats.synthesized, 1);
        // A trailing User message with an is_error result for c1 was inserted.
        let last = messages.last().expect("a message");
        assert_eq!(last.role, Role::User);
        assert!(matches!(
            last.content.first(),
            Some(ContentBlock::ToolResult { tool_use_id, is_error: true, .. }) if tool_use_id == "c1"
        ));
    }

    #[test]
    fn duplicate_results_keep_the_last() {
        let mut messages = vec![
            assistant_tool_use("c1"),
            Message {
                role: Role::User,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "c1".into(),
                        content: vec![ResultChunk::Text {
                            text: "first".into(),
                        }],
                        is_error: false,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "c1".into(),
                        content: vec![ResultChunk::Text {
                            text: "second".into(),
                        }],
                        is_error: false,
                    },
                ],
            },
        ];
        let stats = repair_pairing(&mut messages);
        assert_eq!(stats.deduped, 1);
        assert_eq!(stats.synthesized, 0);
        let results: Vec<_> = messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .collect();
        assert_eq!(results.len(), 1, "only the last result should survive");
        assert!(matches!(
            results[0],
            ContentBlock::ToolResult { content, .. }
                if matches!(content.first(), Some(ResultChunk::Text { text }) if text == "second")
        ));
    }

    #[test]
    fn valid_transcript_is_unchanged() {
        let mut messages = vec![assistant_tool_use("c1"), user_result("c1", "ok")];
        let before = messages.clone();
        let stats = repair_pairing(&mut messages);
        assert!(stats.is_noop());
        assert_eq!(messages, before, "a paired transcript must pass through");
    }

    #[test]
    fn repair_is_idempotent() {
        let mut messages = vec![assistant_tool_use("c1")];
        let first = repair_pairing(&mut messages);
        assert_eq!(first.synthesized, 1);
        let second = repair_pairing(&mut messages);
        assert!(second.is_noop(), "second pass must find nothing to fix");
    }
}
