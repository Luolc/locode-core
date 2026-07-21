//! `run_terminal_cmd` — a faithful port of Grok Build's Bash tool (`gb/bash/mod.rs`),
//! **foreground slice, background-disabled configuration** (Task 26; background
//! mode deferred per user decision 2026-07-20).
//!
//! Fidelity notes (see `tasks/audits/run_terminal_cmd.md`):
//! - Schema: `is_background` absent — faithful: grok *removes* it from the
//!   exported schema when backgrounding is disabled (`gb:1348-1350,1377-1381`).
//!   The timeout description is grok's exported wire form, whose background
//!   sentence is deliberately unconditional (`gb:1356-1359` "this note is
//!   unconditional") — kept verbatim even though our config has no kill tool.
//! - Type-strict `timeout` (no numeric-string coercion — user deviation).
//! - Shell shape: detected bash/zsh + `-c` + cached login-PATH probe via the
//!   host `ShellSpec` (Slice 0b). Grace period: our host default is 2s vs the
//!   description's "~1s" (host-config territory; text kept verbatim).
//! - Output pipeline: 20k-char front/back retention (host `FrontBackSpec`) →
//!   grok's separator — then ANSI-strip → soft-wrap(2000)
//!   (`make_output_for_prompt`, `gb-out:454-458`); header `exit: …` with
//!   `killed (reason)` variants + the truncation annotation incl. the spill
//!   path (`gb:385-443`).
//! - Guardrails: unwaited-`&` rejection (current contract scanner ported
//!   verbatim from `gb:640-840`) with the background-disabled message
//!   (`gb:1472-1474`); self-matching `pkill`/`pgrep -f` rejection with grok's
//!   exact message (`gb:1887-1898`). The pkill detector is re-implemented
//!   without a regex dependency — same accept/reject semantics, pinned by the
//!   tests below.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use locode_host::{ExecRequest, FrontBackSpec, Host, ShellSpec};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok's default command timeout and foreground ceiling, in ms (`gb:450-459`).
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 300_000;
/// grok's retained-output budget in chars (`{max_output_bytes}` → 20000).
const OUTPUT_CHAR_BUDGET: usize = 20_000;
/// grok's soft-wrap width (`truncate.rs:4` `DEFAULT_SOFT_WRAP_WIDTH`).
const SOFT_WRAP_WIDTH: usize = 2_000;
/// grok's front/back separator (`truncate.rs:255`).
const TRUNCATION_SEPARATOR: &str = "\n\n... (output truncated) ...\n\n";

/// grok's `run_terminal_cmd` tool.
pub(crate) struct GrokRunTerminalCmd {
    host: Arc<Host>,
}

impl GrokRunTerminalCmd {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `run_terminal_cmd` — grok's background-disabled exported
/// schema (`command` + `timeout` + `description`; no `is_background`).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RunTerminalCmdArgs {
    #[schemars(description = "The bash command to run.")]
    command: String,
    /// grok's exported wire description (`gb:1360-1370`) — the background
    /// sentence is unconditional in grok even when backgrounding is disabled.
    #[schemars(
        description = "Optional timeout in milliseconds (max 300000). Default: 120000. `timeout: 0` in background mode disables the wrapper timeout entirely; the task runs until it exits or is killed via the kill task tool."
    )]
    #[serde(default)]
    timeout: Option<u64>,
    #[schemars(
        description = "One sentence explanation as to why this command needs to be run and how it contributes to the goal."
    )]
    #[allow(dead_code)] // required for schema fidelity; grok uses it for UX only
    description: String,
}

/// The structured (report) face of a terminal run. The formatted body is the
/// prompt face.
#[derive(Debug, Serialize)]
pub(crate) struct RunTerminalCmdOutput {
    /// Process exit code; `-1` when the command was killed.
    exit_code: i64,
    /// Whether the command hit the hard timeout.
    timed_out: bool,
    /// Whether the retained output is a front/back subset.
    truncated: bool,
    /// True total output size in bytes.
    total_bytes: u64,
    /// The rendered prompt body (header + processed output).
    #[serde(skip)]
    prompt: String,
}

