use super::*;
use crate::store::{
    EvidenceCompletion, FencedTurnRowStore, PublishedEvidence, RelationRecord, SessionRecord,
};
use rusqlite::{Connection, params};

#[path = "opencode_v2_tests/output_limits.rs"]
mod output_limits;

fn database() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode-v2.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY, parent_id TEXT, fork_session_id TEXT,
                 directory TEXT NOT NULL DEFAULT '/repo', title TEXT,
                 time_created INTEGER NOT NULL DEFAULT 1000,
                 time_updated INTEGER NOT NULL DEFAULT 4000
             );
             CREATE TABLE session_message (
                 id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                 seq INTEGER NOT NULL, time_created INTEGER NOT NULL DEFAULT 1000,
                 time_updated INTEGER NOT NULL DEFAULT 4000, data TEXT NOT NULL
             );
             CREATE UNIQUE INDEX native_sequence ON session_message(session_id, seq);
             ALTER TABLE session_v2 ADD COLUMN cost REAL;
             ALTER TABLE session_v2 ADD COLUMN tokens_input INTEGER;
             ALTER TABLE session_v2 ADD COLUMN tokens_output INTEGER;
             ALTER TABLE session_v2 ADD COLUMN tokens_reasoning INTEGER;
             ALTER TABLE session_v2 ADD COLUMN tokens_cache_read INTEGER;
             ALTER TABLE session_v2 ADD COLUMN tokens_cache_write INTEGER;
              INSERT INTO session_v2 (id,parent_id) VALUES
                  ('ses_parent',NULL),('ses_child','ses_parent'),('ses_grandchild','ses_child');
              UPDATE session_v2 SET title = 'Review source' WHERE id = 'ses_child';
              UPDATE session_v2 SET title = 'Check tests' WHERE id = 'ses_grandchild';",
        )
        .unwrap();
    for (session_id, model, input, output, created) in [
        ("ses_parent", "claude-opus-4-6", 10, 2, 1000),
        ("ses_child", "claude-sonnet-4-6", 30, 5, 500),
        ("ses_grandchild", "claude-haiku-4-5", 40, 7, 3000),
    ] {
        let mut message = serde_json::json!({
            "agent":"build", "model":{"id":model,"providerID":"anthropic","variant":format!("native-{session_id}")},
            "time":{"created":created,"completed":created+500},
            "tokens":{"input":input,"output":output,"reasoning":3,"cache":{"read":0,"write":0}},
            "cost":0.01,
            "content":[{"type":"text","text":format!("Output from {session_id}")}]
        });
        if session_id == "ses_parent" {
            for (id, continued) in [("call-new", false), ("call-continued", true)] {
                let mut input = serde_json::json!({
                    "agent":"review","description":"Inspect code","prompt":"Review it","background":true
                });
                if continued {
                    input["sessionID"] = serde_json::json!("ses_child");
                }
                message["content"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({
                        "type":"tool","name":"subagent","id":id,
                        "time":{"created":1000,"ran":1100,"completed":1400},
                        "state":{
                            "status":"completed","input":input,
                            "metadata":{"sessionID":"ses_child","status":"running"},
                            "content":[{"type":"text","text":"Subagent is running in the background."}]
                        }
                    }));
            }
        }
        connection
            .execute(
                "INSERT INTO session_message (id,session_id,type,seq,data)
                 VALUES (?1,?2,'assistant',0,?3)",
                params![format!("msg_{session_id}"), session_id, message.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE session_v2 SET cost=?2,tokens_input=?3,tokens_output=?4,tokens_reasoning=3,
                 tokens_cache_read=0,tokens_cache_write=0 WHERE id=?1",
                params![
                    session_id,
                    if session_id == "ses_parent" {
                        0.02
                    } else {
                        0.01
                    },
                    input + if session_id == "ses_parent" { 5 } else { 0 },
                    output
                ],
            )
            .unwrap();
    }
    (directory, path)
}

