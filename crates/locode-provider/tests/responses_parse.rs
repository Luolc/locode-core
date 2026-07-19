//! Parse tests for the OpenAI Responses wire (Task 18, plan §6): whole-item
//! reasoning capture with unknown-field round-trip, call-id preservation,
//! degraded-arguments unwrap, stop/usage mapping, failed-response mapping.

use std::collections::HashSet;

use locode_protocol::{ContentBlock, ReasoningFormat};
use locode_provider::openai::responses::{response_to_completion, wire};
use locode_provider::{ProviderError, StopReason};
use serde_json::{Value, json};

fn parse(body: Value, freeform: &[&str]) -> Result<locode_provider::Completion, ProviderError> {
    let resp: wire::ResponsesResponse =
        serde_json::from_value(body).unwrap_or_else(|e| panic!("deserializes: {e}"));
    let names: HashSet<String> = freeform.iter().map(ToString::to_string).collect();
    response_to_completion(resp, &names)
}

#[test]
fn reasoning_captured_whole_with_unknown_fields() {
    // The probe-P4 shape: fields we never modeled must round-trip byte-true.
    let item = json!({"type": "reasoning", "id": "rs_x",
        "summary": [{"type": "summary_text", "text": "first"},
                     {"type": "summary_text", "text": "second"}],
        "encrypted_content": "gAAAAB",
        "format": "xai-responses-v1",
        "some_future_field": {"nested": [1, 2, 3]}});
    let completion = parse(
        json!({"status": "completed", "output": [item.clone(),
            {"type": "message", "content": [{"type": "output_text", "text": "hi"}]}],
            "usage": {"input_tokens": 5, "output_tokens": 2}}),
        &[],
    )
    .expect("ok");
    match &completion.content[0] {
        ContentBlock::Reasoning {
            format: ReasoningFormat::OpenAiResponses,
            text,
            payload: Some(payload),
            ..
        } => {
            assert_eq!(text, "first\n\nsecond", "summary concatenation");
            assert_eq!(payload, &item, "the WHOLE item, byte-true");
        }
        other => panic!("expected openai_responses reasoning, got {other:?}"),
    }
}

#[test]
fn function_and_custom_calls_map_with_verbatim_call_ids() {
    let completion = parse(
        json!({"status": "completed", "output": [
            {"type": "function_call", "id": "fc_1", "call_id": "call-x1",
             "name": "word_length", "arguments": "{\"word\":\"hi\"}"},
            {"type": "custom_tool_call", "call_id": "call-x2",
             "name": "apply_patch", "input": "*** Begin Patch"}
        ]}),
        &["apply_patch"],
    )
    .expect("ok");
    assert_eq!(completion.stop, StopReason::ToolUse);
    match &completion.content[0] {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "call-x1", "call_id, not the item id");
            assert_eq!(name, "word_length");
            assert_eq!(input["word"], "hi");
        }
        other => panic!("{other:?}"),
    }
    match &completion.content[1] {
        ContentBlock::ToolUse { id, input, .. } => {
            assert_eq!(id, "call-x2");
            assert_eq!(input, &Value::String("*** Begin Patch".into()));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn degraded_freeform_arguments_unwrap_to_the_raw_string() {
    // A degraded backend returns a function_call whose arguments wrap the raw
    // text — the tool must see the identical Value::String either way.
    let completion = parse(
        json!({"status": "completed", "output": [
            {"type": "function_call", "call_id": "c1", "name": "apply_patch",
             "arguments": "{\"input\":\"*** Begin Patch\"}"}
        ]}),
        &["apply_patch"],
    )
    .expect("ok");
    match &completion.content[0] {
        ContentBlock::ToolUse { input, .. } => {
            assert_eq!(
                input,
                &Value::String("*** Begin Patch".into()),
                "delivery-invisible normalization"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn invalid_arguments_are_kept_as_string_for_a_soft_error() {
    let completion = parse(
        json!({"status": "completed", "output": [
            {"type": "function_call", "call_id": "c1", "name": "shell",
             "arguments": "{not json"}
        ]}),
        &[],
    )
    .expect("ok");
    match &completion.content[0] {
        ContentBlock::ToolUse { input, .. } => {
            assert_eq!(input, &Value::String("{not json".into()));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn stop_mapping_completed_incomplete_refusal() {
    let done = parse(
        json!({"status": "completed", "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "hi"}]}]}),
        &[],
    )
    .expect("ok");
    assert_eq!(done.stop, StopReason::EndTurn);

    let truncated = parse(
        json!({"status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "output": [{"type": "message",
                "content": [{"type": "output_text", "text": "partial"}]}]}),
        &[],
    )
    .expect("ok");
    assert_eq!(
        truncated.stop,
        StopReason::MaxTokens,
        "truncation is visible"
    );

    let refused = parse(
        json!({"status": "completed", "output": [
            {"type": "message", "content": [{"type": "refusal",
                "refusal": "cannot help with that"}]}]}),
        &[],
    )
    .expect("ok");
    assert_eq!(refused.stop, StopReason::Refusal);
    assert_eq!(refused.text().as_deref(), Some("cannot help with that"));
}

#[test]
fn failed_response_maps_by_error_code() {
    let rate = parse(
        json!({"status": "failed",
            "error": {"code": "rate_limit_exceeded", "message": "slow down"},
            "output": []}),
        &[],
    )
    .expect_err("failed");
    assert!(matches!(rate, ProviderError::RateLimited { .. }));

    let server = parse(
        json!({"status": "failed",
            "error": {"code": "server_error", "message": "boom"}, "output": []}),
        &[],
    )
    .expect_err("failed");
    assert!(matches!(server, ProviderError::Api { status: 500, .. }));
    assert!(server.retryable(), "grok converts these to retryable 500s");

    let other = parse(
        json!({"status": "failed",
            "error": {"code": "invalid_prompt", "message": "bad"}, "output": []}),
        &[],
    )
    .expect_err("failed");
    assert!(!other.retryable());
}

#[test]
fn usage_maps_option_through_with_reasoning_tokens() {
    let completion = parse(
        json!({"status": "completed",
            "output": [{"type": "message",
                "content": [{"type": "output_text", "text": "hi"}]}],
            "usage": {"input_tokens": 100, "output_tokens": 30,
                "input_tokens_details": {"cached_tokens": 80},
                "output_tokens_details": {"reasoning_tokens": 12}}}),
        &[],
    )
    .expect("ok");
    assert_eq!(completion.usage.input_tokens, 100);
    assert_eq!(completion.usage.cache_read_tokens, Some(80));
    assert_eq!(
        completion.usage.cache_creation_tokens, None,
        "never reported here → honest None"
    );
    assert_eq!(completion.usage.reasoning_tokens, Some(12));
}

#[test]
fn unknown_output_item_types_never_fail_the_parse() {
    let completion = parse(
        json!({"status": "completed", "output": [
            {"type": "web_search_call", "id": "ws_1", "status": "completed"},
            {"type": "some_2027_item", "who": "knows"},
            {"type": "message", "content": [{"type": "output_text", "text": "hi"}]}
        ]}),
        &[],
    )
    .expect("hosted/future items are ignored");
    assert_eq!(completion.text().as_deref(), Some("hi"));
}
