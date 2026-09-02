use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use antiburn_local::analysis::{
    ANALYZER_REVISION, CompositeSink, CoverageReason, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage,
    EvidenceSource, EvidenceValue, MAX_RECORD_BYTES, MemoryTurnRowStore, NormalizedSession,
    OrderingObservation, PARSER_REVISION, PartialReason, RawSource, RecordCoverage,
    SessionCollector, SessionEvidence, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceCapabilities, SourceKind, TurnFacts, TurnRowSink,
    TurnRowStore, TurnScope, adapter_for, analyze_session, analyze_sources_with, merge_metrics,
    merge_subagent_events, normalize_source,
};
use antiburn_local::insights::{
    CoverageCounts, DetectorId, DetectorStatus, EfficiencyReport, EfficiencyReportAccumulator,
    NotAssessedReason, ReportContext, ReportWindow,
};
use rusqlite::params;
use serde_json::{Value, json};

#[path = "support/pricing.rs"]
mod pricing;

fn fixture(name: &str) -> &'static str {
    pricing::install();
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
        "unrecognized_role_with_usage" => {
            include_str!("fixtures/claude_characterization/unrecognized_role_with_usage.jsonl")
        }
        "unrecognized_evidence_shapes" => {
            include_str!("fixtures/claude_characterization/unrecognized_evidence_shapes.jsonl")
        }
        "unrecognized_inert_records" => {
            include_str!("fixtures/claude_characterization/unrecognized_inert_records.jsonl")
        }
        "unrecognized_inert_sidechain" => {
            include_str!("fixtures/claude_characterization/unrecognized_inert_sidechain.jsonl")
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
        "housekeeping_records" => {
            include_str!("fixtures/claude_characterization/housekeeping_records.jsonl")
        }
        "mcp_and_skill_sources" => {
            include_str!("fixtures/claude_characterization/mcp_and_skill_sources.jsonl")
        }
        "reasoning_and_fast_mode" => {
            include_str!("fixtures/claude_characterization/reasoning_and_fast_mode.jsonl")
        }
        "delegated_turns" => {
            include_str!("fixtures/claude_characterization/delegated_turns.jsonl")
        }
        "delegated_models" => {
            include_str!("fixtures/claude_characterization/delegated_models.jsonl")
        }
        "delegated_model_missing" => {
            include_str!("fixtures/claude_characterization/delegated_model_missing.jsonl")
        }
        "thread_identity_chain" => {
            include_str!("fixtures/claude_characterization/thread_identity_chain.jsonl")
        }
        "thread_identity_missing_uuid" => {
            include_str!("fixtures/claude_characterization/thread_identity_missing_uuid.jsonl")
        }
        "sidechain_in_parent" => {
            include_str!("fixtures/claude_characterization/sidechain_in_parent.jsonl")
        }
        "late_skill_metrics" => {
            include_str!("fixtures/claude_characterization/late_skill_metrics.jsonl")
        }
        "two_compactions_second_without_metadata" => include_str!(
            "fixtures/claude_characterization/two_compactions_second_without_metadata.jsonl"
        ),
        "rehydration_gap_none" => {
            include_str!("fixtures/claude_characterization/rehydration_gap_none.jsonl")
        }
        "disorder_ladder" => {
            include_str!("fixtures/claude_characterization/disorder_ladder.jsonl")
        }
        "subagent_single_timestamp" => {
            include_str!("fixtures/claude_characterization/subagent_single_timestamp.jsonl")
        }
        "compaction_continues_thread" => {
            include_str!("fixtures/claude_characterization/compaction_continues_thread.jsonl")
        }
        "inline_sidechain_own_thread" => {
            include_str!("fixtures/claude_characterization/inline_sidechain_own_thread.jsonl")
        }
        "within_file_duplicate_uuid" => {
            include_str!("fixtures/claude_characterization/within_file_duplicate_uuid.jsonl")
        }
        "session_overdepth_finding" => {
            include_str!("fixtures/claude_characterization/session_overdepth_finding.jsonl")
        }
        "model_overthinking_finding" => {
            include_str!("fixtures/claude_characterization/model_overthinking_finding.jsonl")
        }
        "fast_mode_overuse_clean" => {
            include_str!("fixtures/claude_characterization/fast_mode_overuse_clean.jsonl")
        }
        "fork_replay_parent" => {
            include_str!("fixtures/claude_characterization/fork_replay_parent.jsonl")
        }
        "fork_replay_subagent" => {
            include_str!("fixtures/claude_characterization/fork_replay_subagent.jsonl")
        }
        "fork_replay_subagent_meta" => {
            include_str!("fixtures/claude_characterization/fork_replay_subagent.meta.json")
        }
        "fork_replay_fork" => {
            include_str!("fixtures/claude_characterization/fork_replay_fork.jsonl")
        }
        "fork_replay_fork_meta" => {
            include_str!("fixtures/claude_characterization/fork_replay_fork.meta.json")
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
    let store = MemoryTurnRowStore::new(input.agent.clone(), input.session_id.clone());
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("claude")
        .visit(input, &mut composite)
        .expect("Claude source must be visited");
    composite.observe_source_outcome(outcome);
    composite
}

fn fixture_names() -> [&'static str; 27] {
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
        "housekeeping_records",
        "delegated_models",
        "delegated_model_missing",
        "thread_identity_chain",
        "thread_identity_missing_uuid",
        "sidechain_in_parent",
        "late_skill_metrics",
        "two_compactions_second_without_metadata",
        "rehydration_gap_none",
        "disorder_ladder",
        "subagent_single_timestamp",
        "compaction_continues_thread",
        "inline_sidechain_own_thread",
        "within_file_duplicate_uuid",
        "session_overdepth_finding",
        "model_overthinking_finding",
        "fast_mode_overuse_clean",
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
fn effort_tiers_come_only_from_explicit_fields() {
    let evidence = stream_composite(&input("reasoning_and_fast_mode"))
        .evidence()
        .expect("evidence must publish");
    let models = match evidence.models {
        EvidenceValue::Complete(models)
        | EvidenceValue::Partial {
            observed: models, ..
        } => models,
        EvidenceValue::Unsupported => panic!("Claude models must be supported"),
    };
    assert_eq!(models.effort_tiers["high"].main_loop, 1);
    assert_eq!(models.effort_tiers["low"].delegated, 1);
    assert!(!models.effort_tiers.contains_key("wording"));
}

#[test]
fn fast_mode_counts_split_main_loop_from_delegated() {
    let evidence = stream_composite(&input("reasoning_and_fast_mode"))
        .evidence()
        .expect("evidence must publish");
    let models = match evidence.models {
        EvidenceValue::Complete(models)
        | EvidenceValue::Partial {
            observed: models, ..
        } => models,
        EvidenceValue::Unsupported => panic!("Claude models must be supported"),
    };
    assert_eq!(models.fast_modes["fast"].main_loop, 1);
    assert_eq!(models.fast_modes["fast"].delegated, 1);
}

#[test]
fn delegated_turns_are_not_double_counted() {
    let evidence = stream_composite(&input("reasoning_and_fast_mode"))
        .evidence()
        .expect("evidence must publish");
    let subagents = match evidence.subagents {
        EvidenceValue::Complete(subagents)
        | EvidenceValue::Partial {
            observed: subagents,
            ..
        } => subagents,
        EvidenceValue::Unsupported => panic!("Claude subagents must be supported"),
    };
    assert_eq!(subagents.delegated_turns, 1);
}

#[test]
fn a_skill_origin_is_unsupported_not_guessed() {
    let evidence = stream_composite(&input("mcp_and_skill_sources"))
        .evidence()
        .expect("evidence must publish");
    let sources = match evidence.context_sources {
        EvidenceValue::Complete(sources)
        | EvidenceValue::Partial {
            observed: sources, ..
        } => sources,
        EvidenceValue::Unsupported => panic!("Claude sources must be supported"),
    };
    assert!(sources.skills.values().all(|source| {
        matches!(source.origin, EvidenceValue::Unsupported) && source.description.is_some()
    }));
}

#[test]
fn mcp_sources_keep_names_but_never_instruction_blocks() {
    let evidence = stream_composite(&input("mcp_and_skill_sources"))
        .evidence()
        .expect("evidence must publish");
    let persisted = serde_json::to_string(&evidence).expect("evidence must serialize");
    let sources = match evidence.context_sources {
        EvidenceValue::Complete(sources)
        | EvidenceValue::Partial {
            observed: sources, ..
        } => sources,
        EvidenceValue::Unsupported => panic!("Claude sources must be supported"),
    };
    assert!(sources.mcp_servers.contains_key("nebula-docs"));
    assert!(sources.mcp_servers.contains_key("lunar-data"));
    assert!(
        sources
            .mcp_servers
            .values()
            .all(|source| source.description.is_none()),
        "MCP instruction blocks must never persist as descriptions"
    );
    assert!(!persisted.contains("Search synthetic nebula documentation."));
    assert!(!persisted.contains("Read synthetic lunar measurements."));
}

#[test]
fn tool_definitions_are_unsupported_not_inferred_from_invocations() {
    let evidence = stream_composite(&input("mcp_and_skill_sources"))
        .evidence()
        .expect("evidence must publish");
    let sources = match evidence.context_sources {
        EvidenceValue::Complete(sources)
        | EvidenceValue::Partial {
            observed: sources, ..
        } => sources,
        EvidenceValue::Unsupported => panic!("Claude sources must be supported"),
    };
    assert!(matches!(
        sources.tool_definitions,
        EvidenceValue::Unsupported
    ));
}

fn evidence_fixture_names() -> [&'static str; 31] {
    [
        "records_all_kinds",
        "timestamps_repeated_and_out_of_order",
        "malformed_between_valid",
        "incomplete_final_record",
        "unrecognized_type",
        "unrecognized_role_with_usage",
        "unrecognized_evidence_shapes",
        "unrecognized_inert_records",
        "unrecognized_inert_sidechain",
        "parent_with_task_spawn",
        "subagent_child",
        "multi_model_session",
        "compaction_with_cache_rehydration",
        "inferred_cache_rehydration",
        "mcp_and_skill_sources",
        "reasoning_and_fast_mode",
        "delegated_turns",
        "delegated_models",
        "delegated_model_missing",
        "housekeeping_records",
        "thread_identity_chain",
        "thread_identity_missing_uuid",
        "sidechain_in_parent",
        "late_skill_metrics",
        "two_compactions_second_without_metadata",
        "rehydration_gap_none",
        "disorder_ladder",
        "subagent_single_timestamp",
        "compaction_continues_thread",
        "inline_sidechain_own_thread",
        "within_file_duplicate_uuid",
    ]
}

fn collect_private_strings(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(value) if !value.is_empty() => output.push(value.clone()),
        Value::Array(values) => {
            for value in values {
                collect_private_strings(value, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_private_strings(value, output);
            }
        }
        _ => {}
    }
}

fn collect_tool_private_input(value: &Value, output: &mut Vec<String>) {
    if let Value::Object(values) = value {
        for (key, value) in values {
            if !matches!(key.as_str(), "skill" | "name" | "skill_name" | "skillName") {
                collect_private_strings(value, output);
            }
        }
    } else {
        collect_private_strings(value, output);
    }
}

fn collect_private_message_content(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(value) if !value.is_empty() => output.push(value.clone()),
        Value::Array(blocks) => {
            for block in blocks {
                for key in ["text", "thinking", "content"] {
                    if let Some(value) = block.get(key) {
                        collect_private_strings(value, output);
                    }
                }
                for key in ["input", "arguments"] {
                    if let Some(value) = block.get(key) {
                        collect_tool_private_input(value, output);
                    }
                }
            }
        }
        _ => {}
    }
}

#[test]
fn evidence_holds_no_prompt_or_message_text() {
    for name in evidence_fixture_names() {
        let evidence = stream_composite(&input(name))
            .evidence()
            .expect("evidence must publish");
        let persisted = serde_json::to_value(&evidence).expect("evidence must serialize");
        let mut persisted_strings = Vec::new();
        collect_private_strings(&persisted, &mut persisted_strings);
        for line in fixture(name).lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let mut private_strings = Vec::new();
            if let Some(content) = record
                .get("message")
                .and_then(|message| message.get("content"))
                .or_else(|| record.get("content"))
            {
                collect_private_message_content(content, &mut private_strings);
            }
            if let Some(input) = record.get("input").or_else(|| record.get("arguments")) {
                collect_tool_private_input(input, &mut private_strings);
            }
            for value in private_strings {
                assert!(
                    !persisted_strings.contains(&value),
                    "fixture {name} persisted private transcript text: {value}"
                );
            }
        }
    }
}

