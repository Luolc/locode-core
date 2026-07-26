//! HTTP client construction, header assembly, and the single send (plan §4.8).
//!
//! One send, JSON in / JSON out; retry is [`run_with_retry`](super::retry)'s
//! job. Every request carries `anthropic-version`, the auth header selected by
//! the backend (`x-api-key` native, `Authorization: Bearer` gateway), the
//! comma-joined `anthropic-beta` list when non-empty — **mirrored to
//! `x-anthropic-beta` for OpenRouter** (plan §9.2) — and any extra headers.

use std::collections::BTreeMap;

use tokio_stream::StreamExt;

use super::config::{ApiBackend, AuthScheme, ModelConfig};
use super::error::classify;
use super::parse::response_to_completion;
use super::wire;
use crate::completion::{Completion, CompletionDelta};
use crate::http::{HttpFailure, parse_retry_after};
use crate::provider::ProviderError;

/// Assemble the header pairs for one request (pure; unit-tested).
pub(crate) fn build_header_pairs(cfg: &ModelConfig, auth: &AuthScheme) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();
    headers.push((
        "anthropic-version".to_string(),
        cfg.anthropic_version.clone(),
    ));
    match auth {
        AuthScheme::ApiKey(key) => headers.push(("x-api-key".to_string(), key.clone())),
        AuthScheme::Bearer(token) => {
            headers.push(("authorization".to_string(), format!("Bearer {token}")));
        }
    }
    if !cfg.betas.is_empty() {
        let joined = cfg.betas.join(",");
        headers.push(("anthropic-beta".to_string(), joined.clone()));
        // OpenRouter reads betas from x-anthropic-beta (plan §9.2); mirror like
        // cc-reverse-proxy does — sending both is harmless.
        if cfg.api_backend == ApiBackend::OpenRouter {
            headers.push(("x-anthropic-beta".to_string(), joined));
        }
    }
    headers.extend(cfg.extra_headers.iter().cloned());
    headers
}

/// POST the built request once and normalize the outcome.
///
/// Success → parsed [`Completion`]; HTTP error → classified [`HttpFailure`]
/// (carrying `x-should-retry` / `Retry-After`); transport error → retryable
/// [`HttpFailure`].
pub(crate) async fn send_once(
    http: &reqwest::Client,
    cfg: &ModelConfig,
    auth: &AuthScheme,
    request: &wire::MessagesRequest,
) -> Result<Completion, HttpFailure> {
    let url = format!("{}/v1/messages", cfg.base_url);
    let mut builder = http.post(&url).json(request);
    for (name, value) in build_header_pairs(cfg, auth) {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| HttpFailure::transport(e.to_string()))?;

    let status = response.status();
    if status.is_success() {
        let parsed: wire::MessagesResponse = response
            .json()
            .await
            .map_err(|e| HttpFailure::decode(format!("response body: {e}")))?;
        return response_to_completion(parsed).map_err(|error| HttpFailure {
            error,
            force_terminal: false,
            retry_after: None,
        });
    }

    Err(classify_error_response(response).await)
}

/// Classify a non-2xx `Response` into an [`HttpFailure`] (shared by the
/// non-streaming and streaming sends). Consumes the response to read its body.
async fn classify_error_response(response: reqwest::Response) -> HttpFailure {
    let status = response.status();
    let x_should_retry = response
        .headers()
        .get("x-should-retry")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| match s {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_retry_after);
    // Best-effort body parse: a non-Anthropic error shape (e.g. an OpenRouter
    // body) lands verbatim in `message` so the wording sniffs still apply.
    let text = response.text().await.unwrap_or_default();
    let body: wire::ErrorBody = serde_json::from_str(&text).unwrap_or_else(|_| wire::ErrorBody {
        error: wire::ErrorDetail {
            r#type: String::new(),
            message: text,
        },
    });
    classify(status.as_u16(), x_should_retry, retry_after, &body)
}

