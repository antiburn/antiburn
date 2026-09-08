use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, params};

use super::*;

const JSON_V1: &str = r#"{"version":1}"#;

fn remediation(
    remediation_id: &str,
    target_key: &str,
    baseline_session_id: &str,
    created_at_epoch: i64,
) -> Remediation {
    Remediation {
        remediation_id: remediation_id.into(),
        target_key: target_key.into(),
        origin: RemediationOrigin::Antiburn,
        environment_key: "native".into(),
        agent: "claude-code".into(),
        source_format: "claudeJsonl".into(),
        workspace_key: "/home/avery/code/widgets".into(),
        baseline_session_id: baseline_session_id.into(),
        baseline_source_generation: 1,
        baseline_published_fence: 1,
        baseline_source_fingerprint: Some("sv1:baseline".into()),
        baseline_processed_fingerprint: Some("sv1:baseline".into()),
        baseline_parser_revision: 1,
        baseline_analyzer_revision: 1,
        baseline_evidence_schema_revision: 1,
        finding_json: JSON_V1.into(),
        change_json: JSON_V1.into(),
        boundary_json: JSON_V1.into(),
        verification_json: JSON_V1.into(),
        savings_json: JSON_V1.into(),
        revisions_json: JSON_V1.into(),
        created_at_epoch,
        applied_at_epoch: Some(created_at_epoch),
    }
}

fn seed_baseline(store: &Store, session_id: &str, updated_at_epoch: i64) {
    let mut record = session(session_id, updated_at_epoch);
    record.source_fingerprint = Some("sv1:baseline".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET status = 'ready', analyzed_generation = 1,
                    processed_fingerprint = 'sv1:baseline', parser_revision = 1,
                    analyzer_revision = 1, evidence_schema_revision = 1,
                    evidence_json = ?1, published_fence = 1
              WHERE environment_key = 'native' AND agent = 'claude-code'
                AND session_id = ?2",
            params![JSON_V1, session_id],
        )
        .unwrap();
}

fn evidence_version() -> RemediationEvidenceVersion {
    RemediationEvidenceVersion {
        source_generation: 1,
        published_fence: 1,
        source_fingerprint: Some("sv1:baseline".into()),
        processed_fingerprint: Some("sv1:baseline".into()),
        parser_revision: 1,
        analyzer_revision: 1,
        evidence_schema_revision: 1,
    }
}

fn result(state: RemediationState, evaluated_at_epoch: i64) -> RemediationResult {
    RemediationResult {
        state,
        verification_json: JSON_V1.into(),
        savings_json: JSON_V1.into(),
        evaluated_at_epoch,
    }
}

fn table_names(connection: &Connection) -> HashSet<String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn v40_adds_only_the_strict_remediation_table_and_required_indexes() {
    let connection = Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..39] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 39).unwrap();
    let tables_before = table_names(&connection);

    let store = Store::from_connection(connection, Path::new("/tmp/remediation-v40").into())
        .expect("migrate v39 to v40");
    assert_eq!(store.schema_version().unwrap(), 40);

    let connection = store.lock();
    let tables_after = table_names(&connection);
    let added_tables = tables_after
        .difference(&tables_before)
        .cloned()
        .collect::<HashSet<_>>();
    assert_eq!(added_tables, HashSet::from(["remediation".to_string()]));
    let strict: i64 = connection
        .query_row(
            "SELECT strict FROM pragma_table_list WHERE name = 'remediation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(strict, 1);

    let indexes = [
        "remediation_dirty",
        "remediation_page",
        "remediation_active_target",
    ];
    for index in indexes {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
                [index],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "missing {index}");
    }
    let transition_trigger_exists: bool = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master
                  WHERE type = 'trigger' AND name = 'remediation_state_transition')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(transition_trigger_exists);
}