#[test]
fn a_subagent_child_carries_no_task_description_or_prompt() {
    let evidence = stream_composite(&input("parent_with_task_spawn"))
        .evidence()
        .expect("evidence must publish");
    let persisted = serde_json::to_string(&evidence).expect("evidence must serialize");
    assert!(!persisted.contains("Inspect the fictional orbit module."));
    assert!(!persisted.contains("Read the fictional module and report its shape."));
}

#[test]
fn every_group_reports_a_three_state_value() {
    let evidence = stream_composite(&input("records_all_kinds"))
        .evidence()
        .expect("evidence must publish");
    let value = serde_json::to_value(evidence).expect("evidence must serialize");
    for group in [
        "timeRange",
        "eligibility",
        "context",
        "models",
        "tools",
        "contextSources",
        "subagents",
        "cache",
        "compactions",
        "quotaIncidents",
    ] {
        let state = value[group]["state"].as_str().expect("group state");
        assert!(matches!(state, "complete" | "partial" | "unsupported"));
    }
}

#[test]
fn the_capability_matrix_names_every_group_and_every_capability() {
    let readme = include_str!("fixtures/claude_characterization/README.md");
    for name in [
        "time_range",
        "eligibility",
        "context",
        "models",
        "tools",
        "context_sources",
        "subagents",
        "cache",
        "compactions",
        "quota_incidents",
        "model_identity",
        "token_classes",
        "request_context_tokens",
        "cache_write_tokens",
        "timestamps_and_order",
        "reasoning_effort_tier",
        "fast_tier",
        "tool_invocations",
        "skill_mcp_attribution",
        "compaction_boundaries",
        "subagent_relationships",
        "tool_definitions",
        "service_tier",
        "subagent_models",
        "thread_identity",
        "harness_version",
    ] {
        assert!(readme.contains(name), "matrix omits {name}");
    }
    assert!(readme.matches("Upgrade").count() >= 5);
}

