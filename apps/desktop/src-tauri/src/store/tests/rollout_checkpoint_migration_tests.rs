use std::path::Path;

use super::*;

#[test]
fn migrating_forward_over_a_v40_database_adds_the_rollout_checkpoint_table() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..40] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO session (
                 environment_key, agent, session_id, source_kind, source_label, surface,
                 activity_cursor, activity_source, first_seen_at, last_seen_at
             ) VALUES ('native', 'codex', 'session', 'file', 'test', 'cli',
                       'test', 'event', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .pragma_update(None, "user_version", 40i64)
        .unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-migration-test").to_path_buf(),
    )
    .expect("migrates cleanly past a V40 database");
    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );

    let connection = store.lock();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_usage_rollout_checkpoint (
                 environment_key, agent, session_id, provider, source_label,
                 status, updated_at_epoch
             ) VALUES ('native', 'codex', 'session', 'openai', 'test', 'pending', 1)",
            [],
        )
        .expect("V41 accepts a checkpoint row for an existing session");

    connection
        .execute(
            "DELETE FROM session WHERE environment_key = 'native' AND agent = 'codex'
               AND session_id = 'session'",
            [],
        )
        .unwrap();
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM provider_usage_rollout_checkpoint",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        remaining, 0,
        "the checkpoint's foreign key cascades a session delete"
    );
}
