use super::*;

fn database(path: &Path, sessions: usize, messages: usize) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../tests/fixtures/opencode_pinned_mixed.sql"
        ))
        .unwrap();
    if sessions > 0 {
        connection.execute_batch(&format!(
            "BEGIN;
             WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x < {})
             INSERT INTO session_v2 (id, project_id, slug, directory, version, time_created, time_updated)
             SELECT 'ses_old_'||x, 'global', 'old', '/repo', '2.0.32', 1600000000000, 1600000000000 FROM n;
             WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x < {})
             INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
             SELECT 'msg_'||s.id||'_'||n.x, s.id, 'assistant', n.x, 1600000000000, 1600000000000, '{{}}'
             FROM session_v2 s CROSS JOIN n;
             COMMIT;",
            sessions - 1, messages - 1,
        )).unwrap();
    }
    connection.execute_batch(
        "INSERT INTO session_v2 (id, project_id, slug, directory, version, time_created, time_updated)
         VALUES ('ses_current', 'global', 'current', '/repo', '2.0.32', 1700000000000, 1700000000000);",
    ).unwrap();
    connection
}

fn discover(root: &Path) -> Vec<SessionLog> {
    match query_recent_db_session_sources(root, 1_699_999_999) {
        DbSourceRead::Ready(logs) => logs,
        DbSourceRead::Failed(stage) => panic!("discovery failed at {stage}"),
        _ => panic!("discovery did not return sources"),
    }
}

fn sources(logs: &[SessionLog]) -> BTreeMap<String, (SourceFormat, i64)> {
    logs.iter()
        .map(|log| {
            let SessionSource::ProviderDb {
                session_id,
                source_format: Some(format),
                ..
            } = &log.source
            else {
                panic!("expected an exact database source");
            };
            (session_id.clone(), (*format, log.updated_at.unwrap()))
        })
        .collect()
}

fn add_mixed_headers(connection: &Connection) {
    connection.execute_batch(
        "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_v1', 'global', 'legacy', '/repo', 'Legacy', '1', 1700000000000, 1700000000000),
                ('ses_current', 'global', 'copy', '/copy', 'Migration copy', '1', 1700000100000, 1700000100000);
         INSERT INTO session (id, project_id, slug, parent_id, directory, title, version, time_created, time_updated)
         VALUES ('ses_v1_child', 'global', 'legacy-child', 'ses_current', '/repo', 'New legacy child', '1', 1700000001000, 1700000001000);",
    ).unwrap();
}

fn expected_headers() -> BTreeMap<String, (SourceFormat, i64)> {
    BTreeMap::from([
        (
            "ses_current".to_owned(),
            (SourceFormat::OpenCodeSqliteV2, 1_700_000_000),
        ),
        (
            "ses_v1".to_owned(),
            (SourceFormat::OpenCodeSqliteV1, 1_700_000_000),
        ),
        (
            "ses_v1_child".to_owned(),
            (SourceFormat::OpenCodeSqliteV1, 1_700_000_001),
        ),
    ])
}

#[test]
fn four_thousand_old_sessions_with_one_hundred_messages_keep_current_sources_discoverable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 4_000, 100);
    assert_eq!(
        sources(&discover(directory.path())),
        BTreeMap::from([(
            "ses_current".to_owned(),
            (SourceFormat::OpenCodeSqliteV2, 1_700_000_000)
        )]),
    );
    add_mixed_headers(&connection);
    assert_eq!(sources(&discover(directory.path())), expected_headers());
}

#[test]
fn complete_metadata_scan_keeps_old_message_updates_appends_and_mixed_v1_activity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 4_000, 100);
    connection.execute_batch(
        "UPDATE session_message SET time_updated = 1700000020000 WHERE id = 'msg_ses_old_0_0';
         UPDATE session_v2 SET parent_id = 'ses_old_1' WHERE id = 'ses_old_0';
         INSERT INTO session_message VALUES ('msg_appended', 'ses_old_2', 'user', 100, 1700000030000, 1700000030000, '{}');
         INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_v1_root', 'global', 'old-legacy', '/repo', 'Legacy', '1', 1600000000000, 1600000000000);
         INSERT INTO session (id, project_id, slug, parent_id, directory, title, version, time_created, time_updated)
         VALUES ('ses_v1_child', 'global', 'old-child', 'ses_v1_root', '/repo', 'Legacy child', '1', 1600000000000, 1600000000000);
         INSERT INTO message VALUES ('msg_v1', 'ses_v1_child', 1600000000000, 1700000040000, '{}');
         INSERT INTO part VALUES ('part_v1', 'msg_v1', 'ses_v1_child', 1600000000000, 1700000050000, '{}');",
    ).unwrap();
    let found = sources(&discover(directory.path()));
    assert_eq!(found.len(), 4);
    assert_eq!(
        found["ses_old_1"],
        (SourceFormat::OpenCodeSqliteV2, 1_700_000_020)
    );
    assert_eq!(
        found["ses_old_2"],
        (SourceFormat::OpenCodeSqliteV2, 1_700_000_030)
    );
    assert_eq!(
        found["ses_v1_root"],
        (SourceFormat::OpenCodeSqliteV1, 1_700_000_050)
    );
}

