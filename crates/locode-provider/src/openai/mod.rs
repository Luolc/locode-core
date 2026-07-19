//! The OpenAI wire family (Task 18): shared config record, backend detection,
//! and error classification for the Responses wire (and the deferred Chat
//! Completions wire, Task 17).
//!
//! Design: `tasks/plans/task-18-openai-responses-wire.md` (+ addenda A.1–A.6).
//! Auth is **always Bearer** for this family; the backend variants select
//! quirks only (OpenRouter `provider` prefs injection).

pub mod responses;

use std::time::Duration;

use crate::http::HttpFailure;
use crate::provider::ProviderError;

/// The default first-party endpoint.
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

/// The default model (plan §A.5 Q1): the convenience default, mirroring the
/// Anthropic wire. Namespaced per backend at resolution time.
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5-mini";

/// Endpoint family — quirks only; auth is always Bearer here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiBackend {
    /// First-party `api.openai.com`.
    Native,
    /// OpenRouter's `/api/v1/responses` (beta; stateless-only).
    OpenRouter,
    /// Any other OpenAI-compatible gateway.
    Proxy,
}

impl OpenAiBackend {
    /// Detect the backend from a base URL (pinnable by setting the field).
    #[must_use]
    pub fn detect(base_url: &str) -> Self {
        let host = host_of(base_url);
        if host == "openrouter.ai" || host.ends_with(".openrouter.ai") {
            OpenAiBackend::OpenRouter
        } else if host == "api.openai.com" {
            OpenAiBackend::Native
        } else {
            OpenAiBackend::Proxy
        }
    }
}

/// Extract the host portion of a URL without a URL crate.
fn host_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let end = after_scheme
        .find(['/', ':', '?', '#'])
        .unwrap_or(after_scheme.len());
    &after_scheme[..end]
}

/// The per-model config record for the OpenAI wire family (plan §3.2 + §A.5).
#[derive(Debug, Clone)]
pub struct OpenAiModelConfig {
    /// The model id (`gpt-…` native; `openai/…`/`x-ai/…` via OpenRouter).
    pub model: String,
    /// The endpoint base URL (the wire appends its request path).
    pub base_url: String,
    /// The endpoint family — quirks only (auth is always Bearer).
    pub backend: OpenAiBackend,
    /// The bearer credential.
    pub bearer: String,
    /// Clamp for `max_output_tokens` (which includes reasoning tokens).
    pub max_tokens_cap: u32,
    /// `reasoning.summary` request value. Default `Some("auto")` (plan §A.5
    /// Q2: trace readability); `None` omits the field for score-only runs.
    pub reasoning_summary: Option<String>,
    /// `prompt_cache_key` — the facade passes the session id (plan §A.5 Q4;
    /// codex's behavior; probe P6 confirmed harmless for xAI models).
    pub prompt_cache_key: Option<String>,
    /// Whether the target model/provider accepts `type:"custom"` tools. Manual
    /// flag (plan §A.5 Q5 — no model-id sniffing): xAI 422s them (probe P3);
    /// `false` degrades freeform specs to the `{"input": string}` function
    /// framing on both the tools array and the call/replay items.
    pub custom_tools_supported: bool,
    /// Where `Role::System` goes: codex's top-level `instructions` (default)
    /// or grok's in-stream `role:"system"` message.
    pub system_placement: SystemPlacement,
    /// Extra headers appended to every request.
    pub extra_headers: Vec<(String, String)>,
    /// OpenRouter routing preferences; `None` → the default pair
    /// (`allow_fallbacks:false, require_parameters:true`) on OpenRouter.
    pub provider_prefs: Option<serde_json::Value>,
}

/// Where the hoisted System prompt is placed on the Responses wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SystemPlacement {
    /// Top-level `instructions` (codex's shape; probes P4/P5 confirm it works
    /// through OpenRouter for OpenAI and xAI models). The default.
    #[default]
    Instructions,
    /// A `role:"system"` input message (grok's shape) — the escape hatch.
    InputMessage,
}

