//! LIVE smoke tests for the OpenAI Responses wire (Task 18 plan §6.4) — manual
//! only, never CI. Run with the direnv env (OpenRouter):
//!
//! ```sh
//! LOCODE_MODEL=openai/gpt-5-mini cargo test -p locode-provider \
//!     --test responses_live_smoke -- --ignored
//! LOCODE_MODEL=x-ai/grok-4.5 SMOKE_XAI=1 cargo test -p locode-provider \
//!     --test responses_live_smoke live_grok -- --ignored
//! ```
//!
//! Proves on the real backend: encrypted-reasoning whole-item replay across
//! turns (no 400), function-tool `call_id` round-trip, custom+grammar tools
//! (OpenAI models), the degraded path (xAI), and cache-read reporting.

use std::collections::HashSet;
use std::fmt::Write as _;

use locode_protocol::{
    ContentBlock, GrammarSyntax, Message, ReasoningFormat, ResultChunk, Role, ToolInputFormat,
    ToolSpec,
};
use locode_provider::openai::responses::OpenAiResponsesProvider;
use locode_provider::{
    CacheHint, ConversationRequest, OpenAiModelConfig, Provider, ReasoningEffort, SamplingArgs,
};

fn word_length_tool() -> ToolSpec {
    ToolSpec {
        name: "word_length".into(),
        description: "Count the letters in a word. Always use this tool for letter counts.".into(),
        input: ToolInputFormat::JsonSchema {
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"word": {"type": "string"}},
                "required": ["word"]
            }),
        },
    }
}

fn echo_patch_tool() -> ToolSpec {
    ToolSpec {
        name: "apply_patch".into(),
        description: "Apply a patch. Emit exactly the raw text `hello` — no JSON.".into(),
        input: ToolInputFormat::Freeform {
            syntax: GrammarSyntax::Lark,
            definition: "start: \"hello\"".into(),
        },
    }
}

fn base_messages() -> Vec<Message> {
    vec![
        Message {
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: "You are locode, a careful headless coding agent under test. \
                       Reason briefly, then act."
                    .repeat(20),
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Which is longer, 'locomotive' or 'perspicacious'? Measure each \
                       with the word_length tool (one call per word, 'locomotive' first)."
                    .into(),
            }],
        },
    ]
}

fn sampling() -> SamplingArgs {
    SamplingArgs {
        max_tokens: 4096,
        reasoning_effort: Some(ReasoningEffort::Low),
        ..SamplingArgs::default()
    }
}

fn write_summary(summary: &str) {
    if let Ok(path) = std::env::var("SMOKE_OUT") {
        std::fs::write(path, summary).unwrap_or_else(|e| panic!("write summary: {e}"));
    }
}

/// The core invariant run, shared by the OpenAI and xAI smokes: a two-turn
/// tool round-trip with reasoning replay + cache accounting.
async fn tool_round_trip(provider: &OpenAiResponsesProvider, summary: &mut String) {
    let turn1 = ConversationRequest {
        messages: base_messages(),
        tools: vec![word_length_tool()],
        sampling_args: sampling(),
        cache_hint: CacheHint::Standard,
    };
    let completion1 = provider
        .complete(&turn1)
        .await
        .unwrap_or_else(|e| panic!("turn 1 failed: {e}"));

    let reasoning_payloads = completion1
        .content
        .iter()
        .filter(|b| {
            matches!(
                b,
                ContentBlock::Reasoning {
                    format: ReasoningFormat::OpenAiResponses,
                    payload: Some(_),
                    ..
                }
            )
        })
        .count();
    assert!(
        reasoning_payloads >= 1,
        "turn 1 must carry an opaque reasoning item: {:?}",
        completion1.content
    );
    assert!(completion1.has_tool_calls(), "turn 1 should call the tool");
    let first_call = completion1
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool_use"));
    let _ = writeln!(
        summary,
        "turn1: reasoning_items={reasoning_payloads} first_call={first_call} usage={:?}",
        completion1.usage
    );

    // Answer EVERY call from turn 1, replay the full assistant turn
    // (reasoning payloads included) — a 400 here = broken whole-item replay.
    let results: Vec<ContentBlock> = completion1
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, input, .. } => Some(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: vec![ResultChunk::Text {
                    text: if input.to_string().contains("locomotive") {
                        "10".into()
                    } else {
                        "13".into()
                    },
                }],
                is_error: false,
            }),
            _ => None,
        })
        .collect();
    let mut messages = base_messages();
    messages.push(Message {
        role: Role::Assistant,
        content: completion1.content.clone(),
    });
    messages.push(Message {
        role: Role::User,
        content: results,
    });
    let turn2 = ConversationRequest {
        messages,
        tools: vec![word_length_tool()],
        sampling_args: sampling(),
        cache_hint: CacheHint::Standard,
    };
    let completion2 = provider
        .complete(&turn2)
        .await
        .unwrap_or_else(|e| panic!("turn 2 failed (reasoning replay?): {e}"));
    let _ = writeln!(
        summary,
        "turn2: text={:?} usage={:?}",
        completion2.text(),
        completion2.usage
    );
    assert!(
        !completion2.content.is_empty(),
        "turn 2 returned an empty completion"
    );
}

