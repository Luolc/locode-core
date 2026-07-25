//! The loop's terminal outcome and the cross-turn report accumulator (internal).

use locode_protocol::{Status, ToolCallRecord, Usage};

/// Why the loop stopped. Maps 1:1 onto [`Status`].
pub(crate) enum Terminal {
    /// The assistant turn had no tool calls.
    Completed { final_message: Option<String> },
    /// The max-turns ceiling was hit after a dispatch batch.
    MaxTurns,
    /// A provider error after the bounded resample budget was spent.
    ModelError { error: String },
    /// A `Dispatched.fatal` aborted the turn.
    Error { error: String },
    /// The run's cancel token fired (ADR-0018) — a structured stop, not a fault.
    Cancelled,
}

impl Terminal {
    pub(crate) fn status(&self) -> Status {
        match self {
            Terminal::Completed { .. } => Status::Completed,
            Terminal::MaxTurns => Status::MaxTurns,
            Terminal::ModelError { .. } => Status::ModelError,
            Terminal::Error { .. } => Status::Error,
            Terminal::Cancelled => Status::Cancelled,
        }
    }
}

/// Report-side state accumulated across turns.
#[derive(Default)]
pub(crate) struct RunAcc {
    pub(crate) turns: u32,
    pub(crate) tool_calls: Vec<ToolCallRecord>,
    pub(crate) usage: Usage,
    /// The **final** turn's usage, kept alongside the running sum: the sum is the cost
    /// basis, this one is the context occupancy the next request starts from.
    pub(crate) last_usage: Usage,
    pub(crate) last_assistant_text: Option<String>,
    /// The final completion's stop reason, as the neutral string for the report
    /// (ADR-0009 amendment 2026-07-19).
    pub(crate) last_stop: Option<String>,
}
