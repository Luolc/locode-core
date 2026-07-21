//! The unified `locode` binary's `-p` headless mode (Task 28): drive the
//! built `locode` binary with the keyless mock wire and assert the headless
//! stdout contract — the same guarantees `locode-exec` gives, now under `-p`.

// Test code; unwrap/expect are the intended failure signal.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;

fn locode() -> Command {
    let mut cmd = Command::cargo_bin("locode").unwrap_or_else(|e| panic!("binary builds: {e}"));
    cmd.env_remove("LOCODE_API_KEY")
        .env_remove("LOCODE_API_SCHEMA")
        .env_remove("LOCODE_MOCK_SCRIPT")
        .env_remove("RUST_LOG");
    cmd
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"))
}

#[test]
fn dash_p_json_is_one_report_line() {
    let dir = tempdir();
    let out = locode()
        .args(["-p", "say hi", "--api-schema", "mock", "--cwd"])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "one report line: {stdout:?}");
    // Parseable JSON with the completed status.
    let v: serde_json::Value = serde_json::from_str(lines[0]).expect("parses as JSON");
    assert_eq!(v["status"], "completed");
    assert_eq!(v["api_schema"], "mock");
}

#[test]
fn dash_p_text_prints_final_message_only() {
    let dir = tempdir();
    locode()
        .args([
            "--print", // long form
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
fn dash_p_unknown_schema_fails_before_running() {
    let dir = tempdir();
    locode()
        .args(["-p", "hi", "--api-schema", "no-such-wire", "--cwd"])
        .arg(dir.path())
        .assert()
        .code(1)
        .stdout("") // no partial report
        .stderr(predicate::str::contains("no-such-wire"));
}
