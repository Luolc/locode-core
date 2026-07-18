//! The Anthropic Messages wire — the one live `Provider` (Task 12, ADR-0007).
//!
//! Converts the provider-neutral [`ConversationRequest`](crate::ConversationRequest)
//! into a Messages request, sends it (non-streaming), and parses the response back
//! into a [`Completion`](crate::Completion) — preserving tool-use ids verbatim,
//! thinking blocks *with* signatures, and usage. Owns the transport-tier retry.
//!
//! Design: `tasks/plans/task-12-anthropic-wire.md` (+ §9 addendum) and the
//! ADR-0007 amendment (2026-07-18). Wire structs are ported from Grok Build's
//! `messages.rs`; the conversion logic lives in [`build`] and `parse`.

pub mod build;
pub mod config;
pub mod error;
pub mod parse;
pub mod retry;
pub mod wire;

pub use build::{build_request, count_cache_controls, normalize_input_schema};
pub use config::{ApiBackend, AuthScheme, DeveloperRendering, ModelConfig, ReasoningEncoding};
pub use error::{HttpFailure, classify, parse_retry_after};
pub use parse::response_to_completion;
pub use retry::{RetryPolicy, backoff, run_with_retry};
