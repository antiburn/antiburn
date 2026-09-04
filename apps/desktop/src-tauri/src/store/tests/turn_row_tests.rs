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
    assert_eq!(super::schema::MIGRATIONS.len(), 33);

    let store = store();
    assert_eq!(store.schema_version().unwrap(), 33);
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
    let mut user_turn = turn_row(1);
    user_turn.role = "user";
    insert_turn_rows(
        &connection,
        &TurnSessionKey {
            environment_key: "native",
            agent: "claude-code",
            session_id: "indexed",
        },
        7,
        &[turn_row(0), user_turn],
    )
    .unwrap();

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
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    record.key
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
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

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
fn session_usage_turns_returns_published_rows_at_the_time_boundary() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "session-usage-turns", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence + 1,
            &[turn_row(2)],
        )
        .unwrap();
    }

    let rows = store.session_usage_turns(1_001).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, key);
    assert_eq!(rows[0].turns.len(), 1);
    assert_eq!(rows[0].turns[0].ts_ms, Some(1_001));
    assert_eq!(rows[0].turns[0].model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(rows[0].turns[0].input_tokens, 10);
    assert_eq!(rows[0].turns[0].output_tokens, 5);
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
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

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

    assert!(store.delete_session(&key).unwrap());

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

    assert_eq!(store.clear_local_session_data().unwrap(), 2);

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

    assert!(store.delete_session(&key).unwrap());

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
