//! Per-model wire configuration: backend detection, auth, betas, env resolution.
//!
//! The record follows grok's per-model `{ base_url, api_backend, extra_headers }`
//! shape (ADR-0007) plus the auth split a base-URL override forces (native
//! `x-api-key` → proxy `Authorization: Bearer`). OpenRouter is a first-class,
//! auto-detected backend with two quirks of its own (plan §9.2, ADR-0007
//! amendment 2026-07-18): the beta list is mirrored onto `x-anthropic-beta`, and a
//! default `provider` preferences block is injected into the request body.

use crate::provider::ProviderError;

/// The default model when neither config nor `LOCODE_MODEL` names one (plan §9.1).
pub const DEFAULT_MODEL: &str = "claude-sonnet-5";

/// The default first-party endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// The pinned `anthropic-version` header value (Claude Code sends the same).
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The interleaved-thinking beta — **on by default** (plan §9.3): thinking blocks
/// may interleave with tool calls within one assistant turn, and `budget_tokens`
/// may exceed `max_tokens`. Proxy-safe (OpenRouter documents it).
pub const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// The effort-based reasoning beta, required by
/// [`ReasoningEncoding::EffortAdaptive`] (opt-in; Claude Code `betas.ts:15`).
pub const EFFORT_BETA: &str = "effort-2025-11-24";

/// Default output-token cap (Claude Code's `CAPPED_DEFAULT_MAX_TOKENS`).
pub const DEFAULT_MAX_TOKENS_CAP: u32 = 8000;

/// Which endpoint family the wire talks to — selects auth header + quirks.
///
/// This is *configuration*, not a wire schema: all three speak Anthropic Messages
/// (`Provider::api_schema()` stays `"anthropic"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiBackend {
    /// First-party `api.anthropic.com` — auth via `x-api-key`.
    Native,
    /// OpenRouter's Anthropic-compatible endpoint (plan §9.2) — Bearer auth,
    /// betas mirrored to `x-anthropic-beta`, `provider` preferences injected.
    OpenRouter,
    /// Any other Anthropic-compatible gateway — Bearer auth, no extra quirks.
    Proxy,
}

impl ApiBackend {
    /// Detect the backend from a base URL (pinnable by setting the field directly).
    ///
    /// `openrouter.ai` (any subdomain) → [`ApiBackend::OpenRouter`];
    /// `api.anthropic.com` → [`ApiBackend::Native`]; anything else →
    /// [`ApiBackend::Proxy`]. Matches the "base-URL override changes the auth
    /// header" rule (ADR-0007 consequences).
    #[must_use]
    pub fn detect(base_url: &str) -> Self {
        let host = host_of(base_url);
        if host == "openrouter.ai" || host.ends_with(".openrouter.ai") {
            ApiBackend::OpenRouter
        } else if host == "api.anthropic.com" {
            ApiBackend::Native
        } else {
            ApiBackend::Proxy
        }
    }
}

/// Extract the host portion of a URL without pulling in a URL crate.
fn host_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let end = after_scheme
        .find(['/', ':', '?', '#'])
        .unwrap_or(after_scheme.len());
    &after_scheme[..end]
}

/// The resolved credential and how to present it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthScheme {
    /// Native `x-api-key: <key>`.
    ApiKey(String),
    /// Gateway `Authorization: Bearer <token>`.
    Bearer(String),
}

/// How `Role::Developer` messages are rendered on the wire (plan §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeveloperRendering {
    /// Portable default: a `role:"user"` message wrapped in
    /// `<system-reminder>…</system-reminder>`. No beta needed.
    #[default]
    SystemReminder,
    /// A mid-conversation `role:"system"` message (beta-gated opt-in).
    MidConversationSystemBeta,
}

/// How `reasoning_effort` is encoded on the wire (plan §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningEncoding {
    /// `thinking: {type:"enabled", budget_tokens}` — the v0 default; no beta,
    /// widest model support.
    #[default]
    Budget,
    /// `output_config.effort` + `thinking: {type:"adaptive"}` — opt-in; requires
    /// [`EFFORT_BETA`] and an adaptive-capable model.
    EffortAdaptive,
}

/// The per-model wire configuration record (ADR-0007 + plan §3.1/§9).
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// The model id (native `claude-…`; OpenRouter `anthropic/claude-…`).
    pub model: String,
    /// The endpoint base URL (`{base_url}/v1/messages` is the request path).
    pub base_url: String,
    /// The endpoint family — selects the auth header and backend quirks.
    pub api_backend: ApiBackend,
    /// The resolved credential.
    pub auth: AuthScheme,
    /// The `anthropic-version` header value.
    pub anthropic_version: String,
    /// `anthropic-beta` values, **latched per session** (plan §4.8). Defaults to
    /// [`INTERLEAVED_THINKING_BETA`] (plan §9.3).
    pub betas: Vec<String>,
    /// Extra headers appended to every request (gateway auth quirks etc.).
    pub extra_headers: Vec<(String, String)>,
    /// Hard ceiling clamped onto `SamplingArgs.max_tokens`.
    pub max_tokens_cap: u32,
    /// How Developer messages are rendered.
    pub developer_rendering: DeveloperRendering,
    /// How `reasoning_effort` is encoded.
    pub reasoning_encoding: ReasoningEncoding,
    /// OpenRouter `provider` routing preferences. `None` → the default trio
    /// (`ignore: ["amazon-bedrock"], allow_fallbacks: false,
    /// require_parameters: true`) when the backend is OpenRouter; ignored on
    /// other backends.
    pub provider_prefs: Option<serde_json::Value>,
}

