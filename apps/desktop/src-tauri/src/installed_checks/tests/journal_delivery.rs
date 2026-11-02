use super::*;
use crate::insights_worker::journal::tests::{SyntheticJournal, run_page};
use crate::store::native_tool_results::evidence::{
    EvidenceEventKind, EvidencePublicationPayload, NativeEvidencePayload,
};

fn catch_up(host: &mut InstalledCheckHost<ScriptedEvaluator>) {
    for _ in 0..20 {
        if !host.catch_up().unwrap().1 {
            return;
        }
    }
    panic!("synthetic host catch-up exceeded its page budget");
}

fn setup(store: &Store, global: &Path, project: &Path) {
    store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "true")
        .unwrap();
    std::fs::create_dir_all(global).unwrap();
    std::fs::create_dir_all(project.join(".antiburn/checks")).unwrap();
    std::fs::write(global.join("failed-retry-loop.yml"), CHECK).unwrap();
    store.replace_repositories(&[repository(project)]).unwrap();
}

fn result_in_message(source: &mut SyntheticJournal, message: &str, index: usize) {
    let mut part = json!({"part":{
        "id":format!("part-{index}"), "messageID":message, "sessionID":"native-session",
        "type":"tool", "callID":format!("call-{index}"), "tool":"bash",
        "state":{"status":"running", "input":{"command":"cargo test parser"}}
    }});
    source.event("message.part.updated.1", part.clone());
    part["part"]["state"]["status"] = json!("error");
    part["part"]["state"]["error"] = json!(format!("Parser failure {index}"));
    source.event("message.part.updated.1", part);
}

#[tokio::test]
async fn fresh_journal_user_before_attachment_keeps_result_seed_before_mixed_selection() {
    for mixed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let global = root.join("global/checks");
        let project = root.join("project");
        let store = Arc::new(Store::open_in_memory(&root).unwrap());
        setup(&store, &global, &project);
        std::fs::write(global.join("failed-retry-loop.yml"), format!("name: Event startup\non:\n  tool_result:\n{}  window: {{size: 4, min: 3}}\n  cooldown: 0s\nactions:\n  - type: status\n    input: {{result: failure}}\n", if mixed { "  user_message:\n" } else { "" })).unwrap();
        let mut source = SyntheticJournal::new(16);
        source.results(0..3);
        source.index(&store, project.to_str().unwrap());
        let mut host = InstalledCheckHost::new(
            store.clone(),
            Arc::new(ScriptedEvaluator::default()),
            Some(global),
        )
        .unwrap();
        host.reconcile(&Admission::Ready).unwrap();
        assert!(run_page(&store, source.source(), 100).await);
        source.event(
            "message.updated.1",
            json!({"info":{"id":"fresh-user","role":"user"}}),
        );
        source.event("message.part.updated.1", json!({"part":{"id":"fresh-text","messageID":"fresh-user","type":"text","text":"Fresh user work"}}));
        source.index(&store, project.to_str().unwrap());
        assert!(run_page(&store, source.source(), 101).await);
        catch_up(&mut host);
        super::assert_result_seed_before_fresh_user(&store, mixed);
    }
}

