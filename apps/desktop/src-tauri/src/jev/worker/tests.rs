use super::checkpoint_tests::{checkpoint_fixture, checkpoint_fixture_in};
use super::*;

#[test]
fn built_in_descriptors_remain_registered_but_cannot_execute() {
    assert_eq!(registered_checks().len(), 4);
    assert!(executable_checks().is_empty());
}

#[tokio::test(start_paused = true)]
async fn provider_admission_waits_for_deadline_and_cancels_on_connection_change() {
    let (_store, input, handle, generation) = checkpoint_fixture();
    let events = SessionEvents::default();
    let now = tokio::time::Instant::now();
    {
        let mut pacing = provider_pacing(SystemOneProvider::Custom).lock().unwrap();
        *pacing = ProviderPacing::new(now);
        assert!(pacing.admit("first", 10, now));
    }
    let waiting = wait_for_provider(
        SystemOneProvider::Custom,
        "second",
        10,
        &handle,
        generation,
        &events,
        &input.key,
    );
    tokio::pin!(waiting);
    std::future::poll_fn(|cx| {
        assert!(waiting.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_millis(25)).await;
    assert_eq!(waiting.await, Ok(()));
    assert_eq!(tokio::time::Instant::now(), now + Duration::from_millis(25));

    provider_pacing(SystemOneProvider::Custom)
        .lock()
        .unwrap()
        .defer(tokio::time::Instant::now(), Duration::from_secs(60));
    let waiting = wait_for_provider(
        SystemOneProvider::Custom,
        "cancelled",
        10,
        &handle,
        generation,
        &events,
        &input.key,
    );
    tokio::pin!(waiting);
    std::future::poll_fn(|cx| {
        assert!(waiting.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    handle.suspend_system_one();
    assert_eq!(waiting.await, Err(JevError::Cancelled));
    assert_eq!(tokio::time::Instant::now(), now + Duration::from_millis(25));
}

#[test]
fn provider_deadline_observes_spacing_cooldown_and_multiple_token_expiries() {
    let now = tokio::time::Instant::now();
    let mut pacing = ProviderPacing::new(now);
    assert!(pacing.admit("first", 40_000, now));
    assert_eq!(
        pacing.next_eligible(10, now),
        now + Duration::from_millis(25)
    );
    assert!(pacing.admit("second", 40_000, now + Duration::from_millis(25)));
    assert_eq!(
        pacing.next_eligible(70_000, now + Duration::from_millis(50)),
        now + Duration::from_millis(1025)
    );
    pacing.defer(now, Duration::from_secs(5));
    assert_eq!(
        pacing.next_eligible(70_000, now + Duration::from_millis(50)),
        now + Duration::from_secs(5)
    );
    assert!(pacing.admit("after", 70_000, now + Duration::from_secs(5)));
}

#[tokio::test]
async fn successful_discovery_caches_unknown_limits_and_accepts_configured_only_limits() {
    use antiburn_local::analysis::jev::capabilities::CapabilitySource;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    tokio::time::timeout(Duration::from_secs(10), async {
        for configured in [None, Some(2048)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let connection = SystemOneConnection {
                provider: SystemOneProvider::Ollama,
                endpoint: SystemOneEndpoint::BaseUrl(format!("http://{}", listener.local_addr().unwrap())),
                model: "tev1:0.8b".into(), credential: None, context_override: None,
                ..SystemOneConnection::default()
            };
            let server = tokio::spawn(async move {
                for (route, body) in [
                    ("/api/version", serde_json::json!({"version": "0.35.0"})),
                    ("/api/tags", serde_json::json!({"models": [{"name": "tev1:0.8b", "digest": "digest"}]})),
                    ("/api/show", serde_json::json!({"capabilities": ["decision"],
                        "parameters": configured.map(|value| format!("num_ctx {value}")),
                        "model_info": {"general.architecture": "test", "test.context_length": 65536}})),
                    ("/api/ps", serde_json::json!({"models": []})),
                ] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = [0; 4096];
                    let count = socket.read(&mut bytes).await.unwrap();
                    assert!(String::from_utf8_lossy(&bytes[..count]).contains(route));
                    let body = body.to_string();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            });
            let handle = WorkerHandle::default();
            handle.set_system_one_connection(connection.clone(), None).unwrap();
            let generation = handle.key_generation.load(Ordering::Acquire);
            let result = handle.resolve_capabilities(generation).await;
            server.await.unwrap();
            let cached = handle.discovery.lock().await.as_ref().unwrap().capabilities.clone();
            assert_eq!(cached.total_input_tokens.value, configured);
            assert_eq!(cached.runtime_context_tokens.value, None);
            if configured.is_some() {
                assert_eq!(cached.total_input_tokens.source, CapabilitySource::ProviderMetadata);
                assert_eq!(result.as_ref().unwrap().usable_input_tokens(), Some(1024));
            } else {
                assert_eq!(result, Err(JevError::ProviderUnavailable));
            }
            assert_eq!(handle.resolve_capabilities(generation).await, result);

            let mut manual_connection = connection;
            manual_connection.context_override = Some(crate::jev::config::ContextLimitOverride {
                total_input_tokens: Some(2048), ..Default::default()
            });
            handle.set_system_one_connection(manual_connection, None).unwrap();
            let generation = handle.key_generation.load(Ordering::Acquire);
            let manual = handle.resolve_capabilities(generation).await.unwrap();
            assert_eq!(manual.total_input_tokens.source, CapabilitySource::Manual);
            assert_eq!(manual.usable_input_tokens(), Some(1024));
            assert_eq!(manual.runtime_context_tokens.value, None);
        }
    }).await.unwrap();
}

fn scheduler_fixture() -> (Store, Vec<BurnCheckInput>) {
    let (store, base, _, _) = checkpoint_fixture();
    let mut inputs = Vec::new();
    for session in ["checkpoint", "second"] {
        if session != "checkpoint" {
            let mut record = store.session(&base.key).unwrap().unwrap();
            record.key.session_id = session.into();
            store
                .upsert_sessions(&[record], &crate::agents::evidence_cohort())
                .unwrap();
            store
                .lock()
                .execute(
                    "UPDATE session_evidence SET status = 'ready', analyzed_generation = 0,
                    parser_revision = ?1, analyzer_revision = ?2, evidence_schema_revision = ?3,
                    evidence_json = '{}', claim_fence = 1, published_fence = 1",
                    rusqlite::params![
                        antiburn_local::analysis::PARSER_REVISION,
                        antiburn_local::analysis::ANALYZER_REVISION,
                        antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION
                    ],
                )
                .unwrap();
        }
        let key = SessionKey::new("native", "claude-code", session);
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO turn (environment_key, agent, session_id, claim_fence, source_key,
                thread_id, turn_index, scope, role, input_tokens, cache_read_tokens,
                cache_write_tokens, output_tokens, is_compaction_boundary)
             VALUES (?1, ?2, ?3, 1, 'source', 'thread', 0, 'main', 'assistant', 0, 0, 0, 0, 0)",
                rusqlite::params![key.environment_key, key.agent, key.session_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated)
                 VALUES (?1, 0, 'text', ?2, 0)",
                rusqlite::params![
                    connection.last_insert_rowid(),
                    b"synthetic activity".as_slice()
                ],
            )
            .unwrap();
        drop(connection);
        let incarnation = store.lock().query_row(
            "SELECT incarnation FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            rusqlite::params![key.environment_key, key.agent, key.session_id], |row| row.get(0),
        ).unwrap();
        for check in registered_checks() {
            store
                .set_check_enabled(DetectorId::from_key(check.id()).unwrap(), true)
                .unwrap();
            let input = BurnCheckInput {
                key: key.clone(),
                incarnation,
                check_id: check.id().into(),
                evaluator_revision: check.evaluator_revision(),
                ..base.clone()
            };
            // Replace the checkpoint fixture's active lease with a queued row.
            store
                .lock()
                .execute(
                    "DELETE FROM burn_check_assessment WHERE environment_key = ?1
                AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
                    rusqlite::params![key.environment_key, key.agent, key.session_id, check.id()],
                )
                .unwrap();
            assert!(
                store
                    .queue_burn_check_assessment(&input, unix_now(), 180)
                    .unwrap()
            );
            inputs.push(input);
        }
    }
    (store, inputs)
}

#[test]
fn preflight_blocks_deferred_and_unknown_delivery_before_packing() {
    assert_eq!(
        preflight_admission(BurnCheckRequestAdmission::Admitted),
        Ok(())
    );
    assert_eq!(
        preflight_admission(BurnCheckRequestAdmission::Deferred),
        Err(JevError::Cancelled)
    );
    assert_eq!(
        preflight_admission(BurnCheckRequestAdmission::Unresolved),
        Err(JevError::RequestOutcomeUnknown)
    );
}

#[test]
fn scheduler_rotates_checks_and_least_served_sessions_across_restarts() {
    let (store, _) = scheduler_fixture();
    let mut cursor = SchedulerCursor::default();
    let mut selected = Vec::new();
    for _ in 0..8 {
        let (index, candidate) = select_turn(&store, registered_checks(), &cursor, unix_now())
            .unwrap()
            .unwrap();
        selected.push((
            registered_checks()[index].id(),
            candidate.session.key.session_id.clone(),
        ));
        cursor.turn += 1;
        cursor.next_check = (index + 1) % 4;
        store
            .serve_burn_check_candidate(
                &candidate,
                registered_checks()[index].id(),
                cursor.turn,
                &serde_json::to_string(&cursor).unwrap(),
            )
            .unwrap();
        cursor = scheduler_cursor(&store).unwrap();
    }
    assert_eq!(
        selected.iter().map(|(check, _)| *check).collect::<Vec<_>>(),
        [
            "ignored_instructions",
            "scope_creep",
            "over_exploring",
            "skill_opportunities"
        ]
        .repeat(2)
    );
    assert!(
        selected[..4]
            .iter()
            .all(|(_, session)| session == "checkpoint")
    );
    assert!(selected[4..].iter().all(|(_, session)| session == "second"));
}

#[test]
fn scheduler_skips_held_preclaim_candidates_without_blocking_other_sessions() {
    let (store, _) = scheduler_fixture();
    let checks: &[&dyn JevCheckDescriptor] = &[&crate::scope_creep_worker::CHECK];
    let cursor = SchedulerCursor::default();
    let mut held = std::collections::BTreeSet::new();
    let (_, first) = select_turn_excluding(&store, checks, &cursor, unix_now(), &held)
        .unwrap()
        .unwrap();
    held.insert((checks[0].id(), first.session.key.clone()));
    let (_, second) = select_turn_excluding(&store, checks, &cursor, unix_now(), &held)
        .unwrap()
        .unwrap();
    assert_ne!(first.session.key, second.session.key);
    assert_eq!(
        store
            .burn_check_assessment(&first.session.key, checks[0].id())
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );
    held.insert((checks[0].id(), second.session.key));
    assert!(
        select_turn_excluding(&store, checks, &cursor, unix_now(), &held)
            .unwrap()
            .is_none()
    );
    let (index, _) = select_turn_excluding(&store, registered_checks(), &cursor, unix_now(), &held)
        .unwrap()
        .unwrap();
    assert_ne!(registered_checks()[index].id(), checks[0].id());
}

#[test]
fn transient_progress_storage_failure_keeps_checkpoint_and_persisted_backoff() {
    let (store, inputs) = scheduler_fixture();
    let checks: &[&dyn JevCheckDescriptor] = &[&crate::scope_creep_worker::CHECK];
    let (_, candidate) = select_turn(&store, checks, &SchedulerCursor::default(), unix_now())
        .unwrap()
        .unwrap();
    let input = inputs
        .iter()
        .find(|input| input.key == candidate.session.key && input.check_id == checks[0].id())
        .unwrap();
    let now = unix_now();
    assert!(
        store
            .claim_burn_check_assessment(input, now, 300, 180)
            .unwrap()
    );
    assert!(
        store
            .save_burn_check_checkpoint(input, "{\"accepted_answer\":true}", None, now, 300, 180)
            .unwrap()
    );
    store
        .save_burn_check_scheduling(input, Some(1), 1, 0)
        .unwrap();
    store
        .release_failed_burn_check_lease(input, "progress_storage_failed", now + 900)
        .unwrap();
    let error = anyhow::Error::new(JevError::ProgressStorageFailure);
    let (category, retry) = candidate_error_policy(&error, checks[0].policy(), now);
    assert_eq!(category, "progress_storage_failed");
    assert!(
        store
            .settle_burn_check_candidate_error(
                &candidate,
                checks[0].id(),
                &checks[0].evaluator_revision(),
                now,
                category,
                retry
            )
            .unwrap()
    );
    assert_eq!(
        store.burn_check_next_attempt_at(input).unwrap(),
        Some(now + 900)
    );
    assert_eq!(
        store
            .burn_check_assessment(&input.key, checks[0].id())
            .unwrap()
            .unwrap()
            .progress_json,
        "{\"accepted_answer\":true}"
    );
    assert!(
        !store
            .burn_check_candidates_for_revision(
                checks[0].id(),
                &checks[0].evaluator_revision(),
                now + 899,
                180,
                8
            )
            .unwrap()
            .iter()
            .any(|current| current.session.key == input.key)
    );
    assert!(
        store
            .burn_check_candidates_for_revision(
                checks[0].id(),
                &checks[0].evaluator_revision(),
                now + 900,
                180,
                8
            )
            .unwrap()
            .iter()
            .any(|current| current.session.key == input.key)
    );
}

#[test]
fn deterministic_preflight_failure_is_source_fenced_and_blocks_identical_reconstruction() {
    let (store, _) = scheduler_fixture();
    let checks: &[&dyn JevCheckDescriptor] = &[&crate::scope_creep_worker::CHECK];
    let now = unix_now();
    let (_, candidate) = select_turn(&store, checks, &SchedulerCursor::default(), now)
        .unwrap()
        .unwrap();
    store.lock().execute("DELETE FROM burn_check_assessment WHERE agent = ?1 AND session_id = ?2 AND check_id = ?3",
        rusqlite::params![candidate.session.key.agent, candidate.session.key.session_id, checks[0].id()]).unwrap();
    let transient = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::WouldBlock));
    let (category, retry) = candidate_error_policy(&transient, checks[0].policy(), now);
    let retry_at = retry.unwrap();
    assert!(
        store
            .settle_burn_check_candidate_error(
                &candidate,
                checks[0].id(),
                &checks[0].evaluator_revision(),
                now,
                category,
                retry
            )
            .unwrap()
    );
    assert!(
        !store
            .burn_check_candidates_for_revision(
                checks[0].id(),
                &checks[0].evaluator_revision(),
                retry_at - 1,
                180,
                8
            )
            .unwrap()
            .iter()
            .any(|current| current.session.key == candidate.session.key)
    );
    assert!(
        store
            .burn_check_candidates_for_revision(
                checks[0].id(),
                &checks[0].evaluator_revision(),
                retry_at,
                180,
                8
            )
            .unwrap()
            .iter()
            .any(|current| current.session.key == candidate.session.key)
    );
    let error = anyhow::Error::new(JevError::InvalidCheckPlan);
    let (category, retry) = candidate_error_policy(&error, checks[0].policy(), now);
    assert_eq!((category, retry), ("candidate_error", None));
    assert!(
        store
            .settle_burn_check_candidate_error(
                &candidate,
                checks[0].id(),
                &checks[0].evaluator_revision(),
                now,
                category,
                retry
            )
            .unwrap()
    );
    let saved = store
        .burn_check_assessment(&candidate.session.key, checks[0].id())
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, "failed");
    assert_eq!(saved.input_revision, None);
    assert!(
        !store
            .burn_check_candidates_for_revision(
                checks[0].id(),
                &checks[0].evaluator_revision(),
                now + 900,
                180,
                8
            )
            .unwrap()
            .iter()
            .any(|current| current.session.key == candidate.session.key)
    );
    store.lock().execute("UPDATE session SET source_generation = source_generation + 1 WHERE agent = ?1 AND session_id = ?2",
        rusqlite::params![candidate.session.key.agent, candidate.session.key.session_id]).unwrap();
    assert!(
        !store
            .settle_burn_check_candidate_error(
                &candidate,
                checks[0].id(),
                &checks[0].evaluator_revision(),
                now + 1,
                "candidate_retry",
                Some(now + 301)
            )
            .unwrap()
    );
    assert_eq!(
        store
            .burn_check_assessment(&candidate.session.key, checks[0].id())
            .unwrap()
            .unwrap()
            .progress_json,
        saved.progress_json
    );
    store
        .record_burn_check_candidate_issue_for_check(
            checks[0].id(),
            &candidate,
            false,
            now + 301,
            now + 1,
        )
        .unwrap();
    let category: String = store.lock().query_row(
        "SELECT last_error_category FROM burn_check_assessment WHERE agent = ?1 AND session_id = ?2 AND check_id = ?3",
        rusqlite::params![candidate.session.key.agent, candidate.session.key.session_id, checks[0].id()], |row| row.get(0),
    ).unwrap();
    assert_eq!(category, "candidate_error");
}

#[tokio::test]
async fn preparation_runs_off_the_async_executor() {
    let caller = std::thread::current().id();
    let prepared = run_blocking_preparation(move || {
        assert_ne!(std::thread::current().id(), caller);
        Ok(42)
    })
    .await
    .unwrap();
    assert_eq!(prepared, 42);
}

#[tokio::test]
async fn cancelled_preparation_holds_its_slot_until_blocking_work_finishes() {
    use std::sync::{Arc, Condvar};
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let active = Arc::new(AtomicU64::new(0));
    let peak = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for index in 0..CANDIDATE_WORKERS + 1 {
        let gate = Arc::clone(&gate);
        let started = started.clone();
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        tasks.push(tokio::spawn(async move {
            run_blocking_preparation(move || {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                started.send(index).unwrap();
                let (lock, release) = &*gate;
                let mut finished = lock.lock().unwrap();
                while !*finished {
                    finished = release.wait(finished).unwrap();
                }
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .await
        }));
    }
    let cancelled = starts.recv().await.unwrap();
    for _ in 1..CANDIDATE_WORKERS {
        starts.recv().await.unwrap();
    }
    tasks[cancelled].abort();
    let held = tokio::time::timeout(Duration::from_millis(50), starts.recv())
        .await
        .is_err();
    let (lock, release) = &*gate;
    *lock.lock().unwrap() = true;
    release.notify_all();
    for (index, task) in tasks.into_iter().enumerate() {
        if index == cancelled {
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            task.await.unwrap().unwrap();
        }
    }
    assert!(held);
    assert_eq!(peak.load(Ordering::SeqCst), CANDIDATE_WORKERS as u64);
}

#[test]
fn current_review_counts_use_only_fresh_persisted_counters() {
    let (store, input, _, _) = checkpoint_fixture();
    let read = |connection: &rusqlite::Connection| {
        Store::current_burn_check_review_counts(
            connection,
            &input.key,
            &input.check_id,
            &input.evaluator_revision,
        )
    };
    assert!(read(&store.lock()).unwrap().is_none());
    assert!(
        store
            .save_burn_check_scheduling(&input, Some(5), 3, 2)
            .unwrap()
    );
    let connection = store.lock();
    let counts = read(&connection).unwrap().unwrap();
    assert_eq!(
        (counts.eligible, counts.reviewed, counts.runnable),
        (Some(5), 3, 2)
    );
    // These fields have no report schema. Counter reads do not parse them.
    connection
        .execute(
            "UPDATE burn_check_assessment SET result_json = '{\"private\":true}',
        progress_json = '{\"private\":true}'",
            [],
        )
        .unwrap();
    assert_eq!(read(&connection).unwrap(), Some(counts.clone()));
    for change in [
        "UPDATE burn_check_assessment SET scheduling_revision = 'old'",
        "UPDATE burn_check_assessment SET input_revision = 'old'",
        "UPDATE burn_check_assessment SET evaluator_revision = 'old'",
        "UPDATE burn_check_assessment SET source_fingerprint = 'other-fingerprint'",
        "UPDATE session SET source_fingerprint = 'other-fingerprint'",
        "UPDATE session SET source_generation = source_generation + 1",
        "UPDATE session SET incarnation = incarnation + 1",
        "UPDATE session_evidence SET published_fence = published_fence + 1",
        "UPDATE session_evidence SET processed_fingerprint = 'other-fingerprint'",
        "UPDATE session_evidence SET parser_revision = parser_revision - 1",
        "UPDATE session_evidence SET analyzer_revision = analyzer_revision - 1",
        "UPDATE session_evidence SET evidence_schema_revision = evidence_schema_revision - 1",
        "UPDATE session_evidence SET status = 'failed'",
        "UPDATE burn_check_assessment SET status = 'superseded'",
    ] {
        connection.execute_batch("SAVEPOINT stale_counts").unwrap();
        connection.execute(change, []).unwrap();
        assert!(read(&connection).unwrap().is_none(), "{change}");
        connection
            .execute_batch("ROLLBACK TO stale_counts; RELEASE stale_counts")
            .unwrap();
    }
    drop(connection);
    let stale = BurnCheckInput {
        source_fingerprint: Some("other-fingerprint".into()),
        ..input.clone()
    };
    assert!(
        !store
            .save_burn_check_scheduling(&stale, Some(100), 99, 1)
            .unwrap()
    );
    assert_eq!(read(&store.lock()).unwrap(), Some(counts));
    assert!(
        store
            .save_burn_check_scheduling(&input, None, 3, 2)
            .unwrap()
    );
    let unknown = read(&store.lock()).unwrap().unwrap();
    assert_eq!(
        (unknown.eligible, unknown.reviewed, unknown.runnable),
        (None, 3, 2)
    );
    assert!(
        store
            .save_burn_check_scheduling(&input, Some(0), 0, 0)
            .unwrap()
    );
    assert_eq!(read(&store.lock()).unwrap().unwrap().eligible, Some(0));
}

#[test]
fn fifth_turn_reserves_continuation_and_exhausted_work_leaves_both_lanes() {
    let (store, inputs) = scheduler_fixture();
    for input in &inputs {
        assert!(
            store
                .save_burn_check_scheduling(input, Some(5), 0, 5)
                .unwrap()
        );
    }
    let continuation = &inputs[3];
    assert!(
        store
            .save_burn_check_scheduling(continuation, Some(5), 3, 2)
            .unwrap()
    );
    let cursor = SchedulerCursor {
        turn: 4,
        next_check: 0,
    };
    let (index, candidate) = select_turn(&store, registered_checks(), &cursor, unix_now())
        .unwrap()
        .unwrap();
    assert_eq!(index, 3);
    assert_eq!(candidate.session.key, continuation.key);
    let priority = select_turn(
        &store,
        registered_checks(),
        &SchedulerCursor::default(),
        unix_now(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(priority.0, 0);
    assert!(
        store
            .save_burn_check_scheduling(continuation, Some(5), 3, 0)
            .unwrap()
    );
    assert_eq!(
        select_turn(&store, registered_checks(), &cursor, unix_now())
            .unwrap()
            .unwrap()
            .0,
        0
    );
    for input in &inputs {
        store
            .save_burn_check_scheduling(input, Some(5), 0, 0)
            .unwrap();
    }
    assert!(
        select_turn(&store, registered_checks(), &cursor, unix_now())
            .unwrap()
            .is_none()
    );
    // A changed evaluator invalidates the inventory lane and terminal state.
    store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET evaluator_revision = 'old'",
            [],
        )
        .unwrap();
    assert!(
        select_turn(&store, registered_checks(), &cursor, unix_now())
            .unwrap()
            .is_some()
    );
}

#[test]
fn transport_attempts_and_backoff_survive_reenrollment_and_store_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let (mut store, input, _, _) = checkpoint_fixture_in(Store::open(directory.path()).unwrap());
    let identities = ["semantic-target".to_owned()];
    let now = unix_now();
    for (attempt, time, delay) in [(1, now, 5), (2, now + 5, 30), (3, now + 35, 0)] {
        assert_eq!(
            store
                .admit_burn_check_requests(&input, &identities, "reservation", time)
                .unwrap(),
            BurnCheckRequestAdmission::Admitted
        );
        assert_eq!(
            store.burn_check_dispatch_attempts(&identities).unwrap(),
            attempt
        );
        store
            .clear_burn_check_request_outcomes(&identities)
            .unwrap();
        store
            .defer_burn_check_dispatch(&input, &identities, (delay > 0).then_some(time + delay))
            .unwrap();
        drop(store);
        store = Store::open(directory.path()).unwrap();
        if delay > 0 {
            assert_eq!(
                store
                    .admit_burn_check_requests(&input, &identities, "other", time)
                    .unwrap(),
                BurnCheckRequestAdmission::Deferred
            );
        }
    }
    assert_eq!(
        store
            .admit_burn_check_requests(&input, &identities, "fourth", now + 100)
            .unwrap(),
        BurnCheckRequestAdmission::Exhausted
    );
    assert!(
        !store
            .burn_check_requests_are_unresolved(&identities)
            .unwrap()
    );
    assert_eq!(store.burn_check_dispatch_attempts(&identities).unwrap(), 3);
    store
        .release_failed_burn_check_lease(&input, "provider_unavailable", now + 200)
        .unwrap();
    assert_eq!(
        store.next_burn_check_retry_at(now).unwrap(),
        Some(now + 200)
    );
    assert_eq!(store.next_burn_check_retry_at(now + 200).unwrap(), None);
    assert!(
        store
            .queue_burn_check_assessment(&input, now + 200, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, now + 200, 300, 180)
            .unwrap()
    );
    assert_eq!(
        store
            .admit_burn_check_requests(&input, &identities, "reenrolled", now + 200)
            .unwrap(),
        BurnCheckRequestAdmission::Exhausted
    );
    store
        .set_internal_value_checked(
            "internal:burnCheckSchedulerV1",
            "{\"turn\":7,\"next_check\":2}",
        )
        .unwrap();
    store.clear_local_session_data().unwrap();
    assert_eq!(store.burn_check_dispatch_attempts(&identities).unwrap(), 0);
    assert!(store.burn_check_scheduler_cursor().unwrap().is_none());
}

#[test]
fn dispatch_attempt_retention_prunes_expired_records_but_keeps_unknown_delivery() {
    let (store, input, _, _) = checkpoint_fixture();
    let now = unix_now();
    store
        .track_burn_check_request("unknown-delivery", "reservation", now)
        .unwrap();
    store
        .lock()
        .execute_batch(
            "INSERT INTO burn_check_dispatch_attempt
               (request_identity, environment_key, agent, session_id, attempts, last_attempt_at_epoch)
             VALUES ('unknown-delivery', 'native', 'claude-code', 'checkpoint', 1, 1);
             WITH RECURSIVE identities(value) AS (
                 SELECT 1 UNION ALL SELECT value + 1 FROM identities WHERE value < 32768
             )
             INSERT INTO burn_check_dispatch_attempt
               (request_identity, environment_key, agent, session_id, attempts, last_attempt_at_epoch)
             SELECT 'expired-' || value, 'native', 'claude-code', 'checkpoint', 3, 1
               FROM identities;",
        )
        .unwrap();
    let identities = vec!["new-request".to_owned()];

    assert_eq!(
        store
            .admit_burn_check_requests(&input, &identities, "new-reservation", now)
            .unwrap(),
        BurnCheckRequestAdmission::Admitted
    );
    assert_eq!(
        store
            .burn_check_dispatch_attempts(&["unknown-delivery".to_owned()])
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .burn_check_dispatch_readiness(&["unknown-delivery".to_owned()], now)
            .unwrap(),
        BurnCheckRequestAdmission::Unresolved
    );
    let retained: usize = store
        .lock()
        .query_row(
            "SELECT count(*) FROM burn_check_dispatch_attempt",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, 2);
}

#[test]
fn provider_retry_after_cooldown_blocks_other_requests_for_same_provider() {
    let now = tokio::time::Instant::now();
    let mut pacing = ProviderPacing::new(now);
    apply_provider_cooldown(
        &mut pacing,
        &JevError::RateLimited {
            retry_after: Some(Duration::from_secs(17)),
        },
        now,
    );

    assert!(!pacing.admit("first", 10, now + Duration::from_secs(16)));
    assert!(pacing.admit("second", 10, now + Duration::from_secs(17)));
}

#[tokio::test]
async fn cached_response_recovers_after_third_transport_attempt() {
    use antiburn_local::analysis::jev::{JevAnswer, JevQuestion, JevRequest, JevUsage};

    tokio::time::timeout(Duration::from_secs(10), async {
    for provider in [SystemOneProvider::Jev, SystemOneProvider::Ollama] {
    let (store, input, handle, generation) = checkpoint_fixture();
    let mut connection = handle.system_one_connection();
    let mut generation = generation;
    let mut capabilities = connection.capabilities().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    if provider == SystemOneProvider::Ollama {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        connection.provider = provider;
        connection.endpoint = SystemOneEndpoint::BaseUrl(format!("http://{}", listener.local_addr().unwrap()));
        connection.credential = None;
        connection.context_override = None;
        handle.set_system_one_connection(connection.clone(), None).unwrap();
        generation = handle.key_generation.load(Ordering::Acquire);
        capabilities.model_revision = Some("accepted-model-digest".into());
        let failed_discovery = async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(String::from_utf8_lossy(&buffer[..count]).starts_with("GET /api/version "));
            socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        };
        let (discovered, ()) = tokio::join!(handle.resolve_capabilities(generation), failed_discovery);
        assert_eq!(discovered, Err(JevError::ProviderUnavailable));
        assert_eq!(handle.resolve_capabilities(generation).await, Err(JevError::ProviderUnavailable));
        connection.model_revision.clone_from(&capabilities.model_revision);
    }
    let request = JevRequest {
        model: connection.model.clone(),
        state: serde_json::json!({"activity": "synthetic"}),
        questions: std::collections::BTreeMap::from([(
            "q".into(),
            JevQuestion::Noul {
                instructions: serde_json::json!("Is this valid?"),
                criteria: None,
            },
        )]),
    };
    let batch = std::sync::Arc::new(JevRequestBatch {
        id: "batch".into(),
        serialized_bytes: serde_json::to_vec(&request).unwrap().len(),
        request,
        work_item_ids: vec!["work".into()],
        work_item_digests: std::collections::BTreeMap::from([(
            "work".into(),
            "semantic-work".into(),
        )]),
        answer_owners: std::collections::BTreeMap::from([(
            "q".into(),
            ("work".into(), "q".into()),
        )]),
        evidence_owners: Default::default(),
        digest: "cacheable-request".into(),
    });
    let digest = compatible_request_identity(&connection, &input, &batch.digest);
    let response = JevResponse {
        model: connection.model.clone(),
        answers: std::collections::BTreeMap::from([("q".into(), JevAnswer::Noul { noul: 0.9 })]),
        usage: JevUsage {
            input_tokens: 12,
            output_tokens: 3,
        },
    };
    store
        .lock()
        .execute(
            "INSERT INTO burn_check_response_cache
               (provider, request_digest, returned_model, response_json, input_tokens,
                output_tokens, created_at_epoch)
             VALUES (?1, ?2, ?3, ?4, 12, 3, ?5)",
            rusqlite::params![
                provider_id(connection.provider),
                digest,
                response.model,
                serde_json::to_string(&response).unwrap(),
                unix_now()
            ],
        )
        .unwrap();
    let identities = batch_request_identities(&connection, &input, &batch);
    for identity in &identities {
        store
            .lock()
            .execute(
                "INSERT INTO burn_check_dispatch_attempt
                   (request_identity, environment_key, agent, session_id, attempts, terminal)
                 VALUES (?1, ?2, ?3, ?4, 3, 1)",
                rusqlite::params![
                    identity,
                    input.key.environment_key,
                    input.key.agent,
                    input.key.session_id
                ],
            )
            .unwrap();
    }
    assert_eq!(
        preflight_admission(BurnCheckRequestAdmission::Exhausted),
        Ok(())
    );

    let events = SessionEvents::default();
    let notify = || {};
    let cached = execute_batch(
        BatchContext {
            store: &store,
            input: &input,
            client: TypeSafeClient::new("synthetic-key".into()).unwrap(),
            handle: &handle,
            key_generation: generation,
            events: &events,
            idle_secs: IDLE_SECS,
            lease_secs: 300,
            capabilities: &capabilities,
        },
        batch.clone(),
        &notify,
    )
    .await
    .expect("cached response bypasses the exhausted network-attempt cap");

    assert_eq!(cached.answers, response.answers);
    assert_eq!(store.burn_check_dispatch_attempts(&identities).unwrap(), 3);
    store.lock().execute("DELETE FROM burn_check_response_cache", []).unwrap();
    assert_eq!(execute_batch(
        BatchContext {
            store: &store, input: &input,
            client: TypeSafeClient::new("synthetic-key".into()).unwrap(),
            handle: &handle, key_generation: generation, events: &events,
            idle_secs: IDLE_SECS, lease_secs: 300, capabilities: &capabilities,
        }, batch, &notify,
    ).await, Err(JevError::ProviderUnavailable));
    assert_eq!(store.burn_check_dispatch_attempts(&identities).unwrap(), 3);
    let accept = listener.accept();
    tokio::pin!(accept);
    std::future::poll_fn(|cx| {
        assert!(accept.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    }).await;
    }
    }).await.unwrap();
}