#[test]
fn delegated_models_come_from_sidechain_assistant_records() {
    let evidence = stream_composite(&input("delegated_models"))
        .evidence()
        .expect("evidence must publish");
    assert!(evidence.capabilities.subagent_models);
    let EvidenceValue::Complete(subagents) = evidence.subagents else {
        panic!("explicit delegated models must keep subagent evidence complete");
    };
    assert_eq!(subagents.delegated_turns, 2);
    assert_eq!(
        subagents.delegated_models,
        BTreeSet::from(["claude-opus-4-6".to_owned(), "claude-sonnet-4-6".to_owned(),])
    );
    assert!(
        subagents
            .children
            .iter()
            .all(|child| matches!(child.child_model, EvidenceValue::Unsupported))
    );
}

#[test]
fn a_delegated_turn_without_a_model_degrades_subagent_evidence() {
    let evidence = stream_composite(&input("delegated_model_missing"))
        .evidence()
        .expect("evidence must publish");
    let EvidenceValue::Partial {
        observed: subagents,
        reason: CoverageReason::AttributionIncomplete,
    } = evidence.subagents
    else {
        panic!("a missing delegated model must degrade subagent evidence");
    };
    assert_eq!(subagents.delegated_turns, 1);
    assert!(subagents.delegated_models.is_empty());
}

