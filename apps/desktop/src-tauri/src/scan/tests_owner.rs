use super::super::prepare::{PreparationHooks, PreparationStage};
use super::*;
use std::sync::atomic::AtomicI64;
use tokio::sync::Semaphore;

#[path = "tests/live_admission.rs"]
mod live_admission;

fn fixture_log(home: &std::path::Path, id: &str) -> SessionLog {
    let project = home.join(".claude/projects/synthetic");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "user", "sessionId": id, "cwd": home,
                "timestamp": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap(),
                "message": {"role": "user", "content": "Repair the fixture"},
            })
        ),
    )
    .unwrap();
    SessionLog {
        agent_type: AgentKind::Claude,
        source: SessionSource::File(path),
        updated_at: Some(unix_now()),
        environment: DiscoveryEnvironment::Native,
    }
}

fn writer(store: &Store) -> IncrementalAdmission<'_> {
    IncrementalAdmission {
        io: AdmissionIo::Test {
            cancel: Arc::new(AtomicBool::new(false)),
            reports: Arc::new(Mutex::new(Vec::new())),
            now: Arc::new(AtomicI64::new(unix_now())),
            cancel_after_report: false,
        },
        store,
        previous_records: HashMap::new(),
        evidence_agents: agents::evidence_cohort(),
        write_fence: store.capture_destructive_write_epoch(),
        error: None,
        outcomes: HashMap::new(),
        list_changed: false,
    }
}

fn policy(home: &std::path::Path, hooks: Option<Arc<PreparationHooks>>) -> OwnerPolicy {
    OwnerPolicy {
        home: home.to_path_buf(),
        ignored: HashSet::new(),
        cutoff: Some(0),
        include_non_repo_folders: true,
        progress: false,
        refresh_repositories: false,
        hooks,
    }
}

fn batch(logs: Vec<SessionLog>) -> DiscoveryBatch {
    DiscoveryBatch {
        agent: AgentKind::Claude,
        candidates: logs,
        complete: false,
    }
}

#[tokio::test]
async fn opencode_metadata_and_parser_reset_claim_commit_while_strict_hash_is_blocked() {
    for parser_revision in [62, 0] {
        opencode_admission_barrier(false, true, parser_revision).await;
    }
}

#[tokio::test]
async fn opencode_late_strict_hash_cannot_overwrite_a_new_metadata_generation() {
    for parser_revision in [62, 0] {
        opencode_admission_barrier(true, true, parser_revision).await;
    }
}

#[tokio::test]
async fn opencode_completed_snapshot_rechecks_source_before_store_commit() {
    for parser_revision in [62, 0] {
        opencode_admission_barrier(true, false, parser_revision).await;
    }
}

