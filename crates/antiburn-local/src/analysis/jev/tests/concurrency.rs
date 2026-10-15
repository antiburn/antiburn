use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

struct SignalledCheck {
    prepared: tokio::sync::mpsc::UnboundedSender<usize>,
    index: usize,
}

impl JevCheck for SignalledCheck {
    type Prepared = Value;
    type Result = usize;

    fn id(&self) -> &'static str {
        ManyItemsCheck.id()
    }

    fn revisions(&self) -> JevCheckRevisions {
        ManyItemsCheck.revisions()
    }

    fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan<Value>, JevError> {
        self.prepared.send(self.index).unwrap();
        ManyItemsCheck.prepare(context)
    }

    fn reduce(
        &self,
        plan: &JevCheckPlan<Value>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<usize, JevError> {
        ManyItemsCheck.reduce(plan, results, complete)
    }
}

struct ActiveCall(Arc<AtomicUsize>);

impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn verify_admission_and_execution(cancel: bool) {
    let (prepared, mut preparation) = tokio::sync::mpsc::unbounded_channel();
    let (entered, mut entries) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut runs = tokio::task::JoinSet::new();
    for index in 0..4 {
        let check = SignalledCheck {
            prepared: prepared.clone(),
            index,
        };
        let entered = entered.clone();
        let release = release.clone();
        let active = active.clone();
        let peak = peak.clone();
        runs.spawn(async move {
            let context = JevSessionContext {
                input_revision: format!("parallel-input-{index}"),
                session_identity: format!("parallel-session-{index}"),
                check_context: Value::Null,
                limitations: Vec::new(),
                evidence_store: JevEvidenceStore::default(),
                reference_snapshots: Vec::new(),
            };
            let outcome = run_jev_check(
                &check,
                &context,
                JevRunProgress::default(),
                |batch| {
                    let entered = entered.clone();
                    let release = release.clone();
                    let active = active.clone();
                    let peak = peak.clone();
                    async move {
                        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        let _call = ActiveCall(active);
                        peak.fetch_max(count, Ordering::SeqCst);
                        entered.send(index).unwrap();
                        release.acquire().await.unwrap().forget();
                        Ok(response_for(&batch.request))
                    }
                },
                |_| Ok(()),
            )
            .await
            .unwrap();
            assert!(outcome.complete);
            assert_eq!(outcome.result, 70);
        });
    }
    let mut prepared_indices = BTreeSet::new();
    for _ in 0..4 {
        prepared_indices.insert(preparation.recv().await.unwrap());
    }
    assert_eq!(prepared_indices.len(), 4);
    let mut executing_indices = BTreeSet::new();
    for _ in 0..MAX_PARALLEL_REQUESTS {
        executing_indices.insert(entries.recv().await.unwrap());
    }
    assert_eq!(executing_indices.len(), 4);
    assert_eq!(active.load(Ordering::SeqCst), 16);
    assert_eq!(request_execution_slots().available_permits(), 0);

    let fifth_calls = Arc::new(AtomicUsize::new(0));
    let fifth = EmptyCountingCheck {
        prepare_calls: fifth_calls.clone(),
    };
    let context = JevSessionContext {
        input_revision: "waiting".into(),
        session_identity: "waiting".into(),
        check_context: Value::Null,
        limitations: Vec::new(),
        evidence_store: JevEvidenceStore::default(),
        reference_snapshots: Vec::new(),
    };
    let waiting = run_jev_check(
        &fifth,
        &context,
        JevRunProgress::default(),
        |_| async { unreachable!() },
        |_| Ok(()),
    );
    tokio::pin!(waiting);
    std::future::poll_fn(|cx| {
        assert!(waiting.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(fifth_calls.load(Ordering::SeqCst), 0);

    if cancel {
        runs.abort_all();
        while let Some(result) = runs.join_next().await {
            assert!(result.unwrap_err().is_cancelled());
        }
    } else {
        release.add_permits(1024);
        while let Some(result) = runs.join_next().await {
            result.unwrap();
        }
    }
    assert!(waiting.await.unwrap().complete);
    assert_eq!(fifth_calls.load(Ordering::SeqCst), 1);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(peak.load(Ordering::SeqCst), 16);
    assert_eq!(request_execution_slots().available_permits(), 16);
    assert_eq!(orchestration_slot().available_permits(), 4);
}

#[tokio::test]
async fn four_assessments_admit_before_preparation_and_share_sixteen_execute_slots() {
    tokio::time::timeout(
        Duration::from_secs(10),
        verify_admission_and_execution(false),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn cancellation_releases_assessment_and_global_execute_permits() {
    tokio::time::timeout(
        Duration::from_secs(10),
        verify_admission_and_execution(true),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn cancelling_admission_waiter_does_not_prepare_or_consume_a_permit() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let held = orchestration_slot().acquire_many(4).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let check = EmptyCountingCheck {
            prepare_calls: calls.clone(),
        };
        let context = JevSessionContext {
            input_revision: "cancelled-waiter".into(),
            session_identity: "cancelled-waiter".into(),
            check_context: Value::Null,
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        {
            let waiting = run_jev_check(
                &check,
                &context,
                JevRunProgress::default(),
                |_| async { unreachable!() },
                |_| Ok(()),
            );
            tokio::pin!(waiting);
            std::future::poll_fn(|cx| {
                assert!(waiting.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(held);
        let all = orchestration_slot().acquire_many(4).await.unwrap();
        drop(all);
        assert_eq!(orchestration_slot().available_permits(), 4);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cancelling_global_execute_waiter_does_not_call_provider_or_leak_capacity() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let held = request_execution_slots().acquire_many(16).await.unwrap();
        let limits = capabilities::ModelCapabilities::jev_default();
        let items = vec![JevWorkItem {
            id: "waiting".into(),
            window: JevInputWindow {
                fields: json!({"text": "complete evidence"}),
                evidence: Vec::new(),
            },
            questions: BTreeMap::from([("decision".into(), choice())]),
        }];
        let calls = AtomicUsize::new(0);
        let execute = |batch: Arc<JevRequestBatch>| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(response_for(&batch.request)) }
        };
        {
            let waiting = execute_batches(
                &execute,
                pack_work_items_with_capabilities(&items, &limits).batches,
                &limits,
                |_, _| Ok(()),
            );
            tokio::pin!(waiting);
            for _ in 0..2 {
                std::future::poll_fn(|cx| {
                    assert!(waiting.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(held);
        let all = request_execution_slots().acquire_many(16).await.unwrap();
        drop(all);
        assert_eq!(request_execution_slots().available_permits(), 16);
    })
    .await
    .unwrap();
}
