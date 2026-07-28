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
/// `System` is the static base identity; `Developer` is app-author instructions that map
/// **1:1 and losslessly** onto a native provider role. On the wire, `System` maps to
/// Anthropic's top-level `system` (or an OpenAI `system` message), while `Developer` maps to
/// an Anthropic mid-conversation `system` message (or an OpenAI `developer` message).
/// Injected framing/reminders (e.g. `AGENTS.md` project instructions) are **not** `Developer`
/// — they are authored as `User` `<system-reminder>` so the conversation ⇄ payload
/// conversion stays reversible (ADR-0013 amendment / ADR-0023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Immutable base identity and safety/policy — the "constitution".
    System,
    /// App-author instructions with a native provider role (OpenAI `developer` / Anthropic
    /// beta mid-conversation `system`). Not the vehicle for reminders — those are `User`.
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
    /// Assistant reasoning, unified across wires (ADR-0013 amendment
    /// 2026-07-19; replaces the earlier `Thinking`/`RedactedThinking` pair).
    ///
    /// [`ReasoningFormat`] selects the replay contract; each wire's build
    /// replays only its own format(s) and **drops foreign formats** (a session
    /// never crosses wires, so nothing is lost).
    Reasoning {
        /// Which encoding/replay contract this data follows.
        format: ReasoningFormat,
        /// Human-readable reasoning: the full text (`anthropic`), empty
        /// (`anthropic_redacted`), the summary (`openai_responses`), or the
        /// captured text (`text_only`).
        text: String,
        /// Anthropic's validator over `text` (`anthropic` format only).
        signature: Option<String>,
        /// The wire's opaque replay payload, replayed verbatim and never
        /// interpreted: the whole Responses reasoning item
        /// (`openai_responses`) or Anthropic's redacted-thinking data
        /// (`anthropic_redacted`).
        payload: Option<Value>,
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

/// The encoding/replay contract of a [`ContentBlock::Reasoning`] block.
///
/// Named after the wire's own vocabulary (Responses reasoning items tag
/// themselves `format: "openai-responses-v1"`). Serialized values deliberately
/// echo the `api_schema` strings so a trace reader maps block → wire at a
/// glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReasoningFormat {
    /// Anthropic extended thinking: full `text` + `signature` validator.
    Anthropic,
    /// Anthropic `redacted_thinking`: encrypted `payload`, empty `text`.
    AnthropicRedacted,
    /// OpenAI Responses reasoning item: summary in `text`, the WHOLE item in
    /// `payload` (id + summary + `encrypted_content` + `format` + future fields).
    OpenAiResponses,
    /// Capture-only reasoning with no replay contract (e.g. Chat Completions
    /// gateway extensions). Never replayed by any wire.
    TextOnly,
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
    /// The wire schema the run used (e.g. `anthropic`) — the provider's `api_schema()`.
    /// Names the request/response protocol shape, not a gateway/endpoint.
    pub api_schema: String,
    /// The assistant's final text message, if the run completed with one.
    pub final_message: Option<String>,
    /// A schema-constrained task answer, if one was requested (`--json-schema`).
    pub structured_output: Option<Value>,
    /// How many sample→dispatch→append turns the loop ran.
    pub turns: u32,
    /// A record of every tool call made during the run.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Token accounting for the run: **summed over every turn**. This is the cost
    /// basis — what the run generated in total — and grows without bound as a run takes
    /// more turns.
    pub usage: Usage,
    /// Token accounting for the run's **final turn only** — the context occupancy the
    /// next request starts from.
    ///
    /// Not derivable from [`Report::usage`]: each turn's request re-sends the whole
    /// conversation, so a per-turn sum counts the same history once per turn and has no
    /// relationship to the context window. The last turn's request *is* the whole
    /// conversation, which is why this one number answers "how full is the context?".
    ///
    /// Additive field (ADR-0018's envelope-evolution policy: new optional report fields
    /// are non-breaking and do not bump `schema_version`); absent in traces written
    /// before 2026-07-25, where it reads as all-zero.
    #[serde(default)]
    pub context_usage: Usage,
    /// The session identifier.
    pub session_id: String,
    /// The final model stop reason (`"end_turn"`, `"max_tokens"`, …), when a
    /// completion was received (ADR-0009 amendment 2026-07-19): lets an eval
    /// pipeline distinguish "model finished" from "model got truncated"
    /// without re-reading the trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// A fatal error message, if the run ended in `status == error`/`model_error`.
    pub error: Option<String>,
}

