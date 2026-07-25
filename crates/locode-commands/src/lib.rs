//! Slash commands for the locode coding agent: the contract, and the registry the UI
//! renders (ADR-0026).
//!
//! A command **returns** its effect rather than performing it, so the whole command set
//! is testable without a terminal. The dropdown — ranking, highlighting, submenus, ghost
//! text — is `locode-tui`'s; this crate only guarantees it has what it needs.

mod command;
mod registry;

pub use command::{ArgItem, CommandCtx, CommandResult, SlashCommand, UiAction};
pub use registry::{
    CommandRegistry, CommandSource, CommandTrigger, Invocation, LookupError, parse_invocation,
};
