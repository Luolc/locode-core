//! The sample → dispatch → append → re-sample loop (ADR-0005, ADR-0004, ADR-0014).

use locode_protocol::{ContentBlock, Event, Message, Report, ResultChunk, Role, ToolCallRecord};
use locode_provider::{Completion, CompletionDelta, ConversationRequest, ProviderError};
use locode_tools::{ToolCtx, ToolKind};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::approve::{ApprovalRequest, Decision};
use crate::session::Session;
use crate::terminal::{RunAcc, Terminal};

/// Why a sample didn't produce a completion: the run was cancelled mid-await,
/// or the provider failed after the bounded retry budget (ADR-0018 vs ADR-0007
/// — cancellation is a structured stop, not a provider fault).
enum SampleError {
    Cancelled,
    Provider(ProviderError),
}

impl Session {
    /// Refresh the shared project-instruction `<system-reminder>` (ADR-0023), rescanning
    /// every turn. Injects a `User` message on first appearance, a replace-bannered one when
    /// the files changed, or a removal notice when they vanished; a no-op when unchanged or
    /// disabled. Never mutates prior history — the transcript stays immutable. Shared
    /// machinery, identical for every pack.
    ///
    /// **The comparison is against the transcript, not a remembered hash.** ADR-0023's
    /// Refresh rule asks for two things a stored hash cannot both give: "never
    /// double-injected on fork/resume" and "re-injected after compaction". A resumed
    /// session starts with an empty field and re-injects instructions the replayed
    /// transcript already carries; a compacted one keeps the field and never re-injects
    /// what was dropped. Reading the conversation instead gets both right, and is the
    /// resolution ADR-0025 §3.1 already uses for skills.
    fn refresh_project_instructions(&mut self) {
        if !self.config.instructions.enabled {
            return;
        }
        let discovered = locode_instructions::load_project_instructions(
            &self.config.cwd,
            &self.config.instructions,
        );
        let budget = self.config.instructions.byte_budget;
        let message = match locode_instructions::render_body(&discovered, budget) {
            Some(body) => {
                if locode_instructions::already_delivered(&self.history, &body) {
                    return; // this exact body is already in the conversation
                }
                // Anything delivered before — instructions or a removal notice — makes
                // this a *replacement*, which the banner says out loud.
                let replace = locode_instructions::any_delivered(&self.history);
                locode_instructions::render_instructions(&discovered, budget, replace)
            }
            // Nothing to inject: say so once, and only if something was there before.
            None => {
                if locode_instructions::any_delivered(&self.history)
                    && !locode_instructions::removal_delivered(&self.history)
                {
                    Some(locode_instructions::removal_message())
                } else {
                    None
                }
            }
        };
        if let Some(msg) = message {
            self.history.push(msg.clone());
            self.sink.emit(Event::Message { message: msg });
        }
    }

    /// Rescan the skill roots and cache the rendered listing body (ADR-0025 §3.2).
    ///
    /// Deliberately **not** called at the top of a turn: filesystem work there lands on
    /// the user's critical path and reads as a stall. It runs at session start (there is
    /// no earlier turn to hide behind) and again once each run has reached its terminal
    /// state and emitted its `Result`, where the user is reading the reply and a few
    /// hundred milliseconds are invisible.
    ///
    /// Rescanning beats watching for the same reason ADR-0023 gave for `AGENTS.md`: the
    /// walk is small and bounded, so watch/invalidate would buy a filesystem-watcher
    /// dependency for nothing. Net effect: a skill written mid-session is usable on the
    /// very next turn, with no restart.
    pub(crate) fn rescan_skills(&mut self) {
        if !self.config.skills.enabled {
            return;
        }
        let discovered = locode_skills::discover(&self.config.cwd, &self.config.skills);
        for warning in &discovered.warnings {
            self.sink.emit(Event::Error {
                message: warning.clone(),
            });
        }
        let budget = locode_skills::char_budget(self.config.context_window_tokens);
        self.skills_body = locode_skills::render_body(&discovered.skills, budget);
    }

