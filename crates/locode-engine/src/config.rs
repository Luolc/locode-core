//! Static per-session configuration for the loop.

use std::path::PathBuf;
use std::time::Duration;

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
    /// The max sample→dispatch turns before terminating with `MaxTurns` (ADR-0005).
    pub max_turns: u32,
    /// Bounded loop-level resample budget on retryable provider errors (ADR-0007).
    pub resample_retries: u32,
    /// Base backoff between resamples; the nth retry waits `base * n`. Set to zero
    /// in tests to avoid real sleeps.
    pub resample_backoff: Duration,
    /// Provider-neutral sampling knobs.
    pub sampling_args: SamplingArgs,
    /// Prompt-cache placement hint for the wire.
    pub cache_hint: CacheHint,
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
            max_turns: 30,
            resample_retries: 2,
            resample_backoff: Duration::from_millis(500),
            sampling_args: SamplingArgs::default(),
            cache_hint: CacheHint::default(),
        }
    }
}
