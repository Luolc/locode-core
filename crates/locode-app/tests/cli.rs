//! End-to-end CLI tests for the shipped `locode` binary's `-p` headless mode.
//!
//! Migrated here from `locode-exec/tests/cli.rs` when the standalone `locode-exec`
//! binary was removed (2026-07-23, ADR-0019 amendment): `locode -p` is the one
//! shipped headless path and calls the same `locode_exec::run_headless`, so these
//! guarantees now live on the real product binary. Keyless (mock wire), CI-safe.

// Test code; unwrap/expect are the intended failure signal.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use locode_core::{Event, Report, Status, reconstruct_conversation};
use predicates::prelude::*;

/// One fresh `~/.locode` stand-in for the whole test binary — hermetic against
/// both the developer's real home AND the repo-local dev home (whose
/// settings.json contents must not steer assertions). The scaffold writes the
/// current defaults here on first use, which is exactly what the
/// default-behavior tests assert.
static TEST_HOME: std::sync::LazyLock<tempfile::TempDir> =
    std::sync::LazyLock::new(|| tempfile::tempdir().unwrap());

fn locode() -> Command {
    let mut cmd = Command::cargo_bin("locode").unwrap_or_else(|e| panic!("binary builds: {e}"));
    // Hermetic: never inherit a real key/base-url/schema from the dev env.
    cmd.env_remove("LOCODE_API_KEY")
        .env_remove("LOCODE_BASE_URL")
        .env_remove("LOCODE_MODEL")
        .env_remove("LOCODE_API_SCHEMA")
        .env_remove("LOCODE_MOCK_SCRIPT")
        .env_remove("RUST_LOG")
        .env("LOCODE_HOME", TEST_HOME.path());
    cmd
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"))
}

#[test]
fn mock_json_is_exactly_one_parseable_report() {
    let dir = tempdir();
    let assert = locode()
        .args(["-p", "say hi", "--api-schema", "mock", "--cwd"])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stdout line: {stdout:?}");
    let report: Report = serde_json::from_str(lines[0]).expect("parses as Report");
    assert_eq!(report.status, Status::Completed);
    assert_eq!(report.harness, "claude");
    assert_eq!(report.api_schema, "mock");
    assert_eq!(report.final_message.as_deref(), Some("Mock run complete."));
    assert_eq!(report.schema_version, 1);
}

