//! End-to-end `AnthropicProvider` tests against a canned local HTTP server
//! (std `TcpListener` — no new deps, no network): happy path with header/body
//! capture, 401 refresh-once, 429 surfacing, OpenRouter quirks.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use locode_protocol::{ContentBlock, Message, Role};
use locode_provider::anthropic::{
    AnthropicProvider, ApiBackend, AuthRefresh, AuthScheme, ModelConfig, RetryPolicy,
};
use locode_provider::{
    CacheHint, ConversationRequest, Provider, ProviderError, SamplingArgs, StopReason,
};

/// One canned response: (status, extra headers, body).
type CannedResponse = (u16, Vec<(String, String)>, String);

/// One captured request: the raw head (request line + headers) and the body.
#[derive(Debug, Clone)]
struct Captured {
    head: String,
    body: serde_json::Value,
}

impl Captured {
    fn header(&self, name: &str) -> Option<String> {
        let prefix = format!("{name}: ");
        self.head
            .lines()
            .find(|l| {
                l.to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
            })
            .map(|l| l[prefix.len()..].trim().to_string())
    }
}

/// Serve `responses` (status line + body) one connection each, capturing every
/// request. Returns the base URL and the capture log.
fn canned_server(responses: Vec<CannedResponse>) -> (String, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .unwrap_or_else(|e| panic!("addr: {e}"))
    );
    let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&captured);

    std::thread::spawn(move || {
        for (status, headers, body) in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Read until end of headers, then the content-length body.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let n = stream
                    .read(&mut chunk)
                    .unwrap_or_else(|e| panic!("read: {e}"));
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = find_header_end(&buf) {
                    break pos;
                }
                if n == 0 {
                    return;
                }
            };
            let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let content_length: usize = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|v| v.trim().parse().ok())
                })
                .unwrap_or(0);
            let mut body_bytes = buf[header_end + 4..].to_vec();
            while body_bytes.len() < content_length {
                let n = stream
                    .read(&mut chunk)
                    .unwrap_or_else(|e| panic!("read body: {e}"));
                if n == 0 {
                    break;
                }
                body_bytes.extend_from_slice(&chunk[..n]);
            }
            let parsed_body =
                serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);
            log.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Captured {
                    head,
                    body: parsed_body,
                });

            let mut extra = String::new();
            for (k, v) in &headers {
                extra.push_str(k);
                extra.push_str(": ");
                extra.push_str(v);
                extra.push_str("\r\n");
            }
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .unwrap_or_else(|e| panic!("write: {e}"));
        }
    });

    (base_url, captured)
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn ok_response_body() -> String {
    r#"{
        "id": "msg_live", "type": "message", "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "plan the answer", "signature": "sig-live-1"},
            {"type": "text", "text": "hello from the wire"}
        ],
        "model": "claude-sonnet-5", "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 5,
                  "cache_creation_input_tokens": 8, "cache_read_input_tokens": 2}
    }"#
    .to_string()
}

fn error_body(r#type: &str, message: &str) -> String {
    format!(r#"{{"type":"error","error":{{"type":"{type}","message":"{message}"}}}}"#)
}

fn simple_request() -> ConversationRequest {
    ConversationRequest {
        messages: vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "you are locode".into(),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "say hello".into(),
                }],
            },
        ],
        tools: vec![],
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Standard,
    }
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(4),
        retry_after_cap: Duration::from_millis(10),
        ..RetryPolicy::default()
    }
}

fn provider_for(base_url: &str, key: &str) -> AnthropicProvider {
    AnthropicProvider::new(ModelConfig::new("claude-sonnet-5", base_url, key))
        .unwrap_or_else(|e| panic!("provider: {e}"))
        .with_retry_policy(fast_retry())
}

