//! `ConversationRequest` → [`wire::MessagesRequest`] (plan §4.1–§4.4).
//!
//! This is where the Anthropic-specific judgement lives: the System hoist, the
//! Developer rendering, `cache_control` placement (≤4 hard cap, asserted), the
//! temperature-omit rule, and the `reasoning_effort → thinking` mapping.

use locode_protocol::{ContentBlock, ImageSource, ResultChunk, Role};

use super::config::{DeveloperRendering, ModelConfig};
use super::wire;
pub use crate::http::normalize_input_schema;
use crate::request::{CacheHint, ConversationRequest, ReasoningEffort};

/// The `max_tokens` this request will carry: the caller's budget, clamped only
/// when the model config pins an explicit ceiling.
///
/// `max_tokens` is required on every Messages request, so there is always a
/// value to compute — unlike the OpenAI protocols, where the field is optional
/// (opencode encodes the same asymmetry: `Schema.Number` for Anthropic against
/// `Schema.optional` for both OpenAI wires).
fn effective_max_tokens(req: &ConversationRequest, cfg: &ModelConfig) -> u32 {
    cfg.max_tokens_cap
        .map_or(req.sampling_args.max_tokens, |cap| {
            req.sampling_args.max_tokens.min(cap)
        })
}

/// Build the Messages request from the neutral request + per-model config.
///
/// # Panics
/// Asserts the built request carries **at most 4** `cache_control` markers —
/// Anthropic 400s a 5th (plan §4.3). With the v0 `Standard` policy the count is
/// ≤2 by construction, so a trip means a placement bug, not bad input.
#[must_use]
pub fn build_request(req: &ConversationRequest, cfg: &ModelConfig) -> wire::MessagesRequest {
    let (mut system_blocks, mut messages) = map_messages(req, cfg);

    // ---- cache_control placement (plan §4.3): last system block + last
    // cache-capable block of the last message; ≤4 asserted below. ----
    if req.cache_hint == CacheHint::Standard {
        if let Some(last) = system_blocks.last_mut() {
            last.cache_control = Some(wire::CacheControl::ephemeral());
        }
        if let Some(last_message) = messages.last_mut() {
            mark_last_cache_capable(last_message);
        }
    }

    let system = match system_blocks.len() {
        0 => None,
        // grok's rule: a single unmarked block collapses to the bare-string form.
        1 if system_blocks[0].cache_control.is_none() => {
            Some(wire::SystemParam::Text(system_blocks.remove(0).text))
        }
        _ => Some(wire::SystemParam::Blocks(system_blocks)),
    };

    // ---- thinking + effort (always adaptive; see `map_reasoning`) ----
    let (thinking, output_config) = map_reasoning(req);
    // Temperature is never sent: the API requires temp=1 whenever thinking is
    // on, and thinking is now unconditional here (it is also removed outright
    // on Opus 5 / Fable 5 / Opus 4.8 / 4.7 / Sonnet 5, which 400 on it). The
    // neutral `SamplingArgs.temperature` stays meaningful for other wires.
    let temperature = None;

    let tools: Vec<wire::ToolParam> = req
        .tools
        .iter()
        .map(|spec| wire::ToolParam {
            name: spec.name.clone(),
            description: Some(spec.description.clone()),
            input_schema: match &spec.input {
                locode_protocol::ToolInputFormat::JsonSchema { parameters } => {
                    normalize_input_schema(parameters.clone())
                }
                // Freeform degradation: this wire has no custom tools — render
                // the historical {"input": string} function framing (ADR-0012;
                // the raw text reaches the tool identically, task-18 plan §4.6).
                locode_protocol::ToolInputFormat::Freeform { .. } => serde_json::json!({
                    "type": "object",
                    "properties": {"input": {"type": "string",
                        "description": "The entire raw text input."}},
                    "required": ["input"],
                    "additionalProperties": false
                }),
            },
        })
        .collect();

    let built = wire::MessagesRequest {
        model: cfg.model.clone(),
        messages,
        max_tokens: effective_max_tokens(req, cfg),
        system,
        tools: (!tools.is_empty()).then_some(tools),
        tool_choice: None,
        temperature,
        top_p: req.sampling_args.top_p,
        top_k: None,
        stream: Some(false),
        stop_sequences: None,
        thinking: Some(thinking),
        output_config,
        metadata: None,
        provider: cfg.effective_provider_prefs(),
    };

    let markers = count_cache_controls(&built);
    assert!(
        markers <= 4,
        "cache_control marker count {markers} exceeds Anthropic's hard cap of 4"
    );
    built
}

