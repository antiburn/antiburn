use super::*;
use crate::analysis::SourceFormat;

fn db_session_admission_revision_connection(
    connection: &rusqlite::Connection,
    session_id: &str,
) -> Option<String> {
    let identity = match connection.path()? {
        "" => "memory".to_owned(),
        path => {
            let file = std::fs::File::open(path).ok()?;
            crate::discovery::SourceStat::from_open_std_file(&file)?.identity?
        }
    };
    crate::discovery::agents::opencode::db_session_admission_revision_connection_with_identity(
        connection, session_id, &identity,
    )
}

fn db_session_fingerprint_connection(
    connection: &rusqlite::Connection,
    session_id: &str,
) -> Option<(u64, u64)> {
    let identity = match connection.path()? {
        "" => "memory".to_owned(),
        path => {
            let file = std::fs::File::open(path).ok()?;
            crate::discovery::SourceStat::from_open_std_file(&file)?.identity?
        }
    };
    crate::discovery::agents::opencode::db_session_fingerprint_cancellable_with_identity(
        connection,
        session_id,
        &identity,
        &|| false,
    )
}

#[test]
fn native_directory_entry_limit_does_not_silently_select_a_subset() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..=MAX_DB_DIRECTORY_ENTRIES {
        std::fs::write(directory.path().join(format!("unrelated{index}")), []).unwrap();
    }
    assert_eq!(
        database_paths_for_root_config(directory.path(), None),
        Err("directory_limit")
    );
}

#[test]
fn native_mixed_store_keeps_new_v1_children_out_of_migrated_v2_clusters() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection.execute_batch(
        "INSERT INTO session (id, project_id, slug, parent_id, directory, title, version, time_created, time_updated)
         VALUES ('ses_new_child', 'global', 'new-child', 'ses_root', '/legacy', 'New V1 child', '1', 1700000020000, 1700000020000);",
    ).unwrap();
    let logs = recent_db_session_logs(directory.path(), 0);
    assert!(logs.iter().any(|log| matches!(
        &log.source,
        SessionSource::ProviderDb { session_id, source_format: Some(SourceFormat::OpenCodeSqliteV1), .. }
            if session_id == "ses_new_child"
    )));
}

#[test]
fn native_mixed_store_does_not_hash_migrated_child_work_into_a_v1_root() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection.execute_batch(
        "INSERT INTO session (id, project_id, slug, parent_id, directory, title, version, time_created, time_updated)
         VALUES ('ses_child', 'global', 'copied-child', 'ses_v1_only', '/legacy', 'V1 copy', '1', 1700000000000, 1700000000000);
         INSERT INTO message VALUES ('msg_copied_child', 'ses_child', 1700000000000, 1700000000000, '{\"role\":\"user\"}');",
    ).unwrap();
    let before = db_session_fingerprint_connection(&connection, "ses_v1_only").unwrap();
    let admission_before =
        db_session_admission_revision_connection(&connection, "ses_v1_only").unwrap();
    connection
        .execute(
            "UPDATE message SET data = '{\"role\":\"assistant\"}' WHERE id = 'msg_copied_child'",
            [],
        )
        .unwrap();
    assert_eq!(
        before,
        db_session_fingerprint_connection(&connection, "ses_v1_only").unwrap()
    );
    connection.execute_batch(
        "UPDATE session SET title = 'Changed copy', time_updated = 1700000040000 WHERE id = 'ses_child';
         UPDATE message SET time_updated = 1700000040000 WHERE id = 'msg_copied_child';
         INSERT INTO part VALUES ('prt_copied_child', 'msg_copied_child', 'ses_child', 1700000040000, 1700000040000, '{}');",
    ).unwrap();
    assert_eq!(
        admission_before,
        db_session_admission_revision_connection(&connection, "ses_v1_only").unwrap()
    );
    assert_eq!(
        before,
        db_session_fingerprint_connection(&connection, "ses_v1_only").unwrap()
    );
    let records = query_db_records_for_root_connection(&connection, &path, "ses_v1_only").unwrap();
    assert!(records.sessions.iter().all(|(id, _)| id != "ses_child"));
    assert!(
        records
            .messages
            .iter()
            .all(|message| message.session_id != "ses_child")
    );
}

#[test]
fn native_v2_fork_parent_uses_the_explicit_native_field() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    assert_eq!(
        db_fork_parent_blocking(&path, "ses_fork").as_deref(),
        Some("ses_root")
    );
    connection
        .execute(
            "UPDATE session_v2 SET title = 'Unrelated title' WHERE id = 'ses_fork'",
            [],
        )
        .unwrap();
    assert_eq!(
        db_fork_parent_blocking(&path, "ses_fork").as_deref(),
        Some("ses_root")
    );
    assert!(db_fork_parent_blocking(&path, "ses_child").is_none());
}

#[test]
fn native_missing_and_busy_stores_do_not_look_like_successful_empty_discovery() {
    let missing = tempfile::tempdir().unwrap();
    assert!(matches!(
        query_recent_db_session_sources(missing.path(), 0),
        DbSourceRead::NotPresent
    ));
    let locked = tempfile::tempdir().unwrap();
    let connection = pinned_mixed_db(&locked.path().join("opencode.db"));
    connection.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(matches!(
        query_recent_db_session_sources(locked.path(), 0),
        DbSourceRead::Failed(_)
    ));
}

