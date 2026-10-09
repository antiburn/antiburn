use std::sync::Arc;

use antiburn_local::analysis::{
    AppendOnlyGuarantee, CompositeSink, ContentKind, EvidenceCoverage, EvidenceSource,
    EvidenceValue, MemoryTurnRowStore, NormalizedEvent, NormalizedRecord, PartialReason, RawSource,
    RecordSink, SessionCollector, SessionEvidence, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SessionSummary, SourceClaim, SourceFormat, SourceKind, ToolClass,
    TurnContent, TurnRowSink, TurnRowStore, reader_for,
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
    assert_eq!(
        sink.contents[0].parts[0].text,
        "<user_query>run the focused tests</user_query><context>private context omitted</context>"
    );
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

#[test]
fn every_accepted_cursor_tool_call_spelling_emits_a_call_and_input() {
    for kind in ["tool_use", "tool-use", "toolCall", "tool-call", "tool_call"] {
        let source = format!(
            r#"{{"role":"assistant","model":"test-model","timestamp":1000,"content":[{{"type":"{kind}","name":"read_file","input":{{"path":"README.md"}}}}]}}"#
        );
        let mut sink = CursorRecordingSink::default();
        reader_for("cursor")
            .visit(&input(RawSource::Jsonl(source)), &mut sink)
            .unwrap();

        assert_eq!(sink.events[0].tools.len(), 1, "{kind}");
        assert_eq!(sink.events[0].tools[0].name, "read_file", "{kind}");
        assert_eq!(sink.contents[0].parts.len(), 1, "{kind}");
        assert_eq!(
            sink.contents[0].parts[0].kind,
            ContentKind::ToolInput,
            "{kind}"
        );
    }
}

#[test]
fn native_message_variants_preserve_authority_and_tool_identity() {
    use antiburn_local::analysis::{ContentAuthority, Role};

    let source = include_str!("fixtures/cursor_characterization/native_message_variants.jsonl");
    let mut sink = CursorRecordingSink::default();
    reader_for("cursor")
        .visit(&input(RawSource::Jsonl(source.to_owned())), &mut sink)
        .unwrap();

    assert_eq!(sink.events.len(), 5);
    assert_eq!(sink.events[0].role, Role::System);
    assert_eq!(
        sink.contents[0].parts[0].authority,
        ContentAuthority::System
    );
    assert_eq!(
        sink.contents[1].parts[0].text,
        "A scalar assistant message."
    );
    assert_eq!(sink.events[2].tools.len(), 2);
    assert_eq!(sink.contents[2].parts.len(), 2);
    assert_eq!(
        sink.contents[2].parts[0].tool_call_id.as_deref(),
        Some("call-1")
    );
    assert_eq!(
        sink.contents[2].parts[1].tool_call_id.as_deref(),
        Some("call-2")
    );
    assert_eq!(sink.contents[3].parts.len(), 2);
    assert_eq!(
        sink.contents[3].parts[0].tool_name.as_deref(),
        Some("Shell")
    );
    assert_eq!(
        sink.contents[3].parts[0].tool_call_id.as_deref(),
        Some("call-1")
    );
    assert_eq!(
        sink.contents[3].parts[1].tool_call_id.as_deref(),
        Some("call-2")
    );
    assert_eq!(sink.contents[4].parts.len(), 1);
    assert_eq!(
        sink.contents[4].parts[0].text,
        "<user_query>user request</user_query>"
    );
}

#[test]
fn array_tool_result_keeps_outer_tool_identity() {
    let source = include_str!("fixtures/cursor_characterization/array_tool_result.jsonl");
    let mut sink = CursorRecordingSink::default();
    reader_for("cursor")
        .visit(&input(RawSource::Jsonl(source.to_owned())), &mut sink)
        .unwrap();
    let result = &sink.contents[1].parts[0];
    assert_eq!(result.kind, ContentKind::ToolResult);
    assert_eq!(result.text, "first\nsecond");
    assert_eq!(result.tool_name.as_deref(), Some("Shell"));
    assert_eq!(result.tool_call_id.as_deref(), Some("call-1"));
}

#[test]
fn resource_tool_calls_remain_unclassified_without_resource_metadata() {
    let input = input(RawSource::Jsonl(
        include_str!("fixtures/cursor_characterization/unclassified_resource_calls.jsonl")
            .to_owned(),
    ));
    let evidence = evidence(&input);
    let tools = match evidence.tools {
        EvidenceValue::Complete(tools)
        | EvidenceValue::Partial {
            observed: tools, ..
        } => tools,
        EvidenceValue::Unsupported => panic!("cursor tool evidence must be available"),
    };

    assert_eq!(tools.by_name.len(), 5);
    assert!(
        tools
            .by_name
            .values()
            .all(|tool| tool.calls == 1 && tool.class == ToolClass::Unclassified)
    );
}

#[test]
fn public_store_reader_shape_preserves_structure_identity_and_repeated_scope() {
    use antiburn_local::analysis::{ContentAuthority, Role};

    let source = include_str!("fixtures/cursor_characterization/store_reader_blocks.jsonl");
    let mut sink = CursorRecordingSink::default();
    reader_for("cursor")
        .visit(&input(RawSource::Jsonl(source.to_owned())), &mut sink)
        .unwrap();

    assert_eq!(sink.events.len(), 4);
    assert_eq!(sink.events[0].uuid.as_deref(), Some("user-1"));
    assert_eq!(sink.events[3].uuid.as_deref(), Some("user-2"));
    assert_eq!(sink.contents[0], sink.contents[3]);
    assert_eq!(sink.contents[0].parts[0].authority, ContentAuthority::User);
    assert_eq!(
        sink.contents[0].parts[0].text,
        "  Keep the change small.\n    Keep this indentation.\n"
    );
    let call = &sink.contents[1].parts[1];
    assert_eq!(sink.events[1].tools[0].name, "Read");
    assert_eq!(call.tool_name.as_deref(), Some("Read"));
    assert_eq!(call.tool_call_id.as_deref(), Some("read-1"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&call.text).unwrap(),
        serde_json::json!({"path":"/synthetic/project/task.plan.md"})
    );
    assert_eq!(sink.events[2].role, Role::Tool);
    let results = &sink.contents[2].parts;
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(result.kind, ContentKind::ToolResult);
        assert_eq!(result.authority, ContentAuthority::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some("read-1"));
        assert!(result.metadata.user_answers.is_empty());
        assert!(result.metadata.plan_references.is_empty());
    }
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&results[0].text).unwrap(),
        serde_json::json!([
            {"type":"text","text":"# Task\n"},
            {"type":"text","text":"Keep the change small."}
        ])
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&results[1].text).unwrap()["providerOptions"]["cursor"]
            ["highLevelToolCallResult"]["workspaceResults"][0]["filePath"],
        "/synthetic/project/task.plan.md"
    );
}

