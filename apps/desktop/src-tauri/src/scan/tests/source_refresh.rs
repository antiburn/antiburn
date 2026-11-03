use super::*;

async fn scan_one(
    store: &Store,
    log: &SessionLog,
    policy: &OwnerPolicy,
) -> (Described, Vec<session_lifecycle::Observation>) {
    let mut admission = writer(store);
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![log.clone()])).await.unwrap();
    drop(sender);
    let described = tokio::time::timeout(
        Duration::from_secs(10),
        run(finite, live, None, policy, &mut admission),
    )
    .await
    .unwrap();
    admission.finish().unwrap();
    let AdmissionIo::Test { reports, .. } = &admission.io else {
        unreachable!()
    };
    let reports = std::mem::take(&mut *reports.lock().unwrap());
    (described, reports)
}

fn source_generation(store: &Store, key: &SessionKey) -> i64 {
    store
        .lock()
        .query_row(
            "SELECT source_generation FROM session
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            rusqlite::params![key.environment_key, key.agent, key.session_id],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn source_refresh_updates_same_size_event_fingerprint_without_dropping_previous_record() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let log = fixture_log(home.path(), "refresh-event");
    let mut policy = policy(home.path(), None);
    let (first, _) = scan_one(&store, &log, &policy).await;
    let previous = &first.records[0];
    let generation = source_generation(&store, &previous.key);
    assert_eq!(previous.activity_source, "event");
    assert!(previous.source_fingerprint.is_some());

    let SessionSource::File(path) = &log.source else {
        unreachable!()
    };
    let before = std::fs::read_to_string(path).unwrap();
    let after = before.replace("Repair the fixture", "Review the fixture");
    assert_ne!(before, after);
    assert_eq!(before.len(), after.len());
    std::fs::write(path, after).unwrap();
    assert_eq!(
        stat_activity_cursor(&log).await.as_deref(),
        Some(previous.activity_cursor.as_str())
    );

    antiburn_local::discovery::track_head_reads(path);
    let (ordinary, _) = scan_one(&store, &log, &policy).await;
    assert_eq!(ordinary.records, first.records);
    assert!(ordinary.changed.is_empty());
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(path), 0);
    assert_eq!(source_generation(&store, &previous.key), generation);

    policy.refresh_file_sources = true;
    policy.cutoff = None;
    antiburn_local::discovery::track_head_reads(path);
    let (refreshed, reports) = scan_one(&store, &log, &policy).await;
    let current = &refreshed.records[0];
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(path), 1);
    assert_eq!(current.key, previous.key);
    assert_eq!(current.activity_cursor, previous.activity_cursor);
    assert_eq!(current.updated_at_epoch, previous.updated_at_epoch);
    assert_eq!(current.title.as_deref(), Some("Review the fixture"));
    assert_ne!(current.source_fingerprint, previous.source_fingerprint);
    assert!(source_generation(&store, &current.key) > generation);
    assert_eq!(refreshed.changed, std::slice::from_ref(&current.key));
    assert!(reports.iter().any(|report| matches!(
        report,
        session_lifecycle::Observation::Indexed { sessions, .. }
            if sessions.iter().any(|session| session.key == current.key && !session.is_new)
    )));
    assert!(reports.iter().any(|report| matches!(
        report,
        session_lifecycle::Observation::RowChanged { session, facets, .. }
            if session == &current.key && facets.title && facets.metadata
    )));
}

