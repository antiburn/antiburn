use super::*;
use antiburn_local::analysis::SourceFormat;
#[cfg(test)]
use antiburn_local::discovery::agents::opencode::db_session_fingerprint_snapshot;
use antiburn_local::discovery::agents::opencode::{
    db_session_admission_revision_connection_with_identity,
    db_session_fingerprint_snapshot_with_identity, db_session_source_format_connection,
};
use antiburn_local::discovery::source_version::SourceStat;

// A permit stays with the blocking work even if its async owner stops.
static HASH_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

struct HashCancellation(Arc<AtomicBool>);

impl Drop for HashCancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) fn uses_bounded_admission(log: &SessionLog) -> bool {
    admission_format(log).is_some()
}

fn admission_format(log: &SessionLog) -> Option<SourceFormat> {
    if !log.environment.is_native()
        || log.agent_type != AgentKind::OpenCode
        || !matches!(log.source, SessionSource::ProviderDb { .. })
    {
        return None;
    }
    let format = crate::analysis::source_format(log.agent_type, &log.source);
    matches!(
        format,
        SourceFormat::OpenCodeSqliteV1 | SourceFormat::OpenCodeSqliteV2
    )
    .then_some(format)
}

fn session_query(log: &SessionLog) -> Option<&'static str> {
    match admission_format(log)? {
        SourceFormat::OpenCodeSqliteV1 => Some("SELECT 1 FROM session WHERE id = ?1"),
        SourceFormat::OpenCodeSqliteV2 => Some("SELECT 1 FROM session_v2 WHERE id = ?1"),
        _ => None,
    }
}

fn revision_value(
    path: &std::path::Path,
    session: &str,
    identity: Option<String>,
    scope: String,
) -> String {
    format!(
        "opencode-admission-v2:{}",
        serde_json::json!([path, session, identity, scope])
    )
}

pub(super) async fn admission_revision(log: &SessionLog) -> Option<String> {
    let expected_format = admission_format(log)?;
    let SessionSource::ProviderDb {
        db_path,
        session_id,
        ..
    } = &log.source
    else {
        return None;
    };
    let path = db_path.clone();
    let session = session_id.clone();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path).ok()?;
        let identity = SourceStat::from_open_std_file(&file)?.identity?;
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        let snapshot = connection.unchecked_transaction().ok()?;
        if db_session_source_format_connection(&snapshot, &session)? != expected_format {
            return None;
        }
        let scope =
            db_session_admission_revision_connection_with_identity(&snapshot, &session, &identity)?;
        Some(revision_value(&path, &session, Some(identity), scope))
    })
    .await
    .ok()?
}

pub(super) async fn strict_fingerprint(log: &SessionLog, revision: &str) -> Option<String> {
    if admission_revision(log).await.as_deref() != Some(revision) {
        return None;
    }
    let SessionSource::ProviderDb {
        db_path,
        session_id,
        ..
    } = &log.source
    else {
        return None;
    };
    let path = db_path.clone();
    let session = session_id.clone();
    let permit = HASH_SLOTS.acquire().await.ok()?;
    let cancellation = HashCancellation(Arc::new(AtomicBool::new(false)));
    let cancelled = cancellation.0.clone();
    let started = Instant::now();
    ::tracing::debug!(
        event = "scan_strict_fingerprint_started",
        agent = log.agent_type.slug()
    );
    let snapshot = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let file = std::fs::File::open(&path).ok()?;
        let identity = SourceStat::from_open_std_file(&file)?.identity?;
        let (scope, (hash, rows)) =
            db_session_fingerprint_snapshot_with_identity(&path, &session, &identity, &|| {
                cancelled.load(Ordering::Acquire)
            })?;
        Some((
            revision_value(&path, &session, Some(identity), scope),
            format!("sv1:db:{hash}:{rows}"),
        ))
    })
    .await
    .ok()
    .flatten();
    let unchanged = admission_revision(log).await.as_deref() == Some(revision);
    ::tracing::debug!(
        event = "scan_strict_fingerprint_finished",
        agent = log.agent_type.slug(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        unchanged,
        available = snapshot.is_some()
    );
    let (snapshot_revision, fingerprint) = snapshot?;
    (unchanged && snapshot_revision == revision).then_some(fingerprint)
}

