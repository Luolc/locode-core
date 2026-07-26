//! The mid-run input queue (ADR-0028).
//!
//! A **handle taken before the run**, not a command. [`Session::run`] takes
//! `&mut self` and the caller awaits it, so a frontend's command loop is not
//! polled while a run is in flight — the same constraint that made
//! `cancel_handle()` a pre-run clone (ADR-0018). A frontend clones this handle
//! before starting a run and pushes into it; the loop drains it between
//! dispatch and the next sample.
//!
//! [`Session::run`]: crate::Session::run

use std::sync::{Arc, Mutex, PoisonError};

/// The preamble that marks text as having arrived **mid-run**.
///
/// A mid-run message is not the same speech act as a fresh prompt: it may amend
/// an earlier instruction, be an aside, or be an instruction for after the
/// current work ("when you finish this, also…"). The model can only act
/// correctly — including changing course inside the running loop — if it knows
/// which. Unmarked, it reads as though it had been there from the start.
///
/// Grok Build ships the same marker and the same wording
/// (`INTERJECTION_WIRE_PREFIX`), and its PTY tests assert the distinction this
/// module preserves: the mid-run path carries the preamble, and a queued
/// message that lands as an ordinary next-turn prompt does not.
pub const MID_RUN_PREAMBLE: &str = "The user sent a message while you were working";

/// A clonable handle to one session's pending mid-run input.
///
/// Clones share one queue. Cheap to clone and safe to hold across an await —
/// the lock is only ever taken inside these methods, never returned.
#[derive(Debug, Clone, Default)]
pub struct InputQueue {
    items: Arc<Mutex<Vec<String>>>,
}

impl InputQueue {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a message typed while the run is in flight.
    pub fn push(&self, text: impl Into<String>) {
        self.lock().push(text.into());
    }

    /// The pending items, for a frontend to render. Does not consume.
    #[must_use]
    pub fn pending(&self) -> Vec<String> {
        self.lock().clone()
    }

    /// Whether anything is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Take everything, leaving the queue empty.
    #[must_use]
    pub fn take_all(&self) -> Vec<String> {
        std::mem::take(&mut *self.lock())
    }

    /// Drop every pending item.
    ///
    /// Called on cancel: the message was written for a run the user then
    /// abandoned, so delivering it into the next turn would be surprising. This
    /// covers **undelivered** items only — anything already drained is in
    /// `history` and cancel rolls nothing back (ADR-0028).
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// The queued text as one marked block, or `None` when nothing is waiting.
    ///
    /// Multiple items concatenate rather than arriving as separate turns: the
    /// common case is a user typing several lines before sending, not issuing
    /// competing instructions.
    #[must_use]
    pub(crate) fn take_marked(&self) -> Option<String> {
        let items = self.take_all();
        if items.is_empty() {
            return None;
        }
        Some(format!("{MID_RUN_PREAMBLE}:\n\n{}", items.join("\n\n")))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        // The guard only ever protects a Vec and is never held across an await,
        // so recovering from a poisoned lock is strictly better than panicking
        // inside the loop (denied lints forbid unwrap here anyway).
        self.items.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_queue() {
        let a = InputQueue::new();
        let b = a.clone();
        a.push("from a");
        assert_eq!(b.pending(), vec!["from a".to_string()]);
        assert!(!b.is_empty());
    }

    #[test]
    fn multiple_items_concatenate_under_one_marker() {
        let q = InputQueue::new();
        q.push("first line");
        q.push("second line");
        let marked = q.take_marked().expect("something queued");
        assert!(marked.starts_with(MID_RUN_PREAMBLE));
        assert!(marked.contains("first line"));
        assert!(marked.contains("second line"));
        assert_eq!(
            marked.matches(MID_RUN_PREAMBLE).count(),
            1,
            "one marker for the whole batch, not one per item"
        );
        assert!(q.is_empty(), "taking consumes");
    }

    #[test]
    fn an_empty_queue_yields_nothing_to_inject() {
        assert!(InputQueue::new().take_marked().is_none());
    }

    #[test]
    fn clear_drops_undelivered_items() {
        let q = InputQueue::new();
        q.push("abandoned");
        q.clear();
        assert!(q.is_empty());
        assert!(q.take_marked().is_none());
    }
}