    /// Inject the cached listing when it differs from what the transcript already
    /// carries (ADR-0025 §3.1) — pure bookkeeping, no disk access.
    ///
    /// The comparison unit is the **whole rendered body**: unchanged ⇒ nothing is sent;
    /// changed at all ⇒ the entire listing is re-sent, never a per-skill delta. Whether
    /// a body counts as already delivered is read off the transcript itself, not a
    /// stored hash — which is what makes a compacted-away listing re-appear and a
    /// resumed session stay quiet, with no bookkeeping either way.
    fn inject_skills(&mut self) {
        if !self.config.skills.enabled {
            return;
        }
        let message = if let Some(body) = self.skills_body.clone() {
            if locode_skills::already_delivered(&self.history, &body) {
                return;
            }
            locode_skills::listing_message(&body)
        } else {
            // Nothing listable. Say so only if something *was* listed before — otherwise
            // a skill-less project would open with a pointless denial.
            if !locode_skills::any_listing_delivered(&self.history)
                || locode_skills::already_delivered(&self.history, locode_skills::NO_SKILLS_BODY)
            {
                return;
            }
            locode_skills::removal_message()
        };
        self.history.push(message.clone());
        self.sink.emit(Event::Message { message });
    }

    /// The stream's self-sufficient header (ADR-0014), emitted once per session on the
    /// first run only (ADR-0016) — a follow-up run continues the same stream and carries
    /// its own `Result`.
    fn emit_init(&mut self) {
        let tools: Vec<Value> = self
            .registry
            .specs()
            .iter()
            .filter_map(|spec| serde_json::to_value(spec).ok())
            .collect();
        self.sink.emit(Event::Init {
            session_id: self.config.session_id.clone(),
            harness: self.config.harness.clone(),
            api_schema: self.config.api_schema.clone(),
            model: self.config.model.clone(),
            cwd: self.config.cwd.to_string_lossy().into_owned(),
            max_turns: self.config.max_turns,
            preamble: self.preamble.clone(),
            tools,
        });
    }

    /// The driver behind [`Session::run`]. Infallible — all terminal conditions land
    /// in the returned [`Report`].
    pub(crate) async fn drive(&mut self, user_content: Vec<ContentBlock>) -> Report {
        // Init: the stream's self-sufficient header (ADR-0014) — emitted once
        // per session, on the first run only (ADR-0016). A follow-up run
        // continues the same stream, which then carries one `Result` per run
        // (ADR-0014 amendment 2026-07-21).
        if self.turns_run == 0 {
            self.emit_init();
        }
        self.turns_run += 1;

        // Project instructions (ADR-0023): rescanned every turn — injected/refreshed after
        // the pack preamble and before this turn's user message (a no-op when unchanged).
        self.refresh_project_instructions();

        // Skills (ADR-0025): session start is the one synchronous scan — there is no
        // earlier turn to hide it behind. Every later turn injects a value the previous
        // run already computed (§3.2).
        if self.turns_run == 1 {
            self.rescan_skills();
        }
        self.inject_skills();

        let user_msg = Message {
            role: Role::User,
            content: user_content,
        };
        self.history.push(user_msg.clone());
        self.sink.emit(Event::Message { message: user_msg });

        let mut acc = RunAcc::default();

        let terminal = loop {
            // (0) Cancellation check at the iteration top (ADR-0018): also the
            // exit taken after a mid-batch cancel paired the rest of the batch.
            if self.cancel.is_cancelled() {
                break Terminal::Cancelled;
            }

            // (a) Pre-send hygiene — unconditional, before every sample (ADR-0004).
            locode_provider::repair_pairing(&mut self.history);

            // (b) Sample, with the bounded loop-level resample tier (ADR-0007).
            let request = ConversationRequest {
                messages: self.history.clone(),
                tools: self.registry.specs(),
                sampling_args: self.config.sampling_args.clone(),
                cache_hint: self.config.cache_hint,
            };
            let completion = match self.sample_nonempty(request).await {
                Ok(completion) => completion,
                // Cancelled mid-sample: no assistant message was appended —
                // the history is unchanged since the last append.
                Err(SampleError::Cancelled) => break Terminal::Cancelled,
                Err(SampleError::Provider(err)) => {
                    break Terminal::ModelError {
                        error: err.to_string(),
                    };
                }
            };
            acc.turns += 1;
            acc.usage += completion.usage;
            // Overwritten, not accumulated: the last turn's request carried the whole
            // conversation, so it alone measures how full the context is.
            acc.last_usage = completion.usage;
            acc.last_stop = Some(stop_reason_str(&completion.stop));

            // (c) Extract tool calls, then append the assistant turn VERBATIM so
            // Thinking{signature} blocks are preserved for replay (ADR-0013).
            let calls: Vec<(String, String, Value)> = completion
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();
            let truncated_call = truncated_tool_call(&completion);
            let assistant_text = join_text(&completion.content);
            acc.last_assistant_text = assistant_text.clone();
            let assistant_msg = Message {
                role: Role::Assistant,
                content: completion.content,
            };
            self.history.push(assistant_msg.clone());
            self.sink.emit(Event::Message {
                message: assistant_msg,
            });

            // (d) No tool calls ⇒ Completed. The structural decision keys off the
            // presence of tool_use blocks, not `stop` (which is advisory).
            if calls.is_empty() {
                break Terminal::Completed {
                    final_message: assistant_text,
                };
            }

            // (e) Dispatch the batch SERIALLY (see `dispatch_batch`).
            let (results, fatal) = self
                .dispatch_batch(calls, &mut acc, truncated_call.as_deref())
                .await;

            // (f) Append the result batch as one User message (Anthropic shape).
            let tool_msg = Message {
                role: Role::User,
                content: results,
            };
            self.history.push(tool_msg.clone());
            self.sink.emit(Event::Message { message: tool_msg });

            // (g) Fatal ⇒ Error (transcript already valid — the batch is fully paired).
            if let Some(error) = fatal {
                break Terminal::Error { error };
            }

            // (h) Max-turns (only when a ceiling is configured — unlimited by
            // default, like every studied harness), checked AFTER dispatch so the
            // ceiling never severs a tool_use/tool_result pair (grok/claude do
            // the same).
            if let Some(cap) = self.config.max_turns
                && acc.turns >= cap
            {
                break Terminal::MaxTurns;
            }
        };

        let report = self.build_report(terminal, acc);
        self.sink.emit(Event::Result {
            report: report.clone(),
        });

        // Retire this run's cancel token and install a fresh one (ADR-0018
        // Decision 1: per-run scope). A cancel landing after this point hits
        // the retired token — a harmless no-op — and the next run starts
        // uncancelled; frontends re-fetch `cancel_handle()` each turn.
        self.cancel = CancellationToken::new();

        // Post-run rescan (ADR-0025 §3.2): after the terminal `Result` is out, so the
        // frontend already has everything it needs to render and this work overlaps the
        // user reading the reply instead of delaying their next turn.
        self.rescan_skills();

        report
    }

