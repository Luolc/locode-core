//! Parse tests for the Anthropic wire (Task 12, plan §6): id preservation,
//! thinking-signature round-trip (incl. the interleaved fixture), usage mapping,
//! stop-reason mapping, context-overflow folding.

use locode_protocol::{ContentBlock, Message, ReasoningFormat, Role};
use locode_provider::anthropic::wire;
use locode_provider::anthropic::{ModelConfig, build_request, response_to_completion};
use locode_provider::{CacheHint, ConversationRequest, ProviderError, SamplingArgs, StopReason};

fn fixture(name: &str) -> wire::MessagesResponse {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

#[test]
fn tool_use_ids_preserved_verbatim() {
    let completion = response_to_completion(fixture("tool_use_response.json")).expect("parses");
    assert_eq!(completion.stop, StopReason::ToolUse);
    assert!(completion.has_tool_calls());
    let ids: Vec<_> = completion
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec!["toolu_01A09q90qw90lq917835lq9"],
        "no sanitization"
    );
}

#[test]
fn usage_maps_all_four_fields() {
    let completion = response_to_completion(fixture("tool_use_response.json")).expect("parses");
    assert_eq!(completion.usage.input_tokens, 1024);
    assert_eq!(completion.usage.output_tokens, 76);
    assert_eq!(completion.usage.cache_creation_tokens, Some(900));
    assert_eq!(completion.usage.cache_read_tokens, Some(0));
}

/// The load-bearing test (plan §6/§9.3): an interleaved
/// `thinking→tool_use→thinking→tool_use` turn parses with both signatures, and
/// feeding it back through `build_request` re-emits every block in order with
/// the SAME signatures — proving replay end to end.
#[test]
fn interleaved_thinking_signatures_round_trip_through_build() {
    let completion =
        response_to_completion(fixture("interleaved_thinking_response.json")).expect("parses");

    // Parsed: 5 blocks in wire order (incl. redacted thinking), signatures attached.
    assert_eq!(completion.content.len(), 5);
    assert!(matches!(
        &completion.content[3],
        ContentBlock::Reasoning { format: ReasoningFormat::AnthropicRedacted, payload: Some(p), .. }
            if p.as_str() == Some("EncryptedOpaquePayload==")
    ));
    assert!(matches!(
        &completion.content[0],
        ContentBlock::Reasoning { signature: Some(sig), .. }
            if sig.starts_with("EqQBCgIYAhIM1gbcDa9GJwZA")
    ));
    assert!(matches!(
        &completion.content[2],
        ContentBlock::Reasoning { signature: Some(sig), .. }
            if sig == "EqQBCgIYAhIMSecondSignature999AbCdEfGh"
    ));
    assert_eq!(completion.usage.cache_read_tokens, Some(1800));

    // Replay: append the completion as an assistant turn and rebuild.
    let req = ConversationRequest {
        messages: vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "summarize the repo".into(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: completion.content.clone(),
            },
        ],
        tools: vec![],
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Off,
    };
    let cfg = ModelConfig::new("claude-sonnet-5", "https://api.anthropic.com", "k");
    let built = build_request(&req, &cfg);
    let json = serde_json::to_value(&built).expect("serializes");

    let replayed = json["messages"][1]["content"].as_array().expect("blocks");
    assert_eq!(replayed.len(), 5, "all five blocks replayed in order");
    assert_eq!(replayed[0]["type"], "thinking");
    assert_eq!(
        replayed[0]["signature"],
        "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds"
    );
    assert_eq!(replayed[1]["type"], "tool_use");
    assert_eq!(replayed[1]["id"], "toolu_staged_01");
    assert_eq!(replayed[2]["type"], "thinking");
    assert_eq!(
        replayed[2]["signature"],
        "EqQBCgIYAhIMSecondSignature999AbCdEfGh"
    );
    assert_eq!(replayed[3]["type"], "redacted_thinking");
    assert_eq!(
        replayed[3]["data"], "EncryptedOpaquePayload==",
        "encrypted thinking replays verbatim"
    );
    assert_eq!(replayed[4]["type"], "tool_use");
    assert_eq!(replayed[4]["id"], "toolu_staged_02");
}

