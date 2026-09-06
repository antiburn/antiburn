use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use antiburn_local::analysis::{
    AppendOnlyGuarantee, CompositeSink, EvidenceCoverage, EvidenceSource, EvidenceValue,
    MAX_RECORD_BYTES, MemoryTurnRowStore, NormalizedRecord, NormalizedSession, PartialReason,
    PiAdapter, RawSource, RecordCoverage, RecordSink, SessionCollector, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SessionSummary,
    SourceCapabilities, SourceClaim, SourceKind, TurnFacts, TurnRowSink, TurnRowStore,
    VendorAdapter, VisitOutcome, adapter_for, analyze_session, append_only_guarantee,
};
use antiburn_local::discovery::source_version::{
    FINGERPRINT_HEAD_BYTES, FingerprintInputs, SourceStat, head_hash_of,
};
use antiburn_local::insights::{
    BadgeId, BadgeStatus, DetectorId, NotAssessedReason, ReportCatalogs, eligible, session_badges,
};
use rusqlite::params;
use serde_json::{Value, json};

#[path = "support/pricing.rs"]
mod pricing;

fn fixture(name: &str) -> &'static str {
    pricing::install();
    match name {
        "minimal_session" => include_str!("fixtures/pi_characterization/minimal_session.jsonl"),
        "role_ordering" => include_str!("fixtures/pi_characterization/role_ordering.jsonl"),
        "content_blocks" => include_str!("fixtures/pi_characterization/content_blocks.jsonl"),
        "usage_all_buckets" => {
            include_str!("fixtures/pi_characterization/usage_all_buckets.jsonl")
        }
        "usage_subset_keys" => {
            include_str!("fixtures/pi_characterization/usage_subset_keys.jsonl")
        }
        "model_change" => include_str!("fixtures/pi_characterization/model_change.jsonl"),
        "thinking_level_change" => {
            include_str!("fixtures/pi_characterization/thinking_level_change.jsonl")
        }
        "compaction_and_inert" => {
            include_str!("fixtures/pi_characterization/compaction_and_inert.jsonl")
        }
        "unknown_row_type" => {
            include_str!("fixtures/pi_characterization/unknown_row_type.jsonl")
        }
        "unknown_content_block" => {
            include_str!("fixtures/pi_characterization/unknown_content_block.jsonl")
        }
        "custom_rows" => include_str!("fixtures/pi_characterization/custom_rows.jsonl"),
        "malformed_middle" => {
            include_str!("fixtures/pi_characterization/malformed_middle.jsonl")
        }
        "incomplete_final_record" => {
            include_str!("fixtures/pi_characterization/incomplete_final_record.jsonl")
        }
        "header_only" => include_str!("fixtures/pi_characterization/header_only.jsonl"),
        "unsupported_version" => {
            include_str!("fixtures/pi_characterization/unsupported_version.jsonl")
        }
        "fork_hazard_parent" => {
            include_str!("fixtures/pi_characterization/fork_hazard_parent.jsonl")
        }
        "fork_hazard_child" => {
            include_str!("fixtures/pi_characterization/fork_hazard_child.jsonl")
        }
        "fork_no_inherited" => {
            include_str!("fixtures/pi_characterization/fork_no_inherited.jsonl")
        }
        "timestamp_disagreement" => {
            include_str!("fixtures/pi_characterization/timestamp_disagreement.jsonl")
        }
        "image_block" => include_str!("fixtures/pi_characterization/image_block.jsonl"),
        "bash_execution_role" => {
            include_str!("fixtures/pi_characterization/bash_execution_role.jsonl")
        }
        "bash_execution_with_usage" => {
            include_str!("fixtures/pi_characterization/bash_execution_with_usage.jsonl")
        }
        "mixed_api" => include_str!("fixtures/pi_characterization/mixed_api.jsonl"),
        "skill_tool_privacy" => {
            include_str!("fixtures/pi_characterization/skill_tool_privacy.jsonl")
        }
        "inert_signal_guard" => {
            include_str!("fixtures/pi_characterization/inert_signal_guard.jsonl")
        }
        "non_turn_timestamp_ordering" => {
            include_str!("fixtures/pi_characterization/non_turn_timestamp_ordering.jsonl")
        }
        "session_start" => include_str!("fixtures/pi_characterization/session_start.jsonl"),
        "headerless_tools" => {
            include_str!("fixtures/pi_characterization/headerless_tools.jsonl")
        }
        "headerless_usage" => {
            include_str!("fixtures/pi_characterization/headerless_usage.jsonl")
        }
        "thread_chain_through_non_message_rows" => {
            include_str!("fixtures/pi_characterization/thread_chain_through_non_message_rows.jsonl")
        }
        "fork_child_one_thread" => {
            include_str!("fixtures/pi_characterization/fork_child_one_thread.jsonl")
        }
        "unresolved_parent_link" => {
            include_str!("fixtures/pi_characterization/unresolved_parent_link.jsonl")
        }
        "message_without_id" => {
            include_str!("fixtures/pi_characterization/message_without_id.jsonl")
        }
        "session_overdepth_finding" => {
            include_str!("fixtures/pi_characterization/session_overdepth_finding.jsonl")
        }
        "model_overthinking_finding" => {
            include_str!("fixtures/pi_characterization/model_overthinking_finding.jsonl")
        }
        "excess_cache_rehydration_finding" => {
            include_str!("fixtures/pi_characterization/excess_cache_rehydration_finding.jsonl")
        }
        _ => panic!("unknown Pi characterization fixture: {name}"),
    }
}

