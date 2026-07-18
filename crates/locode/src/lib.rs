//! locode — the facade crate: the public surface for consumers (the future
//! `locode-app`, and `locode-exec` as the reference consumer).
//!
//! Two curated surfaces (Task 14, SPEC Users #3/#4):
//!
//! - **The driving API** — assemble and run one session: [`Session`] +
//!   [`EngineConfig`], harness selection ([`resolve`]/[`available`] +
//!   [`PackContext`]), the providers ([`AnthropicProvider`]/[`MockProvider`]),
//!   the host ([`Host`]/[`HostConfig`]/[`PathPolicy`]), and the report/event
//!   types ([`Report`]/[`Status`]/[`Event`]/[`Usage`]).
//! - **The tool surface** — use our typed tools in *your own* loop: [`Tool`],
//!   [`Registry`] (+ the [`Registry::dispatch`] door), [`ToolCtx`],
//!   [`ToolOutput`], [`ToolSpec`], and the packs' concrete tool sets via
//!   [`Pack::build_registry`].
//!
//! Downstream code should depend on this crate, not the `locode-*` internals —
//! the internals may churn; this surface is the stable contract.

// ---- protocol: conversation, report envelope, stream events ----
pub use locode_protocol::{
    ContentBlock, Conversation, Event, ImageSource, Message, Report, ResultChunk, Role, Status,
    ToolCallRecord, ToolSpec, Usage, reconstruct_conversation,
};

// ---- engine: the driving API ----
pub use locode_engine::{EngineConfig, EventSink, FnSink, NullSink, Session};

// ---- providers: the wire schemas ----
pub use locode_provider::anthropic::{
    ApiBackend, AuthRefresh, AuthScheme, DeveloperRendering, ModelConfig, ReasoningEncoding,
    RetryPolicy,
};
pub use locode_provider::{
    AnthropicProvider, CacheHint, Completion, ConversationRequest, MockProvider, Provider,
    ProviderError, ReasoningEffort, SamplingArgs, StopReason,
};

// ---- packs: harness selection ----
pub use locode_packs::{GrokPack, Pack, PackContext, UnknownHarness, available, grok, resolve};

// ---- host: the side-effect seam ----
pub use locode_host::{ExecLimits, Host, HostConfig, PathPolicy};

// ---- tools: the full tool surface (SPEC Users #4) ----
pub use locode_tools::{
    Dispatched, DynTool, MODEL_OUTPUT_BUDGET, Registry, Tool, ToolCtx, ToolError, ToolKind,
    ToolOutput, truncate_for_model,
};
