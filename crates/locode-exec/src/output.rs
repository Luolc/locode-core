//! The crate's ONLY stdout/stderr writers (ADR-0009 stdout discipline).
//!
//! Everything else in the crate is denied `print_stdout`/`print_stderr` by the
//! workspace lints; the narrow `#[allow]`s below are the audited exceptions —
//! exactly Codex-exec's pattern (named, narrow emitters; the crate-wide deny
//! stays intact).

use std::process::ExitCode;

use locode::Status;

/// Write one JSON value as one stdout line (the report, or one stream event).
///
/// A serialize failure (realistically unreachable — every field is plain data)
/// emits a `{"type":"error"}` object instead, so stdout still carries exactly
/// one machine-readable line (Codex's jsonl fallback). Write errors (EPIPE
/// from `| head`, a closed pipe) are deliberately ignored — panicking on a
/// consumer closing the pipe would be wrong for a CLI.
pub fn write_json_line(value: &impl serde::Serialize) {
    use std::io::Write;
    let line = serde_json::to_string(value).unwrap_or_else(|e| {
        format!(r#"{{"type":"error","message":"failed to serialize output: {e}"}}"#)
    });
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// Write the `text`-mode artifact (the final assistant message).
pub fn write_text(text: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{text}");
    let _ = stdout.flush();
}

/// Write a pre-run failure to stderr (`error: …`), Codex's pre-run pattern.
pub fn error_line(message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "error: {message}");
}

/// ADR-0009 exit-code mapping: 0 for any **structured** terminal state
/// (`completed`/`max_turns` — the run produced a valid report), 1 for fatal
/// (`model_error`/`error`); clap owns exit 2 for usage errors.
pub fn exit_code(status: Status) -> ExitCode {
    match status {
        Status::Completed | Status::MaxTurns => ExitCode::SUCCESS,
        Status::ModelError | Status::Error => ExitCode::from(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_map_all_four_statuses() {
        assert_eq!(exit_code(Status::Completed), ExitCode::SUCCESS);
        assert_eq!(exit_code(Status::MaxTurns), ExitCode::SUCCESS);
        assert_eq!(exit_code(Status::ModelError), ExitCode::from(1));
        assert_eq!(exit_code(Status::Error), ExitCode::from(1));
    }
}
