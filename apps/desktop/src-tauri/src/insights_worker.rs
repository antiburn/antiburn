//! Drains the durable transcript evidence queue outside the scan pass.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::{SessionEvidence, TurnRowStore};
use antiburn_local::insights::{DetectorId, eligible};
use antiburn_local::model::AgentKind;
use tauri::{Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::analysis::{self, EvidencePass, PassOutcome, PassSignal, UnreadableReason};
use crate::analytics::ingested_incidents::{self, IngestedIncidents};
use crate::commands;
use crate::fork_lineage;
use crate::store::{
    EvidenceClaim, EvidenceCompletion, EvidenceFailure, FencedTurnRowStore, PublishedEvidence,
    RelationKind, RelationRecord, SessionKey, SessionRecord, Store,
};

pub(crate) const LEASE_SECS: i64 = 300;
pub(crate) const LEASE_RENEW_SECS: u64 = 60;
pub(crate) const IDLE_POLL_SECS: u64 = 60;
/// Parse several independent transcripts at once without saturating the machine.
const WORKER_CONCURRENCY: usize = 4;
/// How long `spawn` waits for the main window's first content-ready
/// notification before ramping to full concurrency on its own. Covers a
/// launch that never opens the main window (tray-only, HUD).
const WORKER_RAMP_SECS: u64 = 30;
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

/// The insights worker pool's shared backlog state: how many workers are
/// currently busy, how many evidence rows they have drained since the pool
/// last went idle, and when the current busy stretch began.
#[derive(Default)]
struct Backlog {
    active: usize,
    processed: usize,
    started_at: Option<Instant>,
}

/// This handle wakes the worker.
#[derive(Default)]
pub struct WorkerHandle {
    wake: Notify,
    backlog: Mutex<Backlog>,
    /// Ends `spawn`'s launch-time throttle early. See [`notify_ramp`].
    ramp: Notify,
}

impl WorkerHandle {
    /// Marks one worker's idle→busy transition. Returns the pending count
    /// only for the worker that takes the pool from zero to one active
    /// worker, so `worker_loop` logs the backlog's start once per stretch,
    /// not once per worker.
    fn note_backlog_busy(&self, store: &Store) -> Option<usize> {
        let mut backlog = self.backlog.lock().expect("backlog lock");
        backlog.active += 1;
        if backlog.active == 1 {
            backlog.started_at = Some(Instant::now());
            Some(
                store
                    .pending_evidence_count(&crate::agents::evidence_cohort())
                    .unwrap_or(0),
            )
        } else {
            None
        }
    }

    /// Counts one evidence row as processed in the current busy stretch.
    fn note_backlog_processed(&self) {
        self.backlog.lock().expect("backlog lock").processed += 1;
    }

    /// Marks one worker's busy→idle transition. Returns the drained total
    /// and its elapsed time only when this was the last busy worker and the
    /// stretch processed at least one row, resetting the counters for the
    /// next stretch.
    fn note_backlog_idle(&self) -> Option<(usize, u64)> {
        let mut backlog = self.backlog.lock().expect("backlog lock");
        if backlog.active == 0 {
            return None;
        }
        backlog.active -= 1;
        if backlog.active == 0 && backlog.processed > 0 {
            let elapsed_ms = backlog
                .started_at
                .map(|started_at| started_at.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let processed = backlog.processed;
            backlog.processed = 0;
            backlog.started_at = None;
            Some((processed, elapsed_ms))
        } else {
            None
        }
    }

    /// Whether the pool has at least one worker busy on the backlog right
    /// now. Backs the `get_insights_backlog` command's initial read.
    pub fn backlog_active(&self) -> bool {
        self.backlog.lock().expect("backlog lock").active > 0
    }
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
        Option<String>,
    ) -> PassFuture
    + Send
    + Sync
    + 'a;

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
        &|agent,
          session_id,
          wsl_distro,
          claimed,
          signal,
          turn_row_store,
          fork_parent_session_id| {
            Box::pin(async move {
                analysis::analyze_for_evidence(
                    agent,
                    &session_id,
                    wsl_distro.as_deref(),
                    claimed,
                    signal,
                    turn_row_store,
                    fork_parent_session_id,
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
    // The relation changes what the adapter counts (a linked Claude fork
    // excludes its inherited prefix), so this pass needs it fresh every
    // time, not cached on `record`. A lookup failure reads the same as "no
    // parent known yet" — the pass still runs, just without the skip set.
    let fork_parent_session_id = store.fork_parent(&record.key).ok().flatten();
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
        fork_parent_session_id,
    )
}

/// The first minute after a cold launch runs one worker, so the main
/// window's first paint competes with a single parse thread rather than
/// [`WORKER_CONCURRENCY`]. Full concurrency resumes as soon as the main
/// window reports its content ready, or after [`WORKER_RAMP_SECS`],
/// whichever comes first.
pub fn spawn(app: &tauri::AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut workers = JoinSet::new();
        workers.spawn(run_worker(app.clone()));
        let handle = app.state::<WorkerHandle>();
        let (reason, elapsed) =
            await_ramp(&handle.ramp, Duration::from_secs(WORKER_RAMP_SECS)).await;
        for _ in 1..WORKER_CONCURRENCY {
            workers.spawn(run_worker(app.clone()));
        }
        ::tracing::info!(
            event = "insights_worker_ramped",
            workers = WORKER_CONCURRENCY,
            reason,
            elapsed_ms = elapsed.as_millis() as u64
        );
        while workers.join_next().await.is_some() {}
    })
}

/// Waits for `ramp` or `timeout`, whichever comes first. A small function
/// so a test with paused time can drive both branches directly. Times
/// itself against tokio's own clock, not [`Instant`], so a paused-time test
/// sees the timeout branch's elapsed time as the requested timeout instead
/// of the real time the wait actually took.
async fn await_ramp(ramp: &Notify, timeout: Duration) -> (&'static str, Duration) {
    let started = tokio::time::Instant::now();
    tokio::select! {
        () = ramp.notified() => ("content_ready", started.elapsed()),
        () = tokio::time::sleep(timeout) => ("timeout", started.elapsed()),
    }
}

/// Ends [`spawn`]'s launch-time throttle. The main window's
/// `main_window::content_ready` calls this on its first report; a launch
/// that never opens the main window ramps on [`WORKER_RAMP_SECS`] instead.
pub fn notify_ramp(app: &tauri::AppHandle) {
    app.state::<WorkerHandle>().ramp.notify_one();
}

async fn run_worker(app: tauri::AppHandle) {
    let store_handle: Store = (*app.state::<Store>()).clone();
    let run_pass = move |record: &SessionRecord, signal: PassSignal, claim_fence: i64| {
        run_record_pass(record, signal, claim_fence, store_handle.clone())
    };
    let announce_app = app.clone();
    // The worker reports a typed fact only. The projection worker
    // rebuilds the rich row and emits the frontend events.
    let announce = move |key: &SessionKey| {
        crate::session_lifecycle::report(
            &announce_app,
            crate::session_lifecycle::SyncObservation::RowChanged {
                session: key.clone(),
                facets: crate::session_lifecycle::UpdateFacets {
                    analysis: true,
                    ..Default::default()
                },
                at: unix_now(),
            },
        );
    };
    let report_app = app.clone();
    let announce_idle = move || {
        let _ = report_app.emit(commands::CHECKS_REPORT_CHANGED_EVENT, ());
    };
    let backlog_app = app.clone();
    let announce_backlog = move |active: bool| {
        let _ = backlog_app.emit(
            commands::INSIGHTS_BACKLOG_CHANGED_EVENT,
            crate::dto::InsightsBacklog { active },
        );
    };
    let analytics_app = app.clone();
    let report_ingested = move |agent: AgentKind, ingested: IngestedIncidents| {
        crate::analytics::record_provider_incidents_ingested(&analytics_app, agent, &ingested);
    };
    let clock = || unix_now();
    let store = app.state::<Store>();
    let handle = app.state::<WorkerHandle>();
    let signals = WorkerLoopSignals {
        idle: &announce_idle,
        backlog: &announce_backlog,
    };
    worker_loop(
        &store,
        &handle,
        &clock,
        &run_pass,
        &announce,
        &signals,
        &report_ingested,
    )
    .await;
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

/// What applying one evidence pass's outcome did to the store.
pub(crate) struct AppliedOutcome {
    /// Whether the claim-fenced store write actually applied. This is
    /// `false` only when this claim lost the race against a newer one.
    /// [`apply_outcome`] returned this same meaning as a bare `bool` before
    /// this type existed.
    pub applied: bool,
    /// The agent and its newly reportable incidents. This is set only when
    /// this outcome published a session, and its transcript gained a fresh
    /// incident its previously published evidence did not carry. It is
    /// `None` on every other outcome, including a publish with nothing new
    /// to report.
    pub ingested: Option<(AgentKind, IngestedIncidents)>,
}

pub(crate) fn apply_outcome(
    store: &Store,
    claim: &EvidenceClaim,
    pass: &EvidencePass,
    now: i64,
) -> anyhow::Result<AppliedOutcome> {
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
            // Read this session's previously published evidence before
            // `publish_projections` overwrites it. This lets
            // `ingested_incidents::newly_reportable` find which incidents
            // are new. Both this read and the write below lock the store's
            // single `Mutex<Connection>` (see `Store::lock`). Nothing here
            // awaits between them, so no other Store operation can run in
            // the gap. This claim's own row also stays `status =
            // 'processing'` under this claim's fence until
            // `publish_projections` updates it. So no other claim can
            // complete a competing publish for the same session in that
            // gap either.
            let previous = store
                .evidence(&claim.key)?
                .and_then(|row| row.evidence_json)
                .and_then(|json| serde_json::from_str::<SessionEvidence>(&json).ok());
            let applied = store.publish_projections(
                &record,
                pass.analysis.started_at_epoch,
                &completion,
                &relations,
                &pass.source_outcomes,
            )?;
            let ingested = applied
                .then(|| {
                    let reportable = ingested_incidents::newly_reportable(
                        previous.as_ref(),
                        evidence,
                        now_ms(now),
                    );
                    if reportable.is_empty() {
                        return None;
                    }
                    crate::agents::kind_from_slug(&claim.key.agent).map(|agent| (agent, reportable))
                })
                .flatten();
            Ok(AppliedOutcome { applied, ingested })
        }
        PassOutcome::SourceChanged => Ok(AppliedOutcome {
            applied: store.fail_evidence(
                claim,
                EvidenceFailure::Retry {
                    next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
                    counts_as_attempt: true,
                },
                EVIDENCE_ERROR_SOURCE_CHANGED,
            )?,
            ingested: None,
        }),
        PassOutcome::SourceMissing => Ok(AppliedOutcome {
            applied: store.fail_evidence(
                claim,
                EvidenceFailure::Failed {
                    revisions: analysis::projection_revisions(),
                },
                EVIDENCE_ERROR_SOURCE_MISSING,
            )?,
            ingested: None,
        }),
        PassOutcome::Unsupported => Ok(AppliedOutcome {
            applied: store.fail_evidence(
                claim,
                EvidenceFailure::Failed {
                    revisions: analysis::projection_revisions(),
                },
                EVIDENCE_ERROR_UNSUPPORTED,
            )?,
            ingested: None,
        }),
        PassOutcome::Unreadable(reason) => {
            let last_error = format!(
                "{EVIDENCE_ERROR_UNREADABLE}{UNREADABLE_REASON_SEPARATOR}{}",
                reason.as_error_suffix()
            );
            let applied = if reason == UnreadableReason::Cancelled {
                // The source was never actually tried, so this retry must
                // not consume one of the claim's attempts.
                store.fail_evidence(
                    claim,
                    EvidenceFailure::Retry {
                        next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
                        counts_as_attempt: false,
                    },
                    &last_error,
                )?
            } else if claim.retry_count < MAX_EVIDENCE_ATTEMPTS {
                store.fail_evidence(
                    claim,
                    EvidenceFailure::Retry {
                        next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
                        counts_as_attempt: true,
                    },
                    &last_error,
                )?
            } else {
                store.fail_evidence(
                    claim,
                    EvidenceFailure::Failed {
                        revisions: analysis::projection_revisions(),
                    },
                    &last_error,
                )?
            };
            Ok(AppliedOutcome {
                applied,
                ingested: None,
            })
        }
    }
}

/// Convert the worker's whole-second publish clock to milliseconds, the
/// unit every incident's `ts_ms` uses.
fn now_ms(now_epoch_secs: i64) -> i64 {
    now_epoch_secs.saturating_mul(1000)
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
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
    on_claimed: &(dyn Fn() + Send + Sync),
) -> anyhow::Result<bool> {
    let Some(claim) =
        store.claim_next_evidence(&crate::agents::evidence_cohort(), clock(), LEASE_SECS)?
    else {
        return Ok(false);
    };
    // Mark the pool busy now, before this claim's pass runs. The pass
    // itself can take a while, and a published pass's `announce` event
    // reaches the frontend as soon as the pass returns — waiting for that
    // return to also flip the backlog signal would let the event arrive
    // while the backlog still read idle.
    on_claimed();
    let Some(record) = store.session(&claim.key)? else {
        return Ok(true);
    };
    let Some(_agent) = crate::agents::kind_from_slug(&record.key.agent) else {
        let pass = analysis::unsupported_evidence_pass();
        apply_outcome(store, &claim, &pass, clock())?;
        return Ok(true);
    };
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
    let outcome = apply_outcome(store, &claim, &pass, clock())?;
    let published = outcome.applied && pass.outcome == PassOutcome::Published;
    if published {
        fork_lineage::link_claude_fork(store, &claim.key)?;
        announce(&claim.key);
    }
    if let Some((agent, ingested)) = outcome.ingested {
        report_ingested(agent, ingested);
    }
    Ok(true)
}

pub(crate) async fn process_next_work(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
    on_claimed: &(dyn Fn() + Send + Sync),
) -> anyhow::Result<bool> {
    let now = clock();
    if let Some(recovery) = store.next_remediation_write_recovery(now)? {
        on_claimed();
        crate::remediation::recover_uncertain_write(store, &recovery, now)?;
        return Ok(true);
    }
    let remediation_first = store.take_remediation_work_turn();
    if remediation_first && let Some(remediation) = store.next_dirty_remediation()? {
        on_claimed();
        let _ = crate::remediation::evaluate_dirty_remediation(
            store.state_dir(),
            store,
            &remediation,
            clock(),
        )?;
        return Ok(true);
    }
    let processed = process_next(
        store,
        clock,
        run_pass,
        announce,
        report_ingested,
        on_claimed,
    )
    .await?;
    if processed {
        return Ok(true);
    }
    if !remediation_first && let Some(remediation) = store.next_dirty_remediation()? {
        on_claimed();
        let _ = crate::remediation::evaluate_dirty_remediation(
            store.state_dir(),
            store,
            &remediation,
            clock(),
        )?;
        return Ok(true);
    }
    Ok(false)
}

/// `worker_loop`'s two report-only signals to the app layer: `checks:
/// report-changed` on every settle, and the pool-wide backlog start/drain.
/// Bundled into one parameter so adding the backlog signal did not tip the
/// loop over clippy's argument-count limit.
pub(crate) struct WorkerLoopSignals<'a> {
    pub idle: &'a (dyn Fn() + Send + Sync),
    pub backlog: &'a (dyn Fn(bool) + Send + Sync),
}

