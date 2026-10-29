use super::*;
use crate::jev_ollama::test_hooks;
use crate::store::native_tool_results::{evidence, publications::PublicationPolicy};
use antiburn_checks::engine::{EvaluationDiagnostic, EvaluationFailureCode};
use antiburn_local::analysis::{
    NativeEvidenceBatch, NativeEvidenceEvent, NativeEvidenceFact, NativeMessageObservation,
    ToolResultObservation,
};

fn harness(a: &Server) -> Harness {
    let fixture = Fixture::new();
    std::fs::write(fixture.global.join("failed-retry-loop.yml"), SOURCE).unwrap();
    let mut connection = a.connection("synthetic-local-model");
    connection.provider = SystemOneProvider::Ollama;
    connection.endpoint = SystemOneEndpoint::BaseUrl(format!("http://{}", a.address));
    headless::setup_keyless_profiles(
        &fixture.store,
        BTreeMap::from([("a".into(), connection)]),
        "a",
        true,
    )
    .unwrap();
    let worker = Arc::new(WorkerHandle::default());
    headless::restore_keyless_profile(&fixture.store, &worker).unwrap();
    assert_eq!(
        worker.execution_snapshot().unwrap().connection.provider,
        SystemOneProvider::Ollama
    );
    Harness::with_worker(fixture, worker)
}

#[test]
fn ollama_headless_setup_requires_valid_keyless_loopback_profiles() {
    let connection = SystemOneConnection {
        provider: SystemOneProvider::Ollama,
        endpoint: SystemOneEndpoint::BaseUrl("http://127.0.0.1:11434".into()),
        model: "synthetic-local-model".into(),
        credential: None,
        ..SystemOneConnection::default()
    };
    for base in ["http://127.0.0.1:11434", "http://[::1]:11434"] {
        let fixture = Fixture::new();
        let mut local = connection.clone();
        local.endpoint = SystemOneEndpoint::BaseUrl(base.into());
        headless::setup_keyless_profiles(
            &fixture.store,
            BTreeMap::from([("local".into(), local)]),
            "local",
            true,
        )
        .unwrap();
    }
    let mut rejected = Vec::new();
    for base in [
        "http://192.0.2.1:11434",
        "http://[2001:db8::1]:11434",
        "http://provider.invalid:11434",
        "http://localhost:11434",
        "http://127.0.0.1:11434?query=unsupported",
    ] {
        let mut nonlocal = connection.clone();
        nonlocal.endpoint = SystemOneEndpoint::BaseUrl(base.into());
        rejected.push(nonlocal);
    }
    for credential in [
        crate::jev::config::CredentialReference::LegacyTypeSafe,
        crate::jev::config::CredentialReference::Connection("synthetic-slot".into()),
    ] {
        let mut authenticated = connection.clone();
        authenticated.credential = Some(credential);
        rejected.push(authenticated);
    }
    let mut wrong_endpoint = connection.clone();
    wrong_endpoint.endpoint = SystemOneEndpoint::ExactUrl("http://127.0.0.1:11434".into());
    rejected.push(wrong_endpoint);
    let mut invalid_model = connection.clone();
    invalid_model.model.clear();
    rejected.push(invalid_model);
    for provider in [SystemOneProvider::Jev, SystemOneProvider::Cloudflare] {
        let mut unsupported = connection.clone();
        unsupported.provider = provider;
        rejected.push(unsupported);
    }
    for connection in rejected {
        let fixture = Fixture::new();
        let before = fixture
            .store
            .internal_value("internal:smartChecksConnectionsV1");
        assert!(
            headless::setup_keyless_profiles(
                &fixture.store,
                BTreeMap::from([("rejected".into(), connection)]),
                "rejected",
                true,
            )
            .is_err()
        );
        assert_eq!(
            fixture
                .store
                .internal_value("internal:smartChecksConnectionsV1"),
            before
        );
    }
}

