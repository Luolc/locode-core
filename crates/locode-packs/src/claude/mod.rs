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

use std::sync::Arc;

use locode_host::Host;
use locode_protocol::{ContentBlock, Message, Role};
use locode_tools::Registry;

use crate::pack::{Pack, PackContext};
use bash::ClaudeBash;

/// The Claude Code harness pack (a zero-sized `&'static` singleton for Slice 1;
/// gains a per-run `ClaudeSessionState` when Read/Edit/Write land).
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudePack;

impl Pack for ClaudePack {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn register(&self, host: &Arc<Host>, registry: &mut Registry) {
        // Claude Code's exact UpperCamelCase wire names (contrast grok's snake_case).
        registry.register("Bash", ClaudeBash::new(Arc::clone(host)));
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
    fn pack_registers_exactly_bash_this_slice() {
        let (_dir, registry, _root) = setup();
        let mut names: Vec<&str> = registry.names().collect();
        names.sort_unstable();
        assert_eq!(names, vec!["Bash"]);
        assert_eq!(
            registry.kind_of("Bash"),
            Some(locode_tools::ToolKind::Shell)
        );
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
