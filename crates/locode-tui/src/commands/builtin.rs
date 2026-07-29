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
    registry.register(Arc::new(AddDir), CommandSource::Builtin);
    registry.register(Arc::new(EffortCmd), CommandSource::Builtin);
    registry.register(Arc::new(Help), CommandSource::Builtin);
    registry.register(Arc::new(Model), CommandSource::Builtin);
    registry.register(Arc::new(NewSession), CommandSource::Builtin);
    registry.register(Arc::new(Resume), CommandSource::Builtin);
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
                // No leading slash: inside `/help`'s own menu a `/quit` row reads as
                // "run /quit", when it means "explain quit". The bare name is also what
                // the row inserts, so shown and inserted agree.
                .map(|t| ArgItem {
                    display: t.match_text.clone(),
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

/// Models offered by `/model`'s menu.
///
/// A short curated list, not a catalog: locode keeps none, and `--model` passes whatever
/// you give it straight to the configured wire. Anything not listed can still be typed —
/// the menu closes when nothing matches and Enter submits what you wrote — so this is a
/// shortcut, never a restriction. These are Anthropic ids; on another wire they will
/// fail at request time, which is the user's call to make.
const MODELS: &[&str] = &[
    "claude-fable-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-sonnet-5",
];

/// `/model [id]` — report the model in use, or switch to another.
///
/// Switching does two things, as both reference harnesses do: it changes the **running**
/// session (in memory — two sessions never fight over each other's model) and writes the
/// id to the user-global settings, which is what the **next** session starts with
/// (Claude Code: `updateSettingsForSource('userSettings', { model })`; grok:
/// `[models].default` in `~/.grok/config.toml`). Neither has a project-scoped model.
struct Model;

#[async_trait::async_trait]
impl SlashCommand for Model {
    fn name(&self) -> &'static str {
        "model"
    }

    fn description(&self) -> &'static str {
        "show or switch the model this session uses"
    }

    fn usage(&self) -> &'static str {
        "/model [id]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, ctx: &CommandCtx<'_>, _query: &str) -> Option<Vec<ArgItem>> {
        Some(
            MODELS
                .iter()
                .map(|id| ArgItem {
                    display: (*id).to_string(),
                    match_text: (*id).to_string(),
                    insert_text: (*id).to_string(),
                    description: if ctx.model == Some(*id) {
                        "in use".to_string()
                    } else {
                        String::new()
                    },
                })
                .collect(),
        )
    }

    async fn execute(&self, ctx: &CommandCtx<'_>, args: &str) -> CommandResult {
        let wanted = args.trim();
        if wanted.is_empty() {
            let Some(model) = ctx.model else {
                return CommandResult::Message(
                    "no model yet — the session is still starting".into(),
                );
            };
            return CommandResult::Message(format!("{model}\ntype /model <id> to switch"));
        }
        if ctx.model == Some(wanted) {
            return CommandResult::Message(format!("already using {wanted}"));
        }
        CommandResult::Action(UiAction::SetModel(wanted.to_string()))
    }
}

/// `/add-dir <path>` — the interactive half of `--add-dir`.
///
/// Widens the tool jail immediately and registers the directory as a discovery
/// root, so its `AGENTS.md` and `.agents/skills` land on the next turn (both
/// rescans already run per turn — ADR-0023, ADR-0025).
///
/// Not persisted, unlike `/model` and `/effort`: those are preferences, whereas
/// a working directory belongs to the task at hand. Carrying it into every
/// future session would keep widening the jail of unrelated runs — `--add-dir`
/// is how a root becomes part of a session's startup.
struct AddDir;

