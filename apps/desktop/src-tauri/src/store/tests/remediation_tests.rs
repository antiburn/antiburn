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

fn display_snapshot(remediation_id: &str) -> crate::store::remediation::RemediationDisplaySnapshot {
    crate::store::remediation::RemediationDisplaySnapshot {
        remediation_id: remediation_id.into(),
        origin: "passive".into(),
        display_snapshot_json: r#"{"version":1,"title":"Unused tool"}"#.into(),
        effective_boundary_ms: 10_001,
        verified_boundary_ms: Some(20_002),
        recurred_boundary_ms: None,
    }
}

fn contribution(
    owner_key: &str,
    remediation_id: &str,
    ends_at_ms: i64,
) -> crate::store::remediation::RemediationContribution {
    crate::store::remediation::RemediationContribution {
        owner_key: owner_key.into(),
        remediation_id: remediation_id.into(),
        detector_id: "unusedBuiltInTools".into(),
        origin: "passive".into(),
        display_snapshot_json: r#"{"version":1,"title":"Unused tool"}"#.into(),
        facts_json: r#"{"version":1,"tokens":1200}"#.into(),
        starts_at_ms: 10_001,
        ends_at_ms,
        updated_at_ms: ends_at_ms,
    }
}

fn passive_candidate(index: usize) -> crate::store::PassiveRemediation {
    crate::store::PassiveRemediation {
        remediation_id: format!("passive-{index:03}"),
        target_key: format!("target-{index:03}"),
        environment_key: "native".into(),
        agent: "claude-code".into(),
        scope_kind: "session".into(),
        scope_key: format!("scope-{index:03}"),
        definition_json: JSON_V1.into(),
        result_json: JSON_V1.into(),
        display_snapshot_json: r#"{"version":1,"title":"Bounded"}"#.into(),
        boundary_ms: 10_001,
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
    assert_eq!(store.schema_version().unwrap(), 46);
    let connection = store.lock();
    let columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('remediation')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 22);
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
fn v44_adds_nullable_snapshots_and_strict_contributions_without_backfill() {
    let connection = Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..43] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 43).unwrap();
    connection
        .execute(
            "INSERT INTO remediation (
            remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
            state, definition_json, result_json, created_at_epoch, updated_at_epoch,
            effective_boundary_ms)
         VALUES ('old', 'target', 'native', 'claude-code', 'project', 'scope',
                 'watching', ?1, ?1, 10, 10, 10000)",
            [JSON_V1],
        )
        .unwrap();

    let store =
        Store::from_connection(connection, Path::new("/tmp/remediation-v44").into()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 46);
    assert!(store.remediation_display_snapshot("old").unwrap().is_none());
    let connection = store.lock();
    let strict: i64 = connection
        .query_row(
            "SELECT strict FROM pragma_table_list WHERE name = 'remediation_contribution'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(strict, 1);
}

#[test]
fn v46_adds_reasoning_attribution_without_rewriting_model_columns() {
    let connection = Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..45] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 45).unwrap();
    let store =
        Store::from_connection(connection, Path::new("/tmp/remediation-v46").into()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 46);
    let columns: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session_evidence')
             WHERE name IN (
                'effective_model_target_hash','effective_model_scope','effective_model',
                'effective_reasoning_target_hash','effective_reasoning_scope','effective_reasoning')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 6);
}

#[test]
fn display_snapshot_round_trips_exact_millisecond_boundaries() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    let snapshot = display_snapshot(&watch.remediation_id);
    assert!(
        store
            .upsert_remediation_display_snapshot(&snapshot)
            .unwrap()
    );
    assert_eq!(
        store
            .remediation_display_snapshot(&watch.remediation_id)
            .unwrap(),
        Some(snapshot)
    );

    let mut invalid = display_snapshot(&watch.remediation_id);
    invalid.recurred_boundary_ms = Some(20_003);
    invalid.verified_boundary_ms = None;
    assert!(store.upsert_remediation_display_snapshot(&invalid).is_err());
    invalid.origin = "external".into();
    assert!(store.upsert_remediation_display_snapshot(&invalid).is_err());
}

