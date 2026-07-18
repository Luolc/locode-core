//! Transport-tier retry: bounded backoff + jitter, `Retry-After`, 429 surfacing
//! (plan §4.6). The loop-level rebuild-and-resample tier lives in the engine.

use std::future::Future;
use std::time::Duration;

use super::error::HttpFailure;
use crate::completion::Completion;
use crate::provider::ProviderError;

/// Transport-retry knobs (plan §3.6; sources: Claude Code 500ms·2ⁿ cap 32s,
/// grok's 429-cap-2 and 120s `Retry-After` cap, codex's ±10% jitter).
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts for retryable failures (Claude Code 10, codex 5; ~8 ≈ a
    /// few minutes of transient-error tolerance).
    pub max_attempts: u32,
    /// First backoff delay; doubles each attempt.
    pub base_delay: Duration,
    /// Exponential-backoff ceiling (`Retry-After` bypasses it).
    pub max_delay: Duration,
    /// 429-specific attempt cap — surface the rate limit, never hammer.
    pub rate_limit_attempts: u32,
    /// Ceiling on a server-sent `Retry-After` (guards a misbehaving upstream).
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

/// Compute the pre-retry sleep for `attempt` (1-based).
///
/// `Retry-After` takes precedence and **bypasses the exponential cap** (Claude
/// Code's rule), itself capped at [`RetryPolicy::retry_after_cap`]. Otherwise
/// exponential `base·2^(attempt-1)` capped at [`RetryPolicy::max_delay`], with
/// ±10% jitter (codex `retry.rs:45`).
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
///
/// The rules (plan §4.6): `force_terminal` (`x-should-retry: false`) overrides
/// everything; non-retryable errors return immediately; 429s get their own
/// tighter cap ([`RetryPolicy::rate_limit_attempts`]) and are then **surfaced**;
/// other retryable failures back off up to [`RetryPolicy::max_attempts`].
///
/// # Errors
/// The last failure's [`ProviderError`] when attempts are exhausted or the
/// failure is terminal.
pub async fn run_with_retry<F, Fut>(
    policy: &RetryPolicy,
    mut op: F,
) -> Result<Completion, ProviderError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<Completion, HttpFailure>>,
{
    let mut rate_limit_hits: u32 = 0;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let failure = match op(attempt).await {
            Ok(completion) => return Ok(completion),
            Err(failure) => failure,
        };

        if failure.force_terminal || !failure.error.retryable() {
            return Err(failure.error);
        }
        if matches!(failure.error, ProviderError::RateLimited { .. }) {
            rate_limit_hits += 1;
            if rate_limit_hits > policy.rate_limit_attempts {
                // Surfaced, not silently hammered (grok caps 429 at 2).
                return Err(failure.error);
            }
        }
        if attempt >= policy.max_attempts {
            return Err(failure.error);
        }
        tokio::time::sleep(backoff(policy, attempt, failure.retry_after)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fast policy so retry tests sleep microseconds, not seconds.
    fn fast() -> RetryPolicy {
        RetryPolicy {
            base_delay: Duration::from_micros(10),
            max_delay: Duration::from_micros(160),
            retry_after_cap: Duration::from_micros(500),
            ..RetryPolicy::default()
        }
    }

    fn ok() -> Completion {
        Completion {
            content: vec![],
            usage: locode_protocol::Usage::default(),
            stop: crate::completion::StopReason::EndTurn,
        }
    }

    fn retryable_5xx() -> HttpFailure {
        HttpFailure {
            error: ProviderError::Api {
                status: 503,
                message: "unavailable".into(),
            },
            force_terminal: false,
            retry_after: None,
        }
    }

    #[tokio::test]
    async fn retries_5xx_until_success() {
        let policy = fast();
        let attempts = std::cell::Cell::new(0u32);
        let result = run_with_retry(&policy, |n| {
            attempts.set(n);
            async move {
                if n < 3 {
                    Err(retryable_5xx())
                } else {
                    Ok(ok())
                }
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(attempts.get(), 3, "two failures then success");
    }

    #[tokio::test]
    async fn terminal_returns_immediately() {
        let policy = fast();
        let attempts = std::cell::Cell::new(0u32);
        let result = run_with_retry(&policy, |n| {
            attempts.set(n);
            async move {
                Err(HttpFailure {
                    error: ProviderError::ContextOverflow,
                    force_terminal: false,
                    retry_after: None,
                })
            }
        })
        .await;
        assert!(matches!(result, Err(ProviderError::ContextOverflow)));
        assert_eq!(attempts.get(), 1, "no retry on terminal");
    }

    #[tokio::test]
    async fn x_should_retry_false_overrides_a_retryable_status() {
        let policy = fast();
        let attempts = std::cell::Cell::new(0u32);
        let result = run_with_retry(&policy, |n| {
            attempts.set(n);
            async move {
                Err(HttpFailure {
                    force_terminal: true,
                    ..retryable_5xx()
                })
            }
        })
        .await;
        assert!(matches!(
            result,
            Err(ProviderError::Api { status: 503, .. })
        ));
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test]
    async fn rate_limit_surfaced_after_its_own_cap() {
        let policy = fast();
        let attempts = std::cell::Cell::new(0u32);
        let result = run_with_retry(&policy, |n| {
            attempts.set(n);
            async move {
                Err(HttpFailure {
                    error: ProviderError::RateLimited {
                        retry_after: Some(Duration::from_micros(20)),
                    },
                    force_terminal: false,
                    retry_after: Some(Duration::from_micros(20)),
                })
            }
        })
        .await;
        assert!(matches!(result, Err(ProviderError::RateLimited { .. })));
        // rate_limit_attempts = 2 → two retried 429s, surfaced on the third.
        assert_eq!(attempts.get(), 3, "capped at 2 rate-limit retries");
    }

    #[tokio::test]
    async fn exhaustion_returns_the_last_error() {
        let policy = RetryPolicy {
            max_attempts: 3,
            ..fast()
        };
        let attempts = std::cell::Cell::new(0u32);
        let result = run_with_retry(&policy, |n| {
            attempts.set(n);
            async move { Err(retryable_5xx()) }
        })
        .await;
        assert!(matches!(
            result,
            Err(ProviderError::Api { status: 503, .. })
        ));
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn backoff_bounds_and_retry_after_precedence() {
        let policy = RetryPolicy::default();
        // Exponential with ±10% jitter, capped at max_delay.
        for attempt in 1..=10 {
            let d = backoff(&policy, attempt, None);
            let exp = policy
                .base_delay
                .saturating_mul(2u32.saturating_pow(attempt - 1))
                .min(policy.max_delay);
            assert!(
                d >= exp.mul_f64(0.9) && d <= exp.mul_f64(1.1),
                "attempt {attempt}: {d:?} outside [{:?}, {:?}]",
                exp.mul_f64(0.9),
                exp.mul_f64(1.1)
            );
        }
        // Retry-After bypasses the exp cap (64s > max_delay 32s)…
        assert_eq!(
            backoff(&policy, 1, Some(Duration::from_secs(64))),
            Duration::from_secs(64)
        );
        // …but is itself capped at retry_after_cap.
        assert_eq!(
            backoff(&policy, 1, Some(Duration::from_secs(500))),
            policy.retry_after_cap
        );
    }
}