async fn opencode_admission_barrier(
    replace_before_completion: bool,
    persist_new_revision: bool,
    parser_revision: i64,
) {
    use crate::insights_worker::journal::tests::SyntheticJournal;
    let state = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().to_str().unwrap();
    let source = SyntheticJournal::new(16);
    source
        .connection
        .execute_batch(
            "ALTER TABLE session ADD COLUMN directory TEXT;
         ALTER TABLE session ADD COLUMN title TEXT;",
        )
        .unwrap();
    let now = unix_now();
    source
        .connection
        .execute(
            "UPDATE session SET directory = ?1, title = 'Synthetic session', time_updated = ?2",
            rusqlite::params![cwd, now * 1000],
        )
        .unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, cwd);
    let mut old = source.record(cwd);
    old.updated_at_epoch = Some(now - 420);
    old.activity_source = "unknown".into();
    store
        .upsert_sessions(&[old], &agents::evidence_cohort())
        .unwrap();
    store
        .claim_next_opencode_journal(now - 420, 60)
        .unwrap()
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = ?1, pending = 0,
                 observed_generation = (SELECT source_generation FROM session
                     WHERE session.environment_key = opencode_journal_delivery.environment_key
                       AND session.agent = opencode_journal_delivery.agent
                       AND session.session_id = opencode_journal_delivery.session_id),
                 next_attempt_at_epoch = NULL, lease_expires_at_epoch = NULL",
            [parser_revision],
        )
        .unwrap();
    drop(store);
    let store = Store::open(state.path()).unwrap();
    assert!(
        store
            .claim_next_opencode_journal(now, 60)
            .unwrap()
            .is_none()
    );
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: source.source(),
        updated_at: Some(now),
        environment: DiscoveryEnvironment::Native,
    };
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: if replace_before_completion {
            PreparationStage::FingerprintCommit
        } else {
            PreparationStage::Fingerprint
        },
        blocked: HashSet::from([log.source_label()]),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let policy = policy(home.path(), Some(hooks.clone()));
    let mut writer = writer(&store);
    let (sender, finite) = mpsc::channel(1);
    let (live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![log.clone()])).await.unwrap();
    drop(sender);
    let control = async {
        tokio::time::timeout(Duration::from_secs(10), arrivals.recv())
            .await
            .unwrap()
            .unwrap();
        let key = SessionKey::new("native", "opencode", "native-session");
        let record = store.session(&key).unwrap().unwrap();
        assert_eq!(record.updated_at_epoch, Some(now));
        assert!(record.source_fingerprint.is_none());
        assert!(store.pending_source_fingerprint(&key).unwrap().is_some());
        assert!(
            store
                .claim_next_evidence(&["opencode"], now + 1, 60)
                .unwrap()
                .is_none()
        );
        let claim = store.claim_next_opencode_journal(now + 1, 60).unwrap();
        if parser_revision == 62 {
            let claim =
                claim.expect("the named predecessor can read before the strict hash finishes");
            assert!(claim.cursor().unwrap().is_none());
            assert!(claim.requires_full_read);
        } else {
            assert!(
                claim.is_none(),
                "metadata cannot authorize an unknown parser"
            );
        }
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT parser_revision FROM opencode_journal_delivery",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            parser_revision,
            "only a successful source-page commit can stamp the current parser"
        );
        live_sender
            .send(batch(vec![fixture_log(
                home.path(),
                "live-during-strict-hash",
            )]))
            .await
            .unwrap();
        wait_for_row(&store, "live-during-strict-hash").await;
        if replace_before_completion {
            source
                .connection
                .execute("UPDATE session SET title = 'Replacement title'", [])
                .unwrap();
            if persist_new_revision {
                let mut newer = record;
                newer.activity_cursor = fingerprint::admission_revision(&log).await.unwrap();
                store
                    .upsert_sessions(&[newer], &agents::evidence_cohort())
                    .unwrap();
            }
        }
        hooks.release.add_permits(2);
    };
    tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(run(finite, live, None, &policy, &mut writer), control)
    })
    .await
    .unwrap();
    writer.finish().unwrap();
    let key = SessionKey::new("native", "opencode", "native-session");
    if replace_before_completion {
        assert!(store.pending_source_fingerprint(&key).unwrap().is_some());
        let retry = store.next_source_fingerprint_attempt().unwrap().unwrap();
        assert_eq!(retry, 0, "a changed revision retries without stale backoff");
        recover_at(&store, home.path(), now).await;
    }
    assert!(store.pending_source_fingerprint(&key).unwrap().is_none());
    let revision = fingerprint::admission_revision(&log).await.unwrap();
    let expected = fingerprint::strict_fingerprint(&log, &revision)
        .await
        .unwrap();
    assert_eq!(
        store
            .session(&key)
            .unwrap()
            .unwrap()
            .source_fingerprint
            .as_deref(),
        Some(expected.as_str())
    );
}

async fn recover_at(store: &Store, home: &std::path::Path, epoch: i64) {
    let mut admission = writer(store);
    let AdmissionIo::Test { now, .. } = &admission.io else {
        unreachable!()
    };
    now.store(epoch, Ordering::Release);
    let policy = policy(home, None);
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    drop(sender);
    tokio::time::timeout(
        Duration::from_secs(10),
        run(finite, live, None, &policy, &mut admission),
    )
    .await
    .unwrap();
    admission.finish().unwrap();
}