    /// Dispatch one assistant turn's tool calls serially, returning the paired
    /// results and the first `Fatal` message (which aborts the turn). Calls after a
    /// fatal are not run but are still paired with synthetic `is_error` results so
    /// the transcript stays valid (ADR-0004).
    ///
    /// Each call first passes the approval gate (ADR-0017): the injected
    /// [`Approver`](crate::Approver) is consulted per call — serially, so an
    /// interactive frontend naturally receives one prompt at a time — and every
    /// resolution emits [`Event::Approval`] with the decision latency. A deny is
    /// **soft**: a paired `is_error` result carries the reason to the model,
    /// the record lands in the report with `denial_reason` set, and the run
    /// continues ("deny and stop" is deny + the cancel handle, composed by the
    /// frontend).
    async fn dispatch_batch(
        &mut self,
        calls: Vec<(String, String, Value)>,
        acc: &mut RunAcc,
        truncated_call: Option<&str>,
    ) -> (Vec<ContentBlock>, Option<String>) {
        let mut results: Vec<ContentBlock> = Vec::with_capacity(calls.len());
        let mut fatal: Option<String> = None;
        for (id, name, input) in calls {
            if fatal.is_some() {
                results.push(synthetic_error(
                    &id,
                    "tool not executed: a prior tool in this batch aborted the turn",
                ));
                continue;
            }

            // Between-calls cancellation check (ADR-0018): the currently
            // running tool finished (its own cooperative cancel produced a
            // real result); the rest of the batch is paired synthetically —
            // never recorded, never consulted for approval — and the loop top
            // turns the cancel into the terminal state. These synthetics never
            // carry `denial_reason` (deny and cancel stay separable).
            if self.cancel.is_cancelled() {
                results.push(synthetic_error(
                    &id,
                    "tool not executed: the run was cancelled",
                ));
                continue;
            }

            // Arguments cut off by the output-token limit: never dispatched, so
            // the check sits in front of the approval gate — the user is not
            // asked to approve a call that cannot run.
            if truncated_call == Some(id.as_str()) {
                results.push(synthetic_error(&id, TRUNCATED_TOOL_CALL));
                continue;
            }

            // The approval gate — in front of the dispatch door, so the tools
            // crate stays interaction-free (ADR-0017 Option P1).
            let request = ApprovalRequest {
                tool_use_id: &id,
                tool_name: &name,
                kind: self.registry.kind_of(&name),
                input: &input,
            };
            let asked = std::time::Instant::now();
            let decision = self.approver.decide(&request).await;
            let wait_ms = u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.sink.emit(Event::Approval {
                tool_use_id: id.clone(),
                tool_name: name.clone(),
                decision: match &decision {
                    Decision::Allow => "allow".to_owned(),
                    Decision::Deny { .. } => "deny".to_owned(),
                },
                wait_ms,
            });
            if let Decision::Deny { reason } = decision {
                results.push(synthetic_error(&id, &format!("tool call denied: {reason}")));
                acc.tool_calls.push(denied_record(
                    &id,
                    &name,
                    &input,
                    self.registry.kind_of(&name),
                    reason,
                ));
                continue;
            }

            let ctx = ToolCtx::new(
                self.config.cwd.clone(),
                id.clone(),
                self.config.workspace_root.clone(),
                self.cancel.clone(),
            );
            // The shared model-facing truncation applies inside `dispatch`
            // itself (the dispatch door, ADR-0008 amendment 2026-07-18) — the
            // result arriving here is already bounded.
            let dispatched = self.registry.dispatch(&name, input, &ctx).await;
            results.push(dispatched.tool_result);
            acc.tool_calls.push(dispatched.record);
            if let Some(message) = dispatched.fatal {
                fatal = Some(message);
            }
        }
        (results, fatal)
    }

