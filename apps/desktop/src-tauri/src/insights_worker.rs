//! Drains the durable transcript evidence queue outside the scan pass.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::SessionEvidence;
use antiburn_local::discovery::SessionSource;
use antiburn_local::insights::{DetectorId, requirements};
use antiburn_local::model::AgentKind;
use tauri::{Emitter, Manager};
use tokio::sync::{Notify, Semaphore};

use crate::analysis::{self, EvidencePass, PassOutcome, PassSignal};
use crate::commands;
use crate::dto::ActivityEntry;
use crate::store::{
    EvidenceClaim, EvidenceCompletion, EvidenceFailure, PublishedEvidence, RelationKind,
    RelationRecord, SessionKey, SessionRecord, Store,
};

pub(crate) const LEASE_SECS: i64 = 300;
pub(crate) const LEASE_RENEW_SECS: u64 = 60;
pub(crate) const IDLE_POLL_SECS: u64 = 60;
pub(crate) const BACKOFF_BASE_SECS: i64 = 30;
pub(crate) const BACKOFF_MAX_SECS: i64 = 900;
pub(crate) const MAX_EVIDENCE_ATTEMPTS: i64 = 5;
pub(crate) const EVIDENCE_ERROR_SOURCE_CHANGED: &str = "source-changed";
pub(crate) const EVIDENCE_ERROR_SOURCE_MISSING: &str = "source-missing";
pub(crate) const EVIDENCE_ERROR_UNREADABLE: &str = "source-unreadable";
pub(crate) const EVIDENCE_ERROR_UNSUPPORTED: &str = "source-unsupported";

struct Permits {
    cpu: Semaphore,
    source: Semaphore,
    provider_db: Semaphore,
}

impl Default for Permits {
    fn default() -> Self {
        Self {
            cpu: Semaphore::new(1),
            source: Semaphore::new(1),
            provider_db: Semaphore::new(1),
        }
    }
}

/// This handle wakes the worker and limits its shared processing resources.
#[derive(Default)]
pub struct WorkerHandle {
    wake: Notify,
    permits: Permits,
}

pub(crate) type PassFuture = Pin<Box<dyn Future<Output = EvidencePass> + Send>>;
pub(crate) type PassRunner<'a> =
    dyn Fn(&SessionRecord, PassSignal) -> PassFuture + Send + Sync + 'a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermitKind {
    Source,
    ProviderDb,
}

fn run_record_pass(record: &SessionRecord, signal: PassSignal) -> PassFuture {
    let Some(agent) = crate::agents::kind_from_slug(&record.key.agent) else {
        return Box::pin(async { analysis::unsupported_evidence_pass() });
    };
    let session_id = record.key.session_id.clone();
    let wsl_distro = record.wsl_distro.clone();
    let claimed = analysis::ClaimedSource {
        fingerprint: record.source_fingerprint.clone(),
        generation: 0,
    };
    Box::pin(async move {
        analysis::analyze_for_evidence(agent, &session_id, wsl_distro.as_deref(), claimed, signal)
            .await
    })
}

pub fn spawn(app: &tauri::AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let run_pass = |record: &SessionRecord, signal: PassSignal| run_record_pass(record, signal);
        let announce_app = app.clone();
        let announce = move |entry: ActivityEntry| {
            let _ = announce_app.emit(commands::SESSION_ENTRY_CHANGED_EVENT, &entry);
        };
        let clock = || unix_now();
        let store = app.state::<Store>();
        let handle = app.state::<WorkerHandle>();
        worker_loop(&store, &handle, &clock, &run_pass, &announce).await;
    })
}

pub fn wake(app: &tauri::AppHandle) {
    app.state::<WorkerHandle>().wake.notify_one();
}

pub(crate) fn backoff_secs(retry_count: i64) -> i64 {
    let exponent = u32::try_from(retry_count.max(0))
        .unwrap_or(u32::MAX)
        .min(30);
    BACKOFF_BASE_SECS
        .saturating_mul(1_i64.checked_shl(exponent).unwrap_or(i64::MAX))
        .min(BACKOFF_MAX_SECS)
}

pub(crate) fn permit_for(source: &SessionSource) -> PermitKind {
    match source {
        SessionSource::ProviderDb { .. } => PermitKind::ProviderDb,
        SessionSource::File(_) | SessionSource::Inline { .. } => PermitKind::Source,
    }
}

fn source_for_record(record: &SessionRecord, agent: AgentKind) -> SessionSource {
    match record.source_kind.as_str() {
        "providerDb" => SessionSource::ProviderDb {
            agent,
            db_path: PathBuf::from(&record.source_label),
            session_id: record.key.session_id.clone(),
        },
        "inline" => SessionSource::Inline {
            label: record.source_label.clone(),
            content: String::new(),
        },
        _ => SessionSource::File(PathBuf::from(&record.source_label)),
    }
}

