//! The model-facing skill listing (ADR-0025 §3): grok's verbatim format, its char
//! budget, and its three-tier degrade.
//!
//! Pure — a function of `(skills, context_window)`. Rendering the body is separate from
//! deciding whether to *send* it (S4), because the update rule compares whole bodies:
//! the same text must come out for the same inputs or the diff would fire spuriously.

use std::fmt::Write as _;

use crate::discover::{Skill, ambiguous_names};

/// Fraction of the context window the listing may occupy — grok's
/// `SKILL_BUDGET_CONTEXT_PERCENT` (`listing.rs:12`).
///
/// A **cap, not a reservation**: at any realistic skill count it never binds, so a
/// tighter number could only ever truncate the text that does the routing while saving
/// nothing in the common case (ADR-0025 §3).
const BUDGET_CONTEXT_PERCENT: f64 = 0.5;
/// Fallback when the context window is unknown — grok's `DEFAULT_CHAR_BUDGET`
/// (200k tokens × 4 bytes × 50 %).
const DEFAULT_CHAR_BUDGET: usize = 400_000;
/// Per-entry cap on description + `when-to-use` combined (grok's
/// `MAX_LISTING_COMBINED_BYTES`).
const MAX_ENTRY_BYTES: usize = 400;
/// Floor below which shortening a description stops being useful (grok's
/// `MIN_DESC_LENGTH`).
const MIN_DESC_LEN: usize = 20;
/// Grok's header — it names no tool, which is exactly right for us: there is none.
const HEADER: &str = "The following skills are available for use:";
/// Emitted when the last skill disappears (ADR-0025 §3.1). Codex is the only surveyed
/// harness that says anything here; the other two go silent and leave a stale
/// instruction standing.
pub const NO_SKILLS_BODY: &str = "No skills are currently available.";

/// The char budget for a context window of `context_window_tokens`.
#[must_use]
pub fn char_budget(context_window_tokens: Option<u64>) -> usize {
    context_window_tokens.map_or(DEFAULT_CHAR_BUDGET, |tokens| {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let budget = (tokens as f64 * 4.0 * BUDGET_CONTEXT_PERCENT) as usize;
        budget
    })
}

/// Render the listing body, or `None` when there is nothing listable.
///
/// The body is what the update rule compares (ADR-0025 §3.1), so it must be a pure
/// function of the inputs — no timestamps, no iteration-order dependence.
#[must_use]
pub fn render_body(skills: &[Skill], budget: usize) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let ambiguous = ambiguous_names(skills);
    let entries: Vec<Entry<'_>> = skills
        .iter()
        .map(|s| Entry {
            name: s.display_name(ambiguous.iter().any(|a| a == &s.name)),
            description: s.description.as_str(),
            when_to_use: s.when_to_use.as_deref(),
            path: s.path.display().to_string(),
        })
        .collect();

    // Tier 1: everything, uncapped per entry beyond the 400-byte rule.
    let full = assemble(&entries, |e| e.render(MAX_ENTRY_BYTES));
    if full.len() <= budget {
        return Some(full);
    }
    // Tier 2: shorten descriptions toward the floor.
    let short = assemble(&entries, |e| e.render(MIN_DESC_LEN));
    if short.len() <= budget {
        return Some(short);
    }
    // Tier 3: names only, dropping entries that no longer fit, with an overflow marker.
    Some(names_only(&entries, budget))
}

/// One rendered row's inputs.
struct Entry<'a> {
    name: String,
    description: &'a str,
    when_to_use: Option<&'a str>,
    path: String,
}

impl Entry<'_> {
    /// `- name: desc` (+ `  Use when: …`) + `  Absolute path: …`.
    ///
    /// `combined` caps description + `when-to-use` together, split proportionally with a
    /// floor for either field — grok's `proportional_budgets`.
    fn render(&self, combined: usize) -> String {
        let (desc_budget, wtu_budget) = split_budget(
            combined,
            self.description.len(),
            self.when_to_use.map_or(0, str::len),
        );
        let mut out = format!("- {}: {}", self.name, clip(self.description, desc_budget));
        if let Some(wtu) = self.when_to_use {
            let _ = write!(out, "\n  Use when: {}", clip(wtu, wtu_budget));
        }
        let _ = write!(out, "\n  Absolute path: {}", self.path);
        out
    }

    fn name_only(&self) -> String {
        format!("- {}", self.name)
    }
}

fn assemble(entries: &[Entry<'_>], render: impl Fn(&Entry<'_>) -> String) -> String {
    let rows: Vec<String> = entries.iter().map(render).collect();
    format!("{HEADER}\n\n{}", rows.join("\n"))
}