#[tokio::test]
async fn source_refresh_reads_replacement_identity_through_both_cache_gates() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let log = fixture_log(home.path(), "refresh-old");
    let mut policy = policy(home.path(), None);
    let (first, _) = scan_one(&store, &log, &policy).await;
    let previous = &first.records[0];
    let SessionSource::File(path) = &log.source else {
        unreachable!()
    };
    let before = std::fs::read_to_string(path).unwrap();
    let after = before.replace("refresh-old", "refresh-new");
    assert_ne!(before, after);
    assert_eq!(before.len(), after.len());
    std::fs::write(path, after).unwrap();
    assert_eq!(
        stat_activity_cursor(&log).await.as_deref(),
        Some(previous.activity_cursor.as_str())
    );

    policy.refresh_file_sources = true;
    policy.cutoff = None;
    antiburn_local::discovery::track_head_reads(path);
    let (refreshed, _) = scan_one(&store, &log, &policy).await;
    assert_eq!(antiburn_local::discovery::take_tracked_head_reads(path), 1);
    assert_eq!(refreshed.records.len(), 1);
    let replacement = &refreshed.records[0];
    assert_eq!(replacement.key.session_id, "refresh-new");
    assert_eq!(replacement.activity_cursor, previous.activity_cursor);
    assert_ne!(replacement.source_fingerprint, previous.source_fingerprint);
    assert!(store.session(&replacement.key).unwrap().is_some());
    assert_eq!(
        store.session(&previous.key).unwrap().as_ref(),
        Some(previous)
    );
}

#[tokio::test]
async fn source_refresh_rejects_identity_changes_after_a_failed_identity_read() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let log = fixture_log(home.path(), "refresh-old");
    let (first, _) = scan_one(&store, &log, &policy(home.path(), None)).await;
    let previous = &first.records[0];
    let SessionSource::File(path) = &log.source else {
        unreachable!()
    };
    let before = std::fs::read_to_string(path).unwrap();
    std::fs::remove_file(path).unwrap();
    let unreadable = session_log_read(&log).await.unwrap();
    assert!(unreadable.head_hash.is_none());
    assert!(unreadable.content.is_none());
    assert!(unreadable.metadata.session_id.is_none());
    assert!(unreadable.metadata.cwd.is_none());
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: PreparationStage::Filter,
        blocked: HashSet::from([log.source_label()]),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let mut policy = policy(home.path(), Some(hooks.clone()));
    policy.refresh_file_sources = true;
    policy.cutoff = None;
    let mut admission = writer(&store);
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![log.clone()])).await.unwrap();
    drop(sender);
    let control = async {
        arrivals.recv().await.unwrap();
        assert_eq!(
            store.session(&previous.key).unwrap().as_ref(),
            Some(previous)
        );
        std::fs::write(path, before.replace("refresh-old", "refresh-new")).unwrap();
        hooks.release.add_permits(1);
    };
    let (described, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(run(finite, live, None, &policy, &mut admission), control)
    })
    .await
    .unwrap();
    admission.finish().unwrap();
    assert!(described.records.is_empty());
    assert_eq!(described.rejected, std::slice::from_ref(&previous.key));
    assert_eq!(described.gate.missing_cwd, 1);
    assert!(store.session(&previous.key).unwrap().is_none());
    assert!(
        store
            .session(&SessionKey::new(
                "native",
                AgentKind::Claude.slug(),
                "refresh-new"
            ))
            .unwrap()
            .is_none()
    );

    policy.hooks = None;
    let (retry, _) = scan_one(&store, &log, &policy).await;
    assert_eq!(retry.records.len(), 1);
    assert_eq!(retry.records[0].key.session_id, "refresh-new");
    assert!(store.session(&previous.key).unwrap().is_none());
    assert_eq!(
        store.session(&retry.records[0].key).unwrap().as_ref(),
        Some(&retry.records[0])
    );
}

#[tokio::test]
async fn source_refresh_does_not_restore_a_session_deleted_after_its_source_read() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let log = fixture_log(home.path(), "refresh-deleted");
    let (first, _) = scan_one(&store, &log, &policy(home.path(), None)).await;
    let key = &first.records[0].key;
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: PreparationStage::Git,
        blocked: HashSet::from([log.source_label()]),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let mut policy = policy(home.path(), Some(hooks.clone()));
    policy.refresh_file_sources = true;
    policy.cutoff = None;
    let mut admission = writer(&store);
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![log])).await.unwrap();
    drop(sender);
    let control = async {
        arrivals.recv().await.unwrap();
        store.delete_session(key).unwrap().unwrap();
        hooks.release.add_permits(1);
    };
    let (described, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(run(finite, live, None, &policy, &mut admission), control)
    })
    .await
    .unwrap();
    assert!(admission.finish().is_err());
    assert!(described.records.is_empty());
    assert!(store.session(key).unwrap().is_none());
}