impl ToolOutput for RunTerminalCmdOutput {
    fn to_prompt_text(&self) -> String {
        self.prompt.clone()
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
        // grok's background-disabled description template (`gb:1440-1453`),
        // rendered for a default unix session: 300000/120000 interpolated,
        // `{max_output_bytes}` → 20000, windows/semicolon/no-unix-utilities
        // branches dropped.
        "Run a bash command and return its output.\n\nUsage notes:\n  - You can specify an optional timeout in milliseconds (up to 300000ms). If not specified, commands will timeout after 120000ms.\n  - Timeout enforcement: when the timeout fires, the wrapper kills the child process group (SIGTERM, escalated to SIGKILL after a ~1s grace period).\n  - If the output exceeds 20000 characters, output will be truncated before being returned to you."
    }

    async fn run(
        &self,
        ctx: &ToolCtx,
        args: RunTerminalCmdArgs,
    ) -> Result<Self::Output, ToolError> {
        // --- Guardrails (grok's parse-time validation, `gb:1860-1899`) ---
        if contains_unwaited_background_operator(&args.command) {
            // Background-disabled, current-contract message (`gb:1472-1474`).
            return Err(ToolError::Respond(
                "Remove the background '&' from your command; background execution is disabled."
                    .to_string(),
            ));
        }
        if let Some(hit) = self_matching_pkill_pattern(&args.command) {
            return Err(ToolError::Respond(format!(
                "self-matching {cmd}/-f: `{cmd} -f <pat>` matches against the full \
                 /proc/PID/cmdline of every process, including the bash wrapper that \
                 runs this command (its argv contains `{pattern}`). The wrapper would \
                 be killed by the resulting signal before the rest of the script runs. \
                 Use one of: `pkill -x <basename>` (no `-f`), `pgrep -f <pat> | xargs \
                 -r kill` invoked from a separate command, a fully-qualified path that \
                 does not appear later in the script, or kill by PID file.",
                cmd = hit.cmd,
                pattern = hit.pattern,
            )));
        }

        let timeout_ms = args
            .timeout
            .filter(|&t| t > 0) // `0` is background-only semantics; foreground treats it as unset
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let request = ExecRequest {
            command: args.command,
            cwd: ctx.cwd.clone(),
            timeout: Some(Duration::from_millis(timeout_ms)),
            env: Vec::new(),
            // grok's shape: detected bash/zsh, `-c`, cached login-PATH probe.
            shell: Some(ShellSpec {
                detect_program: true,
                login_arg: false,
                login_path_probe: true,
            }),
            front_back: Some(FrontBackSpec {
                char_budget: OUTPUT_CHAR_BUDGET,
                spill: true,
            }),
        };
        // Only a spawn/capture failure is a (soft) error; a non-zero exit /
        // timeout is a successful capture the model reads (ADR-0004).
        let out = self
            .host
            .exec(request, &ctx.cancel)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        let Some(capture) = out.front_back else {
            return Err(ToolError::Respond(
                "internal: combined capture missing".to_string(),
            ));
        };
        let truncated = capture.back.is_some();
        let retained_len = capture.front.len() + capture.back.as_ref().map_or(0, String::len);
        let raw_body = match &capture.back {
            Some(back) => format!("{}{TRUNCATION_SEPARATOR}{back}", capture.front),
            None => capture.front.clone(),
        };
        // grok's prompt pipeline: strip ANSI, then soft-wrap long lines
        // (`make_output_for_prompt`, `gb-out:454-458`).
        let body = soft_wrap_lines(&strip_ansi(&raw_body), SOFT_WRAP_WIDTH);

        // Header: `exit: N` / `exit: killed (reason)` (`gb:433-441`), plus the
        // truncation annotation (`gb:385-397`). Synthetic kill reasons never
        // add `[signal=…]` (`gb:398-404`).
        let header = if out.timed_out {
            "exit: killed (timeout)".to_string()
        } else if out.cancelled {
            "exit: killed (cancelled)".to_string()
        } else {
            match out.exit_code {
                Some(code) => format!("exit: {code}"),
                None => "exit: killed (killed)".to_string(),
            }
        };
        let annotation = if truncated {
            let shown = format_bytes(retained_len);
            let total = format_bytes(usize::try_from(capture.total_bytes).unwrap_or(usize::MAX));
            match &capture.spill_path {
                Some(path) => format!(
                    " [truncated: showing first/last {shown} of {total} - full output at: {}]",
                    path.display()
                ),
                // Spill unavailable: drop the location clause (documented in
                // the audit's criterion-4 note).
                None => format!(" [truncated: showing first/last {shown} of {total}]"),
            }
        } else {
            String::new()
        };

        Ok(RunTerminalCmdOutput {
            exit_code: out.exit_code.map_or(-1, i64::from),
            timed_out: out.timed_out,
            truncated,
            total_bytes: capture.total_bytes,
            prompt: format!("{header}{annotation}\n{body}"),
        })
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Output post-processing (ports of gb `util/truncate.rs` + ANSI strip)
// ───────────────────────────────────────────────────────────────────────────

/// grok's human-size rendering (`truncate.rs:200-208`).
fn format_bytes(bytes: usize) -> String {
    #[allow(clippy::cast_precision_loss)] // display-only, grok's exact math
    if bytes >= 1_000_000 {
        format!("{:.1}MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1}KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes}B")
    }
}

/// Strip ANSI escape sequences (CSI/OSC/single-char escapes). grok uses the
/// `strip_ansi_escapes` crate (`gb:414`); this covers the same sequences for
/// terminal output without the dependency.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ … final byte in @-~
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] … terminated by BEL or ESC \
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-char escapes (ESC + one byte).
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// grok's per-line soft wrap (`truncate.rs:56-78`): hard char-wrap lines over
/// `wrap_width`; shorter lines untouched; all content preserved.
fn soft_wrap_line(line: &str, wrap_width: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if line.len() <= wrap_width {
        return Cow::Borrowed(line);
    }
    let char_count = line.chars().count();
    if char_count <= wrap_width {
        return Cow::Borrowed(line);
    }
    let num_wraps = char_count.saturating_sub(1) / wrap_width;
    let mut result = String::with_capacity(line.len() + num_wraps);
    let mut chars_on_current_line = 0;
    for ch in line.chars() {
        if chars_on_current_line >= wrap_width {
            result.push('\n');
            chars_on_current_line = 0;
        }
        result.push(ch);
        chars_on_current_line += 1;
    }
    Cow::Owned(result)
}

/// grok's multi-line soft wrap (`truncate.rs:212-224`).
fn soft_wrap_lines(text: &str, wrap_width: usize) -> String {
    let mut result = String::with_capacity(text.len() + 256);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        result.push_str(&soft_wrap_line(line, wrap_width));
    }
    if text.ends_with('\n') && !text.is_empty() {
        result.push('\n');
    }
    result
}

