//! `grep` — a faithful port of Grok Build's ripgrep-backed search (`gb/grep/mod.rs`),
//! over `locode-host`'s resolved `rg` (ADR-0011).
//!
//! Fidelity notes (Task 26 — full-schema port; supersedes the Task 11 cut):
//! - The wire schema reproduces grok's `GrepSearchInput` exactly, including the
//!   flag-literal field names (`-B`/`-A`/`-C`/`-i`) and the `output_mode` quirk:
//!   **accepted on the wire but hidden from the JSON schema** (grok marks it
//!   `#[schemars(skip)]`, `gb/grep/mod.rs:63-67`).
//! - Deviation (host seam): grok reads `head_limit + 1` stdout lines and kills
//!   `rg` early on overflow (`gb/grep/mod.rs:355-360`); our host captures under
//!   its byte cap and we apply the line budget post-capture. Output-equivalent
//!   (same first-N lines, same truncation flag); the difference is perf only.

use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{Host, rg_program};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Hard max when the model passes an explicit `head_limit` (content lines)
/// (`gb/grep/mod.rs:153`).
const CONTENT_LINE_LIMIT: usize = 2_000;
/// Default when `head_limit` is omitted (content) (`gb/grep/mod.rs:157`).
const CONTENT_LINE_DEFAULT: usize = 200;
/// Hard max for `files_with_matches` / `count` entry lists (`gb/grep/mod.rs:159`).
const FILE_COUNT_LIMIT: usize = 10_000;
/// Default when `head_limit` is omitted (files/count) (`gb/grep/mod.rs:161`).
const FILE_COUNT_DEFAULT: usize = 500;

/// grok's `grep` tool.
pub(crate) struct GrokGrep {
    host: Arc<Host>,
}

impl GrokGrep {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// grok's `output_mode` values (`gb/grep/mod.rs:39-47`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputMode {
    /// Matching lines (the default).
    #[default]
    Content,
    /// File paths with at least one match (rg `-l`).
    FilesWithMatches,
    /// Per-file match counts (rg `-c`).
    Count,
}

/// Arguments for `grep` — grok's real `GrepSearchInput`, field for field
/// (`gb/grep/mod.rs:48-128`).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GrepArgs {
    #[schemars(
        description = "The regular expression pattern to search for in file contents (rg --regexp)"
    )]
    pattern: String,
    #[serde(default)]
    #[schemars(
        description = "File or directory to search in (rg pattern -- PATH). Defaults to workspace path."
    )]
    path: Option<String>,
    #[serde(default)]
    #[schemars(
        description = "Glob pattern (rg --glob GLOB -- PATH) to filter files (e.g. \"*.js\", \"*.{ts,tsx}\")."
    )]
    glob: Option<String>,
    /// Accepted on the wire when present; omitted from the JSON schema —
    /// grok's exact quirk (`gb/grep/mod.rs:63-67`).
    #[serde(default)]
    #[schemars(skip)]
    output_mode: Option<OutputMode>,
    #[serde(default, rename = "-B")]
    #[schemars(description = "Number of lines to show before each match (rg -B).")]
    before_context: Option<usize>,
    #[serde(default, rename = "-A")]
    #[schemars(description = "Number of lines to show after each match (rg -A).")]
    after_context: Option<usize>,
    #[serde(default, rename = "-C")]
    #[schemars(description = "Number of lines to show before and after each match (rg -C).")]
    context: Option<usize>,
    #[serde(default, rename = "-i")]
    #[schemars(description = "Case insensitive search (rg -i). Defaults to false.")]
    case_insensitive: Option<bool>,
    #[serde(default)]
    #[schemars(
        description = "File type to search (rg --type). Common types: js, py, rust, go, java, etc. More efficient than glob for standard file types."
    )]
    r#type: Option<String>,
    #[serde(default)]
    #[schemars(
        description = "Limit output to first N lines/entries, equivalent to \"| head -N\". Defaults to 200 lines or 500 entries."
    )]
    head_limit: Option<usize>,
    #[serde(default)]
    #[schemars(
        description = "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall). Default: false."
    )]
    multiline: Option<bool>,
}

