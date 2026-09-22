use std::path::Path;

use antiburn_local::analysis::{
    ContentKind, ContentPart, TurnSessionKey, count_turn_content_rows, count_turn_rows,
    insert_turn_rows,
};
use rusqlite::params;

use super::*;

#[test]
fn the_migration_ladder_reaches_the_turn_row_schema() {
    // Pin the count so each new migration requires an explicit test update.
    assert_eq!(super::schema::MIGRATIONS.len(), 57);

    let store = store();
    assert_eq!(store.schema_version().unwrap(), 57);
    let index_exists = store
        .lock()
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                 WHERE type = 'index' AND name = 'turn_usage_timestamp'
            )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap();
    assert!(index_exists);
    let assistant_index_sql = store
        .lock()
        .query_row(
            "SELECT sql FROM sqlite_master
              WHERE type = 'index' AND name = 'turn_assistant_session'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert!(assistant_index_sql.contains("environment_key, agent, session_id, claim_fence"));
    assert!(assistant_index_sql.contains("WHERE role = 'assistant'"));
}

#[test]
fn v50_removes_legacy_live_usage_history_but_preserves_snapshot() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..49] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 49).unwrap();
    connection
        .execute(
            "INSERT INTO setting (key, value) VALUES
                ('internal:liveUsageHistoryV2', 'legacy-history'),
                ('internal:liveUsageSnapshotV2', 'current-snapshot')",
            [],
        )
        .unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-v50-live-usage-migration").to_path_buf(),
    )
    .unwrap();

    assert_eq!(store.schema_version().unwrap(), 57);
    assert_eq!(store.internal_value("internal:liveUsageHistoryV2"), None);
    assert_eq!(
        store.internal_value("internal:liveUsageSnapshotV2"),
        Some("current-snapshot".to_string())
    );
}

#[test]
fn v48_adds_typed_config_attribution_columns_without_backfill() {
    let store = store();
    let connection = store.lock();
    for column in [
        "effective_config_path",
        "effective_config_selector",
        "effective_config_precedence_hash",
        "effective_config_resource_name",
        "effective_config_value_json",
    ] {
        assert!(connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_evidence') WHERE name = ?1)",
                [column],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
    }
}

#[test]
fn v32_indexes_existing_assistant_turns() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..31] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 31).unwrap();
    connection
        .execute(
            "INSERT INTO session (
                environment_key, agent, session_id, source_kind, source_label,
                first_seen_at, last_seen_at
             ) VALUES ('native', 'claude-code', 'indexed', 'file', 'fixture', 'now', 'now')",
            [],
        )
        .unwrap();
    for (index, role) in [(0, "assistant"), (1, "user")] {
        connection
            .execute(
                "INSERT INTO turn (
                environment_key, agent, session_id, claim_fence, source_key, thread_id,
                turn_index, scope, role, input_tokens, cache_read_tokens,
                cache_write_tokens, output_tokens, is_compaction_boundary
            ) VALUES ('native', 'claude-code', 'indexed', 7, 's1', 's1', ?1,
                      'main', ?2, 10, 0, 0, 5, 0)",
                params![index, role],
            )
            .unwrap();
    }

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-v32-index-test").to_path_buf(),
    )
    .unwrap();
    let connection = store.lock();
    let assistant_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*)
               FROM turn INDEXED BY turn_assistant_session
              WHERE environment_key = 'native'
                AND agent = 'claude-code'
                AND session_id = 'indexed'
                AND claim_fence = 7
                AND role = 'assistant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let all_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM turn", [], |row| row.get(0))
        .unwrap();
    let query_plan: String = connection
        .query_row(
            "EXPLAIN QUERY PLAN
             SELECT scope, model
               FROM turn
              WHERE environment_key = 'native'
                AND agent = 'claude-code'
                AND session_id = 'indexed'
                AND claim_fence = 7
                AND role = 'assistant'",
            [],
            |row| row.get(3),
        )
        .unwrap();

    assert_eq!(assistant_rows, 1);
    assert_eq!(all_rows, 2);
    let unknown_routes: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM turn WHERE provider IS NULL AND api IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unknown_routes, 2);
    assert!(query_plan.contains("USING INDEX turn_assistant_session"));
}