#[tokio::test]
#[ignore = "live network + spend; run manually with the direnv env"]
async fn live_openai_tool_and_reasoning_round_trip() {
    let mut provider = OpenAiResponsesProvider::from_env().unwrap_or_else(|e| panic!("env: {e}"));
    // from_env no longer reads LOCODE_MODEL (models are --model/settings
    // territory); the live smoke keeps its env knob test-locally.
    if let Ok(model) = std::env::var("LOCODE_MODEL") {
        provider.config_mut().model = model;
    }
    let mut summary = String::new();
    let _ = writeln!(
        summary,
        "model: {} backend: {:?}",
        provider.config().model,
        provider.config().backend
    );
    tool_round_trip(&provider, &mut summary).await;
    write_summary(&summary);
}

#[tokio::test]
#[ignore = "live network + spend; run manually with the direnv env"]
async fn live_openai_custom_tool_grammar() {
    let mut provider = OpenAiResponsesProvider::from_env().unwrap_or_else(|e| panic!("env: {e}"));
    // from_env no longer reads LOCODE_MODEL (models are --model/settings
    // territory); the live smoke keeps its env knob test-locally.
    if let Ok(model) = std::env::var("LOCODE_MODEL") {
        provider.config_mut().model = model;
    }
    let request = ConversationRequest {
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Call the apply_patch tool with the exact text: hello".into(),
            }],
        }],
        tools: vec![echo_patch_tool()],
        sampling_args: SamplingArgs {
            max_tokens: 512,
            ..SamplingArgs::default()
        },
        cache_hint: CacheHint::Off,
    };
    let completion = provider
        .complete(&request)
        .await
        .unwrap_or_else(|e| panic!("custom tool run failed: {e}"));
    let call = completion
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no custom tool call: {:?}", completion.content));
    assert_eq!(call.0, "apply_patch");
    assert_eq!(
        call.1,
        serde_json::Value::String("hello".into()),
        "grammar-constrained raw text as Value::String"
    );
    write_summary(&format!("custom_tool_call input={:?}\n", call.1));
}

/// xAI grok: function tools + encrypted reasoning through the SAME wire, with
/// the manual degradation flag; run with LOCODE_MODEL=x-ai/grok-4.5.
#[tokio::test]
#[ignore = "live network + spend; run manually with the direnv env"]
async fn live_grok_function_tools_and_reasoning() {
    let mut cfg = OpenAiModelConfig::from_env().unwrap_or_else(|e| panic!("env: {e}"));
    cfg.custom_tools_supported = false; // the manual flag (plan §A.5 Q5)
    let provider = OpenAiResponsesProvider::new(cfg).unwrap_or_else(|e| panic!("provider: {e}"));
    let mut summary = String::new();
    let _ = writeln!(summary, "model: {}", provider.config().model);
    tool_round_trip(&provider, &mut summary).await;

    // The degraded freeform path: the tools array must render apply_patch as a
    // function tool and the exchange must not 422.
    let request = ConversationRequest {
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Call the apply_patch tool with input exactly: hello".into(),
            }],
        }],
        tools: vec![echo_patch_tool()],
        sampling_args: SamplingArgs {
            max_tokens: 2048,
            ..SamplingArgs::default()
        },
        cache_hint: CacheHint::Off,
    };
    let completion = provider
        .complete(&request)
        .await
        .unwrap_or_else(|e| panic!("degraded freeform run failed: {e}"));
    let call_inputs: Vec<serde_json::Value> = completion
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { name, input, .. } if name == "apply_patch" => {
                Some(input.clone())
            }
            _ => None,
        })
        .collect();
    let _ = writeln!(summary, "degraded apply_patch inputs: {call_inputs:?}");
    assert!(
        call_inputs
            .iter()
            .all(|input| matches!(input, serde_json::Value::String(_))),
        "degraded calls must normalize to Value::String: {call_inputs:?}"
    );
    let freeform: HashSet<String> = ["apply_patch".to_string()].into();
    let _ = freeform; // (normalization exercised through complete())
    write_summary(&summary);
}
