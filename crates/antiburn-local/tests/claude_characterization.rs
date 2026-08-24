use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use antiburn_local::analysis::{RawSource, SessionInput, analyze_sources_with, normalize_source};
use serde_json::{Value, json};

fn fixture(name: &str) -> &'static str {
    match name {
        "records_all_kinds" => {
            include_str!("fixtures/claude_characterization/records_all_kinds.jsonl")
        }
        "timestamps_repeated_and_out_of_order" => include_str!(
            "fixtures/claude_characterization/timestamps_repeated_and_out_of_order.jsonl"
        ),
        "malformed_between_valid" => {
            include_str!("fixtures/claude_characterization/malformed_between_valid.jsonl")
        }
        "incomplete_final_record" => {
            include_str!("fixtures/claude_characterization/incomplete_final_record.jsonl")
        }
        "unrecognized_type" => {
            include_str!("fixtures/claude_characterization/unrecognized_type.jsonl")
        }
        "parent_with_task_spawn" => {
            include_str!("fixtures/claude_characterization/parent_with_task_spawn.jsonl")
        }
        "subagent_child" => include_str!("fixtures/claude_characterization/subagent_child.jsonl"),
        "multi_model_session" => {
            include_str!("fixtures/claude_characterization/multi_model_session.jsonl")
        }
        "compaction_with_cache_rehydration" => {
            include_str!("fixtures/claude_characterization/compaction_with_cache_rehydration.jsonl")
        }
        "inferred_cache_rehydration" => {
            include_str!("fixtures/claude_characterization/inferred_cache_rehydration.jsonl")
        }
        _ => panic!("unknown characterization fixture: {name}"),
    }
}

fn input(name: &str) -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: name.to_string(),
        source: RawSource::Jsonl(fixture(name).to_string()),
    }
}

fn actual_document(name: &str) -> Value {
    let input = input(name);
    let normalized_session = normalize_source(&input).expect("fixture must normalize");
    let sessions = analyze_sources_with(vec![input], true).sessions;
    json!({
        "normalizedSession": normalized_session,
        "sessions": sessions,
    })
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude_characterization/goldens"
    ))
    .join(format!("{name}.json"))
}

fn check_golden(name: &str, actual: Value) {
    let path = golden_path(name);
    let rendered = serde_json::to_string_pretty(&actual).expect("actual value must serialize");
    if std::env::var("UPDATE_GOLDENS").as_deref() == Ok("1") {
        fs::create_dir_all(path.parent().expect("golden path must have a parent"))
            .expect("golden directory must be created");
        fs::write(&path, format!("{rendered}\n")).expect("golden must be written");
    }

    let expected_text = fs::read_to_string(&path).expect("golden must exist");
    let expected: Value = serde_json::from_str(&expected_text).expect("golden must be valid JSON");
    let actual: Value =
        serde_json::from_str(&rendered).expect("rendered actual must be valid JSON");
    let expected_object = expected.as_object().expect("golden must be an object");
    let actual_object = actual
        .as_object()
        .expect("actual document must be an object");
    let expected_keys: BTreeSet<_> = expected_object.keys().collect();
    let actual_keys: BTreeSet<_> = actual_object.keys().collect();
    assert_eq!(actual_keys, expected_keys, "golden top-level keys differ");
    for key in expected_keys {
        assert_eq!(
            actual_object.get(key),
            expected_object.get(key),
            "golden field {key} differs"
        );
    }
}

fn check_fixture_golden(name: &str) {
    check_golden(name, actual_document(name));
}

fn many_records_jsonl(record_count: usize) -> String {
    let mut source = String::with_capacity(record_count * 260);
    for index in 0..record_count {
        source.push_str(&format!(
            "{{\"type\":\"assistant\",\"timestamp\":{},\"message\":{{\"id\":\"msg-many-{index}\",\"role\":\"assistant\",\"model\":\"claude-3-5-haiku-20241022\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":3,\"cache_creation_input_tokens\":5}},\"content\":[{{\"type\":\"text\",\"text\":\"Synthetic record {index}.\"}}]}}}}\n",
            1_760_000_000 + index
        ));
    }
    source
}

fn oversized_line_jsonl(payload_bytes: usize) -> String {
    let mut source = String::with_capacity(payload_bytes + 500);
    source.push_str(
        "{\"type\":\"user\",\"timestamp\":1761000000,\"message\":{\"role\":\"user\",\"content\":\"Synthetic first neighbour.\"}}\n",
    );
    source.push_str("{\"type\":\"oversized_probe\",\"payload\":\"");
    source.push_str(&"x".repeat(payload_bytes));
    source.push_str("\"}\n");
    source.push_str(
        "{\"type\":\"assistant\",\"timestamp\":1761000001,\"message\":{\"id\":\"msg-oversized-1\",\"role\":\"assistant\",\"model\":\"claude-3-5-haiku-20241022\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2},\"content\":[{\"type\":\"text\",\"text\":\"Synthetic second neighbour.\"}]}}\n",
    );
    source
}

