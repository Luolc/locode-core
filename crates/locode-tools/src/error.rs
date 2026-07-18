//! The tool error taxonomy (ADR-0004).

use thiserror::Error;

/// How a tool call failed, split into exactly two recovery paths (ADR-0004).
///
/// **Default everything to [`ToolError::Respond`].** Bad arguments, an unknown
/// tool, a not-found path, a failed command, a timeout — all are *soft*: they
/// become a `tool_result { is_error: true }` the model reads and retries from.
/// [`ToolError::Fatal`] is reserved for the rare case where the transcript itself
/// is unrecoverable; it aborts the turn with a non-zero exit.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Soft error: surfaced to the model as an `is_error` tool result so it can
    /// self-correct. The loop keeps iterating.
    #[error("{0}")]
    Respond(String),
    /// Hard error: the transcript is unrecoverable; abort the turn.
    #[error("{0}")]
    Fatal(String),
}
