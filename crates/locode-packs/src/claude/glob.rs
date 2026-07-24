//! `Glob` — a faithful port of Claude Code's `GlobTool`: ripgrep-backed file
//! pattern matching, sorted by modification time, capped at 100.
//!
//! Fidelity notes (Claude Code submodule commit `6a25909`):
//! - **Schema** (`GlobTool.ts:26-36`, `z.strictObject`): `pattern` + optional
//!   `path` (verbatim descriptions, incl. the "IMPORTANT: Omit this field …
//!   DO NOT enter 'undefined'" sentence). `deny_unknown_fields`.
//! - **Description** (`prompt.ts:3-7`) verbatim in `descriptions/glob.md`.
//!   Mentions the `Agent` tool (not in our pool) — kept verbatim (D8 gap).
//! - **Mechanism** (`utils/glob.ts:66-155`): `rg --files --glob <pattern>
//!   --sort=modified --no-ignore --hidden` under the search dir. `--sort=modified`
//!   is **oldest first** (rg ascending) — **correcting plan §4.5's "mtime desc"**;
//!   `--no-ignore` (does NOT respect `.gitignore`) and `--hidden` are the CC
//!   defaults (`CLAUDE_CODE_GLOB_NO_IGNORE`/`CLAUDE_CODE_GLOB_HIDDEN` || 'true').
//!   Take the first 100 (`GlobTool.ts:157` `?? 100`); paths relativized under cwd.
//! - **Path validation** (`validateInput`, `GlobTool.ts:96-133`): nonexistent →
//!   errorCode 1 ("Directory does not exist: {p}. Note: your current working
//!   directory is {cwd}."); not a directory → errorCode 2 ("Path is not a
//!   directory: {p}").
//! - **Result** (`mapToolResultToToolResultBlockParam`, `:177-197`): no files →
//!   "No files found"; else the relative paths joined by `\n`, plus, when
//!   truncated, "(Results are truncated. Consider using a more specific path or
//!   pattern.)".
//! - **Gaps (D8):** CC's permission ignore-patterns + plugin-cache exclusions →
//!   our `PathPolicy` jail (ADR-0008); absolute glob patterns not base-extracted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host, rg_program};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// CC's default result cap (`GlobTool.ts:157`).
const GLOB_LIMIT: usize = 100;

/// Claude Code's `Glob` tool.
pub(crate) struct ClaudeGlob {
    host: Arc<Host>,
}

impl ClaudeGlob {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `Glob` (`GlobTool.ts:26-36`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobArgs {
    #[schemars(description = "The glob pattern to match files against")]
    pattern: String,
    #[schemars(
        description = "The directory to search in. If not specified, the current working directory will be used. IMPORTANT: Omit this field to use the default directory. DO NOT enter \"undefined\" or \"null\" - simply omit it for the default behavior. Must be a valid directory path if provided."
    )]
    #[serde(default)]
    path: Option<String>,
}

/// The structured (report) face; the joined list is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct GlobOutput {
    /// Matching paths, relative to cwd, capped at 100 (mtime-sorted, oldest first).
    filenames: Vec<String>,
    /// Total matches (may exceed `filenames.len()` when truncated).
    num_files: usize,
    /// Whether the result was capped at 100.
    truncated: bool,
    /// The rendered body (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for GlobOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

#[async_trait]
impl Tool for ClaudeGlob {
    type Args = GlobArgs;
    type Output = GlobOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Glob
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        include_str!("descriptions/glob.md")
    }

    async fn run(&self, ctx: &ToolCtx, args: GlobArgs) -> Result<Self::Output, ToolError> {
        // getPath: an explicit `path` (validated as a directory) or cwd.
        let search_dir: PathBuf = match args.path.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => {
                let resolved = self
                    .host
                    .resolve_in_jail(&ctx.cwd, Path::new(p))
                    .await
                    .map_err(|e| ToolError::Respond(e.to_string()))?;
                match self.host.read_dir(&ctx.cwd, Path::new(p)).await {
                    Ok(_) => resolved,
                    Err(e) if matches!(&e, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        return Err(ToolError::Respond(format!(
                            "Directory does not exist: {p}. Note: your current working directory is {}.",
                            ctx.cwd.display()
                        )));
                    }
                    Err(_) => {
                        return Err(ToolError::Respond(format!("Path is not a directory: {p}")));
                    }
                }
            }
            None => ctx.cwd.clone(),
        };

        // `rg --files --glob <pattern> --sort=modified --no-ignore --hidden`.
        let rg_args: Vec<String> = vec![
            "--files".into(),
            "--glob".into(),
            args.pattern.clone(),
            "--sort=modified".into(),
            "--no-ignore".into(),
            "--hidden".into(),
        ];
        let out = self
            .host
            .run_capture(&rg_program(), &rg_args, &search_dir, None, &ctx.cancel)
            .await
            .map_err(|e| {
                ToolError::Respond(format!(
                    "ripgrep (rg) could not be run ({e}); install rg or set LOCODE_RG_PATH."
                ))
            })?;

        // rg exit codes: 0 = files, 1 = none, 2+ = error.
        match out.exit_code {
            Some(0) => {}
            Some(1) => {
                return Ok(GlobOutput {
                    filenames: Vec::new(),
                    num_files: 0,
                    truncated: false,
                    body: "No files found".to_string(),
                });
            }
            _ => {
                return Err(ToolError::Respond(format!(
                    "glob failed: {}",
                    if out.stderr.is_empty() {
                        "ripgrep error".to_owned()
                    } else {
                        out.stderr
                    }
                )));
            }
        }

        // rg returns paths relative to `search_dir` (mtime-sorted). Relativize
        // under cwd to save tokens (CC's `toRelativePath`).
        let all: Vec<String> = out
            .stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                let abs = search_dir.join(line);
                abs.strip_prefix(&ctx.cwd)
                    .unwrap_or(&abs)
                    .display()
                    .to_string()
            })
            .collect();
        let num_files = all.len();
        let truncated = num_files > GLOB_LIMIT;
        let filenames: Vec<String> = all.into_iter().take(GLOB_LIMIT).collect();

        let body = if filenames.is_empty() {
            "No files found".to_string()
        } else if truncated {
            format!(
                "{}\n(Results are truncated. Consider using a more specific path or pattern.)",
                filenames.join("\n")
            )
        } else {
            filenames.join("\n")
        };

        Ok(GlobOutput {
            filenames,
            num_files,
            truncated,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn description_is_the_pinned_glob_md() {
        let desc = include_str!("descriptions/glob.md");
        // sha256 6194aea168bb308f0fd6801bc938a35d3160a8f5339d5d5170ab688990239f80.
        assert_eq!(desc.len(), 371, "glob.md byte length changed");
        assert!(desc.starts_with("- Fast file pattern matching tool"));
        assert!(desc.trim_end().ends_with("use the Agent tool instead"));
    }
}
