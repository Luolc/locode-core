//! Turning a rendered listing into an injected message, and deciding whether to inject
//! at all (ADR-0025 §3.1).
//!
//! The decision is **not** a per-skill ledger and **not** a stored hash: it is "does the
//! conversation I am about to send already contain this exact body?". That is codex's
//! `PreviousSectionState` resolution (`world_state/mod.rs:342-362`), and two properties
//! fall out of it for free that the other two harnesses each had to hand-manage —
//! compaction self-heals (drop the message, the next turn re-injects) and resume is
//! correct by construction (the marker is in the replayed transcript, so nothing is
//! re-sent).

use locode_protocol::{ContentBlock, Message, Role};

use crate::listing::NO_SKILLS_BODY;

/// Opening marker of an injected skills reminder — also what
/// [`already_delivered`] searches the transcript for.
const OPEN: &str = "<system-reminder>";
/// Closing marker.
const CLOSE: &str = "</system-reminder>";
/// Distinguishes *our* reminders from any other `<system-reminder>` in history
/// (project instructions use the same envelope).
const SENTINEL: &str = "The following skills are available for use:";

/// Wrap a listing body in the `User` `<system-reminder>` envelope (ADR-0023 §3 — this
/// role, never `Developer`, so the conversation ⇄ payload conversion stays losslessly
/// bidirectional).
#[must_use]
pub fn listing_message(body: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!("{OPEN}\n{body}\n{CLOSE}"),
        }],
    }
}

/// The message announcing that the last skill is gone.
#[must_use]
pub fn removal_message() -> Message {
    listing_message(NO_SKILLS_BODY)
}

/// Whether `history` already carries exactly this rendered body.
///
/// Compares the **whole body**, so any change at all — a new skill, an edited
/// description, one removed — is a mismatch that re-sends the entire listing. There is
/// deliberately no per-skill bookkeeping: both harnesses that keep one record the
/// announced key *before* budget truncation, which permanently silences a skill dropped
/// by the names-only tier (ADR-0025 §3.1).
#[must_use]
pub fn already_delivered(history: &[Message], body: &str) -> bool {
    history.iter().rev().any(|m| {
        m.role == Role::User
            && m.content.iter().any(|b| match b {
                ContentBlock::Text { text } => text.contains(body),
                _ => false,
            })
    })
}

/// Whether `history` carries any skills reminder at all — used to tell "never announced"
/// from "announced, then everything was removed", so a removal notice is only sent when
/// there was something to remove (codex's `previous_is_absent` guard).
#[must_use]
pub fn any_listing_delivered(history: &[Message]) -> bool {
    history.iter().any(|m| {
        m.role == Role::User
            && m.content.iter().any(|b| match b {
                ContentBlock::Text { text } => text.contains(SENTINEL),
                _ => false,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    #[test]
    fn wraps_the_body_in_a_user_system_reminder() {
        let m = listing_message("BODY");
        assert_eq!(m.role, Role::User);
        let ContentBlock::Text { text } = &m.content[0] else {
            panic!("text block")
        };
        assert_eq!(text, "<system-reminder>\nBODY\n</system-reminder>");
    }

    #[test]
    fn delivered_iff_the_exact_body_is_present() {
        let history = vec![listing_message("A\nB")];
        assert!(already_delivered(&history, "A\nB"));
        assert!(
            !already_delivered(&history, "A\nB\nC"),
            "any change re-sends"
        );
        assert!(!already_delivered(&[], "A"));
    }

    /// The compaction property: drop the message from history and the next turn treats
    /// the listing as never delivered — no hook, no flag, no bookkeeping.
    #[test]
    fn a_compacted_away_listing_counts_as_undelivered() {
        let body = "The following skills are available for use:\n\n- a: A";
        let mut history = vec![user("earlier"), listing_message(body), user("later")];
        assert!(already_delivered(&history, body));
        history.retain(
            |m| !matches!(&m.content[0], ContentBlock::Text { text } if text.contains(body)),
        );
        assert!(!already_delivered(&history, body));
    }

    /// The resume property: a replayed transcript still carries the marker, so nothing
    /// is re-sent — no process-local latch, no persisted announced-set.
    #[test]
    fn a_replayed_transcript_suppresses_re_injection() {
        let body = "The following skills are available for use:\n\n- a: A";
        let replayed = vec![listing_message(body), user("hello again")];
        assert!(already_delivered(&replayed, body));
    }

    #[test]
    fn removal_is_only_meaningful_after_something_was_announced() {
        assert!(!any_listing_delivered(&[user("hi")]));
        let history = vec![listing_message(
            "The following skills are available for use:\n\n- a: A",
        )];
        assert!(any_listing_delivered(&history));
    }

    #[test]
    fn an_assistant_echo_does_not_count_as_delivery() {
        let assistant = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "The following skills are available for use:".to_string(),
            }],
        };
        assert!(!any_listing_delivered(&[assistant]));
    }
}