/// Map the role-tagged protocol stream into (`system` blocks, wire messages):
/// the System hoist and per-role block mapping of plan §4.1.
fn map_messages(
    req: &ConversationRequest,
    cfg: &ModelConfig,
) -> (Vec<wire::TextBlock>, Vec<wire::Message>) {
    let mut system_blocks: Vec<wire::TextBlock> = Vec::new();
    let mut messages: Vec<wire::Message> = Vec::new();

    for message in &req.messages {
        match message.role {
            // System → top-level `system`, regardless of position (grok routes
            // every System item into system_blocks; ADR-0013 "front-loaded").
            Role::System => {
                for block in &message.content {
                    if let ContentBlock::Text { text } = block {
                        system_blocks.push(wire::TextBlock {
                            r#type: "text".to_string(),
                            text: text.clone(),
                            cache_control: None,
                        });
                    }
                }
            }
            Role::Developer => {
                let text = concat_text(&message.content);
                if !text.is_empty() {
                    messages.push(match cfg.developer_rendering {
                        // Portable default: a user turn wrapped in <system-reminder>.
                        DeveloperRendering::SystemReminder => wire::Message {
                            role: wire::MessageRole::User,
                            content: wire::MessageContent::Text(format!(
                                "<system-reminder>\n{text}\n</system-reminder>"
                            )),
                        },
                        // Beta path: a real mid-conversation system message.
                        DeveloperRendering::MidConversationSystemBeta => wire::Message {
                            role: wire::MessageRole::System,
                            content: wire::MessageContent::Text(text),
                        },
                    });
                }
            }
            Role::User => {
                let blocks = map_user_blocks(&message.content);
                if !blocks.is_empty() {
                    messages.push(wire::Message {
                        role: wire::MessageRole::User,
                        content: wire::MessageContent::Blocks(blocks),
                    });
                }
            }
            Role::Assistant => {
                let blocks = map_assistant_blocks(&message.content);
                if !blocks.is_empty() {
                    messages.push(wire::Message {
                        role: wire::MessageRole::Assistant,
                        content: wire::MessageContent::Blocks(blocks),
                    });
                }
            }
        }
    }
    (system_blocks, messages)
}

/// Concatenate the text blocks of a message (Developer rendering).
fn concat_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if let ContentBlock::Text { text } = block {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    out
}

/// Map a protocol `User` message's blocks (text / image / `tool_result`).
fn map_user_blocks(blocks: &[ContentBlock]) -> Vec<wire::ContentBlock> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            ContentBlock::Text { text } => out.push(wire::ContentBlock::Text {
                text: text.clone(),
                cache_control: None,
            }),
            ContentBlock::Image { source } => out.push(wire::ContentBlock::Image {
                source: map_image_source(source),
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => out.push(wire::ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: map_tool_result_content(content),
                is_error: *is_error,
                cache_control: None,
            }),
            // Thinking/ToolUse in a user turn (or future block kinds) have no
            // wire position — drop rather than corrupt the request.
            _ => {}
        }
    }
    out
}