#[test]
fn delegated_models_unblock_overpowered_subagents() {
    let report = fixture_report("delegated_models");
    assert!(matches!(
        report.detector_statuses[DetectorId::OverpoweredSubagents.index()],
        DetectorStatus::Findings(_)
    ));
}

#[test]
fn a_missing_delegated_model_blocks_a_clean_overpowered_subagents_claim() {
    let report = fixture_report("delegated_model_missing");
    let counts = report.detectors[DetectorId::OverpoweredSubagents.index()];
    assert_eq!(counts.eligible, 1);
    assert_eq!(counts.assessed, 0);
    assert_eq!(
        report.detector_statuses[DetectorId::OverpoweredSubagents.index()],
        DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
    );
}

#[test]
fn quota_incidents_are_unsupported_for_claude() {
    let evidence = stream_composite(&input("delegated_turns"))
        .evidence()
        .expect("evidence must publish");
    assert!(!evidence.capabilities.quota_incidents);
    assert!(matches!(
        evidence.quota_incidents,
        EvidenceValue::Unsupported
    ));
}

fn fixture_cache(name: &str) -> antiburn_local::analysis::CacheEvidence {
    let evidence = stream_composite(&input(name))
        .evidence()
        .expect("evidence must publish");
    match evidence.cache {
        EvidenceValue::Complete(cache)
        | EvidenceValue::Partial {
            observed: cache, ..
        } => cache,
        EvidenceValue::Unsupported => panic!("Claude cache evidence must be supported"),
    }
}

fn fixture_report(name: &str) -> EfficiencyReport {
    let evidence = stream_composite(&input(name))
        .evidence()
        .expect("evidence must publish");
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence);
    accumulator.finish(ReportContext {
        environment_key: "native".to_owned(),
        window: ReportWindow {
            start_epoch: 0,
            end_epoch: 1,
        },
        computed_at_epoch: 1,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage: CoverageCounts::default(),
    })
}

#[test]
fn a_verified_uuid_chain_completes_previous_turn() {
    let evidence = stream_composite(&input("thread_identity_chain"))
        .evidence()
        .expect("evidence must publish");
    assert!(evidence.capabilities.thread_identity);
    let EvidenceValue::Complete(cache) = evidence.cache else {
        panic!("a fully identified chain must keep the cache group complete");
    };
    assert!(matches!(cache.previous_turn, EvidenceValue::Complete(())));
    assert!(matches!(
        cache.provider_eviction,
        EvidenceValue::Unsupported
    ));
}