/// Classifies a provider against the shipped detector prerequisite
/// sets. A source whose capability set satisfies no detector's
/// capability clauses publishes Unsupported: its rows can never
/// join an assessed cohort, and the report's coverage denominator
/// shows the session as unsupported instead of ready.
fn published_status(evidence: &SessionEvidence) -> PublishedEvidence {
    let supported = DetectorId::ALL.into_iter().any(|detector| {
        requirements(detector).capabilities.iter().all(|clause| {
            clause
                .iter()
                .any(|flag| flag.is_set(&evidence.capabilities))
        })
    });
    if supported {
        PublishedEvidence::Ready
    } else {
        PublishedEvidence::Unsupported
    }
}

pub(crate) fn completion_entry(store: &Store, key: &SessionKey, now: i64) -> Option<ActivityEntry> {
    let session = store.session(key).ok()??;
    let repositories = store.repositories().ok()?;
    commands::activity_entry(store, &repositories, session, now).ok()
}

pub(crate) fn apply_outcome(
    store: &Store,
    claim: &EvidenceClaim,
    pass: &EvidencePass,
    now: i64,
) -> anyhow::Result<bool> {
    match pass.outcome {
        PassOutcome::Published => {
            let record = pass
                .analysis
                .record(&claim.key)
                .ok_or_else(|| anyhow::anyhow!("published pass has no metrics projection"))?;
            let evidence = pass
                .evidence
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("published pass has no evidence projection"))?;
            let relations = pass
                .analysis
                .orchestration
                .as_ref()
                .map(|orchestration| {
                    orchestration
                        .members
                        .iter()
                        .map(|member| RelationRecord {
                            kind: RelationKind::Subagent,
                            related_id: member.subagent_id.clone(),
                            label: Some(member.label.clone()),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let completion = EvidenceCompletion {
                claim_fence: claim.claim_fence,
                status: published_status(evidence),
                evidence_schema_revision: evidence.schema_revision,
                evidence_json: serde_json::to_string(evidence)?,
                diagnostics_json: Some(serde_json::to_string(&evidence.diagnostics)?),
            };
            store.publish_projections(
                &record,
                pass.analysis.started_at_epoch,
                &completion,
                &relations,
            )
        }
        PassOutcome::SourceChanged => store.fail_evidence(
            claim,
            EvidenceFailure::Retry {
                next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
            },
            EVIDENCE_ERROR_SOURCE_CHANGED,
        ),
        PassOutcome::SourceMissing => store.fail_evidence(
            claim,
            EvidenceFailure::Failed {
                revisions: analysis::projection_revisions(),
            },
            EVIDENCE_ERROR_SOURCE_MISSING,
        ),
        PassOutcome::Unsupported => store.fail_evidence(
            claim,
            EvidenceFailure::Failed {
                revisions: analysis::projection_revisions(),
            },
            EVIDENCE_ERROR_UNSUPPORTED,
        ),
        PassOutcome::Unreadable if claim.retry_count < MAX_EVIDENCE_ATTEMPTS => store
            .fail_evidence(
                claim,
                EvidenceFailure::Retry {
                    next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
                },
                EVIDENCE_ERROR_UNREADABLE,
            ),
        PassOutcome::Unreadable => store.fail_evidence(
            claim,
            EvidenceFailure::Failed {
                revisions: analysis::projection_revisions(),
            },
            EVIDENCE_ERROR_UNREADABLE,
        ),
    }
}

#[cfg(not(test))]
fn lease_renew_interval() -> Duration {
    Duration::from_secs(LEASE_RENEW_SECS)
}

#[cfg(test)]
fn lease_renew_interval() -> Duration {
    Duration::from_secs(LEASE_RENEW_SECS).min(Duration::from_millis(10))
}

pub(crate) async fn process_next(
    store: &Store,
    handle: &WorkerHandle,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(ActivityEntry) + Send + Sync),
) -> anyhow::Result<bool> {
    let Some(claim) =
        store.claim_next_evidence(&crate::agents::evidence_cohort(), clock(), LEASE_SECS)?
    else {
        return Ok(false);
    };
    let Some(record) = store.session(&claim.key)? else {
        return Ok(true);
    };
    let Some(agent) = crate::agents::kind_from_slug(&record.key.agent) else {
        let pass = analysis::unsupported_evidence_pass();
        apply_outcome(store, &claim, &pass, clock())?;
        return Ok(true);
    };
    let source = source_for_record(&record, agent);
    let _cpu = handle
        .permits
        .cpu
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("CPU permit closed"))?;
    let permit = match permit_for(&source) {
        PermitKind::Source => &handle.permits.source,
        PermitKind::ProviderDb => &handle.permits.provider_db,
    };
    let _source = permit
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("source permit closed"))?;
    let signal = PassSignal::new();
    let mut pass = run_pass(&record, signal.clone());
    let mut progress = signal.progress();
    let result = loop {
        tokio::select! {
            result = &mut pass => break Some(result),
            () = tokio::time::sleep(lease_renew_interval()) => {
                let observed = signal.progress();
                if observed == progress {
                    continue;
                }
                progress = observed;
                if !store.renew_evidence_lease(&claim, clock(), LEASE_SECS)? {
                    signal.cancel();
                    let _ = pass.await;
                    break None;
                }
            }
        }
    };
    let Some(mut pass) = result else {
        return Ok(true);
    };
    pass.analysis.analyzed_generation = claim.source_generation;
    let applied = apply_outcome(store, &claim, &pass, clock())?;
    let published = applied && pass.outcome == PassOutcome::Published;
    if published && let Some(entry) = completion_entry(store, &claim.key, clock()) {
        announce(entry);
    }
    Ok(true)
}

