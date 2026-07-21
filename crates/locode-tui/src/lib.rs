//! locode-tui — terminal UI components and the runnable interactive app
//! (SPEC-TUI; ADR-0001 amendment 2026-07-21).
//!
//! One library crate carrying the whole app behind [`main_with`]; the product
//! binary (`locode-app`, command `locode`) is flag-free composition. The
//! architecture is the study-distilled shape (`docs/research/
//! tui-harness-study.md`): inline viewport + print-once transcript, a
//! dedicated input thread, one biased `select!` loop, and a sans-IO
//! `Msg → update → Cmd` reducer.

use std::process::ExitCode;

use clap::Parser;
use locode_core::ProviderRegistry;

pub mod app;
pub mod approval;
pub mod cli;
pub mod engine;
pub mod term;
pub mod ui;

mod event_loop;

/// Parse the CLI, set up the terminal, and run the app to its exit code —
/// the whole interactive binary, minus `fn main`. Custom providers come in
/// through `registry` (the ADR-0015 pattern); the stock binary passes
/// [`ProviderRegistry::builtin`].
#[must_use]
// By-value on purpose: the downstream `fn main` is a single expression
// (`main_with(builtin().register(…))`) with no borrow to scope.
#[allow(clippy::needless_pass_by_value)]
pub fn main_with(registry: ProviderRegistry) -> ExitCode {
    let cli = cli::Cli::parse(); // clap usage errors exit 2 on their own

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            term::error_line(&format!("tokio runtime: {e}"));
            return ExitCode::from(1);
        }
    };
    match runtime.block_on(event_loop::run(cli, &registry)) {
        Ok(code) => code,
        Err(e) => {
            // The terminal was restored by the guard/teardown before we get
            // here; report the failure on stderr like any CLI.
            term::error_line(&e.to_string());
            ExitCode::from(1)
        }
    }
}
