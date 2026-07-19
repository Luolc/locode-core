//! Transport-tier retry — hoisted to [`crate::http`] at Task 18; this module
//! re-exports the shared pieces so existing paths keep working, and keeps the
//! behavior tests (they pin the shared loop's contract).

pub use crate::http::{HttpFailure, RetryPolicy, backoff, run_with_retry};

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::completion::Completion;
    use crate::provider::ProviderError;

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
        let result: Result<Completion, ProviderError> = run_with_retry(&policy, |n| {
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
        let result: Result<Completion, ProviderError> = run_with_retry(&policy, |n| {
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
        let result: Result<Completion, ProviderError> = run_with_retry(&policy, |n| {
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
        let result: Result<Completion, ProviderError> = run_with_retry(&policy, |n| {
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
        let result: Result<Completion, ProviderError> = run_with_retry(&policy, |n| {
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
