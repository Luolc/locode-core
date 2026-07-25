//! Static per-session configuration for the loop.

use std::path::PathBuf;
use std::time::Duration;

use locode_instructions::InstructionsConfig;
use locode_provider::{CacheHint, SamplingArgs};

/// Everything the loop needs that isn't the provider, registry, preamble, or sink.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// The session identifier (stamped into the report + `Init`).
    pub session_id: String,
    /// The harness pack in use, e.g. `grok` (stamped into the report + `Init`).
    pub harness: String,
    /// The wire schema in use, e.g. `anthropic`/`mock` (the provider's `api_schema`).
    pub api_schema: String,
    /// The model id (for `Init`).
    pub model: String,
    /// The working directory handed to each tool call.
    pub cwd: PathBuf,
    /// The path-jail root (ADR-0008), handed to each `ToolCtx`.
    pub workspace_root: PathBuf,
    /// The max sample→dispatch turns before terminating with `MaxTurns`
    /// (ADR-0005 amendment 2026-07-18). **`None` (the default) = unlimited** —
    /// every studied harness defaults to no cap (Claude Code's `maxTurns?` is
    /// enforced only when set; grok's `max_turns: Option<u32>` defaults `None`;
    /// codex has no turn cap at all).
    pub max_turns: Option<u32>,
    /// Bounded loop-level resample budget on retryable provider errors (ADR-0007).
    pub resample_retries: u32,
    /// Base backoff between resamples; the nth retry waits `base * n`. Set to zero
    /// in tests to avoid real sleeps.
    pub resample_backoff: Duration,
    /// Provider-neutral sampling knobs.
    pub sampling_args: SamplingArgs,
    /// Prompt-cache placement hint for the wire.
    pub cache_hint: CacheHint,
    /// Use the streaming sample path ([`locode_provider::Provider::stream`]) instead of
    /// [`locode_provider::Provider::complete`] (ADR-0021). Default **false** — the headless one-shot
    /// stays non-streaming; the TUI sets this, and headless can opt in via
    /// `--stream` (Anthropic rejects non-streaming requests past ~10 min, so
    /// streaming is required for unbounded output). Emits `Event::MessageDelta`s;
    /// the final `Message` is still appended whole, so the trace can stay
    /// whole-message by dropping the deltas (Q1).
    pub streaming: bool,
    /// Shared project-instruction loading (`AGENTS.md`, ADR-0023): discovered from `cwd`
    /// and injected once per session as a `User` `<system-reminder>`. Default = enabled.
    pub instructions: InstructionsConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            harness: String::new(),
            api_schema: String::new(),
            model: String::new(),
            cwd: PathBuf::from("."),
            workspace_root: PathBuf::from("."),
            max_turns: None,
            resample_retries: 2,
            resample_backoff: Duration::from_millis(500),
            sampling_args: SamplingArgs::default(),
            cache_hint: CacheHint::default(),
            streaming: false,
            instructions: InstructionsConfig::default(),
        }
    }
}