#[test]
fn remediation_constraints_reject_invalid_state_and_versioned_json() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    store
        .insert_awaiting_remediation(&remediation("r1", "target", "baseline", 10))
        .unwrap();

    let connection = store.lock();
    assert!(
        connection
            .execute(
                "UPDATE remediation SET state = 'fixed' WHERE remediation_id = 'r1'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE remediation SET baseline_parser_revision = 0
                  WHERE remediation_id = 'r1'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE remediation SET baseline_source_fingerprint = ''
                  WHERE remediation_id = 'r1'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE remediation SET finding_json = '{}' WHERE remediation_id = 'r1'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE remediation SET change_json = ?1 WHERE remediation_id = 'r1'",
                [format!(
                    r#"{{"version":1,"value":"{}"}}"#,
                    "x".repeat(65_536)
                )],
            )
            .is_err()
    );
}

#[test]
fn guarded_insert_rejects_stale_evidence_and_duplicate_active_targets() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    assert!(
        store
            .insert_awaiting_remediation(&remediation("r1", "target", "baseline", 10))
            .unwrap()
    );
    assert!(
        !store
            .insert_awaiting_remediation(&remediation("r2", "target", "baseline", 20))
            .unwrap()
    );

    assert!(
        store
            .replace_remediation_result(
                "r1",
                1,
                &evidence_version(),
                &result(RemediationState::Verified, 30),
            )
            .unwrap()
    );
    assert_eq!(
        store
            .reconcile_remediation_revisions(r#"{"version":2}"#, 35)
            .unwrap(),
        1
    );
    assert!(
        store
            .replace_remediation_result(
                "r1",
                2,
                &evidence_version(),
                &result(RemediationState::Recurred, 40),
            )
            .unwrap()
    );
    assert!(
        store
            .insert_awaiting_remediation(&remediation("r2", "target", "baseline", 40))
            .unwrap()
    );

    let mut stale = remediation("r3", "other-target", "baseline", 50);
    stale.baseline_published_fence = 2;
    assert!(!store.insert_awaiting_remediation(&stale).unwrap());
}

#[test]
fn guarded_insert_checks_each_ready_evidence_identity_value() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    let mut stale_inputs = Vec::new();
    let mut stale = remediation("generation", "generation", "baseline", 10);
    stale.baseline_source_generation = 2;
    stale_inputs.push(stale);
    let mut stale = remediation("fence", "fence", "baseline", 10);
    stale.baseline_published_fence = 2;
    stale_inputs.push(stale);
    let mut stale = remediation("source", "source", "baseline", 10);
    stale.baseline_source_fingerprint = Some("sv1:other".into());
    stale_inputs.push(stale);
    let mut stale = remediation("processed", "processed", "baseline", 10);
    stale.baseline_processed_fingerprint = Some("sv1:other".into());
    stale_inputs.push(stale);
    let mut stale = remediation("parser", "parser", "baseline", 10);
    stale.baseline_parser_revision = 2;
    stale_inputs.push(stale);
    let mut stale = remediation("analyzer", "analyzer", "baseline", 10);
    stale.baseline_analyzer_revision = 2;
    stale_inputs.push(stale);
    let mut stale = remediation("evidence", "evidence", "baseline", 10);
    stale.baseline_evidence_schema_revision = 2;
    stale_inputs.push(stale);

    for stale in stale_inputs {
        assert!(!store.insert_awaiting_remediation(&stale).unwrap());
    }
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'unsupported'
              WHERE environment_key = 'native' AND agent = 'claude-code'
                AND session_id = 'baseline'",
            [],
        )
        .unwrap();
    assert!(
        !store
            .insert_awaiting_remediation(&remediation("status", "status", "baseline", 10))
            .unwrap()
    );
}

