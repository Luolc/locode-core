//! `Write` — a faithful port of Claude Code's `FileWriteTool`: create-or-overwrite
//! a file, gated (for existing files) by the same read-before-write + staleness
//! store that `Read`/`Edit` share.
//!
//! Fidelity notes (Claude Code submodule commit `6a25909`):
//! - **Schema** (`FileWriteTool.ts:56-64`, `z.strictObject`): `file_path` +
//!   `content`. `deny_unknown_fields`.
//! - **Description** (`prompt.ts:10-18`, `getWriteToolDescription`) verbatim in
//!   `descriptions/write.md` (pinned).
//! - **Gate** (`validateInput`, `FileWriteTool.ts:153-222`): a **new** file writes
//!   freely; an **existing** file must have been `Read` first (errorCode 2 — "File
//!   has not been read yet…") and not modified since (errorCode 3 — "File has been
//!   modified since read…") — the same messages + store as `Edit`.
//! - **Write is verbatim** (`:302`, `writeTextContent(..., 'LF')`): the model's
//!   `content` is written as-is (no line-ending preservation — contrast `Edit`).
//! - **Success text** (`mapToolResultToToolResultBlockParam`, `:418-430`): new →
//!   "File created successfully at: {path}"; existing → "The file {path} has been
//!   updated successfully."
//! - **Records** the post-write mtime (offset=None → Write-origin).
//! - **Gaps (D8):** the host does not mkdir parents (ADR-0008 footgun avoidance;
//!   grok is consistent) — creation needs an existing parent dir (shared with
//!   Edit/S3; batched mkdir-seam question).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::state::ClaudeSessionState;

/// Claude Code's `Write` tool.
pub(crate) struct ClaudeWrite {
    host: Arc<Host>,
    state: Arc<ClaudeSessionState>,
}

impl ClaudeWrite {
    pub(crate) fn new(host: Arc<Host>, state: Arc<ClaudeSessionState>) -> Self {
        Self { host, state }
    }
}

/// Arguments for `Write` (`FileWriteTool.ts:56-64`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteArgs {
    #[schemars(
        description = "The absolute path to the file to write (must be absolute, not relative)"
    )]
    file_path: String,
    #[schemars(description = "The content to write to the file")]
    content: String,
}

/// The structured (report) face; the success text is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct WriteOutput {
    /// The written path (as the model supplied it).
    path: String,
    /// Whether the file was newly created (vs overwritten).
    created: bool,
    /// The model-facing success text (prompt face only).
    #[serde(skip)]
    message: String,
}

impl ToolOutput for WriteOutput {
    fn to_prompt_text(&self) -> String {
        self.message.clone()
    }
}

/// Whether a host failure is a plain not-found (vs a dir/permission/jail error).
fn is_not_found(err: &FsError) -> bool {
    matches!(err, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
}

#[async_trait]
impl Tool for ClaudeWrite {
    type Args = WriteArgs;
    type Output = WriteOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Write
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        include_str!("descriptions/write.md")
    }

    async fn run(&self, ctx: &ToolCtx, args: WriteArgs) -> Result<Self::Output, ToolError> {
        let path = Path::new(&args.file_path);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        // Existence + current mtime (no need to read the content).
        let existing = match self.host.stat(&ctx.cwd, path).await {
            Ok(stat) => Some(stat),
            Err(e) if is_not_found(&e) => None,
            Err(e) => return Err(ToolError::Respond(e.to_string())),
        };
        let created = existing.is_none();

        // Existing files must be Read first and unmodified since (errorCode 2/3).
        if let Some(stat) = &existing {
            match self.state.check_fresh(&resolved, stat.modified) {
                None => {
                    return Err(ToolError::Respond(
                        "File has not been read yet. Read it first before writing to it."
                            .to_string(),
                    ));
                }
                Some(false) => {
                    return Err(ToolError::Respond(
                        "File has been modified since read, either by the user or by a linter. \
                         Read it again before attempting to write it."
                            .to_string(),
                    ));
                }
                Some(true) => {}
            }
        }

        // Write the model's content verbatim (CC writes LF as sent).
        let stat = self
            .host
            .write_file(&ctx.cwd, path, &args.content)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        self.state.record_write(resolved, stat.modified);

        let message = if created {
            format!("File created successfully at: {}", args.file_path)
        } else {
            format!("The file {} has been updated successfully.", args.file_path)
        };
        Ok(WriteOutput {
            path: args.file_path.clone(),
            created,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn description_is_the_pinned_write_md() {
        let desc = include_str!("descriptions/write.md");
        // sha256 70103362310e68e4354843200e7d54db8cd08aefd058617ac9ddd98fc5b8eae0.
        assert_eq!(desc.len(), 620, "write.md byte length changed");
        assert!(desc.starts_with("Writes a file to the local filesystem."));
        assert!(desc.contains("you MUST use the Read tool first"));
    }
}