#[test]
fn native_v2_fingerprint_tracks_the_exact_parent_boundary_without_parent_usage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/opencode_pinned_mixed.sql"))
        .unwrap();
    connection.execute_batch(
        "INSERT INTO session_v2 (id, project_id, slug, directory, version, time_created, time_updated)
           VALUES ('ses_parent', 'global', 'parent', '/repo', '2.0.32', 1000, 1000),
                  ('ses_fork', 'global', 'fork', '/repo', '2.0.32', 1000, 1000);
         UPDATE session_v2 SET fork_session_id = 'ses_parent',
           fork_boundary = '{\"type\":\"through\",\"messageID\":\"msg_boundary\"}' WHERE id = 'ses_fork';
         INSERT INTO session_message VALUES
           ('msg_boundary', 'ses_parent', 'assistant', 4, 1000, 1000, '{}');"
    ).unwrap();
    let original = db_session_fingerprint_connection(&connection, "ses_fork").unwrap();
    connection
        .execute(
            "UPDATE session_message SET data = '{\"unrelated\":true}' WHERE id = 'msg_boundary'",
            [],
        )
        .unwrap();
    assert_eq!(
        db_session_fingerprint_connection(&connection, "ses_fork").unwrap(),
        original
    );
    connection
        .execute(
            "UPDATE session_message SET seq = 5 WHERE id = 'msg_boundary'",
            [],
        )
        .unwrap();
    let moved = db_session_fingerprint_connection(&connection, "ses_fork").unwrap();
    assert_ne!(moved, original);
    connection
        .execute("DELETE FROM session_message WHERE id = 'msg_boundary'", [])
        .unwrap();
    let missing = db_session_fingerprint_connection(&connection, "ses_fork").unwrap();
    assert_ne!(missing, moved);
    connection.execute("INSERT INTO session_message VALUES ('msg_boundary', 'ses_parent', 'assistant', 4, 1000, 1000, '{}')", []).unwrap();
    assert_eq!(
        db_session_fingerprint_connection(&connection, "ses_fork").unwrap(),
        original
    );
}

#[test]
fn native_v2_fork_lookup_rejects_invalid_ids_without_v1_title_inference() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    for parent in [
        None,
        Some("ses_fork"),
        Some("not a session id"),
        Some("msg_parent"),
    ] {
        connection.execute(
            "UPDATE session_v2 SET fork_session_id = ?1, title = 'V1 copy (fork #1)' WHERE id = 'ses_fork'",
            [parent],
        ).unwrap();
        assert!(db_fork_parent_blocking(&path, "ses_fork").is_none());
    }
}

#[test]
fn native_v2_fork_fingerprint_uses_the_parsers_duplicate_key_rules() {
    let directory = tempfile::tempdir().unwrap();
    let connection = pinned_mixed_db(&directory.path().join("opencode.db"));
    connection
        .execute_batch(
            "INSERT INTO session_message VALUES
          ('msg_decoy', 'ses_root', 'assistant', 2, 1000, 1000, '{}'),
          ('msg_used', 'ses_root', 'assistant', 3, 1000, 1000, '{}');",
        )
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET fork_boundary = ?1 WHERE id = 'ses_fork'",
            [r#"{"type":"through","messageID":"msg_decoy","messageID":"msg_used"}"#],
        )
        .unwrap();
    let before = db_session_fingerprint_connection(&connection, "ses_fork").unwrap();
    connection
        .execute(
            "UPDATE session_message SET seq = 4 WHERE id = 'msg_used'",
            [],
        )
        .unwrap();
    assert_ne!(
        db_session_fingerprint_connection(&connection, "ses_fork").unwrap(),
        before
    );
}

#[test]
fn native_v2_fork_fingerprint_does_not_parse_an_oversized_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let connection = pinned_mixed_db(&directory.path().join("opencode.db"));
    let boundary = serde_json::json!({
        "type":"through", "messageID":"msg_old", "padding":"x".repeat(4096),
    })
    .to_string();
    connection
        .execute(
            "UPDATE session_v2 SET fork_boundary = ?1 WHERE id = 'ses_fork'",
            [&boundary],
        )
        .unwrap();
    let before = db_session_fingerprint_connection(&connection, "ses_fork").unwrap();
    connection
        .execute(
            "UPDATE session_message SET seq = 99 WHERE id = 'msg_old'",
            [],
        )
        .unwrap();
    assert_eq!(
        db_session_fingerprint_connection(&connection, "ses_fork").unwrap(),
        before
    );
}

#[test]
fn native_v2_fork_fingerprint_rejects_a_null_sequence_instead_of_claiming_absence() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(directory.path().join("opencode.db")).unwrap();
    connection
        .execute_batch(
            &include_str!("fixtures/opencode_pinned_mixed.sql")
                .replace("seq INTEGER NOT NULL", "seq INTEGER"),
        )
        .unwrap();
    connection.execute_batch(
        "INSERT INTO session_v2 (id, project_id, slug, directory, version, time_created, time_updated)
           VALUES ('ses_parent', 'global', 'parent', '/repo', '2.0.32', 1000, 1000),
                  ('ses_fork', 'global', 'fork', '/repo', '2.0.32', 1000, 1000);
         UPDATE session_v2 SET fork_session_id = 'ses_parent',
           fork_boundary = '{\"type\":\"through\",\"messageID\":\"msg_boundary\"}' WHERE id = 'ses_fork';"
    ).unwrap();
    assert!(db_session_fingerprint_connection(&connection, "ses_fork").is_some());
    connection.execute("INSERT INTO session_message VALUES ('msg_boundary', 'ses_parent', 'assistant', NULL, 1000, 1000, '{}')", []).unwrap();
    assert!(db_session_fingerprint_connection(&connection, "ses_fork").is_none());
}

