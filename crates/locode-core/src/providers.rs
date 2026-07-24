//! Custom-provider injection (ADR-0015): a name → factory registry the binary
//! entry points thread through to session assembly.
//!
//! `ProviderRegistry::builtin()` carries the built-in wires; a downstream
//! binary crate registers its own [`Provider`] implementations under new names
//! and selects them with `--api-schema <name>` — no fork of the CLI needed.

use std::fmt;
use std::sync::Arc;

use locode_protocol::{ContentBlock, Usage};
use locode_provider::{
    AnthropicProvider, Completion, MockProvider, OpenAiResponsesProvider, Provider, StopReason,
};

/// Per-run inputs a factory may use when constructing its provider.
pub struct ProviderInit {
    /// The session id — e.g. for cache-routing hints (the `openai-responses`
    /// wire sets it as `prompt_cache_key`, codex's rule).
    pub session_id: String,
    /// A model override (the `model` settings field, ADR-0024 §1.4). `None` =
    /// the factory's own default (env/config). Built-in factories honor it;
    /// custom factories may ignore it.
    pub model: Option<String>,
}

/// A constructed provider plus the model name it resolved (for the report
/// envelope and engine config).
pub struct BuiltProvider {
    /// The wire, ready to drive.
    pub provider: Arc<dyn Provider>,
    /// The resolved model id (config/env/default — the factory decides).
    pub model: String,
}

impl fmt::Debug for BuiltProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuiltProvider")
            .field("model", &self.model)
            .finish_non_exhaustive() // the dyn provider has no Debug bound
    }
}

/// A provider construction failure (missing env, bad config, unknown name):
/// pre-run by nature — surfaced before any session starts.
#[derive(Debug)]
pub struct ProviderBuildError(pub String);

impl fmt::Display for ProviderBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderBuildError {}

/// A named provider constructor: env/config resolution belongs inside the
/// factory so selection stays lazy — only the chosen wire pays its setup.
pub type ProviderFactory =
    Box<dyn Fn(&ProviderInit) -> Result<BuiltProvider, ProviderBuildError> + Send + Sync>;

/// The `--api-schema` name space: ordered name → factory entries.
///
/// Ordered so help/error listings are stable; [`ProviderRegistry::register`]
/// replaces in place when the name already exists (letting a downstream binary
/// override a built-in, e.g. a scripted `mock`).
pub struct ProviderRegistry {
    entries: Vec<(String, ProviderFactory)>,
}

impl ProviderRegistry {
    /// An empty registry (no names). Most callers want [`Self::builtin`].
    #[must_use]
    pub fn new() -> Self {
        ProviderRegistry {
            entries: Vec::new(),
        }
    }

