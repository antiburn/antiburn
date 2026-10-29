#[path = "../src/analysis/opencode_v2_schema.rs"]
mod opencode_v2_schema;
#[path = "../src/analysis/vendors/opencode_v2_snapshot.rs"]
mod opencode_v2_snapshot;

mod analysis {
    pub(crate) use crate::opencode_v2_schema;
    pub(crate) use antiburn_local::analysis::opencode_v2_capture;
    pub use antiburn_local::analysis::*;
}

use antiburn_local::analysis::{SourceFormat, opencode_v2_capture};
use opencode_v2_snapshot::{SnapshotMessage, SnapshotReadSummary};
use rusqlite::{Connection, params};
use tempfile::tempdir;

fn visit_snapshot(
    path: &std::path::Path,
    source_format: SourceFormat,
    session_id: &str,
    cancel: &dyn Fn() -> bool,
    visitor: &mut dyn FnMut(SnapshotMessage) -> anyhow::Result<()>,
) -> anyhow::Result<SnapshotReadSummary> {
    if source_format != SourceFormat::OpenCodeSqliteV2 {
        anyhow::bail!("rejects source format");
    }
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection.execute_batch("BEGIN")?;
    assert!(
        opencode_v2_snapshot::session_cluster_ids(&connection, session_id)?
            .contains(&session_id.to_owned())
    );
    let result = opencode_v2_snapshot::visit_snapshot_connection(
        &connection,
        session_id,
        cancel,
        visitor,
        opencode_v2_snapshot::MAX_SNAPSHOT_ROWS,
        opencode_v2_snapshot::MAX_SNAPSHOT_TOTAL_BYTES,
    )?;
    connection.execute_batch("COMMIT")?;
    Ok(result)
}

fn snapshot() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY, parent_id TEXT, fork_session_id TEXT,
                 directory TEXT NOT NULL DEFAULT '/repo',
                 title TEXT, time_created INTEGER NOT NULL DEFAULT 0,
                 time_updated INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE session_message (
                  id TEXT PRIMARY KEY,
                  session_id TEXT NOT NULL,
                  type TEXT NOT NULL,
                  seq INTEGER NOT NULL,
                  time_created INTEGER NOT NULL DEFAULT 0,
                  time_updated INTEGER NOT NULL DEFAULT 0,
                  data TEXT NOT NULL
              );
              CREATE UNIQUE INDEX session_message_session_seq_idx
                  ON session_message(session_id, seq);
              CREATE TABLE event_sequence (
                  aggregate_id TEXT PRIMARY KEY NOT NULL, seq INTEGER NOT NULL
              );
              INSERT INTO session_v2 (id) VALUES ('ses_fixture');",
        )
        .unwrap();
    (directory, path)
}

fn insert(path: &std::path::Path, id: &str, message_type: &str, seq: i64, data: &str) {
    Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO session_message (id, session_id, type, seq, data)
             VALUES (?1, 'ses_fixture', ?2, ?3, ?4)",
            params![id, message_type, seq, data],
        )
        .unwrap();
}

fn collect(path: &std::path::Path) -> (SnapshotReadSummary, Vec<SnapshotMessage>) {
    let mut messages = Vec::new();
    let summary = visit_snapshot(
        path,
        SourceFormat::OpenCodeSqliteV2,
        "ses_fixture",
        &|| false,
        &mut |message| {
            messages.push(message);
            Ok(())
        },
    )
    .unwrap();
    (summary, messages)
}

