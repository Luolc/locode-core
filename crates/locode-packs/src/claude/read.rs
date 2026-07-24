//! `Read` — a faithful port of Claude Code's `FileReadTool`, text path.
//!
//! Fidelity notes (Claude Code submodule commit `6a25909`):
//! - **Schema** (`FileReadTool.ts:227-244`, `z.strictObject`): `file_path` +
//!   `offset` + `limit` + `pages`. `deny_unknown_fields`; `offset`/`limit`
//!   type-strict (no string coercion — repo policy; CC's `semanticNumber`
//!   coerces). `pages` is kept **schema-visible** and accepted but ignored for
//!   text (PDF tier deferred) — grok's `read_file` precedent (`grok/read.rs` kept
//!   `pages`/`format`); a faithfulness-over-plan-§4.2 amendment.
//! - **Description** (`FileReadTool.ts:347-358` → `renderPromptTemplate`,
//!   `prompt.ts:27-49`) rendered for our config (2000-line default; no max-size
//!   clause; default offset nudge; `isPDFSupported()`=true → the PDF bullet
//!   renders). Verbatim in `descriptions/read.md` (pinned). Gap (D8): the
//!   description advertises images/PDF/notebooks; we serve text only (non-text
//!   degrades to lossy UTF-8), and `pages` is ignored.
//! - **Output** (`addLineNumbers`, `file.ts:290-318`; default compact branch,
//!   `isCompactLinePrefixEnabled` killswitch off): `${lineNo}\t${line}`, 1-indexed
//!   absolute, tab separator. We honor the documented "cat -n" contract with real
//!   cat -n line semantics (`str::lines()` — a trailing newline adds no phantom
//!   numbered line). `maxResultSizeChars: Infinity` (`:342`) — no result cap; the
//!   guard is the token cap.
//! - **Window** (`call()`, `:497,1019`): `offset` default 1 (1-indexed,
//!   `offset===0?0:offset-1`), `limit` default = read to the token cap.
//! - **Warnings** (`:704-707`): empty file / window past EOF → the verbatim
//!   `<system-reminder>` texts.
//! - **Token cap** (`limits.ts:18`, `:181`): 25000 tokens; error text verbatim.
//!   We use a byte/4 estimate (grok precedent — no API tokenizer); documented gap.
//! - **Freshness / dedup** (`:540-570,1032`): records mtime + window in
//!   `ClaudeSessionState`; an unchanged same-window re-read returns
//!   `FILE_UNCHANGED_STUB`. Edit/Write (S3/S4) gate on the same store.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::state::ClaudeSessionState;

/// CC's default `offset` (`call()` param default, `FileReadTool.ts:497`): 1-indexed
/// from the start of the file.
const DEFAULT_OFFSET: u64 = 1;
/// CC's `DEFAULT_MAX_OUTPUT_TOKENS` (`limits.ts:18`).
const MAX_TOKENS: usize = 25_000;
/// CC's `FILE_UNCHANGED_STUB` (`prompt.ts:7-8`).
const FILE_UNCHANGED_STUB: &str = "File unchanged since last read. The content from the earlier Read tool_result in this conversation is still current — refer to that instead of re-reading.";

/// Claude Code's `Read` tool.
pub(crate) struct ClaudeRead {
    host: Arc<Host>,
    state: Arc<ClaudeSessionState>,
}

impl ClaudeRead {
    pub(crate) fn new(host: Arc<Host>, state: Arc<ClaudeSessionState>) -> Self {
        Self { host, state }
    }
}

/// Arguments for `Read` (`FileReadTool.ts:227-244`). `pages` kept for schema
/// fidelity; PDF tier deferred.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadArgs {
    #[schemars(description = "The absolute path to the file to read")]
    file_path: String,
    #[schemars(
        description = "The line number to start reading from. Only provide if the file is too large to read at once"
    )]
    #[serde(default)]
    offset: Option<u64>,
    #[schemars(
        description = "The number of lines to read. Only provide if the file is too large to read at once."
    )]
    #[serde(default)]
    limit: Option<u64>,
    #[schemars(
        description = "Page range for PDF files (e.g., \"1-5\", \"3\", \"10-20\"). Only applicable to PDF files. Maximum 20 pages per request."
    )]
    #[serde(default)]
    #[allow(dead_code)] // PDF tier deferred; kept for schema fidelity.
    pages: Option<String>,
}

/// The structured (report) face; the numbered body is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct ReadOutput {
    /// The jail-resolved absolute path.
    path: String,
    /// Total lines in the file (real cat -n line count).
    lines: usize,
    /// Whether the returned window is a subset of the file.
    truncated: bool,
    /// The rendered body (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for ReadOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

/// grok/CC's byte/4 token estimate (no API tokenizer available).
fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
}