#[tokio::test]
async fn report_counts_remaining_trigger_events_and_returns_to_waiting_after_a_check() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let global = root.join("global/checks");
    let project = root.join("project");
    let store = Arc::new(Store::open(&root.join("state")).unwrap());
    setup(&store, &global, &project);
    let mut source = SyntheticJournal::new(16);
    source.index(&store, project.to_str().unwrap());
    let evaluator = Arc::new(ScriptedEvaluator::default());
    let mut host = InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global)).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(run_page(&store, source.source(), 100).await);
    catch_up(&mut host);
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET pending = 1 WHERE session_id = 'native-session'",
            [],
        )
        .unwrap();
    let report = read_installed_checks(&store, &host.definitions).unwrap();
    assert_eq!(report[0].waiting_for_events.unwrap().count, 3);
    assert!(report[0].waiting_for_events.unwrap().catching_up);
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET pending = 0 WHERE session_id = 'native-session'",
            [],
        )
        .unwrap();
    let report = read_installed_checks(&store, &host.definitions).unwrap();
    assert!(!report[0].waiting_for_events.unwrap().catching_up);
    for (index, remaining) in [(0, 2), (1, 1), (2, 1)] {
        source.start_result(index);
        source.result(index);
        source.index(&store, project.to_str().unwrap());
        store.lock().execute(
            "UPDATE session SET updated_at_epoch = ?1
             WHERE environment_key = 'native' AND agent = 'opencode' AND session_id = 'native-session'",
            [time::OffsetDateTime::now_utc().unix_timestamp() - if index == 2 { 3600 } else { 120 }],
        ).unwrap();
        assert!(run_page(&store, source.source(), 101 + index as i64).await);
        catch_up(&mut host);
        let report = read_installed_checks(&store, &host.definitions).unwrap();
        assert_eq!(report[0].waiting_for_events.unwrap().count, remaining);
        assert_eq!(report[0].checking_count, u64::from(index == 2));
    }
    evaluator.answer("clear");
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
    let report = read_installed_checks(&store, &host.definitions).unwrap();
    assert_eq!(report[0].clean, 1);
    assert_eq!(report[0].checking_count, 0);
    let waiting = report[0].waiting_for_events.unwrap();
    assert_eq!(waiting.count, 1);
    assert!(!waiting.catching_up);
}