#[test]
fn reconstructs_row_identity_and_type_and_orders_by_sequence() {
    let (_directory, path) = snapshot();
    insert(&path, "msg_later", "assistant", 9, r#"{"text":"answer"}"#);
    insert(&path, "msg_first", "user", 2, r#"{"text":"request"}"#);

    let (summary, messages) = collect(&path);

    assert_eq!(
        messages
            .iter()
            .map(|message| message.seq)
            .collect::<Vec<_>>(),
        [2, 9]
    );
    assert_eq!(messages[0].value["id"], "msg_first");
    assert_eq!(messages[0].value["type"], "user");
    assert_eq!(messages[0].value["text"], "request");
    assert_eq!(messages[1].value["id"], "msg_later");
    assert_eq!(messages[1].value["type"], "assistant");
    assert_eq!(
        messages[0].session.source_format(),
        SourceFormat::OpenCodeSqliteV2
    );
    assert!(!summary.capacity_limit_reached);
    assert!(!summary.is_partial());
}

#[test]
fn skips_malformed_message_json_and_keeps_the_valid_suffix() {
    let (_directory, path) = snapshot();
    insert(&path, "msg_bad", "user", 0, "{");
    insert(&path, "msg_after", "user", 1, r#"{"text":"after"}"#);

    let mut messages_visited = 0;
    let summary = visit_snapshot(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        "ses_fixture",
        &|| false,
        &mut |_| {
            messages_visited += 1;
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(summary.malformed_rows, 1);
    assert_eq!(messages_visited, 1);
    assert!(summary.is_partial());
}

#[test]
fn skips_non_object_data_and_retains_unknown_type_for_the_adapter() {
    let (_directory, path) = snapshot();
    insert(&path, "msg_array", "user", 0, "[]");
    assert_eq!(
        visit_snapshot(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            "ses_fixture",
            &|| false,
            &mut |_| Ok(()),
        )
        .unwrap()
        .malformed_rows,
        1
    );

    Connection::open(&path)
        .unwrap()
        .execute("DELETE FROM session_message", [])
        .unwrap();
    insert(&path, "msg_unknown", "future-message", 0, "{}");
    let mut messages = Vec::new();
    let summary = visit_snapshot(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        "ses_fixture",
        &|| false,
        &mut |message| {
            messages.push(message);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(summary.rows_seen, 1);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].value["type"], "future-message");
}

#[test]
fn live_scope_baseline_rechecks_native_cwd_and_lineage() {
    let (_directory, path) = snapshot();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("INSERT INTO event_sequence (aggregate_id, seq) VALUES ('ses_fixture', 11);")
        .unwrap();
    let scopes = vec![("ses_fixture".to_owned(), "/repo".to_owned())];
    let read = || {
        opencode_v2_snapshot::read_capture_baseline_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &scopes,
            &|| false,
        )
    };
    assert_eq!(read().unwrap().get("ses_fixture"), Some(&Some(11)));
    connection
        .execute("UPDATE session_v2 SET directory = '/repo/moved'", [])
        .unwrap();
    assert!(read().is_err());
    connection
        .execute(
            "UPDATE session_v2 SET directory = '/repo', parent_id = 'ses_parent'",
            [],
        )
        .unwrap();
    assert!(read().is_err());
    connection
        .execute(
            "UPDATE session_v2 SET parent_id = NULL, fork_session_id = 'ses_fork'",
            [],
        )
        .unwrap();
    assert!(read().is_err());
    connection.execute("DELETE FROM session_v2", []).unwrap();
    assert!(read().is_err());
}

#[test]
fn live_scope_cwd_and_watermark_share_the_same_wal_snapshot() {
    use std::cell::Cell;
    let (_directory, path) = snapshot();
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
         INSERT INTO event_sequence (aggregate_id, seq) VALUES ('ses_fixture', 7);",
        )
        .unwrap();
    let checks = Cell::new(0);
    let cancel = || {
        let check = checks.get();
        checks.set(check + 1);
        if check == 2 {
            writer
                .execute_batch(
                    "BEGIN IMMEDIATE;
                 UPDATE session_v2 SET directory = '/moved';
                 UPDATE event_sequence SET seq = 8;
                 COMMIT;",
                )
                .unwrap();
        }
        false
    };
    let baseline = opencode_v2_snapshot::read_capture_baseline_for_scope(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        &[("ses_fixture".into(), "/repo".into())],
        &cancel,
    )
    .unwrap();
    assert_eq!(baseline.get("ses_fixture"), Some(&Some(7)));
    assert!(checks.get() > 2);
}

#[test]
fn rejects_other_source_formats() {
    let (_directory, path) = snapshot();

    let error = opencode_v2_snapshot::read_capture_baseline_for_scope(
        &path,
        SourceFormat::OpenCodeSqliteV1,
        &[("ses_fixture".into(), "/repo".into())],
        &|| false,
    )
    .unwrap_err();

    assert_eq!(
        error.downcast_ref::<opencode_v2_schema::OpenCodeV2SnapshotError>(),
        Some(&opencode_v2_schema::OpenCodeV2SnapshotError::UnsupportedSource)
    );
}

#[test]
fn reads_committed_wal_rows_from_a_read_only_snapshot() {
    let (_directory, path) = snapshot();
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE;")
        .unwrap();
    writer
        .execute(
            "INSERT INTO session_message (id, session_id, type, seq, data)
             VALUES ('msg_wal', 'ses_fixture', 'user', 0, '{\"text\":\"wal\"}')",
            [],
        )
        .unwrap();
    writer.execute_batch("COMMIT;").unwrap();

    let (summary, messages) = collect(&path);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].value["text"], "wal");
    assert!(!summary.capacity_limit_reached);
}

#[test]
fn capture_baseline_reads_wal_watermarks_from_one_snapshot() {
    use std::cell::{Cell, RefCell};

    let (_directory, path) = snapshot();
    let writer = Connection::open(&path).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer
        .execute_batch("INSERT INTO event_sequence (aggregate_id, seq) VALUES ('ses_fixture', 7);")
        .unwrap();

    let mut epoch =
        opencode_v2_capture::OpenCodeV2CaptureEpoch::new(vec!["ses_fixture".into()]).unwrap();
    epoch.confirm_subscription().unwrap();
    assert_eq!(epoch.observe(durable_event("before", 4)).unwrap(), None);
    let writer = RefCell::new(writer);
    let epoch = RefCell::new(&mut epoch);
    let cancellation_checks = Cell::new(0);
    let cancel = || {
        let check = cancellation_checks.get();
        cancellation_checks.set(check + 1);
        if check == 1 {
            writer
                .borrow()
                .execute(
                    "UPDATE event_sequence SET seq = 8 WHERE aggregate_id = 'ses_fixture'",
                    [],
                )
                .unwrap();
            epoch
                .borrow_mut()
                .observe(durable_event("during_read", 8))
                .unwrap();
        }
        false
    };
    let baseline = opencode_v2_snapshot::read_capture_baseline_for_scope(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        &[("ses_fixture".to_owned(), "/repo".to_owned())],
        &cancel,
    )
    .unwrap();
    assert_eq!(baseline.get("ses_fixture"), Some(&Some(7)));
    assert_eq!(
        epoch
            .borrow_mut()
            .observe(durable_event("during", 9))
            .unwrap(),
        None
    );
    let buffered = epoch.borrow_mut().finish_hydration(baseline).unwrap();
    let after = epoch
        .borrow_mut()
        .observe(durable_event("after", 10))
        .unwrap()
        .unwrap();

    assert_eq!(buffered.len(), 3);
    assert_eq!(buffered[0].kind, opencode_v2_capture::BoundaryKind::Seed);
    assert_eq!(buffered[1].kind, opencode_v2_capture::BoundaryKind::Live);
    assert_eq!(buffered[2].kind, opencode_v2_capture::BoundaryKind::Live);
    assert_eq!(after.kind, opencode_v2_capture::BoundaryKind::Live);
}

#[test]
fn capture_baseline_checks_cancellation_for_an_empty_scope() {
    let (_directory, path) = snapshot();

    let error = opencode_v2_snapshot::read_capture_baseline_for_scope(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        &[],
        &|| true,
    )
    .unwrap_err();

    assert!(error.to_string().contains("cancelled"));
}

fn durable_event(id: &str, sequence: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "type": "session.tool.success",
        "durable": {"aggregateID":"ses_fixture", "seq":sequence, "version":1},
        "data": {"sessionID":"ses_fixture"}
    })
}