impl ModelConfig {
    /// Build a config for `model` against `base_url` with `key`, detecting the
    /// backend and choosing the matching auth scheme.
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        base_url: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        let base_url: String = base_url.into();
        // Trailing slashes would double up when the request path is appended.
        let base_url = base_url.trim_end_matches('/').to_string();
        let api_backend = ApiBackend::detect(&base_url);
        let key = key.into();
        let auth = match api_backend {
            ApiBackend::Native => AuthScheme::ApiKey(key),
            ApiBackend::OpenRouter | ApiBackend::Proxy => AuthScheme::Bearer(key),
        };
        Self {
            model: model.into(),
            base_url,
            api_backend,
            auth,
            anthropic_version: ANTHROPIC_VERSION.to_string(),
            betas: vec![INTERLEAVED_THINKING_BETA.to_string()],
            extra_headers: Vec::new(),
            max_tokens_cap: DEFAULT_MAX_TOKENS_CAP,
            developer_rendering: DeveloperRendering::default(),
            reasoning_encoding: ReasoningEncoding::default(),
            provider_prefs: None,
        }
    }

    /// Resolve the common case from the environment: `LOCODE_API_KEY` (required),
    /// `LOCODE_BASE_URL` (default [`DEFAULT_BASE_URL`]), `LOCODE_MODEL`
    /// (default [`DEFAULT_MODEL`]).
    ///
    /// # Errors
    /// [`ProviderError::Auth`] when `LOCODE_API_KEY` is unset or empty.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = std::env::var("LOCODE_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| ProviderError::Auth("LOCODE_API_KEY is not set".to_string()))?;
        let base_url =
            std::env::var("LOCODE_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let model = std::env::var("LOCODE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Ok(Self::new(model, base_url, key))
    }

    /// Whether the interleaved-thinking beta is active (waives the thinking
    /// budget clamp, plan §9.3).
    #[must_use]
    pub fn interleaved_thinking(&self) -> bool {
        self.betas.iter().any(|b| b == INTERLEAVED_THINKING_BETA)
    }

    /// The OpenRouter `provider` preferences to inject, if any (plan §9.2):
    /// the configured value, or the default trio on the OpenRouter backend.
    #[must_use]
    pub fn effective_provider_prefs(&self) -> Option<serde_json::Value> {
        if self.api_backend != ApiBackend::OpenRouter {
            return None;
        }
        Some(self.provider_prefs.clone().unwrap_or_else(|| {
            serde_json::json!({
                "ignore": ["amazon-bedrock"],
                "allow_fallbacks": false,
                "require_parameters": true,
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_detection() {
        assert_eq!(
            ApiBackend::detect("https://api.anthropic.com"),
            ApiBackend::Native
        );
        assert_eq!(
            ApiBackend::detect("https://openrouter.ai/api"),
            ApiBackend::OpenRouter
        );
        assert_eq!(
            ApiBackend::detect("https://gateway.openrouter.ai/api"),
            ApiBackend::OpenRouter
        );
        assert_eq!(
            ApiBackend::detect("https://my-proxy.example.com:8080/v1"),
            ApiBackend::Proxy
        );
        assert_eq!(
            ApiBackend::detect("http://localhost:8080"),
            ApiBackend::Proxy
        );
    }

    #[test]
    fn auth_scheme_follows_backend() {
        let native = ModelConfig::new("m", "https://api.anthropic.com", "k1");
        assert_eq!(native.auth, AuthScheme::ApiKey("k1".to_string()));
        assert_eq!(native.api_backend, ApiBackend::Native);

        let or = ModelConfig::new("m", "https://openrouter.ai/api/", "k2");
        assert_eq!(or.auth, AuthScheme::Bearer("k2".to_string()));
        assert_eq!(or.api_backend, ApiBackend::OpenRouter);
        assert_eq!(
            or.base_url, "https://openrouter.ai/api",
            "trailing slash trimmed"
        );
    }

    #[test]
    fn interleaved_thinking_default_on_and_removable() {
        let mut cfg = ModelConfig::new("m", DEFAULT_BASE_URL, "k");
        assert!(cfg.interleaved_thinking(), "beta on by default (plan §9.3)");
        cfg.betas.clear();
        assert!(!cfg.interleaved_thinking());
    }

    #[test]
    fn provider_prefs_only_for_openrouter() {
        let native = ModelConfig::new("m", DEFAULT_BASE_URL, "k");
        assert!(native.effective_provider_prefs().is_none());

        let or = ModelConfig::new("m", "https://openrouter.ai/api", "k");
        let prefs = or.effective_provider_prefs().expect("default trio");
        assert_eq!(prefs["require_parameters"], true);
        assert_eq!(prefs["allow_fallbacks"], false);
        assert_eq!(prefs["ignore"][0], "amazon-bedrock");

        let mut custom = ModelConfig::new("m", "https://openrouter.ai/api", "k");
        custom.provider_prefs = Some(serde_json::json!({"only": ["anthropic"]}));
        assert_eq!(
            custom.effective_provider_prefs().expect("custom")["only"][0],
            "anthropic"
        );
    }
}
