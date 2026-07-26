//! The user-facing effort ladder: **our** five tiers, and how they reach a wire.
//!
//! Effort naming is not portable. Anthropic exposes `low`/`medium`/`high`/
//! `xhigh`/`max`; other vendors ship fewer tiers, different names, or none — and
//! a tier that exists on one model can be rejected by the next. Exposing the
//! provider's vocabulary directly would make `/effort` mean different things on
//! different days and break the moment a model is switched.
//!
//! So the ladder below is **ours**. It is what the CLI flag, the settings key,
//! and `/effort` speak; each wire maps it onto whatever that API accepts. The
//! five rungs deliberately mirror Anthropic's, because Anthropic is what we run
//! today and a 1:1 mapping is the honest starting point — but the indirection is
//! the point: a wire with three tiers collapses rungs in *its* mapping rather
//! than forcing a different menu on the user.
//!
//! The rungs were confirmed against the live API rather than read off the
//! vendored Claude Code snapshot, which predates Fable 5 and lists only
//! `low|medium|high|max`. Probing `claude-fable-5`, `claude-opus-5` and
//! `claude-sonnet-5`: `low`, `medium`, `high`, `xhigh`, `max` are all accepted;
//! `ultra`, `ultrathink` and `extreme` are 400s. **`max` is the top rung.**
//!
//! "ultrathink" is not a rung and is deliberately absent: in Claude Code it is a
//! keyword matched in the *prompt* (`utils/thinking.ts:45`) that bumps effort for
//! that turn (`utils/effort.ts:321`) — a per-message nudge layered over the
//! setting, not a level of it.

use crate::request::ReasoningEffort;

/// One rung of the locode effort ladder.
///
/// Ordered shallow → deep. This is a closed set on purpose: an arbitrary
/// vendor string still reaches a wire through
/// [`ReasoningEffort::Other`](crate::ReasoningEffort::Other), but the *menu* is
/// a fixed ladder so the same word means the same intent everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effort {
    /// Short, scoped work; latency over depth.
    Low,
    /// The balanced middle.
    Medium,
    /// The default for intelligence-sensitive work.
    High,
    /// Best for coding and agentic runs.
    XHigh,
    /// Correctness over cost; can overthink simple tasks.
    Max,
}

impl Effort {
    /// Every rung, shallow → deep (the menu order).
    pub const ALL: [Effort; 5] = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::XHigh,
        Effort::Max,
    ];

    /// The canonical locode name (what the flag, settings, and `/effort` take).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }

    /// Parse a locode name, case-insensitively. `None` for anything else.
    #[must_use]
    pub fn parse(name: &str) -> Option<Effort> {
        let name = name.trim().to_ascii_lowercase();
        Effort::ALL.into_iter().find(|e| e.as_str() == name)
    }

    /// What this rung means, for the menu's second column.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            Effort::Low => "fastest; short, scoped tasks",
            Effort::Medium => "balanced",
            Effort::High => "the API default",
            Effort::XHigh => "best for coding and agentic work",
            Effort::Max => "deepest; correctness over cost",
        }
    }

    /// The tier this rung becomes on `api_schema`, for the menu's mapping
    /// column.
    ///
    /// Anthropic is 1:1 today, which is why the rungs are named as they are.
    /// The function exists so that stops being an assumption the moment a wire
    /// with a different ladder is added — the menu will show the collapse
    /// instead of hiding it.
    #[must_use]
    pub fn maps_to(self, api_schema: &str) -> &'static str {
        match api_schema {
            "anthropic" | "openai-responses" => self.as_str(),
            _ => "—",
        }
    }
}

impl From<Effort> for ReasoningEffort {
    fn from(effort: Effort) -> Self {
        match effort {
            Effort::Low => ReasoningEffort::Low,
            Effort::Medium => ReasoningEffort::Medium,
            Effort::High => ReasoningEffort::High,
            Effort::XHigh => ReasoningEffort::XHigh,
            Effort::Max => ReasoningEffort::Max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_case_insensitively() {
        for effort in Effort::ALL {
            assert_eq!(Effort::parse(effort.as_str()), Some(effort));
            assert_eq!(Effort::parse(&effort.as_str().to_uppercase()), Some(effort));
            assert_eq!(
                Effort::parse(&format!("  {}  ", effort.as_str())),
                Some(effort)
            );
        }
        assert_eq!(Effort::parse("ultra"), None, "the ladder is closed");
        assert_eq!(Effort::parse(""), None);
    }

    #[test]
    fn the_ladder_is_ordered_shallow_to_deep() {
        assert!(Effort::Low < Effort::Medium);
        assert!(Effort::High < Effort::XHigh);
        assert!(Effort::XHigh < Effort::Max);
    }

    /// Anthropic is 1:1 today — the indirection exists for the wire that is not.
    #[test]
    fn every_rung_has_a_distinct_neutral_tier() {
        let tiers: Vec<ReasoningEffort> = Effort::ALL.into_iter().map(Into::into).collect();
        for (i, a) in tiers.iter().enumerate() {
            for b in &tiers[i + 1..] {
                assert_ne!(a, b, "two rungs collapsed onto the same neutral tier");
            }
        }
    }

    #[test]
    fn anthropic_mapping_is_identity_and_unknown_wires_say_so() {
        for effort in Effort::ALL {
            assert_eq!(effort.maps_to("anthropic"), effort.as_str());
        }
        assert_eq!(Effort::High.maps_to("mock"), "—");
    }
}
