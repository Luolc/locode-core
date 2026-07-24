//! The grok pack's real system prompt + first-message shaping (Task 13).
//!
//! **Faithful mimicry, byte-exact where possible:**
//!
//! - `templates/prompt.md` is a **verbatim copy** of Grok Build's
//!   `xai-grok-agent/templates/prompt.md` (submodule commit `b189869b`,
//!   sha256 `c805ee84…`). Grok's own `test_encrypted_templates_not_stale`
//!   proves that file is byte-identical to what ships in their binary (the
//!   XOR wrapper is obfuscation we deliberately don't port). **Never edit the
//!   copy** — refresh it from the submodule and update the provenance note.
//! - The renderer is grok's own: `MiniJinja` with the custom
//!   `${{ }}` / `${% %}` / `${# #}` delimiters
//!   (`xai-grok-tools/src/types/description.rs` `make_desc_env`).
//! - [`user_info_block`] / [`user_query`] port grok's first-user-message
//!   shaping (`xai-grok-shell/src/session/user_message.rs`):
//!   `construct_user_message_minimal` — grok's **own headless-minimal
//!   variant** (no workspace snapshot, no git status) — and the `<user_query>`
//!   wrapper. Format strings are verbatim; only value *sourcing* moves to
//!   [`PackContext`] so the preamble stays a pure function of context.
//!
//! Environment info (cwd/OS/shell/date) is **not** part of grok's system
//! prompt — it rides in the first user message, which is why `preamble()`
//! returns `[System(prompt), User(user_info)]` (ADR-0013's mapping).

use std::collections::BTreeMap;

use crate::pack::PackContext;

/// Grok's default identity label (`prompt/context.rs:153`,
/// `DEFAULT_SYSTEM_PROMPT_LABEL`). Renders as "You are Grok released by xAI."
/// — kept verbatim per the faithful-mimicry rule (user decision, 2026-07-18).
pub const SYSTEM_PROMPT_LABEL: &str = "Grok";

/// The verbatim template bytes (see the module docs for provenance).
const BASE_TEMPLATE: &str = include_str!("templates/prompt.md");

/// The grok pack's `ToolKind`-key → wire-name map for `${{ tools.by_kind.* }}`.
///
/// Keys are grok's `snake_case` kind names as used *in the template*
/// (`read`/`edit`/`monitor`); the minimal template references only those
/// three. Our pack registers no monitor tool, so the `<background_tasks>`
/// section drops out — exactly what real grok renders for the same tool set
/// (their `test_no_monitor_tool_omits_watch_section`).
fn tools_by_kind() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("shell", "run_terminal_cmd"),
        ("read", "read_file"),
        ("edit", "search_replace"),
        ("grep", "grep"),
        ("glob", "list_dir"),
    ])
}

/// Build grok's `MiniJinja` environment: custom delimiters, nothing else
/// (mirrors `make_desc_env`, `description.rs:38-50`).
fn make_env() -> minijinja::Environment<'static> {
    use minijinja::syntax::SyntaxConfig;
    let syntax = SyntaxConfig::builder()
        .block_delimiters("${%", "%}")
        .variable_delimiters("${{", "}}")
        .comment_delimiters("${#", "#}")
        .build()
        .unwrap_or_else(|e| panic!("custom syntax config is valid: {e}"));
    let mut env = minijinja::Environment::new();
    env.set_syntax(syntax);
    env
}

/// The exact identity sentence the template opens with (label = [`SYSTEM_PROMPT_LABEL`]).
/// Pinned so [`strip_identity`] fails loudly (via tests) if a template refresh
/// rewords it.
const IDENTITY_SENTENCE: &str = "You are Grok released by xAI. ";