#[test]
fn native_v1_grandchild_starts_a_new_cluster_below_a_migrated_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection.execute_batch(
        "INSERT INTO session (id, project_id, slug, parent_id, directory, title, version, time_created, time_updated)
         VALUES ('ses_child', 'global', 'copied-child', 'ses_v1_only', '/legacy', 'V1 child copy', '1', 1700000000000, 1700000000000),
                ('ses_new_grandchild', 'global', 'new-child', 'ses_child', '/legacy/new', 'New V1 work', '1', 1700000020000, 1700000020000),
                ('ses_new_descendant', 'global', 'new-descendant', 'ses_new_grandchild', '/legacy/new', 'New V1 descendant', '1', 1700000030000, 1700000030000);",
    ).unwrap();
    let logs = recent_db_session_logs(directory.path(), 1_700_000_015);
    assert_eq!(logs.len(), 1);
    assert!(matches!(
        &logs[0].source,
        SessionSource::ProviderDb { session_id, source_format: Some(SourceFormat::OpenCodeSqliteV1), .. }
            if session_id == "ses_new_grandchild"
    ));
    assert_eq!(logs[0].updated_at, Some(1_700_000_030));
    let records =
        query_db_records_for_root_connection(&connection, &path, "ses_new_grandchild").unwrap();
    let ids: HashSet<_> = records.sessions.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        HashSet::from(["ses_new_grandchild", "ses_new_descendant"])
    );
}

#[test]
fn native_wal_session_deletion_removes_the_exact_source_without_changing_the_main_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE)",
        )
        .unwrap();
    let main_file = std::fs::read(&path).unwrap();
    assert!(db_session_fingerprint_blocking(&path, "ses_fork").is_some());
    connection
        .execute("DELETE FROM session_v2 WHERE id = 'ses_fork'", [])
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), main_file);
    assert!(db_session_source_format(&path, "ses_fork").is_none());
    assert!(
        db_session_metadata_blocking(&path, "ses_fork", SourceFormat::OpenCodeSqliteV2).is_none()
    );
    assert!(db_session_fingerprint_blocking(&path, "ses_fork").is_none());
    assert!(db_session_admission_revision_connection(&connection, "ses_fork").is_none());
    assert!(db_fork_parent_blocking(&path, "ses_fork").is_none());
    assert!(query_session_title_by_id(&path, "ses_fork").is_none());
    assert!(
        recent_db_session_logs(directory.path(), 0)
            .iter()
            .all(|log| !matches!(&log.source, SessionSource::ProviderDb { session_id, .. } if session_id == "ses_fork"))
    );
}

#[test]
fn native_session_recreation_reads_the_new_rows_instead_of_cached_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    let before = db_session_fingerprint_blocking(&path, "ses_fork").unwrap();
    connection
        .execute("DELETE FROM session_v2 WHERE id = 'ses_fork'", [])
        .unwrap();
    assert!(db_session_fingerprint_blocking(&path, "ses_fork").is_none());
    connection.execute_batch(
        "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_fork', 'global', 'recreated', '/new/exact', 'Recreated session', '2.0.32', 1700000040000, 1700000040000);
         INSERT INTO session_message VALUES ('msg_recreated', 'ses_fork', 'user', 0, 1700000040000, 1700000040000, '{}');",
    ).unwrap();
    let after = db_session_fingerprint_blocking(&path, "ses_fork").unwrap();
    assert_ne!(before, after);
    let metadata =
        db_session_metadata_blocking(&path, "ses_fork", SourceFormat::OpenCodeSqliteV2).unwrap();
    assert_eq!(metadata.title.as_deref(), Some("Recreated session"));
    assert_eq!(metadata.cwd.as_deref(), Some("/new/exact"));
    assert!(db_fork_parent_blocking(&path, "ses_fork").is_none());
    let logs = recent_db_session_logs(directory.path(), 1_700_000_035);
    assert_eq!(logs.len(), 1);
    assert!(matches!(
        &logs[0].source,
        SessionSource::ProviderDb { session_id, source_format: Some(SourceFormat::OpenCodeSqliteV2), .. }
            if session_id == "ses_fork"
    ));
}

#[test]
fn native_database_deletion_and_identical_recreation_change_source_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    let before = db_session_fingerprint_blocking(&path, "ses_fork").unwrap();
    let admission_before =
        db_session_admission_revision_connection(&connection, "ses_fork").unwrap();
    drop(connection);
    let contents = std::fs::read(&path).unwrap();
    let original_file = std::fs::File::open(&path).unwrap();
    let original_identity = crate::discovery::SourceStat::from_open_std_file(&original_file)
        .unwrap()
        .identity
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 0),
        DbSourceRead::NotPresent
    ));
    assert!(db_session_source_format(&path, "ses_fork").is_none());
    assert!(
        db_session_metadata_blocking(&path, "ses_fork", SourceFormat::OpenCodeSqliteV2).is_none()
    );
    assert!(db_session_fingerprint_blocking(&path, "ses_fork").is_none());
    assert!(!path.exists());
    std::fs::write(&path, contents).unwrap();
    let after = db_session_fingerprint_blocking(&path, "ses_fork").unwrap();
    assert_eq!(before.1, after.1);
    assert_ne!(before.0, after.0);
    let recreated = Connection::open(&path).unwrap();
    assert!(
        crate::discovery::agents::opencode::db_session_admission_revision_connection_with_identity(
            &recreated,
            "ses_fork",
            &original_identity,
        )
        .is_none()
    );
    assert_ne!(
        admission_before,
        db_session_admission_revision_connection(&recreated, "ses_fork").unwrap()
    );
    drop(original_file);
}

#[test]
fn native_deleted_v2_without_a_tombstone_exposes_the_retained_v1_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    assert_eq!(
        db_session_source_format(&path, "ses_root"),
        Some(SourceFormat::OpenCodeSqliteV2)
    );
    let v2_fingerprint = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
    connection
        .execute_batch(
            "DELETE FROM session_message WHERE session_id IN ('ses_root', 'ses_child');
         DELETE FROM session_v2 WHERE id IN ('ses_root', 'ses_child');",
        )
        .unwrap();
    assert!(
        db_session_metadata_blocking(&path, "ses_root", SourceFormat::OpenCodeSqliteV2).is_none()
    );
    assert_eq!(
        db_session_source_format(&path, "ses_root"),
        Some(SourceFormat::OpenCodeSqliteV1)
    );
    assert_ne!(
        v2_fingerprint,
        db_session_fingerprint_blocking(&path, "ses_root").unwrap()
    );
    let metadata =
        db_session_metadata_blocking(&path, "ses_root", SourceFormat::OpenCodeSqliteV1).unwrap();
    assert_eq!(metadata.title.as_deref(), Some("V1 copy"));
    assert_eq!(metadata.cwd.as_deref(), Some("/old"));
    let selected: Vec<_> = recent_db_session_logs(directory.path(), 0)
        .into_iter()
        .filter(|log| matches!(&log.source, SessionSource::ProviderDb { session_id, .. } if session_id == "ses_root"))
        .collect();
    assert_eq!(selected.len(), 1);
    assert!(matches!(
        selected[0].source,
        SessionSource::ProviderDb {
            source_format: Some(SourceFormat::OpenCodeSqliteV1),
            ..
        }
    ));
}