#[test]
fn contribution_upsert_is_idempotent_and_rejects_stale_or_cross_watch_replays() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    let first = contribution("owner", "watch", 20_002);
    assert!(store.upsert_remediation_contribution(&first).unwrap());
    assert!(store.upsert_remediation_contribution(&first).unwrap());
    assert_eq!(
        store.remediation_contributions(10).unwrap(),
        vec![first.clone()]
    );

    let mut stale = first.clone();
    stale.facts_json = r#"{"version":1,"tokens":1}"#.into();
    stale.ends_at_ms -= 1;
    stale.updated_at_ms -= 1;
    assert!(!store.upsert_remediation_contribution(&stale).unwrap());
    let mut collision = first.clone();
    collision.remediation_id = "other-watch".into();
    collision.updated_at_ms += 1;
    assert!(!store.upsert_remediation_contribution(&collision).unwrap());
    assert_eq!(store.remediation_contributions(10).unwrap(), vec![first]);
}

#[test]
fn aggregate_win_reads_are_bounded_and_ordered() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    for index in 0..105 {
        store
            .upsert_remediation_contribution(&contribution(
                &format!("owner-{index:03}"),
                "watch",
                20_000 + index,
            ))
            .unwrap();
    }
    assert!(store.remediation_contributions(0).unwrap().is_empty());
    let wins = store.remediation_contributions(1_000).unwrap();
    assert_eq!(wins.len(), 105);
    assert_eq!(wins[0].owner_key, "owner-104");
    assert_eq!(wins[104].owner_key, "owner-000");
}

#[test]
fn passive_enrollment_is_bounded_resumable_and_replay_is_idempotent() {
    let store = store();
    let candidates = (0..105).map(passive_candidate).collect::<Vec<_>>();
    let connection = store.lock();
    assert_eq!(
        crate::store::remediation::enroll_passive_remediations_in(&connection, &candidates, 10,)
            .unwrap(),
        100
    );
    assert_eq!(
        crate::store::remediation::enroll_passive_remediations_in(&connection, &candidates, 11,)
            .unwrap(),
        5
    );
    assert_eq!(
        crate::store::remediation::enroll_passive_remediations_in(&connection, &candidates, 12,)
            .unwrap(),
        0
    );
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM remediation", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 105);
}

#[test]
fn passive_enrollment_caps_durable_active_watches() {
    let store = store();
    let candidates = (0..1_001).map(passive_candidate).collect::<Vec<_>>();
    let connection = store.lock();
    let mut inserted = 0;
    for now in 10..=20 {
        inserted += crate::store::remediation::enroll_passive_remediations_in(
            &connection,
            &candidates,
            now,
        )
        .unwrap();
    }
    assert_eq!(inserted, 1_000);
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM remediation WHERE state != 'recurred'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1_000);
}

#[test]
fn fixed_watches_with_durable_contributions_archive_before_new_admission() {
    let store = store();
    let connection = store.lock();
    for index in 0..1_000 {
        connection
            .execute(
                "INSERT INTO remediation (
                    remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                    state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                    effective_boundary_ms)
                 VALUES (?1, ?2, 'native', 'claude-code', 'session', ?2,
                         ?3, ?4, ?4, 10, ?5, 10000)",
                params![
                    format!("watch-{index:04}"),
                    format!("target-{index:04}"),
                    "watching",
                    JSON_V1,
                    i64::from(index) + 10,
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "UPDATE remediation SET state = 'fixed', verified_at_epoch = 11, updated_at_epoch = 11
              WHERE remediation_id = 'watch-0000'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO remediation_contribution (
                owner_key, remediation_id, detector_id, origin, display_snapshot_json,
                facts_json, starts_at_ms, ends_at_ms, updated_at_ms)
             VALUES ('retained-owner', 'watch-0000', 'old_model_usage', 'passive',
                     ?1, ?2, 10000, 20000, 20000)",
            params![
                r#"{"version":1,"title":"Bounded"}"#,
                r#"{"version":1,"tokens":1200}"#
            ],
        )
        .unwrap();

    let candidate = passive_candidate(1_001);
    assert_eq!(
        crate::store::remediation::enroll_passive_remediations_in(
            &connection,
            std::slice::from_ref(&candidate),
            20,
        )
        .unwrap(),
        1
    );
    assert!(
        !connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM remediation WHERE remediation_id = 'watch-0000')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT facts_json FROM remediation_contribution WHERE owner_key = 'retained-owner'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        r#"{"version":1,"tokens":1200}"#
    );
}

