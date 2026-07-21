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
        .env_remove("LOCODE_MOCK_SCRIPT")
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
fn mock_script_env_overrides_the_default_turn() {
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
        .env("LOCODE_MOCK_SCRIPT", r#"[{"text": "scripted answer"}]"#)
        .assert()
        .success()
        .stdout("scripted answer\n");
}

#[test]
fn malformed_mock_script_fails_pre_run() {
    let dir = tempdir();
    exec()
        .args(["say hi", "--api-schema", "mock", "--cwd"])
        .arg(dir.path())
        .env("LOCODE_MOCK_SCRIPT", "not json")
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains("LOCODE_MOCK_SCRIPT"));
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

// ==================== SIGTERM (ADR-0018 / Task 21 → Task 24) ====================
//
// These drive the real binary with an env-scripted mock: the model "asks" for
// `run_terminal_cmd {command: "sleep 30"}`, which holds the run open until the
// test SIGTERMs the process — exercising signal → cancel handle → cooperative
// tool cancel → synthetic pairing → cancelled report, end to end.
#[cfg(unix)]
mod sigterm {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    /// The script that holds a run open: one slow shell turn, then a text turn
    /// the run never reaches when cancelled.
    const SLOW_SCRIPT: &str = r#"[
        {"tool": "run_terminal_cmd",
         "input": {"command": "sleep 30",
                   "description": "hold the run open for the SIGTERM test"}},
        {"text": "never reached"}
    ]"#;

    fn spawn_exec(dir: &tempfile::TempDir, extra_args: &[&str], script: Option<&str>) -> Child {
        let mut cmd = StdCommand::new(assert_cmd::cargo::cargo_bin("locode-exec"));
        cmd.args(["--api-schema", "mock", "--cwd"])
            .arg(dir.path())
            .args(extra_args)
            .env_remove("LOCODE_API_KEY")
            .env_remove("LOCODE_BASE_URL")
            .env_remove("LOCODE_MODEL")
            .env_remove("LOCODE_API_SCHEMA")
            .env_remove("LOCODE_MOCK_SCRIPT")
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(script) = script {
            cmd.env("LOCODE_MOCK_SCRIPT", script);
        }
        cmd.spawn()
            .unwrap_or_else(|e| panic!("spawn locode-exec: {e}"))
    }

    fn sigterm(child: &Child) {
        let status = StdCommand::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap_or_else(|e| panic!("run kill: {e}"));
        assert!(status.success(), "kill -TERM failed");
    }

    /// Wait for exit with a hard cap so a regression can't hang the suite.
    fn wait_capped(child: &mut Child, cap: Duration) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap_or_else(|e| panic!("try_wait: {e}")) {
                return status;
            }
            assert!(
                start.elapsed() < cap,
                "locode-exec did not exit within {cap:?} after SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn sigterm_mid_run_stream_json_ends_in_a_cancelled_result() {
        let dir = tempdir();
        let mut child = spawn_exec(
            &dir,
            &["say hi", "--output-format", "stream-json"],
            Some(SLOW_SCRIPT),
        );

        // Read the live stream until the assistant's tool_use turn appears —
        // dispatch (and the 30s sleep) starts right after it.
        let stdout = child.stdout.take().expect("stdout piped");
        let mut reader = BufReader::new(stdout);
        let mut lines: Vec<String> = Vec::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("read stream line");
            assert!(n > 0, "stream ended before the tool_use turn: {lines:?}");
            let event: Event = serde_json::from_str(line.trim()).expect("valid event line");
            let is_tool_turn = matches!(
                &event,
                Event::Message { message }
                    if message.content.iter().any(|b| matches!(
                        b, locode_core::ContentBlock::ToolUse { .. }))
            );
            lines.push(line.trim().to_string());
            if is_tool_turn {
                break;
            }
        }
        // Give dispatch a beat to spawn the sleep, then SIGTERM.
        std::thread::sleep(Duration::from_millis(300));
        sigterm(&child);

        // Drain the rest of the stream to EOF, then reap the exit status.
        let mut rest = String::new();
        std::io::Read::read_to_string(&mut reader, &mut rest).expect("drain stream");
        lines.extend(rest.lines().map(|l| l.trim().to_string()));
        let status = wait_capped(&mut child, Duration::from_secs(20));

        // Exit 0: cancelled is a structured terminal state.
        assert_eq!(status.code(), Some(0), "exit code; stream: {lines:?}");

        // The tail stays valid JSONL ending in a cancelled `result`.
        let events: Vec<Event> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad line {l:?}: {e}")))
            .collect();
        match events.last().expect("non-empty stream") {
            Event::Result { report } => {
                assert_eq!(report.status, Status::Cancelled);
                assert_eq!(report.error, None);
            }
            other => panic!("last event must be result, got {other:?}"),
        }

        // Transcript validity: every tool_use id is answered by a tool_result.
        let conversation = reconstruct_conversation(&events);
        let mut used: Vec<String> = Vec::new();
        let mut answered: Vec<String> = Vec::new();
        for message in &conversation.messages {
            for block in &message.content {
                match block {
                    locode_core::ContentBlock::ToolUse { id, .. } => used.push(id.clone()),
                    locode_core::ContentBlock::ToolResult { tool_use_id, .. } => {
                        answered.push(tool_use_id.clone());
                    }
                    _ => {}
                }
            }
        }
        assert!(!used.is_empty(), "the slow tool call is in the transcript");
        for id in &used {
            assert!(answered.contains(id), "unpaired tool_use {id}: {events:?}");
        }
    }

    #[test]
    fn sigterm_mid_run_json_still_emits_one_report() {
        let dir = tempdir();
        let mut child = spawn_exec(&dir, &["say hi"], Some(SLOW_SCRIPT));

        // json mode is silent until the end — no stream to key off; the 30s
        // sleep leaves a wide window for a fixed grace period.
        std::thread::sleep(Duration::from_secs(2));
        sigterm(&child);
        let status = wait_capped(&mut child, Duration::from_secs(20));

        let mut stdout = String::new();
        std::io::Read::read_to_string(child.stdout.as_mut().expect("stdout"), &mut stdout)
            .expect("read stdout");
        assert_eq!(status.code(), Some(0), "stdout: {stdout:?}");
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 1, "exactly one report line: {stdout:?}");
        let report: Report = serde_json::from_str(lines[0]).expect("parses as Report");
        assert_eq!(report.status, Status::Cancelled);
        assert_eq!(report.error, None);
        assert_eq!(report.schema_version, 1);
    }

    #[test]
    fn sigterm_before_the_run_exits_1_with_empty_stdout() {
        let dir = tempdir();
        // No positional prompt + stdin held open: the binary blocks pre-run
        // reading the prompt from stdin.
        let mut child = spawn_exec(&dir, &[], None);
        std::thread::sleep(Duration::from_millis(500));
        sigterm(&child);
        let status = wait_capped(&mut child, Duration::from_secs(20));

        let mut stdout = String::new();
        std::io::Read::read_to_string(child.stdout.as_mut().expect("stdout"), &mut stdout)
            .expect("read stdout");
        let mut stderr = String::new();
        std::io::Read::read_to_string(child.stderr.as_mut().expect("stderr"), &mut stderr)
            .expect("read stderr");
        assert_eq!(status.code(), Some(1), "stderr: {stderr:?}");
        assert_eq!(stdout, "", "nothing on stdout pre-run");
        assert!(
            stderr.contains("SIGTERM before the run started"),
            "stderr names the cause: {stderr:?}"
        );
    }
}