async fn publish_fixture(
    directory: &std::path::Path,
    path: std::path::PathBuf,
) -> (Store, SessionKey, SessionAnalysis) {
    publish_fixture_with_prompts(directory, path, PublishedEvidence::Unsupported, &[]).await
}

async fn publish_fixture_with_prompts(
    directory: &std::path::Path,
    path: std::path::PathBuf,
    status: PublishedEvidence,
    prompt_ids: &[&str],
) -> (Store, SessionKey, SessionAnalysis) {
    let (latest, rows) = Explorers::DISK
        .provider_db_fingerprint(&AgentKind::OpenCode, &path, "ses_parent")
        .await
        .unwrap();
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    let store = Store::open_in_memory(directory).unwrap();
    for id in prompt_ids {
        store.lock().execute(
            "INSERT INTO remediation (remediation_id,target_key,environment_key,agent,scope_kind,scope_key,
                 state,definition_json,result_json,created_at_epoch,updated_at_epoch)
             VALUES (?1,?1,'native','opencode','project','scope','waitingForPromptUse',
                 '{\"version\":1}','{\"version\":1}',1,1)", [id],
        ).unwrap();
    }
    let key = SessionKey::new("native", "opencode", "ses_parent");
    let source = SessionSource::ProviderDb {
        agent: AgentKind::OpenCode,
        db_path: path,
        session_id: key.session_id.clone(),
        source_format: Some(SourceFormat::OpenCodeSqliteV2),
    };
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "providerDb".to_owned(),
                source_label: "opencode:ses_parent:open_code_v2_sqlite".to_owned(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: Some("/repo".to_owned()),
                surface: "cli".to_owned(),
                updated_at_epoch: Some(4),
                activity_cursor: String::new(),
                activity_source: "native".to_owned(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(fingerprint.clone()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&["opencode"], 10, 60)
        .unwrap()
        .unwrap();
    let row_store = Arc::new(FencedTurnRowStore::new(
        store.clone(),
        key.clone(),
        claim.claim_fence,
    ));
    let pass = analyze_located_for_evidence(
        AgentKind::OpenCode,
        &key.session_id,
        ClaimedSource {
            fingerprint: Some(fingerprint),
            generation: claim.source_generation,
        },
        PassSignal::new(),
        Some(row_store),
        None,
        LocatedTranscripts {
            source,
            children: Vec::new(),
        },
    )
    .await;
    assert_eq!(pass.outcome, PassOutcome::Published);
    let relations = pass
        .analysis
        .orchestration
        .as_ref()
        .into_iter()
        .flat_map(|status| &status.members)
        .map(|member| RelationRecord {
            kind: RelationKind::Subagent,
            related_id: member.subagent_id.clone(),
            label: Some(member.label.clone()),
        })
        .collect::<Vec<_>>();
    let completion = EvidenceCompletion {
        claim_fence: claim.claim_fence,
        status,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        evidence_json: serde_json::to_string(pass.evidence.as_ref().unwrap()).unwrap(),
    };
    assert!(
        store
            .publish_projections(
                &pass.analysis.record(&key).unwrap(),
                pass.analysis.started_at_epoch,
                &completion,
                &relations,
                &pass.source_outcomes,
            )
            .unwrap()
    );
    (store, key, pass.analysis)
}

#[tokio::test]
async fn opencode_v2_source_groups_publish_separate_costs_and_replay_child_drilldowns() {
    let (directory, path) = database();
    let (store, key, analysis) = publish_fixture(directory.path(), path).await;
    let live = &analysis;
    assert_eq!(live.started_at_epoch, Some(1));
    assert_eq!(live.inclusive_tokens.as_ref().unwrap().input_tokens, 80);
    assert_eq!(live.subagents_tokens.as_ref().unwrap().input_tokens, 70);
    let parent_cost = live.top_level_cost.unwrap().total_usd;
    let child_cost = live.subagents_cost.unwrap().total_usd;
    assert!(parent_cost > 0.0);
    assert!(child_cost > 0.0);
    assert!((live.cost.unwrap().total_usd - parent_cost - child_cost).abs() < 1e-12);
    let members = &live.orchestration.as_ref().unwrap().members;
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].subagent_id, "ses_child");
    assert_eq!(members[0].label, "Review source");
    assert_eq!(members[1].label, "Check tests");
    assert_eq!(members[0].tokens.as_ref().unwrap().input_tokens, 30);
    assert_eq!(members[1].tokens.as_ref().unwrap().input_tokens, 40);
    assert_eq!(live.source_summaries.as_ref().unwrap().len(), 3);
    let launches = |analysis: &SessionAnalysis| {
        analysis
            .metrics
            .as_ref()
            .unwrap()
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_launches)
            .sum::<u32>()
    };
    assert_eq!(launches(live), 1);
    let rows = store.published_turn_rows(&key).unwrap().unwrap();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.source_key, row.thread_id);
        assert_eq!(row.turn_index, 0);
        assert_eq!(
            row.subagent_launches,
            u32::from(row.source_key == "ses_parent")
        );
        assert_eq!(
            row.scope,
            if row.source_key == "ses_parent" {
                TurnScope::Main
            } else {
                TurnScope::Delegated
            }
        );
    }
    let replay = analysis_from_rows(&store, &key, &key.session_id, AgentKind::OpenCode).unwrap();
    assert_eq!(replay.started_at_epoch, live.started_at_epoch);
    assert_eq!(replay.inclusive_tokens, live.inclusive_tokens);
    assert_eq!(replay.subagents_tokens, live.subagents_tokens);
    assert_eq!(replay.cost, live.cost);
    assert_eq!(replay.top_level_cost, live.top_level_cost);
    assert_eq!(replay.subagents_cost, live.subagents_cost);
    assert_eq!(replay.model_runs, live.model_runs);
    let accounting = live.metrics.as_ref().unwrap().accounting.unwrap();
    assert_eq!(
        replay.metrics.as_ref().unwrap().accounting,
        Some(accounting)
    );
    assert_eq!(accounting.retained_tokens.unwrap().input_tokens, 80);
    assert_eq!(accounting.retained_tokens.unwrap().reasoning_tokens, 9);
    assert_eq!(
        accounting.retained_tokens.unwrap().visible_output_tokens,
        14
    );
    assert!(
        matches!(accounting.token_reconciliation(), antiburn_local::analysis::UsageReconciliation::Unallocated(tokens) if tokens.input_tokens == 5)
    );
    assert!(
        live.model_runs
            .iter()
            .all(|run| run.native_variant.is_some() && run.thinking_mode.is_none())
    );
    assert_eq!(launches(&replay), 1);
    for (id, input) in [("ses_child", 30), ("ses_grandchild", 40)] {
        let child =
            subagent_analysis_from_rows(&store, &key, &key.session_id, id, AgentKind::OpenCode)
                .unwrap();
        assert_eq!(child.inclusive_tokens.unwrap().input_tokens, input);
        assert_eq!(
            child.cost,
            members
                .iter()
                .find(|member| member.subagent_id == id)
                .unwrap()
                .cost
        );
    }
}

