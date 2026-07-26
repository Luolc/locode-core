//! locode-exec — the minimal headless CLI (Task 14, ADR-0009), as a library.
//!
//! The crate ships both a binary (the stock CLI) and this lib target so
//! downstream binary crates can reuse the *entire* CLI — flags, session
//! assembly, stdout discipline, exit codes — while injecting custom providers
//! (ADR-0015):
//!
//! ```no_run
//! use locode_exec::{ProviderRegistry, main_with};
//!
//! fn main() -> std::process::ExitCode {
//!     main_with(ProviderRegistry::builtin() /* .register("my-wire", …) */)
//! }
//! ```
//!
//! Stdout discipline: stdout carries exactly one machine-readable artifact
//! (the JSON report, the final message, or the JSONL event stream — by
//! `--output-format`), enforced structurally: the workspace denies
//! `print_stdout`/`print_stderr`, and the ONLY allows live in `output.rs`
//! (Codex-exec's pattern). All diagnostics go to stderr; exit codes map the
//! run's terminal status (0 = structured, 1 = fatal, 2 = usage via clap).

use std::process::ExitCode;

use clap::Parser;

pub mod cli;
mod logging;
mod output;
pub mod run;
#[cfg(unix)]
mod signal;

pub use cli::{Cli, Harness, OutputFormat};
pub use run::canonicalize_add_dirs;
// The permission-posture notices, shared verbatim with the interactive UI so the
// two surfaces cannot drift (ADR-0008 amendment 2026-07-24).
pub use locode_core::ProviderRegistry;
pub use output::{RESTRICTED_MODE_NOTICE, UNRESTRICTED_MODE_NOTICE};

/// Parse the CLI, install logging, and drive one session to its exit code —
/// the whole binary, minus `fn main`. Custom providers come in through
/// `registry`; the stock binary passes [`ProviderRegistry::builtin`].
#[must_use]
// By-value on purpose: the downstream `fn main` is a single expression
// (`main_with(builtin().register(…))`) with no borrow to scope.
#[allow(clippy::needless_pass_by_value)]
pub fn main_with(registry: ProviderRegistry) -> ExitCode {
    let cli = cli::Cli::parse(); // clap usage errors exit 2 on their own
    run_headless(cli, registry)
}

/// Drive one headless run from an already-parsed [`Cli`] — installs logging,
/// builds the runtime, runs the engine, and emits per `--output-format`.
///
/// Split out of [`main_with`] so a unified front-end (the `locode` binary's
/// `-p` mode) can reuse the exact headless engine without re-parsing argv.
/// When `locode-exec` retires, this logic migrates into the front-end crate
/// (ADR-0019 amendment 2026-07-21).
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn run_headless(cli: cli::Cli, registry: ProviderRegistry) -> ExitCode {
    logging::init();

    // `ExitCode` return (not `process::exit`) so buffered stdout is flushed —
    // the "exactly one JSON document" contract survives (plan §3.2).
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            output::error_line(&format!("tokio runtime: {e}"));
            return ExitCode::from(1);
        }
    };
    match runtime.block_on(run::run(cli, &registry)) {
        Ok(code) => code,
        Err(pre_run) => {
            // Config/setup failed before a run existed: no report, exit 1
            // (Codex's pre-run pattern). Never a partial report on stdout.
            output::error_line(&pre_run.0);
            ExitCode::from(1)
        }
    }
}
