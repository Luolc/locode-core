//! The API-agnostic request the engine hands to any [`Provider`](crate::Provider).

use locode_protocol::{Message, ToolSpec};

/// One provider-neutral sampling request (ADR-0007).
///
/// The loop builds exactly one of these and knows nothing about any wire. Per
/// ADR-0013 there is **no separate `system` field** — `messages` carries the whole
/// role-tagged stream (System/Developer included), and each wire hoists leading
/// System messages into its own top-level slot (e.g. Anthropic's `system`).
#[derive(Debug, Clone)]
pub struct ConversationRequest {
    /// The full role-tagged conversation stream (System/Developer/User/Assistant).
    pub messages: Vec<Message>,
    /// The tool specs offered to the model, mapped by the wire to its tool format.
    pub tools: Vec<ToolSpec>,
    /// Provider-neutral sampling knobs.
    pub sampling_args: SamplingArgs,
    /// A hint for where the wire should place prompt-cache breakpoints.
    pub cache_hint: CacheHint,
}

/// Provider-neutral sampling knobs — the **common core** only.
///
/// Wire-specific parameters (Anthropic `top_k`/`stop_sequences`/thinking budget,
/// OpenAI `frequency_penalty`, …) are the concern of each wire's request builder,
/// not this type, keeping [`ConversationRequest`] API-agnostic (ADR-0007). Grok
/// Build uses the same layering: a neutral core plus per-wire superset structs.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplingArgs {
    /// The maximum number of tokens to generate.
    pub max_tokens: u32,
    /// Sampling temperature; a wire may omit it (e.g. Anthropic requires temp=1
    /// when thinking is on, so the wire drops it there).
    pub temperature: Option<f32>,
    /// Nucleus-sampling probability mass.
    pub top_p: Option<f32>,
    /// Neutral reasoning budget; each wire maps it to its own control
    /// (Anthropic `budget_tokens`, OpenAI reasoning `effort`).
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl Default for SamplingArgs {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            temperature: None,
            top_p: None,
            reasoning_effort: None,
        }
    }
}

/// A neutral reasoning-effort level, mapped per-wire (ADR-0007).
///
/// Grok Build proves this shape: one neutral enum with per-backend mappings
/// (`to_messages_api()` → Anthropic budget, `to_responses_api()` → OpenAI effort).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    /// Minimal reasoning.
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
}

/// Where the wire should place prompt-cache breakpoints (ADR-0007).
///
/// A reserved seam: the actual `cache_control` placement (Anthropic: one marker on
/// the last message + ≤4 on system blocks) lives in the wire (Task 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheHint {
    /// No prompt caching.
    Off,
    /// Apply the wire's default breakpoint policy.
    #[default]
    Standard,
}
