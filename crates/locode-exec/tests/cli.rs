//! End-to-end CLI tests (Task 14 acceptance, plan §6): drive the built binary
//! with the mock provider — keyless, CI-safe — and assert the stdout contract,
//! stream reconstruction, stderr discipline, and exit codes.

use assert_cmd::Command;
use locode_core::{Event, Report, Status, reconstruct_conversation};
use predicates::prelude::*;

fn exec() -> Command {
    let mut cmd =
        Command::cargo_bin("locode-exec").unwrap_or_else(|e| panic!("binary builds: {e}"));
    // Hermetic: never inherit a real key/base-url/schema from the dev env.
    cmd.env_remove("LOCODE_API_KEY")
        .env_remove("LOCODE_BASE_URL")
        .env_remove("LOCODE_MODEL")
        .env_remove("LOCODE_API_SCHEMA")
        .env_remove("RUST_LOG");
    cmd
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"))
}

#[test]
fn mock_json_is_exactly_one_parseable_report() {
    let dir = tempdir();
    let assert = exec()
        .args(["say hi", "--api-schema", "mock", "--cwd"])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stdout line: {stdout:?}");
    let report: Report = serde_json::from_str(lines[0]).expect("parses as Report");
    assert_eq!(report.status, Status::Completed);
    assert_eq!(report.harness, "grok");
    assert_eq!(report.api_schema, "mock");
    assert_eq!(report.final_message.as_deref(), Some("Mock run complete."));
    assert_eq!(report.schema_version, 1);
}

#[test]
fn text_mode_prints_final_message_only() {
    let dir = tempdir();
    exec()
        .args([
            "say hi",
            "--api-schema",
            "mock",
            "--output-format",
            "text",
            "--cwd",
        ])
        .arg(dir.path())
        .assert()
        .success()
        .stdout("Mock run complete.\n");
}

#[test]
fn stream_json_is_valid_jsonl_and_reconstructs() {
    let dir = tempdir();
    let assert = exec()
        .args([
            "say hi",
            "--api-schema",
            "mock",
            "--output-format",
            "stream-json",
            "--cwd",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let events: Vec<Event> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad event line {l:?}: {e}")))
        .collect();
    assert!(events.len() >= 3, "init + message(s) + result");

    // First event: init, carrying the full preamble + the grok tool specs.
    match &events[0] {
        Event::Init {
            preamble, tools, ..
        } => {
            assert!(!preamble.is_empty(), "init carries the preamble");
            assert_eq!(tools.len(), 5, "the grok pack's five tools");
        }
        other => panic!("first event must be init, got {other:?}"),
    }
    // Last event: result, with the same Report shape as json mode.
    match events.last().expect("non-empty") {
        Event::Result { report } => assert_eq!(report.status, Status::Completed),
        other => panic!("last event must be result, got {other:?}"),
    }
    // The stream is self-sufficient: the full conversation rebuilds from it.
    let conversation = reconstruct_conversation(&events);
    assert!(
        conversation.messages.len() >= 3,
        "system + user_info + user prompt + assistant: {}",
        conversation.messages.len()
    );
}

#[test]
fn logs_go_to_stderr_never_stdout() {
    let dir = tempdir();
    let assert = exec()
        .args(["say hi", "--api-schema", "mock", "--cwd"])
        .arg(dir.path())
        .env("RUST_LOG", "debug")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert_eq!(stdout.lines().count(), 1, "stdout stays a single JSON doc");
    serde_json::from_str::<Report>(stdout.trim()).expect("still parses");
}

#[test]
fn anthropic_without_key_fails_before_running() {
    let dir = tempdir();
    exec()
        .args(["say hi", "--cwd"]) // default --api-schema anthropic
        .arg(dir.path())
        .assert()
        .code(1)
        .stdout("") // no partial report
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn unknown_harness_is_a_clean_usage_error() {
    exec()
        .args(["say hi", "--harness", "bogus"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("grok"));
}

#[test]
fn empty_prompt_is_an_error() {
    let dir = tempdir();
    exec()
        .args(["--api-schema", "mock", "--cwd"])
        .arg(dir.path())
        .write_stdin("")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no prompt"));
}

#[test]
fn prompt_reads_from_stdin_when_dash() {
    let dir = tempdir();
    exec()
        .args([
            "-",
            "--api-schema",
            "mock",
            "--output-format",
            "text",
            "--cwd",
        ])
        .arg(dir.path())
        .write_stdin("from stdin\n")
        .assert()
        .success()
        .stdout("Mock run complete.\n");
}

#[test]
fn strip_identity_removes_grok_from_the_stream() {
    let dir = tempdir();
    let assert = exec()
        .args([
            "say hi",
            "--api-schema",
            "mock",
            "--output-format",
            "stream-json",
            "--strip-identity",
            "--cwd",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let init_line = stdout.lines().next().expect("init event");
    assert!(
        !init_line.contains("released by xAI"),
        "identity sentence stripped from the preamble"
    );
}
