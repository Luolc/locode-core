//! Serde DTOs for the Anthropic Messages API (`POST /v1/messages`).
//!
//! Ported nearly verbatim from Grok Build's `xai-grok-sampling-types/src/messages.rs`
//! (already correct and forward-compatible), with two deliberate deltas (plan §3.2):
//!
//! 1. [`ContentBlock::ToolResult`] carries **`is_error`** — our protocol has it
//!    (ADR-0013) and Anthropic honours it; grok's version dropped it.
//! 2. [`MessagesRequest`] carries an optional OpenRouter **`provider`** preferences
//!    field (plan §9.2) — injected only when the backend is OpenRouter.
//!
//! These are pure wire shapes: no conversion logic lives here (that is
//! [`build`](super::build) and `parse`). The streaming event types are
//! ported present-but-unused for the deferred streaming path (plan §1).

use serde::{Deserialize, Serialize};

// ============================================================================
// Request types
// ============================================================================

/// `POST /v1/messages` request body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessagesRequest {
    /// The model id (native `claude-…` or OpenRouter `anthropic/claude-…`).
    pub model: String,
    /// The conversation turns (user/assistant; system is top-level).
    pub messages: Vec<Message>,
    /// Hard output-token cap for this call.
    pub max_tokens: u32,
    /// Top-level system prompt (hoisted from `Role::System` messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemParam>,
    /// Tool definitions offered to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolParam>>,
    /// Tool-choice constraint (v0 leaves this to the default `auto`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoiceParam>,
    /// Sampling temperature. **Omitted whenever thinking is enabled** (plan §4.3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus-sampling probability mass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k sampling cutoff (wire-specific; unused by the neutral core).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Whether to stream the response (v0 always sends `false`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Custom stop sequences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// Extended-thinking configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// Effort/format output configuration (effort-based reasoning encoding).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    /// Request metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// OpenRouter provider-routing preferences (plan §9.2). **Not** part of the
    /// Anthropic API — set only when [`ApiBackend::OpenRouter`](super::config::ApiBackend)
    /// is active, where `require_parameters: true` prevents OpenRouter from routing
    /// to a backend that silently drops `cache_control`/`thinking`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<serde_json::Value>,
}

/// Effort/format output configuration (grok `messages.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Reasoning effort string (`"low"`/`"medium"`/`"high"`), effort-beta gated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Structured-output format (deferred with `--json-schema`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
}

/// Structured-output format selector.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputFormat {
    /// Constrain the final answer to a JSON schema.
    JsonSchema {
        /// The JSON schema to constrain to.
        schema: serde_json::Value,
    },
}

/// One conversation turn on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Who authored the turn.
    pub role: MessageRole,
    /// The turn's content.
    pub content: MessageContent,
}

/// Wire message roles.
///
/// Grok's enum is `User | Assistant` only; we add `System` for the
/// mid-conversation-system beta path (`DeveloperRendering::MidConversationSystemBeta`,
/// plan §4.1) — never emitted under the portable default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// A human / tool-result turn.
    User,
    /// A model turn.
    Assistant,
    /// A mid-conversation system message (beta-gated; see enum docs).
    System,
}

/// Message content: a bare string or a block list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain-text shorthand.
    Text(String),
    /// Structured content blocks.
    Blocks(Vec<ContentBlock>),
}

/// The top-level `system` parameter: a bare string or text blocks.
///
/// Grok's rule (ported): a single block *without* `cache_control` collapses to
/// `Text`; otherwise use `Blocks` (plan §4.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemParam {
    /// Plain-text shorthand.
    Text(String),
    /// Text blocks (each may carry a cache marker).
    Blocks(Vec<TextBlock>),
}

/// A system text block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBlock {
    /// Always `"text"`.
    #[serde(rename = "type")]
    pub r#type: String,
    /// The text content.
    pub text: String,
    /// Prompt-cache breakpoint marker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// A prompt-cache breakpoint (`{"type": "ephemeral"}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    /// Always `"ephemeral"`.
    #[serde(rename = "type")]
    pub r#type: String,
}

impl CacheControl {
    /// The standard ephemeral marker.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            r#type: "ephemeral".to_string(),
        }
    }
}