#[test]
fn native_v2_archive_and_reparent_use_native_links_without_merging_forks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    let root_before = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
    let admission_before =
        db_session_admission_revision_connection(&connection, "ses_root").unwrap();
    connection
        .execute_batch(
            "UPDATE session_v2 SET time_archived = 1700000100000;
         UPDATE session_v2 SET title = 'Unrelated native title' WHERE id = 'ses_fork';",
        )
        .unwrap();
    let logs = recent_db_session_logs(directory.path(), 0);
    let v2_ids: HashSet<_> = logs
        .iter()
        .filter_map(|log| match &log.source {
            SessionSource::ProviderDb {
                session_id,
                source_format: Some(SourceFormat::OpenCodeSqliteV2),
                ..
            } => Some(session_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        v2_ids,
        HashSet::from(["ses_root", "ses_fork", "ses_archived"])
    );
    assert_eq!(
        db_fork_parent_blocking(&path, "ses_fork").as_deref(),
        Some("ses_root")
    );
    assert_ne!(
        db_session_fingerprint_connection(&connection, "ses_root").unwrap(),
        root_before
    );
    assert_ne!(
        db_session_admission_revision_connection(&connection, "ses_root").unwrap(),
        admission_before
    );

    let archived_root = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
    let archived_fork = db_session_fingerprint_connection(&connection, "ses_fork").unwrap();
    connection
        .execute_batch(
            "UPDATE session_v2 SET parent_id = 'ses_fork', directory = '/moved/exact',
            time_updated = 1700000200000 WHERE id = 'ses_child'",
        )
        .unwrap();
    assert_ne!(
        db_session_fingerprint_connection(&connection, "ses_root").unwrap(),
        archived_root
    );
    assert_ne!(
        db_session_fingerprint_connection(&connection, "ses_fork").unwrap(),
        archived_fork
    );
    let metadata =
        db_session_metadata_blocking(&path, "ses_child", SourceFormat::OpenCodeSqliteV2).unwrap();
    assert_eq!(metadata.cwd.as_deref(), Some("/moved/exact"));
    let logs = recent_db_session_logs(directory.path(), 1_700_000_150);
    assert_eq!(logs.len(), 1);
    assert!(matches!(
        &logs[0].source,
        SessionSource::ProviderDb { session_id, source_format: Some(SourceFormat::OpenCodeSqliteV2), .. }
            if session_id == "ses_fork"
    ));
}

#[test]
fn native_v2_admission_ignores_payloads_and_rejects_oversized_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    let before = db_session_admission_revision_connection(&connection, "ses_root").unwrap();
    connection
        .execute(
            "UPDATE session_message SET data = ?1 WHERE id = 'msg_old'",
            ["x".repeat(8 * 1024 * 1024 + 1)],
        )
        .unwrap();
    assert_eq!(
        db_session_admission_revision_connection(&connection, "ses_root").unwrap(),
        before
    );
    assert!(!recent_db_session_logs(directory.path(), 0).is_empty());
    assert!(db_session_fingerprint_connection(&connection, "ses_root").is_none());
    connection
        .execute(
            "UPDATE session_v2 SET title = ?1 WHERE id = 'ses_root'",
            ["x".repeat(1024 * 1024 + 1)],
        )
        .unwrap();
    assert!(db_session_admission_revision_connection(&connection, "ses_root").is_none());
    assert!(
        db_session_metadata_blocking(&path, "ses_root", SourceFormat::OpenCodeSqliteV2).is_none()
    );
}

// Windows blocks renames while SQLite has the database open.
#[cfg(unix)]
#[test]
fn native_v2_admission_rejects_a_database_replaced_after_sqlite_opened_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    drop(pinned_mixed_db(&path));
    let file = std::fs::File::open(&path).unwrap();
    let original_identity = crate::discovery::SourceStat::from_open_std_file(&file)
        .unwrap()
        .identity
        .unwrap();
    let connection = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();

    std::fs::rename(&path, directory.path().join("replaced.db")).unwrap();
    drop(pinned_mixed_db(&path));

    assert!(
        crate::discovery::agents::opencode::db_session_admission_revision_connection_with_identity(
            &connection,
            "ses_root",
            &original_identity,
        )
        .is_none()
    );
}

#[cfg(unix)]
#[test]
fn native_v2_claim_rejects_a_replacement_that_was_restored_after_sqlite_opened_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    drop(pinned_mixed_db(&path));
    let file = std::fs::File::open(&path).unwrap();
    let original_identity = crate::discovery::SourceStat::from_open_std_file(&file)
        .unwrap()
        .identity
        .unwrap();
    let original = directory.path().join("original.db");
    std::fs::rename(&path, &original).unwrap();
    drop(pinned_mixed_db(&path));
    let connection = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    std::fs::rename(&path, directory.path().join("replacement.db")).unwrap();
    std::fs::rename(&original, &path).unwrap();

    assert!(
        crate::discovery::agents::opencode::db_session_fingerprint_cancellable_with_identity(
            &connection,
            "ses_root",
            &original_identity,
            &|| false,
        )
        .is_none()
    );
}