#[async_trait::async_trait]
impl SlashCommand for AddDir {
    fn name(&self) -> &'static str {
        "add-dir"
    }

    fn description(&self) -> &'static str {
        "let the agent work in another directory, for this session"
    }

    fn usage(&self) -> &'static str {
        "/add-dir <path>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    async fn execute(&self, _ctx: &CommandCtx<'_>, args: &str) -> CommandResult {
        let raw = args.trim();
        if raw.is_empty() {
            return CommandResult::Error("give a directory: /add-dir <path>".into());
        }
        // `~` is expanded by the host on resolve, but a jail root is canonicalized
        // at add time, so expand here too — otherwise `~/src` looks like a
        // relative directory named `~`.
        let expanded = match raw.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map_or_else(
                || std::path::PathBuf::from(raw),
                |home| std::path::PathBuf::from(home).join(rest),
            ),
            None => std::path::PathBuf::from(raw),
        };
        CommandResult::Action(UiAction::AddDir(expanded))
    }
}

/// `/effort [rung]` — report or change how hard the model thinks.
///
/// The rungs are **locode's**, not a provider's (see `locode_provider::Effort`):
/// effort vocabularies differ per vendor and per model generation, so the menu
/// stays fixed and each wire maps it. The second column shows what the rung
/// becomes on the wire in use, so a future collapse (a provider with three
/// tiers) is visible rather than silent.
///
/// `auto` clears the override and lets the API apply its own default, mirroring
/// Claude Code's `/effort [low|medium|high|max|auto]`.
struct EffortCmd;

/// The menu entry that clears the override.
const EFFORT_AUTO: &str = "auto";

