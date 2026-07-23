//! OpenAI Responses SSE streaming (ADR-0021 slice 2).
//!
//! The Responses stream carries the whole final response object on its terminal
//! `response.completed`/`incomplete`/`failed` event, so the assembled
//! [`Completion`] is **byte-identical** to the non-streaming parse: we capture
//! that object and reuse [`response_to_completion`]. The `response.*.delta`
//! events drive the display-only [`CompletionDelta`] channel; tool arguments
//! arrive whole inside the finalized item, so nothing is parsed mid-stream.

use std::collections::HashSet;

use serde_json::Value;
use tokio_stream::StreamExt;

use super::wire;
use super::{parse::response_to_completion, wire::ResponsesRequest};
use crate::completion::{Completion, CompletionDelta};
use crate::http::HttpFailure;
use crate::openai::{OpenAiModelConfig, classify};

/// POST the streaming Responses request and assemble the SSE frames back into the
/// same [`Completion`] the non-streaming path produces, emitting
/// [`CompletionDelta`]s to `on_delta` as they arrive. Single attempt — a
/// retryable failure surfaces to the engine's loop-level resample.
pub(crate) async fn send_once_streaming(
    http: &reqwest::Client,
    config: &OpenAiModelConfig,
    request: &ResponsesRequest,
    freeform_names: &HashSet<String>,
    on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
) -> Result<Completion, HttpFailure> {
    let url = format!("{}/v1/responses", config.base_url);
    let mut builder = http.post(&url).bearer_auth(&config.bearer).json(request);
    for (name, value) in &config.extra_headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| HttpFailure::transport(e.to_string()))?;

    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::http::parse_retry_after);
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let body: crate::openai::OpenAiErrorBody = match serde_json::from_str(&text) {
            Ok(body) => body,
            Err(_) => crate::openai::OpenAiErrorBody {
                error: crate::openai::OpenAiErrorDetail {
                    code: None,
                    r#type: None,
                    message: text,
                },
            },
        };
        return Err(classify(status.as_u16(), retry_after, &body));
    }

    let mut stream = response.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| HttpFailure::transport(format!("stream read: {e}")))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        if let Some(value) = drain_frames(&mut buf, on_delta) {
            return finish(value, freeform_names, retry_after);
        }
    }
    // Codex's "stream closed before response.completed" — a retryable truncation.
    Err(HttpFailure::transport(
        "stream closed before response.completed".to_string(),
    ))
}

/// Drain complete SSE frames from `buf`, emitting deltas; returns the terminal
/// event's whole response object once seen. Incomplete trailing bytes stay in
/// `buf` for the next chunk.
fn drain_frames(
    buf: &mut String,
    on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
) -> Option<Value> {
    while let Some(pos) = buf.find("\n\n") {
        let frame: String = buf.drain(..pos + 2).collect();
        let Some(data) = sse_data(&frame) else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        // Unknown/newer event types deserialize-fail → skip (forward-compat).
        let Ok(event) = serde_json::from_str::<StreamEvent>(&data) else {
            continue;
        };
        let mut terminal = None;
        if handle(event, on_delta, &mut terminal) {
            return terminal;
        }
    }
    None
}

/// Normalize the terminal response object into a [`Completion`] via the
/// non-streaming parse (byte-identical guarantee).
fn finish(
    value: Value,
    freeform_names: &HashSet<String>,
    retry_after: Option<std::time::Duration>,
) -> Result<Completion, HttpFailure> {
    let parsed: wire::ResponsesResponse = serde_json::from_value(value)
        .map_err(|e| HttpFailure::decode(format!("terminal stream response: {e}")))?;
    response_to_completion(parsed, freeform_names).map_err(|error| HttpFailure {
        error,
        force_terminal: false,
        retry_after,
    })
}

/// Handle one Responses SSE event: emit any display delta, and capture the whole
/// response object on a terminal event. Returns `true` on a terminal event.
fn handle(
    event: StreamEvent,
    on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
    final_response: &mut Option<Value>,
) -> bool {
    match event.kind.as_str() {
        "response.output_text.delta" => {
            if let Some(delta) = event.delta {
                on_delta(CompletionDelta::Text(delta));
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = event.delta {
                on_delta(CompletionDelta::Thinking(delta));
            }
        }
        "response.function_call_arguments.delta" => {
            // Display-only "typing" channel — the whole args land in the finalized
            // item, so this is never assembled/parsed.
            if let Some(delta) = event.delta {
                on_delta(CompletionDelta::ToolArgs(delta));
            }
        }
        "response.output_item.added" => {
            if let Some(item) = &event.item
                && item.get("type").and_then(Value::as_str) == Some("function_call")
            {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                on_delta(CompletionDelta::ToolUseStart { id, name });
            }
        }
        // The whole final response rides the terminal event (completed carries the
        // full output + usage; incomplete/failed carry status/error). Reusing it
        // makes the streamed Completion byte-identical to the non-streaming parse.
        "response.completed" | "response.incomplete" | "response.failed" => {
            *final_response = event.response;
            return true;
        }
        _ => {}
    }
    false
}

/// A Responses SSE event — flat, matching codex's `ResponsesStreamEvent` shape.
/// Only the fields we consume are modeled; everything else is ignored.
#[derive(serde::Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    item: Option<Value>,
    #[serde(default)]
    response: Option<Value>,
}

