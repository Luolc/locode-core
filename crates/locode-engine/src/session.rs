//! The public driving API.

use std::sync::Arc;

use locode_protocol::{ContentBlock, Message, Report};
use locode_provider::Provider;
use locode_tools::Registry;
use tokio_util::sync::CancellationToken;

use crate::config::EngineConfig;
use crate::sink::EventSink;

/// One driven agent session. Owns the conversation history for the run (ephemeral).
///
/// Construct with [`Session::new`], then call [`Session::run`] (or
/// [`Session::run_text`]) to drive one run to a terminal state. `run` is
/// **infallible** — every terminal condition (including provider and `Fatal` tool
/// errors) is captured in the returned [`Report`]'s `status`/`error`, so a caller
/// gets a structured result every time (`locode-exec` maps status → exit code).
pub struct Session {
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) registry: Registry,
    pub(crate) preamble: Vec<Message>,
    pub(crate) config: EngineConfig,
    pub(crate) sink: Box<dyn EventSink>,
    pub(crate) cancel: CancellationToken,
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
            preamble,
            config,
            sink,
            cancel: CancellationToken::new(),
        }
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
