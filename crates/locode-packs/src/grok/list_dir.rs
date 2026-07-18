//! `list_dir` — a port of Grok Build's directory tool (`gb/list_dir/mod.rs`), a
//! **self-implemented recursive walk** over `locode-host`'s jailed `read_dir` (ADR-0011
//! amendment: ported packs mimic their harness's real dir tool, not an rg-glob).
//!
//! Simplifications vs grok (flagged): no gitignore filtering and a simpler budget than
//! grok's seed+deep-walk; the char cap matches grok's `DEFAULT_MAX_OUTPUT_CHARS`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::Host;
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok's `DEFAULT_MAX_OUTPUT_CHARS` (`gb/list_dir/mod.rs:90`) and a walk-item ceiling.
const MAX_OUTPUT_CHARS: usize = 10_000;
const MAX_ITEMS: usize = 100_000;

/// grok's `list_dir` tool.
pub(crate) struct GrokListDir {
    host: Arc<Host>,
}

impl GrokListDir {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `list_dir` (grok's real schema).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListDirArgs {
    #[serde(rename = "target_directory")]
    #[schemars(
        description = "Path to directory to list contents of, relative to the workspace root or absolute."
    )]
    directory: String,
}

/// The structured (report) face; the tree listing is the prompt face.
#[derive(Debug, Serialize)]
pub(crate) struct ListDirOutput {
    /// The jail-resolved absolute directory.
    path: String,
    /// Whether the listing was truncated by the item/char budget.
    truncated: bool,
    #[serde(skip)]
    listing: String,
}

impl ToolOutput for ListDirOutput {
    fn to_prompt_text(&self) -> String {
        self.listing.clone()
    }
}

struct Budget {
    items: usize,
    truncated: bool,
}

/// Depth-first walk, appending an indented tree. Boxed for async recursion.
fn walk<'a>(
    host: &'a Host,
    cwd: &'a Path,
    rel: PathBuf,
    depth: usize,
    out: &'a mut String,
    budget: &'a mut Budget,
) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if budget.truncated {
            return;
        }
        let Ok(mut entries) = host.read_dir(cwd, &rel).await else {
            return;
        };
        entries.sort_by(|a, b| (b.is_dir, &a.name).cmp(&(a.is_dir, &b.name)));
        for entry in entries {
            if budget.items >= MAX_ITEMS || out.len() >= MAX_OUTPUT_CHARS {
                budget.truncated = true;
                return;
            }
            budget.items += 1;
            let indent = "  ".repeat(depth);
            let slash = if entry.is_dir { "/" } else { "" };
            let _ = writeln!(out, "{indent}{}{slash}", entry.name);
            if entry.is_dir {
                walk(host, cwd, rel.join(&entry.name), depth + 1, out, budget).await;
            }
        }
    })
}

#[async_trait]
impl Tool for GrokListDir {
    type Args = ListDirArgs;
    type Output = ListDirOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Glob
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        "List the contents of a directory as an indented tree."
    }

    async fn run(&self, ctx: &ToolCtx, args: ListDirArgs) -> Result<Self::Output, ToolError> {
        let dir = Path::new(&args.directory);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, dir)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        // Fail early on a non-directory / missing path (soft).
        self.host
            .read_dir(&ctx.cwd, dir)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        let mut listing = String::new();
        let mut budget = Budget {
            items: 0,
            truncated: false,
        };
        walk(
            &self.host,
            &ctx.cwd,
            dir.to_path_buf(),
            0,
            &mut listing,
            &mut budget,
        )
        .await;

        Ok(ListDirOutput {
            path: resolved.display().to_string(),
            truncated: budget.truncated,
            listing,
        })
    }
}
