//! A scripted [`Provider`] — the zero-API-spend test seam for the loop (ADR-0007).

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::completion::{Completion, CompletionDelta};
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

    /// Streams the next scripted completion as **word-ish text deltas** (the chunks
    /// concatenate back to the completion's text exactly), then returns the same
    /// whole `Completion` — a realistic multi-delta stream for engine/TUI tests
    /// without a network.
    async fn stream(
        &self,
        request: &ConversationRequest,
        on_delta: &mut (dyn FnMut(CompletionDelta) + Send),
    ) -> Result<Completion, ProviderError> {
        let completion = self.complete(request).await?;
        if let Some(text) = completion.text() {
            for chunk in chunk_text(&text) {
                on_delta(CompletionDelta::Text(chunk));
            }
        }
        Ok(completion)
    }
}

/// Split `text` into word-ish chunks (each a run of non-space chars plus its
/// trailing spaces) that concatenate back to `text` byte-for-byte.
fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            cur.push(ch);
            in_word = false;
        } else {
            if !in_word && !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            cur.push(ch);
            in_word = true;
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{CacheHint, SamplingArgs};
    use locode_protocol::{ContentBlock, Usage};

    use crate::completion::StopReason;

    fn req() -> ConversationRequest {
        ConversationRequest {
            messages: vec![],
            tools: vec![],
            sampling_args: SamplingArgs::default(),
            cache_hint: CacheHint::default(),
        }
    }

    fn text(t: &str) -> Completion {
        Completion {
            content: vec![ContentBlock::Text { text: t.into() }],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        }
    }

    #[test]
    fn chunk_text_concatenates_back_to_input_exactly() {
        for input in [
            "",
            "hello",
            "hello world",
            "  leading and  double  spaces ",
            "line one\nline two\n",
            "tabs\tand\nnewlines here",
            "unicode → café ☕ done",
        ] {
            let joined: String = chunk_text(input).concat();
            assert_eq!(joined, input, "chunk_text must reconstruct {input:?}");
        }
    }

    #[test]
    fn chunk_text_splits_on_word_boundaries() {
        assert_eq!(chunk_text("all done"), vec!["all ", "done"]);
        assert_eq!(chunk_text("one two three"), vec!["one ", "two ", "three"]);
        assert_eq!(chunk_text("solo"), vec!["solo"]);
        assert!(chunk_text("").is_empty());
    }

    #[tokio::test]
    async fn mock_stream_chunks_text_and_returns_the_whole_completion() {
        let mock = MockProvider::new(vec![text("hello there world")]);
        let mut deltas = Vec::new();
        let completion = mock
            .stream(&req(), &mut |d| deltas.push(d))
            .await
            .expect("mock stream ok");
        // Multiple text deltas that concatenate back to the full text.
        assert!(deltas.len() > 1, "expected multiple chunks: {deltas:?}");
        let joined: String = deltas
            .iter()
            .map(|d| match d {
                CompletionDelta::Text(t) => t.as_str(),
                other => panic!("mock should only emit Text: {other:?}"),
            })
            .collect();
        assert_eq!(joined, "hello there world");
        // The returned Completion is the whole scripted turn, unchanged.
        assert_eq!(completion.text().as_deref(), Some("hello there world"));
        assert_eq!(completion.stop, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn mock_stream_with_no_text_emits_no_deltas() {
        // A tool-only turn has no text → no deltas, same Completion back.
        let tool = Completion {
            content: vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "echo".into(),
                input: serde_json::json!({}),
            }],
            usage: Usage::default(),
            stop: StopReason::ToolUse,
        };
        let mock = MockProvider::new(vec![tool]);
        let mut deltas = Vec::new();
        let completion = mock
            .stream(&req(), &mut |d| deltas.push(d))
            .await
            .expect("ok");
        assert!(
            deltas.is_empty(),
            "tool-only turn streams no text: {deltas:?}"
        );
        assert!(completion.has_tool_calls());
    }
}