// ───────────────────────────────────────────────────────────────────────────
// Background-operator detection (verbatim port of gb:640-840)
// ───────────────────────────────────────────────────────────────────────────

/// `true` when the command's last statement is the `wait` builtin
/// (`gb:616-636`): `cmd1 & cmd2 & wait` parallelises sub-tasks and blocks.
fn ends_with_wait_builtin(command: &str) -> bool {
    let trimmed = command
        .trim_end()
        .trim_end_matches(';')
        .trim_end_matches('\n')
        .trim_end();
    trimmed == "wait"
        || trimmed.ends_with(" wait")
        || trimmed.ends_with(";wait")
        || trimmed.ends_with("\twait")
        || trimmed.ends_with("\nwait")
}

/// An unwaited backgrounding `&` (`gb:634-638`): the current-contract check.
fn contains_unwaited_background_operator(command: &str) -> bool {
    contains_background_operator(command) && !ends_with_wait_builtin(command)
}

/// Parse the heredoc operator at `chars[start]` (`gb:638-720` shape):
/// `(delimiter, strip_tabs, position_after_delimiter_token)`.
fn parse_heredoc_start(chars: &[char], start: usize) -> Option<(String, bool, usize)> {
    let len = chars.len();
    let mut i = start + 2; // skip `<<`
    let strip_tabs = i < len && chars[i] == '-';
    if strip_tabs {
        i += 1;
    }
    while i < len && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    if i >= len || chars[i] == '\n' {
        return None;
    }
    let delimiter: String;
    if chars[i] == '\'' {
        i += 1;
        let d_start = i;
        while i < len && chars[i] != '\'' {
            i += 1;
        }
        if i >= len {
            return None; // unclosed quote
        }
        delimiter = chars[d_start..i].iter().collect();
        i += 1;
    } else if chars[i] == '"' {
        i += 1;
        let d_start = i;
        while i < len && chars[i] != '"' {
            i += 1;
        }
        if i >= len {
            return None;
        }
        delimiter = chars[d_start..i].iter().collect();
        i += 1;
    } else {
        let d_start = i;
        while i < len
            && !chars[i].is_whitespace()
            && !matches!(chars[i], ';' | '&' | '|' | '(' | ')' | '<' | '>')
        {
            i += 1;
        }
        if i == d_start {
            return None;
        }
        delimiter = chars[d_start..i].iter().filter(|&&c| c != '\\').collect();
    }
    if delimiter.is_empty() {
        return None;
    }
    Some((delimiter, strip_tabs, i))
}

