//! The normalized model response — provider-neutral, not any wire's raw shape.

use locode_protocol::{ContentBlock, Usage};

/// One normalized completion: the parsed result of a single model call (ADR-0007).
///
/// This is **not** a serde mirror of any wire (Anthropic returns interleaved content
/// blocks, OpenAI returns split fields); each wire parses its raw response *into*
/// this shape. Following Grok Build's normalized response, the assistant turn is an
/// **ordered list of [`ContentBlock`]s** (`Text` / `Thinking{signature}` / `ToolUse`,
/// in order) rather than pre-split `text` + `tool_calls` — so nothing is lost and
/// the engine can append `content` straight into an assistant
/// [`Message`](locode_protocol::Message) (ADR-0013),
/// preserving thinking blocks and their signatures for replay.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// The assistant turn's content blocks, in order.
    pub content: Vec<ContentBlock>,
    /// Token accounting parsed from the terminal usage event.
    pub usage: Usage,
    /// Why the model stopped.
    pub stop: StopReason,
}

impl Completion {
    /// The `tool_use` blocks the model emitted, in order.
    pub fn tool_uses(&self) -> impl Iterator<Item = &ContentBlock> {
        self.content
            .iter()
            .filter(|block| matches!(block, ContentBlock::ToolUse { .. }))
    }

    /// Whether this completion asked to call any tools.
    #[must_use]
    pub fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
    }

    /// The concatenated text of all `text` blocks, if any.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let mut out = String::new();
        for block in &self.content {
            if let ContentBlock::Text { text } = block {
                out.push_str(text);
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

/// Why the model stopped generating (Anthropic-shaped, provider-neutral).
///
/// Mirrors an **open** wire enum (a provider can return a reason we don't model),
/// so it is `#[non_exhaustive]` with an [`StopReason::Unknown`] catch-all carrying
/// the raw string — the same approach codex-rs takes for forward-compatibility. Not
/// serialized (it is mapped to [`locode_protocol::Status`] by the engine), so no
/// serde derive is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    /// The model finished its turn with a natural stop.
    EndTurn,
    /// The output hit the `max_tokens` ceiling.
    MaxTokens,
    /// The model wants to call one or more tools.
    ToolUse,
    /// A configured stop sequence was produced.
    StopSequence,
    /// The model refused to continue.
    Refusal,
    /// The turn was paused (e.g. a server-tool round-trip).
    PauseTurn,
    /// A stop reason this client does not model, carried verbatim.
    Unknown(String),
}