#[test]
fn a_counted_turn_without_identity_degrades_previous_turn_and_the_cache_group() {
    let evidence = stream_composite(&input("thread_identity_missing_uuid"))
        .evidence()
        .expect("evidence must publish");
    let EvidenceValue::Partial {
        observed: cache,
        reason: CoverageReason::AttributionIncomplete,
    } = evidence.cache
    else {
        panic!("a missing record identity must degrade the cache group");
    };
    assert!(matches!(
        cache.previous_turn,
        EvidenceValue::Partial {
            observed: (),
            reason: CoverageReason::AttributionIncomplete,
        }
    ));
}

#[test]
fn thread_identity_unblocks_sessions_over_depth_and_cache_churn() {
    let report = fixture_report("thread_identity_chain");
    // Depth stays under the report-time cap: a real clean verdict, not
    // CapabilityMissing.
    assert_eq!(
        report.detector_statuses[DetectorId::SessionsOverDepth.index()],
        DetectorStatus::Clean
    );
    // The model switch sits on the delegated sidechain, not the main
    // loop: a real clean verdict, not CapabilityMissing. Row-derived
    // transitions never cross a thread boundary into a different scope.
    assert_eq!(
        report.detector_statuses[DetectorId::CacheChurn.index()],
        DetectorStatus::Clean
    );
    let cache = fixture_cache("thread_identity_chain");
    assert!(cache.model_transitions.is_empty());
    assert!(cache.cache_creation_tokens > 0);
}

#[test]
fn missing_identity_blocks_a_clean_cache_churn_claim() {
    let report = fixture_report("thread_identity_missing_uuid");
    let counts = report.detectors[DetectorId::CacheChurn.index()];
    assert_eq!(counts.eligible, 1);
    assert_eq!(counts.assessed, 0);
    assert_eq!(
        report.detector_statuses[DetectorId::CacheChurn.index()],
        DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
    );
}

#[test]
fn a_compaction_boundary_keeps_the_main_loop_as_one_thread() {
    let evidence = stream_composite(&input("compaction_continues_thread"))
        .evidence()
        .expect("evidence must publish");
    let EvidenceValue::Complete(cache) = evidence.cache else {
        panic!("a resolved compaction link must keep the cache group complete");
    };
    // The boundary's `parentUuid` is null, but its `logicalParentUuid`
    // resolves the link, so the chain never looks unlinked.
    assert!(matches!(cache.previous_turn, EvidenceValue::Complete(())));
    // The model switch on either side of the boundary is still one main
    // thread, so it still counts as a transition.
    assert_eq!(cache.model_transitions.len(), 1);
    assert_eq!(cache.model_transitions[0].from_model, "claude-opus-4-6");
    assert_eq!(cache.model_transitions[0].to_model, "claude-sonnet-4-6");
    assert_eq!(cache.user_controlled_churn.manual_compactions, 1);
    let EvidenceValue::Complete(compactions) = evidence.compactions else {
        panic!("compactions must be complete");
    };
    assert_eq!(compactions.boundaries.len(), 1);
}

#[test]
fn an_inline_sidechain_gets_its_own_thread_and_does_not_affect_the_main_loop() {
    let evidence = stream_composite(&input("inline_sidechain_own_thread"))
        .evidence()
        .expect("evidence must publish");
    let EvidenceValue::Complete(cache) = evidence.cache else {
        panic!("an inline sidechain must keep the cache group complete");
    };
    // The sidechain's own model never reaches the main-thread scan: no
    // transition, no idle gap contributed by its four turns.
    assert!(cache.model_transitions.is_empty());
    assert_eq!(cache.longest_idle_gap_ms, 0);
    assert_eq!(cache.idle_gap_ms_total, 0);
    let EvidenceValue::Complete(subagents) = evidence.subagents else {
        panic!("subagents must be complete");
    };
    assert_eq!(subagents.delegated_turns, 4);
    assert!(subagents.delegated_models.contains("claude-sonnet-4-6"));
}

#[test]
fn a_within_file_duplicate_uuid_keeps_one_thread_and_is_not_a_duplicate_identity() {
    let evidence = stream_composite(&input("within_file_duplicate_uuid"))
        .evidence()
        .expect("evidence must publish");
    // The duplicate-identity diagnostic is a cross-source-key signal — a
    // within-file re-logged uuid must not trip it.
    assert_eq!(evidence.diagnostics.duplicate_turn_identities, 0);
    let EvidenceValue::Complete(cache) = evidence.cache else {
        panic!("a within-file duplicate uuid must keep the cache group complete");
    };
    assert!(matches!(cache.previous_turn, EvidenceValue::Complete(())));
    let EvidenceValue::Complete(models) = evidence.models else {
        panic!("models must be complete");
    };
    let tokens = &models.by_model["claude-opus-4-6"];
    // `dedup_usage` keeps the re-logged message's final usage from being
    // counted twice: 15 input from the completed copy (not 15 + 15) plus 5
    // from the follow-up turn; 6 output from the completed copy (not
    // double-counted against the partial copy's 0) plus 2 from the
    // follow-up turn.
    assert_eq!(tokens.input, 20);
    assert_eq!(tokens.output, 8);
}