#[tokio::test]
async fn large_history_starts_at_the_live_frontier_and_appends_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let global = root.join("global/checks");
    let project = root.join("project");
    let state = root.join("state");
    let store = Arc::new(Store::open(&state).unwrap());
    setup(&store, &global, &project);
    let request_text = "x".repeat(16 * 1024);
    let mut source = SyntheticJournal::new(request_text.len());
    for index in 0..300 {
        let assistant = format!("historical-assistant-{}", index / 20);
        if index % 20 == 0 {
            source.event(
                "message.updated.1",
                json!({"info":{"id":assistant,"sessionID":"native-session","role":"assistant",
                    "parentID":"request","time":{"completed":1000}}}),
            );
        }
        source.event(
            "message.part.updated.1",
            json!({"part":{
                "id":format!("part-{index}"),"messageID":assistant,"sessionID":"native-session",
                "type":"tool","callID":format!("call-{index}"),"tool":"bash",
                "state":{"status":"error","input":{"command":"cargo test parser"},
                    "error":format!("Parser failure {index}")}
            }}),
        );
    }
    let public_frontier = source.sequence as u64;
    source.index(&store, project.to_str().unwrap());
    let evaluator = Arc::new(ScriptedEvaluator::default());
    let mut host =
        InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global.clone())).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    let activation = host.projects.values().next().unwrap().active[0].clone();
    assert_eq!(activation.bootstrap_publication.0, 0);
    assert!(run_page(&store, source.source(), 100).await);
    assert_eq!(store.next_opencode_journal_attempt(100).unwrap(), Some(100));
    assert!(run_page(&store, source.source(), 100).await);
    assert_eq!(store.next_opencode_journal_attempt(100).unwrap(), Some(101));
    let cursor: String = store
        .lock()
        .query_row(
            "SELECT cursor_json FROM opencode_journal_delivery",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let cursor: antiburn_local::analysis::OpenCodeJournalCursor =
        serde_json::from_str(&cursor).unwrap();
    assert_eq!(cursor.public_baseline(), Some(public_frontier));
    catch_up(&mut host);
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
    assert!(evaluator.inputs.lock().unwrap().is_empty());
    drop(host);
    drop(store);

    let store = Arc::new(Store::open(&state).unwrap());
    let mut host =
        InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global.clone())).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    assert_eq!(host.projects.values().next().unwrap().active[0], activation);
    let assistant = "appended-assistant";
    source.event(
        "message.updated.1",
        json!({"info":{"id":assistant, "sessionID":"native-session", "role":"assistant", "parentID":"request"}}),
    );
    for index in 300..303 {
        result_in_message(&mut source, assistant, index);
    }
    source.index(&store, project.to_str().unwrap());
    assert!(run_page(&store, source.source(), 101).await);
    catch_up(&mut host);
    assert_eq!(store.next_opencode_journal_attempt(110).unwrap(), Some(110));
    let counts: (i64, i64, i64) = store
        .lock()
        .query_row(
            "SELECT COUNT(*), SUM(live_eligible = 0), SUM(live_eligible = 1)
           FROM native_tool_result_publication WHERE outcome = 'update' AND event_kind = 'tool_result'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (46, 0, 46));
    let mut publications = Vec::new();
    let mut after = 0;
    let mut exhausted = false;
    for _ in 0..20 {
        let page = store
            .native_evidence_publications_after(after, 1000)
            .unwrap();
        let Some(last) = page.last() else {
            exhausted = true;
            break;
        };
        assert!(last.publication_sequence > after);
        after = last.publication_sequence;
        publications.extend(page);
    }
    assert!(exhausted, "synthetic evidence read exceeds its page budget");
    let calls = publications
        .iter()
        .filter_map(|publication| match publication.payload.as_ref()? {
            EvidencePublicationPayload::Native(NativeEvidencePayload::PublicMessage(message))
                if publication.event_kind == EvidenceEventKind::ToolCall
                    && message.record_id == assistant =>
            {
                Some((publication, message))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!calls.is_empty());
    assert_eq!(
        calls
            .iter()
            .map(|(_, message)| message.call_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("call-300"), Some("call-301"), Some("call-302")]
    );
    for (publication, message) in calls {
        assert_eq!(message.record_id, assistant);
        assert_eq!(message.session_id, "native-session");
        assert!(message.eligible);
        assert_eq!(publication.live_eligible, Some(true));
        assert!(matches!(
            message.context.as_ref().unwrap().first_eligible_position,
            antiburn_local::analysis::ToolResultPosition::OpenCodeEvent { sequence, .. }
                if sequence > public_frontier
        ));
    }
    let resumed_cursor: String = store
        .lock()
        .query_row(
            "SELECT cursor_json FROM opencode_journal_delivery",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let resumed_cursor: antiburn_local::analysis::OpenCodeJournalCursor =
        serde_json::from_str(&resumed_cursor).unwrap();
    assert_eq!(resumed_cursor.public_baseline(), cursor.public_baseline());
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_current WHERE is_removed = 1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    evaluator.answer("stuck");
    let outcome = host
        .evaluation_jobs(1)
        .pop()
        .unwrap()
        .run()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.outcome, RunOutcome::Accepted);
    let input = evaluator.inputs.lock().unwrap()[0].clone();
    assert_eq!(input["requests"].as_array().unwrap().len(), 1);
    assert_eq!(input["requests"][0]["text"], request_text);
    assert_eq!(input["activity"].as_array().unwrap().len(), 5);
    let work = input["activity"].to_string();
    for index in 300..303 {
        assert!(work.contains(&format!("Parser failure {index}")));
    }
    assert!(work.contains("Parser failure 299"));
    assert!(!work.contains("Parser failure 297"));
    assert_eq!(input["source_coverage"], "partial");
    let status = status_writes(&store, &host.definitions[0]);
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].0, "failure");
    store.lock().execute("UPDATE session_evidence SET status = 'failed', last_error = 'source-unreadable:no-events'", []).unwrap();
    let report = read_installed_checks(&store, &host.definitions).unwrap();
    assert_eq!(
        (
            report[0].finding,
            report[0].clean,
            report[0].waiting_for_data_count
        ),
        (1, 0, 0)
    );
    assert_eq!(report[0].targets.len(), 1);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let evidence = crate::insights_report::installed_checks::read_evidence(
        &store,
        &host.definitions,
        &crate::insights_report::ReportRequest {
            environment_key: "native".into(),
            window: antiburn_local::insights::ReportWindow {
                start_epoch: now - 30 * 86_400,
                end_epoch: now + 1,
            },
            computed_at_epoch: now,
        },
        &report[0].targets[0].id,
    )
    .unwrap();
    assert_eq!(
        evidence.status,
        crate::remediation::BurnCheckEvidenceStatus::Available
    );
    drop(host);
    drop(store);

    let store = Arc::new(Store::open(&state).unwrap());
    let mut host = InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global)).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    catch_up(&mut host);
    assert!(run_page(&store, source.source(), 200).await);
    assert_eq!(evaluator.inputs.lock().unwrap().len(), 1);
    assert_eq!(status_writes(&store, &host.definitions[0]), status);
    result_in_message(&mut source, assistant, 303);
    source.index(&store, project.to_str().unwrap());
    assert!(run_page(&store, source.source(), 201).await);
    catch_up(&mut host);
    evaluator.answer("clear");
    let now_ms =
        u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap();
    assert_eq!(
        run_due_at(&mut host, now_ms + 61_000).await.outcome,
        RunOutcome::Accepted
    );
    assert_eq!(evaluator.inputs.lock().unwrap().len(), 2);
    assert_eq!(status_writes(&store, &host.definitions[0])[0].0, "success");
    let report = read_installed_checks(&store, &host.definitions).unwrap();
    assert_eq!(
        (
            report[0].finding,
            report[0].clean,
            report[0].waiting_for_data_count
        ),
        (0, 1, 0)
    );
}