#[test]
fn optional_question_and_plan_evidence_stays_unavailable_without_a_dedicated_pin() {
    use antiburn_local::analysis::ContentAuthority;

    let source = include_str!("fixtures/cursor_characterization/optional_evidence_negative.jsonl");
    for format in [
        SourceFormat::CursorJsonl,
        SourceFormat::CursorCliAgentJsonl,
        SourceFormat::CursorCliStoreDb,
        SourceFormat::CursorChatStoreDb,
        SourceFormat::CursorIdeComposer,
    ] {
        let mut session_input = input(RawSource::Jsonl(source.to_owned()));
        session_input.source_format = format;
        let mut sink = CursorRecordingSink::default();
        reader_for("cursor")
            .visit(&session_input, &mut sink)
            .unwrap();
        assert_eq!(sink.events.len(), 8);
        let parts = sink
            .contents
            .iter()
            .flat_map(|turn| &turn.parts)
            .collect::<Vec<_>>();
        assert!(
            parts
                .iter()
                .all(|part| part.metadata.user_answers.is_empty()
                    && part.metadata.plan_references.is_empty())
        );
        let users = parts
            .iter()
            .filter(|part| part.authority == ContentAuthority::User)
            .collect::<Vec<_>>();
        assert_eq!(users.len(), 2);
        assert_eq!(users[1].text, "No. Do not implement extra work.");
        assert!(
            parts
                .iter()
                .any(|part| part.tool_call_id.as_deref() == Some("question-2")
                    && part.kind == ContentKind::ToolInput)
        );
        assert!(
            parts
                .iter()
                .any(|part| part.tool_call_id.as_deref() == Some("unrelated")
                    && part.authority == ContentAuthority::Tool)
        );
    }
}