/// grok's effective head limit: mode default when omitted, mode cap always
/// (`resolve_effective_head_limit`, `gb/grep/mod.rs:197-203`).
fn resolve_effective_head_limit(head_limit: Option<usize>, output_mode: &OutputMode) -> usize {
    let (default, cap) = match output_mode {
        OutputMode::Content => (CONTENT_LINE_DEFAULT, CONTENT_LINE_LIMIT),
        OutputMode::FilesWithMatches | OutputMode::Count => (FILE_COUNT_DEFAULT, FILE_COUNT_LIMIT),
    };
    head_limit.unwrap_or(default).min(cap)
}

/// The structured (report) face; the rg output is the prompt face.
#[derive(Debug, Serialize)]
pub(crate) struct GrepOutput {
    /// Whether any match was found (rg exit 0 vs 1).
    matched: bool,
    /// Whether the output was line- or byte-capped.
    truncated: bool,
    #[serde(skip)]
    text: String,
}

impl ToolOutput for GrepOutput {
    fn to_prompt_text(&self) -> String {
        if self.matched {
            self.text.clone()
        } else {
            "No matches found.".to_owned()
        }
    }
}

#[async_trait]
impl Tool for GrokGrep {
    type Args = GrepArgs;
    type Output = GrepOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Grep
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        // grok's real description template, verbatim (`gb/grep/mod.rs:248-258`).
        r#"Search file contents with regular expressions (ripgrep).

- Full regex syntax, so escape literal special characters: `functionCall\(`, or `interface\{\}` to find interface{} in Go.
- Pass the pattern as a raw regex string — no surrounding quotes.
- Respects .gitignore unless you pass a broad glob like '--glob *'.
- Only filter by 'type' or 'glob' when you are sure of the file type; import paths may not match source file types (.js vs .ts).
- Output is ripgrep-style: ':' marks match lines, '-' marks context lines, grouped by file. Large results are capped and report "at least" counts."#
    }

    async fn run(&self, ctx: &ToolCtx, args: GrepArgs) -> Result<Self::Output, ToolError> {
        let output_mode = args.output_mode.clone().unwrap_or_default();
        let effective_head_limit = resolve_effective_head_limit(args.head_limit, &output_mode);

        // rg's flag set and ORDER, faithful to grok (`gb/grep/mod.rs:760-828`):
        // base flags → -i → --glob → --type → multiline → -C → -B → -A →
        // mode flag → `-e PATTERN` → PATH → --max-filesize 5M.
        let mut rg_args: Vec<String> = vec![
            "--heading".into(),
            "--with-filename".into(),
            "--line-number".into(),
            "--color=never".into(),
            "--max-columns".into(),
            "1000".into(),
            "--max-columns-preview".into(),
        ];
        if args.case_insensitive.unwrap_or(false) {
            rg_args.push("--ignore-case".into());
        }
        if let Some(glob) = &args.glob
            && !glob.is_empty()
        {
            rg_args.push("--glob".into());
            rg_args.push(glob.clone());
        }
        if let Some(t) = &args.r#type
            && !t.is_empty()
        {
            rg_args.push("--type".into());
            rg_args.push(t.clone());
        }
        if args.multiline.unwrap_or(false) {
            rg_args.push("-U".into());
            rg_args.push("--multiline-dotall".into());
        }
        if let Some(c) = args.context
            && c > 0
        {
            rg_args.push("-C".into());
            rg_args.push(c.to_string());
        }
        if let Some(b) = args.before_context
            && b > 0
        {
            rg_args.push("-B".into());
            rg_args.push(b.to_string());
        }
        if let Some(a) = args.after_context
            && a > 0
        {
            rg_args.push("-A".into());
            rg_args.push(a.to_string());
        }
        match output_mode {
            OutputMode::FilesWithMatches => rg_args.push("-l".into()),
            OutputMode::Count => rg_args.push("-c".into()),
            OutputMode::Content => {}
        }
        rg_args.push("-e".into());
        rg_args.push(args.pattern.clone());
        rg_args.push(args.path.clone().unwrap_or_else(|| ".".into()));
        rg_args.push("--max-filesize".into());
        rg_args.push("5M".into());

        let out = self
            .host
            .run_capture(&rg_program(), &rg_args, &ctx.cwd, None, &ctx.cancel)
            .await
            .map_err(|e| {
                // The only failure that reaches here is a spawn failure (rg unresolvable).
                ToolError::Respond(format!(
                    "ripgrep (rg) could not be run ({e}); install rg or set LOCODE_RG_PATH."
                ))
            })?;

        // rg exit codes: 0 = matches, 1 = no matches (not an error), 2+ = real error.
        match out.exit_code {
            Some(0) => {
                let (text, line_capped) = apply_head_limit(&out.stdout, effective_head_limit);
                Ok(GrepOutput {
                    matched: true,
                    truncated: out.truncated || line_capped,
                    text,
                })
            }
            Some(1) => Ok(GrepOutput {
                matched: false,
                truncated: false,
                text: String::new(),
            }),
            _ => Err(ToolError::Respond(format!(
                "grep failed: {}",
                if out.stderr.is_empty() {
                    "ripgrep error".to_owned()
                } else {
                    out.stderr
                }
            ))),
        }
    }
}

