use rusqlite::{Connection, params};

use super::{MIGRATIONS, V91};
use crate::store::{Store, database_path};

fn legacy(connection: &Connection) {
    for migration in &MIGRATIONS[..90] {
        connection.execute_batch(migration).unwrap();
    }
    connection.pragma_update(None, "user_version", 90).unwrap();
    connection
        .execute_batch(
            "INSERT INTO burn_check_decision_usage (attempt_id, usage_known)
            VALUES ('legacy-known:1', 1), ('legacy-unknown:1', 0);
         INSERT INTO setting (key, value) VALUES ('synthetic-profile',
            '{ \"model\": \"jev-1.13.0\", \"revision\": 7 }');
         INSERT INTO setting (key, value) VALUES ('internal:burnCheckUsageLedgerV1',
            '{ \"summary\": { \"unknown_outcomes\": 1, \"future_field\": true } }');",
        )
        .unwrap();
}

fn usage_rows(connection: &Connection) -> Vec<(String, bool, Option<String>)> {
    connection.prepare(
        "SELECT attempt_id, usage_known, served_model FROM burn_check_decision_usage ORDER BY attempt_id",
    ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

#[test]
fn fresh_usage_schema_bounds_observations_without_candidate_ownership() {
    let store = Store::open_in_memory(std::path::Path::new("usage-schema")).unwrap();
    assert_eq!(store.schema_version().unwrap(), 92);
    let connection = store.lock();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('burn_check_decision_usage')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    for model in ["jev-1.14.0", "future/Model:2_0-rc", &"x".repeat(128)] {
        connection.execute(
            "INSERT INTO burn_check_decision_usage (attempt_id, usage_known, served_model) VALUES (?1, 1, ?1)",
            [model],
        ).unwrap();
    }
    for (known, model) in [
        (true, "".to_owned()),
        (true, "x".repeat(129)),
        (true, "unsafe model".to_owned()),
        (true, "model\ncanary".to_owned()),
        (true, "model\0canary".to_owned()),
        (true, "模型".to_owned()),
        (false, "jev-1.14.0".to_owned()),
    ] {
        assert!(connection.execute(
            "INSERT INTO burn_check_decision_usage (attempt_id, usage_known, served_model) VALUES ('invalid', ?1, ?2)",
            params![known, model],
        ).is_err());
    }
    connection
        .execute(
            "INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('unknown', 0)",
            [],
        )
        .unwrap();
}

#[test]
fn v91_upgrade_and_reopen_keep_raw_legacy_rows_null_and_settings_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(database_path(directory.path())).unwrap();
    legacy(&connection);
    let settings: Vec<(String, String)> = connection
        .prepare("SELECT key, value FROM setting ORDER BY key")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    drop(connection);
    for _ in 0..2 {
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(store.schema_version().unwrap(), 92);
        let connection = store.lock();
        assert_eq!(
            usage_rows(&connection),
            vec![
                ("legacy-known:1".into(), true, None),
                ("legacy-unknown:1".into(), false, None),
            ]
        );
        for (key, value) in &settings {
            assert_eq!(
                connection
                    .query_row("SELECT value FROM setting WHERE key = ?1", [key], |row| row
                        .get::<_, String>(0))
                    .unwrap(),
                *value
            );
        }
    }
}

#[test]
fn dropped_usage_migration_transaction_rolls_back_column_rows_and_version() {
    let mut connection = Connection::open_in_memory().unwrap();
    legacy(&connection);
    {
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch(V91).unwrap();
        transaction.pragma_update(None, "user_version", 91).unwrap();
        assert!(
            transaction
                .execute(
                    "UPDATE burn_check_decision_usage SET served_model = 'invalid model'",
                    [],
                )
                .is_err()
        );
    }
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        90
    );
    assert_eq!(connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('burn_check_decision_usage') WHERE name = 'served_model'", [],
        |row| row.get::<_, i64>(0),
    ).unwrap(), 0);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM burn_check_decision_usage",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    let directory = tempfile::tempdir().unwrap();
    let store = Store::from_connection(connection, directory.path().to_path_buf()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 92);
    assert_eq!(usage_rows(&store.lock()).len(), 2);
}

#[test]
fn production_usage_migration_rolls_back_when_version_write_fails() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let directory = tempfile::tempdir().unwrap();
    let path = database_path(directory.path());
    let connection = Connection::open(&path).unwrap();
    legacy(&connection);
    let denied = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&denied);
    connection.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::Pragma {
                pragma_name: "user_version",
                pragma_value: Some("91")
            }
        ) {
            observed.store(true, Ordering::Relaxed);
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }));
    assert!(Store::from_connection(connection, directory.path().to_path_buf()).is_err());
    assert!(denied.load(Ordering::Relaxed));

    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        90
    );
    assert_eq!(connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('burn_check_decision_usage') WHERE name = 'served_model'", [],
        |row| row.get::<_, i64>(0),
    ).unwrap(), 0);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM burn_check_decision_usage",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM setting WHERE key = 'synthetic-profile'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "{ \"model\": \"jev-1.13.0\", \"revision\": 7 }"
    );
    drop(connection);

    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 92);
    assert!(
        usage_rows(&store.lock())
            .iter()
            .all(|(_, _, model)| model.is_none())
    );
}
