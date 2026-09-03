use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use antiburn_local::analysis::{
    EvidenceSource, MemoryTurnRowStore, RawSource, SessionEvidenceAccumulator, SessionInput,
    SourceCapabilities, SourceKind, TurnFacts, TurnRowStore,
};

use super::*;
use crate::store::EvidenceStatus;

mod resume_tests;

fn store() -> Store {
    Store::open_in_memory(std::path::Path::new("/tmp/antiburn-worker-test")).unwrap()
}

fn record(id: &str) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "claude-code", id),
        source_kind: "file".into(),
        source_label: format!("/tmp/{id}.jsonl"),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: Some(100),
        activity_cursor: String::new(),
        activity_source: "event".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some(format!("sv1:{id}")),
    }
}

fn claim(store: &Store, id: &str, now: i64) -> EvidenceClaim {
    store
        .upsert_sessions(&[record(id)], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .claim_next_evidence(&crate::agents::evidence_cohort(), now, LEASE_SECS)
        .unwrap()
        .unwrap()
}

fn failed_pass(outcome: PassOutcome) -> EvidencePass {
    EvidencePass {
        analysis: analysis::SessionAnalysis::unavailable(),
        evidence: None,
        outcome,
        source_outcomes: Vec::new(),
    }
}

fn published_pass(record: &SessionRecord) -> EvidencePass {
    let store: Arc<dyn TurnRowStore> =
        MemoryTurnRowStore::new("claude", record.key.session_id.clone());
    let mut pass = analysis::evidence_pass_with_turn_rows(
        &[SessionInput {
            agent: "claude".into(),
            session_id: record.key.session_id.clone(),
            source: RawSource::Jsonl(
                r#"{"type":"assistant","timestamp":100,"message":{"id":"m","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}
"#
                .into(),
            ),
        }],
        &|| false,
        Some(store),
    );
    pass.analysis.fingerprint = record
        .source_fingerprint
        .clone()
        .unwrap_or_else(|| analysis::MISSING_FINGERPRINT.into());
    pass
}

/// Like [`published_pass`], but for a generic-JSONL agent (Copilot):
/// `capabilities_for_vendor` maps it to `SourceCapabilities::generic()`
/// (every field unset), so this exercises the real GENERIC-adapter path
/// a widened-cohort worker pass now takes for an uncharacterized vendor.
fn generic_published_pass(record: &SessionRecord) -> EvidencePass {
    let store: Arc<dyn TurnRowStore> =
        MemoryTurnRowStore::new("copilot", record.key.session_id.clone());
    let mut pass = analysis::evidence_pass_with_turn_rows(
        &[SessionInput {
            agent: "copilot".into(),
            session_id: record.key.session_id.clone(),
            source: RawSource::Jsonl(
                r#"{"role":"user","content":"hi"}
{"role":"assistant","content":"hello","usage":{"prompt_tokens":2,"completion_tokens":3}}
"#
                .into(),
            ),
        }],
        &|| false,
        Some(store),
    );
    pass.analysis.fingerprint = record
        .source_fingerprint
        .clone()
        .unwrap_or_else(|| analysis::MISSING_FINGERPRINT.into());
    pass
}

async fn captured_signal(slot: &Mutex<Option<PassSignal>>) -> PassSignal {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(signal) = slot.lock().unwrap().clone() {
                break signal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the pass receives its signal")
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the worker reaches the expected state");
}

fn evidence_with(capabilities: SourceCapabilities) -> SessionEvidence {
    SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: "caps".to_owned(),
        kind: SourceKind::File,
        capabilities,
    })
    .evidence(&TurnFacts::default())
}

fn no_capabilities() -> SourceCapabilities {
    SourceCapabilities {
        request_context_tokens: false,
        cache_write_tokens: false,
        timestamps_and_order: false,
        tool_invocations: false,
        skill_mcp_attribution: false,
        tool_definitions: false,
        model_identity: false,
        token_classes: false,
        reasoning_effort_tier: false,
        fast_tier: false,
        service_tier: false,
        subagent_relationships: false,
        subagent_models: false,
        compaction_boundaries: false,
        thread_identity: false,
        record_identity: false,
        linear_record_order: false,
        quota_incidents: false,
        harness_version: false,
    }
}

#[tokio::test]
async fn unknown_store_slug_is_rejected_instead_of_routing_to_claude() {
    let mut unknown = record("unknown");
    unknown.key.agent = "unknown-agent".to_owned();

    let pass = run_record_pass(&unknown, PassSignal::new(), 1, store()).await;

    assert_eq!(pass.outcome, PassOutcome::Unsupported);
    assert!(pass.evidence.is_none());
    assert!(pass.analysis.metrics.is_none());
}