/// Skip a heredoc body starting at `chars[start]` (`gb:688-745`): scan lines
/// until the delimiter; unclosed heredocs consume the rest.
fn skip_heredoc_body(chars: &[char], start: usize, delimiter: &str, strip_tabs: bool) -> usize {
    let len = chars.len();
    let mut i = start;
    while i < len {
        let line_start = i;
        while i < len && chars[i] != '\n' {
            i += 1;
        }
        let line: String = chars[line_start..i].iter().collect();
        let check = if strip_tabs {
            line.trim_start_matches('\t')
        } else {
            line.as_str()
        };
        if check == delimiter {
            if i < len {
                i += 1;
            }
            return i;
        }
        if i < len {
            i += 1;
        }
    }
    len
}

/// grok's quote/escape/heredoc-aware backgrounding-`&` scanner (`gb:750-840`).
fn contains_background_operator(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut pending_heredocs: Vec<(String, bool)> = Vec::new();

    while i < len {
        let ch = chars[i];
        if ch == '\\' && !in_single_quote {
            i += 2;
            continue;
        }
        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }
        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            i += 1;
            continue;
        }
        if ch == '\n' && !in_single_quote && !in_double_quote && !pending_heredocs.is_empty() {
            i += 1;
            for (delim, strip_tabs) in pending_heredocs.drain(..) {
                i = skip_heredoc_body(&chars, i, &delim, strip_tabs);
            }
            continue;
        }
        if ch == '<'
            && !in_single_quote
            && !in_double_quote
            && i + 1 < len
            && chars[i + 1] == '<'
            && !(i + 2 < len && chars[i + 2] == '<')
            && let Some((delim, strip_tabs, after)) = parse_heredoc_start(&chars, i)
        {
            pending_heredocs.push((delim, strip_tabs));
            i = after;
            continue;
        }
        if ch == '&' && !in_single_quote && !in_double_quote {
            if i + 1 < len && chars[i + 1] == '&' {
                i += 2;
                continue;
            }
            if i + 1 < len && chars[i + 1] == '>' {
                i += 2;
                if i < len && chars[i] == '>' {
                    i += 1;
                }
                continue;
            }
            if i > 0 && (chars[i - 1] == '>' || chars[i - 1] == '<') {
                i += 1;
                continue;
            }
            return true;
        }
        i += 1;
    }
    false
}

// ───────────────────────────────────────────────────────────────────────────
// Self-matching pkill/pgrep detection (gb:841-1000; regex re-implemented as a
// scanner — same accept/reject semantics, no regex dependency)
// ───────────────────────────────────────────────────────────────────────────

/// Minimum flagged pattern length (`gb:871-874`).
const MIN_PKILL_PATTERN_LEN: usize = 3;

/// A self-matching `pkill`/`pgrep -f` hit (`gb:876-883`).
struct SelfMatchingPkill {
    cmd: &'static str,
    pattern: String,
}

