//! Registration, lookup, and the per-query view the dropdown renders (ADR-0026 §1, §3).

use std::sync::Arc;

use crate::command::{CommandCtx, SlashCommand};

/// Where a command came from. Kept from registration so precedence and later sources
/// (plugins, server-advertised) have somewhere to land — grok's `CommandSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandSource {
    /// Shipped with the binary.
    Builtin,
    /// Backed by a `SKILL.md` on disk.
    Skill,
}

/// One dropdown row: a canonical name **or** an alias.
///
/// Aliases are flattened into their own triggers (grok's `CommandTrigger`) so `/quit`
/// matches and displays as itself rather than hiding behind `/exit`. Matching operates
/// on triggers; execution resolves back to the command.
#[derive(Debug, Clone)]
pub struct CommandTrigger {
    /// The command's canonical name.
    pub canonical: String,
    /// The alias text, when this trigger is one.
    pub alias: Option<String>,
    /// Row label, with the leading slash (`/exit`).
    pub display: String,
    /// What a query is matched against (the alias when this is one).
    pub match_text: String,
    /// The dimmed second column.
    pub description: String,
    /// Usage line for the argument hint.
    pub usage: String,
    /// See [`SlashCommand::takes_args`].
    pub takes_args: bool,
    /// See [`SlashCommand::args_required`].
    pub args_required: bool,
    /// Provenance.
    pub source: CommandSource,
    /// Index into the registry's command list.
    pub command_index: usize,
}

/// Why a `/…` line did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    /// The text is not a command invocation at all (bare `/`, or `/ ` with a space).
    ///
    /// Distinguished from `Unknown` on purpose: `/usr/bin/env` and a sentence starting
    /// with a path are ordinary prompt text, not a typo the user wants corrected.
    NotAnInvocation,
    /// A well-formed `/name` that nothing is registered under.
    Unknown {
        /// The name as typed.
        name: String,
        /// Registered names close enough to be worth suggesting.
        did_you_mean: Vec<String>,
    },
}

/// A parsed `/name args` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation<'a> {
    /// The name, without the slash.
    pub name: &'a str,
    /// Everything after the first whitespace run, trimmed at the front.
    pub args: &'a str,
}

/// Split `/name rest` (grok's `parse_invocation`, `slash/mod.rs:1124-1147`).
///
/// `None` when the line does not start with `/`, or the token is empty — which is what
/// keeps a leading-slash *path* out of the command machinery.
#[must_use]
pub fn parse_invocation(line: &str) -> Option<Invocation<'_>> {
    let remainder = line.strip_prefix('/')?;
    if remainder.is_empty() {
        return None;
    }
    let end = remainder
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map_or(remainder.len(), |(i, _)| i);
    let name = remainder[..end].trim();
    if name.is_empty() {
        return None;
    }
    Some(Invocation {
        name,
        args: remainder[end..].trim_start(),
    })
}

/// The registered command set.
#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Arc<dyn SlashCommand>>,
    sources: Vec<CommandSource>,
    triggers: Vec<CommandTrigger>,
}

impl std::fmt::Debug for CommandRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `commands` holds trait objects with no `Debug`, so counts stand in for them.
        f.debug_struct("CommandRegistry")
            .field("commands", &self.commands.len())
            .field("sources", &self.sources)
            .field("triggers", &self.triggers.len())
            .finish()
    }
}

