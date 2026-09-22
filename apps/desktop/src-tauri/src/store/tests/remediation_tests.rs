use std::path::Path;
use std::sync::{Arc, Barrier};

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
        origin: "action".into(),
        prompt_group_id: None,
        definition_json: JSON_V1.into(),
        result_json: JSON_V1.into(),
        created_at_epoch: now,
        effective_boundary_ms: (state == RemediationState::Watching).then_some(now * 1_000),
    }
}

fn prompt_remediation(id: &str, scope: &str, token: &str, prompt: &str) -> Remediation {
    let mut value = remediation(id, "target", RemediationState::WaitingForPromptUse, 10);
    value.scope_key = scope.into();
    value.prompt_group_id = Some(token.into());
    value.result_json = serde_json::json!({
        "version": 1,
        "verification": {"status": "verificationUnavailable"},
        "savings": {"status": "unavailable"},
        "promptText": prompt,
    })
    .to_string();
    value
}

fn mark_fixed(store: &Store, remediation_id: &str) {
    store
        .lock()
        .execute(
            "UPDATE remediation SET state = 'fixed', verified_at_epoch = 11,
             updated_at_epoch = 11 WHERE remediation_id = ?1",
            [remediation_id],
        )
        .unwrap();
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
    assert_eq!(store.schema_version().unwrap(), 53);
    let connection = store.lock();
    let columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('remediation')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 23);
    for index in [
        "remediation_dirty",
        "remediation_scope",
        "remediation_active_passive_target",
        "remediation_active_action_target",
        "remediation_prompt_group",
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
    assert_eq!(store.schema_version().unwrap(), 53);
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
    assert_eq!(store.schema_version().unwrap(), 53);
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
fn remediation_batch_rolls_back_when_a_later_insert_fails() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let batch = [
        (
            remediation("duplicate", "first-target", RemediationState::Watching, 10),
            vec![guard.clone()],
        ),
        (
            remediation("duplicate", "second-target", RemediationState::Watching, 10),
            vec![guard],
        ),
    ];

    assert!(store.create_or_reuse_remediations(&batch).is_err());
    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM remediation", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn only_exact_reserved_resource_writes_can_start_without_session_guards() {
    let store = store();
    let ordinary = remediation(
        "ordinary",
        "ordinary-target",
        RemediationState::Reserved,
        10,
    );
    assert!(store.create_or_reuse_remediation(&ordinary, &[]).is_err());

    let mut resource = remediation(
        "resource",
        "resource-target",
        RemediationState::Reserved,
        10,
    );
    resource.definition_json = serde_json::json!({
        "version": 1,
        "detector": "unused_mcp_servers",
        "resource": "docs",
        "physicalTargetKey": "opaque-target",
        "configSetting": "mcpServer",
        "configExpectedValue": "docs=true",
        "configProposedValue": "docs=false"
    })
    .to_string();

    assert!(
        store
            .create_or_reuse_remediation(&resource, &[])
            .unwrap()
            .is_some()
    );
}

