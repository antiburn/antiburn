use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use antiburn_local::analysis::{
    AppendOnlyGuarantee, CompositeSink, CoverageReason, EvidenceCoverage, EvidenceSource,
    EvidenceValue, FAST_SPEED_KEY, MemoryTurnRowStore, NormalizedSession, PartialReason, RawSource,
    RecordCoverage, SessionCollector, SessionEvidence, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceCapabilities, SourceClaim, SourceKind, TurnCounts,
    TurnRowSink, TurnRowStore, VisitOutcome, adapter_for, analyze_sources_with,
    append_only_guarantee, normalize_source,
};
use antiburn_local::discovery::source_version::{
    FINGERPRINT_HEAD_BYTES, FingerprintInputs, SourceStat, head_hash_of,
};
use antiburn_local::insights::{DetectorId, requirements};
use serde_json::{Value, json};

fn fixture(name: &str) -> &'static str {
    match name {
        "records_all_kinds" => {
            include_str!("fixtures/codex_characterization/records_all_kinds.jsonl")
        }
        "malformed_between_valid" => {
            include_str!("fixtures/codex_characterization/malformed_between_valid.jsonl")
        }
        "unrecognized_type" => {
            include_str!("fixtures/codex_characterization/unrecognized_type.jsonl")
        }
        "absent_model_and_effort" => {
            include_str!("fixtures/codex_characterization/absent_model_and_effort.jsonl")
        }
        "resolved_fork" => include_str!("fixtures/codex_characterization/resolved_fork.jsonl"),
        "fork_developer_lookbehind" => {
            include_str!("fixtures/codex_characterization/fork_developer_lookbehind.jsonl")
        }
        "fork_disputed_window" => {
            include_str!("fixtures/codex_characterization/fork_disputed_window.jsonl")
        }
        "unresolved_fork" => {
            include_str!("fixtures/codex_characterization/unresolved_fork.jsonl")
        }
        "incomplete_final_record" => {
            include_str!("fixtures/codex_characterization/incomplete_final_record.jsonl")
        }
        "service_tier_priority" => {
            include_str!("fixtures/codex_characterization/service_tier_priority.jsonl")
        }
        "service_tier_absent" => {
            include_str!("fixtures/codex_characterization/service_tier_absent.jsonl")
        }
        _ => panic!("unknown Codex characterization fixture: {name}"),
    }
}

fn fixture_names() -> [&'static str; 11] {
    [
        "records_all_kinds",
        "malformed_between_valid",
        "unrecognized_type",
        "absent_model_and_effort",
        "resolved_fork",
        "fork_developer_lookbehind",
        "fork_disputed_window",
        "unresolved_fork",
        "incomplete_final_record",
        "service_tier_priority",
        "service_tier_absent",
    ]
}

fn input(name: &str) -> SessionInput {
    SessionInput {
        agent: "codex".to_owned(),
        session_id: name.to_owned(),
        source: RawSource::Jsonl(fixture(name).to_owned()),
    }
}

fn collect(input: &SessionInput) -> (RecordCoverage, BTreeSet<PartialReason>, NormalizedSession) {
    let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
    adapter_for("codex")
        .visit(input, &mut collector)
        .expect("Codex fixture must stream");
    let coverage = collector.coverage();
    let reasons = collector.partial_reasons().clone();
    let session = collector.into_session().expect("Codex stream must finish");
    (coverage, reasons, session)
}

fn composite(input: &SessionInput) -> (SessionEvidence, SessionMetricsAccumulator) {
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::codex(),
    });
    let store = MemoryTurnRowStore::new(input.agent.clone(), input.session_id.clone());
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("codex")
        .visit(input, &mut sink)
        .expect("Codex fixture must stream");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("Codex evidence must publish");
    let (metrics, _evidence_accumulator) = sink.into_parts().expect("Codex metrics must publish");
    (evidence, metrics)
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex_characterization/goldens"
    ))
    .join(format!("{name}.json"))
}

fn check_golden(name: &str) {
    let input = input(name);
    let (evidence, metrics) = composite(&input);
    let (_, reasons, normalized) = collect(&input);
    let actual = json!({
        "normalizedSession": normalized,
        "partialReasons": reasons.into_iter().map(|reason| format!("{reason:?}")).collect::<Vec<_>>(),
        "metrics": metrics.metrics(),
        "evidence": evidence,
    });
    let rendered = serde_json::to_string_pretty(&actual).unwrap();
    let path = golden_path(name);
    if std::env::var("UPDATE_GOLDENS").as_deref() == Ok("1") {
        fs::write(&path, format!("{rendered}\n")).unwrap();
    }
    let expected: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(actual, expected, "golden differs for {name}");
}

