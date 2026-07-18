//! locode-exec — the minimal headless binary (Task 14, ADR-0009).
//!
//! Stdout discipline: stdout carries exactly one machine-readable artifact
//! (the JSON report, the final message, or the JSONL event stream — by
//! `--output-format`), enforced structurally: the workspace denies
//! `print_stdout`/`print_stderr`, and the ONLY allows live in `output.rs`
//! (Codex-exec's pattern). All diagnostics go to stderr; exit codes map the
//! run's terminal status (0 = structured, 1 = fatal, 2 = usage via clap).

use std::process::ExitCode;

use clap::Parser;

mod cli;
mod logging;
mod output;
mod run;

fn main() -> ExitCode {
    let cli = cli::Cli::parse(); // clap usage errors exit 2 on their own
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
    match runtime.block_on(run::run(cli)) {
        Ok(code) => code,
        Err(pre_run) => {
            // Config/setup failed before a run existed: no report, exit 1
            // (Codex's pre-run pattern). Never a partial report on stdout.
            output::error_line(&pre_run.0);
            ExitCode::from(1)
        }
    }
}
