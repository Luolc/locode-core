//! The Anthropic Messages wire — the one live `Provider` (Task 12, ADR-0007).
//!
//! Converts the provider-neutral [`ConversationRequest`]
//! into a Messages request, sends it (non-streaming), and parses the response back
//! into a [`Completion`] — preserving tool-use ids verbatim,
//! thinking blocks *with* signatures, and usage. Owns the transport-tier retry.
//!
//! Design: `tasks/plans/task-12-anthropic-wire.md` (+ §9 addendum) and the
//! ADR-0007 amendment (2026-07-18). Wire structs are ported from Grok Build's
//! `messages.rs`; the conversion logic lives in [`build`] and `parse`.

pub mod build;
mod client;
pub mod config;
pub mod error;
pub mod parse;
pub mod retry;
pub mod wire;

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;

pub use build::{build_request, count_cache_controls, normalize_input_schema};
pub use config::{ApiBackend, AuthScheme, DeveloperRendering, ModelConfig, ReasoningEncoding};
pub use error::{HttpFailure, classify, parse_retry_after};
pub use parse::response_to_completion;
pub use retry::{RetryPolicy, backoff, run_with_retry};

use crate::completion::Completion;
use crate::provider::{Provider, ProviderError};
use crate::repair::repair_pairing;
use crate::request::ConversationRequest;

/// Re-resolve the credential after a 401/403 (plan §4.5).
///
/// Returns `Some(new)` iff a **different** credential was obtained — grok's
/// `refresh_after_unauthorized` only fires on a changed token. v0 is static-key,
/// so the default provider carries no refresher and a 401 is terminal
/// immediately; this seam is what a future OAuth flow plugs into.
pub trait AuthRefresh: Send + Sync {
    /// Produce a fresh credential, or `None` if nothing new is available.
    fn refresh(&self) -> Option<AuthScheme>;
}

/// The live Anthropic Messages `Provider` (non-streaming).
pub struct AnthropicProvider {
    http: reqwest::Client,
    config: ModelConfig,
    retry: RetryPolicy,
    /// Current credential; swapped by the 401 refresh-once path.
    auth: Mutex<AuthScheme>,
    auth_refresh: Option<Arc<dyn AuthRefresh>>,
}

impl AnthropicProvider {
    /// Build a provider from a resolved [`ModelConfig`].
    ///
    /// # Errors
    /// [`ProviderError::Transport`] when the HTTP client cannot be constructed.
    pub fn new(config: ModelConfig) -> Result<Self, ProviderError> {
        Ok(Self {
            http: crate::http::build_http_client()?,
            auth: Mutex::new(config.auth.clone()),
            config,
            retry: RetryPolicy::default(),
            auth_refresh: None,
        })
    }

    /// Build from the environment (`LOCODE_API_KEY` / `LOCODE_BASE_URL` /
    /// `LOCODE_MODEL`).
    ///
    /// # Errors
    /// [`ProviderError::Auth`] when the key is missing;
    /// [`ProviderError::Transport`] when the HTTP client cannot be constructed.
    pub fn from_env() -> Result<Self, ProviderError> {
        Self::new(ModelConfig::from_env()?)
    }

    /// Override the transport retry policy.
    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Install a credential refresher for the 401 refresh-once path.
    #[must_use]
    pub fn with_auth_refresh(mut self, refresh: Arc<dyn AuthRefresh>) -> Self {
        self.auth_refresh = Some(refresh);
        self
    }

    /// The active config (read-only view).
    #[must_use]
    pub fn config(&self) -> &ModelConfig {
        &self.config
    }

    fn current_auth(&self) -> AuthScheme {
        self.auth
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Swap in a refreshed credential; `false` when no *different* credential
    /// is available (grok only re-sends on a changed token).
    fn try_refresh(&self) -> bool {
        let Some(refresher) = &self.auth_refresh else {
            return false;
        };
        let Some(new_auth) = refresher.refresh() else {
            return false;
        };
        let mut current = self.auth.lock().unwrap_or_else(PoisonError::into_inner);
        if *current == new_auth {
            return false;
        }
        *current = new_auth;
        true
    }

    /// One full retry-wrapped exchange with the current credential.
    async fn exchange(&self, request: &wire::MessagesRequest) -> Result<Completion, ProviderError> {
        let auth = self.current_auth();
        run_with_retry(&self.retry, |_attempt| {
            client::send_once(&self.http, &self.config, &auth, request)
        })
        .await
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    #[allow(clippy::unnecessary_literal_bound)] // signature is the trait's
    fn api_schema(&self) -> &str {
        "anthropic"
    }

    async fn complete(&self, request: &ConversationRequest) -> Result<Completion, ProviderError> {
        // 0. This wire has fixed effort mappings on the Budget encoding — an
        //    unknown tier has no principled budget; reject clearly pre-send
        //    (never silently clamp).
        if let Some(crate::request::ReasoningEffort::Other(tier)) =
            &request.sampling_args.reasoning_effort
            && self.config.reasoning_encoding == config::ReasoningEncoding::Budget
        {
            return Err(ProviderError::Config(format!(
                "unknown reasoning-effort tier {tier:?} has no budget mapping on the \
                 anthropic wire (Budget encoding)"
            )));
        }

        // 1. Defensive transcript repair on a clone — a request must never
        //    leave this crate with a dangling tool_use or duplicate tool_result,
        //    regardless of who called it (ADR-0004; the engine runs the same
        //    shared function as the canonical pass).
        let mut repaired = request.clone();
        let _ = repair_pairing(&mut repaired.messages);

        // 2. Build the wire request (system hoist, cache markers, thinking).
        let wire_request = build_request(&repaired, &self.config);

        // 3. Send with transport retry; on a terminal Auth error, refresh once
        //    and re-run (plan §4.5) — a second Auth failure is terminal.
        match self.exchange(&wire_request).await {
            Err(ProviderError::Auth(message)) => {
                if self.try_refresh() {
                    self.exchange(&wire_request).await
                } else {
                    Err(ProviderError::Auth(message))
                }
            }
            other => other,
        }
    }
}
