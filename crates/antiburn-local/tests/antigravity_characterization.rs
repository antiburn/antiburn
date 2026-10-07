use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceCoverage, EvidenceSource, EvidenceValue, FenceScope, MemoryTurnRowStore,
    RawSource, SessionEvidence, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceFormat, SourceKind, ToolCategory, ToolClass, TurnRowSink,
    TurnRowStore, TurnSessionKey, query_turn_content, reader_for,
};
use antiburn_local::insights::{
    CoverageCounts, DetectorCounts, DetectorId, EfficiencyReportAccumulator, ModelRegistry,
    ModelReplacementEntry, ReportCatalogs, ReportContext, ReportWindow,
};
use rusqlite::{Connection, params};

#[derive(Default)]
struct ContentSink(Vec<antiburn_local::analysis::ContentPart>);

impl antiburn_local::analysis::RecordSink for ContentSink {
    fn record(&mut self, record: antiburn_local::analysis::NormalizedRecord) {
        if let antiburn_local::analysis::NormalizedRecord::TurnContent(content) = record {
            self.0.extend(content.parts);
        }
    }

    fn finish(&mut self, _: antiburn_local::analysis::SessionSummary) {}
}

fn content_parts(input: &SessionInput) -> Vec<antiburn_local::analysis::ContentPart> {
    let mut sink = ContentSink::default();
    reader_for("antigravity").visit(input, &mut sink).unwrap();
    sink.0
}

fn input(source: RawSource) -> SessionInput {
    let source_format = match &source {
        RawSource::Sqlite(_) => SourceFormat::AntigravitySqlite,
        RawSource::File(_) => SourceFormat::AntigravityBrainJsonl,
        RawSource::Jsonl(content) if content.contains("\"steps\"") => {
            SourceFormat::AntigravityCascadeJson
        }
        RawSource::Jsonl(_) => SourceFormat::AntigravityBrainJsonl,
        RawSource::ClineBundle { .. } => SourceFormat::Uncharacterized,
        RawSource::KiroCliV2Bundle { .. } => SourceFormat::Uncharacterized,
        RawSource::KiroCliV3Bundle { .. } => SourceFormat::Uncharacterized,
        RawSource::CopilotCliBundle { .. } => SourceFormat::Uncharacterized,
    };
    SessionInput {
        agent: "antigravity".into(),
        session_id: "synthetic".into(),
        source,
        fork_parent_session_id: None,
        source_format,
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
    connection.execute_batch("PRAGMA user_version = 1; CREATE TABLE steps (idx INTEGER, metadata BLOB); CREATE TABLE gen_metadata (idx INTEGER, data BLOB);").unwrap();
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
    let mut input = input(RawSource::Sqlite(path));
    input.source_format = SourceFormat::AntigravitySqlite;
    (directory, input)
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

#[test]
fn sqlite_companion_retains_assistant_content_without_double_counting_usage() {
    let (directory, input) = database(true, false, false, false);
    let path = directory
        .path()
        .join("brain/synthetic/.system_generated/logs/transcript.jsonl");
    let mut transcript = std::fs::read_to_string(&path).unwrap();
    transcript.push_str(
        r#"{"type":"PLANNER_RESPONSE","step_index":2,"content":"companion assistant content","usage":{"input_tokens":900,"output_tokens":900}}"#,
    );
    transcript.push('\n');
    std::fs::write(path, transcript).unwrap();

    let normalized = reader_for("antigravity").normalize(&input).unwrap();
    let assistant_events = normalized
        .events
        .iter()
        .filter(|event| event.role == antiburn_local::analysis::Role::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(assistant_events.len(), 1);
    assert_eq!(assistant_events[0].usage.input_tokens, 10);
    assert_eq!(assistant_events[0].usage.output_tokens, 2);

    let store = MemoryTurnRowStore::new("antigravity", "synthetic");
    let mut rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "synthetic",
        None,
    );
    reader_for("antigravity").visit(&input, &mut rows).unwrap();
    let key = TurnSessionKey {
        environment_key: "native",
        agent: "antigravity",
        session_id: "synthetic",
    };
    let content = store.with_connection(|connection| {
        query_turn_content(connection, &key, &FenceScope::single(1)).unwrap()
    });
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.text == "companion assistant content")
    );
}

