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
#[command(
    name = "locode",
    version,
    about = "A coding agent: an interactive terminal UI, or a headless one-shot with `-p`."
)]
#[allow(clippy::struct_excessive_bools)] // CLI flags are naturally bools
pub struct Cli {
    /// The task prompt. With `-p` it is the headless task (`-` or omitted
    /// reads stdin); without `-p` it pre-fills the composer.
    pub prompt: Option<String>,

    /// Headless mode: run one turn to a machine-readable result and exit
    /// (no interactive UI).
    #[arg(short = 'p', long = "print")]
    pub print: bool,

    /// The harness pack (its toolset + system prompt) to run. Omitted uses the
    /// `harness` setting, then `claude`.
    #[arg(long, value_enum)]
    pub harness: Option<Harness>,

    /// The provider wire schema: `anthropic`, `openai-responses`, or `mock`
    /// (keyless), plus any custom-registered names. Omitted uses the
    /// `api_schema` setting, then `anthropic`.
    #[arg(long, env = "LOCODE_API_SCHEMA")]
    pub api_schema: Option<String>,

    /// Model id. Omitted uses the `model` setting, then the wire's default.
    /// There is no model environment variable — set it here or in settings.
    #[arg(long)]
    pub model: Option<String>,

    /// Extra settings layer: a path to a JSON file, or inline JSON. Highest
    /// precedence, above the user (`~/.locode`) and project settings.
    #[arg(long)]
    pub settings: Option<String>,

    /// Do not write a session trace for this run. `--continue`/`--resume` still
    /// read earlier sessions.
    #[arg(long)]
    pub no_session_persistence: bool,

    /// Continue the newest session started in this directory.
    #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
    pub continue_session: bool,

    /// Resume the session with this id (this directory's sessions first, then
    /// anywhere).
    #[arg(short = 'r', long = "resume", value_name = "SESSION_ID")]
    pub resume: Option<String>,

    /// Working directory (defaults to the current directory).
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Headless stdout artifact: `json` = one report; `text` = the final
    /// message; `stream-json` = the event stream. Ignored without `-p`.
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub output_format: OutputFormat,

    /// Hard ceiling on the number of model turns (unlimited when omitted).
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// Auto-allow every tool call (no approval prompts) and lift the path jail
    /// (full filesystem access). Use with care.
    #[arg(long = "dangerously-skip-permissions", alias = "yolo")]
    pub dangerously_skip_permissions: bool,

    /// Strip the harness's identity sentence from the system prompt.
    #[arg(long)]
    pub strip_identity: bool,

    // Anthropic rejects non-streaming requests that may exceed ~10 min, so
    // `--stream` is required for unbounded headless output (ADR-0021).
    /// Stream the reply as it is generated in `-p` headless mode (the
    /// interactive UI always streams). Needed for very long outputs. The
    /// `stream-json` trace stays whole-message.
    #[arg(long)]
    pub stream: bool,

    /// Skip loading project instructions (`AGENTS.md`). Discovery is otherwise
    /// on by default.
    #[arg(long)]
    pub no_project_instructions: bool,
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
            model: self.model.clone(),
            no_session_persistence: self.no_session_persistence,
            settings: self.settings.clone(),
            continue_session: self.continue_session,
            resume: self.resume.clone(),
            max_turns: self.max_turns,
            output_format: self.output_format,
            dangerously_skip_permissions: self.dangerously_skip_permissions,
            strip_identity: self.strip_identity,
            stream: self.stream,
            no_project_instructions: self.no_project_instructions,
        }
    }
}
