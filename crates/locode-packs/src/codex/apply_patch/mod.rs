//! `apply_patch` — codex's freeform edit tool (submodule `f201c30c`). Codex has
//! no read/write/edit tools: the shell is the read path and *all* editing flows
//! through this one tool. The model emits a raw V4A patch (constrained by a Lark
//! grammar, not JSON params); our wire delivers it as a `Value::String`
//! (`openai/responses/parse.rs`), so `type Args = String`.
//!
//! Fidelity notes:
//! - **Registration** (`core/src/tools/handlers/apply_patch_spec.rs:9-27`): name
//!   `apply_patch`; `ToolSpec::Freeform { syntax: lark, definition: apply_patch.lark }`.
//! - **Description** (`apply_patch_spec.rs:20`, verbatim in `descriptions/apply_patch.md`).
//! - **Parser / matcher / apply** ported line-for-line — see `parser`, `seek`, `apply`.
//! - **Summary** (`lib.rs:870-885`): `Success. Updated the following files:` + `A/M/D`.
//! - **Gaps (D8/S2):** single-environment grammar only (no `*** Environment ID:`);
//!   the TUI unified-diff preview + `AppliedPatchDelta` turn-diff tracking are
//!   loop-adjacent (ADR-0023) and dropped; partial-success is left on disk (faithful).

mod apply;
mod parser;
mod seek;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host};
use locode_protocol::{GrammarSyntax, ToolInputFormat};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use serde::Serialize;

use parser::{Hunk, ParseError};

/// The pinned Lark grammar (base, single-environment) offered to the model.
pub(crate) const APPLY_PATCH_LARK: &str = include_str!("../apply_patch.lark");
const DESCRIPTION: &str = include_str!("../descriptions/apply_patch.md");

/// Codex's `apply_patch` tool.
pub(crate) struct CodexApplyPatch {
    host: Arc<Host>,
}

impl CodexApplyPatch {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// The model-facing result: the git-style summary of what changed.
#[derive(Debug, Serialize)]
pub(crate) struct ApplyPatchOutput {
    /// The rendered `Success. Updated the following files:` summary.
    summary: String,
}

impl ToolOutput for ApplyPatchOutput {
    fn to_prompt_text(&self) -> String {
        self.summary.clone()
    }
}

#[async_trait]
impl Tool for CodexApplyPatch {
    type Args = String;
    type Output = ApplyPatchOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn input_format(&self) -> ToolInputFormat {
        // A freeform (custom-grammar) tool: the model emits raw patch text, not JSON.
        ToolInputFormat::Freeform {
            syntax: GrammarSyntax::Lark,
            definition: APPLY_PATCH_LARK.to_string(),
        }
    }

    async fn run(&self, ctx: &ToolCtx, patch: String) -> Result<ApplyPatchOutput, ToolError> {
        let hunks =
            parser::parse(&patch).map_err(|e| ToolError::Respond(format_parse_error(&e)))?;
        if hunks.is_empty() {
            return Err(ToolError::Respond("No files were modified.".to_string()));
        }
        let summary = self.apply_hunks(ctx, &hunks).await?;
        Ok(ApplyPatchOutput { summary })
    }
}

impl CodexApplyPatch {
    /// Apply every hunk against the (jailed) filesystem, then render the summary.
    /// Hunks apply sequentially; a later failure does not roll back earlier writes
    /// (faithful to codex).
    async fn apply_hunks(&self, ctx: &ToolCtx, hunks: &[Hunk]) -> Result<String, ToolError> {
        let cwd = &ctx.cwd;
        let (mut added, mut modified, mut deleted): (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) =
            (Vec::new(), Vec::new(), Vec::new());

        for hunk in hunks {
            match hunk {
                Hunk::AddFile { path, contents } => {
                    self.write_with_parent_retry(cwd, path, contents).await?;
                    added.push(path.clone());
                }
                Hunk::DeleteFile { path } => {
                    self.host.remove_file(cwd, path).await.map_err(|e| {
                        ToolError::Respond(fs_err(&e, "Failed to delete file", path))
                    })?;
                    deleted.push(path.clone());
                }
                Hunk::UpdateFile {
                    path,
                    move_path,
                    chunks,
                } => {
                    let original = self.host.read_file(cwd, path).await.map_err(|e| {
                        ToolError::Respond(fs_err(&e, "Failed to read file to update", path))
                    })?;
                    let new_contents = apply::derive_new_contents(
                        &original.contents,
                        &path.to_string_lossy(),
                        chunks,
                    )
                    .map_err(ToolError::Respond)?;
                    if let Some(dest) = move_path {
                        self.write_with_parent_retry(cwd, dest, &new_contents)
                            .await?;
                        self.host.remove_file(cwd, path).await.map_err(|e| {
                            ToolError::Respond(fs_err(&e, "Failed to remove original", path))
                        })?;
                    } else {
                        self.host
                            .write_file(cwd, path, &new_contents)
                            .await
                            .map_err(|e| {
                                ToolError::Respond(fs_err(&e, "Failed to write file", path))
                            })?;
                    }
                    // A moved file is summarized under M by its original path.
                    modified.push(path.clone());
                }
            }
        }
        Ok(render_summary(&added, &modified, &deleted))
    }

    /// Write a file, creating missing parent dirs on `NotFound` then retrying
    /// (codex's `write_file_with_missing_parent_retry`, `lib.rs:629-665`).
    async fn write_with_parent_retry(
        &self,
        cwd: &Path,
        path: &Path,
        contents: &str,
    ) -> Result<(), ToolError> {
        match self.host.write_file(cwd, path, contents).await {
            Ok(_) => Ok(()),
            Err(e) if is_not_found(&e) => {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    self.host
                        .create_dir(cwd, parent, true, true)
                        .await
                        .map_err(|e| {
                            ToolError::Respond(fs_err(
                                &e,
                                "Failed to create parent directories for",
                                path,
                            ))
                        })?;
                }
                self.host
                    .write_file(cwd, path, contents)
                    .await
                    .map(|_| ())
                    .map_err(|e| ToolError::Respond(fs_err(&e, "Failed to write file", path)))
            }
            Err(e) => Err(ToolError::Respond(fs_err(&e, "Failed to write file", path))),
        }
    }
}

fn is_not_found(err: &FsError) -> bool {
    matches!(err, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
}

fn fs_err(err: &FsError, context: &str, path: &Path) -> String {
    // A jail rejection keeps its own message; an IO failure gets codex's context prefix.
    match err {
        FsError::Path(_) => err.to_string(),
        FsError::Io { source, .. } => format!("{context} {}: {source}", path.display()),
    }
}

fn format_parse_error(err: &ParseError) -> String {
    match err {
        ParseError::InvalidPatchError(message) => format!("Invalid patch: {message}"),
        ParseError::InvalidHunkError {
            message,
            line_number,
        } => format!("Invalid patch hunk on line {line_number}: {message}"),
    }
}

/// `Success. Updated the following files:` + `A`/`M`/`D` lines (`print_summary`).
fn render_summary(added: &[PathBuf], modified: &[PathBuf], deleted: &[PathBuf]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("Success. Updated the following files:\n");
    for (prefix, paths) in [("A", added), ("M", modified), ("D", deleted)] {
        for path in paths {
            let _ = writeln!(out, "{prefix} {}", path.display());
        }
    }
    out
}

#[cfg(test)]
mod tests;
