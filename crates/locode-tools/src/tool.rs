//! The typed [`Tool`] contract, its [`ToolKind`] tag, and the [`ToolOutput`] face
//! for model-facing text (ADR-0003).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ctx::ToolCtx;
use crate::error::ToolError;

/// The model-facing rendering of a tool's output.
///
/// A tool result has two faces (ADR-0003): the structured [`Tool::Output`] value
/// goes into the report's `tool_calls[]`, while `to_prompt_text()` renders the
/// text the model sees in history. They are independent renderings of one call —
/// e.g. a read tool reports `{path, lines, truncated}` but shows the file body.
pub trait ToolOutput {
    /// Render this output as the text the model reads back in the transcript.
    fn to_prompt_text(&self) -> String;
}

/// A canonical classification tag for a tool, used only to **align comparable
/// tools across harness packs** for A/B analysis (ADR-0003) — it is *not* the
/// wire name (that is assigned per-pack at registration) and *not* the Rust type
/// name.
///
/// Closed for v0 but carries an [`ToolKind::Other`] catch-all (via
/// `#[serde(other)]`) so tools without a cross-pack analog — MCP tools, and
/// harness-unique specials — still classify. Grok Build's real `ToolKind` has the
/// same shape (a fixed set plus `Other`); we start small and grow the canonical
/// set as packs need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Runs a shell command.
    Shell,
    /// Reads a file's contents.
    Read,
    /// Creates or overwrites a file.
    Write,
    /// Edits part of an existing file (e.g. exact-string replace).
    Edit,
    /// Expands a path/glob pattern to a list of files.
    Glob,
    /// Searches file contents (e.g. ripgrep).
    Grep,
    /// No cross-pack analog: MCP tools and harness-unique specials.
    #[serde(other)]
    Other,
}

impl ToolKind {
    /// The stable `snake_case` key for this kind (what lands in the report record).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Shell => "shell",
            ToolKind::Read => "read",
            ToolKind::Write => "write",
            ToolKind::Edit => "edit",
            ToolKind::Glob => "glob",
            ToolKind::Grep => "grep",
            ToolKind::Other => "other",
        }
    }
}

/// A tool authored against concrete types; the wire schema is *derived* from
/// [`Tool::Args`], never hand-written (ADR-0003).
///
/// The tool has **no name of its own** — its model-facing wire name is assigned by
/// the harness pack when it registers the tool into a [`crate::Registry`]
/// (ADR-0006/0012). Author tools with concrete `Args`/`Output` types; the registry
/// erases them to `Box<dyn `[`DynTool`](crate::DynTool)`>` at its boundary.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The deserializable, schema-deriving argument type the model must supply.
    type Args: DeserializeOwned + JsonSchema + Send;
    /// The structured output; also rendered to model-facing text via [`ToolOutput`].
    type Output: Serialize + ToolOutput + Send;

    /// The cross-pack classification tag (see [`ToolKind`]).
    fn kind(&self) -> ToolKind;

    /// A human-readable description offered to the model alongside the schema.
    fn description(&self) -> &str;

    /// The JSON Schema for [`Tool::Args`], derived via `schemars` (ADR-0003).
    ///
    /// The default derives it from the type and should rarely be overridden.
    #[must_use]
    fn parameters_schema(&self) -> Value {
        // `schema_for!` yields a `schemars::Schema`, which is `Serialize`;
        // converting an already-JSON schema to a `Value` cannot realistically
        // fail, but we avoid `unwrap`/`expect` and degrade to an empty object.
        serde_json::to_value(schemars::schema_for!(Self::Args))
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
    }

    /// Execute the call.
    ///
    /// # Errors
    /// Returns [`ToolError::Respond`] for any recoverable failure (the model
    /// retries) and [`ToolError::Fatal`] only when the transcript is
    /// unrecoverable (ADR-0004).
    async fn run(&self, ctx: &ToolCtx, args: Self::Args) -> Result<Self::Output, ToolError>;
}