#[test]
fn a_later_publication_can_enroll_a_new_attempt_after_recurrence() {
    let store = store();
    let first = passive_candidate(1);
    {
        let connection = store.lock();
        crate::store::remediation::enroll_passive_remediations_in(
            &connection,
            std::slice::from_ref(&first),
            10,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE remediation SET state = 'fixed', verified_at_epoch = 11,
                    verified_boundary_ms = 11000, updated_at_epoch = 11
                  WHERE remediation_id = ?1",
                [&first.remediation_id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE remediation SET state = 'recurred', recurred_at_epoch = 12,
                    recurred_boundary_ms = 12000, updated_at_epoch = 12
                  WHERE remediation_id = ?1",
                [&first.remediation_id],
            )
            .unwrap();
        let mut next = first.clone();
        next.remediation_id = "passive-next-publication".into();
        next.boundary_ms = 13_000;
        assert_eq!(
            crate::store::remediation::enroll_passive_remediations_in(&connection, &[next], 13,)
                .unwrap(),
            1
        );
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM remediation", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}

#[test]
fn fixed_transition_and_contribution_commit_with_exact_milliseconds() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap();
    store
        .upsert_remediation_display_snapshot(&display_snapshot("watch"))
        .unwrap();
    let saved = contribution("owner-exact", "watch", 20_002);
    assert!(
        store
            .replace_remediation_result_with_contribution(
                "watch",
                1,
                &RemediationResult {
                    state: RemediationState::Fixed,
                    result_json: JSON_V1.into(),
                    evaluated_at_epoch: 21,
                    transition_at_ms: Some(20_002),
                },
                Some(&saved),
            )
            .unwrap()
    );
    let connection = store.lock();
    let boundary: i64 = connection
        .query_row(
            "SELECT verified_boundary_ms FROM remediation WHERE remediation_id = 'watch'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(boundary, 20_002);
    let facts: String = connection
        .query_row(
            "SELECT facts_json FROM remediation_contribution WHERE owner_key = 'owner-exact'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(facts, saved.facts_json);
}

#[test]
fn session_retention_does_not_erase_durable_contributions() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 1);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    let saved = contribution("owner", &watch.remediation_id, 20_000);
    store.upsert_remediation_contribution(&saved).unwrap();
    store
        .delete_session(&SessionKey::new("native", "claude-code", "baseline"))
        .unwrap();
    assert_eq!(store.remediation_contributions(10).unwrap(), vec![saved]);
}

#[test]
fn snapshots_and_contributions_reject_private_fields() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    let mut snapshot = display_snapshot("watch");
    snapshot.display_snapshot_json = r#"{"version":1,"sample":{"sessionId":"private"}}"#.into();
    assert!(
        store
            .upsert_remediation_display_snapshot(&snapshot)
            .is_err()
    );
    snapshot.display_snapshot_json = r#"{"version":2}"#.into();
    assert!(
        store
            .upsert_remediation_display_snapshot(&snapshot)
            .is_err()
    );

    let mut saved = contribution("owner", "watch", 20_000);
    saved.facts_json = r#"{"version":1,"path":"/private/work"}"#.into();
    assert!(store.upsert_remediation_contribution(&saved).is_err());
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
    prompt_input.result_json = r#"{"version":1,"verification":{"status":"watching","methodRevision":7},"savings":{"status":"pending","methodRevision":9}}"#.into();
    let prompt = store
        .create_or_reuse_remediation(&prompt_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let mut passive = display_snapshot(&prompt.remediation_id);
    passive.effective_boundary_ms = prompt.effective_boundary_ms.unwrap();
    passive.verified_boundary_ms = None;
    store.upsert_remediation_display_snapshot(&passive).unwrap();
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
    assert_eq!(restored.definition_json, prompt.definition_json);
    assert_eq!(restored.result_json, prompt.result_json);
    assert_eq!(restored.effective_boundary_ms, prompt.effective_boundary_ms);
    assert_eq!(restored.action_joined_at_ms, None);
    assert_eq!(
        store
            .remediation_display_snapshot(&restored.remediation_id)
            .unwrap(),
        Some(passive.clone())
    );
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
    assert_eq!(watching.effective_boundary_ms, prompt.effective_boundary_ms);
    assert_eq!(watching.action_joined_at_ms, Some(22_000));
    assert_eq!(
        store
            .remediation_display_snapshot(&watching.remediation_id)
            .unwrap()
            .unwrap()
            .origin,
        "passive"
    );
}

#[test]
fn a_new_auto_write_starts_its_boundary_at_successful_readback() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let reserved = store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            &[guard],
        )
        .unwrap()
        .unwrap();
    let mut snapshot = display_snapshot(&reserved.remediation_id);
    snapshot.origin = "action".into();
    snapshot.effective_boundary_ms = 20_000;
    snapshot.verified_boundary_ms = None;
    store
        .upsert_remediation_display_snapshot(&snapshot)
        .unwrap();

    assert!(
        store
            .begin_remediation_write(&reserved.remediation_id, 21)
            .unwrap()
    );
    assert!(
        store
            .finalize_remediation_write(&reserved.remediation_id, 23_456, 24)
            .unwrap()
    );
    let watching = store
        .remediation(&reserved.remediation_id)
        .unwrap()
        .unwrap();
    assert_eq!(watching.effective_boundary_ms, Some(23_456));
    assert_eq!(watching.action_joined_at_ms, None);
    assert_eq!(
        store
            .remediation_display_snapshot(&reserved.remediation_id)
            .unwrap()
            .unwrap()
            .effective_boundary_ms,
        23_456
    );
}

