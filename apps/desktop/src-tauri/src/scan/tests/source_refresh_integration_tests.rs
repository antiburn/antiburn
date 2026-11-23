use super::*;

#[tokio::test]
async fn source_refresh_repairs_a_second_source_that_changes_mid_pass() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(temp.path()).unwrap();
    let first = fixture_log(temp.path(), "11111111-1111-4111-8111-111111111111");
    let second = fixture_log(temp.path(), "22222222-2222-4222-8222-222222222222");
    let unrelated_id = "33333333-3333-4333-8333-333333333333";
    let unrelated = fixture_log(temp.path(), unrelated_id);
    let unrelated_key = SessionKey::new("native", unrelated.agent_type.slug(), unrelated_id);
    let first_key = index_source(&store, temp.path(), &first).await;
    let first_fingerprint = fingerprint(&store, &first_key);
    fail_changed(&store, &first_key);
    let expected_first = replace_same_size(&first).await;
    assert_ne!(expected_first, first_fingerprint);
    let second_key = index_source(&store, temp.path(), &second).await;
    let second_fingerprint = fingerprint(&store, &second_key);

    let (entered, mut waiting) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: PreparationStage::Identity,
        blocked: HashSet::from([first.source_label()]),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let mut policy = policy(&store, temp.path(), Some(hooks.clone()));
    policy.source_refresh = Some(Arc::default());

    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![first.clone()])).await.unwrap();
    let mut admission = writer(&store);
    let change_second = async {
        assert_eq!(
            waiting.recv().await.as_deref(),
            Some(first.source_label().as_str())
        );
        fail_changed(&store, &second_key);
        let expected_second = replace_same_size(&second).await;
        sender.send(batch(vec![second, unrelated])).await.unwrap();
        drop(sender);
        hooks.release.add_permits(1);
        expected_second
    };
    let (described, expected_second) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            run(finite, live, None, &policy, &mut admission),
            change_second
        )
    })
    .await
    .unwrap();
    admission.finish().unwrap();

    assert_eq!(described.changed.len(), 2);
    assert_ne!(expected_second, second_fingerprint);
    assert_eq!(fingerprint(&store, &first_key), expected_first);
    assert_eq!(fingerprint(&store, &second_key), expected_second);
    assert!(store.session(&unrelated_key).unwrap().is_none());
    assert!(
        store
            .claim_next_evidence(&[first_key.agent.as_str()], 399, 60)
            .unwrap()
            .is_none()
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn source_refresh_replays_a_superseded_folder_update_after_evidence_is_ready() {
    const HOME_ENV: &str = "ANTIBURN_TEST_SOURCE_REFRESH_REPLAY_HOME";
    let Some(home) = std::env::var_os(HOME_ENV).map(PathBuf::from) else {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "scan::owner::tests::source_refresh::source_refresh_replays_a_superseded_folder_update_after_evidence_is_ready",
                "--nocapture",
            ])
            .env(HOME_ENV, &home)
            .env("HOME", &home)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };

    let store = Store::open_in_memory(&home).unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    let mut restored = fixture_log(&home, id);
    let old_activity = unix_now() - CURRENT_WINDOW_SECS - 60;
    restored.updated_at = Some(old_activity);
    let SessionSource::File(transcript) = &restored.source else {
        unreachable!();
    };
    let cwd = home.join("Documents/project");
    std::fs::create_dir_all(&cwd).unwrap();
    let mut event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(transcript).unwrap()).unwrap();
    event["cwd"] = serde_json::json!(cwd);
    event["timestamp"] = serde_json::json!(
        time::OffsetDateTime::from_unix_timestamp(old_activity)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    );
    let bytes = format!("{event}\n");
    std::fs::write(transcript, &bytes).unwrap();
    let key = index_source(&store, &home, &restored).await;
    std::fs::remove_file(transcript).unwrap();
    let claim = store
        .claim_next_evidence(&[key.agent.as_str()], 100, 60)
        .unwrap()
        .unwrap();
    assert!(
        store
            .fail_evidence(
                &claim,
                crate::store::EvidenceFailure::Failed {
                    revisions: crate::analysis::projection_revisions(),
                },
                crate::insights_worker::EVIDENCE_ERROR_SOURCE_MISSING,
            )
            .unwrap()
    );
    assert!(
        store
            .has_local_evidence_source_refresh_targets(&key.agent)
            .unwrap()
    );
    std::fs::write(transcript, bytes).unwrap();

    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let hooks = Arc::new(PreparationHooks {
        stage: PreparationStage::DeferralPublication,
        blocked: HashSet::from([id.to_owned()]),
        panic: HashSet::new(),
        panic_once: AtomicBool::new(false),
        entered,
        release: Semaphore::new(0),
    });
    let mut first_policy = policy(&store, &home, Some(hooks.clone()));
    first_policy.cutoff = None;
    first_policy.source_refresh = Some(Arc::default());
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![restored.clone()])).await.unwrap();
    drop(sender);
    let mut admission = writer(&store);
    let (described, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            run(finite, live, None, &first_policy, &mut admission),
            async {
                assert_eq!(arrivals.recv().await.as_deref(), Some(id));
                store.grant_dir("Desktop").unwrap();
                hooks.release.add_permits(1);
            }
        )
    })
    .await
    .unwrap();
    admission.finish().unwrap();
    assert!(store.deferred_permission_dirs().unwrap().is_empty());

    let controller = ScanController::default();
    let now = tokio::time::Instant::now();
    controller
        .source_refreshes()
        .next_allowed
        .insert(restored.agent_type, now + TICK);
    retry_project_deferrals(
        &controller,
        &ScanTrigger::SourceRefresh {
            agents: vec![restored.agent_type.slug()],
            project_deferral_replays: Arc::default(),
        },
        &described.superseded_project_deferrals.unwrap(),
    );
    assert!(controller.source_refreshes().take_due(now).is_empty());
    assert!(controller.take_pending_trigger().0.is_none());

    // The source refresh requeues the missing evidence, and the worker settles
    // that evidence before the next scan can run.
    let claim = store
        .claim_next_evidence(&[key.agent.as_str()], unix_now(), 60)
        .unwrap()
        .unwrap();
    assert_eq!(claim.key, key);
    store
        .lock()
        .execute(
            "UPDATE session_evidence
         SET status = 'ready', analyzed_generation = ?4, last_error = NULL,
             claimed_at_epoch = NULL, lease_expires_at_epoch = NULL
         WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            rusqlite::params![
                key.environment_key,
                key.agent,
                key.session_id,
                claim.source_generation,
            ],
        )
        .unwrap();
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().status,
        crate::store::EvidenceStatus::Ready
    );
    assert!(
        !store
            .has_local_evidence_source_refresh_targets(&key.agent)
            .unwrap()
    );

    controller.request_source_refresh(AgentKind::Codex);
    let due = controller.source_refreshes().take_due(now + TICK);
    let eligible = eligible_source_refresh_agents(
        &store,
        due.iter().map(|(agent, keys)| (*agent, !keys.is_empty())),
    )
    .unwrap();
    assert_eq!(eligible, BTreeSet::from([restored.agent_type]));
    let mut replay_policy = policy(&store, &home, None);
    replay_policy.cutoff = None;
    replay_policy.source_refresh = Some(Arc::new(due.into_values().flatten().collect()));
    let unrelated = fixture_log(&home, "22222222-2222-4222-8222-222222222222");
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![unrelated, restored])).await.unwrap();
    drop(sender);
    let mut admission = writer(&store);
    let replay = run(finite, live, None, &replay_policy, &mut admission).await;
    admission.finish().unwrap();
    assert!(replay.superseded_project_deferrals.is_none());
    assert_eq!(replay.records.len(), 1);
    assert_eq!(replay.records[0].key, key);
    assert_eq!(
        store.deferred_permission_dirs().unwrap(),
        vec![crate::dto::DeferredPermissionDir {
            dir: "Documents".to_owned(),
            path_count: 1,
        }]
    );
}

