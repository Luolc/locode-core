//! Codex's base system prompt (Slice 1: a **minimal** render — the gpt-5.6-sol
//! identity opener). Slice 3 replaces this with the full byte-exact 17730-char
//! `base_instructions` (D6) + the `<environment_context>` preamble.
//!
//! Source (codex submodule `f201c30c`): gpt-5.6-sol's `base_instructions`
//! (`models-manager/models.json`) opens with the identity line below. `strip_identity`
//! (default off) removes the identity sentence (D6).

use crate::pack::PackContext;

/// The gpt-5.6-sol identity sentence (`base_instructions` opener). `strip_identity`
/// removes exactly this (D6).
pub(crate) const IDENTITY: &str = "You are Codex, an agent based on GPT-5. ";

/// The rest of the opener (kept even when the identity is stripped).
const OPENER_REST: &str = "You and the user share one workspace, and your job is to collaborate with them until their goal is genuinely handled.";

/// Render the (minimal, Slice 1) base prompt for `ctx`.
pub(crate) fn render(ctx: &PackContext) -> String {
    if ctx.strip_identity {
        OPENER_REST.to_string()
    } else {
        format!("{IDENTITY}{OPENER_REST}")
    }
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
            strip_identity,
        }
    }

    #[test]
    fn render_starts_with_identity() {
        assert!(render(&ctx(false)).starts_with("You are Codex, an agent based on GPT-5."));
    }

    #[test]
    fn strip_identity_removes_the_identity_sentence() {
        let out = render(&ctx(true));
        assert!(!out.contains("You are Codex, an agent based on GPT-5."));
        assert!(out.starts_with("You and the user share one workspace"));
    }
}
