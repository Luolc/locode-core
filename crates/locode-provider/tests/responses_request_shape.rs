//! Request-shape tests for the OpenAI Responses wire (Task 18, plan §6):
//! stateless non-negotiables, the instructions hoist, item flattening,
//! whole-item reasoning replay, freeform emission + degradation, effort/summary.

use locode_protocol::{
    ContentBlock, GrammarSyntax, Message, ReasoningFormat, ResultChunk, Role, ToolInputFormat,
    ToolSpec,
};
use locode_provider::openai::responses::build_request;
use locode_provider::{
    CacheHint, ConversationRequest, OpenAiModelConfig, ReasoningEffort, SamplingArgs,
};
use serde_json::{Value, json};

fn cfg() -> OpenAiModelConfig {
    OpenAiModelConfig::new("openai/gpt-5-mini", "https://openrouter.ai/api", "sk-or-x")
}

fn native_cfg() -> OpenAiModelConfig {
    OpenAiModelConfig::new("gpt-5-mini", "https://api.openai.com", "sk-x")
}

fn text(t: &str) -> ContentBlock {
    ContentBlock::Text { text: t.into() }
}

fn msg(role: Role, blocks: Vec<ContentBlock>) -> Message {
    Message {
        role,
        content: blocks,
    }
}

fn request(messages: Vec<Message>) -> ConversationRequest {
    ConversationRequest {
        messages,
        tools: vec![],
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Standard,
    }
}

fn function_tool(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "a function tool".into(),
        input: ToolInputFormat::JsonSchema {
            parameters: json!({"type": "object", "properties": {}}),
        },
    }
}

fn freeform_tool(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "a freeform tool".into(),
        input: ToolInputFormat::Freeform {
            syntax: GrammarSyntax::Lark,
            definition: "start: \"hello\"".into(),
        },
    }
}

#[test]
fn stateless_non_negotiables() {
    let built = build_request(&request(vec![msg(Role::User, vec![text("hi")])]), &cfg());
    let json = serde_json::to_value(&built).unwrap();
    assert_eq!(json["store"], false, "store:false always");
    assert_eq!(json["stream"], false);
    assert_eq!(json["include"][0], "reasoning.encrypted_content");
    assert!(
        json.get("previous_response_id").is_none(),
        "the field must not even exist"
    );
    // OpenRouter backend → provider prefs injected. require_parameters is
    // deliberately ABSENT (it 404s /v1/responses when tools are present —
    // live finding 2026-07-19).
    assert_eq!(json["provider"]["allow_fallbacks"], false);
    assert!(json["provider"].get("require_parameters").is_none());
    // Native backend → no prefs.
    let native = build_request(
        &request(vec![msg(Role::User, vec![text("hi")])]),
        &native_cfg(),
    );
    assert!(
        serde_json::to_value(&native)
            .unwrap()
            .get("provider")
            .is_none()
    );
}

#[test]
fn system_hoists_to_instructions_and_developer_is_native() {
    let built = build_request(
        &request(vec![
            msg(Role::System, vec![text("base prompt")]),
            msg(Role::Developer, vec![text("injected context")]),
            msg(Role::User, vec![text("hi")]),
        ]),
        &cfg(),
    );
    let json = serde_json::to_value(&built).unwrap();
    assert_eq!(json["instructions"], "base prompt");
    let input = json["input"].as_array().unwrap();
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["role"], "developer", "the exact semantic match");
    assert_eq!(input[1]["role"], "user");
    assert_eq!(input[1]["content"][0]["type"], "input_text");
}

#[test]
fn assistant_turn_flattens_to_sibling_items_with_verbatim_call_ids() {
    let reasoning_item = json!({"type": "reasoning", "id": "rs_1",
        "summary": [{"type": "summary_text", "text": "plan"}],
        "encrypted_content": "gAAA", "format": "openai-responses-v1",
        "status": "completed"});
    let built = build_request(
        &request(vec![
            msg(Role::User, vec![text("go")]),
            msg(
                Role::Assistant,
                vec![
                    ContentBlock::Reasoning {
                        format: ReasoningFormat::OpenAiResponses,
                        text: "plan".into(),
                        signature: None,
                        payload: Some(reasoning_item.clone()),
                    },
                    ContentBlock::ToolUse {
                        id: "call_abc".into(),
                        name: "shell".into(),
                        input: json!({"command": "ls"}),
                    },
                ],
            ),
            msg(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_abc".into(),
                    content: vec![ResultChunk::Text { text: "ok".into() }],
                    is_error: false,
                }],
            ),
        ]),
        &cfg(),
    );
    let json = serde_json::to_value(&built).unwrap();
    let input = json["input"].as_array().unwrap();
    // user, reasoning, function_call, function_call_output — flat siblings.
    assert_eq!(input.len(), 4);
    assert_eq!(input[1]["type"], "reasoning");
    assert_eq!(input[1]["id"], "rs_1", "whole-item replay");
    assert_eq!(
        input[1]["format"], "openai-responses-v1",
        "unknown fields kept"
    );
    assert!(
        input[1].get("status").is_none(),
        "status stripped (grok's rule)"
    );
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call_abc", "verbatim");
    assert!(input[2].get("id").is_none(), "no item id on replay");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_abc");
    assert_eq!(input[3]["output"], "ok");
}