/// Publish `uuid` under a fresh claim for `session_id`, so
/// [`Store::sessions_owning_turn_uuids`] can find it as an owner.
fn publish_turn_row_with_uuid(store: &Store, session_id: &str, uuid: &str) -> SessionKey {
    let (record, claim) = claimed_projection(store, session_id, 1_000, 60);
    let mut row = turn_row(0);
    row.uuid = Some(uuid.to_string());
    FencedTurnRowStore::new(store.clone(), record.key.clone(), claim.claim_fence)
        .write_turn_rows(&[row])
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    record.key
}

#[test]
fn latest_session_model_reports_the_model_of_the_newest_turn() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "model-switch", 1_000, 60);
    // A session that changes its model keeps both turns. Only the newer one
    // says which model the session runs now.
    let mut older = turn_row(0);
    older.ts_ms = Some(1_000);
    older.model = Some("claude-fable-5".into());
    older.provider = Some("anthropic".into());
    let mut newer = turn_row(1);
    newer.ts_ms = Some(2_000);
    newer.model = Some("claude-opus-4-6".into());
    newer.provider = Some("openrouter".into());
    FencedTurnRowStore::new(store.clone(), record.key.clone(), claim.claim_fence)
        .write_turn_rows(&[older, newer])
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    assert_eq!(
        store.latest_session_model(&record.key).unwrap(),
        Some("claude-opus-4-6".to_string())
    );
    let rows = store.published_models_for_keys(&[record.key]).unwrap().0;
    assert_eq!(rows[0].provider.as_deref(), Some("openrouter"));
}

#[test]
fn latest_session_model_reports_nothing_before_a_session_publishes() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "never-published", 1_000, 60);
    FencedTurnRowStore::new(store.clone(), record.key.clone(), claim.claim_fence)
        .write_turn_rows(&[turn_row(0)])
        .unwrap();

    // The rows sit under the claim fence, and no publish moved
    // `published_fence`. A meter must not read a pass in flight.
    assert_eq!(store.latest_session_model(&record.key).unwrap(), None);
}

#[test]
fn compact_model_pages_distinguish_missing_unpublished_and_reused_fences() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "model-page", 1_000, 60);
    let missing = SessionKey::new("native", "claude-code", "missing");
    let keys = [record.key.clone(), missing.clone()];
    let (rows, before) = store.published_models_for_keys(&keys).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].published_fence, None);
    assert_eq!(rows[0].model, None);
    let incarnation = rows[0].incarnation;
    let sink = FencedTurnRowStore::new(store.clone(), record.key.clone(), claim.claim_fence);
    let mut row = turn_row(0);
    row.ts_ms = Some(2_000);
    row.model = Some("old".into());
    row.provider = Some("openai-codex".into());
    sink.write_turn_rows(&[row.clone()]).unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    let (first, first_revision) = store.published_models_for_keys(&keys).unwrap();
    assert!(first_revision > before);
    assert_eq!(first[0].model.as_deref(), Some("old"));
    assert_eq!(first[0].provider.as_deref(), Some("openai-codex"));
    let mut unpublished = turn_row(4);
    unpublished.ts_ms = Some(9_000);
    unpublished.model = Some("unpublished-model".into());
    unpublished.provider = Some("unpublished-route".into());
    insert_turn_rows(
        &store.lock(),
        &turn_session_key(&record.key),
        claim.claim_fence + 1,
        &[unpublished],
    )
    .unwrap();
    assert_eq!(store.published_models_for_keys(&keys).unwrap().0, first);
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'processing' WHERE session_id = 'model-page'",
            [],
        )
        .unwrap();
    row.turn_index = 1;
    row.model = Some("new".into());
    row.provider = Some("anthropic".into());
    sink.write_turn_rows(&[row.clone()]).unwrap();
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    let (second, second_revision) = store.published_models_for_keys(&keys).unwrap();
    assert!(second_revision > first_revision);
    assert_eq!(second[0].published_fence, first[0].published_fence);
    assert_eq!(second[0].model.as_deref(), Some("new"));
    assert_eq!(second[0].provider.as_deref(), Some("anthropic"));
    assert_eq!(second[0].incarnation, incarnation);
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'processing' WHERE session_id = 'model-page'",
            [],
        )
        .unwrap();
    let mut tie = turn_row(2);
    tie.ts_ms = row.ts_ms;
    tie.model = Some("tie-winner".into());
    tie.provider = None;
    let mut empty = turn_row(3);
    empty.ts_ms = Some(3_000);
    empty.model = Some(String::new());
    empty.provider = Some("must-not-cross-join".into());
    sink.write_turn_rows(&[tie, empty]).unwrap();
    assert_eq!(
        store.published_models_for_keys(&keys).unwrap().0[0]
            .model
            .as_deref(),
        Some("tie-winner")
    );
    assert_eq!(
        store.published_models_for_keys(&keys).unwrap().0[0].provider,
        None
    );
    store.delete_session(&record.key).unwrap();
    assert!(store.published_models_for_keys(&keys).unwrap().0.is_empty());
    store
        .upsert_sessions(
            &[session("model-page", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let (recreated, revision) = store.published_models_for_keys(&keys).unwrap();
    assert!(revision > second_revision);
    assert!(recreated[0].incarnation > incarnation);
    assert_eq!(recreated[0].published_fence, None);
    assert_eq!(recreated[0].model, None);
    assert!(
        store
            .published_models_for_keys(&vec![missing.clone(); 256])
            .is_ok()
    );
    assert!(
        store
            .published_models_for_keys(&vec![missing; 257])
            .is_err()
    );
}

#[test]
fn sessions_owning_turn_uuids_finds_the_owner_and_excludes_self() {
    let store = store();
    let uuid = "11111111-1111-4111-8111-000000000001";
    publish_turn_row_with_uuid(&store, "uuid-owner-parent", uuid);
    // The querying session also owns a copy of the same uuid: it must not
    // come back as its own candidate.
    let querying_key = publish_turn_row_with_uuid(&store, "uuid-owner-fork", uuid);

    let owners = store
        .sessions_owning_turn_uuids(&querying_key, &[uuid.to_string()])
        .unwrap();

    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].session_id, "uuid-owner-parent");
    assert_eq!(owners[0].published_turn_rows, 1);
}