#[tokio::test]
async fn opencode_v2_empty_or_rejected_parent_replays_zero_one_and_multiple_children() {
    for rejected in [false, true] {
        for child_count in 0..=2 {
            let (directory, path) = database();
            let connection = Connection::open(&path).unwrap();
            connection
                .execute(
                    "DELETE FROM session_message WHERE session_id='ses_parent'",
                    [],
                )
                .unwrap();
            if rejected {
                connection.execute("INSERT INTO session_message (id,session_id,type,seq,data) VALUES ('msg_bad','ses_parent','assistant',0,'{')", []).unwrap();
            }
            if child_count < 2 {
                connection
                    .execute(
                        "DELETE FROM session_message WHERE session_id='ses_grandchild'",
                        [],
                    )
                    .unwrap();
                connection
                    .execute("DELETE FROM session_v2 WHERE id='ses_grandchild'", [])
                    .unwrap();
            }
            if child_count == 0 {
                connection
                    .execute(
                        "DELETE FROM session_message WHERE session_id='ses_child'",
                        [],
                    )
                    .unwrap();
                connection
                    .execute("DELETE FROM session_v2 WHERE id='ses_child'", [])
                    .unwrap();
            }
            drop(connection);
            let (store, key, live) = publish_fixture(directory.path(), path).await;
            let rows = store.published_turn_rows(&key).unwrap().unwrap();
            assert_eq!(rows.len(), child_count);
            assert!(rows.iter().all(|row| row.source_key != "ses_parent"));
            let replay =
                analysis_from_rows(&store, &key, "ses_parent", AgentKind::OpenCode).unwrap();
            assert_eq!(replay.metrics.as_ref().unwrap().event_count, child_count);
            assert_eq!(replay.summary.as_ref().unwrap().session_count, 1);
            assert_eq!(
                replay.summary.as_ref().unwrap().sessions[0].accounting,
                live.metrics.as_ref().unwrap().accounting
            );
            assert_eq!(
                replay.metrics.as_ref().unwrap().accounting,
                live.metrics.as_ref().unwrap().accounting
            );
            assert_eq!(replay.inclusive_tokens, live.inclusive_tokens);
            assert_eq!(replay.subagents_tokens, live.subagents_tokens);
            assert_eq!(replay.cost, live.cost);
            assert_eq!(replay.top_level_cost, live.top_level_cost);
            assert_eq!(replay.model_runs, live.model_runs);
            let parent = live.source_summaries.as_ref().unwrap()["ses_parent"]
                .accounting
                .unwrap();
            assert_eq!(parent.retained_tokens, Some(Default::default()));
            assert!(
                matches!(parent.token_reconciliation(), antiburn_local::analysis::UsageReconciliation::Unallocated(tokens) if tokens.input_tokens == 15)
            );
        }
    }
}

