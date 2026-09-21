use std::sync::Arc;

use antiburn_local::analysis::{
    AppendOnlyGuarantee, CompositeSink, ContentKind, EvidenceCoverage, EvidenceSource,
    EvidenceValue, MemoryTurnRowStore, NormalizedEvent, NormalizedRecord, PartialReason, RawSource,
    RecordSink, SessionCollector, SessionEvidence, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SessionSummary, SourceClaim, SourceFormat, SourceKind, TurnContent,
    TurnRowSink, TurnRowStore, reader_for,
};
use antiburn_local::discovery::source_version::{FingerprintInputs, SourceStat, head_hash_of};
use antiburn_local::insights::{
    CoverageCounts, DetectorCounts, DetectorId, EfficiencyReportAccumulator, ModelRegistry,
    ModelReplacementEntry, ReportCatalogs, ReportContext, ReportWindow,
};

fn input(source: RawSource) -> SessionInput {
    SessionInput {
        agent: "cursor".into(),
        session_id: "synthetic".into(),
        source,
        fork_parent_session_id: None,
        source_format: Default::default(),
    }
}

fn evidence(input: &SessionInput) -> SessionEvidence {
    let reader = reader_for("cursor");
    let store = MemoryTurnRowStore::new("cursor", "synthetic");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("cursor", "synthetic"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(input),
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
    let catalogs = ReportCatalogs {
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
    };
    let mut report = EfficiencyReportAccumulator::with_catalogs(catalogs);
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
fn direct_model_findings_survive_unknown_and_malformed_records() {
    for suffix in ["", "\n{broken", "\n{\"type\":\"future\"}"] {
        let input = input(RawSource::Jsonl(format!(
            "{{\"role\":\"assistant\",\"model\":\"old-model\",\"timestamp\":1000}}{suffix}"
        )));
        let evidence = evidence(&input);
        assert_eq!(
            matches!(evidence.coverage, EvidenceCoverage::Complete),
            suffix.is_empty()
        );
        assert_eq!(old_model_report(evidence).finding, 1);
    }
}

#[test]
fn missing_model_and_time_are_not_filled_from_other_records() {
    for missing in [
        r#"{"role":"assistant","timestamp":2000}"#,
        r#"{"role":"assistant","model":"old-model"}"#,
    ] {
        let input = input(RawSource::Jsonl(format!(
            "{{\"role\":\"assistant\",\"model\":\"old-model\",\"timestamp\":1000}}\n{missing}"
        )));
        let session = reader_for("cursor").normalize(&input).unwrap();
        let evidence = evidence(&input);
        if missing.contains("model") {
            assert_eq!(session.events[1].ts_ms, None);
            assert!(matches!(evidence.time_range, EvidenceValue::Partial { .. }));
        } else {
            assert_eq!(session.events[1].model, None);
            assert!(matches!(evidence.models, EvidenceValue::Partial { .. }));
        }
        assert_eq!(old_model_report(evidence).finding, 1);
    }
    let evidence = evidence(&input(RawSource::Jsonl(
        r#"{"role":"assistant","model":"old-model"}"#.into(),
    )));
    let report = old_model_report(evidence);
    assert_eq!(report.finding, 0);
    assert_eq!(report.clean, 0);
}

#[test]
fn claimed_and_default_file_visits_keep_the_same_parse_gaps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    let content = b"{\"role\":\"assistant\",\"model\":\"old-model\",\"timestamp\":1000}\n{broken\n{\"type\":\"future\"}\n";
    std::fs::write(&path, content).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let claim = SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat: SourceStat::from_open_std_file(&file).unwrap(),
        head_hash: Some(head_hash_of(content)),
    });
    let input = input(RawSource::File(path));
    let reader = reader_for("cursor");
    let mut default = SessionCollector::new("cursor", "synthetic");
    let mut claimed = SessionCollector::new("cursor", "synthetic");
    reader.visit(&input, &mut default).unwrap();
    reader
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut claimed,
        )
        .unwrap();
    assert_eq!(default.partial_reasons(), claimed.partial_reasons());
    assert!(
        default
            .partial_reasons()
            .contains(&PartialReason::MalformedRecord)
    );
    assert!(
        default
            .partial_reasons()
            .contains(&PartialReason::UnrecognizedRecordType)
    );
    assert_eq!(default.into_session().unwrap().events.len(), 1);
    assert_eq!(claimed.into_session().unwrap().events.len(), 1);
}

