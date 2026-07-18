//! HTTP client construction, header assembly, and the single send (plan §4.8).
//!
//! One send, JSON in / JSON out; retry is [`run_with_retry`](super::retry)'s
//! job. Every request carries `anthropic-version`, the auth header selected by
//! the backend (`x-api-key` native, `Authorization: Bearer` gateway), the
//! comma-joined `anthropic-beta` list when non-empty — **mirrored to
//! `x-anthropic-beta` for OpenRouter** (plan §9.2) — and any extra headers.

use std::time::Duration;

use super::config::{ApiBackend, AuthScheme, ModelConfig};
use super::error::{HttpFailure, classify, parse_retry_after};
use super::parse::response_to_completion;
use super::wire;
use crate::completion::Completion;
use crate::provider::ProviderError;

/// Build the reqwest client with the wire's timeouts.
///
/// Non-streaming thinking turns can run minutes, so the total timeout is
/// generous (10 min); the connect timeout stays tight (30 s).
pub(crate) fn build_http_client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_mins(10))
        .build()
        .map_err(|e| ProviderError::Transport(format!("client construction: {e}")))
}

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
    Err(classify(
        status.as_u16(),
        x_should_retry,
        retry_after,
        &body,
    ))
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