#[tokio::test]
async fn opencode_v2_summary_only_children_publish_replay_and_open_without_source_reads() {
    for empty_parent in [false, true] {
        for empty_children in [
            &["ses_child"][..],
            &["ses_grandchild"][..],
            &["ses_child", "ses_grandchild"][..],
        ] {
            let (directory, path) = database();
            let connection = Connection::open(&path).unwrap();
            for session_id in empty_children
                .iter()
                .copied()
                .chain(empty_parent.then_some("ses_parent"))
            {
                connection
                    .execute(
                        "DELETE FROM session_message WHERE session_id=?1",
                        [session_id],
                    )
                    .unwrap();
            }
            drop(connection);
            let (store, key, live) = publish_fixture(directory.path(), path.clone()).await;
            std::fs::remove_file(&path).unwrap();

            let rows = store.published_turn_rows(&key).unwrap().unwrap();
            let row_count = 3 - empty_children.len() - usize::from(empty_parent);
            assert_eq!(rows.len(), row_count);
            assert!(
                rows.iter()
                    .all(|row| !empty_children.contains(&row.source_key.as_str()))
            );
            let replay =
                analysis_from_rows(&store, &key, "ses_parent", AgentKind::OpenCode).unwrap();
            let live_metrics = live.metrics.as_ref().unwrap();
            let replay_metrics = replay.metrics.as_ref().unwrap();
            assert_eq!(live_metrics.event_count, row_count);
            assert_eq!(replay_metrics.event_count, row_count);
            assert_eq!(replay_metrics.accounting, live_metrics.accounting);
            assert_eq!(
                replay_metrics.token_reconciliation,
                live_metrics.token_reconciliation
            );
            assert_eq!(
                replay_metrics.cost_reconciliation,
                live_metrics.cost_reconciliation
            );
            assert_eq!(replay.inclusive_tokens, live.inclusive_tokens);
            assert_eq!(replay.subagents_tokens, live.subagents_tokens);
            assert_eq!(replay.cost, live.cost);
            assert_eq!(replay.top_level_cost, live.top_level_cost);
            assert_eq!(replay.subagents_cost, live.subagents_cost);
            assert_eq!(replay.model_runs, live.model_runs);
            assert_eq!(
                serde_json::to_value(&replay.orchestration).unwrap(),
                serde_json::to_value(&live.orchestration).unwrap()
            );
            let accounting = replay_metrics.accounting.unwrap();
            let retained_input = u64::from(!empty_parent) * 10
                + u64::from(!empty_children.contains(&"ses_child")) * 30
                + u64::from(!empty_children.contains(&"ses_grandchild")) * 40;
            assert_eq!(accounting.aggregate_tokens.unwrap().input_tokens, 85);
            assert_eq!(
                accounting.retained_tokens.unwrap().input_tokens,
                retained_input
            );
            assert!(
                matches!(accounting.token_reconciliation(), antiburn_local::analysis::UsageReconciliation::Unallocated(tokens) if tokens.input_tokens == 85 - retained_input)
            );
            assert!((accounting.aggregate_calculated_cost.unwrap().usd - 0.04).abs() < 1e-12);
            assert!(
                matches!(accounting.cost_reconciliation(), antiburn_local::analysis::UsageReconciliation::Unallocated(cost) if (cost.usd - (0.04 - row_count as f64 * 0.01)).abs() < 1e-12)
            );

            let summaries = live.source_summaries.as_ref().unwrap();
            assert_eq!(summaries.len(), 3);
            for (id, aggregate_input) in [("ses_child", 30), ("ses_grandchild", 40)] {
                let child = subagent_analysis_from_rows(
                    &store,
                    &key,
                    "ses_parent",
                    id,
                    AgentKind::OpenCode,
                )
                .unwrap();
                let metrics = child.metrics.as_ref().unwrap();
                let expected_accounting = summaries[id].accounting.unwrap();
                assert_eq!(metrics.accounting, Some(expected_accounting));
                assert_eq!(
                    metrics.token_reconciliation,
                    Some(expected_accounting.token_reconciliation())
                );
                assert_eq!(
                    metrics.cost_reconciliation,
                    Some(expected_accounting.cost_reconciliation())
                );
                let member = replay
                    .orchestration
                    .as_ref()
                    .unwrap()
                    .members
                    .iter()
                    .find(|member| member.subagent_id == id)
                    .unwrap();
                assert_eq!(child.inclusive_tokens, member.tokens);
                assert_eq!(child.cost, member.cost);
                assert_eq!(child.model_runs, member.model_runs);
                assert_eq!(child.started_at_epoch, member.started_at_epoch);
                if empty_children.contains(&id) {
                    assert_eq!(metrics.event_count, 0);
                    assert_eq!(child.inclusive_tokens.unwrap().input_tokens, 0);
                    assert!(child.model_runs.is_empty());
                    assert_eq!(
                        expected_accounting.retained_tokens,
                        Some(Default::default())
                    );
                    assert!(
                        matches!(expected_accounting.token_reconciliation(), antiburn_local::analysis::UsageReconciliation::Unallocated(tokens) if tokens.input_tokens == aggregate_input)
                    );
                    assert!(
                        matches!(expected_accounting.cost_reconciliation(), antiburn_local::analysis::UsageReconciliation::Unallocated(cost) if (cost.usd - 0.01).abs() < 1e-12)
                    );
                } else {
                    assert_eq!(metrics.event_count, 1);
                    assert_eq!(
                        child.inclusive_tokens.unwrap().input_tokens,
                        aggregate_input
                    );
                }
            }
            assert!(
                subagent_analysis_from_rows(
                    &store,
                    &key,
                    "ses_parent",
                    "ses_absent",
                    AgentKind::OpenCode,
                )
                .is_none()
            );
        }
    }
}

