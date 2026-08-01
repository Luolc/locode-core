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

use std::collections::HashSet;

use locode_protocol::{ContentBlock, Message, ResultChunk, Role};

/// What a [`repair_pairing`] pass changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RepairStats {
    /// Results **invented** for a call that never got one — a synthetic
    /// `is_error` block, so the model sees that the tool did not report.
    pub synthesized: usize,
    /// Result blocks **dropped**: duplicates beyond the last, results that were
    /// not where the API requires them, and orphans whose `tool_use` is nowhere.
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

/// Make `messages` valid to send. Idempotent — a valid transcript passes through
/// unchanged.
///
/// **The rule is positional.** Anthropic requires every `tool_use` in a message to
/// be answered by a `tool_result` in the message **immediately after**; a result
/// living somewhere else does not satisfy it (`messages.N: 'tool_use' … found
/// without 'tool_result' blocks immediately after`). An earlier version asked only
/// whether the id appeared *anywhere*, so a stray result looked like an answer,
/// nothing was repaired, and — because the whole history replays every turn — the
/// session was permanently unsendable (ADR-0004 amendment 2026-08-01).
///
/// So, for each assistant turn that called tools: take the results out of the
/// following user turn, keep the ones its calls asked for (in **call order**),
/// synthesize an `is_error` block for the rest, and **drop every other result
/// block in the transcript**.
///
/// What this deliberately does *not* do is hunt down a misplaced result and move
/// it back. A result in the wrong place means something reordered the transcript,
/// and the only thing that ever did was two processes appending to one rollout —
/// now prevented at the source (ADR-0024 amendment 2026-08-01). Quietly restitching
/// such a file would make a scrambled conversation *sendable* rather than *right*,
/// and hide the next cause of scrambling. Missing results are repaired because
/// they have a mundane cause (a process killed between the call and its results);
/// misplaced ones are not, because they do not.
pub fn repair_pairing(messages: &mut Vec<Message>) -> RepairStats {
    let mut stats = RepairStats::default();
    // The turns this pass wrote answers into. The sweep at the end must not undo
    // its own work — every *other* result block in the transcript answers nothing.
    let mut answer_turns: HashSet<usize> = HashSet::new();

    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != Role::Assistant {
            i += 1;
            continue;
        }
        let ids: Vec<String> = messages[i]
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            i += 1;
            continue;
        }

        // Everything the following user turn holds, split into "answers these
        // calls" and "the rest" (its text, images, …).
        let (mut answers, other): (Vec<ContentBlock>, Vec<ContentBlock>) =
            match messages.get_mut(i + 1) {
                Some(next) if next.role == Role::User => std::mem::take(&mut next.content)
                    .into_iter()
                    .partition(|b| matches!(b, ContentBlock::ToolResult { .. })),
                _ => (Vec::new(), Vec::new()),
            };

        let mut blocks = Vec::with_capacity(ids.len());
        for id in &ids {
            // Last match wins, matching the long-standing dedup rule.
            let found = answers.iter().rposition(
                |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id),
            );
            if let Some(at) = found {
                blocks.push(answers.remove(at));
            } else {
                stats.synthesized += 1;
                blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: vec![ResultChunk::Text {
                        text: DANGLING_RESULT_TEXT.to_owned(),
                    }],
                    is_error: true,
                });
            }
        }
        // Whatever is left over answered nothing this turn asked for.
        stats.deduped += answers.len();
        // Results lead the user turn: the Responses wire flushes a leading text run
        // ahead of the result items, which would read as the user speaking before
        // the tools reported (ADR-0028's ordering rule).
        blocks.extend(other);

        if messages.get(i + 1).is_some_and(|m| m.role == Role::User) {
            messages[i + 1].content = blocks;
        } else {
            messages.insert(
                i + 1,
                Message {
                    role: Role::User,
                    content: blocks,
                },
            );
        }
        answer_turns.insert(i + 1);
        i += 2; // skip the result turn just written
    }

    // Any result block still standing is in a turn that answers no call —
    // an orphan or a leftover from a reordered file. The API rejects those as
    // loudly as a dangling call, so they go.
    for (index, message) in messages.iter_mut().enumerate() {
        if answer_turns.contains(&index) {
            continue;
        }
        let before = message.content.len();
        message
            .content
            .retain(|b| !matches!(b, ContentBlock::ToolResult { .. }));
        stats.deduped += before - message.content.len();
    }
    messages.retain(|m| !m.content.is_empty());
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tool_use(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "bash".into(),
                input: serde_json::json!({}),
            }],
        }
    }

    fn tool_result(id: &str, text: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: vec![ResultChunk::Text { text: text.into() }],
            is_error: false,
        }
    }

    fn result_text(message: &Message, id: &str) -> Option<String> {
        message.content.iter().find_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == id => content.first().map(|c| match c {
                ResultChunk::Text { text } => text.clone(),
                ResultChunk::Image { .. } => "<image>".into(),
            }),
            _ => None,
        })
    }

    /// A result that is **not** where the API requires it is dropped, and the call
    /// is answered by a synthetic block instead of being quietly restitched.
    ///
    /// Reordering only ever came from two processes appending to one rollout, which
    /// the trace format now handles by recording lineage (ADR-0024 amendment
    /// 2026-08-01) rather than by trusting file order. Silently moving results back
    /// would make a scrambled conversation *sendable* instead of *right*, and hide
    /// whatever scrambled it next time.
    #[test]
    fn a_result_in_the_wrong_place_is_dropped_not_restitched() {
        let mut messages = vec![
            tool_use("A"),
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "something got between them".into(),
                }],
            },
            Message {
                role: Role::User,
                content: vec![tool_result("A", "the real output")],
            },
        ];

        let stats = repair_pairing(&mut messages);
        assert_eq!(
            stats.synthesized, 1,
            "the call is answered where it must be"
        );
        assert_eq!(stats.deduped, 1, "the stray result is dropped");

        assert!(
            result_text(&messages[1], "A")
                .expect("answered in place")
                .contains("tool result missing"),
            "the answer says the tool did not report — it does not fake the output"
        );
        assert!(
            messages.iter().skip(2).all(|m| !m
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))),
            "no result blocks left anywhere else"
        );
    }

    /// An orphan result — one whose `tool_use` is nowhere — is rejected by the API
    /// just as loudly as a dangling call, so it is dropped rather than replayed.
    #[test]
    fn an_orphan_result_is_dropped() {
        let mut messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text { text: "go".into() }],
            },
            Message {
                role: Role::User,
                content: vec![tool_result("ghost", "from a call that is not here")],
            },
        ];
        let stats = repair_pairing(&mut messages);
        assert_eq!(stats.deduped, 1, "the orphan was removed");
        assert_eq!(messages.len(), 1, "its now-empty message went with it");
        assert!(matches!(messages[0].content[0], ContentBlock::Text { .. }));
    }

    /// Two calls in one assistant turn are answered in **call order**, which is what
    /// makes the batch readable to the model even when dispatch finished them out of
    /// order.
    #[test]
    fn a_batch_is_answered_in_call_order() {
        let mut messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "first".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "second".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![tool_result("second", "b"), tool_result("first", "a")],
            },
        ];
        repair_pairing(&mut messages);
        let ids: Vec<&str> = messages[1]
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["first", "second"]);
    }
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
