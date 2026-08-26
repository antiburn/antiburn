use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use antiburn_local::analysis::{
    ANALYZER_REVISION, CompositeSink, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage, EvidenceSource,
    EvidenceValue, MAX_RECORD_BYTES, NormalizedSession, OrderingObservation, PARSER_REVISION,
    PartialReason, RawSource, RecordCoverage, SessionCollector, SessionEvidenceAccumulator,
    SessionInput, SessionMetricsAccumulator, SourceCapabilities, SourceKind, adapter_for,
    analyze_session, analyze_sources_with, merge_metrics, merge_subagent_events, normalize_source,
};
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
    file_input_bytes(name, source.as_bytes(), directory)
}

fn file_input_bytes(name: &str, source: &[u8], directory: &tempfile::TempDir) -> SessionInput {
    let path = directory.path().join(format!("{name}.jsonl"));
    fs::write(&path, source).expect("generated source must be written");
    SessionInput {
        agent: "claude".to_string(),
        session_id: name.to_string(),
        source: RawSource::File(path),
    }
}

fn assistant_record(id: &str, timestamp: u64, input: u64, output: u64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":{timestamp},"message":{{"id":"{id}","role":"assistant","model":"claude-3-5-haiku-20241022","usage":{{"input_tokens":{input},"output_tokens":{output}}},"content":[{{"type":"text","text":"{id}"}}]}}}}"#
    )
}

fn three_record_source() -> String {
    [
        assistant_record("first", 1_761_000_000, 2, 3),
        assistant_record("second", 1_761_000_001, 4, 5),
        assistant_record("third", 1_761_000_002, 6, 7),
    ]
    .join("\n")
}

fn stream_claude(input: &SessionInput) -> SessionMetricsAccumulator {
    let mut accumulator =
        SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    adapter_for("claude")
        .visit(input, &mut accumulator)
        .expect("Claude source must be visited");
    accumulator
}

fn stream_composite(input: &SessionInput) -> CompositeSink {
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::claude(),
    });
    let mut composite = CompositeSink::new(metrics, evidence);
    let outcome = adapter_for("claude")
        .visit(input, &mut composite)
        .expect("Claude source must be visited");
    composite.observe_source_outcome(outcome);
    composite
}

fn fixture_names() -> [&'static str; 10] {
    [
        "records_all_kinds",
        "timestamps_repeated_and_out_of_order",
        "malformed_between_valid",
        "incomplete_final_record",
        "unrecognized_type",
        "parent_with_task_spawn",
        "subagent_child",
        "multi_model_session",
        "compaction_with_cache_rehydration",
        "inferred_cache_rehydration",
    ]
}

fn collect_claude(
    input: &SessionInput,
) -> (RecordCoverage, BTreeSet<PartialReason>, NormalizedSession) {
    let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
    adapter_for("claude")
        .visit(input, &mut collector)
        .expect("Claude source must be visited");
    let coverage = collector.coverage();
    let reasons = collector.partial_reasons().clone();
    let session = collector
        .into_session()
        .expect("visited source must produce a session");
    (coverage, reasons, session)
}

fn late_skill_source(marker_has_role: bool) -> String {
    let command = r#"{"type":"user","message":{"role":"user","content":"<command-name>/orbit-tracker</command-name>"}}"#;
    let marker = if marker_has_role {
        r#"{"type":"user","message":{"role":"user","content":"Base directory for this skill: /tmp/orbit-tracker/SKILL.md"}}"#
    } else {
        r#"{"type":"attachment","message":{"content":"Base directory for this skill: /tmp/orbit-tracker/SKILL.md"}}"#
    };
    format!("{command}\n{marker}\n")
}

fn oversized_metric_jsonl(payload_bytes: usize) -> String {
    let mut source = String::with_capacity(payload_bytes + 1_000);
    source.push_str(&assistant_record("first-neighbour", 1_761_000_000, 2, 3));
    source.push('\n');
    source.push_str("{\"type\":\"assistant\",\"padding\":\"");
    source.push_str(&"x".repeat(payload_bytes));
    source.push_str("\",\"timestamp\":1761000001,\"message\":{\"id\":\"oversized\",\"role\":\"assistant\",\"model\":\"claude-3-5-haiku-20241022\",\"usage\":{\"input_tokens\":999999,\"output_tokens\":888888},\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"echo oversized\"}}]}}\n");
    source.push_str(&assistant_record("second-neighbour", 1_761_000_002, 4, 5));
    source.push('\n');
    source
}