async fn index_source(store: &Store, home: &std::path::Path, log: &SessionLog) -> SessionKey {
    let DescribeOutcome::Session(record) = describe_one(log.clone(), home, None).await else {
        panic!("fixture file was not admitted");
    };
    store
        .upsert_sessions(std::slice::from_ref(&record), &[log.agent_type.slug()])
        .unwrap();
    record.key
}

fn fail_changed(store: &Store, key: &SessionKey) {
    let claim = store
        .claim_next_evidence(&[key.agent.as_str()], 100, 60)
        .unwrap()
        .unwrap();
    assert_eq!(&claim.key, key);
    assert!(
        store
            .fail_evidence(
                &claim,
                crate::store::EvidenceFailure::Retry {
                    next_attempt_at_epoch: 400,
                    counts_as_attempt: true,
                },
                crate::insights_worker::EVIDENCE_ERROR_SOURCE_CHANGED,
            )
            .unwrap()
    );
}

fn fingerprint(store: &Store, key: &SessionKey) -> String {
    store
        .session(key)
        .unwrap()
        .unwrap()
        .source_fingerprint
        .unwrap()
}

async fn replace_same_size(log: &SessionLog) -> String {
    let SessionSource::File(path) = &log.source else {
        unreachable!();
    };
    let original = tokio::fs::read_to_string(path).await.unwrap();
    let changed = original.replace("Repair the fixture", "Update the fixture");
    assert_ne!(changed, original);
    assert_eq!(changed.len(), original.len());
    let replacement = path.with_extension("tmp");
    tokio::fs::write(&replacement, changed).await.unwrap();
    tokio::fs::remove_file(path).await.unwrap();
    tokio::fs::rename(replacement, path).await.unwrap();
    let read = session_log_read(log).await.unwrap();
    antiburn_local::discovery::FingerprintInputs {
        stat: read.stat.unwrap(),
        head_hash: read.head_hash,
    }
    .fingerprint()
}