fn fixture_names() -> [&'static str; 35] {
    [
        "minimal_session",
        "role_ordering",
        "content_blocks",
        "usage_all_buckets",
        "usage_subset_keys",
        "model_change",
        "thinking_level_change",
        "compaction_and_inert",
        "unknown_row_type",
        "unknown_content_block",
        "custom_rows",
        "malformed_middle",
        "header_only",
        "unsupported_version",
        "fork_hazard_parent",
        "fork_hazard_child",
        "fork_no_inherited",
        "timestamp_disagreement",
        "image_block",
        "bash_execution_role",
        "bash_execution_with_usage",
        "mixed_api",
        "skill_tool_privacy",
        "inert_signal_guard",
        "non_turn_timestamp_ordering",
        "session_start",
        "headerless_tools",
        "headerless_usage",
        "thread_chain_through_non_message_rows",
        "fork_child_one_thread",
        "unresolved_parent_link",
        "message_without_id",
        "session_overdepth_finding",
        "model_overthinking_finding",
        "excess_cache_rehydration_finding",
    ]
}

fn input(name: &str) -> SessionInput {
    SessionInput {
        agent: "pi".to_owned(),
        session_id: name.to_owned(),
        source: RawSource::Jsonl(fixture(name).to_owned()),
        fork_parent_session_id: None,
    }
}

fn collect(input: &SessionInput) -> (RecordCoverage, BTreeSet<PartialReason>, NormalizedSession) {
    let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
    PiAdapter
        .visit(input, &mut collector)
        .expect("Pi fixture must stream");
    let coverage = collector.coverage();
    let reasons = collector.partial_reasons().clone();
    let session = collector.into_session().expect("Pi stream must finish");
    (coverage, reasons, session)
}

fn composite(input: &SessionInput) -> (SessionEvidence, SessionMetricsAccumulator) {
    composite_with(input, SourceCapabilities::pi(), &PiAdapter)
}

fn composite_with(
    input: &SessionInput,
    capabilities: SourceCapabilities,
    adapter: &dyn VendorAdapter,
) -> (SessionEvidence, SessionMetricsAccumulator) {
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities,
    });
    let store = MemoryTurnRowStore::new(input.agent.clone(), input.session_id.clone());
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter
        .visit(input, &mut sink)
        .expect("Pi fixture must stream into both sinks");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("Pi evidence must publish");
    let (metrics, _evidence_accumulator) = sink.into_parts().expect("Pi metrics must publish");
    (evidence, metrics)
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/pi_characterization/goldens"
    ))
    .join(format!("{name}.json"))
}

