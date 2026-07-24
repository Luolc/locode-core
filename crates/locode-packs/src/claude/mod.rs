//! The `claude` pack — a faithful port of Claude Code's headless-relevant toolset
//! plus its static system prompt (ADR-0012, ADR-0023), over `locode-host`. The
//! highest-value A/B counterpart to the grok pack: same engine, same wire,
//! genuinely different tool surface.
//!
//! Current state (through Slice 6): all six tools — `Bash`, `Read`, `Edit`,
//! `Write` (the middle three sharing the `ClaudeSessionState` read-before-write +
//! staleness gate), `Glob`, and `Grep` — plus a minimal system prompt (identity +
//! intro). The full byte-exact prompt lands in Slice 7 — see `docs/claude-pack-dev-process.md`.
//!
//! Fidelity boundary (ADR-0023): the pack reproduces tools + prompt + static
//! preamble only. Loop-adjacent machinery (project-instruction loading, reminder
//! re-injection, compaction, subagents) stays on the shared engine.

mod bash;
mod edit;
mod glob;
mod grep;
pub mod prompt;
mod read;
mod state;
mod write;

use std::sync::Arc;

use locode_host::Host;
use locode_protocol::{ContentBlock, Message, Role};
use locode_tools::Registry;

use crate::pack::{Pack, PackContext};
use bash::ClaudeBash;
use edit::ClaudeEdit;
use glob::ClaudeGlob;
use grep::ClaudeGrep;
use read::ClaudeRead;
use state::ClaudeSessionState;
use write::ClaudeWrite;