/// Names-only tier: keep what fits, then say how many were dropped and where they live.
fn names_only(entries: &[Entry<'_>], budget: usize) -> String {
    let mut out = format!("{HEADER}\n\n");
    let mut kept = 0usize;
    for e in entries {
        let row = e.name_only();
        // Leave room for the overflow line; if even that does not fit we still emit the
        // header, because a truncated listing is more useful than none.
        if out.len() + row.len() + 1 > budget.saturating_sub(80) {
            break;
        }
        out.push_str(&row);
        out.push('\n');
        kept += 1;
    }
    let remaining = entries.len() - kept;
    if remaining > 0 {
        let dir = entries
            .get(kept)
            .map_or_else(String::new, |e| parent_of(&e.path));
        let _ = writeln!(out, "... and {remaining} more skills in {dir}");
    }
    out.trim_end().to_string()
}

fn parent_of(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .and_then(std::path::Path::parent)
        .map_or_else(|| path.to_string(), |p| p.display().to_string())
}

/// Split `combined` between description and `when-to-use` proportionally to their
/// lengths, with a `MIN_DESC_LEN` floor for either when the budget allows.
fn split_budget(combined: usize, desc_len: usize, wtu_len: usize) -> (usize, usize) {
    if wtu_len == 0 {
        return (combined, 0);
    }
    let total = desc_len + wtu_len;
    if total <= combined {
        return (desc_len, wtu_len);
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let mut desc = ((combined as f64) * (desc_len as f64) / (total as f64)) as usize;
    desc = desc.max(MIN_DESC_LEN.min(combined));
    let wtu = combined.saturating_sub(desc);
    (desc, wtu)
}

/// Truncate on a char boundary, marking the cut so the model can tell.
fn clip(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    let mut end = budget.saturating_sub(1);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::SkillScope;
    use std::path::PathBuf;

    fn skill(name: &str, desc: &str, wtu: Option<&str>) -> Skill {
        Skill {
            name: name.to_string(),
            scope: SkillScope::User,
            description: desc.to_string(),
            when_to_use: wtu.map(str::to_string),
            path: PathBuf::from(format!("/home/u/.locode/skills/{name}/SKILL.md")),
            disable_model_invocation: false,
            user_invocable: true,
        }
    }

    /// The verbatim shape from ADR-0025 §3 — header, `- name: desc`, the optional
    /// `Use when:` line, and the absolute path, each continuation indented two spaces.
    #[test]
    fn renders_the_documented_block() {
        let skills = vec![
            skill("commit", "Make a commit", Some("on push")),
            skill("review", "Review a diff", None),
        ];
        let body = render_body(&skills, 10_000).expect("body");
        assert_eq!(
            body,
            "The following skills are available for use:\n\
             \n\
             - commit: Make a commit\n\
             \x20\x20Use when: on push\n\
             \x20\x20Absolute path: /home/u/.locode/skills/commit/SKILL.md\n\
             - review: Review a diff\n\
             \x20\x20Absolute path: /home/u/.locode/skills/review/SKILL.md"
        );
    }

    /// A skill with no `when-to-use` renders **two** lines, not three with an empty one.
    #[test]
    fn missing_when_to_use_omits_the_line_entirely() {
        let body = render_body(&[skill("x", "D", None)], 10_000).unwrap();
        assert!(!body.contains("Use when:"), "{body}");
        assert_eq!(body.lines().filter(|l| l.starts_with("  ")).count(), 1);
    }

    #[test]
    fn no_skills_renders_nothing() {
        assert!(render_body(&[], 10_000).is_none());
    }

    /// Ambiguous names render qualified so the model can tell them apart.
    #[test]
    fn a_name_in_two_scopes_renders_qualified() {
        let mut project = skill("commit", "P", None);
        project.scope = SkillScope::Project;
        let body = render_body(&[project, skill("commit", "U", None)], 10_000).unwrap();
        assert!(body.contains("- project:commit: P"), "{body}");
        assert!(body.contains("- user:commit: U"), "{body}");
    }

    #[test]
    fn tier2_shortens_descriptions_before_dropping_anything() {
        let long = "x".repeat(300);
        let skills: Vec<Skill> = (0..5)
            .map(|i| skill(&format!("s{i}"), &long, None))
            .collect();
        let full = render_body(&skills, 100_000).unwrap();
        let squeezed = render_body(&skills, full.len() - 200).unwrap();
        assert!(squeezed.len() < full.len());
        for i in 0..5 {
            assert!(squeezed.contains(&format!("- s{i}:")), "kept all names");
        }
        assert!(squeezed.contains('…'), "descriptions were clipped");
    }

    #[test]
    fn tier3_falls_back_to_names_only_with_an_overflow_marker() {
        let skills: Vec<Skill> = (0..30)
            .map(|i| skill(&format!("s{i:02}"), &"d".repeat(200), None))
            .collect();
        // Tight enough that even bare names must be dropped — 30 short names would
        // otherwise fit, which is what makes tier 3 rare in practice.
        let body = render_body(&skills, 200).unwrap();
        assert!(body.starts_with(HEADER), "{body}");
        assert!(!body.contains("Absolute path:"), "names only: {body}");
        assert!(body.contains("... and "), "{body}");
        assert!(
            body.contains("more skills in /home/u/.locode/skills"),
            "{body}"
        );
    }

    /// The budget is a cap, not a reservation: a realistic set never trips a tier.
    #[test]
    fn a_realistic_set_stays_on_tier_one() {
        let skills: Vec<Skill> = (0..12)
            .map(|i| {
                skill(
                    &format!("skill-{i}"),
                    "A reasonably detailed description of what this skill does.",
                    Some("when the task matches"),
                )
            })
            .collect();
        let body = render_body(&skills, char_budget(Some(200_000))).unwrap();
        assert!(!body.contains('…'), "nothing clipped: {body}");
        assert_eq!(body.matches("Absolute path:").count(), 12);
    }

    #[test]
    fn budget_scales_with_the_context_window_and_falls_back() {
        assert_eq!(char_budget(Some(200_000)), 400_000);
        assert_eq!(char_budget(None), DEFAULT_CHAR_BUDGET);
    }

    /// Rendering must be a pure function of its inputs — the update rule compares whole
    /// bodies, so any instability would re-send the listing every turn.
    #[test]
    fn rendering_is_stable() {
        let skills = vec![skill("a", "A", None), skill("b", "B", Some("w"))];
        assert_eq!(render_body(&skills, 10_000), render_body(&skills, 10_000));
    }

    #[test]
    fn clip_respects_char_boundaries() {
        let s = "日本語のテキスト";
        let out = clip(s, 7);
        assert!(out.len() <= 7 + '…'.len_utf8());
        assert!(out.ends_with('…'));
    }
}
