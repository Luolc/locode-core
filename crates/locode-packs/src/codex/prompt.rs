//! Codex's base system prompt (Slice 3: the full, byte-exact gpt-5.6-sol
//! `base_instructions`, D6) + the `<environment_context>` renderer.
//!
//! Source (codex submodule `f201c30c`): gpt-5.6-sol's `base_instructions`
//! (`models-manager/models.json`, `models[0]`), 17730 chars / 17766 bytes, pinned
//! verbatim in `prompts/base_instructions.md`. The prompt has **no** runtime
//! placeholders and bakes in no runtime-computed value (only the identity opener
//! names "GPT-5" — handled by `strip_identity`), so it is reproduced verbatim
//! (truth-vs-fidelity: nothing here is false for our run).
//!
//! `strip_identity` (default off, A/B contamination control) removes exactly the
//! identity opener sentence (D6). The dynamic per-run values (cwd/shell/date/tz)
//! live in the separate `<environment_context>` User item, not in this static
//! prompt — codex's per-turn world-state re-injection/diffing is loop-adjacent
//! machinery (ADR-0023) and is NOT reproduced; we emit the static first-turn
//! snapshot as part of the preamble.

use crate::pack::PackContext;

/// The pinned gpt-5.6-sol `base_instructions` (byte-exact).
pub(crate) const BASE_INSTRUCTIONS: &str = include_str!("prompts/base_instructions.md");

/// The identity opener sentence `strip_identity` removes (`base_instructions`
/// begins with exactly this; D6).
pub(crate) const IDENTITY: &str = "You are Codex, an agent based on GPT-5. ";

/// Render the base prompt for `ctx` — the full `base_instructions`, minus the
/// identity opener when `strip_identity` is set.
pub(crate) fn render(ctx: &PackContext) -> String {
    if ctx.strip_identity {
        BASE_INSTRUCTIONS
            .strip_prefix(IDENTITY)
            .unwrap_or(BASE_INSTRUCTIONS)
            .to_string()
    } else {
        BASE_INSTRUCTIONS.to_string()
    }
}

/// Render the `<environment_context>` block (`world_state/environment.rs`
/// `EnvironmentsState`, single local environment): `<cwd>`, `<shell>` (basename),
/// `<current_date>`, `<timezone>` (each 2-space indented, `\n`-joined). Codex omits
/// `<filesystem>`/`<network>` in the default case, and we do not reproduce codex's
/// managed-sandbox / network-rule model (we use a path jail) — reproducing it would
/// inject values untrue for our run (truth wins, ADR-0023 fidelity boundary). The
/// `<timezone>` line is emitted only when the host resolves one (codex's field is
/// `Option<String>`).
pub(crate) fn environment_context(ctx: &PackContext) -> String {
    use std::fmt::Write as _;
    let shell = shell_name(&ctx.shell);
    let mut body = format!(
        "<environment_context>\n  <cwd>{}</cwd>\n  <shell>{}</shell>\n  <current_date>{}</current_date>",
        xml_escape(&ctx.cwd.to_string_lossy()),
        xml_escape(&shell),
        xml_escape(&ctx.date),
    );
    if let Some(tz) = &ctx.timezone {
        let _ = write!(body, "\n  <timezone>{}</timezone>", xml_escape(tz));
    }
    body.push_str("\n</environment_context>");
    body
}

/// The shell basename codex renders (`<shell>bash</shell>`, not the full path).
fn shell_name(shell: &str) -> String {
    std::path::Path::new(shell)
        .file_name()
        .map_or_else(|| shell.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Codex's XML escaping for context text (`environment_context.rs`
/// `push_xml_escaped_text`).
fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx(strip_identity: bool) -> PackContext {
        PackContext {
            cwd: PathBuf::from("/repo"),
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            date: "2026-07-24".into(),
            headless: true,
            is_git_repo: true,
            model: Some("gpt-5.6-sol".into()),
            os_version: Some("Darwin 24.6.0".into()),
            timezone: Some("America/Los_Angeles".into()),
            strip_identity,
        }
    }

    #[test]
    fn render_is_the_full_pinned_prompt() {
        let out = render(&ctx(false));
        // 17730 chars / 17766 bytes; sha256 cbefa6b0bede0e332d957fca70ccacf9f12f4c0ecdf81b819e5cbe1a3b16e265.
        assert_eq!(out.len(), 17766, "base_instructions byte length changed");
        assert_eq!(
            out.chars().count(),
            17730,
            "base_instructions char count changed"
        );
        assert!(out.starts_with("You are Codex, an agent based on GPT-5. "));
        assert!(out.contains("# Personality"));
    }

    #[test]
    fn strip_identity_removes_only_the_identity_opener() {
        let out = render(&ctx(true));
        assert!(out.starts_with("You and the user share one workspace"));
        assert_eq!(out.len(), 17766 - IDENTITY.len());
    }

    #[test]
    fn environment_context_renders_cwd_shell_date_tz() {
        let block = environment_context(&ctx(false));
        assert_eq!(
            block,
            "<environment_context>\n  <cwd>/repo</cwd>\n  <shell>zsh</shell>\n  <current_date>2026-07-24</current_date>\n  <timezone>America/Los_Angeles</timezone>\n</environment_context>"
        );
    }

    #[test]
    fn environment_context_omits_timezone_when_absent() {
        let mut c = ctx(false);
        c.timezone = None;
        let block = environment_context(&c);
        assert!(!block.contains("<timezone>"));
        assert!(block.contains("<current_date>2026-07-24</current_date>"));
    }
}