#[test]
fn published_status_classifies_capability_sets_against_detector_prerequisites() {
    assert_eq!(
        published_status(&evidence_with(SourceCapabilities::claude())),
        PublishedEvidence::Ready
    );
    assert_eq!(
        published_status(&evidence_with(SourceCapabilities::pi())),
        PublishedEvidence::Ready
    );
    assert_eq!(
        published_status(&evidence_with(no_capabilities())),
        PublishedEvidence::Unsupported
    );
}

#[test]
fn a_capability_free_source_publishes_as_unsupported() {
    let store = store();
    let claim = claim(&store, "unsupported-caps", 100);
    let source = store.session(&claim.key).unwrap().unwrap();
    let mut published = published_pass(&source);
    published.analysis.analyzed_generation = claim.source_generation;
    // Replace the whole evidence value rather than only its
    // `capabilities` field: every fact this session's evidence
    // already carries (`context`, `models`, ...) was derived from
    // the real Claude capability set the fixture streamed under,
    // and swapping only the `capabilities` field afterward leaves
    // that already-derived evidence untouched.
    published.evidence = Some(evidence_with(no_capabilities()));

    assert!(apply_outcome(&store, &claim, &published, 100).unwrap());
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().status,
        EvidenceStatus::Unsupported
    );
}

/// The widened cohort now enqueues generic-JSONL agents (Copilot, Cline,
/// Kiro, Amp, Windsurf) alongside the vendors with a dedicated adapter.
/// Before capabilities_for_vendor was made total, a Published pass with
/// no evidence (the legacy analyze_sources_with path's shape) made
/// apply_outcome error, leaving the claim stuck reprocessing forever.
/// With every vendor streaming through a real `SourceCapabilities`
/// profile, a generic-agent session must instead complete terminally —
/// here, `SourceCapabilities::generic()` is all-unset, so no detector is
/// eligible and the terminal status is `Unsupported`, never a stuck
/// `Processing` claim or an `apply_outcome` error.
#[tokio::test]
async fn a_generic_agent_session_completes_terminally_through_process_next() {
    let store = store();
    let mut copilot_record = record("generic-terminal");
    copilot_record.key.agent = "copilot".to_owned();
    store
        .upsert_sessions(&[copilot_record], &crate::agents::evidence_cohort())
        .unwrap();
    let handle = WorkerHandle::default();
    let runner = |record: &SessionRecord, _: PassSignal, _: i64| {
        let pass = generic_published_pass(record);
        Box::pin(async move { pass }) as PassFuture
    };

    let processed = process_next(&store, &handle, &|| 100, &runner, &|_| {})
        .await
        .unwrap();
    assert!(processed);

    let key = SessionKey::new("native", "copilot", "generic-terminal");
    let evidence = store.evidence(&key).unwrap().unwrap();
    assert_eq!(
        evidence.status,
        EvidenceStatus::Unsupported,
        "a capability-free source publishes as unsupported, not stuck processing"
    );
    assert_ne!(evidence.status, EvidenceStatus::Processing);
}

#[test]
fn backoff_grows_and_saturates() {
    let values: Vec<_> = (0..12).map(backoff_secs).collect();
    assert_eq!(values[0], BACKOFF_BASE_SECS);
    assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(*values.last().unwrap(), BACKOFF_MAX_SECS);
}

#[test]
fn a_repeatedly_changing_transcript_backs_off_without_spinning() {
    let store = store();
    let mut now = 100;
    for attempt in 0..3 {
        let claim = if attempt == 0 {
            claim(&store, "changing", now)
        } else {
            store
                .claim_next_evidence(&crate::agents::evidence_cohort(), now, LEASE_SECS)
                .unwrap()
                .unwrap()
        };
        assert!(
            apply_outcome(
                &store,
                &claim,
                &failed_pass(PassOutcome::SourceChanged),
                now
            )
            .unwrap()
        );
        let row = store.evidence(&claim.key).unwrap().unwrap();
        assert_eq!(row.status, EvidenceStatus::Pending);
        assert!(row.next_attempt_at_epoch.unwrap() > now);
        now = row.next_attempt_at_epoch.unwrap();
    }
}

#[test]
fn source_changed_leaves_both_stored_projections_untouched() {
    let store = store();
    let first = claim(&store, "unchanged-projections", 100);
    let source = store.session(&first.key).unwrap().unwrap();
    let mut published = published_pass(&source);
    published.analysis.analyzed_generation = first.source_generation;
    assert!(apply_outcome(&store, &first, &published, 100).unwrap());
    let analysis_before = store.analysis(&first.key).unwrap().unwrap();
    let evidence_before = store.evidence(&first.key).unwrap().unwrap();
    let mut changed = source;
    changed.source_fingerprint = Some("sv1:changed".into());
    store
        .upsert_sessions(&[changed], &crate::agents::evidence_cohort())
        .unwrap();
    let second = store
        .claim_next_evidence(&crate::agents::evidence_cohort(), 101, LEASE_SECS)
        .unwrap()
        .unwrap();

    assert!(
        apply_outcome(
            &store,
            &second,
            &failed_pass(PassOutcome::SourceChanged),
            101,
        )
        .unwrap()
    );
    assert_eq!(
        store.analysis(&first.key).unwrap().unwrap(),
        analysis_before
    );
    let evidence_after = store.evidence(&first.key).unwrap().unwrap();
    assert_eq!(evidence_after.evidence_json, evidence_before.evidence_json);
    assert_eq!(evidence_after.status, EvidenceStatus::Pending);
    assert_eq!(evidence_after.retry_count, 1);
    assert_eq!(
        evidence_after.next_attempt_at_epoch,
        Some(101 + BACKOFF_BASE_SECS)
    );
}