#[test]
fn auto_write_does_not_upgrade_an_existing_action_watch() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("prompt", "target", RemediationState::Watching, 10),
            std::slice::from_ref(&guard),
        )
        .unwrap()
        .unwrap();
    let mut snapshot = display_snapshot(&watch.remediation_id);
    snapshot.origin = "action".into();
    snapshot.effective_boundary_ms = 10_000;
    snapshot.verified_boundary_ms = None;
    store
        .upsert_remediation_display_snapshot(&snapshot)
        .unwrap();

    let existing = store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            &[guard],
        )
        .unwrap()
        .unwrap();
    assert_eq!(existing.remediation_id, watch.remediation_id);
    assert_eq!(existing.state, RemediationState::Watching);
    assert_eq!(existing.effective_boundary_ms, Some(10_000));
}

#[test]
fn startup_restores_a_complete_upgraded_reservation() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut prompt_input = remediation("prompt", "target", RemediationState::Watching, 10);
    prompt_input.definition_json =
        r#"{"version":1,"catalogRevision":"catalog-pinned","pricingRevision":"pricing-pinned"}"#
            .into();
    prompt_input.result_json = r#"{"version":1,"verification":{"status":"watching","methodRevision":7},"savings":{"status":"pending","methodRevision":9}}"#.into();
    let prompt = store
        .create_or_reuse_remediation(&prompt_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let mut snapshot = display_snapshot(&prompt.remediation_id);
    snapshot.effective_boundary_ms = prompt.effective_boundary_ms.unwrap();
    snapshot.verified_boundary_ms = None;
    store
        .upsert_remediation_display_snapshot(&snapshot)
        .unwrap();
    store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            &[guard],
        )
        .unwrap()
        .unwrap();

    assert_eq!(store.reconcile_remediations(30).unwrap(), 1);
    let restored = store.remediation(&prompt.remediation_id).unwrap().unwrap();
    assert_eq!(restored.state, RemediationState::Watching);
    assert_eq!(restored.definition_json, prompt.definition_json);
    assert_eq!(restored.result_json, prompt.result_json);
    assert_eq!(restored.effective_boundary_ms, prompt.effective_boundary_ms);
    assert_eq!(restored.action_joined_at_ms, None);
    assert_eq!(
        store
            .remediation_display_snapshot(&prompt.remediation_id)
            .unwrap(),
        Some(snapshot)
    );
}