impl CommandRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command, flattening its aliases into triggers.
    ///
    /// A name already taken is **ignored, first-wins**: builtins register before skills
    /// (ADR-0026 §4), so a skill cannot shadow `/quit`. The shadowed skill stays
    /// reachable under its qualified name, which the caller registers separately.
    pub fn register(&mut self, command: Arc<dyn SlashCommand>, source: CommandSource) {
        let index = self.commands.len();
        let canonical = command.name().to_string();
        if self.trigger_for(&canonical).is_some() {
            return;
        }
        let mut new_triggers = vec![Self::make_trigger(
            &command, &canonical, None, index, source,
        )];
        for alias in command.aliases() {
            if self.trigger_for(alias).is_none() {
                new_triggers.push(Self::make_trigger(
                    &command,
                    &canonical,
                    Some(alias),
                    index,
                    source,
                ));
            }
        }
        self.commands.push(command);
        self.sources.push(source);
        self.triggers.extend(new_triggers);
    }

    fn make_trigger(
        command: &Arc<dyn SlashCommand>,
        canonical: &str,
        alias: Option<&str>,
        index: usize,
        source: CommandSource,
    ) -> CommandTrigger {
        let match_text = alias.unwrap_or(canonical).to_string();
        CommandTrigger {
            canonical: canonical.to_string(),
            alias: alias.map(str::to_string),
            display: format!("/{match_text}"),
            match_text,
            description: command.description().to_string(),
            usage: command.usage().to_string(),
            takes_args: command.takes_args(),
            args_required: command.args_required(),
            source,
            command_index: index,
        }
    }

    fn trigger_for(&self, text: &str) -> Option<&CommandTrigger> {
        self.triggers.iter().find(|t| t.match_text == text)
    }

    /// Every trigger, unfiltered — registration order.
    #[must_use]
    pub fn triggers(&self) -> &[CommandTrigger] {
        &self.triggers
    }

    /// Triggers offerable right now, sorted by source then name.
    ///
    /// `visible` runs here rather than at registration so a command gated on session
    /// state disappears the moment it stops being runnable.
    #[must_use]
    pub fn visible_triggers(&self, ctx: &CommandCtx<'_>) -> Vec<&CommandTrigger> {
        let mut out: Vec<&CommandTrigger> = self
            .triggers
            .iter()
            .filter(|t| self.commands[t.command_index].visible(ctx))
            .collect();
        out.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then_with(|| a.match_text.cmp(&b.match_text))
        });
        out
    }

    /// The command behind a trigger.
    #[must_use]
    pub fn command_at(&self, index: usize) -> Option<&Arc<dyn SlashCommand>> {
        self.commands.get(index)
    }

    /// Resolve a typed line to a command and its arguments.
    ///
    /// # Errors
    /// [`LookupError::NotAnInvocation`] when the line is not `/name…` at all;
    /// [`LookupError::Unknown`] for a well-formed name nothing is registered under —
    /// carrying near names, since an unknown command is a typo far more often than an
    /// intent we should silently forward (ADR-0026 §5).
    pub fn resolve<'a>(
        &self,
        line: &'a str,
    ) -> Result<(&Arc<dyn SlashCommand>, &'a str), LookupError> {
        let Some(inv) = parse_invocation(line) else {
            return Err(LookupError::NotAnInvocation);
        };
        match self.trigger_for(inv.name) {
            Some(trigger) => Ok((&self.commands[trigger.command_index], inv.args)),
            None => Err(LookupError::Unknown {
                name: inv.name.to_string(),
                did_you_mean: self.near_names(inv.name),
            }),
        }
    }

    /// Registered names sharing a prefix with `name`, or containing it — enough to catch
    /// a typo without pretending to be a spell-checker (real ranking arrives with the
    /// fuzzy matcher in the UI slice).
    fn near_names(&self, name: &str) -> Vec<String> {
        let lowered = name.to_ascii_lowercase();
        let mut out: Vec<String> = self
            .triggers
            .iter()
            .filter(|t| {
                let m = t.match_text.to_ascii_lowercase();
                m.starts_with(&lowered) || lowered.starts_with(&m) || m.contains(&lowered)
            })
            .map(|t| t.match_text.clone())
            .collect();
        out.sort();
        out.dedup();
        out.truncate(3);
        out
    }

    /// Whether Enter should execute this line, per the two-bit model.
    ///
    /// An unknown command is **not** complete — we error rather than forward it, so
    /// there is nothing to submit (contrast grok, which passes unknowns to its server).
    #[must_use]
    pub fn is_complete(&self, line: &str) -> bool {
        match self.resolve(line) {
            Ok((command, args)) => {
                !(command.takes_args() && command.args_required() && args.trim().is_empty())
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ArgItem, CommandResult, UiAction};

    struct Fake {
        name: &'static str,
        aliases: Vec<&'static str>,
        takes: bool,
        required: bool,
        visible: bool,
    }

    impl Fake {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                aliases: Vec::new(),
                takes: false,
                required: false,
                visible: true,
            }
        }
        fn alias(mut self, a: &'static str) -> Self {
            self.aliases.push(a);
            self
        }
        fn needs_args(mut self) -> Self {
            self.takes = true;
            self.required = true;
            self
        }
        fn optional_args(mut self) -> Self {
            self.takes = true;
            self
        }
        fn hidden(mut self) -> Self {
            self.visible = false;
            self
        }
        fn arc(self) -> Arc<dyn SlashCommand> {
            Arc::new(self)
        }
    }

    #[async_trait::async_trait]
    impl SlashCommand for Fake {
        fn name(&self) -> &str {
            self.name
        }
        fn aliases(&self) -> &[&str] {
            &self.aliases
        }
        fn description(&self) -> &'static str {
            "a fake"
        }
        fn usage(&self) -> &'static str {
            "/fake"
        }
        fn takes_args(&self) -> bool {
            self.takes
        }
        fn args_required(&self) -> bool {
            self.required
        }
        fn suggest_args(&self, _c: &CommandCtx<'_>, _q: &str) -> Option<Vec<ArgItem>> {
            None
        }
        fn visible(&self, _c: &CommandCtx<'_>) -> bool {
            self.visible
        }
        async fn execute(&self, _c: &CommandCtx<'_>, args: &str) -> CommandResult {
            CommandResult::Message(format!("{}:{args}", self.name))
        }
    }

    fn registry(cmds: Vec<Arc<dyn SlashCommand>>) -> CommandRegistry {
        let mut r = CommandRegistry::new();
        for c in cmds {
            r.register(c, CommandSource::Builtin);
        }
        r
    }

    #[test]
    fn parses_name_and_args() {
        let inv = parse_invocation("/model gpt-5 high").expect("invocation");
        assert_eq!(inv.name, "model");
        assert_eq!(inv.args, "gpt-5 high");

        let inv = parse_invocation("/quit").expect("invocation");
        assert_eq!((inv.name, inv.args), ("quit", ""));
    }

    /// A leading slash is not enough: a path, or a bare `/`, is ordinary text and must
    /// never reach the command machinery (ADR-0026 §5).
    #[test]
    fn a_leading_slash_alone_is_not_an_invocation() {
        assert!(parse_invocation("/").is_none());
        assert!(parse_invocation("/ spaced").is_none());
        assert!(parse_invocation("no slash").is_none());
        // A path *does* parse as a name — the registry rejects it as unknown, which is
        // where the "did you mean" lives. Pinned so the split stays deliberate.
        assert_eq!(
            parse_invocation("/usr/bin/env").unwrap().name,
            "usr/bin/env"
        );
    }

    #[test]
    fn aliases_become_their_own_rows_and_resolve_to_the_command() {
        let r = registry(vec![Fake::new("exit").alias("quit").arc()]);
        let labels: Vec<&str> = r.triggers().iter().map(|t| t.display.as_str()).collect();
        assert_eq!(labels, vec!["/exit", "/quit"]);
        let (cmd, _) = r.resolve("/quit").expect("resolves");
        assert_eq!(
            cmd.name(),
            "exit",
            "alias resolves to the canonical command"
        );
    }

    /// Builtins register first, so a later same-named command cannot shadow one.
    #[test]
    fn first_registration_wins() {
        let mut r = CommandRegistry::new();
        r.register(Fake::new("commit").arc(), CommandSource::Builtin);
        r.register(Fake::new("commit").arc(), CommandSource::Skill);
        assert_eq!(r.triggers().len(), 1);
        assert_eq!(r.triggers()[0].source, CommandSource::Builtin);
    }

    #[test]
    fn hidden_commands_are_filtered_per_query_but_stay_registered() {
        let r = registry(vec![
            Fake::new("shown").arc(),
            Fake::new("gone").hidden().arc(),
        ]);
        let ctx = CommandCtx::default();
        let visible: Vec<&str> = r
            .visible_triggers(&ctx)
            .iter()
            .map(|t| t.match_text.as_str())
            .collect();
        assert_eq!(visible, vec!["shown"]);
        assert_eq!(r.triggers().len(), 2, "still registered, just not offered");
    }

    #[test]
    fn unknown_names_carry_near_matches() {
        let r = registry(vec![Fake::new("model").arc(), Fake::new("modes").arc()]);
        match r.resolve("/mode") {
            Err(LookupError::Unknown { name, did_you_mean }) => {
                assert_eq!(name, "mode");
                assert_eq!(did_you_mean, vec!["model", "modes"]);
            }
            Err(other) => panic!("expected Unknown, got {other:?}"),
            Ok(_) => panic!("`/mode` must not resolve"),
        }
    }

    #[test]
    fn not_an_invocation_is_distinct_from_unknown() {
        let r = registry(vec![Fake::new("model").arc()]);
        assert!(matches!(
            r.resolve("hello"),
            Err(LookupError::NotAnInvocation)
        ));
        assert!(matches!(
            r.resolve("/nope"),
            Err(LookupError::Unknown { .. })
        ));
    }

    /// The two-bit model, exactly as the ADR's table states it.
    #[test]
    fn completeness_follows_the_two_bit_model() {
        let r = registry(vec![
            Fake::new("exit").arc(),
            Fake::new("compact").optional_args().arc(),
            Fake::new("model").needs_args().arc(),
        ]);
        assert!(r.is_complete("/exit"), "no args, none needed");
        assert!(r.is_complete("/compact"), "optional args omitted");
        assert!(r.is_complete("/compact ctx"), "optional args given");
        assert!(!r.is_complete("/model"), "required args missing → blocked");
        assert!(r.is_complete("/model gpt-5"));
    }

    /// Unlike grok, an unknown command is never "complete" — we error instead of
    /// forwarding it, so there is nothing to submit.
    #[test]
    fn unknown_commands_are_never_complete() {
        let r = registry(vec![Fake::new("exit").arc()]);
        assert!(!r.is_complete("/nope"));
    }

    #[tokio::test]
    async fn execute_returns_a_value_and_touches_nothing() {
        let r = registry(vec![Fake::new("echo").optional_args().arc()]);
        let (cmd, args) = r.resolve("/echo hello world").expect("resolves");
        let ctx = CommandCtx::default();
        assert_eq!(
            cmd.execute(&ctx, args).await,
            CommandResult::Message("echo:hello world".into())
        );
    }

    #[test]
    fn ui_actions_are_a_closed_set() {
        assert_ne!(UiAction::NewSession, UiAction::Quit);
    }
}
