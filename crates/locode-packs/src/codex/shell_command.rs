//! `shell_command` — a faithful port of Codex CLI's non-PTY shell tool.
//!
//! **Deprecated substitution (see `docs/codex-pack-dev-process.md` D2):** at commit
//! `f201c30c` Codex's *default* shell on mac/Linux is **unified exec**
//! (`exec_command` + `write_stdin` — a stateful PTY/session tool), and
//! `shell_command` is hidden. We expose `shell_command` anyway — it is gpt-5.6-sol's
//! own declared `shell_type`, i.e. the visible tool with the unified-exec feature
//! disabled (a real Codex config) — because session/background infra is out of
//! current scope. **Switch to unified exec when background support (P0.5) lands.**
//!
//! Fidelity notes (Codex submodule `f201c30c`):
//! - **Schema** (`core/src/tools/handlers/shell_spec.rs` `create_shell_command_tool`):
//!   `command` + `workdir` + `timeout_ms` + `login`; `additionalProperties:false`,
//!   only `command` required. The approval params (`sandbox_permissions`/
//!   `justification`/`prefix_rule`) are **dropped** (D8, no interactive permission
//!   flow — the pack's top faithfulness gap). Type-strict (repo policy).
//! - **Description** (non-Windows branch, verbatim) in `descriptions/shell_command.md`.
//! - **Timeout** (`core/src/exec.rs:58`): default 10000 ms, **no max clamp** (our host
//!   still caps at its 10-min ceiling — a documented host-safety divergence). Kill on
//!   timeout → exit code **124** (`EXEC_TIMEOUT_EXIT_CODE`).
//! - **Output framing** (`core/src/tools/mod.rs:78-100` `format_exec_output_for_model`,
//!   the model-facing path per `events.rs:375`): `Exit code: N` / `Wall time: S
//!   seconds` / [`Total output lines: N` only when truncated] / `Output:` / body,
//!   joined by `\n`. Timeout body is prefixed `command timed out after {ms}
//!   milliseconds\n{output}` (`mod.rs:116`). A non-zero exit / timeout is a
//!   **successful capture** (Codex sets `success:true`) — only a spawn failure is a
//!   soft error.
//! - **Truncation** (gpt-5.6-sol `truncation_policy = {tokens: 10000}`): middle
//!   truncation at a 10000-token budget; `APPROX_BYTES_PER_TOKEN = 4`
//!   (`utils/string/src/truncate.rs`) → ~40000-byte budget; marker
//!   `…{removed} tokens truncated…`. Approximated byte-safely (D9).
//! - **Output ordering:** the host's `combined` is stdout then stderr concatenated,
//!   not real-time interleaved (a documented host limitation).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use locode_host::{ExecRequest, Host, ShellSpec};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Codex's default shell timeout, ms (`exec.rs:58`).
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// gpt-5.6-sol's output token budget (`models.json` `truncation_policy`).
const OUTPUT_TOKEN_BUDGET: usize = 10_000;
/// Codex's token↔byte estimate (`utils/string/src/truncate.rs`).
const APPROX_BYTES_PER_TOKEN: usize = 4;

/// Codex's `shell_command` tool.
pub(crate) struct CodexShellCommand {
    host: Arc<Host>,
}

impl CodexShellCommand {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `shell_command` (`shell_spec.rs:157-225`; approval params dropped, D8).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShellCommandArgs {
    #[schemars(description = "Shell script to run in the user's default shell.")]
    command: String,
    #[schemars(description = "Working directory for the command. Defaults to the turn cwd.")]
    #[serde(default)]
    workdir: Option<String>,
    #[schemars(description = "Maximum command runtime. Defaults to 10000 ms.")]
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[schemars(
        description = "True runs with login shell semantics; false disables them. Defaults to true."
    )]
    #[serde(default)]
    login: Option<bool>,
}

/// The structured (report) face; the framed body is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct ShellCommandOutput {
    /// Exit code (124 on timeout, -1 on signal, else the process code).
    exit_code: i64,
    /// Whether the command hit the timeout.
    timed_out: bool,
    /// Whether the output body was middle-truncated.
    truncated: bool,
    /// The framed prompt body (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for ShellCommandOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

/// Codex's `truncate_middle_with_token_budget` (approximated, byte-safe): keep the
/// head and tail around a `…{removed} tokens truncated…` marker at ~`budget` bytes.
fn truncate_middle(content: &str) -> (String, bool) {
    let budget = OUTPUT_TOKEN_BUDGET * APPROX_BYTES_PER_TOKEN;
    if content.len() <= budget {
        return (content.to_string(), false);
    }
    let removed_tokens = content
        .len()
        .saturating_sub(budget)
        .div_ceil(APPROX_BYTES_PER_TOKEN);
    let marker = format!("…{removed_tokens} tokens truncated…");
    let keep = budget.saturating_sub(marker.len());
    let head_target = keep / 2;
    let tail_target = keep - head_target;
    // Byte-safe head/tail at char boundaries.
    let head_end = (0..=head_target.min(content.len()))
        .rev()
        .find(|&i| content.is_char_boundary(i))
        .unwrap_or(0);
    let tail_from = content.len().saturating_sub(tail_target);
    let tail_start = (tail_from..=content.len())
        .find(|&i| content.is_char_boundary(i))
        .unwrap_or(content.len());
    (
        format!("{}{marker}{}", &content[..head_end], &content[tail_start..]),
        true,
    )
}

#[async_trait]
impl Tool for CodexShellCommand {
    type Args = ShellCommandArgs;
    type Output = ShellCommandOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        include_str!("descriptions/shell_command.md")
    }

