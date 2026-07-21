//! The public driving API.

use std::sync::Arc;

use locode_protocol::{ContentBlock, Message, Report};
use locode_provider::Provider;
use locode_tools::Registry;
use tokio_util::sync::CancellationToken;

use crate::config::EngineConfig;
use crate::sink::EventSink;

/// One driven agent session. Owns the conversation history **across runs**: a
/// second [`Session::run`] on the same session continues the same conversation
/// (ADR-0016) — the exact call shape an interactive frontend needs for
/// follow-up turns.
///
/// Construct with [`Session::new`], then call [`Session::run`] (or
/// [`Session::run_text`]) to drive one run to a terminal state. `run` is
/// **infallible** — every terminal condition (including provider and `Fatal` tool
/// errors) is captured in the returned [`Report`]'s `status`/`error`, so a caller
/// gets a structured result every time (`locode-exec` maps status → exit code).
///
/// Each [`Report`] is **per-run**: `turns`/`usage`/`tool_calls` count the current
/// run only (a cumulative view is derivable from the event stream). Continuing
/// after a failed run is allowed unconditionally — for `ModelError` the history
/// simply didn't advance, and for `Error` the transcript was fully paired before
/// the break; the pre-send pairing repair heals any residue on the next sample.
pub struct Session {
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) registry: Registry,
    pub(crate) preamble: Vec<Message>,
    pub(crate) config: EngineConfig,
    pub(crate) sink: Box<dyn EventSink>,
    pub(crate) cancel: CancellationToken,
    /// The conversation so far: preamble + every appended turn, across runs.
    pub(crate) history: Vec<Message>,
    /// Runs driven on this session; gates the once-per-session `Init` event.
    pub(crate) turns_run: u32,
}

impl Session {
    /// Assemble a session from its parts.
    ///
    /// `preamble` is the base `System` + `Developer` messages (the pack supplies
    /// these); `provider`/`sink` are trait objects so the binary can select them at
    /// runtime.
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        registry: Registry,
        preamble: Vec<Message>,
        config: EngineConfig,
        sink: Box<dyn EventSink>,
    ) -> Self {
        Self {
            provider,
            registry,
            history: preamble.clone(),
            preamble,
            config,
            sink,
            cancel: CancellationToken::new(),
            turns_run: 0,
        }
    }

    /// The conversation so far: the preamble plus every appended turn across all
    /// runs on this session (ADR-0016). Lets a frontend render the transcript
    /// after a run without replaying the event stream.
    #[must_use]
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Drive the loop to a terminal state and return the run's [`Report`].
    pub async fn run(&mut self, user: Vec<ContentBlock>) -> Report {
        self.drive(user).await
    }

    /// Convenience: drive with a plain-text user prompt.
    pub async fn run_text(&mut self, prompt: impl Into<String>) -> Report {
        self.run(vec![ContentBlock::Text {
            text: prompt.into(),
        }])
        .await
    }
}
