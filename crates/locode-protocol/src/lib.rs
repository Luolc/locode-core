//! locode-protocol — shared, provider-neutral types with no I/O.
//!
//! Two concerns live here:
//! - the **conversation model** (4-role, Anthropic-shaped content blocks — [ADR-0013]),
//!   which the loop accumulates and hands to a `Provider`; and
//! - the **report envelope** ([ADR-0009]), the single JSON artifact `locode-exec` prints.
//!
//! Types are Rust-native and serialize with `serde` for our own persistence/reporting;
//! conversion to a specific provider wire (Anthropic, OpenAI) lives in each `Provider`
//! impl, not here.
//!
//! [ADR-0013]: https://github.com/Luolc/locode-core/blob/main/docs/decisions/ADR-0013-conversation-protocol.md
//! [ADR-0009]: https://github.com/Luolc/locode-core/blob/main/docs/decisions/ADR-0009-headless-io-contract.md

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================ Conversation model ============================

/// A full conversation: one uniform stream of role-tagged messages (ADR-0013).
///
/// There is no separate `system` field — a [`Role::System`] message *is* the base
/// prompt; the Anthropic wire hoists leading System messages into its top-level
/// `system` param.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    /// The ordered turns of the conversation.
    pub messages: Vec<Message>,
}

/// One message: a role plus an ordered list of content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who this message is from.
    pub role: Role,
    /// The message's content, as typed blocks.
    pub content: Vec<ContentBlock>,
}

/// The author of a message (ADR-0013).
///
/// `System` is the static base identity; `Developer` is client-injected instructions
/// and dynamic context. On the wire, `System` maps to Anthropic's top-level `system`
/// (or an OpenAI `system` message), while `Developer` maps to an Anthropic
/// mid-conversation `system` message (or an OpenAI `developer` message).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Immutable base identity and safety/policy — the "constitution".
    System,
    /// App-author instructions and dynamically injected operational context.
    Developer,
    /// The human's turns; also carries [`ContentBlock::ToolResult`] blocks.
    User,
    /// The model's turns: text, thinking, and tool-use blocks.
    Assistant,
}

/// A typed piece of message content, modeled on Anthropic's content blocks.
///
/// `#[non_exhaustive]` so new block kinds can be added without a breaking change;
/// only `Text`, `ToolUse`, and `ToolResult` are exercised in v0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },
    /// An image (multimodal).
    Image {
        /// Where the image bytes come from.
        source: ImageSource,
    },
    /// Assistant reasoning (extended thinking).
    Thinking {
        /// The reasoning text.
        text: String,
        /// Opaque provider signature required to replay the thinking block, if any.
        signature: Option<String>,
    },
    /// A tool call emitted by the assistant.
    ToolUse {
        /// Provider-assigned id, paired with a later [`ContentBlock::ToolResult`].
        id: String,
        /// The client-facing tool name (the harness pack's name).
        name: String,
        /// The tool arguments as a JSON value.
        input: Value,
    },
    /// The result of a tool call, carried in a [`Role::User`] message.
    ToolResult {
        /// The id of the [`ContentBlock::ToolUse`] this answers.
        tool_use_id: String,
        /// The result content (text and/or images).
        content: Vec<ResultChunk>,
        /// Whether the tool call failed (a soft error the model can recover from).
        is_error: bool,
    },
}

/// A single chunk of a tool result (a restricted set of block kinds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResultChunk {
    /// Text output.
    Text {
        /// The text content.
        text: String,
    },
    /// Image output (e.g. a screenshot).
    Image {
        /// Where the image bytes come from.
        source: ImageSource,
    },
}

/// The source of an image block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Inline base64-encoded bytes.
    Base64 {
        /// The MIME type, e.g. `image/png`.
        media_type: String,
        /// The base64-encoded image data.
        data: String,
    },
    /// A URL the provider fetches.
    Url {
        /// The image URL.
        url: String,
    },
}

// ============================== Report envelope ==============================

/// The single JSON document `locode-exec` prints to stdout (ADR-0009).
///
/// `schema_version` is frozen at `1`; changing the envelope shape is a deliberate,
/// versioned change (see the golden test).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// Envelope schema version. Always `1` for this contract.
    pub schema_version: u32,
    /// The terminal state of the run.
    pub status: Status,
    /// The harness pack the run used (e.g. `grok`).
    pub harness: String,
    /// The provider wire the run used (e.g. `anthropic`).
    pub provider: String,
    /// The assistant's final text message, if the run completed with one.
    pub final_message: Option<String>,
    /// A schema-constrained task answer, if one was requested (`--json-schema`).
    pub structured_output: Option<Value>,
    /// How many sample→dispatch→append turns the loop ran.
    pub turns: u32,
    /// A record of every tool call made during the run.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Token accounting for the run.
    pub usage: Usage,
    /// The session identifier.
    pub session_id: String,
    /// A fatal error message, if the run ended in `status == error`/`model_error`.
    pub error: Option<String>,
}

