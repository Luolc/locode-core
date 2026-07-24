//! The OpenAI Responses wire — the second live `Provider` (Task 18).
//!
//! `api_schema() = "openai-responses"`, `POST {base_url}/v1/responses`,
//! non-streaming, always-Bearer, **stateless always** (`store:false`, no
//! `previous_response_id`). Drives both OpenAI models (function + custom/
//! grammar tools, encrypted-reasoning replay) and xAI grok models (function
//! tools + encrypted reasoning; `custom_tools_supported=false` degrades
//! freeform specs). Design: `tasks/plans/task-18-openai-responses-wire.md`.

pub mod build;
pub mod parse;
mod stream;
pub mod wire;

use std::sync::Arc;

use async_trait::async_trait;

pub use build::{build_request, freeform_fallback_parameters, freeform_tool_names};
pub use parse::response_to_completion;

use crate::completion::{Completion, CompletionDelta};
use crate::http::{self, HttpFailure, RetryPolicy};
use crate::openai::{OpenAiModelConfig, classify};
use crate::provider::{Provider, ProviderError};
use crate::repair::repair_pairing;
use crate::request::ConversationRequest;

/// The live OpenAI Responses `Provider` (non-streaming, stateless).
pub struct OpenAiResponsesProvider {
    http: reqwest::Client,
    config: OpenAiModelConfig,
    retry: RetryPolicy,
}

impl OpenAiResponsesProvider {
    /// Build a provider from a resolved [`OpenAiModelConfig`].
    ///
    /// # Errors
    /// [`ProviderError::Transport`] when the HTTP client cannot be constructed.
    pub fn new(config: OpenAiModelConfig) -> Result<Self, ProviderError> {
        Ok(Self {
            http: http::build_http_client()?,
            config,
            retry: RetryPolicy::default(),
        })
    }

    /// Build from the environment (`LOCODE_API_KEY` / `LOCODE_BASE_URL`); the
    /// model comes from `--model`/settings, not env (ADR-0024 §1.4).
    ///
    /// # Errors
    /// [`ProviderError::Auth`] when the key is missing;
    /// [`ProviderError::Transport`] when the client cannot be constructed.
    pub fn from_env() -> Result<Self, ProviderError> {
        Self::new(OpenAiModelConfig::from_env()?)
    }

    /// Override the transport retry policy.
    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The active config (read-only view).
    #[must_use]
    pub fn config(&self) -> &OpenAiModelConfig {
        &self.config
    }

    /// Mutable config access (the facade sets `prompt_cache_key` to the
    /// session id, plan §A.5 Q4).
    pub fn config_mut(&mut self) -> &mut OpenAiModelConfig {
        &mut self.config
    }

    async fn send_once(
        &self,
        request: &wire::ResponsesRequest,
        freeform_names: &std::collections::HashSet<String>,
    ) -> Result<Completion, HttpFailure> {
        let url = format!("{}/v1/responses", self.config.base_url);
        let mut builder = self
            .http
            .post(&url)
            .bearer_auth(&self.config.bearer)
            .json(request);
        for (name, value) in &self.config.extra_headers {
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
            .and_then(http::parse_retry_after);

        if status.is_success() {
            let parsed: wire::ResponsesResponse = response
                .json()
                .await
                .map_err(|e| HttpFailure::decode(format!("response body: {e}")))?;
            return response_to_completion(parsed, freeform_names).map_err(|error| HttpFailure {
                // A `failed` response's rate-limit/server-error codes ARE
                // retryable — let the shared loop see them as such.
                force_terminal: false,
                retry_after,
                error,
            });
        }

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
        Err(classify(status.as_u16(), retry_after, &body))
    }
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
    #[allow(clippy::unnecessary_literal_bound)] // signature is the trait's
    fn api_schema(&self) -> &str {
        "openai-responses"
    }

    async fn complete(&self, request: &ConversationRequest) -> Result<Completion, ProviderError> {
        // 1. Defensive transcript repair on a clone (ADR-0004 — same pass as
        //    the anthropic wire; the engine runs the canonical one).
        let mut repaired = request.clone();
        let _ = repair_pairing(&mut repaired.messages);

        // 2. Build (stateless, include, reasoning, tools + degradation).
        let wire_request = build_request(&repaired, &self.config);
        let freeform_names = freeform_tool_names(&repaired.tools);

        // 3. Send with the shared transport retry.
        let freeform_ref = &freeform_names;
        http::run_with_retry(&self.retry, |_attempt| {
            self.send_once(&wire_request, freeform_ref)
        })
        .await
    }

    async fn stream(
        &self,
        request: &ConversationRequest,
        on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
    ) -> Result<Completion, ProviderError> {
        // Same repair + build as `complete`, but streaming (the whole final
        // response still rides the terminal SSE event → byte-identical result).
        let mut repaired = request.clone();
        let _ = repair_pairing(&mut repaired.messages);
        let mut wire_request = build_request(&repaired, &self.config);
        wire_request.stream = true;
        let freeform_names = freeform_tool_names(&repaired.tools);

        // Single attempt — a retryable failure surfaces to the engine's resample.
        stream::send_once_streaming(
            &self.http,
            &self.config,
            &wire_request,
            &freeform_names,
            on_delta,
        )
        .await
        .map_err(|f| f.error)
    }
}

/// The provider wrapped in [`Arc`] for the engine's `Arc<dyn Provider>` slot.
#[must_use]
pub fn into_provider(provider: OpenAiResponsesProvider) -> Arc<dyn Provider> {
    Arc::new(provider)
}