#[tokio::test]
async fn unsafe_journal_observation_keeps_selection_partial_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let global = root.join("global/checks");
    let project = root.join("project");
    let state = root.join("state");
    let store = Arc::new(Store::open(&state).unwrap());
    setup(&store, &global, &project);
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, project.to_str().unwrap());
    let evaluator = Arc::new(ScriptedEvaluator::default());
    let mut host =
        InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global.clone())).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(run_page(&store, source.source(), 100).await);
    catch_up(&mut host);
    source.event(
        "message.part.removed.1",
        json!({"sessionID":"native-session","messageID":"wrong-message","partID":"part-0"}),
    );
    for index in 1..4 {
        source.start_result(index);
        source.result(index);
    }
    source.index(&store, project.to_str().unwrap());
    assert!(run_page(&store, source.source(), 101).await);
    catch_up(&mut host);
    evaluator.answer("stuck");
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
    assert_eq!(
        evaluator.inputs.lock().unwrap()[0]["source_coverage"],
        "partial"
    );
    assert_eq!(
        read_installed_checks(&store, &host.definitions).unwrap()[0].finding,
        1
    );
    drop(host);
    drop(store);

    let store = Arc::new(Store::open(&state).unwrap());
    let mut host = InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global)).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    let project_checks = host.projects.values().next().unwrap();
    assert!(
        project_checks
            .store
            .load_source_progress(&project_checks.active[0])
            .unwrap()
            .selection
            .branches
            .values()
            .any(|branch| !branch.coverage_complete)
    );
    source.start_result(4);
    source.result(4);
    source.index(&store, project.to_str().unwrap());
    assert!(run_page(&store, source.source(), 102).await);
    catch_up(&mut host);
    evaluator.answer("uncertain");
    let now_ms =
        u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap();
    run_due_at(&mut host, now_ms + 61_000).await;
    assert_eq!(
        evaluator.inputs.lock().unwrap()[1]["source_coverage"],
        "partial"
    );
    let project_checks = host.projects.values().next().unwrap();
    assert!(
        project_checks
            .store
            .load_source_progress(&project_checks.active[0])
            .unwrap()
            .selection
            .branches
            .values()
            .any(|branch| !branch.coverage_complete)
    );
}