#[tokio::test]
async fn production_owner_recovers_dropped_hash_after_restart_outside_the_activity_cutoff() {
    use crate::insights_worker::journal::tests::SyntheticJournal;
    let state = tempfile::tempdir().unwrap();
    let source = SyntheticJournal::new(16);
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: source.source(),
        updated_at: Some(1),
        environment: DiscoveryEnvironment::Native,
    };
    let mut record = source.record("/synthetic");
    record.updated_at_epoch = Some(1);
    record.activity_cursor = fingerprint::admission_revision(&log).await.unwrap();
    record.source_fingerprint = None;
    let key = record.key.clone();
    let store = Store::open(state.path()).unwrap();
    store.upsert_sessions(&[record], &["opencode"]).unwrap();
    assert_eq!(
        store
            .claim_source_fingerprints(1000, &HashSet::new(), 2, 1000)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    let store = Store::open(state.path()).unwrap();
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(0));
    recover_at(&store, state.path(), 1000).await;
    let first = store
        .session(&key)
        .unwrap()
        .unwrap()
        .source_fingerprint
        .unwrap();
    assert!(store.pending_source_fingerprint(&key).unwrap().is_none());
    let old_claim = store
        .claim_next_evidence(&["opencode"], 1000, 60)
        .unwrap()
        .unwrap();
    let admission = fingerprint::admission_revision(&log).await.unwrap();
    source
        .connection
        .execute(
            "UPDATE message SET data = json_set(data, '$.tokens.input', 2)",
            [],
        )
        .unwrap();
    assert_eq!(
        fingerprint::admission_revision(&log).await.unwrap(),
        admission
    );
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(1300));
    recover_at(&store, state.path(), 1299).await;
    assert_eq!(
        store
            .session(&key)
            .unwrap()
            .unwrap()
            .source_fingerprint
            .as_deref(),
        Some(first.as_str())
    );
    recover_at(&store, state.path(), 1300).await;
    assert_ne!(
        store
            .session(&key)
            .unwrap()
            .unwrap()
            .source_fingerprint
            .unwrap(),
        first
    );
    let new_claim = store
        .claim_next_evidence(&["opencode"], 1300, 60)
        .unwrap()
        .unwrap();
    assert!(new_claim.source_generation > old_claim.source_generation);
}

#[tokio::test]
async fn production_owner_retries_failed_hash_with_backoff_after_checkpoint() {
    use crate::insights_worker::journal::tests::SyntheticJournal;
    let state = tempfile::tempdir().unwrap();
    let source = SyntheticJournal::new(16);
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: source.source(),
        updated_at: Some(1),
        environment: DiscoveryEnvironment::Native,
    };
    let mut record = source.record("/synthetic");
    record.updated_at_epoch = Some(1);
    record.activity_cursor = fingerprint::admission_revision(&log).await.unwrap();
    record.source_fingerprint = None;
    let key = record.key.clone();
    let store = Store::open(state.path()).unwrap();
    store.upsert_sessions(&[record], &["opencode"]).unwrap();
    source
        .connection
        .execute_batch("ALTER TABLE message RENAME TO unavailable_message")
        .unwrap();
    recover_at(&store, state.path(), 1000).await;
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(1001));
    assert!(store.pending_source_fingerprint(&key).unwrap().is_some());
    recover_at(&store, state.path(), 1000).await;
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(1001));
    source
        .connection
        .execute_batch(
            "ALTER TABLE unavailable_message RENAME TO message; PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    recover_at(&store, state.path(), 1001).await;
    assert!(store.pending_source_fingerprint(&key).unwrap().is_none());
}

#[tokio::test]
async fn production_restart_recovers_the_early_v80_pending_source_shape() {
    use crate::insights_worker::journal::tests::SyntheticJournal;
    let state = tempfile::tempdir().unwrap();
    let source = SyntheticJournal::new(16);
    source.connection.execute_batch(
        "ALTER TABLE session ADD COLUMN directory TEXT; ALTER TABLE session ADD COLUMN title TEXT;"
    ).unwrap();
    source
        .connection
        .execute(
            "UPDATE session SET directory = ?1, title = 'Recovered fixture'",
            [state.path().to_str().unwrap()],
        )
        .unwrap();
    let mut record = source.record(state.path().to_str().unwrap());
    record.updated_at_epoch = Some(1);
    record.source_fingerprint = None;
    record.activity_cursor = format!(
        "opencode-admission-v1:{}",
        serde_json::json!([format!(
            "providerDb:{}:native-session",
            source.path.display()
        )])
    );
    let key = record.key.clone();
    let store = Store::open(state.path()).unwrap();
    store.upsert_sessions(&[record], &["opencode"]).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(crate::store::database_path(state.path())).unwrap();
    connection
        .execute_batch(
            "DROP INDEX session_fingerprint_due;
         ALTER TABLE session DROP COLUMN fingerprint_source_path;
         ALTER TABLE session DROP COLUMN fingerprint_checked_at_epoch;
         ALTER TABLE session DROP COLUMN fingerprint_next_attempt_at_epoch;
         ALTER TABLE session DROP COLUMN fingerprint_retry_count;
         ALTER TABLE session DROP COLUMN fingerprint_in_flight;",
        )
        .unwrap();
    drop(connection);
    let store = Store::open(state.path()).unwrap();
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(0));
    recover_at(&store, state.path(), 1000).await;
    assert!(store.pending_source_fingerprint(&key).unwrap().is_some());
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(0));
    recover_at(&store, state.path(), 1000).await;
    assert!(store.pending_source_fingerprint(&key).unwrap().is_none());
    let recovered = store.session(&key).unwrap().unwrap();
    assert!(
        recovered
            .activity_cursor
            .starts_with("opencode-admission-v2:")
    );
    assert!(recovered.source_fingerprint.unwrap().starts_with("sv1:db:"));
}