fn check_golden(name: &str) {
    let input = input(name);
    let (evidence, metrics) = composite(&input);
    let (_, reasons, normalized) = collect(&input);
    let mut metrics = serde_json::to_value(metrics.metrics()).unwrap();
    metrics
        .as_object_mut()
        .expect("metrics serialize as an object")
        .remove("cost");
    let actual = json!({
        "normalizedSession": normalized,
        "partialReasons": reasons.into_iter().map(|reason| format!("{reason:?}")).collect::<Vec<_>>(),
        "metrics": metrics,
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

fn is_supported<T>(value: &EvidenceValue<T>) -> bool {
    !matches!(value, EvidenceValue::Unsupported)
}

#[derive(Default)]
struct SummarySink {
    summary: Option<SessionSummary>,
}

impl RecordSink for SummarySink {
    fn record(&mut self, _record: NormalizedRecord) {}

    fn finish(&mut self, summary: SessionSummary) {
        self.summary = Some(summary);
    }
}

fn summary(name: &str) -> SessionSummary {
    let mut sink = SummarySink::default();
    PiAdapter.visit(&input(name), &mut sink).unwrap();
    sink.summary.expect("Pi stream must finish")
}

#[test]
fn pi_registry_uses_the_dedicated_adapter() {
    assert_eq!(adapter_for("pi").agent(), "pi");
    assert_eq!(adapter_for("PI").agent(), "pi");
}

#[test]
fn streaming_and_batch_normalization_match_for_every_fixture() {
    for name in fixture_names() {
        let input = input(name);
        let parsed = PiAdapter.normalize(&input).expect("fixture must normalize");
        let (_, _, streamed) = collect(&input);
        assert_eq!(streamed, parsed, "normalized fixture {name}");
    }
}

#[test]
fn minimal_roles_tools_and_thinking_are_preserved() {
    let (_, _, minimal) = collect(&input("minimal_session"));
    assert_eq!(minimal.events.len(), 2);
    assert_eq!(minimal.events[0].role, antiburn_local::analysis::Role::User);
    assert_eq!(
        minimal.events[1].role,
        antiburn_local::analysis::Role::Assistant
    );
    assert_eq!(minimal.events[1].usage.context_tokens(), 14);

    let (_, _, ordered) = collect(&input("role_ordering"));
    assert_eq!(
        ordered
            .events
            .iter()
            .map(|event| event.role)
            .collect::<Vec<_>>(),
        vec![
            antiburn_local::analysis::Role::User,
            antiburn_local::analysis::Role::Assistant,
            antiburn_local::analysis::Role::Tool,
        ]
    );

    let (_, _, content) = collect(&input("content_blocks"));
    assert!(content.events[0].has_thinking);
    assert_eq!(
        content.events[0]
            .tools
            .iter()
            .map(|tool| tool.category)
            .collect::<Vec<_>>(),
        vec![
            antiburn_local::analysis::ToolCategory::Read,
            antiburn_local::analysis::ToolCategory::Edit,
            antiburn_local::analysis::ToolCategory::Test,
        ]
    );
    assert_eq!(content.events[1].role, antiburn_local::analysis::Role::Tool);
}

#[test]
fn usage_buckets_are_disjoint_and_context_uses_all_input_classes() {
    let (_, _, usage) = collect(&input("usage_all_buckets"));
    assert_eq!(usage.events[0].usage.input_tokens, 2);
    assert_eq!(usage.events[0].usage.output_tokens, 3);
    assert_eq!(usage.events[0].usage.cache_read_tokens, 5);
    assert_eq!(usage.events[0].usage.cache_creation_tokens, 7);
    assert_eq!(usage.events[0].usage.context_tokens(), 14);
    assert_eq!(usage.events[1].usage.context_tokens(), 16);
    assert_eq!(analyze_session(&usage).peak_context_tokens, 16);

    let (_, _, subset) = collect(&input("usage_subset_keys"));
    assert_eq!(subset.events[0].speed, None);
    let usage = subset.events[0].usage;
    assert_eq!(usage.input_tokens, 2);
    assert_eq!(usage.output_tokens, 8);
    assert_eq!(usage.cache_read_tokens, 5);
    assert_eq!(usage.cache_creation_tokens, 7);
    assert_eq!(
        usage.input_tokens
            + usage.output_tokens
            + usage.cache_read_tokens
            + usage.cache_creation_tokens,
        22
    );
    let (evidence, _) = composite(&input("usage_subset_keys"));
    let EvidenceValue::Complete(models) = evidence.models else {
        panic!("Pi model evidence must be complete");
    };
    assert!(models.fast_modes.is_empty());
}

#[test]
fn models_thinking_levels_and_compaction_preserve_transitions() {
    let (_, _, models) = collect(&input("model_change"));
    assert_eq!(models.events[0].model.as_deref(), Some("model-a"));
    assert_eq!(models.events[1].model.as_deref(), Some("model-c"));

    let (_, _, levels) = collect(&input("thinking_level_change"));
    assert_eq!(levels.events[0].thinking_mode.as_deref(), Some("low"));
    assert_eq!(levels.events[1].thinking_mode.as_deref(), Some("high"));

    let (coverage, reasons, compacted) = collect(&input("compaction_and_inert"));
    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert_eq!(compacted.events.len(), 1);
    assert!(compacted.events[0].is_compaction_boundary);
    assert_eq!(compacted.events[0].compaction_pre_tokens, Some(100));
    assert_eq!(compacted.events[0].compaction_post_tokens, None);
    assert_eq!(compacted.events[0].compaction_trigger, None);
}

#[test]
fn recognition_is_strict_without_persisting_extension_discriminators() {
    let (coverage, reasons, _) = collect(&input("unknown_row_type"));
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(
        reasons,
        BTreeSet::from([PartialReason::UnrecognizedRecordType])
    );
    let (evidence, _) = composite(&input("unknown_row_type"));
    assert_eq!(
        evidence.diagnostics.unrecognized_types,
        BTreeSet::from(["future_row".to_owned()])
    );

    let (evidence, _) = composite(&input("unknown_content_block"));
    assert_eq!(
        evidence.diagnostics.unrecognized_types,
        BTreeSet::from([
            "futureBlock".to_owned(),
            "futureBlockThree".to_owned(),
            "futureBlockTwo".to_owned(),
        ])
    );
    assert_eq!(evidence.diagnostics.records_unusable, 1);
    let EvidenceValue::Partial { observed, .. } = evidence.models else {
        panic!("the unknown block must retain readable usage as partial evidence");
    };
    assert_eq!(observed.by_model["model-a"].input, 1);

    let (evidence, _) = composite(&input("custom_rows"));
    assert!(evidence.diagnostics.unrecognized_types.is_empty());
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    let serialized = serde_json::to_string(&evidence).unwrap();
    for private in ["extension-one", "extension-two", "extension-three"] {
        assert!(!serialized.contains(private));
    }
}

#[test]
fn every_recognized_inert_family_fails_closed_on_hidden_signals() {
    let (coverage, reasons, session) = collect(&input("inert_signal_guard"));
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(
        reasons,
        BTreeSet::from([PartialReason::UnrecognizedRecordType])
    );
    assert_eq!(session.events.len(), 1);
    assert_eq!(session.events[0].usage.input_tokens, 1);

    let (evidence, metrics) = composite(&input("inert_signal_guard"));
    assert_eq!(
        evidence.diagnostics.unrecognized_types,
        BTreeSet::from([
            "custom".to_owned(),
            "custom_message".to_owned(),
            "session".to_owned(),
            "session_info".to_owned(),
        ])
    );
    assert_eq!(evidence.diagnostics.records_unusable, 7);
    assert_eq!(metrics.metrics().billable_input_tokens, 1);
    assert!(
        !serde_json::to_string(&evidence)
            .unwrap()
            .contains("synthetic-hidden-tool")
    );
}

#[test]
fn skill_arguments_never_become_pi_tool_or_skill_evidence() {
    let (evidence, metrics) = composite(&input("skill_tool_privacy"));
    let EvidenceValue::Complete(tools) = evidence.tools else {
        panic!("Pi tool evidence must be complete");
    };
    assert!(tools.by_name.contains_key("Skill"));
    assert!(!tools.by_name.contains_key("synthetic-private-skill-marker"));
    assert_eq!(metrics.metrics().skill_uses[0].name, "skill");
}

#[test]
fn conditional_inert_roles_fail_closed_when_they_carry_usage() {
    let (coverage, reasons, session) = collect(&input("bash_execution_role"));
    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert!(session.events.is_empty());

    let (coverage, reasons, session) = collect(&input("bash_execution_with_usage"));
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(
        reasons,
        BTreeSet::from([PartialReason::UnrecognizedRecordType])
    );
    assert!(session.events.is_empty());
    let (evidence, metrics) = composite(&input("bash_execution_with_usage"));
    assert_eq!(
        evidence.diagnostics.unrecognized_types,
        BTreeSet::from(["bashExecution".to_owned()])
    );
    assert_eq!(metrics.metrics().billable_input_tokens, 0);
}

#[test]
fn malformed_incomplete_unsupported_and_header_only_sources_are_honest() {
    let (coverage, reasons, session) = collect(&input("malformed_middle"));
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(reasons, BTreeSet::from([PartialReason::MalformedRecord]));
    assert_eq!(session.events.len(), 2);

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("active.jsonl");
    fs::write(&path, fixture("incomplete_final_record")).unwrap();
    let file_input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "active".to_owned(),
        source: RawSource::File(path),
        fork_parent_session_id: None,
    };
    let (coverage, reasons, session) = collect(&file_input);
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(reasons, BTreeSet::from([PartialReason::IncompleteTail]));
    assert_eq!(session.events.len(), 1);

    let (coverage, reasons, session) = collect(&input("unsupported_version"));
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(
        reasons,
        BTreeSet::from([PartialReason::UnrecognizedRecordType])
    );
    assert_eq!(session.events.len(), 1);

    let (coverage, reasons, session) = collect(&input("header_only"));
    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert!(session.events.is_empty());
}

#[test]
fn missing_required_fields_degrade_without_discarding_valid_rows() {
    let input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "missing-required-fields".to_owned(),
        source: RawSource::Jsonl(
            concat!(
                r#"{"type":"model_change","timestamp":"2026-01-01T00:00:00Z"}"#,
                "\n",
                r#"{"type":"session_info","name":"synthetic title"}"#,
                "\n",
                r#"{"type":"message","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","model":"model-a","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0},"content":[]}}"#,
                "\n"
            )
            .to_owned(),
        ),
        fork_parent_session_id: None,
    };
    let (coverage, reasons, session) = collect(&input);
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(reasons, BTreeSet::from([PartialReason::MalformedRecord]));
    assert_eq!(session.events.len(), 1);
}

#[test]
fn oversized_records_are_skipped_and_valid_neighbours_remain() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized.jsonl");
    let before = fixture("minimal_session").lines().nth(1).unwrap();
    let after = fixture("minimal_session").lines().nth(2).unwrap();
    let mut file = fs::File::create(&path).unwrap();
    writeln!(file, "{before}").unwrap();
    file.write_all(&vec![b'x'; MAX_RECORD_BYTES + 1]).unwrap();
    writeln!(file).unwrap();
    writeln!(file, "{after}").unwrap();
    drop(file);

    let file_input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "oversized".to_owned(),
        source: RawSource::File(path),
        fork_parent_session_id: None,
    };
    let (coverage, reasons, session) = collect(&file_input);
    assert_eq!(coverage, RecordCoverage::Partial);
    assert_eq!(reasons, BTreeSet::from([PartialReason::Oversized]));
    assert_eq!(session.events.len(), 2);
}