#[test]
fn stale_verification_result_cannot_replace_newer_inputs() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    store
        .insert_awaiting_remediation(&remediation("r1", "target", "baseline", 10))
        .unwrap();
    let observed = store.next_dirty_remediation().unwrap().unwrap();
    assert_eq!(observed.verification_input_revision, 1);

    assert_eq!(
        store
            .reconcile_remediation_revisions(r#"{"version":2}"#, 20)
            .unwrap(),
        1
    );
    assert!(
        !store
            .replace_remediation_result(
                "r1",
                observed.verification_input_revision,
                &evidence_version(),
                &RemediationResult {
                    state: RemediationState::Verified,
                    verification_json: r#"{"version":1,"result":"stale"}"#.into(),
                    savings_json: JSON_V1.into(),
                    evaluated_at_epoch: 30,
                },
            )
            .unwrap()
    );
    assert!(
        store
            .replace_remediation_result(
                "r1",
                2,
                &evidence_version(),
                &RemediationResult {
                    state: RemediationState::Verified,
                    verification_json: r#"{"version":1,"result":"current"}"#.into(),
                    savings_json: JSON_V1.into(),
                    evaluated_at_epoch: 40,
                },
            )
            .unwrap()
    );
    let saved = store.remediation("r1").unwrap().unwrap();
    assert_eq!(saved.state, RemediationState::Verified);
    assert_eq!(saved.evaluated_input_revision, 2);
    assert_eq!(saved.verified_at_epoch, Some(40));
    assert!(store.next_dirty_remediation().unwrap().is_none());
}

#[test]
fn result_replacement_enforces_evidence_guards_and_state_transitions() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    assert!(
        store
            .insert_awaiting_remediation(&remediation("r1", "target", "baseline", 10))
            .unwrap()
    );
    assert!(
        store
            .insert_awaiting_remediation(&remediation("r2", "other-target", "baseline", 10))
            .unwrap()
    );
    assert!(
        store
            .replace_remediation_result(
                "r2",
                1,
                &evidence_version(),
                &result(RemediationState::AwaitingVerification, 20),
            )
            .unwrap()
    );

    let mut stale_versions = Vec::new();
    let mut stale = evidence_version();
    stale.source_generation = 2;
    stale_versions.push(stale);
    let mut stale = evidence_version();
    stale.published_fence = 2;
    stale_versions.push(stale);
    let mut stale = evidence_version();
    stale.source_fingerprint = Some("sv1:other".into());
    stale_versions.push(stale);
    let mut stale = evidence_version();
    stale.processed_fingerprint = Some("sv1:other".into());
    stale_versions.push(stale);
    let mut stale = evidence_version();
    stale.parser_revision = 2;
    stale_versions.push(stale);
    let mut stale = evidence_version();
    stale.analyzer_revision = 2;
    stale_versions.push(stale);
    let mut stale = evidence_version();
    stale.evidence_schema_revision = 2;
    stale_versions.push(stale);

    for stale in stale_versions {
        assert!(
            !store
                .replace_remediation_result(
                    "r1",
                    1,
                    &stale,
                    &result(RemediationState::Verified, 20),
                )
                .unwrap()
        );
    }
    assert!(
        !store
            .replace_remediation_result(
                "r1",
                1,
                &evidence_version(),
                &result(RemediationState::Recurred, 20),
            )
            .unwrap()
    );

    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'pending'
              WHERE environment_key = 'native' AND agent = 'claude-code'
                AND session_id = 'baseline'",
            [],
        )
        .unwrap();
    assert!(
        !store
            .replace_remediation_result(
                "r1",
                1,
                &evidence_version(),
                &result(RemediationState::Verified, 20),
            )
            .unwrap()
    );
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'ready'
              WHERE environment_key = 'native' AND agent = 'claude-code'
                AND session_id = 'baseline'",
            [],
        )
        .unwrap();
    assert!(
        store
            .replace_remediation_result(
                "r1",
                1,
                &evidence_version(),
                &result(RemediationState::Verified, 20),
            )
            .unwrap()
    );

    store
        .reconcile_remediation_revisions(r#"{"version":2}"#, 30)
        .unwrap();
    assert!(
        !store
            .replace_remediation_result(
                "r1",
                2,
                &evidence_version(),
                &result(RemediationState::AwaitingVerification, 40),
            )
            .unwrap()
    );
    assert!(
        store
            .replace_remediation_result(
                "r1",
                2,
                &evidence_version(),
                &result(RemediationState::Recurred, 40),
            )
            .unwrap()
    );
}

