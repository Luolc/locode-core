//! What a slash command *is* (ADR-0026 §1) and what it may return (§2).

/// A suggestion for a command's argument, feeding the second-level dropdown.
///
/// The three text fields are deliberately separate — that split is what lets `/model`
/// list "Grok 4.5 (current)" while matching on `grok-4.5` and inserting a model id
/// (grok's `ArgItem`, `slash/command.rs:81-93`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgItem {
    /// Shown in the dropdown.
    pub display: String,
    /// Matched against what the user typed.
    pub match_text: String,
    /// Written into the composer on acceptance.
    pub insert_text: String,
    /// The dimmed second column.
    pub description: String,
}

/// What executing a command produced.
///
/// A command **returns** its effect instead of performing it, so the command set is
/// testable without a terminal and the caller owns every side effect (ADR-0026 §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    /// Done; nothing to show.
    Handled,
    /// User-visible text.
    Message(String),
    /// Failed, with a reason.
    Error(String),
    /// Send this text as an ordinary prompt.
    Prompt(String),
    /// A skill body, spliced into the turn (ADR-0026 §4).
    InjectSkill {
        /// What the transcript shows (e.g. `/commit fix the typo`).
        display_text: String,
        /// What the model receives — body plus the arguments block (§8).
        prompt_text: String,
    },
    /// A UI action the caller interprets.
    Action(UiAction),
}

/// UI actions a command can ask for. Deliberately a closed set: a command that needs
/// something not listed here is asking the UI to grow a capability, which should be a
/// visible change rather than an opaque string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAction {
    /// Start a fresh session.
    NewSession,
    /// Exit the app.
    Quit,
}

/// Read-only context handed to `visible` and `suggest_args`.
///
/// Intentionally tiny (ADR-0003's rejection of god-object contexts applies here too);
/// it grows only when a command genuinely needs more.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandCtx<'a> {
    /// The model id in use, for commands that report or change it.
    pub model: Option<&'a str>,
    /// Whether a turn is in flight, for the commands that cannot run under one.
    pub is_running: bool,
}

/// A slash command.
///
/// A trait rather than an enum (ADR-0026 §1): skills are discovered per run and cannot
/// be enum variants, and `suggest_args` has to live on the command itself.
#[async_trait::async_trait]
pub trait SlashCommand: Send + Sync {
    /// Canonical name, without the leading `/`.
    fn name(&self) -> &str;

    /// Alternative names. Each becomes its own dropdown row (see `CommandTrigger`).
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// The dropdown's second column.
    fn description(&self) -> &str;

    /// Usage line, shown as the argument hint — e.g. `/model <name>`.
    fn usage(&self) -> &str;

    /// Whether the command accepts arguments at all.
    fn takes_args(&self) -> bool {
        false
    }

    /// Whether arguments are **required**. Only meaningful when `takes_args` is true.
    ///
    /// The two-bit model (grok's, `slash/command.rs:157-165`) — the distinction is real
    /// and easy to get wrong:
    ///
    /// | `takes_args` | `args_required` | Example | Enter with no args |
    /// |---|---|---|---|
    /// | `false` | `false` | `/exit` | executes |
    /// | `true` | `false` | `/compact [ctx]` | executes |
    /// | `true` | `true` | `/model <id>` | blocked, with the usage string |
    fn args_required(&self) -> bool {
        false
    }

    /// Argument suggestions for the second-level dropdown; `None` = no submenu.
    fn suggest_args(&self, _ctx: &CommandCtx<'_>, _query: &str) -> Option<Vec<ArgItem>> {
        None
    }

    /// Whether the command is currently offerable. Evaluated **per query**, not at
    /// registration, so a command that cannot run is never shown (Claude Code's rule).
    fn visible(&self, _ctx: &CommandCtx<'_>) -> bool {
        true
    }

    /// Run it. Async from day 0 (ADR-0026 §6) so a future `/model` can consult a
    /// provider without a breaking trait change.
    async fn execute(&self, ctx: &CommandCtx<'_>, args: &str) -> CommandResult;
}