    /// The built-in wires: `anthropic`, `openai-responses`, and the keyless
    /// `mock` (one scripted no-tool text turn → `completed`, for CI).
    ///
    /// The `mock` script is overridable via the `LOCODE_MOCK_SCRIPT` env var —
    /// a JSON array of turns, each `{"text": "…"}` (a final text turn) or
    /// `{"tool": "<name>", "input": {…}}` (a `tool_use` turn) — so keyless
    /// integration tests can drive multi-turn/tool runs (e.g. the SIGTERM
    /// test holds a run open with a slow shell command). A malformed script
    /// fails pre-run.
    #[must_use]
    pub fn builtin() -> Self {
        Self::new()
            .register("anthropic", |init| {
                let mut provider = AnthropicProvider::from_env()
                    .map_err(|e| ProviderBuildError(format!("anthropic wire: {e}")))?;
                // The settings `model` override (ADR-0024 §1.4) beats the
                // factory's env/config default.
                if let Some(model) = &init.model {
                    provider.config_mut().model.clone_from(model);
                }
                let model = provider.config().model.clone();
                Ok(BuiltProvider {
                    provider: Arc::new(provider),
                    model,
                })
            })
            .register("openai-responses", |init| {
                let mut provider = OpenAiResponsesProvider::from_env()
                    .map_err(|e| ProviderBuildError(format!("openai-responses wire: {e}")))?;
                // Cache-routing hint = the session id (codex's rule; probe-verified
                // harmless for xAI models).
                provider.config_mut().prompt_cache_key = Some(init.session_id.clone());
                if let Some(model) = &init.model {
                    provider.config_mut().model.clone_from(model);
                }
                let model = provider.config().model.clone();
                Ok(BuiltProvider {
                    provider: Arc::new(provider),
                    model,
                })
            })
            .register("mock", |init| {
                let script = match std::env::var("LOCODE_MOCK_SCRIPT") {
                    Ok(json) => mock_script(&json)
                        .map_err(|e| ProviderBuildError(format!("LOCODE_MOCK_SCRIPT: {e}")))?,
                    Err(_) => vec![Completion {
                        content: vec![ContentBlock::Text {
                            text: "Mock run complete.".to_string(),
                        }],
                        usage: Usage::default(),
                        stop: StopReason::EndTurn,
                    }],
                };
                Ok(BuiltProvider {
                    provider: Arc::new(MockProvider::new(script)),
                    // Honor the model override like the real wires do, so the
                    // flag > settings > default chain is observable keylessly.
                    model: init.model.clone().unwrap_or_else(|| "mock-1".to_string()),
                })
            })
    }

    /// Add a provider under `name`, replacing any existing entry of that name
    /// (registration order is otherwise preserved for listings).
    #[must_use]
    pub fn register<F>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        F: Fn(&ProviderInit) -> Result<BuiltProvider, ProviderBuildError> + Send + Sync + 'static,
    {
        let name = name.into();
        let factory: ProviderFactory = Box::new(factory);
        match self.entries.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = factory,
            None => self.entries.push((name, factory)),
        }
        self
    }

    /// The registered names, in registration order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// Construct the provider registered under `name`.
    ///
    /// # Errors
    /// [`ProviderBuildError`] when `name` is unknown (the message lists the
    /// available names) or when the factory itself fails (missing env, …).
    pub fn build(
        &self,
        name: &str,
        init: &ProviderInit,
    ) -> Result<BuiltProvider, ProviderBuildError> {
        let factory = self
            .entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f)
            .ok_or_else(|| {
                ProviderBuildError(format!(
                    "unknown --api-schema `{name}`; available: {}",
                    self.names().join(", ")
                ))
            })?;
        factory(init)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Parse a `LOCODE_MOCK_SCRIPT` value into mock completions.
///
/// Format: a JSON array; each element is `{"text": "…"}` (an end-turn text
/// completion) or `{"tool": "<name>", "input": {…}}` (a single `tool_use`
/// completion — `input` defaults to `{}`, ids are `call_0`, `call_1`, …).
fn mock_script(json: &str) -> Result<Vec<Completion>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let turns = value
        .as_array()
        .ok_or_else(|| "expected a JSON array of turns".to_string())?;
    if turns.is_empty() {
        return Err("expected at least one turn".to_string());
    }
    turns
        .iter()
        .enumerate()
        .map(|(i, turn)| {
            if let Some(text) = turn.get("text").and_then(serde_json::Value::as_str) {
                Ok(Completion {
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
                    usage: Usage::default(),
                    stop: StopReason::EndTurn,
                })
            } else if let Some(tool) = turn.get("tool").and_then(serde_json::Value::as_str) {
                Ok(Completion {
                    content: vec![ContentBlock::ToolUse {
                        id: format!("call_{i}"),
                        name: tool.to_string(),
                        input: turn
                            .get("input")
                            .cloned()
                            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
                    }],
                    usage: Usage::default(),
                    stop: StopReason::ToolUse,
                })
            } else {
                Err(format!(
                    "turn {i}: expected {{\"text\": …}} or {{\"tool\": …, \"input\": …}}"
                ))
            }
        })
        .collect()
}