/// The terminal state of a run. Serializes to the exact strings in ADR-0009.
///
/// **Envelope evolution policy at `schema_version: 1` (ADR-0018):** additions
/// — new status values, new optional record/report fields — are
/// **non-breaking** and do not bump `schema_version`; renames and removals
/// are breaking and would. JSON consumers should therefore treat an
/// unrecognized status string as "unknown terminal state", not a parse
/// error; Rust consumers get the same discipline from `#[non_exhaustive]`
/// (match with a wildcard arm — `locode-exec` maps unknown statuses to
/// exit 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Status {
    /// The model finished with a text answer and no further tool calls.
    Completed,
    /// The max-turns ceiling was hit.
    MaxTurns,
    /// A provider/network error after bounded retry.
    ModelError,
    /// A fatal (`Tool`/host) error aborted the run.
    Error,
    /// The run was cancelled through the session's cancel handle (Esc, a
    /// SIGTERM-driven timeout, …) — a **structured** terminal state, distinct
    /// from failure (ADR-0018): partial work is preserved and the report is
    /// still emitted.
    Cancelled,
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
    /// The approver's reason, iff this call was **denied by the approval seam**
    /// (ADR-0017). Set only from the approver-deny path — never reused for
    /// other failures, and cancellation synthetics never carry it — so deny
    /// stays structurally separable from failure and from cancel (the
    /// codex-`Declined` / grok-taxonomy lesson).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_reason: Option<String>,
}

/// Token accounting parsed from the provider's terminal usage event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input (prompt) tokens.
    pub input_tokens: u64,
    /// Output (completion) tokens.
    pub output_tokens: u64,
    /// Tokens served from the prompt cache. **`None` = this wire/provider does
    /// not report the counter; `Some(0)` = reported as zero** (a real signal:
    /// no cache hit). ADR-0009 amendment 2026-07-19 — zero-as-N/A rejected.
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to the prompt cache (`None` on wires that never report
    /// writes, e.g. the OpenAI family).
    pub cache_creation_tokens: Option<u64>,
    /// Reasoning/thinking tokens (`None` on wires that fold them into
    /// `output_tokens`, e.g. Anthropic).
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    /// Everything this turn put on the wire and got back: the full prompt (input plus
    /// both cache counters) plus the completion.
    ///
    /// **Both cache counters belong in the total.** A cached read and a cache write are
    /// prompt tokens that a provider bills differently — they occupy the context window
    /// exactly like uncached input, so leaving either out under-reports how full it is.
    #[must_use]
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens
            + self.cache_read_tokens.unwrap_or(0)
            + self.cache_creation_tokens.unwrap_or(0)
            + self.output_tokens
    }
}

/// `Some+Some` sums; `None` is the identity — a run total is `None` only if no
/// turn ever reported the counter.
fn add_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

impl std::ops::AddAssign for Usage {
    /// Accumulate another turn's usage field-wise (the engine sums across turns).
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.cache_read_tokens = add_opt(self.cache_read_tokens, rhs.cache_read_tokens);
        self.cache_creation_tokens = add_opt(self.cache_creation_tokens, rhs.cache_creation_tokens);
        self.reasoning_tokens = add_opt(self.reasoning_tokens, rhs.reasoning_tokens);
    }
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// The model-facing wire name (the harness pack's name for the tool).
    pub name: String,
    /// The tool description offered to the model.
    pub description: String,
    /// How the tool's input is specified to the model (ADR-0003 amendment
    /// 2026-07-19; replaces the bare `parameters: Value`).
    pub input: ToolInputFormat,
}

