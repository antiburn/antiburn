use super::*;

fn v2_source(path: &std::path::Path) -> (rusqlite::Connection, SessionLog) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    connection
        .execute_batch(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../crates/antiburn-local/src/discovery/agents/tests/fixtures/opencode_pinned_mixed.sql"
        )))
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO session_v2
                (id, project_id, slug, directory, title, version, time_created, time_updated)
             VALUES ('ses_session', 'project', 'session', '/work/project', 'V2 title', '2', 1000, 1000),
                    ('ses_other', 'project', 'other', '/work/project', 'Other', '2', 1000, 1000);
             INSERT INTO session
                (id, project_id, slug, directory, title, version, time_created, time_updated)
             VALUES ('ses_session', 'project', 'session', '/work/project', 'V1 copy', '1', 1000, 1000);
             INSERT INTO session_message VALUES
                ('message', 'ses_session', 'user', 1, 1000, 1000, '{\"text\":\"before\"}');",
        )
        .unwrap();
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: path.to_path_buf(),
            session_id: "ses_session".into(),
            source_format: Some(SourceFormat::OpenCodeSqliteV2),
        },
        updated_at: Some(1),
        environment: DiscoveryEnvironment::Native,
    };
    (connection, log)
}

#[tokio::test]
async fn describing_v2_admits_metadata_before_strict_fingerprint_work() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let (_writer, log) = v2_source(&path);
    let revision = admission_revision(&log).await.unwrap();
    let label = log.source_label();
    let DescribeOutcome::Session(record) = describe_one(log, directory.path(), None).await else {
        panic!("expected admitted V2 metadata");
    };
    assert_eq!(record.source_label, label);
    assert_eq!(record.activity_cursor, revision);
    assert_eq!(record.source_fingerprint, None);
    assert_eq!(record.title.as_deref(), Some("V2 title"));
    assert_eq!(record.cwd.as_deref(), Some("/work/project"));
}

#[tokio::test]
async fn v2_admission_ignores_payload_edits_but_strict_reconciliation_reads_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let (writer, log) = v2_source(&path);
    assert!(uses_bounded_admission(&log));
    let revision = admission_revision(&log).await.unwrap();
    let fingerprint = strict_fingerprint(&log, &revision).await.unwrap();
    writer
        .execute(
            "UPDATE session_message SET data = '{\"text\":\"edited\"}' WHERE id = 'message'",
            [],
        )
        .unwrap();
    assert_eq!(admission_revision(&log).await.as_ref(), Some(&revision));
    assert_ne!(
        strict_fingerprint(&log, &revision).await.as_ref(),
        Some(&fingerprint)
    );
    writer
        .execute(
            "UPDATE session_v2 SET time_updated = 2000 WHERE id = 'ses_session'",
            [],
        )
        .unwrap();
    assert_ne!(admission_revision(&log).await.as_ref(), Some(&revision));
    assert!(strict_fingerprint(&log, &revision).await.is_none());
}

#[tokio::test]
async fn v2_unrelated_wal_writes_and_checkpoints_keep_the_scoped_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let (writer, log) = v2_source(&path);
    let revision = admission_revision(&log).await.unwrap();
    let fingerprint = strict_fingerprint(&log, &revision).await.unwrap();
    writer
        .execute_batch(
            "UPDATE session_v2 SET time_updated = 2000 WHERE id = 'ses_other';
             UPDATE session SET time_updated = 2000 WHERE id = 'ses_session';
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    assert_eq!(admission_revision(&log).await.as_ref(), Some(&revision));
    assert_eq!(
        strict_fingerprint(&log, &revision).await.as_ref(),
        Some(&fingerprint)
    );
}

#[tokio::test]
async fn deleting_v2_does_not_use_the_retained_v1_row_for_admission_or_existence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let (writer, log) = v2_source(&path);
    let revision = admission_revision(&log).await.unwrap();
    let mut v1 = log.clone();
    let SessionSource::ProviderDb { source_format, .. } = &mut v1.source else {
        unreachable!();
    };
    *source_format = Some(SourceFormat::OpenCodeSqliteV1);
    assert!(admission_revision(&v1).await.is_none());
    assert!(!source_is_deleted(&log).await);
    writer
        .execute("DELETE FROM session_v2 WHERE id = 'ses_session'", [])
        .unwrap();
    assert!(source_is_deleted(&log).await);
    assert!(admission_revision(&log).await.is_none());
    assert!(strict_fingerprint(&log, &revision).await.is_none());

    assert!(!source_is_deleted(&v1).await);
    assert!(admission_revision(&v1).await.is_some());
}

#[tokio::test]
async fn deleting_the_retained_v1_row_does_not_delete_v2() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let (writer, log) = v2_source(&path);
    writer
        .execute("DELETE FROM session WHERE id = 'ses_session'", [])
        .unwrap();
    assert!(!source_is_deleted(&log).await);
    assert!(admission_revision(&log).await.is_some());
}

#[tokio::test]
async fn missing_v2_tables_are_not_proof_of_session_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let (writer, log) = v2_source(&path);
    writer.execute_batch("DROP TABLE session_v2").unwrap();
    assert!(!source_is_deleted(&log).await);
    assert!(admission_revision(&log).await.is_none());
}

#[tokio::test]
async fn replacing_identical_v2_rows_invalidates_admission_and_strict_work() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let (writer, log) = v2_source(&path);
    let revision = admission_revision(&log).await.unwrap();
    let fingerprint = strict_fingerprint(&log, &revision).await.unwrap();
    writer
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(writer);
    std::fs::rename(&path, directory.path().join("previous.db")).unwrap();
    let (_replacement, _) = v2_source(&path);
    let current = admission_revision(&log).await.unwrap();
    assert_ne!(current, revision);
    assert!(strict_fingerprint(&log, &revision).await.is_none());
    assert_ne!(
        strict_fingerprint(&log, &current).await.as_ref(),
        Some(&fingerprint)
    );
}
