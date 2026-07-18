//! `read_file` — a faithful port of Grok Build's `ReadFile` tool (`gb/read_file/mod.rs`),
//! text-only, over `locode-host`. No mtime-freshness store (grok has none — faithful
//! mimicry; the interview confirmed we do not add one).

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::Host;
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok's default line cap and token cap for a single read (`gb/read_file/mod.rs:55-56`).
const MAX_LINES: usize = 1_000;
const MAX_TOKENS: usize = 25_000;

/// grok's `read_file` tool.
pub(crate) struct GrokReadFile {
    host: Arc<Host>,
}

impl GrokReadFile {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `read_file` (grok's real schema; `pages`/`format` dropped in v0).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReadFileArgs {
    #[serde(rename = "target_file")]
    #[schemars(
        description = "The path of the file to read. You can use either a relative path in the workspace or an absolute path. If an absolute path is provided, it will be preserved as is."
    )]
    path: String,
    #[serde(default)]
    #[schemars(
        description = "The line number to start reading from. Only provide if the file is too large to read at once."
    )]
    offset: Option<i64>,
    #[serde(default)]
    #[schemars(
        description = "The number of lines to read. Only provide if the file is too large to read at once."
    )]
    limit: Option<usize>,
}

/// The structured (report) face; the numbered body is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct ReadFileOutput {
    /// The jail-resolved absolute path.
    path: String,
    /// Total lines in the file.
    lines: usize,
    /// Whether the returned window is a subset of the file.
    truncated: bool,
    /// The `LINE_NUMBER→CONTENT` projection (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for ReadFileOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

#[async_trait]
impl Tool for GrokReadFile {
    type Args = ReadFileArgs;
    type Output = ReadFileOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        "Read a text file from the workspace, returned as numbered lines (`N→content`)."
    }

    async fn run(&self, ctx: &ToolCtx, args: ReadFileArgs) -> Result<Self::Output, ToolError> {
        let start = match args.offset {
            Some(offset) if offset < 0 => {
                return Err(ToolError::Respond(
                    "negative offset is not supported in v0".into(),
                ));
            }
            Some(offset) => usize::try_from(offset).unwrap_or(1).max(1),
            None => 1,
        };
        let limit = args.limit.unwrap_or(MAX_LINES).min(MAX_LINES);

        let path = Path::new(&args.path);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        let read = self
            .host
            .read_file(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        let all: Vec<&str> = read.contents.lines().collect();
        let total = all.len();
        let start_idx = start.saturating_sub(1);
        let end_idx = start_idx.saturating_add(limit).min(total);

        let mut body = String::new();
        for (offset, line) in all
            .get(start_idx..end_idx)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let number = start + offset;
            let _ = writeln!(body, "{number}→{line}");
        }

        // grok's token guard: a too-large read is a soft error nudging a narrower range.
        if body.len() / 4 > MAX_TOKENS {
            return Err(ToolError::Respond(format!(
                "file is too large (~{} tokens > {MAX_TOKENS}); read a narrower range with offset/limit, or use grep",
                body.len() / 4
            )));
        }

        let truncated = end_idx < total || start_idx > 0;
        Ok(ReadFileOutput {
            path: resolved.display().to_string(),
            lines: total,
            truncated,
            body,
        })
    }
}