/// Extract the joined `data:` payload of one SSE frame (`None` for a
/// comment/keep-alive). Multiple `data:` lines join with `\n` per the SSE spec.
fn sse_data(frame: &str) -> Option<String> {
    let mut data = String::new();
    let mut has = false;
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if has {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            has = true;
        }
    }
    has.then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::CompletionDelta;
    use locode_protocol::ContentBlock;
    use serde_json::json;

    /// The whole terminal response — ground truth for the byte-identical test.
    fn completed_response() -> Value {
        json!({
            "status": "completed",
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "Hello world"}]},
                {"type": "function_call", "call_id": "call_1", "name": "get_weather",
                 "arguments": "{\"city\":\"SF\"}"}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        })
    }

    /// The SSE event sequence that leads to `completed_response()`.
    fn transcript() -> String {
        let events = [
            json!({"type": "response.created"}),
            json!({"type": "response.output_item.added",
                   "item": {"type": "function_call", "call_id": "call_1", "name": "get_weather"}}),
            json!({"type": "response.output_text.delta", "delta": "Hello "}),
            json!({"type": "response.output_text.delta", "delta": "world"}),
            json!({"type": "response.function_call_arguments.delta", "delta": "{\"city\":"}),
            json!({"type": "response.function_call_arguments.delta", "delta": "\"SF\"}"}),
            json!({"type": "response.output_item.done",
                   "item": {"type": "function_call", "call_id": "call_1", "name": "get_weather",
                            "arguments": "{\"city\":\"SF\"}"}}),
            json!({"type": "response.completed", "response": completed_response()}),
        ];
        let mut out = String::new();
        for e in events {
            out.push_str("event: x\ndata: ");
            out.push_str(&serde_json::to_string(&e).unwrap());
            out.push_str("\n\n");
        }
        out
    }

    #[test]
    fn stream_assembly_is_byte_identical_and_emits_deltas() {
        let names = HashSet::new();
        let expected = response_to_completion(
            serde_json::from_value(completed_response()).unwrap(),
            &names,
        )
        .unwrap();

        for chunk_size in [1usize, 3, 7, 50, 100_000] {
            let full = transcript();
            let mut buf = String::new();
            let mut deltas = Vec::new();
            let mut got = None;
            for chunk in full.as_bytes().chunks(chunk_size) {
                buf.push_str(&String::from_utf8_lossy(chunk));
                if let Some(value) = drain_frames(&mut buf, &mut |d| deltas.push(d)) {
                    got = Some(finish(value, &names, None).unwrap());
                    break;
                }
            }
            assert_eq!(got.unwrap(), expected, "chunk_size {chunk_size}");
            assert_eq!(
                deltas,
                vec![
                    CompletionDelta::ToolUseStart {
                        id: "call_1".into(),
                        name: "get_weather".into()
                    },
                    CompletionDelta::Text("Hello ".into()),
                    CompletionDelta::Text("world".into()),
                    CompletionDelta::ToolArgs("{\"city\":".into()),
                    CompletionDelta::ToolArgs("\"SF\"}".into()),
                ],
                "chunk_size {chunk_size}"
            );
        }
        // Sanity: the assembled Completion actually carries the text + tool call.
        assert!(
            matches!(&expected.content[0], ContentBlock::Text { text } if text == "Hello world")
        );
        assert!(
            matches!(&expected.content[1], ContentBlock::ToolUse { name, .. } if name == "get_weather")
        );
    }

    #[test]
    fn sse_data_extracts_payload() {
        assert_eq!(
            sse_data("event: x\ndata: {\"a\":1}\n"),
            Some("{\"a\":1}".to_string())
        );
        assert_eq!(sse_data(": comment\n"), None);
    }

    #[test]
    fn failed_terminal_event_maps_to_provider_error() {
        let failed = json!({"status": "failed", "error": {"code": "rate_limit_exceeded", "message": "slow"}});
        let err = finish(failed, &HashSet::new(), None).unwrap_err();
        assert!(matches!(
            err.error,
            crate::provider::ProviderError::RateLimited { .. }
        ));
    }

    #[test]
    fn no_terminal_event_yields_none_then_truncation() {
        let mut buf =
            String::from("data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n");
        let mut deltas = Vec::new();
        assert!(drain_frames(&mut buf, &mut |d| deltas.push(d)).is_none());
        assert_eq!(deltas, vec![CompletionDelta::Text("hi".into())]);
    }

    /// Opt-in live smoke against the real Responses wire via OpenRouter. Needs
    /// `LOCODE_API_KEY`/`LOCODE_BASE_URL` and an **openai-responses** model
    /// (`LOCODE_MODEL=openai/gpt-4o-mini`). Run: `cargo test -p locode-provider --
    /// --ignored live_smoke_responses`.
    #[tokio::test]
    #[ignore = "hits the live API; needs an openai-responses model in LOCODE_MODEL"]
    async fn live_smoke_responses_streaming() {
        use crate::provider::Provider;
        use crate::request::{CacheHint, ConversationRequest, SamplingArgs};
        use locode_protocol::{Message, Role};
        let provider = super::super::OpenAiResponsesProvider::from_env().expect("from_env");
        let request = ConversationRequest {
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Reply with exactly: streaming works".into(),
                }],
            }],
            tools: vec![],
            sampling_args: SamplingArgs::default(),
            cache_hint: CacheHint::default(),
        };
        let mut deltas = Vec::new();
        let completion = provider
            .stream(&request, &mut |d| deltas.push(d))
            .await
            .expect("live stream ok");
        let joined: String = deltas
            .iter()
            .filter_map(|d| match d {
                CompletionDelta::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(Some(joined), completion.text());
    }
}
