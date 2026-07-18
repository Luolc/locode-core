//! `run_terminal_cmd` — a faithful port of Grok Build's Bash tool (`gb/bash/mod.rs`),
//! foreground-only, over `locode-host`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use locode_host::{ExecRequest, Host};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok's default command timeout (2 minutes) and hard ceiling (5 minutes), in ms.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 300_000;

/// grok's `run_terminal_cmd` tool.
pub(crate) struct GrokRunTerminalCmd {
    host: Arc<Host>,
}

impl GrokRunTerminalCmd {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `run_terminal_cmd` (grok's real schema; `is_background` dropped in v0).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RunTerminalCmdArgs {
    #[schemars(description = "The bash command to run.")]
    command: String,
    #[schemars(
        description = "Optional timeout in milliseconds (max 300000). Default: 120000 (2 minutes)."
    )]
    #[serde(default)]
    timeout: Option<u64>,
    #[schemars(
        description = "One sentence explanation as to why this command needs to be run and how it contributes to the goal."
    )]
    #[allow(dead_code)] // required for schema fidelity; grok uses it for UX only
    description: String,
}

/// The structured (report) face of a terminal run. The full output is the prompt face.
#[derive(Debug, Serialize)]
pub(crate) struct RunTerminalCmdOutput {
    /// Process exit code; `-1` when the command was killed by a signal.
    exit_code: i64,
    /// Whether the command hit the hard timeout.
    timed_out: bool,
    /// Whether the output was byte-capped (older bytes dropped).
    truncated: bool,
    /// The combined stdout+stderr (prompt face only — kept out of the report).
    #[serde(skip)]
    output: String,
}

impl ToolOutput for RunTerminalCmdOutput {
    fn to_prompt_text(&self) -> String {
        // grok's header shape: `exit: {code}` (or `killed`) then the output.
        let status = if self.timed_out {
            "killed (timeout)".to_string()
        } else if self.exit_code < 0 {
            "killed".to_string()
        } else {
            self.exit_code.to_string()
        };
        let marker = if self.truncated {
            " [output truncated]"
        } else {
            ""
        };
        format!("exit: {status}{marker}\n{}", self.output)
    }
}

#[async_trait]
impl Tool for GrokRunTerminalCmd {
    type Args = RunTerminalCmdArgs;
    type Output = RunTerminalCmdOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        "Run a bash command in the workspace shell and return its combined output and exit code."
    }

    async fn run(
        &self,
        ctx: &ToolCtx,
        args: RunTerminalCmdArgs,
    ) -> Result<Self::Output, ToolError> {
        let timeout_ms = args
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let request = ExecRequest {
            command: args.command,
            cwd: ctx.cwd.clone(),
            timeout: Some(Duration::from_millis(timeout_ms)),
            env: Vec::new(),
        };
        // Only a spawn/capture failure is a (soft) error; a non-zero exit / timeout is a
        // successful capture the model reads and reacts to (ADR-0004).
        let out = self
            .host
            .exec(request, &ctx.cancel)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        Ok(RunTerminalCmdOutput {
            exit_code: out.exit_code.map_or(-1, i64::from),
            timed_out: out.timed_out,
            truncated: out.truncated,
            output: out.combined,
        })
    }
}
