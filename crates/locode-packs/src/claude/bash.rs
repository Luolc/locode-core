//! `Bash` — a faithful port of Claude Code's `BashTool`, headless configuration
//! (background tasks + sandbox disabled).
//!
//! Fidelity notes (Claude Code submodule commit `6a25909`):
//! - **Schema** (`BashTool.tsx:227-260`, `z.strictObject`): `command` +
//!   `timeout` + `description`. `run_in_background` is `.omit()`ed when
//!   `isBackgroundTasksDisabled` (our config, `:254-256`); `_simulatedSedEdit`
//!   is *always* `.omit()`ed (`:249-259`) and `dangerouslyDisableSandbox` is a
//!   sandbox knob we have no seam for — dropping all three is faithful.
//!   `#[serde(deny_unknown_fields)]` mirrors `z.strictObject`
//!   (`additionalProperties:false`). `timeout` is type-strict (no string
//!   coercion — repo standing policy; CC's `semanticNumber` coerces).
//! - **Description** (`BashTool.tsx:431-433` → `getSimplePrompt()`,
//!   `prompt.ts:275-370`) rendered for our config: `hasEmbeddedSearchTools=false`
//!   (Glob/Grep in the pool), `MONITOR_TOOL=off`, background disabled (no
//!   background note), sandbox off (empty section), external user + git
//!   instructions on. Stored verbatim in `descriptions/bash.md` (pinned below).
//!   Gaps (D8): the sleep bullets + git section mention `run_in_background` /
//!   `TodoWrite` / `Agent` unconditionally in CC — kept verbatim. **Attribution
//!   removed (truth-first, AGENTS.md "Fidelity vs. truth"):** CC's git section
//!   embeds the *running model's* name in `Co-Authored-By: {model}` /
//!   `Generated with [Claude Code]`, which a static `&str` `description()` (built
//!   at `register()`, no `PackContext`) cannot render truthfully — a hardcoded
//!   name would lie about the model in every commit trace. We render CC's real
//!   `includeCoAuthoredBy: false` branch instead (attribution off), which drops
//!   the line honestly. The static-vs-dynamic `description()` interface question
//!   is researched + deferred in `docs/research/tool-description-interface.md`.
//! - **Result text** (`mapToolResultToToolResultBlockParam`, `BashTool.tsx:555-624`):
//!   CC runs a merged fd (stderr in stdout, `:692,717`), strips leading
//!   whitespace-only lines + `trimEnd`, appends `Exit code N` on a non-zero exit
//!   (`:697-699`), and sets `is_error` only when interrupted (`:621`). We render
//!   the host's front/back capture over the merged stream the same way (plan
//!   §4.1). The 30k `maxResultSizeChars` cap (`:424`) rides the `FrontBackSpec`
//!   char budget (front + marker + back); the engine dispatch-door belt
//!   (`MODEL_OUTPUT_BUDGET` = 30k, ADR-0008) truncates on top and supplies the
//!   visible marker when both layers fire. CC's persist-to-disk preview for very
//!   large output is a P1 gap (no persist seam in the tool); the host's
//!   `combined` is stdout then stderr *concatenated*, not real-time interleaved
//!   (a documented ordering gap).
//! - **Shell / guardrails:** `Host::exec` (grok's `ShellSpec`, detected bash/zsh
//!   with login PATH) approximates CC's "initialized from the user's profile".
//!   CC's permission + sandbox pipeline and persistent shell session are out of scope
//!   (our `PathPolicy` substitution, ADR-0008; per-call `bash -lc`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use locode_host::{ExecRequest, FrontBackSpec, Host, ShellSpec};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// CC's Bash timeout defaults, ms (`utils/timeouts.ts:2-3`).
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// CC's `maxResultSizeChars` for Bash (`BashTool.tsx:424`) — the retained
/// front+back char budget for the merged output.
const MAX_RESULT_CHARS: usize = 30_000;
/// The middle-truncation marker (our approximation of CC's persist-to-disk
/// preview — no persist seam in the tool; a documented P1 gap).
const TRUNCATION_SEPARATOR: &str = "\n\n... [output truncated] ...\n\n";

/// Claude Code's `Bash` tool.
pub(crate) struct ClaudeBash {
    host: Arc<Host>,
}

impl ClaudeBash {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `Bash` — CC's model-facing schema in the background-disabled
/// config (`command` + `timeout` + `description`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BashArgs {
    #[schemars(description = "The command to execute")]
    command: String,
    #[schemars(description = "Optional timeout in milliseconds (max 600000)")]
    #[serde(default)]
    timeout: Option<u64>,
    #[schemars(
        description = r#"Clear, concise description of what this command does in active voice. Never use words like "complex" or "risk" in the description - just describe what it does.

For simple commands (git, npm, standard CLI tools), keep it brief (5-10 words):
- ls → "List files in current directory"
- git status → "Show working tree status"
- npm install → "Install package dependencies"

For commands that are harder to parse at a glance (piped commands, obscure flags, etc.), add enough context to clarify what it does:
- find . -name "*.tmp" -exec rm {} \; → "Find and delete all .tmp files recursively"
- git reset --hard origin/main → "Discard all local changes and match remote main"
- curl -s url | jq '.data[]' → "Fetch JSON from URL and extract data array elements""#
    )]
    #[serde(default)]
    #[allow(dead_code)] // required for schema fidelity; CC uses it for UX only
    description: Option<String>,
}

