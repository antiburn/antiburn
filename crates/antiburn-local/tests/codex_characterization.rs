use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use antiburn_local::analysis::{
    AppendOnlyGuarantee, CompositeSink, CoverageReason, EvidenceCoverage, EvidenceSource,
    EvidenceValue, NormalizedSession, PartialReason, RawSource, RecordCoverage, SessionCollector,
    SessionEvidence, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SourceCapabilities, SourceClaim, SourceKind, VisitOutcome, adapter_for, append_only_guarantee,
    normalize_source,
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
        "unresolved_fork" => {
            include_str!("fixtures/codex_characterization/unresolved_fork.jsonl")
        }
        "incomplete_final_record" => {
            include_str!("fixtures/codex_characterization/incomplete_final_record.jsonl")
        }
        _ => panic!("unknown Codex characterization fixture: {name}"),
    }
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
    let mut sink = CompositeSink::new(metrics, evidence);
    let outcome = adapter_for("codex")
        .visit(input, &mut sink)
        .expect("Codex fixture must stream");
    sink.observe_source_outcome(outcome);
    let (metrics, evidence) = sink.into_parts().expect("Codex evidence must publish");
    (evidence.evidence(), metrics)
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

    assert!(!capabilities.cache_write_tokens);
    assert!(!capabilities.skill_mcp_attribution);
    assert!(!capabilities.tool_definitions);
    assert!(!capabilities.fast_tier);
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
    let mut sink = CompositeSink::new(metrics, accumulator);
    let outcome = adapter_for("claude").visit(&input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    let (_, accumulator) = sink.into_parts().unwrap();
    let evidence = accumulator.evidence();

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
fn unresolved_fork_drops_inherited_usage_and_degrades_coverage() {
    let (coverage, reasons, streamed) = collect(&input("unresolved_fork"));
    let (evidence, metrics) = composite(&input("unresolved_fork"));

    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(
        reasons,
        BTreeSet::from([PartialReason::AttributionIncomplete])
    );
    assert!(streamed.events.is_empty());
    assert_eq!(metrics.metrics().tokens_in, 0);
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::AttributionIncomplete)
    );
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