#[test]
fn text_mode_prints_final_message_only() {
    let dir = tempdir();
    locode()
        .args([
            "-p",
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
    let assert = locode()
        .args([
            "-p",
            "say hi",
            "--harness",
            "grok",
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
fn unknown_schema_fails_before_running() {
    let dir = tempdir();
    locode()
        .args(["-p", "say hi", "--api-schema", "bogus", "--cwd"])
        .arg(dir.path())
        .assert()
        .failure()
        .stdout("");
}

#[test]
fn project_instructions_injected_from_agents_md() {
    // A repo with an AGENTS.md at the root: the loader (default on) discovers it and the
    // engine injects it as a User <system-reminder> in the trace (ADR-0023, Task 30).
    let dir = tempdir();
    std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
    std::fs::write(dir.path().join("AGENTS.md"), "Always be brief.").expect("write AGENTS.md");

    let assert = locode()
        .args([
            "-p",
            "say hi",
            "--harness",
            "grok",
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
    assert!(
        stdout.contains("system-reminder"),
        "injected reminder present: {stdout}"
    );
    assert!(
        stdout.contains("Always be brief."),
        "AGENTS.md content present: {stdout}"
    );
}

#[test]
fn project_instructions_nested_repo_root_to_cwd_order() {
    // Full stack: a git repo with an AGENTS.md at the root AND a subdir, run from the
    // subdir. The injected reminder carries both, labeled, root→cwd (deepest last).
    let dir = tempdir();
    std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
    std::fs::write(dir.path().join("AGENTS.md"), "ROOT-RULE").expect("root AGENTS.md");
    let sub = dir.path().join("crates").join("app");
    std::fs::create_dir_all(&sub).expect("mkdir subdir");
    std::fs::write(sub.join("AGENTS.md"), "LEAF-RULE").expect("leaf AGENTS.md");

    let assert = locode()
        .args([
            "-p",
            "say hi",
            "--harness",
            "grok",
            "--api-schema",
            "mock",
            "--output-format",
            "stream-json",
            "--cwd",
        ])
        .arg(&sub)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");

    // Locate the injected reminder line and assert both files, labeled, in root→cwd order.
    let reminder = stdout
        .lines()
        .find(|l| l.contains("system-reminder"))
        .unwrap_or_else(|| panic!("a reminder event: {stdout}"));
    assert!(reminder.contains("## From:"), "labeled by source path");
    let root_at = reminder.find("ROOT-RULE").expect("root rule present");
    let leaf_at = reminder.find("LEAF-RULE").expect("leaf rule present");
    assert!(root_at < leaf_at, "root before leaf (deepest wins, last)");
}

#[test]
fn no_project_instructions_flag_suppresses_injection() {
    let dir = tempdir();
    std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
    std::fs::write(dir.path().join("AGENTS.md"), "Always be brief.").expect("write AGENTS.md");

    let assert = locode()
        .args([
            "-p",
            "say hi",
            "--harness",
            "grok",
            "--api-schema",
            "mock",
            "--output-format",
            "stream-json",
            "--no-project-instructions",
            "--cwd",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(
        !stdout.contains("system-reminder"),
        "no reminder when disabled: {stdout}"
    );
    assert!(!stdout.contains("Always be brief."), "no AGENTS.md content");
}

#[test]
fn logs_go_to_stderr_never_stdout() {
    let dir = tempdir();
    let assert = locode()
        .args(["-p", "say hi", "--api-schema", "mock", "--cwd"])
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
    locode()
        .args(["-p", "say hi", "--cwd"]) // default --api-schema anthropic
        .arg(dir.path())
        .assert()
        .code(1)
        .stdout("") // no partial report
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn unknown_harness_is_a_clean_usage_error() {
    locode()
        .args(["-p", "say hi", "--harness", "bogus"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("grok"));
}

#[test]
fn empty_prompt_is_an_error() {
    let dir = tempdir();
    locode()
        .args(["-p", "--api-schema", "mock", "--cwd"])
        .arg(dir.path())
        .write_stdin("")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no prompt"));
}

#[test]
fn prompt_reads_from_stdin_when_dash() {
    let dir = tempdir();
    locode()
        .args([
            "-p",
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
    locode()
        .args([
            "-p",
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
    locode()
        .args(["-p", "say hi", "--api-schema", "mock", "--cwd"])
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
    let assert = locode()
        .args([
            "-p",
            "say hi",
            "--harness",
            "grok",
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
// These drive the real `locode` binary with an env-scripted mock: the model "asks"
// for `run_terminal_cmd {command: "sleep 30"}`, which holds the run open until the
// test SIGTERMs the process — exercising signal → cancel handle → cooperative tool
// cancel → synthetic pairing → cancelled report, end to end.
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

    fn spawn_locode(dir: &tempfile::TempDir, extra_args: &[&str], script: Option<&str>) -> Child {
        let mut cmd = StdCommand::new(assert_cmd::cargo::cargo_bin("locode"));
        cmd.args(["-p", "--harness", "grok", "--api-schema", "mock", "--cwd"])
            .arg(dir.path())
            .args(extra_args)
            .env_remove("LOCODE_API_KEY")
            .env_remove("LOCODE_BASE_URL")
            .env_remove("LOCODE_MODEL")
            .env_remove("LOCODE_API_SCHEMA")
            .env_remove("LOCODE_MOCK_SCRIPT")
            .env_remove("RUST_LOG")
            // Hermetic like `locode()`: never touch the developer's real
            // `~/.locode` (these spawn real runs that write a trace).
            .env("LOCODE_HOME", super::TEST_HOME.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(script) = script {
            cmd.env("LOCODE_MOCK_SCRIPT", script);
        }
        cmd.spawn().unwrap_or_else(|e| panic!("spawn locode: {e}"))
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
                "locode did not exit within {cap:?} after SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn sigterm_mid_run_stream_json_ends_in_a_cancelled_result() {
        let dir = tempdir();
        let mut child = spawn_locode(
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
        let mut child = spawn_locode(&dir, &["say hi"], Some(SLOW_SCRIPT));

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
        let mut child = spawn_locode(&dir, &[], None);
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

/// `~/.locode` settings + resume semantics (ADR-0024 §1.4/§2.5), end-to-end.
mod locode_home {
    use super::{locode, tempdir};

    /// The model the run actually used, from the `stream-json` init event.
    fn run_model(home: &std::path::Path, cwd: &std::path::Path, extra: &[&str]) -> String {
        let assert = locode()
            .env("LOCODE_HOME", home)
            .args(["-p", "hi", "--output-format", "stream-json", "--cwd"])
            .arg(cwd)
            .args(extra)
            .assert()
            .success();
        let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
        for line in stdout.lines() {
            let event: serde_json::Value = serde_json::from_str(line).expect("jsonl");
            if event["type"] == "init" {
                return event["model"].as_str().expect("model").to_string();
            }
        }
        panic!("no init event: {stdout}");
    }

    fn write_settings(home: &std::path::Path, model: &str) {
        std::fs::create_dir_all(home).expect("home");
        std::fs::write(
            home.join("settings.json"),
            format!(r#"{{"harness":"claude","api_schema":"mock","model":"{model}"}}"#),
        )
        .expect("settings");
    }

    #[test]
    fn first_run_scaffolds_settings_with_sorted_keys_and_current_defaults() {
        let home = tempdir();
        let cwd = tempdir();
        let path = home.path().join("settings.json");
        assert!(!path.exists(), "absent before the first run");

        locode()
            .env("LOCODE_HOME", home.path())
            .args(["-p", "hi", "--api-schema", "mock", "--cwd"])
            .arg(cwd.path())
            .assert()
            .success();

        let text = std::fs::read_to_string(&path).expect("scaffolded");
        // Deterministic key order: top-level keys (indent 2 in pretty JSON) are
        // emitted lexicographically, so the scaffold is byte-stable.
        let keys: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("  \""))
            .filter_map(|l| l.split('"').next())
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "top-level keys sorted: {keys:?}");
        assert!(
            keys.contains(&"model") && keys.contains(&"harness"),
            "{keys:?}"
        );
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["harness"], "claude");
        assert_eq!(value["api_schema"], "anthropic");
        assert_eq!(value["model"], "claude-sonnet-5");

        // A second run never rewrites it.
        std::fs::write(&path, r#"{"harness":"grok","api_schema":"mock"}"#).expect("edit");
        locode()
            .env("LOCODE_HOME", home.path())
            .args(["-p", "hi", "--api-schema", "mock", "--cwd"])
            .arg(cwd.path())
            .assert()
            .success();
        let kept = std::fs::read_to_string(&path).expect("still there");
        assert!(kept.contains("grok"), "user edits survive: {kept}");
    }

    #[test]
    fn resume_takes_the_model_from_flag_or_settings_never_the_header() {
        let home = tempdir();
        let cwd = tempdir();

        // Session starts under model-A.
        write_settings(home.path(), "model-A");
        assert_eq!(run_model(home.path(), cwd.path(), &[]), "model-A");

        // Settings change; --continue must use the NEW model, not the recorded one.
        write_settings(home.path(), "model-B");
        assert_eq!(
            run_model(home.path(), cwd.path(), &["-c"]),
            "model-B",
            "resume resolves the model like a fresh run"
        );

        // An explicit --model beats settings, resumed or not.
        assert_eq!(
            run_model(home.path(), cwd.path(), &["-c", "--model", "model-C"]),
            "model-C"
        );
        assert_eq!(
            run_model(home.path(), cwd.path(), &["--model", "model-C"]),
            "model-C"
        );
    }

    #[test]
    fn resume_keeps_the_recorded_harness_and_rejects_a_conflicting_flag() {
        let home = tempdir();
        let cwd = tempdir();
        write_settings(home.path(), "model-A"); // harness claude

        locode()
            .env("LOCODE_HOME", home.path())
            .args(["-p", "hi", "--api-schema", "mock", "--cwd"])
            .arg(cwd.path())
            .assert()
            .success();

        // Settings flip to grok — the resumed session still runs its recorded pack.
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"harness":"grok","api_schema":"mock","model":"model-A"}"#,
        )
        .expect("settings");
        let assert = locode()
            .env("LOCODE_HOME", home.path())
            .args(["-p", "hi", "-c", "--cwd"])
            .arg(cwd.path())
            .assert()
            .success();
        let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
        let report: serde_json::Value =
            serde_json::from_str(stdout.lines().next().expect("line")).expect("report");
        assert_eq!(report["harness"], "claude", "pack is header-bound");

        // And an explicit conflicting --harness is a clean pre-run error.
        locode()
            .env("LOCODE_HOME", home.path())
            .args(["-p", "hi", "-c", "--harness", "grok", "--cwd"])
            .arg(cwd.path())
            .assert()
            .failure()
            .stderr(predicates::str::contains("conflicts with the resumed"));
    }
}