#[test]
fn remediation_snapshot_reads_are_bounded_and_ordered() {
    let directory = tempfile::TempDir::new().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for (id, updated_at_epoch) in [("older", 1), ("newer", 2), ("legacy", 3)] {
        store
            .lock()
            .execute(
                "INSERT INTO remediation (
                    remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                    state, definition_json, result_json, created_at_epoch, updated_at_epoch)
                 VALUES (?1, ?1, 'native', 'claude-code', 'project', 'scope',
                         'waitingForPromptUse', ?2, ?2, 1, ?3)",
                params![id, JSON_V1, updated_at_epoch],
            )
            .unwrap();
    }
    for id in ["older", "newer"] {
        let mut snapshot = display_snapshot(id);
        snapshot.verified_boundary_ms = None;
        store
            .upsert_remediation_display_snapshot(&snapshot)
            .unwrap();
    }

    let latest = store.remediations_with_display_snapshots(1).unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].record.remediation_id, "newer");
    assert_eq!(latest[0].snapshot.effective_boundary_ms, 0);
    assert!(
        store
            .remediations_with_display_snapshots(0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn remediation_snapshot_bound_prioritizes_active_cycles_over_recurred_history() {
    let store = store();
    let connection = store.lock();
    for index in 0..1_001 {
        connection
            .execute(
                "INSERT INTO remediation (
                    remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                    state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                    effective_boundary_ms, verified_at_epoch, recurred_at_epoch, origin,
                    display_snapshot_json, verified_boundary_ms, recurred_boundary_ms)
                 VALUES (?1, ?1, 'native', 'claude-code', 'project', ?1, 'recurred',
                    ?2, ?2, 1, ?3, 1000, 1, ?3, 'action', ?4, 1000, ?5)",
                params![
                    format!("history-{index:04}"),
                    JSON_V1,
                    index as i64 + 2,
                    r#"{"version":1,"findingId":"history","display":{}}"#,
                    (index as i64 + 2) * 1_000,
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO remediation (
                remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                origin, display_snapshot_json)
             VALUES ('active', 'active-target', 'native', 'claude-code', 'project', 'active-scope',
                'waitingForPromptUse', ?1, ?1, 1, 1, 'action', ?2)",
            params![
                JSON_V1,
                r#"{"version":1,"findingId":"active","display":{}}"#
            ],
        )
        .unwrap();
    drop(connection);

    let records = store.remediations_with_display_snapshots(1_000).unwrap();
    assert_eq!(records.len(), 1_000);
    assert!(
        records
            .iter()
            .any(|record| record.record.remediation_id == "active")
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.record.state == RemediationState::Recurred)
            .count(),
        999
    );
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
    mark_fixed(&store, "watch");
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
    mark_fixed(&store, "watch");
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
    mark_fixed(&store, "watch");
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
fn an_auto_write_uses_a_distinct_action_row_and_boundary() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut passive_input = remediation("passive", "target", RemediationState::Watching, 10);
    passive_input.origin = "passive".into();
    let passive = store
        .create_or_reuse_remediation(&passive_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let mut passive_snapshot = display_snapshot(&passive.remediation_id);
    passive_snapshot.effective_boundary_ms = passive.effective_boundary_ms.unwrap();
    passive_snapshot.verified_boundary_ms = None;
    store
        .upsert_remediation_display_snapshot(&passive_snapshot)
        .unwrap();
    let reserved = store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            std::slice::from_ref(&guard),
        )
        .unwrap()
        .unwrap();
    assert_eq!(reserved.remediation_id, "auto");
    assert_eq!(reserved.state, RemediationState::Reserved);
    let mut action_snapshot = display_snapshot(&reserved.remediation_id);
    action_snapshot.origin = "action".into();
    action_snapshot.effective_boundary_ms = 20_000;
    action_snapshot.verified_boundary_ms = None;
    store
        .upsert_remediation_display_snapshot(&action_snapshot)
        .unwrap();
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
    let restored = store.remediation(&passive.remediation_id).unwrap().unwrap();
    assert_eq!(restored.state, RemediationState::Watching);
    assert_eq!(
        restored.effective_boundary_ms,
        passive.effective_boundary_ms
    );
    assert_eq!(
        store
            .remediation_display_snapshot(&passive.remediation_id)
            .unwrap(),
        Some(passive_snapshot.clone())
    );
    let reserved = store
        .create_or_reuse_remediation(
            &remediation("auto-2", "target", RemediationState::Reserved, 22),
            &[guard],
        )
        .unwrap()
        .unwrap();
    let mut action_snapshot = display_snapshot(&reserved.remediation_id);
    action_snapshot.origin = "action".into();
    action_snapshot.effective_boundary_ms = 22_000;
    action_snapshot.verified_boundary_ms = None;
    store
        .upsert_remediation_display_snapshot(&action_snapshot)
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
            .finalize_remediation_write(&reserved.remediation_id, 23_000, 23, true)
            .unwrap()
    );
    let watching = store
        .remediation(&reserved.remediation_id)
        .unwrap()
        .unwrap();
    assert_eq!(watching.state, RemediationState::Watching);
    assert_eq!(watching.effective_boundary_ms, Some(23_000));
    assert_eq!(watching.action_joined_at_ms, None);
    assert_eq!(
        store
            .remediation_display_snapshot(&watching.remediation_id)
            .unwrap()
            .unwrap()
            .origin,
        "action"
    );
    let passive_after = store.remediation(&passive.remediation_id).unwrap().unwrap();
    assert_eq!(
        passive_after.effective_boundary_ms,
        passive.effective_boundary_ms
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
            .finalize_remediation_write(&reserved.remediation_id, 23_456, 24, true)
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
fn auto_write_does_not_replace_an_unverifiable_action_watch() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut prompt_input = remediation("prompt", "target", RemediationState::Watching, 10);
    prompt_input.result_json = r#"{"version":1,"verification":{"status":"verificationUnavailable"},"savings":{"status":"unavailable"}}"#.into();
    let prompt = store
        .create_or_reuse_remediation(&prompt_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let mut snapshot = display_snapshot(&prompt.remediation_id);
    snapshot.origin = "action".into();
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
    assert_eq!(reserved.remediation_id, prompt.remediation_id);
    assert_eq!(reserved.state, RemediationState::Watching);

    assert!(
        !store
            .cancel_remediation_reservation(&reserved.remediation_id)
            .unwrap()
    );
    let restored = store
        .remediation(&reserved.remediation_id)
        .unwrap()
        .unwrap();
    assert_eq!(restored.state, RemediationState::Watching);
    assert_eq!(restored.result_json, prompt.result_json);
    assert_eq!(restored.action_joined_at_ms, None);
    assert_eq!(
        store
            .remediation_display_snapshot(&restored.remediation_id)
            .unwrap(),
        Some(snapshot)
    );
}

#[test]
fn auto_write_does_not_replace_a_waiting_prompt_attempt() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let waiting = store
        .create_or_reuse_remediation(
            &remediation(
                "prompt",
                "target",
                RemediationState::WaitingForPromptUse,
                10,
            ),
            std::slice::from_ref(&guard),
        )
        .unwrap()
        .unwrap();

    let reserved = store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            &[guard],
        )
        .unwrap()
        .unwrap();
    assert_eq!(reserved.remediation_id, waiting.remediation_id);
    assert_eq!(reserved.state, RemediationState::WaitingForPromptUse);

    assert!(
        !store
            .cancel_remediation_reservation(&reserved.remediation_id)
            .unwrap()
    );
    assert_eq!(
        store
            .remediation(&reserved.remediation_id)
            .unwrap()
            .unwrap()
            .state,
        RemediationState::WaitingForPromptUse
    );
}

#[test]
fn copied_prompt_does_not_replace_an_active_passive_watch() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let passive = store
        .create_or_reuse_remediation(
            &remediation("passive", "target", RemediationState::Watching, 10),
            std::slice::from_ref(&guard),
        )
        .unwrap()
        .unwrap();
    let snapshot = display_snapshot(&passive.remediation_id);
    store
        .upsert_remediation_display_snapshot(&snapshot)
        .unwrap();

    let copied = store
        .create_or_reuse_remediation(
            &remediation(
                "copied",
                "target",
                RemediationState::WaitingForPromptUse,
                20,
            ),
            &[guard],
        )
        .unwrap()
        .unwrap();

    assert_eq!(copied.remediation_id, "copied");
    assert_eq!(copied.state, RemediationState::WaitingForPromptUse);
    assert_eq!(copied.origin.as_deref(), Some("action"));
    assert_eq!(copied.effective_boundary_ms, None);
    assert_eq!(
        store
            .remediation_display_snapshot(&passive.remediation_id)
            .unwrap(),
        Some(snapshot)
    );
    assert!(store.remediation("copied").unwrap().is_some());
}

