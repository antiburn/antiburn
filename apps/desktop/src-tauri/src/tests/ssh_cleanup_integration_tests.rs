use super::*;
use crate::store::{SessionKey, SessionRecord};

fn session(environment: &str, id: &str, source: &Path) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new(environment, "claude-code", id),
        source_kind: "file".into(),
        source_label: source.to_string_lossy().into_owned(),
        wsl_distro: environment.strip_prefix("wsl:").map(str::to_owned),
        title: Some("Synthetic session".into()),
        title_source: Some("explicit".into()),
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: Some(1000),
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn preferences(store: &Store) -> Vec<(String, String)> {
    let connection = store.lock();
    let mut query = connection
        .prepare("SELECT key, value FROM setting ORDER BY key")
        .unwrap();
    query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

#[tokio::test]
async fn startup_removes_only_ssh_owned_data_and_known_cache_files() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("app");
    fs::create_dir(&data).unwrap();
    let original = temporary.path().join("original.jsonl");
    fs::write(&original, "original transcript").unwrap();
    let store = Store::open(&data).unwrap();
    let records = ["native", "wsl:Ubuntu", "ssh:host-a", "ssh:host-b", "ssh:"]
        .map(|environment| session(environment, "same-id", &original));
    store.upsert_sessions(&records, &["claude-code"]).unwrap();
    for record in &records {
        assert!(store.evidence(&record.key).unwrap().is_some());
    }
    store
        .set_internal_value_checked("ssh-cleanup-test-preference", "keep")
        .unwrap();
    let before = preferences(&store);
    let root = data.join("remote-sessions");
    fs::create_dir_all(root.join("transcripts/host-a/session")).unwrap();
    fs::create_dir_all(root.join("staging/job")).unwrap();
    fs::write(
        root.join("transcripts/host-a/session/transcript.jsonl"),
        "copy",
    )
    .unwrap();
    fs::write(root.join("staging/job/partial"), "partial").unwrap();
    fs::write(root.join("host-a.snapshot.json"), "inventory").unwrap();
    for name in ["hosts.json", "scan-interval.json", "unrelated.json"] {
        fs::write(root.join(name), name).unwrap();
    }
    drop(store);

    let store = open_store(data.clone()).await.unwrap();
    for record in &records {
        if record.key.environment_key.starts_with("ssh:") {
            assert!(store.session(&record.key).unwrap().is_none());
            assert!(store.evidence(&record.key).unwrap().is_none());
        } else {
            assert_eq!(store.session(&record.key).unwrap().as_ref(), Some(record));
            assert!(store.evidence(&record.key).unwrap().is_some());
        }
    }
    assert_eq!(preferences(&store), before);
    assert_eq!(
        fs::read_to_string(&original).unwrap(),
        "original transcript"
    );
    for name in ["transcripts", "staging", "host-a.snapshot.json"] {
        assert!(!root.join(name).exists());
    }
    for name in ["hosts.json", "scan-interval.json", "unrelated.json"] {
        assert_eq!(fs::read_to_string(root.join(name)).unwrap(), name);
    }
    drop(store);
    let repeated = open_store(data).await.unwrap();
    assert_eq!(preferences(&repeated), before);
    assert!(repeated.session(&records[0].key).unwrap().is_some());
}

#[tokio::test]
async fn cleanup_processes_multiple_batches_and_missing_cache_without_creating_it() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::open(temporary.path()).unwrap();
    let records = (0..517)
        .map(|index| session("ssh:host", &index.to_string(), Path::new("/unused")))
        .collect::<Vec<_>>();
    store.upsert_sessions(&records, &["claude-code"]).unwrap();
    drop(store);
    let store = open_store(temporary.path().to_path_buf()).await.unwrap();
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM session", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(!temporary.path().join("remote-sessions").exists());
}

#[tokio::test]
async fn database_failure_propagates_and_next_start_retries_cleanup() {
    let temporary = tempfile::tempdir().unwrap();
    let record = session("ssh:host", "retry", Path::new("/unused"));
    let store = Store::open(temporary.path()).unwrap();
    store
        .upsert_sessions(std::slice::from_ref(&record), &["claude-code"])
        .unwrap();
    store.lock().execute_batch("CREATE TRIGGER refuse_ssh_delete BEFORE DELETE ON session WHEN OLD.environment_key GLOB 'ssh:*' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
    drop(store);
    assert!(open_store(temporary.path().to_path_buf()).await.is_err());
    let store = Store::open(temporary.path()).unwrap();
    assert!(store.session(&record.key).unwrap().is_some());
    store
        .lock()
        .execute_batch("DROP TRIGGER refuse_ssh_delete;")
        .unwrap();
    drop(store);
    let store = open_store(temporary.path().to_path_buf()).await.unwrap();
    assert!(store.session(&record.key).unwrap().is_none());
}

#[tokio::test]
async fn filesystem_failure_keeps_startup_available_and_retries_cleanup_next_launch() {
    let temporary = tempfile::tempdir().unwrap();
    let record = session("ssh:host", "retry", Path::new("/unused"));
    let local = session("native", "retry", Path::new("/local/original"));
    let wsl = session("wsl:Ubuntu", "retry", Path::new("/wsl/original"));
    let store = Store::open(temporary.path()).unwrap();
    store
        .upsert_sessions(
            &[record.clone(), local.clone(), wsl.clone()],
            &["claude-code"],
        )
        .unwrap();
    drop(store);
    let root = temporary.path().join("remote-sessions");
    fs::write(&root, "unexpected file").unwrap();
    let store = open_store(temporary.path().to_path_buf()).await.unwrap();
    assert!(store.session(&record.key).unwrap().is_none());
    assert!(store.evidence(&record.key).unwrap().is_none());
    for retained in [&local, &wsl] {
        assert_eq!(
            store.session(&retained.key).unwrap().as_ref(),
            Some(retained)
        );
        assert!(store.evidence(&retained.key).unwrap().is_some());
    }
    assert_eq!(fs::read_to_string(&root).unwrap(), "unexpected file");
    drop(store);
    fs::remove_file(&root).unwrap();
    fs::create_dir_all(root.join("transcripts/host")).unwrap();
    fs::write(root.join("transcripts/host/copy.jsonl"), "copy").unwrap();
    let store = open_store(temporary.path().to_path_buf()).await.unwrap();
    assert!(store.session(&record.key).unwrap().is_none());
    assert!(!root.join("transcripts").exists());
    for retained in [&local, &wsl] {
        assert_eq!(
            store.session(&retained.key).unwrap().as_ref(),
            Some(retained)
        );
    }
}

#[cfg(unix)]
#[test]
fn cleanup_unlinks_cache_symlinks_without_following_external_targets() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let outside = temporary.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("keep.jsonl");
    fs::write(&sentinel, "untouched").unwrap();
    let root = temporary.path().join("remote-sessions");
    symlink(&outside, &root).unwrap();
    remove_cached_copies(&root).unwrap();
    assert!(fs::symlink_metadata(&root).is_err());
    fs::create_dir(&root).unwrap();
    symlink(&outside, root.join("transcripts")).unwrap();
    symlink(&outside, root.join("staging")).unwrap();
    symlink(&sentinel, root.join("host.snapshot.json")).unwrap();
    remove_cached_copies(&root).unwrap();
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "untouched");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::create_dir_all(root.join("transcripts/session")).unwrap();
    symlink(&outside, root.join("transcripts/session/link")).unwrap();
    remove_cached_copies(&root).unwrap();
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "untouched");
}
