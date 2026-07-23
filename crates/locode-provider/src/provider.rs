//! The `Provider` trait (one wire schema per impl) and its error taxonomy (ADR-0007).

use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

use crate::completion::{Completion, CompletionDelta};
use crate::request::ConversationRequest;

/// A model-sampling wire: builds a request, calls the model, normalizes the response.
///
/// **One impl per wire schema** — not per gateway. A `Provider` identifies a
/// request/response *protocol shape* (Anthropic Messages, OpenAI Chat, OpenAI
/// Responses, or the `mock` double); a gateway/endpoint (OpenRouter, Bedrock, a
/// proxy, a local model) is *configuration* (`base_url` + auth + headers) pointed at
/// one of these schemas, never a separate impl. This is Grok Build's split
/// (`ApiBackend` schema vs an un-enumerated `base_url`/auth) and what ADR-0007's
/// per-model `{ base_url, api_backend, extra_headers }` record encodes.
#[async_trait]
pub trait Provider: Send + Sync {
    /// The wire-schema id this provider speaks — e.g. `"anthropic"`, `"mock"`.
    ///
    /// This is the *protocol shape*, not a gateway. Stamped into the report's
    /// `provider` field.
    fn api_schema(&self) -> &str;

    /// Sample one completion for `request`.
    ///
    /// # Errors
    /// Returns [`ProviderError`]; use [`ProviderError::retryable`] to classify
    /// retry-vs-terminal. The transport-tier backoff lives in the wire; the
    /// bounded loop-level resample lives in the engine.
    async fn complete(&self, request: &ConversationRequest) -> Result<Completion, ProviderError>;

    /// Sample one completion, emitting [`CompletionDelta`]s to `on_delta` as they
    /// arrive, and returning the **same final [`Completion`]** as [`complete`]
    /// (ADR-0021). Deltas are a display-only side channel; the returned
    /// `Completion` is what the engine appends and dispatches from.
    ///
    /// **Default** (this impl): non-streaming wires call [`complete`] and emit one
    /// synthetic text delta, so the seam works for every provider (`mock`, and any
    /// wire that hasn't implemented SSE). SSE wires override this to stream live.
    ///
    /// # Errors
    /// Returns [`ProviderError`], same taxonomy as [`complete`].
    async fn stream(
        &self,
        request: &ConversationRequest,
        on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
    ) -> Result<Completion, ProviderError> {
        let completion = self.complete(request).await?;
        if let Some(text) = completion.text() {
            on_delta(CompletionDelta::Text(text));
        }
        Ok(completion)
    }
}

/// Why a model call failed (ADR-0007).
///
/// An **exhaustive** taxonomy (not `#[non_exhaustive]`): like Grok's `SamplingError`
/// and Codex's `CodexErr`, [`ProviderError::retryable`] matches every variant with
/// no wildcard, so a new error cannot be added without classifying it. Distinct
/// terminal variants (context overflow, quota, auth) follow Codex; the general
/// [`ProviderError::Api`] is the escape hatch for unclassified HTTP statuses.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// A transport/network failure (connection reset, DNS, TLS). Retryable.
    #[error("transport error: {0}")]
    Transport(String),
    /// Rate limited (HTTP 429). Surfaced, not silently hammered; a bounded retry
    /// honors `retry_after` when present.
    #[error("rate limited")]
    RateLimited {
        /// The server-requested delay before retrying, if given (`Retry-After`).
        retry_after: Option<Duration>,
    },
    /// A general API error carrying the HTTP status; retryable iff 5xx/overloaded.
    #[error("api error (status {status}): {message}")]
    Api {
        /// The HTTP status code.
        status: u16,
        /// The error message from the response body.
        message: String,
    },
    /// The request exceeded the model's context window. Terminal.
    #[error("context window exceeded")]
    ContextOverflow,
    /// The account's quota/usage limit is exhausted. Terminal.
    #[error("quota exceeded")]
    Quota,
    /// Authentication failed (e.g. 401 after a refresh attempt). Terminal here;
    /// refresh-and-retry-once is the wire's job before surfacing this.
    #[error("authentication error: {0}")]
    Auth(String),
    /// The response could not be parsed into a [`Completion`]. Terminal.
    #[error("failed to decode provider response: {0}")]
    Decode(String),
    /// The request configuration is invalid for this wire (e.g. an unknown
    /// reasoning-effort tier on a fixed-mapping wire). Terminal, pre-send.
    #[error("invalid configuration: {0}")]
    Config(String),
}

impl ProviderError {
    /// Whether the loop/transport should retry this error (vs treat it as terminal).
    ///
    /// `Api` is retryable for any 5xx plus 408/409 — Claude Code's retryable
    /// set (Task-12 plan §4.6); everything 4xx-terminal stays terminal.
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            ProviderError::Transport(_) | ProviderError::RateLimited { .. } => true,
            ProviderError::Api { status, .. } => {
                matches!(status, 408 | 409 | 500..=599)
            }
            ProviderError::ContextOverflow
            | ProviderError::Quota
            | ProviderError::Auth(_)
            | ProviderError::Decode(_)
            | ProviderError::Config(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::StopReason;
    use crate::request::{CacheHint, SamplingArgs};
    use locode_protocol::{ContentBlock, Usage};

    /// A minimal provider that implements only `complete`, exercising the default
    /// `stream` fallback (non-streaming wires emit one synthetic text delta).
    struct TextOnly(String);

    #[async_trait]
    impl Provider for TextOnly {
        #[allow(clippy::unnecessary_literal_bound)]
        fn api_schema(&self) -> &str {
            "text-only"
        }
        async fn complete(
            &self,
            _request: &ConversationRequest,
        ) -> Result<Completion, ProviderError> {
            Ok(Completion {
                content: vec![ContentBlock::Text {
                    text: self.0.clone(),
                }],
                usage: Usage::default(),
                stop: StopReason::EndTurn,
            })
        }
    }

    fn req() -> ConversationRequest {
        ConversationRequest {
            messages: vec![],
            tools: vec![],
            sampling_args: SamplingArgs::default(),
            cache_hint: CacheHint::default(),
        }
    }

    #[tokio::test]
    async fn default_stream_emits_one_text_delta_and_same_completion() {
        let provider = TextOnly("hello there".into());
        let mut deltas = Vec::new();
        let completion = provider
            .stream(&req(), &mut |d| deltas.push(d))
            .await
            .expect("default stream ok");
        assert_eq!(deltas, vec![CompletionDelta::Text("hello there".into())]);
        assert_eq!(completion.text().as_deref(), Some("hello there"));
    }

    #[tokio::test]
    async fn default_stream_with_empty_text_emits_nothing() {
        let provider = TextOnly(String::new());
        let mut deltas = Vec::new();
        provider
            .stream(&req(), &mut |d| deltas.push(d))
            .await
            .expect("ok");
        assert!(
            deltas.is_empty(),
            "no text → no synthetic delta: {deltas:?}"
        );
    }
}
