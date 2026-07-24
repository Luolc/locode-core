//! clap surface (plan §3.1 + the positional-prompt addendum).
//!
//! The prompt is the **positional** argument — the field convention (Claude
//! Code's `-p/--print` is a mode flag with the prompt positional; `codex exec
//! "…"` likewise), and `locode-exec` is always headless so it needs no mode
//! flag at all. `-` or omitted → read stdin (Codex's convention).

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Minimal headless runner for the locode engine: one JSON report on stdout
/// (ADR-0009), diagnostics on stderr, exit code from the run's status.
#[derive(Parser, Debug)]
#[command(name = "locode-exec", version, about)]
#[allow(clippy::struct_excessive_bools)] // CLI flags are naturally bools
pub struct Cli {
    /// The task prompt. `-` or omitted reads the prompt from stdin.
    pub prompt: Option<String>,

    /// Workspace root / working directory (the path-jail root). Defaults to
    /// the current directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Harness pack selecting the toolset + system prompt. Omitted ⇒ the
    /// settings `harness` default (ADR-0024 §1.4), else `grok`.
    #[arg(long, value_enum)]
    pub harness: Option<Harness>,

    /// Provider wire schema: `anthropic`, `openai-responses`, or `mock`
    /// (keyless CI) — plus any custom providers the binary registered
    /// (ADR-0015). Unknown names fail pre-run listing the available set.
    /// Omitted ⇒ the settings `api_schema` default (ADR-0024 §1.4 — the
    /// project layers may not set it), else `anthropic`.
    #[arg(long, env = "LOCODE_API_SCHEMA")]
    pub api_schema: Option<String>,

    /// Extra settings layer: a path to a JSON file, or inline JSON (highest-
    /// precedence settings layer, ADR-0024 §1.2).
    #[arg(long)]
    pub settings: Option<String>,

    /// Hard ceiling on sample→dispatch turns. **Unlimited when omitted**
    /// (ADR-0005 amendment — no studied harness caps turns by default).
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// stdout contract: `json` = one Report; `stream-json` = Event JSONL;
    /// `text` = the final message.
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub output_format: OutputFormat,

    /// Disable the path jail (`PathPolicy::Unrestricted`). The shell's
    /// timeout/output caps stay on (ADR-0008 amendment).
    #[arg(long, visible_alias = "yolo")]
    pub dangerously_skip_permissions: bool,

    /// Strip harness identity sentences from the rendered system prompt
    /// (A/B contamination control; default = faithful reproduction).
    #[arg(long)]
    pub strip_identity: bool,

    /// Stream the model turn (SSE) instead of one buffered call (ADR-0021).
    /// Needed for **unbounded output** — Anthropic rejects non-streaming
    /// requests that may exceed ~10 min. The `stream-json` trace stays
    /// whole-message: token deltas are assembled but **not** written to it.
    #[arg(long)]
    pub stream: bool,

    /// Skip project-instruction loading (`AGENTS.md`) — the `--bare`-style disable
    /// (ADR-0023). Auto-discovery is otherwise on by default.
    #[arg(long)]
    pub no_project_instructions: bool,
}

/// The registered harness packs (a closed set — clap validates and lists them).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Harness {
    /// Grok Build's toolset + system prompt (ADR-0012).
    Grok,
    /// Claude Code's toolset + system prompt (ADR-0012, ADR-0023).
    Claude,
    /// Codex CLI's toolset + system prompt (ADR-0012, ADR-0023).
    Codex,
}

impl Harness {
    /// The pack name for `locode_core::resolve`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Grok => "grok",
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }
}

/// The stdout artifact selector (ADR-0014; Claude Code's three-way shape).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// One `Report` JSON object (the default).
    Json,
    /// The final assistant message only.
    Text,
    /// The live JSONL `Event` stream (`init` → `message`… → `result`).
    StreamJson,
}
