use std::path::Path;

use antiburn_local::analysis::{ContentKind, ContentPart, TurnSessionKey, insert_turn_rows};

use crate::store::{BurnCheckInput, SessionKey, SessionRecord, Store};

const NOW: i64 = 1_000_000;
const WEEK: i64 = 7 * 24 * 60 * 60;
const MONTH: i64 = 30 * 24 * 60 * 60;

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-history-check-test")).unwrap()
}

fn session(id: &str, updated: i64) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "claude-code", id),
        source_kind: "file".into(),
        source_label: format!("/synthetic/{id}.jsonl"),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        client: "unknown".into(),
        updated_at_epoch: Some(updated),
        activity_cursor: id.into(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn add_session_content(store: &Store, record: &SessionRecord) {
    let mut row = super::turn_row(0);
    row.source_key = record.key.session_id.clone();
    row.thread_id = row.source_key.clone();
    row.content = vec![ContentPart::new(
        ContentKind::AssistantText,
        "synthetic session action",
    )];
    let key = TurnSessionKey {
        environment_key: &record.key.environment_key,
        agent: &record.key.agent,
        session_id: &record.key.session_id,
    };
    insert_turn_rows(&store.lock(), &key, 1, &[row]).unwrap();
}

#[test]
fn recent_history_uses_last_activity_even_for_old_sessions() {
    let store = store();
    store
        .upsert_sessions(
            &[
                session("at-cutoff", NOW - WEEK),
                session("before-cutoff", NOW - WEEK - 1),
                session("old-but-active", NOW - 1),
                session("future", NOW + 1),
            ],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    add_session_content(&store, &session("at-cutoff", NOW - WEEK));
    add_session_content(&store, &session("old-but-active", NOW - 1));
    store
        .lock()
        .execute(
            "UPDATE session SET first_seen_at = '2020-01-01T00:00:00Z'
             WHERE session_id = 'old-but-active'",
            [],
        )
        .unwrap();
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 2 * WEEK)
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET boundary_positions_json = '{\"old-source\":7}'",
            [],
        )
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 2);
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 0);
    assert_eq!(
        store
            .historical_burn_check_status(NOW, 180)
            .unwrap()
            .waiting_for_data,
        2
    );
    for id in ["at-cutoff", "old-but-active"] {
        let assessment = store
            .burn_check_assessment(
                &SessionKey::new("native", "claude-code", id),
                "ignored_instructions",
            )
            .unwrap()
            .unwrap();
        assert_eq!(assessment.status, "idle");
    }
    let positions: String = store
        .lock()
        .query_row(
            "SELECT boundary_positions_json FROM burn_check_assessment WHERE session_id = 'old-but-active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(positions, r#"{"*":0}"#);
    assert_eq!(
        store
            .burn_check_assessment(
                &SessionKey::new("native", "claude-code", "before-cutoff"),
                "ignored_instructions"
            )
            .unwrap()
            .unwrap()
            .status,
        "idle"
    );
}

#[test]
fn terminal_evidence_is_not_reported_as_waiting_for_analysis() {
    let store = store();
    let records = [
        session("terminal-failed", NOW - 1),
        session("terminal-unsupported", NOW - 1),
    ];
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();
    for record in &records {
        add_session_content(&store, record);
    }
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 10)
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = CASE session_id
                 WHEN 'terminal-failed' THEN 'failed' ELSE 'unsupported' END
              WHERE agent = 'claude-code' AND session_id LIKE 'terminal-%'",
            [],
        )
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 2);
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET status = 'queued'
              WHERE session_id LIKE 'terminal-%'",
            [],
        )
        .unwrap();

    let status = store.historical_burn_check_status(NOW, 180).unwrap();
    assert_eq!(status.waiting_for_data, 0);
    assert_eq!(status.queued, 0);
    assert_eq!(status.failed, 1);
    assert_eq!(status.skipped, 1);
    assert_eq!(status.total, 2);
}

#[test]
fn thirty_day_history_uses_the_selected_activity_window() {
    let store = store();
    let records = [
        session("inside-month", NOW - 29 * 24 * 60 * 60),
        session("at-cutoff", NOW - MONTH),
        session("outside-month", NOW - MONTH - 1),
    ];
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();
    add_session_content(&store, &records[0]);
    add_session_content(&store, &records[1]);
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - MONTH - WEEK)
        .unwrap();

    assert_eq!(store.enqueue_burn_checks(NOW, 30).unwrap(), 2);
    assert_eq!(
        store
            .historical_burn_check_status(NOW, 180)
            .unwrap()
            .waiting_for_data,
        2
    );
}

