use std::path::Path;

use rusqlite::{Connection, params};

use super::*;

const JSON_V1: &str = r#"{"version":1}"#;

fn seed_evidence(
    store: &Store,
    session_id: &str,
    updated_at_epoch: i64,
) -> RemediationEvidenceGuard {
    let mut record = session(session_id, updated_at_epoch);
    record.source_fingerprint = Some("sv1:baseline".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'ready', analyzed_generation = 1,
            processed_fingerprint = 'sv1:processed', parser_revision = 1,
            analyzer_revision = 1, evidence_schema_revision = 1,
            evidence_json = ?1, published_fence = 1
          WHERE environment_key = 'native' AND agent = 'claude-code' AND session_id = ?2",
            params![JSON_V1, session_id],
        )
        .unwrap();
    RemediationEvidenceGuard {
        environment_key: "native".into(),
        agent: "claude-code".into(),
        session_id: session_id.into(),
        source_generation: 1,
        published_fence: 1,
        source_fingerprint: Some("sv1:baseline".into()),
        processed_fingerprint: Some("sv1:processed".into()),
        parser_revision: 1,
        analyzer_revision: 1,
        evidence_schema_revision: 1,
    }
}

fn remediation(id: &str, target: &str, state: RemediationState, now: i64) -> Remediation {
    Remediation {
        remediation_id: id.into(),
        target_key: target.into(),
        environment_key: "native".into(),
        agent: "claude-code".into(),
        scope_kind: "project".into(),
        scope_key: "scope".into(),
        state,
        definition_json: JSON_V1.into(),
        result_json: JSON_V1.into(),
        created_at_epoch: now,
        effective_boundary_ms: (state == RemediationState::Watching).then_some(now * 1_000),
    }
}

#[test]
fn v43_adds_remediation_and_model_attribution() {
    let connection = Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..42] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 42).unwrap();
    let store =
        Store::from_connection(connection, Path::new("/tmp/remediation-v43").into()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 43);
    let connection = store.lock();
    let columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('remediation')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 16);
    for index in [
        "remediation_dirty",
        "remediation_scope",
        "remediation_active_target",
    ] {
        assert!(
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
                    [index],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
    }
    let attribution_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session_evidence')
             WHERE name IN ('effective_model_target_hash','effective_model_scope','effective_model')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attribution_columns, 3);
}

#[test]
fn create_requires_exact_fresh_evidence_and_reuses_the_active_target() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let first = store
        .create_or_reuse_remediation(
            &remediation("r1", "target", RemediationState::Watching, 10),
            std::slice::from_ref(&guard),
        )
        .unwrap()
        .unwrap();
    let second = store
        .create_or_reuse_remediation(
            &remediation("r2", "target", RemediationState::Watching, 20),
            std::slice::from_ref(&guard),
        )
        .unwrap()
        .unwrap();
    assert_eq!(first.remediation_id, second.remediation_id);

    let mut stale = guard;
    stale.published_fence = 2;
    assert!(
        store
            .create_or_reuse_remediation(
                &remediation("r3", "other", RemediationState::Watching, 20),
                &[stale]
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn prompt_watch_upgrades_to_a_crash_safe_auto_write() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut prompt_input = remediation("prompt", "target", RemediationState::Watching, 10);
    prompt_input.definition_json = r#"{"version":1,"pricingRevision":"pinned"}"#.into();
    let prompt = store
        .create_or_reuse_remediation(&prompt_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let reserved = store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            std::slice::from_ref(&guard),
        )
        .unwrap()
        .unwrap();
    assert_eq!(reserved.remediation_id, prompt.remediation_id);
    assert_eq!(reserved.state, RemediationState::Reserved);
    assert_eq!(reserved.definition_json, JSON_V1);
    assert!(
        store
            .begin_remediation_write(&reserved.remediation_id, 21)
            .unwrap()
    );
    assert!(
        store
            .cancel_pre_replacement_write(&reserved.remediation_id)
            .unwrap()
    );
    let restored = store
        .remediation(&reserved.remediation_id)
        .unwrap()
        .unwrap();
    assert_eq!(restored.state, RemediationState::Watching);
    assert_eq!(restored.result_json, prompt.result_json);
    assert_eq!(restored.effective_boundary_ms, prompt.effective_boundary_ms);
    let reserved = store
        .create_or_reuse_remediation(
            &remediation("auto-2", "target", RemediationState::Reserved, 22),
            &[guard],
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .begin_remediation_write(&reserved.remediation_id, 22)
            .unwrap()
    );
    assert!(
        store
            .mark_remediation_recovery_needed(&reserved.remediation_id, "writeOutcomeUnknown", 22)
            .unwrap()
    );
    assert_eq!(
        store
            .next_remediation_write_recovery(22)
            .unwrap()
            .unwrap()
            .state,
        RemediationState::RecoveryNeeded
    );
    assert!(
        store
            .finalize_remediation_write(&reserved.remediation_id, 23_000, 23)
            .unwrap()
    );
    let watching = store
        .remediation(&reserved.remediation_id)
        .unwrap()
        .unwrap();
    assert_eq!(watching.state, RemediationState::Watching);
    assert_eq!(watching.effective_boundary_ms, Some(23_000));
}

#[test]
fn temporary_recovery_failure_stays_retryable_without_spinning() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let reserved = store
        .create_or_reuse_remediation(
            &remediation("retry", "target", RemediationState::Reserved, 20),
            &[guard],
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .begin_remediation_write(&reserved.remediation_id, 20)
            .unwrap()
    );
    assert!(
        store
            .defer_remediation_recovery(&reserved.remediation_id, "verificationUnavailable", 20)
            .unwrap()
    );
    assert!(store.next_remediation_write_recovery(79).unwrap().is_none());
    assert_eq!(
        store
            .next_remediation_write_recovery(80)
            .unwrap()
            .unwrap()
            .remediation_id,
        reserved.remediation_id
    );
}