#[test]
fn oversized_rows_are_skipped_and_report_partial_results() {
    let (_directory, path) = snapshot();
    let oversized = format!("{{\"text\":\"{}\"}}", "x".repeat(8 * 1024 * 1024));
    insert(&path, "msg_large", "user", 0, &oversized);
    insert(&path, "msg_after", "assistant", 1, r#"{"text":"after"}"#);

    let (summary, messages) = collect(&path);

    assert_eq!(summary.rows_seen, 2);
    assert_eq!(messages.len(), 1);
    assert_eq!(summary.oversized_rows, 1);
    assert!(!summary.capacity_limit_reached);
    assert!(summary.is_partial());
    assert_eq!(messages[0].value["id"], "msg_after");
}

#[test]
fn visitor_streams_all_rows_before_claiming_eof() {
    let (_directory, path) = snapshot();
    let connection = Connection::open(&path).unwrap();
    for seq in 0..257 {
        connection
            .execute(
                "INSERT INTO session_message (id, session_id, type, seq, data)
                 VALUES (?1, 'ses_fixture', 'user', ?2, '{}')",
                params![format!("msg_{seq}"), seq],
            )
            .unwrap();
    }
    drop(connection);

    let (summary, messages) = collect(&path);

    assert_eq!(summary.rows_seen, 257);
    assert_eq!(messages.len(), 257);
    assert!(!summary.capacity_limit_reached);
    assert_eq!(messages.last().unwrap().seq, 256);
}

#[test]
fn row_cap_marks_the_snapshot_partial_without_claiming_eof() {
    let (_directory, path) = snapshot();
    let mut connection = Connection::open(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO session_message (id, session_id, type, seq, data)
                 VALUES (?1, 'ses_fixture', 'user', ?2, '{}')",
            )
            .unwrap();
        for seq in 0..100_001_i64 {
            statement
                .execute(params![format!("msg_{seq}"), seq])
                .unwrap();
        }
    }
    transaction.commit().unwrap();

    let mut messages_visited = 0;
    let summary = visit_snapshot(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        "ses_fixture",
        &|| false,
        &mut |_| {
            messages_visited += 1;
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(summary.rows_seen, 100_000);
    assert_eq!(messages_visited, 100_000);
    assert!(summary.capacity_limit_reached);
    assert!(summary.is_partial());
}

#[test]
fn cancellation_stops_without_reporting_eof() {
    let (_directory, path) = snapshot();
    insert(&path, "msg_one", "user", 0, "{}");
    let error = visit_snapshot(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        "ses_fixture",
        &|| true,
        &mut |_| Ok(()),
    )
    .unwrap_err();

    assert!(error.to_string().contains("cancelled"));
}