/// Content blocks used in both requests and responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
        /// Prompt-cache breakpoint marker (last-message placement, plan §4.3).
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// An image.
    Image {
        /// Where the image bytes come from.
        source: ImageSource,
    },
    /// A tool call emitted by the assistant.
    ToolUse {
        /// Provider-assigned id — preserved **verbatim** (ADR-0007, plan §4.5).
        id: String,
        /// The tool's wire name.
        name: String,
        /// The tool arguments.
        input: serde_json::Value,
    },
    /// A tool result, carried in a user turn.
    ToolResult {
        /// The id of the `tool_use` this answers.
        tool_use_id: String,
        /// The result payload.
        content: ToolResultContent,
        /// Whether the tool call failed. **Our delta over grok's structs** —
        /// the protocol carries it (ADR-0013) and Anthropic honours it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
        /// Prompt-cache breakpoint marker.
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Assistant extended thinking. Replayed **verbatim with its signature** on
    /// subsequent requests (plan §4.2) — Anthropic 400s a thinking + `tool_use`
    /// turn whose signed thinking block is not echoed back.
    Thinking {
        /// The reasoning text.
        thinking: String,
        /// The opaque signature required for replay.
        signature: String,
    },
    /// Encrypted assistant thinking, replayed verbatim like signed thinking.
    /// **Delta over grok's structs**: observed live during the Task-12 smoke —
    /// without this variant the whole response fails to deserialize.
    RedactedThinking {
        /// The encrypted payload.
        data: String,
    },
}

/// The source of an image block.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Tool-result payload: a bare string or nested blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// Plain-text shorthand.
    Text(String),
    /// Structured content blocks (text/image).
    Blocks(Vec<ContentBlock>),
}

/// A tool definition (Anthropic Messages format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParam {
    /// The model-facing tool name.
    pub name: String,
    /// The tool description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The JSON schema for the tool's arguments (normalized, plan §9 spike).
    pub input_schema: serde_json::Value,
}

/// Tool-choice constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoiceParam {
    /// The model decides (the default).
    Auto,
    /// The model must call some tool.
    Any,
    /// The model must call the named tool.
    Tool {
        /// The required tool's name.
        name: String,
    },
}

/// How thinking content is displayed for adaptive-thinking models.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingDisplay {
    /// Thinking content omitted from the response.
    Omitted,
    /// Thinking content returned as a summary.
    Summarized,
}

/// Extended-thinking configuration (grok `messages.rs`: three modes).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    /// Explicit token budget (4.0–4.5 models; the v0 default encoding).
    Enabled {
        /// The thinking-token budget. May exceed `max_tokens` under the
        /// interleaved-thinking beta (plan §9.3).
        budget_tokens: u32,
    },
    /// The API decides the budget (4.6+ models, effort encoding).
    Adaptive {
        /// Thinking display mode; omitted when `None` for back-compat.
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    /// Thinking off.
    Disabled,
}

/// Request metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// An opaque end-user id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

// ============================================================================
// Response types
// ============================================================================

/// Non-streaming response from `POST /v1/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    /// The server-assigned message id.
    pub id: String,
    /// Always `"message"`.
    #[serde(rename = "type")]
    pub r#type: String,
    /// Always `"assistant"`.
    pub role: String,
    /// The assistant turn's content blocks, in order.
    pub content: Vec<ContentBlock>,
    /// The model that produced the response.
    pub model: String,
    /// Why generation stopped.
    pub stop_reason: Option<StopReason>,
    /// Token accounting.
    pub usage: MessagesUsage,
}

/// Wire stop reasons.
///
/// The `Unknown` catch-all (grok `messages.rs:219-226`) means a new server-side
/// value can never fail the parse; it preserves the wire string and **must stay
/// the last variant** (serde tries the tagged variants first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural end of turn.
    EndTurn,
    /// Hit the `max_tokens` cap.
    MaxTokens,
    /// The model wants to call tools.
    ToolUse,
    /// A stop sequence fired.
    StopSequence,
    /// The model refused to continue.
    Refusal,
    /// The turn was paused (server-tool round-trip).
    PauseTurn,
    /// The request exceeded the model's context window.
    ModelContextWindowExceeded,
    /// A stop reason this client does not model, carried verbatim.
    #[serde(untagged)]
    Unknown(String),
}