#[test]
fn composite_metrics_json_equals_the_streaming_metrics_json_for_every_fixture() {
    for name in fixture_names() {
        let input = input(name);
        let composite = stream_composite(&input);
        let composite_json = serde_json::to_string_pretty(
            &composite
                .metrics()
                .expect("finished source must publish metrics"),
        )
        .expect("composite metrics must serialize");
        let streaming_json = serde_json::to_string_pretty(&stream_claude(&input).metrics())
            .expect("streaming metrics must serialize");
        let composite_value: Value =
            serde_json::from_str(&composite_json).expect("composite JSON must parse");
        let streaming_value: Value =
            serde_json::from_str(&streaming_json).expect("streaming JSON must parse");

        assert_eq!(composite_value, streaming_value, "fixture {name}");
    }
}

#[test]
fn evidence_context_depth_equals_the_fixture_maximum() {
    for name in fixture_names() {
        let input = input(name);
        let expected = normalize_source(&input)
            .expect("fixture must normalize")
            .events
            .iter()
            .map(|event| event.usage.context_tokens())
            .max()
            .unwrap_or(0);
        let evidence = stream_composite(&input)
            .evidence()
            .expect("finished source must publish evidence");
        let observed = match evidence.context {
            EvidenceValue::Complete(context)
            | EvidenceValue::Partial {
                observed: context, ..
            } => context.max_request_context_tokens,
            EvidenceValue::Unsupported => panic!("Claude context evidence must be supported"),
            #[cfg(debug_assertions)]
            EvidenceValue::Unimplemented => panic!("context evidence must be implemented"),
        };

        assert_eq!(observed, expected, "fixture {name}");
    }
}

#[test]
fn evidence_coverage_is_complete_for_every_clean_fixture() {
    for name in fixture_names() {
        let input = input(name);
        let (coverage, _, _) = collect_claude(&input);
        if coverage == RecordCoverage::Partial {
            continue;
        }
        let evidence = stream_composite(&input)
            .evidence()
            .expect("finished source must publish evidence");

        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Complete,
            "fixture {name}"
        );
        assert!(matches!(evidence.context, EvidenceValue::Complete(_)));
    }
}

#[test]
fn fixture_provenance_records_the_source_kind_and_ordering() {
    let monotonic = stream_composite(&input("parent_with_task_spawn"))
        .evidence()
        .expect("finished source must publish evidence");
    assert_eq!(monotonic.provenance.source_kind, SourceKind::Jsonl);
    assert_eq!(monotonic.provenance.parser_revision, PARSER_REVISION);
    assert_eq!(monotonic.provenance.analyzer_revision, ANALYZER_REVISION);
    assert_eq!(
        monotonic.provenance.evidence_schema_revision,
        EVIDENCE_SCHEMA_REVISION
    );
    assert_eq!(
        monotonic.provenance.ordering,
        OrderingObservation::Monotonic
    );

    let out_of_order = stream_composite(&input("timestamps_repeated_and_out_of_order"))
        .evidence()
        .expect("finished source must publish evidence");
    assert_eq!(
        out_of_order.provenance.ordering,
        OrderingObservation::OutOfOrder
    );
}

#[test]
fn streaming_metrics_equal_the_shipped_batch_for_every_fixture() {
    for name in [
        "records_all_kinds",
        "timestamps_repeated_and_out_of_order",
        "malformed_between_valid",
        "incomplete_final_record",
        "unrecognized_type",
        "parent_with_task_spawn",
        "subagent_child",
        "multi_model_session",
        "compaction_with_cache_rehydration",
        "inferred_cache_rehydration",
    ] {
        let input = input(name);
        let expected = analyze_sources_with(vec![input.clone()], true)
            .sessions
            .remove(0);
        assert_eq!(stream_claude(&input).metrics(), expected, "fixture {name}");
    }
}

