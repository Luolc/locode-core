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

mod providers;

// ---- custom providers: name → factory registry (ADR-0015) ----
pub use providers::{
    BuiltProvider, ProviderBuildError, ProviderFactory, ProviderInit, ProviderRegistry,
};

// ---- protocol: conversation, report envelope, stream events ----
pub use locode_protocol::{
    ContentBlock, Conversation, Event, GrammarSyntax, ImageSource, Message, ReasoningFormat,
    Report, ResultChunk, Role, Status, ToolCallRecord, ToolInputFormat, ToolSpec, Usage,
    reconstruct_conversation,
};

// ---- engine: the driving API ----
pub use locode_engine::{
    AllowAll, ApprovalRequest, Approver, CancellationToken, Decision, EngineConfig, EventSink,
    FnSink, NullSink, Session,
};

// ---- providers: the wire schemas ----
pub use locode_provider::anthropic::{
    ApiBackend, AuthRefresh, AuthScheme, DeveloperRendering, ModelConfig, ReasoningEncoding,
    RetryPolicy,
};
pub use locode_provider::{
    AnthropicProvider, CacheHint, Completion, ConversationRequest, MockProvider, OpenAiBackend,
    OpenAiModelConfig, OpenAiResponsesProvider, Provider, ProviderError, ReasoningEffort,
    SamplingArgs, StopReason, SystemPlacement,
};

// ---- packs: harness selection ----
pub use locode_packs::{GrokPack, Pack, PackContext, UnknownHarness, available, grok, resolve};

// ---- host: the side-effect seam ----
pub use locode_host::{
    ExecLimits, GitMeta, Host, HostConfig, InstructionEntry, InstructionsConfig, PathPolicy,
    ProjectInstructions, RolloutContents, Settings, SettingsLoad, SkillsExtraEntry, TraceExtras,
    TraceWriter, decode_cwd_dirname, encode_cwd_dirname, find_latest_rollout, find_rollout_by_id,
    load_project_instructions, load_settings, locode_home, read_rollout,
};

// ---- tools: the full tool surface (SPEC Users #4) ----
pub use locode_tools::{
    Dispatched, DynTool, MODEL_OUTPUT_BUDGET, Registry, Tool, ToolCtx, ToolError, ToolKind,
    ToolOutput, truncate_for_model,
};
