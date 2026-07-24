//! The `claude` pack — a faithful port of Claude Code's headless-relevant toolset
//! plus its static system prompt (ADR-0012, ADR-0023), over `locode-host`. The
//! highest-value A/B counterpart to the grok pack: same engine, same wire,
//! genuinely different tool surface.
//!
//! Slice 1 (this file's current state): the pack scaffold + `Bash` + a minimal
//! system prompt (identity + intro). Read/Edit/Write (+ the `ClaudeSessionState`
//! read-before-edit gate), Glob, Grep, and the full byte-exact prompt land in
//! later slices — see `docs/claude-pack-dev-process.md`.
//!
//! Fidelity boundary (ADR-0023): the pack reproduces tools + prompt + static
//! preamble only. Loop-adjacent machinery (project-instruction loading, reminder
//! re-injection, compaction, subagents) stays on the shared engine.

mod bash;
pub mod prompt;
mod read;
mod state;

use std::sync::Arc;

use locode_host::Host;
use locode_protocol::{ContentBlock, Message, Role};
use locode_tools::Registry;

use crate::pack::{Pack, PackContext};
use bash::ClaudeBash;
use read::ClaudeRead;
use state::ClaudeSessionState;

/// The Claude Code harness pack (a zero-sized `&'static` singleton for Slice 1;
/// gains a per-run `ClaudeSessionState` when Read/Edit/Write land).
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudePack;

impl Pack for ClaudePack {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn register(&self, host: &Arc<Host>, registry: &mut Registry) {
        // Per-run freshness store (CC's per-session `readFileState`): Read records
        // into it; Edit/Write (S3/S4) gate on it. Cloned into each tool that shares it.
        let state = Arc::new(ClaudeSessionState::default());
        // Claude Code's exact UpperCamelCase wire names (contrast grok's snake_case).
        registry.register("Bash", ClaudeBash::new(Arc::clone(host)));
        registry.register(
            "Read",
            ClaudeRead::new(Arc::clone(host), Arc::clone(&state)),
        );
    }

