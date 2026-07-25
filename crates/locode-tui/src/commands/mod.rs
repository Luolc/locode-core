//! Slash commands: the contract, and the registry the dropdown renders (ADR-0026).
//!
//! A command **returns** its effect rather than performing it, so the whole command set
//! is testable without a terminal — the reducer applies the result.
//!
//! Lives in `locode-tui` rather than its own crate *(user decision, 2026-07-25; ADR-0026
//! §7 amended)*: the dependency runs commands → skills and the only consumer is this
//! TUI, which makes a separate crate a module in disguise. It earns one the day a
//! non-TUI surface needs commands — headless `locode -p "/commit …"` being the concrete
//! candidate.

mod builtin;
mod command;
mod dispatch;
mod matcher;
mod registry;
mod skill;
mod state;

pub use builtin::register_builtins;
pub use command::{ArgItem, CommandCtx, CommandResult, SlashCommand, UiAction};
pub use dispatch::execute;
pub use matcher::FuzzyMatcher;
pub use registry::{
    CommandRegistry, CommandSource, CommandTrigger, Invocation, LookupError, parse_invocation,
};
pub use skill::{SkillCommand, register_skills};
pub use state::{MAX_VISIBLE_ROWS, SlashState, SuggestionRow};
