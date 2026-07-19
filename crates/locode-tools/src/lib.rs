//! locode-tools — the tool framework: the typed [`Tool`] contract, the [`Registry`],
//! and the single [`Registry::dispatch`] door (host-agnostic; ADR-0003/0004/0008).
//!
//! Tools are authored against concrete `Args`/`Output` types, with the wire JSON
//! Schema *derived* from `Args`. The registry erases them to `Box<dyn `[`DynTool`]`>`
//! at its boundary and routes every call — by the pack-assigned wire name — through
//! one `dispatch` function, which returns both the history `tool_result` and the
//! report record. No concrete tools, filesystem, or shell live here (those are the
//! grok pack over `locode-host`).
//!
//! [ADR-0003]: https://github.com/Luolc/locode-core/blob/main/docs/decisions/ADR-0003-typed-tool-contract.md

mod ctx;
mod error;
mod registry;
mod tool;
mod truncate;

pub use ctx::ToolCtx;
pub use error::ToolError;
pub use locode_protocol::{GrammarSyntax, ToolInputFormat, ToolSpec};
pub use registry::{Dispatched, DynTool, Registry, ToolRunResult};
pub use tool::{Tool, ToolKind, ToolOutput};
pub use truncate::{MODEL_OUTPUT_BUDGET, truncate_for_model};

#[cfg(test)]
mod tests {
    // The test tools return `&'static str` literals from `description`; the trait
    // ties it to `&self` so real tools can return a stored field.
    #![allow(clippy::unnecessary_literal_bound)]

    use super::*;
    use async_trait::async_trait;
    use locode_protocol::{ContentBlock, ResultChunk};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    // ---- A trivial echo tool exercising the typed contract ----

    #[derive(Debug, Deserialize, JsonSchema)]
    struct EchoArgs {
        message: String,
    }

    #[derive(Debug, Serialize)]
    struct EchoOut {
        echoed: String,
    }

    impl ToolOutput for EchoOut {
        fn to_prompt_text(&self) -> String {
            self.echoed.clone()
        }
    }

    struct Echo;

    #[async_trait]
    impl Tool for Echo {
        type Args = EchoArgs;
        type Output = EchoOut;

        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }
        fn description(&self) -> &str {
            "echoes its input"
        }
        async fn run(&self, _ctx: &ToolCtx, args: EchoArgs) -> Result<EchoOut, ToolError> {
            Ok(EchoOut {
                echoed: args.message,
            })
        }
    }

    /// A tool that always fails hard — for the fatal path.
    struct Boom;

    #[async_trait]
    impl Tool for Boom {
        type Args = Value;
        type Output = EchoOut;

        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }
        fn description(&self) -> &str {
            "always fatal"
        }
        async fn run(&self, _ctx: &ToolCtx, _args: Value) -> Result<EchoOut, ToolError> {
            Err(ToolError::Fatal("unrecoverable".into()))
        }
    }

    fn ctx(call_id: &str) -> ToolCtx {
        ToolCtx::new(
            PathBuf::from("/repo"),
            call_id.to_owned(),
            PathBuf::from("/repo"),
            CancellationToken::new(),
        )
    }

    fn registry_with_echo() -> Registry {
        let mut reg = Registry::new();
        reg.register("echo", Echo);
        reg
    }

    #[test]
    fn schema_is_derived_from_args() {
        let schema = Echo.parameters_schema();
        assert_eq!(
            schema["properties"]["message"]["type"],
            json!("string"),
            "derived schema should describe the `message: String` arg"
        );
        assert_eq!(schema["required"], json!(["message"]));
    }

    #[tokio::test]
    async fn echo_round_trips_output_and_prompt_text() {
        let reg = registry_with_echo();
        let out = reg
            .dispatch("echo", json!({ "message": "hi" }), &ctx("c1"))
            .await;

        // History view: the model reads the prompt_text.
        let ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = &out.tool_result
        else {
            panic!("expected a tool_result block");
        };
        assert_eq!(tool_use_id, "c1");
        assert!(!is_error);
        assert_eq!(content, &vec![ResultChunk::Text { text: "hi".into() }]);

        // Report view: the structured output, independent of the text.
        assert!(out.record.ok);
        assert_eq!(out.record.name, "echo");
        assert_eq!(out.record.kind, "shell");
        assert_eq!(out.record.output, json!({ "echoed": "hi" }));
        assert!(out.fatal.is_none());
    }

    #[tokio::test]
    async fn bad_args_are_soft() {
        let reg = registry_with_echo();
        // `message` missing → decode fails → Respond, not a panic or fatal.
        let out = reg.dispatch("echo", json!({}), &ctx("c1")).await;

        let ContentBlock::ToolResult { is_error, .. } = &out.tool_result else {
            panic!("expected a tool_result block");
        };
        assert!(is_error, "bad args should be a soft is_error result");
        assert!(!out.record.ok);
        assert!(out.fatal.is_none());
    }

    #[tokio::test]
    async fn unknown_tool_is_soft() {
        let reg = registry_with_echo();
        let out = reg.dispatch("nope", json!({}), &ctx("c1")).await;

        let ContentBlock::ToolResult { is_error, .. } = &out.tool_result else {
            panic!("expected a tool_result block");
        };
        assert!(is_error);
        assert!(!out.record.ok);
        assert_eq!(out.record.kind, "other");
        assert!(out.fatal.is_none());
    }

    #[tokio::test]
    async fn fatal_sets_flag_and_still_pairs() {
        let mut reg = Registry::new();
        reg.register("boom", Boom);
        let out = reg.dispatch("boom", json!({}), &ctx("c9")).await;

        // Paired result exists even on fatal (transcript stays valid).
        let ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } = &out.tool_result
        else {
            panic!("expected a tool_result block");
        };
        assert_eq!(tool_use_id, "c9");
        assert!(is_error);
        assert_eq!(out.fatal.as_deref(), Some("unrecoverable"));
    }

    #[test]
    #[should_panic(expected = "duplicate tool registration")]
    fn duplicate_registration_panics() {
        let mut reg = Registry::new();
        reg.register("echo", Echo);
        reg.register("echo", Echo);
    }

    // ---- The MCP seam: a DynTool implemented directly, no compile-time Args ----

    struct FakeMcpTool;

    #[async_trait]
    impl DynTool for FakeMcpTool {
        fn kind(&self) -> ToolKind {
            ToolKind::Other
        }
        fn description(&self) -> &str {
            "a dynamically-described tool"
        }
        fn parameters_schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn call(&self, _ctx: &ToolCtx, raw_args: Value) -> Result<ToolRunResult, ToolError> {
            Ok(ToolRunResult {
                output: raw_args.clone(),
                prompt_text: raw_args.to_string(),
            })
        }
    }

    #[tokio::test]
    async fn register_dyn_supports_mcp_like_tools() {
        let mut reg = Registry::new();
        reg.register_dyn("mcp__thing", Box::new(FakeMcpTool));
        assert!(reg.contains("mcp__thing"));

        let out = reg
            .dispatch("mcp__thing", json!({ "x": 1 }), &ctx("m1"))
            .await;
        assert!(out.record.ok);
        assert_eq!(out.record.kind, "other");
        assert_eq!(out.record.output, json!({ "x": 1 }));
    }

    #[test]
    fn tool_kind_key_matches_serde() {
        for kind in [
            ToolKind::Shell,
            ToolKind::Read,
            ToolKind::Write,
            ToolKind::Edit,
            ToolKind::Glob,
            ToolKind::Grep,
            ToolKind::Other,
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        }
    }
}
