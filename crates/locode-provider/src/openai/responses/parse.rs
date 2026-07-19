//! [`wire::ResponsesResponse`] → [`Completion`] (plan §4.4/§4.7).
//!
//! Output items are peeked by `type` and kept maximally opaque: a reasoning
//! item is captured **whole** into `Reasoning.payload` (fields we have never
//! heard of round-trip untouched); unknown item types are ignored, never a
//! parse failure.

use std::collections::HashSet;

use locode_protocol::{ContentBlock, ReasoningFormat, Usage};
use serde_json::Value;

use super::wire;
use crate::completion::{Completion, StopReason};
use crate::provider::ProviderError;

/// Parse a Responses response into the normalized [`Completion`].
///
/// `freeform_names` drives the degraded-arguments unwrap: a `function_call`
/// whose tool is freeform arrived through the `{"input": string}` fallback —
/// unwrap it so the tool sees the identical `Value::String` it would have
/// received from a native `custom_tool_call` (delivery-invisible, plan §4.6).
///
/// # Errors
/// [`ProviderError`] for a `failed` response (mapped per its error code) —
/// never for unknown item types or unmodeled fields.
#[allow(clippy::implicit_hasher)] // an internal call surface, not a generic API
pub fn response_to_completion(
    resp: wire::ResponsesResponse,
    freeform_names: &HashSet<String>,
) -> Result<Completion, ProviderError> {
    // A `failed` response (HTTP 200) maps like the non-streaming analog of
    // codex's response.failed handling: rate-limit → RateLimited, server error
    // → retryable 500 (grok converts in-stream failures to retryable 500s
    // deliberately), anything else terminal.
    if resp.status.as_deref() == Some("failed") || resp.error.is_some() {
        let error = resp.error.unwrap_or(wire::ResponseError {
            code: None,
            message: "response status: failed (no error body)".to_string(),
        });
        return Err(match error.code.as_deref() {
            Some("rate_limit_exceeded") => ProviderError::RateLimited { retry_after: None },
            Some("server_error") => ProviderError::Api {
                status: 500,
                message: error.message,
            },
            _ => ProviderError::Api {
                status: 400,
                message: error.message,
            },
        });
    }

    let mut content: Vec<ContentBlock> = Vec::with_capacity(resp.output.len());
    let mut saw_refusal = false;

    for item in resp.output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                let text = summary_text(&item);
                content.push(ContentBlock::Reasoning {
                    format: ReasoningFormat::OpenAiResponses,
                    text,
                    signature: None,
                    // The WHOLE item, opaquely — the only robust replay unit.
                    payload: Some(item),
                });
            }
            Some("message") => {
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                content.push(ContentBlock::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                        Some("refusal") => {
                            saw_refusal = true;
                            if let Some(text) = part.get("refusal").and_then(Value::as_str) {
                                content.push(ContentBlock::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("function_call") => {
                let call_id = str_field(&item, "call_id");
                let name = str_field(&item, "name");
                let raw = str_field(&item, "arguments");
                let input = decode_arguments(&name, &raw, freeform_names);
                content.push(ContentBlock::ToolUse {
                    id: call_id,
                    name,
                    input,
                });
            }
            Some("custom_tool_call") => {
                content.push(ContentBlock::ToolUse {
                    id: str_field(&item, "call_id"),
                    name: str_field(&item, "name"),
                    input: Value::String(str_field(&item, "input")),
                });
            }
            // Hosted-tool items, unknown future types: ignored, never fatal.
            _ => {}
        }
    }

    let has_tool_calls = content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
    let stop = map_stop(
        resp.status.as_deref(),
        resp.incomplete_details.as_ref(),
        has_tool_calls,
        saw_refusal,
    );

    Ok(Completion {
        content,
        usage: map_usage(resp.usage.as_ref()),
        stop,
    })
}

/// Concatenate `summary[].text` — the human-readable side of a reasoning item
/// (may be empty when no summary was requested/produced).
fn summary_text(item: &Value) -> String {
    let mut out = String::new();
    for entry in item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(text) = entry.get("text").and_then(Value::as_str) {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(text);
        }
    }
    out
}

fn str_field(item: &Value, key: &str) -> String {
    item.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Function-call arguments: freeform tools unwrap the `{"input": string}`
/// fallback to the raw string; invalid JSON is kept as `Value::String` so
/// dispatch produces a soft "invalid arguments" the model can react to
/// (better than grok's silent `"{}"` substitution).
fn decode_arguments(name: &str, raw: &str, freeform_names: &HashSet<String>) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(parsed) => {
            if freeform_names.contains(name)
                && let Some(input) = parsed.get("input").and_then(Value::as_str)
            {
                return Value::String(input.to_string());
            }
            parsed
        }
        Err(_) => Value::String(raw.to_string()),
    }
}

fn map_stop(
    status: Option<&str>,
    incomplete: Option<&wire::IncompleteDetails>,
    has_tool_calls: bool,
    saw_refusal: bool,
) -> StopReason {
    if saw_refusal {
        return StopReason::Refusal;
    }
    match status {
        Some("completed") => {
            if has_tool_calls {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        }
        Some("incomplete") => match incomplete.and_then(|d| d.reason.as_deref()) {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some(reason) => StopReason::Unknown(reason.to_string()),
            None => StopReason::Unknown("incomplete".to_string()),
        },
        Some(other) => StopReason::Unknown(other.to_string()),
        None => StopReason::Unknown("(missing status)".to_string()),
    }
}

/// Usage mapping — Option-through everywhere (ADR-0009 amendment): `None` =
/// not reported, never a fake zero.
fn map_usage(usage: Option<&wire::ResponsesUsage>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };
    Usage {
        input_tokens: usage.input_tokens.unwrap_or_default(),
        output_tokens: usage.output_tokens.unwrap_or_default(),
        cache_read_tokens: usage
            .input_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens),
        cache_creation_tokens: usage
            .input_tokens_details
            .as_ref()
            .and_then(|d| d.cache_write_tokens),
        reasoning_tokens: usage
            .output_tokens_details
            .as_ref()
            .and_then(|d| d.reasoning_tokens),
    }
}
