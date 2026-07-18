//! Request-shape tests for the Anthropic wire (Task 12, plan §6): cache-marker
//! count, system hoist, Developer rendering, temp-omit, effort mapping, verbatim
//! ids, `is_error` serialization, schema normalization, OpenRouter prefs.

use locode_protocol::{ContentBlock, Message, ResultChunk, Role, ToolSpec};
use locode_provider::anthropic::{
    ApiBackend, DeveloperRendering, ModelConfig, ReasoningEncoding, build_request,
    count_cache_controls, normalize_input_schema,
};
use locode_provider::{CacheHint, ConversationRequest, ReasoningEffort, SamplingArgs};
use serde_json::json;

fn native_cfg() -> ModelConfig {
    ModelConfig::new("claude-sonnet-5", "https://api.anthropic.com", "test-key")
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

fn base_request(messages: Vec<Message>) -> ConversationRequest {
    ConversationRequest {
        messages,
        tools: vec![],
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Standard,
    }
}

// ---- cache_control placement (plan §4.3) ----

#[test]
fn standard_hint_places_exactly_two_markers() {
    let req = base_request(vec![
        msg(Role::System, vec![text("base identity")]),
        msg(Role::User, vec![text("hello")]),
        msg(Role::Assistant, vec![text("hi")]),
        msg(Role::User, vec![text("tail")]),
    ]);
    let built = build_request(&req, &native_cfg());
    assert_eq!(
        count_cache_controls(&built),
        2,
        "last system block + last message"
    );

    // The marker is on the LAST message's last cache-capable block.
    let json = serde_json::to_value(&built).unwrap();
    let last = json["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(
        last["content"][0]["cache_control"]["type"], "ephemeral",
        "tail user text carries the message marker"
    );
    // System took the Blocks form with the marker on its last block.
    assert_eq!(json["system"][0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn cache_off_places_zero_markers() {
    let mut req = base_request(vec![
        msg(Role::System, vec![text("s")]),
        msg(Role::User, vec![text("u")]),
    ]);
    req.cache_hint = CacheHint::Off;
    let built = build_request(&req, &native_cfg());
    assert_eq!(count_cache_controls(&built), 0);

    // With no marker and a single system block, grok's collapse rule applies:
    // the bare-string form.
    let json = serde_json::to_value(&built).unwrap();
    assert_eq!(json["system"], json!("s"));
}

#[test]
fn tool_result_tail_carries_the_message_marker() {
    // The common agentic shape: the last message is a user turn holding a
    // tool_result — it must carry the marker (ToolUse/Image cannot).
    let req = base_request(vec![
        msg(Role::System, vec![text("s")]),
        msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "grep".into(),
                input: json!({"pattern": "x"}),
            }],
        ),
        msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: vec![ResultChunk::Text {
                    text: "match".into(),
                }],
                is_error: false,
            }],
        ),
    ]);
    let built = build_request(&req, &native_cfg());
    assert_eq!(count_cache_controls(&built), 2);
    let json = serde_json::to_value(&built).unwrap();
    let last = json["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["content"][0]["type"], "tool_result");
    assert_eq!(last["content"][0]["cache_control"]["type"], "ephemeral");
}

// ---- system hoist + Developer rendering (plan §4.1) ----

#[test]
fn system_hoists_out_of_the_message_stream() {
    let req = base_request(vec![
        msg(Role::System, vec![text("you are locode")]),
        msg(Role::User, vec![text("hi")]),
        msg(Role::Assistant, vec![text("hello")]),
        msg(Role::User, vec![text("bye")]),
    ]);
    let built = build_request(&req, &native_cfg());
    let json = serde_json::to_value(&built).unwrap();
    assert!(json["system"].is_array() || json["system"].is_string());
    for m in json["messages"].as_array().unwrap() {
        assert_ne!(m["role"], "system", "no system role in the message array");
    }
    assert_eq!(json["messages"].as_array().unwrap().len(), 3);
}

