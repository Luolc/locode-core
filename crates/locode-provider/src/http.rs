//! The shared HTTP transport layer (hoisted from `anthropic/` at Task 18):
//! retry policy + backoff, the [`HttpFailure`] carrier, `Retry-After` parsing,
//! client construction, and schema normalization. Per-wire error *classification*
//! stays wire-local (body shapes and terminal heuristics genuinely differ).

use std::future::Future;
use std::time::Duration;

use crate::provider::ProviderError;

/// A classified HTTP failure plus the retry-control signals that ride beside
/// the error taxonomy (`x-should-retry` override — Anthropic-only, other wires
/// always pass `false` — and `Retry-After`).
#[derive(Debug)]
pub struct HttpFailure {
    /// The classified error.
    pub error: ProviderError,
    /// A wire-specific "do not retry" override (Anthropic's
    /// `x-should-retry: false`); forces terminal regardless of the taxonomy.
    pub force_terminal: bool,
    /// Parsed `Retry-After` delay (integer seconds only; HTTP-dates → `None`).
    pub retry_after: Option<Duration>,
}

impl HttpFailure {
    /// A transport-layer failure (no HTTP status).
    pub fn transport(message: impl Into<String>) -> Self {
        Self {
            error: ProviderError::Transport(message.into()),
            force_terminal: false,
            retry_after: None,
        }
    }

    /// A terminal decode failure.
    pub fn decode(message: impl Into<String>) -> Self {
        Self {
            error: ProviderError::Decode(message.into()),
            force_terminal: false,
            retry_after: None,
        }
    }
}

/// Parse a `Retry-After` header value: integer delta-seconds only. HTTP-date
/// forms → `None` (fall back to exp backoff) — grok's simplification.
#[must_use]
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Transport-retry knobs (Task-12 plan §3.6; sources: Claude Code 500ms·2ⁿ cap
/// 32s, grok's 429-cap-2 and 120s `Retry-After` cap, codex's ±10% jitter).
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts for retryable failures.
    pub max_attempts: u32,
    /// First backoff delay; doubles each attempt.
    pub base_delay: Duration,
    /// Exponential-backoff ceiling (`Retry-After` bypasses it).
    pub max_delay: Duration,
    /// 429-specific attempt cap — surface the rate limit, never hammer.
    pub rate_limit_attempts: u32,
    /// Ceiling on a server-sent `Retry-After`.
    pub retry_after_cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(32),
            rate_limit_attempts: 2,
            retry_after_cap: Duration::from_mins(2),
        }
    }
}

/// Compute the pre-retry sleep for `attempt` (1-based). `Retry-After` takes
/// precedence and bypasses the exponential cap (itself capped); otherwise
/// exponential with ±10% jitter.
#[must_use]
pub fn backoff(policy: &RetryPolicy, attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(after) = retry_after {
        return after.min(policy.retry_after_cap);
    }
    let exp = policy
        .base_delay
        .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)))
        .min(policy.max_delay);
    let jitter: f64 = rand::Rng::random_range(&mut rand::rng(), 0.9..1.1);
    exp.mul_f64(jitter)
}

/// Drive `op` until success, a terminal classification, or attempt exhaustion.
/// Generic over the success type — the loop only inspects [`HttpFailure`].
///
/// # Errors
/// The last failure's [`ProviderError`] when attempts are exhausted or the
/// failure is terminal.
pub async fn run_with_retry<T, F, Fut>(policy: &RetryPolicy, mut op: F) -> Result<T, ProviderError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, HttpFailure>>,
{
    let mut rate_limit_hits: u32 = 0;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let failure = match op(attempt).await {
            Ok(value) => return Ok(value),
            Err(failure) => failure,
        };

        if failure.force_terminal || !failure.error.retryable() {
            return Err(failure.error);
        }
        if matches!(failure.error, ProviderError::RateLimited { .. }) {
            rate_limit_hits += 1;
            if rate_limit_hits > policy.rate_limit_attempts {
                return Err(failure.error);
            }
        }
        if attempt >= policy.max_attempts {
            return Err(failure.error);
        }
        tokio::time::sleep(backoff(policy, attempt, failure.retry_after)).await;
    }
}

/// Build the reqwest client with the wires' shared timeouts (30s connect,
/// 10min total — non-streaming reasoning turns can run minutes).
///
/// # Errors
/// [`ProviderError::Transport`] when the client cannot be constructed.
pub fn build_http_client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_mins(10))
        .build()
        .map_err(|e| ProviderError::Transport(format!("client construction: {e}")))
}

/// Normalize a schemars-derived schema into the `input_schema`/`parameters`
/// the APIs accept: our flat `Args` structs emit no `$defs`/`$ref` (generation
/// inlines subschemas at the source), so normalization reduces to stripping
/// the top-level `$schema` meta-annotation. Shared by every wire.
#[must_use]
pub fn normalize_input_schema(mut schema: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$schema");
    }
    schema
}