#[tokio::test]
async fn opencode_v2_compaction_accounting_matches_production_replay() {
    let (directory, path) = database();
    let connection = Connection::open(&path).unwrap();
    let compaction = serde_json::json!({
        "time":{"created":5000},"status":"completed","reason":"auto",
        "model":{"id":"claude-haiku-4-5","providerID":"anthropic","variant":"compact-native"},
        "tokens":{"input":20,"output":4,"reasoning":2,"cache":{"read":4,"write":1}},
        "cost":0.02,"summary":"private summary","recent":"opaque checkpoint"
    });
    connection.execute("INSERT INTO session_message (id,session_id,type,seq,data) VALUES ('msg_compact','ses_parent','compaction',1,?1)", [compaction.to_string()]).unwrap();
    connection.execute("UPDATE session_v2 SET cost=.04,tokens_input=35,tokens_output=6,tokens_reasoning=5,tokens_cache_read=4,tokens_cache_write=1 WHERE id='ses_parent'", []).unwrap();
    drop(connection);
    let (store, key, live) = publish_fixture(directory.path(), path).await;
    let replay = analysis_from_rows(&store, &key, "ses_parent", AgentKind::OpenCode).unwrap();
    assert_eq!(replay.inclusive_tokens, live.inclusive_tokens);
    assert_eq!(replay.cost, live.cost);
    assert_eq!(replay.model_runs, live.model_runs);
    assert_eq!(
        replay.metrics.as_ref().unwrap().accounting,
        live.metrics.as_ref().unwrap().accounting
    );
    let accounting = live.metrics.as_ref().unwrap().accounting.unwrap();
    assert_eq!(accounting.retained_tokens.unwrap().input_tokens, 100);
    assert_eq!(
        accounting.retained_tokens.unwrap().visible_output_tokens,
        18
    );
    assert_eq!(accounting.retained_tokens.unwrap().reasoning_tokens, 11);
    assert!((accounting.retained_calculated_cost.unwrap().usd - 0.05).abs() < 1e-12);
    assert_eq!(store.published_turn_rows(&key).unwrap().unwrap().len(), 4);
    assert!(
        live.model_runs
            .iter()
            .any(|run| run.native_variant.as_deref() == Some("compact-native"))
    );
}