#[test]
fn developer_defaults_to_system_reminder_user_block() {
    let req = base_request(vec![
        msg(Role::System, vec![text("s")]),
        msg(Role::Developer, vec![text("the cwd is /repo")]),
        msg(Role::User, vec![text("hi")]),
    ]);
    let built = build_request(&req, &native_cfg());
    let json = serde_json::to_value(&built).unwrap();
    let first = &json["messages"][0];
    assert_eq!(first["role"], "user");
    let content = first["content"].as_str().expect("bare-string content");
    assert!(content.starts_with("<system-reminder>"));
    assert!(content.contains("the cwd is /repo"));
    assert!(content.ends_with("</system-reminder>"));
}

#[test]
fn developer_beta_path_emits_mid_conversation_system() {
    let mut cfg = native_cfg();
    cfg.developer_rendering = DeveloperRendering::MidConversationSystemBeta;
    let req = base_request(vec![
        msg(Role::Developer, vec![text("injected context")]),
        msg(Role::User, vec![text("hi")]),
    ]);
    let built = build_request(&req, &cfg);
    let json = serde_json::to_value(&built).unwrap();
    assert_eq!(json["messages"][0]["role"], "system");
    assert_eq!(json["messages"][0]["content"], "injected context");
}

// ---- temperature-omit + reasoning mapping (plan §4.3/§4.4/§9.3) ----

#[test]
fn temperature_omitted_when_thinking_on() {
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.temperature = Some(0.7);
    req.sampling_args.reasoning_effort = Some(ReasoningEffort::Medium);
    let built = build_request(&req, &native_cfg());
    assert!(built.thinking.is_some());
    assert!(
        built.temperature.is_none(),
        "temperature must be dropped when thinking is on"
    );

    // And absent thinking, temperature passes through.
    req.sampling_args.reasoning_effort = None;
    let built = build_request(&req, &native_cfg());
    assert!(built.thinking.is_none());
    assert_eq!(built.temperature, Some(0.7));
}

#[test]
fn budget_mapping_with_interleaved_beta_unclamped() {
    // Default config keeps the interleaved-thinking beta → no clamp (plan §9.3):
    // High = 16384 even though max_tokens is 4096.
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.reasoning_effort = Some(ReasoningEffort::High);
    let built = build_request(&req, &native_cfg());
    let json = serde_json::to_value(&built).unwrap();
    assert_eq!(json["thinking"]["type"], "enabled");
    assert_eq!(json["thinking"]["budget_tokens"], 16384);
}

#[test]
fn budget_mapping_clamps_without_the_beta() {
    let mut cfg = native_cfg();
    cfg.betas.clear(); // beta off → the clamp is mandatory
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.max_tokens = 4096;
    req.sampling_args.reasoning_effort = Some(ReasoningEffort::High);
    let built = build_request(&req, &cfg);
    let json = serde_json::to_value(&built).unwrap();
    assert_eq!(
        json["thinking"]["budget_tokens"], 4095,
        "min(16384, 4096-1)"
    );
}

#[test]
fn minimal_effort_means_no_thinking() {
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.reasoning_effort = Some(ReasoningEffort::Minimal);
    let built = build_request(&req, &native_cfg());
    assert!(built.thinking.is_none());
    assert!(built.output_config.is_none());
}

#[test]
fn effort_adaptive_encoding_emits_output_config() {
    let mut cfg = native_cfg();
    cfg.reasoning_encoding = ReasoningEncoding::EffortAdaptive;
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.reasoning_effort = Some(ReasoningEffort::Medium);
    let built = build_request(&req, &cfg);
    let json = serde_json::to_value(&built).unwrap();
    assert_eq!(json["thinking"]["type"], "adaptive");
    assert_eq!(json["thinking"]["display"], "summarized");
    assert_eq!(json["output_config"]["effort"], "medium");
    assert!(json.get("temperature").is_none());
}

// ---- verbatim ids + thinking replay + is_error (plan §4.2/§4.5) ----