/// The terminal state of a run. Serializes to the exact strings in ADR-0009.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The model finished with a text answer and no further tool calls.
    Completed,
    /// The max-turns ceiling was hit.
    MaxTurns,
    /// A provider/network error after bounded retry.
    ModelError,
    /// A fatal (`Tool`/host) error aborted the run.
    Error,
}

/// A report-side record of one tool call (distinct from the in-conversation
/// [`ContentBlock::ToolUse`]): the structured `output` view, not the model-facing text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// The tool-call id (matches the conversation's `tool_use` id).
    pub id: String,
    /// The client-facing tool name the model called.
    pub name: String,
    /// The canonical `ToolKind` tag for cross-pack A/B alignment (e.g. `shell`).
    pub kind: String,
    /// The arguments the model supplied.
    pub args: Value,
    /// Whether the call succeeded.
    pub ok: bool,
    /// The structured output of the call (the report view, not `prompt_text`).
    pub output: Value,
}

/// Token accounting parsed from the provider's terminal usage event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input (prompt) tokens.
    pub input_tokens: u64,
    /// Output (completion) tokens.
    pub output_tokens: u64,
    /// Tokens served from the prompt cache.
    pub cache_read_tokens: u64,
    /// Tokens written to the prompt cache.
    pub cache_creation_tokens: u64,
}

// ================================= Tool spec =================================

/// A provider-neutral tool spec: name + description + args JSON Schema.
///
/// This is the wire-agnostic representation a harness pack produces (from a
/// `Registry`) and a `Provider` wire maps onto its own tool format (e.g. Anthropic's
/// `{name, description, input_schema}` vs OpenAI's `{type:"function", function:{…}}`).
/// It lives in `locode-protocol` because both `locode-tools` (which builds it) and
/// `locode-provider` (which consumes it via `ConversationRequest`) need it, and the
/// dependency graph forbids `provider → tools`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// The model-facing wire name (the harness pack's name for the tool).
    pub name: String,
    /// The tool description offered to the model.
    pub description: String,
    /// The JSON Schema for the tool's arguments, derived from the arg type.
    pub parameters: Value,
}

// ========================= Streaming events (stream-json) =========================

/// One event in the `stream-json` trajectory (one JSON object per line).
///
/// The stream is a **self-sufficient, replayable source of the whole run**: `Init`
/// carries the base prompt + tool specs + model, and each [`Event::Message`] carries a
/// full turn — so [`reconstruct_conversation`] rebuilds the entire history with nothing
/// else. (Claude Code's stream omits `system`/`tools`, forcing a proxy capture to
/// recover them; `Init` closes that gap.) `#[non_exhaustive]` so events can be added
/// (e.g. per-turn markers) without a breaking change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// Emitted once at the start — everything needed to reconstruct context.
    Init {
        /// The session identifier.
        session_id: String,
        /// The harness pack in use (e.g. `grok`).
        harness: String,
        /// The provider wire in use (e.g. `anthropic`).
        provider: String,
        /// The model id.
        model: String,
        /// The working directory.
        cwd: String,
        /// The max-turns ceiling.
        max_turns: u32,
        /// The base `System` + `Developer` messages (prompt + capabilities).
        preamble: Vec<Message>,
        /// The tool specs offered to the model (name + JSON schema), as JSON values.
        tools: Vec<Value>,
    },
    /// A full message appended to the history (the trace): role + content blocks.
    Message {
        /// The appended message.
        message: Message,
    },
    /// The terminal event: the final report (identical to `--output-format json`).
    Result {
        /// The run's report envelope.
        report: Report,
    },
    /// A non-terminal error note (e.g. a retry); terminal errors ride in [`Event::Result`].
    Error {
        /// A human-readable message.
        message: String,
    },
}