#[test]
fn claimed_reads_accept_stable_files_and_reject_changes_without_finishing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claimed.jsonl");
    fs::write(&path, fixture("minimal_session")).unwrap();
    let claim = source_claim(&path);
    let input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "claimed".to_owned(),
        source: RawSource::File(path.clone()),
        fork_parent_session_id: None,
    };
    let mut collector = SessionCollector::new("pi", "claimed");
    let outcome = PiAdapter
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut collector,
        )
        .unwrap();
    assert_eq!(outcome, VisitOutcome::AcceptedFull);
    assert_eq!(collector.into_session().unwrap().events.len(), 2);

    let changed_path = directory.path().join("changed.jsonl");
    fs::write(&changed_path, fixture("minimal_session")).unwrap();
    let changed_claim = source_claim(&changed_path);
    fs::OpenOptions::new()
        .append(true)
        .open(&changed_path)
        .unwrap()
        .write_all(b"{}\n")
        .unwrap();
    let changed_input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "changed".to_owned(),
        source: RawSource::File(changed_path),
        fork_parent_session_id: None,
    };
    let mut collector = SessionCollector::new("pi", "changed");
    let outcome = PiAdapter
        .visit_claimed(
            &changed_input,
            &changed_claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut collector,
        )
        .unwrap();
    assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));
    assert_eq!(append_only_guarantee("pi"), AppendOnlyGuarantee::Absent);
    assert_eq!(
        collector.into_session().unwrap_err().to_string(),
        "record stream ended without a session summary"
    );
}

