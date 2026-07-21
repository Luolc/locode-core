//! The interactive app's CLI — the same selection surface as `locode-exec`
//! (SPEC-TUI §Config/CLI plumbing), minus headless-only flags.

use std::path::PathBuf;

use clap::Parser;

/// `locode` — drive a locode session interactively.
#[derive(Debug, Clone, Parser)]
#[command(name = "locode", version, about)]
pub struct Cli {
    /// The harness pack to run (currently: grok).
    #[arg(long, default_value = "grok")]
    pub harness: String,

    /// The provider wire schema (anthropic | openai-responses | mock, plus
    /// any custom-registered names).
    #[arg(long, env = "LOCODE_API_SCHEMA", default_value = "anthropic")]
    pub api_schema: String,

    /// Working directory (defaults to the current directory); canonicalized
    /// once and shared by the jail, engine, and pack.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Auto-allow every tool call (no approval prompts) and lift the path
    /// jail — the harnesses' full-access behavior.
    #[arg(long = "dangerously-skip-permissions", alias = "yolo")]
    pub dangerously_skip_permissions: bool,

    /// Strip the harness identity sentence from the pack prompt.
    #[arg(long)]
    pub strip_identity: bool,
}
