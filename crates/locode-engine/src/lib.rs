//! locode-engine — the sample→dispatch→append loop and the [`Session`] driving API
//! (ADR-0005, ADR-0004, ADR-0014).
//!
//! A [`Session`] drives one run to a terminal [`locode_protocol::Status`] against any
//! [`locode_provider::Provider`], dispatching tool calls through a
//! [`locode_tools::Registry`], emitting `stream-json` events to an [`EventSink`], and
//! returning one [`locode_protocol::Report`]. Proven end-to-end against
//! `MockProvider` with zero network.

mod approve;
mod config;
mod run;
mod session;
mod sink;
mod terminal;

pub use approve::{AllowAll, ApprovalRequest, Approver, Decision};
pub use config::EngineConfig;
pub use session::Session;
pub use sink::{EventSink, FnSink, NullSink};
// The type `Session::cancel_handle` returns (ADR-0018) — re-exported so
// frontends need no direct tokio-util dependency.
pub use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests {
    // Test tools return `&'static str` literals from `description`; the trait ties it
    // to `&self` so real tools can return a stored field.
    #![allow(clippy::unnecessary_literal_bound)]

    use super::*;
    use async_trait::async_trait;
    use locode_protocol::{
        ContentBlock, Conversation, Event, Message, ReasoningFormat, Role, Status, Usage,
        reconstruct_conversation,
    };
    use locode_provider::{
        Completion, ConversationRequest, MockProvider, Provider, ProviderError, StopReason,
    };
    use locode_tools::{Registry, Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
    use serde::Serialize;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    // ---- trivial in-test tools ----

    #[derive(Serialize)]
    struct EchoOut {
        echoed: String,
    }
    impl ToolOutput for EchoOut {
        fn to_prompt_text(&self) -> String {
            self.echoed.clone()
        }
    }

    struct Echo;
    #[async_trait]
    impl Tool for Echo {
        type Args = Value;
        type Output = EchoOut;
        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }
        fn description(&self) -> &str {
            "echo"
        }
        async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<EchoOut, ToolError> {
            Ok(EchoOut {
                echoed: args.to_string(),
            })
        }
    }

    struct Boom;
    #[async_trait]
    impl Tool for Boom {
        type Args = Value;
        type Output = EchoOut;
        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }
        fn description(&self) -> &str {
            "boom"
        }
        async fn run(&self, _ctx: &ToolCtx, _args: Value) -> Result<EchoOut, ToolError> {
            Err(ToolError::Fatal("boom aborted the turn".into()))
        }
    }

    // ---- harness ----

    fn text_turn(text: &str) -> Completion {
        Completion {
            content: vec![ContentBlock::Text { text: text.into() }],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        }
    }

    fn tool_turn(id: &str, name: &str) -> Completion {
        Completion {
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input: json!({}),
            }],
            usage: Usage::default(),
            stop: StopReason::ToolUse,
        }
    }

    fn config() -> EngineConfig {
        EngineConfig {
            session_id: "sess-1".into(),
            harness: "grok".into(),
            api_schema: "mock".into(),
            model: "mock-1".into(),
            max_turns: None,
            resample_retries: 2,
            resample_backoff: Duration::ZERO, // no real sleeps in tests
            ..EngineConfig::default()
        }
    }

    /// Build a session with a scripted provider + registry, collecting events.
    fn session_with(
        script: Vec<Result<Completion, ProviderError>>,
        registry: Registry,
        cfg: EngineConfig,
    ) -> (Session, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let sink = Box::new(FnSink(move |event| {
            sink_events.lock().unwrap().push(event);
        }));
        let provider = Arc::new(MockProvider::with_results(script));
        let session = Session::new(provider, registry, vec![], cfg, sink);
        (session, events)
    }

    fn echo_registry() -> Registry {
        let mut reg = Registry::new();
        reg.register("echo", Echo);
        reg
    }

    fn dump(events: &Arc<Mutex<Vec<Event>>>) -> Vec<Event> {
        events.lock().unwrap().clone()
    }

    // ---- terminal-state matrix ----

    #[tokio::test]
    async fn completed_with_no_tools() {
        let (mut s, events) =
            session_with(vec![Ok(text_turn("all done"))], Registry::new(), config());
        let report = s.run_text("hi").await;
        assert_eq!(report.status, Status::Completed);
        assert_eq!(report.final_message.as_deref(), Some("all done"));
        assert_eq!(report.turns, 1);
        assert!(report.tool_calls.is_empty());
        assert_eq!(report.api_schema, "mock");
        // Init, Message(user), Message(assistant), Result.
        let evs = dump(&events);
        assert!(matches!(evs.first(), Some(Event::Init { .. })));
        assert!(matches!(evs.last(), Some(Event::Result { .. })));
    }

    // ---- streaming (ADR-0021 slice 1) ----

    #[tokio::test]
    async fn streaming_run_emits_text_deltas_and_the_whole_message() {
        let mut cfg = config();
        cfg.streaming = true;
        let (mut s, events) = session_with(
            vec![Ok(text_turn("hello streamed world"))],
            Registry::new(),
            cfg,
        );
        let report = s.run_text("hi").await;
        assert_eq!(report.status, Status::Completed);
        assert_eq!(
            report.final_message.as_deref(),
            Some("hello streamed world")
        );

        let evs = dump(&events);
        // The deltas concatenate exactly to the finalized assistant text...
        let delta_text: String = evs
            .iter()
            .filter_map(|e| match e {
                Event::MessageDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(delta_text, "hello streamed world", "{evs:?}");
        // ...and there was more than one (proving it actually streamed).
        let n_deltas = evs
            .iter()
            .filter(|e| matches!(e, Event::MessageDelta { .. }))
            .count();
        assert!(n_deltas > 1, "expected multiple deltas, got {n_deltas}");
        // The whole assistant Message is STILL appended — deltas don't replace it.
        assert!(
            evs.iter().any(|e| matches!(
                e,
                Event::Message { message } if message.role == Role::Assistant
            )),
            "whole assistant Message still emitted: {evs:?}"
        );
        // Deltas precede the finalized assistant Message in the stream.
        let first_delta = evs
            .iter()
            .position(|e| matches!(e, Event::MessageDelta { .. }))
            .expect("a delta");
        let asst_msg = evs
            .iter()
            .position(
                |e| matches!(e, Event::Message { message } if message.role == Role::Assistant),
            )
            .expect("assistant message");
        assert!(
            first_delta < asst_msg,
            "deltas come before the whole message"
        );
    }

    #[tokio::test]
    async fn non_streaming_run_emits_no_deltas() {
        let (mut s, events) =
            session_with(vec![Ok(text_turn("no stream"))], Registry::new(), config());
        let _ = s.run_text("hi").await;
        let evs = dump(&events);
        assert!(
            !evs.iter().any(|e| matches!(e, Event::MessageDelta { .. })),
            "default (non-streaming) run must not emit deltas: {evs:?}"
        );
    }

    #[tokio::test]
    async fn streaming_and_non_streaming_reports_match() {
        let (mut a, _ea) = session_with(
            vec![Ok(text_turn("same result"))],
            Registry::new(),
            config(),
        );
        let mut cfg = config();
        cfg.streaming = true;
        let (mut b, _eb) = session_with(vec![Ok(text_turn("same result"))], Registry::new(), cfg);
        let ra = a.run_text("go").await;
        let rb = b.run_text("go").await;
        // Streaming is display-only: the Report is identical either way.
        assert_eq!(ra.status, rb.status);
        assert_eq!(ra.final_message, rb.final_message);
        assert_eq!(ra.turns, rb.turns);
        assert_eq!(ra.tool_calls.len(), rb.tool_calls.len());
    }

    #[tokio::test]
    async fn tool_call_then_complete() {
        let (mut s, _e) = session_with(
            vec![Ok(tool_turn("c1", "echo")), Ok(text_turn("done"))],
            echo_registry(),
            config(),
        );
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::Completed);
        assert_eq!(report.turns, 2);
        assert_eq!(report.tool_calls.len(), 1);
        assert!(report.tool_calls[0].ok);
        assert_eq!(report.tool_calls[0].name, "echo");
    }

    #[tokio::test]
    async fn hits_max_turns_after_dispatch() {
        // Always asks for a tool → never completes; ceiling of 2.
        let mut cfg = config();
        cfg.max_turns = Some(2);
        let (mut s, _e) = session_with(
            vec![
                Ok(tool_turn("c1", "echo")),
                Ok(tool_turn("c2", "echo")),
                Ok(tool_turn("c3", "echo")),
            ],
            echo_registry(),
            cfg,
        );
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::MaxTurns);
        assert_eq!(report.turns, 2);
        assert_eq!(report.tool_calls.len(), 2);
    }

    #[tokio::test]
    async fn model_error_after_bounded_retry() {
        // Retryable every time → 1 + resample_retries attempts, then ModelError.
        let script = vec![
            Err(ProviderError::Transport("reset".into())),
            Err(ProviderError::Transport("reset".into())),
            Err(ProviderError::Transport("reset".into())),
        ];
        let (mut s, events) = session_with(script, Registry::new(), config());
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::ModelError);
        assert!(report.error.is_some());
        assert_eq!(report.turns, 0);
        // Two non-terminal Error retry notes emitted (resample_retries == 2).
        let retries = dump(&events)
            .iter()
            .filter(|e| matches!(e, Event::Error { .. }))
            .count();
        assert_eq!(retries, 2);
    }

    #[tokio::test]
    async fn model_error_non_retryable_is_immediate() {
        let (mut s, events) = session_with(
            vec![Err(ProviderError::ContextOverflow)],
            Registry::new(),
            config(),
        );
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::ModelError);
        let retries = dump(&events)
            .iter()
            .filter(|e| matches!(e, Event::Error { .. }))
            .count();
        assert_eq!(retries, 0, "a non-retryable error must not resample");
    }

    #[tokio::test]
    async fn fatal_tool_error_ends_the_run() {
        let mut reg = Registry::new();
        reg.register("boom", Boom);
        let (mut s, _e) = session_with(vec![Ok(tool_turn("c1", "boom"))], reg, config());
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::Error);
        assert!(report.error.is_some());
        // The boom call still produced a paired (is_error) record.
        assert_eq!(report.tool_calls.len(), 1);
        assert!(!report.tool_calls[0].ok);
    }

    /// An empty completion (no text, no tool calls — e.g. a reasoning-only
    /// turn truncated by `max_output_tokens`) is resampled, not labeled
    /// Completed (ADR-0005 amendment 2026-07-19; grok's `is_empty` rule).
    #[tokio::test]
    async fn empty_completion_resamples_then_succeeds() {
        let empty = Completion {
            content: vec![ContentBlock::Reasoning {
                format: ReasoningFormat::Anthropic,
                text: "thinking only".into(),
                signature: Some("sig".into()),
                payload: None,
            }],
            usage: Usage::default(),
            stop: StopReason::MaxTokens,
        };
        let (mut session, _events) = session_with(
            vec![Ok(empty), Ok(text_turn("recovered"))],
            echo_registry(),
            config(),
        );
        let report = session.run_text("go").await;
        assert_eq!(report.status, Status::Completed);
        assert_eq!(report.final_message.as_deref(), Some("recovered"));
        assert_eq!(report.stop_reason.as_deref(), Some("end_turn"));
    }

    #[tokio::test]
    async fn persistent_empty_completions_are_model_error() {
        let empty = || Completion {
            content: vec![],
            usage: Usage::default(),
            stop: StopReason::MaxTokens,
        };
        // resample_retries = 2 → initial + 2 resamples, all empty → ModelError.
        let (mut session, _events) = session_with(
            vec![Ok(empty()), Ok(empty()), Ok(empty())],
            echo_registry(),
            config(),
        );
        let report = session.run_text("go").await;
        assert_eq!(report.status, Status::ModelError);
        assert!(
            report
                .error
                .as_deref()
                .unwrap_or("")
                .contains("empty completion"),
            "error names the cause: {:?}",
            report.error
        );
        assert_eq!(report.stop_reason, None, "no completion was accepted");
    }

    // ---- transcript hygiene ----

    #[tokio::test]
    async fn mid_batch_abort_synthesizes_results() {
        // One assistant turn asks for TWO tools: boom (Fatal) then echo. echo must
        // not run, yet both tool_use ids must be answered in the transcript.
        let mut reg = Registry::new();
        reg.register("boom", Boom);
        reg.register("echo", Echo);
        let completion = Completion {
            content: vec![
                ContentBlock::ToolUse {
                    id: "c_boom".into(),
                    name: "boom".into(),
                    input: json!({}),
                },
                ContentBlock::ToolUse {
                    id: "c_echo".into(),
                    name: "echo".into(),
                    input: json!({}),
                },
            ],
            usage: Usage::default(),
            stop: StopReason::ToolUse,
        };
        let (mut s, events) = session_with(vec![Ok(completion)], reg, config());
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::Error);

        // The appended tool-result message pairs BOTH ids.
        let evs = dump(&events);
        let answered: Vec<String> = evs
            .iter()
            .filter_map(|e| match e {
                Event::Message { message } if message.role == Role::User => Some(&message.content),
                _ => None,
            })
            .flatten()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect();
        assert!(answered.iter().any(|id| id == "c_boom"));
        assert!(
            answered.iter().any(|id| id == "c_echo"),
            "the un-run echo must be paired"
        );
        // boom recorded (ran, fatal); echo NOT recorded (never executed).
        assert_eq!(report.tool_calls.len(), 1);
    }

    // ---- replay + stream fidelity ----

    #[tokio::test]
    async fn thinking_block_is_appended_verbatim() {
        let completion = Completion {
            content: vec![
                ContentBlock::Reasoning {
                    format: ReasoningFormat::Anthropic,
                    text: "reasoning".into(),
                    signature: Some("sig-xyz".into()),
                    payload: None,
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        };
        let (mut s, events) = session_with(vec![Ok(completion)], Registry::new(), config());
        let report = s.run_text("think").await;
        assert_eq!(report.status, Status::Completed);
        assert_eq!(report.final_message.as_deref(), Some("answer"));
        // The emitted assistant message preserves the Thinking block + signature.
        let has_thinking = dump(&events).iter().any(|e| match e {
            Event::Message { message } if message.role == Role::Assistant => {
                message.content.iter().any(|b| {
                    matches!(
                        b,
                        ContentBlock::Reasoning { signature: Some(sig), .. } if sig == "sig-xyz"
                    )
                })
            }
            _ => false,
        });
        assert!(
            has_thinking,
            "thinking + signature must survive into history"
        );
    }

    #[tokio::test]
    async fn events_reconstruct_the_history() {
        let (mut s, events) = session_with(
            vec![Ok(tool_turn("c1", "echo")), Ok(text_turn("done"))],
            echo_registry(),
            config(),
        );
        let _ = s.run_text("go").await;
        let rebuilt: Conversation = reconstruct_conversation(&dump(&events));
        // user + assistant(tool_use) + user(tool_result) + assistant(text).
        let roles: Vec<Role> = rebuilt.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    // ---- the approval seam (ADR-0017) ----

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A tool that counts its executions — proves a denied call never ran.
    struct Counting(Arc<AtomicUsize>);
    #[async_trait]
    impl Tool for Counting {
        type Args = Value;
        type Output = EchoOut;
        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }
        fn description(&self) -> &str {
            "counting"
        }
        async fn run(&self, _ctx: &ToolCtx, _args: Value) -> Result<EchoOut, ToolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(EchoOut {
                echoed: "ran".into(),
            })
        }
    }

    type SeenKinds = Arc<Mutex<Vec<(String, Option<ToolKind>)>>>;

    /// Denies tools whose name is in the list; allows everything else. Records
    /// the `kind` seen on each request so tests can assert it is populated.
    struct DenyNamed {
        deny: Vec<&'static str>,
        seen_kinds: SeenKinds,
    }
    #[async_trait]
    impl Approver for DenyNamed {
        async fn decide(&self, request: &ApprovalRequest<'_>) -> Decision {
            self.seen_kinds
                .lock()
                .unwrap()
                .push((request.tool_name.to_owned(), request.kind));
            if self.deny.contains(&request.tool_name) {
                Decision::Deny {
                    reason: format!("{} is not allowed here", request.tool_name),
                }
            } else {
                Decision::Allow
            }
        }
    }

    fn approvals(events: &Arc<Mutex<Vec<Event>>>) -> Vec<(String, String, String)> {
        dump(events)
            .iter()
            .filter_map(|e| match e {
                Event::Approval {
                    tool_use_id,
                    tool_name,
                    decision,
                    ..
                } => Some((tool_use_id.clone(), tool_name.clone(), decision.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn deny_is_a_soft_paired_error_and_the_run_continues() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut reg = Registry::new();
        reg.register("counting", Counting(Arc::clone(&ran)));
        let (s, events) = session_with(
            vec![Ok(tool_turn("c1", "counting")), Ok(text_turn("done"))],
            reg,
            config(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut s = s.with_approver(Arc::new(DenyNamed {
            deny: vec!["counting"],
            seen_kinds: Arc::clone(&seen),
        }));
        let report = s.run_text("go").await;

        // Soft: the run continued to Completed; the tool never executed.
        assert_eq!(report.status, Status::Completed);
        assert_eq!(ran.load(Ordering::SeqCst), 0, "denied tool must not run");

        // The record: ok=false, denial_reason set (and only here), no output.
        assert_eq!(report.tool_calls.len(), 1);
        let record = &report.tool_calls[0];
        assert!(!record.ok);
        assert_eq!(
            record.denial_reason.as_deref(),
            Some("counting is not allowed here")
        );
        assert_eq!(record.kind, "shell", "kind still recorded on denial");

        // The transcript: a paired is_error result carrying the reason.
        let denied_result = dump(&events).iter().any(|e| match e {
            Event::Message { message } => message.content.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::ToolResult { tool_use_id, is_error: true, content, .. }
                        if tool_use_id == "c1"
                            && content.iter().any(|c| matches!(
                                c,
                                locode_protocol::ResultChunk::Text { text }
                                    if text == "tool call denied: counting is not allowed here"
                            ))
                )
            }),
            _ => false,
        });
        assert!(denied_result, "the model sees the denial reason, paired");

        // The trace: a deny Approval event for c1.
        assert_eq!(
            approvals(&events),
            vec![("c1".into(), "counting".into(), "deny".into())]
        );
    }

    #[tokio::test]
    async fn deny_then_allow_within_one_batch_keeps_order_and_pairing() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut reg = Registry::new();
        reg.register("blocked", Counting(Arc::clone(&ran)));
        reg.register("echo", Echo);
        let batch = Completion {
            content: vec![
                ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "blocked".into(),
                    input: json!({}),
                },
                ContentBlock::ToolUse {
                    id: "c2".into(),
                    name: "echo".into(),
                    input: json!({}),
                },
            ],
            usage: Usage::default(),
            stop: StopReason::ToolUse,
        };
        let (s, events) = session_with(vec![Ok(batch), Ok(text_turn("done"))], reg, config());
        let mut s = s.with_approver(Arc::new(DenyNamed {
            deny: vec!["blocked"],
            seen_kinds: Arc::new(Mutex::new(Vec::new())),
        }));
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::Completed);
        assert_eq!(ran.load(Ordering::SeqCst), 0);

        // Both calls answered, in call order, denied first.
        let pairs: Vec<(String, bool)> = dump(&events)
            .iter()
            .filter_map(|e| match e {
                Event::Message { message } if message.role == Role::User => Some(&message.content),
                _ => None,
            })
            .flatten()
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => Some((tool_use_id.clone(), *is_error)),
                _ => None,
            })
            .collect();
        assert_eq!(pairs, vec![("c1".into(), true), ("c2".into(), false)]);

        // Records: denied (with reason) then executed (without).
        assert_eq!(report.tool_calls.len(), 2);
        assert!(report.tool_calls[0].denial_reason.is_some());
        assert_eq!(report.tool_calls[0].kind, "shell");
        assert!(report.tool_calls[1].ok);
        assert_eq!(report.tool_calls[1].denial_reason, None);

        // Approval trace: deny then allow, in order.
        assert_eq!(
            approvals(&events),
            vec![
                ("c1".into(), "blocked".into(), "deny".into()),
                ("c2".into(), "echo".into(), "allow".into()),
            ]
        );
    }

    #[tokio::test]
    async fn approval_request_carries_the_registry_kind() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (s, _e) = session_with(
            vec![Ok(tool_turn("c1", "echo")), Ok(text_turn("done"))],
            echo_registry(),
            config(),
        );
        let mut s = s.with_approver(Arc::new(DenyNamed {
            deny: vec![],
            seen_kinds: Arc::clone(&seen),
        }));
        let _ = s.run_text("go").await;
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "echo");
        assert_eq!(
            seen[0].1,
            Some(ToolKind::Shell),
            "kind resolves from the registry pre-dispatch"
        );
    }

    /// An approver that suspends on a oneshot until an external task resolves
    /// it — the exact shape of a TUI prompt. Proves the engine awaits the
    /// decision without deadlocking the run.
    #[tokio::test]
    async fn async_approver_suspends_the_call_until_resolved() {
        struct OneshotApprover(Mutex<Option<tokio::sync::oneshot::Receiver<Decision>>>);
        #[async_trait]
        impl Approver for OneshotApprover {
            async fn decide(&self, _request: &ApprovalRequest<'_>) -> Decision {
                let rx = self.0.lock().unwrap().take().expect("one decision");
                rx.await.expect("decider dropped")
            }
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let (s, _e) = session_with(
            vec![Ok(tool_turn("c1", "echo")), Ok(text_turn("done"))],
            echo_registry(),
            config(),
        );
        let mut s = s.with_approver(Arc::new(OneshotApprover(Mutex::new(Some(rx)))));

        // Resolve the prompt from "the UI" after the run has started.
        let ui = tokio::spawn(async move {
            tokio::task::yield_now().await;
            let _ = tx.send(Decision::Allow);
        });
        let report = s.run_text("go").await;
        ui.await.expect("ui task");
        assert_eq!(report.status, Status::Completed);
        assert_eq!(report.tool_calls.len(), 1);
        assert!(report.tool_calls[0].ok);
    }

    #[tokio::test]
    async fn allowed_calls_emit_approval_events_by_default() {
        // The default AllowAll approver still journals every resolution.
        let (mut s, events) = session_with(
            vec![Ok(tool_turn("c1", "echo")), Ok(text_turn("done"))],
            echo_registry(),
            config(),
        );
        let report = s.run_text("go").await;
        assert_eq!(report.status, Status::Completed);
        assert_eq!(
            approvals(&events),
            vec![("c1".into(), "echo".into(), "allow".into())]
        );
        // And denial_reason is absent on ordinary success records.
        assert_eq!(report.tool_calls[0].denial_reason, None);
    }

    // ---- cancellation (ADR-0018) ----

    /// A provider whose sample never returns on its own — cancellation is the
    /// only way out (models a long in-flight request).
    struct HangingProvider;
    #[async_trait]
    impl Provider for HangingProvider {
        #[allow(clippy::unnecessary_literal_bound)]
        fn api_schema(&self) -> &str {
            "mock"
        }
        async fn complete(
            &self,
            _request: &ConversationRequest,
        ) -> Result<Completion, ProviderError> {
            tokio::time::sleep(Duration::from_hours(1)).await;
            Err(ProviderError::Transport("unreachable".into()))
        }
    }

    /// A tool that parks on its ctx cancel token and returns cleanly once it
    /// fires — the cooperative-cancel shape the host implements for real.
    struct WaitsForCancel;
    #[async_trait]
    impl Tool for WaitsForCancel {
        type Args = Value;
        type Output = EchoOut;
        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }
        fn description(&self) -> &str {
            "waits"
        }
        async fn run(&self, ctx: &ToolCtx, _args: Value) -> Result<EchoOut, ToolError> {
            ctx.cancel.cancelled().await;
            Ok(EchoOut {
                echoed: "stopped cooperatively".into(),
            })
        }
    }

    #[tokio::test]
    async fn cancel_mid_sample_yields_cancelled_report() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let sink = Box::new(FnSink(move |event| {
            sink_events.lock().unwrap().push(event);
        }));
        let mut s = Session::new(
            Arc::new(HangingProvider),
            Registry::new(),
            vec![],
            config(),
            sink,
        );
        let handle = s.cancel_handle();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            handle.cancel();
            handle.cancel(); // idempotent double-cancel
        });
        let report = s.run_text("go").await;
        canceller.await.expect("canceller");

        assert_eq!(report.status, Status::Cancelled);
        assert_eq!(report.error, None, "cancelled is a stop, not a fault");
        assert_eq!(report.final_message, None, "no assistant text this run");
        assert_eq!(report.turns, 0, "no completion was accepted");
        // No assistant message was appended: history is user-prompt only.
        let roles: Vec<Role> = s.history().iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::User]);
        // The stream still terminates in a Result carrying the same report.
        let evs = dump(&events);
        assert!(
            matches!(evs.last(), Some(Event::Result { report }) if report.status == Status::Cancelled)
        );
    }

    #[tokio::test]
    async fn cancel_mid_batch_pairs_the_rest_synthetically() {
        // One turn asks for TWO tools: a cooperative waiter, then echo. The
        // cancel fires while the waiter runs → its own result is real; echo is
        // never run (no approval consult, no record) but still paired.
        let mut reg = Registry::new();
        reg.register("waits", WaitsForCancel);
        reg.register("echo", Echo);
        let batch = Completion {
            content: vec![
                ContentBlock::ToolUse {
                    id: "c_wait".into(),
                    name: "waits".into(),
                    input: json!({}),
                },
                ContentBlock::ToolUse {
                    id: "c_echo".into(),
                    name: "echo".into(),
                    input: json!({}),
                },
            ],
            usage: Usage::default(),
            stop: StopReason::ToolUse,
        };
        let (s, events) = session_with(vec![Ok(batch)], reg, config());
        let mut s = s; // provider script has ONE turn: cancel must end the run
        let handle = s.cancel_handle();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            handle.cancel();
        });
        let report = s.run_text("go").await;
        canceller.await.expect("canceller");

        assert_eq!(report.status, Status::Cancelled);
        // The waiter executed (cooperatively) and is the only record; no
        // cancellation synthetic ever carries denial_reason.
        assert_eq!(report.tool_calls.len(), 1);
        assert_eq!(report.tool_calls[0].id, "c_wait");
        assert!(report.tool_calls[0].ok);
        assert_eq!(report.tool_calls[0].denial_reason, None);

        // Both tool_use ids are answered: real result + cancellation synthetic.
        let pairs: Vec<(String, bool)> = dump(&events)
            .iter()
            .filter_map(|e| match e {
                Event::Message { message } if message.role == Role::User => Some(&message.content),
                _ => None,
            })
            .flatten()
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => Some((tool_use_id.clone(), *is_error)),
                _ => None,
            })
            .collect();
        assert_eq!(
            pairs,
            vec![("c_wait".into(), false), ("c_echo".into(), true)]
        );
        // Only the executed call was consulted for approval.
        assert_eq!(
            approvals(&events),
            vec![("c_wait".into(), "waits".into(), "allow".into())]
        );
    }

    /// The token is per-run (ADR-0018 Decision 1): a cancelled run 1 must not
    /// poison run 2, and run 2 continues the same conversation (with ADR-0016).
    #[tokio::test]
    async fn cancelled_session_continues_on_the_next_run_with_a_fresh_token() {
        let mut reg = Registry::new();
        reg.register("waits", WaitsForCancel);
        let (s, _e) = session_with(
            vec![Ok(tool_turn("c1", "waits")), Ok(text_turn("second run"))],
            reg,
            config(),
        );
        let mut s = s;
        let handle1 = s.cancel_handle();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            handle1.cancel();
        });
        let r1 = s.run_text("q1").await;
        canceller.await.expect("canceller");
        assert_eq!(r1.status, Status::Cancelled);

        // The retired handle stays cancelled, but the session got a fresh
        // token at run end — run 2 must not see the old cancel.
        assert!(!s.cancel_handle().is_cancelled());
        let r2 = s.run_text("q2").await;
        assert_eq!(r2.status, Status::Completed);
        assert_eq!(r2.final_message.as_deref(), Some("second run"));
        // Continuity intact: q1's turns are still in the history.
        assert!(s.history().len() >= 4, "history: {:?}", s.history().len());
    }

    // ---- session continuity (ADR-0016) ----

    /// A scripted provider that also records each request's message array, so a
    /// test can assert what the model actually saw on a follow-up run.
    struct CapturingProvider {
        inner: MockProvider,
        requests: Arc<Mutex<Vec<Vec<Message>>>>,
    }
    #[async_trait]
    impl Provider for CapturingProvider {
        #[allow(clippy::unnecessary_literal_bound)]
        fn api_schema(&self) -> &str {
            "mock"
        }
        async fn complete(
            &self,
            request: &ConversationRequest,
        ) -> Result<Completion, ProviderError> {
            self.requests.lock().unwrap().push(request.messages.clone());
            self.inner.complete(request).await
        }
    }

    /// Like `session_with`, but the provider records every request's messages.
    #[allow(clippy::type_complexity)]
    fn capturing_session_with(
        script: Vec<Result<Completion, ProviderError>>,
        registry: Registry,
    ) -> (
        Session,
        Arc<Mutex<Vec<Vec<Message>>>>,
        Arc<Mutex<Vec<Event>>>,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let sink = Box::new(FnSink(move |event| {
            sink_events.lock().unwrap().push(event);
        }));
        let provider = Arc::new(CapturingProvider {
            inner: MockProvider::with_results(script),
            requests: Arc::clone(&requests),
        });
        let session = Session::new(provider, registry, vec![], config(), sink);
        (session, requests, events)
    }

    fn user_text(message: &Message) -> Option<&str> {
        match (message.role, message.content.as_slice()) {
            (Role::User, [ContentBlock::Text { text }]) => Some(text.as_str()),
            _ => None,
        }
    }

    #[tokio::test]
    async fn second_run_continues_the_conversation() {
        let (mut s, requests, _e) = capturing_session_with(
            vec![
                Ok(text_turn("first answer")),
                Ok(text_turn("second answer")),
            ],
            Registry::new(),
        );
        let r1 = s.run_text("q1").await;
        let r2 = s.run_text("q2").await;
        assert_eq!(r1.status, Status::Completed);
        assert_eq!(r2.status, Status::Completed);
        assert_eq!(r2.final_message.as_deref(), Some("second answer"));

        // Run 2's request contains run 1's full exchange, then the new prompt.
        let reqs = requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        let run2 = &reqs[1];
        assert_eq!(run2.len(), 3, "user q1, assistant, user q2: {run2:?}");
        assert_eq!(user_text(&run2[0]), Some("q1"));
        assert_eq!(run2[1].role, Role::Assistant);
        assert_eq!(user_text(&run2[2]), Some("q2"));

        // The public accessor exposes the same transcript (empty test preamble).
        let roles: Vec<Role> = s.history().iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    #[tokio::test]
    async fn init_emitted_once_across_runs_with_one_result_each() {
        let (mut s, events) = session_with(
            vec![Ok(text_turn("one")), Ok(text_turn("two"))],
            Registry::new(),
            config(),
        );
        let _ = s.run_text("q1").await;
        let _ = s.run_text("q2").await;
        let evs = dump(&events);
        let inits = evs
            .iter()
            .filter(|e| matches!(e, Event::Init { .. }))
            .count();
        let results = evs
            .iter()
            .filter(|e| matches!(e, Event::Result { .. }))
            .count();
        assert_eq!(inits, 1, "Init is once per session, not per run");
        assert_eq!(results, 2, "one Result per run");
        assert!(
            matches!(evs.first(), Some(Event::Init { .. })),
            "Init still opens the stream"
        );
    }

    #[tokio::test]
    async fn report_counts_are_per_run_not_cumulative() {
        // Run 1: tool turn + text (2 turns, 1 tool call, 10/5 tokens).
        // Run 2: text only (1 turn, 0 tool calls, 20/7 tokens).
        let mut t1 = tool_turn("c1", "echo");
        t1.usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Usage::default()
        };
        let t2 = text_turn("done one");
        let mut t3 = text_turn("done two");
        t3.usage = Usage {
            input_tokens: 20,
            output_tokens: 7,
            ..Usage::default()
        };
        let (mut s, _e) = session_with(vec![Ok(t1), Ok(t2), Ok(t3)], echo_registry(), config());
        let r1 = s.run_text("q1").await;
        let r2 = s.run_text("q2").await;
        assert_eq!(r1.turns, 2);
        assert_eq!(r1.tool_calls.len(), 1);
        assert_eq!(r2.turns, 1, "run 2 counts its own turns only");
        assert!(r2.tool_calls.is_empty());
        assert_eq!(r2.usage.input_tokens, 20, "usage is per-run");
        assert_eq!(r2.usage.output_tokens, 7);
    }

    /// Golden: a two-run stream (`Init M+ Result M+ Result`) reconstructs the
    /// full cross-run conversation (ADR-0014 amendment 2026-07-21).
    #[tokio::test]
    async fn two_run_stream_reconstructs_the_full_conversation() {
        let (mut s, events) = session_with(
            vec![
                Ok(tool_turn("c1", "echo")),
                Ok(text_turn("done one")),
                Ok(text_turn("done two")),
            ],
            echo_registry(),
            config(),
        );
        let _ = s.run_text("q1").await;
        let _ = s.run_text("q2").await;
        let rebuilt: Conversation = reconstruct_conversation(&dump(&events));
        // Run 1: user, assistant(tool_use), user(tool_result), assistant(text);
        // run 2: user, assistant(text).
        let roles: Vec<Role> = rebuilt.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
            ]
        );
        // And the reconstruction matches the session's own history exactly.
        assert_eq!(rebuilt.messages.as_slice(), s.history());
    }

    /// Continuing after a `ModelError` run is allowed unconditionally
    /// (ADR-0016 Resolution): the history simply didn't advance.
    #[tokio::test]
    async fn continues_after_model_error() {
        let (mut s, requests, _e) = capturing_session_with(
            vec![Err(ProviderError::ContextOverflow), Ok(text_turn("ok now"))],
            Registry::new(),
        );
        let r1 = s.run_text("q1").await;
        let r2 = s.run_text("q2").await;
        assert_eq!(r1.status, Status::ModelError);
        assert_eq!(r2.status, Status::Completed);
        // Run 2's request: q1's user message survived; no phantom assistant turn.
        let reqs = requests.lock().unwrap();
        let run2 = &reqs[1];
        assert_eq!(run2.len(), 2, "user q1 + user q2: {run2:?}");
        assert_eq!(user_text(&run2[0]), Some("q1"));
        assert_eq!(user_text(&run2[1]), Some("q2"));
    }

    /// Continuing after a fatal tool `Error` run: the transcript was fully
    /// paired before the break, so the next sample sees a valid history.
    #[tokio::test]
    async fn continues_after_fatal_tool_error_with_valid_pairing() {
        let mut reg = Registry::new();
        reg.register("boom", Boom);
        let (mut s, requests, _e) = capturing_session_with(
            vec![Ok(tool_turn("c1", "boom")), Ok(text_turn("recovered"))],
            reg,
        );
        let r1 = s.run_text("q1").await;
        let r2 = s.run_text("q2").await;
        assert_eq!(r1.status, Status::Error);
        assert_eq!(r2.status, Status::Completed);

        // Run 2's request replays the failed run intact: the boom tool_use is
        // answered by its (is_error) tool_result.
        let reqs = requests.lock().unwrap();
        let run2 = &reqs[1];
        assert_eq!(run2.len(), 4, "q1, assistant, tool_result, q2: {run2:?}");
        assert!(
            run2[1]
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "c1"))
        );
        assert!(run2[2].content.iter().any(|b| matches!(
            b,
            ContentBlock::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "c1"
        )));
        assert_eq!(user_text(&run2[3]), Some("q2"));
    }

    #[tokio::test]
    async fn usage_is_summed_across_turns() {
        let mut first = tool_turn("c1", "echo");
        first.usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Usage::default()
        };
        let mut second = text_turn("done");
        second.usage = Usage {
            input_tokens: 20,
            output_tokens: 7,
            ..Usage::default()
        };
        let (mut s, _e) = session_with(vec![Ok(first), Ok(second)], echo_registry(), config());
        let report = s.run_text("go").await;
        assert_eq!(report.usage.input_tokens, 30);
        assert_eq!(report.usage.output_tokens, 12);
    }
}