/// The Claude Code harness pack (a zero-sized `&'static` singleton; the per-run
/// `ClaudeSessionState` lives in the tools that share it, constructed in `register`).
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
        registry.register(
            "Edit",
            ClaudeEdit::new(Arc::clone(host), Arc::clone(&state)),
        );
        registry.register(
            "Write",
            ClaudeWrite::new(Arc::clone(host), Arc::clone(&state)),
        );
        registry.register("Glob", ClaudeGlob::new(Arc::clone(host)));
        registry.register("Grep", ClaudeGrep::new(Arc::clone(host)));
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
        assert_eq!(names, vec!["Bash", "Edit", "Glob", "Grep", "Read", "Write"]);
        assert_eq!(
            registry.kind_of("Bash"),
            Some(locode_tools::ToolKind::Shell)
        );
        assert_eq!(registry.kind_of("Read"), Some(locode_tools::ToolKind::Read));
        assert_eq!(registry.kind_of("Edit"), Some(locode_tools::ToolKind::Edit));
        assert_eq!(
            registry.kind_of("Write"),
            Some(locode_tools::ToolKind::Write)
        );
        assert_eq!(registry.kind_of("Glob"), Some(locode_tools::ToolKind::Glob));
        assert_eq!(registry.kind_of("Grep"), Some(locode_tools::ToolKind::Grep));
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

    // ---- Edit: the read-before-edit + staleness gate is the star ----

    async fn read_then(registry: &Registry, root: &Path, file: &str) {
        let _ = registry
            .dispatch("Read", json!({ "file_path": file }), &ctx(root))
            .await;
    }

    #[tokio::test]
    async fn edit_requires_prior_read() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "alpha beta").unwrap();
        // No Read first → the gate rejects (errorCode 6).
        let out = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "beta", "new_string": "BETA" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "File has not been read yet. Read it first before writing to it."
        );
    }

    #[tokio::test]
    async fn edit_after_read_succeeds_and_sequential_edit_is_fresh() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "one two three").unwrap();
        read_then(&registry, &root, "f.txt").await;
        let out = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "two", "new_string": "TWO" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok, "{}", result_text(&out.tool_result));
        assert_eq!(
            result_text(&out.tool_result),
            "The file f.txt has been updated successfully."
        );
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "one TWO three"
        );
        // A second edit without re-reading is still fresh (Edit recorded post-edit mtime).
        let out2 = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "one", "new_string": "ONE" }),
                &ctx(&root),
            )
            .await;
        assert!(out2.record.ok, "{}", result_text(&out2.tool_result));
    }

    #[tokio::test]
    async fn edit_stale_after_external_modification_is_rejected() {
        let (_dir, registry, root) = setup();
        let p = root.join("f.txt");
        std::fs::write(&p, "hello world").unwrap();
        read_then(&registry, &root, "f.txt").await;
        // External modification bumps mtime past the recorded read.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p, "hello there").unwrap();
        let out = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "hello", "new_string": "HI" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert!(
            result_text(&out.tool_result).starts_with("File has been modified since read"),
            "{}",
            result_text(&out.tool_result)
        );
    }

    #[tokio::test]
    async fn edit_no_op_not_found_and_multi_match() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "a a a").unwrap();
        read_then(&registry, &root, "f.txt").await;
        // old == new (errorCode 1) — checked before the gate.
        let noop = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "a", "new_string": "a" }),
                &ctx(&root),
            )
            .await;
        assert!(!noop.record.ok);
        assert!(result_text(&noop.tool_result).contains("exactly the same"));
        // not found (errorCode 8).
        let nf = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "zzz", "new_string": "q" }),
                &ctx(&root),
            )
            .await;
        assert!(!nf.record.ok);
        assert!(result_text(&nf.tool_result).starts_with("String to replace not found in file."));
        // multi-match without replace_all (errorCode 9).
        let mm = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "a", "new_string": "b" }),
                &ctx(&root),
            )
            .await;
        assert!(!mm.record.ok);
        assert!(result_text(&mm.tool_result).starts_with("Found 3 matches"));
        // replace_all succeeds.
        let all = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "a", "new_string": "b", "replace_all": true }),
                &ctx(&root),
            )
            .await;
        assert!(all.record.ok);
        assert_eq!(
            result_text(&all.tool_result),
            "The file f.txt has been updated. All occurrences were successfully replaced."
        );
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "b b b"
        );
    }

    #[tokio::test]
    async fn edit_creates_file_with_empty_old_string() {
        // CC's Edit creates a new file when old_string is empty (plan §4.3 corrected).
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "new.txt", "old_string": "", "new_string": "fresh content" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok, "{}", result_text(&out.tool_result));
        assert_eq!(out.record.output["created"], json!(true));
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "fresh content"
        );
    }

    #[tokio::test]
    async fn edit_empty_old_string_on_existing_nonempty_is_rejected() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "existing").unwrap();
        let out = registry
            .dispatch(
                "Edit",
                json!({ "file_path": "f.txt", "old_string": "", "new_string": "x" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "Cannot create new file - file already exists."
        );
    }

    // ---- Write: new writes freely; existing must be read first ----

    #[tokio::test]
    async fn write_new_file_creates_freely() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "Write",
                json!({ "file_path": "new.txt", "content": "hello\nworld\n" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok, "{}", result_text(&out.tool_result));
        assert_eq!(
            result_text(&out.tool_result),
            "File created successfully at: new.txt"
        );
        assert_eq!(out.record.output["created"], json!(true));
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "hello\nworld\n"
        );
    }

    #[tokio::test]
    async fn write_existing_unread_is_rejected() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "old").unwrap();
        let out = registry
            .dispatch(
                "Write",
                json!({ "file_path": "f.txt", "content": "new" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "File has not been read yet. Read it first before writing to it."
        );
    }

    #[tokio::test]
    async fn write_existing_after_read_overwrites() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "old content").unwrap();
        read_then(&registry, &root, "f.txt").await;
        let out = registry
            .dispatch(
                "Write",
                json!({ "file_path": "f.txt", "content": "brand new" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok, "{}", result_text(&out.tool_result));
        assert_eq!(
            result_text(&out.tool_result),
            "The file f.txt has been updated successfully."
        );
        assert_eq!(out.record.output["created"], json!(false));
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "brand new"
        );
    }

    #[tokio::test]
    async fn write_stale_after_external_modification_is_rejected() {
        let (_dir, registry, root) = setup();
        let p = root.join("f.txt");
        std::fs::write(&p, "v1").unwrap();
        read_then(&registry, &root, "f.txt").await;
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p, "v2 external").unwrap();
        let out = registry
            .dispatch(
                "Write",
                json!({ "file_path": "f.txt", "content": "v3" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert!(result_text(&out.tool_result).starts_with("File has been modified since read"));
    }

    // ---- Glob (needs `rg`; happy paths gated on its presence) ----

    fn rg_present() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .is_ok()
    }

    #[tokio::test]
    async fn glob_finds_matching_files() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        let out = registry
            .dispatch("Glob", json!({ "pattern": "**/*.rs" }), &ctx(&root))
            .await;
        assert!(out.record.ok, "{}", result_text(&out.tool_result));
        let text = result_text(&out.tool_result);
        assert!(text.contains("src/main.rs"), "{text}");
        assert!(text.contains("src/lib.rs"), "{text}");
        assert!(!text.contains("README.md"), "{text}");
        assert_eq!(out.record.output["truncated"], json!(false));
    }

    #[tokio::test]
    async fn glob_no_match_says_no_files_found() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "").unwrap();
        let out = registry
            .dispatch("Glob", json!({ "pattern": "**/*.zzz" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        assert_eq!(result_text(&out.tool_result), "No files found");
    }

    #[tokio::test]
    async fn glob_path_missing_is_soft_error() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "Glob",
                json!({ "pattern": "*", "path": "nope" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert!(result_text(&out.tool_result).starts_with("Directory does not exist: nope."));
    }

    #[tokio::test]
    async fn glob_path_is_a_file_is_soft_error() {
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("f.txt"), "x").unwrap();
        let out = registry
            .dispatch(
                "Glob",
                json!({ "pattern": "*", "path": "f.txt" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
        assert_eq!(
            result_text(&out.tool_result),
            "Path is not a directory: f.txt"
        );
    }

    // ---- Grep (needs `rg`; happy paths gated on its presence) ----

    #[tokio::test]
    async fn grep_files_with_matches_default() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "needle here\n").unwrap();
        std::fs::write(root.join("b.txt"), "nothing\n").unwrap();
        let out = registry
            .dispatch("Grep", json!({ "pattern": "needle" }), &ctx(&root))
            .await;
        assert!(out.record.ok, "{}", result_text(&out.tool_result));
        let text = result_text(&out.tool_result);
        assert!(text.starts_with("Found 1 file"), "{text}");
        assert!(text.contains("a.txt"), "{text}");
        assert!(!text.contains("b.txt"), "{text}");
    }

    #[tokio::test]
    async fn grep_no_match_files_mode() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();
        let out = registry
            .dispatch("Grep", json!({ "pattern": "zzznope" }), &ctx(&root))
            .await;
        assert!(out.record.ok);
        assert_eq!(result_text(&out.tool_result), "No files found");
    }

    #[tokio::test]
    async fn grep_content_mode_with_line_numbers() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "one\nneedle two\nthree\n").unwrap();
        let out = registry
            .dispatch(
                "Grep",
                json!({ "pattern": "needle", "output_mode": "content" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        let text = result_text(&out.tool_result);
        // Line numbers on by default in content mode; path relativized.
        assert!(text.contains("a.txt:2:needle two"), "{text}");
    }

    #[tokio::test]
    async fn grep_content_no_match() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();
        let out = registry
            .dispatch(
                "Grep",
                json!({ "pattern": "zzz", "output_mode": "content" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        assert_eq!(result_text(&out.tool_result), "No matches found");
    }

    #[tokio::test]
    async fn grep_count_mode_summary() {
        if !rg_present() {
            return;
        }
        let (_dir, registry, root) = setup();
        std::fs::write(root.join("a.txt"), "x\nx\nx\n").unwrap();
        let out = registry
            .dispatch(
                "Grep",
                json!({ "pattern": "x", "output_mode": "count" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        let text = result_text(&out.tool_result);
        assert!(
            text.contains("Found 3 total occurrences across 1 file."),
            "{text}"
        );
        assert_eq!(out.record.output["num_matches"], json!(3));
    }

    #[test]
    fn grep_schema_is_faithful() {
        let (_dir, registry, _root) = setup();
        let specs = registry.specs();
        let grep = specs.iter().find(|s| s.name == "Grep").expect("Grep spec");
        let params = match &grep.input {
            locode_protocol::ToolInputFormat::JsonSchema { parameters } => parameters,
            locode_protocol::ToolInputFormat::Freeform { .. } => panic!("Grep is JSON-schema"),
        };
        assert_eq!(params["additionalProperties"], json!(false));
        let props = params["properties"].as_object().unwrap();
        // The rg-flag-named fields keep their exact wire keys.
        for key in [
            "pattern",
            "path",
            "glob",
            "output_mode",
            "-A",
            "-B",
            "-C",
            "context",
            "-n",
            "-i",
            "type",
            "head_limit",
            "offset",
            "multiline",
        ] {
            assert!(props.contains_key(key), "missing schema field {key}");
        }
        assert_eq!(
            props["pattern"]["description"],
            json!("The regular expression pattern to search for in file contents")
        );
        let required: Vec<&str> = params["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["pattern"]);
    }

    #[test]
    fn glob_schema_is_faithful() {
        let (_dir, registry, _root) = setup();
        let specs = registry.specs();
        let glob = specs.iter().find(|s| s.name == "Glob").expect("Glob spec");
        let params = match &glob.input {
            locode_protocol::ToolInputFormat::JsonSchema { parameters } => parameters,
            locode_protocol::ToolInputFormat::Freeform { .. } => panic!("Glob is JSON-schema"),
        };
        assert_eq!(params["additionalProperties"], json!(false));
        let props = params["properties"].as_object().unwrap();
        assert_eq!(
            props["pattern"]["description"],
            json!("The glob pattern to match files against")
        );
        assert!(
            props["path"]["description"]
                .as_str()
                .unwrap()
                .contains("DO NOT enter \"undefined\" or \"null\"")
        );
        let required: Vec<&str> = params["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["pattern"]);
    }

    #[test]
    fn write_schema_is_faithful() {
        let (_dir, registry, _root) = setup();
        let specs = registry.specs();
        let write = specs
            .iter()
            .find(|s| s.name == "Write")
            .expect("Write spec");
        let params = match &write.input {
            locode_protocol::ToolInputFormat::JsonSchema { parameters } => parameters,
            locode_protocol::ToolInputFormat::Freeform { .. } => panic!("Write is JSON-schema"),
        };
        assert_eq!(params["additionalProperties"], json!(false));
        let props = params["properties"].as_object().unwrap();
        assert_eq!(
            props["file_path"]["description"],
            json!("The absolute path to the file to write (must be absolute, not relative)")
        );
        assert_eq!(
            props["content"]["description"],
            json!("The content to write to the file")
        );
        let mut required: Vec<&str> = params["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        required.sort_unstable();
        assert_eq!(required, vec!["content", "file_path"]);
    }

    #[test]
    fn edit_schema_is_faithful() {
        let (_dir, registry, _root) = setup();
        let specs = registry.specs();
        let edit = specs.iter().find(|s| s.name == "Edit").expect("Edit spec");
        let params = match &edit.input {
            locode_protocol::ToolInputFormat::JsonSchema { parameters } => parameters,
            locode_protocol::ToolInputFormat::Freeform { .. } => panic!("Edit is JSON-schema"),
        };
        assert_eq!(params["additionalProperties"], json!(false));
        let props = params["properties"].as_object().unwrap();
        assert_eq!(
            props["new_string"]["description"],
            json!("The text to replace it with (must be different from old_string)")
        );
        for key in ["file_path", "old_string", "new_string", "replace_all"] {
            assert!(props.contains_key(key), "missing schema field {key}");
        }
        let mut required: Vec<&str> = params["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        required.sort_unstable();
        assert_eq!(required, vec!["file_path", "new_string", "old_string"]);
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
