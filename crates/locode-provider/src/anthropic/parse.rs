//! [`wire::MessagesResponse`] → [`Completion`] (plan §4.5).
//!
//! Preserves `tool_use` ids **verbatim** (no grok-style sanitization — our ids
//! come only from the Messages wire, and sanitizing would risk the pairing
//! invariant, ADR-0004), keeps thinking blocks **with** their signatures for
//! replay, and never fails on an unknown stop reason.

use locode_protocol::{ContentBlock, Usage};

use super::wire;
use crate::completion::{Completion, StopReason};
use crate::provider::ProviderError;

/// Parse a Messages response into the normalized [`Completion`].
///
/// # Errors
/// [`ProviderError::ContextOverflow`] when the response stopped with
/// `model_context_window_exceeded` **and** carries no content — there is nothing
/// usable to hand the engine, so the overflow surfaces as the terminal error the
/// loop maps to compaction (plan §4.5). With partial content the completion is
/// returned and the overflow is carried in [`StopReason::Unknown`].
pub fn response_to_completion(resp: wire::MessagesResponse) -> Result<Completion, ProviderError> {
    let mut content: Vec<ContentBlock> = Vec::with_capacity(resp.content.len());
    for block in resp.content {
        match block {
            wire::ContentBlock::Text { text, .. } => {
                content.push(ContentBlock::Text { text });
            }
            // id preserved VERBATIM (ADR-0007).
            wire::ContentBlock::ToolUse { id, name, input } => {
                content.push(ContentBlock::ToolUse { id, name, input });
            }
            // Signature captured for replay (plan §4.2); non-streaming hands us
            // the whole block at once.
            wire::ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                content.push(ContentBlock::Reasoning {
                    format: locode_protocol::ReasoningFormat::Anthropic,
                    text: thinking,
                    signature: Some(signature),
                    payload: None,
                });
            }
            // Encrypted thinking is kept opaquely for verbatim replay.
            wire::ContentBlock::RedactedThinking { data } => {
                content.push(ContentBlock::Reasoning {
                    format: locode_protocol::ReasoningFormat::AnthropicRedacted,
                    text: String::new(),
                    signature: None,
                    payload: Some(serde_json::Value::String(data)),
                });
            }
            // Unexpected in an assistant response → ignore (grok does the same).
            wire::ContentBlock::Image { .. } | wire::ContentBlock::ToolResult { .. } => {}
        }
    }

    if matches!(
        resp.stop_reason,
        Some(wire::StopReason::ModelContextWindowExceeded)
    ) && content.is_empty()
    {
        return Err(ProviderError::ContextOverflow);
    }

    Ok(Completion {
        content,
        usage: map_usage(&resp.usage),
        stop: map_stop_reason(resp.stop_reason),
    })
}

/// Map wire usage onto the protocol's [`Usage`] — the **authoritative** counts
/// (any client-side estimate is the engine's concern, overwritten by this).
fn map_usage(usage: &wire::MessagesUsage) -> Usage {
    Usage {
        input_tokens: usage.input_tokens.unwrap_or_default(),
        output_tokens: usage.output_tokens.unwrap_or_default(),
        // Option-through: None = the wire/gateway did not report the counter
        // (ADR-0009 amendment — Some(0) is a real zero, None is N/A).
        cache_read_tokens: usage.cache_read_input_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        // Anthropic folds thinking tokens into output_tokens — never reported.
        reasoning_tokens: None,
    }
}

/// Map the wire stop reason onto the neutral enum; unknown values (and the
/// context-overflow stop, which has no neutral variant) are carried verbatim in
/// [`StopReason::Unknown`] — never a parse failure (grok's discipline).
fn map_stop_reason(stop: Option<wire::StopReason>) -> StopReason {
    match stop {
        Some(wire::StopReason::EndTurn) => StopReason::EndTurn,
        Some(wire::StopReason::MaxTokens) => StopReason::MaxTokens,
        Some(wire::StopReason::ToolUse) => StopReason::ToolUse,
        Some(wire::StopReason::StopSequence) => StopReason::StopSequence,
        Some(wire::StopReason::Refusal) => StopReason::Refusal,
        Some(wire::StopReason::PauseTurn) => StopReason::PauseTurn,
        Some(wire::StopReason::ModelContextWindowExceeded) => {
            StopReason::Unknown("model_context_window_exceeded".to_string())
        }
        Some(wire::StopReason::Unknown(raw)) => StopReason::Unknown(raw),
        // A non-streaming response should always carry one; don't invent a stop.
        None => StopReason::Unknown("(missing stop_reason)".to_string()),
    }
}