/// How a tool's input is specified: a JSON-schema function tool, or a freeform
/// tool whose raw-text input is constrained by a server-side grammar (OpenAI
/// Responses `custom` tools — codex's `apply_patch`). Exactly one of the two —
/// an enum, not optional fields, so invalid states are unrepresentable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolInputFormat {
    /// A JSON-schema function tool (every tool today).
    JsonSchema {
        /// The derived JSON Schema for the tool's arguments.
        parameters: Value,
    },
    /// A freeform tool: raw text constrained by a grammar. On wires without
    /// custom-tool support it degrades to a `{"input": string}` function tool;
    /// the raw text reaches the tool identically either way.
    Freeform {
        /// The grammar language.
        syntax: GrammarSyntax,
        /// The grammar source, verbatim.
        definition: String,
    },
}

/// The grammar language of a [`ToolInputFormat::Freeform`] tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrammarSyntax {
    /// A Lark grammar (codex `apply_patch.lark`).
    Lark,
    /// A regular expression.
    Regex,
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
        /// The wire schema in use (e.g. `anthropic`) — the provider's `api_schema()`.
        api_schema: String,
        /// The model id.
        model: String,
        /// The working directory.
        cwd: String,
        /// The max-turns ceiling; absent = unlimited (the default — ADR-0005
        /// amendment 2026-07-18).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_turns: Option<u32>,
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
    /// An incremental assistant-text fragment during a **streaming** turn
    /// (ADR-0021). **Display-only**: the whole [`Message`] is still appended at
    /// turn end, so deltas are *not* part of the reconstructed history (the trace
    /// stays whole-message — Q1). Emitted only when the engine runs in `streaming`
    /// mode; consumers that want a whole-message trace drop this variant.
    MessageDelta {
        /// One assistant-text fragment — append to the live buffer.
        text: String,
    },
    /// The in-flight streamed message was **abandoned**, and every
    /// [`Event::MessageDelta`] emitted since the last [`Event::Message`] is void.
    ///
    /// The engine resamples a turn after a retryable provider error, re-running
    /// the *same* request — so a stream that failed part-way is followed by a
    /// second stream of the same reply from the start. Without this marker a
    /// consumer that buffers deltas shows the reply twice. Discard the buffer and
    /// start it over; the turn itself is not lost.
    ///
    /// Consumers that ignore [`Event::MessageDelta`] (the whole-message trace)
    /// can ignore this too — the eventual [`Event::Message`] is unaffected.
    ///
    /// A caveat this event cannot fix: text a UI has already committed to the
    /// terminal's scrollback cannot be withdrawn, so a long partial reply may
    /// still be visible above the re-streamed one.
    MessageDeltaReset {
        /// Why the stream was abandoned — the provider error, for the trace.
        reason: String,
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
    /// An approver resolution at the dispatch gate (ADR-0017) — grok's journal
    /// shape (`PermissionResolved`). Emitted for **every** consulted call,
    /// allowed or denied, so interactive traces are complete; `wait_ms` (human
    /// decision latency) is unrecoverable from any other artifact.
    Approval {
        /// The `tool_use` id the decision applies to.
        tool_use_id: String,
        /// The client-facing tool name.
        tool_name: String,
        /// The resolution: `"allow"` or `"deny"`.
        decision: String,
        /// Milliseconds spent awaiting the approver (human decision latency).
        wait_ms: u64,
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
            // Deltas are display-only — the whole `Message` is appended at turn
            // end, so skipping them here keeps reconstruction whole-message (Q1).
            // `MessageDeltaReset` annuls deltas only, so a reconstruction that
            // ignores them has nothing to undo: the resampled turn still arrives
            // as one `Message`.
            Event::MessageDelta { .. }
            | Event::MessageDeltaReset { .. }
            | Event::Result { .. }
            | Event::Error { .. }
            | Event::Approval { .. } => {}
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
            api_schema: "anthropic".into(),
            final_message: Some("done".into()),
            structured_output: None,
            turns: 1,
            tool_calls: vec![],
            usage: Usage::default(),
            context_usage: Usage::default(),
            session_id: "sess-1".into(),
            stop_reason: None,
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
                api_schema: "anthropic".into(),
                model: "claude-opus-4-8".into(),
                cwd: "/repo".into(),
                max_turns: Some(30),
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

    #[test]
    fn message_delta_round_trips_as_jsonl() {
        let event = Event::MessageDelta {
            text: "hello ".into(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], json!("message_delta"), "{value}");
        assert_eq!(value["text"], json!("hello "));
        let back: Event = serde_json::from_value(value).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn message_delta_is_not_part_of_reconstructed_history() {
        // A streaming turn: deltas then the whole Message. Reconstruction must
        // ignore the deltas (the trace stays whole-message — ADR-0021 Q1).
        let assistant = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "hello world".into(),
            }],
        };
        let with_deltas = vec![
            Event::MessageDelta {
                text: "hello ".into(),
            },
            Event::MessageDelta {
                text: "world".into(),
            },
            Event::Message {
                message: assistant.clone(),
            },
        ];
        let without_deltas = vec![Event::Message {
            message: assistant.clone(),
        }];
        assert_eq!(
            reconstruct_conversation(&with_deltas),
            reconstruct_conversation(&without_deltas),
            "deltas must not affect reconstruction"
        );
        // And the reconstructed history is exactly the one whole message.
        assert_eq!(
            reconstruct_conversation(&with_deltas).messages,
            vec![assistant]
        );
    }

    /// `denial_reason` (ADR-0017) is additive at `schema_version: 1`: absent
    /// from the wire when `None`, round-trips when set, and pre-field JSON
    /// still deserializes.
    #[test]
    fn tool_call_record_denial_reason_is_additive() {
        let record = ToolCallRecord {
            id: "c1".into(),
            name: "shell".into(),
            kind: "shell".into(),
            args: json!({}),
            ok: false,
            output: Value::Null,
            denial_reason: None,
        };
        let value = serde_json::to_value(&record).unwrap();
        assert!(
            !value.as_object().unwrap().contains_key("denial_reason"),
            "None must not appear on the wire: {value}"
        );

        let denied = ToolCallRecord {
            denial_reason: Some("not allowed".into()),
            ..record
        };
        let value = serde_json::to_value(&denied).unwrap();
        assert_eq!(value["denial_reason"], json!("not allowed"));
        let back: ToolCallRecord = serde_json::from_value(value).unwrap();
        assert_eq!(back, denied);

        // A record serialized before the field existed still parses.
        let old = json!({
            "id": "c1", "name": "shell", "kind": "shell",
            "args": {}, "ok": true, "output": null
        });
        let back: ToolCallRecord = serde_json::from_value(old).unwrap();
        assert_eq!(back.denial_reason, None);
    }

    /// `Status::Cancelled` (ADR-0018) rides the wire as `"cancelled"` — an
    /// additive value at `schema_version: 1` per the documented policy.
    #[test]
    fn cancelled_status_wire_string() {
        assert_eq!(
            serde_json::to_value(Status::Cancelled).unwrap(),
            json!("cancelled")
        );
        let back: Status = serde_json::from_value(json!("cancelled")).unwrap();
        assert_eq!(back, Status::Cancelled);
    }

    /// `Event::Approval` (ADR-0017): grok's journal shape, `snake_case`
    /// tagged, round-trips, and reconstruction ignores it.
    #[test]
    fn approval_event_shape_and_reconstruction() {
        let event = Event::Approval {
            tool_use_id: "c1".into(),
            tool_name: "run_terminal_cmd".into(),
            decision: "deny".into(),
            wait_ms: 1234,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "approval",
                "tool_use_id": "c1",
                "tool_name": "run_terminal_cmd",
                "decision": "deny",
                "wait_ms": 1234
            })
        );
        let back: Event = serde_json::from_value(value).unwrap();
        assert_eq!(back, event);

        let conversation = reconstruct_conversation(&[event]);
        assert!(
            conversation.messages.is_empty(),
            "approval events are run metadata, not history"
        );
    }
}
