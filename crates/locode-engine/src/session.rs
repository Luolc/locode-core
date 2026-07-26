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
    /// Text typed while a run is in flight, drained into the next tool-result
    /// batch (ADR-0028).
    pub(crate) input_queue: crate::InputQueue,
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
            input_queue: crate::InputQueue::new(),
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

    /// Register another discovery root, so the next turn's instruction and skill
    /// rescans see that directory's `AGENTS.md` and `.agents/skills`.
    ///
    /// Only the *config* changes here — widening the tool jail is the host's
    /// job (`Host::add_root`), and the caller does both. Both discoveries
    /// already re-run per turn (ADR-0023 whole-body diff, ADR-0025 post-run
    /// rescan), so nothing needs re-injecting by hand: the added root simply
    /// appears in the next scan.
    pub fn add_root(&mut self, root: std::path::PathBuf) {
        if !self.config.instructions.extra_roots.contains(&root) {
            self.config.instructions.extra_roots.push(root.clone());
        }
        if !self.config.skills.extra_roots.contains(&root) {
            self.config.skills.extra_roots.push(root);
        }
    }

    /// A clonable handle to this session's mid-run input queue (ADR-0028).
    ///
    /// Clone it **before** calling [`Session::run`] — `run` takes `&mut self`,
    /// so nothing on the session is reachable while a turn is in flight. Same
    /// shape as [`Session::cancel_handle`] and for the same reason.
    #[must_use]
    pub fn input_queue(&self) -> crate::InputQueue {
        self.input_queue.clone()
    }

    /// Set the reasoning effort subsequent turns sample with.
    ///
    /// Unlike [`Session::set_model`] this announces nothing: effort changes how
    /// hard the model thinks, not who it is, so there is no stale statement in
    /// the transcript to correct — and injecting a reminder would cost a cache
    /// breakpoint for no gain.
    pub fn set_effort(&mut self, effort: Option<locode_provider::ReasoningEffort>) {
        self.config.sampling_args.reasoning_effort = effort;
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