/// POST the streaming request and assemble the SSE frames back into the **same**
/// [`Completion`] the non-streaming path produces (ADR-0021), emitting
/// [`CompletionDelta`]s to `on_delta` as they arrive. A single attempt — a
/// retryable failure surfaces to the engine's loop-level resample.
pub(crate) async fn send_once_streaming(
    http: &reqwest::Client,
    cfg: &ModelConfig,
    auth: &AuthScheme,
    request: &wire::MessagesRequest,
    on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
) -> Result<Completion, HttpFailure> {
    let url = format!("{}/v1/messages", cfg.base_url);
    let mut builder = http.post(&url).json(request);
    for (name, value) in build_header_pairs(cfg, auth) {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| HttpFailure::transport(e.to_string()))?;

    if !response.status().is_success() {
        return Err(classify_error_response(response).await);
    }

    // Frame the SSE byte stream by hand (no eventsource dep): accumulate bytes,
    // split on the blank-line event boundary, feed each `data:` payload to the
    // assembler until `message_stop`.
    let mut stream = response.bytes_stream();
    let mut buf = String::new();
    let mut asm = StreamAssembler::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| HttpFailure::transport(format!("stream read: {e}")))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        if drain_frames(&mut buf, &mut asm, on_delta)? {
            return asm.finish();
        }
    }
    // The stream ended without `message_stop` — a truncation (retryable), like
    // codex's "stream closed before response.completed".
    Err(HttpFailure::transport(
        "stream closed before message_stop".to_string(),
    ))
}

