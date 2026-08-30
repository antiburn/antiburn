//! End-to-end pipeline composition over the synthetic corpus tiers.
//!
//! Every scenario drives the full in-crate pipeline — framing → vendor
//! normalization → metrics + evidence accumulation → report reduction — and
//! asserts outcome shape (counts, coverage, bounded-memory contracts), never
//! timing. Timing lives in `benches/pipeline_baseline.rs` over the same
//! generator; keep the in-test tiers small so the suite stays fast.

#[path = "support/corpus.rs"]
mod corpus;

use std::fs::OpenOptions;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use antiburn_local::analysis::{
    ANALYZER_REVISION, AppendOnlyGuarantee, BoundedJsonlReader, ClaudeAdapter, CompositeSink,
    CoverageReason, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage, EvidenceSource, EvidenceValue,
    MAX_RECORD_BYTES, MemoryTurnRowStore, NormalizedRecord, PARSER_REVISION,
    RETAINED_METRICS_BYTES_BOUND, RawSource, RecordSink, SCAN_QUANTUM_BYTES, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SessionSummary,
    SourceCapabilities, SourceChangedReason, SourceClaim, SourceKind, TurnRowSink, TurnRowStore,
    VisitOutcome, adapter_for,
};
use antiburn_local::discovery::source_version::{FingerprintInputs, SourceStat, head_hash_of};
use antiburn_local::insights::{
    CoverageBucket, CoverageCounts, EfficiencyReportAccumulator, ReportContext, ReportWindow,
};
use corpus::{
    GeneratedSession, SessionSpec, generate_session, generate_session_of_bytes,
    generate_session_of_bytes_with_identity, write_provider_db,
};
use tempfile::TempDir;

#[test]
fn thread_identity_is_opt_in_and_chained() {
    let ordinary = generate_session(&SessionSpec::tier_s(7, 0, 3));
    assert!(!ordinary.jsonl.contains("\"uuid\""));
    assert!(!ordinary.jsonl.contains("\"parentUuid\""));

    let identified = generate_session_of_bytes_with_identity(7, 0, 1_024, None);
    let records = identified
        .jsonl
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("synthetic JSON"))
        .collect::<Vec<_>>();
    assert!(records.len() >= 2);
    assert!(records[0]["parentUuid"].is_null());
    assert_eq!(records[1]["parentUuid"], records[0]["uuid"]);
}

fn file_input(session_id: &str, path: &Path) -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: session_id.to_string(),
        source: RawSource::File(path.to_path_buf()),
    }
}

fn write_session(directory: &TempDir, session: &GeneratedSession) -> PathBuf {
    let path = directory
        .path()
        .join(format!("{}.jsonl", session.session_id));
    std::fs::write(&path, session.jsonl.as_bytes()).expect("write synthetic session");
    path
}

fn composite_for(input: &SessionInput) -> CompositeSink {
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
    CompositeSink::with_turn_rows(metrics, evidence, turn_rows)
}

/// Runs framing → normalization → metrics + evidence for one input.
fn run_pipeline(input: &SessionInput) -> CompositeSink {
    let mut composite = composite_for(input);
    let outcome = adapter_for("claude")
        .visit(input, &mut composite)
        .expect("synthetic source must stream");
    composite.observe_source_outcome(outcome);
    composite
}

fn observed<T>(value: &EvidenceValue<T>) -> &T {
    match value {
        EvidenceValue::Complete(observed) | EvidenceValue::Partial { observed, .. } => observed,
        EvidenceValue::Unsupported => panic!("evidence group must be observed"),
    }
}

fn report_context(ready_sessions: u64) -> ReportContext {
    let mut coverage = CoverageCounts::default();
    coverage.observe(CoverageBucket::Ready, ready_sessions);
    ReportContext {
        environment_key: "synthetic".to_owned(),
        window: ReportWindow {
            start_epoch: 1_770_000_000,
            end_epoch: 1_772_600_000,
        },
        computed_at_epoch: 1_772_600_000,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage,
    }
}

fn claim_for_path(path: &Path) -> SourceClaim {
    let file = std::fs::File::open(path).expect("open source for claim");
    let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
    let bytes = std::fs::read(path).expect("read source for claim");
    SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&bytes)),
    })
}

