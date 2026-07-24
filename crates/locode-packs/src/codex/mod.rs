//! The `codex` pack — a faithful port of Codex CLI's stock headless tool surface +
//! base prompt (ADR-0012, ADR-0023), over `locode-host`. Codex is the **minimal-tool
//! extreme**: no read/grep/glob/write/edit tools — the shell is the read path and all
//! editing goes through `apply_patch`.
//!
//! Current state (Slice 2): the pack scaffold + `shell_command` + freeform
//! `apply_patch` + a minimal prompt (identity opener) + the always-appended
//! `apply_patch` instructions block + the openai-responses-only wire requirement.
//! The full gpt-5.6-sol prompt + `<environment_context>` preamble (Slice 3) land
//! next — see `docs/codex-pack-dev-process.md`.
//!
//! Fidelity boundary (ADR-0023): the pack reproduces tools + prompt + static preamble
//! only. `update_plan`, unified exec (PTY/background), the code-mode wrapper, subagents,
//! skills, MCP, and compaction stay on the shared engine / are deferred.

mod apply_patch;
mod prompt;
mod shell_command;

use std::sync::Arc;

use locode_host::Host;
use locode_protocol::{ContentBlock, Message, Role};
use locode_tools::Registry;

use apply_patch::CodexApplyPatch;
use shell_command::CodexShellCommand;

use crate::pack::{Pack, PackContext};

/// Codex's `apply_patch` instructions block (`prompts/templates/apply_patch_tool_instructions.md`,
/// submodule `f201c30c`, 3084 bytes). Codex appends this to the base instructions
/// **only when the freeform tool is absent** (`core/tests/suite/prompt_caching.rs:212`).
/// We **always append** it (D4): we run non-codex models whose base prompt does not
/// bake in the V4A format, and appending "肯定不会让它变更差". The faithful append *form*
/// (join into the system instructions with `\n`) is preserved.
const APPLY_PATCH_INSTRUCTIONS: &str = include_str!("templates/apply_patch_tool_instructions.md");

/// The Codex CLI harness pack (a zero-sized `&'static` singleton).
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexPack;

impl Pack for CodexPack {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn register(&self, host: &Arc<Host>, registry: &mut Registry) {
        // Codex's stock non-Windows duo: the shell (unified exec disabled — see the
        // deprecated-substitution note in `shell_command.rs`, D2) + freeform apply_patch
        // (codex's only edit path — no read/write/edit tools).
        registry.register("shell_command", CodexShellCommand::new(Arc::clone(host)));
        registry.register("apply_patch", CodexApplyPatch::new(Arc::clone(host)));
    }

