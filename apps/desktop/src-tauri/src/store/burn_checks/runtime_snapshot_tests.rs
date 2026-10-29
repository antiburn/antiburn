use super::*;
use crate::store::SessionKey;
use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION, SourceFormat,
};

fn cursor_snapshot(store: &Store) -> SessionKey {
    store
        .lock()
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
           cwd, updated_at_epoch, first_seen_at, last_seen_at, source_generation, source_format)
         VALUES ('native', 'cursor', 'snapshot', 'inline', 'cursor-desktop:snapshot',
           '/repo', 100, 'now', 'now', 3, ?1)",
            [serde_json::to_string(&SourceFormat::CursorIdeComposer).unwrap()],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id, status,
           analyzed_generation, parser_revision, analyzer_revision, evidence_schema_revision)
         VALUES ('native', 'cursor', 'snapshot', 'ready', 3, ?1, ?2, ?3)",
            params![PARSER_REVISION, ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION],
        )
        .unwrap();
    SessionKey::new("native", "cursor", "snapshot")
}

fn table_count(store: &Store, table: &str) -> i64 {
    store
        .lock()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn source_state(store: &Store) -> String {
    store
        .lock()
        .query_row(
            "SELECT json_array(s.incarnation, s.source_kind, s.source_label, s.source_generation,
           s.source_format, e.status, e.analyzed_generation, e.parser_revision,
           e.analyzer_revision, e.evidence_schema_revision)
         FROM session s LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
         WHERE s.agent = 'cursor'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn cursor_snapshot_attachment_does_not_create_session_observers_or_delivery() {
    let (store, adapter, active) = fixture();
    let key = cursor_snapshot(&store);
    let mut sessions = adapter.startup_sessions(&active, 0).unwrap();
    assert_eq!(sessions.len(), 1);
    let session = sessions.pop().unwrap();
    assert_eq!(session.key, key);
    assert_eq!(session.receipt, "3");
    let before = source_state(&store);

    let outcome = adapter
        .initialize_recent(&active, &session, &[])
        .unwrap()
        .unwrap();
    assert_eq!((outcome.observations, outcome.candidates), (0, 0));
    assert!(
        store
            .lock()
            .query_row(
                "SELECT observer_epoch IS NULL AND source_label IS NULL AND source_format IS NULL
           AND source_receipt IS NULL AND observation_started_at_ms IS NULL
           AND idle_deadline_ms IS NULL AND start_publication IS NULL AND continuity_eligible = 0
         FROM burn_check_session_attachment WHERE branch_key = ?1",
                [key.to_identity_json()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    assert_eq!(table_count(&store, "burn_check_session_attachment"), 1);
    assert_eq!(
        adapter.initialized_branches(&active).unwrap(),
        [BranchId(key.to_identity_json())].into_iter().collect()
    );
    assert!(adapter.startup_sessions(&active, 0).unwrap().is_empty());
    assert!(
        adapter
            .initialize_recent(&active, &session, &[])
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .initialize_session_observers(&active.id, 100_000)
            .unwrap(),
        0
    );
    assert_eq!(store.next_session_idle_deadline().unwrap(), None);
    assert_eq!(store.request_due_session_idle_catchup(500_000).unwrap(), 0);
    assert_eq!(source_state(&store), before);
    for table in [
        "native_file_delivery",
        "opencode_journal_delivery",
        "opencode_v2_capture",
        "native_tool_result_publication",
        "burn_check_observation",
        "burn_check_candidate",
        "burn_check_evaluation_attempt",
        "burn_check_decision_usage",
    ] {
        assert_eq!(table_count(&store, table), 0, "{table}");
    }
}

#[test]
fn cursor_snapshot_attachment_rechecks_source_scope_and_current_cache() {
    for sql in [
        "UPDATE session SET source_kind = 'file'",
        "UPDATE session SET source_label = 'cursor-cli:snapshot'",
        "UPDATE session SET cwd = '/other-project'",
        "UPDATE session SET wsl_distro = 'synthetic'",
        "INSERT INTO session_relation (environment_key, agent, session_id, kind, related_id)
         VALUES ('native', 'cursor', 'snapshot', 'forkParent', 'parent')",
        "INSERT INTO session_relation (environment_key, agent, session_id, kind, related_id)
         VALUES ('native', 'cursor', 'parent', 'subagent', 'snapshot')",
        "UPDATE session SET source_generation = source_generation + 1",
        "UPDATE session_evidence SET analyzed_generation = analyzed_generation + 1",
        "UPDATE session_evidence SET status = 'failed'",
        "UPDATE session_evidence SET parser_revision = 0",
        "UPDATE session_evidence SET analyzer_revision = 0",
        "UPDATE session_evidence SET evidence_schema_revision = 0",
    ] {
        let (store, adapter, active) = fixture();
        cursor_snapshot(&store);
        let session = adapter.startup_sessions(&active, 0).unwrap().pop().unwrap();
        let before = adapter
            .load_source_progress(&active)
            .unwrap()
            .encode()
            .unwrap();
        store.lock().execute_batch(sql).unwrap();
        assert!(
            adapter.startup_sessions(&active, 0).unwrap().is_empty(),
            "{sql}"
        );
        assert!(
            adapter
                .initialize_recent(&active, &session, &[])
                .unwrap()
                .is_none(),
            "{sql}"
        );
        assert_eq!(
            adapter
                .load_source_progress(&active)
                .unwrap()
                .encode()
                .unwrap(),
            before,
            "{sql}"
        );
        for table in [
            "burn_check_session_attachment",
            "native_file_delivery",
            "opencode_journal_delivery",
            "native_tool_result_publication",
            "burn_check_candidate",
            "burn_check_evaluation_attempt",
        ] {
            assert_eq!(table_count(&store, table), 0, "{sql}: {table}");
        }
    }
}

#[test]
fn native_file_snapshot_attaches_an_observer_without_changing_the_source_receipt() {
    use antiburn_local::analysis::file_live::read_native_file_page;
    use antiburn_local::analysis::{RawSource, SessionInput};

    let (store, adapter, active) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"session\",\"version\":3,\"id\":\"snapshot\",\"cwd\":\"/repo\"}\n",
    )
    .unwrap();
    let page = read_native_file_page(
        &SessionInput {
            agent: "pi".into(),
            session_id: "snapshot".into(),
            source: RawSource::File(path.clone()),
            source_format: SourceFormat::PiV3Jsonl,
            fork_parent_session_id: None,
        },
        None,
        &|| false,
    )
    .unwrap();
    assert!(!page.has_more);
    let receipt = serde_json::to_string(&page.cursor).unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
           cwd, updated_at_epoch, first_seen_at, last_seen_at, source_format)
         VALUES ('native', 'pi', 'snapshot', 'file', ?1, '/repo', 100, 'now', 'now', ?2)",
            params![
                path.to_string_lossy(),
                serde_json::to_string(&SourceFormat::PiV3Jsonl).unwrap()
            ],
        )
        .unwrap();
    store.lock().execute(
        "INSERT INTO native_file_delivery (environment_key, agent, session_id, incarnation,
           source_label, cwd, parser_revision, cursor_json, observed_generation, pending, active_until_epoch)
         SELECT environment_key, agent, session_id, incarnation, source_label, cwd, ?1, ?2,
           source_generation, 0, 0 FROM session",
        params![PARSER_REVISION, receipt],
    ).unwrap();
    let sessions = adapter.startup_sessions(&active, 0).unwrap();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.receipt, receipt);
    assert!(
        adapter
            .initialize_recent(&active, session, &[])
            .unwrap()
            .is_some()
    );
    let (observer, saved_receipt): (bool, String) = store
        .lock()
        .query_row(
            "SELECT observer_epoch IS NOT NULL AND continuity_eligible = 1, source_receipt
         FROM burn_check_session_attachment",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(observer);
    assert_eq!(saved_receipt, receipt);
    let source_receipt: String = store
        .lock()
        .query_row("SELECT cursor_json FROM native_file_delivery", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(source_receipt, receipt);
    assert_eq!(table_count(&store, "burn_check_evaluation_attempt"), 0);
    assert_eq!(table_count(&store, "burn_check_decision_usage"), 0);
}

#[test]
fn unknown_snapshot_source_is_not_admitted_or_silently_attached_without_an_observer() {
    let (store, adapter, active) = fixture();
    cursor_snapshot(&store);
    let mut session = adapter.startup_sessions(&active, 0).unwrap().pop().unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
           cwd, updated_at_epoch, first_seen_at, last_seen_at, incarnation)
         VALUES ('native', 'unknown', 'snapshot', 'file', 'unknown-source',
           '/repo', 100, 'now', 'now', ?1)",
            [session.incarnation],
        )
        .unwrap();
    let error = store.lock().execute(
        "INSERT INTO native_file_delivery (environment_key, agent, session_id, incarnation,
           source_label, cwd, parser_revision, cursor_json, observed_generation, pending, active_until_epoch)
         SELECT environment_key, agent, session_id, incarnation, source_label, cwd, ?1, ?2,
           source_generation, 0, 0 FROM session WHERE agent = 'unknown'",
        params![PARSER_REVISION, session.receipt],
    ).unwrap_err();
    assert!(matches!(error, rusqlite::Error::SqliteFailure(error, _)
        if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_CHECK));
    assert_eq!(table_count(&store, "native_file_delivery"), 0);
    assert_eq!(
        adapter
            .startup_sessions(&active, 0)
            .unwrap()
            .iter()
            .map(|session| session.key.agent.as_str())
            .collect::<Vec<_>>(),
        ["cursor"]
    );
    session.key.agent = "unknown".into();
    let before = adapter.load_source_progress(&active).unwrap();
    let source_before = source_state(&store);
    assert!(
        adapter
            .initialize_recent(&active, &session, &[])
            .unwrap()
            .is_none()
    );
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .encode()
            .unwrap(),
        before.encode().unwrap()
    );
    assert_eq!(source_state(&store), source_before);
    assert_eq!(table_count(&store, "native_file_delivery"), 0);
    for table in [
        "burn_check_session_attachment",
        "native_tool_result_publication",
        "burn_check_observation",
        "burn_check_candidate",
        "burn_check_evaluation_attempt",
        "burn_check_decision_usage",
    ] {
        assert_eq!(table_count(&store, table), 0, "{table}");
    }
}
