//! Integration: the engine task drives a real `Session` (grok pack + a
//! scripted mock wire) end-to-end over the typed channels — slice-2 preset
//! targets 4-5.

// This whole file is test code; unwrap/expect are the intended failure signal
// (the `allow-*-in-tests` clippy config only covers `#[test]` fns, not the
// free helper fns here).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use locode_core::{
    BuiltProvider, Completion, ContentBlock, Event, MockProvider, ProviderRegistry, Role, Status,
    StopReason, Usage,
};
use locode_tui::cli::Cli;
use locode_tui::engine::{self, EngineMsg, UiCommand};
use serde_json::json;

fn cli(dir: &tempfile::TempDir, api_schema: &str) -> Cli {
    Cli {
        prompt: None,
        print: false,
        harness: locode_exec::Harness::Grok,
        api_schema: api_schema.into(),
        cwd: Some(dir.path().to_path_buf()),
        output_format: locode_exec::OutputFormat::Json,
        max_turns: None,
        dangerously_skip_permissions: false,
        strip_identity: false,
    }
}

fn tool_turn(id: &str, name: &str, input: serde_json::Value) -> Completion {
    Completion {
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
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

/// A registry whose `mock` wire replays a fixed script. The factory clones
/// the script each call so `/new` can rebuild the session (a fresh mock with
/// the full script again). Env-free (the builtin mock's `LOCODE_MOCK_SCRIPT`
/// seam stays untouched here).
fn scripted_registry(turns: Vec<Completion>) -> ProviderRegistry {
    ProviderRegistry::new().register("mock", move |_init| {
        Ok(BuiltProvider {
            provider: Arc::new(MockProvider::new(turns.clone())),
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

/// Drive one submit to its report, tracking which roles streamed.
async fn run_once(
    tx: &tokio::sync::mpsc::UnboundedSender<UiCommand>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<EngineMsg>,
    prompt: &str,
) -> (Box<locode_core::Report>, bool, bool) {
    tx.send(UiCommand::Submit(prompt.into())).unwrap();
    assert!(matches!(recv(rx).await, EngineMsg::RunStarted { .. }));
    let (mut saw_user, mut saw_assistant) = (false, false);
    loop {
        match recv(rx).await {
            EngineMsg::Event(event) => {
                if let Event::Message { message } = *event {
                    match message.role {
                        Role::User => saw_user = true,
                        Role::Assistant => saw_assistant = true,
                        _ => {}
                    }
                }
            }
            EngineMsg::RunFinished(report) => return (report, saw_user, saw_assistant),
            other => panic!("unexpected mid-run message: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_runs_to_completion_and_the_session_continues() {
    let dir = tempfile::tempdir().unwrap();
    let registry = scripted_registry(vec![text_turn("first answer"), text_turn("second answer")]);
    let (tx, mut rx) = engine::spawn(cli(&dir, "mock"), registry);

    let EngineMsg::Ready { model, cwd } = recv(&mut rx).await else {
        panic!("first message must be Ready");
    };
    assert_eq!(model, "mock-scripted");
    assert!(!cwd.is_empty(), "cwd is reported for the status line");

    let (report, saw_user, saw_assistant) = run_once(&tx, &mut rx, "say hi").await;
    assert!(saw_user && saw_assistant, "both turns streamed");
    assert_eq!(report.status, Status::Completed);
    assert_eq!(report.final_message.as_deref(), Some("first answer"));

    // Continuity (ADR-0016): the second submit continues the SAME session —
    // the scripted mock's second turn answers it; per-run turns == 1.
    let (report2, _, _) = run_once(&tx, &mut rx, "and again").await;
    assert_eq!(report2.status, Status::Completed);
    assert_eq!(report2.final_message.as_deref(), Some("second answer"));
    assert_eq!(report2.turns, 1, "per-run counters (ADR-0016)");
}

#[tokio::test(flavor = "multi_thread")]
async fn new_session_resets_and_a_fresh_run_starts_clean() {
    let dir = tempfile::tempdir().unwrap();
    let registry = scripted_registry(vec![text_turn("one"), text_turn("two")]);
    let (tx, mut rx) = engine::spawn(cli(&dir, "mock"), registry);
    assert!(matches!(recv(&mut rx).await, EngineMsg::Ready { .. }));

    let (report, _, _) = run_once(&tx, &mut rx, "hi").await;
    assert_eq!(report.final_message.as_deref(), Some("one"));

    // /new → SessionReset then a fresh Ready.
    tx.send(UiCommand::NewSession).unwrap();
    assert!(matches!(recv(&mut rx).await, EngineMsg::SessionReset));
    assert!(matches!(recv(&mut rx).await, EngineMsg::Ready { .. }));

    // The next run starts a FRESH session (the rebuilt mock replays its script
    // from the top, so turn 1 answers "one" again) — proving the reset.
    let (report2, _, _) = run_once(&tx, &mut rx, "again").await;
    assert_eq!(report2.final_message.as_deref(), Some("one"));
    assert_eq!(report2.turns, 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn esc_cancels_a_running_turn_via_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    // Turn 1 asks the grok terminal tool to sleep; the run parks in dispatch
    // until we fire the cancel handle (real host cooperative cancel).
    let registry = scripted_registry(vec![tool_turn(
        "c1",
        "run_terminal_cmd",
        json!({ "command": "sleep 30", "description": "hold the run open" }),
    )]);
    let (tx, mut rx) = engine::spawn(cli(&dir, "mock"), registry);

    assert!(matches!(recv(&mut rx).await, EngineMsg::Ready { .. }));
    tx.send(UiCommand::Submit("go".into())).unwrap();

    // Capture the run's cancel handle from RunStarted, then fire it after the
    // dispatch has had a beat to spawn the sleep.
    let cancel = match recv(&mut rx).await {
        EngineMsg::RunStarted { cancel } => cancel,
        other => panic!("expected RunStarted, got {other:?}"),
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    cancel.cancel();

    // The run settles with a cancelled report (not a fault) within seconds —
    // far short of the 30s sleep.
    let report = loop {
        if let EngineMsg::RunFinished(report) = recv(&mut rx).await {
            break report;
        }
    };
    assert_eq!(report.status, Status::Cancelled);
    assert_eq!(report.error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_schema_fails_build_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let (_tx, mut rx) = engine::spawn(cli(&dir, "no-such-wire"), ProviderRegistry::builtin());
    let EngineMsg::BuildFailed(message) = recv(&mut rx).await else {
        panic!("must fail the build");
    };
    assert!(message.contains("no-such-wire"), "{message}");
}