/// Render the base system prompt for this context.
///
/// The render context mirrors grok's placeholder set (`template.rs`
/// `default_placeholders` + `TemplateContext`): the minimal template consumes
/// only `system_prompt_label`, `is_non_interactive`, and `tools.by_kind.*`,
/// but the full set is supplied so a template refresh never meets an
/// undefined variable.
///
/// With `ctx.strip_identity`, identity-revealing text is removed **from the
/// rendered output** (the template copy stays verbatim): the opening identity
/// sentence — leaving the turn opener "You are an autonomous agent …" /
/// "You are an interactive CLI tool …" — and, in interactive mode, the
/// `<user_guide>` block that names the Grok Build TUI.
///
/// # Panics
/// On a malformed template — unreachable for the committed verbatim copy
/// (rendering is pure; a panic here is a build-time bug, not runtime input).
#[must_use]
pub fn render_base_prompt(ctx: &PackContext) -> String {
    let render_ctx = minijinja::context! {
        tools => minijinja::context! { by_kind => tools_by_kind() },
        system_prompt_label => SYSTEM_PROMPT_LABEL,
        is_non_interactive => ctx.headless,
        os_name => ctx.os,
        shell_path => ctx.shell,
        working_directory => ctx.cwd.to_string_lossy(),
        current_date => ctx.date,
        memory_enabled => false,
    };
    let rendered = make_env()
        .render_str(BASE_TEMPLATE, render_ctx)
        .unwrap_or_else(|e| panic!("grok prompt template failed to render: {e}"));
    debug_assert!(
        !rendered.contains("${{") && !rendered.contains("${%"),
        "unresolved template tokens in the rendered grok prompt"
    );
    if ctx.strip_identity {
        strip_identity(&rendered)
    } else {
        rendered
    }
}

/// Remove identity-revealing text from a rendered prompt (see
/// [`render_base_prompt`]). Unknown shapes pass through unchanged — the
/// pinning tests, not runtime fallbacks, guard against template drift.
fn strip_identity(rendered: &str) -> String {
    let mut out = rendered
        .strip_prefix(IDENTITY_SENTENCE)
        .map_or_else(|| rendered.to_string(), str::to_string);
    // The <user_guide> block (interactive only) names the Grok Build TUI.
    if let (Some(start), Some(end)) = (out.find("\n\n<user_guide>"), out.find("</user_guide>"))
        && start < end
    {
        out.replace_range(start..end + "</user_guide>".len(), "");
    }
    out
}

/// grok's minimal first-user-message prefix (`user_message.rs`
/// `construct_user_message_minimal`) — the headless variant: `<user_info>`
/// only, deliberately excluding the workspace snapshot and git status.
/// Format string verbatim; values from [`PackContext`].
#[must_use]
pub fn user_info_block(ctx: &PackContext) -> String {
    let os = &ctx.os;
    let shell = &ctx.shell;
    let cwd = ctx.cwd.to_string_lossy();
    let today = &ctx.date;
    format!(
        r"<user_info>
OS Version: {os}
Shell: {shell}
Workspace Path: {cwd}
Today's date: {today}
Note: Prefer using relative paths over absolute paths as tool call args when possible.
</user_info>",
    )
}