#[test]
fn cli_transcript_recovers_settings_model_thinking_tools_and_clipped_coverage() {
    let mut input = input(RawSource::Jsonl(
        include_str!("fixtures/antigravity_characterization/cli_realistic.jsonl").to_owned(),
    ));
    input.source_format = SourceFormat::AntigravityBrainJsonl;

    let session = reader_for("antigravity").normalize(&input).unwrap();
    assert_eq!(session.model.as_deref(), Some("old-model"));
    assert!(session.events.iter().any(|event| event.has_thinking));
    assert_eq!(session.events[2].tools.len(), 1);
    assert_eq!(session.events[2].tools[0].category, ToolCategory::Read);
    assert_eq!(session.events[3].role, antiburn_local::analysis::Role::Tool);

    let evidence = evidence(&input);
    assert!(matches!(evidence.coverage, EvidenceCoverage::Partial(_)));
    assert_eq!(old_model_report(evidence).finding, 1);
}

#[test]
fn cascade_transcript_preserves_thinking_and_nested_tool_calls() {
    let mut input = input(RawSource::Jsonl(
        include_str!("fixtures/antigravity_characterization/cascade_content.json").to_owned(),
    ));
    input.source_format = SourceFormat::AntigravityCascadeJson;

    let session = reader_for("antigravity").normalize(&input).unwrap();
    assert_eq!(session.events.len(), 3);
    assert!(session.events[1].has_thinking);
    assert_eq!(session.events[1].model.as_deref(), Some("gemini-3.6-flash"));
    assert_eq!(session.events[1].tools.len(), 1);
    assert_eq!(session.events[1].tools[0].category, ToolCategory::Read);
    assert_eq!(session.events[2].role, antiburn_local::analysis::Role::Tool);

    let store = MemoryTurnRowStore::new("antigravity", "synthetic");
    let mut rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "synthetic",
        None,
    );
    reader_for("antigravity").visit(&input, &mut rows).unwrap();
    let key = TurnSessionKey {
        environment_key: "native",
        agent: "antigravity",
        session_id: "synthetic",
    };
    let content = store.with_connection(|connection| {
        query_turn_content(connection, &key, &FenceScope::single(1)).unwrap()
    });
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.text == "cascade-user-response")
    );
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.text == "cascade-user-item")
    );
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.text == "cascade-assistant-text")
    );
}

#[test]
fn depth_finding_survives_complete_and_incomplete_antigravity_evidence() {
    let over_depth = r#"{"type":"PLANNER_RESPONSE","model":"test-model","created_at":1000,"usage":{"input_tokens":400001}}"#;
    for (content, expected_incomplete) in [
        (over_depth.to_owned(), false),
        (format!("{over_depth}\n{{\"type\":\"FUTURE\"}}"), true),
    ] {
        let evidence = evidence(&input(RawSource::Jsonl(content)));
        let incomplete = matches!(evidence.coverage, EvidenceCoverage::Partial(_));
        assert_eq!(incomplete, expected_incomplete);
        let mut report = EfficiencyReportAccumulator::new();
        report.observe_session(evidence);
        let counts = report
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
            .detectors[DetectorId::SessionsOverDepth.index()];
        assert_eq!(counts.finding, 1);
        assert_eq!(counts.clean, 0);
        assert_eq!(counts.unavailable, 0, "incomplete={incomplete}");
    }
}