#[test]
fn startup_preserves_upgrade_rollback_data_for_an_uncertain_write() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut prompt_input = remediation("prompt", "target", RemediationState::Watching, 10);
    prompt_input.definition_json = r#"{"version":1,"pricingRevision":"pinned"}"#.into();
    prompt_input.result_json =
        r#"{"version":1,"verification":{"status":"watching"},"savings":{"status":"pending"}}"#
            .into();
    let prompt = store
        .create_or_reuse_remediation(&prompt_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let mut snapshot = display_snapshot(&prompt.remediation_id);
    snapshot.effective_boundary_ms = prompt.effective_boundary_ms.unwrap();
    snapshot.verified_boundary_ms = None;
    store
        .upsert_remediation_display_snapshot(&snapshot)
        .unwrap();
    let reserved = store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            &[guard],
        )
        .unwrap()
        .unwrap();
    store
        .begin_remediation_write(&reserved.remediation_id, 21)
        .unwrap();

    assert_eq!(store.reconcile_remediations(30).unwrap(), 0);
    let recovery = store.next_remediation_write_recovery(30).unwrap().unwrap();
    assert_eq!(recovery.state, RemediationState::RecoveryNeeded);
    assert_eq!(recovery.definition_json, JSON_V1);
    let envelope: serde_json::Value = serde_json::from_str(&recovery.result_json).unwrap();
    assert_eq!(envelope["reservationKind"], "upgraded");
    assert_eq!(envelope["priorDefinition"], prompt.definition_json);
    assert_eq!(envelope["priorResult"], prompt.result_json);
    assert_eq!(envelope["priorBoundaryMs"], 10_000);
    assert_eq!(
        envelope["priorDisplaySnapshot"],
        snapshot.display_snapshot_json
    );
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
            false,
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
                    transition_at_ms: Some(21_000),
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
                    transition_at_ms: Some(21_000),
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
            false,
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
                    transition_at_ms: Some(31_000),
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
fn correction_replay_replaces_recurred_facts_without_reopening_the_attempt() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    store
        .upsert_remediation_display_snapshot(&display_snapshot(&watch.remediation_id))
        .unwrap();

    let fixed = contribution("owner", &watch.remediation_id, 20_002);
    assert!(
        store
            .replace_remediation_result_with_contribution(
                &watch.remediation_id,
                1,
                &RemediationResult {
                    state: RemediationState::Fixed,
                    result_json: r#"{"version":1,"verification":{"status":"fixed"}}"#.into(),
                    evaluated_at_epoch: 21,
                    transition_at_ms: Some(20_002),
                },
                Some(&fixed),
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
            false,
        )
        .unwrap();
    }
    let mut recurred = contribution("owner", &watch.remediation_id, 30_003);
    recurred.facts_json = r#"{"version":1,"tokens":900}"#.into();
    assert!(
        store
            .replace_remediation_result_with_contribution(
                &watch.remediation_id,
                2,
                &RemediationResult {
                    state: RemediationState::Recurred,
                    result_json:
                        r#"{"version":1,"verification":{"status":"recurred","fact":"old"}}"#.into(),
                    evaluated_at_epoch: 31,
                    transition_at_ms: Some(30_003),
                },
                Some(&recurred),
            )
            .unwrap()
    );

    {
        let connection = store.lock();
        assert_eq!(
            crate::store::remediation::mark_remediations_dirty_in(
                &connection,
                "native",
                "claude-code",
                40,
                false,
            )
            .unwrap(),
            0
        );
    }
    assert!(store.next_dirty_remediation().unwrap().is_none());
    assert_eq!(store.remediation_contributions(10).unwrap(), vec![recurred]);

    {
        let connection = store.lock();
        assert_eq!(
            crate::store::remediation::mark_remediations_dirty_in(
                &connection,
                "native",
                "claude-code",
                41,
                true,
            )
            .unwrap(),
            1
        );
    }
    assert_eq!(
        store.next_dirty_remediation().unwrap().unwrap().state,
        RemediationState::Recurred
    );
    let mut corrected = contribution("owner", &watch.remediation_id, 30_001);
    corrected.facts_json = r#"{"version":1,"tokens":600}"#.into();
    corrected.updated_at_ms = 41_000;
    let corrected_result =
        r#"{"version":1,"verification":{"status":"recurred","fact":"corrected"}}"#;
    assert!(
        store
            .replace_remediation_result_with_contribution(
                &watch.remediation_id,
                3,
                &RemediationResult {
                    state: RemediationState::Recurred,
                    result_json: corrected_result.into(),
                    evaluated_at_epoch: 41,
                    transition_at_ms: Some(30_001),
                },
                Some(&corrected),
            )
            .unwrap()
    );
    assert_eq!(
        store.remediation_contributions(10).unwrap(),
        vec![corrected]
    );
    assert_eq!(
        store
            .remediation(&watch.remediation_id)
            .unwrap()
            .unwrap()
            .result_json,
        corrected_result
    );

    {
        let connection = store.lock();
        crate::store::remediation::mark_remediations_dirty_in(
            &connection,
            "native",
            "claude-code",
            42,
            true,
        )
        .unwrap();
    }
    let removed_result = r#"{"version":1,"verification":{"status":"verificationUnavailable"},"savings":{"status":"unavailable"}}"#;
    let mut reopened = contribution("owner", &watch.remediation_id, 50_000);
    reopened.facts_json = r#"{"version":1,"tokens":1500}"#.into();
    assert!(
        store
            .replace_remediation_result_with_contribution(
                &watch.remediation_id,
                4,
                &RemediationResult {
                    state: RemediationState::Recurred,
                    result_json: removed_result.into(),
                    evaluated_at_epoch: 42,
                    transition_at_ms: Some(20_002),
                },
                Some(&reopened),
            )
            .unwrap()
    );
    let closed = store.remediation(&watch.remediation_id).unwrap().unwrap();
    assert_eq!(closed.state, RemediationState::Recurred);
    assert_eq!(closed.result_json, removed_result);
    assert!(store.remediation_contributions(10).unwrap().is_empty());

    let mut later = passive_candidate(1);
    later.target_key = "target".into();
    later.boundary_ms = 43_000;
    assert_eq!(
        crate::store::remediation::enroll_passive_remediations_in(&store.lock(), &[later], 43)
            .unwrap(),
        1
    );
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
        crate::store::remediation::mark_remediations_dirty_in(
            &connection,
            "native",
            "codex",
            20,
            false,
        )
        .unwrap(),
        0
    );
    assert_eq!(
        crate::store::remediation::mark_remediations_dirty_in(
            &connection,
            "native",
            "claude-code",
            20,
            false,
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
fn clearing_local_data_removes_remediation_contributions() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    store
        .upsert_remediation_contribution(&contribution("owner", &watch.remediation_id, 20_000))
        .unwrap();

    assert_eq!(store.clear_local_session_data().unwrap(), 1);
    assert!(store.remediation_contributions(1_000).unwrap().is_empty());
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

#[test]
fn unsupported_remediation_envelope_versions_are_rejected_on_write_and_read() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut invalid = remediation("invalid", "target", RemediationState::Watching, 10);
    invalid.definition_json = r#"{"version":2}"#.into();
    assert!(
        store
            .create_or_reuse_remediation(&invalid, std::slice::from_ref(&guard))
            .is_err()
    );
    invalid.definition_json = JSON_V1.into();
    invalid.result_json = r#"{"version":2}"#.into();
    assert!(
        store
            .create_or_reuse_remediation(&invalid, &[guard])
            .is_err()
    );

    let guard = seed_evidence(&store, "second", 101);
    let watch = store
        .create_or_reuse_remediation(
            &remediation("watch", "target", RemediationState::Watching, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .replace_remediation_result(
                &watch.remediation_id,
                watch.dirty_revision,
                &RemediationResult {
                    state: RemediationState::Watching,
                    result_json: r#"{"version":2}"#.into(),
                    evaluated_at_epoch: 11,
                    transition_at_ms: None,
                },
            )
            .is_err()
    );
    store
        .lock()
        .execute(
            "UPDATE remediation SET definition_json = '{\"version\":2}' WHERE remediation_id = ?1",
            [&watch.remediation_id],
        )
        .unwrap();
    assert!(store.remediation(&watch.remediation_id).is_err());
}