fn is_supported<T>(value: &EvidenceValue<T>) -> bool {
    !matches!(value, EvidenceValue::Unsupported)
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
    for name in fixture_names() {
        let (evidence, _) = composite(&input(name));
        let persisted = serde_json::to_value(evidence).expect("evidence must serialize");
        let mut persisted_strings = Vec::new();
        collect_private_strings(&persisted, &mut persisted_strings);

        for line in fixture(name).lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let payload = record.get("payload").unwrap_or(&record);
            let mut private_strings = Vec::new();
            if let Some(content) = payload.get("content") {
                collect_private_message_content(content, &mut private_strings);
            }
            for key in ["input", "arguments", "output", "action", "command"] {
                if let Some(value) = payload.get(key) {
                    collect_tool_private_input(value, &mut private_strings);
                }
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
fn codex_capabilities_match_published_evidence() {
    let (evidence, _) = composite(&input("records_all_kinds"));
    let capabilities = evidence.capabilities;

    assert_eq!(capabilities, SourceCapabilities::codex());
    assert!(capabilities.request_context_tokens && is_supported(&evidence.context));
    assert!(capabilities.timestamps_and_order && is_supported(&evidence.time_range));
    assert!(capabilities.tool_invocations && is_supported(&evidence.tools));
    assert!(capabilities.model_identity && is_supported(&evidence.models));
    assert!(capabilities.token_classes && is_supported(&evidence.models));
    assert!(capabilities.reasoning_effort_tier && is_supported(&evidence.models));
    assert!(capabilities.compaction_boundaries && is_supported(&evidence.compactions));

    assert!(capabilities.fast_tier);
    assert!(!capabilities.cache_write_tokens);
    assert!(!capabilities.skill_mcp_attribution);
    assert!(!capabilities.tool_definitions);
    assert!(!capabilities.service_tier);
    assert!(!capabilities.subagent_relationships);
    assert!(!capabilities.subagent_models);
    assert!(!capabilities.thread_identity);
    assert!(!capabilities.quota_incidents);
    assert!(!capabilities.harness_version);
    assert!(matches!(
        evidence.context_sources,
        EvidenceValue::Unsupported
    ));
    assert!(matches!(evidence.subagents, EvidenceValue::Unsupported));
    assert!(matches!(
        evidence.quota_incidents,
        EvidenceValue::Unsupported
    ));
    assert!(matches!(
        evidence.provenance.harness_version,
        EvidenceValue::Unsupported
    ));
    let EvidenceValue::Complete(cache) = evidence.cache else {
        panic!("the supported cache fields must remain available");
    };
    assert_eq!(cache.cache_creation_tokens, 0);
    assert!(matches!(cache.previous_turn, EvidenceValue::Unsupported));
    let EvidenceValue::Complete(models) = evidence.models else {
        panic!("model evidence must be complete");
    };
    assert!(models.fast_modes.is_empty());
    assert!(matches!(models.service_tiers, EvidenceValue::Unsupported));
}

#[test]
fn claude_capabilities_still_match_published_evidence() {
    let input = SessionInput {
        agent: "claude".to_owned(),
        session_id: "claude-contract".to_owned(),
        source: RawSource::Jsonl(
            concat!(
                r#"{"type":"assistant","uuid":"record-1","timestamp":"2026-08-01T10:00:00Z","message":{"id":"message-1","role":"assistant","model":"claude-test","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":3},"content":[]}}"#,
                "\n"
            )
            .to_owned(),
        ),
    };
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
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
    let mut sink = CompositeSink::with_turn_rows(metrics, accumulator, turn_rows);
    let outcome = adapter_for("claude").visit(&input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().unwrap();

    assert_eq!(evidence.capabilities, SourceCapabilities::claude());
    assert!(is_supported(&evidence.context));
    assert!(is_supported(&evidence.time_range));
    assert!(is_supported(&evidence.tools));
    assert!(is_supported(&evidence.context_sources));
    assert!(is_supported(&evidence.models));
    assert!(is_supported(&evidence.subagents));
    assert!(is_supported(&evidence.cache));
    assert!(is_supported(&evidence.compactions));
    assert!(matches!(
        evidence.quota_incidents,
        EvidenceValue::Unsupported
    ));
    assert!(matches!(
        evidence.provenance.harness_version,
        EvidenceValue::Unsupported
    ));
    let EvidenceValue::Complete(context_sources) = evidence.context_sources else {
        panic!("Claude context sources must be complete");
    };
    assert!(matches!(
        context_sources.tool_definitions,
        EvidenceValue::Unsupported
    ));
    let EvidenceValue::Complete(models) = evidence.models else {
        panic!("Claude models must be complete");
    };
    assert!(matches!(models.service_tiers, EvidenceValue::Unsupported));
}

#[test]
fn codex_detector_prerequisites_assess_only_supported_detectors() {
    let capabilities = SourceCapabilities::codex();
    let assessed = DetectorId::ALL
        .into_iter()
        .filter(|detector| {
            requirements(*detector)
                .capabilities
                .iter()
                .all(|clause| clause.iter().any(|flag| flag.is_set(&capabilities)))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        assessed,
        vec![DetectorId::ModelOverthinking, DetectorId::OldModelUsage]
    );
}

#[test]
fn resolved_fork_stream_matches_the_existing_parser_and_owns_only_child_usage() {
    let input = input("resolved_fork");
    let parsed = normalize_source(&input).unwrap();
    let (coverage, reasons, streamed) = collect(&input);

    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert_eq!(streamed, parsed);
    assert_eq!(streamed.events.len(), 1);
    assert_eq!(streamed.events[0].usage.context_tokens(), 300);
    assert_eq!(streamed.events[0].model.as_deref(), Some("gpt-child"));
}

#[test]
fn fork_developer_lookbehind_matches_the_existing_parser() {
    let input = input("fork_developer_lookbehind");
    let parsed = normalize_source(&input).unwrap();
    let (coverage, reasons, streamed) = collect(&input);

    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert_eq!(streamed, parsed);
    assert_eq!(streamed.events.len(), 2);
    assert_eq!(
        streamed.events[0].role,
        antiburn_local::analysis::Role::System
    );
    assert_eq!(streamed.events[1].usage.context_tokens(), 320);
}

#[test]
fn fork_usage_between_task_start_and_discriminator_is_owned() {
    let input = input("fork_disputed_window");
    let parsed = normalize_source(&input).unwrap();
    let (coverage, reasons, streamed) = collect(&input);

    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert_eq!(streamed, parsed);
    assert_eq!(streamed.events.len(), 2);
    assert_eq!(streamed.events[0].usage.context_tokens(), 450);
    assert_eq!(streamed.events[1].usage.context_tokens(), 250);
}

#[test]
fn unresolved_fork_matches_batch_and_attributes_all_usage() {
    let input = input("unresolved_fork");
    let parsed = normalize_source(&input).unwrap();
    let (coverage, reasons, streamed) = collect(&input);
    let (evidence, metrics) = composite(&input);

    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert_eq!(streamed, parsed);
    assert_eq!(streamed.events.len(), 1);
    assert_eq!(streamed.events[0].usage.context_tokens(), 700);
    assert_eq!(metrics.metrics().tokens_in, 200);
    assert_eq!(metrics.metrics().peak_context_tokens, 700);
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
}

#[test]
fn streaming_metrics_equal_the_shipped_batch_for_every_fixture() {
    for name in fixture_names() {
        let input = input(name);
        let parsed = normalize_source(&input).expect("fixture must normalize");
        let (_, _, streamed) = collect(&input);
        assert_eq!(streamed, parsed, "normalized fixture {name}");

        let expected = analyze_sources_with(vec![input.clone()], true)
            .sessions
            .into_iter()
            .next();
        let (_, metrics) = composite(&input);
        let actual = metrics.metrics();
        if let Some(expected) = expected {
            assert_eq!(actual, expected, "metrics fixture {name}");
        } else {
            assert!(parsed.events.is_empty(), "batch omitted fixture {name}");
            assert_eq!(actual.event_count, 0, "metrics fixture {name}");
        }
    }
}

#[test]
fn missing_event_timestamp_is_unusable_and_not_counted() {
    let input = SessionInput {
        agent: "codex".to_owned(),
        session_id: "missing-event-timestamp".to_owned(),
        source: RawSource::Jsonl(
            concat!(
                r#"{"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"id":"synthetic-missing-timestamp","timestamp":"2026-08-09T10:00:00Z","source":"cli"}}"#,
                "\n",
                r#"{"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-test","effort":"low"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":300,"cached_input_tokens":100,"output_tokens":20,"total_tokens":320},"total_token_usage":{"input_tokens":300,"cached_input_tokens":100,"output_tokens":20,"total_tokens":320},"model_context_window":100000}}}"#,
                "\n"
            )
            .to_owned(),
        ),
    };
    let (coverage, reasons, streamed) = collect(&input);
    let (_, metrics) = composite(&input);

    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(reasons, BTreeSet::from([PartialReason::MalformedRecord]));
    assert!(streamed.events.is_empty());
    assert_eq!(metrics.metrics().event_count, 0);
    assert_eq!(metrics.metrics().tokens_in, 0);
}

#[test]
fn missing_model_and_effort_never_publish_a_clean_model_group() {
    let (evidence, _) = composite(&input("absent_model_and_effort"));

    assert!(matches!(
        evidence.models,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
}

#[test]
fn incomplete_active_writer_tail_is_partial_and_keeps_the_valid_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("active.jsonl");
    let bytes = fixture("incomplete_final_record").as_bytes();
    fs::write(&path, bytes.strip_suffix(b"\n").unwrap_or(bytes)).unwrap();
    let input = SessionInput {
        agent: "codex".to_owned(),
        session_id: "active".to_owned(),
        source: RawSource::File(path),
    };
    let (coverage, reasons, session) = collect(&input);
    let (evidence, _) = composite(&input);

    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(reasons, BTreeSet::from([PartialReason::IncompleteTail]));
    assert!(session.events.is_empty());
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::IncompleteTail)
    );
}

fn source_claim(path: &std::path::Path) -> SourceClaim {
    let mut file = fs::File::open(path).unwrap();
    let stat = SourceStat::from_open_std_file(&file).unwrap();
    let mut head = Vec::new();
    Read::by_ref(&mut file)
        .take(FINGERPRINT_HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .unwrap();
    SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&head)),
    })
}

#[test]
fn claimed_codex_source_rejects_a_change_instead_of_publishing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("changed.jsonl");
    fs::write(&path, fixture("records_all_kinds")).unwrap();
    let claim = source_claim(&path);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{}\n")
        .unwrap();
    let input = SessionInput {
        agent: "codex".to_owned(),
        session_id: "changed".to_owned(),
        source: RawSource::File(path),
    };
    let mut collector = SessionCollector::new("codex", "changed");
    let outcome = adapter_for("codex")
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut collector,
        )
        .unwrap();

    assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));
    assert_eq!(append_only_guarantee("codex"), AppendOnlyGuarantee::Absent);
    assert_eq!(
        collector.into_session().unwrap_err().to_string(),
        "record stream ended without a session summary"
    );
}