#[test]
fn streaming_metrics_match_every_golden() {
    for name in [
        "records_all_kinds",
        "timestamps_repeated_and_out_of_order",
        "malformed_between_valid",
        "incomplete_final_record",
        "unrecognized_type",
        "parent_with_task_spawn",
        "subagent_child",
        "multi_model_session",
        "compaction_with_cache_rehydration",
        "inferred_cache_rehydration",
    ] {
        let expected_text = fs::read_to_string(golden_path(name)).expect("golden must exist");
        let expected: Value = serde_json::from_str(&expected_text).expect("golden must be valid");
        let rendered = serde_json::to_string_pretty(&stream_claude(&input(name)).metrics())
            .expect("metrics must serialize");
        let actual: Value =
            serde_json::from_str(&rendered).expect("rendered metrics must be valid JSON");
        assert_eq!(actual, expected["sessions"][0], "fixture {name}");
    }
}

#[test]
fn golden_cost_values_compare_after_a_text_round_trip() {
    let value = 0.0009119999999999999_f64;
    let rendered = serde_json::to_string(&value).expect("cost must serialize");
    let parsed: f64 = serde_json::from_str(&rendered).expect("cost must parse");

    // The golden contract compares serialized text, not in-memory f64 bits.
    assert_ne!(parsed.to_bits(), value.to_bits());
}

#[test]
fn merged_streaming_metrics_equal_the_merged_batch() {
    let parent_input = input("parent_with_task_spawn");
    let child_input = input("subagent_child");
    let parent = stream_claude(&parent_input);
    let child = stream_claude(&child_input);
    let expected = analyze_session(&merge_subagent_events(
        normalize_source(&parent_input).expect("parent fixture must normalize"),
        vec![normalize_source(&child_input).expect("child fixture must normalize")],
    ));
    assert_eq!(merge_metrics(&parent, &[child]), expected);
}

#[test]
fn a_compaction_boundary_bucket_reports_zero_context_tokens() {
    let metrics = stream_claude(&input("compaction_with_cache_rehydration")).metrics();
    let boundary = metrics
        .buckets
        .iter()
        .find(|bucket| bucket.is_compaction_boundary)
        .expect("fixture must contain a compaction boundary");
    assert_eq!(boundary.context_tokens, 0);
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
fn a_malformed_record_keeps_both_neighbour_events() {
    let actual = normalize_source(&input("malformed_between_valid"))
        .ok()
        .map(|session| {
            (
                session.events.len(),
                session.events[0].ts_ms,
                session.events[1].ts_ms,
            )
        });
    assert_eq!(
        actual,
        Some((2, Some(1_772_352_000_000), Some(1_772_352_002_000)))
    );
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

#[test]
fn a_file_source_does_not_commit_an_unterminated_final_record() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let input = file_input("unterminated-file", &three_record_source(), &directory);
    let session = normalize_source(&input).expect("file source must normalize");
    assert_eq!(session.events.len(), 2);
}

#[test]
fn an_in_memory_source_commits_an_unterminated_final_record() {
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "unterminated-memory".to_string(),
        source: RawSource::Jsonl(three_record_source()),
    };
    let session = normalize_source(&input).expect("in-memory source must normalize");
    assert_eq!(session.events.len(), 3);
}

#[test]
fn a_slash_command_skill_resolves_when_its_marker_arrives_later() {
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "late-skill-marker".to_string(),
        source: RawSource::Jsonl(late_skill_source(true)),
    };
    let session = normalize_source(&input).expect("skill source must normalize");
    let detail = session.events[0]
        .tools
        .iter()
        .find(|tool| tool.name == "skill")
        .and_then(|tool| tool.detail.as_deref());
    assert_eq!(detail, Some("orbit-tracker"));
}

#[test]
fn a_skill_marker_in_a_record_with_no_role_is_still_collected() {
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "roleless-skill-marker".to_string(),
        source: RawSource::Jsonl(late_skill_source(false)),
    };
    let session = normalize_source(&input).expect("skill source must normalize");
    let detail = session.events[0]
        .tools
        .iter()
        .find(|tool| tool.name == "skill")
        .and_then(|tool| tool.detail.as_deref());
    assert_eq!(detail, Some("orbit-tracker"));
}

#[test]
fn two_priceable_models_of_equal_rank_keep_the_first_seen() {
    let source = [
        assistant_record("opus-47", 1_761_000_000, 2, 3)
            .replace("claude-3-5-haiku-20241022", "claude-opus-4-7-20260115"),
        assistant_record("opus-46", 1_761_000_001, 4, 5)
            .replace("claude-3-5-haiku-20241022", "claude-opus-4-6-20260115"),
    ]
    .join("\n");
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "equal-rank-models".to_string(),
        source: RawSource::Jsonl(source),
    };
    let session = normalize_source(&input).expect("model source must normalize");
    assert_eq!(session.model.as_deref(), Some("claude-opus-4-7-20260115"));
}

