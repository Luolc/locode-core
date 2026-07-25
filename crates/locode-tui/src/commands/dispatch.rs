//! Resolving a typed line and running it.
//!
//! The one awaiting step in the command path, kept out of the reducer: `App::update`
//! stays sans-IO and asks for execution with a `Cmd`, the event loop awaits this, and
//! the reducer applies the `CommandResult` it gets back. Everything here is a pure
//! function of the registry, so the whole dispatch table is testable without a terminal.

use super::command::{CommandCtx, CommandResult};
use super::registry::{CommandRegistry, LookupError};

/// Resolve `line` and run whatever it names.
///
/// Errors — an unknown name, or a required argument the user did not supply — come back
/// as [`CommandResult::Error`] rather than a separate channel, so the caller has exactly
/// one thing to render.
pub async fn execute(
    registry: &CommandRegistry,
    ctx: &CommandCtx<'_>,
    line: &str,
) -> CommandResult {
    match registry.resolve(line) {
        Ok((command, args)) => {
            // The blocked row of the two-bit table (ADR-0026 §1): a command whose
            // arguments are required refuses with its usage line rather than running
            // with none, which is the difference between a hint and a wrong result.
            if command.takes_args() && command.args_required() && args.trim().is_empty() {
                return CommandResult::Error(format!("usage: {}", command.usage()));
            }
            command.execute(ctx, args).await
        }
        Err(LookupError::Unknown { name, did_you_mean }) => {
            CommandResult::Error(unknown_message(&name, &did_you_mean))
        }
        // The caller decides what is worth dispatching; a line that is not an
        // invocation should never have reached here.
        Err(LookupError::NotAnInvocation) => CommandResult::Error(format!("not a command: {line}")),
    }
}

/// `unknown command: /foo — did you mean /fmt, /find?`
fn unknown_message(name: &str, did_you_mean: &[String]) -> String {
    if did_you_mean.is_empty() {
        return format!("unknown command: /{name}");
    }
    let names: Vec<String> = did_you_mean.iter().map(|n| format!("/{n}")).collect();
    format!(
        "unknown command: /{name} — did you mean {}?",
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::command::{SlashCommand, UiAction};
    use crate::commands::registry::CommandSource;
    use std::sync::Arc;

    struct Echo {
        required: bool,
    }

    #[async_trait::async_trait]
    impl SlashCommand for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "echo the arguments"
        }
        fn usage(&self) -> &'static str {
            "/echo <text>"
        }
        fn takes_args(&self) -> bool {
            true
        }
        fn args_required(&self) -> bool {
            self.required
        }
        async fn execute(&self, _c: &CommandCtx<'_>, args: &str) -> CommandResult {
            CommandResult::Message(args.to_string())
        }
    }

    fn registry(required: bool) -> CommandRegistry {
        let mut r = CommandRegistry::new();
        r.register(Arc::new(Echo { required }), CommandSource::Builtin);
        crate::commands::register_builtins(&mut r);
        r
    }

    async fn run(r: &CommandRegistry, line: &str) -> CommandResult {
        execute(r, &CommandCtx::default(), line).await
    }

    #[tokio::test]
    async fn a_known_command_runs_with_its_arguments() {
        let r = registry(false);
        assert_eq!(
            run(&r, "/echo hello world").await,
            CommandResult::Message("hello world".into())
        );
        assert_eq!(
            run(&r, "/quit").await,
            CommandResult::Action(UiAction::Quit)
        );
    }

    /// A required argument that is missing produces the usage line, not a run.
    #[tokio::test]
    async fn a_missing_required_argument_is_refused_with_the_usage_line() {
        let r = registry(true);
        assert_eq!(
            run(&r, "/echo").await,
            CommandResult::Error("usage: /echo <text>".into())
        );
        assert_eq!(run(&r, "/echo x").await, CommandResult::Message("x".into()));
        // Optional arguments still run with none.
        assert_eq!(
            run(&registry(false), "/echo").await,
            CommandResult::Message(String::new())
        );
    }

    #[tokio::test]
    async fn an_unknown_name_names_the_near_misses() {
        let r = registry(false);
        let CommandResult::Error(msg) = run(&r, "/ech").await else {
            panic!("expected an error");
        };
        assert!(msg.contains("unknown command: /ech"), "{msg}");
        assert!(msg.contains("/echo"), "suggests the near miss: {msg}");
    }

    #[tokio::test]
    async fn an_unknown_name_with_nothing_close_says_only_that() {
        let r = registry(false);
        let CommandResult::Error(msg) = run(&r, "/zzzz").await else {
            panic!("expected an error");
        };
        assert_eq!(msg, "unknown command: /zzzz");
    }
}
