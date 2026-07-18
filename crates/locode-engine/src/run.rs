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
        let mut history = self.preamble.clone();

        // Init: the stream's self-sufficient header (ADR-0014).
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

        let user_msg = Message {
            role: Role::User,
            content: user_content,
        };
        history.push(user_msg.clone());
        self.sink.emit(Event::Message { message: user_msg });

        let mut acc = RunAcc::default();

        let terminal = loop {
            // (a) Pre-send hygiene — unconditional, before every sample (ADR-0004).
            locode_provider::repair_pairing(&mut history);

            // (b) Sample, with the bounded loop-level resample tier (ADR-0007).
            let request = ConversationRequest {
                messages: history.clone(),
                tools: self.registry.specs(),
                sampling_args: self.config.sampling_args.clone(),
                cache_hint: self.config.cache_hint,
            };
            let completion = match self.sample_with_retry(request).await {
                Ok(completion) => completion,
                Err(err) => {
                    break Terminal::ModelError {
                        error: err.to_string(),
                    };
                }
            };
            acc.turns += 1;
            acc.usage += completion.usage;

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
            history.push(assistant_msg.clone());
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
            history.push(tool_msg.clone());
            self.sink.emit(Event::Message { message: tool_msg });

            // (g) Fatal ⇒ Error (transcript already valid — the batch is fully paired).
            if let Some(error) = fatal {
                break Terminal::Error { error };
            }

            // (h) Max-turns, checked AFTER dispatch so the ceiling never severs a
            // tool_use/tool_result pair (grok/claude do the same).
            if acc.turns >= self.config.max_turns {
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
            let dispatched = self.registry.dispatch(&name, input, &ctx).await;
            // TODO(Task 7/9): once `locode-host` lands, apply the shared
            // `truncate_for_model` to `dispatched.tool_result` here, before append.
            results.push(dispatched.tool_result);
            acc.tool_calls.push(dispatched.record);
            if let Some(message) = dispatched.fatal {
                fatal = Some(message);
            }
        }
        (results, fatal)
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
            error,
        }
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
