use super::*;

const LOW: VacuumThreshold = VacuumThreshold {
    min_free_bytes: 1,
    min_free_ratio: 0.25,
};

fn wal_bytes(directory: &std::path::Path) -> u64 {
    let mut path = crate::store::database_path(directory).into_os_string();
    path.push("-wal");
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn fill_then_delete(store: &Store) {
    let connection = store.lock();
    connection
        .execute_batch(
            "CREATE TABLE synthetic_filler(payload BLOB NOT NULL);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
             INSERT INTO synthetic_filler SELECT zeroblob(4096) FROM n;
             DELETE FROM synthetic_filler;",
        )
        .unwrap();
}

#[test]
fn the_writer_caps_the_wal_size() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let limit: i64 = store
        .lock()
        .pragma_query_value(None, "journal_size_limit", |row| row.get(0))
        .unwrap();
    assert_eq!(limit, WAL_SIZE_LIMIT_BYTES);
}

#[test]
fn upkeep_vacuums_free_space_and_empties_the_wal() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    fill_then_delete(&store);
    assert!(wal_bytes(directory.path()) > 0);

    let upkeep = store.maintain_storage(LOW).unwrap();

    assert!(upkeep.vacuumed);
    assert!(upkeep.wal_truncated);
    assert!(upkeep.free_bytes_before >= 2000 * 4096);
    assert!(upkeep.file_bytes_after < upkeep.file_bytes_before);
    assert_eq!(wal_bytes(directory.path()), 0);
    let free: u64 = store
        .lock()
        .pragma_query_value(None, "freelist_count", |row| row.get(0))
        .unwrap();
    assert_eq!(free, 0);
}

#[test]
fn upkeep_skips_the_vacuum_below_the_threshold() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    fill_then_delete(&store);

    let upkeep = store.maintain_storage(VacuumThreshold::default()).unwrap();

    assert!(!upkeep.vacuumed);
    assert!(upkeep.free_bytes_before > 0);
    assert_eq!(upkeep.file_bytes_after, upkeep.file_bytes_before);
    assert_eq!(wal_bytes(directory.path()), 0);
}

#[test]
fn upkeep_keeps_live_rows_and_revisions_moving_forward() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    fill_then_delete(&store);
    store
        .lock()
        .execute_batch(
            "CREATE TABLE synthetic_kept(value TEXT NOT NULL);
             INSERT INTO synthetic_kept VALUES ('kept');",
        )
        .unwrap();
    let before = store.lock().total_changes();

    store.maintain_storage(LOW).unwrap();

    let kept: String = store
        .lock()
        .query_row("SELECT value FROM synthetic_kept", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, "kept");
    assert!(store.lock().total_changes() >= before);
}