#[test]
fn exact_target_cycles_are_independent_across_environment_and_scope() {
    let store = store();
    let native_project = prompt_remediation("native-project", "project-a", "one", "prompt one");
    let mut wsl_project = prompt_remediation("wsl-project", "project-a", "two", "prompt two");
    wsl_project.environment_key = "wsl:Ubuntu".into();
    let native_other = prompt_remediation("native-other", "project-b", "three", "prompt three");

    for input in [&native_project, &wsl_project, &native_other] {
        store
            .create_or_reuse_remediation(input, &[])
            .unwrap()
            .unwrap();
    }

    assert_eq!(
        store
            .latest_action_remediation_for_target(
                "native",
                "claude-code",
                "project",
                "project-a",
                "target",
            )
            .unwrap()
            .unwrap()
            .remediation_id,
        "native-project"
    );
    assert_eq!(
        store
            .latest_action_remediation_for_target(
                "wsl:Ubuntu",
                "claude-code",
                "project",
                "project-a",
                "target",
            )
            .unwrap()
            .unwrap()
            .remediation_id,
        "wsl-project"
    );
    assert_eq!(
        store
            .latest_action_remediation_for_target(
                "native",
                "claude-code",
                "project",
                "project-b",
                "target",
            )
            .unwrap()
            .unwrap()
            .remediation_id,
        "native-other"
    );
}