#[test]
fn session_metadata_start_precedes_the_first_normalized_event() {
    let (_, metrics) = composite(&input("records_all_kinds"));

    assert_eq!(metrics.started_at_ms(), Some(1_785_578_398_000));
    assert_eq!(metrics.earliest_ts_ms(), Some(1_785_578_402_000));
}

#[test]
fn complete_codex_fixture_matches_golden() {
    check_golden("records_all_kinds");
}

#[test]
fn malformed_codex_fixture_matches_golden() {
    check_golden("malformed_between_valid");
}

#[test]
fn unrecognized_codex_fixture_matches_golden() {
    check_golden("unrecognized_type");
}

#[test]
fn service_tier_changes_split_fast_modes_and_cover_every_assistant_turn() {
    let (evidence, _) = composite(&input("service_tier_priority"));

    let EvidenceValue::Complete(models) = evidence.models else {
        panic!("service_tier_priority must publish a complete model group");
    };
    assert_eq!(
        models.speed_signal.present_turns,
        models.speed_signal.eligible_turns
    );
    assert_eq!(models.speed_signal.eligible_turns, 5);
    assert_eq!(
        models.fast_modes.get("standard"),
        Some(&TurnCounts {
            main_loop: 3,
            delegated: 0
        })
    );
    assert_eq!(
        models.fast_modes.get(FAST_SPEED_KEY),
        Some(&TurnCounts {
            main_loop: 2,
            delegated: 0
        })
    );
}

#[test]
fn absent_service_tier_reports_the_speed_signal_as_missing() {
    let (evidence, _) = composite(&input("service_tier_absent"));

    let EvidenceValue::Complete(models) = evidence.models else {
        panic!("service_tier_absent must publish a complete model group");
    };
    assert_eq!(models.speed_signal.present_turns, 0);
    // Every turn is eligible (assistant role, model attributed) but none
    // carries a speed value, so `overuse_of_fast_mode::evaluate` reads this
    // as `SignalMissing`, never as clean: `eligible_turns > 0` and
    // `present_turns < eligible_turns`.
    assert!(models.speed_signal.eligible_turns > 0);
    assert!(models.fast_modes.is_empty());
}
