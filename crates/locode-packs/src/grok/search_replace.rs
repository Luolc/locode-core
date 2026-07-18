//! `search_replace` — a faithful port of Grok Build's exact-string edit
//! (`gb/search_replace/mod.rs`), over `locode-host`. This is grok's **only** edit/create
//! tool: there is no `write` tool (an empty `old_string` creates the file). No mtime
//! freshness (grok has none — faithful mimicry).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok's `search_replace` tool.
pub(crate) struct GrokSearchReplace {
    host: Arc<Host>,
}

impl GrokSearchReplace {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `search_replace` (grok's real schema; `${{…}}` template refs rendered).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SearchReplaceArgs {
    #[schemars(
        description = "The path to the file to modify. You can use either a relative path in the workspace or an absolute path."
    )]
    file_path: String,
    #[schemars(description = "The text to replace")]
    old_string: String,
    #[schemars(description = "The text to replace it with (must be different from old_string)")]
    new_string: String,
    #[serde(default)]
    #[schemars(description = "Replace all occurrences of old_string (default false)")]
    replace_all: bool,
}

/// The structured (report) face of an edit.
#[derive(Debug, Serialize)]
pub(crate) struct SearchReplaceOutput {
    /// The jail-resolved absolute path written.
    path: String,
    /// Whether the file was created (empty `old_string`) vs edited.
    created: bool,
    /// Number of occurrences replaced (0 on create).
    replacements: usize,
}

impl ToolOutput for SearchReplaceOutput {
    fn to_prompt_text(&self) -> String {
        if self.created {
            format!("Created {}.", self.path)
        } else {
            format!(
                "Replaced {} occurrence(s) in {}.",
                self.replacements, self.path
            )
        }
    }
}

#[async_trait]
impl Tool for GrokSearchReplace {
    type Args = SearchReplaceArgs;
    type Output = SearchReplaceOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        "Replace an exact, unique string in a file with new text (or create the file when old_string is empty). Set replace_all to change every occurrence."
    }

    async fn run(&self, ctx: &ToolCtx, args: SearchReplaceArgs) -> Result<Self::Output, ToolError> {
        // grok: reject a no-op edit up front (`search_replace/mod.rs:187`).
        if args.old_string == args.new_string {
            return Err(ToolError::Respond(
                "Old string and new string are the same".into(),
            ));
        }

        let path = Path::new(&args.file_path);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        let display = resolved.display().to_string();

        // Empty old_string ⇒ create the file (grok's `handle_new_file_creation`).
        if args.old_string.is_empty() {
            let occupied = matches!(
                self.host.read_file(&ctx.cwd, path).await,
                Ok(existing) if !existing.contents.is_empty()
            );
            if occupied {
                return Err(ToolError::Respond(format!(
                    "File already exists: {display}. To edit it, provide the exact text to replace in old_string."
                )));
            }
            self.host
                .write_file(&ctx.cwd, path, &args.new_string)
                .await
                .map_err(|e| ToolError::Respond(e.to_string()))?;
            return Ok(SearchReplaceOutput {
                path: display,
                created: true,
                replacements: 0,
            });
        }

        // Edit an existing file.
        let read = match self.host.read_file(&ctx.cwd, path).await {
            Ok(read) => read,
            Err(FsError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolError::Respond(format!(
                    "File not found: {}. Please check the path and try again.",
                    args.file_path
                )));
            }
            Err(e) => return Err(ToolError::Respond(e.to_string())),
        };
        let content = read.contents;

        let count = content.matches(&args.old_string).count();
        if count == 0 {
            return Err(ToolError::Respond(
                "The string to replace was not found in the file, use the read_file tool to see the correct string.".into(),
            ));
        }
        if count > 1 && !args.replace_all {
            return Err(ToolError::Respond(
                "The string to replace was found multiple times in the file. Use replace_all to replace all occurrences, or include more context to only edit one occurrence.".into(),
            ));
        }

        let new_content = if args.replace_all {
            content.replace(&args.old_string, &args.new_string)
        } else {
            content.replacen(&args.old_string, &args.new_string, 1)
        };
        self.host
            .write_file(&ctx.cwd, path, &new_content)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        Ok(SearchReplaceOutput {
            path: display,
            created: false,
            replacements: count,
        })
    }
}
