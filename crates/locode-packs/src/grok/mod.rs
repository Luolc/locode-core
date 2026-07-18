//! The `grok` pack — a faithful port of Grok Build's `xai-grok-tools` toolset, trimmed
//! to headless-minimal (ADR-0012), over `locode-host`. Task 13 renders its real system
//! prompt; the remaining tools (`search_replace`, `grep`, `list_dir`) land in Tasks 10-11.

mod read;
mod terminal;

use std::sync::Arc;

use locode_host::Host;
use locode_protocol::{ContentBlock, Message, Role};
use locode_tools::Registry;

use crate::pack::{Pack, PackContext};
use read::GrokReadFile;
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
        // Tasks 10-11 add: search_replace, grep, list_dir.
    }

    fn preamble(&self, ctx: &PackContext) -> Vec<Message> {
        // Scaffold. Task 13 renders grok's real prompt (minijinja) and decides its final
        // System/Developer split. For now: a single `System` message with the
        // headless-branched identity line.
        let identity = if ctx.headless {
            "You are Grok, an autonomous coding agent operating headlessly."
        } else {
            "You are Grok, an interactive coding assistant."
        };
        vec![Message {
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: identity.to_owned(),
            }],
        }]
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
    async fn read_file_numbers_lines() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let out = registry
            .dispatch("read_file", json!({ "target_file": "f.txt" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        assert_eq!(out.record.output["lines"], json!(3));
        assert_eq!(out.record.output["truncated"], json!(false));
        let text = result_text(&out.tool_result);
        assert!(text.contains("1→alpha"), "{text}");
        assert!(text.contains("3→gamma"), "{text}");
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
        assert_eq!(out.record.output["lines"], json!(1500));
        assert_eq!(out.record.output["truncated"], json!(true));
        // Body holds the first 1000 numbered lines only.
        let text = result_text(&out.tool_result);
        assert!(text.contains("1000→line 1000"), "capped at 1000");
        assert!(!text.contains("1001→"), "line 1001 excluded");
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
}
