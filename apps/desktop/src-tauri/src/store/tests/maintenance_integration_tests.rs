use std::{sync::mpsc, thread};

use super::*;

fn store(directory: &std::path::Path) -> Store {
    let store = Store::open(directory).unwrap();
    store
        .lock()
        .execute_batch(
            "PRAGMA wal_autocheckpoint = 0;
             CREATE TABLE synthetic_filler(payload BLOB NOT NULL);
             INSERT INTO synthetic_filler VALUES (zeroblob(4096));",
        )
        .unwrap();
    store
}

fn page_count(connection: &Connection, pragma: &str) -> u64 {
    connection
        .pragma_query_value(None, pragma, |row| row.get(0))
        .unwrap()
}

#[test]
fn the_writer_caps_the_wal_size() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let limit: i64 = store
        .lock()
        .pragma_query_value(None, "journal_size_limit", |row| row.get(0))
        .unwrap();
    assert_eq!(limit, WAL_SIZE_LIMIT_BYTES);
}

#[test]
fn the_checkpoint_completes_while_the_store_mutex_is_held() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let connection = store.lock();
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let store = &store;
        scope.spawn(move || sender.send(store.checkpoint_wal()).unwrap());
        let result = receiver.recv_timeout(Duration::from_secs(5));
        drop(connection);
        let checkpoint = result.unwrap().unwrap();
        assert!(!checkpoint.busy);
        assert!(checkpoint.log_frames > 0);
        assert_eq!(checkpoint.checkpointed_frames, checkpoint.log_frames);
    });
}

#[test]
fn a_pinned_reader_leaves_new_frames_for_a_later_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let reader = store.open_reader(Duration::ZERO).unwrap();
    let mut reader_connection = reader.lock();
    let snapshot = reader_connection.transaction().unwrap();
    let count = "SELECT COUNT(*) FROM synthetic_filler";
    assert_eq!(
        snapshot
            .query_row(count, [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    store
        .lock()
        .execute("INSERT INTO synthetic_filler VALUES (zeroblob(4096))", [])
        .unwrap();

    let partial = store.checkpoint_wal().unwrap();
    assert!(!partial.busy);
    assert!(partial.checkpointed_frames >= 0);
    assert!(partial.checkpointed_frames < partial.log_frames);
    assert_eq!(
        snapshot
            .query_row(count, [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(snapshot);

    let complete = store.checkpoint_wal().unwrap();
    assert!(!complete.busy);
    assert!(complete.log_frames > 0);
    assert_eq!(complete.checkpointed_frames, complete.log_frames);
    assert_eq!(
        reader_connection
            .query_row(count, [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn freed_pages_remain_available_for_reuse_after_the_checkpoint() {
    const INSERT: &str =
        "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i + 1 FROM n WHERE i < 128)
         INSERT INTO synthetic_filler SELECT zeroblob(4096) FROM n;";

    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let original_pages = {
        let connection = store.lock();
        connection.execute_batch(INSERT).unwrap();
        let pages = page_count(&connection, "page_count");
        connection
            .execute_batch("DELETE FROM synthetic_filler;")
            .unwrap();
        pages
    };

    store.checkpoint_wal().unwrap();
    let connection = store.lock();
    assert_eq!(page_count(&connection, "page_count"), original_pages);
    assert!(page_count(&connection, "freelist_count") >= 128);
    connection.execute_batch(INSERT).unwrap();
    assert_eq!(page_count(&connection, "page_count"), original_pages);
    assert!(page_count(&connection, "freelist_count") < 128);
}
