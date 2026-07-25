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
    /// The maximum number of tokens to generate; see
    /// [`DEFAULT_MAX_TOKENS`] for the default and why.
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

/// The default output-token budget for one sample.
///
/// A file-writing agent spends most of a turn inside one `tool_use` argument
/// blob, so this is not a "typical reply length" knob — it is the ceiling on
/// the largest single tool call the model can emit. Set it too low and the
/// wire truncates mid-call: the API returns the `tool_use` block with an
/// **empty** `input` (`{}`) and `stop_reason: "max_tokens"`, the typed decode
/// then reports a missing required field, and the model retries the same
/// oversized call forever. That is what 4096 did here.
///
/// 32k is the value the studied harnesses converge on for this tier. Claude
/// Code resolves a per-model `{default, upperLimit}` pair
/// (`utils/context.ts:149-208`) — 32k default for the sonnet-4-6 / 4.5
/// families, 64k for opus-4-6 — and its 8k `CAPPED_DEFAULT_MAX_TOKENS`
/// (`utils/context.ts:24`) is **not** the shipped default: it is gated on the
/// `tengu_otk_slot_v1` slot-reservation experiment, which defaults to *false*
/// off first-party (`services/api/claude.ts:3394-3397`), and is paired with a
/// one-shot escalate to `ESCALATED_MAX_TOKENS = 64_000` on truncation. Codex
/// never sends a sampling cap at all; Grok Build leaves `max_tokens: None`.
/// Our own Responses wire already caps at 32k (`openai/mod.rs:119`), so this
/// also stops the Anthropic wire being the odd one out.
///
/// A per-model table (Claude Code's shape) is the eventual answer; a single
/// safe default is the honest v0 — every current Anthropic and OpenAI model
/// this crate targets accepts 32k.
pub const DEFAULT_MAX_TOKENS: u32 = 32_000;

impl Default for SamplingArgs {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_TOKENS,
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
/// Tiers fragment per vendor/model generation (OpenAI `none…xhigh`, codex
/// `minimal…xhigh`, grok stores per-model allowed-effort lists), so the ladder
/// covers the observed union and `Other` passes vendor-specific strings
/// through verbatim (ADR-0007 amendment 2026-07-19). Wires surface the API's
/// own error on unsupported tiers — never silently clamp (silent clamping
/// would corrupt eval comparisons).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReasoningEffort {
    /// Explicitly send the wire's "none" (distinct from omitting the param —
    /// `SamplingArgs.reasoning_effort: None` omits it entirely).
    None,
    /// Minimal reasoning.
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Extra-high reasoning effort.
    XHigh,
    /// A vendor/model-specific tier, passed through verbatim by wires that
    /// take effort strings; wires with fixed mappings soft-reject it.
    Other(String),
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