fn correct_native_source(store: &Store, candidate: &str, now_ms: u64) {
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    assert_eq!(
        tx.query_row(
            "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
            [candidate],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "dispatching"
    );
    let payload: String = tx
        .query_row(
            "SELECT payload_json FROM native_tool_result_current
         WHERE environment_key = 'native' AND agent = 'pi' AND session_id = 'main'
           AND event_kind = 'tool_result' AND is_removed = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut corrected: ToolResultObservation = serde_json::from_str(&payload).unwrap();
    corrected.revision += 1;
    corrected.content = Some("Corrected synthetic result: parser tests passed.".into());
    let generation: i64 = tx
        .query_row(
            "SELECT source_generation FROM session
         WHERE environment_key = 'native' AND agent = 'pi' AND session_id = 'main'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        evidence::reconcile_evidence_in(
            &tx,
            &SessionKey::new("native", "pi", "main"),
            generation,
            0,
            &NativeEvidenceBatch {
                source_format: SourceFormat::PiV3Jsonl,
                events: vec![NativeEvidenceEvent {
                    live_eligible: true,
                    fact: NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(
                        corrected
                    )),
                }],
                history_complete: true,
            },
            PublicationPolicy {
                suppress_coverage: true,
                ..Default::default()
            },
            i64::try_from(now_ms).unwrap(),
        )
        .unwrap()
    );
    assert_eq!(
        tx.query_row(
            "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
            [candidate],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "stale"
    );
    tx.commit().unwrap();
}

fn assert_candidate_diagnostic(
    harness: &Harness,
    candidate: &str,
    expected: EvaluationFailureCode,
) {
    let diagnostic: EvaluationDiagnostic = serde_json::from_str(
        &harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT diagnostic_json FROM burn_check_candidate WHERE candidate_id = ?1",
                [candidate],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(diagnostic.code, expected);
}

fn assert_no_provider_usage(harness: &Harness) {
    let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
    assert_eq!((usage.confirmed_calls, usage.unknown_outcomes), (0, 0));
    assert_eq!(
        harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_decision_usage",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
}

fn assert_unsent_preflight_failure(
    harness: &Harness,
    server: &Server,
    candidate: &str,
    diagnostic: EvaluationFailureCode,
) {
    let (state, attempt_state, outcome, attempts, statuses): (String, String, String, u64, u64) = harness
        .fixture.store.lock().query_row(
            "SELECT c.state, a.state, a.outcome,
                (SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE candidate_id = c.candidate_id),
                (SELECT COUNT(*) FROM burn_check_current_status)
             FROM burn_check_candidate c JOIN burn_check_evaluation_attempt a USING(candidate_id)
             WHERE c.candidate_id = ?1",
            [candidate],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).unwrap();
    assert_eq!(
        (state.as_str(), attempt_state.as_str(), outcome.as_str()),
        ("execution_failed", "settled", "permanent")
    );
    assert_eq!((attempts, statuses), (1, 0));
    assert_candidate_diagnostic(harness, candidate, diagnostic);
    assert_no_provider_usage(harness);
    assert_eq!(server.count(), 0);
    assert_eq!(server.tags_count(), 1);
}

#[tokio::test]
async fn ollama_matching_preflight_keeps_unpinned_selection_and_records_inference_usage() {
    let a = Server::new();
    let mut harness = harness(&a);
    assert!(
        harness
            .runtime
            .worker
            .execution_snapshot()
            .unwrap()
            .connection
            .model_revision
            .is_none()
    );
    harness.publish(1);
    a.reply("clear", None);
    let settlement = harness.job().await.unwrap().unwrap().unwrap();
    assert_eq!(settlement.outcome, RunOutcome::Accepted);
    assert_eq!((settlement.attempts, settlement.new_intents), (1, 1));
    assert_eq!((a.tags_count(), a.count()), (1, 1));
    let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
    assert_eq!((usage.confirmed_calls, usage.unknown_outcomes), (1, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ollama_retag_during_slot_wait_stays_unsent_without_usage_or_retry() {
    let a = Server::new();
    let mut harness = harness(&a);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let held = Arc::new(Mutex::new(Some(test_hooks::hold_inference_slots().await)));
    let released = Arc::clone(&held);
    let tags = Arc::clone(&a.tags_reply);
    let reads = Arc::clone(&a.tags_reads);
    let job = harness.host.evaluation_jobs(1).pop().unwrap();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let settlement = test_hooks::with_inference_slot_pending(
        move || {
            assert_eq!(reads.load(Ordering::Acquire), 0);
            *tags.lock().unwrap() = TagsReply::body(
                200,
                json!({"models": [
                    {"name": "synthetic-local-model:latest", "digest": "replacement-digest"}
                ]})
                .to_string(),
            );
            drop(released.lock().unwrap().take().unwrap());
        },
        job.run_selected(runtime),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(settlement.outcome, RunOutcome::Failed(_)));
    assert_eq!((settlement.attempts, settlement.new_intents), (1, 1));
    assert!(held.lock().unwrap().is_none());
    assert_unsent_preflight_failure(
        &harness,
        &a,
        &candidate,
        EvaluationFailureCode::InvalidRequest,
    );
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_unsent_preflight_failure(
        &harness,
        &a,
        &candidate,
        EvaluationFailureCode::InvalidRequest,
    );
}

#[tokio::test]
async fn ollama_invalid_preflight_metadata_stays_unsent_without_usage_or_retry() {
    let unavailable = EvaluationFailureCode::ProviderUnavailable;
    let unsupported = EvaluationFailureCode::InvalidRequest;
    let tag = json!({"name": "synthetic-local-model:latest", "digest": "synthetic-digest"});
    let mut remote = tag.clone();
    remote["remote_model"] = json!("selected:cloud");
    let mut remote_host = tag.clone();
    remote_host["remote_host"] = json!("https://remote.invalid");
    let mut alias = tag.clone();
    alias["name"] = json!("registry.ollama.ai/library/synthetic-local-model:latest");
    let mut malformed_digest = tag.clone();
    malformed_digest["digest"] = json!("digest\n");
    let mut too_many = vec![tag.clone()];
    too_many.extend(
        (0..1024).map(|index| json!({"name": format!("other:{index}"), "digest": "other"})),
    );
    let oversized = "x".repeat(2 * 1024 * 1024 + 1);
    let cases = [
        (
            "missing",
            TagsReply::body(200, json!({"models": []}).to_string()),
            unsupported,
        ),
        (
            "duplicate",
            TagsReply::body(
                200,
                json!({"models": [tag.clone(), tag.clone()]}).to_string(),
            ),
            unavailable,
        ),
        (
            "canonical duplicate",
            TagsReply::body(200, json!({"models": [tag.clone(), alias]}).to_string()),
            unavailable,
        ),
        (
            "remote model",
            TagsReply::body(200, json!({"models": [remote]}).to_string()),
            unsupported,
        ),
        (
            "remote host",
            TagsReply::body(200, json!({"models": [remote_host]}).to_string()),
            unsupported,
        ),
        (
            "malformed json",
            TagsReply::body(200, "not json".into()),
            unavailable,
        ),
        (
            "malformed list",
            TagsReply::body(200, json!({"models": {}}).to_string()),
            unavailable,
        ),
        (
            "missing digest",
            TagsReply::body(
                200,
                json!({"models": [{"name": "synthetic-local-model"}]}).to_string(),
            ),
            unavailable,
        ),
        (
            "malformed digest",
            TagsReply::body(200, json!({"models": [malformed_digest]}).to_string()),
            unavailable,
        ),
        (
            "too many models",
            TagsReply::body(200, json!({"models": too_many}).to_string()),
            unavailable,
        ),
        (
            "oversized length",
            TagsReply {
                wire: "HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\nConnection: close\r\n\r\n"
                    .into(),
                gate: None,
            },
            unavailable,
        ),
        (
            "oversized stream",
            TagsReply {
                wire: format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{oversized}"),
                gate: None,
            },
            unavailable,
        ),
        (
            "closed connection",
            TagsReply {
                wire: String::new(),
                gate: None,
            },
            unavailable,
        ),
        (
            "partial body",
            TagsReply {
                wire: "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{".into(),
                gate: None,
            },
            unavailable,
        ),
        (
            "server failure",
            TagsReply::body(503, "{}".into()),
            unavailable,
        ),
    ];
    for (name, reply, diagnostic) in cases {
        let a = Server::new();
        a.set_tags_reply(reply);
        let mut harness = harness(&a);
        harness.publish(1);
        let candidate = pending_candidate(&harness);
        let settlement = harness.job().await.unwrap().unwrap().unwrap();
        assert!(
            matches!(settlement.outcome, RunOutcome::Failed(_)),
            "{name}"
        );
        assert_eq!(
            (settlement.attempts, settlement.new_intents),
            (1, 1),
            "{name}"
        );
        assert_unsent_preflight_failure(&harness, &a, &candidate, diagnostic);
        harness.host.catch_up().unwrap();
        assert!(harness.job().await.unwrap().unwrap().is_none(), "{name}");
        assert_unsent_preflight_failure(&harness, &a, &candidate, diagnostic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ollama_preflight_cancellation_stays_unsent_without_usage_or_retry() {
    let a = Server::new();
    let mut harness = harness(&a);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    a.tags_reply.lock().unwrap().gate = Some(Arc::clone(&gate));
    let task = harness.job();
    gate.wait().await;
    correct_native_source(
        &harness.fixture.store,
        &candidate,
        harness.runtime.clock.load(Ordering::Acquire),
    );
    let settlement = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(settlement.outcome, RunOutcome::Stale);
    assert_stale_attempt(&harness, &candidate, "permanent");
    assert_candidate_diagnostic(&harness, &candidate, EvaluationFailureCode::Cancelled);
    assert_no_provider_usage(&harness);
    assert_eq!((a.tags_count(), a.count()), (1, 0));
    gate.open();
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_no_provider_usage(&harness);
    assert_eq!((a.tags_count(), a.count()), (1, 0));
}

#[tokio::test]
async fn ollama_preflight_deadline_stays_unsent_without_usage_or_retry() {
    let a = Server::new();
    let mut harness = harness(&a);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    a.tags_reply.lock().unwrap().gate = Some(Arc::clone(&gate));
    let task = harness.job();
    gate.wait().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(10)).await;
    let settlement = task.await.unwrap().unwrap().unwrap();
    assert!(matches!(settlement.outcome, RunOutcome::Failed(_)));
    assert_eq!((settlement.attempts, settlement.new_intents), (1, 1));
    assert_unsent_preflight_failure(
        &harness,
        &a,
        &candidate,
        EvaluationFailureCode::ProviderUnavailable,
    );
    gate.open();
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_unsent_preflight_failure(
        &harness,
        &a,
        &candidate,
        EvaluationFailureCode::ProviderUnavailable,
    );
}

#[tokio::test]
async fn ollama_preflight_authentication_rejection_stays_unsent_without_usage() {
    for status in [401, 403] {
        let a = Server::new();
        a.set_tags_reply(TagsReply {
            wire: format!(
                "HTTP/1.1 {status} Rejected\r\nContent-Length: 2097153\r\nConnection: close\r\n\r\n"
            ),
            gate: None,
        });
        let mut harness = harness(&a);
        harness.publish(1);
        let candidate = pending_candidate(&harness);
        let settlement = harness.job().await.unwrap().unwrap().unwrap();
        assert!(matches!(settlement.outcome, RunOutcome::Failed(_)));
        assert!(harness.runtime.worker.authentication_rejected());
        assert_unsent_preflight_failure(
            &harness,
            &a,
            &candidate,
            EvaluationFailureCode::AuthenticationRejected,
        );
    }
}

#[tokio::test(start_paused = true)]
async fn ollama_held_slot_claim_invalidation_stays_unsent() {
    let a = Server::new();
    let mut harness = harness(&a);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let generation = harness
        .runtime
        .worker
        .execution_snapshot()
        .unwrap()
        .generation;
    let held = test_hooks::hold_inference_slots().await;
    let pending = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&pending);
    let store = Arc::clone(&harness.fixture.store);
    let queued_candidate = candidate.clone();
    let clock = harness.runtime.clock.load(Ordering::Acquire);
    let job = harness.host.evaluation_jobs(1).pop().unwrap();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let task = tokio::spawn(test_hooks::with_inference_slot_pending(
        move || {
            assert_eq!(observed.fetch_add(1, Ordering::AcqRel), 0);
            correct_native_source(&store, &queued_candidate, clock);
        },
        job.run_selected(runtime),
    ));
    let settlement = task.await.unwrap().unwrap().unwrap();
    assert_eq!(settlement.outcome, RunOutcome::Stale);
    assert_eq!((settlement.attempts, settlement.new_intents), (1, 1));
    assert_eq!(pending.load(Ordering::Acquire), 1);
    assert!(harness.runtime.worker.key_is_current(generation));
    assert_stale_attempt(&harness, &candidate, "permanent");
    assert_candidate_diagnostic(&harness, &candidate, EvaluationFailureCode::Cancelled);
    assert_no_provider_usage(&harness);
    assert_eq!(a.count(), 0);
    drop(held);
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_stale_attempt(&harness, &candidate, "permanent");
    assert_no_provider_usage(&harness);
    assert_eq!(a.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn ollama_held_slot_deadline_stays_unsent() {
    let a = Server::new();
    let mut harness = harness(&a);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let generation = harness
        .runtime
        .worker
        .execution_snapshot()
        .unwrap()
        .generation;
    let held = test_hooks::hold_inference_slots().await;
    let (pending, reached) = tokio::sync::oneshot::channel();
    let pending = Mutex::new(Some(pending));
    let job = harness.host.evaluation_jobs(1).pop().unwrap();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let task = tokio::spawn(test_hooks::with_inference_slot_pending(
        move || {
            pending
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(tokio::time::Instant::now())
                .unwrap();
        },
        job.run_selected(runtime),
    ));
    let started = reached.await.unwrap();
    tokio::time::advance(Duration::from_secs(59)).await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    let settlement = task.await.unwrap().unwrap().unwrap();
    assert!(matches!(settlement.outcome, RunOutcome::Failed(_)));
    assert_eq!((settlement.attempts, settlement.new_intents), (1, 1));
    assert_eq!(
        tokio::time::Instant::now(),
        started + Duration::from_secs(60)
    );
    assert!(harness.runtime.worker.key_is_current(generation));
    let (state, attempt_state, outcome, attempts, statuses): (String, String, String, u64, u64) = harness
        .fixture.store.lock().query_row(
            "SELECT c.state, a.state, a.outcome,
                (SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE candidate_id = c.candidate_id),
                (SELECT COUNT(*) FROM burn_check_current_status)
             FROM burn_check_candidate c JOIN burn_check_evaluation_attempt a USING(candidate_id)
             WHERE c.candidate_id = ?1",
            [&candidate],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).unwrap();
    assert_eq!(
        (state.as_str(), attempt_state.as_str(), outcome.as_str()),
        ("execution_failed", "settled", "permanent")
    );
    assert_eq!((attempts, statuses), (1, 0));
    assert_candidate_diagnostic(
        &harness,
        &candidate,
        EvaluationFailureCode::ProviderUnavailable,
    );
    assert_no_provider_usage(&harness);
    assert_eq!(a.count(), 0);
    drop(held);
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_no_provider_usage(&harness);
    assert_eq!(a.count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ollama_inner_slot_release_rechecks_the_exact_claim_before_http_send() {
    let a = Server::new();
    let mut harness = harness(&a);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let generation = harness
        .runtime
        .worker
        .execution_snapshot()
        .unwrap()
        .generation;
    let held = Arc::new(Mutex::new(Some(test_hooks::hold_inference_slots().await)));
    let pending = Arc::new(AtomicUsize::new(0));
    let store = Arc::clone(&harness.fixture.store);
    let queued_candidate = candidate.clone();
    let clock = harness.runtime.clock.load(Ordering::Acquire);
    let released = Arc::clone(&held);
    let observed = Arc::clone(&pending);
    a.reply("clear", None);
    let job = harness.host.evaluation_jobs(1).pop().unwrap();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let task = tokio::spawn(test_hooks::with_inference_slot_pending(
        move || {
            assert_eq!(observed.fetch_add(1, Ordering::AcqRel), 0);
            correct_native_source(&store, &queued_candidate, clock);
            drop(released.lock().unwrap().take().unwrap());
        },
        job.run_selected(runtime),
    ));
    let settlement = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(settlement.outcome, RunOutcome::Stale);
    assert_eq!((settlement.attempts, settlement.new_intents), (1, 1));
    assert_eq!(pending.load(Ordering::Acquire), 1);
    assert!(held.lock().unwrap().is_none());
    assert!(harness.runtime.worker.key_is_current(generation));
    assert_stale_attempt(&harness, &candidate, "permanent");
    assert_candidate_diagnostic(&harness, &candidate, EvaluationFailureCode::Cancelled);
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_stale_attempt(&harness, &candidate, "permanent");
    let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
    assert_eq!((usage.confirmed_calls, usage.unknown_outcomes), (0, 0));
    assert_eq!(a.count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ollama_sent_source_cancellation_keeps_unknown_delivery_without_resend() {
    let a = Server::new();
    let mut harness = harness(&a);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let generation = harness
        .runtime
        .worker
        .execution_snapshot()
        .unwrap()
        .generation;
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    a.reply("clear", Some(Arc::clone(&gate)));
    let task = harness.job();
    gate.wait().await;
    assert_eq!(a.count(), 1);
    correct_native_source(
        &harness.fixture.store,
        &candidate,
        harness.runtime.clock.load(Ordering::Acquire),
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap()
            .outcome,
        RunOutcome::Stale
    );
    gate.open();
    assert!(harness.runtime.worker.key_is_current(generation));
    assert_stale_attempt(&harness, &candidate, "unknown");
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_stale_attempt(&harness, &candidate, "unknown");
    let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
    assert_eq!((usage.confirmed_calls, usage.unknown_outcomes), (0, 1));
    assert_eq!(a.count(), 1);
}