/// Map a host read failure to a soft-error message (jail rejections keep our text).
fn read_error_text(display_path: &str, err: &FsError) -> String {
    match err {
        FsError::Io { source, .. } => match source.kind() {
            std::io::ErrorKind::NotFound => format!("Error: {display_path} does not exist."),
            std::io::ErrorKind::IsADirectory => {
                format!("Error: {display_path} is a directory, not a file.")
            }
            std::io::ErrorKind::PermissionDenied => format!("Permission denied: {display_path}"),
            _ => format!("Failed to read file: {display_path}, {err}"),
        },
        FsError::Path(_) => err.to_string(),
    }
}

#[async_trait]
impl Tool for ClaudeRead {
    type Args = ReadArgs;
    type Output = ReadOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        include_str!("descriptions/read.md")
    }

    async fn run(&self, ctx: &ToolCtx, args: ReadArgs) -> Result<Self::Output, ToolError> {
        let path = Path::new(&args.file_path);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        let read = self
            .host
            .read_file(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(read_error_text(&args.file_path, &e)))?;
        let contents = read.contents;
        let modified = read.stat.modified;

        let effective_offset = args.offset.unwrap_or(DEFAULT_OFFSET);
        let limit = args.limit;

        // Dedup (`FileReadTool.ts:540-570`): an unchanged same-window re-read
        // returns the stub instead of re-sending the content.
        if self
            .state
            .is_unchanged_read(&resolved, modified, Some(effective_offset), limit)
        {
            return Ok(ReadOutput {
                path: resolved.display().to_string(),
                lines: contents.lines().count(),
                truncated: false,
                body: FILE_UNCHANGED_STUB.to_string(),
            });
        }

        let lines: Vec<&str> = contents.lines().collect();
        let total_lines = lines.len();
        // `offset===0 ? 0 : offset-1` (`:1019`).
        let line_offset = if effective_offset == 0 {
            0
        } else {
            usize::try_from(effective_offset - 1).unwrap_or(usize::MAX)
        };
        let take = limit.map_or(usize::MAX, |l| usize::try_from(l).unwrap_or(usize::MAX));

        // The window and its absolute 1-indexed line numbers (CC's `startLine +
        // index`, `addLineNumbers`); startLine = the raw effective offset.
        let windowed: Vec<(u64, &str)> = lines
            .iter()
            .enumerate()
            .skip(line_offset)
            .take(take)
            .map(|(i, &line)| {
                let n = effective_offset + (i as u64 - line_offset as u64);
                (n, line)
            })
            .collect();

        // Token cap on the raw window content (`validateContentTokens`, `:1030`).
        let raw: String = windowed
            .iter()
            .map(|(_, l)| *l)
            .collect::<Vec<_>>()
            .join("\n");
        let token_count = estimate_tokens(&raw);
        if token_count > MAX_TOKENS {
            return Err(ToolError::Respond(format!(
                "File content ({token_count} tokens) exceeds maximum allowed tokens ({MAX_TOKENS}). \
                 Use offset and limit parameters to read specific portions of the file, or search \
                 for specific content instead of reading the whole file."
            )));
        }

        let body = if windowed.is_empty() {
            // CC: empty content → the empty-file / short-offset warnings (`:704-707`).
            if total_lines == 0 {
                "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>"
                    .to_string()
            } else {
                format!(
                    "<system-reminder>Warning: the file exists but is shorter than the provided \
                     offset ({effective_offset}). The file has {total_lines} lines.</system-reminder>"
                )
            }
        } else {
            windowed
                .iter()
                .map(|(n, l)| format!("{n}\t{l}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // Record the read (CC's `readFileState.set` with offset set).
        self.state
            .record_read(resolved.clone(), modified, Some(effective_offset), limit);

        let truncated = line_offset > 0 || line_offset + windowed.len() < total_lines;
        Ok(ReadOutput {
            path: resolved.display().to_string(),
            lines: total_lines,
            truncated,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_is_the_pinned_read_md() {
        let desc = include_str!("descriptions/read.md");
        // Provenance pin: byte length + opening + closing lines.
        // sha256 5c903c060a6d4f7f191ba7a5fc8553b28fcae15c302e7d143a073332821640fd.
        assert_eq!(desc.len(), 1680, "read.md byte length changed");
        assert!(desc.starts_with("Reads a file from the local filesystem."));
        assert!(
            desc.trim_end()
                .ends_with("you will receive a system reminder warning in place of file contents.")
        );
        // The PDF bullet renders (non-haiku model); documented gap.
        assert!(desc.contains("This tool can read PDF files (.pdf)."));
    }

    #[test]
    fn estimate_tokens_is_byte_quarter() {
        assert_eq!(estimate_tokens(&"x".repeat(400)), 100);
    }
}
