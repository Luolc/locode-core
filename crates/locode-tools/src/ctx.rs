//! The small per-call tool context (ADR-0003).

use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

/// The context handed to a tool for one call.
///
/// Deliberately small (ADR-0003 rejects a god-object context): just the working
/// directory, the id of the `tool_use` being answered, the workspace jail root,
/// and a cancellation handle. The loop builds one of these per tool call, setting
/// [`ToolCtx::call_id`] to the `tool_use` id so the produced `tool_result` pairs
/// back to it (ADR-0004).
#[derive(Debug, Clone)]
pub struct ToolCtx {
    /// The directory the tool should treat as "current" for this call.
    pub cwd: PathBuf,
    /// The id of the `tool_use` block this call answers (the pairing link).
    pub call_id: String,
    /// The path jail root; the host resolves every path under this (ADR-0008).
    pub workspace_root: PathBuf,
    /// Cooperative cancellation: a running tool should observe this and bail out
    /// cleanly (kill its subprocess, stop reading) when it fires.
    pub cancel: CancellationToken,
}

impl ToolCtx {
    /// Build a context for one tool call.
    #[must_use]
    pub fn new(
        cwd: PathBuf,
        call_id: String,
        workspace_root: PathBuf,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            cwd,
            call_id,
            workspace_root,
            cancel,
        }
    }
}