#[test]
fn v40_rejects_inconsistent_timestamps_and_state_timestamps() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    assert!(
        store
            .insert_awaiting_remediation(&remediation("r1", "target", "baseline", 10))
            .unwrap()
    );
    let connection = store.lock();
    for sql in [
        "UPDATE remediation SET created_at_epoch = -1 WHERE remediation_id = 'r1'",
        "UPDATE remediation SET updated_at_epoch = 9 WHERE remediation_id = 'r1'",
        "UPDATE remediation SET applied_at_epoch = 11 WHERE remediation_id = 'r1'",
        "UPDATE remediation SET verified_at_epoch = 10 WHERE remediation_id = 'r1'",
        "UPDATE remediation SET state = 'verified' WHERE remediation_id = 'r1'",
    ] {
        assert!(connection.execute(sql, []).is_err(), "accepted {sql}");
    }
}

#[test]
fn reconciliation_and_retry_pending_transitions_mark_remediations_dirty() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    assert!(
        store
            .insert_awaiting_remediation(&remediation("r1", "target", "baseline", 10))
            .unwrap()
    );
    assert_eq!(
        store
            .reconcile_evidence_revisions(
                &["claude-code"],
                ProjectionRevisions {
                    parser_revision: 2,
                    analyzer_revision: 1,
                    metrics_schema_revision: 1,
                    evidence_schema_revision: 1,
                },
            )
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .remediation("r1")
            .unwrap()
            .unwrap()
            .verification_input_revision,
        2
    );
    store
        .lock()
        .execute(
            "UPDATE remediation SET evaluated_input_revision = verification_input_revision
              WHERE remediation_id = 'r1'",
            [],
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();
    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Retry {
                    next_attempt_at_epoch: 200,
                    counts_as_attempt: true,
                },
                "retry",
            )
            .unwrap()
    );
    assert_eq!(
        store
            .remediation("r1")
            .unwrap()
            .unwrap()
            .verification_input_revision,
        3
    );
}

#[test]
fn remediation_pages_use_a_bounded_exclusive_keyset() {
    let store = store();
    seed_baseline(&store, "baseline", 1_000);
    for index in 0..102 {
        store
            .insert_awaiting_remediation(&remediation(
                &format!("r{index:03}"),
                &format!("target-{index:03}"),
                "baseline",
                10,
            ))
            .unwrap();
    }

    let default_page = store.remediations("native", None, None).unwrap();
    assert_eq!(default_page.remediations.len(), 50);
    let second_page = store
        .remediations("native", default_page.next_cursor.as_ref(), None)
        .unwrap();
    assert_eq!(second_page.remediations.len(), 50);
    assert!(default_page.remediations.iter().all(|left| {
        second_page
            .remediations
            .iter()
            .all(|right| left.remediation_id != right.remediation_id)
    }));

    let capped_page = store.remediations("native", None, Some(1_000)).unwrap();
    assert_eq!(capped_page.remediations.len(), 100);
    assert!(capped_page.next_cursor.is_some());

    let mut other_environment = session("other-baseline", 1_000);
    other_environment.key.environment_key = "wsl:ubuntu".into();
    other_environment.source_fingerprint = Some("sv1:baseline".into());
    store
        .upsert_sessions(&[other_environment], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET status = 'ready', analyzed_generation = 1,
                    processed_fingerprint = 'sv1:baseline', parser_revision = 1,
                    analyzer_revision = 1, evidence_schema_revision = 1,
                    evidence_json = ?1, published_fence = 1
              WHERE environment_key = 'wsl:ubuntu' AND agent = 'claude-code'
                AND session_id = 'other-baseline'",
            [JSON_V1],
        )
        .unwrap();
    let mut other_remediation = remediation("other-r", "other", "other-baseline", 20);
    other_remediation.environment_key = "wsl:ubuntu".into();
    assert!(
        store
            .insert_awaiting_remediation(&other_remediation)
            .unwrap()
    );
    assert!(
        store
            .remediations("native", None, Some(100))
            .unwrap()
            .remediations
            .iter()
            .all(|record| record.environment_key == "native")
    );
}

