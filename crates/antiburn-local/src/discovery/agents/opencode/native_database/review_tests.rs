use super::*;

#[test]
fn fork_inference_does_not_return_a_partial_winner_after_budget_exhaustion() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!(
            "../../tests/fixtures/opencode_pinned_mixed.sql"
        ))
        .unwrap();
    connection.execute_batch(
        r#"DROP TABLE session_message; DROP TABLE session_v2;
         CREATE INDEX message_session_idx ON message(session_id);
         CREATE INDEX part_message_idx ON part(message_id);
         INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
           VALUES ('ses_first', 'global', 'first', '/repo', 'Task', '1', 1, 1),
                  ('ses_later', 'global', 'later', '/repo', 'Task', '1', 1, 1),
                  ('ses_child', 'global', 'child', '/repo', 'Task (fork #1)', '1', 2, 2);
         INSERT INTO message VALUES
           ('msg_first', 'ses_first', 1, 1, '{"role":"user"}'),
           ('msg_later', 'ses_later', 1, 1, '{"role":"user"}'),
           ('msg_child', 'ses_child', 1, 1, '{"role":"user"}');
         INSERT INTO part VALUES
           ('prt_first', 'msg_first', 'ses_first', 1, 1, '{"type":"text","text":"shared"}'),
           ('prt_first_2', 'msg_first', 'ses_first', 2, 2, '{"type":"text","text":"shared again"}'),
           ('prt_child_1', 'msg_child', 'ses_child', 1, 1, '{"type":"text","text":"shared"}'),
           ('prt_child_2', 'msg_child', 'ses_child', 2, 2, '{"type":"text","text":"shared again"}'),
           ('prt_child_3', 'msg_child', 'ses_child', 3, 3, '{"type":"text","text":"new"}');
         WITH RECURSIVE numbers(n) AS (
           VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 5000
         ) INSERT INTO part SELECT 'prt_later_' || n, 'msg_later', 'ses_later', n, n,
             '{"type":"text","text":"shared"}' FROM numbers;"#
    ).unwrap();
    {
        let budget = QueryBudget::new(&connection, Arc::new(AtomicBool::new(false)));
        assert_eq!(
            db_fork_parent_connection(&connection, "ses_child", &budget).as_deref(),
            Some("ses_first")
        );
        assert_eq!(budget.check(), Ok(()));
    }
    let budget = QueryBudget::with_limits(
        &connection,
        Arc::new(AtomicBool::new(false)),
        250_000,
        Duration::from_secs(10),
    );
    assert!(db_fork_parent_connection(&connection, "ses_child", &budget).is_none());
    assert_eq!(budget.check(), Err("query_limit"));
}

#[test]
fn rust_value_sizes_match_the_sql_metadata_size_rule() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE sizes(value); INSERT INTO sizes VALUES (NULL), (1), (1.5), ('é'), (x'010203');").unwrap();
    let (selected, bytes) = bounded_v2_columns(&connection, "sizes", &["value"], 100);
    let mut statement = connection
        .prepare(&format!("SELECT {selected}, {bytes} FROM sizes"))
        .unwrap();
    let mut rows = statement.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        assert_eq!(
            value_ref_bytes(row.get_ref(0).unwrap()),
            row.get::<_, u64>(1).unwrap()
        );
    }
}

#[test]
fn maximum_v2_cluster_has_an_admission_revision() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!(
            "../../tests/fixtures/opencode_pinned_mixed.sql"
        ))
        .unwrap();
    for index in 0..MAX_V2_CLUSTER_SESSIONS {
        let id = format!("ses_{index}");
        let parent = (index != 0).then_some("ses_0");
        connection.execute(
            "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, version, time_created, time_updated)
             VALUES (?1, 'global', ?2, 'test', '/repo', '2', 1, 1)",
            params![id, parent],
        ).unwrap();
    }
    let revision =
        db_session_admission_revision_connection_with_identity(&connection, "ses_0", "memory");
    assert!(revision.is_some());
}

#[test]
fn recent_v2_and_v1_headers_survive_v1_activity_budget_exhaustion() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!(
            "../../tests/fixtures/opencode_pinned_mixed.sql"
        ))
        .unwrap();
    connection.execute_batch(
        "INSERT INTO session_v2 (id, project_id, slug, directory, version, time_created, time_updated)
           VALUES ('ses_healthy', 'global', 'healthy', '/repo', '2', 1800000000000, 1800000000000);
         INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
           VALUES ('ses_partial_v1', 'global', 'partial', '/repo', 'Partial V1', '1', 1800000000000, 1800000000000);
         DROP TABLE part;
         CREATE VIEW part AS WITH RECURSIVE numbers(n) AS (
             VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 6000000
         ) SELECT 'prt_' || n AS id, 'msg_old' AS message_id, 'ses_old' AS session_id,
                  n AS time_created, n AS time_updated, '{}' AS data FROM numbers;"
    ).unwrap();
    let budget = QueryBudget::new(&connection, Arc::new(AtomicBool::new(false)));
    let result = query_recent_db_session_sources_connection(
        &connection,
        Path::new("/synthetic/opencode.db"),
        1_800_000_000,
        &budget,
    );
    let DbSourceRead::Ready(logs) = result else {
        panic!("completed V2 results must survive");
    };
    assert_eq!(logs.len(), 2);
    assert!(
        logs.iter()
            .any(|log| matches!(&log.source, SessionSource::ProviderDb {
        session_id, source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2), ..
    } if session_id == "ses_healthy"))
    );
    assert!(
        logs.iter()
            .any(|log| matches!(&log.source, SessionSource::ProviderDb {
        session_id, source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV1), ..
    } if session_id == "ses_partial_v1"))
    );
    assert!(budget.check().is_ok());
    drop(budget);
    let budget = QueryBudget::new(&connection, Arc::new(AtomicBool::new(true)));
    assert!(matches!(
        query_recent_db_session_sources_connection(
            &connection,
            Path::new("/synthetic/opencode.db"),
            1_800_000_000,
            &budget,
        ),
        DbSourceRead::Failed("cancelled")
    ));
}