/// Map a protocol `Assistant` message's blocks (text / thinking / `tool_use`).
fn map_assistant_blocks(blocks: &[ContentBlock]) -> Vec<wire::ContentBlock> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            ContentBlock::Text { text } => out.push(wire::ContentBlock::Text {
                text: text.clone(),
                cache_control: None,
            }),
            // Reasoning replay (ADR-0013 unified block): this wire replays its
            // OWN formats — signed thinking (same signature, in place, so it
            // stays contiguous with the tool_use that follows) and redacted
            // thinking (payload verbatim). Foreign formats (openai_responses,
            // text_only) are dropped: a session never crosses wires.
            ContentBlock::Reasoning {
                format: locode_protocol::ReasoningFormat::Anthropic,
                text,
                signature: Some(signature),
                ..
            } => {
                if !text.is_empty() || !signature.is_empty() {
                    out.push(wire::ContentBlock::Thinking {
                        thinking: text.clone(),
                        signature: signature.clone(),
                    });
                }
            }
            ContentBlock::Reasoning {
                format: locode_protocol::ReasoningFormat::AnthropicRedacted,
                payload: Some(payload),
                ..
            } => {
                if let Some(data) = payload.as_str() {
                    out.push(wire::ContentBlock::RedactedThinking {
                        data: data.to_owned(),
                    });
                }
            }
            ContentBlock::ToolUse { id, name, input } => {
                // id preserved VERBATIM — pairing is load-bearing (ADR-0004).
                // Anthropic requires `tool_use.input` to be a JSON OBJECT. A
                // recovered malformed-args call preserves the model's raw as a
                // JSON string (streaming assembler, client.rs); coerce any
                // non-object here so replaying that turn stays a valid request.
                let input = if input.is_object() {
                    input.clone()
                } else {
                    serde_json::json!({})
                };
                out.push(wire::ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input,
                });
            }
            // ToolResult/Image in an assistant turn (or future kinds): no wire
            // position — drop.
            _ => {}
        }
    }
    out
}

/// Map a protocol tool-result payload onto the wire shape: a single text chunk
/// collapses to the bare-string form; anything else becomes blocks.
fn map_tool_result_content(chunks: &[ResultChunk]) -> wire::ToolResultContent {
    if let [ResultChunk::Text { text }] = chunks {
        return wire::ToolResultContent::Text(text.clone());
    }
    let blocks = chunks
        .iter()
        .map(|chunk| match chunk {
            ResultChunk::Text { text } => wire::ContentBlock::Text {
                text: text.clone(),
                cache_control: None,
            },
            ResultChunk::Image { source } => wire::ContentBlock::Image {
                source: map_image_source(source),
            },
        })
        .collect();
    wire::ToolResultContent::Blocks(blocks)
}

fn map_image_source(source: &ImageSource) -> wire::ImageSource {
    match source {
        ImageSource::Base64 { media_type, data } => wire::ImageSource::Base64 {
            media_type: media_type.clone(),
            data: data.clone(),
        },
        ImageSource::Url { url } => wire::ImageSource::Url { url: url.clone() },
    }
}

