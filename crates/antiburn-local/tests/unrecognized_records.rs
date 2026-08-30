#[path = "support/claude_fixture.rs"]
mod claude_fixture;

use antiburn_local::analysis::{
    ANALYZER_REVISION, CoverageReason, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage, EvidenceValue,
    PARSER_REVISION,
};
use antiburn_local::insights::{
    BadgeStatus, CoverageCounts, DetectorId, DetectorStatus, EfficiencyReportAccumulator,
    NotAssessedReason, ReportCatalogs, ReportContext, ReportWindow, session_badges,
};
use claude_fixture::{stream_composite, stream_source};

fn report_context() -> ReportContext {
    ReportContext {
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
    }
}

#[test]
fn an_inert_unknown_session_enters_the_denominator_and_is_assessed() {
    let composite = stream_composite("unrecognized_inert_records");
    let metrics = composite.metrics().expect("metrics must publish");
    let evidence = composite.evidence().expect("evidence must publish");

    assert_eq!(metrics.event_count, 2);
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 3);
    assert_eq!(evidence.diagnostics.records_unusable, 0);
    assert_eq!(evidence.diagnostics.unrecognized_types.len(), 3);

    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence);
    let report = accumulator.finish(report_context());
    let counts = report.detectors[DetectorId::ModelOverthinking.index()];
    assert_eq!(counts.eligible, 1);
    assert_eq!(counts.assessed, 1);
    // The session carries zero assistant turns, so zero turns are
    // eligible to carry an effort value: the signal reads as missing.
    assert_eq!(
        report.detector_statuses[DetectorId::ModelOverthinking.index()],
        DetectorStatus::NotAssessed(NotAssessedReason::SignalMissing)
    );
    assert_eq!(report.unrecognized_records.sessions_with_types, 1);
    assert_eq!(report.unrecognized_records.inert_sessions, 1);

    for badge in session_badges(
        &composite
            .evidence()
            .expect("evidence must remain available"),
        &ReportCatalogs::default(),
    ) {
        assert_ne!(
            badge.status,
            BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
    }
}

#[test]
fn an_unrecognized_role_with_usage_fails_closed_and_keeps_neighbours() {
    let composite = stream_composite("unrecognized_role_with_usage");
    let metrics = composite.metrics().expect("metrics must publish");
    let evidence = composite.evidence().expect("evidence must publish");

    assert_eq!(metrics.event_count, 2);
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
    );
    assert_eq!(evidence.diagnostics.records_observed, 3);
    assert_eq!(evidence.diagnostics.records_unusable, 1);
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 0);
    assert!(matches!(
        evidence.models,
        EvidenceValue::Partial {
            reason: CoverageReason::UnrecognizedRecordType,
            ..
        }
    ));

    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence);
    let report = accumulator.finish(report_context());
    let counts = report.detectors[DetectorId::ModelOverthinking.index()];
    assert_eq!(counts.eligible, 1);
    assert_eq!(counts.assessed, 0);
    // The session carries zero assistant turns, so zero turns are
    // eligible to carry an effort value: the signal reads as missing.
    assert_eq!(
        report.detector_statuses[DetectorId::ModelOverthinking.index()],
        DetectorStatus::NotAssessed(NotAssessedReason::SignalMissing)
    );
}

#[test]
fn every_unknown_evidence_shape_fails_closed_including_an_allowlisted_name() {
    let composite = stream_composite("unrecognized_evidence_shapes");
    let metrics = composite.metrics().expect("metrics must publish");
    let evidence = composite.evidence().expect("evidence must publish");

    assert_eq!(metrics.event_count, 1);
    assert_eq!(evidence.diagnostics.records_unusable, 6);
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 0);
    assert!(
        evidence
            .diagnostics
            .unrecognized_types
            .contains("cost-state")
    );
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
    );
}

#[test]
fn large_and_deep_allowlisted_records_keep_complete_coverage() {
    let large = serde_json::json!({
        "type": "file-history-snapshot",
        "files": (0..500)
            .map(|index| serde_json::json!({"path": format!("file-{index}")}))
            .collect::<Vec<_>>()
    });
    let deep = serde_json::json!({
        "type": "summary",
        "payload": {"a": {"b": {"c": {"d": {"e": {"f": {"g": {"h": {}}}}}}}}}
    });
    let source = format!("{large}\n{deep}\n");
    let evidence = stream_source("large-allowlisted", source)
        .evidence()
        .expect("evidence must publish");

    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert!(evidence.diagnostics.unrecognized_types.is_empty());
    assert_eq!(evidence.diagnostics.records_unusable, 0);
}

#[test]
fn an_unknown_command_marker_fails_closed() {
    let source = concat!(
        r#"{"type":"user","message":{"role":"user","content":"Base directory for this skill: /tmp/skills/orbit-tracker/SKILL.md"}}"#,
        "\n",
        r#"{"type":"slash_v2","text":"<command-name>/orbit-tracker</command-name>"}"#,
        "\n",
    )
    .to_owned();
    let evidence = stream_source("unknown-command", source)
        .evidence()
        .expect("evidence must publish");

    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
    );
    assert!(evidence.diagnostics.unrecognized_types.contains("slash_v2"));
}