#[test]
fn claimed_reads_reject_short_or_replaced_sources_without_finishing() {
    let directory = tempfile::tempdir().unwrap();

    let short_path = directory.path().join("short.jsonl");
    fs::write(&short_path, fixture("minimal_session")).unwrap();
    let short_claim = source_claim(&short_path);
    fs::write(&short_path, "{}\n").unwrap();
    let short_input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "short".to_owned(),
        source: RawSource::File(short_path),
        fork_parent_session_id: None,
    };
    let mut short_collector = SessionCollector::new("pi", "short");
    let outcome = PiAdapter
        .visit_claimed(
            &short_input,
            &short_claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut short_collector,
        )
        .unwrap();
    assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));
    assert!(short_collector.into_session().is_err());

    let replaced_path = directory.path().join("replaced.jsonl");
    fs::write(&replaced_path, fixture("minimal_session")).unwrap();
    let replaced_claim = source_claim(&replaced_path);
    fs::remove_file(&replaced_path).unwrap();
    fs::write(&replaced_path, fixture("header_only")).unwrap();
    let replaced_input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "replaced".to_owned(),
        source: RawSource::File(replaced_path),
        fork_parent_session_id: None,
    };
    let mut replaced_collector = SessionCollector::new("pi", "replaced");
    let outcome = PiAdapter
        .visit_claimed(
            &replaced_input,
            &replaced_claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut replaced_collector,
        )
        .unwrap();
    assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));
    assert!(replaced_collector.into_session().is_err());
}

#[test]
fn claimed_reads_honor_cancellation_without_publishing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cancelled.jsonl");
    fs::write(&path, fixture("minimal_session")).unwrap();
    let claim = source_claim(&path);
    let input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "synthetic-private-session-marker".to_owned(),
        source: RawSource::File(path),
        fork_parent_session_id: None,
    };
    let mut collector = SessionCollector::new("pi", "cancelled");
    let error = PiAdapter
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| true,
            &mut collector,
        )
        .unwrap_err();
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string().contains("record 0 read was cancelled"))
    );
    assert!(!format!("{error:#}").contains("synthetic-private-session-marker"));
    assert!(collector.into_session().is_err());
}

#[test]
fn fork_ownership_drops_inherited_usage_without_persisting_parent_paths() {
    let (_, parent_metrics) = composite(&input("fork_hazard_parent"));
    let (child_evidence, child_metrics) = composite(&input("fork_hazard_child"));
    assert_eq!(parent_metrics.metrics().billable_input_tokens, 1);
    assert_eq!(child_metrics.metrics().billable_input_tokens, 2);
    assert_eq!(
        parent_metrics.metrics().billable_input_tokens
            + child_metrics.metrics().billable_input_tokens,
        3
    );
    assert_eq!(child_evidence.diagnostics.records_observed, 3);
    let EvidenceValue::Complete(child_time) = &child_evidence.time_range else {
        panic!("child time range must be complete");
    };
    // The row-derived time range spans only real turns. "shared-row"
    // is dropped as inherited, so the child's first turn is its own
    // "child-row" at 00:00:11, not the session header's 00:00:10.
    assert_eq!(child_time.first_ts_ms, 1_767_312_011_000);
    assert!(!child_evidence.capabilities.subagent_relationships);
    assert!(matches!(
        child_evidence.subagents,
        EvidenceValue::Unsupported
    ));
    assert!(
        !serde_json::to_string(&child_evidence)
            .unwrap()
            .contains("pi-parent")
    );
}

#[test]
fn fork_without_inherited_rows_keeps_the_normal_preamble_and_usage() {
    let (coverage, reasons, session) = collect(&input("fork_no_inherited"));
    assert_eq!(coverage, RecordCoverage::Complete);
    assert!(reasons.is_empty());
    assert_eq!(session.events.len(), 1);
    assert_eq!(session.events[0].usage.input_tokens, 3);
    assert_eq!(session.events[0].thinking_mode.as_deref(), Some("medium"));
}

