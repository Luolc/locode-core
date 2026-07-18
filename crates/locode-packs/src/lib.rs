//! locode-packs — faithful per-harness toolsets (ADR-0012), one module per harness.
//!
//! A [`Pack`] bundles a harness's real tools (registered under their real wire names)
//! with its base [`Pack::preamble`], selected whole via `--harness`. [`resolve`] maps a
//! `--harness <name>` to its pack. v0 wires the [`GrokPack`]; other packs are the next
//! milestone (one `match`/`static` each).

mod grok;
mod pack;

pub use grok::GrokPack;
pub use pack::{Pack, PackContext};

/// The process-wide grok singleton (zero-sized).
static GROK: GrokPack = GrokPack;

/// Every wired harness, in a stable order (for `--help` and error messages).
const PACKS: &[&(dyn Pack + 'static)] = &[&GROK];

/// The registered `--harness` names, in stable order.
#[must_use]
pub fn available() -> Vec<&'static str> {
    PACKS.iter().map(|pack| pack.name()).collect()
}

/// Resolve a `--harness <name>` to its pack.
///
/// # Errors
/// [`UnknownHarness`] when no pack matches — carries the requested name and the available
/// list so the caller can print a clear, actionable error.
pub fn resolve(name: &str) -> Result<&'static dyn Pack, UnknownHarness> {
    PACKS
        .iter()
        .copied()
        .find(|pack| pack.name() == name)
        .ok_or_else(|| UnknownHarness {
            requested: name.to_owned(),
            available: available(),
        })
}

/// An unknown `--harness` selector — a caller/config error (soft, never a panic).
#[derive(Debug, thiserror::Error)]
#[error("unknown harness `{requested}`; available: {}", .available.join(", "))]
pub struct UnknownHarness {
    /// The name the caller asked for.
    pub requested: String,
    /// The names that ARE registered.
    pub available: Vec<&'static str>,
}

#[cfg(test)]
mod tests {
    // The fake pack's tools return `&'static str` literals from `description`.
    #![allow(clippy::unnecessary_literal_bound)]

    use super::*;
    use async_trait::async_trait;
    use locode_host::{Host, HostConfig};
    use locode_protocol::{ContentBlock, Message, Role};
    use locode_tools::{Registry, Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
    use serde::Serialize;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    /// A host over a fresh temp workspace (the tempdir must outlive the host).
    fn test_host() -> (tempfile::TempDir, Arc<Host>) {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(Host::new(HostConfig::new(dir.path())).unwrap());
        (dir, host)
    }

    // ---- a test-local pack with real typed tools, so the framework is proven
    // deterministically regardless of how grok's real registry grows (Tasks 9-11). ----

    #[derive(Serialize)]
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
        type Args = Value;
        type Output = EchoOut;
        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }
        fn description(&self) -> &str {
            "echo"
        }
        async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<EchoOut, ToolError> {
            Ok(EchoOut {
                echoed: args.to_string(),
            })
        }
    }

    struct FakePack;
    impl Pack for FakePack {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn register(&self, _host: &Arc<Host>, registry: &mut Registry) {
            registry.register("alpha", Echo);
            registry.register("beta", Echo);
        }
        fn preamble(&self, _ctx: &PackContext) -> Vec<Message> {
            vec![Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "fake pack".into(),
                }],
            }]
        }
    }

    struct DupPack;
    impl Pack for DupPack {
        fn name(&self) -> &'static str {
            "dup"
        }
        fn register(&self, _host: &Arc<Host>, registry: &mut Registry) {
            registry.register("alpha", Echo);
            registry.register("alpha", Echo); // collision → panic
        }
        fn preamble(&self, _ctx: &PackContext) -> Vec<Message> {
            Vec::new()
        }
    }

    fn ctx(headless: bool) -> PackContext {
        PackContext {
            cwd: PathBuf::from("/repo"),
            os: "linux".into(),
            shell: "/bin/bash".into(),
            date: "2026-07-18".into(),
            headless,
        }
    }

    // ---- resolver ----

    #[test]
    fn resolve_grok_returns_grok_pack() {
        assert_eq!(resolve("grok").unwrap().name(), "grok");
    }

    #[test]
    fn available_lists_grok() {
        assert_eq!(available(), vec!["grok"]);
    }

    #[test]
    fn unknown_harness_errors_clearly() {
        let err = resolve("gpt").err().expect("unknown harness");
        let msg = err.to_string();
        assert!(msg.contains("gpt"), "{msg}");
        assert!(msg.contains("grok"), "names the available packs: {msg}");
        assert_eq!(err.requested, "gpt");
    }

    // ---- the pack → Registry → ToolSpec path (via the fake pack) ----

    #[test]
    fn pack_builds_expected_specs() {
        let (_dir, host) = test_host();
        let registry = FakePack.build_registry(&host);
        assert!(registry.contains("alpha"));
        assert!(registry.contains("beta"));
        let specs = registry.specs();
        let mut names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["alpha", "beta"]);
        // Each tool carries its description; the schema is derived from its Args type.
        let alpha = specs.iter().find(|s| s.name == "alpha").unwrap();
        assert_eq!(alpha.description, "echo");
    }

    #[tokio::test]
    async fn pack_routes_to_impl() {
        let (_dir, host) = test_host();
        let registry = FakePack.build_registry(&host);
        let tool_ctx = ToolCtx::new(
            PathBuf::from("/repo"),
            "c1".into(),
            PathBuf::from("/repo"),
            CancellationToken::new(),
        );
        let dispatched = registry
            .dispatch("alpha", json!({ "x": 1 }), &tool_ctx)
            .await;
        assert!(dispatched.record.ok, "routes to the registered impl");
        assert_eq!(dispatched.record.name, "alpha");
        // The echo tool serialized `args.to_string()` into its output.
        assert_eq!(dispatched.record.output["echoed"], json!(r#"{"x":1}"#));
    }

    #[test]
    #[should_panic(expected = "duplicate tool registration")]
    fn duplicate_registration_panics() {
        let (_dir, host) = test_host();
        let _ = DupPack.build_registry(&host);
    }

    // ---- grok wiring ----

    #[test]
    fn grok_registers_real_tools_and_preamble() {
        let (_dir, host) = test_host();
        let pack = resolve("grok").unwrap();
        let registry = pack.build_registry(&host);
        for tool in [
            "run_terminal_cmd",
            "read_file",
            "search_replace",
            "grep",
            "list_dir",
        ] {
            assert!(registry.contains(tool), "grok pack registers {tool}");
        }

        let headless = pack.preamble(&ctx(true));
        assert_eq!(headless.len(), 1);
        assert_eq!(headless[0].role, Role::System);
        let interactive = pack.preamble(&ctx(false));
        assert_ne!(
            headless, interactive,
            "headless branch changes the identity"
        );
    }
}
