//! HTTP status + wire error body → [`ProviderError`] classification (plan §4.6).
//!
//! The table, sourced from grok `client.rs` and Claude Code's retryable set:
//!
//! | condition | error | tier-1 behavior |
//! |---|---|---|
//! | transport failure | `Transport` | backoff (classified at the client, not here) |
//! | 5xx | `Api{status}` | backoff |
//! | 529 or `overloaded_error` body | `Api{529}` | backoff (status can be masked) |
//! | 429 | `RateLimited{retry_after}` | honor `Retry-After`, cap 2, then surface |
//! | 408/409 | `Api{status}` | backoff |
//! | 401/403 | `Auth` | refresh once → retry, else terminal |
//! | 400 context / 413 | `ContextOverflow` | terminal (engine compacts) |
//! | 402 / quota wording | `Quota` | terminal |
//! | other 4xx | `Api{status}` | terminal |
//!
//! `x-should-retry: false` **forces terminal** regardless of the row; `true` is
//! ignored (grok `client.rs:214-227`). The flag rides on [`HttpFailure`] so the
//! retry loop can honor it without the taxonomy needing a variant.

use std::time::Duration;

use super::wire::ErrorBody;
pub use crate::http::{HttpFailure, parse_retry_after};
use crate::provider::ProviderError;

/// Classify an HTTP error response (plan §4.6 table).
#[must_use]
pub fn classify(
    status: u16,
    x_should_retry: Option<bool>,
    retry_after: Option<Duration>,
    body: &ErrorBody,
) -> HttpFailure {
    let error_type = body.error.r#type.as_str();
    let message = body.error.message.as_str();
    let lower = message.to_ascii_lowercase();

    let error = if status == 401 || status == 403 {
        ProviderError::Auth(format!("http {status}: {message}"))
    } else if status == 413
        || (status == 400
            && (lower.contains("prompt is too long")
                || lower.contains("context window")
                || error_type == "context_length_exceeded"))
    {
        ProviderError::ContextOverflow
    } else if status == 402
        || lower.contains("credit balance")
        || error_type.contains("insufficient_quota")
    {
        ProviderError::Quota
    } else if status == 429 {
        ProviderError::RateLimited { retry_after }
    } else if status == 529 || error_type == "overloaded_error" {
        // Claude Code matches status OR body — the SDK sometimes masks the 529.
        ProviderError::Api {
            status: 529,
            message: message.to_string(),
        }
    } else {
        ProviderError::Api {
            status,
            message: message.to_string(),
        }
    };

    HttpFailure {
        error,
        // `false` forces terminal; `true` is deliberately IGNORED (never forces
        // a retry) — grok's exact rule.
        force_terminal: x_should_retry == Some(false),
        retry_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(r#type: &str, message: &str) -> ErrorBody {
        serde_json::from_str(&format!(
            r#"{{"type":"error","error":{{"type":"{type}","message":"{message}"}}}}"#,
            r#type = r#type,
        ))
        .unwrap()
    }

    #[test]
    fn table_rows_classify() {
        // 5xx retryable.
        let f = classify(503, None, None, &body("api_error", "unavailable"));
        assert!(matches!(f.error, ProviderError::Api { status: 503, .. }));
        assert!(f.error.retryable());

        // 529 by status, and by masked body.
        let f = classify(529, None, None, &body("overloaded_error", "overloaded"));
        assert!(f.error.retryable());
        let masked = classify(500, None, None, &body("overloaded_error", "overloaded"));
        assert!(matches!(
            masked.error,
            ProviderError::Api { status: 529, .. }
        ));

        // 429 with Retry-After.
        let f = classify(
            429,
            None,
            Some(Duration::from_secs(7)),
            &body("rate_limit_error", "slow down"),
        );
        assert!(matches!(
            f.error,
            ProviderError::RateLimited {
                retry_after: Some(d)
            } if d == Duration::from_secs(7)
        ));

        // 408/409 retryable via the Api arm.
        assert!(classify(408, None, None, &body("t", "m")).error.retryable());
        assert!(classify(409, None, None, &body("t", "m")).error.retryable());

        // Auth.
        let f = classify(401, None, None, &body("authentication_error", "bad key"));
        assert!(matches!(f.error, ProviderError::Auth(_)));
        assert!(!f.error.retryable());

        // Context overflow: 413, and 400 + wording.
        assert!(matches!(
            classify(413, None, None, &body("t", "m")).error,
            ProviderError::ContextOverflow
        ));
        assert!(matches!(
            classify(
                400,
                None,
                None,
                &body("invalid_request_error", "prompt is too long: 250000 tokens")
            )
            .error,
            ProviderError::ContextOverflow
        ));

        // Quota: 402 and credit wording.
        assert!(matches!(
            classify(
                400,
                None,
                None,
                &body(
                    "invalid_request_error",
                    "Your credit balance is too low to access the API"
                )
            )
            .error,
            ProviderError::Quota
        ));

        // Plain 400 terminal.
        let f = classify(400, None, None, &body("invalid_request_error", "bad field"));
        assert!(matches!(f.error, ProviderError::Api { status: 400, .. }));
        assert!(!f.error.retryable());
    }

    #[test]
    fn x_should_retry_false_forces_terminal_true_is_ignored() {
        let forced = classify(503, Some(false), None, &body("api_error", "m"));
        assert!(forced.force_terminal, "false forces terminal");
        assert!(forced.error.retryable(), "taxonomy still says retryable");

        let ignored = classify(400, Some(true), None, &body("invalid_request_error", "m"));
        assert!(!ignored.force_terminal, "true never forces a retry");
        assert!(!ignored.error.retryable());
    }

    #[test]
    fn retry_after_parses_integer_seconds_only() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after(" 5 "), Some(Duration::from_secs(5)));
        assert_eq!(
            parse_retry_after("Fri, 18 Jul 2026 12:00:00 GMT"),
            None,
            "HTTP-date falls back to exp backoff"
        );
    }
}