/// Reconstruct the full [`Conversation`] from a `stream-json` event trajectory.
///
/// `Init.preamble` seeds the `System`/`Developer` prompt and each [`Event::Message`]
/// appends a turn; `Result`/`Error` events are run metadata, not part of the history.
/// This is the inverse of what `locode-exec` emits — the stream is a complete source.
#[must_use]
pub fn reconstruct_conversation(events: &[Event]) -> Conversation {
    let mut messages = Vec::new();
    for event in events {
        match event {
            Event::Init { preamble, .. } => messages.extend(preamble.iter().cloned()),
            Event::Message { message } => messages.push(message.clone()),
            Event::Result { .. } | Event::Error { .. } => {}
        }
    }
    Conversation { messages }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn conversation_round_trips_all_roles_and_tool_pairing() {
        let call_id = "call_42";
        let conversation = Conversation {
            messages: vec![
                Message {
                    role: Role::System,
                    content: vec![ContentBlock::Text {
                        text: "You are locode.".into(),
                    }],
                },
                Message {
                    role: Role::Developer,
                    content: vec![ContentBlock::Text {
                        text: "Available tools: run_terminal_command.".into(),
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "run echo hi".into(),
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Text {
                            text: "Sure.".into(),
                        },
                        ContentBlock::ToolUse {
                            id: call_id.into(),
                            name: "run_terminal_command".into(),
                            input: json!({ "command": "echo hi" }),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: call_id.into(),
                        content: vec![ResultChunk::Text {
                            text: "hi\n".into(),
                        }],
                        is_error: false,
                    }],
                },
            ],
        };

        let wire = serde_json::to_string(&conversation).expect("serialize");
        let back: Conversation = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(
            conversation, back,
            "conversation did not round-trip losslessly"
        );

        // The tool_use id and tool_result tool_use_id are the pairing link (ADR-0004).
        let ContentBlock::ToolUse { id, .. } = &conversation.messages[3].content[1] else {
            panic!("expected a tool_use block");
        };
        let ContentBlock::ToolResult { tool_use_id, .. } = &conversation.messages[4].content[0]
        else {
            panic!("expected a tool_result block");
        };
        assert_eq!(id, tool_use_id);
    }

    #[test]
    fn content_block_uses_anthropic_style_type_tags() {
        let block = ContentBlock::Text { text: "hi".into() };
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "type": "text", "text": "hi" })
        );
    }

    #[test]
    fn status_serializes_to_adr_0009_strings() {
        let cases = [
            (Status::Completed, "completed"),
            (Status::MaxTurns, "max_turns"),
            (Status::ModelError, "model_error"),
            (Status::Error, "error"),
        ];
        for (status, want) in cases {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(want));
        }
    }

    fn minimal_report() -> Report {
        Report {
            schema_version: 1,
            status: Status::Completed,
            harness: "grok".into(),
            provider: "anthropic".into(),
            final_message: Some("done".into()),
            structured_output: None,
            turns: 1,
            tool_calls: vec![],
            usage: Usage::default(),
            session_id: "sess-1".into(),
            error: None,
        }
    }

    /// The JSONL event stream is a self-sufficient source: `init.preamble` + every
    /// `message` event reconstruct the entire conversation (system/developer included).
    #[test]
    fn events_reconstruct_full_conversation() {
        let system = Message {
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: "base".into(),
            }],
        };
        let developer = Message {
            role: Role::Developer,
            content: vec![ContentBlock::Text {
                text: "capabilities".into(),
            }],
        };
        let user = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "run echo hi".into(),
            }],
        };
        let assistant = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "run_terminal_command".into(),
                input: json!({ "command": "echo hi" }),
            }],
        };
        let tool_result = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "c1".into(),
                content: vec![ResultChunk::Text {
                    text: "hi\n".into(),
                }],
                is_error: false,
            }],
        };

        let events = vec![
            Event::Init {
                session_id: "sess-1".into(),
                harness: "grok".into(),
                provider: "anthropic".into(),
                model: "claude-opus-4-8".into(),
                cwd: "/repo".into(),
                max_turns: 30,
                preamble: vec![system.clone(), developer.clone()],
                tools: vec![json!({ "name": "run_terminal_command" })],
            },
            Event::Message {
                message: user.clone(),
            },
            Event::Message {
                message: assistant.clone(),
            },
            Event::Message {
                message: tool_result.clone(),
            },
            Event::Result {
                report: minimal_report(),
            },
        ];

        // JSONL round-trip: one JSON object per line, parsed back losslessly.
        let jsonl = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: Vec<Event> = jsonl
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(parsed, events, "events did not round-trip through JSONL");

        // Reconstruction yields the FULL history, system/developer included.
        let rebuilt = reconstruct_conversation(&parsed);
        assert_eq!(
            rebuilt,
            Conversation {
                messages: vec![system, developer, user, assistant, tool_result]
            }
        );
    }

    #[test]
    fn event_uses_snake_case_type_tags() {
        let event = Event::Message {
            message: Message {
                role: Role::User,
                content: vec![],
            },
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap()["type"],
            json!("message")
        );
    }
}