    fn preamble(&self, ctx: &PackContext) -> Vec<Message> {
        // Slice 1: a single System message (the minimal render). Slice 7 adds the
        // env block + the `<system-reminder>` currentDate User message (D10). CC
        // sends the raw user prompt (no wrapper) — see `shape_user_prompt`.
        vec![Message {
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: prompt::render(ctx),
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

    /// A claude registry over a fresh temp workspace; `bash -c` (non-login) keeps
    /// login-profile output out of the captured streams. Returns the host's
    /// canonical root (the jail root the `ToolCtx.cwd` must match).
    fn setup() -> (tempfile::TempDir, Registry, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = HostConfig::new(dir.path());
        config.login_shell = false;
        let host = Arc::new(Host::new(config).unwrap());
        let root = host.workspace_root().to_path_buf();
        let registry = ClaudePack.build_registry(&host);
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

    #[test]
    fn pack_registers_expected_tools_this_slice() {
        let (_dir, registry, _root) = setup();
        let mut names: Vec<&str> = registry.names().collect();
        names.sort_unstable();
        assert_eq!(names, vec!["Bash", "Read"]);
        assert_eq!(
            registry.kind_of("Bash"),
            Some(locode_tools::ToolKind::Shell)
        );
        assert_eq!(registry.kind_of("Read"), Some(locode_tools::ToolKind::Read));
    }

    #[test]
    fn bash_schema_is_faithful() {
        let (_dir, registry, _root) = setup();
        let specs = registry.specs();
        let bash = specs.iter().find(|s| s.name == "Bash").expect("Bash spec");
        let params = match &bash.input {
            locode_protocol::ToolInputFormat::JsonSchema { parameters } => parameters,
            locode_protocol::ToolInputFormat::Freeform { .. } => panic!("Bash is JSON-schema"),
        };
        // z.strictObject → additionalProperties:false (deny_unknown_fields).
        assert_eq!(params["additionalProperties"], json!(false));
        let props = params["properties"].as_object().unwrap();
        // Present: command, timeout, description (verbatim field descriptions).
        assert_eq!(
            props["command"]["description"],
            json!("The command to execute")
        );
        assert_eq!(
            props["timeout"]["description"],
            json!("Optional timeout in milliseconds (max 600000)")
        );
        assert!(
            props["description"]["description"]
                .as_str()
                .unwrap()
                .starts_with("Clear, concise description of what this command does")
        );
        // Absent (faithfully omitted): background/sandbox/internal fields.
        for absent in [
            "run_in_background",
            "dangerouslyDisableSandbox",
            "_simulatedSedEdit",
        ] {
            assert!(!props.contains_key(absent), "{absent} must be absent");
        }
        // Only `command` is required.
        let required: Vec<&str> = params["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["command"]);
    }

    #[test]
    fn shape_user_prompt_is_verbatim_for_claude() {
        // CC sends the raw prompt (no <user_query> wrapper); grok wraps it.
        assert_eq!(ClaudePack.shape_user_prompt("do the thing"), "do the thing");
        assert_ne!(
            crate::GrokPack.shape_user_prompt("do the thing"),
            "do the thing"
        );
    }

    #[test]
    fn preamble_is_a_single_system_message() {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(Host::new(HostConfig::new(dir.path())).unwrap());
        let _ = host;
        let pc = PackContext {
            cwd: dir.path().to_path_buf(),
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            date: "2026-07-24".into(),
            headless: true,
            strip_identity: false,
        };
        let msgs = ClaudePack.preamble(&pc);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::System);
    }

    // ---- Bash behavior (via build_registry + dispatch over a tempdir host) ----

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_echo_ok() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch("Bash", json!({ "command": "echo hi" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        assert!(!is_error(&out.tool_result));
        assert_eq!(result_text(&out.tool_result), "hi");
        assert_eq!(out.record.output["exit_code"], json!(0));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_nonzero_exit_is_soft_ok_with_exit_code_note() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "Bash",
                json!({ "command": "echo oops; exit 3" }),
                &ctx(&root),
            )
            .await;
        // CC: is_error is false for a non-zero exit; "Exit code N" is appended.
        assert!(out.record.ok);
        assert!(!is_error(&out.tool_result));
        let text = result_text(&out.tool_result);
        assert_eq!(text, "oops\nExit code 3", "{text}");
        assert_eq!(out.record.output["exit_code"], json!(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_timeout_is_interrupted_soft_error() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "Bash",
                json!({ "command": "sleep 5", "timeout": 50 }),
                &ctx(&root),
            )
            .await;
        // CC sets is_error when interrupted (timeout).
        assert!(is_error(&out.tool_result));
        assert!(
            result_text(&out.tool_result).contains("Command was aborted before completion"),
            "{}",
            result_text(&out.tool_result)
        );
        // The error path carries the message, not a structured output record.
        assert!(!out.record.ok);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_large_output_is_middle_truncated() {
        let (_dir, registry, root) = setup();
        // ~40k chars of output overflows the 30k maxResultSizeChars cap.
        let out = registry
            .dispatch(
                "Bash",
                json!({ "command": "for i in $(seq 1 8000); do echo LINE$i; done" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        let text = result_text(&out.tool_result);
        // The tool caps at 30k (front/back over the merged stream); the engine
        // dispatch-door belt (MODEL_OUTPUT_BUDGET=30k, ADR-0008) truncates on top
        // and supplies the visible marker. Either way the model gets head + tail
        // + a truncation marker.
        assert!(
            text.contains("truncated"),
            "carries a truncation marker: {}",
            &text[..60]
        );
        assert!(text.contains("LINE1\n"), "head retained: {}", &text[..40]);
        assert!(text.contains("LINE8000"), "tail retained");
        assert_eq!(out.record.output["truncated"], json!(true));
    }

    // ---- Read behavior ----

    #[tokio::test]
    async fn read_numbers_lines_cat_n_compact() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let out = registry
            .dispatch("Read", json!({ "file_path": "f.txt" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        // Compact cat -n: `N\tline`, 1-indexed; trailing newline adds no phantom.
        assert_eq!(result_text(&out.tool_result), "1\talpha\n2\tbeta\n3\tgamma");
        assert_eq!(out.record.output["lines"], json!(3));
        assert_eq!(out.record.output["truncated"], json!(false));
    }

    #[tokio::test]
    async fn read_offset_and_limit_window() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "l1\nl2\nl3\nl4\nl5\n").unwrap();
        let out = registry
            .dispatch(
                "Read",
                json!({ "file_path": "f.txt", "offset": 2, "limit": 2 }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        // Absolute line numbers preserved (2,3); window is a subset → truncated.
        assert_eq!(result_text(&out.tool_result), "2\tl2\n3\tl3");
        assert_eq!(out.record.output["truncated"], json!(true));
    }

    #[tokio::test]
    async fn read_empty_file_warns() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("e.txt"), "").unwrap();
        let out = registry
            .dispatch("Read", json!({ "file_path": "e.txt" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>"
        );
    }

    #[tokio::test]
    async fn read_offset_past_eof_warns() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "a\nb\n").unwrap();
        let out = registry
            .dispatch(
                "Read",
                json!({ "file_path": "f.txt", "offset": 99 }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "<system-reminder>Warning: the file exists but is shorter than the provided offset (99). The file has 2 lines.</system-reminder>"
        );
    }

    #[tokio::test]
    async fn read_missing_file_is_soft_error() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch("Read", json!({ "file_path": "nope.txt" }), &ctx(&root))
            .await;
        assert!(!out.record.ok);
        assert!(is_error(&out.tool_result));
    }

    #[tokio::test]
    async fn read_dedup_unchanged_then_rereads_after_change() {
        let (_dir, registry, root) = setup();
        let p = root.join("f.txt");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        // First read: full content.
        let first = registry
            .dispatch("Read", json!({ "file_path": "f.txt" }), &ctx(&root))
            .await;
        assert_eq!(result_text(&first.tool_result), "1\tone\n2\ttwo");
        // Re-read unchanged → stub.
        let second = registry
            .dispatch("Read", json!({ "file_path": "f.txt" }), &ctx(&root))
            .await;
        assert!(second.record.ok);
        assert!(result_text(&second.tool_result).starts_with("File unchanged since last read."));
        // Change the file (bump mtime well past ms granularity) → full re-read.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        let third = registry
            .dispatch("Read", json!({ "file_path": "f.txt" }), &ctx(&root))
            .await;
        assert_eq!(result_text(&third.tool_result), "1\tone\n2\ttwo\n3\tthree");
    }

    #[test]
    fn read_schema_is_faithful() {
        let (_dir, registry, _root) = setup();
        let specs = registry.specs();
        let read = specs.iter().find(|s| s.name == "Read").expect("Read spec");
        let params = match &read.input {
            locode_protocol::ToolInputFormat::JsonSchema { parameters } => parameters,
            locode_protocol::ToolInputFormat::Freeform { .. } => panic!("Read is JSON-schema"),
        };
        assert_eq!(params["additionalProperties"], json!(false));
        let props = params["properties"].as_object().unwrap();
        assert_eq!(
            props["file_path"]["description"],
            json!("The absolute path to the file to read")
        );
        for key in ["file_path", "offset", "limit", "pages"] {
            assert!(props.contains_key(key), "missing schema field {key}");
        }
        let required: Vec<&str> = params["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["file_path"]);
    }

    #[test]
    fn read_offset_rejects_string_and_float() {
        // Type-strict (repo policy): no lenient coercion.
        for bad in [
            json!({"file_path": "f", "offset": "3"}),
            json!({"file_path": "f", "limit": 2.5}),
        ] {
            assert!(serde_json::from_value::<read::ReadArgs>(bad).is_err());
        }
    }

    #[tokio::test]
    async fn bash_rejects_unknown_field() {
        let (_dir, registry, root) = setup();
        // deny_unknown_fields (z.strictObject): run_in_background is not in our schema.
        let out = registry
            .dispatch(
                "Bash",
                json!({ "command": "echo hi", "run_in_background": true }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok, "unknown field rejected");
        assert!(is_error(&out.tool_result));
    }
}
