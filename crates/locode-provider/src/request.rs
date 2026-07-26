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
/// 64k is the ceiling every current model this crate targets accepts, and the
/// value Claude Code escalates to when a turn truncates (`ESCALATED_MAX_TOKENS`,
/// `utils/context.ts:25`). Claude Code otherwise resolves a per-model
/// `{default, upperLimit}` pair (`utils/context.ts:149-208`) — 64k default for
/// opus-4-6, 32k for the sonnet-4-6 / 4.5 families — and its 8k
/// `CAPPED_DEFAULT_MAX_TOKENS` is **not** the shipped default: it is gated on
/// the `tengu_otk_slot_v1` slot-reservation experiment, which defaults to
/// *false* off first-party (`services/api/claude.ts:3394-3397`).
///
/// Not 128k, even though the current frontier models allow it: `upperLimit` is
/// per model, and Haiku 4.5 stops at 64k. 64k is the largest value that is
/// correct everywhere, and the gap is academic — a single tool call whose
/// arguments exceed 64k tokens is not a call worth completing.
///
/// This is a **ceiling on one response**, not a reservation: nothing is spent
/// unless the model generates it, so a generous value costs only the risk of a
/// long generation, never tokens.
///
/// The field itself cannot become `Option` to mean "let the API decide": the
/// Anthropic Messages API requires `max_tokens` on every request. opencode
/// encodes exactly that asymmetry — `max_tokens: Schema.Number` (required) for
/// Anthropic against `Schema.optional` for both OpenAI protocols — and always
/// sends a value, falling back to the model's declared output limit
/// (`protocols/anthropic-messages.ts:510,546`). A per-model table is the same
/// eventual answer here; one safe default is the honest v0.
pub const DEFAULT_MAX_TOKENS: u32 = 64_000;

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
    /// The deepest tier a wire offers. Distinct from [`ReasoningEffort::XHigh`]:
    /// Anthropic exposes both, and `max` is the "correctness matters more than
    /// cost" setting rather than one more notch of `xhigh`.
    Max,
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