    /// Sample until the completion is non-empty, spending the same bounded
    /// resample budget on **empty completions** (no text, no tool calls — e.g.
    /// a reasoning-only turn truncated by `max_output_tokens`) as on retryable
    /// provider errors. Grok's rule (`is_empty` responses are resampled); an
    /// engine that labeled these `Completed` would silently poison eval data
    /// (ADR-0005 amendment 2026-07-19).
    async fn sample_nonempty(
        &mut self,
        request: ConversationRequest,
    ) -> Result<Completion, SampleError> {
        let mut attempt: u32 = 0;
        loop {
            let completion = self.sample_with_retry(request.clone()).await?;
            let empty = !completion.has_tool_calls() && completion.text().is_none();
            if !empty {
                return Ok(completion);
            }
            if attempt >= self.config.resample_retries {
                return Err(SampleError::Provider(ProviderError::Decode(format!(
                    "model returned an empty completion (no text, no tool calls; \
                     stop: {}) after {attempt} resample(s)",
                    stop_reason_str(&completion.stop)
                ))));
            }
            attempt += 1;
            self.sink.emit(Event::Error {
                message: format!(
                    "empty completion (stop: {}); resample {attempt}/{}",
                    stop_reason_str(&completion.stop),
                    self.config.resample_retries
                ),
            });
        }
    }

    /// Sample once, retrying retryable provider errors up to the bounded budget.
    ///
    /// Both the provider await and the backoff sleep are guarded by a biased
    /// `select!` on the run's cancel token (ADR-0018): sampling dominates
    /// wall-clock, and dropping the in-flight future aborts the HTTP request
    /// cleanly — the studied harnesses all abort the in-flight request.
    async fn sample_with_retry(
        &mut self,
        request: ConversationRequest,
    ) -> Result<Completion, SampleError> {
        let cancel = self.cancel.clone();
        let streaming = self.config.streaming;
        let provider = std::sync::Arc::clone(&self.provider);
        let mut attempt: u32 = 0;
        loop {
            // Scope the `&mut self.sink` borrow (via `on_delta`) to just the
            // sample await, so the retry arm below can emit on `self.sink` again.
            let result = {
                let sink = &mut self.sink;
                let mut on_delta = |delta: CompletionDelta| {
                    // Slice 1: forward only assistant-text fragments (the UI shows
                    // no thinking, and tool blocks are assembled from the returned
                    // whole `Completion`). `Event::MessageDelta` is display-only.
                    if let CompletionDelta::Text(text) = delta {
                        sink.emit(Event::MessageDelta { text });
                    }
                };
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err(SampleError::Cancelled),
                    result = async {
                        if streaming {
                            provider.stream(&request, &mut on_delta).await
                        } else {
                            provider.complete(&request).await
                        }
                    } => result,
                }
            };
            match result {
                Ok(completion) => return Ok(completion),
                Err(err) if err.retryable() && attempt < self.config.resample_retries => {
                    attempt += 1;
                    self.sink.emit(Event::Error {
                        message: format!(
                            "provider error (resample {attempt}/{}): {err}",
                            self.config.resample_retries
                        ),
                    });
                    let backoff = self.config.resample_backoff * attempt;
                    if !backoff.is_zero() {
                        tokio::select! {
                            biased;
                            () = cancel.cancelled() => return Err(SampleError::Cancelled),
                            () = tokio::time::sleep(backoff) => {}
                        }
                    }
                    // The history didn't advance — resample the same request.
                }
                Err(err) => return Err(SampleError::Provider(err)),
            }
        }
    }

    fn build_report(&self, terminal: Terminal, acc: RunAcc) -> Report {
        let status = terminal.status();
        let (final_message, error) = match terminal {
            Terminal::Completed { final_message } => (final_message, None),
            // Like MaxTurns: the last assistant text of this run, no error —
            // cancelled is a structured stop, not a fault (ADR-0018).
            Terminal::MaxTurns | Terminal::Cancelled => (acc.last_assistant_text, None),
            Terminal::ModelError { error } | Terminal::Error { error } => (None, Some(error)),
        };
        Report {
            schema_version: 1,
            status,
            harness: self.config.harness.clone(),
            api_schema: self.config.api_schema.clone(),
            final_message,
            structured_output: None,
            turns: acc.turns,
            tool_calls: acc.tool_calls,
            usage: acc.usage,
            context_usage: acc.last_usage,
            session_id: self.config.session_id.clone(),
            stop_reason: acc.last_stop,
            error,
        }
    }
}