pub(crate) async fn worker_loop(
    store: &Store,
    handle: &WorkerHandle,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    signals: &WorkerLoopSignals<'_>,
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
) {
    let mut processed = false;
    let busy = AtomicBool::new(false);
    // Fires as soon as `process_next_work` claims a unit of work, before it
    // runs that work. Guarded so the pool-wide busy count only counts this
    // worker's idle-to-busy edge once per stretch, the same guard the old
    // `if !busy` check at the loop level used to apply after the work
    // finished instead of before it started.
    let on_claimed = || {
        if !busy.swap(true, Ordering::SeqCst)
            && let Some(pending) = handle.note_backlog_busy(store)
        {
            ::tracing::info!(event = "insights_backlog_started", pending);
            (signals.backlog)(true);
        }
    };
    loop {
        match process_next_work(
            store,
            clock,
            run_pass,
            announce,
            report_ingested,
            &on_claimed,
        )
        .await
        {
            Ok(true) => {
                handle.note_backlog_processed();
                processed = true;
                (signals.idle)();
                continue;
            }
            Ok(false) => {
                if busy.swap(false, Ordering::SeqCst)
                    && let Some((drained, elapsed_ms)) = handle.note_backlog_idle()
                {
                    ::tracing::info!(
                        event = "insights_backlog_drained",
                        processed = drained,
                        elapsed_ms
                    );
                    (signals.backlog)(false);
                }
                if processed {
                    processed = false;
                    (signals.idle)();
                }
                tokio::select! {
                    () = handle.wake.notified() => {}
                    () = tokio::time::sleep(Duration::from_secs(IDLE_POLL_SECS)) => {}
                }
            }
            Err(error) => {
                if busy.swap(false, Ordering::SeqCst)
                    && let Some((drained, elapsed_ms)) = handle.note_backlog_idle()
                {
                    ::tracing::info!(
                        event = "insights_backlog_drained",
                        processed = drained,
                        elapsed_ms
                    );
                    (signals.backlog)(false);
                }
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
