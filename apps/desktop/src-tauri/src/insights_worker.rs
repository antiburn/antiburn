//! Drains the durable transcript evidence queue outside the scan pass.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::{SessionEvidence, TurnRowStore};
use antiburn_local::insights::{DetectorId, eligible};
use antiburn_local::model::AgentKind;
use tauri::{Emitter, Manager};
use tokio::sync::{Notify, Semaphore};

use crate::analysis::{self, EvidencePass, PassOutcome, PassSignal, UnreadableReason};
use crate::commands;
use crate::dto::ActivityEntry;
use crate::fork_lineage;
use crate::store::{
    EvidenceClaim, EvidenceCompletion, EvidenceFailure, FencedTurnRowStore, PublishedEvidence,
    RelationKind, RelationRecord, SessionKey, SessionRecord, Store,
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
/// Joins [`EVIDENCE_ERROR_UNREADABLE`] to an [`UnreadableReason`]'s suffix
/// (`reason.as_error_suffix()`) in a persisted `lastError`, for example
/// `source-unreadable:no-events`. No stored or reachable code compares
/// `lastError` against the bare `EVIDENCE_ERROR_UNREADABLE` string — see
/// `sessions_with_missing_source` for the one exact-match query, which
/// targets `EVIDENCE_ERROR_SOURCE_MISSING` instead — so the prefix can carry
/// this suffix safely.
const UNREADABLE_REASON_SEPARATOR: &str = ":";

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
/// The `i64` is the claim's fence: [`process_next`] already holds the claim
/// when it calls this, so it passes the fence through rather than the
/// runner re-deriving it.
pub(crate) type PassRunner<'a> =
    dyn Fn(&SessionRecord, PassSignal, i64) -> PassFuture + Send + Sync + 'a;
type RecordAnalyzer<'a> = dyn Fn(
        AgentKind,
        String,
        Option<String>,
        analysis::ClaimedSource,
        PassSignal,
        Option<Arc<dyn TurnRowStore>>,
    ) -> PassFuture
    + Send
    + Sync
    + 'a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermitKind {
    Source,
    ProviderDb,
}

/// `store` is a cheap handle (see [`Store`]'s doc comment): this clones it
/// once per pass into a [`FencedTurnRowStore`] stamped with `claim_fence`,
/// so turn rows this pass writes are attributable and cleaned up correctly
/// if the pass loses the claim race.
fn run_record_pass(
    record: &SessionRecord,
    signal: PassSignal,
    claim_fence: i64,
    store: Store,
) -> PassFuture {
    run_record_pass_with(
        record,
        signal,
        claim_fence,
        store,
        &|agent, session_id, wsl_distro, claimed, signal, turn_row_store| {
            Box::pin(async move {
                analysis::analyze_for_evidence(
                    agent,
                    &session_id,
                    wsl_distro.as_deref(),
                    claimed,
                    signal,
                    turn_row_store,
                )
                .await
            })
        },
    )
}

fn run_record_pass_with(
    record: &SessionRecord,
    signal: PassSignal,
    claim_fence: i64,
    store: Store,
    analyze: &RecordAnalyzer<'_>,
) -> PassFuture {
    let Some(agent) = crate::agents::kind_from_slug(&record.key.agent) else {
        return Box::pin(async { analysis::unsupported_evidence_pass() });
    };
    let writer: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
        store,
        record.key.clone(),
        claim_fence,
    ));
    analyze(
        agent,
        record.key.session_id.clone(),
        record.wsl_distro.clone(),
        analysis::ClaimedSource {
            fingerprint: record.source_fingerprint.clone(),
            generation: 0,
        },
        signal,
        Some(writer),
    )
}

pub fn spawn(app: &tauri::AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let store_handle: Store = (*app.state::<Store>()).clone();
        let run_pass = move |record: &SessionRecord, signal: PassSignal, claim_fence: i64| {
            run_record_pass(record, signal, claim_fence, store_handle.clone())
        };
        let announce_app = app.clone();
        let announce = move |entry: ActivityEntry| {
            let _ = announce_app.emit(commands::SESSION_ENTRY_CHANGED_EVENT, &entry);
        };
        let report_app = app.clone();
        let announce_idle = move || {
            let _ = report_app.emit(commands::CHECKS_REPORT_CHANGED_EVENT, ());
        };
        let clock = || unix_now();
        let store = app.state::<Store>();
        let handle = app.state::<WorkerHandle>();
        worker_loop(
            &store,
            &handle,
            &clock,
            &run_pass,
            &announce,
            &announce_idle,
        )
        .await;
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

pub(crate) fn permit_for_source_kind(source_kind: &str) -> PermitKind {
    match source_kind {
        "providerDb" => PermitKind::ProviderDb,
        _ => PermitKind::Source,
    }
}

/// Classifies a provider against the shipped detector fact
/// requirements. A source whose evidence satisfies no detector's
/// finding facts publishes Unsupported: its rows can never join an
/// assessed cohort, and the report's coverage denominator shows the
/// session as unsupported instead of ready.
fn published_status(evidence: &SessionEvidence) -> PublishedEvidence {
    let supported = DetectorId::ALL
        .into_iter()
        .any(|detector| eligible(detector, evidence));
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
            };
            store.publish_projections(
                &record,
                pass.analysis.started_at_epoch,
                &completion,
                &relations,
                &pass.source_outcomes,
            )
        }
        PassOutcome::SourceChanged => store.fail_evidence(
            claim,
            EvidenceFailure::Retry {
                next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
                counts_as_attempt: true,
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
        PassOutcome::Unreadable(reason) => {
            let last_error = format!(
                "{EVIDENCE_ERROR_UNREADABLE}{UNREADABLE_REASON_SEPARATOR}{}",
                reason.as_error_suffix()
            );
            if reason == UnreadableReason::Cancelled {
                // The source was never actually tried, so this retry must
                // not consume one of the claim's attempts.
                store.fail_evidence(
                    claim,
                    EvidenceFailure::Retry {
                        next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
                        counts_as_attempt: false,
                    },
                    &last_error,
                )
            } else if claim.retry_count < MAX_EVIDENCE_ATTEMPTS {
                store.fail_evidence(
                    claim,
                    EvidenceFailure::Retry {
                        next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
                        counts_as_attempt: true,
                    },
                    &last_error,
                )
            } else {
                store.fail_evidence(
                    claim,
                    EvidenceFailure::Failed {
                        revisions: analysis::projection_revisions(),
                    },
                    &last_error,
                )
            }
        }
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
    let Some(_agent) = crate::agents::kind_from_slug(&record.key.agent) else {
        let pass = analysis::unsupported_evidence_pass();
        apply_outcome(store, &claim, &pass, clock())?;
        return Ok(true);
    };
    let _cpu = handle
        .permits
        .cpu
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("CPU permit closed"))?;
    let permit = match permit_for_source_kind(&record.source_kind) {
        PermitKind::Source => &handle.permits.source,
        PermitKind::ProviderDb => &handle.permits.provider_db,
    };
    let _source = permit
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("source permit closed"))?;
    let signal = PassSignal::new();
    let mut pass = run_pass(&record, signal.clone(), claim.claim_fence);
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
    if published {
        fork_lineage::link_claude_fork(store, &claim.key)?;
    }
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
    announce_idle: &(dyn Fn() + Send + Sync),
) {
    let mut processed = false;
    loop {
        match process_next(store, handle, clock, run_pass, announce).await {
            Ok(true) => {
                processed = true;
                continue;
            }
            Ok(false) => {
                if processed {
                    processed = false;
                    announce_idle();
                }
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
mod tests;
