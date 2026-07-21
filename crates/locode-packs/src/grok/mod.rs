//! The `grok` pack — a faithful port of Grok Build's `xai-grok-tools` toolset, trimmed
//! to headless-minimal (ADR-0012), over `locode-host`. Task 13 renders its real system
//! prompt; all five grok tools are wired here.

// Verbatim grok port — keeps gb's code shape; lint exceptions scoped to it.
#[allow(clippy::pedantic, clippy::unwrap_used)]
mod confusables;
mod grep;
mod list_dir;
pub mod prompt;
mod read;
mod search_replace;
mod terminal;

use std::sync::Arc;

use locode_host::Host;
use locode_protocol::{ContentBlock, Message, Role};
use locode_tools::Registry;

use crate::pack::{Pack, PackContext};
use grep::GrokGrep;
use list_dir::GrokListDir;
use read::GrokReadFile;
use search_replace::GrokSearchReplace;
use terminal::GrokRunTerminalCmd;

/// The grok harness pack (a zero-sized `&'static` singleton).
#[derive(Debug, Default, Clone, Copy)]
pub struct GrokPack;

impl Pack for GrokPack {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn register(&self, host: &Arc<Host>, registry: &mut Registry) {
        registry.register(
            "run_terminal_cmd",
            GrokRunTerminalCmd::new(Arc::clone(host)),
        );
        registry.register("read_file", GrokReadFile::new(Arc::clone(host)));
        registry.register("search_replace", GrokSearchReplace::new(Arc::clone(host)));
        registry.register("grep", GrokGrep::new(Arc::clone(host)));
        registry.register("list_dir", GrokListDir::new(Arc::clone(host)));
    }

