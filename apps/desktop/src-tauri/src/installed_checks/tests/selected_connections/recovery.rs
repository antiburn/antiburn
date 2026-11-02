use super::*;

fn saved_attempts(store: &Store) -> Vec<(String, String)> {
    store
        .lock()
        .prepare("SELECT attempt_id, response_json FROM burn_check_evaluation_attempt ORDER BY attempt_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unchanged_restart_recovers_paid_answer_before_report_metadata_is_published() {
    for source_waiting in [false, true] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = Harness::new(&a, &b);
        harness.baseline(&a).await;
        let prior = harness.status();
        let gate = Arc::new(Gate::default());
        let _release = ReleaseGate(Arc::clone(&gate));
        harness.publish(2);
        let callback = Arc::clone(&gate);
        harness
            .fixture
            .store
            .set_burn_check_test_hook(Some(Arc::new(move |stage| {
                if stage == StoreStage::BeforeAcceptance {
                    callback.block();
                }
            })));
        a.reply("clear", None);
        let task = harness.job();
        gate.wait().await;
        if source_waiting {
            harness.fixture.publish("main", 3);
        } else {
            harness
                .runtime
                .installed
                .stopping
                .store(true, Ordering::Release);
        }
        gate.open();
        assert_eq!(
            task.await.unwrap().unwrap().unwrap().outcome,
            RunOutcome::Deferred
        );
        harness.fixture.store.set_burn_check_test_hook(None);
        let saved: (String, String, String) = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT candidate_id, preparation_json, answer_json
             FROM burn_check_candidate WHERE state = 'answered'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let attempts = saved_attempts(&harness.fixture.store);
        let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
        harness
            .runtime
            .prohibit_model_work
            .store(true, Ordering::Release);

        // Reopen constructs a fresh host and reconciles before it publishes report metadata.
        let mut harness = harness.reopen(true);
        let state: String = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
                [&saved.0],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            state,
            if source_waiting {
                "answered"
            } else {
                "accepted"
            }
        );
        if source_waiting {
            assert_eq!(harness.status(), prior);
        } else {
            assert_ne!(harness.status(), prior);
        }
        harness.host.catch_up().unwrap();
        let retained: (String, String, String, String) = harness.fixture.store.lock().query_row(
            "SELECT c.state, c.preparation_json, c.answer_json, s.status FROM burn_check_candidate c
             JOIN burn_check_current_status s ON s.assessment_id = 'assessment:' || c.candidate_id
             WHERE c.candidate_id = ?1",
            [&saved.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(
            retained,
            ("accepted".into(), saved.1, saved.2, "success".into())
        );
        assert_eq!(saved_attempts(&harness.fixture.store), attempts);
        assert_eq!(
            harness.fixture.store.burn_check_usage_summary().unwrap(),
            usage
        );
        assert_eq!(usage.confirmed_calls, 2);
        assert_eq!(usage.unknown_outcomes, 0);
        assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), 0);
        assert_eq!((a.count(), b.count()), (2, 0));
    }
}
