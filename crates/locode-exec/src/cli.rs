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
pub struct Cli {
    /// The task prompt. `-` or omitted reads the prompt from stdin.
    pub prompt: Option<String>,

    /// Workspace root / working directory (the path-jail root). Defaults to
    /// the current directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Harness pack selecting the toolset + system prompt.
    #[arg(long, value_enum, default_value_t = Harness::Grok)]
    pub harness: Harness,

    /// Provider wire schema (`mock` = keyless CI).
    #[arg(long, value_enum, env = "LOCODE_API_SCHEMA", default_value_t = ApiSchema::Anthropic)]
    pub api_schema: ApiSchema,

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
}

/// The registered harness packs (a closed set — clap validates and lists them).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Harness {
    /// Grok Build's toolset + system prompt (ADR-0012).
    Grok,
}

impl Harness {
    /// The pack name for `locode::resolve`.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Grok => "grok",
        }
    }
}

/// The provider wire schemas (ADR-0007: a schema, not a gateway).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ApiSchema {
    /// Anthropic Messages (native, OpenRouter, or any compatible gateway —
    /// selected by `LOCODE_BASE_URL`).
    Anthropic,
    /// The scripted mock provider — keyless CI runs.
    Mock,
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