fn file_input(name: &str, source: &str, directory: &tempfile::TempDir) -> SessionInput {
    let path = directory.path().join(format!("{name}.jsonl"));
    fs::write(&path, source).expect("generated source must be written");
    SessionInput {
        agent: "claude".to_string(),
        session_id: name.to_string(),
        source: RawSource::File(path),
    }
}

#[test]
fn golden_records_all_kinds() {
    check_fixture_golden("records_all_kinds");
}

#[test]
fn golden_timestamps_repeated_and_out_of_order() {
    check_fixture_golden("timestamps_repeated_and_out_of_order");
}

#[test]
fn golden_malformed_between_valid() {
    check_fixture_golden("malformed_between_valid");
}

#[test]
fn golden_incomplete_final_record() {
    check_fixture_golden("incomplete_final_record");
}

#[test]
fn golden_unrecognized_type() {
    check_fixture_golden("unrecognized_type");
}

#[test]
fn golden_parent_with_task_spawn() {
    check_fixture_golden("parent_with_task_spawn");
}

#[test]
fn golden_subagent_child() {
    check_fixture_golden("subagent_child");
}

#[test]
fn golden_multi_model_session() {
    check_fixture_golden("multi_model_session");
}

#[test]
fn golden_compaction_with_cache_rehydration() {
    check_fixture_golden("compaction_with_cache_rehydration");
}

#[test]
fn golden_inferred_cache_rehydration() {
    check_fixture_golden("inferred_cache_rehydration");
}

#[test]
fn records_all_kinds_reports_initial_context() {
    let sessions = analyze_sources_with(vec![input("records_all_kinds")], true).sessions;
    let context = sessions[0]
        .initial_context
        .as_ref()
        .expect("initial context must be available");
    assert_eq!(context.total_tokens, Some(5_000));
    assert!(context.sources.iter().any(|source| {
        source.source == "skill_instructions"
            && source.source_name.as_deref() == Some("orbit-tracker")
    }));
}

#[test]
fn records_all_kinds_grafts_skill_description() {
    let sessions = analyze_sources_with(vec![input("records_all_kinds")], true).sessions;
    let skill = &sessions[0].skill_uses[0];
    assert_eq!(skill.name, "orbit-tracker");
    assert_eq!(
        skill.description.as_deref(),
        Some("Computes fictional satellite passes for the demo app.")
    );
}

#[test]
fn golden_parent_and_subagent_are_independent() {
    let sessions = analyze_sources_with(
        vec![input("parent_with_task_spawn"), input("subagent_child")],
        true,
    )
    .sessions;
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].session_id, "parent_with_task_spawn");
    assert_eq!(sessions[0].event_count, 3);
    assert_eq!(sessions[1].session_id, "subagent_child");
    assert_eq!(sessions[1].event_count, 3);
}

#[test]
fn malformed_line_does_not_discard_neighbours() {
    let normalized =
        normalize_source(&input("malformed_between_valid")).expect("fixture must normalize");
    assert_eq!(normalized.events.len(), 2);
    assert_eq!(normalized.events[0].ts_ms, Some(1_772_352_000_000));
    assert_eq!(normalized.events[1].ts_ms, Some(1_772_352_002_000));
}

#[test]
fn incomplete_final_record_is_not_committed() {
    let incomplete = fixture("incomplete_final_record").trim_end_matches('\n');
    let normalized =
        normalize_source(&input("incomplete_final_record")).expect("fixture must normalize");
    assert_eq!(normalized.events.len(), 2);

    let completed = format!("{incomplete}}}}}\n");
    let completed_input = SessionInput {
        agent: "claude".to_string(),
        session_id: "incomplete_final_record".to_string(),
        source: RawSource::Jsonl(completed),
    };
    let normalized = normalize_source(&completed_input).expect("completed source must normalize");
    assert_eq!(normalized.events.len(), 3);
}

#[test]
fn unrecognized_type_without_role_is_dropped_but_with_role_is_kept() {
    let normalized = normalize_source(&input("unrecognized_type")).expect("fixture must normalize");
    assert_eq!(normalized.events.len(), 3);
    assert_eq!(normalized.events[1].ts_ms, Some(1_777_622_402_000));
}

#[test]
fn oversized_line_between_valid_records() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let source = oversized_line_jsonl(9 * 1024 * 1024);
    let input = file_input("oversized_line", &source, &directory);
    let normalized = normalize_source(&input).expect("generated source must normalize");
    assert_eq!(normalized.events.len(), 2);
    assert_eq!(normalized.events[0].ts_ms, Some(1_761_000_000_000));
    assert_eq!(normalized.events[1].ts_ms, Some(1_761_000_001_000));
}

#[test]
fn many_record_source_analyzes_to_completion() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let source = many_records_jsonl(10_000);
    let input = file_input("many_records", &source, &directory);
    let sessions = analyze_sources_with(vec![input], false).sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].event_count, 10_000);
    assert_eq!(sessions[0].tokens_in, 70_000);
    assert_eq!(sessions[0].tokens_out, 30_000);
}
