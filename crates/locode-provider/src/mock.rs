//! A scripted [`Provider`] — the zero-API-spend test seam for the loop (ADR-0007).

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::completion::Completion;
use crate::provider::{Provider, ProviderError};
use crate::request::ConversationRequest;

/// A provider that replays a fixed script of results, one per `complete()` call.
///
/// This is a real, shippable provider (the `--provider mock` mode runs it in CI
/// without an API key), not test-only. It ignores the request and returns the next
/// scripted [`Completion`] (or [`ProviderError`]) in order — letting a test drive the
/// loop through a tool-call turn, a final-text turn, or a scripted error, with zero
/// network. Calling `complete()` more times than scripted **panics** (a loud signal
/// that the loop ran an unexpected extra turn), per the agreed contract.
pub struct MockProvider {
    script: Mutex<VecDeque<Result<Completion, ProviderError>>>,
}

impl MockProvider {
    /// A mock scripted with a sequence of successful completions.
    #[must_use]
    pub fn new(script: Vec<Completion>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().map(Ok).collect()),
        }
    }

    /// A mock scripted with a sequence of results (to inject errors as well).
    #[must_use]
    pub fn with_results(script: Vec<Result<Completion, ProviderError>>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    // The trait ties `&str` to `&self` so real wires return a stored id; the mock's
    // is a literal.
    #[allow(clippy::unnecessary_literal_bound)]
    fn api_schema(&self) -> &str {
        "mock"
    }

    async fn complete(&self, _request: &ConversationRequest) -> Result<Completion, ProviderError> {
        // Recover from a poisoned lock rather than unwrap/expect (denied lints); the
        // mutex only guards a cursor and is never held across an await.
        let mut script = self.script.lock().unwrap_or_else(PoisonError::into_inner);
        match script.pop_front() {
            Some(result) => result,
            None => panic!(
                "MockProvider script exhausted: complete() was called more times than scripted"
            ),
        }
    }
}
