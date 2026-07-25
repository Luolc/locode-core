//! Integration: the `TuiApprover` round-trips tool approvals through the
//! engine task, and `--yolo` bypasses them — slice-4 preset targets 4-5.

// Test code; unwrap/expect are the intended failure signal.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use locode_core::{
    BuiltProvider, Completion, ContentBlock, MockProvider, ProviderRegistry, StopReason, Usage,
};
use locode_tui::approval::ApprovalOutcome;
use locode_tui::cli::Cli;
use locode_tui::engine::{self, EngineMsg, UiCommand};
use serde_json::json;

fn cli(dir: &tempfile::TempDir, yolo: bool) -> Cli {
    Cli {
        prompt: None,
        print: false,
        harness: Some(locode_exec::Harness::Grok),
        api_schema: Some("mock".into()),
        model: None,
        // Hermetic: never write a rollout into the developer's real
        // `~/.locode` (these tests build real in-process sessions).
        no_session_persistence: true,
        settings: None,
        continue_session: false,
        resume: None,
        cwd: Some(dir.path().to_path_buf()),
        output_format: locode_exec::OutputFormat::Json,
        max_turns: None,
        restricted: !yolo,
        dangerously_skip_permissions: false,
        strip_identity: false,
        stream: false,
        // Isolate these tests from AGENTS.md discovery (they assert fixed trajectories).
        no_project_instructions: true,
    }
}

fn tool_turn(id: &str, command: &str) -> Completion {
    Completion {
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: "run_terminal_cmd".into(),
            input: json!({ "command": command, "description": "d" }),
        }],
        usage: Usage::default(),
        stop: StopReason::ToolUse,
    }
}

fn text_turn(text: &str) -> Completion {
    Completion {
        content: vec![ContentBlock::Text { text: text.into() }],
        usage: Usage::default(),
        stop: StopReason::EndTurn,
    }
}

fn scripted_registry(turns: Vec<Completion>) -> ProviderRegistry {
    let script = std::sync::Mutex::new(Some(turns));
    ProviderRegistry::new().register("mock", move |_init| {
        let turns = script.lock().unwrap().take().expect("once per test");
        Ok(BuiltProvider {
            provider: Arc::new(MockProvider::new(turns)),
            model: "mock-scripted".to_string(),
        })
    })
}

async fn recv(rx: &mut tokio::sync::mpsc::UnboundedReceiver<EngineMsg>) -> EngineMsg {
    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("engine message within 10s")
        .expect("engine channel open")
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_allow_lets_the_tool_run() {
    let dir = tempfile::tempdir().unwrap();
    // Tool turn (echo), then a text turn to finish.
    let registry = scripted_registry(vec![tool_turn("c1", "echo approved"), text_turn("done")]);
    let (tx, mut rx) = engine::spawn(cli(&dir, false), registry);
    assert!(matches!(recv(&mut rx).await, EngineMsg::Ready { .. }));
    tx.send(UiCommand::Submit("go".into())).unwrap();
    assert!(matches!(recv(&mut rx).await, EngineMsg::RunStarted { .. }));

    // The approver surfaces an ask; answer Allow via its oneshot.
    let report = loop {
        match recv(&mut rx).await {
            EngineMsg::Approval(ask) => {
                assert_eq!(ask.view.tool_name, "run_terminal_cmd");
                ask.respond.send(ApprovalOutcome::Allow).unwrap();
            }
            EngineMsg::RunFinished(report) => break report,
            _ => {}
        }
    };
    // The tool ran (allowed) and was recorded as ok (not denied).
    let call = &report.tool_calls[0];
    assert!(call.ok, "allowed tool ran: {call:?}");
    assert_eq!(call.denial_reason, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_deny_records_denial_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let registry = scripted_registry(vec![tool_turn("c1", "echo blocked"), text_turn("done")]);
    let (tx, mut rx) = engine::spawn(cli(&dir, false), registry);
    assert!(matches!(recv(&mut rx).await, EngineMsg::Ready { .. }));
    tx.send(UiCommand::Submit("go".into())).unwrap();
    assert!(matches!(recv(&mut rx).await, EngineMsg::RunStarted { .. }));

    let report = loop {
        match recv(&mut rx).await {
            EngineMsg::Approval(ask) => {
                ask.respond
                    .send(ApprovalOutcome::Deny {
                        reason: "nope".into(),
                    })
                    .unwrap();
            }
            EngineMsg::RunFinished(report) => break report,
            _ => {}
        }
    };
    // Deny is soft: recorded with the reason, run still completed.
    let call = &report.tool_calls[0];
    assert!(!call.ok);
    assert_eq!(call.denial_reason.as_deref(), Some("nope"));
}

#[tokio::test(flavor = "multi_thread")]
async fn yolo_runs_tools_without_surfacing_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let registry = scripted_registry(vec![tool_turn("c1", "echo yolo"), text_turn("done")]);
    let (tx, mut rx) = engine::spawn(cli(&dir, true), registry);
    assert!(matches!(recv(&mut rx).await, EngineMsg::Ready { .. }));
    tx.send(UiCommand::Submit("go".into())).unwrap();

    // No EngineMsg::Approval must ever appear.
    let report = loop {
        match recv(&mut rx).await {
            EngineMsg::Approval(_) => panic!("yolo must not surface approvals"),
            EngineMsg::RunFinished(report) => break report,
            _ => {}
        }
    };
    assert!(report.tool_calls[0].ok, "yolo auto-allowed the tool");
}