#[tokio::test]
async fn a_hot_source_does_not_starve_a_stable_session() {
    let store = store();
    let hot = claim(&store, "hot", 100);
    apply_outcome(&store, &hot, &failed_pass(PassOutcome::SourceChanged), 100).unwrap();
    store
        .upsert_sessions(&[record("stable")], &crate::agents::evidence_cohort())
        .unwrap();
    let next = store
        .claim_next_evidence(&crate::agents::evidence_cohort(), 101, LEASE_SECS)
        .unwrap()
        .unwrap();
    assert_eq!(next.key.session_id, "stable");
}

#[test]
fn an_un_stat_able_source_stops_being_claimed() {
    let store = store();
    let claim = claim(&store, "missing", 100);
    apply_outcome(
        &store,
        &claim,
        &failed_pass(PassOutcome::SourceMissing),
        100,
    )
    .unwrap();
    let row = store.evidence(&claim.key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Failed);
    assert_eq!(row.next_attempt_at_epoch, None);
    assert!(
        store
            .claim_next_evidence(&crate::agents::evidence_cohort(), 101, LEASE_SECS)
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_unsupported_pass_is_terminal_at_once() {
    let store = store();
    let claim = claim(&store, "unsupported", 100);
    apply_outcome(&store, &claim, &failed_pass(PassOutcome::Unsupported), 100).unwrap();
    let row = store.evidence(&claim.key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Failed);
    assert!(store.analysis(&claim.key).unwrap().is_none());
    assert!(row.evidence_json.is_none());
}

#[test]
fn an_unreadable_source_is_terminal_after_the_attempt_cap() {
    let store = store();
    let mut now = 100;
    for attempt in 0..=MAX_EVIDENCE_ATTEMPTS {
        let claim = if attempt == 0 {
            claim(&store, "unreadable", now)
        } else {
            store
                .claim_next_evidence(&crate::agents::evidence_cohort(), now, LEASE_SECS)
                .unwrap()
                .unwrap()
        };
        apply_outcome(
            &store,
            &claim,
            &failed_pass(PassOutcome::Unreadable(UnreadableReason::AdapterFailed)),
            now,
        )
        .unwrap();
        let row = store.evidence(&claim.key).unwrap().unwrap();
        if attempt == MAX_EVIDENCE_ATTEMPTS {
            assert_eq!(row.status, EvidenceStatus::Failed);
        } else {
            assert_eq!(row.status, EvidenceStatus::Pending);
            now = row.next_attempt_at_epoch.unwrap();
        }
    }
}

#[test]
fn a_cancelled_pass_does_not_consume_a_retry() {
    let store = store();
    let claim = claim(&store, "cancelled", 100);
    apply_outcome(
        &store,
        &claim,
        &failed_pass(PassOutcome::Unreadable(UnreadableReason::Cancelled)),
        100,
    )
    .unwrap();
    let row = store.evidence(&claim.key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Pending);
    assert_eq!(row.retry_count, 0);
    assert_eq!(
        row.last_error.as_deref(),
        Some("source-unreadable:cancelled")
    );
}

#[test]
fn an_unreadable_last_error_carries_the_reason_suffix() {
    let store = store();
    let claim = claim(&store, "no-events", 100);
    apply_outcome(
        &store,
        &claim,
        &failed_pass(PassOutcome::Unreadable(UnreadableReason::NoEvents)),
        100,
    )
    .unwrap();
    let row = store.evidence(&claim.key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Pending);
    assert_eq!(row.retry_count, 1);
    assert_eq!(
        row.last_error.as_deref(),
        Some("source-unreadable:no-events")
    );
}

#[test]
fn a_newer_pending_generation_preserves_the_last_payload() {
    let store = store();
    let claim = claim(&store, "payload", 100);
    let source = store.session(&claim.key).unwrap().unwrap();
    let mut published = published_pass(&source);
    published.analysis.analyzed_generation = claim.source_generation;
    assert!(apply_outcome(&store, &claim, &published, 100).unwrap());
    let payload = store.evidence(&claim.key).unwrap().unwrap().evidence_json;
    let mut next = record("payload");
    next.source_fingerprint = Some("sv1:payload-next".into());
    store
        .upsert_sessions(&[next], &crate::agents::evidence_cohort())
        .unwrap();
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().evidence_json,
        payload
    );
}

#[test]
fn a_restart_loses_no_pending_work() {
    let store = store();
    let first = claim(&store, "restart", 100);
    let reclaimed = store
        .claim_next_evidence(&crate::agents::evidence_cohort(), 401, LEASE_SECS)
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.key, first.key);
    assert!(reclaimed.claim_fence > first.claim_fence);
}

