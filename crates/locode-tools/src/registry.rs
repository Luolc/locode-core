//! The registry and the single `dispatch` door (ADR-0003, ADR-0004, ADR-0008).
//!
//! # The tool-identity model
//!
//! A tool has three distinct names; do not conflate them:
//! - its **Rust type name** (e.g. `GrokReadFile`) — compile-time identity only,
//!   invisible to both the model and the registry;
//! - its **wire name** (e.g. `read_file`) — what the model calls and the key this
//!   registry stores it under; assigned by the harness pack at registration time,
//!   which is why [`crate::Tool`] has no `name()` method;
//! - its [`ToolKind`] tag — for cross-pack A/B alignment only.
//!
//! Only one pack is active per run, so a registry holds exactly that pack's tools
//! under that pack's wire names — there is no cross-pack key collision to guard,
//! only duplicates *within* a pack (a wiring bug → panic at startup).

use std::collections::HashMap;

use async_trait::async_trait;
use locode_protocol::{ContentBlock, ResultChunk, ToolCallRecord};
use serde_json::Value;

use crate::ctx::ToolCtx;
use crate::error::ToolError;
use crate::tool::{Tool, ToolKind, ToolOutput};

/// The two faces of a successful tool call, produced by erasure.
///
/// Mirrors Grok Build's `ToolRunResult`: `output` is the structured value for the
/// report; `prompt_text` is the text the model reads back in history (ADR-0003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRunResult {
    /// The structured output value (into the report's `tool_calls[]`).
    pub output: Value,
    /// The model-facing rendering (into the transcript).
    pub prompt_text: String,
}

/// The object-safe, type-erased tool the registry stores.
///
/// Every typed [`Tool`] becomes one of these via the internal `TypedTool` adapter.
/// It is also the seam for **dynamically described tools** with no compile-time
/// `Args` type — MCP tools implement `DynTool` directly (schema fetched from the
/// server, `kind` = [`ToolKind::Other`]) and register via
/// [`Registry::register_dyn`].
#[async_trait]
pub trait DynTool: Send + Sync {
    /// The cross-pack classification tag.
    fn kind(&self) -> ToolKind;
    /// The description offered to the model.
    fn description(&self) -> &str;
    /// The JSON Schema for this tool's arguments.
    fn parameters_schema(&self) -> Value;
    /// Decode raw JSON args, run, and re-serialize into both result faces.
    ///
    /// # Errors
    /// [`ToolError::Respond`] for bad args or any recoverable failure;
    /// [`ToolError::Fatal`] when the transcript is unrecoverable.
    async fn call(&self, ctx: &ToolCtx, raw_args: Value) -> Result<ToolRunResult, ToolError>;
}

/// Adapter that erases a concrete [`Tool`] into a [`DynTool`].
///
/// A wrapper (rather than a blanket `impl<T: Tool> DynTool for T`) is deliberate:
/// the blanket form would forbid any manual `impl DynTool for McpTool` under
/// Rust's coherence rules, closing the MCP seam.
struct TypedTool<T: Tool>(T);

#[async_trait]
impl<T: Tool> DynTool for TypedTool<T> {
    fn kind(&self) -> ToolKind {
        self.0.kind()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn parameters_schema(&self) -> Value {
        self.0.parameters_schema()
    }

    async fn call(&self, ctx: &ToolCtx, raw_args: Value) -> Result<ToolRunResult, ToolError> {
        // Bad args are soft (ADR-0004): the model reads the decode error and retries.
        let args: T::Args = serde_json::from_value(raw_args)
            .map_err(|e| ToolError::Respond(format!("invalid arguments: {e}")))?;
        let output = self.0.run(ctx, args).await?;
        let prompt_text = output.to_prompt_text();
        let output = serde_json::to_value(&output)
            .map_err(|e| ToolError::Fatal(format!("failed to serialize tool output: {e}")))?;
        Ok(ToolRunResult {
            output,
            prompt_text,
        })
    }
}

/// A neutral tool spec (name + description + args schema) for a pack/provider to
/// map onto its wire format (e.g. Anthropic's `{name, description, input_schema}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    /// The wire (model-facing) name — the registry key.
    pub name: String,
    /// The tool description.
    pub description: String,
    /// The derived JSON Schema for the tool's arguments.
    pub parameters: Value,
}

