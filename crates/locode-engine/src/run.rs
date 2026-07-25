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
    fn refresh_project_instructions(&mut self) {
        if !self.config.instructions.enabled {
            return;
        }
        let discovered = locode_instructions::load_project_instructions(
            &self.config.cwd,
            &self.config.instructions,
        );
        let new_hash = locode_instructions::instructions_hash(&discovered);
        if new_hash == self.last_instructions {
            return; // unchanged (including both-empty) — don't re-inject
        }

        let message = match (self.last_instructions, new_hash) {
            // First appearance (or re-appearance after removal): inject fresh, no banner.
            (None, Some(_)) => locode_instructions::render_instructions(
                &discovered,
                self.config.instructions.byte_budget,
                false,
            ),
            // Changed mid-session: re-inject with a replace banner.
            (Some(_), Some(_)) => locode_instructions::render_instructions(
                &discovered,
                self.config.instructions.byte_budget,
                true,
            ),
            // Vanished: emit the removal notice.
            (Some(_), None) => Some(locode_instructions::removal_message()),
            // Both empty is filtered by the equality check above.
            (None, None) => None,
        };
        self.last_instructions = new_hash;
        if let Some(msg) = message {
            self.history.push(msg.clone());
            self.sink.emit(Event::Message { message: msg });
        }
    }

    /// Refresh the skills `<system-reminder>` (ADR-0025 §3.1).
    ///
    /// The comparison unit is the **whole rendered body**: unchanged ⇒ nothing is sent;
    /// changed at all ⇒ the entire listing is re-sent, never a per-skill delta. Whether
    /// a body counts as already delivered is read off the transcript itself, not a
    /// stored hash — which is what makes a compacted-away listing re-appear and a
    /// resumed session stay quiet, with no bookkeeping either way.
    fn refresh_skills(&mut self) {
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
        let message = if let Some(body) = locode_skills::render_body(&discovered.skills, budget) {
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

    /// The driver behind [`Session::run`]. Infallible — all terminal conditions land
    /// in the returned [`Report`].
    pub(crate) async fn drive(&mut self, user_content: Vec<ContentBlock>) -> Report {
        // Init: the stream's self-sufficient header (ADR-0014) — emitted once
        // per session, on the first run only (ADR-0016). A follow-up run
        // continues the same stream, which then carries one `Result` per run
        // (ADR-0014 amendment 2026-07-21).
        if self.turns_run == 0 {
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
        self.turns_run += 1;

        // Project instructions (ADR-0023): rescanned every turn — injected/refreshed after
        // the pack preamble and before this turn's user message (a no-op when unchanged).
        self.refresh_project_instructions();

        // Skills (ADR-0025): the listing rides the same seam, after instructions and
        // before this turn's user message.
        self.refresh_skills();

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
            let (results, fatal) = self.dispatch_batch(calls, &mut acc).await;

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