#[test]
fn history_does_not_queue_sessions_without_saved_content() {
    let store = store();
    store
        .upsert_sessions(
            &[session("empty-session", NOW - 200)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 300)
        .unwrap();

    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 0);
    assert_eq!(
        store
            .historical_burn_check_status(NOW, 180)
            .unwrap()
            .waiting_for_data,
        0
    );
}

#[test]
fn selected_history_session_reaches_the_running_worker_state() {
    let store = store();
    let record = session("worker-candidate", NOW - 200);
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    add_session_content(&store, &record);
    let generation = store
        .lock()
        .query_row(
            "SELECT source_generation FROM session WHERE session_id = 'worker-candidate'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'ready', analyzed_generation = ?1,
             parser_revision = ?2, analyzer_revision = ?3,
             evidence_schema_revision = ?4, published_fence = 1
             WHERE session_id = 'worker-candidate'",
            rusqlite::params![
                generation,
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::ANALYZER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
            ],
        )
        .unwrap();
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - WEEK)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 1);
    assert_eq!(
        store
            .historical_burn_check_status(NOW + 180, 180)
            .unwrap()
            .ready,
        1
    );

    let candidate = store
        .burn_check_candidates("ignored_instructions", NOW + 180, 180, 16)
        .unwrap()
        .pop()
        .expect("selected idle session is available to the worker");
    assert!(candidate.historical);
    let input = BurnCheckInput {
        key: candidate.session.key.clone(),
        check_id: "ignored_instructions".to_owned(),
        incarnation: candidate.incarnation,
        source_generation: candidate.source_generation,
        source_fingerprint: candidate.source_fingerprint.clone(),
        activity_cursor: candidate.activity_cursor.clone(),
        published_fence: candidate.published_fence,
        input_revision: "synthetic-revision".to_owned(),
        evaluator_revision: antiburn_local::analysis::ignored_instructions::evaluator_revision(),
        boundary_at_epoch: candidate.boundary_at_epoch,
    };
    assert!(
        store
            .queue_burn_check_assessment(&input, NOW + 180, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, NOW + 180, 300, 180)
            .unwrap()
    );
    assert_eq!(
        store
            .burn_check_assessment(&input.key, "ignored_instructions")
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    assert!(
        store
            .complete_burn_check_assessment(&input, "{}", NOW + 181, 180)
            .unwrap()
    );
    assert!(
        store
            .burn_check_candidates("ignored_instructions", NOW + 181, 180, 16)
            .unwrap()
            .is_empty()
    );
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET evaluator_revision = 'old-revision'
              WHERE session_id = 'worker-candidate'",
            [],
        )
        .unwrap();
    assert!(
        store
            .burn_check_candidates("ignored_instructions", NOW + 181, 180, 16)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn historical_window_rejects_unsupported_day_counts() {
    let store = store();
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW)
        .unwrap();

    assert!(store.enqueue_burn_checks(NOW, 0).is_err());
    assert!(store.enqueue_burn_checks(NOW, 14).is_err());
}

#[test]
fn recent_history_does_not_start_without_authorization_or_after_key_removal() {
    let store = store();
    store
        .upsert_sessions(&[session("recent", NOW)], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(store.enqueue_burn_checks(NOW, 7).is_err());
    assert!(
        store
            .burn_check_candidates("ignored_instructions", NOW + 200, 180, 16)
            .unwrap()
            .is_empty()
    );
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW)
        .unwrap();
    store.disable_burn_checks().unwrap();
    assert!(store.enqueue_burn_checks(NOW, 7).is_err());
    assert!(
        store
            .burn_check_candidates("ignored_instructions", NOW + 200, 180, 16)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recent_history_selection_and_status_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let record = session("recent", NOW - 200);
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    add_session_content(&store, &record);
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 300)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 1);
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    assert_eq!(
        reopened
            .historical_burn_check_status(NOW, 180)
            .unwrap()
            .waiting_for_data,
        1
    );
    assert_eq!(reopened.enqueue_burn_checks(NOW, 7).unwrap(), 0);
}