#[test]
fn tool_use_ids_round_trip_verbatim() {
    let req = base_request(vec![
        msg(
            Role::Assistant,
            vec![
                ContentBlock::Thinking {
                    text: "let me check".into(),
                    signature: Some("sig-xyz".into()),
                },
                ContentBlock::ToolUse {
                    id: "toolu_01AbCdEf".into(),
                    name: "read_file".into(),
                    input: json!({"target_file": "a.rs"}),
                },
            ],
        ),
        msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_01AbCdEf".into(),
                content: vec![ResultChunk::Text { text: "ok".into() }],
                is_error: false,
            }],
        ),
    ]);
    let built = build_request(&req, &native_cfg());
    let json = serde_json::to_value(&built).unwrap();
    let assistant = &json["messages"][0]["content"];
    // Thinking replays in place, before the tool_use, with the SAME signature.
    assert_eq!(assistant[0]["type"], "thinking");
    assert_eq!(assistant[0]["thinking"], "let me check");
    assert_eq!(assistant[0]["signature"], "sig-xyz");
    assert_eq!(assistant[1]["type"], "tool_use");
    assert_eq!(assistant[1]["id"], "toolu_01AbCdEf");
    assert_eq!(
        json["messages"][1]["content"][0]["tool_use_id"],
        "toolu_01AbCdEf"
    );
}

#[test]
fn unsigned_thinking_is_dropped_on_send() {
    let req = base_request(vec![msg(
        Role::Assistant,
        vec![
            ContentBlock::Thinking {
                text: "unsigned".into(),
                signature: None,
            },
            text("visible"),
        ],
    )]);
    let built = build_request(&req, &native_cfg());
    let json = serde_json::to_value(&built).unwrap();
    let blocks = json["messages"][0]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "unsigned thinking dropped, not sent");
    assert_eq!(blocks[0]["type"], "text");
}

#[test]
fn is_error_serializes_only_when_true() {
    let req = base_request(vec![msg(
        Role::User,
        vec![
            ContentBlock::ToolResult {
                tool_use_id: "toolu_ok".into(),
                content: vec![ResultChunk::Text {
                    text: "fine".into(),
                }],
                is_error: false,
            },
            ContentBlock::ToolResult {
                tool_use_id: "toolu_bad".into(),
                content: vec![ResultChunk::Text {
                    text: "boom".into(),
                }],
                is_error: true,
            },
        ],
    )]);
    let mut req = req;
    req.cache_hint = CacheHint::Off;
    let built = build_request(&req, &native_cfg());
    let json = serde_json::to_value(&built).unwrap();
    let blocks = &json["messages"][0]["content"];
    assert!(
        blocks[0].get("is_error").is_none(),
        "false is omitted from the wire"
    );
    assert_eq!(blocks[1]["is_error"], true);
}

// ---- tools + schema normalization (plan §9 spike) ----

#[test]
fn tool_schema_is_normalized() {
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.tools = vec![ToolSpec {
        name: "read_file".into(),
        description: "Read a file.".into(),
        parameters: json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "title": "ReadFileArgs",
            "properties": {"target_file": {"type": "string"}},
            "required": ["target_file"]
        }),
    }];
    let built = build_request(&req, &native_cfg());
    let json = serde_json::to_value(&built).unwrap();
    let schema = &json["tools"][0]["input_schema"];
    assert!(
        schema.get("$schema").is_none(),
        "$schema meta-annotation stripped"
    );
    assert_eq!(schema["type"], "object");
    assert_eq!(json["tools"][0]["name"], "read_file");
}

#[test]
fn normalize_is_a_noop_on_plain_schemas() {
    let schema = json!({"type": "object", "properties": {}});
    assert_eq!(normalize_input_schema(schema.clone()), schema);
}

// ---- OpenRouter provider prefs (plan §9.2) ----

#[test]
fn openrouter_injects_provider_prefs_native_does_not() {
    let req = base_request(vec![msg(Role::User, vec![text("hi")])]);

    let native = build_request(&req, &native_cfg());
    assert!(native.provider.is_none());

    let or_cfg = ModelConfig::new(
        "anthropic/claude-sonnet-5",
        "https://openrouter.ai/api",
        "sk-or-x",
    );
    assert_eq!(or_cfg.api_backend, ApiBackend::OpenRouter);
    let routed = build_request(&req, &or_cfg);
    let prefs = routed.provider.expect("prefs injected for OpenRouter");
    assert_eq!(prefs["require_parameters"], true);
}

// ---- stream flag ----

#[test]
fn v0_is_always_non_streaming() {
    let req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    let built = build_request(&req, &native_cfg());
    assert_eq!(built.stream, Some(false));
}
