//! Golden test: freezes the report-envelope JSON shape (`schema_version: 1`, ADR-0009).
//! A structural change to the envelope must update `report_snapshot.json` deliberately.

use locode_protocol::{Report, Status, ToolCallRecord, Usage};
use serde_json::{Value, json};

fn sample_report() -> Report {
    Report {
        schema_version: 1,
        status: Status::Completed,
        harness: "grok".to_string(),
        api_schema: "anthropic".to_string(),
        final_message: Some("Done.".to_string()),
        structured_output: None,
        turns: 2,
        tool_calls: vec![ToolCallRecord {
            id: "call_1".to_string(),
            name: "run_terminal_command".to_string(),
            kind: "shell".to_string(),
            args: json!({ "command": "echo hi" }),
            ok: true,
            output: json!({ "exit_code": 0, "truncated": false }),
            denial_reason: None,
        }],
        usage: Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: Some(0),
            cache_creation_tokens: None,
            reasoning_tokens: None,
        },
        // The run's sum above; the final turn's occupancy here (ADR-0018: an added
        // optional field is non-breaking and does not bump `schema_version`).
        context_usage: Usage {
            input_tokens: 80,
            output_tokens: 12,
            cache_read_tokens: Some(0),
            cache_creation_tokens: None,
            reasoning_tokens: None,
        },
        stop_reason: Some("end_turn".into()),
        session_id: "sess-abc".to_string(),
        error: None,
    }
}

#[test]
fn report_matches_committed_snapshot() {
    let got = serde_json::to_value(sample_report()).expect("serialize report");
    let want: Value =
        serde_json::from_str(include_str!("report_snapshot.json")).expect("parse snapshot");
    assert_eq!(
        got, want,
        "report envelope shape drifted from the committed snapshot"
    );
}

#[test]
fn schema_version_is_frozen_at_1() {
    assert_eq!(sample_report().schema_version, 1);
}

/// A report written before `context_usage` existed still parses, reading as all-zero —
/// the additive-field guarantee ADR-0018 makes.
#[test]
fn an_envelope_without_context_usage_still_parses() {
    let mut value = serde_json::to_value(sample_report()).expect("serialize");
    value.as_object_mut().unwrap().remove("context_usage");
    let back: Report = serde_json::from_value(value).expect("older envelope parses");
    assert_eq!(back.context_usage, Usage::default());
    assert_eq!(back.usage.input_tokens, 100, "the run total is untouched");
}