    fn preamble(&self, ctx: &PackContext) -> Vec<Message> {
        // grok's real split (Task 13, ADR-0013): the rendered base prompt is the
        // System item; environment info (cwd/OS/shell/date) is NOT in grok's
        // system prompt — it rides in the first user message as the minimal
        // `<user_info>` prefix (grok's own headless variant). The actual user
        // prompt is wrapped in `<user_query>` by the exec layer (Task 14) via
        // `prompt::user_query`.
        vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: prompt::render_base_prompt(ctx),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: prompt::user_info_block(ctx),
                }],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locode_host::HostConfig;
    use locode_protocol::ResultChunk;
    use locode_tools::ToolCtx;
    use serde_json::json;
    use std::path::Path;
    use tokio_util::sync::CancellationToken;

    /// A grok registry over a fresh temp workspace; the shell runs `bash -c` (non-login)
    /// so login-profile output can't pollute the captured streams. Returns the host's
    /// **canonical** root — the caller must set `ToolCtx.cwd` to match the jail root
    /// (they agree by construction in the engine).
    fn setup() -> (tempfile::TempDir, Registry, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = HostConfig::new(dir.path());
        config.login_shell = false;
        let host = Arc::new(Host::new(config).unwrap());
        let root = host.workspace_root().to_path_buf();
        let registry = GrokPack.build_registry(&host);
        (dir, registry, root)
    }

    fn ctx(dir: &Path) -> ToolCtx {
        ToolCtx::new(
            dir.to_path_buf(),
            "c1".into(),
            dir.to_path_buf(),
            CancellationToken::new(),
        )
    }

    fn result_text(block: &ContentBlock) -> String {
        match block {
            ContentBlock::ToolResult { content, .. } => content
                .iter()
                .filter_map(|chunk| match chunk {
                    ResultChunk::Text { text } => Some(text.clone()),
                    ResultChunk::Image { .. } => None,
                })
                .collect(),
            _ => panic!("expected a tool_result"),
        }
    }

    fn is_error(block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::ToolResult { is_error: true, .. })
    }

    #[tokio::test]
    async fn run_terminal_cmd_echo() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "run_terminal_cmd",
                json!({ "command": "echo hi", "description": "say hi" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        assert_eq!(out.record.output["exit_code"], json!(0));
        let text = result_text(&out.tool_result);
        assert!(text.contains("exit: 0"), "{text}");
        assert!(text.contains("hi"), "{text}");
    }

    #[tokio::test]
    async fn run_terminal_cmd_nonzero_exit_is_soft() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "run_terminal_cmd",
                json!({ "command": "exit 3", "description": "fail" }),
                &ctx(&root),
            )
            .await;
        // Spawn succeeded → ok; the non-zero exit is data, not an error (ADR-0004).
        assert!(out.record.ok);
        assert!(!is_error(&out.tool_result));
        assert_eq!(out.record.output["exit_code"], json!(3));
    }

    #[tokio::test]
    async fn run_terminal_cmd_truncates_front_back_with_grok_markers() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "run_terminal_cmd",
                json!({
                    "command": "for i in $(seq 1 3000); do echo line-$i; done",
                    "description": "spam output"
                }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        let text = result_text(&out.tool_result);
        assert!(
            text.starts_with("exit: 0 [truncated: showing first/last "),
            "{}",
            &text[..80]
        );
        assert!(text.contains(" - full output at: "), "spill path in header");
        assert!(
            text.contains("\n\n... (output truncated) ...\n\n"),
            "grok separator"
        );
        assert!(text.contains("line-1\n"), "front retained");
        assert!(text.contains("line-3000"), "back retained");
        assert_eq!(out.record.output["truncated"], json!(true));
    }

    #[tokio::test]
    async fn run_terminal_cmd_rejects_background_operator() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "run_terminal_cmd",
                json!({ "command": "sleep 5 &", "description": "bg" }),
                &ctx(&root),
            )
            .await;
        assert!(is_error(&out.tool_result));
        assert_eq!(
            result_text(&out.tool_result),
            "Remove the background '&' from your command; background execution is disabled."
        );
    }

    #[tokio::test]
    async fn read_file_numbers_lines() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let out = registry
            .dispatch("read_file", json!({ "target_file": "f.txt" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        // grok's `matches('\n') + 1` semantics: the trailing newline yields a
        // phantom 4th line (gb:442).
        assert_eq!(out.record.output["lines"], json!(4));
        assert_eq!(out.record.output["truncated"], json!(false));
        // Sparse numbering (gb:249-256): only the first visible line is
        // anchored here; lines 2-3 are bare; the phantom renders as `\n`.
        let text = result_text(&out.tool_result);
        assert_eq!(text, "1→alpha\nbeta\ngamma\n");
    }

    #[tokio::test]
    async fn read_file_line_cap_truncates() {
        use std::fmt::Write as _;
        let (_dir, registry, root) = setup();
        let mut big = String::new();
        for n in 1..=1500 {
            writeln!(big, "line {n}").unwrap();
        }
        std::fs::write(root.join("big.txt"), big).unwrap();
        let out = registry
            .dispatch(
                "read_file",
                json!({ "target_file": "big.txt" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        // 1500 lines + trailing newline = 1501 by grok's phantom counting.
        assert_eq!(out.record.output["lines"], json!(1501));
        assert_eq!(out.record.output["truncated"], json!(true));
        // Body holds the first 1000 lines, sparse-numbered: decade anchors
        // only (gb:249-256).
        let text = result_text(&out.tool_result);
        assert!(
            text.starts_with("1→line 1\nline 2\n"),
            "first anchor + bare"
        );
        assert!(text.contains("1000→line 1000"), "capped at 1000");
        assert!(
            !text.contains("990→line 990\nline 991\n991→"),
            "no double anchors"
        );
        assert!(!text.contains("line 1001"), "line 1001 excluded");
    }

    #[tokio::test]
    async fn read_file_not_found_is_soft_error() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "read_file",
                json!({ "target_file": "nope.txt" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert!(is_error(&out.tool_result));
    }

    #[tokio::test]
    async fn read_file_outside_jail_is_soft_error() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "read_file",
                json!({ "target_file": "/etc/passwd" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert!(is_error(&out.tool_result));
    }

    // ---- search_replace ----

    async fn edit(
        registry: &Registry,
        root: &Path,
        args: serde_json::Value,
    ) -> locode_tools::Dispatched {
        registry.dispatch("search_replace", args, &ctx(root)).await
    }

    #[tokio::test]
    async fn search_replace_creates_file_on_empty_old_string() {
        let (_dir, registry, root) = setup();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "new.txt", "old_string": "", "new_string": "hello world" }),
        )
        .await;
        assert!(out.record.ok);
        assert_eq!(out.record.output["created"], json!(true));
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "hello world"
        );
    }

    #[tokio::test]
    async fn search_replace_edits_unique_match() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "alpha beta gamma").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "beta", "new_string": "BETA" }),
        )
        .await;
        assert!(out.record.ok);
        assert_eq!(out.record.output["replacements"], json!(1));
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "alpha BETA gamma"
        );
    }

    #[tokio::test]
    async fn search_replace_no_op_is_soft_error() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "x").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "x", "new_string": "x" }),
        )
        .await;
        assert!(!out.record.ok);
        assert!(result_text(&out.tool_result).contains("same"));
    }

    #[tokio::test]
    async fn search_replace_not_found_is_soft_error() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "abc").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "zzz", "new_string": "q" }),
        )
        .await;
        assert!(!out.record.ok);
        assert!(result_text(&out.tool_result).contains("not found"));
    }

    #[tokio::test]
    async fn search_replace_multiple_matches_without_replace_all_is_soft_error() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "a a a").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "a", "new_string": "b" }),
        )
        .await;
        assert!(!out.record.ok);
        assert!(result_text(&out.tool_result).contains("multiple times"));
    }

    #[tokio::test]
    async fn search_replace_replace_all() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "a a a").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "a", "new_string": "b", "replace_all": true }),
        )
        .await;
        assert!(out.record.ok);
        assert_eq!(out.record.output["replacements"], json!(3));
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "b b b"
        );
    }

    #[tokio::test]
    async fn search_replace_empty_old_string_overwrites_existing() {
        // grok's default: empty old_string OVERWRITES a non-empty file
        // (gb:283-293 + test gb:1264-1282); the Task 11 error was invented.
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "previous content").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "", "new_string": "fresh" }),
        )
        .await;
        assert!(out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "The file f.txt has been created successfully."
        );
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "fresh"
        );
    }

    #[tokio::test]
    async fn search_replace_success_texts_match_grok_current() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "one two one").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "two", "new_string": "TWO" }),
        )
        .await;
        assert_eq!(
            result_text(&out.tool_result),
            "The file f.txt has been updated successfully."
        );
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "one", "new_string": "ONE", "replace_all": true }),
        )
        .await;
        assert_eq!(
            result_text(&out.tool_result),
            "The file f.txt has been updated. All occurrences were successfully replaced."
        );
    }

    #[tokio::test]
    async fn search_replace_no_match_carries_current_era_hints() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "alpha\nthe quick fox\n").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "the quick cat", "new_string": "x" }),
        )
        .await;
        assert!(!out.record.ok);
        let text = result_text(&out.tool_result);
        assert!(
            text.starts_with(
                "The string to replace was not found in the file, use the read_file tool to see the correct string. The user may have changed the file since you last read it."
            ),
            "{text}"
        );
        assert!(
            text.contains("\n\nNearest match: line 2: the quick fox"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn search_replace_crlf_roundtrip() {
        // gb:553-563, 688-692: LF-normalized matching, CRLF re-expanded write.
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "line one\r\nline two\r\n").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "f.txt", "old_string": "one\nline two", "new_string": "1\nline 2" }),
        )
        .await;
        assert!(out.record.ok, "{:?}", result_text(&out.tool_result));
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "line 1\r\nline 2\r\n"
        );
    }

    #[tokio::test]
    async fn search_replace_gitignore_guard_blocks_edit() {
        let (_dir, registry, root) = setup();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(root.join("app.log"), "data").unwrap();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "app.log", "old_string": "data", "new_string": "x" }),
        )
        .await;
        assert!(!out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "Error: app.log is ignored by .gitignore and cannot be edited."
        );
    }

    #[tokio::test]
    async fn search_replace_not_found_uses_current_text() {
        let (_dir, registry, root) = setup();
        let out = edit(
            &registry,
            &root,
            json!({ "file_path": "missing.txt", "old_string": "a", "new_string": "b" }),
        )
        .await;
        assert!(!out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "Error: missing.txt does not exist."
        );
    }

    // ---- grep (needs `rg`; the happy path is gated on its presence) ----

    fn rg_present() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .is_ok()
    }

    #[tokio::test]
    async fn grep_finds_matches() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "needle here\nother line\n").unwrap();
        std::fs::write(root.join("b.txt"), "nothing\n").unwrap();
        let out = registry
            .dispatch("grep", json!({ "pattern": "needle" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        assert_eq!(out.record.output["matched"], json!(true));
        let text = result_text(&out.tool_result);
        assert!(text.contains("needle"), "{text}");
        assert!(text.contains("a.txt"), "{text}");
    }

    #[tokio::test]
    async fn grep_no_match_is_soft_ok() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();
        let out = registry
            .dispatch("grep", json!({ "pattern": "zzzznotfound" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        assert_eq!(out.record.output["matched"], json!(false));
        assert!(result_text(&out.tool_result).contains("No matches"));
    }

    // ---- list_dir (self-implemented walk; no external binary) ----

    #[tokio::test]
    async fn list_dir_walks_the_tree() {
        let (_dir, registry, root) = setup();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        let out = registry
            .dispatch("list_dir", json!({ "target_directory": "." }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        let text = result_text(&out.tool_result);
        assert!(text.contains("src/"), "{text}");
        assert!(text.contains("main.rs"), "{text}");
        assert!(text.contains("Cargo.toml"), "{text}");
    }

    #[tokio::test]
    async fn list_dir_missing_is_soft_error() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "list_dir",
                json!({ "target_directory": "nope" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert!(is_error(&out.tool_result));
    }
}