#[test]
fn an_active_session_is_claimable_at_once() {
    let store = store();
    let mut active = record("active");
    active.updated_at_epoch = Some(100);
    store
        .upsert_sessions(&[active], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(
        store
            .claim_next_evidence(&crate::agents::evidence_cohort(), 100, LEASE_SECS)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn progress_renews_the_lease() {
    let store = Arc::new(store());
    store
        .upsert_sessions(&[record("progress")], &crate::agents::evidence_cohort())
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let clock = Arc::new(AtomicI64::new(100));
    let signal = Arc::new(Mutex::new(None::<PassSignal>));
    let release = Arc::new(Notify::new());
    let task_store = Arc::clone(&store);
    let task_handle = Arc::clone(&handle);
    let task_clock = Arc::clone(&clock);
    let task_signal = Arc::clone(&signal);
    let task_release = Arc::clone(&release);
    let task = tokio::spawn(async move {
        let runner = move |_: &SessionRecord, pass_signal: PassSignal, _: i64| {
            *task_signal.lock().unwrap() = Some(pass_signal);
            let release = Arc::clone(&task_release);
            Box::pin(async move {
                release.notified().await;
                failed_pass(PassOutcome::SourceMissing)
            }) as PassFuture
        };
        process_next(
            &task_store,
            &task_handle,
            &|| task_clock.load(Ordering::SeqCst),
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    });
    let pass_signal = captured_signal(&signal).await;
    let key = SessionKey::new("native", "claude-code", "progress");

    assert!(!pass_signal.observe());
    clock.store(160, Ordering::SeqCst);
    wait_until(|| {
        store
            .evidence(&key)
            .unwrap()
            .unwrap()
            .lease_expires_at_epoch
            == Some(460)
    })
    .await;
    assert!(!pass_signal.observe());
    clock.store(220, Ordering::SeqCst);
    wait_until(|| {
        store
            .evidence(&key)
            .unwrap()
            .unwrap()
            .lease_expires_at_epoch
            == Some(520)
    })
    .await;

    release.notify_one();
    assert!(task.await.unwrap());
}

#[tokio::test]
async fn a_stalled_pass_stops_renewing() {
    let store = Arc::new(store());
    store
        .upsert_sessions(&[record("stalled")], &crate::agents::evidence_cohort())
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let clock = Arc::new(AtomicI64::new(100));
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());
    let task_store = Arc::clone(&store);
    let task_handle = Arc::clone(&handle);
    let task_clock = Arc::clone(&clock);
    let task_entered = Arc::clone(&entered);
    let task_release = Arc::clone(&release);
    let task = tokio::spawn(async move {
        let runner = move |_: &SessionRecord, _: PassSignal, _: i64| {
            task_entered.store(true, Ordering::SeqCst);
            let release = Arc::clone(&task_release);
            Box::pin(async move {
                release.notified().await;
                failed_pass(PassOutcome::SourceMissing)
            }) as PassFuture
        };
        process_next(
            &task_store,
            &task_handle,
            &|| task_clock.load(Ordering::SeqCst),
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    });
    wait_until(|| entered.load(Ordering::SeqCst)).await;
    let key = SessionKey::new("native", "claude-code", "stalled");
    let first = store.evidence(&key).unwrap().unwrap();

    clock.store(160, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(
        store
            .evidence(&key)
            .unwrap()
            .unwrap()
            .lease_expires_at_epoch,
        Some(100 + LEASE_SECS)
    );
    clock.store(401, Ordering::SeqCst);
    let reclaimed = store
        .claim_next_evidence(&crate::agents::evidence_cohort(), 401, LEASE_SECS)
        .unwrap()
        .unwrap();
    assert!(reclaimed.claim_fence > first.claim_fence);

    release.notify_one();
    assert!(task.await.unwrap());
}

#[tokio::test]
async fn a_lost_renewal_cancels_without_a_post_claim_write() {
    let store = Arc::new(store());
    store
        .upsert_sessions(&[record("lost")], &crate::agents::evidence_cohort())
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let clock = Arc::new(AtomicI64::new(100));
    let signal = Arc::new(Mutex::new(None::<PassSignal>));
    let release = Arc::new(Notify::new());
    let task_store = Arc::clone(&store);
    let task_handle = Arc::clone(&handle);
    let task_clock = Arc::clone(&clock);
    let task_signal = Arc::clone(&signal);
    let task_release = Arc::clone(&release);
    let task = tokio::spawn(async move {
        let runner = move |_: &SessionRecord, pass_signal: PassSignal, _: i64| {
            *task_signal.lock().unwrap() = Some(pass_signal);
            let release = Arc::clone(&task_release);
            Box::pin(async move {
                release.notified().await;
                failed_pass(PassOutcome::SourceMissing)
            }) as PassFuture
        };
        process_next(
            &task_store,
            &task_handle,
            &|| task_clock.load(Ordering::SeqCst),
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    });
    let pass_signal = captured_signal(&signal).await;
    let key = SessionKey::new("native", "claude-code", "lost");
    let successor = store
        .claim_next_evidence(&crate::agents::evidence_cohort(), 401, LEASE_SECS)
        .unwrap()
        .unwrap();
    let before = store.evidence(&key).unwrap().unwrap();

    assert!(!pass_signal.observe());
    clock.store(402, Ordering::SeqCst);
    wait_until(|| pass_signal.observe()).await;
    release.notify_one();
    assert!(task.await.unwrap());

    assert_eq!(store.evidence(&key).unwrap().unwrap(), before);
    assert_eq!(successor.claim_fence, before.claim_fence);
    assert!(store.analysis(&key).unwrap().is_none());
}

#[tokio::test]
async fn a_stale_pass_cannot_affect_the_next_claim() {
    let store = Arc::new(store());
    store
        .upsert_sessions(&[record("stale-pass")], &crate::agents::evidence_cohort())
        .unwrap();
    let handle = Arc::new(WorkerHandle::default());
    let clock = Arc::new(AtomicI64::new(100));
    let first_signal_slot = Arc::new(Mutex::new(None::<PassSignal>));
    let first_release = Arc::new(Notify::new());
    let first_store = Arc::clone(&store);
    let first_handle = Arc::clone(&handle);
    let first_clock = Arc::clone(&clock);
    let first_task_signal = Arc::clone(&first_signal_slot);
    let first_task_release = Arc::clone(&first_release);
    let first_task = tokio::spawn(async move {
        let runner = move |_: &SessionRecord, pass_signal: PassSignal, _: i64| {
            *first_task_signal.lock().unwrap() = Some(pass_signal);
            let release = Arc::clone(&first_task_release);
            Box::pin(async move {
                release.notified().await;
                failed_pass(PassOutcome::SourceMissing)
            }) as PassFuture
        };
        process_next(
            &first_store,
            &first_handle,
            &|| first_clock.load(Ordering::SeqCst),
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    });
    let first_signal = captured_signal(&first_signal_slot).await;
    let mut changed = record("stale-pass");
    changed.source_fingerprint = Some("sv1:stale-pass-next".into());
    assert!(!first_signal.observe());
    store
        .upsert_sessions(&[changed], &crate::agents::evidence_cohort())
        .unwrap();
    clock.store(160, Ordering::SeqCst);
    wait_until(|| first_signal.observe()).await;
    first_release.notify_one();
    assert!(first_task.await.unwrap());

    let second_signal_slot = Arc::new(Mutex::new(None::<PassSignal>));
    let second_release = Arc::new(Notify::new());
    let second_store = Arc::clone(&store);
    let second_handle = Arc::clone(&handle);
    let second_clock = Arc::clone(&clock);
    let second_task_signal = Arc::clone(&second_signal_slot);
    let second_task_release = Arc::clone(&second_release);
    let second_task = tokio::spawn(async move {
        let runner = move |record: &SessionRecord, pass_signal: PassSignal, _: i64| {
            *second_task_signal.lock().unwrap() = Some(pass_signal);
            let pass = published_pass(record);
            let release = Arc::clone(&second_task_release);
            Box::pin(async move {
                release.notified().await;
                pass
            }) as PassFuture
        };
        process_next(
            &second_store,
            &second_handle,
            &|| second_clock.load(Ordering::SeqCst),
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    });
    let second_signal = captured_signal(&second_signal_slot).await;
    let key = SessionKey::new("native", "claude-code", "stale-pass");
    let successor_lease = store
        .evidence(&key)
        .unwrap()
        .unwrap()
        .lease_expires_at_epoch;

    first_signal.cancel();
    assert!(first_signal.observe());
    clock.store(220, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(
        store
            .evidence(&key)
            .unwrap()
            .unwrap()
            .lease_expires_at_epoch,
        successor_lease
    );
    assert!(!second_signal.observe());

    second_release.notify_one();
    assert!(second_task.await.unwrap());
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );
    assert!(store.analysis(&key).unwrap().is_some());
}

#[tokio::test]
async fn permits_are_held_before_the_pass_is_scheduled() {
    let store = store();
    store
        .upsert_sessions(&[record("permit")], &crate::agents::evidence_cohort())
        .unwrap();
    let handle = WorkerHandle::default();
    let permit = handle.permits.cpu.acquire().await.unwrap();
    let entered = Arc::new(AtomicBool::new(false));
    let entered_by_pass = entered.clone();
    let runner = move |_: &SessionRecord, _: PassSignal, _: i64| {
        entered_by_pass.store(true, Ordering::SeqCst);
        Box::pin(async { failed_pass(PassOutcome::SourceMissing) }) as PassFuture
    };
    let future = process_next(&store, &handle, &|| 100, &runner, &|_| {});
    tokio::pin!(future);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut future)
            .await
            .is_err()
    );
    assert!(!entered.load(Ordering::SeqCst));
    drop(permit);
}

#[tokio::test]
async fn the_store_is_lockable_while_a_pass_runs() {
    let store = store();
    store
        .upsert_sessions(&[record("lockable")], &crate::agents::evidence_cohort())
        .unwrap();
    let pending = Arc::new(Notify::new());
    let pending_pass = pending.clone();
    let runner = move |_: &SessionRecord, _: PassSignal, _: i64| {
        let pending = pending_pass.clone();
        Box::pin(async move {
            pending.notified().await;
            failed_pass(PassOutcome::SourceMissing)
        }) as PassFuture
    };
    let handle = WorkerHandle::default();
    let future = process_next(&store, &handle, &|| 100, &runner, &|_| {});
    tokio::pin!(future);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut future)
            .await
            .is_err()
    );
    assert!(
        store
            .session(&SessionKey::new("native", "claude-code", "lockable"))
            .unwrap()
            .is_some()
    );
    pending.notify_one();
    assert!(future.await.unwrap());
}

#[tokio::test]
async fn a_published_completion_announces_one_list_entry() {
    let store = store();
    store
        .upsert_sessions(&[record("announcement")], &crate::agents::evidence_cohort())
        .unwrap();
    let runner = |record: &SessionRecord, _: PassSignal, _: i64| {
        let pass = published_pass(record);
        Box::pin(async move { pass }) as PassFuture
    };
    let announced = Mutex::new(Vec::new());

    assert!(
        process_next(
            &store,
            &WorkerHandle::default(),
            &|| 100,
            &runner,
            &|entry| {
                announced.lock().unwrap().push(entry);
            }
        )
        .await
        .unwrap()
    );
    let entries = announced.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id, "announcement");
    // The synthetic pass has no priceable model breakdown, so this test checks the event contract only.
}

#[tokio::test]
async fn a_backed_off_outcome_announces_nothing() {
    let store = store();
    store
        .upsert_sessions(
            &[record("no-announcement")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let runner = |_: &SessionRecord, _: PassSignal, _: i64| {
        Box::pin(async { failed_pass(PassOutcome::SourceChanged) }) as PassFuture
    };
    let announced = Mutex::new(Vec::new());

    assert!(
        process_next(
            &store,
            &WorkerHandle::default(),
            &|| 100,
            &runner,
            &|entry| {
                announced.lock().unwrap().push(entry);
            }
        )
        .await
        .unwrap()
    );
    assert!(announced.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_changed_source_backs_off_through_the_worker() {
    let store = store();
    store
        .upsert_sessions(
            &[record("worker-changed")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let runner = |_: &SessionRecord, _: PassSignal, _: i64| {
        Box::pin(async { failed_pass(PassOutcome::SourceChanged) }) as PassFuture
    };

    process_next(&store, &WorkerHandle::default(), &|| 100, &runner, &|_| {})
        .await
        .unwrap();
    let key = SessionKey::new("native", "claude-code", "worker-changed");
    let row = store.evidence(&key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Pending);
    assert_eq!(row.retry_count, 1);
    assert_eq!(row.next_attempt_at_epoch, Some(100 + BACKOFF_BASE_SECS));
}

#[tokio::test]
async fn an_unsupported_pass_is_terminal_through_the_worker() {
    let store = store();
    store
        .upsert_sessions(
            &[record("worker-unsupported")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let runner = |_: &SessionRecord, _: PassSignal, _: i64| {
        Box::pin(async { failed_pass(PassOutcome::Unsupported) }) as PassFuture
    };

    process_next(&store, &WorkerHandle::default(), &|| 100, &runner, &|_| {})
        .await
        .unwrap();
    let key = SessionKey::new("native", "claude-code", "worker-unsupported");
    let row = store.evidence(&key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Failed);
    assert_eq!(row.next_attempt_at_epoch, None);
}

#[tokio::test]
async fn a_missing_source_stops_being_claimed_through_the_worker() {
    let store = store();
    store
        .upsert_sessions(
            &[record("worker-missing")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let runner = |_: &SessionRecord, _: PassSignal, _: i64| {
        Box::pin(async { failed_pass(PassOutcome::SourceMissing) }) as PassFuture
    };

    process_next(&store, &WorkerHandle::default(), &|| 100, &runner, &|_| {})
        .await
        .unwrap();
    let key = SessionKey::new("native", "claude-code", "worker-missing");
    let row = store.evidence(&key).unwrap().unwrap();
    assert_eq!(row.status, EvidenceStatus::Failed);
    assert!(
        store
            .claim_next_evidence(&crate::agents::evidence_cohort(), 401, LEASE_SECS)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_unreadable_source_reaches_the_cap_through_the_worker() {
    let store = store();
    store
        .upsert_sessions(
            &[record("worker-unreadable")],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let clock = AtomicI64::new(100);
    let runner = |_: &SessionRecord, _: PassSignal, _: i64| {
        Box::pin(async { failed_pass(PassOutcome::Unreadable(UnreadableReason::AdapterFailed)) })
            as PassFuture
    };
    let key = SessionKey::new("native", "claude-code", "worker-unreadable");

    for attempt in 0..=MAX_EVIDENCE_ATTEMPTS {
        process_next(
            &store,
            &WorkerHandle::default(),
            &|| clock.load(Ordering::SeqCst),
            &runner,
            &|_| {},
        )
        .await
        .unwrap();
        let row = store.evidence(&key).unwrap().unwrap();
        if attempt == MAX_EVIDENCE_ATTEMPTS {
            assert_eq!(row.status, EvidenceStatus::Failed);
            assert_eq!(row.next_attempt_at_epoch, None);
        } else {
            assert_eq!(row.status, EvidenceStatus::Pending);
            clock.store(row.next_attempt_at_epoch.unwrap(), Ordering::SeqCst);
        }
    }
}

#[tokio::test]
async fn a_published_pass_leaves_the_expected_turn_rows_under_its_claim_fence() {
    let store = store();
    let target = record("turn-rows-worker");
    store
        .upsert_sessions(
            std::slice::from_ref(&target),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    // A custom analyzer, like the fixture-backed tests above: it
    // bypasses real filesystem discovery, but runs the real
    // `evidence_pass_with_turn_rows` -> `stream_vendor_with_hooks` ->
    // `TurnRowSink` -> `FencedTurnRowStore` path the worker uses in
    // production, so this test proves rows a published pass writes
    // actually land under the claim's fence.
    let analyzer = |_agent: AgentKind,
                    session_id: String,
                    _wsl_distro: Option<String>,
                    claimed: analysis::ClaimedSource,
                    signal: PassSignal,
                    turn_row_store: Option<Arc<dyn TurnRowStore>>| {
        Box::pin(async move {
            let mut pass = analysis::evidence_pass_with_turn_rows(
                &[SessionInput {
                    agent: "claude".into(),
                    session_id,
                    source: RawSource::Jsonl(concat!(
                        r#"{"type":"assistant","timestamp":100,"message":{"id":"m1","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}"#,
                        "\n",
                        r#"{"type":"assistant","timestamp":200,"message":{"id":"m2","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":4,"output_tokens":6},"content":[]}}"#,
                        "\n",
                    )
                    .into()),
                }],
                &|| signal.observe(),
                turn_row_store,
            );
            if let Some(fingerprint) = claimed.fingerprint {
                pass.analysis.fingerprint = fingerprint;
            }
            pass
        }) as PassFuture
    };
    let store_for_runner = store.clone();
    let runner = move |record: &SessionRecord, signal: PassSignal, claim_fence: i64| {
        run_record_pass_with(
            record,
            signal,
            claim_fence,
            store_for_runner.clone(),
            &analyzer,
        )
    };

    assert!(
        process_next(&store, &WorkerHandle::default(), &|| 100, &runner, &|_| {})
            .await
            .unwrap()
    );

    let evidence = store.evidence(&target.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Ready);
    assert_eq!(
        store
            .count_turn_rows_for_session(&target.key, evidence.claim_fence)
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn pi_file_flows_through_worker_persistence_and_report() {
    let data_dir = tempfile::tempdir().unwrap();
    let source_path = data_dir.path().join("synthetic-pi.jsonl");
    std::fs::write(
        &source_path,
        concat!(
            r#"{"type":"session","version":3,"timestamp":"2026-01-01T00:00:00Z"}"#,
            "\n",
            r#"{"type":"thinking_level_change","timestamp":"2026-01-01T00:00:01Z","thinkingLevel":"low"}"#,
            "\n",
            r#"{"type":"message","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","api":"anthropic-messages","model":"model-a","usage":{"input":2,"output":3,"cacheRead":5,"cacheWrite":7},"content":[]}}"#,
            "\n"
        ),
    )
    .unwrap();
    let store = Store::open(data_dir.path()).unwrap();
    let mut pi = record("pi-worker-report");
    pi.key.agent = "pi".to_owned();
    pi.source_label = source_path.to_string_lossy().into_owned();
    pi.source_fingerprint = Some("sv1:synthetic-pi-worker".to_owned());
    store
        .upsert_sessions(std::slice::from_ref(&pi), &crate::agents::evidence_cohort())
        .unwrap();
    let pi_source = source_path.clone();
    let analyzer = move |agent: AgentKind,
                         session_id: String,
                         wsl_distro: Option<String>,
                         claimed: analysis::ClaimedSource,
                         signal: PassSignal,
                         turn_row_store: Option<Arc<dyn TurnRowStore>>| {
        if agent != AgentKind::Pi || session_id != "pi-worker-report" {
            return Box::pin(async move {
                analysis::analyze_for_evidence(
                    agent,
                    &session_id,
                    wsl_distro.as_deref(),
                    claimed,
                    signal,
                    turn_row_store,
                )
                .await
            }) as PassFuture;
        }
        let input = antiburn_local::analysis::SessionInput {
            agent: crate::agents::vendor_label(agent).to_owned(),
            session_id,
            source: antiburn_local::analysis::RawSource::File(pi_source.clone()),
        };
        Box::pin(async move {
            let mut pass = analysis::evidence_pass_with_turn_rows(
                &[input],
                &|| signal.observe(),
                turn_row_store,
            );
            if let Some(fingerprint) = claimed.fingerprint {
                pass.analysis.fingerprint = fingerprint;
            }
            pass
        }) as PassFuture
    };
    let store_for_runner = store.clone();
    let runner = move |record: &SessionRecord, signal: PassSignal, claim_fence: i64| {
        run_record_pass_with(
            record,
            signal,
            claim_fence,
            store_for_runner.clone(),
            &analyzer,
        )
    };

    assert!(
        process_next(
            &store,
            &WorkerHandle::default(),
            &|| 1_767_225_610,
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    );
    let stored = store.evidence(&pi.key).unwrap().unwrap();
    assert_eq!(stored.status, EvidenceStatus::Ready);
    let evidence_json = stored.evidence_json.as_deref().unwrap();
    assert!(evidence_json.contains("\"schemaRevision\":12"));
    let evidence: SessionEvidence = serde_json::from_str(evidence_json).unwrap();
    assert_eq!(evidence.capabilities, SourceCapabilities::pi());
    assert_eq!(evidence.schema_revision, 12);

    let report = crate::insights_report::reduce_report(
        data_dir.path().to_path_buf(),
        crate::insights_report::ReportRequest {
            environment_key: "native".to_owned(),
            window: antiburn_local::insights::ReportWindow {
                start_epoch: 1_767_225_000,
                end_epoch: 1_767_226_000,
            },
            computed_at_epoch: 1_767_226_000,
        },
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();
    assert_eq!(report.assessed_sessions, 1);
    assert_eq!(
        report.detectors[DetectorId::ModelOverthinking.index()].assessed,
        1
    );
    assert_eq!(
        report.detectors[DetectorId::OldModelUsage.index()].assessed,
        1
    );
}

#[test]
fn errors_carry_no_transcript_content() {
    const SOURCE_CONTENT: &str = "PRIVATE_TRANSCRIPT_MARKER";
    const EVIDENCE_ERROR_UNREADABLE_ADAPTER_FAILED: &str = "source-unreadable:adapter-failed";
    let mappings = [
        (PassOutcome::SourceChanged, EVIDENCE_ERROR_SOURCE_CHANGED),
        (PassOutcome::SourceMissing, EVIDENCE_ERROR_SOURCE_MISSING),
        (
            PassOutcome::Unreadable(UnreadableReason::AdapterFailed),
            EVIDENCE_ERROR_UNREADABLE_ADAPTER_FAILED,
        ),
        (PassOutcome::Unsupported, EVIDENCE_ERROR_UNSUPPORTED),
    ];
    let allowed = [
        EVIDENCE_ERROR_SOURCE_CHANGED,
        EVIDENCE_ERROR_SOURCE_MISSING,
        EVIDENCE_ERROR_UNREADABLE_ADAPTER_FAILED,
        EVIDENCE_ERROR_UNSUPPORTED,
    ];

    for (index, (outcome, expected)) in mappings.into_iter().enumerate() {
        let store = store();
        let id = format!("{SOURCE_CONTENT}-{index}");
        let claim = claim(&store, &id, 100);
        assert!(apply_outcome(&store, &claim, &failed_pass(outcome), 100).unwrap());

        let error = store
            .evidence(&claim.key)
            .unwrap()
            .unwrap()
            .last_error
            .unwrap();
        assert_eq!(error, expected);
        assert!(allowed.contains(&error.as_str()));
        assert!(!error.contains(SOURCE_CONTENT));
    }
}

#[test]
fn a_permit_is_chosen_per_source_kind() {
    assert_eq!(permit_for_source_kind("file"), PermitKind::Source);
    assert_eq!(permit_for_source_kind("inline"), PermitKind::Source);
    assert_eq!(permit_for_source_kind("providerDb"), PermitKind::ProviderDb);
}

#[tokio::test]
async fn a_wake_releases_the_idle_wait() {
    let handle = WorkerHandle::default();
    handle.wake.notify_one();
    tokio::time::timeout(Duration::from_millis(10), handle.wake.notified())
        .await
        .unwrap();
}
