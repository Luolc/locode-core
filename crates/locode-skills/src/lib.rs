//! Agent skills for the locode coding agent: discovery from the four roots, and the
//! model-facing `<system-reminder>` listing (ADR-0025).
//!
//! One shared implementation for every harness pack — skills are loop-adjacent engine
//! machinery, not a pack surface (ADR-0023 §1). **No pack gains a tool**: the listing
//! carries each `SKILL.md`'s absolute path and the model reads it with the pack's
//! ordinary read tool (ADR-0025 §4), which is what two of the three shipped harness
//! CLIs actually do.

mod discover;
mod frontmatter;
mod listing;

pub use discover::{
    DiscoveredSkills, PROJECT_SKILLS_DIR, SKILL_FILE, Skill, SkillScope, SkillsConfig,
    ambiguous_names, discover, normalize_name,
};
pub use listing::{NO_SKILLS_BODY, char_budget, render_body};