impl OpenAiModelConfig {
    /// Build a config, detecting the backend from `base_url`.
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        base_url: impl Into<String>,
        bearer: impl Into<String>,
    ) -> Self {
        let base_url: String = base_url.into();
        let base_url = base_url.trim_end_matches('/').to_string();
        let backend = OpenAiBackend::detect(&base_url);
        Self {
            model: model.into(),
            base_url,
            backend,
            bearer: bearer.into(),
            max_tokens_cap: 32_000,
            reasoning_summary: Some("auto".to_string()),
            prompt_cache_key: None,
            custom_tools_supported: true,
            system_placement: SystemPlacement::default(),
            extra_headers: Vec::new(),
            provider_prefs: None,
        }
    }

    /// Resolve from the environment: `LOCODE_API_KEY` (required),
    /// `LOCODE_BASE_URL` (default native), `LOCODE_MODEL` (default
    /// [`DEFAULT_OPENAI_MODEL`], namespaced `openai/…` on OpenRouter).
    ///
    /// # Errors
    /// [`ProviderError::Auth`] when `LOCODE_API_KEY` is unset or empty.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = std::env::var("LOCODE_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| ProviderError::Auth("LOCODE_API_KEY is not set".to_string()))?;
        let base_url = std::env::var("LOCODE_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_OPENAI_BASE_URL.to_string());
        let model = std::env::var("LOCODE_MODEL").unwrap_or_else(|_| {
            // Keep the default pair coherent per backend: OpenRouter slugs are
            // vendor-namespaced.
            if OpenAiBackend::detect(&base_url) == OpenAiBackend::OpenRouter {
                format!("openai/{DEFAULT_OPENAI_MODEL}")
            } else {
                DEFAULT_OPENAI_MODEL.to_string()
            }
        });
        Ok(Self::new(model, base_url, key))
    }

    /// The OpenRouter `provider` preferences to inject, if any: the configured
    /// value, or `{allow_fallbacks:false}` on the OpenRouter backend.
    ///
    /// **`require_parameters` is deliberately absent** (live finding,
    /// 2026-07-19): on the beta `/v1/responses` endpoint, `require_parameters:
    /// true` combined with a `tools` array 404s with "No endpoints found that
    /// can handle the requested parameters" — OpenRouter's parameter registry
    /// does not count `tools` as supported there. The silent-param-drop
    /// protection it provided on the Messages endpoint is covered here by the
    /// live smokes instead.
    #[must_use]
    pub fn effective_provider_prefs(&self) -> Option<serde_json::Value> {
        if self.backend != OpenAiBackend::OpenRouter {
            return None;
        }
        Some(self.provider_prefs.clone().unwrap_or_else(|| {
            serde_json::json!({
                "allow_fallbacks": false,
            })
        }))
    }
}

/// An OpenAI-family error body: OpenAI sends
/// `{"error":{"message","type","param","code"}}` (`code` a string); OpenRouter
/// sends `{"error":{"code": <number>, "message", "metadata"}}`. One shape
/// tolerating both.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct OpenAiErrorBody {
    /// The inner error object.
    #[serde(default)]
    pub error: OpenAiErrorDetail,
}

/// The inner error object of an [`OpenAiErrorBody`].
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct OpenAiErrorDetail {
    /// OpenAI's string code OR OpenRouter's numeric code.
    #[serde(default)]
    pub code: Option<serde_json::Value>,
    /// The error type slug (OpenAI).
    #[serde(rename = "type", default)]
    pub r#type: Option<String>,
    /// The human-readable message.
    #[serde(default)]
    pub message: String,
}

impl OpenAiErrorDetail {
    /// The string form of `code`, if it is a string.
    #[must_use]
    pub fn code_str(&self) -> Option<&str> {
        self.code.as_ref().and_then(|c| c.as_str())
    }
}