#[test]
fn an_unopenable_file_source_omits_the_whole_session() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "unopenable".to_string(),
        source: RawSource::File(directory.path().join("missing.jsonl")),
    };
    let normalize_failed = normalize_source(&input).is_err();
    let session_was_omitted = analyze_sources_with(vec![input], false).sessions.is_empty();
    assert_eq!((normalize_failed, session_was_omitted), (true, true));
}

#[test]
fn invalid_utf8_between_valid_records_omits_the_whole_session() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let mut source = assistant_record("first", 1_761_000_000, 2, 3).into_bytes();
    source.push(b'\n');
    source.extend_from_slice(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"");
    source.extend_from_slice(&[0xff, 0xfe]);
    source.extend_from_slice(b"\"}}\n");
    source.extend_from_slice(assistant_record("second", 1_761_000_002, 4, 5).as_bytes());
    source.push(b'\n');
    let input = file_input_bytes("invalid-middle", &source, &directory);
    assert!(normalize_source(&input).is_err());
}

#[test]
fn an_oversized_metric_bearing_record_is_dropped_for_both_source_variants() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let source = oversized_metric_jsonl(MAX_RECORD_BYTES + 1);
    let inputs = [
        file_input("oversized-metrics-file", &source, &directory),
        SessionInput {
            agent: "claude".to_string(),
            session_id: "oversized-metrics-memory".to_string(),
            source: RawSource::Jsonl(source),
        },
    ];

    let actual: Vec<_> = inputs
        .iter()
        .map(|input| {
            let (coverage, reasons, session) = collect_claude(input);
            let metrics = analyze_session(&session);
            (
                coverage,
                reasons,
                session.events.len(),
                metrics.tokens_in,
                metrics.tokens_out,
            )
        })
        .collect();
    let expected = vec![
        (
            RecordCoverage::Partial,
            BTreeSet::from([PartialReason::Oversized]),
            2,
            6,
            8,
        );
        2
    ];
    assert_eq!(actual, expected);
}

#[test]
fn invalid_utf8_inside_an_oversized_record_does_not_omit_the_session() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let mut source = assistant_record("first", 1_761_000_000, 2, 3).into_bytes();
    source.push(b'\n');
    source.extend(std::iter::repeat_n(b'x', MAX_RECORD_BYTES + 1));
    source.push(0xff);
    source.push(b'\n');
    source.extend_from_slice(assistant_record("second", 1_761_000_002, 4, 5).as_bytes());
    source.push(b'\n');
    let input = file_input_bytes("invalid-oversized", &source, &directory);
    let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
    let visit_succeeded = adapter_for("claude").visit(&input, &mut collector).is_ok();
    let actual = (
        visit_succeeded,
        collector.coverage(),
        collector.partial_reasons().clone(),
        collector
            .into_session()
            .ok()
            .map(|session| session.events.len()),
    );
    assert_eq!(
        actual,
        (
            true,
            RecordCoverage::Partial,
            BTreeSet::from([PartialReason::Oversized]),
            Some(2),
        )
    );
}

#[test]
fn invalid_utf8_inside_an_unterminated_tail_does_not_omit_the_session() {
    let directory = tempfile::tempdir().expect("temporary directory must be created");
    let mut source = assistant_record("first", 1_761_000_000, 2, 3).into_bytes();
    source.push(b'\n');
    source.extend_from_slice(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"");
    source.push(0xff);
    let input = file_input_bytes("invalid-tail", &source, &directory);
    let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
    let visit_succeeded = adapter_for("claude").visit(&input, &mut collector).is_ok();
    let actual = (
        visit_succeeded,
        collector.coverage(),
        collector.partial_reasons().clone(),
        collector
            .into_session()
            .ok()
            .map(|session| session.events.len()),
    );
    assert_eq!(
        actual,
        (
            true,
            RecordCoverage::Partial,
            BTreeSet::from([PartialReason::IncompleteTail]),
            Some(1),
        )
    );
}