#[test]
fn trailing_reasoning_is_dropped_on_replay() {
    let built = build_request(
        &request(vec![
            msg(Role::User, vec![text("go")]),
            msg(
                Role::Assistant,
                vec![
                    text("partial answer"),
                    ContentBlock::Reasoning {
                        format: ReasoningFormat::OpenAiResponses,
                        text: String::new(),
                        signature: None,
                        payload: Some(json!({"type": "reasoning", "id": "rs_t",
                            "summary": [], "encrypted_content": "x"})),
                    },
                ],
            ),
            msg(Role::User, vec![text("continue")]),
        ]),
        &cfg(),
    );
    let json = serde_json::to_value(&built).unwrap();
    let types: Vec<&str> = json["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert!(
        !types.contains(&"reasoning"),
        "a reasoning item with no following item would 400: {types:?}"
    );
}

#[test]
fn foreign_reasoning_formats_are_dropped() {
    let built = build_request(
        &request(vec![
            msg(Role::User, vec![text("go")]),
            msg(
                Role::Assistant,
                vec![
                    ContentBlock::Reasoning {
                        format: ReasoningFormat::Anthropic,
                        text: "anthropic thinking".into(),
                        signature: Some("sig".into()),
                        payload: None,
                    },
                    text("answer"),
                ],
            ),
        ]),
        &cfg(),
    );
    let json = serde_json::to_value(&built).unwrap();
    let types: Vec<&str> = json["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert!(!types.contains(&"reasoning"), "{types:?}");
}

#[test]
fn freeform_tool_emits_custom_natively_and_degrades_on_flag() {
    let mut req = request(vec![msg(Role::User, vec![text("go")])]);
    req.tools = vec![function_tool("shell"), freeform_tool("apply_patch")];

    // Native custom tools (default).
    let built = build_request(&req, &cfg());
    let json = serde_json::to_value(&built).unwrap();
    let tools = json["tools"].as_array().unwrap();
    assert_eq!(tools[0]["type"], "function");
    assert!(tools[0].get("function").is_none(), "FLAT, not Chat-nested");
    assert_eq!(tools[0]["strict"], false);
    assert_eq!(tools[1]["type"], "custom");
    assert_eq!(tools[1]["format"]["type"], "grammar");
    assert_eq!(tools[1]["format"]["syntax"], "lark");

    // Degraded (xAI 422s custom — probe P3).
    let mut degraded_cfg = cfg();
    degraded_cfg.custom_tools_supported = false;
    let built = build_request(&req, &degraded_cfg);
    let json = serde_json::to_value(&built).unwrap();
    let tools = json["tools"].as_array().unwrap();
    assert_eq!(tools[1]["type"], "function", "degraded to a function tool");
    assert_eq!(
        tools[1]["parameters"]["properties"]["input"]["type"], "string",
        "the {{input: string}} framing"
    );
}

#[test]
fn freeform_call_replays_custom_or_degraded_consistently() {
    let mk = |custom_supported: bool| {
        let mut c = cfg();
        c.custom_tools_supported = custom_supported;
        let mut req = request(vec![
            msg(Role::User, vec![text("go")]),
            msg(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_p1".into(),
                    name: "apply_patch".into(),
                    input: Value::String("*** Begin Patch".into()),
                }],
            ),
            msg(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_p1".into(),
                    content: vec![ResultChunk::Text {
                        text: "done".into(),
                    }],
                    is_error: false,
                }],
            ),
        ]);
        req.tools = vec![freeform_tool("apply_patch")];
        serde_json::to_value(build_request(&req, &c)).unwrap()
    };

    let native = mk(true);
    let input = native["input"].as_array().unwrap();
    assert_eq!(input[1]["type"], "custom_tool_call");
    assert_eq!(input[1]["input"], "*** Begin Patch", "raw text verbatim");
    assert_eq!(input[2]["type"], "custom_tool_call_output", "paired family");

    let degraded = mk(false);
    let input = degraded["input"].as_array().unwrap();
    assert_eq!(input[1]["type"], "function_call");
    let args: Value = serde_json::from_str(input[1]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(args["input"], "*** Begin Patch", "wrapped consistently");
    assert_eq!(input[2]["type"], "function_call_output");
}

#[test]
fn reasoning_param_effort_ladder_and_summary_default() {
    let mut req = request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.reasoning_effort = Some(ReasoningEffort::XHigh);
    let json = serde_json::to_value(build_request(&req, &cfg())).unwrap();
    assert_eq!(
        json["reasoning"]["effort"], "xhigh",
        "pass-through, no clamp"
    );
    assert_eq!(json["reasoning"]["summary"], "auto", "A.5 Q2 default");

    // Other(_) passes verbatim — the API's own error surfaces, never a clamp.
    req.sampling_args.reasoning_effort = Some(ReasoningEffort::Other("ultra".into()));
    let json = serde_json::to_value(build_request(&req, &cfg())).unwrap();
    assert_eq!(json["reasoning"]["effort"], "ultra");

    // Absent effort → the field is omitted entirely EVEN with a summary
    // configured (summary-only reads as reasoning-disabled → 400).
    req.sampling_args.reasoning_effort = None;
    let json = serde_json::to_value(build_request(&req, &cfg())).unwrap();
    assert!(json.get("reasoning").is_none());
}

#[test]
fn max_output_tokens_clamps_with_spec_floor() {
    let mut req = request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.max_tokens = 4; // below the spec minimum of 16
    let json = serde_json::to_value(build_request(&req, &cfg())).unwrap();
    assert_eq!(json["max_output_tokens"], 16);
}

#[test]
fn prompt_cache_key_rides_when_configured() {
    let mut c = cfg();
    c.prompt_cache_key = Some("sess-42".into());
    let json = serde_json::to_value(build_request(
        &request(vec![msg(Role::User, vec![text("hi")])]),
        &c,
    ))
    .unwrap();
    assert_eq!(json["prompt_cache_key"], "sess-42");
}
