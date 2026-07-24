//! `Grep` — a faithful port of Claude Code's `GrepTool`: a ripgrep front-end with
//! three output modes, context flags, glob/type filters, and head-limit pagination.
//!
//! Fidelity notes (Claude Code submodule commit `6a25909`):
//! - **Schema** (`GrepTool.ts` inputSchema, `z.strictObject`): `pattern` + `path` +
//!   `glob` + `output_mode` + `-A/-B/-C` + `context` + `-n` + `-i` + `type` +
//!   `head_limit` + `offset` + `multiline` (verbatim field descriptions).
//!   `deny_unknown_fields`; numbers/bools type-strict (CC's `semantic*` coerce —
//!   repo policy declines that).
//! - **Description** (`prompt.ts:6-18`, `getDescription`) verbatim in
//!   `descriptions/grep.md` (mentions the `Agent` tool — kept, D8 gap).
//! - **rg arg order** (`call()`, `GrepTool.ts`): `--hidden` → `--glob !<vcs>`×6
//!   (`.git .svn .hg .bzr .jj .sl`) → `--max-columns 500` → `-U --multiline-dotall`
//!   (if multiline) → `-i` → mode flag (`-l`/`-c`/none) → `-n` (content + line
//!   numbers, default true) → context (`-C`/`context` precedence, else `-B`/`-A`;
//!   content only) → pattern (`-e` if it starts with `-`) → `--type` → `--glob`
//!   (split on whitespace, brace-preserving) → the absolute search target as the
//!   positional (rg emits absolute paths, `ripgrep.ts:365`).
//! - **`head_limit` / `offset`** (`applyHeadLimit`, `:110-128`): default 250, `0` =
//!   unlimited; `appliedLimit` reported only when truncation occurred. Paths
//!   relativized under cwd (`toRelativePath`). `files_with_matches` sorts by mtime
//!   **descending** (filename tiebreak).
//! - **Result rendering** (`mapToolResultToToolResultBlockParam`, `:254-311`):
//!   content → lines (or "No matches found") + optional pagination note; count →
//!   lines + "Found N total occurrences across M files." summary; files → "No
//!   files found" or "Found N file(s) `[limit]` then the paths".
//! - **rg exit codes** (`ripgrep.ts:378-386`): 0 = matches, 1 = none (empty), 2+ =
//!   error.
//! - **Gaps (D8):** CC's permission ignore-patterns + plugin-cache exclusions →
//!   our `PathPolicy` jail (ADR-0008); the 20k `maxResultSizeChars` persist-preview
//!   is approximated by a head char cap + the 30k engine belt (ADR-0008).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{Host, rg_program};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// CC's `DEFAULT_HEAD_LIMIT` (`GrepTool.ts:108`).
const DEFAULT_HEAD_LIMIT: usize = 250;
/// CC's `maxResultSizeChars` for Grep (`GrepTool.ts:164`).
const MAX_RESULT_CHARS: usize = 20_000;
/// VCS metadata dirs CC excludes (`GrepTool.ts:95-102`).
const VCS_EXCLUDES: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// The three output modes (`GrepTool.ts` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GrepOutputMode {
    Content,
    #[default]
    FilesWithMatches,
    Count,
}

/// Claude Code's `Grep` tool.
pub(crate) struct ClaudeGrep {
    host: Arc<Host>,
}