pub(super) async fn source_is_deleted(log: &SessionLog) -> bool {
    let Some(query) = session_query(log) else {
        return false;
    };
    let SessionSource::ProviderDb {
        db_path,
        session_id,
        ..
    } = &log.source
    else {
        return false;
    };
    let path = db_path.clone();
    let session = session_id.clone();
    tokio::task::spawn_blocking(move || {
        if std::fs::File::open(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return true;
        }
        let Ok(connection) = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return false;
        };
        matches!(
            connection.query_row(query, [session], |_| Ok(())),
            Err(rusqlite::Error::QueryReturnedNoRows)
        )
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
#[path = "fingerprint_v2_tests.rs"]
mod v2_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn wal_source(path: &std::path::Path) -> (rusqlite::Connection, SessionLog) {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE INDEX session_parent ON session(parent_id);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE INDEX message_session ON message(session_id);
             CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE INDEX part_session ON part(session_id);
             INSERT INTO session VALUES ('session', NULL, 1000, 1000), ('other', NULL, 1000, 1000);
             INSERT INTO message VALUES ('message', 'session', 1000, 1000, 'before');"
        ).unwrap();
        let log = SessionLog {
            agent_type: AgentKind::OpenCode,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: path.to_path_buf(),
                session_id: "session".into(),
                source_format: Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1),
            },
            updated_at: Some(1),
            environment: DiscoveryEnvironment::Native,
        };
        (connection, log)
    }

    #[test]
    fn native_opencode_database_versions_use_bounded_admission() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let (_writer, v1) = wal_source(&path);
        assert!(uses_bounded_admission(&v1));

        let mut v2 = v1;
        let SessionSource::ProviderDb { source_format, .. } = &mut v2.source else {
            panic!("expected provider database source");
        };
        *source_format = Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2);
        assert!(uses_bounded_admission(&v2));
        v2.environment = DiscoveryEnvironment::Wsl {
            distribution: "test".into(),
            user: "test-user".into(),
        };
        assert!(!uses_bounded_admission(&v2));
    }

    #[tokio::test]
    async fn unrelated_wal_writes_and_checkpoints_do_not_block_scoped_snapshot_completion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let (writer, log) = wal_source(&path);
        let revision = admission_revision(&log).await.unwrap();
        let expected = strict_fingerprint(&log, &revision).await.unwrap();
        let writes = std::cell::Cell::new(0);
        let (snapshot_revision, (hash, rows)) =
            db_session_fingerprint_snapshot(&path, "session", &|| {
                writer
                    .execute(
                        "UPDATE session SET time_updated = time_updated + 1 WHERE id = 'other'",
                        [],
                    )
                    .unwrap();
                writer
                    .execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
                    .unwrap();
                writes.set(writes.get() + 1);
                false
            })
            .unwrap();
        assert!(
            writes.get() > 1,
            "the unrelated writer stays active during hashing"
        );
        assert_eq!(expected, format!("sv1:db:{hash}:{rows}"));
        assert!(revision.ends_with(&format!("\"{snapshot_revision}\"]")));
        assert_eq!(
            admission_revision(&log).await.as_deref(),
            Some(revision.as_str())
        );
        writer
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        assert_eq!(strict_fingerprint(&log, &revision).await.unwrap(), expected);
    }

    #[tokio::test]
    async fn current_session_changes_do_not_mix_tables_from_different_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let (writer, log) = wal_source(&path);
        let revision = admission_revision(&log).await.unwrap();
        let expected = strict_fingerprint(&log, &revision).await.unwrap();
        let changed = std::cell::Cell::new(false);
        let (_, (hash, rows)) = db_session_fingerprint_snapshot(&path, "session", &|| {
            if !changed.replace(true) {
                writer
                    .execute_batch(
                        "BEGIN; UPDATE session SET time_updated = 2000 WHERE id = 'session';
                     UPDATE message SET data = 'replacement' WHERE session_id = 'session'; COMMIT;",
                    )
                    .unwrap();
            }
            false
        })
        .unwrap();
        assert_eq!(expected, format!("sv1:db:{hash}:{rows}"));
        assert_ne!(admission_revision(&log).await.unwrap(), revision);
        assert!(strict_fingerprint(&log, &revision).await.is_none());
        let current = admission_revision(&log).await.unwrap();
        assert_ne!(strict_fingerprint(&log, &current).await.unwrap(), expected);
    }

    #[test]
    fn strict_hash_observes_cancellation_inside_large_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let (writer, _) = wal_source(&path);
        writer
            .execute("UPDATE message SET data = ?1", ["x".repeat(1024 * 1024)])
            .unwrap();
        let polls = std::cell::Cell::new(0);
        assert!(
            db_session_fingerprint_snapshot(&path, "session", &|| {
                polls.set(polls.get() + 1);
                polls.get() > 12
            })
            .is_none()
        );
    }

    #[tokio::test]
    async fn large_clusters_keep_root_admission_and_full_strict_reconciliation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let (writer, log) = wal_source(&path);
        writer.execute_batch("BEGIN").unwrap();
        for index in 0..260 {
            writer
                .execute(
                    "INSERT INTO session VALUES (?1, 'session', 1000, 1000)",
                    [format!("child-{index:03}")],
                )
                .unwrap();
        }
        writer.execute_batch("COMMIT").unwrap();
        let before = admission_revision(&log).await.unwrap();
        let strict_before = strict_fingerprint(&log, &before).await.unwrap();
        writer
            .execute(
                "UPDATE session SET time_updated = 2000 WHERE id = 'session'",
                [],
            )
            .unwrap();
        assert_ne!(admission_revision(&log).await.unwrap(), before);
        writer
            .execute(
                "UPDATE session SET time_updated = 1000 WHERE id = 'session'",
                [],
            )
            .unwrap();
        writer.execute("INSERT INTO message VALUES ('outside-frontier', 'child-259', 1000, 1000, 'new payload')", []).unwrap();
        assert_eq!(admission_revision(&log).await.unwrap(), before);
        assert_ne!(
            strict_fingerprint(&log, &before).await.unwrap(),
            strict_before
        );
    }

    #[tokio::test]
    async fn same_time_and_count_payload_edits_keep_admission_but_change_strict_fingerprints() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE INDEX message_session ON message(session_id);
             CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE INDEX part_session ON part(session_id);
             INSERT INTO session VALUES ('session', NULL, 1000, 1000);
             INSERT INTO message VALUES ('message', 'session', 1000, 1000, '{\"text\":\"before\"}');"
        ).unwrap();
        let log = SessionLog {
            agent_type: AgentKind::OpenCode,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: path,
                session_id: "session".into(),
                source_format: Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1),
            },
            updated_at: Some(1),
            environment: DiscoveryEnvironment::Native,
        };
        let before = admission_revision(&log).await.unwrap();
        let strict_before = strict_fingerprint(&log, &before).await.unwrap();
        connection
            .execute("UPDATE message SET data = '{\"text\":\"edited\"}'", [])
            .unwrap();
        let after = admission_revision(&log).await.unwrap();
        assert_eq!(before, after);
        let strict_after = strict_fingerprint(&log, &after).await.unwrap();
        assert_ne!(strict_before, strict_after);
        assert!(strict_after.starts_with("sv1:db:"));
    }
}