#[test]
fn optional_metadata_limit_keeps_known_v2_and_unmigrated_v1_headers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 4_000, 250);
    add_mixed_headers(&connection);
    connection
        .execute_batch(
            "UPDATE session_message SET time_updated = 1700000020000 WHERE id = 'msg_ses_old_0_0';
             UPDATE session_v2 SET parent_id = 'ses_old_3999' WHERE id = 'ses_current';
             INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
             VALUES ('ses_old_0', 'global', 'old-copy', '/copy', 'Recent migration copy', '1', 1700000100000, 1700000100000);",
        )
        .unwrap();
    let mut expected = expected_headers();
    let current = expected.remove("ses_current").unwrap();
    expected.insert("ses_old_3999".to_owned(), current);
    assert_eq!(sources(&discover(directory.path())), expected);
}

#[test]
fn retained_v1_message_and_part_history_share_the_optional_scan_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 0, 0);
    add_mixed_headers(&connection);
    connection.execute_batch(
        "BEGIN;
         WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x < 400000)
         INSERT INTO message SELECT 'msg_'||x, 'ses_v1', 1600000000000, 1600000000000, '{}' FROM n;
         INSERT INTO part SELECT 'part_'||id, id, session_id, time_created, time_updated, '{}' FROM message;
         COMMIT;",
    ).unwrap();
    assert_eq!(sources(&discover(directory.path())), expected_headers());
}

#[test]
fn metadata_discovery_does_not_change_the_source_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 1, 10);
    add_mixed_headers(&connection);
    drop(connection);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(sources(&discover(directory.path())), expected_headers());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn mixed_v1_ids_keep_the_existing_permissive_metadata_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 0, 0);
    add_mixed_headers(&connection);
    let id = "legacy_".to_owned() + &"x".repeat(300);
    connection
        .execute("UPDATE session SET id = ?1 WHERE id = 'ses_v1'", [&id])
        .unwrap();
    assert_eq!(
        sources(&discover(directory.path()))[&id],
        (SourceFormat::OpenCodeSqliteV1, 1_700_000_000)
    );
}

#[test]
fn mixed_v1_header_error_keeps_v2_sources() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 0, 0);
    add_mixed_headers(&connection);
    connection
        .execute(
            "UPDATE session SET time_updated = 'invalid' WHERE id = 'ses_v1'",
            [],
        )
        .unwrap();
    assert_eq!(
        sources(&discover(directory.path())),
        BTreeMap::from([(
            "ses_current".to_owned(),
            (SourceFormat::OpenCodeSqliteV2, 1_700_000_000)
        )]),
    );
}

#[test]
fn mixed_v2_header_error_keeps_unmigrated_v1_sources() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, 0, 0);
    add_mixed_headers(&connection);
    connection
        .execute("UPDATE session_v2 SET time_updated = 'invalid'", [])
        .unwrap();
    let mut expected = expected_headers();
    expected.remove("ses_current");
    assert_eq!(sources(&discover(directory.path())), expected);
}

#[test]
fn oversized_header_ids_do_not_return_sibling_sources() {
    for (table, id) in [("session", "ses_v1"), ("session_v2", "ses_current")] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = database(&path, 0, 0);
        add_mixed_headers(&connection);
        let oversized_id = "ses_".to_owned() + &"x".repeat(10 * 1024 * 1024);
        connection
            .execute(
                &format!("UPDATE {table} SET id = ?1 WHERE id = ?2"),
                params![oversized_id, id],
            )
            .unwrap();
        assert!(matches!(
            query_recent_db_session_sources(directory.path(), 1_699_999_999),
            DbSourceRead::Failed("query")
        ));
    }
}

#[test]
fn header_errors_without_usable_sibling_sources_still_fail() {
    for (malformed_v1, malformed_v2, only_family) in [
        (true, false, true),
        (false, true, true),
        (true, true, false),
        (true, false, false),
        (false, true, false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = database(&path, 0, 0);
        add_mixed_headers(&connection);
        if malformed_v1 {
            connection
                .execute("UPDATE session SET time_updated = 'invalid'", [])
                .unwrap();
        } else {
            connection.execute("DELETE FROM session", []).unwrap();
        }
        if malformed_v2 {
            connection
                .execute("UPDATE session_v2 SET time_updated = 'invalid'", [])
                .unwrap();
        } else {
            connection.execute("DELETE FROM session_v2", []).unwrap();
        }
        if only_family {
            connection
                .execute_batch(if malformed_v1 {
                    "DROP TABLE session_message; DROP TABLE session_v2;"
                } else {
                    "DROP TABLE part; DROP TABLE message; DROP TABLE session;"
                })
                .unwrap();
        }
        assert!(matches!(
            query_recent_db_session_sources(directory.path(), 1_699_999_999),
            DbSourceRead::Failed("row")
        ));
    }
}

#[test]
fn header_cancellation_and_authorization_errors_still_fail() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    for table in ["session", "session_v2"] {
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("opencode.db");
            let connection = database(&path, 0, 0);
            add_mixed_headers(&connection);
            let cancelled = Arc::new(AtomicBool::new(false));
            let budget = QueryBudget::new(&connection, cancelled.clone());
            connection.authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name,
                        column_name: "time_updated"
                    } if table_name == table
                ) {
                    if cancel {
                        cancelled.store(true, Ordering::Release);
                        return Authorization::Allow;
                    }
                    return Authorization::Deny;
                }
                Authorization::Allow
            }));
            let result = query_recent_db_session_sources_connection(
                &connection,
                &path,
                1_699_999_999,
                &budget,
            );
            assert!(matches!(
                result,
                DbSourceRead::Failed(stage) if stage == if cancel { "cancelled" } else { "query" }
            ));
        }
    }
}