#[test]
fn resource_tool_calls_remain_unclassified_without_resource_metadata() {
    let input = input(RawSource::Jsonl(
        include_str!("fixtures/antigravity_characterization/unclassified_resource_calls.jsonl")
            .to_owned(),
    ));
    let evidence = evidence(&input);
    let tools = match evidence.tools {
        EvidenceValue::Complete(tools)
        | EvidenceValue::Partial {
            observed: tools, ..
        } => tools,
        EvidenceValue::Unsupported => panic!("antigravity tool evidence must be available"),
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
fn antigravity_brain_content_reaches_the_shared_private_turn_content_path() {
    let input = input(RawSource::Jsonl(
        include_str!("fixtures/antigravity_characterization/ignored_instructions_content.jsonl")
            .to_owned(),
    ));
    let store = MemoryTurnRowStore::new("antigravity", "synthetic");
    let mut rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "synthetic",
        None,
    );
    reader_for("antigravity").visit(&input, &mut rows).unwrap();

    let key = TurnSessionKey {
        environment_key: "native",
        agent: "antigravity",
        session_id: "synthetic",
    };
    let content = store.with_connection(|connection| {
        query_turn_content(connection, &key, &FenceScope::single(1)).unwrap()
    });
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.text == "ANTIGRAVITY-USER")
    );
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.text.contains("focused tests passed"))
    );
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.kind.as_str() == "tool_input")
    );
    assert!(content.parts.iter().all(|part| {
        part.part.text != "test result: ok" || part.part.kind.as_str() != "user_text"
    }));
    assert!(
        content
            .parts
            .iter()
            .any(|part| part.part.kind.as_str() == "thinking")
    );
}

#[test]
fn recorded_proposals_tasks_and_user_corrections_keep_order_and_authority() {
    use antiburn_local::analysis::{ContentAuthority, ContentKind};

    let input = input(RawSource::Jsonl(
        include_str!("fixtures/antigravity_characterization/scope_records.jsonl").into(),
    ));
    let parts = content_parts(&input);
    assert_eq!(parts.len(), 6);
    assert_eq!(parts[0].authority, ContentAuthority::User);
    assert!(
        parts[0]
            .text
            .contains("Keep the database schema unchanged.")
    );
    assert_eq!(parts[1].authority, ContentAuthority::Assistant);
    assert!(parts[1].text.starts_with("Proposed work:"));
    assert_eq!(parts[2].kind, ContentKind::ToolInput);
    assert_eq!(parts[2].authority, ContentAuthority::Assistant);
    let args: serde_json::Value = serde_json::from_str(&parts[2].text).unwrap();
    assert_eq!(
        args["CodeContent"],
        "# Tasks\n\n- [ ] Update the parser\n- [ ] Run focused tests\n"
    );
    assert_eq!(parts[2].tool_call_id, None);
    assert_eq!(parts[3].authority, ContentAuthority::Tool);
    assert_eq!(parts[3].tool_call_id, None);
    assert_eq!(parts[4].authority, ContentAuthority::User);
    assert_eq!(
        parts[4].text,
        "Proceed with the parser change only. Do not change discovery."
    );
    assert_eq!(parts[5].authority, ContentAuthority::User);
    assert_eq!(
        parts[5].text,
        "Actually, reject that proposal. Investigate first."
    );
    assert!(
        parts
            .iter()
            .all(|part| part.metadata.user_answers.is_empty()
                && part.metadata.plan_references.is_empty())
    );
}

#[test]
fn accepted_user_fields_retain_conditions_and_repeated_text() {
    let step = r#"{"type":"USER_INPUT","content":"Do not change the schema.","userInput":{"userResponse":"Proceed only after tests pass.","items":[{"text":"Do not change the schema."},{"text":"Actually, investigate first."}]}}"#;
    for source in [step.to_owned(), format!("{{\"steps\":[{step}]}}")] {
        let parts = content_parts(&input(RawSource::Jsonl(source)));
        assert_eq!(
            parts
                .iter()
                .map(|part| part.text.as_str())
                .collect::<Vec<_>>(),
            [
                "Proceed only after tests pass.",
                "Do not change the schema.",
                "Do not change the schema.",
                "Actually, investigate first.",
            ]
        );
        assert!(
            parts
                .iter()
                .all(|part| part.authority == antiburn_local::analysis::ContentAuthority::Unknown)
        );
    }
}

