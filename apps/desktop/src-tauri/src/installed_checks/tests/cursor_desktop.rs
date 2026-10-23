use super::*;

fn cursor_transcript(session_id: &str, count: usize) -> String {
    let mut records = vec![
        json!({"cursor_source":"desktop_state_vscdb","cursor_scope_ordering":"recorded_headers","session_id":session_id,"local_fork_observation":null}),
        json!({"role":"user","bubbleId":"user","content":"Fix the parser tests.","timestamp":"2026-01-01T00:00:00Z"}),
    ];
    for index in 0..count {
        records.push(json!({"role":"assistant","bubbleId":format!("bubble/t:call-{index}\nresult"),"content":"","timestamp":"2026-01-01T00:00:01Z","model":"synthetic","cursor_tool_result":{"name":"run_terminal_command_v2","toolCallId":format!("call-{index}\nresult"),"status":"completed","params":json!({"command":"cargo test parser"}).to_string(),"result":json!({"output":"test parser::parse_empty FAILED: expected an empty list, got null; src/parser.rs:42","exitCode":101,"notInterrupted":true,"rejected":false}).to_string()}}));
    }
    records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn publish_cursor(fixture: &Fixture, count: usize) {
    let environment = "native";
    let session_id = "cursor-main";
    let cwd = &fixture.project;
    fixture
        .store
        .upsert_sessions(
            &[SessionRecord {
                key: SessionKey::new(environment, "cursor", session_id),
                source_kind: "inline".into(),
                source_label: format!("cursor-desktop:{session_id}"),
                wsl_distro: None,
                title: Some("Parser tests".into()),
                title_source: Some("explicit".into()),
                cwd: Some(cwd.to_string_lossy().into_owned()),
                surface: "desktop".into(),
                updated_at_epoch: Some(time::OffsetDateTime::now_utc().unix_timestamp()),
                activity_cursor: String::new(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(format!("{session_id}-{count}")),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let key = SessionKey::new(environment, "cursor", session_id);
    let input = SessionInput {
        agent: "cursor".into(),
        session_id: session_id.into(),
        source: RawSource::Jsonl(cursor_transcript(session_id, count)),
        source_format: SourceFormat::CursorIdeComposer,
        fork_parent_session_id: None,
    };
    let mut sink = CompositeSink::new(
        SessionMetricsAccumulator::new("cursor", session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "cursor".into(),
            session_id: session_id.into(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities {
                source_format: SourceFormat::CursorIdeComposer,
                ..SourceCapabilities::cursor()
            },
        }),
    );
    reader_for("cursor").visit(&input, &mut sink).unwrap();
    let results = sink.tool_result_observations();
    assert_eq!(results.len(), count);
    assert!(
        results
            .iter()
            .all(|result| result.context.as_ref().unwrap().main_session)
    );
    let claim = fixture
        .store
        .claim_next_evidence_by_recency(&["cursor"], 10_000, 60)
        .unwrap()
        .unwrap();
    assert_eq!(claim.key, key);
    let revisions = crate::analysis::projection_revisions();
    let fingerprint = format!("{session_id}-{count}");
    assert!(
        fixture
            .store
            .publish_projections_with_native_tool_results(
                &AnalysisRecord {
                    key: key.clone(),
                    model_breakdown_json: "{}".into(),
                    pricing_breakdown_json: "{}".into(),
                    inclusive_models_json: "[]".into(),
                    initial_context_json: None,
                    source_summaries_json: None,
                    provider_hints_json: None,
                    source_fingerprint: fingerprint.clone(),
                    pricing_generation: 1,
                    analyzed_generation: claim.source_generation,
                    parser_revision: revisions.parser_revision,
                    analyzer_revision: revisions.analyzer_revision,
                    metrics_schema_revision: revisions.metrics_schema_revision,
                },
                None,
                &EvidenceCompletion {
                    claim_fence: claim.claim_fence,
                    status: PublishedEvidence::Ready,
                    evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    evidence_json: crate::store::test_support::evidence_json(&key),
                },
                &[],
                &[],
                ToolResultPublishOptions {
                    source_fingerprint: Some(&fingerprint),
                    batch: Some(&ToolResultBatch {
                        source_format: SourceFormat::CursorIdeComposer,
                        results: results.to_vec(),
                        complete: true
                    }),
                },
            )
            .unwrap()
    );
}

#[tokio::test]
async fn cursor_desktop_startup_and_live_results_reach_one_assessment_without_replay() {
    for source_first in [false, true] {
        let fixture = Fixture::new();
        publish_cursor(&fixture, if source_first { 3 } else { 0 });
        let mut host = fixture.host();
        host.reconcile(&Admission::Ready).unwrap();
        drain(&mut host);
        if !source_first {
            publish_cursor(&fixture, 3);
            drain(&mut host);
        }
        fixture.evaluator.answer("stuck");
        assert_eq!(
            host.evaluation_jobs(1)
                .pop()
                .unwrap()
                .run()
                .await
                .unwrap()
                .unwrap()
                .outcome,
            RunOutcome::Accepted
        );
        {
            let inputs = fixture.evaluator.inputs.lock().unwrap();
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0]["activity"].as_array().unwrap().len(), 3);
            assert_eq!(inputs[0]["request_history_complete"], true);
        }
        drop(host);
        let mut restarted = fixture.host();
        restarted.reconcile(&Admission::Ready).unwrap();
        drain(&mut restarted);
        assert!(
            restarted
                .evaluation_jobs(1)
                .pop()
                .unwrap()
                .run()
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
    }
}

fn drain(host: &mut InstalledCheckHost<ScriptedEvaluator>) {
    for _ in 0..32 {
        if !host.catch_up().unwrap().1 {
            return;
        }
    }
    panic!("Cursor publications did not drain");
}

#[test]
fn cursor_attachment_rejects_stale_snapshot() {
    let fixture = Fixture::new();
    publish_cursor(&fixture, 3);
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    let project = host.projects.values().next().unwrap();
    let activation = project.active[0].clone();
    let adapter = project.store.clone();
    let sessions = adapter
        .startup_sessions(&activation, active_since())
        .unwrap();
    let cursor = sessions
        .iter()
        .find(|session| session.key.agent == "cursor")
        .unwrap();
    publish_cursor(&fixture, 4);
    assert!(
        adapter
            .initialize_recent(&activation, cursor, &[])
            .unwrap()
            .is_none()
    );
}

#[test]
fn cursor_attachment_rejects_paused_activation() {
    let fixture = Fixture::new();
    publish_cursor(&fixture, 3);
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    let project = host.projects.values().next().unwrap();
    let activation = project.active[0].clone();
    let adapter = project.store.clone();
    let sessions = adapter
        .startup_sessions(&activation, active_since())
        .unwrap();
    let cursor = sessions
        .iter()
        .find(|session| session.key.agent == "cursor")
        .unwrap();
    host.deactivate_active().unwrap();
    assert!(adapter.initialize_recent(&activation, cursor, &[]).is_err());
}
