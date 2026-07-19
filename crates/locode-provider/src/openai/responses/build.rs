//! `ConversationRequest` → [`wire::ResponsesRequest`] (plan §4.2–§4.6).
//!
//! The judgement lives here: the System→`instructions` hoist, flattening our
//! nested messages into sibling items, whole-item reasoning replay (with the
//! trailing-reasoning guard), verbatim `call_id`s, and the freeform/custom
//! tool emission + degradation.

use std::collections::HashSet;

use locode_protocol::{ContentBlock, ImageSource, ReasoningFormat, ResultChunk, Role, ToolSpec};
use serde_json::{Value, json};

use super::wire;
use crate::http::normalize_input_schema;
use crate::openai::{OpenAiModelConfig, SystemPlacement};
use crate::request::{ConversationRequest, ReasoningEffort};

/// The set of tool names whose spec is freeform (drives custom-vs-function
/// emission on build AND the degraded-arguments unwrap on parse).
#[must_use]
pub fn freeform_tool_names(tools: &[ToolSpec]) -> HashSet<String> {
    tools
        .iter()
        .filter(|spec| {
            matches!(
                spec.input,
                locode_protocol::ToolInputFormat::Freeform { .. }
            )
        })
        .map(|spec| spec.name.clone())
        .collect()
}

/// The `{"input": string}` fallback schema a freeform tool degrades to on
/// wires/backends without custom-tool support (ADR-0012's reserved framing;
/// the raw text reaches the tool identically either way).
#[must_use]
pub fn freeform_fallback_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {"input": {"type": "string",
            "description": "The entire raw text input."}},
        "required": ["input"],
        "additionalProperties": false
    })
}

/// Build the Responses request.
#[must_use]
#[allow(clippy::too_many_lines)] // one linear pass over the stream; splitting
// it would thread four mutable accumulators through helpers for no clarity win
pub fn build_request(req: &ConversationRequest, cfg: &OpenAiModelConfig) -> wire::ResponsesRequest {
    let freeform = freeform_tool_names(&req.tools);
    let use_custom = cfg.custom_tools_supported;

    let mut instructions_parts: Vec<String> = Vec::new();
    let mut input: Vec<Value> = Vec::new();
    // call_ids emitted as custom_tool_call — their outputs must pair as
    // custom_tool_call_output (single pass works: results always follow calls).
    let mut custom_call_ids: HashSet<String> = HashSet::new();

    for message in &req.messages {
        match message.role {
            Role::System => match cfg.system_placement {
                SystemPlacement::Instructions => {
                    for block in &message.content {
                        if let ContentBlock::Text { text } = block {
                            instructions_parts.push(text.clone());
                        }
                    }
                }
                SystemPlacement::InputMessage => {
                    push_text_message(&mut input, "system", &message.content);
                }
            },
            // The exact semantic match — our protocol borrowed OpenAI's word
            // (ADR-0013); grok never emits it, we do because we have the role.
            Role::Developer => push_text_message(&mut input, "developer", &message.content),
            Role::User => {
                let mut parts: Vec<Value> = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            parts.push(json!({"type": "input_text", "text": text}));
                        }
                        ContentBlock::Image { source } => {
                            parts.push(json!({"type": "input_image",
                                "image_url": image_url(source)}));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            // Flush any pending text/image run first (order is
                            // load-bearing for the prefix cache).
                            flush_user_parts(&mut input, &mut parts);
                            let output = join_chunks(content);
                            // is_error has no wire slot — the output text
                            // carries the error message (OpenAI convention).
                            let item_type = if custom_call_ids.contains(tool_use_id) {
                                "custom_tool_call_output"
                            } else {
                                "function_call_output"
                            };
                            input.push(json!({"type": item_type,
                                "call_id": tool_use_id, "output": output}));
                        }
                        _ => {}
                    }
                }
                flush_user_parts(&mut input, &mut parts);
            }
            Role::Assistant => {
                let start = input.len();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            input.push(json!({"type": "message", "role": "assistant",
                                "content": [{"type": "output_text", "text": text}]}));
                        }
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input: args,
                        } => {
                            let is_freeform = freeform.contains(name);
                            if is_freeform && use_custom {
                                // Native custom tool: raw text input verbatim.
                                let raw = args
                                    .as_str()
                                    .map_or_else(|| args.to_string(), ToString::to_string);
                                custom_call_ids.insert(id.clone());
                                input.push(json!({"type": "custom_tool_call",
                                    "call_id": id, "name": name, "input": raw}));
                            } else {
                                // Function call; a DEGRADED freeform call wraps
                                // the raw text as {"input": s} so the model saw
                                // a consistent schema (plan §4.6).
                                let arguments = if is_freeform {
                                    let raw = args
                                        .as_str()
                                        .map_or_else(|| args.to_string(), ToString::to_string);
                                    json!({"input": raw}).to_string()
                                } else {
                                    args.to_string()
                                };
                                // No item `id` on replay — only call_id is
                                // required; codex clears item ids (plan §4.2).
                                input.push(json!({"type": "function_call",
                                    "call_id": id, "name": name,
                                    "arguments": arguments}));
                            }
                        }
                        // Whole-item reasoning replay: this wire's own format
                        // only; strip the output-only `status` (grok's rule).
                        ContentBlock::Reasoning {
                            format: ReasoningFormat::OpenAiResponses,
                            payload: Some(payload),
                            ..
                        } => {
                            let mut item = payload.clone();
                            if let Some(obj) = item.as_object_mut() {
                                obj.remove("status");
                            }
                            input.push(item);
                        }
                        // Foreign reasoning formats: dropped (never crosses
                        // wires within a session).
                        _ => {}
                    }
                }
                drop_trailing_reasoning(&mut input, start);
            }
        }
    }

    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|spec| tool_def(spec, use_custom))
        .collect();

    wire::ResponsesRequest {
        model: cfg.model.clone(),
        instructions: (!instructions_parts.is_empty()).then(|| instructions_parts.join("\n\n")),
        input,
        tools: (!tools.is_empty()).then_some(tools),
        reasoning: reasoning_param(req, cfg),
        store: false,
        stream: false,
        include: vec!["reasoning.encrypted_content".to_string()],
        max_output_tokens: Some(req.sampling_args.max_tokens.min(cfg.max_tokens_cap).max(16)),
        temperature: req.sampling_args.temperature,
        top_p: req.sampling_args.top_p,
        prompt_cache_key: cfg.prompt_cache_key.clone(),
        provider: cfg.effective_provider_prefs(),
    }
}

