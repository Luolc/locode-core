//! `Edit` — a faithful port of Claude Code's `FileEditTool`: exact string
//! replacement guarded by the read-before-edit + modified-since-read gate (CC's
//! signature guardrail, the deliberate behavioral divergence from the grok pack,
//! which faithfully has none).
//!
//! Fidelity notes (Claude Code submodule commit `6a25909`):
//! - **Schema** (`FileEditTool/types.ts:6-18`, `z.strictObject`): `file_path` +
//!   `old_string` + `new_string` + `replace_all` (bool, default false).
//!   `deny_unknown_fields`; `replace_all` type-strict (CC's `semanticBoolean`
//!   coerces — repo policy declines that).
//! - **Description** (`prompt.ts:8-28`, `getEditToolDescription`) for our config
//!   (compact "line number + tab" prefix; non-ant, no minimal-uniqueness hint).
//!   Verbatim in `descriptions/edit.md` (pinned).
//! - **Check order** (`validateInput`, `FileEditTool.ts:137-345`): `old==new`
//!   (errorCode 1) → [permission deny = our `PathPolicy`/jail, ADR-0008] →
//!   existence: nonexistent + empty `old_string` → **create**; nonexistent +
//!   non-empty → errorCode 4; existing + empty `old_string` + non-empty file →
//!   errorCode 3, + empty file → replace → `.ipynb` errorCode 5 → **read gate**
//!   (errorCode 6) → **staleness** (errorCode 7) → not-found (errorCode 8) →
//!   multi-match without `replace_all` (errorCode 9). All messages verbatim.
//! - **Edit creates files** (`:216-220`, `call()` mkdir note `:427`): empty
//!   `old_string` on a nonexistent path creates it — **correcting plan §4.3**
//!   ("no file creation via Edit"). Our host deliberately does not mkdir parents
//!   (ADR-0008 footgun avoidance; grok is consistent), so creation needs an
//!   existing parent dir — a documented gap (the mkdir seam is a batched
//!   question, shared with Write/S4).
//! - **CRLF** (`:217`, `writeTextContent`): CC normalizes `\r\n`→`\n` for matching
//!   and re-expands on write. Ported (grok `search_replace` precedent).
//! - **Replacement** (`applyEditToFile`, `utils.ts:206-216`): `replace_all` →
//!   all; else the first (validated-unique) occurrence.
//! - **Success text** (`mapToolResultToToolResultBlockParam`, `:575-593`); the
//!   `userModified` note is interactive-only (headless → empty).
//! - **Gaps (D8):** `findActualString`'s quote normalization (`utils.ts:73`) —
//!   exact match only; the 1 GiB size cap (`MAX_EDIT_FILE_SIZE`) not enforced
//!   (host reads fully; un-hit); the `.ipynb` message references `NotebookEdit`
//!   (not in our pool).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::state::ClaudeSessionState;

/// Claude Code's `Edit` tool.
pub(crate) struct ClaudeEdit {
    host: Arc<Host>,
    state: Arc<ClaudeSessionState>,
}

impl ClaudeEdit {
    pub(crate) fn new(host: Arc<Host>, state: Arc<ClaudeSessionState>) -> Self {
        Self { host, state }
    }
}

/// Arguments for `Edit` (`FileEditTool/types.ts:6-18`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditArgs {
    #[schemars(description = "The absolute path to the file to modify")]
    file_path: String,
    #[schemars(description = "The text to replace")]
    old_string: String,
    #[schemars(description = "The text to replace it with (must be different from old_string)")]
    new_string: String,
    #[schemars(description = "Replace all occurrences of old_string (default false)")]
    #[serde(default)]
    replace_all: bool,
}

/// The structured (report) face; the success text is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct EditOutput {
    /// The edited/created path (as the model supplied it).
    path: String,
    /// Whether the file was newly created (empty `old_string`, absent file).
    created: bool,
    /// Number of occurrences replaced.
    replacements: usize,
    /// The model-facing success text (prompt face only).
    #[serde(skip)]
    message: String,
}

impl ToolOutput for EditOutput {
    fn to_prompt_text(&self) -> String {
        self.message.clone()
    }
}