    fn required_api_schemas(&self) -> Option<&'static [&'static str]> {
        // `apply_patch` is a freeform (custom-grammar) tool that only round-trips on
        // the OpenAI Responses wire (D5). `mock` stays allowed (keyless CI) via the
        // universal escape hatch in the pre-run check.
        Some(&["openai-responses"])
    }

    fn preamble(&self, ctx: &PackContext) -> Vec<Message> {
        // A single System message = the base prompt + the always-appended apply_patch
        // instructions (D4/D7), joined with `\n` (codex's own append form). Slice 3
        // adds the `<environment_context>` User item and swaps in the full prompt.
        // Codex sends the base prompt as `instructions` on the Responses wire; our
        // System hoists there (Task 18).
        let text = format!("{}\n{}", prompt::render(ctx), APPLY_PATCH_INSTRUCTIONS);
        vec![Message {
            role: Role::System,
            content: vec![ContentBlock::Text { text }],
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

    fn setup() -> (tempfile::TempDir, Registry, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = HostConfig::new(dir.path());
        config.login_shell = false;
        let host = Arc::new(Host::new(config).unwrap());
        let root = host.workspace_root().to_path_buf();
        let registry = CodexPack.build_registry(&host);
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

    #[test]
    fn pack_registers_shell_command_and_apply_patch() {
        let (_dir, registry, _root) = setup();
        let mut names: Vec<&str> = registry.names().collect();
        names.sort_unstable();
        assert_eq!(names, vec!["apply_patch", "shell_command"]);
        assert_eq!(
            registry.kind_of("shell_command"),
            Some(locode_tools::ToolKind::Shell)
        );
        assert_eq!(
            registry.kind_of("apply_patch"),
            Some(locode_tools::ToolKind::Edit)
        );
    }

    #[test]
    fn shell_command_schema_is_faithful() {
        let (_dir, registry, _root) = setup();
        let specs = registry.specs();
        let spec = specs
            .iter()
            .find(|s| s.name == "shell_command")
            .expect("shell_command spec");
        let params = match &spec.input {
            locode_protocol::ToolInputFormat::JsonSchema { parameters } => parameters,
            locode_protocol::ToolInputFormat::Freeform { .. } => panic!("JSON-schema tool"),
        };
        assert_eq!(params["additionalProperties"], json!(false));
        let props = params["properties"].as_object().unwrap();
        assert_eq!(
            props["command"]["description"],
            json!("Shell script to run in the user's default shell.")
        );
        for key in ["command", "workdir", "timeout_ms", "login"] {
            assert!(props.contains_key(key), "missing field {key}");
        }
        // Approval params dropped (D8).
        for absent in ["sandbox_permissions", "justification", "prefix_rule"] {
            assert!(!props.contains_key(absent), "{absent} must be absent");
        }
        let required: Vec<&str> = params["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["command"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_command_echo_frames_output() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "shell_command",
                json!({ "command": "echo hi" }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        let text = result_text(&out.tool_result);
        assert!(text.starts_with("Exit code: 0\n"), "{text}");
        assert!(text.contains("\nWall time: "), "{text}");
        assert!(text.contains("\nOutput:\nhi"), "{text}");
        assert_eq!(out.record.output["exit_code"], json!(0));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_command_nonzero_exit_is_soft_ok() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "shell_command",
                json!({ "command": "echo oops; exit 3" }),
                &ctx(&root),
            )
            .await;
        // Codex marks success:true; the model reads the exit code in the text.
        assert!(out.record.ok);
        assert!(result_text(&out.tool_result).contains("Exit code: 3"));
        assert_eq!(out.record.output["exit_code"], json!(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_command_timeout_is_exit_124() {
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "shell_command",
                json!({ "command": "sleep 5", "timeout_ms": 50 }),
                &ctx(&root),
            )
            .await;
        assert!(out.record.ok);
        let text = result_text(&out.tool_result);
        assert!(text.contains("Exit code: 124"), "{text}");
        assert!(text.contains("command timed out after"), "{text}");
        assert_eq!(out.record.output["timed_out"], json!(true));
    }

    #[tokio::test]
    async fn shell_command_rejects_unknown_field() {
        // deny_unknown_fields: the dropped approval param is rejected.
        let (_dir, registry, root) = setup();
        let out = registry
            .dispatch(
                "shell_command",
                json!({ "command": "echo hi", "sandbox_permissions": "use_default" }),
                &ctx(&root),
            )
            .await;
        assert!(!out.record.ok);
    }

    #[test]
    fn preamble_appends_apply_patch_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let pc = PackContext {
            cwd: dir.path().to_path_buf(),
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            date: "2026-07-24".into(),
            headless: true,
            is_git_repo: false,
            model: Some("gpt-5.6-sol".into()),
            os_version: None,
            strip_identity: false,
        };
        let msgs = CodexPack.preamble(&pc);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::System);
        // A single System message carrying BOTH the identity render AND the
        // always-appended apply_patch instructions block (D4).
        let ContentBlock::Text { text } = &msgs[0].content[0] else {
            panic!("expected text");
        };
        assert!(text.starts_with("You are Codex"), "{text}");
        assert!(text.contains("## `apply_patch`"), "instructions appended");
        assert!(
            text.contains("*** Begin Patch"),
            "instructions body present"
        );
    }

    #[test]
    fn codex_requires_the_responses_wire() {
        assert_eq!(
            CodexPack.required_api_schemas(),
            Some(&["openai-responses"][..])
        );
    }
}