async fn wait_for_row(store: &Store, id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !store
            .session_records()
            .unwrap()
            .values()
            .any(|record| record.key.session_id == id)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the live row commits while background work stays blocked");
}

async fn barrier_live_commit(stage: PreparationStage) {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let background = (0..16)
        .map(|index| fixture_log(home.path(), &format!("background-{index}")))
        .collect::<Vec<_>>();
    let live_log = fixture_log(home.path(), "live");
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage,
        blocked: background.iter().map(SessionLog::source_label).collect(),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let policy = policy(home.path(), Some(hooks.clone()));
    let mut writer = writer(&store);
    let (finite_sender, finite) = mpsc::channel(2);
    let (live_sender, live) = mpsc::channel(2);
    finite_sender.send(batch(background)).await.unwrap();
    let control = async {
        for _ in 0..15 {
            tokio::time::timeout(Duration::from_secs(10), arrivals.recv())
                .await
                .unwrap()
                .unwrap();
        }
        assert!(
            arrivals.try_recv().is_err(),
            "the sixteenth background slot stays reserved"
        );
        live_sender.send(batch(vec![live_log])).await.unwrap();
        wait_for_row(&store, "live").await;
        assert_eq!(store.session_records().unwrap().len(), 1);
        hooks.release.add_permits(16);
        drop(finite_sender);
    };
    let (described, ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(run(finite, live, None, &policy, &mut writer), control)
    })
    .await
    .unwrap();
    writer.finish().unwrap();
    assert_eq!(
        described.records.len(),
        16,
        "live work does not enter finite coverage"
    );
    assert_eq!(store.session_records().unwrap().len(), 17);
}

#[tokio::test]
async fn live_commits_with_all_background_identity_slots_blocked() {
    barrier_live_commit(PreparationStage::Identity).await;
}

#[tokio::test]
async fn live_commits_with_all_background_filters_blocked() {
    barrier_live_commit(PreparationStage::Filter).await;
}

#[tokio::test]
async fn live_commits_with_all_background_title_lookups_blocked() {
    barrier_live_commit(PreparationStage::Title).await;
}

#[tokio::test]
async fn live_commits_with_all_background_git_lookups_blocked() {
    barrier_live_commit(PreparationStage::Git).await;
}

#[tokio::test]
async fn live_commits_before_discovery_closes_and_outside_the_finite_agent_scope() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let mut writer = writer(&store);
    let policy = policy(home.path(), None);
    let (finite_sender, finite) = mpsc::channel(2);
    let (live_sender, live) = mpsc::channel(2);
    let control = async {
        live_sender
            .send(batch(vec![fixture_log(
                home.path(),
                "live-during-discovery",
            )]))
            .await
            .unwrap();
        wait_for_row(&store, "live-during-discovery").await;
        assert!(
            !finite_sender.is_closed(),
            "finite discovery is still blocked"
        );
        drop(finite_sender);
    };
    let (described, ()) = tokio::join!(run(finite, live, None, &policy, &mut writer), control);
    writer.finish().unwrap();
    assert!(described.records.is_empty());
    let AdmissionIo::Test { reports, .. } = &writer.io else {
        unreachable!()
    };
    let reports = reports.lock().unwrap();
    assert!(reports.iter().any(|report| matches!(report,
        session_lifecycle::Observation::Indexed { sessions, .. } if sessions.iter().any(|session| session.key.session_id == "live-during-discovery")
    )), "unexpected live admission reports: {reports:?}");
}

#[tokio::test]
async fn finite_owner_waits_for_title_producer_handoff() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let mut writer = writer(&store);
    let policy = policy(home.path(), None);
    let (finite_sender, finite) = mpsc::channel(1);
    let (live_sender, live) = mpsc::channel(1);
    let (title_sender, titles) = mpsc::channel(1);
    drop(finite_sender);
    drop(live_sender);

    let owner = run_with_titles(finite, live, titles, None, &policy, &mut writer);
    tokio::pin!(owner);
    tokio::select! {
        _ = &mut owner => {
            panic!("finite owner returned before the title producer closed its channel");
        }
        () = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    drop(title_sender);
    let described = owner.await;
    assert!(described.records.is_empty());
}