#[test]
fn brain_human_authority_requires_pinned_explicit_source_and_scalar_content() {
    use antiburn_local::analysis::{ContentAuthority, ContentKind, Role};

    for (source, expected) in [
        (
            Some(serde_json::json!("USER_EXPLICIT")),
            ContentAuthority::User,
        ),
        (Some(serde_json::json!("SYSTEM")), ContentAuthority::Unknown),
        (Some(serde_json::json!("MODEL")), ContentAuthority::Unknown),
        (Some(serde_json::json!("FUTURE")), ContentAuthority::Unknown),
        (
            Some(serde_json::json!("user_explicit")),
            ContentAuthority::Unknown,
        ),
        (Some(serde_json::json!(null)), ContentAuthority::Unknown),
        (
            Some(serde_json::json!(["USER_EXPLICIT", "SYSTEM"])),
            ContentAuthority::Unknown,
        ),
        (None, ContentAuthority::Unknown),
    ] {
        let mut record = serde_json::json!({
            "type": "USER_INPUT", "created_at": 1000,
            "content": "Proceed only after tests pass. Do not change the schema.",
        });
        if let Some(source) = source {
            record["source"] = source;
        }
        let input = input(RawSource::Jsonl(record.to_string()));
        let parts = content_parts(&input);
        assert_eq!(parts.len(), 1, "{record}");
        assert_eq!(parts[0].text, record["content"].as_str().unwrap());
        assert_eq!(parts[0].kind, ContentKind::UserText);
        assert_eq!(parts[0].authority, expected, "{record}");
        assert!(parts[0].metadata.user_answers.is_empty());
        assert!(parts[0].metadata.plan_references.is_empty());
        let session = reader_for("antigravity").normalize(&input).unwrap();
        assert_eq!(session.events.len(), 1);
        assert_eq!(session.events[0].role, Role::User);
        assert_eq!(session.events[0].ts_ms, Some(1_000_000));
        assert_eq!(
            matches!(evidence(&input).coverage, EvidenceCoverage::Partial(_)),
            expected != ContentAuthority::User,
            "{record}"
        );
    }
}

#[test]
fn unpinned_nested_user_fields_never_inherit_explicit_scalar_authority() {
    use antiburn_local::analysis::ContentAuthority;

    let record = serde_json::json!({
        "type": "USER_INPUT", "source": "USER_EXPLICIT", "created_at": 1000,
        "content": "Investigate first.",
        "userInput": {
            "userResponse": "Proceed",
            "items": [{"text": "Approve the plan"}],
        },
    });
    let session_input = input(RawSource::Jsonl(record.to_string()));
    let parts = content_parts(&session_input);
    assert_eq!(
        parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>(),
        ["Proceed", "Investigate first.", "Approve the plan"]
    );
    assert_eq!(
        parts.iter().map(|part| part.authority).collect::<Vec<_>>(),
        [
            ContentAuthority::Unknown,
            ContentAuthority::User,
            ContentAuthority::Unknown
        ]
    );
    assert!(
        parts
            .iter()
            .all(|part| part.metadata.user_answers.is_empty()
                && part.metadata.plan_references.is_empty())
    );
    assert!(matches!(
        evidence(&session_input).coverage,
        EvidenceCoverage::Partial(_)
    ));

    for kind in ["USER_INPUT", "CORTEX_STEP_TYPE_USER_INPUT"] {
        let mut nested_only = record.clone();
        nested_only["type"] = serde_json::json!(kind);
        nested_only.as_object_mut().unwrap().remove("content");
        for source in [
            nested_only.to_string(),
            format!("{{\"steps\":[{nested_only}]}}"),
        ] {
            let input = input(RawSource::Jsonl(source));
            let parts = content_parts(&input);
            assert_eq!(parts.len(), 2);
            assert!(
                parts
                    .iter()
                    .all(|part| part.authority == ContentAuthority::Unknown)
            );
            assert!(
                parts
                    .iter()
                    .all(|part| part.metadata.user_answers.is_empty()
                        && part.metadata.plan_references.is_empty())
            );
            assert!(matches!(
                evidence(&input).coverage,
                EvidenceCoverage::Partial(_)
            ));
        }
    }
}

#[test]
fn brain_source_provenance_does_not_establish_cascade_human_authority() {
    let input = input(RawSource::Jsonl(r#"{"steps":[{"type":"USER_INPUT","source":"USER_EXPLICIT","created_at":1000,"content":"Proceed"}]}"#.into()));
    let parts = content_parts(&input);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].text, "Proceed");
    assert_eq!(
        parts[0].authority,
        antiburn_local::analysis::ContentAuthority::Unknown
    );
    assert!(matches!(
        evidence(&input).coverage,
        EvidenceCoverage::Partial(_)
    ));
}