/// One parsed `pkill|pgrep` invocation with a `-f` flag: (cmd, pattern,
/// match-range in the original string).
fn find_pkill_invocations(command: &str) -> Vec<(&'static str, String, std::ops::Range<usize>)> {
    let mut hits = Vec::new();
    let bytes = command.as_bytes();
    let mut idx = 0;
    while idx < command.len() {
        // Statement boundary: start of string or one of `;&|(\n` (gb regex).
        let at_boundary = idx == 0
            || matches!(bytes[idx - 1], b';' | b'&' | b'|' | b'(' | b'\n')
            || (command[..idx].ends_with(char::is_whitespace)
                && command[..idx]
                    .trim_end()
                    .ends_with([';', '&', '|', '(', '\n']))
            || command[..idx].trim_end().is_empty();
        let rest = &command[idx..];
        let cmd: &'static str = if rest.starts_with("pkill") {
            "pkill"
        } else if rest.starts_with("pgrep") {
            "pgrep"
        } else {
            idx += command[idx..].chars().next().map_or(1, char::len_utf8);
            continue;
        };
        if !at_boundary {
            idx += cmd.len();
            continue;
        }
        let start = idx;
        let mut j = idx + cmd.len();
        // One or more flag tokens; at least one must carry `f` (short cluster
        // `-X*fX*` or `--full`).
        let mut has_f = false;
        let mut any_flag = false;
        loop {
            let after_ws = command[j..].trim_start_matches([' ', '\t']);
            let ws_len = command[j..].len() - after_ws.len();
            if ws_len == 0 || !after_ws.starts_with('-') {
                break;
            }
            let flag_end = after_ws
                .find(|c: char| c.is_whitespace())
                .unwrap_or(after_ws.len());
            let flag = &after_ws[..flag_end];
            if flag == "--full" {
                has_f = true;
            } else if !flag.starts_with("--")
                && flag.len() > 1
                && flag[1..].chars().all(|c| c.is_ascii_alphabetic())
            {
                if flag[1..].contains('f') {
                    has_f = true;
                }
            } else {
                break; // not a recognised flag token
            }
            any_flag = true;
            j += ws_len + flag.len();
        }
        if !(any_flag && has_f) {
            idx = start + cmd.len();
            continue;
        }
        // Pattern argument: bare / single-quoted / double-quoted.
        let after_ws = command[j..].trim_start_matches([' ', '\t']);
        let ws_len = command[j..].len() - after_ws.len();
        if ws_len == 0 || after_ws.is_empty() {
            idx = start + cmd.len();
            continue;
        }
        j += ws_len;
        let (pattern, pat_len) = if let Some(rest) = after_ws.strip_prefix('\'') {
            let Some(end) = rest.find('\'') else {
                idx = start + cmd.len();
                continue;
            };
            (rest[..end].to_string(), end + 2)
        } else if let Some(rest) = after_ws.strip_prefix('"') {
            let Some(end) = rest.find('"') else {
                idx = start + cmd.len();
                continue;
            };
            (rest[..end].to_string(), end + 2)
        } else {
            let end = after_ws
                .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
                .unwrap_or(after_ws.len());
            (after_ws[..end].to_string(), end)
        };
        let match_end = j + pat_len;
        hits.push((cmd, pattern, start..match_end));
        idx = match_end;
    }
    hits
}

/// `kill` as its own command word in the excised rest (`gb:866-870`).
fn contains_kill_token(rest: &str) -> bool {
    let mut idx = 0;
    while let Some(pos) = rest[idx..].find("kill") {
        let abs = idx + pos;
        let before_ok = abs == 0
            || rest[..abs]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'));
        let after = &rest[abs + 4..];
        let after_ok = after.is_empty() || after.starts_with(char::is_whitespace);
        if before_ok && after_ok {
            return true;
        }
        idx = abs + 4;
    }
    false
}