/// grok's `<user_query>` wrapper (`user_message.rs:9-15`, verbatim): the
/// system prompt directs the model to "complete the user's request, denoted
/// within the `<user_query>` tag", so the exec layer must wrap the prompt in
/// this before appending it as the user turn (Task 14).
#[must_use]
pub fn user_query(user_message: &str) -> String {
    format!(
        r"<user_query>
{user_message}
</user_query>"
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn ctx(headless: bool) -> PackContext {
        PackContext {
            cwd: PathBuf::from("/tmp/test"),
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            date: "2025-01-15".into(),
            headless,
            is_git_repo: false,
            model: None,
            os_version: None,
            strip_identity: false,
        }
    }

    /// Byte-pin the verbatim template copy: its length and the exact opening
    /// line. A refresh from the submodule that changes either must be a
    /// conscious act (update provenance in the module docs + these pins +
    /// the snapshots).
    #[test]
    fn template_copy_is_pinned() {
        assert_eq!(BASE_TEMPLATE.len(), 4638, "template byte length changed");
        assert!(
            BASE_TEMPLATE
                .starts_with("You are ${{ system_prompt_label }} released by xAI. You are ")
        );
        assert!(!BASE_TEMPLATE.contains('\r'), "no CRLF in the copy");
    }

    /// Golden snapshots: the rendered prompt is byte-frozen for both identity
    /// branches. Regenerate with `UPDATE_SNAPSHOTS=1` after a deliberate
    /// template refresh.
    #[test]
    fn rendered_prompt_matches_snapshots() {
        for (headless, name) in [
            (true, "prompt_headless.txt"),
            (false, "prompt_interactive.txt"),
        ] {
            let rendered = render_base_prompt(&ctx(headless));
            // Prefer the runtime env var over the compile-time `env!` macro: some
            // build systems (e.g. Bazel) compile and run tests in different sandbox
            // paths, so the value baked in at compile time can be stale by run time.
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
                .unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_string());
            let path = format!("{manifest_dir}/src/grok/snapshots/{name}");
            if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
                std::fs::write(&path, &rendered)
                    .unwrap_or_else(|e| panic!("write snapshot {path}: {e}"));
                continue;
            }
            let expected = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read snapshot {path} (run UPDATE_SNAPSHOTS=1): {e}"));
            assert_eq!(rendered, expected, "snapshot drift: {name}");
        }
    }

    // ---- identity branches (mirrors grok's own is_non_interactive tests) ----

    #[test]
    fn headless_renders_the_autonomous_identity() {
        let prompt = render_base_prompt(&ctx(true));
        assert!(prompt.starts_with(
            "You are Grok released by xAI. You are an autonomous agent that \
             completes software engineering tasks."
        ));
        assert!(!prompt.contains("interactive CLI tool"));
        assert!(
            !prompt.contains("<user_guide>"),
            "user_guide is interactive-only"
        );
    }

    #[test]
    fn interactive_renders_the_cli_identity_and_user_guide() {
        let prompt = render_base_prompt(&ctx(false));
        assert!(prompt.starts_with(
            "You are Grok released by xAI. You are an interactive CLI tool that \
             helps users with software engineering tasks."
        ));
        assert!(prompt.contains("<user_guide>"));
        assert!(!prompt.contains("autonomous agent"));
    }

    // ---- template semantics for our tool set ----

    #[test]
    fn tool_names_resolve_and_no_tokens_leak() {
        let prompt = render_base_prompt(&ctx(true));
        assert!(prompt.contains("`read_file`"), "by_kind.read resolves");
        assert!(prompt.contains("`search_replace`"), "by_kind.edit resolves");
        assert!(!prompt.contains("${{"), "no unresolved variables");
        assert!(!prompt.contains("${%"), "no unresolved blocks");
        // No monitor tool registered → the background_tasks section drops out,
        // exactly as real grok renders for this tool set.
        assert!(!prompt.contains("<background_tasks>"));
    }

    /// Forward guard against template growth, mirroring grok's own
    /// `PROMPT_SIZE_SOFT_CEILING_BYTES` = 16384.
    #[test]
    fn rendered_prompt_within_grok_size_budget() {
        assert!(render_base_prompt(&ctx(true)).len() < 16384);
    }

    // ---- strip_identity (locode knob; default off = faithful) ----

    #[test]
    fn strip_identity_removes_the_opening_sentence_headless() {
        let mut c = ctx(true);
        c.strip_identity = true;
        let prompt = render_base_prompt(&c);
        assert!(
            prompt.starts_with("You are an autonomous agent"),
            "identity sentence removed, opener intact"
        );
        assert!(!prompt.contains("Grok"), "no identity leakage");
        assert!(!prompt.contains("xAI"), "no vendor leakage");
    }

    #[test]
    fn strip_identity_also_drops_the_user_guide_when_interactive() {
        let mut c = ctx(false);
        c.strip_identity = true;
        let prompt = render_base_prompt(&c);
        assert!(prompt.starts_with("You are an interactive CLI tool"));
        assert!(!prompt.contains("<user_guide>"));
        assert!(!prompt.contains("Grok"), "user_guide named Grok Build");
        assert!(!prompt.contains("xAI"));
    }

    /// Pin that `IDENTITY_SENTENCE` still matches the template's rendering —
    /// if a refresh rewords the opener, this fails instead of `strip_identity`
    /// silently becoming a no-op.
    #[test]
    fn identity_sentence_pin_matches_the_render() {
        let faithful = render_base_prompt(&ctx(true));
        assert!(faithful.starts_with(IDENTITY_SENTENCE));
    }

    // ---- first-user-message shaping (verbatim grok format strings) ----

    #[test]
    fn user_info_block_is_byte_exact() {
        let expected = "<user_info>\n\
                        OS Version: macos\n\
                        Shell: /bin/zsh\n\
                        Workspace Path: /tmp/test\n\
                        Today's date: 2025-01-15\n\
                        Note: Prefer using relative paths over absolute paths as tool call args when possible.\n\
                        </user_info>";
        assert_eq!(user_info_block(&ctx(true)), expected);
    }

    #[test]
    fn user_query_wraps_verbatim() {
        assert_eq!(
            user_query("fix the bug"),
            "<user_query>\nfix the bug\n</user_query>"
        );
    }
}
