//! The `grok` pack — a faithful port of Grok Build's `xai-grok-tools` toolset, trimmed
//! to headless-minimal (ADR-0012). The real tools land in Tasks 9-11 (over `locode-host`)
//! and the real system prompt in Task 13; Task 8 wires the pack as a scaffold.

use locode_protocol::{ContentBlock, Message, Role};
use locode_tools::Registry;

use crate::pack::{Pack, PackContext};

/// The grok harness pack (a zero-sized `&'static` singleton).
#[derive(Debug, Default, Clone, Copy)]
pub struct GrokPack;

impl Pack for GrokPack {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn register(&self, _registry: &mut Registry) {
        // Tasks 9-11 register grok's REAL tools here, over `locode-host`, each under its
        // real wire name and carrying its `ToolKind` tag via `Tool::kind()`:
        //   registry.register("run_terminal_cmd", GrokRunTerminalCmd::new(host));  // Task 9
        //   registry.register("read_file",        GrokReadFile::new(host));        // Task 9
        //   registry.register("search_replace",   GrokSearchReplace::new(host));   // Task 10
        //   registry.register("grep",             GrokGrep::new(host));            // Task 11
        //   registry.register("list_dir",         GrokListDir::new(host));         // Task 11
        // (Empty until then — the framework is proven via a test-local fake pack.)
    }

    fn preamble(&self, ctx: &PackContext) -> Vec<Message> {
        // Scaffold. Task 13 renders grok's real prompt (minijinja) and decides its final
        // System/Developer split. For now: a single `System` message with the
        // headless-branched identity line, so the seam and the headless branch are wired.
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
