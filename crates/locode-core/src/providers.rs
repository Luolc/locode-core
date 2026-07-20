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
    #[must_use]
    pub fn builtin() -> Self {
        Self::new()
            .register("anthropic", |_init| {
                let provider = AnthropicProvider::from_env()
                    .map_err(|e| ProviderBuildError(format!("anthropic wire: {e}")))?;
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
                let model = provider.config().model.clone();
                Ok(BuiltProvider {
                    provider: Arc::new(provider),
                    model,
                })
            })
            .register("mock", |_init| {
                let provider = MockProvider::new(vec![Completion {
                    content: vec![ContentBlock::Text {
                        text: "Mock run complete.".to_string(),
                    }],
                    usage: Usage::default(),
                    stop: StopReason::EndTurn,
                }]);
                Ok(BuiltProvider {
                    provider: Arc::new(provider),
                    model: "mock-1".to_string(),
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