#[test]
fn source_classification_reads_metadata_not_payload_text() {
    for (marker, format) in [
        ("desktop_state_vscdb", SourceFormat::CursorIdeComposer),
        ("store_db", SourceFormat::CursorCliStoreDb),
        ("agent_transcript", SourceFormat::CursorCliAgentJsonl),
    ] {
        let mut input = input(RawSource::Jsonl(format!(
            "{{\"cursor_source\": \"{marker}\", \"sessionId\":\"synthetic\"}}\n{{\"role\":\"assistant\",\"model\":\"old-model\",\"timestamp\":1000}}"
        )));
        input.source_format = format;
        assert_eq!(
            reader_for("cursor").capabilities(&input).source_format,
            format
        );
        assert!(matches!(
            evidence(&input).coverage,
            EvidenceCoverage::Complete
        ));
    }
    let source = input(RawSource::Jsonl(
        r#"{"role":"assistant","content":{"cursor_source":"store_db"}}"#.into(),
    ));
    assert_eq!(
        reader_for("cursor").capabilities(&source).source_format,
        SourceFormat::CursorJsonl
    );
}

#[derive(Default)]
struct CursorRecordingSink {
    events: Vec<NormalizedEvent>,
    contents: Vec<TurnContent>,
}

impl RecordSink for CursorRecordingSink {
    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::MetricsEvent(event) => self.events.push(*event),
            NormalizedRecord::TurnContent(content) => self.contents.push(*content),
            NormalizedRecord::Observation(_) | NormalizedRecord::Unusable(_) => {}
        }
    }

    fn finish(&mut self, _summary: SessionSummary) {}
}

#[test]
fn cursor_agent_content_blocks_capture_clean_text_tools_and_results() {
    let source = include_str!("fixtures/cursor_characterization/agent_content_blocks.jsonl");
    let session_input = input(RawSource::Jsonl(source.to_owned()));
    let mut sink = CursorRecordingSink::default();

    reader_for("cursor")
        .visit(&session_input, &mut sink)
        .unwrap();

    assert_eq!(sink.events.len(), 4);
    assert_eq!(sink.events[1].tools.len(), 1);
    assert_eq!(
        sink.events[1].tools[0].category,
        antiburn_local::analysis::ToolCategory::Test
    );
    assert_eq!(sink.events[2].role, antiburn_local::analysis::Role::Tool);
    assert_eq!(sink.contents.len(), 3);
    assert_eq!(sink.contents[0].parts[0].kind, ContentKind::UserText);
    assert_eq!(sink.contents[0].parts[0].text, "run the focused tests");
    assert_eq!(sink.contents[1].parts[0].kind, ContentKind::Thinking);
    assert_eq!(sink.contents[1].parts[1].kind, ContentKind::ToolInput);
    assert_eq!(sink.contents[2].parts[0].kind, ContentKind::ToolResult);
    assert_eq!(sink.contents[2].parts[0].text, "test result: ok");
    assert_eq!(sink.contents[2].parts[1].text, "finished");

    let mut string_sink = CursorRecordingSink::default();
    reader_for("cursor")
        .visit(
            &input(RawSource::Jsonl(
                r#"{"role":"assistant","message":{"content":"plain text"}}"#.to_owned(),
            )),
            &mut string_sink,
        )
        .unwrap();
    assert_eq!(string_sink.contents[0].parts[0].text, "plain text");
}