#[test]
fn legacy_usage_limit_remains_visible_and_repeat_click_preserves_retry() {
    let store = store();
    store
        .upsert_sessions(
            &[session("paused", NOW - 200)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    add_session_content(&store, &session("paused", NOW - 200));
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 300)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 1);
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET status = 'failed',
             last_error_category = 'usage_limit', next_attempt_at_epoch = ?1,
             evaluator_revision = ?2
             WHERE session_id = 'paused'",
            rusqlite::params![
                NOW + 24 * 60 * 60,
                antiburn_local::analysis::ignored_instructions::evaluator_revision()
            ],
        )
        .unwrap();
    assert_eq!(
        store.historical_burn_check_status(NOW, 180).unwrap().failed,
        1
    );
    assert_eq!(store.enqueue_burn_checks(NOW + 1, 7).unwrap(), 0);
    assert_eq!(store.enqueue_burn_checks(NOW + 24 * 60 * 60, 7).unwrap(), 1);
    let connection = store.lock();
    let retry_at: i64 = connection
        .query_row(
            "SELECT next_attempt_at_epoch FROM burn_check_assessment
             WHERE session_id = 'paused'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retry_at, NOW + 24 * 60 * 60);
}

#[test]
fn completed_history_requeues_on_request_after_a_new_evaluator_revision() {
    let store = store();
    let record = session("completed-history", NOW - 200);
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    add_session_content(&store, &record);
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 300)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 1);
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment
                SET status = 'completed',
                    evaluator_revision = ?1,
                    source_generation = (SELECT source_generation FROM session WHERE session_id = ?2),
                    source_fingerprint = (SELECT source_fingerprint FROM session WHERE session_id = ?2),
                    incarnation = (SELECT incarnation FROM session WHERE session_id = ?2),
                    boundary_activity_cursor = (SELECT activity_cursor FROM session WHERE session_id = ?2),
                    boundary_at_epoch = ?3,
                    boundary_positions_json = ?4,
                    updated_at_epoch = ?5
              WHERE session_id = ?2",
            rusqlite::params![
                antiburn_local::analysis::ignored_instructions::evaluator_revision(),
                record.key.session_id,
                NOW - 10,
                "{\"some-source\":5444}",
                NOW,
            ],
        )
        .unwrap();

    assert_eq!(store.enqueue_burn_checks(NOW + 1, 7).unwrap(), 0);
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET evaluator_revision = 'old-revision'
              WHERE session_id = ?1",
            [&record.key.session_id],
        )
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW + 2, 7).unwrap(), 1);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT status FROM burn_check_assessment WHERE session_id = ?1",
                [&record.key.session_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "idle"
    );
    let connection = store.lock();
    let (boundary, positions): (i64, String) = connection
        .query_row(
            "SELECT boundary_at_epoch, boundary_positions_json FROM burn_check_assessment
             WHERE session_id = ?1",
            [&record.key.session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(boundary, NOW + 2 - 7 * 24 * 60 * 60);
    assert_eq!(positions, "{\"*\":0}");
}

#[test]
fn historical_failures_keep_retry_delay_after_evaluator_changes() {
    let store = store();
    let record = session("revision-retry", NOW - 200);
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    add_session_content(&store, &record);
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 300)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 1);
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment
                SET status = 'failed', last_error_category = 'invalid_response',
                    evaluator_revision = ?1, next_attempt_at_epoch = ?2
              WHERE session_id = ?3",
            rusqlite::params![
                antiburn_local::analysis::ignored_instructions::evaluator_revision(),
                NOW + 24 * 60 * 60,
                record.key.session_id,
            ],
        )
        .unwrap();
    assert!(
        store
            .burn_check_candidates("ignored_instructions", NOW + 1, 0, 7)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.enqueue_burn_checks(NOW + 1, 7).unwrap(), 0);

    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET evaluator_revision = 'old-revision'
              WHERE session_id = ?1",
            [&record.key.session_id],
        )
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW + 2, 7).unwrap(), 0);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT status FROM burn_check_assessment WHERE session_id = ?1",
                [&record.key.session_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .as_str(),
        "failed"
    );
}