/// First `limit` lines of `text`, and whether anything was cut (grok reads
/// `limit + 1` lines so an exact-fit result is never flagged truncated —
/// `gb/grep/mod.rs:355-360`; same rule applied post-capture here).
fn apply_head_limit(text: &str, limit: usize) -> (String, bool) {
    let mut end = 0usize;
    let mut lines = 0usize;
    for (idx, _) in text.match_indices('\n') {
        lines += 1;
        if lines == limit {
            end = idx + 1;
            break;
        }
    }
    if lines < limit || end >= text.len() {
        (text.to_owned(), false)
    } else {
        (text[..end].to_owned(), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_limit_exact_fit_is_not_truncated() {
        let text = "a\nb\nc\n";
        let (out, truncated) = apply_head_limit(text, 3);
        assert_eq!(out, text);
        assert!(!truncated);
    }

    #[test]
    fn head_limit_cuts_and_flags() {
        let (out, truncated) = apply_head_limit("a\nb\nc\nd\n", 2);
        assert_eq!(out, "a\nb\n");
        assert!(truncated);
    }

    #[test]
    fn effective_limit_defaults_and_caps() {
        assert_eq!(
            resolve_effective_head_limit(None, &OutputMode::Content),
            200
        );
        assert_eq!(
            resolve_effective_head_limit(None, &OutputMode::FilesWithMatches),
            500
        );
        assert_eq!(
            resolve_effective_head_limit(Some(9_999_999), &OutputMode::Content),
            2_000
        );
        assert_eq!(
            resolve_effective_head_limit(Some(9_999_999), &OutputMode::Count),
            10_000
        );
        assert_eq!(
            resolve_effective_head_limit(Some(50), &OutputMode::Content),
            50
        );
    }

    #[test]
    fn schema_uses_grok_wire_names_and_hides_output_mode() {
        let schema = serde_json::to_value(schemars::schema_for!(GrepArgs)).unwrap();
        let props = schema["properties"].as_object().unwrap();
        for key in [
            "pattern",
            "path",
            "glob",
            "-B",
            "-A",
            "-C",
            "-i",
            "type",
            "head_limit",
            "multiline",
        ] {
            assert!(props.contains_key(key), "missing schema field {key}");
        }
        // grok's quirk: accepted on the wire, absent from the schema.
        assert!(!props.contains_key("output_mode"));
        // ...but still deserializes.
        let args: GrepArgs =
            serde_json::from_value(serde_json::json!({"pattern": "x", "output_mode": "count"}))
                .unwrap();
        assert_eq!(args.output_mode, Some(OutputMode::Count));
    }

    #[test]
    fn flag_fields_deserialize_from_wire_names() {
        let args: GrepArgs = serde_json::from_value(serde_json::json!({
            "pattern": "x", "-B": 2, "-A": 3, "-C": 1, "-i": true
        }))
        .unwrap();
        assert_eq!(args.before_context, Some(2));
        assert_eq!(args.after_context, Some(3));
        assert_eq!(args.context, Some(1));
        assert_eq!(args.case_insensitive, Some(true));
    }
}
