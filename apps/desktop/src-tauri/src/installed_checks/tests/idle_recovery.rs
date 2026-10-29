use super::*;

struct QueryFault {
    stage: diagnostics::RuntimeStage,
    failing: AtomicBool,
    successes: AtomicU64,
    changed: Notify,
}

impl QueryFault {
    fn query<T>(
        &self,
        stage: diagnostics::RuntimeStage,
        query: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        if stage != self.stage {
            return query();
        }
        if self.failing.load(Ordering::Acquire) {
            return diagnostics::worker_result(
                stage,
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some("PRIVATE_IDLE_QUERY_ERROR".into()),
                )
                .into()),
            );
        }
        let result = query();
        if result.is_ok() {
            self.successes.fetch_add(1, Ordering::AcqRel);
            self.changed.notify_one();
        }
        result
    }
}

struct FaultyQueries<'a> {
    host: &'a InstalledCheckHost<ScriptedEvaluator>,
    fault: &'a QueryFault,
}

impl IdleQueries for FaultyQueries<'_> {
    fn next_idle_deadline(&self) -> Result<Option<i64>> {
        self.fault
            .query(diagnostics::RuntimeStage::IdleDeadline, || {
                self.host.next_idle_deadline()
            })
    }

    fn request_idle_catch_up(&self, now_ms: i64) -> Result<usize> {
        self.fault
            .query(diagnostics::RuntimeStage::IdleCatchUp, || {
                self.host.request_idle_catch_up(now_ms)
            })
    }
}

async fn wait_until(changed: &Notify, ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            changed.notified().await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn idle_query_retry_delay_starts_when_the_failed_query_returns() {
    let mut query = IdleQuery::default();
    let mut failed_at = None;
    assert!(
        query
            .run::<()>(diagnostics::RuntimeStage::IdleDeadline, || {
                std::thread::sleep(Duration::from_millis(10));
                failed_at = Some(tokio::time::Instant::now());
                Err(anyhow::anyhow!("synthetic idle query failure"))
            })
            .is_none()
    );
    assert!(query.retry_at.unwrap() >= failed_at.unwrap() + WORKER_POLL_INTERVAL);
}

#[tokio::test]
async fn production_worker_turns_keep_assessments_and_sources_running_during_idle_query_failures() {
    for stage in [
        diagnostics::RuntimeStage::IdleDeadline,
        diagnostics::RuntimeStage::IdleCatchUp,
    ] {
        let fixture = Fixture::new();
        let mut host = fixture.host();
        host.reconcile(&Admission::Ready).unwrap();
        fixture.publish("main", 3);
        fixture.evaluator.answer("stuck");
        fixture.evaluator.answer("clear");
        fixture.evaluator.wait.store(true, Ordering::Release);
        let state = Arc::new(InstalledChecks::default());
        let fault = Arc::new(QueryFault {
            stage,
            failing: AtomicBool::new(false),
            successes: AtomicU64::new(0),
            changed: Notify::new(),
        });
        let progress = Arc::new(Notify::new());
        let events = crate::session_lifecycle::SessionEvents::default();
        let mut turns = WorkerTurns::new(events.subscribe());
        let worker_state = Arc::clone(&state);
        let worker_fault = Arc::clone(&fault);
        let worker_progress = Arc::clone(&progress);
        let worker = tokio::spawn(async move {
            let mut settled = Vec::new();
            let mut more = false;
            loop {
                match turns
                    .next(
                        &FaultyQueries {
                            host: &host,
                            fault: &worker_fault,
                        },
                        &worker_state,
                        more,
                        || true,
                    )
                    .await
                {
                    WorkerWake::Update { discover, .. } => {
                        if worker_state.stopping.load(Ordering::Acquire) {
                            break;
                        }
                        if discover {
                            host.reconcile(&Admission::Ready)?;
                        }
                        more = host.catch_up()?.1;
                        set_runtime_failure(&mut host.definitions, turns.idle_failure());
                        ensure!(
                            finish_inventory_update(
                                &worker_state,
                                &host,
                                Ok(()),
                                diagnostics::RuntimeStage::SourceCatchUp,
                                &mut None,
                                |_| {},
                            ),
                            "worker inventory update failed"
                        );
                        for job in
                            host.evaluation_jobs(EVALUATIONS.saturating_sub(turns.tasks.len()))
                        {
                            turns.tasks.spawn(job.run());
                        }
                    }
                    WorkerWake::Settled(Some(result)) => {
                        if let Some(result) = result?? {
                            settled.push(result);
                        }
                    }
                    WorkerWake::Settled(None) => {}
                    WorkerWake::Closed => anyhow::bail!("worker lifecycle closed before shutdown"),
                }
                worker_progress.notify_one();
            }
            while let Some(result) = turns.tasks.join_next().await {
                if let Some(result) = result?? {
                    settled.push(result);
                }
            }
            Ok::<_, anyhow::Error>((host, settled))
        });

        wait_until(&fixture.evaluator.started, || {
            fixture.evaluator.inputs.lock().unwrap().len() == 1
        })
        .await;
        fault.failing.store(true, Ordering::Release);
        state.wake.notify_one();
        wait_until(&progress, || {
            state.metadata.read().unwrap().iter().any(|definition| {
                definition
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "storage-failed")
            })
        })
        .await;
        let expected_stage = match stage {
            diagnostics::RuntimeStage::IdleDeadline => "idle_deadline",
            diagnostics::RuntimeStage::IdleCatchUp => "idle_catch_up",
            _ => unreachable!(),
        };
        let definitions = state.metadata.read().unwrap().clone();
        let diagnostic = definitions[0]
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "storage-failed")
            .unwrap();
        assert!(
            diagnostic.details.iter().any(|detail| {
                detail.label == "Runtime stage" && detail.value == expected_stage
            })
        );
        assert!(
            diagnostic
                .details
                .iter()
                .any(|detail| detail.label == "SQLite code" && detail.value == "5")
        );
        assert!(
            !serde_json::to_string(diagnostic)
                .unwrap()
                .contains("PRIVATE_")
        );
        let successes = fault.successes.load(Ordering::Acquire);

        fixture.publish("second", 3);
        state.wake.notify_one();
        wait_until(&fixture.evaluator.started, || {
            fixture.evaluator.inputs.lock().unwrap().len() == 2
        })
        .await;
        assert!(fault.failing.load(Ordering::Acquire));
        assert!(!worker.is_finished());
        assert_eq!(
            fixture
                .store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| { row.get::<_, usize>(0) }
                )
                .unwrap(),
            0
        );

        fault.failing.store(false, Ordering::Release);
        state.wake.notify_one();
        wait_until(&fault.changed, || {
            fault.successes.load(Ordering::Acquire) > successes
        })
        .await;
        wait_until(&progress, || {
            state.metadata.read().unwrap().iter().all(|definition| {
                definition
                    .diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.code != "storage-failed")
            })
        })
        .await;
        fixture.evaluator.wait.store(false, Ordering::Release);
        fixture.evaluator.release.notify_waiters();
        wait_until(&progress, || {
            fixture
                .store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get::<_, usize>(0),
                )
                .unwrap()
                == 2
        })
        .await;
        state.stopping.store(true, Ordering::Release);
        state.wake.notify_one();
        let (host, settled) = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(settled.len(), 2);
        assert!(
            settled
                .iter()
                .all(|result| result.outcome == RunOutcome::Accepted)
        );
        assert_eq!(status_writes(&fixture.store, &host.definitions[0]).len(), 2);
        assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 2);
    }
}
