//! The commands that ship with the binary.
//!
//! Each one is an ordinary [`SlashCommand`] returning a value, so the set is testable
//! without a terminal (ADR-0026 §2) and the reducer decides what a `UiAction` means.

use std::sync::Arc;

use super::command::{ArgItem, CommandCtx, CommandResult, SlashCommand, UiAction};
use super::registry::{CommandRegistry, CommandSource};

/// Register every builtin. Called before skills so a skill cannot shadow one
/// (ADR-0026 §4).
pub fn register_builtins(registry: &mut CommandRegistry) {
    registry.register(Arc::new(Help), CommandSource::Builtin);
    registry.register(Arc::new(Model), CommandSource::Builtin);
    registry.register(Arc::new(NewSession), CommandSource::Builtin);
    registry.register(Arc::new(Quit), CommandSource::Builtin);
}

/// `/help [command]` — what is available, or what one command does.
///
/// Also the argument submenu's first real user: `suggest_args` offers every command
/// the registry currently holds, which is why `CommandCtx` carries it.
struct Help;

#[async_trait::async_trait]
impl SlashCommand for Help {
    fn name(&self) -> &'static str {
        "help"
    }

    fn description(&self) -> &'static str {
        "list the commands, or explain one"
    }

    fn usage(&self) -> &'static str {
        "/help [command]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, ctx: &CommandCtx<'_>, _query: &str) -> Option<Vec<ArgItem>> {
        let registry = ctx.registry?;
        Some(
            registry
                .visible_triggers(ctx)
                .into_iter()
                .map(|t| ArgItem {
                    display: t.display.clone(),
                    match_text: t.match_text.clone(),
                    insert_text: t.match_text.clone(),
                    description: t.description.clone(),
                })
                .collect(),
        )
    }

    async fn execute(&self, ctx: &CommandCtx<'_>, args: &str) -> CommandResult {
        let Some(registry) = ctx.registry else {
            return CommandResult::Error("the command list is unavailable".into());
        };
        let wanted = args.trim().trim_start_matches('/');
        if wanted.is_empty() {
            let mut lines: Vec<String> = registry
                .visible_triggers(ctx)
                .into_iter()
                .map(|t| format!("{} — {}", t.display, t.description))
                .collect();
            lines.insert(0, "commands:".to_string());
            return CommandResult::Message(lines.join("\n"));
        }
        match registry
            .visible_triggers(ctx)
            .into_iter()
            .find(|t| t.match_text == wanted)
        {
            Some(t) => CommandResult::Message(format!("{} — {}", t.usage, t.description)),
            None => CommandResult::Error(format!("unknown command: /{wanted}")),
        }
    }
}

/// `/model` — report the model this session is using, and how to change it.
///
/// **Read-only on purpose.** Switching mid-session needs a model-selection seam on the
/// provider registry, which is an ask-first change to core's public surface and is
/// tracked separately. Rather than offer a list of models we cannot verify — locode
/// passes `--model` through to whatever wire is configured and keeps no catalog — this
/// reports what is in use and names the two places that set it.
struct Model;

#[async_trait::async_trait]
impl SlashCommand for Model {
    fn name(&self) -> &'static str {
        "model"
    }

    fn description(&self) -> &'static str {
        "show the model this session is using"
    }

    fn usage(&self) -> &'static str {
        "/model"
    }

    async fn execute(&self, ctx: &CommandCtx<'_>, _args: &str) -> CommandResult {
        let Some(model) = ctx.model else {
            return CommandResult::Message("no model yet — the session is still starting".into());
        };
        CommandResult::Message(format!(
            "{model}\nto use a different one, start locode with --model <id>, \
             or set \"model\" in ~/.locode/settings.json"
        ))
    }
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

    /// `/help` with no argument lists everything; with one it explains that command.
    #[tokio::test]
    async fn help_lists_the_commands_and_explains_one() {
        let r = registry();
        let ctx = CommandCtx {
            registry: Some(&r),
            ..CommandCtx::default()
        };
        let (cmd, _) = r.resolve("/help").expect("resolves");

        let CommandResult::Message(all) = cmd.execute(&ctx, "").await else {
            panic!("expected a listing");
        };
        for name in ["/help", "/model", "/new", "/quit", "/exit"] {
            assert!(all.contains(name), "{name} listed: {all}");
        }

        let CommandResult::Message(one) = cmd.execute(&ctx, "new").await else {
            panic!("expected an explanation");
        };
        assert!(one.starts_with("/new "), "usage first: {one}");
        // A leading slash is accepted too — it is what the submenu inserts is *not*,
        // but it is what a user types.
        assert_eq!(cmd.execute(&ctx, "/new").await, CommandResult::Message(one));

        assert!(matches!(
            cmd.execute(&ctx, "nope").await,
            CommandResult::Error(_)
        ));
    }

    /// `/help`'s argument rows are the command set — the submenu's first real user.
    #[test]
    fn help_suggests_every_command_as_an_argument() {
        let r = registry();
        let ctx = CommandCtx {
            registry: Some(&r),
            ..CommandCtx::default()
        };
        let (cmd, _) = r.resolve("/help").expect("resolves");
        let items = cmd.suggest_args(&ctx, "").expect("suggestions");
        let shown: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert!(shown.contains(&"/quit"), "{shown:?}");
        let quit = items.iter().find(|i| i.display == "/quit").unwrap();
        assert_eq!(quit.match_text, "quit", "matched without the slash");
        assert_eq!(quit.insert_text, "quit", "inserted without the slash");
    }

    /// `/model` reports; it does not switch. The message names the two surfaces that
    /// actually set the model, both of which exist.
    #[tokio::test]
    async fn model_reports_the_active_model_and_how_to_change_it() {
        let r = registry();
        let (cmd, args) = r.resolve("/model").expect("resolves");
        let ctx = CommandCtx {
            model: Some("claude-sonnet-5"),
            ..CommandCtx::default()
        };
        let CommandResult::Message(msg) = cmd.execute(&ctx, args).await else {
            panic!("expected a report");
        };
        assert!(msg.starts_with("claude-sonnet-5"), "{msg}");
        assert!(msg.contains("--model"), "{msg}");
        assert!(msg.contains("~/.locode/settings.json"), "{msg}");

        // Before the session is ready there is nothing to report, and it says so.
        let CommandResult::Message(msg) = cmd.execute(&CommandCtx::default(), args).await else {
            panic!("expected a report");
        };
        assert!(msg.contains("still starting"), "{msg}");
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