#[test]
fn pending_source_advances_and_winning_publication_mark_remediations_dirty() {
    let losing_store = store();
    seed_baseline(&losing_store, "losing", 1_000);
    assert!(
        losing_store
            .insert_awaiting_remediation(&remediation("losing-r", "target", "losing", 10))
            .unwrap()
    );
    let losing_key = SessionKey::new("native", "claude-code", "losing");
    losing_store.requeue_session_evidence(&losing_key).unwrap();
    let losing_claim = losing_store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();
    let losing_record = projection_record(losing_key, "sv1:baseline", 1);
    losing_store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                losing_record.key.environment_key,
                losing_record.key.agent,
                losing_record.key.session_id
            ],
        )
        .unwrap();
    let losing_completion = evidence_completion(
        &losing_claim,
        PublishedEvidence::Ready,
        r#"{"version":1}"#.into(),
    );
    assert!(
        !losing_store
            .publish_projections(&losing_record, None, &losing_completion, &[], &[])
            .unwrap()
    );
    assert_eq!(
        losing_store
            .remediation("losing-r")
            .unwrap()
            .unwrap()
            .verification_input_revision,
        2
    );

    let winning_store = store();
    seed_baseline(&winning_store, "winning", 1_000);
    assert!(
        winning_store
            .insert_awaiting_remediation(&remediation("winning-r", "target", "winning", 10))
            .unwrap()
    );
    let winning_key = SessionKey::new("native", "claude-code", "winning");
    winning_store
        .requeue_session_evidence(&winning_key)
        .unwrap();
    let winning_claim = winning_store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();
    let winning_record = projection_record(winning_key, "sv1:baseline", 1);
    let winning_completion = evidence_completion(
        &winning_claim,
        PublishedEvidence::Ready,
        r#"{"version":1}"#.into(),
    );
    assert!(
        winning_store
            .publish_projections(&winning_record, None, &winning_completion, &[], &[])
            .unwrap()
    );
    assert_eq!(
        winning_store
            .remediation("winning-r")
            .unwrap()
            .unwrap()
            .verification_input_revision,
        3
    );

    let mut advanced = session("winning", 2_000);
    advanced.source_fingerprint = Some("sv1:advanced".into());
    winning_store
        .upsert_sessions(&[advanced], &crate::agents::evidence_cohort())
        .unwrap();
    assert_eq!(
        winning_store
            .remediation("winning-r")
            .unwrap()
            .unwrap()
            .verification_input_revision,
        4
    );
}

#[test]
fn session_delete_clear_and_retention_remove_remediations() {
    let delete_store = store();
    seed_baseline(&delete_store, "delete", 1_000);
    delete_store
        .insert_awaiting_remediation(&remediation("delete-r", "delete", "delete", 10))
        .unwrap();
    assert!(
        delete_store
            .delete_session(&SessionKey::new("native", "claude-code", "delete"))
            .unwrap()
    );
    assert!(delete_store.remediation("delete-r").unwrap().is_none());

    let clear_store = store();
    seed_baseline(&clear_store, "clear", 1_000);
    clear_store
        .insert_awaiting_remediation(&remediation("clear-r", "clear", "clear", 10))
        .unwrap();
    clear_store.clear_local_session_data().unwrap();
    assert!(clear_store.remediation("clear-r").unwrap().is_none());

    let retention_store = store();
    seed_baseline(&retention_store, "retention", 1);
    retention_store
        .insert_awaiting_remediation(&remediation("retention-r", "retention", "retention", 10))
        .unwrap();
    retention_store
        .save_settings(&AppSettings {
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_30,
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(
        retention_store
            .apply_session_retention(2_000_000_000)
            .unwrap(),
        1
    );
    assert!(
        retention_store
            .remediation("retention-r")
            .unwrap()
            .is_none()
    );
}