#[async_trait::async_trait]
impl SlashCommand for EffortCmd {
    fn name(&self) -> &'static str {
        "effort"
    }

    fn description(&self) -> &'static str {
        "show or set how hard the model thinks"
    }

    fn usage(&self) -> &'static str {
        "/effort [low|medium|high|xhigh|max|auto]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, ctx: &CommandCtx<'_>, _query: &str) -> Option<Vec<ArgItem>> {
        let wire = ctx.api_schema.unwrap_or("");
        let mut items: Vec<ArgItem> = locode_core::Effort::ALL
            .iter()
            .map(|effort| {
                let mapped = effort.maps_to(wire);
                let mut description = if mapped == effort.as_str() {
                    effort.hint().to_string()
                } else {
                    // Only worth the noise when the rung is NOT 1:1 on this wire.
                    format!("{} — sends {mapped}", effort.hint())
                };
                if ctx.effort == Some(*effort) {
                    description = format!("in use · {description}");
                }
                ArgItem {
                    display: effort.as_str().to_string(),
                    match_text: effort.as_str().to_string(),
                    insert_text: effort.as_str().to_string(),
                    description,
                }
            })
            .collect();
        items.push(ArgItem {
            display: EFFORT_AUTO.to_string(),
            match_text: EFFORT_AUTO.to_string(),
            insert_text: EFFORT_AUTO.to_string(),
            description: if ctx.effort.is_none() {
                "in use · let the API choose".to_string()
            } else {
                "let the API choose".to_string()
            },
        });
        Some(items)
    }

    async fn execute(&self, ctx: &CommandCtx<'_>, args: &str) -> CommandResult {
        let wanted = args.trim();
        if wanted.is_empty() {
            let current = ctx.effort.map_or_else(
                || "auto (the API's default)".to_string(),
                |e| e.as_str().to_string(),
            );
            return CommandResult::Message(format!("{current}\ntype /effort <rung> to change it"));
        }
        if wanted.eq_ignore_ascii_case(EFFORT_AUTO) {
            return CommandResult::Action(UiAction::SetEffort(None));
        }
        match locode_core::Effort::parse(wanted) {
            Some(effort) => CommandResult::Action(UiAction::SetEffort(Some(effort))),
            None => CommandResult::Error(format!(
                "unknown effort {wanted:?} — expected one of {}, or {EFFORT_AUTO}",
                locode_core::Effort::ALL
                    .iter()
                    .map(|e| e.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
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

/// `/resume` — pick an earlier session and continue it (ADR-0029).
struct Resume;

#[async_trait::async_trait]
impl SlashCommand for Resume {
    fn name(&self) -> &'static str {
        "resume"
    }

    fn description(&self) -> &'static str {
        "resume an earlier session, chosen from a list"
    }

    fn usage(&self) -> &'static str {
        "/resume"
    }

    async fn execute(&self, ctx: &CommandCtx<'_>, _args: &str) -> CommandResult {
        // Same refusal, same reason, as `/new` above: swapping the conversation
        // under a live turn strands the run's events. The rule lives in the
        // command, not the caller — that is the seam `/new` established, and
        // `/resume` is the same class of action (ADR-0029).
        if ctx.is_running {
            CommandResult::Error("finish or cancel the run before /resume".into())
        } else {
            CommandResult::Action(UiAction::ResumePicker)
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

    /// `/resume` opens the picker when idle and refuses mid-run — the rule `/new`
    /// established, in the command's own `execute` rather than in the caller
    /// (ADR-0029). Swapping the conversation under a live turn would strand that
    /// run's events.
    #[tokio::test]
    async fn resume_opens_the_picker_but_refuses_mid_run() {
        let r = registry();
        let (cmd, args) = r.resolve("/resume").expect("resolves");
        assert_eq!(
            cmd.execute(&CommandCtx::default(), args).await,
            CommandResult::Action(UiAction::ResumePicker),
            "idle: the picker opens"
        );

        let running = CommandCtx {
            is_running: true,
            ..CommandCtx::default()
        };
        let refused = cmd.execute(&running, args).await;
        assert!(
            matches!(refused, CommandResult::Error(ref m) if m.contains("before /resume")),
            "mid-run must refuse, not queue: {refused:?}"
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
        assert!(shown.contains(&"quit"), "{shown:?}");
        assert!(
            !shown.iter().any(|s| s.starts_with('/')),
            "no leading slash — these name a command, they do not run one: {shown:?}"
        );
        let quit = items.iter().find(|i| i.display == "quit").unwrap();
        assert_eq!(quit.match_text, "quit");
        assert_eq!(quit.insert_text, "quit");
    }

    /// Bare `/model` reports; `/model <id>` asks the caller to switch.
    #[tokio::test]
    async fn model_reports_with_no_argument_and_switches_with_one() {
        let r = registry();
        let (cmd, _) = r.resolve("/model").expect("resolves");
        let ctx = CommandCtx {
            model: Some("claude-sonnet-5"),
            ..CommandCtx::default()
        };

        let CommandResult::Message(msg) = cmd.execute(&ctx, "").await else {
            panic!("expected a report");
        };
        assert!(msg.starts_with("claude-sonnet-5"), "{msg}");

        assert_eq!(
            cmd.execute(&ctx, "claude-opus-5").await,
            CommandResult::Action(UiAction::SetModel("claude-opus-5".into()))
        );

        // Switching to what is already running is a no-op with a word about it.
        assert!(matches!(
            cmd.execute(&ctx, "claude-sonnet-5").await,
            CommandResult::Message(m) if m.contains("already using")
        ));

        // Before the session is ready there is nothing to report, and it says so.
        let CommandResult::Message(msg) = cmd.execute(&CommandCtx::default(), "").await else {
            panic!("expected a report");
        };
        assert!(msg.contains("still starting"), "{msg}");
    }

    /// The menu is a shortcut, not a restriction: it lists the curated ids and marks
    /// the one in use, and an unlisted id still switches.
    #[tokio::test]
    async fn model_offers_the_curated_list_but_accepts_anything() {
        let r = registry();
        let (cmd, _) = r.resolve("/model").expect("resolves");
        let ctx = CommandCtx {
            model: Some("claude-opus-5"),
            ..CommandCtx::default()
        };
        let items = cmd.suggest_args(&ctx, "").expect("suggestions");
        let ids: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert_eq!(ids, MODELS);
        let current = items.iter().find(|i| i.display == "claude-opus-5").unwrap();
        assert_eq!(current.description, "in use");

        assert_eq!(
            cmd.execute(&ctx, "some-other-model").await,
            CommandResult::Action(UiAction::SetModel("some-other-model".into())),
            "an id outside the list is still accepted"
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