#[test]
fn native_large_inactive_v2_history_does_not_exhaust_the_metadata_query_budget() {
    let directory = tempfile::tempdir().unwrap();
    let connection = pinned_mixed_db(&directory.path().join("opencode.db"));
    connection
        .execute_batch(
            "WITH RECURSIVE numbers(n) AS (
            VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 400000
         ) INSERT INTO session_message
            SELECT 'msg_bulk_' || n, 'ses_root', 'user', n, 1, 1, '' FROM numbers;",
        )
        .unwrap();
    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 1_800_000_000),
        DbSourceRead::Ready(logs) if logs.is_empty()
    ));
    connection
        .execute(
            "UPDATE session_message SET time_updated = 1800000000000 WHERE id = 'msg_bulk_1'",
            [],
        )
        .unwrap();
    let logs = recent_db_session_logs(directory.path(), 1_800_000_000);
    assert_eq!(logs.len(), 1);
    assert!(matches!(&logs[0].source, SessionSource::ProviderDb {
        session_id, source_format: Some(SourceFormat::OpenCodeSqliteV2), ..
    } if session_id == "ses_root"));
    assert_eq!(logs[0].updated_at, Some(1_800_000_000));
}

fn migration_copies_across_roots() -> (tempfile::TempDir, tempfile::TempDir, Connection) {
    let legacy = tempfile::tempdir().unwrap();
    let current = tempfile::tempdir().unwrap();
    for (root, table, timestamp) in [
        (legacy.path(), "session", 1_800_000_000_000_i64),
        (current.path(), "session_v2", 1_700_000_000_000_i64),
    ] {
        let connection = Connection::open(root.join("opencode.db")).unwrap();
        connection
            .execute_batch(include_str!("fixtures/opencode_pinned_mixed.sql"))
            .unwrap();
        connection.execute(&format!(
            "INSERT INTO {table} (id,project_id,slug,directory,title,version,time_created,time_updated)
             VALUES ('ses_selected','global','selected','/exact','Selected source','2.0.32',?1,?1)"
        ), [timestamp]).unwrap();
    }
    let connection = Connection::open(current.path().join("opencode.db")).unwrap();
    (legacy, current, connection)
}

#[tokio::test]
async fn native_v2_precedence_does_not_depend_on_the_activity_cutoff_or_prior_scans() {
    let (legacy, current, _connection) = migration_copies_across_roots();
    let roots = vec![legacy.path().to_path_buf(), current.path().to_path_buf()];
    for _ in 0..2 {
        let logs = discover_recent_in(&roots, 1_800_000_010, 60).await;
        assert_eq!(logs.len(), 1);
        assert!(matches!(
            &logs[0].source,
            SessionSource::ProviderDb { db_path, session_id, source_format: Some(SourceFormat::OpenCodeSqliteV2), .. }
                if db_path == &current.path().join("opencode.db") && session_id == "ses_selected"
        ));
        assert_eq!(logs[0].updated_at, Some(1_700_000_000));
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    discover_recent_batches_in(&roots, AgentKind::OpenCode, 1_800_000_010, 60, 64, sender).await;
    let mut logs = Vec::new();
    while let Some(batch) = receiver.recv().await {
        logs.extend(batch.candidates);
    }
    assert_eq!(logs.len(), 1);
    assert!(matches!(
        &logs[0].source,
        SessionSource::ProviderDb {
            source_format: Some(SourceFormat::OpenCodeSqliteV2),
            ..
        }
    ));
}

#[tokio::test]
async fn native_v2_precedence_survives_the_recent_session_limit() {
    let (legacy, current, connection) = migration_copies_across_roots();
    connection.execute_batch(
        "UPDATE session_v2 SET time_created = 1800000000000, time_updated = 1800000000000;
         WITH RECURSIVE numbers(n) AS (
            VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 1000
         ) INSERT INTO session_v2 (id,project_id,slug,directory,version,time_created,time_updated)
            SELECT 'ses_newer_' || n,'global','newer','/exact','2.0.32',1800000001000,1800000001000 FROM numbers;",
    ).unwrap();
    let logs = discover_recent_in(
        &[legacy.path().to_path_buf(), current.path().to_path_buf()],
        1_800_000_010,
        60,
    )
    .await;
    let selected: Vec<_> = logs.iter().filter(|log| matches!(&log.source, SessionSource::ProviderDb { session_id, .. } if session_id == "ses_selected")).collect();
    assert_eq!(selected.len(), 1);
    assert!(
        matches!(&selected[0].source, SessionSource::ProviderDb { db_path, source_format: Some(SourceFormat::OpenCodeSqliteV2), .. } if db_path == &current.path().join("opencode.db"))
    );
}

#[tokio::test]
async fn native_promoted_v2_root_keeps_activity_from_its_exact_child_cluster() {
    let (legacy, current, connection) = migration_copies_across_roots();
    connection.execute_batch(
        "INSERT INTO session_v2 (id,project_id,parent_id,slug,directory,version,time_created,time_updated)
         VALUES ('ses_active_child','global','ses_selected','child','/exact','2.0.32',1800000005000,1800000005000);",
    ).unwrap();
    let roots = vec![legacy.path().to_path_buf(), current.path().to_path_buf()];
    let logs = discover_recent_in(&roots, 1_800_000_010, 60).await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].updated_at, Some(1_800_000_005));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    discover_recent_batches_in(&roots, AgentKind::OpenCode, 1_800_000_010, 60, 64, sender).await;
    let mut logs = Vec::new();
    while let Some(batch) = receiver.recv().await {
        logs.extend(batch.candidates);
    }
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].updated_at, Some(1_800_000_005));
}