#[test]
fn context_overflow_with_empty_content_is_terminal() {
    let err = response_to_completion(fixture("context_overflow_response.json"))
        .expect_err("empty overflow folds into the terminal error");
    assert!(matches!(err, ProviderError::ContextOverflow));
    assert!(!err.retryable());
}

fn response_with_stop(stop: Option<&str>) -> wire::MessagesResponse {
    let stop_json = stop.map_or("null".to_string(), |s| format!("\"{s}\""));
    serde_json::from_str(&format!(
        r#"{{
            "id": "msg_x", "type": "message", "role": "assistant",
            "content": [{{"type": "text", "text": "hi"}}],
            "model": "m", "stop_reason": {stop_json},
            "usage": {{"input_tokens": 1, "output_tokens": 1}}
        }}"#
    ))
    .unwrap_or_else(|e| panic!("valid response json: {e}"))
}

#[test]
fn stop_reason_mapping_covers_known_and_unknown() {
    let cases = [
        ("end_turn", StopReason::EndTurn),
        ("max_tokens", StopReason::MaxTokens),
        ("tool_use", StopReason::ToolUse),
        ("stop_sequence", StopReason::StopSequence),
        ("refusal", StopReason::Refusal),
        ("pause_turn", StopReason::PauseTurn),
        (
            "some_future_reason",
            StopReason::Unknown("some_future_reason".into()),
        ),
    ];
    for (raw, expected) in cases {
        let completion =
            response_to_completion(response_with_stop(Some(raw))).expect("never fails parse");
        assert_eq!(completion.stop, expected, "stop_reason {raw}");
    }
}

#[test]
fn overflow_with_partial_content_returns_the_completion() {
    // Non-empty content + overflow stop → the partial completion is usable;
    // the overflow is carried in Unknown, not an error.
    let completion =
        response_to_completion(response_with_stop(Some("model_context_window_exceeded")))
            .expect("partial content is returned");
    assert_eq!(
        completion.stop,
        StopReason::Unknown("model_context_window_exceeded".into())
    );
}

#[test]
fn refusal_is_a_normal_completion() {
    // Plan §9.1: refusal is a Completion{stop: Refusal}; the ENGINE maps it to a
    // terminal report — the wire does not error.
    let completion = response_to_completion(response_with_stop(Some("refusal"))).expect("ok");
    assert_eq!(completion.stop, StopReason::Refusal);
}

/// Pins the live-observed gateway behavior: OpenRouter serving a
/// non-Anthropic model through /v1/messages returns `null` cache counters
/// (`"cache_creation_input_tokens": null`). A `u64` field would fail the
/// whole response; the Option-based usage carries it as `None` (not reported).
#[test]
fn null_usage_counters_parse_as_zero() {
    let resp: wire::MessagesResponse = serde_json::from_str(
        r#"{
            "id": "gen-x", "type": "message", "role": "assistant",
            "content": [{"type": "text", "text": "OK."}],
            "model": "openai/gpt-4o-mini", "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 2,
                      "cache_creation_input_tokens": null,
                      "cache_read_input_tokens": 0}
        }"#,
    )
    .unwrap_or_else(|e| panic!("null counters must not fail the parse: {e}"));
    let completion = response_to_completion(resp).expect("ok");
    assert_eq!(completion.usage.input_tokens, 10);
    assert_eq!(
        completion.usage.cache_creation_tokens, None,
        "null → not reported (the honest N/A, not a fake zero)"
    );
}

#[test]
fn missing_stop_reason_never_invents_one() {
    let completion = response_to_completion(response_with_stop(None)).expect("ok");
    assert!(matches!(completion.stop, StopReason::Unknown(_)));
}
