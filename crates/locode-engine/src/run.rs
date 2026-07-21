//! The sample → dispatch → append → re-sample loop (ADR-0005, ADR-0004, ADR-0014).

use locode_protocol::{ContentBlock, Event, Message, Report, ResultChunk, Role};
use locode_provider::{Completion, ConversationRequest, ProviderError};
use locode_tools::ToolCtx;
use serde_json::Value;

use crate::session::Session;
use crate::terminal::{RunAcc, Terminal};

impl Session {
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

        let user_msg = Message {
            role: Role::User,
            content: user_content,
        };
        self.history.push(user_msg.clone());
        self.sink.emit(Event::Message { message: user_msg });

        let mut acc = RunAcc::default();

        let terminal = loop {
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
                Err(err) => {
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
        report
    }

    /// Dispatch one assistant turn's tool calls serially, returning the paired
    /// results and the first `Fatal` message (which aborts the turn). Calls after a
    /// fatal are not run but are still paired with synthetic `is_error` results so
    /// the transcript stays valid (ADR-0004).
    async fn dispatch_batch(
        &self,
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
    ) -> Result<Completion, ProviderError> {
        let mut attempt: u32 = 0;
        loop {
            let completion = self.sample_with_retry(request.clone()).await?;
            let empty = !completion.has_tool_calls() && completion.text().is_none();
            if !empty {
                return Ok(completion);
            }
            if attempt >= self.config.resample_retries {
                return Err(ProviderError::Decode(format!(
                    "model returned an empty completion (no text, no tool calls; \
                     stop: {}) after {attempt} resample(s)",
                    stop_reason_str(&completion.stop)
                )));
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
    async fn sample_with_retry(
        &mut self,
        request: ConversationRequest,
    ) -> Result<Completion, ProviderError> {
        let mut attempt: u32 = 0;
        loop {
            match self.provider.complete(&request).await {
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
                        tokio::time::sleep(backoff).await;
                    }
                    // The history didn't advance — resample the same request.
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn build_report(&self, terminal: Terminal, acc: RunAcc) -> Report {
        let status = terminal.status();
        let (final_message, error) = match terminal {
            Terminal::Completed { final_message } => (final_message, None),
            Terminal::MaxTurns => (acc.last_assistant_text, None),
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
