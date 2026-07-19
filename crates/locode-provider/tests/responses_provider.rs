//! End-to-end `OpenAiResponsesProvider` tests against a canned local HTTP
//! server (no network): request capture (stateless body, bearer, prefs),
//! tool-call round trip, quota-as-429 terminality, retry behavior.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use locode_protocol::{ContentBlock, Message, Role};
use locode_provider::openai::responses::OpenAiResponsesProvider;
use locode_provider::{
    CacheHint, ConversationRequest, OpenAiModelConfig, Provider, ProviderError, RetryPolicy,
    SamplingArgs, StopReason,
};

/// One canned response: (status, extra headers, body).
type CannedResponse = (u16, Vec<(String, String)>, String);

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

fn canned_server(responses: Vec<CannedResponse>) -> (String, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
    let base_url = format!(
        "http://{}",
        listener.local_addr().unwrap_or_else(|e| panic!("{e}"))
    );
    let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&captured);

    std::thread::spawn(move || {
        for (status, headers, body) in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let n = stream
                    .read(&mut chunk)
                    .unwrap_or_else(|e| panic!("read: {e}"));
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
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
            log.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Captured {
                    head,
                    body: serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null),
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

fn ok_body() -> String {
    r#"{"status": "completed",
        "output": [
            {"type": "reasoning", "id": "rs_live", "summary": [],
             "encrypted_content": "gAAA", "format": "openai-responses-v1"},
            {"type": "message", "content": [{"type": "output_text",
                "text": "hello from the responses wire"}]}
        ],
        "usage": {"input_tokens": 12, "output_tokens": 6,
            "input_tokens_details": {"cached_tokens": 4},
            "output_tokens_details": {"reasoning_tokens": 3}}}"#
        .to_string()
}

fn provider_for(base_url: &str) -> OpenAiResponsesProvider {
    let mut cfg = OpenAiModelConfig::new("gpt-5-mini", base_url, "test-bearer");
    cfg.prompt_cache_key = Some("sess-e2e".into());
    OpenAiResponsesProvider::new(cfg)
        .unwrap_or_else(|e| panic!("provider: {e}"))
        .with_retry_policy(RetryPolicy {
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(4),
            retry_after_cap: Duration::from_millis(10),
            ..RetryPolicy::default()
        })
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

#[tokio::test]
async fn happy_path_sends_stateless_request_and_parses() {
    let (base_url, captured) = canned_server(vec![(200, vec![], ok_body())]);
    let provider = provider_for(&base_url);

    let completion = provider.complete(&simple_request()).await.expect("ok");
    assert_eq!(
        completion.text().as_deref(),
        Some("hello from the responses wire")
    );
    assert_eq!(completion.stop, StopReason::EndTurn);
    assert_eq!(completion.usage.cache_read_tokens, Some(4));
    assert_eq!(completion.usage.reasoning_tokens, Some(3));
    assert!(matches!(
        &completion.content[0],
        ContentBlock::Reasoning {
            payload: Some(_),
            ..
        }
    ));

    let log = captured
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let req = &log[0];
    assert!(req.head.starts_with("POST /v1/responses"));
    assert_eq!(
        req.header("authorization").as_deref(),
        Some("Bearer test-bearer")
    );
    assert_eq!(req.body["store"], false);
    assert_eq!(req.body["instructions"], "you are locode");
    assert_eq!(req.body["include"][0], "reasoning.encrypted_content");
    assert_eq!(req.body["prompt_cache_key"], "sess-e2e");
    assert!(
        req.body.get("provider").is_none(),
        "prefs are OpenRouter-only"
    );
}

#[tokio::test]
async fn quota_as_429_is_terminal_no_retry() {
    let (base_url, captured) = canned_server(vec![(
        429,
        vec![],
        r#"{"error":{"message":"You exceeded your current quota","code":"insufficient_quota"}}"#
            .to_string(),
    )]);
    let provider = provider_for(&base_url);
    let err = provider
        .complete(&simple_request())
        .await
        .expect_err("quota");
    assert!(matches!(err, ProviderError::Quota), "the family trap");
    assert_eq!(
        captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1,
        "no retry against a dead account"
    );
}

#[tokio::test]
async fn retryable_5xx_then_success() {
    let (base_url, captured) = canned_server(vec![
        (
            502,
            vec![],
            r#"{"error":{"message":"bad gateway"}}"#.to_string(),
        ),
        (200, vec![], ok_body()),
    ]);
    let provider = provider_for(&base_url);
    let completion = provider.complete(&simple_request()).await.expect("ok");
    assert_eq!(
        completion.text().as_deref(),
        Some("hello from the responses wire")
    );
    assert_eq!(
        captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        2
    );
}
