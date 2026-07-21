//! locode-engine — the sample→dispatch→append loop and the [`Session`] driving API
//! (ADR-0005, ADR-0004, ADR-0014).
//!
//! A [`Session`] drives one run to a terminal [`locode_protocol::Status`] against any
//! [`locode_provider::Provider`], dispatching tool calls through a
//! [`locode_tools::Registry`], emitting `stream-json` events to an [`EventSink`], and
//! returning one [`locode_protocol::Report`]. Proven end-to-end against
//! `MockProvider` with zero network.

mod config;
mod run;
mod session;
mod sink;
mod terminal;

pub use config::EngineConfig;
pub use session::Session;
pub use sink::{EventSink, FnSink, NullSink};

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