#[test]
fn physical_file_identity_tracks_moves_and_replacements_at_the_binding() {
    let home = tempfile::tempdir().unwrap();
    let original = fixture_log(home.path(), "physical-source");
    let mut moved = original.clone();
    let SessionSource::File(path) = &original.source else {
        unreachable!();
    };
    moved.source = SessionSource::File(path.with_file_name("moved.jsonl"));
    let mut replacement = original.clone();
    replacement.updated_at = Some(unix_now() + 1);

    assert_ne!(identity(&original), identity(&moved));
    assert_eq!(identity(&original), identity(&replacement));
}

#[tokio::test]
async fn continuous_equal_and_backward_live_hints_do_not_extend_finite_completion() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let background = fixture_log(home.path(), "finite");
    let mut live_log = fixture_log(home.path(), "continuous-live");
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: PreparationStage::Git,
        blocked: HashSet::from([background.source_label()]),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let policy = policy(home.path(), Some(hooks.clone()));
    let mut writer = writer(&store);
    let (finite_sender, finite) = mpsc::channel(2);
    let (live_sender, live) = mpsc::channel(2);
    finite_sender.send(batch(vec![background])).await.unwrap();
    let continuous = async {
        let mut turn = 0;
        loop {
            live_log.updated_at = Some(if turn % 2 == 0 { 100 } else { 99 });
            if live_sender
                .send(batch(vec![live_log.clone()]))
                .await
                .is_err()
            {
                break;
            }
            turn += 1;
            tokio::task::yield_now().await;
        }
        turn
    };
    let control = async {
        arrivals.recv().await.unwrap();
        wait_for_row(&store, "continuous-live").await;
        hooks.release.add_permits(1);
        drop(finite_sender);
    };
    let (described, (), turns) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(
            run(finite, live, None, &policy, &mut writer),
            control,
            continuous
        )
    })
    .await
    .unwrap();
    writer.finish().unwrap();
    assert_eq!(described.records.len(), 1);
    assert_eq!(described.records[0].key.session_id, "finite");
    assert!(turns > 1);
}

#[tokio::test]
async fn panic_releases_physical_identity_for_a_pending_dirty_refresh() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let log = fixture_log(home.path(), "panic-then-refresh");
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: PreparationStage::Identity,
        blocked: HashSet::from([log.source_label()]),
        panic: HashSet::from([log.source_label()]),
        panic_once: AtomicBool::new(true),
        entered,
        release: Semaphore::new(0),
    });
    let policy = policy(home.path(), Some(hooks.clone()));
    let mut writer = writer(&store);
    let (finite_sender, finite) = mpsc::channel(2);
    let (live_sender, live) = mpsc::channel(2);
    finite_sender.send(batch(vec![log.clone()])).await.unwrap();
    let control = async {
        arrivals.recv().await.unwrap();
        live_sender.send(batch(vec![log])).await.unwrap();
        hooks.release.add_permits(2);
        arrivals.recv().await.unwrap();
        wait_for_row(&store, "panic-then-refresh").await;
        drop(finite_sender);
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(run(finite, live, None, &policy, &mut writer), control)
    })
    .await
    .unwrap();
    assert!(
        writer.finish().is_err(),
        "the panic remains an explicit failed pass"
    );
}

#[tokio::test]
async fn cancellation_aborts_blocked_preparation_and_fences_late_results() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let log = fixture_log(home.path(), "cancelled");
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: PreparationStage::Git,
        blocked: HashSet::from([log.source_label()]),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let policy = policy(home.path(), Some(hooks.clone()));
    let mut writer = writer(&store);
    let AdmissionIo::Test { cancel, .. } = &writer.io else {
        unreachable!()
    };
    let cancel = cancel.clone();
    let (finite_sender, finite) = mpsc::channel(2);
    let (_live_sender, live) = mpsc::channel(2);
    finite_sender.send(batch(vec![log])).await.unwrap();
    let control = async {
        arrivals.recv().await.unwrap();
        cancel.store(true, Ordering::SeqCst);
    };
    let (described, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(run(finite, live, None, &policy, &mut writer), control)
    })
    .await
    .unwrap();
    hooks.release.add_permits(1);
    tokio::task::yield_now().await;
    assert!(described.records.is_empty());
    assert!(store.session_records().unwrap().is_empty());
}
