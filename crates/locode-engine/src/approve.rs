//! The interactive approval seam at the engine's dispatch step (ADR-0017).
//!
//! The engine consults an injected [`Approver`] before every tool call, at the
//! gate **in front of** the one dispatch door — the tools crate stays
//! interaction-free (`Registry` is a first-class library surface whose
//! consumers must not inherit an approval dependency). The core stays headless
//! (ADR-0001): nothing here renders or waits on a terminal — the engine awaits
//! a trait method that headless callers resolve instantly ([`AllowAll`]).
//!
//! An interactive frontend implements [`Approver`] with a oneshot + FIFO queue
//! (the pattern all four studied harnesses share); prompt queueing and
//! stickiness ("always allow") are the approver implementation's job,
//! client-side — not core vocabulary.

use async_trait::async_trait;
use locode_tools::ToolKind;
use serde_json::Value;

/// Decides whether one tool call may run. Consulted by the engine per call,
/// serially, before dispatch; the await suspends **only this call**.
#[async_trait]
pub trait Approver: Send + Sync {
    /// Resolve one pre-dispatch approval request.
    async fn decide(&self, request: &ApprovalRequest<'_>) -> Decision;
}

/// The pre-dispatch view of one tool call — what the studied UIs render their
/// permission prompts from (request args, not host-resolved detail).
///
/// `#[non_exhaustive]` so richer context can be added without breaking
/// downstream constructors — only the engine builds one.
#[derive(Debug)]
#[non_exhaustive]
pub struct ApprovalRequest<'a> {
    /// The `tool_use` id (pairs the decision to the call).
    pub tool_use_id: &'a str,
    /// The client-facing tool name the model called.
    pub tool_name: &'a str,
    /// The registry's cross-pack classification, when the tool is known —
    /// lets an approver auto-allow read-only kinds without knowing names.
    pub kind: Option<ToolKind>,
    /// The raw arguments the model supplied.
    pub input: &'a Value,
}

/// The approval vocabulary — deliberately minimal (ADR-0017 Option V1):
/// stickiness and richer choices live in approver implementations.
///
/// `#[non_exhaustive]` so variants (e.g. `AllowModified`) are additive.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision {
    /// Run the call.
    Allow,
    /// Do not run the call: the model sees a paired `is_error` result carrying
    /// the reason and the run continues — deny is **soft**, never fatal.
    Deny {
        /// Why the call was denied (shown to the model verbatim).
        reason: String,
    },
}

/// The default approver: allows everything, instantly — headless consumers
/// (`locode-exec`, evals) are byte-for-byte unchanged in behavior.
pub struct AllowAll;

#[async_trait]
impl Approver for AllowAll {
    async fn decide(&self, _request: &ApprovalRequest<'_>) -> Decision {
        Decision::Allow
    }
}