#[test]
fn provider_eviction_is_unsupported_not_estimated() {
    let evidence = stream_composite(&input("compaction_with_cache_rehydration"))
        .evidence()
        .expect("evidence must publish");
    let cache = match evidence.cache {
        EvidenceValue::Complete(cache)
        | EvidenceValue::Partial {
            observed: cache, ..
        } => cache,
        EvidenceValue::Unsupported => panic!("Claude cache evidence must be supported"),
    };
    assert!(matches!(
        cache.provider_eviction,
        EvidenceValue::Unsupported
    ));
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
    for name in fixture_names() {
        let input = input(name);
        let expected = analyze_sources_with(vec![input.clone()], true)
            .sessions
            .remove(0);
        assert_eq!(stream_claude(&input).metrics(), expected, "fixture {name}");
    }
}

#[test]
fn streaming_metrics_match_every_golden() {
    for name in fixture_names() {
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
fn merge_metrics_honours_each_parent_turns_own_source() {
    let input = input("sidechain_in_parent");
    let parent = stream_claude(&input);
    let merged = merge_metrics(&parent, &[]);
    assert_eq!(
        merged
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_tokens)
            .sum::<u64>(),
        220
    );
    assert_eq!(
        merged
            .buckets
            .iter()
            .map(|bucket| bucket.tokens_in)
            .sum::<u64>(),
        100
    );
}

#[test]
fn merged_streaming_metrics_equal_the_merged_batch() {
    let parent_input = input("parent_with_task_spawn");
    let child_input = input("subagent_child");
    let parent = stream_claude(&parent_input);
    let child = stream_claude(&child_input);
    let mut expected = analyze_session(&merge_subagent_events(
        normalize_source(&parent_input).expect("parent fixture must normalize"),
        vec![normalize_source(&child_input).expect("child fixture must normalize")],
    ));
    let actual = merge_metrics(&parent, &[child]);

    let mut per_thread_efficiency = parent.metrics().efficiency;
    per_thread_efficiency.add(stream_claude(&child_input).metrics().efficiency);
    assert_eq!(actual.efficiency, per_thread_efficiency);
    expected.efficiency = actual.efficiency;

    assert_eq!(actual.initial_context, None);
    assert_eq!(actual, expected);
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
fn supplemental_metrics_fixtures_pin_order_sensitive_semantics() {
    let late = stream_claude(&input("late_skill_metrics")).metrics();
    assert_eq!(late.skill_uses.len(), 1);
    assert_eq!(late.skill_uses[0].name, "orbit-tracker");
    assert_eq!(late.skill_uses[0].duration_ms, Some(5_000));

    let compactions = stream_claude(&input("two_compactions_second_without_metadata")).metrics();
    let boundary = compactions
        .buckets
        .iter()
        .find(|bucket| bucket.is_compaction_boundary)
        .expect("compaction bucket");
    assert_eq!(compactions.compaction_count, 2);
    assert_eq!(boundary.compaction_trigger, None);
    assert_eq!(boundary.compaction_pre_tokens, None);

    let provider_miss = stream_claude(&input("rehydration_gap_none")).metrics();
    let cache_bucket = provider_miss
        .buckets
        .iter()
        .find(|bucket| bucket.is_cache_routing_miss)
        .expect("provider cache miss bucket");
    assert_eq!(cache_bucket.secs_since_prior_turn, None);

    let single_timestamp = stream_claude(&input("subagent_single_timestamp")).metrics();
    assert_eq!(single_timestamp.buckets[0].subagent_tokens, 110);
    assert_eq!(single_timestamp.buckets[179].subagent_tokens, 220);
}

#[test]
fn supplemental_fixture_goldens() {
    for name in [
        "sidechain_in_parent",
        "late_skill_metrics",
        "two_compactions_second_without_metadata",
        "rehydration_gap_none",
        "disorder_ladder",
        "subagent_single_timestamp",
        "session_overdepth_finding",
        "model_overthinking_finding",
        "fast_mode_overuse_clean",
    ] {
        check_fixture_golden(name);
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
fn golden_housekeeping_records() {
    check_fixture_golden("housekeeping_records");
}

#[test]
fn housekeeping_records_keep_complete_coverage_and_no_unrecognized_diagnostics() {
    let input = input("housekeeping_records");
    let (coverage, reasons, session) = collect_claude(&input);
    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert_eq!(session.events.len(), 2);

    let evidence = stream_composite(&input)
        .evidence()
        .expect("evidence must publish");
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert!(evidence.diagnostics.unrecognized_types.is_empty());
}

#[test]
fn golden_delegated_models() {
    check_fixture_golden("delegated_models");
}

#[test]
fn golden_delegated_model_missing() {
    check_fixture_golden("delegated_model_missing");
}

#[test]
fn golden_thread_identity_chain() {
    check_fixture_golden("thread_identity_chain");
}

#[test]
fn golden_thread_identity_missing_uuid() {
    check_fixture_golden("thread_identity_missing_uuid");
}

#[test]
fn golden_compaction_continues_thread() {
    check_fixture_golden("compaction_continues_thread");
}

#[test]
fn golden_inline_sidechain_own_thread() {
    check_fixture_golden("inline_sidechain_own_thread");
}

#[test]
fn golden_within_file_duplicate_uuid() {
    check_fixture_golden("within_file_duplicate_uuid");
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
fn a_builtin_named_skill_resolves_when_its_marker_arrives_later() {
    let source = [
        json!({
            "type": "user",
            "timestamp": 1_760_000_000,
            "message": {
                "role": "user",
                "content": "<command-name>/review</command-name>"
            }
        })
        .to_string(),
        json!({
            "type": "attachment",
            "message": {
                "content": "Base directory for this skill: /tmp/synthetic/review/SKILL.md"
            }
        })
        .to_string(),
    ]
    .join("\n");
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "builtin-named-skill".to_string(),
        source: RawSource::Jsonl(source),
    };
    let mut metrics = SessionMetricsAccumulator::new("claude", "builtin-named-skill");
    adapter_for("claude")
        .visit(&input, &mut metrics)
        .expect("synthetic command source must stream");
    let metrics = metrics.metrics();
    assert_eq!(metrics.skill_uses.len(), 1);
    assert_eq!(metrics.skill_uses[0].name, "review");
    assert_eq!(metrics.tool_calls_by_name.get("skill"), Some(&1));
}

#[test]
fn builtin_commands_do_not_exhaust_late_skill_metric_candidates() {
    let mut source = String::from(
        "{\"type\":\"attachment\",\"message\":{\"content\":\"Base directory for this skill: /tmp/synthetic/orbit-tracker/SKILL.md\"}}\n",
    );
    for index in 0..300 {
        source.push_str(
            &json!({
                "type": "user",
                "timestamp": 1_760_000_000 + index,
                "message": {
                    "role": "user",
                    "content": "<command-name>/clear</command-name>"
                }
            })
            .to_string(),
        );
        source.push('\n');
    }
    source.push_str(
        &json!({
            "type": "user",
            "timestamp": 1_760_000_301,
            "message": {
                "role": "user",
                "content": "<command-name>/orbit-tracker</command-name>"
            }
        })
        .to_string(),
    );
    source.push('\n');
    let input = SessionInput {
        agent: "claude".to_string(),
        session_id: "builtin-command-budget".to_string(),
        source: RawSource::Jsonl(source),
    };
    let mut metrics = SessionMetricsAccumulator::new("claude", "builtin-command-budget");
    adapter_for("claude")
        .visit(&input, &mut metrics)
        .expect("synthetic command source must stream");
    assert_eq!(metrics.metrics().skill_uses.len(), 1);
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

/* --------------------------------------------------------------------
 * Fork sub-agent replay: a fork's transcript replays its parent agent's
 * records under the same `uuid`s before it appends its own new records.
 * See `crates/antiburn-local/src/analysis/vendors/claude.rs`.
 * ----------------------------------------------------------------- */

/// A `uuid` the fork sub-agent transcript replays from its direct parent
/// (`fork_replay_subagent.jsonl`'s second record) before it appends its own
/// new record.
const FORK_REPLAY_REPLAYED_UUID: &str = "44444444-4444-4444-8444-000000000002";

/// Writes the fork-replay characterization scenario to `directory`: a
/// parent transcript, a normal sub-agent (`agent-fork-replay-normal-child`),
/// and a fork of that sub-agent (`agent-fork-replay-fork-child`) whose
/// `.meta.json` sidecar names it as the fork's `parentAgentId`. Returns the
/// three `SessionInput`s in ingest order: the parent, then each sub-agent.
fn fork_replay_session(directory: &tempfile::TempDir) -> [SessionInput; 3] {
    let parent_path = directory.path().join("fork-replay-parent.jsonl");
    fs::write(&parent_path, fixture("fork_replay_parent")).expect("write parent transcript");

    let subs = directory
        .path()
        .join("fork-replay-parent")
        .join("subagents");
    fs::create_dir_all(&subs).expect("create subagents dir");

    let normal_path = subs.join("agent-fork-replay-normal-child.jsonl");
    fs::write(&normal_path, fixture("fork_replay_subagent")).expect("write normal sub-agent");
    fs::write(
        normal_path.with_extension("meta.json"),
        fixture("fork_replay_subagent_meta"),
    )
    .expect("write normal sub-agent meta.json");

    let fork_path = subs.join("agent-fork-replay-fork-child.jsonl");
    fs::write(&fork_path, fixture("fork_replay_fork")).expect("write fork sub-agent");
    fs::write(
        fork_path.with_extension("meta.json"),
        fixture("fork_replay_fork_meta"),
    )
    .expect("write fork sub-agent meta.json");

    [
        SessionInput {
            agent: "claude".to_string(),
            session_id: "fork-replay-parent".to_string(),
            source: RawSource::File(parent_path),
        },
        SessionInput {
            agent: "claude".to_string(),
            session_id: "fork-replay-normal-child".to_string(),
            source: RawSource::File(normal_path),
        },
        SessionInput {
            agent: "claude".to_string(),
            session_id: "fork-replay-fork-child".to_string(),
            source: RawSource::File(fork_path),
        },
    ]
}

/// Streams `inputs` through one shared [`MemoryTurnRowStore`], the same way
/// the app's production ingest streams a parent and its discovered
/// sub-agents: index 0 is the parent (`TurnScope` derived from its own
/// events), every later index is a delegated child. Returns the merged
/// [`TurnFacts`], the merged [`SessionEvidence`], and the store, so a test
/// can also inspect the written `turn` rows directly.
fn stream_fork_replay_inputs(
    inputs: &[SessionInput],
) -> (TurnFacts, SessionEvidence, Arc<MemoryTurnRowStore>) {
    let store = MemoryTurnRowStore::new("claude", inputs[0].session_id.clone());
    let mut parent_residual: Option<SessionEvidenceAccumulator> = None;
    for (index, input) in inputs.iter().enumerate() {
        let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities: SourceCapabilities::claude(),
        });
        let scope = (index > 0).then_some(TurnScope::Delegated);
        let turn_rows = TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            input.session_id.clone(),
            scope,
        );
        let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
        let outcome = adapter_for("claude")
            .visit(input, &mut composite)
            .expect("Claude source must be visited");
        composite.observe_source_outcome(outcome);
        let (_metrics, residual) = composite
            .into_parts()
            .expect("fork-replay pass must publish");
        if index == 0 {
            parent_residual = Some(residual);
        } else {
            parent_residual
                .as_mut()
                .expect("the parent must stream first")
                .observe_child_coverage(&residual);
        }
    }
    let facts = store.query_turn_facts().expect("turn facts must query");
    let evidence = parent_residual
        .expect("the parent must have streamed")
        .evidence(&facts);
    (facts, evidence, store)
}

#[test]
fn fork_replayed_uuids_are_counted_once_and_do_not_degrade_evidence() {
    let directory = tempfile::TempDir::new().expect("tempdir");
    let inputs = fork_replay_session(&directory);
    let (facts, evidence, store) = stream_fork_replay_inputs(&inputs);

    // The duplicate-identity diagnostic — the backstop for genuine identity
    // corruption — stays at zero: the fork's replayed rows never reached
    // the `turn` table under their own `source_key`.
    assert_eq!(facts.duplicate_turn_identities, 0);

    // Every replayed `uuid` appears exactly once, under the normal
    // sub-agent's own `source_key`, and its usage is counted once.
    let (row_count, source_keys, input_tokens): (i64, String, i64) = store
        .with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*), GROUP_CONCAT(DISTINCT source_key), SUM(input_tokens)
                   FROM turn WHERE uuid = ?1",
                params![FORK_REPLAY_REPLAYED_UUID],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .expect("query the replayed uuid's turn rows");
    assert_eq!(row_count, 1);
    assert_eq!(source_keys, "fork-replay-normal-child");
    assert_eq!(input_tokens, 40);

    // The fork's own new record still counts, under its own source_key.
    let new_row_count: i64 = store
        .with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM turn
                   WHERE uuid = '44444444-4444-4444-8444-000000000004'
                     AND source_key = 'fork-replay-fork-child'",
                [],
                |row| row.get(0),
            )
        })
        .expect("query the fork's own new row");
    assert_eq!(new_row_count, 1);

    assert!(
        matches!(evidence.models, EvidenceValue::Complete(_)),
        "models must not degrade to attribution_incomplete: {:?}",
        evidence.models
    );
    assert!(
        matches!(evidence.subagents, EvidenceValue::Complete(_)),
        "subagents must not degrade to attribution_incomplete: {:?}",
        evidence.subagents
    );
}