#[test]
fn rich_results_keep_unknown_authority_and_do_not_guess_ambiguous_call_binding() {
    for role in ["assistant", "tool"] {
        let source = serde_json::json!({
            "role": role,
            "content": [
                {"type":"tool-result","toolCallId":"first","result":"one"},
                {"type":"tool-result","toolCallId":"second","result":"two"}
            ],
            "providerOptions":{"cursor":{"highLevelToolCallResult":{"approved":true}}}
        });
        let mut sink = CursorRecordingSink::default();
        reader_for("cursor")
            .visit(&input(RawSource::Jsonl(source.to_string())), &mut sink)
            .unwrap();
        // Result-only blocks have tool authority even when their wrapper says assistant.
        assert_eq!(sink.contents[0].parts.len(), 3);
        let rich = &sink.contents[0].parts[2];
        assert_eq!(rich.tool_call_id, None);
        assert_eq!(
            rich.authority,
            antiburn_local::analysis::ContentAuthority::Tool
        );
        assert!(rich.metadata.user_answers.is_empty());
    }
}

#[test]
fn cursor_scope_content_reports_truncation_instead_of_claiming_complete_text() {
    let text = "x".repeat(300 * 1024);
    let source = serde_json::json!({"role":"user","id":"long-user","content":text});
    let mut sink = CursorRecordingSink::default();
    reader_for("cursor")
        .visit(&input(RawSource::Jsonl(source.to_string())), &mut sink)
        .unwrap();
    let part = &sink.contents[0].parts[0];
    assert_eq!(part.kind, ContentKind::UserText);
    assert!(part.truncated);
    assert_eq!(part.text.len(), 256 * 1024);
}

#[test]
fn native_user_query_tags_never_delete_conditions_examples_or_sections() {
    let source = include_str!("fixtures/cursor_characterization/user_query_text.jsonl");
    let mut sink = CursorRecordingSink::default();
    reader_for("cursor")
        .visit(&input(RawSource::Jsonl(source.to_owned())), &mut sink)
        .unwrap();
    assert_eq!(sink.contents.len(), 5);
    for (turn, line) in sink.contents.iter().zip(source.lines()) {
        let native: serde_json::Value = serde_json::from_str(line).unwrap();
        let text = native["content"]
            .as_str()
            .or_else(|| native["content"][0]["text"].as_str())
            .unwrap();
        assert_eq!(turn.parts.len(), 1);
        assert_eq!(turn.parts[0].text, text);
        assert_eq!(
            turn.parts[0].authority,
            antiburn_local::analysis::ContentAuthority::User
        );
        assert!(turn.parts[0].metadata.user_answers.is_empty());
        assert!(turn.parts[0].metadata.plan_references.is_empty());
    }
}

#[test]
fn synthetic_unproven_order_propagates_to_whole_source_coverage() {
    for marker in ["store_db", "desktop_state_vscdb"] {
        let source = format!(
            "{}\n{}",
            serde_json::json!({"cursor_source":marker,"cursor_scope_ordering":"unproven"}),
            r#"{"role":"user","id":"correction","timestamp":2000,"content":"No. Do not implement it."}"#
        );
        let session_input = input(RawSource::Jsonl(source));
        let observed = evidence(&session_input);
        assert!(matches!(observed.coverage, EvidenceCoverage::Partial(_)));
        let mut collector = SessionCollector::new("cursor", "synthetic");
        reader_for("cursor")
            .visit(&session_input, &mut collector)
            .unwrap();
        assert!(
            collector
                .partial_reasons()
                .contains(&PartialReason::AttributionIncomplete)
        );
        assert_eq!(collector.into_session().unwrap().events.len(), 1);
    }
}