#[test]
fn unresolved_fork_ownership_fails_closed_without_guessing() {
    for (name, source) in [
        (
            "missing-row-timestamp",
            concat!(
                r#"{"type":"session","version":3,"timestamp":"2026-01-01T00:00:00Z","parentSession":"/synthetic/parent.jsonl"}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","model":"model-a","usage":{"input":9,"output":8,"cacheRead":7,"cacheWrite":6},"content":[]}}"#,
                "\n"
            ),
        ),
        (
            "malformed-header-timestamp",
            concat!(
                r#"{"type":"session","version":3,"timestamp":"not-a-time","parentSession":"/synthetic/parent.jsonl"}"#,
                "\n",
                r#"{"type":"message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","model":"model-a","usage":{"input":9,"output":8,"cacheRead":7,"cacheWrite":6},"content":[]}}"#,
                "\n"
            ),
        ),
    ] {
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: name.to_owned(),
            source: RawSource::Jsonl(source.to_owned()),
            fork_parent_session_id: None,
        };
        let (coverage, reasons, session) = collect(&input);
        assert_eq!(coverage, RecordCoverage::Partial, "coverage for {name}");
        assert!(
            reasons.contains(&PartialReason::AttributionIncomplete),
            "attribution reason for {name}"
        );
        assert!(session.events.is_empty(), "owned events for {name}");
    }
}

#[test]
fn top_level_timestamps_and_session_start_are_authoritative() {
    let (_, _, session) = collect(&input("timestamp_disagreement"));
    assert!(session.events[0].ts_ms < session.events[1].ts_ms);

    let (evidence, _) = composite(&input("non_turn_timestamp_ordering"));
    assert_eq!(
        evidence.provenance.ordering,
        antiburn_local::analysis::OrderingObservation::OutOfOrder
    );
    let EvidenceValue::Complete(time_range) = evidence.time_range else {
        panic!("Pi time-range evidence must be complete");
    };
    // The row-derived time range spans only real turns: the earlier
    // `model_change` and `session_info` markers carry top-level
    // timestamps but are not turns, so the first turn's own 00:00:02
    // is authoritative, not their earlier 00:00:00.
    assert_eq!(time_range.first_ts_ms, 1_767_225_602_000);
    assert_eq!(time_range.last_ts_ms, 1_767_225_603_000);

    let (_, metrics) = composite(&input("session_start"));
    assert_eq!(
        summary("session_start").started_at_ms,
        Some(1_767_225_600_000)
    );
    assert_eq!(metrics.earliest_ts_ms(), Some(1_767_225_605_000));
}

#[test]
fn headerless_synthetic_inputs_keep_complete_metrics() {
    let (_, tools) = composite(&input("headerless_tools"));
    assert_eq!(summary("headerless_tools").started_at_ms, None);
    assert_eq!(tools.metrics().event_count, 5);
    assert_eq!(tools.metrics().tool_calls_by_name.values().sum::<u32>(), 3);

    let (evidence, usage) = composite(&input("headerless_usage"));
    assert_eq!(summary("headerless_usage").started_at_ms, None);
    let metrics = usage.metrics();
    assert_eq!(metrics.billable_input_tokens, 3);
    assert_eq!(metrics.billable_output_tokens, 5);
    assert_eq!(metrics.billable_cache_read_tokens, 7);
    assert_eq!(metrics.billable_cache_creation_tokens, 11);
    assert_eq!(metrics.peak_context_tokens, 21);
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
}

#[test]
fn image_and_other_private_payloads_do_not_reach_evidence() {
    for name in fixture_names() {
        let (evidence, metrics) = composite(&input(name));
        let serialized = serde_json::to_string(&json!({
            "evidence": evidence,
            "metrics": metrics.metrics(),
        }))
        .unwrap();
        for private in [
            "/synthetic",
            "src/lib.rs",
            "extension-one",
            "extension-two",
            "extension-three",
            "extension-guard",
            "synthetic-private-skill-marker",
            "/synthetic/private/SKILL.md",
            "/synthetic-private-command",
            "synthetic-hidden-tool",
            "synthetic-provider",
            "synthetic command",
            "synthetic output",
            "synthetic summary",
            "synthetic title",
            "c3ludGhldGlj",
            "image/png",
            "call-1",
            "row-1",
            "row-2",
            "shared-row",
            "child-row",
        ] {
            assert!(
                !serialized.contains(private),
                "fixture {name} persisted a private marker"
            );
        }
    }
}

#[test]
fn pi_capabilities_match_published_evidence_and_session_cache_support() {
    let expected = SourceCapabilities {
        request_context_tokens: true,
        cache_write_tokens: true,
        timestamps_and_order: true,
        tool_invocations: true,
        skill_mcp_attribution: false,
        tool_definitions: false,
        model_identity: true,
        token_classes: true,
        reasoning_effort_tier: true,
        fast_tier: false,
        service_tier: false,
        subagent_relationships: false,
        subagent_models: false,
        compaction_boundaries: true,
        thread_identity: true,
        record_identity: true,
        linear_record_order: false,
        quota_incidents: false,
        harness_version: false,
    };
    assert_eq!(SourceCapabilities::pi(), expected);

    for name in fixture_names() {
        let (evidence, _) = composite(&input(name));
        let expected_cache = name != "mixed_api";
        assert_eq!(
            evidence.capabilities.cache_write_tokens, expected_cache,
            "cache support for {name}"
        );
        assert!(is_supported(&evidence.context));
        assert!(is_supported(&evidence.time_range));
        assert!(is_supported(&evidence.tools));
        assert!(is_supported(&evidence.models));
        assert!(is_supported(&evidence.cache));
        assert!(is_supported(&evidence.compactions));
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
    }
}

