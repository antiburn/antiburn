use super::*;

fn prerelease_connection() -> rusqlite::Connection {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for sql in &super::super::schema::MIGRATIONS[..48] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute_batch(super::super::schema::MIGRATIONS[50])
        .unwrap();
    connection.pragma_update(None, "user_version", 49).unwrap();
    connection
}

#[test]
fn prerelease_upgrade_preserves_incarnations_and_applies_main_migrations() {
    let connection = prerelease_connection();
    connection
        .execute_batch(
            "INSERT INTO session (
            environment_key, agent, session_id, source_kind, source_label,
            first_seen_at, last_seen_at, incarnation)
         VALUES ('native', 'pi', 'legacy', 'file', '/synthetic/legacy.jsonl', 'now', 'now', 42);
         UPDATE session_incarnation_seq SET value = 57 WHERE id = 1;
         INSERT INTO setting (key, value) VALUES ('internal:liveUsageHistoryV2', '{}');",
        )
        .unwrap();
    let store = Store::from_connection(connection, Path::new("/tmp/legacy-v49").into()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 52);
    assert_eq!(counter(&store), 57);
    let key = SessionKey {
        environment_key: "native".into(),
        agent: "pi".into(),
        session_id: "legacy".into(),
    };
    assert_eq!(stored_incarnation(&store, &key), Some(42));
    let connection = store.lock();
    let remediation_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'remediation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(remediation_sql.contains("waitingForPromptUse"));
    let history: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM setting WHERE key = 'internal:liveUsageHistoryV2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history, 0);
    drop(connection);
    store.migrate().unwrap();
    assert_eq!(counter(&store), 57);
    assert_eq!(stored_incarnation(&store, &key), Some(42));
}

#[test]
fn prerelease_repair_rolls_back_if_a_main_migration_fails() {
    let store = store();
    *store.lock() = prerelease_connection();
    store
        .lock()
        .execute_batch(
            "INSERT INTO setting (key, value) VALUES ('internal:liveUsageHistoryV2', '{}');
         CREATE TRIGGER fail_history_delete BEFORE DELETE ON setting
         BEGIN SELECT RAISE(ABORT, 'synthetic migration failure'); END;",
        )
        .unwrap();
    assert!(store.migrate().is_err());
    assert_eq!(store.schema_version().unwrap(), 49);
    let sql: String = store
        .lock()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'remediation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!sql.contains("waitingForPromptUse"));
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_history_delete")
        .unwrap();
    store.migrate().unwrap();
    assert_eq!(store.schema_version().unwrap(), 52);
}
