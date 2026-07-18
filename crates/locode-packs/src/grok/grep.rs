//! `grep` — a faithful port of Grok Build's ripgrep-backed search (`gb/grep/mod.rs`),
//! over `locode-host`'s resolved `rg` (ADR-0011).

use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{Host, rg_program};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok's `grep` tool.
pub(crate) struct GrokGrep {
    host: Arc<Host>,
}

impl GrokGrep {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `grep` (grok's real schema; several optional args — context lines, file
/// `type`, multiline, and `output_mode` — are dropped in v0 as reserved seams).
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
    #[serde(default)]
    #[schemars(description = "Case insensitive search (rg -i). Defaults to false.")]
    case_insensitive: Option<bool>,
}

/// The structured (report) face; the rg output is the prompt face.
#[derive(Debug, Serialize)]
pub(crate) struct GrepOutput {
    /// Whether any match was found (rg exit 0 vs 1).
    matched: bool,
    /// Whether the output was byte-capped.
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
        "Search file contents with a regular expression (ripgrep-backed). Respects .gitignore."
    }

    async fn run(&self, ctx: &ToolCtx, args: GrepArgs) -> Result<Self::Output, ToolError> {
        // rg's flag set (grok `grep/mod.rs:761-767`), then the pattern and path.
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
        if let Some(glob) = &args.glob {
            rg_args.push("--glob".into());
            rg_args.push(glob.clone());
        }
        // `--regexp` so a pattern beginning with `-` is never read as a flag.
        rg_args.push("--regexp".into());
        rg_args.push(args.pattern.clone());
        rg_args.push("--".into());
        rg_args.push(args.path.clone().unwrap_or_else(|| ".".into()));

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
            Some(0) => Ok(GrepOutput {
                matched: true,
                truncated: out.truncated,
                text: out.stdout,
            }),
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