impl ClaudeGrep {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `Grep` (`GrepTool.ts` inputSchema).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrepArgs {
    #[schemars(description = "The regular expression pattern to search for in file contents")]
    pattern: String,
    #[schemars(
        description = "File or directory to search in (rg PATH). Defaults to current working directory."
    )]
    #[serde(default)]
    path: Option<String>,
    #[schemars(
        description = "Glob pattern to filter files (e.g. \"*.js\", \"*.{ts,tsx}\") - maps to rg --glob"
    )]
    #[serde(default)]
    glob: Option<String>,
    #[schemars(
        description = "Output mode: \"content\" shows matching lines (supports -A/-B/-C context, -n line numbers, head_limit), \"files_with_matches\" shows file paths (supports head_limit), \"count\" shows match counts (supports head_limit). Defaults to \"files_with_matches\"."
    )]
    #[serde(default)]
    output_mode: Option<GrepOutputMode>,
    #[schemars(
        description = "Number of lines to show before each match (rg -B). Requires output_mode: \"content\", ignored otherwise."
    )]
    #[serde(default, rename = "-B")]
    before: Option<u64>,
    #[schemars(
        description = "Number of lines to show after each match (rg -A). Requires output_mode: \"content\", ignored otherwise."
    )]
    #[serde(default, rename = "-A")]
    after: Option<u64>,
    #[schemars(description = "Alias for context.")]
    #[serde(default, rename = "-C")]
    dash_c: Option<u64>,
    #[schemars(
        description = "Number of lines to show before and after each match (rg -C). Requires output_mode: \"content\", ignored otherwise."
    )]
    #[serde(default)]
    context: Option<u64>,
    #[schemars(
        description = "Show line numbers in output (rg -n). Requires output_mode: \"content\", ignored otherwise. Defaults to true."
    )]
    #[serde(default, rename = "-n")]
    line_numbers: Option<bool>,
    #[schemars(description = "Case insensitive search (rg -i)")]
    #[serde(default, rename = "-i")]
    case_insensitive: Option<bool>,
    #[schemars(
        description = "File type to search (rg --type). Common types: js, py, rust, go, java, etc. More efficient than include for standard file types."
    )]
    #[serde(default)]
    r#type: Option<String>,
    #[schemars(
        description = "Limit output to first N lines/entries, equivalent to \"| head -N\". Works across all output modes: content (limits output lines), files_with_matches (limits file paths), count (limits count entries). Defaults to 250 when unspecified. Pass 0 for unlimited (use sparingly — large result sets waste context)."
    )]
    #[serde(default)]
    head_limit: Option<u64>,
    #[schemars(
        description = "Skip first N lines/entries before applying head_limit, equivalent to \"| tail -n +N | head -N\". Works across all output modes. Defaults to 0."
    )]
    #[serde(default)]
    offset: Option<u64>,
    #[schemars(
        description = "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall). Default: false."
    )]
    #[serde(default)]
    multiline: Option<bool>,
}

/// The structured (report) face; the rendered body is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct GrepOutput {
    /// The output mode used.
    mode: &'static str,
    /// Number of files (count/files modes).
    num_files: usize,
    /// Total matches (count mode).
    num_matches: usize,
    /// The rendered body (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for GrepOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

/// CC's `applyHeadLimit` (`GrepTool.ts:110-128`): slice `[offset, offset+limit)`;
/// `limit == Some(0)` = unlimited; default 250. Returns the slice + the applied
/// limit *only when truncation occurred* (so the model knows to paginate).
fn apply_head_limit(
    items: &[String],
    limit: Option<u64>,
    offset: usize,
) -> (Vec<String>, Option<usize>) {
    if limit == Some(0) {
        return (items.iter().skip(offset).cloned().collect(), None);
    }
    let effective = limit.map_or(DEFAULT_HEAD_LIMIT, |l| {
        usize::try_from(l).unwrap_or(usize::MAX)
    });
    let sliced: Vec<String> = items.iter().skip(offset).take(effective).cloned().collect();
    let truncated = items.len().saturating_sub(offset) > effective;
    (sliced, truncated.then_some(effective))
}

/// CC's `formatLimitInfo` (`:134-142`): `offset` only shown when > 0.
fn format_limit_info(applied_limit: Option<usize>, applied_offset: usize) -> String {
    let mut parts = Vec::new();
    if let Some(l) = applied_limit {
        parts.push(format!("limit: {l}"));
    }
    if applied_offset > 0 {
        parts.push(format!("offset: {applied_offset}"));
    }
    parts.join(", ")
}