/// Classify an OpenAI-family HTTP error (plan §4.7).
///
/// The family trap: **quota exhaustion arrives as a 429** (`insufficient_quota`
/// / "exceeded your current quota") — blind 429-retry hammers a dead account,
/// so quota is matched before rate limiting. OpenRouter's 402 is quota too.
#[must_use]
pub fn classify(status: u16, retry_after: Option<Duration>, body: &OpenAiErrorBody) -> HttpFailure {
    let detail = &body.error;
    let message = detail.message.as_str();
    let lower = message.to_ascii_lowercase();
    let code = detail.code_str().unwrap_or_default();
    let type_slug = detail.r#type.as_deref().unwrap_or_default();

    let error = if status == 401 || status == 403 {
        ProviderError::Auth(format!("http {status}: {message}"))
    } else if code == "insufficient_quota"
        || type_slug == "insufficient_quota"
        || lower.contains("exceeded your current quota")
        || status == 402
    {
        ProviderError::Quota
    } else if status == 429 {
        ProviderError::RateLimited { retry_after }
    } else if status == 413
        || code == "context_length_exceeded"
        || lower.contains("context window")
        || lower.contains("maximum context length")
    {
        ProviderError::ContextOverflow
    } else {
        ProviderError::Api {
            status,
            message: message.to_string(),
        }
    };

    HttpFailure {
        error,
        // No x-should-retry analog exists in this family.
        force_terminal: false,
        retry_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: &str) -> OpenAiErrorBody {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn backend_detection_and_default_prefs() {
        assert_eq!(
            OpenAiBackend::detect("https://api.openai.com"),
            OpenAiBackend::Native
        );
        assert_eq!(
            OpenAiBackend::detect("https://openrouter.ai/api"),
            OpenAiBackend::OpenRouter
        );
        assert_eq!(
            OpenAiBackend::detect("http://localhost:9999"),
            OpenAiBackend::Proxy
        );

        let native = OpenAiModelConfig::new("gpt-5-mini", "https://api.openai.com", "k");
        assert!(native.effective_provider_prefs().is_none());
        let or = OpenAiModelConfig::new("openai/gpt-5-mini", "https://openrouter.ai/api/", "k");
        assert_eq!(or.base_url, "https://openrouter.ai/api");
        let prefs = or.effective_provider_prefs().expect("prefs");
        assert_eq!(prefs["allow_fallbacks"], false);
        assert!(
            prefs.get("require_parameters").is_none(),
            "404s /v1/responses when tools are present (live finding)"
        );
    }

    #[test]
    fn review_defaults_hold() {
        let cfg = OpenAiModelConfig::new("m", DEFAULT_OPENAI_BASE_URL, "k");
        assert_eq!(cfg.reasoning_summary.as_deref(), Some("auto"), "A.5 Q2");
        assert!(
            cfg.custom_tools_supported,
            "A.5 Q5: manual flag, default on"
        );
        assert_eq!(cfg.prompt_cache_key, None, "facade injects the session id");
    }

    #[test]
    fn quota_beats_rate_limit_on_429() {
        // OpenAI string-code form.
        let f = classify(
            429,
            None,
            &body(
                r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
            ),
        );
        assert!(matches!(f.error, ProviderError::Quota), "the family trap");
        assert!(!f.error.retryable());

        // Plain 429 stays retryable-with-cap.
        let f = classify(
            429,
            Some(Duration::from_secs(3)),
            &body(r#"{"error":{"message":"Rate limit reached","code":"rate_limit_exceeded"}}"#),
        );
        assert!(matches!(f.error, ProviderError::RateLimited { .. }));

        // OpenRouter numeric-code 402.
        let f = classify(
            402,
            None,
            &body(r#"{"error":{"code":402,"message":"Insufficient credits"}}"#),
        );
        assert!(matches!(f.error, ProviderError::Quota));
    }

    #[test]
    fn context_and_auth_and_5xx() {
        let f = classify(
            400,
            None,
            &body(
                r#"{"error":{"message":"This model's maximum context length is 400000 tokens","code":"context_length_exceeded"}}"#,
            ),
        );
        assert!(matches!(f.error, ProviderError::ContextOverflow));

        let f = classify(401, None, &body(r#"{"error":{"message":"bad key"}}"#));
        assert!(matches!(f.error, ProviderError::Auth(_)));

        let f = classify(503, None, &body(r#"{"error":{"message":"overloaded"}}"#));
        assert!(f.error.retryable());
        assert!(!f.force_terminal, "no x-should-retry in this family");
    }
}
