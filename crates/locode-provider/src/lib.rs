//! locode-provider — the [`Provider`] trait over an API-agnostic [`ConversationRequest`]
//! (ADR-0007), the normalized [`Completion`] response, the [`ProviderError`] taxonomy,
//! a scripted [`MockProvider`], and the streaming [`ToolCallAssembler`].
//!
//! One `Provider` impl = one **wire schema** (Anthropic Messages, OpenAI Chat/Responses,
//! or `mock`); gateways (OpenRouter/Bedrock/proxy) are configuration pointed at a
//! schema, not separate impls. v0 ships `mock` here; the live Anthropic wire is Task 12.
//!
//! [ADR-0007]: https://github.com/Luolc/locode-core/blob/main/docs/decisions/ADR-0007-provider-trait.md

pub mod anthropic;
mod assemble;
mod completion;
mod mock;
mod provider;
mod repair;
mod request;

pub use anthropic::{AnthropicProvider, AuthRefresh};
pub use assemble::{AssembleError, ToolCallAssembler};
pub use completion::{Completion, StopReason};
pub use mock::MockProvider;
pub use provider::{Provider, ProviderError};
pub use repair::{RepairStats, repair_pairing};
pub use request::{CacheHint, ConversationRequest, ReasoningEffort, SamplingArgs};

#[cfg(test)]
mod tests {
    use super::*;
    use locode_protocol::{ContentBlock, Message, ReasoningFormat, Role, Usage};
    use serde_json::json;

    fn text_completion(text: &str) -> Completion {
        Completion {
            content: vec![ContentBlock::Text { text: text.into() }],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        }
    }

    fn tool_call_completion(id: &str, name: &str, input: serde_json::Value) -> Completion {
        Completion {
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
            }],
            usage: Usage::default(),
            stop: StopReason::ToolUse,
        }
    }

    fn empty_request() -> ConversationRequest {
        ConversationRequest {
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text { text: "hi".into() }],
            }],
            tools: vec![],
            sampling_args: SamplingArgs::default(),
            cache_hint: CacheHint::default(),
        }
    }

    #[tokio::test]
    async fn mock_emits_scripted_turns_in_order() {
        // A realistic loop: a tool-call turn, then a final-text turn.
        let mock = MockProvider::new(vec![
            tool_call_completion(
                "c1",
                "run_terminal_command",
                json!({ "command": "echo hi" }),
            ),
            text_completion("done"),
        ]);

        let first = mock.complete(&empty_request()).await.expect("first turn");
        assert!(first.has_tool_calls());
        assert_eq!(first.stop, StopReason::ToolUse);
        assert_eq!(first.tool_uses().count(), 1);

        let second = mock.complete(&empty_request()).await.expect("second turn");
        assert!(!second.has_tool_calls());
        assert_eq!(second.text().as_deref(), Some("done"));
        assert_eq!(second.stop, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn mock_can_script_errors() {
        let mock =
            MockProvider::with_results(vec![Err(ProviderError::RateLimited { retry_after: None })]);
        let err = mock
            .complete(&empty_request())
            .await
            .expect_err("scripted error");
        assert!(err.retryable(), "rate limits should be retryable");
    }

    #[tokio::test]
    #[should_panic(expected = "script exhausted")]
    async fn mock_panics_when_over_consumed() {
        let mock = MockProvider::new(vec![text_completion("only one")]);
        let _ = mock.complete(&empty_request()).await;
        // Second call has no script left → panic.
        let _ = mock.complete(&empty_request()).await;
    }

    #[test]
    fn api_schema_is_the_wire_id() {
        let mock = MockProvider::new(vec![]);
        assert_eq!(mock.api_schema(), "mock");
    }

    #[test]
    fn provider_error_classifies_retryable() {
        assert!(ProviderError::Transport("reset".into()).retryable());
        assert!(
            ProviderError::Api {
                status: 503,
                message: "overloaded".into()
            }
            .retryable()
        );
        assert!(
            !ProviderError::Api {
                status: 400,
                message: "bad request".into()
            }
            .retryable()
        );
        assert!(!ProviderError::ContextOverflow.retryable());
        assert!(!ProviderError::Quota.retryable());
        assert!(!ProviderError::Auth("401".into()).retryable());
    }

    #[test]
    fn completion_preserves_thinking_blocks() {
        // Thinking with a signature must survive in the normalized completion so the
        // engine can replay it (ADR-0013).
        let completion = Completion {
            content: vec![
                ContentBlock::Reasoning {
                    format: ReasoningFormat::Anthropic,
                    text: "let me think".into(),
                    signature: Some("sig-abc".into()),
                    payload: None,
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        };
        assert_eq!(completion.text().as_deref(), Some("answer"));
        assert!(matches!(
            completion.content.first(),
            Some(ContentBlock::Reasoning { signature: Some(sig), .. }) if sig == "sig-abc"
        ));
    }

    // ---- ToolCallAssembler: the partial-JSON accumulation contract ----

    #[test]
    fn assembler_stitches_fragmented_args() {
        let mut asm = ToolCallAssembler::new();
        asm.begin(0, "c1", "run_terminal_command");
        // Fragments that are each invalid JSON on their own.
        asm.push_json(0, "{\"comm").unwrap();
        asm.push_json(0, "and\":\"echo").unwrap();
        asm.push_json(0, " hi\"}").unwrap();

        let blocks = asm.finish().expect("valid once assembled");
        assert_eq!(
            blocks,
            vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "run_terminal_command".into(),
                input: json!({ "command": "echo hi" }),
            }]
        );
    }

    #[test]
    fn assembler_empty_input_becomes_empty_object() {
        let mut asm = ToolCallAssembler::new();
        asm.begin(0, "c1", "list");
        // No push_json — Anthropic sends empty input for no-arg tools.
        let blocks = asm.finish().expect("empty is valid");
        assert_eq!(
            blocks,
            vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "list".into(),
                input: json!({}),
            }]
        );
    }

    #[test]
    fn assembler_preserves_index_order() {
        let mut asm = ToolCallAssembler::new();
        // Begin out of order; finish must yield index order (0 then 1).
        asm.begin(1, "c2", "b");
        asm.begin(0, "c1", "a");
        asm.push_json(1, "{}").unwrap();
        asm.push_json(0, "{}").unwrap();

        let blocks = asm.finish().unwrap();
        let ids: Vec<_> = blocks
            .iter()
            .map(|b| match b {
                ContentBlock::ToolUse { id, .. } => id.as_str(),
                _ => panic!("expected tool_use"),
            })
            .collect();
        assert_eq!(ids, vec!["c1", "c2"]);
    }

    #[test]
    fn assembler_rejects_fragment_without_start() {
        let mut asm = ToolCallAssembler::new();
        let err = asm.push_json(3, "{}").expect_err("no begin at index 3");
        assert!(matches!(err, AssembleError::MissingStart(3)));
    }

    #[test]
    fn assembler_reports_invalid_json() {
        let mut asm = ToolCallAssembler::new();
        asm.begin(0, "c1", "a");
        asm.push_json(0, "{not json").unwrap();
        let err = asm.finish().expect_err("bad json");
        assert!(matches!(err, AssembleError::InvalidJson { index: 0, .. }));
    }
}