#[tokio::test]
async fn native_v1_only_discovery_keeps_more_than_4096_sessions_in_both_paths() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(directory.path().join("opencode.db")).unwrap();
    connection
        .execute_batch(include_str!("fixtures/opencode_pinned_mixed.sql"))
        .unwrap();
    connection.execute_batch(
        "WITH RECURSIVE numbers(n) AS (
            VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 4097
         ) INSERT INTO session (id,project_id,slug,directory,title,version,time_created,time_updated)
           SELECT 'ses_legacy_' || n,'global','legacy','/exact','Legacy','1',1800000000000,1800000000000 FROM numbers;",
    ).unwrap();
    let roots = vec![directory.path().to_path_buf()];
    let logs = discover_recent_in(&roots, 1_800_000_010, 60).await;
    assert_eq!(logs.len(), 4097);
    assert!(logs.iter().all(|log| matches!(
        &log.source,
        SessionSource::ProviderDb {
            source_format: Some(SourceFormat::OpenCodeSqliteV1),
            ..
        }
    )));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    discover_recent_batches_in(&roots, AgentKind::OpenCode, 1_800_000_010, 60, 256, sender).await;
    let mut count = 0;
    while let Some(batch) = receiver.recv().await {
        count += batch.candidates.len();
    }
    assert_eq!(count, 4097);
}

#[tokio::test]
#[serial]
async fn native_public_title_apis_bound_values_and_query_only_requested_ids() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection
        .execute(
            "UPDATE session_v2 SET title = ?1 WHERE id = 'ses_root'",
            ["x".repeat(1024 * 1024 + 1)],
        )
        .unwrap();
    connection.execute_batch(
        "WITH RECURSIVE numbers(n) AS (
            VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 20000
         ) INSERT INTO session_v2 (id,project_id,slug,directory,title,version,time_created,time_updated)
           SELECT 'ses_unrelated_' || n,'global','unrelated','/exact','Unrelated','2.0.32',1,1 FROM numbers;
         UPDATE session_v2 SET title = CAST(zeroblob(16777216) AS TEXT) WHERE id = 'ses_unrelated_1';",
    ).unwrap();
    run_with_data_dir(directory.path(), || async {
        assert!(
            OpenCodeExplorer
                .indexed_session_title("ses_root")
                .await
                .is_none()
        );
        let titles = OpenCodeExplorer
            .indexed_session_titles(&["ses_root".into(), "ses_child".into()])
            .await;
        assert_eq!(titles.len(), 1);
        assert_eq!(titles["ses_child"].text, "Child");
    })
    .await;
}

#[test]
fn native_bad_v2_hierarchies_do_not_hide_other_v2_or_v1_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    for parent in [
        "ses_missing".to_owned(),
        "invalid parent".to_owned(),
        "ses_child".to_owned(),
        format!("ses_{}", "x".repeat(257)),
    ] {
        connection
            .execute(
                "UPDATE session_v2 SET parent_id = ?1 WHERE id = 'ses_child'",
                [&parent],
            )
            .unwrap();
        let logs = recent_db_session_logs(directory.path(), 0);
        assert!(logs.iter().any(|log| matches!(
            &log.source, SessionSource::ProviderDb { session_id, source_format: Some(SourceFormat::OpenCodeSqliteV2), .. }
                if session_id == "ses_root"
        )));
        assert!(logs.iter().any(|log| matches!(
            &log.source, SessionSource::ProviderDb { session_id, source_format: Some(SourceFormat::OpenCodeSqliteV1), .. }
                if session_id == "ses_v1_only"
        )));
        assert!(logs.iter().all(|log| !matches!(
            &log.source, SessionSource::ProviderDb { session_id, .. } if session_id == "ses_child"
        )));
    }
}

#[tokio::test]
async fn native_unreadable_stores_preserve_healthy_v1_and_legacy_sources_in_both_paths() {
    for failure in ["corrupt", "busy", "malformed", "directory_limit"] {
        let broken = tempfile::tempdir().unwrap();
        let healthy = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let connection = pinned_mixed_db(&broken.path().join("opencode.db"));
        match failure {
            "corrupt" => {
                drop(connection);
                std::fs::write(broken.path().join("opencode.db"), b"not a database").unwrap();
            }
            "busy" => connection.execute_batch("BEGIN EXCLUSIVE").unwrap(),
            "malformed" => connection
                .execute_batch("DROP INDEX session_message_session_seq_idx")
                .unwrap(),
            "directory_limit" => {
                for index in 0..=MAX_DB_DIRECTORY_ENTRIES {
                    std::fs::write(broken.path().join(format!("unrelated{index}")), []).unwrap();
                }
            }
            _ => unreachable!(),
        }
        seed_session_title_db(
            &healthy.path().join("opencode.db"),
            &[("ses_healthy", "Healthy")],
        );
        Connection::open(healthy.path().join("opencode.db"))
            .unwrap()
            .execute("UPDATE session SET time_updated = 1700000000000", [])
            .unwrap();
        std::fs::write(healthy.path().join("opencode-broken.db"), b"not a database").unwrap();
        write_json(
            &legacy.path().join("storage/session/global/ses_legacy.json"),
            r#"{"id":"ses_legacy","time":{"updated":1700000000000}}"#,
        )
        .await;
        let roots = vec![
            broken.path().to_path_buf(),
            healthy.path().to_path_buf(),
            legacy.path().to_path_buf(),
        ];
        let logs = discover_recent_in(&roots, 1_700_000_010, 60).await;
        assert_eq!(logs.len(), 2, "{failure}: {logs:?}");
        assert!(
            logs.iter()
                .any(|log| log.source_label() == "opencode:ses_healthy")
        );
        assert!(
            logs.iter()
                .any(|log| log.source_label() == "opencode:ses_legacy")
        );
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        discover_recent_batches_in(&roots, AgentKind::OpenCode, 1_700_000_010, 60, 1, sender).await;
        let mut batched = Vec::new();
        while let Some(batch) = receiver.recv().await {
            batched.extend(batch.candidates);
        }
        assert_eq!(format!("{logs:?}"), format!("{batched:?}"), "{failure}");
    }
}