/// grok's detection (`gb:885-965`): a `-f` pattern that substring-matches the
/// rest of the command; `pgrep` only flagged when the rest also kills.
fn self_matching_pkill_pattern(command: &str) -> Option<SelfMatchingPkill> {
    for (cmd, pattern, range) in find_pkill_invocations(command) {
        if pattern.is_empty()
            || pattern.len() < MIN_PKILL_PATTERN_LEN
            || pattern.contains("$(")
            || pattern.contains("$`")
            || pattern.starts_with('`')
            || pattern.contains("${")
        {
            continue;
        }
        let mut rest = String::with_capacity(command.len());
        rest.push_str(&command[..range.start]);
        rest.push('\n');
        rest.push_str(&command[range.end..]);
        if cmd == "pgrep" && !contains_kill_token(&rest) {
            continue;
        }
        if rest.contains(&pattern) {
            return Some(SelfMatchingPkill { cmd, pattern });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // gb's contract: `&` anywhere (unwaited) is a background operator; quotes,
    // escapes, redirects, heredocs, and `wait` exempt.
    #[test]
    fn background_operator_detection_matches_grok() {
        assert!(contains_unwaited_background_operator("sleep 5 &"));
        assert!(contains_unwaited_background_operator("a & b"));
        assert!(!contains_unwaited_background_operator("a && b"));
        assert!(!contains_unwaited_background_operator("cmd &> log"));
        assert!(!contains_unwaited_background_operator("cmd &>> log"));
        assert!(!contains_unwaited_background_operator("cmd 2>&1"));
        assert!(!contains_unwaited_background_operator("cmd <&3"));
        assert!(!contains_unwaited_background_operator("echo '&'"));
        assert!(!contains_unwaited_background_operator("echo \"a & b\""));
        assert!(!contains_unwaited_background_operator("echo \\&"));
        assert!(!contains_unwaited_background_operator("cmd1 & cmd2 & wait"));
        assert!(!contains_unwaited_background_operator(
            "cat <<EOF\nx & y\nEOF"
        ));
        assert!(contains_unwaited_background_operator(
            "cat <<EOF\nbody\nEOF\nsleep 3 &"
        ));
    }

    #[test]
    fn pkill_self_match_detection() {
        // pkill -f with the pattern appearing later → rejected.
        let hit = self_matching_pkill_pattern("pkill -f my_script.py; ./my_script.py").unwrap();
        assert_eq!(hit.cmd, "pkill");
        assert_eq!(hit.pattern, "my_script.py");
        // pgrep without a kill in the rest → allowed.
        assert!(self_matching_pkill_pattern("pgrep -f my_script.py && echo found").is_none());
        // pgrep piped into kill → rejected.
        assert!(
            self_matching_pkill_pattern("pgrep -f my_script.py | xargs kill && ./my_script.py")
                .is_some()
        );
        // gb quirk pinned: `kill;` fails gb's `kill(?:\s|$)` token check, so
        // this stays allowed (KILL_TOKEN_RE, gb:866-870).
        assert!(
            self_matching_pkill_pattern("pgrep -f my_script.py | xargs kill; ./my_script.py")
                .is_none()
        );
        // Non-f pkill → allowed.
        assert!(self_matching_pkill_pattern("pkill -x bash; echo bash").is_none());
        // Short patterns skipped.
        assert!(self_matching_pkill_pattern("pkill -f ab; ab").is_none());
        // Command substitution skipped.
        assert!(self_matching_pkill_pattern("pkill -f \"$(cat pat)\"; $(cat pat)").is_none());
        // Quoted pattern.
        assert!(self_matching_pkill_pattern("pkill -f 'run me'; ./run me").is_some());
        // --full long form.
        assert!(self_matching_pkill_pattern("pkill --full serve.py; python serve.py").is_some());
    }

    #[test]
    fn format_bytes_matches_grok() {
        assert_eq!(format_bytes(999), "999B");
        assert_eq!(format_bytes(20_000), "20.0KB");
        assert_eq!(format_bytes(1_234_567), "1.2MB");
    }

    #[test]
    fn strip_ansi_and_soft_wrap() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m plain"), "red plain");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}body"), "body");
        let wrapped = soft_wrap_lines(&"x".repeat(4100), 2_000);
        assert_eq!(wrapped.lines().count(), 3);
        assert_eq!(wrapped.replace('\n', "").len(), 4100);
        assert_eq!(soft_wrap_lines("short\nlines\n", 2_000), "short\nlines\n");
    }
}