#[test]
fn historical_status_counts_only_the_latest_selected_window() {
    let store = store();
    let records = [
        session("recent", NOW - 2 * 24 * 60 * 60),
        session("older", NOW - 10 * 24 * 60 * 60),
    ];
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();
    for record in &records {
        add_session_content(&store, record);
    }
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - MONTH)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 30).unwrap(), 2);
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment
                SET status = 'completed', evaluator_revision = ?1,
                    source_generation = (SELECT source_generation FROM session s
                        WHERE s.session_id = burn_check_assessment.session_id),
                    source_fingerprint = (SELECT source_fingerprint FROM session s
                        WHERE s.session_id = burn_check_assessment.session_id),
                    incarnation = (SELECT incarnation FROM session s
                        WHERE s.session_id = burn_check_assessment.session_id),
                    boundary_activity_cursor = (SELECT activity_cursor FROM session s
                        WHERE s.session_id = burn_check_assessment.session_id)
              WHERE check_id = 'ignored_instructions'",
            [antiburn_local::analysis::ignored_instructions::evaluator_revision()],
        )
        .unwrap();

    assert_eq!(store.enqueue_burn_checks(NOW + 1, 7).unwrap(), 0);
    let status = store.historical_burn_check_status(NOW + 1, 180).unwrap();
    assert_eq!(status.total, 1);
    assert_eq!(status.completed, 1);
    assert_eq!(status.waiting_for_data, 0);
}

#[test]
fn recent_history_waits_for_active_sessions_and_survives_worker_pages() {
    let store = store();
    let records = (0..33)
        .map(|index| session(&format!("session-{index:02}"), NOW - 10))
        .collect::<Vec<_>>();
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();
    for record in &records {
        add_session_content(&store, record);
    }
    {
        let connection = store.lock();
        connection
            .execute(
                "UPDATE session_evidence SET status = 'ready',
                 analyzed_generation = (SELECT source_generation FROM session s
                    WHERE s.environment_key = session_evidence.environment_key
                      AND s.agent = session_evidence.agent
                      AND s.session_id = session_evidence.session_id),
                  parser_revision = ?1, analyzer_revision = ?2,
                  evidence_schema_revision = ?3,
                  published_fence = 1",
                rusqlite::params![
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::ANALYZER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION
                ],
            )
            .unwrap();
    }
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 100)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 33);
    let progress = store.historical_burn_check_status(NOW, 180).unwrap();
    assert_eq!(progress.waiting_for_idle, 33);
    assert_eq!(progress.ready, 0);
    assert!(
        store
            .burn_check_candidates("ignored_instructions", NOW, 180, 16)
            .unwrap()
            .is_empty()
    );
    let first = store
        .burn_check_candidates("ignored_instructions", NOW + 180, 180, 16)
        .unwrap();
    assert_eq!(first.len(), 16);
    assert_eq!(
        store
            .historical_burn_check_status(NOW + 180, 180)
            .unwrap()
            .ready,
        33
    );
    for candidate in &first {
        assert!(candidate.historical);
        store
            .lock()
            .execute(
                "UPDATE burn_check_assessment SET status = 'superseded'
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = 'ignored_instructions' AND boundary_generation = -2
                AND status = 'idle'",
                rusqlite::params![
                    candidate.session.key.environment_key,
                    candidate.session.key.agent,
                    candidate.session.key.session_id
                ],
            )
            .unwrap();
    }
    let second = store
        .burn_check_candidates("ignored_instructions", NOW + 180, 180, 16)
        .unwrap();
    assert_eq!(second.len(), 16);
    assert!(second.iter().all(|candidate| {
        !first
            .iter()
            .any(|previous| previous.session.key == candidate.session.key)
    }));
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment
                SET status = 'failed', last_error_category = 'continuing'
              WHERE session_id = ?1",
            [&second[0].session.key.session_id],
        )
        .unwrap();
    let progress = store.historical_burn_check_status(NOW + 180, 180).unwrap();
    assert_eq!(progress.total, 33);
    assert_eq!(progress.running, 1);
    assert_eq!(progress.failed, 0);
    assert_eq!(
        progress.ready + progress.skipped + progress.completed + progress.queued + progress.running,
        progress.total,
        "continuing page work stays in the selected denominator"
    );
}

#[test]
fn continuing_instruction_pages_count_as_in_progress_work() {
    let store = store();
    let records = [session("continuing", NOW - 10), session("failed", NOW - 10)];
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();
    for record in &records {
        add_session_content(&store, record);
    }
    store
        .capture_burn_check_boundaries(&["ignored_instructions"], NOW - 100)
        .unwrap();
    assert_eq!(store.enqueue_burn_checks(NOW, 7).unwrap(), 2);
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment
                SET status = 'failed', last_error_category = CASE session_id
                    WHEN 'continuing' THEN 'continuing' ELSE 'assessment_failed' END",
            [],
        )
        .unwrap();

    assert_eq!(
        store
            .burn_check_in_progress_count("ignored_instructions")
            .unwrap(),
        1
    );
}