#[test]
fn an_unpublished_turn_row_does_not_match_a_uuid_lookup() {
    let store = store();
    let uuid = "22222222-2222-4222-8222-000000000001";
    let (record, claim) = claimed_projection(&store, "uuid-unpublished", 1_000, 60);
    let mut row = turn_row(0);
    row.uuid = Some(uuid.to_string());
    FencedTurnRowStore::new(store.clone(), record.key.clone(), claim.claim_fence)
        .write_turn_rows(&[row])
        .unwrap();
    // No `publish_projections` call: the row sits under `claim_fence`,
    // never stamped onto `session_evidence.published_fence`.

    let owners = store
        .sessions_owning_turn_uuids(
            &SessionKey::new("native", "claude-code", "uuid-lookup-key"),
            &[uuid.to_string()],
        )
        .unwrap();

    assert!(owners.is_empty());
}

#[test]
fn sessions_owning_turn_uuids_query_plan_uses_the_turn_uuid_index() {
    let store = store();
    let sql = format!(
        "EXPLAIN QUERY PLAN {}",
        super::sessions_owning_turn_uuids_sql(1)
    );
    let connection = store.lock();
    let mut statement = connection.prepare(&sql).unwrap();
    // env, agent, excluded session_id, one uuid, env, agent — six `?`s for a
    // one-uuid query. The bound values do not matter to a query plan.
    let plan: Vec<String> = statement
        .query_map(
            params![
                "native",
                "claude-code",
                "self",
                "u1",
                "native",
                "claude-code"
            ],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter().any(|line| line.contains("turn_uuid")),
        "expected the plan to use turn_uuid, got: {plan:?}"
    );
}

#[test]
fn a_fenced_turn_row_writer_inserts_rows_the_store_can_count() {
    let store = store();
    let mut record = session("turn-rows-writer", 1_000);
    record.source_fingerprint = Some("sv1:turn-rows-writer".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let key = record.key.clone();

    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), 7);
    writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 7).unwrap(),
        2
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 8).unwrap(),
        0
    );
}