    async fn run(&self, ctx: &ToolCtx, args: ShellCommandArgs) -> Result<Self::Output, ToolError> {
        // `workdir` defaults to the turn cwd; resolve it in the jail when provided.
        let cwd = match args.workdir.as_deref().filter(|w| !w.is_empty()) {
            Some(w) => self
                .host
                .resolve_in_jail(&ctx.cwd, std::path::Path::new(w))
                .await
                .map_err(|e| ToolError::Respond(e.to_string()))?,
            None => ctx.cwd.clone(),
        };
        // No max clamp in codex (the host still applies its own ceiling).
        let timeout_ms = args
            .timeout_ms
            .filter(|&t| t > 0)
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        let request = ExecRequest {
            command: args.command,
            cwd,
            timeout: Some(Duration::from_millis(timeout_ms)),
            env: Vec::new(),
            // "the user's default shell" with login semantics (default true).
            shell: Some(ShellSpec {
                detect_program: true,
                login_arg: args.login.unwrap_or(true),
                login_path_probe: false,
            }),
            front_back: None,
        };
        let out = self
            .host
            .exec(request, &ctx.cancel)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        let timed_out = out.timed_out;
        // `build_content_with_timeout` (`mod.rs:116`).
        let content = if timed_out {
            format!(
                "command timed out after {} milliseconds\n{}",
                timeout_ms, out.combined
            )
        } else {
            out.combined
        };
        let total_lines = content.lines().count();
        let (formatted, truncated) = truncate_middle(&content);

        // Exit code: 124 on timeout (`EXEC_TIMEOUT_EXIT_CODE`), else the process code.
        let exit_code: i64 = if timed_out {
            124
        } else {
            out.exit_code.map_or(-1, i64::from)
        };
        // round(secs_f32 * 10) / 10 (`mod.rs:83`).
        #[allow(clippy::cast_possible_truncation)] // display-only, codex's exact math
        let duration_seconds = ((out.duration.as_secs_f32()) * 10.0).round() / 10.0;

        let mut sections = vec![
            format!("Exit code: {exit_code}"),
            format!("Wall time: {duration_seconds} seconds"),
        ];
        if truncated {
            sections.push(format!("Total output lines: {total_lines}"));
        }
        sections.push("Output:".to_string());
        sections.push(formatted);

        Ok(ShellCommandOutput {
            exit_code,
            timed_out,
            truncated,
            body: sections.join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_is_the_pinned_shell_command_md() {
        let desc = include_str!("descriptions/shell_command.md");
        // sha256 b3ee7fb07780e61dbaefc683dd30aee666d1145984767d63b2906e08d01a5cfa.
        assert_eq!(desc.len(), 161, "shell_command.md byte length changed");
        assert!(desc.starts_with("Runs a shell command and returns its output."));
        assert!(
            desc.trim_end()
                .ends_with("Do not use `cd` unless absolutely necessary.")
        );
    }

    #[test]
    fn truncate_middle_keeps_head_and_tail() {
        let big = "x".repeat(OUTPUT_TOKEN_BUDGET * APPROX_BYTES_PER_TOKEN + 5_000);
        let (out, truncated) = truncate_middle(&big);
        assert!(truncated);
        assert!(out.contains("tokens truncated…"));
        assert!(out.len() < big.len());
        let (small, t2) = truncate_middle("short");
        assert!(!t2);
        assert_eq!(small, "short");
    }
}