#[test]
fn a_historical_recurred_cycle_does_not_override_a_newer_fixed_cycle() {
    let store = store();
    store
        .lock()
        .execute_batch(
            "INSERT INTO remediation (
                remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                effective_boundary_ms, verified_at_epoch, recurred_at_epoch, origin)
             VALUES ('old', 'target', 'native', 'claude-code', 'project', 'scope',
                'recurred', '{\"version\":1}', '{\"version\":1}', 1, 30, 1000, 2, 30, 'action');
             INSERT INTO remediation (
                remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                effective_boundary_ms, verified_at_epoch, origin)
             VALUES ('new', 'target', 'native', 'claude-code', 'project', 'scope',
                'fixed', '{\"version\":1}', '{\"version\":1}', 20, 20, 20000, 20, 'action');",
        )
        .unwrap();

    assert_eq!(
        store
            .latest_action_remediation_for_target(
                "native",
                "claude-code",
                "project",
                "scope",
                "target",
            )
            .unwrap()
            .unwrap()
            .remediation_id,
        "new"
    );
}

#[test]
fn an_active_cycle_wins_when_recurrence_and_creation_share_the_same_second() {
    let store = store();
    store
        .lock()
        .execute_batch(
            "INSERT INTO remediation (
                remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                effective_boundary_ms, verified_at_epoch, origin)
             VALUES ('active', 'target', 'native', 'claude-code', 'project', 'scope',
                'fixed', '{\"version\":1}', '{\"version\":1}', 20, 20, 20000, 20, 'action');
             INSERT INTO remediation (
                remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                effective_boundary_ms, verified_at_epoch, recurred_at_epoch, origin)
             VALUES ('recurred-z', 'target', 'native', 'claude-code', 'project', 'scope',
                'recurred', '{\"version\":1}', '{\"version\":1}', 20, 20, 20000, 20, 20,
                'action');",
        )
        .unwrap();

    assert_eq!(
        store
            .latest_action_remediation_for_target(
                "native",
                "claude-code",
                "project",
                "scope",
                "target",
            )
            .unwrap()
            .unwrap()
            .remediation_id,
        "active"
    );
}

#[test]
fn concurrent_prompt_creation_returns_the_one_stored_token_and_text() {
    let store = Arc::new(store());
    let barrier = Arc::new(Barrier::new(2));
    let threads = [
        ("first", "token-one", "prompt one"),
        ("second", "token-two", "prompt two"),
    ]
    .into_iter()
    .map(|(id, token, prompt)| {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let input = prompt_remediation(id, "scope", token, prompt);
            barrier.wait();
            store
                .create_or_reuse_remediation(&input, &[])
                .unwrap()
                .unwrap()
        })
    })
    .collect::<Vec<_>>();
    let records = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(records[0].remediation_id, records[1].remediation_id);
    assert_eq!(records[0].prompt_group_id, records[1].prompt_group_id);
    assert_eq!(records[0].result_json, records[1].result_json);
    let result: serde_json::Value = serde_json::from_str(&records[0].result_json).unwrap();
    let prompt = result["promptText"].as_str().unwrap();
    match records[0].prompt_group_id.as_deref().unwrap() {
        "token-one" => assert_eq!(prompt, "prompt one"),
        "token-two" => assert_eq!(prompt, "prompt two"),
        token => panic!("unexpected prompt token {token}"),
    }
}