#[test]
fn s_tier_session_flows_end_to_end_into_a_report() {
    let directory = TempDir::new().expect("tempdir");
    let session = generate_session(&SessionSpec::tier_s(11, 0, 500));
    assert_eq!(session.tallies.total_records, 500);
    assert!(session.tallies.compaction_boundaries >= 2);
    let path = write_session(&directory, &session);
    let input = file_input(&session.session_id, &path);

    let composite = run_pipeline(&input);
    let evidence = composite.evidence().expect("clean session must publish");
    let metrics = composite.metrics().expect("clean session must publish");

    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert_eq!(evidence.diagnostics.records_unusable, 0);
    let eligibility = observed(&evidence.eligibility);
    assert_eq!(
        eligibility.assistant_turns,
        session.tallies.assistant_records as u64
    );
    assert!(eligibility.turns >= eligibility.assistant_turns);
    assert!(evidence.diagnostics.records_observed >= session.tallies.assistant_records as u64);
    assert!(evidence.diagnostics.records_observed <= session.tallies.total_records as u64);
    assert_eq!(metrics.session_id, session.session_id);
    // Assistant and user records become events; so does each compaction
    // boundary (it carries a compaction marker through the metrics path).
    assert_eq!(
        metrics.event_count,
        session.tallies.assistant_records
            + session.tallies.user_records
            + session.tallies.compaction_boundaries
    );
    assert!(metrics.tokens_in > 0 && metrics.tokens_out > 0);

    let mut report = EfficiencyReportAccumulator::new();
    report.observe_session(evidence);
    let report = report.finish(report_context(1));
    assert_eq!(report.assessed_sessions, 1);
    assert!(report.coverage_reasons.is_empty());
    assert!(report.context.coverage.is_consistent());
}

#[test]
fn oversized_line_is_bounded_and_neighbours_survive() {
    let directory = TempDir::new().expect("tempdir");
    let mut spec = SessionSpec::tier_s(13, 0, 120);
    spec.oversized_at = Some(60);
    spec.oversized_bytes = MAX_RECORD_BYTES + 4096;
    let session = generate_session(&spec);
    assert_eq!(session.tallies.oversized_records, 1);
    let path = write_session(&directory, &session);
    let input = file_input(&session.session_id, &path);

    // The framing layer never retains more than the record bound.
    let mut reader = BoundedJsonlReader::new(Cursor::new(session.jsonl.as_bytes()));
    while reader.next_record(&|| false).is_some() {}
    assert!(reader.retained_record_bytes_high_water() <= MAX_RECORD_BYTES);

    let composite = run_pipeline(&input);
    let evidence = composite
        .evidence()
        .expect("an oversized line degrades coverage, not publication");
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::Oversized)
    );
    assert_eq!(evidence.diagnostics.records_unusable, 1);
    let eligibility = observed(&evidence.eligibility);
    assert_eq!(
        eligibility.assistant_turns,
        session.tallies.assistant_records as u64
    );
}

#[test]
fn large_session_respects_bounded_memory_contracts() {
    let directory = TempDir::new().expect("tempdir");
    let session = generate_session_of_bytes(17, 0, 3 * 1024 * 1024);
    assert!(session.jsonl.len() >= 3 * 1024 * 1024);
    let path = write_session(&directory, &session);
    let input = file_input(&session.session_id, &path);

    let mut reader = BoundedJsonlReader::new(Cursor::new(session.jsonl.as_bytes()));
    while reader.next_record(&|| false).is_some() {}
    assert!(reader.retained_record_bytes_high_water() <= SCAN_QUANTUM_BYTES * 4);

    let composite = run_pipeline(&input);
    let evidence = composite
        .evidence()
        .expect("large clean session must publish");
    let (metrics, _evidence_accumulator) = composite
        .into_parts()
        .expect("large clean session must publish");
    assert!(metrics.observed_turns() > 0);
    assert!(
        metrics.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND,
        "retained {} bytes after {} turns",
        metrics.retained_bytes(),
        metrics.observed_turns()
    );
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
}

