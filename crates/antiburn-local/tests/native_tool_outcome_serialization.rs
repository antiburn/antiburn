use antiburn_local::analysis::{
    NativeToolOutcome, SourceFormat, ToolCategory, ToolResultChange, ToolResultCompleteness,
    ToolResultObservation, ToolResultPosition, ToolResultStatus, WorkState,
};
use serde_json::json;

fn observation(
    outcome: Option<NativeToolOutcome>,
    provider_executed: Option<bool>,
) -> ToolResultObservation {
    ToolResultObservation {
        facts: None,
        source_format: SourceFormat::OpenCodeSqliteV2,
        session_id: "ses_serialization".into(),
        record_id: Some("msg_serialization".into()),
        call_id: Some("call_serialization".into()),
        result_id: Some("evt_serialization".into()),
        position: ToolResultPosition::OpenCodeV2Event {
            sequence: 7,
            item_index: 0,
        },
        revision: 0,
        change: ToolResultChange::Update,
        tool_name: Some("shell".into()),
        tool_category: Some(ToolCategory::Bash),
        arguments: None,
        content: Some("Exited with code 7".into()),
        status: ToolResultStatus::Success,
        finality: true,
        completeness: ToolResultCompleteness::Incomplete,
        context: None,
        outcome,
        provider_executed,
    }
}

fn sparse_shell() -> NativeToolOutcome {
    NativeToolOutcome::Shell {
        state: None,
        exit: Some(7),
        signal: None,
        timed_out: None,
        shell_id: None,
    }
}

fn assert_postcard_roundtrip(expected: ToolResultObservation) {
    let encoded = postcard::to_stdvec(&expected).unwrap();
    let actual = postcard::from_bytes::<ToolResultObservation>(&encoded).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.status, ToolResultStatus::Success);
    assert!(actual.finality);
}

#[test]
fn postcard_roundtrip_retains_both_absent_outcome_fields() {
    assert_postcard_roundtrip(observation(None, None));
}

#[test]
fn postcard_roundtrip_retains_sparse_shell_outcome_and_local_execution_flag() {
    assert_postcard_roundtrip(observation(Some(sparse_shell()), Some(false)));
}

#[test]
fn postcard_roundtrip_retains_code_mode_and_subagent_outcome_variants() {
    for outcome in [
        NativeToolOutcome::CodeMode { error: None },
        NativeToolOutcome::CodeMode { error: Some(true) },
        NativeToolOutcome::Subagent {
            session_id: "ses_running".into(),
            state: WorkState::Running,
        },
        NativeToolOutcome::Subagent {
            session_id: "ses_completed".into(),
            state: WorkState::Completed,
        },
    ] {
        assert_postcard_roundtrip(observation(Some(outcome), Some(false)));
    }
}

#[test]
fn old_json_without_outcome_fields_keeps_none_defaults_and_recorded_status() {
    let expected = observation(None, None);
    let mut legacy = serde_json::to_value(&expected).unwrap();
    let fields = legacy.as_object_mut().unwrap();
    assert!(fields.remove("outcome").unwrap().is_null());
    assert!(fields.remove("provider_executed").unwrap().is_null());
    let actual: ToolResultObservation = serde_json::from_value(legacy).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.status, ToolResultStatus::Success);
}

#[test]
fn json_keeps_the_kind_tag_and_accepts_missing_optional_shell_facts() {
    let expected = observation(Some(sparse_shell()), Some(false));
    let mut encoded = serde_json::to_value(&expected).unwrap();
    assert_eq!(
        encoded["outcome"],
        json!({
            "kind":"shell", "state":null, "exit":7, "signal":null,
            "timed_out":null, "shell_id":null,
        })
    );
    encoded["outcome"] = json!({"kind":"shell", "exit":7});
    let actual: ToolResultObservation = serde_json::from_value(encoded).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.status, ToolResultStatus::Success);
}
