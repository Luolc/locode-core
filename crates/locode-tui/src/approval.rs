//! The interactive approver (ADR-0017) and its UI vocabulary.
//!
//! `TuiApprover` runs on the engine task inside `run_text`; it round-trips to
//! the UI over a oneshot per call — grok's exact pattern
//! (`acp_handler/permissions.rs:20-89`). YOLO auto-allow and per-tool
//! stickiness live here, client-side, exactly as ADR-0017 prescribes
//! (the core vocabulary stays `Allow`/`Deny`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use locode_core::{ApprovalRequest, Approver, Decision};
use tokio::sync::oneshot;

use crate::engine::EngineMsg;

/// The display view of one pending approval (what the overlay renders).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalView {
    /// The `tool_use` id (resolve key).
    pub tool_use_id: String,
    /// Client-facing tool name.
    pub tool_name: String,
    /// The tool's cross-pack kind, as a display string.
    pub kind: String,
    /// One-line argument summary.
    pub args: String,
}

/// The UI's answer to an approval — richer than the core `Decision` so
/// "allow for session" stays a UI concept the approver maps down (ADR-0017:
/// stickiness is the approver's job).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// Allow this one call.
    Allow,
    /// Allow this call and auto-allow this tool for the rest of the session.
    AllowSession,
    /// Deny with a reason the model sees.
    Deny {
        /// Why the call was denied.
        reason: String,
    },
}

/// One approval request in flight to the UI: the display view plus the
/// oneshot the UI resolves. Manual `Debug` (the sender isn't `Debug`).
pub struct ApprovalAsk {
    /// What the overlay renders.
    pub view: ApprovalView,
    /// The UI resolves this with the user's choice.
    pub respond: oneshot::Sender<ApprovalOutcome>,
}

impl std::fmt::Debug for ApprovalAsk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalAsk")
            .field("view", &self.view)
            .finish_non_exhaustive()
    }
}

/// The interactive approver: yolo auto-allow, per-tool session stickiness, and
/// a round-trip to the UI for everything else.
pub struct TuiApprover {
    yolo: bool,
    session_allow: Arc<Mutex<HashSet<String>>>,
    events: tokio::sync::mpsc::UnboundedSender<EngineMsg>,
}

impl TuiApprover {
    /// Build an approver that surfaces asks on `events` (unless `yolo`).
    #[must_use]
    pub fn new(yolo: bool, events: tokio::sync::mpsc::UnboundedSender<EngineMsg>) -> Self {
        Self {
            yolo,
            session_allow: Arc::new(Mutex::new(HashSet::new())),
            events,
        }
    }

    fn is_session_allowed(&self, tool_name: &str) -> bool {
        self.session_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(tool_name)
    }

    fn remember_session_allow(&self, tool_name: &str) {
        self.session_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tool_name.to_owned());
    }
}

#[async_trait]
impl Approver for TuiApprover {
    async fn decide(&self, request: &ApprovalRequest<'_>) -> Decision {
        // Client-side auto-allow (grok's rule): never surface UI.
        if self.yolo || self.is_session_allowed(request.tool_name) {
            return Decision::Allow;
        }

        let view = ApprovalView {
            tool_use_id: request.tool_use_id.to_owned(),
            tool_name: request.tool_name.to_owned(),
            kind: request.kind.map_or_else(|| "other".to_string(), kind_str),
            args: args_summary(request.input),
        };
        let (tx, rx) = oneshot::channel();
        if self
            .events
            .send(EngineMsg::Approval(ApprovalAsk { view, respond: tx }))
            .is_err()
        {
            // UI gone: fail safe (deny), the run winds down.
            return Decision::Deny {
                reason: "approval unavailable".to_string(),
            };
        }
        match rx.await {
            Ok(ApprovalOutcome::Allow) => Decision::Allow,
            Ok(ApprovalOutcome::AllowSession) => {
                self.remember_session_allow(request.tool_name);
                Decision::Allow
            }
            Ok(ApprovalOutcome::Deny { reason }) => Decision::Deny { reason },
            // The UI dropped the responder (quit/cancel drain): safe default.
            Err(_) => Decision::Deny {
                reason: "run cancelled".to_string(),
            },
        }
    }
}

fn kind_str(kind: locode_core::ToolKind) -> String {
    kind.as_str().to_owned()
}

/// One-line JSON argument summary for the overlay, capped.
fn args_summary(input: &serde_json::Value) -> String {
    let mut s = input.to_string();
    if s.chars().count() > 80 {
        s = s.chars().take(79).collect::<String>() + "…";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // `ApprovalRequest` is `#[non_exhaustive]` and engine-constructed, so this
    // crate can't build one to drive `decide()` directly — the yolo / sticky /
    // send / map branches are covered by the engine-task integration tests
    // (`tests/approvals.rs`). Here we cover the sticky-set bookkeeping.
    #[test]
    fn session_allow_set_records_and_recalls() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let approver = TuiApprover::new(false, tx);
        assert!(!approver.is_session_allowed("grep"));
        approver.remember_session_allow("grep");
        assert!(approver.is_session_allowed("grep"));
        assert!(!approver.is_session_allowed("run_terminal_cmd"));
    }
}