#[test]
fn pi_badges_follow_the_merged_session_coverage_policy() {
    let (complete, _) = composite(&input("minimal_session"));
    let complete_badges = session_badges(&complete, &ReportCatalogs::default());
    assert_eq!(complete_badges.map(|badge| badge.id), BadgeId::ALL);
    assert_eq!(
        complete_badges.map(|badge| badge.status),
        [
            // Thread identity now qualifies session-overdepth for Pi, and
            // this fixture's depth stays under the cap.
            BadgeStatus::Clean,
            // The fixture's assistant turns carry no effort value, so
            // zero eligible turns means the effort signal is missing.
            BadgeStatus::NotAssessed(NotAssessedReason::SignalMissing),
            BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing),
            // The reviewed production registry is non-empty, and this
            // fixture observes zero models, so no catalogued model can
            // have run.
            BadgeStatus::Clean,
            BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing),
            // Record identity now qualifies cache churn for Pi, and this
            // fixture shows no churn.
            BadgeStatus::Clean,
        ]
    );

    let (partial, _) = composite(&input("unknown_row_type"));
    let partial_badges = session_badges(&partial, &ReportCatalogs::default());
    assert_eq!(partial_badges.map(|badge| badge.id), BadgeId::ALL);
    assert_eq!(
        partial_badges.map(|badge| badge.status),
        [
            // Thread identity now qualifies session-overdepth for Pi, but
            // the unrecognized row leaves session coverage short of
            // Complete, so the badge stays unassessed.
            BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
            // The fixture's assistant turns carry no effort value, so
            // zero eligible turns means the effort signal is missing.
            BadgeStatus::NotAssessed(NotAssessedReason::SignalMissing),
            BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing),
            // The reviewed production registry is non-empty, but the
            // same incomplete session coverage keeps this unassessed.
            BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
            BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing),
            // Record identity now qualifies cache churn for Pi, but the
            // same incomplete coverage keeps it unassessed too.
            BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
        ]
    );
}

#[test]
fn pi_detector_eligibility_is_frozen() {
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "pi".to_owned(),
        session_id: "pi-prerequisites".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::pi(),
    })
    .evidence(&TurnFacts::default());
    let assessed = DetectorId::ALL
        .into_iter()
        .filter(|detector| eligible(*detector, &evidence))
        .collect::<Vec<_>>();
    assert_eq!(
        assessed,
        vec![
            DetectorId::SessionsOverDepth,
            DetectorId::ModelOverthinking,
            DetectorId::OldModelUsage,
            DetectorId::CacheChurn,
        ]
    );
}

/// The generic fallback claims no cache-write support for any vendor. It uses
/// uncached-input accounting. This matches Pi when Pi reports no write support.
#[test]
fn provider_cache_miss_behavior_matches_the_generic_fallback_without_cache_writes() {
    for name in fixture_names() {
        if summary(name).cache_write_tokens_available {
            continue;
        }
        let input = input(name);
        let (_, pi_metrics) = composite(&input);
        let generic = adapter_for("pi-generic-fallback");
        let (_, generic_metrics) = composite_with(&input, SourceCapabilities::pi(), generic);
        assert_eq!(
            pi_metrics.metrics().cache_rehydration_count,
            generic_metrics.metrics().cache_rehydration_count,
            "cache rehydrations for {name}"
        );
        assert_eq!(
            pi_metrics.metrics().cache_routing_miss_count,
            generic_metrics.metrics().cache_routing_miss_count,
            "provider cache misses for {name}"
        );
    }

    // `mixed_api` reports cache-write support unavailable because its two
    // API families disagree on whether they report cache writes at all. Pi
    // and the generic fallback both fall back to uncached-input accounting
    // and read the same provider cache miss count from it.
    let (evidence, pi_metrics) = composite(&input("mixed_api"));
    assert!(!evidence.capabilities.cache_write_tokens);
    assert_eq!(pi_metrics.metrics().cache_rehydration_count, 0);
    assert_eq!(pi_metrics.metrics().cache_routing_miss_count, 1);
    let (_, generic_metrics) = composite_with(
        &input("mixed_api"),
        SourceCapabilities::pi(),
        adapter_for("pi-generic-fallback"),
    );
    assert_eq!(generic_metrics.metrics().cache_rehydration_count, 0);
    assert_eq!(generic_metrics.metrics().cache_routing_miss_count, 1);
}