/// Token accounting from a Messages response.
///
/// Every field is `Option` + `default`: gateways translating other providers
/// into the Messages shape return `null` (observed live: OpenRouter serving
/// `openai/*` models sends `"cache_creation_input_tokens": null`), and
/// `#[serde(default)]` alone covers *missing*, not `null` — a bare `u64`
/// would fail the whole response parse.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessagesUsage {
    /// Input (prompt) tokens.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Output (completion) tokens.
    #[serde(default)]
    pub output_tokens: Option<u64>,
    /// Tokens written to the prompt cache.
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    /// Tokens served from the prompt cache.
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    /// Output-token breakdown, when the endpoint reports one.
    #[serde(default)]
    pub output_tokens_details: Option<OutputTokensDetails>,
}

/// The `output_tokens` breakdown (`usage.output_tokens_details`).
///
/// Optional throughout: not every endpoint sends it, and the field is absent on
/// older responses — a missing breakdown must never fail the parse.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    /// Tokens the model spent thinking.
    #[serde(default)]
    pub thinking_tokens: Option<u64>,
}

/// A structured error body (`{"type":"error","error":{"type":…,"message":…}}`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorBody {
    /// The inner error object.
    #[serde(default)]
    pub error: ErrorDetail,
}

/// The inner error object of an [`ErrorBody`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// The error type slug (e.g. `overloaded_error`, `invalid_request_error`).
    #[serde(rename = "type", default)]
    pub r#type: String,
    /// The human-readable error message.
    #[serde(default)]
    pub message: String,
}

// ============================================================================
// Streaming event types (present-but-unused: the deferred streaming path)
// ============================================================================

/// Top-level SSE streaming event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageStreamEvent {
    /// Stream opened with the initial message skeleton.
    MessageStart {
        /// The initial (mostly empty) response shell.
        message: MessagesResponse,
    },
    /// Terminal delta carrying the stop reason and final usage.
    MessageDelta {
        /// Stop reason + details.
        delta: MessageDeltaBody,
        /// Final usage counts.
        usage: MessageDeltaUsage,
    },
    /// Stream closed.
    MessageStop,
    /// A content block opened.
    ContentBlockStart {
        /// The block's index in the content array.
        index: u32,
        /// The opening block shell.
        content_block: ContentBlock,
    },
    /// A content block grew.
    ContentBlockDelta {
        /// The block's index in the content array.
        index: u32,
        /// The delta payload.
        delta: StreamDelta,
    },
    /// A content block closed.
    ContentBlockStop {
        /// The block's index in the content array.
        index: u32,
    },
    /// Keep-alive.
    Ping,
    /// A mid-stream error.
    Error {
        /// The error payload.
        error: StreamError,
    },
}

/// Body of a terminal `message_delta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaBody {
    /// Why generation stopped.
    pub stop_reason: Option<StopReason>,
    /// Provider detail for the stop (e.g. a refusal explanation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<StopDetails>,
}

/// Detail for a terminal `message_delta`; all fields optional so an unknown
/// shape never fails the terminal parse (grok `messages.rs:283-291`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StopDetails {
    /// The detail type (e.g. `"refusal"`).
    #[serde(rename = "type", default)]
    pub r#type: Option<String>,
    /// The refusal category.
    #[serde(default)]
    pub category: Option<String>,
    /// The human-readable explanation.
    #[serde(default)]
    pub explanation: Option<String>,
}

/// Usage payload of a terminal `message_delta`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageDeltaUsage {
    /// Output tokens so far.
    pub output_tokens: u64,
    /// Input tokens (present on the terminal delta).
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Cache-read tokens.
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    /// Cache-write tokens.
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
}

/// Content delta within a `content_block_delta` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamDelta {
    /// Text grew.
    TextDelta {
        /// The appended text.
        text: String,
    },
    /// Tool-call arguments grew (partial JSON; see `ToolCallAssembler`).
    InputJsonDelta {
        /// The appended raw JSON fragment.
        partial_json: String,
    },
    /// Thinking text grew.
    ThinkingDelta {
        /// The appended thinking text.
        thinking: String,
    },
    /// The thinking signature arrived.
    SignatureDelta {
        /// The signature fragment.
        signature: String,
    },
}

/// A mid-stream error payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamError {
    /// The error type slug.
    #[serde(rename = "type")]
    pub r#type: String,
    /// The human-readable message.
    pub message: String,
}