/// Drain every complete SSE frame (delimited by a blank line) currently in `buf`,
/// feeding each to the assembler. Returns `Ok(true)` once `message_stop` is seen.
/// Incomplete trailing bytes stay in `buf` for the next chunk.
fn drain_frames(
    buf: &mut String,
    asm: &mut StreamAssembler,
    on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
) -> Result<bool, HttpFailure> {
    while let Some(pos) = buf.find("\n\n") {
        let frame: String = buf.drain(..pos + 2).collect();
        let Some(data) = sse_data(&frame) else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        // Unknown/newer event types deserialize-fail → skip (forward-compat).
        if let Ok(event) = serde_json::from_str::<wire::MessageStreamEvent>(&data)
            && asm.handle(event, on_delta)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Extract the joined `data:` payload of one SSE frame (`None` if the frame has
/// no data line — e.g. a comment/keep-alive). Multiple `data:` lines join with
/// `\n` per the SSE spec.
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

/// Folds Anthropic Messages SSE events back into a whole [`wire::MessagesResponse`],
/// which [`response_to_completion`] then normalizes — so the streamed result is
/// **byte-identical** to the non-streaming parse. Tool arguments accumulate as a
/// raw string per block index and are parsed **once** at `content_block_stop`.
struct StreamAssembler {
    resp: Option<wire::MessagesResponse>,
    tool_json: BTreeMap<u32, String>,
}

impl StreamAssembler {
    fn new() -> Self {
        Self {
            resp: None,
            tool_json: BTreeMap::new(),
        }
    }

    fn resp_mut(&mut self) -> Result<&mut wire::MessagesResponse, HttpFailure> {
        self.resp
            .as_mut()
            .ok_or_else(|| HttpFailure::decode("stream event before message_start".to_string()))
    }

    /// Handle one event; `Ok(true)` means `message_stop` (stream complete).
    fn handle(
        &mut self,
        event: wire::MessageStreamEvent,
        on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
    ) -> Result<bool, HttpFailure> {
        use wire::MessageStreamEvent as E;
        use wire::StreamDelta as D;
        match event {
            E::MessageStart { message } => self.resp = Some(message),
            E::ContentBlockStart {
                index,
                content_block,
            } => {
                if let wire::ContentBlock::ToolUse { id, name, .. } = &content_block {
                    on_delta(CompletionDelta::ToolUseStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                    self.tool_json.insert(index, String::new());
                }
                let resp = self.resp_mut()?;
                let idx = index as usize;
                while resp.content.len() <= idx {
                    resp.content.push(wire::ContentBlock::Text {
                        text: String::new(),
                        cache_control: None,
                    });
                }
                resp.content[idx] = content_block;
            }
            E::ContentBlockDelta { index, delta } => {
                let idx = index as usize;
                match delta {
                    D::TextDelta { text } => {
                        if let Some(wire::ContentBlock::Text { text: t, .. }) =
                            self.resp_mut()?.content.get_mut(idx)
                        {
                            t.push_str(&text);
                        }
                        on_delta(CompletionDelta::Text(text));
                    }
                    D::InputJsonDelta { partial_json } => {
                        self.tool_json
                            .entry(index)
                            .or_default()
                            .push_str(&partial_json);
                        on_delta(CompletionDelta::ToolArgs(partial_json));
                    }
                    D::ThinkingDelta { thinking } => {
                        if let Some(wire::ContentBlock::Thinking { thinking: t, .. }) =
                            self.resp_mut()?.content.get_mut(idx)
                        {
                            t.push_str(&thinking);
                        }
                        on_delta(CompletionDelta::Thinking(thinking));
                    }
                    D::SignatureDelta { signature } => {
                        // Replay-only — attach to the block, no display delta.
                        if let Some(wire::ContentBlock::Thinking { signature: s, .. }) =
                            self.resp_mut()?.content.get_mut(idx)
                        {
                            s.push_str(&signature);
                        }
                    }
                }
            }
            E::ContentBlockStop { index } => {
                if let Some(raw) = self.tool_json.remove(&index) {
                    // Parse the accumulated arguments ONCE, at the finalize boundary.
                    // Malformed args are a MODEL mistake, not a wire failure — e.g.
                    // grok's grep exposes `-A`/`-B`/`-C`/`-i` as literal JSON keys
                    // (faithful port, Task 26) and the model sometimes emits them
                    // unquoted (`{…,-A:3}`). Do NOT fail the whole response (which
                    // would hard-stop the run as a terminal `model_error`): keep the
                    // raw as a `Value::String`, exactly as the OpenAI-Responses wire
                    // does for invalid function-call arguments
                    // (`openai/responses/parse.rs::decode_arguments`). The tool_use
                    // stays in the transcript (pairing holds, ADR-0004); dispatch's
                    // typed decode rejects a bare string for ANY tool — even one whose
                    // args are all-optional, so the error is never masked — as a SOFT
                    // `Respond`, and the loop lets the model retry. The Anthropic wire
                    // requires an object on replay, so `build.rs` coerces this
                    // non-object input back to `{}`.
                    let value = if raw.trim().is_empty() {
                        serde_json::json!({})
                    } else {
                        match serde_json::from_str(&raw) {
                            Ok(v) => v,
                            Err(_) => serde_json::Value::String(raw),
                        }
                    };
                    if let Some(wire::ContentBlock::ToolUse { input, .. }) =
                        self.resp_mut()?.content.get_mut(index as usize)
                    {
                        *input = value;
                    }
                }
            }
            E::MessageDelta { delta, usage } => {
                let resp = self.resp_mut()?;
                resp.stop_reason = delta.stop_reason;
                resp.usage.output_tokens = Some(usage.output_tokens);
                if usage.input_tokens.is_some() {
                    resp.usage.input_tokens = usage.input_tokens;
                }
                if usage.cache_read_input_tokens.is_some() {
                    resp.usage.cache_read_input_tokens = usage.cache_read_input_tokens;
                }
                if usage.cache_creation_input_tokens.is_some() {
                    resp.usage.cache_creation_input_tokens = usage.cache_creation_input_tokens;
                }
            }
            E::MessageStop => return Ok(true),
            E::Ping => {}
            E::Error { error } => return Err(stream_error_to_failure(&error)),
        }
        Ok(false)
    }

    fn finish(self) -> Result<Completion, HttpFailure> {
        let resp = self
            .resp
            .ok_or_else(|| HttpFailure::decode("stream had no message_start".to_string()))?;
        response_to_completion(resp).map_err(|error| HttpFailure {
            error,
            force_terminal: false,
            retry_after: None,
        })
    }
}

/// Map a mid-stream `error` event to an [`HttpFailure`]: `overloaded_error` and
/// `api_error` are retryable (Anthropic 529 / 500); anything else is terminal.
fn stream_error_to_failure(error: &wire::StreamError) -> HttpFailure {
    let (status, force_terminal) = match error.r#type.as_str() {
        "overloaded_error" => (529, false),
        "api_error" => (500, false),
        _ => (400, true),
    };
    HttpFailure {
        error: ProviderError::Api {
            status,
            message: error.message.clone(),
        },
        force_terminal,
        retry_after: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_headers() {
        let cfg = ModelConfig::new("m", "https://api.anthropic.com", "sk-ant-x");
        let pairs = build_header_pairs(&cfg, &cfg.auth);
        assert!(pairs.contains(&("anthropic-version".into(), "2023-06-01".into())));
        assert!(pairs.contains(&("x-api-key".into(), "sk-ant-x".into())));
        assert!(pairs.contains(&(
            "anthropic-beta".into(),
            "interleaved-thinking-2025-05-14".into()
        )));
        assert!(
            !pairs.iter().any(|(n, _)| n == "x-anthropic-beta"),
            "no mirror for native"
        );
        assert!(!pairs.iter().any(|(n, _)| n == "authorization"));
    }

    #[test]
    fn openrouter_headers_mirror_betas_and_use_bearer() {
        let cfg = ModelConfig::new("m", "https://openrouter.ai/api", "sk-or-x");
        let pairs = build_header_pairs(&cfg, &cfg.auth);
        assert!(pairs.contains(&("authorization".into(), "Bearer sk-or-x".into())));
        assert!(pairs.contains(&(
            "anthropic-beta".into(),
            "interleaved-thinking-2025-05-14".into()
        )));
        assert!(pairs.contains(&(
            "x-anthropic-beta".into(),
            "interleaved-thinking-2025-05-14".into()
        )));
        assert!(!pairs.iter().any(|(n, _)| n == "x-api-key"));
    }

    #[test]
    fn empty_betas_send_no_beta_headers_and_extra_headers_append() {
        let mut cfg = ModelConfig::new("m", "https://api.anthropic.com", "k");
        cfg.betas.clear();
        cfg.extra_headers
            .push(("x-custom".to_string(), "v".to_string()));
        let pairs = build_header_pairs(&cfg, &cfg.auth);
        assert!(!pairs.iter().any(|(n, _)| n.contains("beta")));
        assert!(pairs.contains(&("x-custom".into(), "v".into())));
    }

    #[test]
    fn multiple_betas_join_with_commas() {
        let mut cfg = ModelConfig::new("m", "https://openrouter.ai/api", "k");
        cfg.betas.push("effort-2025-11-24".to_string());
        let pairs = build_header_pairs(&cfg, &cfg.auth);
        let joined = "interleaved-thinking-2025-05-14,effort-2025-11-24";
        assert!(pairs.contains(&("anthropic-beta".into(), joined.into())));
        assert!(pairs.contains(&("x-anthropic-beta".into(), joined.into())));
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use crate::provider::Provider;
    use crate::request::{CacheHint, ConversationRequest, SamplingArgs};
    use locode_protocol::{ContentBlock as PBlock, Message, Role};
    use serde_json::json;

    use wire::MessageStreamEvent as E;
    use wire::StreamDelta as D;

    /// The whole non-streaming response — ground truth for the byte-identical test.
    fn whole_response() -> wire::MessagesResponse {
        wire::MessagesResponse {
            id: "msg_1".into(),
            r#type: "message".into(),
            role: "assistant".into(),
            content: vec![
                wire::ContentBlock::Text {
                    text: "Hello world".into(),
                    cache_control: None,
                },
                wire::ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "get_weather".into(),
                    input: json!({"city": "SF"}),
                },
            ],
            model: "claude-x".into(),
            stop_reason: Some(wire::StopReason::ToolUse),
            usage: wire::MessagesUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                output_tokens_details: None,
            },
        }
    }

    fn start_shell() -> wire::MessagesResponse {
        wire::MessagesResponse {
            content: vec![],
            stop_reason: None,
            usage: wire::MessagesUsage {
                input_tokens: Some(10),
                output_tokens: Some(0),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                output_tokens_details: None,
            },
            ..whole_response()
        }
    }

    /// The SSE event sequence that reconstructs `whole_response()`.
    fn events() -> Vec<wire::MessageStreamEvent> {
        vec![
            E::MessageStart {
                message: start_shell(),
            },
            E::ContentBlockStart {
                index: 0,
                content_block: wire::ContentBlock::Text {
                    text: String::new(),
                    cache_control: None,
                },
            },
            E::ContentBlockDelta {
                index: 0,
                delta: D::TextDelta {
                    text: "Hello ".into(),
                },
            },
            E::ContentBlockDelta {
                index: 0,
                delta: D::TextDelta {
                    text: "world".into(),
                },
            },
            E::ContentBlockStop { index: 0 },
            E::ContentBlockStart {
                index: 1,
                content_block: wire::ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "get_weather".into(),
                    input: json!({}),
                },
            },
            E::ContentBlockDelta {
                index: 1,
                delta: D::InputJsonDelta {
                    partial_json: "{\"city\":".into(),
                },
            },
            E::ContentBlockDelta {
                index: 1,
                delta: D::InputJsonDelta {
                    partial_json: "\"SF\"}".into(),
                },
            },
            E::ContentBlockStop { index: 1 },
            E::MessageDelta {
                delta: wire::MessageDeltaBody {
                    stop_reason: Some(wire::StopReason::ToolUse),
                    stop_details: None,
                },
                usage: wire::MessageDeltaUsage {
                    output_tokens: 5,
                    input_tokens: None,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
            },
            E::MessageStop,
        ]
    }

    fn run(
        events: Vec<wire::MessageStreamEvent>,
    ) -> (Result<Completion, HttpFailure>, Vec<CompletionDelta>) {
        let mut deltas = Vec::new();
        let mut asm = StreamAssembler::new();
        for ev in events {
            match asm.handle(ev, &mut |d| deltas.push(d)) {
                Ok(true) => return (asm.finish(), deltas),
                Ok(false) => {}
                Err(e) => return (Err(e), deltas),
            }
        }
        (asm.finish(), deltas)
    }

    #[test]
    fn stream_assembly_is_byte_identical_to_non_streaming() {
        let expected = response_to_completion(whole_response()).expect("non-streaming parse");
        let (got, deltas) = run(events());
        assert_eq!(
            got.expect("streamed"),
            expected,
            "streamed Completion must equal the parsed whole response"
        );
        assert_eq!(
            deltas,
            vec![
                CompletionDelta::Text("Hello ".into()),
                CompletionDelta::Text("world".into()),
                CompletionDelta::ToolUseStart {
                    id: "toolu_1".into(),
                    name: "get_weather".into(),
                },
                CompletionDelta::ToolArgs("{\"city\":".into()),
                CompletionDelta::ToolArgs("\"SF\"}".into()),
            ],
            "delta sequence: text chunks, then ToolUseStart (name/id early), then raw arg fragments"
        );
    }

    fn to_sse(events: &[wire::MessageStreamEvent]) -> String {
        let mut out = String::new();
        for e in events {
            out.push_str("event: x\ndata: ");
            out.push_str(&serde_json::to_string(e).unwrap());
            out.push_str("\n\n");
        }
        out
    }

    #[test]
    fn frames_reassemble_across_arbitrary_byte_boundaries() {
        let transcript = to_sse(&events()); // ASCII-only, safe to split anywhere
        let expected = response_to_completion(whole_response()).unwrap();
        for chunk_size in [1usize, 2, 3, 7, 13, 50, 100_000] {
            let mut buf = String::new();
            let mut asm = StreamAssembler::new();
            let mut deltas = Vec::new();
            let mut done = false;
            for chunk in transcript.as_bytes().chunks(chunk_size) {
                buf.push_str(&String::from_utf8_lossy(chunk));
                if drain_frames(&mut buf, &mut asm, &mut |d| deltas.push(d)).unwrap() {
                    done = true;
                    break;
                }
            }
            assert!(done, "message_stop reached at chunk_size {chunk_size}");
            assert_eq!(asm.finish().unwrap(), expected, "chunk_size {chunk_size}");
            assert_eq!(
                deltas.len(),
                5,
                "same deltas regardless of framing (chunk {chunk_size})"
            );
        }
    }

    #[test]
    fn sse_data_extracts_and_joins_payload_lines() {
        assert_eq!(
            sse_data("event: ping\ndata: {\"a\":1}\n"),
            Some("{\"a\":1}".to_string())
        );
        assert_eq!(sse_data("data: a\ndata: b\n"), Some("a\nb".to_string()));
        assert_eq!(
            sse_data("data:no-leading-space"),
            Some("no-leading-space".to_string())
        );
        assert_eq!(sse_data(": comment only\n"), None);
        assert_eq!(sse_data("event: x\n"), None);
    }

    #[test]
    fn tool_args_parse_once_at_stop_not_mid_stream() {
        let mut asm = StreamAssembler::new();
        let mut sink = |_d| {};
        asm.handle(
            E::MessageStart {
                message: start_shell(),
            },
            &mut sink,
        )
        .unwrap();
        asm.handle(
            E::ContentBlockStart {
                index: 0,
                content_block: wire::ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "n".into(),
                    input: json!({}),
                },
            },
            &mut sink,
        )
        .unwrap();
        // Each fragment is invalid JSON in isolation — must NOT error mid-stream.
        asm.handle(
            E::ContentBlockDelta {
                index: 0,
                delta: D::InputJsonDelta {
                    partial_json: "{\"a\":".into(),
                },
            },
            &mut sink,
        )
        .unwrap();
        asm.handle(
            E::ContentBlockDelta {
                index: 0,
                delta: D::InputJsonDelta {
                    partial_json: "true}".into(),
                },
            },
            &mut sink,
        )
        .unwrap();
        // Parsed ONCE here.
        asm.handle(E::ContentBlockStop { index: 0 }, &mut sink)
            .unwrap();
        asm.handle(
            E::MessageDelta {
                delta: wire::MessageDeltaBody {
                    stop_reason: Some(wire::StopReason::ToolUse),
                    stop_details: None,
                },
                usage: wire::MessageDeltaUsage {
                    output_tokens: 1,
                    input_tokens: None,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
            },
            &mut sink,
        )
        .unwrap();
        assert!(asm.handle(E::MessageStop, &mut sink).unwrap());
        let completion = asm.finish().unwrap();
        match &completion.content[0] {
            PBlock::ToolUse { input, .. } => assert_eq!(input, &json!({"a": true})),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    /// Malformed streamed tool args are a MODEL mistake, not a wire failure: the
    /// stream must NOT hard-error (which would terminate the run as `model_error`).
    /// The raw is preserved as a `Value::String` (parity with the OpenAI-Responses
    /// wire) so dispatch soft-rejects it and the loop lets the model retry.
    #[test]
    fn malformed_tool_json_recovers_as_string_not_a_hard_error() {
        let mut asm = StreamAssembler::new();
        let mut sink = |_d| {};
        asm.handle(
            E::MessageStart {
                message: start_shell(),
            },
            &mut sink,
        )
        .unwrap();
        asm.handle(
            E::ContentBlockStart {
                index: 0,
                content_block: wire::ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "n".into(),
                    input: json!({}),
                },
            },
            &mut sink,
        )
        .unwrap();
        // The model emitted an unquoted key (grok's `-A`/`-B` dash-flag style).
        asm.handle(
            E::ContentBlockDelta {
                index: 0,
                delta: D::InputJsonDelta {
                    partial_json: "{\"pattern\":\"x\",-A:3}".into(),
                },
            },
            &mut sink,
        )
        .unwrap();
        // No error at stop — the run keeps going.
        asm.handle(E::ContentBlockStop { index: 0 }, &mut sink)
            .unwrap();
        asm.handle(E::MessageStop, &mut sink).unwrap();
        let completion = asm.finish().unwrap();
        // The tool_use survives with the raw args preserved as a string, so
        // typed dispatch will reject it as a soft error (never masked).
        match &completion.content[0] {
            PBlock::ToolUse { input, name, .. } => {
                assert_eq!(name, "n");
                assert_eq!(input, &json!("{\"pattern\":\"x\",-A:3}"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn overloaded_error_event_is_retryable() {
        let mut asm = StreamAssembler::new();
        let err = asm
            .handle(
                E::Error {
                    error: wire::StreamError {
                        r#type: "overloaded_error".into(),
                        message: "overloaded".into(),
                    },
                },
                &mut |_d| {},
            )
            .unwrap_err();
        assert!(err.error.retryable(), "overloaded_error must be retryable");
        assert!(matches!(err.error, ProviderError::Api { status: 529, .. }));
    }

    #[test]
    fn unknown_error_event_is_terminal() {
        let mut asm = StreamAssembler::new();
        let err = asm
            .handle(
                E::Error {
                    error: wire::StreamError {
                        r#type: "invalid_request_error".into(),
                        message: "bad".into(),
                    },
                },
                &mut |_d| {},
            )
            .unwrap_err();
        assert!(!err.error.retryable(), "unknown error type is terminal");
        assert!(err.force_terminal);
    }

    /// Opt-in live smoke test against the real Anthropic Messages wire via
    /// OpenRouter. Requires `LOCODE_API_KEY` (+ `LOCODE_BASE_URL`).
    /// Run with: `cargo test -p locode-provider -- --ignored live_smoke`.
    #[tokio::test]
    #[ignore = "hits the live API; needs LOCODE_API_KEY/BASE_URL"]
    async fn live_smoke_streaming_matches_deltas() {
        let provider =
            super::super::AnthropicProvider::from_env().expect("from_env (set LOCODE_API_KEY)");
        let request = ConversationRequest {
            messages: vec![Message {
                role: Role::User,
                content: vec![PBlock::Text {
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
        assert!(!deltas.is_empty(), "expected streamed deltas");
        let joined: String = deltas
            .iter()
            .filter_map(|d| {
                if let CompletionDelta::Text(t) = d {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            Some(joined),
            completion.text(),
            "deltas must reconstruct the final text"
        );
    }
}