/// Like [`composite`], with the backing [`MemoryTurnRowStore`] returned
/// alongside the published evidence, so a test can read `thread_id` straight
/// off the rows the pass wrote — the same way `opencode_characterization.rs`
/// reads `turn_identities` off its own row store.
fn composite_with_store(input: &SessionInput) -> (SessionEvidence, Arc<MemoryTurnRowStore>) {
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::pi(),
    });
    let store = MemoryTurnRowStore::new(input.agent.clone(), input.session_id.clone());
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = PiAdapter
        .visit(input, &mut sink)
        .expect("Pi fixture must stream into both sinks");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("Pi evidence must publish");
    (evidence, store)
}

/// Reads back every row's `thread_id`, in turn order, straight off
/// [`MemoryTurnRowStore`]'s in-memory database.
fn turn_thread_ids(store: &MemoryTurnRowStore, session_id: &str) -> Vec<String> {
    store.with_connection(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT thread_id FROM turn
                  WHERE environment_key = 'native' AND agent = 'pi'
                    AND session_id = ?1 AND claim_fence = 1
                  ORDER BY turn_index",
            )
            .expect("prepare");
        statement
            .query_map(params![session_id], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    })
}

/// Reads the value out of an `EvidenceValue`, for `Complete` and `Partial`
/// alike. Panics on `Unsupported`.
fn observed<T: Clone>(value: &EvidenceValue<T>) -> T {
    match value {
        EvidenceValue::Complete(observed) | EvidenceValue::Partial { observed, .. } => {
            observed.clone()
        }
        EvidenceValue::Unsupported => panic!("evidence group must be supported"),
    }
}

#[test]
fn a_message_chained_through_non_message_rows_stays_one_thread() {
    let input = input("thread_chain_through_non_message_rows");
    let (evidence, store) = composite_with_store(&input);

    let thread_ids = turn_thread_ids(&store, "thread_chain_through_non_message_rows");
    // Five rows become turns: the user prompt, the two assistant turns
    // (model A then model B), the compaction boundary, and the final
    // assistant turn. All five share one thread — a `model_change` or
    // `thinking_level_change` row between them never starts a new one.
    assert_eq!(thread_ids.len(), 5);
    assert!(
        thread_ids.iter().all(|id| id == &thread_ids[0]),
        "every row must share one thread: {thread_ids:?}"
    );

    assert!(matches!(evidence.cache, EvidenceValue::Complete(_)));
    let cache = observed(&evidence.cache);
    assert!(matches!(cache.previous_turn, EvidenceValue::Complete(())));
    assert_eq!(cache.model_transitions.len(), 1);
    assert_eq!(cache.model_transitions[0].from_model, "model-a");
    assert_eq!(cache.model_transitions[0].to_model, "model-b");

    assert!(matches!(evidence.compactions, EvidenceValue::Complete(_)));
    assert_eq!(observed(&evidence.compactions).boundaries.len(), 1);
}

#[test]
fn a_fork_child_with_no_owned_root_still_reads_as_one_thread() {
    let input = input("fork_child_one_thread");
    let (evidence, store) = composite_with_store(&input);

    let thread_ids = turn_thread_ids(&store, "fork_child_one_thread");
    // Only the two owned rows become turns; the three copied, pre-fork rows
    // are inherited and dropped. Both owned rows still read as one thread,
    // continuing the parent's (dropped) chain.
    assert_eq!(thread_ids.len(), 2);
    assert_eq!(thread_ids[0], thread_ids[1]);

    let EvidenceValue::Complete(models) = &evidence.models else {
        panic!("models evidence must be complete");
    };
    // The inherited assistant turn's usage (input/output/cache all 9) never
    // counts; only the owned assistant turn's usage (2 in, 3 out) does.
    let totals = &models.by_model["model-a"];
    assert_eq!(totals.input, 2);
    assert_eq!(totals.output, 3);
    assert!(matches!(evidence.cache, EvidenceValue::Complete(_)));
}

#[test]
fn an_unresolved_parent_link_degrades_without_panicking_or_looping() {
    let (evidence, _) = composite(&input("unresolved_parent_link"));

    let EvidenceValue::Partial {
        observed: cache, ..
    } = &evidence.cache
    else {
        panic!("cache evidence must be partial: {:?}", evidence.cache);
    };
    assert!(matches!(
        cache.previous_turn,
        EvidenceValue::Partial {
            reason: antiburn_local::analysis::CoverageReason::AttributionIncomplete,
            ..
        }
    ));
}

#[test]
fn a_message_without_an_id_still_counts_its_tokens_but_loses_thread_identity() {
    let (evidence, metrics) = composite(&input("message_without_id"));

    assert!(matches!(
        evidence.cache,
        EvidenceValue::Partial {
            reason: antiburn_local::analysis::CoverageReason::AttributionIncomplete,
            ..
        }
    ));
    assert_eq!(metrics.metrics().billable_input_tokens, 3);
    assert_eq!(metrics.metrics().billable_output_tokens, 4);
}

#[test]
fn selected_pi_fixtures_match_reviewable_goldens() {
    for name in [
        "content_blocks",
        "usage_all_buckets",
        "unknown_row_type",
        "unknown_content_block",
        "session_overdepth_finding",
        "model_overthinking_finding",
        "excess_cache_rehydration_finding",
    ] {
        check_golden(name);
    }
}