#[tokio::test]
async fn opencode_v2_parser_to_store_markers_use_only_exact_accepted_user_text() {
    let (directory, path) = database();
    let connection = Connection::open(&path).unwrap();
    let marker = |id: &str| format!("Remediation reference: ABR-{id}");
    let messages = [
        (
            "user",
            serde_json::json!({"text":format!("{}\n{}",marker("exact"),marker("prefix-more")),
            "skills":[{"id":"guide","name":"guide","text":marker("injected")}]}),
        ),
        ("synthetic", serde_json::json!({"text":marker("synthetic")})),
        ("system", serde_json::json!({"text":marker("system")})),
        (
            "skill",
            serde_json::json!({"skill":"guide","name":"guide","text":marker("skill")}),
        ),
        (
            "assistant",
            serde_json::json!({"agent":"build","model":{"id":"claude-opus-4-6","providerID":"anthropic"},
            "content":[{"type":"text","text":marker("assistant")},{"type":"tool","id":"q-marker","name":"question",
                "time":{"created":6000,"ran":6000,"completed":6001},
                "state":{"status":"completed","input":{"questions":[{"question":"Select","header":"Scope","options":[{"label":"Small","description":"Small scope"}]}]},
                    "metadata":{"answers":[[marker("question")]]},"content":[{"type":"text","text":marker("question")}]}}]}),
        ),
    ];
    for (index, (kind, mut value)) in messages.into_iter().enumerate() {
        value["time"] = serde_json::json!({"created":6000});
        connection.execute("INSERT INTO session_message (id,session_id,type,seq,data) VALUES (?1,'ses_parent',?2,?3,?4)",
            params![format!("msg_marker_{index}"),kind,i64::try_from(index).unwrap()+10,value.to_string()]).unwrap();
    }
    drop(connection);
    // Exercise enrollment of saved prompts without enabling V2 recommendation gates.
    let (store, key, _) = publish_fixture_with_prompts(
        directory.path(),
        path,
        PublishedEvidence::Ready,
        &[
            "exact",
            "prefix",
            "injected",
            "synthetic",
            "system",
            "skill",
            "assistant",
            "question",
        ],
    )
    .await;
    assert_eq!(
        store.remediation("exact").unwrap().unwrap().state,
        crate::store::RemediationState::Watching
    );
    for id in [
        "prefix",
        "injected",
        "synthetic",
        "system",
        "skill",
        "assistant",
        "question",
    ] {
        assert_eq!(
            store.remediation(id).unwrap().unwrap().state,
            crate::store::RemediationState::WaitingForPromptUse,
            "{id}"
        );
    }
    let native_user_parts: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM turn_content c JOIN turn t ON t.rowid=c.turn_rowid
          WHERE t.session_id=?1 AND t.role='user' AND c.kind='user' AND c.authority='user'",
            [&key.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(native_user_parts, 1);
    let human_proofs: i64 = store.lock().query_row(
        "SELECT COUNT(*) FROM turn_content c JOIN turn t ON t.rowid=c.turn_rowid
          WHERE t.session_id=?1 AND json_extract(c.normalized_fields_json,'$.metadata.human_text') IS NOT NULL",
        [&key.session_id], |row| row.get(0),
    ).unwrap();
    assert_eq!(human_proofs, 0);
    let human_answers: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM turn_content c JOIN turn t ON t.rowid=c.turn_rowid,
             json_each(c.normalized_fields_json,'$.metadata.user_answers') answer
          WHERE t.session_id=?1 AND json_extract(answer.value,'$.origin')='user'",
            [&key.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(human_answers, 0);
}

#[tokio::test]
async fn opencode_v2_failed_compaction_keeps_usage_without_borrowing_the_parent_model() {
    let (directory, path) = database();
    let connection = Connection::open(&path).unwrap();
    let compaction = serde_json::json!({
        "time":{"created":5000},"status":"failed","reason":"manual",
        "error":{"type":"SyntheticFailure","message":"Synthetic compaction failure"},
        "tokens":{"input":20,"output":4,"reasoning":2,"cache":{"read":4,"write":1}},
        "cost":0.02
    });
    connection.execute("INSERT INTO session_message (id,session_id,type,seq,data) VALUES ('msg_failed_compact','ses_parent','compaction',1,?1)", [compaction.to_string()]).unwrap();
    connection.execute("UPDATE session_v2 SET cost=.04,tokens_input=35,tokens_output=6,tokens_reasoning=5,tokens_cache_read=4,tokens_cache_write=1 WHERE id='ses_parent'", []).unwrap();
    drop(connection);
    let (store, key, live) = publish_fixture(directory.path(), path).await;
    let replay = analysis_from_rows(&store, &key, "ses_parent", AgentKind::OpenCode).unwrap();
    assert_eq!(live.inclusive_tokens.as_ref().unwrap().input_tokens, 100);
    assert_eq!(live.inclusive_model_breakdown[""].input_tokens, 20);
    assert_eq!(
        live.inclusive_model_breakdown["claude-opus-4-6"].input_tokens,
        10
    );
    assert!(live.cost.is_none());
    assert_eq!(replay.inclusive_tokens, live.inclusive_tokens);
    assert_eq!(replay.cost, live.cost);
    assert_eq!(
        replay.inclusive_model_breakdown,
        live.inclusive_model_breakdown
    );
    assert_eq!(replay.model_runs, live.model_runs);
    assert_eq!(
        replay.metrics.as_ref().unwrap().accounting,
        live.metrics.as_ref().unwrap().accounting
    );
    assert_eq!(
        replay
            .metrics
            .as_ref()
            .unwrap()
            .accounting
            .unwrap()
            .retained_tokens
            .unwrap()
            .reasoning_tokens,
        11
    );
}
