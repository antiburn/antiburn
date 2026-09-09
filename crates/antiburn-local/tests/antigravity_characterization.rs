use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceCoverage, EvidenceSource, EvidenceValue, MemoryTurnRowStore, RawSource,
    SessionEvidence, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SourceFormat, SourceKind, TurnRowSink, TurnRowStore, reader_for,
};
use antiburn_local::insights::{
    CoverageCounts, DetectorCounts, DetectorId, EfficiencyReportAccumulator, ModelRegistry,
    ModelReplacementEntry, ReportCatalogs, ReportContext, ReportWindow,
};
use rusqlite::{Connection, params};

fn input(source: RawSource) -> SessionInput {
    SessionInput {
        agent: "antigravity".into(),
        session_id: "synthetic".into(),
        source,
        fork_parent_session_id: None,
    }
}

fn evidence(input: &SessionInput) -> SessionEvidence {
    let reader = reader_for("antigravity");
    let store = MemoryTurnRowStore::new("antigravity", "synthetic");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("antigravity", "synthetic"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(&input.source),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            "synthetic",
            None,
        ),
    );
    let outcome = reader.visit(input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    sink.evidence().unwrap()
}

fn old_model_report(evidence: SessionEvidence) -> DetectorCounts {
    let mut report = EfficiencyReportAccumulator::with_catalogs(ReportCatalogs {
        model_replacements: ModelRegistry {
            revision: 1,
            entries: [(
                "old-model".into(),
                ModelReplacementEntry {
                    replacement: "new-model".into(),
                    available_since_ts_ms: 100,
                    rationale: "Synthetic replacement".into(),
                    source_url: "https://example.invalid/model".into(),
                },
            )]
            .into(),
        },
        ..ReportCatalogs::default()
    });
    report.observe_session(evidence);
    report
        .finish(ReportContext {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: 2_000_000_000,
            },
            computed_at_epoch: 2_000_000_000,
            parser_revision: 1,
            analyzer_revision: 1,
            evidence_schema_revision: 1,
            coverage: CoverageCounts::default(),
        })
        .detectors[DetectorId::OldModelUsage.index()]
}

#[test]
fn brain_and_cascade_keep_direct_findings_with_unknown_steps() {
    let known = r#"{"type":"PLANNER_RESPONSE","model":"old-model","created_at":1000}"#;
    for unknown in [
        r#"{"type":"FUTURE"}"#,
        r#"{"type":"FUTURE","content":"PRIVATE"}"#,
    ] {
        for content in [
            format!("{known}\n{unknown}"),
            format!("{{\"steps\":{{\"steps\":[{known},{unknown}]}}}}"),
        ] {
            let evidence = evidence(&input(RawSource::Jsonl(content)));
            assert!(matches!(evidence.coverage, EvidenceCoverage::Partial(_)));
            assert_eq!(old_model_report(evidence).finding, 1);
        }
    }
    let evidence = evidence(&input(RawSource::Jsonl(format!("{known}\n{{broken"))));
    assert!(matches!(evidence.coverage, EvidenceCoverage::Partial(_)));
    assert_eq!(old_model_report(evidence).finding, 1);
}

#[test]
fn brain_and_cascade_do_not_inherit_a_previous_step_model() {
    let steps = [
        r#"{"type":"PLANNER_RESPONSE","model":"old-model","created_at":1000}"#,
        r#"{"type":"PLANNER_RESPONSE","created_at":2000}"#,
        r#"{"type":"PLANNER_RESPONSE","model":"old-model"}"#,
    ];
    for content in [
        steps.join("\n"),
        format!("{{\"steps\":[{}]}}", steps.join(",")),
    ] {
        let input = input(RawSource::Jsonl(content));
        let session = reader_for("antigravity").normalize(&input).unwrap();
        assert_eq!(session.events[1].model, None);
        assert_eq!(session.events[2].ts_ms, None);
        let evidence = evidence(&input);
        assert!(matches!(evidence.models, EvidenceValue::Partial { .. }));
        assert!(matches!(evidence.time_range, EvidenceValue::Partial { .. }));
        assert_eq!(old_model_report(evidence).finding, 1);
    }
}