#[test]
fn revision_guard_rejects_stale_results_and_supports_fixed_then_recurred() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    {
        let connection = store.lock();
        crate::store::remediation::mark_remediations_dirty_in(
            &connection,
            "native",
            "claude-code",
            20,
        )
        .unwrap();
    }
    assert!(
        !store
            .replace_remediation_result(
                &watch.remediation_id,
                1,
                &RemediationResult {
                    state: RemediationState::Fixed,
                    result_json: JSON_V1.into(),
                    evaluated_at_epoch: 21,
                    transition_at_epoch: Some(21),
                }
            )
            .unwrap()
    );
    assert!(
        store
            .replace_remediation_result(
                &watch.remediation_id,
                2,
                &RemediationResult {
                    state: RemediationState::Fixed,
                    result_json: JSON_V1.into(),
                    evaluated_at_epoch: 21,
                    transition_at_epoch: Some(21),
                }
            )
            .unwrap()
    );
    {
        let connection = store.lock();
        crate::store::remediation::mark_remediations_dirty_in(
            &connection,
            "native",
            "claude-code",
            30,
        )
        .unwrap();
    }
    assert!(
        store
            .replace_remediation_result(
                &watch.remediation_id,
                3,
                &RemediationResult {
                    state: RemediationState::Recurred,
                    result_json: JSON_V1.into(),
                    evaluated_at_epoch: 31,
                    transition_at_epoch: Some(31),
                }
            )
            .unwrap()
    );
    let closed = store.remediation(&watch.remediation_id).unwrap().unwrap();
    assert_eq!(closed.state, RemediationState::Recurred);
    assert_eq!(closed.verified_at_epoch, Some(21));
    assert_eq!(closed.recurred_at_epoch, Some(31));
}

#[test]
fn publication_dirties_matching_active_watches_only() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    let connection = store.lock();
    assert_eq!(
        crate::store::remediation::mark_remediations_dirty_in(&connection, "native", "codex", 20)
            .unwrap(),
        0
    );
    assert_eq!(
        crate::store::remediation::mark_remediations_dirty_in(
            &connection,
            "native",
            "claude-code",
            20
        )
        .unwrap(),
        1
    );
    drop(connection);
    assert_eq!(
        store
            .remediation(&watch.remediation_id)
            .unwrap()
            .unwrap()
            .dirty_revision,
        2
    );
}

#[test]
fn session_deletion_and_retention_do_not_delete_a_watch() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 1);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .delete_session(&SessionKey::new("native", "claude-code", "baseline"))
            .unwrap()
    );
    assert!(store.remediation(&watch.remediation_id).unwrap().is_some());
}

#[test]
fn bounded_versioned_documents_are_enforced() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut invalid = remediation("invalid", "target", RemediationState::Watching, 10);
    invalid.definition_json = "{}".into();
    assert!(
        store
            .create_or_reuse_remediation(&invalid, std::slice::from_ref(&guard))
            .is_err()
    );
    invalid.definition_json = format!(r#"{{"version":1,"value":"{}"}}"#, "x".repeat(32_768));
    assert!(
        store
            .create_or_reuse_remediation(&invalid, &[guard])
            .is_err()
    );
}