/// Put the ephemeral marker on the last cache-capable block of `message`
/// (`Text`/`ToolResult` carry `cache_control`; `ToolUse`/`Image`/`Thinking`
/// cannot). If no block qualifies, skip — a documented invariant, not an error.
fn mark_last_cache_capable(message: &mut wire::Message) {
    match &mut message.content {
        // Bare-string content (Developer rendering) cannot carry a marker.
        wire::MessageContent::Text(_) => {}
        wire::MessageContent::Blocks(blocks) => {
            for block in blocks.iter_mut().rev() {
                match block {
                    wire::ContentBlock::Text { cache_control, .. }
                    | wire::ContentBlock::ToolResult { cache_control, .. } => {
                        *cache_control = Some(wire::CacheControl::ephemeral());
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Build the thinking + output config. **Adaptive thinking is unconditional on
/// this wire** (user decision, 2026-07-25); `reasoning_effort` only chooses the
/// depth.
///
/// Omitting `thinking` does not mean "no thinking" on any model this wire
/// targets — Fable 5 thinks unconditionally and rejects `{type:"disabled"}`,
/// and Opus 5 runs adaptive when the field is absent. Leaving it unset bought
/// no control, only ambiguity: the same request thought or didn't depending on
/// which model served it, and traces showed reasoning we never asked for.
/// Sending `{type:"adaptive"}` states what was already happening.
///
/// The old `budget_tokens` encoding is gone. `{type:"enabled", budget_tokens}`
/// is **removed on every current model** — Fable 5, Opus 5, Opus 4.8/4.7,
/// Sonnet 5 all 400 on it; only Opus 4.6 / Sonnet 4.6 still accept it, and
/// deprecated. It survived here only because nothing ever set
/// `reasoning_effort`, so the branch was never reached.
///
/// `display: "summarized"` is deliberate: the default is `"omitted"`, which
/// streams thinking blocks whose text is empty while still carrying a
/// multi-KB signature — a trace full of unreadable blobs. Display affects
/// visibility only; thinking happens and is billed the same either way.
fn map_reasoning(req: &ConversationRequest) -> (wire::ThinkingConfig, Option<wire::OutputConfig>) {
    let thinking = wire::ThinkingConfig::Adaptive {
        display: Some(wire::ThinkingDisplay::Summarized),
    };
    // No effort ⇒ omit `output_config` and take the API's own default (`high`).
    // Every tier is forwarded verbatim, including `Other`: an unsupported one
    // surfaces the API's error rather than being silently remapped (ADR-0007).
    let effort = match req.sampling_args.reasoning_effort.as_ref() {
        None => return (thinking, None),
        // `None`/`Minimal` used to mean thinking-off, which this wire can no
        // longer express (Fable 5 rejects `{type:"disabled"}` outright, and
        // Opus 5 only accepts it at effort `high` or below). The nearest
        // truthful rendering is the shallowest tier.
        Some(ReasoningEffort::None | ReasoningEffort::Minimal | ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
        Some(ReasoningEffort::XHigh) => "xhigh",
        Some(ReasoningEffort::Max) => "max",
        Some(ReasoningEffort::Other(s)) => s.as_str(),
    };
    (
        thinking,
        Some(wire::OutputConfig {
            effort: Some(effort.to_string()),
            format: None,
        }),
    )
}

/// Count every `cache_control` across `system` + `messages` (the ≤4 guard).
#[must_use]
pub fn count_cache_controls(req: &wire::MessagesRequest) -> usize {
    let mut count = 0;
    if let Some(wire::SystemParam::Blocks(blocks)) = &req.system {
        count += blocks.iter().filter(|b| b.cache_control.is_some()).count();
    }
    for message in &req.messages {
        if let wire::MessageContent::Blocks(blocks) = &message.content {
            for block in blocks {
                match block {
                    wire::ContentBlock::Text {
                        cache_control: Some(_),
                        ..
                    }
                    | wire::ContentBlock::ToolResult {
                        cache_control: Some(_),
                        ..
                    } => count += 1,
                    _ => {}
                }
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use locode_protocol::ContentBlock;
    use serde_json::json;

    /// A recovered malformed-args tool call preserves the model's raw as a JSON
    /// string (streaming assembler); Anthropic requires `tool_use.input` to be an
    /// object, so replaying that turn must coerce the non-object to `{}`. A valid
    /// object passes through unchanged.
    #[test]
    fn assistant_tool_use_non_object_input_coerces_to_empty_object() {
        let coerced = map_assistant_blocks(&[ContentBlock::ToolUse {
            id: "t1".into(),
            name: "grep".into(),
            input: json!("{\"pattern\":\"x\",-A:3}"),
        }]);
        match &coerced[0] {
            wire::ContentBlock::ToolUse { id, input, .. } => {
                assert_eq!(id, "t1");
                assert_eq!(input, &json!({}), "non-object input coerced to {{}}");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }

        let passthrough = map_assistant_blocks(&[ContentBlock::ToolUse {
            id: "t2".into(),
            name: "grep".into(),
            input: json!({"pattern": "x"}),
        }]);
        match &passthrough[0] {
            wire::ContentBlock::ToolUse { input, .. } => {
                assert_eq!(input, &json!({"pattern": "x"}), "valid object unchanged");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }
}
