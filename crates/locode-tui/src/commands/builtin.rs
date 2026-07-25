//! The commands that ship with the binary.
//!
//! Each one is an ordinary [`SlashCommand`] returning a value, so the set is testable
//! without a terminal (ADR-0026 §2) and the reducer decides what a `UiAction` means.

use std::sync::Arc;

use super::command::{CommandCtx, CommandResult, SlashCommand, UiAction};
use super::registry::{CommandRegistry, CommandSource};

/// Register every builtin. Called before skills so a skill cannot shadow one
/// (ADR-0026 §4).
pub fn register_builtins(registry: &mut CommandRegistry) {
    registry.register(Arc::new(NewSession), CommandSource::Builtin);
    registry.register(Arc::new(Quit), CommandSource::Builtin);
}

/// `/new` — discard the conversation and start over.
struct NewSession;

#[async_trait::async_trait]
impl SlashCommand for NewSession {
    fn name(&self) -> &'static str {
        "new"
    }

    fn description(&self) -> &'static str {
        "start a fresh session, clearing the conversation"
    }

    fn usage(&self) -> &'static str {
        "/new"
    }

    async fn execute(&self, ctx: &CommandCtx<'_>, _args: &str) -> CommandResult {
        // Refusing mid-run is the command's own rule, not the caller's: rebuilding the
        // session under a live turn would strand the run's events with nowhere to land.
        if ctx.is_running {
            CommandResult::Error("finish or cancel the run before /new".into())
        } else {
            CommandResult::Action(UiAction::NewSession)
        }
    }
}

/// `/quit` (alias `/exit`) — leave.
struct Quit;

#[async_trait::async_trait]
impl SlashCommand for Quit {
    fn name(&self) -> &'static str {
        "quit"
    }

    fn aliases(&self) -> &[&str] {
        &["exit"]
    }

    fn description(&self) -> &'static str {
        "exit locode"
    }

    fn usage(&self) -> &'static str {
        "/quit"
    }

    async fn execute(&self, _ctx: &CommandCtx<'_>, _args: &str) -> CommandResult {
        CommandResult::Action(UiAction::Quit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CommandRegistry {
        let mut r = CommandRegistry::new();
        register_builtins(&mut r);
        r
    }

    #[test]
    fn quit_is_reachable_under_both_names() {
        let r = registry();
        assert_eq!(r.resolve("/quit").expect("resolves").0.name(), "quit");
        assert_eq!(
            r.resolve("/exit").expect("resolves").0.name(),
            "quit",
            "the alias resolves to the same command"
        );
    }

    #[tokio::test]
    async fn quit_asks_the_caller_to_exit() {
        let r = registry();
        let (cmd, args) = r.resolve("/exit").expect("resolves");
        assert_eq!(
            cmd.execute(&CommandCtx::default(), args).await,
            CommandResult::Action(UiAction::Quit)
        );
    }

    #[tokio::test]
    async fn new_refuses_mid_run_and_starts_a_session_otherwise() {
        let r = registry();
        let (cmd, args) = r.resolve("/new").expect("resolves");
        assert_eq!(
            cmd.execute(&CommandCtx::default(), args).await,
            CommandResult::Action(UiAction::NewSession)
        );
        let running = CommandCtx {
            is_running: true,
            ..CommandCtx::default()
        };
        assert!(matches!(
            cmd.execute(&running, args).await,
            CommandResult::Error(_)
        ));
    }
}
