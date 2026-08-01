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
    /// Results **invented** because the call never got one — a synthetic
    /// `is_error` block, so the model sees that the tool did not report.
    pub synthesized: usize,
    /// Result blocks **dropped**: duplicates beyond the last, and orphans whose
    /// `tool_use` is nowhere in the transcript.
    pub deduped: usize,
    /// Results that existed but sat in the wrong message and were **moved** to the
    /// turn right after their call, content intact. Counted apart from the other
    /// two because it is the case a purely existential check cannot see — and the
    /// one that poisoned a session on 2026-08-01.
    pub relocated: usize,
}

impl RepairStats {
    /// Whether the pass left the transcript unchanged.
    #[must_use]
    pub fn is_noop(self) -> bool {
        self.synthesized == 0 && self.deduped == 0 && self.relocated == 0
    }
}

/// The text a synthesized result carries for a `tool_use` that never got a result.
const DANGLING_RESULT_TEXT: &str =
    "tool result missing: this call was not completed (synthesized to keep the transcript valid)";

/// Make `messages` valid to send. Idempotent — a valid transcript passes through
/// unchanged.
///
/// **The rule is positional, not existential.** Anthropic requires every
/// `tool_use` in a message to be answered by a `tool_result` **in the message
/// immediately after**; a result that exists somewhere else in the transcript does
/// not satisfy it (`messages.N: 'tool_use' … found without 'tool_result' blocks
/// immediately after`). An earlier version of this pass checked only whether the id
/// appeared *anywhere*, so a misplaced result looked answered, nothing was
/// repaired, and — because the whole history replays every turn — the session was
/// permanently unsendable. That is the failure this shape exists to prevent
/// (2026-08-01; ADR-0004 amendment).
///
/// So the pass rebuilds the pairing rather than patching it:
///
/// 1. remember every result's content by id (the **last** occurrence wins, keeping
///    the old dedup semantics);
/// 2. strip every result block out of the transcript, dropping messages left empty;
/// 3. for each assistant turn that called tools, write exactly those results — in
///    the order the calls were made — at the front of the following user message,
///    inserting one when there is none.
///
/// Results whose `tool_use` is nowhere in the transcript are **not** written back:
/// an orphan `tool_result` is rejected just as loudly as a dangling `tool_use`.
pub fn repair_pairing(messages: &mut Vec<Message>) -> RepairStats {
    // Which calls are already answered in the right place — computed **before**
    // anything moves, because stripping drops emptied messages and every index
    // after them shifts. Without this, rewriting a correct transcript would report
    // itself as a repair.
    let already_paired = correctly_paired(messages);
    let recovered = take_results(messages);
    let mut stats = RepairStats::default();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();

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
        let blocks: Vec<ContentBlock> = ids
            .into_iter()
            .map(|id| {
                if let Some(found) = recovered.by_id.get(&id) {
                    if !already_paired.contains(&id) {
                        stats.relocated += 1;
                    }
                    used.insert(id.clone());
                    return ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: found.content.clone(),
                        is_error: found.is_error,
                    };
                }
                stats.synthesized += 1;
                ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: vec![ResultChunk::Text {
                        text: DANGLING_RESULT_TEXT.to_owned(),
                    }],
                    is_error: true,
                }
            })
            .collect();

        // Results lead the following user turn: the wire lowers a leading text run
        // ahead of the result items, which would read as the user speaking before
        // the tools reported (ADR-0028's ordering rule, from the other direction).
        if messages.get(i + 1).is_some_and(|m| m.role == Role::User) {
            let existing = std::mem::take(&mut messages[i + 1].content);
            let mut merged = blocks;
            merged.extend(existing);
            messages[i + 1].content = merged;
        } else {
            messages.insert(
                i + 1,
                Message {
                    role: Role::User,
                    content: blocks,
                },
            );
        }
        i += 2; // skip the result turn we just wrote
    }

    // Only now: a message the strip emptied and the rebuild did not refill held
    // nothing but duplicates or orphans, and a content-less message is invalid on
    // the wire. Dropping earlier would have merged a results turn into the next
    // user prompt — valid, but a needless rewrite of the transcript's shape.
    messages.retain(|m| !m.content.is_empty());
    // Everything pulled out and not written back was a duplicate or an orphan.
    stats.deduped = recovered.total_blocks - used.len();
    stats
}

/// Every `tool_result` in the transcript, pulled out: content by id (last wins) and
/// how many blocks were removed. Messages left with nothing are dropped, since a
/// content-less message is invalid on the wire.
struct Recovered {
    by_id: HashMap<String, FoundResult>,
    total_blocks: usize,
}

/// One recovered result.
struct FoundResult {
    content: Vec<ResultChunk>,
    is_error: bool,
}

/// The `tool_use` ids whose result already sits in the message right after the
/// call — i.e. the ones the API would have accepted as-is. Everything else this
/// pass touches is a real change.
fn correctly_paired(messages: &[Message]) -> HashSet<String> {
    let mut ok = HashSet::new();
    for (i, message) in messages.iter().enumerate() {
        if message.role != Role::Assistant {
            continue;
        }
        let Some(next) = messages.get(i + 1).filter(|m| m.role == Role::User) else {
            continue;
        };
        for block in &message.content {
            let ContentBlock::ToolUse { id, .. } = block else {
                continue;
            };
            let answered_here = next.content.iter().any(
                |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id),
            );
            if answered_here {
                ok.insert(id.clone());
            }
        }
    }
    ok
}

fn take_results(messages: &mut [Message]) -> Recovered {
    let mut by_id: HashMap<String, FoundResult> = HashMap::new();
    let mut total_blocks = 0usize;
    for message in messages.iter_mut() {
        message.content.retain(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                total_blocks += 1;
                by_id.insert(
                    tool_use_id.clone(),
                    FoundResult {
                        content: content.clone(),
                        is_error: *is_error,
                    },
                );
                false
            }
            _ => true,
        });
    }
    Recovered {
        by_id,
        total_blocks,
    }
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

    /// **The 2026-08-01 poisoning.** A result that exists but sits one message too
    /// late satisfies an existential check and still gets the request rejected —
    /// `messages.N: 'tool_use' … found without 'tool_result' blocks immediately
    /// after`. Because the whole history replays every turn, that session could
    /// never be sent again. The repair must **move** it, keeping its content.
    #[test]
    fn a_result_in_the_wrong_message_is_moved_next_to_its_call() {
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
        assert_eq!(stats.relocated, 1, "the misplaced result was moved");
        assert_eq!(
            stats.synthesized, 0,
            "its content was recovered, not invented"
        );

        assert_eq!(messages[1].role, Role::User);
        assert_eq!(
            result_text(&messages[1], "A").as_deref(),
            Some("the real output"),
            "the result now leads the turn right after the call, content intact"
        );
        assert!(
            messages[1]
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("between"))),
            "the user's own text is kept, after the result"
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