#[test]
fn an_allowlisted_command_echo_keeps_complete_coverage() {
    let source = concat!(
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/review</command-name>"}}"#,
        "\n",
        r#"{"type":"last-prompt","prompt":"<command-name>/review</command-name>"}"#,
        "\n",
        r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet","usage":{"input_tokens":3,"output_tokens":2}}}"#,
        "\n",
    )
    .to_owned();
    let evidence = stream_source("allowlisted-command-echo", source)
        .evidence()
        .expect("evidence must publish");

    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert!(evidence.diagnostics.unrecognized_types.is_empty());
    assert_eq!(evidence.diagnostics.records_unusable, 0);
}

#[test]
fn an_allowlisted_nested_scalar_key_keeps_complete_coverage() {
    let source = concat!(
        r#"{"type":"attachment","attachment":{"config":{"model":"display-only"}}}"#,
        "\n",
        r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet","usage":{"input_tokens":3,"output_tokens":2}}}"#,
        "\n",
    )
    .to_owned();
    let evidence = stream_source("allowlisted-nested-key", source)
        .evidence()
        .expect("evidence must publish");

    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert!(evidence.diagnostics.unrecognized_types.is_empty());
    assert_eq!(evidence.diagnostics.records_unusable, 0);
}

#[test]
fn missing_and_non_object_records_are_auditable_and_fail_closed_as_needed() {
    let source = "{\"timestamp\":1,\"payload\":{\"ok\":true}}\n[]\n".to_owned();
    let evidence = stream_source("missing-types", source)
        .evidence()
        .expect("evidence must publish");

    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
    );
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 1);
    assert_eq!(evidence.diagnostics.records_unusable, 1);
    assert!(
        evidence
            .diagnostics
            .unrecognized_types
            .contains("<missing>")
    );
}

#[test]
fn a_capped_unknown_session_is_eligible_but_not_assessed() {
    let mut source = String::new();
    for index in 0..17 {
        source.push_str(&format!(
            "{{\"type\":\"inert-{index:02}\",\"timestamp\":{index},\"payload\":{{\"ok\":true}}}}\n"
        ));
    }
    let evidence = stream_source("capped-inert", source)
        .evidence()
        .expect("evidence must publish");
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 17);

    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence);
    let report = accumulator.finish(report_context());
    let counts = report.detectors[DetectorId::ModelOverthinking.index()];
    assert_eq!(counts.eligible, 1);
    assert_eq!(counts.assessed, 0);
    assert_eq!(report.unrecognized_records.capped_sessions, 1);
    assert_eq!(report.unrecognized_records.truncated_sessions, 0);
    assert!(report.unrecognized_records.types_truncated);
    // The session carries zero assistant turns, so zero turns are
    // eligible to carry an effort value: the signal reads as missing.
    assert_eq!(
        report.detector_statuses[DetectorId::ModelOverthinking.index()],
        DetectorStatus::NotAssessed(NotAssessedReason::SignalMissing)
    );
}

#[test]
fn an_overlong_type_is_truncated_without_claiming_set_overflow() {
    let record = serde_json::json!({
        "type": "x".repeat(300),
        "timestamp": 1,
        "payload": {"ok": true}
    });
    let evidence = stream_source("truncated-type", format!("{record}\n"))
        .evidence()
        .expect("evidence must publish");
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );

    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence);
    let report = accumulator.finish(report_context());
    let counts = report.detectors[DetectorId::ModelOverthinking.index()];
    assert_eq!(counts.eligible, 1);
    assert_eq!(counts.assessed, 0);
    // The session carries zero assistant turns, so zero turns are
    // eligible to carry an effort value: the signal reads as missing.
    assert_eq!(
        report.detector_statuses[DetectorId::ModelOverthinking.index()],
        DetectorStatus::NotAssessed(NotAssessedReason::SignalMissing)
    );

    let summary = report.unrecognized_records;
    assert_eq!(summary.types.len(), 1);
    assert_eq!(summary.capped_sessions, 0);
    assert_eq!(summary.truncated_sessions, 1);
    assert!(!summary.types_truncated);
}

/// Allowlisted housekeeping-only sessions already use this zero-work arithmetic.
#[test]
fn a_session_of_only_inert_unknowns_is_a_zero_work_session() {
    let evidence = stream_composite("unrecognized_inert_sidechain")
        .evidence()
        .expect("evidence must publish");
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    let EvidenceValue::Complete(eligibility) = &evidence.eligibility else {
        panic!("eligibility must be complete");
    };
    assert_eq!(eligibility.turns, 0);

    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence);
    let report = accumulator.finish(report_context());
    for detector in [
        DetectorId::SessionsOverDepth,
        DetectorId::ModelOverthinking,
        DetectorId::OverpoweredSubagents,
        DetectorId::OldModelUsage,
        DetectorId::OveruseOfFastMode,
        DetectorId::CacheChurn,
    ] {
        assert_eq!(
            report.detectors[detector.index()].eligible,
            1,
            "{detector:?}"
        );
        assert_eq!(
            report.detectors[detector.index()].assessed,
            1,
            "{detector:?}"
        );
    }
    for detector in [DetectorId::UnusedMcpServers, DetectorId::UnusedSkills] {
        assert_eq!(report.detectors[detector.index()].eligible, 0);
    }
    // The inert sidechain marker never becomes a turn row, so it carries no
    // delegated turn or spawn: a real clean verdict, not a contract gap.
    assert_eq!(
        report.detector_statuses[DetectorId::OverpoweredSubagents.index()],
        DetectorStatus::Clean
    );
}