#[tokio::test]
#[serial]
async fn native_direct_and_title_lookups_share_selection_across_failed_channels() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("opencode.db"), b"not a database").unwrap();
    let selected = directory.path().join("opencode-healthy.db");
    drop(pinned_mixed_db(&selected));
    std::fs::write(directory.path().join("opencode-z.db"), b"not a database").unwrap();
    run_with_data_dir(directory.path(), || async {
        assert!(matches!(locate_db_session_source("ses_child".into()).await,
            DirectSessionSource::Found(SessionSource::ProviderDb { db_path, session_id, source_format: Some(SourceFormat::OpenCodeSqliteV2), .. })
            if db_path == selected && session_id == "ses_root"));
        assert_eq!(OpenCodeExplorer.indexed_session_title("ses_child").await.unwrap().text, "Child");
        assert_eq!(OpenCodeExplorer.indexed_session_title("ses_v1_only").await.unwrap().text, "Not migrated");
        let batch = OpenCodeExplorer.indexed_session_titles(&["ses_child".into(), "ses_v1_only".into()]).await;
        assert_eq!(batch["ses_child"].text, "Child");
        assert_eq!(batch["ses_v1_only"].text, "Not migrated");
    }).await;
    for index in 0..=MAX_DB_DIRECTORY_ENTRIES {
        std::fs::write(directory.path().join(format!("unrelated{index}")), []).unwrap();
    }
    run_with_data_dir(directory.path(), || async {
        assert!(matches!(
            locate_db_session_source("ses_child".into()).await,
            DirectSessionSource::Missing
        ));
    })
    .await;
}

#[test]
fn native_large_v1_history_keeps_recent_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection.execute_batch(
        "DROP TABLE session_message; DROP TABLE session_v2;
         UPDATE session SET time_updated = 1800000000000 WHERE id = 'ses_v1_only';
         WITH RECURSIVE numbers(n) AS (
             VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 1050000
         ) INSERT INTO part SELECT 'prt_old_' || n, 'msg_old', 'ses_inactive', 1, 1, '{}' FROM numbers;"
    ).unwrap();
    let logs = recent_db_session_logs(directory.path(), 1_800_000_000);
    assert_eq!(logs.len(), 1);
    assert!(matches!(&logs[0].source, SessionSource::ProviderDb {
        session_id, source_format: Some(SourceFormat::OpenCodeSqliteV1), ..
    } if session_id == "ses_v1_only"));
}

#[test]
fn wsl_v2_success_keeps_v1_only_sessions_and_prefers_the_v2_duplicate() {
    let inline = |label: &str, source_format| SessionLog {
        environment: Default::default(),
        agent_type: AgentKind::OpenCode,
        source: SessionSource::Inline {
            label: label.into(),
            content: String::new(),
            source_format,
        },
        updated_at: Some(1),
    };
    let legacy = vec![
        inline("opencode:ses_migrated", None),
        inline("opencode:ses_v1_only", None),
    ];
    let mut logs = vec![inline(
        "opencode-v2-export:ses_migrated",
        Some(SourceFormat::OpenCodeV2SessionExport),
    )];
    merge_wsl_session_sources(&mut logs, legacy.clone());
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].source_label(), "opencode-v2-export:ses_migrated");
    assert_eq!(logs[1].source_label(), "opencode:ses_v1_only");
    let mut empty = Vec::new();
    merge_wsl_session_sources(&mut empty, legacy);
    assert_eq!(empty.len(), 2);
}

#[test]
fn native_fingerprints_use_one_wal_snapshot_for_v1_and_v2() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    for v2 in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = pinned_mixed_db(&path);
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        if !v2 {
            connection
                .execute_batch("DROP TABLE session_message; DROP TABLE session_v2;")
                .unwrap();
        }
        let table = if v2 { "session_v2" } else { "session" };
        let messages = if v2 { "session_message" } else { "message" };
        let state_sql = |state: &str| {
            format!(
                "BEGIN; UPDATE {table} SET title = '{state}'; UPDATE {messages} SET data = '{state}'; {} COMMIT;",
                if v2 {
                    String::new()
                } else {
                    format!("UPDATE part SET data = '{state}';")
                }
            )
        };
        let a = state_sql(&"a".repeat(64 * 1024));
        let b = state_sql(&"b".repeat(64 * 1024));
        connection.execute_batch(&a).unwrap();
        let fingerprint_a = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
        connection.execute_batch(&b).unwrap();
        let fingerprint_b = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
        assert_ne!(fingerprint_a, fingerprint_b);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let writer = std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                connection.execute_batch(&a).unwrap();
                connection.execute_batch(&b).unwrap();
            }
        });
        let mut fingerprints = Vec::new();
        for _ in 0..32 {
            fingerprints.push(db_session_fingerprint_blocking(&path, "ses_root"));
        }
        stop.store(true, Ordering::Release);
        writer.join().unwrap();
        assert!(
            fingerprints
                .into_iter()
                .all(|value| value == Some(fingerprint_a) || value == Some(fingerprint_b)),
            "v2: {v2}"
        );
    }
}