#[test]
fn m_tier_report_reduction_covers_every_session() {
    let directory = TempDir::new().expect("tempdir");
    const SESSIONS: usize = 24;
    let mut report = EfficiencyReportAccumulator::new();
    for session_index in 0..SESSIONS {
        let session = generate_session(&SessionSpec::tier_s(19, session_index, 80));
        let path = write_session(&directory, &session);
        let input = file_input(&session.session_id, &path);
        let composite = run_pipeline(&input);
        let evidence = composite.evidence().expect("clean session must publish");
        assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
        report.observe_session(evidence);
    }
    let report = report.finish(report_context(SESSIONS as u64));
    assert_eq!(report.assessed_sessions, SESSIONS as u64);
    assert!(report.coverage_reasons.is_empty());
    for counts in report.detectors {
        assert!(counts.eligible <= SESSIONS as u64);
        assert!(counts.assessed <= counts.eligible);
    }
}

#[test]
fn roster_parent_and_subagent_children_compose() {
    let directory = TempDir::new().expect("tempdir");
    let mut parent_spec = SessionSpec::tier_s(23, 0, 60);
    parent_spec.task_spawns = 3;
    let parent = generate_session(&parent_spec);
    assert_eq!(parent.tallies.task_spawns, 3);

    let mut evidences: Vec<SessionEvidence> = Vec::new();
    let parent_path = write_session(&directory, &parent);
    let parent_evidence = run_pipeline(&file_input(&parent.session_id, &parent_path))
        .evidence()
        .expect("parent must publish");
    let subagents = observed(&parent_evidence.subagents).clone();
    assert_eq!(subagents.spawn_count, 3);
    evidences.push(parent_evidence);

    for child_index in 1..=3 {
        let mut child_spec = SessionSpec::tier_s(23, child_index, 30);
        child_spec.delegated = true;
        let child = generate_session(&child_spec);
        let child_path = write_session(&directory, &child);
        let child_evidence = run_pipeline(&file_input(&child.session_id, &child_path))
            .evidence()
            .expect("child must publish");
        let child_subagents = observed(&child_evidence.subagents);
        assert_eq!(
            child_subagents.delegated_turns,
            child.tallies.assistant_records as u64
        );
        evidences.push(child_evidence);
    }

    let mut report = EfficiencyReportAccumulator::new();
    let cohort = evidences.len() as u64;
    for evidence in evidences {
        report.observe_session(evidence);
    }
    let report = report.finish(report_context(cohort));
    assert_eq!(report.assessed_sessions, cohort);
}

#[test]
fn housekeeping_tail_with_inert_unrecognized_types_keeps_coverage() {
    let directory = TempDir::new().expect("tempdir");
    let mut spec = SessionSpec::tier_s(29, 0, 200);
    spec.unrecognized_every = Some(25);
    let session = generate_session(&spec);
    assert!(session.tallies.unrecognized_records > 0);
    let path = write_session(&directory, &session);

    let composite = run_pipeline(&file_input(&session.session_id, &path));
    let evidence = composite
        .evidence()
        .expect("inert unrecognized housekeeping must publish");
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert!(
        evidence
            .diagnostics
            .unrecognized_types
            .iter()
            .any(|discriminator| discriminator == "relay_probe" || discriminator == "shelf_audit")
    );
    assert_eq!(
        evidence.diagnostics.records_unrecognized_inert,
        session.tallies.unrecognized_records as u64
    );
    let eligibility = observed(&evidence.eligibility);
    assert_eq!(
        eligibility.assistant_turns,
        session.tallies.assistant_records as u64
    );
}

#[test]
fn evidence_bearing_unrecognized_types_degrade_coverage() {
    let directory = TempDir::new().expect("tempdir");
    let mut spec = SessionSpec::tier_s(31, 0, 200);
    spec.evidence_bearing_unrecognized_every = Some(25);
    let session = generate_session(&spec);
    assert!(session.tallies.evidence_bearing_unrecognized_records > 0);
    let path = write_session(&directory, &session);

    let evidence = run_pipeline(&file_input(&session.session_id, &path))
        .evidence()
        .expect("evidence-bearing unknowns degrade coverage, not publication");
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
    );
    assert_eq!(
        evidence.diagnostics.records_unusable,
        session.tallies.evidence_bearing_unrecognized_records as u64
    );
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 0);
}