#[tokio::test]
async fn message_removal_does_not_block_new_tool_results_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let global = root.join("global/checks");
    let project = root.join("project");
    let state = root.join("state");
    let store = Arc::new(Store::open(&state).unwrap());
    setup(&store, &global, &project);
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, project.to_str().unwrap());
    let evaluator = Arc::new(ScriptedEvaluator::default());
    let mut host =
        InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global.clone())).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(run_page(&store, source.source(), 100).await);
    catch_up(&mut host);
    assert_eq!(
        read_installed_checks(&store, &host.definitions).unwrap()[0]
            .waiting_for_events
            .unwrap()
            .count,
        2
    );

    for message in ["request", "assistant"] {
        source.event("message.removed.1", json!({"messageID": message}));
    }
    source.index(&store, project.to_str().unwrap());
    assert!(run_page(&store, source.source(), 101).await);
    catch_up(&mut host);
    assert_eq!(
        read_installed_checks(&store, &host.definitions).unwrap()[0]
            .waiting_for_events
            .unwrap()
            .count,
        3
    );
    drop(host);
    drop(store);

    let store = Arc::new(Store::open(&state).unwrap());
    let mut host = InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global)).unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    source.event(
        "message.updated.1",
        json!({"info":{"id":"new-request","role":"user"}}),
    );
    source.event(
        "message.part.updated.1",
        json!({"part":{"id":"new-text","messageID":"new-request","type":"text","text":"Fix the parser"}}),
    );
    source.event(
        "message.updated.1",
        json!({"info":{"id":"new-assistant","role":"assistant","parentID":"new-request"}}),
    );
    for index in 1..=3 {
        for status in ["running", "error"] {
            source.event(
                "message.part.updated.1",
                json!({"part":{
                    "id":format!("new-part-{index}"),"messageID":"new-assistant",
                    "type":"tool","callID":format!("new-call-{index}"),"tool":"bash",
                    "state":{"status":status,"input":{"command":"cargo test parser"},"error":format!("New parser failure {index}")}
                }}),
            );
        }
    }
    source.index(&store, project.to_str().unwrap());
    assert!(run_page(&store, source.source(), 102).await);
    catch_up(&mut host);
    let report = read_installed_checks(&store, &host.definitions).unwrap();
    assert_eq!(report[0].checking_count, 1);
    assert!(report[0].has_started);
    evaluator.answer("stuck");
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
    let inputs = evaluator.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0]["activity"].as_array().unwrap().len(), 3);
    assert_eq!(inputs[0]["source_coverage"], "partial");
    assert!(
        !inputs[0]["activity"]
            .to_string()
            .contains("Parser failure 0")
    );
    assert_eq!(status_writes(&store, &host.definitions[0])[0].0, "failure");
    assert_eq!(
        read_installed_checks(&store, &host.definitions).unwrap()[0].finding,
        1
    );
}

fn legacy_projection_status(store: &Store, status: &str) {
    store.lock().execute(
        "INSERT INTO session_evidence (environment_key, agent, session_id, status,
            analyzed_generation, parser_revision, analyzer_revision, evidence_schema_revision)
         VALUES ('native', 'opencode', 'native-session', ?1,
            (SELECT source_generation FROM session WHERE agent = 'opencode' AND session_id = 'native-session'), ?2, ?3, ?4)
         ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
            status = excluded.status, analyzed_generation = excluded.analyzed_generation,
            parser_revision = excluded.parser_revision, analyzer_revision = excluded.analyzer_revision,
            evidence_schema_revision = excluded.evidence_schema_revision",
        rusqlite::params![status, antiburn_local::analysis::PARSER_REVISION,
            antiburn_local::analysis::ANALYZER_REVISION, antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION],
    ).unwrap();
}

