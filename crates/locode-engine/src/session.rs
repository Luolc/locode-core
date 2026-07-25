//! The public driving API.

use std::sync::Arc;

use locode_protocol::{ContentBlock, Event, Message, Report, Role};
use locode_provider::Provider;
use locode_tools::Registry;
use tokio_util::sync::CancellationToken;

use crate::approve::{AllowAll, Approver};
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
    /// The pre-dispatch approval gate (ADR-0017); [`AllowAll`] by default.
    pub(crate) approver: Arc<dyn Approver>,
    /// The listing body from the most recent scan, injected on the next turn
    /// (ADR-0025 §3.2 — scanning happens after a run, never at the top of one).
    pub(crate) skills_body: Option<String>,
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
            approver: Arc::new(AllowAll),
            skills_body: None,
        }
    }

    /// Install an [`Approver`] consulted before every tool call (ADR-0017).
    ///
    /// Builder-style so [`Session::new`]'s signature stays intact. The default
    /// is [`AllowAll`] — headless consumers are unchanged without this call.
    #[must_use]
    pub fn with_approver(mut self, approver: Arc<dyn Approver>) -> Self {
        self.approver = approver;
        self
    }

    /// Switch the model this session samples with, mid-conversation.
    ///
    /// The caller rebuilds the provider (the registry's factory already takes a model
    /// override) and hands it in; this swaps it and updates the config so the trace's
    /// later records name the model actually in use.
    ///
    /// **The preamble is not rewritten.** A pack's system prompt may name the model —
    /// Claude Code's env block does, and so does our port — and after a switch that line
    /// is stale. Rewriting the `System` message would desync the transcript from the
    /// trace, whose `Init` record already captured the original preamble: a resumed
    /// session would replay one preamble while the live session had another. So the
    /// change is **announced instead**, as an appended `<system-reminder>` — the same
    /// never-mutate-history discipline project instructions and skills already follow.
    ///
    /// Returns the announcement, which the caller appends and emits like any other
    /// message. (Doing it here would bypass the sink the caller owns.)
    pub fn set_model(&mut self, provider: Arc<dyn Provider>, model: &str) -> Message {
        self.provider = provider;
        self.config.model = model.to_string();
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!(
                    "<system-reminder>\nThe model for this conversation is now {model}. \
                     Any earlier statement about which model you are is out of date.\n\
                     </system-reminder>"
                ),
            }],
        }
    }

    /// Append a message to the conversation and emit it, exactly as a turn would.
    pub fn announce(&mut self, message: Message) {
        self.history.push(message.clone());
        self.sink.emit(Event::Message { message });
    }

    /// The conversation so far: the preamble plus every appended turn across all
    /// runs on this session (ADR-0016). Lets a frontend render the transcript
    /// after a run without replaying the event stream.
    #[must_use]
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// The cancellation handle for the **current run** (ADR-0018).
    ///
    /// Clone it *before* calling [`Session::run`] (mandatory — `run` takes
    /// `&mut self`, so nothing is callable mid-run) and move it into an Esc
    /// handler, signal handler, or timeout. Firing it stops the run at the
    /// next observation point — mid-sample (the in-flight request is
    /// aborted), between batch calls (the rest of the batch is paired
    /// synthetically), or at the loop top — and the run returns a report with
    /// [`Status::Cancelled`](locode_protocol::Status). Partial work is
    /// preserved: with session continuity, the next `run()` continues the
    /// same conversation.
    ///
    /// The token is **per-run, replaced when `run` returns**: a cancel landing
    /// after the run ended hits the retired token — a harmless no-op — so the
    /// Esc-lands-late race is resolved by construction. Re-fetch the handle
    /// each turn. `cancel()` is idempotent; there is no reset.
    #[must_use]
    pub fn cancel_handle(&self) -> CancellationToken {
        self.cancel.clone()
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
