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
    policy.refresh_changed_sources = true;

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