/// The structured (report) face; the rendered body is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct BashOutput {
    /// Process exit code; `None` when killed by a signal.
    exit_code: Option<i32>,
    /// Whether the command was interrupted (timed out or cancelled).
    interrupted: bool,
    /// Whether the rendered body was middle-truncated at the 30k cap.
    truncated: bool,
    /// The rendered prompt body (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for BashOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

/// CC's `processedStdout` shaping (`BashTool.tsx:580-586`):
/// `stdout.replace(/^(\s*\n)+/, '').trimEnd()` — strip the leading run of
/// whitespace-only lines, then trailing whitespace. Internal content (including
/// `\r\n`) is preserved byte-for-byte.
fn process_output(combined: &str) -> String {
    // Advance past the longest leading prefix of whitespace that ends in a
    // newline (the `^(\s*\n)+` match). `cut` only ever lands after a single-byte
    // `\n` or at a non-whitespace byte, so it stays on a char boundary.
    let bytes = combined.as_bytes();
    let mut cut = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            i += 1;
            cut = i;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else {
            break;
        }
    }
    combined[cut..].trim_end().to_string()
}

#[async_trait]
impl Tool for ClaudeBash {
    type Args = BashArgs;
    type Output = BashOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        include_str!("descriptions/bash.md")
    }

    async fn run(&self, ctx: &ToolCtx, args: BashArgs) -> Result<Self::Output, ToolError> {
        let timeout_ms = args
            .timeout
            .filter(|&t| t > 0)
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let request = ExecRequest {
            command: args.command,
            cwd: ctx.cwd.clone(),
            timeout: Some(Duration::from_millis(timeout_ms)),
            env: Vec::new(),
            // Detect the user's bash/zsh + inject login PATH — approximates CC's
            // "the shell environment is initialized from the user's profile".
            shell: Some(ShellSpec {
                detect_program: true,
                login_arg: false,
                login_path_probe: true,
            }),
            // Retain front+back within CC's 30k `maxResultSizeChars` over the
            // merged stream (CC runs a merged fd — stderr in stdout).
            front_back: Some(FrontBackSpec {
                char_budget: MAX_RESULT_CHARS,
                spill: false,
            }),
        };
        // Only a spawn/capture failure is a (soft) error; a non-zero exit is a
        // successful capture the model reads (ADR-0004; CC is_error=interrupted).
        let out = self
            .host
            .exec(request, &ctx.cancel)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        let interrupted = out.timed_out || out.cancelled;
        let capture = out
            .front_back
            .ok_or_else(|| ToolError::Respond("internal: combined capture missing".to_string()))?;
        let truncated = capture.back.is_some();
        let raw = match &capture.back {
            Some(back) => format!("{}{TRUNCATION_SEPARATOR}{back}", capture.front),
            None => capture.front.clone(),
        };
        let mut body = process_output(&raw);
        // CC appends `Exit code N` for a non-zero exit that wasn't interrupted
        // (`BashTool.tsx:697-699`).
        if !interrupted && matches!(out.exit_code, Some(code) if code != 0) {
            let code = out.exit_code.unwrap_or_default();
            if body.is_empty() {
                body = format!("Exit code {code}");
            } else {
                body = format!("{body}\nExit code {code}");
            }
        }
        // CC's interrupted marker (`BashTool.tsx:602-606`), joined with '\n'.
        if interrupted {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("<error>Command was aborted before completion</error>");
        }

        let output = BashOutput {
            exit_code: out.exit_code,
            interrupted,
            truncated,
            body,
        };
        // CC sets is_error only when interrupted (`BashTool.tsx:621`): surface
        // that as a soft error carrying the same body text.
        if interrupted {
            return Err(ToolError::Respond(output.body));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_output_strips_leading_blanks_and_trailing() {
        assert_eq!(process_output("\n\n  \nhello\nworld\n\n"), "hello\nworld");
        assert_eq!(process_output("plain"), "plain");
        assert_eq!(process_output(""), "");
        // Leading spaces on the first content line survive (the `^(\s*\n)+`
        // run ends at the last leading newline).
        assert_eq!(process_output("\n\n  hello"), "  hello");
        // Internal CRLF preserved byte-for-byte; only trailing trimmed.
        assert_eq!(process_output("a\r\nb\r\n"), "a\r\nb");
    }

    #[test]
    fn description_is_the_pinned_bash_md() {
        let desc = include_str!("descriptions/bash.md");
        // Provenance pin (grok `template_copy_is_pinned` pattern): byte length +
        // opening line. sha256 71e60fe7d090d8ab1cc9604bd6ed745249f3fe88c33ac723f913f8959bbb7439.
        assert_eq!(desc.len(), 9451, "bash.md byte length changed");
        assert!(
            desc.starts_with("Executes a given bash command and returns its output."),
            "bash.md opening line changed"
        );
        assert!(
            desc.trim_end()
                .ends_with("gh api repos/foo/bar/pulls/123/comments"),
            "bash.md closing line changed"
        );
        // Attribution removed (truth-first): CC's `includeCoAuthoredBy: false`
        // branch — no model name to lie about in commit traces.
        assert!(
            !desc.contains("Co-Authored-By"),
            "attribution must be absent"
        );
        assert!(
            !desc.contains("Generated with [Claude Code]"),
            "PR attribution must be absent"
        );
        assert!(desc.contains("- Create the commit with a message."));
    }
}
