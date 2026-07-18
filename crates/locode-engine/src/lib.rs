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
        ContentBlock, Conversation, Event, Role, Status, Usage, reconstruct_conversation,
    };
    use locode_provider::{Completion, MockProvider, ProviderError, StopReason};
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
                ContentBlock::Thinking {
                    text: "reasoning".into(),
                    signature: Some("sig-xyz".into()),
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
                        ContentBlock::Thinking { signature: Some(sig), .. } if sig == "sig-xyz"
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