#[tokio::test]
async fn happy_path_sends_the_right_request_and_parses() {
    let (base_url, captured) = canned_server(vec![(200, vec![], ok_response_body())]);
    let provider = provider_for(&base_url, "test-key");

    let completion = provider.complete(&simple_request()).await.expect("ok");
    assert_eq!(completion.text().as_deref(), Some("hello from the wire"));
    assert_eq!(completion.stop, StopReason::EndTurn);
    assert!(matches!(
        &completion.content[0],
        ContentBlock::Reasoning { signature: Some(sig), .. } if sig == "sig-live-1"
    ));
    assert_eq!(completion.usage.cache_creation_tokens, Some(8));

    let log = captured.lock().expect("lock");
    let req = &log[0];
    // Bearer auth: 127.0.0.1 is a Proxy backend (non-Anthropic base URL).
    assert_eq!(
        req.header("authorization").as_deref(),
        Some("Bearer test-key")
    );
    assert_eq!(
        req.header("anthropic-version").as_deref(),
        Some("2023-06-01")
    );
    assert_eq!(
        req.header("anthropic-beta").as_deref(),
        Some("interleaved-thinking-2025-05-14")
    );
    assert!(req.head.starts_with("POST /v1/messages"));
    // Body shape: hoisted system, stream:false, cache markers, no provider prefs.
    assert_eq!(req.body["stream"], false);
    assert_eq!(req.body["model"], "claude-sonnet-5");
    assert!(
        req.body.get("provider").is_none(),
        "prefs are OpenRouter-only"
    );
    assert_eq!(req.body["system"][0]["cache_control"]["type"], "ephemeral");
}

#[tokio::test]
async fn openrouter_backend_mirrors_betas_and_injects_prefs() {
    let (base_url, captured) = canned_server(vec![(200, vec![], ok_response_body())]);
    // 127.0.0.1 cannot auto-detect as OpenRouter — pin the backend explicitly
    // (the detection itself is covered in config tests).
    let mut cfg = ModelConfig::new("anthropic/claude-sonnet-5", &base_url, "sk-or-key");
    cfg.api_backend = ApiBackend::OpenRouter;
    let provider = AnthropicProvider::new(cfg)
        .expect("provider")
        .with_retry_policy(fast_retry());

    provider.complete(&simple_request()).await.expect("ok");

    let log = captured.lock().expect("lock");
    let req = &log[0];
    assert_eq!(
        req.header("x-anthropic-beta").as_deref(),
        Some("interleaved-thinking-2025-05-14"),
        "beta mirrored for OpenRouter"
    );
    assert_eq!(req.body["provider"]["require_parameters"], true);
    assert_eq!(req.body["provider"]["allow_fallbacks"], false);
}

#[tokio::test]
async fn retries_5xx_then_succeeds() {
    let (base_url, captured) = canned_server(vec![
        (503, vec![], error_body("api_error", "unavailable")),
        (200, vec![], ok_response_body()),
    ]);
    let provider = provider_for(&base_url, "k");
    let completion = provider.complete(&simple_request()).await.expect("ok");
    assert_eq!(completion.text().as_deref(), Some("hello from the wire"));
    assert_eq!(captured.lock().expect("lock").len(), 2, "one retry");
}

#[tokio::test]
async fn rate_limit_is_surfaced_after_cap() {
    let (base_url, captured) = canned_server(vec![
        (
            429,
            vec![("retry-after".into(), "0".into())],
            error_body("rate_limit_error", "slow"),
        ),
        (
            429,
            vec![("retry-after".into(), "0".into())],
            error_body("rate_limit_error", "slow"),
        ),
        (
            429,
            vec![("retry-after".into(), "0".into())],
            error_body("rate_limit_error", "slow"),
        ),
    ]);
    let provider = provider_for(&base_url, "k");
    let err = provider
        .complete(&simple_request())
        .await
        .expect_err("surfaced");
    assert!(matches!(err, ProviderError::RateLimited { .. }));
    assert_eq!(
        captured.lock().expect("lock").len(),
        3,
        "2 rate-limit retries then surfaced"
    );
}

