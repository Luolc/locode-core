//! Serde DTOs for `POST /v1/responses` (Task-18 plan §3.3).
//!
//! Deliberately **`Value`-forward**: `input`/`tools`/`output` are raw JSON
//! values built/peeked by `build`/`parse`. This is the plan's opaque-first
//! philosophy taken one step further than its enum sketch — reasoning items
//! must round-trip fields we have never heard of (probes found
//! `format: "openai-responses-v1"` / `"xai-responses-v1"`), and gateways add
//! response fields freely, so the typed surface is kept to the envelope where
//! it pays (top-level request knobs, usage, status) and stays out of the way
//! where it would rot (item internals). Hand-rolled, not `async_openai` — grok
//! has to monkey-patch that crate's serialization (plan §5.2's cautionary tale).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The request body (stateless always: `store:false`, no `previous_response_id`).
#[derive(Debug, Clone, Serialize)]
pub struct ResponsesRequest {
    /// The model id.
    pub model: String,
    /// The hoisted System prompt (codex's placement; `None` under
    /// `SystemPlacement::InputMessage`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The input item list (messages, calls, outputs, replayed reasoning).
    pub input: Vec<Value>,
    /// Tool definitions (flat `function` shape / `custom` with grammar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    /// Reasoning controls (`{effort, summary}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Value>,
    /// ALWAYS `false` — the stateless non-negotiable (plan §4.1): grok forces
    /// it for ZDR, codex sends false for OpenAI, OpenRouter 400s `true`.
    pub store: bool,
    /// ALWAYS `false` in v0 (non-streaming).
    pub stream: bool,
    /// ALWAYS `["reasoning.encrypted_content"]` — returned by default since
    /// the 2026 API change, sent anyway to guard older gateways.
    pub include: Vec<String>,
    /// Output-token cap (includes reasoning tokens; spec minimum 16).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Sampling temperature (no thinking-omit rule on this wire).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus-sampling probability mass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Cache-routing hint — the facade passes the session id (codex's rule).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// OpenRouter routing preferences (never sent to other backends).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Value>,
}

/// The non-streaming response envelope. Unknown fields tolerated everywhere
/// (OpenRouter appends `cost`/`is_byok`/…; the spec adds fields freely).
#[derive(Debug, Clone, Deserialize)]
pub struct ResponsesResponse {
    /// `completed` | `incomplete` | `failed` | …
    #[serde(default)]
    pub status: Option<String>,
    /// The output items, kept raw — `parse` peeks each item's `type`.
    #[serde(default)]
    pub output: Vec<Value>,
    /// Why an `incomplete` response stopped (`{reason}`).
    #[serde(default)]
    pub incomplete_details: Option<IncompleteDetails>,
    /// The error payload of a `failed` response.
    #[serde(default)]
    pub error: Option<ResponseError>,
    /// Token accounting (Option everything: gateway variance).
    #[serde(default)]
    pub usage: Option<ResponsesUsage>,
}

/// `incomplete_details` of an `incomplete` response.
#[derive(Debug, Clone, Deserialize)]
pub struct IncompleteDetails {
    /// e.g. `"max_output_tokens"`, `"content_filter"`.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The `error` object of a `failed` response (HTTP 200).
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseError {
    /// e.g. `"rate_limit_exceeded"`, `"server_error"`.
    #[serde(default)]
    pub code: Option<String>,
    /// The human-readable message.
    #[serde(default)]
    pub message: String,
}

/// Usage block: every counter `Option` (the Task-12 null-usage lesson).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResponsesUsage {
    /// Input (prompt) tokens.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Input-side detail (`cached_tokens`, `cache_write_tokens`).
    #[serde(default)]
    pub input_tokens_details: Option<InputTokensDetails>,
    /// Output (completion) tokens, reasoning included.
    #[serde(default)]
    pub output_tokens: Option<u64>,
    /// Output-side detail (`reasoning_tokens`).
    #[serde(default)]
    pub output_tokens_details: Option<OutputTokensDetails>,
}

/// `usage.input_tokens_details`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct InputTokensDetails {
    /// Prompt-cache read tokens.
    #[serde(default)]
    pub cached_tokens: Option<u64>,
    /// Prompt-cache write tokens (2026 spec addition; often absent).
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
}

/// `usage.output_tokens_details`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OutputTokensDetails {
    /// Reasoning/thinking tokens.
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}