/// Relativize an absolute path under `cwd` (CC's `toRelativePath`).
fn relativize(path: &str, cwd: &Path) -> String {
    Path::new(path)
        .strip_prefix(cwd)
        .map_or_else(|_| path.to_string(), |p| p.display().to_string())
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Cap the rendered body at 20k chars (CC's `maxResultSizeChars`), head-truncated
/// with a marker (CC persists large output — a P1 gap; the 30k engine belt is on top).
fn cap(body: String) -> String {
    if body.chars().count() <= MAX_RESULT_CHARS {
        return body;
    }
    let head: String = body.chars().take(MAX_RESULT_CHARS).collect();
    format!("{head}\n... [output truncated at 20000 characters] ...")
}

#[async_trait]
impl Tool for ClaudeGrep {
    type Args = GrepArgs;
    type Output = GrepOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Grep
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        include_str!("descriptions/grep.md")
    }

    #[allow(clippy::too_many_lines)] // faithful 1:1 port of CC's call() arg build + render
    async fn run(&self, ctx: &ToolCtx, args: GrepArgs) -> Result<Self::Output, ToolError> {
        let mode = args.output_mode.unwrap_or_default();
        let offset = args
            .offset
            .map_or(0, |o| usize::try_from(o).unwrap_or(usize::MAX));

        // Search target: an explicit `path` (jail-resolved to absolute) or cwd.
        let target: PathBuf = match args.path.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => self
                .host
                .resolve_in_jail(&ctx.cwd, Path::new(p))
                .await
                .map_err(|e| ToolError::Respond(e.to_string()))?,
            None => ctx.cwd.clone(),
        };

        // --- rg args, faithful to CC's order ---
        let mut rg: Vec<String> = vec!["--hidden".into()];
        for dir in VCS_EXCLUDES {
            rg.push("--glob".into());
            rg.push(format!("!{dir}"));
        }
        rg.push("--max-columns".into());
        rg.push("500".into());
        if args.multiline.unwrap_or(false) {
            rg.push("-U".into());
            rg.push("--multiline-dotall".into());
        }
        if args.case_insensitive.unwrap_or(false) {
            rg.push("-i".into());
        }
        match mode {
            GrepOutputMode::FilesWithMatches => rg.push("-l".into()),
            GrepOutputMode::Count => rg.push("-c".into()),
            GrepOutputMode::Content => {}
        }
        if mode == GrepOutputMode::Content && args.line_numbers.unwrap_or(true) {
            rg.push("-n".into());
        }
        if mode == GrepOutputMode::Content {
            if let Some(c) = args.context {
                rg.push("-C".into());
                rg.push(c.to_string());
            } else if let Some(c) = args.dash_c {
                rg.push("-C".into());
                rg.push(c.to_string());
            } else {
                if let Some(b) = args.before {
                    rg.push("-B".into());
                    rg.push(b.to_string());
                }
                if let Some(a) = args.after {
                    rg.push("-A".into());
                    rg.push(a.to_string());
                }
            }
        }
        // A pattern starting with `-` is passed via `-e` so rg doesn't read it as a flag.
        if args.pattern.starts_with('-') {
            rg.push("-e".into());
        }
        rg.push(args.pattern.clone());
        if let Some(t) = args.r#type.as_deref().filter(|t| !t.is_empty()) {
            rg.push("--type".into());
            rg.push(t.to_string());
        }
        if let Some(glob) = args.glob.as_deref().filter(|g| !g.is_empty()) {
            for pat in split_glob(glob) {
                rg.push("--glob".into());
                rg.push(pat);
            }
        }
        // The absolute search target as the positional (rg emits absolute paths).
        rg.push(target.display().to_string());

        let out = self
            .host
            .run_capture(&rg_program(), &rg, &ctx.cwd, None, &ctx.cancel)
            .await
            .map_err(|e| {
                ToolError::Respond(format!(
                    "ripgrep (rg) could not be run ({e}); install rg or set LOCODE_RG_PATH."
                ))
            })?;

        // rg exit codes: 0 = matches, 1 = none, 2+ = error.
        let lines: Vec<String> = match out.exit_code {
            Some(0) => out
                .stdout
                .lines()
                .map(|l| l.trim_end_matches('\r'))
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
            Some(1) => Vec::new(),
            _ => {
                return Err(ToolError::Respond(format!(
                    "grep failed: {}",
                    if out.stderr.is_empty() {
                        "ripgrep error".to_owned()
                    } else {
                        out.stderr
                    }
                )));
            }
        };

        let (body, num_files, num_matches, mode_str) = match mode {
            GrepOutputMode::Content => {
                let (limited, applied_limit) = apply_head_limit(&lines, args.head_limit, offset);
                let rendered: Vec<String> = limited
                    .iter()
                    .map(|line| relativize_prefix(line, &ctx.cwd, false))
                    .collect();
                let content = rendered.join("\n");
                let limit_info = format_limit_info(applied_limit, offset);
                let result = if content.is_empty() {
                    "No matches found".to_string()
                } else {
                    content
                };
                let body = if limit_info.is_empty() {
                    result
                } else {
                    format!("{result}\n\n[Showing results with pagination = {limit_info}]")
                };
                (body, 0, 0, "content")
            }
            GrepOutputMode::Count => {
                let (limited, applied_limit) = apply_head_limit(&lines, args.head_limit, offset);
                let rendered: Vec<String> = limited
                    .iter()
                    .map(|line| relativize_prefix(line, &ctx.cwd, true))
                    .collect();
                let mut total = 0usize;
                let mut files = 0usize;
                for line in &rendered {
                    if let Some(idx) = line.rfind(':')
                        && let Ok(c) = line[idx + 1..].parse::<usize>()
                    {
                        total += c;
                        files += 1;
                    }
                }
                let content = rendered.join("\n");
                let raw = if content.is_empty() {
                    "No matches found".to_string()
                } else {
                    content
                };
                let limit_info = format_limit_info(applied_limit, offset);
                let occ = if total == 1 {
                    "occurrence"
                } else {
                    "occurrences"
                };
                let fw = if files == 1 { "file" } else { "files" };
                let pag = if limit_info.is_empty() {
                    String::new()
                } else {
                    format!(" with pagination = {limit_info}")
                };
                let summary = format!("\n\nFound {total} total {occ} across {files} {fw}.{pag}");
                (format!("{raw}{summary}"), files, total, "count")
            }
            GrepOutputMode::FilesWithMatches => {
                // Sort by mtime desc (filename tiebreak), then head-limit.
                let mut with_mtime: Vec<(String, std::time::SystemTime)> = Vec::new();
                for path in &lines {
                    let mtime = self
                        .host
                        .stat(&ctx.cwd, Path::new(path))
                        .await
                        .ok()
                        .and_then(|s| s.modified)
                        .unwrap_or(std::time::UNIX_EPOCH);
                    with_mtime.push((path.clone(), mtime));
                }
                with_mtime.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                let sorted: Vec<String> = with_mtime.into_iter().map(|(p, _)| p).collect();
                let (limited, applied_limit) = apply_head_limit(&sorted, args.head_limit, offset);
                let rel: Vec<String> = limited.iter().map(|p| relativize(p, &ctx.cwd)).collect();
                let num_files = rel.len();
                let limit_info = format_limit_info(applied_limit, offset);
                let body = if num_files == 0 {
                    "No files found".to_string()
                } else {
                    let head = if limit_info.is_empty() {
                        format!("Found {}", plural(num_files, "file"))
                    } else {
                        format!("Found {} {limit_info}", plural(num_files, "file"))
                    };
                    format!("{head}\n{}", rel.join("\n"))
                };
                (body, num_files, 0, "files_with_matches")
            }
        };

        Ok(GrepOutput {
            mode: mode_str,
            num_files,
            num_matches,
            body: cap(body),
        })
    }
}