pub(crate) async fn worker_loop(
    store: &Store,
    handle: &WorkerHandle,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(ActivityEntry) + Send + Sync),
) {
    loop {
        match process_next(store, handle, clock, run_pass, announce).await {
            Ok(true) => continue,
            Ok(false) => {
                tokio::select! {
                    () = handle.wake.notified() => {}
                    () = tokio::time::sleep(Duration::from_secs(IDLE_POLL_SECS)) => {}
                }
            }
            Err(error) => {
                ::tracing::error!(event = "insights_worker_failed", error = %error);
                tokio::time::sleep(Duration::from_secs(IDLE_POLL_SECS)).await;
            }
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    use antiburn_local::analysis::{
        EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput, SourceCapabilities,
        SourceKind,
    };

    use super::*;
    use crate::store::EvidenceStatus;

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
        }
    }

    fn published_pass(record: &SessionRecord) -> EvidencePass {
        let mut pass = analysis::evidence_pass(
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
        .evidence()
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
            quota_incidents: false,
            harness_version: false,
        }
    }

    #[tokio::test]
    async fn unknown_store_slug_is_rejected_instead_of_routing_to_claude() {
        let mut unknown = record("unknown");
        unknown.key.agent = "unknown-agent".to_owned();

        let pass = run_record_pass(&unknown, PassSignal::new()).await;

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
        published
            .evidence
            .as_mut()
            .expect("a published pass carries evidence")
            .capabilities = no_capabilities();

        assert!(apply_outcome(&store, &claim, &published, 100).unwrap());
        assert_eq!(
            store.evidence(&claim.key).unwrap().unwrap().status,
            EvidenceStatus::Unsupported
        );
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
        assert_eq!(
            evidence_after.diagnostics_json,
            evidence_before.diagnostics_json
        );
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
        assert!(row.diagnostics_json.is_none());
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
            apply_outcome(&store, &claim, &failed_pass(PassOutcome::Unreadable), now).unwrap();
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
            let runner = move |_: &SessionRecord, pass_signal: PassSignal| {
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
            let runner = move |_: &SessionRecord, _: PassSignal| {
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
            let runner = move |_: &SessionRecord, pass_signal: PassSignal| {
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
            let runner = move |_: &SessionRecord, pass_signal: PassSignal| {
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
            let runner = move |record: &SessionRecord, pass_signal: PassSignal| {
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
        let runner = move |_: &SessionRecord, _: PassSignal| {
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
        let runner = move |_: &SessionRecord, _: PassSignal| {
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
        let runner = |record: &SessionRecord, _: PassSignal| {
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
        let runner = |_: &SessionRecord, _: PassSignal| {
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
        let runner = |_: &SessionRecord, _: PassSignal| {
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
        let runner = |_: &SessionRecord, _: PassSignal| {
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
        let runner = |_: &SessionRecord, _: PassSignal| {
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
        let runner = |_: &SessionRecord, _: PassSignal| {
            Box::pin(async { failed_pass(PassOutcome::Unreadable) }) as PassFuture
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

    #[test]
    fn errors_carry_no_transcript_content() {
        const SOURCE_CONTENT: &str = "PRIVATE_TRANSCRIPT_MARKER";
        let mappings = [
            (PassOutcome::SourceChanged, EVIDENCE_ERROR_SOURCE_CHANGED),
            (PassOutcome::SourceMissing, EVIDENCE_ERROR_SOURCE_MISSING),
            (PassOutcome::Unreadable, EVIDENCE_ERROR_UNREADABLE),
            (PassOutcome::Unsupported, EVIDENCE_ERROR_UNSUPPORTED),
        ];
        let allowed = [
            EVIDENCE_ERROR_SOURCE_CHANGED,
            EVIDENCE_ERROR_SOURCE_MISSING,
            EVIDENCE_ERROR_UNREADABLE,
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
        assert_eq!(
            permit_for(&SessionSource::File("a".into())),
            PermitKind::Source
        );
        assert_eq!(
            permit_for(&SessionSource::Inline {
                label: "a".into(),
                content: String::new()
            }),
            PermitKind::Source
        );
        assert_eq!(
            permit_for(&SessionSource::ProviderDb {
                agent: AgentKind::Claude,
                db_path: "a".into(),
                session_id: "s".into()
            }),
            PermitKind::ProviderDb
        );
    }

    #[tokio::test]
    async fn a_wake_releases_the_idle_wait() {
        let handle = WorkerHandle::default();
        handle.wake.notify_one();
        tokio::time::timeout(Duration::from_millis(10), handle.wake.notified())
            .await
            .unwrap();
    }
}