/// A reasoning item must precede its "required following item" — an assistant
/// turn whose trailing block(s) are reasoning-only would 400 on replay
/// (concern #1 of the pre-implementation review). The engine's empty-completion
/// resample makes this near-unreachable; the guard is defense in depth.
fn drop_trailing_reasoning(input: &mut Vec<Value>, start: usize) {
    while input.len() > start
        && input
            .last()
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            == Some("reasoning")
    {
        input.pop();
    }
}

fn push_text_message(input: &mut Vec<Value>, role: &str, content: &[ContentBlock]) {
    let mut text = String::new();
    for block in content {
        if let ContentBlock::Text { text: t } = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(t);
        }
    }
    if !text.is_empty() {
        input.push(json!({"type": "message", "role": role,
            "content": [{"type": "input_text", "text": text}]}));
    }
}

fn flush_user_parts(input: &mut Vec<Value>, parts: &mut Vec<Value>) {
    if !parts.is_empty() {
        input.push(json!({"type": "message", "role": "user",
            "content": std::mem::take(parts)}));
    }
}

fn image_url(source: &ImageSource) -> String {
    match source {
        ImageSource::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        ImageSource::Url { url } => url.clone(),
    }
}

fn join_chunks(chunks: &[ResultChunk]) -> String {
    let mut out = String::new();
    for chunk in chunks {
        if let ResultChunk::Text { text } = chunk {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    out
}

/// One tool definition: flat `function` shape (NOT Chat Completions' nested
/// `function:{}`), or `custom` + grammar when supported (probe P2 validated
/// the exact shape through OpenRouter).
fn tool_def(spec: &ToolSpec, use_custom: bool) -> Value {
    match &spec.input {
        locode_protocol::ToolInputFormat::JsonSchema { parameters } => {
            json!({"type": "function", "name": spec.name,
                "description": spec.description,
                "parameters": normalize_input_schema(parameters.clone()),
                "strict": false})
        }
        locode_protocol::ToolInputFormat::Freeform { syntax, definition } => {
            if use_custom {
                let syntax = match syntax {
                    locode_protocol::GrammarSyntax::Lark => "lark",
                    locode_protocol::GrammarSyntax::Regex => "regex",
                };
                json!({"type": "custom", "name": spec.name,
                    "description": spec.description,
                    "format": {"type": "grammar", "syntax": syntax,
                        "definition": definition}})
            } else {
                // xAI 422s `custom` (probe P3) — the {"input": string} framing.
                json!({"type": "function", "name": spec.name,
                    "description": spec.description,
                    "parameters": freeform_fallback_parameters(),
                    "strict": false})
            }
        }
    }
}

/// The `reasoning` request object: effort (extended ladder, pass-through — an
/// unsupported tier surfaces the API's own error, never clamps) + summary
/// (default `"auto"`, plan §A.5 Q2). **Summary rides only alongside an
/// effort** — a summary-only object reads as "reasoning disabled" and 400s on
/// reasoning-mandatory endpoints (live finding, 2026-07-19).
fn reasoning_param(req: &ConversationRequest, cfg: &OpenAiModelConfig) -> Option<Value> {
    let effort = req
        .sampling_args
        .reasoning_effort
        .as_ref()
        .map(|e| match e {
            ReasoningEffort::None => "none".to_string(),
            ReasoningEffort::Minimal => "minimal".to_string(),
            ReasoningEffort::Low => "low".to_string(),
            ReasoningEffort::Medium => "medium".to_string(),
            ReasoningEffort::High => "high".to_string(),
            ReasoningEffort::XHigh => "xhigh".to_string(),
            ReasoningEffort::Other(s) => s.clone(),
        });
    let effort = effort?;
    let mut obj = serde_json::Map::new();
    obj.insert("effort".to_string(), Value::String(effort));
    if let Some(s) = cfg.reasoning_summary.as_ref() {
        obj.insert("summary".to_string(), Value::String(s.clone()));
    }
    Some(Value::Object(obj))
}
