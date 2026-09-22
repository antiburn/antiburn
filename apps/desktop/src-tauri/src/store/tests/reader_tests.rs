//! Store-level tests for [`Store::open_reader`]: the UI reader connection
//! the Overview commands use, and the property it exists for — a reader
//! read never queues behind the writer's mutex.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// `open_reader` needs a real database file: two connections cannot share
/// one in-memory database.
fn file_backed_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().expect("creates a temp dir");
    let store = Store::open(directory.path()).expect("opens a file-backed store");
    (directory, store)
}

#[test]
fn open_reader_sees_rows_the_writer_committed() {
    let (_directory, writer) = file_backed_store();
    writer
        .upsert_sessions(&[session("seen", 1_000)], &crate::agents::evidence_cohort())
        .expect("writer commits the session");

    let reader = writer
        .open_reader(Duration::from_millis(100))
        .expect("opens a reader");
    let found = reader
        .session(&SessionKey::new("native", "claude-code", "seen"))
        .expect("the reader can query")
        .expect("the reader sees the writer's committed row");
    assert_eq!(found.key.session_id, "seen");
}

#[test]
fn a_write_through_the_reader_fails_read_only_instead_of_panicking() {
    let (_directory, writer) = file_backed_store();
    let settings = writer.settings().expect("reads the default settings");
    let reader = writer
        .open_reader(Duration::from_millis(100))
        .expect("opens a reader");

    let error = reader
        .save_settings(&settings)
        .expect_err("a read-only connection cannot write");
    let message = error.to_string().to_lowercase();
    assert!(
        message.contains("readonly") || message.contains("read-only"),
        "unexpected error: {error:#}"
    );
}

/// The property the reader exists for: a read through it finishes quickly
/// even while another thread holds the writer's own lock for far longer.
#[test]
fn a_reader_read_does_not_wait_on_a_500ms_writer_lock_hold() {
    let (_directory, writer) = file_backed_store();
    writer
        .upsert_sessions(&[session("seen", 1_000)], &crate::agents::evidence_cohort())
        .expect("writer commits the session");
    let reader = writer
        .open_reader(Duration::from_millis(50))
        .expect("opens a reader");

    let (ready_tx, ready_rx) = mpsc::channel();
    let held = thread::spawn(move || {
        let guard = writer.lock();
        ready_tx.send(()).expect("signals the guard is held");
        thread::sleep(Duration::from_millis(500));
        drop(guard);
    });
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the writer thread holds its lock");

    let started = Instant::now();
    let found = reader
        .session(&SessionKey::new("native", "claude-code", "seen"))
        .expect("the reader's own connection does not wait on the writer's mutex");
    let elapsed = started.elapsed();

    assert!(found.is_some(), "the reader still sees the committed row");
    assert!(
        elapsed < Duration::from_millis(200),
        "a reader read took {elapsed:?} while the writer held its lock for 500 ms"
    );

    held.join().expect("the writer thread finishes");
}