#[test]
fn optional_native_call_ids_are_preserved_without_invented_joins() {
    let parts = content_parts(&input(RawSource::Jsonl(concat!(
        "{\"type\":\"PLANNER_RESPONSE\",\"tool_calls\":[{\"id\":\"recorded-call\",\"name\":\"view_file\",\"args\":{\"AbsolutePath\":\"/synthetic/plan.md\"}},{\"name\":\"view_file\",\"args\":{\"AbsolutePath\":\"/synthetic/task.md\"}}]}\n",
        "{\"type\":\"VIEW_FILE\",\"tool_call_id\":\"recorded-call\",\"content\":\"# Proposed work\"}\n"
    ).into())));
    assert_eq!(parts[0].tool_call_id.as_deref(), Some("recorded-call"));
    assert_eq!(parts[1].tool_call_id, None);
    assert_eq!(parts[2].tool_call_id.as_deref(), Some("recorded-call"));
    assert_eq!(
        parts[2].authority,
        antiburn_local::analysis::ContentAuthority::Tool
    );
    assert!(parts[2].metadata.plan_references.is_empty());
}

#[test]
fn notification_policy_and_unproven_review_fields_do_not_grant_approval() {
    let parts = content_parts(&input(RawSource::Jsonl(concat!(
        "{\"type\":\"PLANNER_RESPONSE\",\"tool_calls\":[{\"name\":\"notify_user\",\"args\":{\"Message\":\"Review requested\",\"PathsToReview\":[\"/synthetic/implementation_plan.md\"]}}]}\n",
        "{\"type\":\"TOOL\",\"content\":\"Notification delivered. approved\"}\n",
        "{\"type\":\"SETTINGS\",\"content\":\"Always Proceed\"}\n",
        "{\"type\":\"USER_INPUT\",\"review\":{\"proceed\":true,\"comments\":[\"approved\"],\"version\":1}}\n"
    ).into())));
    assert_eq!(parts.len(), 3);
    assert!(
        parts
            .iter()
            .all(|part| part.authority != antiburn_local::analysis::ContentAuthority::User)
    );
    assert!(
        parts
            .iter()
            .all(|part| part.metadata.user_answers.is_empty()
                && part.metadata.plan_references.is_empty())
    );
}

#[test]
fn missing_wrong_owner_and_stale_plan_companions_supply_no_historical_approval() {
    for status in ["missing", "wrong-owner", "stale-plan"] {
        let (directory, mut input) = database(false, false, false, false);
        let brain = directory.path().join("brain/synthetic");
        std::fs::create_dir_all(&brain).unwrap();
        if status == "wrong-owner" {
            let logs = directory.path().join("brain/other/.system_generated/logs");
            std::fs::create_dir_all(&logs).unwrap();
            std::fs::write(
                logs.join("transcript.jsonl"),
                "{\"type\":\"USER_INPUT\",\"content\":\"Proceed\"}\n",
            )
            .unwrap();
            input.session_id = "other".into();
        } else if status == "stale-plan" {
            std::fs::write(
                brain.join("implementation_plan.md"),
                "# Current unreviewed revision\nChange the schema.\n",
            )
            .unwrap();
            std::fs::write(
                brain.join("implementation_plan.md.metadata.json"),
                "{\"unproven_review\":\"approved\",\"unproven_version\":1}",
            )
            .unwrap();
        }
        assert!(content_parts(&input).is_empty(), "{status}");
        let session = reader_for("antigravity").normalize(&input).unwrap();
        assert_eq!(session.events.len(), 1);
        assert_eq!(session.events[0].usage.input_tokens, 10);
    }
}

#[test]
fn unsupported_sqlite_versions_do_not_admit_companion_review_text() {
    for version in [0, 2] {
        let (_directory, input) = database(true, false, false, false);
        let RawSource::Sqlite(path) = &input.source else {
            unreachable!()
        };
        Connection::open(path)
            .unwrap()
            .pragma_update(None, "user_version", version)
            .unwrap();
        let error = reader_for("antigravity").normalize(&input).unwrap_err();
        assert!(format!("{error:#}").contains(&format!("unsupported user_version {version}")));
    }
}
