//! The unified `locode` CLI: the interactive app by default, and a headless
//! one-shot under `-p`/`--print` (Claude-Code's shape — `main_with` detects
//! `-p` and dispatches; Task 28). Shared selection flags (`--harness`,
//! `--api-schema`, `--cwd`, `--yolo`, `--strip-identity`) apply to both modes;
//! `--output-format`/`--max-turns` are headless-only.

use std::path::PathBuf;

use clap::Parser;
use locode_exec::{Harness, OutputFormat};

/// `locode` — an interactive coding agent (default), or a headless one-shot
/// with `-p`.
#[derive(Debug, Clone, Parser)]
#[command(name = "locode", version, about)]
#[allow(clippy::struct_excessive_bools)] // CLI flags are naturally bools
pub struct Cli {
    /// The task prompt. With `-p` it is the headless task (`-` or omitted
    /// reads stdin); without `-p` it pre-fills the composer.
    pub prompt: Option<String>,

    /// Headless mode: run one turn to a machine-readable result and exit
    /// (no TUI) — Claude Code's `-p`/`--print`.
    #[arg(short = 'p', long = "print")]
    pub print: bool,

    /// The harness pack to run.
    #[arg(long, value_enum, default_value_t = Harness::Grok)]
    pub harness: Harness,

    /// The provider wire schema (`anthropic` | `openai-responses` | `mock`,
    /// plus any custom-registered names).
    #[arg(long, env = "LOCODE_API_SCHEMA", default_value = "anthropic")]
    pub api_schema: String,

    /// Working directory (defaults to the current directory); canonicalized
    /// once and shared by the jail, engine, and pack.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Headless stdout artifact: `json` = one Report; `text` = the final
    /// message; `stream-json` = the Event JSONL. Ignored without `-p`.
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub output_format: OutputFormat,

    /// Hard ceiling on sample→dispatch turns (unlimited when omitted).
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// Auto-allow every tool call (no approval prompts) and lift the path
    /// jail — the harnesses' full-access behavior.
    #[arg(long = "dangerously-skip-permissions", alias = "yolo")]
    pub dangerously_skip_permissions: bool,

    /// Strip the harness identity sentence from the pack prompt.
    #[arg(long)]
    pub strip_identity: bool,

    /// Stream the model turn (SSE) in `-p` headless mode (ADR-0021). The
    /// interactive TUI always streams; this opts the headless one-shot in —
    /// needed for **unbounded output**, since Anthropic rejects non-streaming
    /// requests that may exceed ~10 min. The `stream-json` trace stays
    /// whole-message (token deltas are dropped).
    #[arg(long)]
    pub stream: bool,
}

impl Cli {
    /// The headless [`locode_exec::Cli`] for `-p` mode — the shared fields map
    /// straight across (the two CLIs deliberately share `Harness` /
    /// `OutputFormat`).
    #[must_use]
    pub fn to_headless(&self) -> locode_exec::Cli {
        locode_exec::Cli {
            prompt: self.prompt.clone(),
            cwd: self.cwd.clone(),
            harness: self.harness,
            api_schema: self.api_schema.clone(),
            max_turns: self.max_turns,
            output_format: self.output_format,
            dangerously_skip_permissions: self.dangerously_skip_permissions,
            strip_identity: self.strip_identity,
            stream: self.stream,
        }
    }
}