/// The neutral stop-reason string for the report envelope.
fn stop_reason_str(stop: &locode_provider::StopReason) -> String {
    use locode_provider::StopReason as S;
    match stop {
        S::EndTurn => "end_turn".to_string(),
        S::MaxTokens => "max_tokens".to_string(),
        S::ToolUse => "tool_use".to_string(),
        S::StopSequence => "stop_sequence".to_string(),
        S::Refusal => "refusal".to_string(),
        S::PauseTurn => "pause_turn".to_string(),
        S::Unknown(raw) => raw.clone(),
        // StopReason is #[non_exhaustive] in locode-provider.
        _ => "unknown".to_string(),
    }
}

/// Join the `Text` blocks of a turn with newlines, or `None` if there are none.
fn join_text(content: &[ContentBlock]) -> Option<String> {
    let mut out = String::new();
    for block in content {
        if let ContentBlock::Text { text } = block {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The report-side record for an approver-denied call (never executed):
/// `ok: false`, no output, and `denial_reason` set — the **only** producer of
/// that field (ADR-0017: denial stays structurally separable from failure).
fn denied_record(
    id: &str,
    name: &str,
    input: &Value,
    kind: Option<ToolKind>,
    reason: String,
) -> ToolCallRecord {
    ToolCallRecord {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind.unwrap_or(ToolKind::Other).as_str().to_owned(),
        args: input.clone(),
        ok: false,
        output: Value::Null,
        denial_reason: Some(reason),
    }
}

/// The id of the `tool_use` whose arguments the output-token limit cut short,
/// if this turn ended that way.
///
/// A `max_tokens` stop halts generation mid-block, so ONLY the final content
/// block can be incomplete — which makes the rule precise: `stop` is
/// `max_tokens` *and* the last block is a `tool_use`. Its arguments are then
/// partial, and the Anthropic wire hands us an empty `input` (`{}`) rather
/// than partial JSON, so a typed decode would blame a missing required field
/// and the model would retry the same oversized call (ADR-0004 amendment
/// 2026-07-25).
///
/// A model that lands its last token exactly on the closing brace is a false
/// positive; skipping a complete call is the safe side of that trade — the
/// model simply re-emits it, whereas running a half-written `Write` is not
/// recoverable.
fn truncated_tool_call(completion: &locode_provider::Completion) -> Option<String> {
    if !matches!(completion.stop, locode_provider::StopReason::MaxTokens) {
        return None;
    }
    match completion.content.last() {
        Some(ContentBlock::ToolUse { id, .. }) => Some(id.clone()),
        _ => None,
    }
}

/// The result text for a `tool_use` whose arguments the output-token limit cut
/// short. Names the cause (the previous behavior surfaced the typed decode's
/// "missing field" complaint, which reads as a model mistake) and tells the
/// model not to re-send the call unchanged, which is what made it loop.
const TRUNCATED_TOOL_CALL: &str = "tool not executed: the model reached its output-token limit \
     (stop_reason: max_tokens) while writing this call, so the arguments arrived incomplete. \
     Do not repeat the call unchanged — it will be cut off again. Re-issue it with a smaller \
     payload, splitting the work across several calls (for a file write, write it in parts).";

/// A synthesized `is_error` result to keep an un-run `tool_use` paired.
fn synthetic_error(id: &str, message: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.to_owned(),
        content: vec![ResultChunk::Text {
            text: message.to_owned(),
        }],
        is_error: true,
    }
}
