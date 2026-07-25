//! Request-shape tests for the Anthropic wire (Task 12, plan §6): cache-marker
//! count, system hoist, Developer rendering, temp-omit, effort mapping, verbatim
//! ids, `is_error` serialization, schema normalization, OpenRouter prefs.

use locode_protocol::{
    ContentBlock, Message, ReasoningFormat, ResultChunk, Role, ToolInputFormat, ToolSpec,
};
use locode_provider::anthropic::{
    ApiBackend, DeveloperRendering, ModelConfig, build_request, count_cache_controls,
    normalize_input_schema,
};
use locode_provider::{
    CacheHint, ConversationRequest, DEFAULT_MAX_TOKENS, ReasoningEffort, SamplingArgs,
};
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

/// The default config pins no ceiling, so the caller's budget reaches the wire
/// verbatim — a silent `min` would corrupt eval comparisons the same way a
/// silently clamped `reasoning_effort` would (ADR-0007).
#[test]
fn max_tokens_passes_through_unclamped_by_default() {
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    assert_eq!(
        native_cfg().max_tokens_cap,
        None,
        "no ceiling unless one is pinned"
    );

    let built = build_request(&req, &native_cfg());
    assert_eq!(built.max_tokens, DEFAULT_MAX_TOKENS);

    // Including a value above every current model's limit: the request is the
    // caller's to make, and the API's own error is the honest answer.
    req.sampling_args.max_tokens = 200_000;
    assert_eq!(build_request(&req, &native_cfg()).max_tokens, 200_000);
}

/// The ceiling is opt-in, for pinning a model whose real limit is lower.
#[test]
fn max_tokens_cap_clamps_only_when_pinned() {
    let mut cfg = native_cfg();
    cfg.max_tokens_cap = Some(4096); // e.g. claude-3-haiku
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.max_tokens = 64_000;
    assert_eq!(build_request(&req, &cfg).max_tokens, 4096);

    // A budget already under the ceiling is untouched.
    req.sampling_args.max_tokens = 1024;
    assert_eq!(build_request(&req, &cfg).max_tokens, 1024);
}

/// Adaptive thinking is unconditional on this wire: a request that names no
/// effort still asks for it explicitly, rather than leaving the outcome to
/// whatever the serving model does with an absent field.
#[test]
fn thinking_is_always_adaptive_even_with_no_effort() {
    let req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    assert!(req.sampling_args.reasoning_effort.is_none());

    let json = serde_json::to_value(build_request(&req, &native_cfg())).unwrap();
    assert_eq!(json["thinking"]["type"], "adaptive");
    // Summarized, not the API's "omitted" default — otherwise every trace
    // carries a multi-KB signature wrapped around empty text.
    assert_eq!(json["thinking"]["display"], "summarized");
    // No effort named ⇒ no output_config; the API applies its own default.
    assert!(json.get("output_config").is_none());
}

/// `budget_tokens` is removed on every model this wire targets — the old
/// Budget encoding must not be reachable by any input.
#[test]
fn no_effort_tier_can_produce_budget_tokens() {
    for effort in [
        ReasoningEffort::None,
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
        ReasoningEffort::Other("max".into()),
    ] {
        let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
        req.sampling_args.reasoning_effort = Some(effort.clone());
        let json = serde_json::to_value(build_request(&req, &native_cfg())).unwrap();
        assert_eq!(json["thinking"]["type"], "adaptive", "{effort:?}");
        assert!(
            json["thinking"].get("budget_tokens").is_none(),
            "{effort:?} must not emit budget_tokens"
        );
    }
}

#[test]
fn effort_tiers_map_to_output_config() {
    for (effort, expected) in [
        (ReasoningEffort::None, "low"),
        (ReasoningEffort::Minimal, "low"),
        (ReasoningEffort::Low, "low"),
        (ReasoningEffort::Medium, "medium"),
        (ReasoningEffort::High, "high"),
        (ReasoningEffort::XHigh, "xhigh"),
        // Unknown tiers ride through verbatim so the API's own error surfaces,
        // rather than being silently remapped (ADR-0007).
        (ReasoningEffort::Other("max".into()), "max"),
    ] {
        let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
        req.sampling_args.reasoning_effort = Some(effort.clone());
        let json = serde_json::to_value(build_request(&req, &native_cfg())).unwrap();
        assert_eq!(json["output_config"]["effort"], expected, "{effort:?}");
    }
}

/// Thinking is always on, so temperature is never sendable — the API demands
/// temp=1 with thinking, and the current models reject the field outright.
#[test]
fn temperature_is_never_sent() {
    let mut req = base_request(vec![msg(Role::User, vec![text("hi")])]);
    req.sampling_args.temperature = Some(0.7);
    req.sampling_args.reasoning_effort = None;
    let json = serde_json::to_value(build_request(&req, &native_cfg())).unwrap();
    assert!(json.get("temperature").is_none());
}

// ---- verbatim ids + thinking replay + is_error (plan §4.2/§4.5) ----

#[test]
fn tool_use_ids_round_trip_verbatim() {
    let req = base_request(vec![
        msg(
            Role::Assistant,
            vec![
                ContentBlock::Reasoning {
                    format: ReasoningFormat::Anthropic,
                    text: "let me check".into(),
                    signature: Some("sig-xyz".into()),
                    payload: None,
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
            ContentBlock::Reasoning {
                format: ReasoningFormat::Anthropic,
                text: "unsigned".into(),
                signature: None,
                payload: None,
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
        input: ToolInputFormat::JsonSchema {
            parameters: json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "title": "ReadFileArgs",
                "properties": {"target_file": {"type": "string"}},
                "required": ["target_file"]
            }),
        },
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