/// The outcome of one [`Registry::dispatch`]: both the history and report views,
/// plus a fatal signal.
///
/// `tool_result` is **always** present and paired to the call id — even on a fatal
/// error — so the transcript-validity invariant holds regardless of outcome
/// (ADR-0004). A `Some(fatal)` tells the loop to append the result and then abort.
#[derive(Debug, Clone, PartialEq)]
pub struct Dispatched {
    /// The `tool_result` block to append to history (paired to the call id).
    pub tool_result: ContentBlock,
    /// The structured record for the report's `tool_calls[]`.
    pub record: ToolCallRecord,
    /// `Some(message)` iff the tool returned [`ToolError::Fatal`]: append, then abort.
    pub fatal: Option<String>,
}

/// A set of tools keyed by their model-facing wire names, plus the one `dispatch`
/// door every call funnels through (ADR-0008).
#[derive(Default)]
pub struct Registry {
    tools: HashMap<String, Box<dyn DynTool>>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a typed [`Tool`] under its pack-assigned wire `name`.
    ///
    /// # Panics
    /// Panics if `name` is already registered — a startup wiring bug, not runtime
    /// input.
    pub fn register<T: Tool + 'static>(&mut self, name: impl Into<String>, tool: T) {
        self.register_dyn(name, Box::new(TypedTool(tool)));
    }

    /// Register an already-erased [`DynTool`] — the door for MCP and other
    /// dynamically-described tools with no compile-time `Args` type.
    ///
    /// # Panics
    /// Panics if `name` is already registered.
    pub fn register_dyn(&mut self, name: impl Into<String>, tool: Box<dyn DynTool>) {
        let name = name.into();
        assert!(
            !self.tools.contains_key(&name),
            "duplicate tool registration for name `{name}`"
        );
        self.tools.insert(name, tool);
    }

    /// Whether a tool is registered under `name`.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// The registered wire names, unordered.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    /// The neutral tool specs for every registered tool, unordered.
    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|(name, tool)| ToolSpec {
                name: name.clone(),
                description: tool.description().to_owned(),
                parameters: tool.parameters_schema(),
            })
            .collect()
    }

    /// The single door every tool call funnels through (ADR-0008).
    ///
    /// Resolves `name`, decodes `raw_args`, runs the tool, and returns both the
    /// history `tool_result` and the report [`ToolCallRecord`]. An unknown tool and
    /// bad args are **soft** ([`ToolError::Respond`] shape); a [`ToolError::Fatal`]
    /// still yields a paired result but sets [`Dispatched::fatal`]. `ctx.call_id`
    /// supplies the pairing id.
    pub async fn dispatch(&self, name: &str, raw_args: Value, ctx: &ToolCtx) -> Dispatched {
        let id = ctx.call_id.clone();

        let Some(tool) = self.tools.get(name) else {
            let message = format!("unknown tool: {name}");
            return Dispatched {
                tool_result: error_result(&id, &message),
                record: record(&id, name, ToolKind::Other, raw_args, false, Value::Null),
                fatal: None,
            };
        };

        let kind = tool.kind();
        match tool.call(ctx, raw_args.clone()).await {
            Ok(ToolRunResult {
                output,
                prompt_text,
            }) => Dispatched {
                tool_result: ok_result(&id, prompt_text),
                record: record(&id, name, kind, raw_args, true, output),
                fatal: None,
            },
            Err(ToolError::Respond(message)) => Dispatched {
                tool_result: error_result(&id, &message),
                record: record(&id, name, kind, raw_args, false, Value::Null),
                fatal: None,
            },
            Err(ToolError::Fatal(message)) => Dispatched {
                tool_result: error_result(&id, &message),
                record: record(&id, name, kind, raw_args, false, Value::Null),
                fatal: Some(message),
            },
        }
    }
}

/// Build a `tool_result` for a successful call.
fn ok_result(id: &str, text: String) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.to_owned(),
        content: vec![ResultChunk::Text { text }],
        is_error: false,
    }
}

/// Build an `is_error` `tool_result` the model can recover from.
fn error_result(id: &str, message: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.to_owned(),
        content: vec![ResultChunk::Text {
            text: message.to_owned(),
        }],
        is_error: true,
    }
}

/// Build the report-side record for a call.
fn record(
    id: &str,
    name: &str,
    kind: ToolKind,
    args: Value,
    ok: bool,
    output: Value,
) -> ToolCallRecord {
    ToolCallRecord {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind.as_str().to_owned(),
        args,
        ok,
        output,
    }
}
