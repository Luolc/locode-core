//! Project instructions (`AGENTS.md`) for the locode coding agent: discovery,
//! assembly, and the one `User` `<system-reminder>` message they are injected as.
//!
//! One shared implementation for every harness pack — instruction loading is
//! loop-adjacent engine machinery, not a pack surface (ADR-0023 §1). Split out of
//! `locode-host`/`locode-engine` into its own crate by the ADR-0002 amendment of
//! 2026-07-24, alongside a sibling `locode-skills`, because the two are separate
//! features that happen to share an envelope — not one "context" concern.

pub mod load;
pub mod render;

pub use load::{
    InstructionEntry, InstructionsConfig, ProjectInstructions, load_project_instructions,
};
pub use render::{
    already_delivered, any_delivered, instructions_hash, removal_delivered, removal_message,
    render_body, render_instructions,
};
