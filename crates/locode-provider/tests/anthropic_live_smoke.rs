//! LIVE smoke test for the Anthropic wire (Task 12, plan §9.4) — manual only.
//!
//! Ignored by default so CI never touches the network; run by hand with the
//! direnv env loaded (`LOCODE_API_KEY` / `LOCODE_BASE_URL` / `LOCODE_MODEL`):
//!
//! ```sh
//! cargo test -p locode-provider --test anthropic_live_smoke -- --ignored --nocapture
//! ```
//!
//! Proves on the real backend (OpenRouter in this project's setup):
//! 1. interleaved-thinking replay across turns (signatures echoed, no 400);
//! 2. `cache_control` survives routing (cache tokens non-zero on request 2);
//! 3. one real error body classifies sensibly (invalid model → terminal).
//!
//! Writes a summary to `$SMOKE_OUT` when set (stdout printing is denied
//! workspace-wide).

use std::fmt::Write as _;

use locode_protocol::{ContentBlock, Message, ResultChunk, Role, ToolSpec};
use locode_provider::anthropic::{AnthropicProvider, ModelConfig};
use locode_provider::{
    CacheHint, ConversationRequest, Provider, ProviderError, ReasoningEffort, SamplingArgs,
};

fn word_length_tool() -> ToolSpec {
    ToolSpec {
        name: "word_length".into(),
        description:
            "Count the letters in a word. Always use this tool when asked for a word's length."
                .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "word": {"type": "string", "description": "The word to measure."}
            },
            "required": ["word"]
        }),
    }
}

fn sampling_with_thinking() -> SamplingArgs {
    SamplingArgs {
        max_tokens: 4096,
        reasoning_effort: Some(ReasoningEffort::Low),
        ..SamplingArgs::default()
    }
}

#[tokio::test]
#[ignore = "live network + spend; run manually with the direnv env"]
#[allow(clippy::too_many_lines)] // a linear live scenario reads better unsplit
async fn live_interleaved_thinking_tool_round_trip() {
    let mut cfg = ModelConfig::from_env().unwrap_or_else(|e| panic!("env not configured: {e}"));
    // Pin first-party routing FOR THIS TEST ONLY: the default trio keeps Vertex
    // (production-relevant), but cross-provider routing between the two turns
    // would forfeit the cache-read proof this smoke asserts.
    cfg.provider_prefs = Some(serde_json::json!({
        "only": ["anthropic"], "allow_fallbacks": false, "require_parameters": true
    }));
    let provider = AnthropicProvider::new(cfg).unwrap_or_else(|e| panic!("provider: {e}"));
    let mut summary = String::new();
    let _ = writeln!(summary, "model: {}", provider.config().model);
    let _ = writeln!(summary, "backend: {:?}", provider.config().api_backend);

    // ---- Turn 1: thinking + tool_use expected ----
    let system = Message {
        role: Role::System,
        content: vec![ContentBlock::Text {
            text: "You are locode, a careful headless coding agent under test. \
                   Reason through problems step by step before acting. \
                   A long identity preamble helps exercise prompt caching, so: \
                   you value correct tool pairing, verbatim ids, reproducible \
                   builds, deterministic tests, small diffs, honest reports, \
                   spec-first development, typed contracts, single dispatch \
                   doors, and transcripts that always validate."
                .repeat(8),
        }],
    };
    let turn1 = ConversationRequest {
        messages: vec![
            system.clone(),
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Which is longer, 'locomotive' or 'perspicacious', \
                           and by how many letters? Think it through, then \
                           measure each word with the word_length tool (one \
                           call per word, starting with 'locomotive')."
                        .into(),
                }],
            },
        ],
        tools: vec![word_length_tool()],
        sampling_args: sampling_with_thinking(),
        cache_hint: CacheHint::Standard,
    };

    let completion1 = provider
        .complete(&turn1)
        .await
        .unwrap_or_else(|e| panic!("turn 1 failed: {e}"));

    let thinking_blocks = completion1
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::Thinking { .. }))
        .count();
    assert!(
        thinking_blocks >= 1,
        "turn 1 must produce thinking (first-party routing + budget encoding); \
         zero blocks means the backend dropped the thinking config: {:?}",
        completion1.content
    );
    let signed = completion1
        .content
        .iter()
        .all(|b| !matches!(b, ContentBlock::Thinking { signature, .. } if signature.is_none()));
    assert!(signed, "every thinking block must carry a signature");
    assert!(
        completion1.has_tool_calls(),
        "turn 1 should call word_length; content: {:?}",
        completion1.content
    );
    let tool_use_id = completion1
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool_use in turn 1"));
    let _ = writeln!(
        summary,
        "turn1: thinking_blocks={thinking_blocks} tool_use_id={tool_use_id} usage={:?}",
        completion1.usage
    );

    // ---- Turn 2: replay the full assistant turn (thinking + signature)
    //      and answer the tool call. A 400 here = broken signature replay. ----
    let turn2 = ConversationRequest {
        messages: vec![
            system,
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Which is longer, 'locomotive' or 'perspicacious', \
                           and by how many letters? Think it through, then \
                           measure each word with the word_length tool (one \
                           call per word, starting with 'locomotive')."
                        .into(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: completion1.content.clone(),
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id,
                    content: vec![ResultChunk::Text { text: "10".into() }],
                    is_error: false,
                }],
            },
        ],
        tools: vec![word_length_tool()],
        sampling_args: sampling_with_thinking(),
        cache_hint: CacheHint::Standard,
    };

    let completion2 = provider
        .complete(&turn2)
        .await
        .unwrap_or_else(|e| panic!("turn 2 failed (signature replay?): {e}"));
    // Turn 2 may answer OR call the tool again for the second word — both
    // are valid continuations; what matters is that the replay was accepted.
    let text = completion2.text().unwrap_or_default();
    assert!(
        !completion2.content.is_empty(),
        "turn 2 returned an empty completion"
    );
    let _ = writeln!(
        summary,
        "turn2: text={text:?} usage={:?}",
        completion2.usage
    );

    // ---- Caching proof: turn 2 must actually READ the cache turn 1 wrote
    //      (creation-only on turn 2 = a different backend instance re-wrote it —
    //      which the test-local `only: ["anthropic"]` pin exists to prevent). ----
    assert!(
        completion2.usage.cache_read_tokens > 0,
        "turn 2 did not hit the prompt cache: {:?}",
        completion2.usage
    );

    if let Ok(path) = std::env::var("SMOKE_OUT") {
        std::fs::write(path, &summary).unwrap_or_else(|e| panic!("write summary: {e}"));
    }
}

#[tokio::test]
#[ignore = "live network; run manually with the direnv env"]
async fn live_error_body_classifies_sensibly() {
    let mut cfg = ModelConfig::from_env().unwrap_or_else(|e| panic!("env not configured: {e}"));
    cfg.model = "anthropic/does-not-exist-v0".into();
    let provider = AnthropicProvider::new(cfg).unwrap_or_else(|e| panic!("provider: {e}"));

    let request = ConversationRequest {
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        tools: vec![],
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Off,
    };
    let err = provider
        .complete(&request)
        .await
        .expect_err("invalid model must fail");
    assert!(
        !err.retryable(),
        "an invalid-model error must be terminal, got retryable: {err}"
    );
    assert!(
        matches!(err, ProviderError::Api { .. } | ProviderError::Decode(_)),
        "unexpected classification: {err}"
    );
    if let Ok(path) = std::env::var("SMOKE_OUT") {
        std::fs::write(path, format!("error classification: {err}\n"))
            .unwrap_or_else(|e| panic!("write summary: {e}"));
    }
}