#[tokio::test]
async fn blocked_journal_delivery_is_unavailable_regardless_of_legacy_projection_status() {
    use crate::store::native_tool_results::source_delivery::opencode_journal::JournalFailure;
    for failure in [JournalFailure::InvalidCursor, JournalFailure::SourceChanged] {
        for legacy_status in ["pending", "ready", "failed"] {
            let directory = tempfile::tempdir().unwrap();
            let root = std::fs::canonicalize(directory.path()).unwrap();
            let global = root.join("global/checks");
            let project = root.join("project");
            let store = Arc::new(Store::open_in_memory(&root).unwrap());
            setup(&store, &global, &project);
            let source = SyntheticJournal::new(16);
            source.index(&store, project.to_str().unwrap());
            legacy_projection_status(&store, legacy_status);
            let mut host = InstalledCheckHost::new(
                store.clone(),
                Arc::new(ScriptedEvaluator::default()),
                Some(global),
            )
            .unwrap();
            host.reconcile(&Admission::Ready).unwrap();
            assert!(run_page(&store, source.source(), 100).await);
            let claim = store.claim_next_opencode_journal(105, 60).unwrap().unwrap();
            assert!(
                store
                    .fail_opencode_journal(&claim, failure, 105, 110)
                    .unwrap()
            );
            assert_eq!(store.next_opencode_journal_attempt(110).unwrap(), None);
            let captured = LogCapture(Arc::default(), "installed_check_source_state");
            tracing::subscriber::with_default(captured.clone(), || catch_up(&mut host));
            let report = read_installed_checks(&store, &host.definitions).unwrap();
            let row = &report[0];
            assert_eq!(
                (
                    row.finding,
                    row.clean,
                    row.unavailable,
                    row.checking_count,
                    row.waiting_for_data_count,
                    row.waiting_for_results_count
                ),
                (0, 0, 1, 0, 0, 0),
                "{legacy_status}"
            );
            assert!(row.waiting_for_events.is_none());
            assert_eq!(row.diagnostics[0].code, "source-unavailable");
            assert_eq!(row.diagnostics[0].affected_session_count, 1);
            assert!(row.diagnostics[0].title.contains("journal"));
            let serialized = serde_json::to_string(row).unwrap();
            assert!(!serialized.contains("journal-prefix-changed"));
            assert!(!serialized.contains("journal-cursor-invalid"));
            assert!(
                captured
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|log| log.contains("state=\"source_unavailable\""))
            );

            store
                .lock()
                .execute(
                    "UPDATE session SET incarnation = 8 WHERE agent = 'opencode'",
                    [],
                )
                .unwrap();
            assert!(run_page(&store, source.source(), 110).await);
            catch_up(&mut host);
            let repaired = read_installed_checks(&store, &host.definitions).unwrap();
            assert!(repaired[0].diagnostics.is_empty());
            assert_eq!(
                (
                    repaired[0].finding,
                    repaired[0].clean,
                    repaired[0].waiting_for_results_count
                ),
                (0, 0, 1)
            );
        }
    }
}

#[tokio::test]
async fn journal_retries_wait_only_while_delivery_metadata_has_more_work() {
    use crate::store::native_tool_results::source_delivery::opencode_journal::JournalFailure;
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let global = root.join("global/checks");
    let project = root.join("project");
    let store = Arc::new(Store::open_in_memory(&root).unwrap());
    setup(&store, &global, &project);
    let source = SyntheticJournal::new(16);
    source.index(&store, project.to_str().unwrap());
    legacy_projection_status(&store, "failed");
    let mut host = InstalledCheckHost::new(
        store.clone(),
        Arc::new(ScriptedEvaluator::default()),
        Some(global),
    )
    .unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    let before = read_installed_checks(&store, &host.definitions).unwrap();
    assert_eq!(before[0].waiting_for_data_count, 1);
    assert!(before[0].diagnostics.is_empty());
    assert!(run_page(&store, source.source(), 100).await);
    for attempt in 1..=5 {
        let now = 100 + attempt;
        let claim = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
        assert!(
            store
                .fail_opencode_journal(&claim, JournalFailure::ReadFailed, now, now + 1)
                .unwrap()
        );
        let captured = LogCapture(Arc::default(), "installed_check_source_state");
        tracing::subscriber::with_default(captured.clone(), || catch_up(&mut host));
        let report = read_installed_checks(&store, &host.definitions).unwrap();
        let row = &report[0];
        assert_eq!((row.finding, row.clean, row.unavailable), (0, 0, 1));
        assert_eq!(row.waiting_for_data_count, u64::from(attempt < 5));
        assert_eq!(row.waiting_for_results_count, 0);
        assert_eq!(
            row.waiting_for_events.map(|waiting| waiting.catching_up),
            (attempt < 5).then_some(true)
        );
        if attempt < 5 {
            assert!(row.diagnostics.is_empty());
            assert_eq!(
                store.next_opencode_journal_attempt(now).unwrap(),
                Some(now + 1)
            );
            if attempt == 1 {
                assert!(
                    captured
                        .0
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|log| log.contains("state=\"waiting_for_data\""))
                );
            }
        } else {
            assert_eq!(row.diagnostics[0].code, "source-unavailable");
            assert_eq!(store.next_opencode_journal_attempt(now).unwrap(), None);
            assert!(
                captured
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|log| log.contains("state=\"source_unavailable\""))
            );
        }
    }
}