#[tokio::test]
async fn x_should_retry_false_stops_a_retryable_status() {
    let (base_url, captured) = canned_server(vec![(
        503,
        vec![("x-should-retry".into(), "false".into())],
        error_body("api_error", "do not retry"),
    )]);
    let provider = provider_for(&base_url, "k");
    let err = provider
        .complete(&simple_request())
        .await
        .expect_err("terminal");
    assert!(matches!(err, ProviderError::Api { status: 503, .. }));
    assert_eq!(captured.lock().expect("lock").len(), 1, "no retry");
}

struct ScriptedRefresh(AuthScheme);
impl AuthRefresh for ScriptedRefresh {
    fn refresh(&self) -> Option<AuthScheme> {
        Some(self.0.clone())
    }
}

#[tokio::test]
async fn auth_refresh_once_retries_with_the_new_credential() {
    let (base_url, captured) = canned_server(vec![
        (401, vec![], error_body("authentication_error", "expired")),
        (200, vec![], ok_response_body()),
    ]);
    let provider = provider_for(&base_url, "stale-key").with_auth_refresh(Arc::new(
        ScriptedRefresh(AuthScheme::Bearer("fresh-key".into())),
    ));

    let completion = provider.complete(&simple_request()).await.expect("ok");
    assert_eq!(completion.text().as_deref(), Some("hello from the wire"));

    let log = captured.lock().expect("lock");
    assert_eq!(log.len(), 2);
    assert_eq!(
        log[0].header("authorization").as_deref(),
        Some("Bearer stale-key")
    );
    assert_eq!(
        log[1].header("authorization").as_deref(),
        Some("Bearer fresh-key"),
        "second attempt carries the refreshed credential"
    );
}

#[tokio::test]
async fn auth_refresh_with_same_credential_is_terminal() {
    let (base_url, captured) = canned_server(vec![(
        401,
        vec![],
        error_body("authentication_error", "bad key"),
    )]);
    // Refresher returns the SAME credential → no re-send (grok's changed-token
    // rule); the 401 is terminal after one request.
    let provider = provider_for(&base_url, "same-key").with_auth_refresh(Arc::new(
        ScriptedRefresh(AuthScheme::Bearer("same-key".into())),
    ));
    let err = provider
        .complete(&simple_request())
        .await
        .expect_err("terminal");
    assert!(matches!(err, ProviderError::Auth(_)));
    assert_eq!(captured.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn quota_error_is_terminal_with_no_retry() {
    let (base_url, captured) = canned_server(vec![(
        400,
        vec![],
        error_body(
            "invalid_request_error",
            "Your credit balance is too low to access the API",
        ),
    )]);
    let provider = provider_for(&base_url, "k");
    let err = provider
        .complete(&simple_request())
        .await
        .expect_err("quota");
    assert!(matches!(err, ProviderError::Quota));
    assert_eq!(captured.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn dangling_tool_use_is_repaired_before_send() {
    let (base_url, captured) = canned_server(vec![(200, vec![], ok_response_body())]);
    let provider = provider_for(&base_url, "k");

    // An assistant tool_use with NO tool_result — the defensive pre-send repair
    // must synthesize an is_error result (ADR-0004).
    let request = ConversationRequest {
        messages: vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text { text: "go".into() }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_dangling".into(),
                    name: "grep".into(),
                    input: serde_json::json!({}),
                }],
            },
        ],
        tools: vec![],
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Off,
    };
    provider.complete(&request).await.expect("ok");

    let log = captured.lock().expect("lock");
    let messages = log[0].body["messages"].as_array().expect("messages");
    let last = messages.last().expect("non-empty");
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"][0]["type"], "tool_result");
    assert_eq!(last["content"][0]["tool_use_id"], "toolu_dangling");
    assert_eq!(last["content"][0]["is_error"], true);
}