#[test]
fn cancellation_and_authorization_errors_do_not_return_header_fallback() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    for cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = database(&path, 1, 100);
        let cancelled = Arc::new(AtomicBool::new(false));
        let budget = QueryBudget::new(&connection, cancelled.clone());
        connection.authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "session_message",
                    column_name: "time_updated"
                }
            ) {
                if cancel {
                    cancelled.store(true, Ordering::Release);
                    return Authorization::Allow;
                }
                return Authorization::Deny;
            }
            Authorization::Allow
        }));
        let result =
            query_recent_db_session_sources_connection(&connection, &path, 1_699_999_999, &budget);
        if cancel {
            assert!(matches!(result, DbSourceRead::Failed("cancelled")));
            assert_eq!(budget.check(), Err("cancelled"));
        } else {
            assert!(matches!(result, DbSourceRead::Failed("activity")));
        }
    }
    let directory = tempfile::tempdir().unwrap();
    database(&directory.path().join("opencode.db"), 1, 10);
    assert!(matches!(
        query_recent_db_session_sources_cancellable(
            directory.path(),
            0,
            Arc::new(AtomicBool::new(true))
        ),
        DbSourceRead::Failed("cancelled")
    ));
}

#[test]
fn required_hierarchy_errors_still_fail_closed() {
    for parent in ["ses_missing", "ses_current"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = database(&path, 0, 0);
        connection
            .execute("UPDATE session_v2 SET parent_id = ?1", [parent])
            .unwrap();
        assert!(matches!(
            query_recent_db_session_sources(directory.path(), 0),
            DbSourceRead::Ready(logs) if logs.is_empty()
        ));
    }
    for depth in [MAX_V2_PARENT_DEPTH, MAX_V2_PARENT_DEPTH + 1] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = database(&path, depth, 1);
        for index in 0..depth {
            let id = if index == 0 {
                "ses_current".to_owned()
            } else {
                format!("ses_old_{}", index - 1)
            };
            connection
                .execute(
                    "UPDATE session_v2 SET parent_id = ?1 WHERE id = ?2",
                    params![format!("ses_old_{index}"), id],
                )
                .unwrap();
        }
        let result = query_recent_db_session_sources(directory.path(), 1_699_999_999);
        if depth == MAX_V2_PARENT_DEPTH {
            let DbSourceRead::Ready(logs) = result else {
                panic!("bounded hierarchy failed")
            };
            assert!(sources(&logs).contains_key(&format!("ses_old_{}", depth - 1)));
        } else {
            assert!(matches!(result, DbSourceRead::Ready(logs) if logs.is_empty()));
        }
    }
}

#[test]
fn optional_activity_does_not_hide_an_unsafe_hierarchy_or_oversized_parent() {
    for parent in ["ses_old_0".to_owned(), "ses_".to_owned() + &"x".repeat(256)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = database(&path, 1, 1);
        connection
            .execute(
                "UPDATE session_v2 SET parent_id = ?1 WHERE id = 'ses_old_0'",
                [parent],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE session_message SET time_updated = 1700000020000",
                [],
            )
            .unwrap();
        let found = sources(&discover(directory.path()));
        assert_eq!(found.len(), 1);
        assert_eq!(
            found["ses_current"],
            (SourceFormat::OpenCodeSqliteV2, 1_700_000_000)
        );
    }
}

#[test]
fn recent_candidates_keep_the_existing_v2_limit_and_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = database(&path, MAX_V2_RECENT_SESSIONS, 1);
    connection
        .execute(
            "UPDATE session_v2 SET time_updated = 1700000000000 WHERE id != 'ses_current'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET time_updated = 1700000001000 WHERE id = 'ses_current'",
            [],
        )
        .unwrap();
    let logs = discover(directory.path());
    assert_eq!(logs.len(), MAX_V2_RECENT_SESSIONS);
    assert!(
        matches!(&logs[0].source, SessionSource::ProviderDb { session_id, .. } if session_id == "ses_current")
    );
}