#[test]
fn provider_db_backed_source_flows_end_to_end_into_a_report() {
    let directory = TempDir::new().expect("tempdir");
    let session = generate_session(&SessionSpec::tier_s(37, 0, 300));
    let db_path = directory.path().join("provider.db");
    write_provider_db(&db_path, &session).expect("write synthetic provider DB");

    // A raw provider DB streams through the native OpenCode adapter.
    let input = SessionInput {
        agent: "opencode".to_string(),
        session_id: session.session_id.clone(),
        source: RawSource::Sqlite(db_path),
    };
    let mut composite = composite_for(&input);
    let outcome = adapter_for(&input.agent)
        .visit(&input, &mut composite)
        .expect("synthetic provider DB must be readable");
    assert_eq!(outcome, VisitOutcome::Unvalidated);
    composite.observe_source_outcome(outcome);

    let evidence = composite
        .evidence()
        .expect("provider-DB session must publish");
    let metrics = composite
        .metrics()
        .expect("provider-DB session must publish");
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    let eligibility = observed(&evidence.eligibility);
    // The synthetic conversion keeps every conversational record.
    assert_eq!(
        eligibility.assistant_turns,
        session.tallies.assistant_records as u64
    );
    assert!(eligibility.turns >= eligibility.assistant_turns);
    assert_eq!(evidence.diagnostics.records_unusable, 0);
    assert_eq!(metrics.session_id, session.session_id);
    assert!(metrics.event_count >= session.tallies.assistant_records);
    assert!(metrics.tokens_in > 0 && metrics.tokens_out > 0);

    let mut report = EfficiencyReportAccumulator::new();
    report.observe_session(evidence);
    let report = report.finish(report_context(1));
    assert_eq!(report.assessed_sessions, 1);
    assert!(report.coverage_reasons.is_empty());
}

/// Forwards to a composite sink and appends to the source file mid-read,
/// like an agent still writing its transcript.
struct AppendingSink {
    inner: CompositeSink,
    path: PathBuf,
    append_after_records: usize,
    seen: usize,
    appended: bool,
}

impl RecordSink for AppendingSink {
    fn record(&mut self, record: NormalizedRecord) {
        self.seen += 1;
        if !self.appended && self.seen >= self.append_after_records {
            self.appended = true;
            let mut file = OpenOptions::new()
                .append(true)
                .open(&self.path)
                .expect("open source for mid-read append");
            file.write_all(
                b"{\"type\":\"assistant\",\"timestamp\":1770009999,\"message\":{\"id\":\"msg-writer-1\",\"role\":\"assistant\",\"model\":\"claude-3-5-haiku-20241022\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3},\"content\":[{\"type\":\"text\",\"text\":\"Appended synthetic turn.\"}]}}\n",
            )
            .expect("append to source");
        }
        self.inner.record(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.inner.finish(summary);
    }
}

#[test]
fn an_active_writer_forces_source_changed_and_nothing_publishes() {
    let directory = TempDir::new().expect("tempdir");
    let session = generate_session(&SessionSpec::tier_s(31, 0, 400));
    let path = write_session(&directory, &session);
    let input = file_input(&session.session_id, &path);
    let claim = claim_for_path(&path);

    let mut sink = AppendingSink {
        inner: composite_for(&input),
        path: path.clone(),
        append_after_records: 20,
        seen: 0,
        appended: false,
    };
    let outcome = ClaudeAdapter
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut sink,
        )
        .expect("actively-written source must stream");
    assert!(sink.appended, "the synthetic writer must have appended");
    assert_eq!(
        outcome,
        VisitOutcome::SourceChanged(SourceChangedReason::FingerprintMismatch)
    );
    sink.inner.observe_source_outcome(outcome);
    assert!(sink.inner.metrics().is_none(), "no projection may publish");
    assert!(sink.inner.evidence().is_none(), "no projection may publish");

    // Control: once the writer is quiet, the same full-reprocess claim passes.
    let claim = claim_for_path(&path);
    let mut composite = composite_for(&input);
    let outcome = ClaudeAdapter
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut composite,
        )
        .expect("quiescent source must stream");
    assert_eq!(outcome, VisitOutcome::AcceptedFull);
    composite.observe_source_outcome(outcome);
    assert!(composite.metrics().is_some());
    assert!(composite.evidence().is_some());
}
