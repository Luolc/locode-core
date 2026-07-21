//! SIGTERM → the session's cancel handle (ADR-0018, delivering Task 21).
//!
//! Timeout harnesses (swe-lab) SIGTERM the process; without a handler that
//! loses the report. With one: mid-run SIGTERM fires the run's cancel token,
//! the loop pairs the unfinished batch, and the normal artifact still lands
//! on stdout with `status: "cancelled"` (exit 0 — a structured stop).
//! Pre-run SIGTERM (no session yet) exits 1 with nothing on stdout.
//!
//! Unix-only: Windows is out of scope (Task 24 plan; user decision
//! 2026-07-21) — on other platforms `locode-exec` simply has no handler.

use std::sync::{Arc, Mutex, PoisonError};

use locode_core::CancellationToken;

use crate::output;

/// The slot the run loop fills once a session exists: `None` = pre-run.
pub type CancelSlot = Arc<Mutex<Option<CancellationToken>>>;

/// Install the SIGTERM listener and return the (initially empty) cancel slot.
///
/// Call inside the tokio runtime, before any pre-run work. The listener task:
/// - pre-run (`None` in the slot): stderr note + exit 1 (nothing on stdout);
/// - in-run (`Some(handle)`): fires the cancel handle — idempotent, so
///   repeated SIGTERMs are harmless while the run winds down to its report.
pub fn install_sigterm() -> CancelSlot {
    let slot: CancelSlot = Arc::new(Mutex::new(None));
    let listener_slot = Arc::clone(&slot);
    tokio::spawn(async move {
        let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            return; // no handler — default SIGTERM disposition applies
        };
        while sigterm.recv().await.is_some() {
            let guard = listener_slot.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(handle) = guard.as_ref() {
                handle.cancel();
            } else {
                drop(guard);
                output::error_line("SIGTERM before the run started; exiting");
                // Pre-run: no report exists and none is owed; stdout is empty.
                std::process::exit(1);
            }
        }
    });
    slot
}

/// Record the running session's cancel handle in the slot.
pub fn arm(slot: &CancelSlot, handle: CancellationToken) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
}
