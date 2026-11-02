use antiburn_local::analysis::*;
use antiburn_local::insights::{
    CoverageCounts, DetectorId, EfficiencyReportAccumulator, ReportCatalogs, ReportContext,
    ReportWindow,
};
use antiburn_local::remediation::{FindingAssessment, assess_detector, remediation_prompt};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

struct Fixture {
    _directory: tempfile::TempDir,
    connection: Connection,
    input: SessionInput,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, title TEXT, time_created INTEGER, time_updated INTEGER);
            CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, data TEXT, time_created INTEGER, time_updated INTEGER);
            CREATE UNIQUE INDEX native_order ON session_message(session_id, seq);
            INSERT INTO session_v2 VALUES ('root', NULL, '/synthetic', '', 0, 0);").unwrap();
        Self {
            _directory: directory,
            connection,
            input: SessionInput {
                agent: "opencode".into(),
                session_id: "root".into(),
                source: RawSource::Sqlite(path),
                source_format: SourceFormat::OpenCodeSqliteV2,
                fork_parent_session_id: None,
            },
        }
    }
    fn message(&self, session: &str, id: &str, kind: &str, seq: i64, data: Value) {
        self.connection
            .execute(
                "INSERT INTO session_message VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
                params![id, session, kind, seq, data.to_string()],
            )
            .unwrap();
    }
    fn evidence(&self) -> SessionEvidence {
        let reader = reader_for_input(&self.input);
        let metrics = SessionMetricsAccumulator::new("opencode", "root");
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "opencode".into(),
            session_id: "root".into(),
            kind: SourceKind::from(&self.input.source),
            capabilities: reader.capabilities(&self.input),
        });
        let store = MemoryTurnRowStore::new("opencode", "root");
        let rows = TurnRowSink::new(store, "root", None);
        let mut sink = CompositeSink::with_turn_rows(metrics, evidence, rows);
        let outcome = reader.visit(&self.input, &mut sink).unwrap();
        sink.observe_source_outcome(outcome);
        assert!(!sink.turn_row_write_failed());
        sink.evidence().unwrap()
    }
}

fn assistant(model: &str, timestamp: i64, input: u64, cached: u64) -> Value {
    json!({"time":{"created":timestamp},"agent":"build","model":{"id":model,"providerID":"openai"},"tokens":{"input":input,"output":1,"reasoning":0,"cache":{"read":cached,"write":0}},"cost":0.0,"content":[]})
}

fn has_finding(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    matches!(
        assess_detector(detector, evidence, &ReportCatalogs::default()),
        FindingAssessment::Findings(_)
    )
}

fn assert_report_estimate(detector: DetectorId, evidence: &SessionEvidence) {
    let FindingAssessment::Findings(findings) =
        assess_detector(detector, evidence, &ReportCatalogs::default())
    else {
        panic!("native evidence must reach the production finding gate");
    };
    for finding in findings {
        assert_eq!(finding.source_format, SourceFormat::OpenCodeSqliteV2);
        let prompt = remediation_prompt(&finding).unwrap();
        assert!(prompt.as_str().contains("Native V2"));
        assert!(!prompt.as_str().contains("not CoreV2 session_message"));
        assert_eq!(prompt, remediation_prompt(&finding).unwrap());
    }
    let mut accumulator = EfficiencyReportAccumulator::with_catalogs(ReportCatalogs::default());
    accumulator.observe_session(evidence.clone());
    let report = accumulator.finish(ReportContext {
        environment_key: "native".into(),
        window: ReportWindow {
            start_epoch: 0,
            end_epoch: 2_000_000_010,
        },
        computed_at_epoch: 2_000_000_010,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage: CoverageCounts {
            ready: 1,
            discovered: 1,
            ..Default::default()
        },
    });
    let estimate = report.detector_estimated_token_burn_basis_points[detector.index()]
        .expect("a reachable native finding has a bounded report estimate");
    assert!((1..=10_000).contains(&estimate));
}