fn database(
    companion: bool,
    missing_model: bool,
    missing_time: bool,
    malformed: bool,
) -> (tempfile::TempDir, SessionInput) {
    let directory = tempfile::tempdir().unwrap();
    let conversations = directory.path().join("conversations");
    std::fs::create_dir(&conversations).unwrap();
    if companion {
        let logs = directory
            .path()
            .join("brain/synthetic/.system_generated/logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("transcript.jsonl"),
            "{\"source\":\"antigravity_brain\",\"model\":\"old-model\"}\n{\"type\":\"USER_INPUT\",\"created_at\":1000}\n").unwrap();
    }
    let path = conversations.join("synthetic.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE steps (idx INTEGER, metadata BLOB); CREATE TABLE gen_metadata (idx INTEGER, data BLOB);").unwrap();
    let usage = [0x10, 10, 0x18, 2, 0x3a, 1, b'a'];
    let mut step = Vec::new();
    if !missing_time {
        step.extend_from_slice(&[0x0a, 3, 0x08, 0xe8, 7]);
    }
    step.extend_from_slice(&[0x4a, usage.len() as u8]);
    step.extend_from_slice(&usage);
    connection
        .execute("INSERT INTO steps VALUES (0, ?1)", params![step])
        .unwrap();
    if !missing_model {
        let mut chat = vec![0x22, usage.len() as u8];
        chat.extend_from_slice(&usage);
        chat.extend_from_slice(&[0x9a, 1, 9]);
        chat.extend_from_slice(b"old-model");
        let mut generation = vec![0x0a, chat.len() as u8];
        generation.extend(chat);
        connection
            .execute(
                "INSERT INTO gen_metadata VALUES (0, ?1)",
                params![generation],
            )
            .unwrap();
    }
    if malformed {
        connection.execute_batch("INSERT INTO steps VALUES (1, X'80'); INSERT INTO steps VALUES (2, NULL); INSERT INTO gen_metadata VALUES (1, X'80');").unwrap();
    }
    (directory, input(RawSource::Sqlite(path)))
}

#[test]
fn database_and_companion_do_not_fabricate_model_or_time_completeness() {
    for companion in [false, true] {
        for (missing_model, missing_time, malformed) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let (_directory, input) = database(companion, missing_model, missing_time, malformed);
            let evidence = evidence(&input);
            assert_eq!(
                evidence.capabilities.source_format,
                SourceFormat::AntigravitySqlite
            );
            if missing_model {
                assert!(matches!(evidence.models, EvidenceValue::Partial { .. }));
            }
            if missing_time {
                assert!(matches!(evidence.time_range, EvidenceValue::Partial { .. }));
            }
            if malformed {
                assert!(matches!(evidence.coverage, EvidenceCoverage::Partial(_)));
            }
            let report = old_model_report(evidence);
            assert_eq!(report.finding, u64::from(!missing_model && !missing_time));
            if missing_model || missing_time || malformed {
                assert_eq!(report.clean, 0);
            }
        }
    }
}

#[test]
fn companion_parse_gaps_do_not_hide_database_findings() {
    for suffix in ["{broken\n", "{\"type\":\"FUTURE\"}\n"] {
        let (directory, input) = database(true, false, false, false);
        let path = directory
            .path()
            .join("brain/synthetic/.system_generated/logs/transcript.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, format!("{content}{suffix}")).unwrap();
        let evidence = evidence(&input);
        assert!(matches!(evidence.coverage, EvidenceCoverage::Partial(_)));
        let report = old_model_report(evidence);
        assert_eq!(report.finding, 1);
        assert_eq!(report.clean, 0);
    }
}