/// Relativize the path prefix of a `path:rest` line. `last` uses the final colon
/// (count mode: `path:count`); otherwise the first (content mode: `path:...`).
fn relativize_prefix(line: &str, cwd: &Path, last: bool) -> String {
    let idx = if last {
        line.rfind(':')
    } else {
        line.find(':')
    };
    match idx {
        Some(i) if i > 0 => format!("{}{}", relativize(&line[..i], cwd), &line[i..]),
        _ => line.to_string(),
    }
}

/// CC's glob splitting (`GrepTool.ts` call): split on whitespace; a token with
/// `{` and `}` stays whole, else split on commas.
fn split_glob(glob: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in glob.split_whitespace() {
        if raw.contains('{') && raw.contains('}') {
            out.push(raw.to_string());
        } else {
            out.extend(raw.split(',').filter(|s| !s.is_empty()).map(str::to_string));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_is_the_pinned_grep_md() {
        let desc = include_str!("descriptions/grep.md");
        // sha256 b51741096735333cfc72878140e2313c0acc9f73226149fc1855ac348df91df4.
        assert_eq!(desc.len(), 866, "grep.md byte length changed");
        assert!(desc.starts_with("A powerful search tool built on ripgrep"));
    }

    #[test]
    fn head_limit_and_format() {
        let items: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let (sliced, applied) = apply_head_limit(&items, Some(3), 0);
        assert_eq!(sliced, vec!["0", "1", "2"]);
        assert_eq!(applied, Some(3));
        // offset skips; exact fit is not truncated.
        let (s2, a2) = apply_head_limit(&items, Some(0), 8);
        assert_eq!(s2, vec!["8", "9"]);
        assert_eq!(a2, None);
        assert_eq!(format_limit_info(Some(3), 2), "limit: 3, offset: 2");
        assert_eq!(format_limit_info(None, 0), "");
    }

    #[test]
    fn glob_split_preserves_braces() {
        assert_eq!(split_glob("*.js,*.ts"), vec!["*.js", "*.ts"]);
        assert_eq!(split_glob("*.{ts,tsx}"), vec!["*.{ts,tsx}"]);
        assert_eq!(split_glob("a.js b.ts"), vec!["a.js", "b.ts"]);
    }

    #[test]
    fn plural_matches_cc() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(3, "file"), "3 files");
    }
}
