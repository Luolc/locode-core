//! Claude Code's static system prompt (Slice 1: a **minimal** render — identity
//! prefix + intro section). Slice 7 replaces this with the full byte-exact
//! assembly (all D7 sections + env + the currentDate User reminder).
//!
//! Source (submodule commit `6a25909`): the identity prefix
//! (`constants/system.ts:10-46`, `getCLISyspromptPrefix`) and the intro section
//! (`constants/prompts.ts` `getSimpleIntroSection` + `cyberRiskInstruction.ts:24`).
//! CC assembles the system prompt as an array of blocks — `[prefix, ...sections]`
//! (`services/api/claude.ts:1358-1369`, `constants/prompts.ts:444`) — that become
//! separate wire blocks; we flatten to one System text joined with `\n\n` (plan
//! §4.7 decision #6: our wire owns cache placement, so the block split isn't
//! needed).

use crate::pack::PackContext;

/// Headless identity (`AGENT_SDK_PREFIX`, `system.ts:12`): what non-interactive
/// Claude Code sends.
pub(crate) const AGENT_SDK_PREFIX: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

/// Interactive identity (`DEFAULT_PREFIX`, `system.ts:10`).
pub(crate) const DEFAULT_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// The intro section (`getSimpleIntroSection`, null output-style branch) with the
/// cyber-risk instruction (`cyberRiskInstruction.ts:24`) and the URL-guessing ban.
/// Keeps CC's exact shape, including the leading newline of the template literal.
pub(crate) const INTRO: &str = "\nYou are an interactive agent that helps users with software engineering tasks. Use the instructions below and the tools available to you to assist the user.\n\nIMPORTANT: Assist with authorized security testing, defensive security, CTF challenges, and educational contexts. Refuse requests for destructive techniques, DoS attacks, mass targeting, supply chain compromise, or detection evasion for malicious purposes. Dual-use security tools (C2 frameworks, credential testing, exploit development) require clear authorization context: pentesting engagements, CTF competitions, security research, or defensive use cases.\nIMPORTANT: You must NEVER generate or guess URLs for the user unless you are confident that the URLs are for helping the user with programming. You may use URLs provided by the user in their messages or local files.";

/// CC's block separator when flattening the system-prompt array to one text
/// (plan §4.7 #6). Empty blocks are dropped (CC's `.filter(Boolean)`).
pub(crate) fn join_blocks(blocks: &[&str]) -> String {
    blocks
        .iter()
        .filter(|b| !b.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The identity prefix for `ctx` (D6): headless → Agent SDK; interactive →
/// Claude Code. `strip_identity` removes it (both variants).
fn identity_prefix(ctx: &PackContext) -> &'static str {
    if ctx.strip_identity {
        ""
    } else if ctx.headless {
        AGENT_SDK_PREFIX
    } else {
        DEFAULT_PREFIX
    }
}

/// Render the (minimal, Slice 1) system prompt for `ctx`.
pub(crate) fn render(ctx: &PackContext) -> String {
    join_blocks(&[identity_prefix(ctx), INTRO])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx(headless: bool, strip_identity: bool) -> PackContext {
        PackContext {
            cwd: PathBuf::from("/repo"),
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            date: "2026-07-24".into(),
            headless,
            strip_identity,
        }
    }

    #[test]
    fn headless_render_starts_with_agent_sdk_identity() {
        let out = render(&ctx(true, false));
        assert!(out.starts_with(AGENT_SDK_PREFIX), "{out}");
        assert!(out.contains("interactive agent that helps users"));
    }

    #[test]
    fn interactive_render_starts_with_claude_code_identity() {
        let out = render(&ctx(false, false));
        assert!(out.starts_with(DEFAULT_PREFIX), "{out}");
    }

    #[test]
    fn strip_identity_removes_both_variants() {
        for headless in [true, false] {
            let out = render(&ctx(headless, true));
            assert!(
                !out.contains(AGENT_SDK_PREFIX),
                "headless={headless}: {out}"
            );
            assert!(!out.contains(DEFAULT_PREFIX), "headless={headless}: {out}");
            // The intro survives; only the identity block is dropped.
            assert!(out.contains("interactive agent that helps users"));
        }
    }

    #[test]
    fn render_carries_cyber_risk_and_url_ban() {
        let out = render(&ctx(true, false));
        assert!(out.contains("authorized security testing, defensive security"));
        assert!(out.contains("NEVER generate or guess URLs"));
    }
}
