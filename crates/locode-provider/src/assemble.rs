//! Streaming tool-call assembly: accumulate partial-JSON args, parse at stop.
//!
//! Every studied harness consumes an SSE stream and **accumulates a tool call's
//! arguments as a raw string** across `input_json_delta` fragments, because a single
//! delta is not valid JSON — then parses once when the block closes. This helper
//! encodes exactly that rule (raw string per content-block index, parse at
//! [`ToolCallAssembler::finish`]). v0 is non-streaming, but the helper is built and
//! tested standalone so the streaming wire drops straight in later.

use std::collections::BTreeMap;

use locode_protocol::ContentBlock;
use serde_json::Value;
use thiserror::Error;

/// Accumulates streamed tool-call fragments by content-block index.
#[derive(Debug, Default)]
pub struct ToolCallAssembler {
    // Keyed by content-block index so `finish` yields calls in stream order.
    partials: BTreeMap<usize, Partial>,
}

#[derive(Debug)]
struct Partial {
    id: String,
    name: String,
    json: String,
}

/// A failure assembling streamed tool-call arguments.
#[derive(Debug, Error)]
pub enum AssembleError {
    /// A fragment arrived for an index with no preceding [`ToolCallAssembler::begin`].
    #[error("no tool call started at content-block index {0}")]
    MissingStart(usize),
    /// The accumulated argument buffer was not valid JSON.
    #[error("invalid tool-call arguments JSON at index {index}: {source}")]
    InvalidJson {
        /// The content-block index whose buffer failed to parse.
        index: usize,
        /// The underlying JSON parse error.
        source: serde_json::Error,
    },
}

impl ToolCallAssembler {
    /// A fresh, empty assembler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a tool call at `index` (from the stream's `content_block_start`).
    pub fn begin(&mut self, index: usize, id: impl Into<String>, name: impl Into<String>) {
        self.partials.insert(
            index,
            Partial {
                id: id.into(),
                name: name.into(),
                json: String::new(),
            },
        );
    }

    /// Append a raw partial-JSON fragment for `index`. **Never parsed here.**
    ///
    /// # Errors
    /// [`AssembleError::MissingStart`] if no [`ToolCallAssembler::begin`] was seen for
    /// `index`.
    pub fn push_json(&mut self, index: usize, fragment: &str) -> Result<(), AssembleError> {
        match self.partials.get_mut(&index) {
            Some(partial) => {
                partial.json.push_str(fragment);
                Ok(())
            }
            None => Err(AssembleError::MissingStart(index)),
        }
    }

    /// Finish: parse each accumulated buffer once, in index order, into
    /// [`ContentBlock::ToolUse`] blocks.
    ///
    /// An empty/whitespace buffer becomes `{}` — Anthropic sends empty input for a
    /// no-argument tool call.
    ///
    /// # Errors
    /// [`AssembleError::InvalidJson`] if any accumulated buffer is not valid JSON.
    pub fn finish(self) -> Result<Vec<ContentBlock>, AssembleError> {
        let mut blocks = Vec::with_capacity(self.partials.len());
        for (index, partial) in self.partials {
            let input: Value = if partial.json.trim().is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(&partial.json)
                    .map_err(|source| AssembleError::InvalidJson { index, source })?
            };
            blocks.push(ContentBlock::ToolUse {
                id: partial.id,
                name: partial.name,
                input,
            });
        }
        Ok(blocks)
    }
}