/// Whether a host read failure is a plain not-found (vs a dir/permission/jail error).
fn is_not_found(err: &FsError) -> bool {
    matches!(err, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
}

#[async_trait]
impl Tool for ClaudeEdit {
    type Args = EditArgs;
    type Output = EditOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        include_str!("descriptions/edit.md")
    }

    async fn run(&self, ctx: &ToolCtx, args: EditArgs) -> Result<Self::Output, ToolError> {
        // errorCode 1 — no-op (checked before touching the filesystem).
        if args.old_string == args.new_string {
            return Err(ToolError::Respond(
                "No changes to make: old_string and new_string are exactly the same.".to_string(),
            ));
        }

        let path = Path::new(&args.file_path);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        // Read the file (existence + content + mtime in one call).
        let read = match self.host.read_file(&ctx.cwd, path).await {
            Ok(read) => Some(read),
            Err(e) if is_not_found(&e) => None,
            Err(e) => return Err(ToolError::Respond(e.to_string())),
        };

        let Some(read) = read else {
            // Nonexistent file.
            if args.old_string.is_empty() {
                // errorCode-valid — Edit creates the file (CC `:216-220`).
                return self
                    .write_result(ctx, &resolved, &args, &args.new_string, true, 1)
                    .await;
            }
            // errorCode 4.
            return Err(ToolError::Respond(format!(
                "File does not exist. Note: your current working directory is {}.",
                ctx.cwd.display()
            )));
        };

        // CC normalizes `\r\n`→`\n` for matching and re-expands on write.
        let has_crlf = read.contents.contains("\r\n");
        let content = read.contents.replace("\r\n", "\n");
        let mtime = read.stat.modified;

        // Existing file + empty old_string (CC `:224-243`).
        if args.old_string.is_empty() {
            if !content.trim().is_empty() {
                // errorCode 3.
                return Err(ToolError::Respond(
                    "Cannot create new file - file already exists.".to_string(),
                ));
            }
            // Empty file: replace empty content with new_string.
            let write_back = reexpand(&args.new_string, has_crlf);
            return self
                .write_result(ctx, &resolved, &args, &write_back, false, 1)
                .await;
        }

        // errorCode 5 — Jupyter notebooks route to NotebookEdit (not in our pool).
        // Case-sensitive, matching CC's `.endsWith('.ipynb')` (`FileEditTool.ts:266`).
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        if args.file_path.ends_with(".ipynb") {
            return Err(ToolError::Respond(
                "File is a Jupyter Notebook. Use the NotebookEdit to edit this file.".to_string(),
            ));
        }

        // Read-before-edit + staleness gate (errorCode 6/7).
        match self.state.check_fresh(&resolved, mtime) {
            None => {
                return Err(ToolError::Respond(
                    "File has not been read yet. Read it first before writing to it.".to_string(),
                ));
            }
            Some(false) => {
                return Err(ToolError::Respond(
                    "File has been modified since read, either by the user or by a linter. Read it \
                     again before attempting to write it."
                        .to_string(),
                ));
            }
            Some(true) => {}
        }

        // errorCode 8 — not found (exact match; quote-normalization is a gap).
        let matches = content.matches(&args.old_string).count();
        if matches == 0 {
            return Err(ToolError::Respond(format!(
                "String to replace not found in file.\nString: {}",
                args.old_string
            )));
        }
        // errorCode 9 — ambiguous without replace_all.
        if matches > 1 && !args.replace_all {
            return Err(ToolError::Respond(format!(
                "Found {matches} matches of the string to replace, but replace_all is false. To \
                 replace all occurrences, set replace_all to true. To replace only one occurrence, \
                 please provide more context to uniquely identify the instance.\nString: {}",
                args.old_string
            )));
        }

        let updated = if args.replace_all {
            content.replace(&args.old_string, &args.new_string)
        } else {
            content.replacen(&args.old_string, &args.new_string, 1)
        };
        let write_back = reexpand(&updated, has_crlf);
        self.write_result(ctx, &resolved, &args, &write_back, false, matches)
            .await
    }
}

/// Re-expand `\n`→`\r\n` when the source file used CRLF (CC's endings preservation).
fn reexpand(text: &str, has_crlf: bool) -> String {
    if has_crlf {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

impl ClaudeEdit {
    /// Write `contents`, record the post-write mtime (offset=None → Edit-origin),
    /// and render CC's success text.
    async fn write_result(
        &self,
        ctx: &ToolCtx,
        resolved: &Path,
        args: &EditArgs,
        contents: &str,
        created: bool,
        replacements: usize,
    ) -> Result<EditOutput, ToolError> {
        let path = Path::new(&args.file_path);
        let stat = self
            .host
            .write_file(&ctx.cwd, path, contents)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        // CC: `readFileState.set` with offset=undefined so sequential edits stay
        // fresh and Read's dedup never matches (`FileEditTool.ts:520`).
        self.state
            .record_write(resolved.to_path_buf(), stat.modified);
        // Success text (`:583-593`); the `userModified` note is interactive-only.
        let message = if args.replace_all {
            format!(
                "The file {} has been updated. All occurrences were successfully replaced.",
                args.file_path
            )
        } else {
            format!("The file {} has been updated successfully.", args.file_path)
        };
        Ok(EditOutput {
            path: args.file_path.clone(),
            created,
            replacements,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_is_the_pinned_edit_md() {
        let desc = include_str!("descriptions/edit.md");
        // sha256 50a851ce06c2e219aa83c30ead8f0a70693dcc9bfec417d0355aa7fe948a3833.
        assert_eq!(desc.len(), 1095, "edit.md byte length changed");
        assert!(desc.starts_with("Performs exact string replacements in files."));
        assert!(desc.contains("You must use your `Read` tool at least once"));
    }

    #[test]
    fn reexpand_only_on_crlf() {
        assert_eq!(reexpand("a\nb", false), "a\nb");
        assert_eq!(reexpand("a\nb", true), "a\r\nb");
    }
}