#[tokio::test]
#[serial]
async fn native_child_title_uses_its_row_source_after_a_cross_database_root_redirect() {
    let directory = tempfile::tempdir().unwrap();
    let primary_path = directory.path().join("opencode.db");
    let primary = pinned_mixed_db(&primary_path);
    primary.execute_batch(
        "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_parent', 'global', 'parent', '/repo', 'Preferred parent', '2', 1700000000000, 1700000000000);"
    ).unwrap();
    let channel = pinned_mixed_db(&directory.path().join("opencode-beta.db"));
    channel.execute_batch(
        "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_parent', 'global', NULL, 'parent', '/repo', 'Other parent', '2', 1800000000000, 1800000000000),
                ('ses_title_child', 'global', 'ses_parent', 'child', '/repo', 'Child', '2', 1800000000000, 1800000000000);"
    ).unwrap();
    for retained_v1 in [false, true] {
        if retained_v1 {
            primary.execute_batch(
                "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
                 VALUES ('ses_title_child', 'global', 'child', '/repo', 'Retained V1 child', '1', 1700000000000, 1700000000000);"
            ).unwrap();
        }
        run_with_data_dir(directory.path(), || async {
            assert!(
                matches!(locate_db_session_source("ses_title_child".into()).await,
                DirectSessionSource::Found(SessionSource::ProviderDb { db_path, session_id, .. })
                if db_path == primary_path && session_id == "ses_parent")
            );
            assert_eq!(
                OpenCodeExplorer
                    .indexed_session_title("ses_title_child")
                    .await
                    .unwrap()
                    .text,
                "Child"
            );
            let titles = OpenCodeExplorer
                .indexed_session_titles(&["ses_title_child".into(), "ses_parent".into()])
                .await;
            assert_eq!(titles["ses_title_child"].text, "Child");
            assert_eq!(titles["ses_parent"].text, "Preferred parent");
        })
        .await;
    }
}

#[tokio::test]
#[serial]
async fn native_v1_precedence_uses_canonical_activity_across_cutoffs_and_both_discovery_paths() {
    let directory = tempfile::tempdir().unwrap();
    let primary_path = directory.path().join("opencode.db");
    let primary = pinned_mixed_db(&primary_path);
    primary.execute_batch(
        "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_v1_selected', 'global', 'selected', '/repo', 'Old canonical copy', '1', 1700000000000, 1700000000000);"
    ).unwrap();
    let channel = pinned_mixed_db(&directory.path().join("opencode-beta.db"));
    channel.execute_batch(
        "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_v1_selected', 'global', 'selected', '/repo', 'Recent second copy', '1', 1800000000000, 1800000000000);"
    ).unwrap();
    run_with_data_dir(directory.path(), || async {
        for expected_activity in [1_700_000_000, 1_700_000_000, 1_800_000_005] {
            if expected_activity == 1_800_000_005 {
                primary.execute_batch("INSERT INTO message VALUES ('msg_v1_activity', 'ses_v1_selected', 1800000005000, 1800000005000, '{}');").unwrap();
                channel.execute_batch("UPDATE session SET time_updated = 1800000009000 WHERE id = 'ses_v1_selected';").unwrap();
            }
            assert!(matches!(locate_db_session_source("ses_v1_selected".into()).await,
                DirectSessionSource::Found(SessionSource::ProviderDb { db_path, source_format: Some(SourceFormat::OpenCodeSqliteV1), .. })
                if db_path == primary_path));
            assert_eq!(OpenCodeExplorer.indexed_session_title("ses_v1_selected").await.unwrap().text, "Old canonical copy");
            let roots = [directory.path().to_path_buf()];
            let logs = discover_recent_in(&roots, 1_800_000_010, 60).await;
            let selected: Vec<_> = logs.iter().filter(|log| log.source_label() == "opencode:ses_v1_selected").collect();
            assert_eq!(selected.len(), 1);
            assert!(matches!(&selected[0].source, SessionSource::ProviderDb { db_path, .. } if db_path == &primary_path));
            assert_eq!(selected[0].updated_at, Some(expected_activity));
            let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
            discover_recent_batches_in(&roots, AgentKind::OpenCode, 1_800_000_010, 60, 64, sender).await;
            let mut batched = Vec::new();
            while let Some(batch) = receiver.recv().await { batched.extend(batch.candidates); }
            let selected: Vec<_> = batched.iter().filter(|log| log.source_label() == "opencode:ses_v1_selected").collect();
            assert_eq!(selected.len(), 1);
            assert!(matches!(&selected[0].source, SessionSource::ProviderDb { db_path, .. } if db_path == &primary_path));
            assert_eq!(selected[0].updated_at, Some(expected_activity));
        }
    }).await;
}

#[tokio::test]
async fn native_database_batches_arrive_before_pending_fragment_reads() {
    let database = tempfile::tempdir().unwrap();
    let legacy = tempfile::tempdir().unwrap();
    seed_session_title_db(
        &database.path().join("opencode.db"),
        &[("ses_early", "Early")],
    );
    Connection::open(database.path().join("opencode.db"))
        .unwrap()
        .execute("UPDATE session SET time_updated = 1800000000000", [])
        .unwrap();
    let roots = vec![database.path().to_path_buf(), legacy.path().to_path_buf()];
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (started, fragment_started) = tokio::sync::oneshot::channel();
    let (release, fragment_release) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        discover_recent_batches_with_fragments(
            &roots,
            AgentKind::OpenCode,
            1_800_000_010,
            60,
            64,
            sender,
            |_| async move {
                started.send(()).unwrap();
                fragment_release.await.unwrap();
                Vec::new()
            },
        )
        .await;
    });
    let first = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.candidates.len(), 1);
    assert_eq!(first.candidates[0].source_label(), "opencode:ses_early");
    assert!(!first.complete);
    fragment_started.await.unwrap();
    release.send(()).unwrap();
    task.await.unwrap();
    assert!(receiver.recv().await.unwrap().complete);
}

#[tokio::test]
async fn native_batch_receiver_drop_cancels_pending_fragment_reads() {
    let directory = tempfile::tempdir().unwrap();
    let roots = vec![directory.path().to_path_buf()];
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (started, fragment_started) = tokio::sync::oneshot::channel();
    let (release, fragment_release) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        discover_recent_batches_with_fragments(
            &roots,
            AgentKind::OpenCode,
            1_800_000_010,
            60,
            64,
            sender,
            |_| async move {
                started.send(()).unwrap();
                fragment_release.await.unwrap();
                Vec::new()
            },
        )
        .await;
    });
    assert!(!receiver.recv().await.unwrap().complete);
    fragment_started.await.unwrap();
    drop(receiver);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(release.send(()).is_err());
}