#[test]
fn native_depth_and_old_model_findings_require_observed_request_facts() {
    let fixture = Fixture::new();
    let now = 2_000_000_000_000_i64;
    fixture.message(
        "root",
        "a",
        "assistant",
        0,
        assistant(
            "gpt-5.5",
            now,
            ReportCatalogs::default().depth_cap_tokens + 1,
            0,
        ),
    );
    let evidence = fixture.evidence();
    for detector in DetectorId::ALL {
        assert!(!antiburn_local::insights::clean_facts_complete(
            detector, &evidence
        ));
    }
    assert!(
        has_finding(DetectorId::SessionsOverDepth, &evidence),
        "{evidence:#?}"
    );
    assert!(has_finding(DetectorId::OldModelUsage, &evidence));
    for detector in [DetectorId::SessionsOverDepth, DetectorId::OldModelUsage] {
        assert_report_estimate(detector, &evidence);
    }
    let unsafe_fixture = Fixture::new();
    let mut missing = assistant("gpt-5.5", now, 250_000, 0);
    missing.as_object_mut().unwrap().remove("tokens");
    missing.as_object_mut().unwrap().remove("model");
    unsafe_fixture.message("root", "missing", "assistant", 0, missing);
    let missing = unsafe_fixture.evidence();
    assert!(!has_finding(DetectorId::SessionsOverDepth, &missing));
    assert!(!has_finding(DetectorId::OldModelUsage, &missing));
    for detector in [DetectorId::ModelOverthinking, DetectorId::OveruseOfFastMode] {
        assert!(matches!(
            assess_detector(detector, &evidence, &ReportCatalogs::default()),
            FindingAssessment::NotApplicable | FindingAssessment::Unavailable(_)
        ));
    }
}

#[test]
fn native_subagent_finding_needs_exact_child_model_and_foreground_binding() {
    for bound in [true, false] {
        let fixture = Fixture::new();
        fixture
            .connection
            .execute(
                "INSERT INTO session_v2 VALUES ('child', 'root', '/synthetic', '', 0, 0)",
                [],
            )
            .unwrap();
        let mut parent = assistant("gpt-5.6-sol", 2_000_000_000_000, 100, 0);
        parent["content"] = json!([{"type":"tool","id":"delegate","name":"subagent","time":{"created":2_000_000_000_000_i64,"ran":2_000_000_000_001_i64,"completed":2_000_000_000_010_i64},"state":{"status":"completed","input":{"agent":"review","description":"Review changes","prompt":"Review the test change."},"metadata":{"sessionID":if bound { "child" } else { "other" },"status":"completed"},"content":[{"type":"text","text":"Reviewed."}]}}]);
        fixture.message("root", "parent", "assistant", 0, parent);
        fixture.message(
            "child",
            "worker",
            "assistant",
            0,
            assistant("gpt-5.6-sol", 2_000_000_000_005, 100, 0),
        );
        let evidence = fixture.evidence();
        assert_eq!(
            has_finding(DetectorId::OverpoweredSubagents, &evidence),
            bound,
            "{evidence:#?}"
        );
        if bound {
            assert_report_estimate(DetectorId::OverpoweredSubagents, &evidence);
        }
    }
}

#[test]
fn native_cache_finding_requires_a_same_route_expired_miss_and_recovery() {
    for recovered in [true, false] {
        let fixture = Fixture::new();
        let base = 2_000_000_000_000_i64;
        for (seq, time, input, cached) in [
            (0, base, 2_000, 8_000),
            (2, base + 1_801_000, 10_000, 0),
            (4, base + 1_803_000, 2_000, 8_000),
        ] {
            let mut step = assistant("gpt-5.6-sol", time, input, cached);
            if !recovered && seq == 4 {
                step["model"]["providerID"] = "custom".into();
            }
            fixture.message("root", &format!("a{seq}"), "assistant", seq, step);
            if seq < 4 {
                fixture.message("root", &format!("u{seq}"), "user", seq + 1, json!({"time":{"created":if seq == 0 { base + 1_800_000 } else { time + 1 }},"text":"Continue."}));
            }
        }
        let evidence = fixture.evidence();
        assert_eq!(
            has_finding(DetectorId::CacheChurn, &evidence),
            recovered,
            "{evidence:#?}"
        );
        if recovered {
            assert_report_estimate(DetectorId::CacheChurn, &evidence);
        }
    }
}