#[test]
fn prompt_retry_rejects_an_active_auto_fix_cycle() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 10),
            &[guard],
        )
        .unwrap()
        .unwrap();

    assert!(
        store
            .create_or_reuse_remediation(
                &prompt_remediation("prompt", "scope", "token", "prompt"),
                &[],
            )
            .is_err()
    );
}

#[test]
fn startup_retains_waiting_prompt_attempts() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let waiting = store
        .create_or_reuse_remediation(
            &remediation(
                "prompt",
                "target",
                RemediationState::WaitingForPromptUse,
                10,
            ),
            &[guard],
        )
        .unwrap()
        .unwrap();

    assert_eq!(store.reconcile_remediations(30).unwrap(), 0);
    assert_eq!(
        store
            .remediation(&waiting.remediation_id)
            .unwrap()
            .unwrap()
            .state,
        RemediationState::WaitingForPromptUse
    );
}

#[test]
fn startup_removes_only_a_new_action_reservation() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut passive_input = remediation("passive", "target", RemediationState::Watching, 10);
    passive_input.origin = "passive".into();
    let passive = store
        .create_or_reuse_remediation(&passive_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let action = store
        .create_or_reuse_remediation(
            &remediation("auto", "target", RemediationState::Reserved, 20),
            &[guard],
        )
        .unwrap()
        .unwrap();

    assert_eq!(store.reconcile_remediations(30).unwrap(), 1);
    assert!(store.remediation(&action.remediation_id).unwrap().is_none());
    assert_eq!(
        store
            .remediation(&passive.remediation_id)
            .unwrap()
            .unwrap()
            .state,
        RemediationState::Watching
    );
}

#[test]
fn startup_preserves_passive_data_during_action_recovery() {
    let store = store();
    let guard = seed_evidence(&store, "baseline", 100);
    let mut passive_input = remediation("passive", "target", RemediationState::Watching, 10);
    passive_input.origin = "passive".into();
    let passive = store
        .create_or_reuse_remediation(&passive_input, std::slice::from_ref(&guard))
        .unwrap()
        .unwrap();
    let mut snapshot = display_snapshot(&passive.remediation_id);
    snapshot.effective_boundary_ms = passive.effective_boundary_ms.unwrap();
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

    assert_eq!(store.reconcile_remediations(30).unwrap(), 1);
    let recovery = store.next_remediation_write_recovery(30).unwrap().unwrap();
    assert_eq!(recovery.state, RemediationState::RecoveryNeeded);
    assert_eq!(recovery.definition_json, JSON_V1);
    let envelope: serde_json::Value = serde_json::from_str(&recovery.result_json).unwrap();
    assert_eq!(envelope["verification"]["status"], "recoveryNeeded");
    assert!(envelope.get("priorDisplaySnapshot").is_none());
    let passive_after = store.remediation(&passive.remediation_id).unwrap().unwrap();
    assert_eq!(passive_after.state, RemediationState::Watching);
    assert_eq!(
        passive_after.effective_boundary_ms,
        passive.effective_boundary_ms
    );
    assert_eq!(
        store
            .remediation_display_snapshot(&passive.remediation_id)
            .unwrap(),
        Some(snapshot)
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
    assert!(store.remediation_contributions(10).unwrap().is_empty());

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
    assert!(store.remediation_contributions(10).unwrap().is_empty());
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
            .is_some()
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

    assert_eq!(store.clear_local_session_data().unwrap().0, 1);
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