#[test]
fn publishing_evidence_keeps_only_the_current_fence_turn_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-current-fence", 100, 60);
    let key = record.key.clone();

    // A stale pass can leave a row under an old fence.
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 1,
            &[turn_row(0)],
        )
        .unwrap();
    }
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );

    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence - 1).unwrap(),
        0
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        2
    );
}

#[test]
fn a_lost_publish_race_deletes_only_its_own_fences_turn_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "lost-race-turn-rows", 100, 60);
    let key = record.key.clone();
    // A lost race must not remove a row from an earlier pass.
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 1,
            &[turn_row(0)],
        )
        .unwrap();
    }
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    // Change the generation so publish_projections loses the race.
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );

    assert!(
        !store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        0
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence - 1).unwrap(),
        1
    );
}

#[test]
fn deleting_a_session_removes_its_turn_rows() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "turn-rows-delete");
    store
        .upsert_sessions(
            &[session("turn-rows-delete", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    {
        let connection = store.lock();
        insert_turn_rows(&connection, &turn_session_key(&key), 1, &[turn_row(0)]).unwrap();
    }

    assert!(store.delete_session(&key).unwrap().is_some());

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 1).unwrap(),
        0
    );
}

#[test]
fn clearing_local_session_data_removes_every_turn_row() {
    let store = store();
    let mut first = session("clear-turn-rows-one", 1_000);
    first.source_fingerprint = Some("sv1:clear-turn-rows-one".into());
    let mut second = session("clear-turn-rows-two", 1_000);
    second.source_fingerprint = Some("sv1:clear-turn-rows-two".into());
    store
        .upsert_sessions(&[first, second], &crate::agents::evidence_cohort())
        .unwrap();
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude-code",
                session_id: "clear-turn-rows-one",
            },
            1,
            &[turn_row(0)],
        )
        .unwrap();
        insert_turn_rows(
            &connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude-code",
                session_id: "clear-turn-rows-two",
            },
            1,
            &[turn_row(0)],
        )
        .unwrap();
    }

    assert_eq!(store.clear_local_session_data().unwrap().0, 2);

    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM turn", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn deleting_the_session_row_directly_cascades_to_its_turn_rows() {
    // Use raw SQL to verify that the schema enforces the cascade.
    let store = store();
    let key = SessionKey::new("native", "claude-code", "turn-rows-cascade");
    store
        .upsert_sessions(
            &[session("turn-rows-cascade", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    {
        let connection = store.lock();
        insert_turn_rows(&connection, &turn_session_key(&key), 1, &[turn_row(0)]).unwrap();
    }

    store
        .lock()
        .execute(
            "DELETE FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .unwrap();

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 1).unwrap(),
        0
    );
}

fn turn_row_with_content(turn_index: u64, text: &str) -> TurnRow {
    TurnRow {
        content: vec![ContentPart::new(ContentKind::AssistantText, text)],
        ..turn_row(turn_index)
    }
}

#[test]
fn deleting_a_session_removes_turn_content_written_through_the_fenced_writer() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "turn-content-delete-session");
    store
        .upsert_sessions(
            &[session("turn-content-delete-session", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), 1);
    writer
        .write_turn_rows(&[turn_row_with_content(0, "PRIVATE_TURN_CONTENT")])
        .unwrap();
    assert_eq!(
        count_turn_content_rows(&store.lock(), &turn_session_key(&key), 1).unwrap(),
        1
    );

    assert!(store.delete_session(&key).unwrap().is_some());

    assert_eq!(
        count_turn_content_rows(&store.lock(), &turn_session_key(&key), 1).unwrap(),
        0
    );
}

#[test]
fn clearing_local_session_data_removes_turn_content_written_through_the_fenced_writer() {
    let store = store();
    let mut record = session("turn-content-clear", 1_000);
    record.source_fingerprint = Some("sv1:turn-content-clear".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), 1);
    writer
        .write_turn_rows(&[turn_row_with_content(0, "PRIVATE_TURN_CONTENT")])
        .unwrap();
    assert_eq!(
        count_turn_content_rows(&store.lock(), &turn_session_key(&key), 1).unwrap(),
        1
    );

    store.clear_local_session_data().unwrap();

    let remaining: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM turn_content", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
}
