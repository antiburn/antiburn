use super::*;

async fn assert_pass_panic_is_settled(during_poll: bool) {
    let store = store();
    let mut broken = record("panic-first");
    broken.updated_at_epoch = Some(200);
    store
        .upsert_sessions(
            &[broken.clone(), record("stable-next")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let runner = |record: &SessionRecord, _: PassSignal, _: i64| -> PassFuture {
        if record.key == broken.key {
            assert!(during_poll, "synthetic runner construction panic");
            return Box::pin(async { panic!("synthetic runner poll panic") });
        }
        let pass = published_pass(record);
        Box::pin(async move { pass })
    };
    let announced = Mutex::new(Vec::new());
    let announce = |key: &SessionKey| announced.lock().unwrap().push(key.clone());
    assert!(
        process_next(&store, &|| 100, &runner, &announce, &|_, _| {}, &|| {})
            .await
            .unwrap()
    );
    let row = store.evidence(&broken.key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Pending);
    assert_eq!(row.retry_count, 1);
    assert_eq!(row.last_error.as_deref(), Some("worker-pass-panicked"));
    assert_eq!(row.next_attempt_at_epoch, Some(100 + BACKOFF_BASE_SECS));
    assert_eq!(announced.lock().unwrap().as_slice(), &[broken.key.clone()]);

    assert!(
        process_next(&store, &|| 100, &runner, &announce, &|_, _| {}, &|| {})
            .await
            .unwrap()
    );
    let stable = SessionKey::new("native", "claude-code", "stable-next");
    assert_eq!(
        store.evidence(&stable).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );
    assert_eq!(announced.lock().unwrap().last(), Some(&stable));
}

#[tokio::test]
async fn runner_construction_panic_retries_and_does_not_block_the_next_session() {
    assert_pass_panic_is_settled(false).await;
}

#[tokio::test]
async fn runner_poll_panic_retries_and_does_not_block_the_next_session() {
    assert_pass_panic_is_settled(true).await;
}

#[tokio::test]
async fn a_panicked_local_pass_waits_for_its_blocking_writer_before_recovery() {
    let store = store();
    let record = record("writer-panic");
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let (release, released) = std::sync::mpsc::channel();
    let released = Arc::new(Mutex::new(Some(released)));
    let entered = Arc::new(AtomicBool::new(false));
    let writer_done = Arc::new(AtomicBool::new(false));
    let task_store = store.clone();
    let task_entered = Arc::clone(&entered);
    let task_writer_done = Arc::clone(&writer_done);
    let announced = Arc::new(AtomicUsize::new(0));
    let task_announced = Arc::clone(&announced);
    let task = tokio::spawn(async move {
        let runner_store = task_store.clone();
        let writer_finished = Arc::clone(&task_writer_done);
        let analyzer = move |_: AgentKind,
                             _: String,
                             _: Option<String>,
                             _: analysis::ClaimedSource,
                             signal: PassSignal,
                             writer: Option<Arc<dyn TurnRowStore>>,
                             _: Option<String>|
              -> PassFuture {
            let released = released.lock().unwrap().take().unwrap();
            let entered = Arc::clone(&task_entered);
            let done = Arc::clone(&writer_finished);
            signal.spawn_parser(move || {
                let writer = writer.unwrap();
                entered.store(true, Ordering::SeqCst);
                released.recv().unwrap();
                writer.query_turn_facts().unwrap();
                done.store(true, Ordering::SeqCst);
            });
            panic!("synthetic construction panic after starting a writer");
        };
        let runner = move |record: &SessionRecord, signal: PassSignal, fence: i64| {
            run_record_pass_with(record, signal, fence, runner_store.clone(), &analyzer)
        };
        process_next(
            &task_store,
            &|| 100,
            &runner,
            &|_| {
                assert!(task_writer_done.load(Ordering::SeqCst));
                task_announced.fetch_add(1, Ordering::SeqCst);
            },
            &|_, _| {},
            &|| {},
        )
        .await
        .unwrap()
    });
    wait_until(|| entered.load(Ordering::SeqCst)).await;
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Processing
    );
    assert_eq!(announced.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    assert!(task.await.unwrap());
    assert_eq!(announced.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[tokio::test]
async fn the_shared_pass_boundary_waits_for_a_remote_writer_after_a_poll_panic() {
    let store = store();
    let mut record = record("remote-writer-panic");
    record.key = SessionKey::for_origin(
        "claude-code",
        "remote-writer-panic",
        None,
        Some("synthetic-host"),
    )
    .unwrap();
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let (release, released) = std::sync::mpsc::channel();
    let released = Arc::new(Mutex::new(Some(released)));
    let entered = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let announced = Arc::new(AtomicBool::new(false));
    let runner_store = store.clone();
    let task_store = store.clone();
    let task_entered = Arc::clone(&entered);
    let task_finished = Arc::clone(&finished);
    let task_announced = Arc::clone(&announced);
    let task = tokio::spawn(async move {
        let runner = move |record: &SessionRecord, signal: PassSignal, fence: i64| {
            let released = released.lock().unwrap().take().unwrap();
            let entered = Arc::clone(&task_entered);
            let finished = Arc::clone(&task_finished);
            let writer: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
                runner_store.clone(),
                record.key.clone(),
                fence,
            ));
            run_pass_with_writer(signal.clone(), writer, |writer| {
                Box::pin(async move {
                    signal.spawn_parser(move || {
                        entered.store(true, Ordering::SeqCst);
                        released.recv().unwrap();
                        let pass = analysis::evidence_pass_with_turn_rows(&[SessionInput {
                            agent: "claude".into(), session_id: "remote-writer-panic".into(),
                            source: RawSource::Jsonl(concat!(
                                r#"{"type":"assistant","timestamp":100,"message":{"id":"late-message","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}"#,
                                "\n",
                            ).into()),
                            source_format: Default::default(), fork_parent_session_id: None,
                        }], &|| false, Some(writer));
                        assert_eq!(pass.outcome, PassOutcome::Published);
                        finished.store(true, Ordering::SeqCst);
                    });
                    panic!("synthetic poll panic after starting a remote writer");
                })
            })
        };
        process_next(
            &task_store,
            &|| 100,
            &runner,
            &|_| {
                assert!(finished.load(Ordering::SeqCst));
                task_announced.store(true, Ordering::SeqCst);
            },
            &|_, _| {},
            &|| {},
        )
        .await
        .unwrap()
    });
    wait_until(|| entered.load(Ordering::SeqCst)).await;
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Processing
    );
    assert!(!announced.load(Ordering::SeqCst));
    release.send(()).unwrap();
    assert!(task.await.unwrap());
    assert!(announced.load(Ordering::SeqCst));
    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(
        store
            .count_turn_rows_for_session(&record.key, evidence.claim_fence)
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn dropping_a_pass_boundary_cancels_its_blocking_parser() {
    let signal = PassSignal::new();
    let parser_signal = signal.clone();
    let entered = Arc::new(AtomicBool::new(false));
    let task_entered = Arc::clone(&entered);
    let (finished, mut completion) = tokio::sync::oneshot::channel();
    let writer: Arc<dyn TurnRowStore> = MemoryTurnRowStore::new("claude", "cancel-parser");
    let pass = run_pass_with_writer(signal.clone(), writer, |writer| {
        tokio::task::spawn_blocking(move || {
            task_entered.store(true, Ordering::SeqCst);
            while !parser_signal.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            writer.query_turn_facts().unwrap();
            finished.send(()).unwrap();
        });
        Box::pin(std::future::pending())
    });
    wait_until(|| entered.load(Ordering::SeqCst)).await;
    assert!(!signal.cancelled());
    assert!(matches!(
        completion.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    drop(pass);
    assert!(signal.cancelled());
    tokio::time::timeout(Duration::from_secs(1), completion)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn an_unused_writer_clone_does_not_hold_the_evidence_claim() {
    let signal = PassSignal::new();
    let writer: Arc<dyn TurnRowStore> = MemoryTurnRowStore::new("claude", "held-writer");
    let _held_writer = writer.clone();
    let pass = run_pass_with_writer(signal, writer, |_| {
        Box::pin(async { analysis::unsupported_evidence_pass() })
    });
    let pass = tokio::time::timeout(Duration::from_secs(1), pass)
        .await
        .unwrap();
    assert_eq!(pass.outcome, PassOutcome::Unsupported);
}

#[tokio::test]
async fn a_remote_cache_pass_uses_the_shared_writer_boundary_and_publishes() {
    use antiburn_local::discovery::source_version::{FingerprintInputs, SourceStat, head_hash_of};

    let temporary = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(temporary.path()).unwrap();
    let id = "remote-boundary";
    let data = concat!(
        r#"{"type":"assistant","timestamp":100,"message":{"id":"remote-message","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}"#,
        "\n",
    );
    let manifest = antiburn_remote::export::BundleManifest {
        session: antiburn_remote::RemoteSession {
            agent: "claude-code".into(),
            session_id: id.into(),
            title: "Synthetic remote session".into(),
            cwd: None,
            surface: "cli".into(),
            updated_at: Some(100),
        },
        files: vec![antiburn_remote::export::BundleFile {
            size: data.len() as u64,
            version: "synthetic".into(),
            subagent_id: None,
            label: None,
            sidecar_for: None,
            fork_parent: false,
        }],
        fork_parent_session_id: None,
    };
    let path = temporary.path().join(manifest.file_name(0));
    std::fs::write(&path, data).unwrap();
    std::fs::write(
        temporary.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let mut remote = record(id);
    remote.key = SessionKey::for_origin("claude-code", id, None, Some("synthetic-host")).unwrap();
    remote.source_label = path.to_string_lossy().into_owned();
    remote.source_fingerprint = Some(
        FingerprintInputs {
            stat: SourceStat::from_open_std_file(&std::fs::File::open(path).unwrap()).unwrap(),
            head_hash: Some(head_hash_of(data.as_bytes())),
        }
        .fingerprint(),
    );
    store
        .upsert_sessions(
            std::slice::from_ref(&remote),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let signal = Arc::new(Mutex::new(None::<PassSignal>));
    let runner_store = store.clone();
    let runner = |record: &SessionRecord, pass_signal: PassSignal, fence: i64| {
        *signal.lock().unwrap() = Some(pass_signal.clone());
        crate::remote_cache::run_pass(record.clone(), pass_signal, fence, runner_store.clone())
    };
    assert!(
        process_next(&store, &|| 100, &runner, &|_| {}, &|_, _| {}, &|| {})
            .await
            .unwrap()
    );
    let evidence = store.evidence(&remote.key).unwrap().unwrap();
    assert_eq!(
        evidence.status,
        EvidenceStatus::Ready,
        "last error: {:?}",
        evidence.last_error
    );
    assert!(signal.lock().unwrap().as_ref().unwrap().cancelled());
}

#[tokio::test]
async fn publication_runs_real_blocking_client_and_runtime_construction_off_the_async_thread() {
    let store = store();
    let claim = claim(&store, "blocking-publication", 100);
    let record = store.session(&claim.key).unwrap().unwrap();
    let mut pass = published_pass(&record);
    pass.analysis.analyzed_generation = claim.source_generation;
    let outcome = apply_outcome_blocking(&store, &claim, pass, 100, |store, claim, pass, now| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        drop(client);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {});
        drop(runtime);
        apply_outcome(store, claim, pass, now)
    })
    .await
    .unwrap();
    assert!(outcome.applied);
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );
}

#[tokio::test]
async fn a_publication_task_panic_after_commit_keeps_accepted_evidence() {
    let store = store();
    let claim = claim(&store, "commit-before-panic", 100);
    let record = store.session(&claim.key).unwrap().unwrap();
    let mut pass = published_pass(&record);
    pass.analysis.analyzed_generation = claim.source_generation;
    assert!(
        apply_outcome_blocking(&store, &claim, pass, 100, |store, claim, pass, now| {
            assert!(apply_outcome(store, claim, pass, now)?.applied);
            panic!("synthetic publication panic after commit");
        })
        .await
        .is_err()
    );
    let accepted = store.evidence(&claim.key).unwrap().unwrap();
    let analysis = store.analysis(&claim.key).unwrap().unwrap();
    let announced = AtomicBool::new(false);
    let published = AtomicBool::new(false);
    assert!(
        settle_failed_claim(
            &store,
            &claim,
            100,
            "worker-publication-failed",
            true,
            &|_| {
                announced.store(true, Ordering::SeqCst);
            },
            &|| published.store(true, Ordering::SeqCst)
        )
        .await
        .unwrap()
    );
    assert!(announced.load(Ordering::SeqCst));
    assert!(published.load(Ordering::SeqCst));
    assert_eq!(store.evidence(&claim.key).unwrap().unwrap(), accepted);
    assert_eq!(store.analysis(&claim.key).unwrap().unwrap(), analysis);
}

#[tokio::test]
async fn a_first_item_error_announces_idle_without_a_processed_row() {
    let store = store();
    store
        .upsert_sessions(&[record("first-error")], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_worker_retry BEFORE UPDATE OF status ON session_evidence
         WHEN new.status = 'pending' BEGIN SELECT RAISE(FAIL, 'synthetic retry failure'); END;",
        )
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let states = Arc::new(Mutex::new(Vec::new()));
    let reports = Arc::new(AtomicUsize::new(0));
    let task_handle = Arc::clone(&handle);
    let task_states = Arc::clone(&states);
    let task_reports = Arc::clone(&reports);
    let task = tokio::spawn(async move {
        let runner = |_: &SessionRecord, _: PassSignal, _: i64| {
            Box::pin(async { failed_pass(PassOutcome::Published) }) as PassFuture
        };
        worker_loop(
            &store,
            &task_handle,
            &|| 100,
            &runner,
            &|_| {},
            &WorkerLoopSignals {
                report_changed: &|| {
                    task_reports.fetch_add(1, Ordering::SeqCst);
                },
                backlog: &|active| task_states.lock().unwrap().push(active),
                gate: &always_open_gate,
            },
            &|_, _| {},
        )
        .await;
    });
    wait_until(|| states.lock().unwrap().len() == 2).await;
    assert_eq!(*states.lock().unwrap(), vec![true, false]);
    assert!(!handle.backlog_active());
    assert_eq!(handle.backlog.lock().unwrap().processed, 0);
    assert!(reports.load(Ordering::SeqCst) > 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[derive(Default)]
struct LearningBatchState {
    backlog: Mutex<Vec<bool>>,
    publication_batches: AtomicUsize,
    announcements: AtomicUsize,
}

fn spawn_learning_worker(
    store: Store,
    handle: Arc<WorkerHandle>,
    gate: FirstRunGate,
    state: Arc<LearningBatchState>,
    abort_first: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let runner = move |record: &SessionRecord, _: PassSignal, _: i64| {
            if record.key.session_id == "learning-failed-first" {
                return Box::pin(async move {
                    if abort_first {
                        std::future::pending::<()>().await;
                    }
                    failed_pass(PassOutcome::Published)
                }) as PassFuture;
            }
            let pass = published_pass(record);
            Box::pin(async move { pass }) as PassFuture
        };
        worker_loop_with_reports(
            &store,
            &handle,
            &|| 100,
            &runner,
            &|_| {
                state.announcements.fetch_add(1, Ordering::SeqCst);
            },
            &WorkerLoopSignals {
                report_changed: &|| {},
                backlog: &|active| state.backlog.lock().unwrap().push(active),
                gate: &always_open_gate,
            },
            &WorkerLoopReports {
                ingested: &|_, _| {},
                publication_drained: &|generation| {
                    state.publication_batches.fetch_add(1, Ordering::SeqCst);
                    gate.mark_turns_published(generation);
                },
                generation: &|| gate.generation(),
            },
        )
        .await;
    })
}

async fn assert_learning_waits_for_an_accepted_publication(abort_first: bool) {
    let store = store();
    store
        .upsert_sessions(
            &[record("learning-failed-first")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let gate = FirstRunGate::new(false);
    gate.advance(FirstRunStage::Checks);
    let state = Arc::new(LearningBatchState::default());
    let mut task = spawn_learning_worker(
        store.clone(),
        Arc::clone(&handle),
        gate.clone(),
        Arc::clone(&state),
        abort_first,
    );
    if abort_first {
        wait_until(|| handle.backlog_active()).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        task = spawn_learning_worker(
            store.clone(),
            Arc::clone(&handle),
            gate.clone(),
            Arc::clone(&state),
            false,
        );
    } else {
        wait_until(|| state.backlog.lock().unwrap().len() == 2).await;
        assert_eq!(state.announcements.load(Ordering::SeqCst), 1);
    }
    assert_eq!(*state.backlog.lock().unwrap(), vec![true, false]);
    assert_eq!(state.publication_batches.load(Ordering::SeqCst), 0);
    assert!(!gate.limit_learning_ready());

    let accepted = record("learning-accepted-next");
    store
        .upsert_sessions(
            std::slice::from_ref(&accepted),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    handle.wake.notify_one();
    wait_until(|| gate.limit_learning_ready() && !handle.backlog_active()).await;
    assert_eq!(
        store.evidence(&accepted.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );
    assert_eq!(
        *state.backlog.lock().unwrap(),
        vec![true, false, true, false]
    );
    assert_eq!(state.publication_batches.load(Ordering::SeqCst), 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn a_failed_first_batch_keeps_learning_closed_until_an_accepted_publication_drains() {
    assert_learning_waits_for_an_accepted_publication(false).await;
}

#[tokio::test]
async fn an_aborted_first_batch_keeps_learning_closed_until_an_accepted_publication_drains() {
    assert_learning_waits_for_an_accepted_publication(true).await;
}

#[tokio::test]
async fn a_busy_old_publication_cannot_enable_learning_after_reset_and_a_failed_batch() {
    let store = store();
    let old = record("learning-before-reset");
    store
        .upsert_sessions(
            std::slice::from_ref(&old),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let gate = FirstRunGate::new(false);
    gate.advance(FirstRunStage::Checks);
    let old_generation = gate.generation();
    let gate_calls = Arc::new(AtomicUsize::new(0));
    let blocked = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let drained = Arc::new(Mutex::new(Vec::new()));
    let states = Arc::new(Mutex::new(Vec::new()));
    let task_store = store.clone();
    let task_handle = Arc::clone(&handle);
    let task_gate = gate.clone();
    let task_gate_calls = Arc::clone(&gate_calls);
    let task_blocked = Arc::clone(&blocked);
    let task_release = Arc::clone(&release);
    let task_drained = Arc::clone(&drained);
    let task_states = Arc::clone(&states);
    let task = tokio::spawn(async move {
        let runner_gate_calls = Arc::clone(&task_gate_calls);
        let runner = move |record: &SessionRecord, _: PassSignal, _: i64| {
            if record.key.session_id == "learning-failed-first" {
                assert_eq!(runner_gate_calls.load(Ordering::SeqCst), 3);
                return Box::pin(async { failed_pass(PassOutcome::Published) }) as PassFuture;
            }
            let pass = published_pass(record);
            Box::pin(async move { pass }) as PassFuture
        };
        let wait_gate = task_gate.clone();
        let wait_for_gate = move || -> GateFuture {
            let call = task_gate_calls.fetch_add(1, Ordering::SeqCst);
            let gate = wait_gate.clone();
            let blocked = Arc::clone(&task_blocked);
            let release = Arc::clone(&task_release);
            Box::pin(async move {
                gate.wait_until(FirstRunStage::Checks, || false).await;
                if call == 1 {
                    blocked.notify_one();
                    release.notified().await;
                }
            })
        };
        worker_loop_with_reports(
            &task_store,
            &task_handle,
            &|| 100,
            &runner,
            &|_| {},
            &WorkerLoopSignals {
                report_changed: &|| {},
                backlog: &|active| task_states.lock().unwrap().push(active),
                gate: &wait_for_gate,
            },
            &WorkerLoopReports {
                ingested: &|_, _| {},
                publication_drained: &|generation| {
                    task_drained.lock().unwrap().push(generation);
                    task_gate.mark_turns_published(generation);
                },
                generation: &|| task_gate.generation(),
            },
        )
        .await;
    });
    tokio::time::timeout(Duration::from_secs(1), blocked.notified())
        .await
        .unwrap();
    assert_eq!(
        store.evidence(&old.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );
    assert!(handle.backlog_active());
    assert!(!gate.limit_learning_ready());

    gate.reset();
    store.clear_local_session_data().unwrap();
    assert!(store.evidence(&old.key).unwrap().is_none());
    store
        .upsert_sessions(
            &[record("learning-failed-first")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    gate.advance(FirstRunStage::Checks);
    release.notify_one();
    wait_until(|| !handle.backlog_active() && !drained.lock().unwrap().is_empty()).await;
    assert_eq!(*drained.lock().unwrap(), vec![old_generation]);
    assert_eq!(*states.lock().unwrap(), vec![true, false]);
    assert!(!gate.limit_learning_ready());
    let failed = store
        .evidence(&record("learning-failed-first").key)
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, EvidenceStatus::Pending);
    assert_eq!(
        failed.last_error.as_deref(),
        Some("worker-publication-failed")
    );

    let fresh = record("learning-after-reset");
    store
        .upsert_sessions(
            std::slice::from_ref(&fresh),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    handle.wake.notify_one();
    wait_until(|| gate.limit_learning_ready() && !handle.backlog_active()).await;
    assert_eq!(
        store.evidence(&fresh.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );
    assert_eq!(
        *drained.lock().unwrap(),
        vec![old_generation, gate.generation()]
    );
    assert_eq!(*states.lock().unwrap(), vec![true, false, true, false]);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn late_old_publication_and_drain_preserve_the_current_generation_marker() {
    let store = store();
    let handle = WorkerHandle::default();
    let gate = FirstRunGate::new(false);
    gate.advance(FirstRunStage::Checks);
    let old_generation = gate.generation();
    let drained = Mutex::new(Vec::new());
    let publication_drained = |generation| {
        drained.lock().unwrap().push(generation);
        gate.mark_turns_published(generation);
    };
    let signals = WorkerLoopSignals {
        report_changed: &|| {},
        backlog: &|_| {},
        gate: &always_open_gate,
    };
    let activity = BacklogActivity {
        busy: AtomicBool::new(false),
        handle: &handle,
        signals: &signals,
        publication_drained: &publication_drained,
    };
    let first = claim(&store, "late-old-publication", 100);
    activity.claimed(&store);
    let mut first_pass = published_pass(&store.session(&first.key).unwrap().unwrap());
    first_pass.analysis.analyzed_generation = first.source_generation;
    assert!(
        apply_outcome_blocking(&store, &first, first_pass, 100, apply_outcome)
            .await
            .unwrap()
            .applied
    );
    handle.note_backlog_published(old_generation);

    gate.reset();
    store.clear_local_session_data().unwrap();
    gate.advance(FirstRunStage::Checks);
    let current_generation = gate.generation();
    publication_drained(old_generation);
    assert!(!gate.limit_learning_ready());

    let current = claim(&store, "current-publication", 100);
    let mut current_pass = published_pass(&store.session(&current.key).unwrap().unwrap());
    current_pass.analysis.analyzed_generation = current.source_generation;
    assert!(
        apply_outcome_blocking(&store, &current, current_pass, 100, apply_outcome)
            .await
            .unwrap()
            .applied
    );
    handle.note_backlog_published(current_generation);
    handle.note_backlog_published(old_generation);
    assert_eq!(
        handle.backlog.lock().unwrap().published_generation,
        Some(current_generation)
    );
    publication_drained(old_generation);
    assert!(!gate.limit_learning_ready());
    assert!(activity.idle());
    assert!(gate.limit_learning_ready());
    assert_eq!(
        *drained.lock().unwrap(),
        vec![old_generation, old_generation, current_generation]
    );
    assert_eq!(handle.backlog.lock().unwrap().published_generation, None);
}

#[tokio::test]
async fn cancelling_a_busy_worker_clears_its_backlog_count_and_announces_idle() {
    let store = store();
    store
        .upsert_sessions(&[record("cancel-busy")], &crate::agents::evidence_cohort())
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let states = Arc::new(Mutex::new(Vec::new()));
    let task_handle = Arc::clone(&handle);
    let task_states = Arc::clone(&states);
    let task = tokio::spawn(async move {
        let runner = |_: &SessionRecord, _: PassSignal, _: i64| {
            Box::pin(std::future::pending()) as PassFuture
        };
        worker_loop(
            &store,
            &task_handle,
            &|| 100,
            &runner,
            &|_| {},
            &WorkerLoopSignals {
                report_changed: &|| {},
                backlog: &|active| task_states.lock().unwrap().push(active),
                gate: &always_open_gate,
            },
            &|_, _| {},
        )
        .await;
    });
    wait_until(|| handle.backlog_active()).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!handle.backlog_active());
    assert_eq!(*states.lock().unwrap(), vec![true, false]);
}

#[tokio::test]
async fn a_supervisor_replaces_a_failed_worker_and_aborts_its_child_on_shutdown() {
    struct ChildLifetime(Arc<AtomicBool>);
    impl Drop for ChildLifetime {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let store = store();
    let stable = record("replacement-stable");
    store
        .upsert_sessions(
            std::slice::from_ref(&stable),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let published = Arc::new(Notify::new());
    let task_starts = Arc::clone(&starts);
    let task_dropped = Arc::clone(&dropped);
    let task_published = Arc::clone(&published);
    let task_store = store.clone();
    let supervisor = tokio::spawn(supervision::run("synthetic", move || {
        let start = task_starts.fetch_add(1, Ordering::SeqCst);
        let store = task_store.clone();
        let dropped = Arc::clone(&task_dropped);
        let published = Arc::clone(&task_published);
        async move {
            assert_ne!(start, 0, "synthetic first worker panic");
            let _lifetime = ChildLifetime(dropped);
            let runner = |record: &SessionRecord, _: PassSignal, _: i64| {
                let pass = published_pass(record);
                Box::pin(async move { pass }) as PassFuture
            };
            assert!(
                process_next(&store, &|| 100, &runner, &|_| {}, &|_, _| {}, &|| {})
                    .await
                    .unwrap()
            );
            published.notify_one();
            std::future::pending::<()>().await;
        }
    }));
    tokio::time::timeout(Duration::from_secs(3), published.notified())
        .await
        .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(
        store.evidence(&stable.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );
    supervisor.abort();
    assert!(supervisor.await.unwrap_err().is_cancelled());
    wait_until(|| dropped.load(Ordering::SeqCst)).await;
}

#[tokio::test(start_paused = true)]
async fn repeated_worker_failures_use_a_capped_restart_delay() {
    let starts = Arc::new(AtomicUsize::new(0));
    let (attempts, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let task_starts = Arc::clone(&starts);
    let supervisor = tokio::spawn(supervision::run("synthetic-restart", move || {
        let count = task_starts.fetch_add(1, Ordering::SeqCst) + 1;
        attempts.send(count).unwrap();
        async { panic!("synthetic repeated worker failure") }
    }));
    assert_eq!(observed.recv().await, Some(1));
    for (index, delay) in [1, 2, 4, 8, 16, 32, 60, 60].into_iter().enumerate() {
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(delay - 1)).await;
        assert_eq!(starts.load(Ordering::SeqCst), index + 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(observed.recv().await, Some(index + 2));
    }
    supervisor.abort();
    assert!(supervisor.await.unwrap_err().is_cancelled());
}
