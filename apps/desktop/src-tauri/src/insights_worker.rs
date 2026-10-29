//! Drains the durable transcript evidence queue outside the scan pass.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
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
use crate::first_run_gate::{FirstRunGate, FirstRunStage};
use crate::fork_lineage;
use crate::scan::ScanController;
use crate::store::{
    EvidenceClaim, EvidenceCompletion, EvidenceFailure, FencedTurnRowStore, PublishedEvidence,
    RelationKind, RelationRecord, SessionKey, SessionRecord, Store, ToolResultBatch,
    ToolResultPublishOptions,
};

pub(crate) mod file_live;
pub(crate) mod journal;
pub(crate) mod opencode_v2;
mod supervision;

pub(crate) const LEASE_SECS: i64 = 300;
pub(crate) const LEASE_RENEW_SECS: u64 = 60;
pub(crate) const IDLE_POLL_SECS: u64 = 60;
/// Parse several independent transcripts at once without saturating the machine.
const WORKER_CONCURRENCY: usize = 4;
/// How long `spawn` waits for the main window's first content-ready
/// notification before ramping to full concurrency on its own. Covers a
/// launch that never opens the main window (tray-only, HUD).
const WORKER_RAMP_SECS: u64 = 30;
/// The shortest time between two checks report refreshes while the pool is
/// busy. Without these refreshes, a long backlog shows no checks progress
/// until it drains.
const REPORT_PROGRESS_INTERVAL: Duration = Duration::from_secs(2);
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
    published_generation: Option<u64>,
    started_at: Option<Instant>,
    /// When the pool last asked for a checks report refresh while busy.
    reported_at: Option<Instant>,
}

/// This handle wakes the worker.
#[derive(Default)]
pub struct WorkerHandle {
    wake: Notify,
    capture_wake: Notify,
    backlog: Mutex<Backlog>,
    /// Ends `spawn`'s launch-time throttle early. See [`notify_ramp`].
    ramp: Notify,
}

impl WorkerHandle {
    #[cfg(test)]
    pub(crate) fn with_active_backlog_for_test(store: &Store) -> Self {
        let handle = Self::default();
        handle.note_backlog_busy(store);
        handle
    }

    /// Marks one worker's idle→busy transition. Returns the pending count
    /// only for the worker that takes the pool from zero to one active
    /// worker, so `worker_loop` logs the backlog's start once per stretch,
    /// not once per worker.
    fn note_backlog_busy(&self, store: &Store) -> Option<usize> {
        let mut backlog = self.backlog.lock().expect("backlog lock");
        backlog.active += 1;
        let first = backlog.active == 1;
        if first {
            backlog.started_at = Some(Instant::now());
            backlog.reported_at = Some(Instant::now());
        }
        drop(backlog);
        first.then(|| {
            store
                .pending_evidence_count(&crate::agents::evidence_cohort())
                .unwrap_or(0)
        })
    }

    /// Counts one evidence row as processed in the current busy stretch.
    /// Returns true when the pool last asked for a checks report refresh at
    /// least [`REPORT_PROGRESS_INTERVAL`] ago, and records this refresh.
    fn note_backlog_processed(&self) -> bool {
        let mut backlog = self.backlog.lock().expect("backlog lock");
        backlog.processed += 1;
        let due = backlog
            .reported_at
            .is_none_or(|reported_at| reported_at.elapsed() >= REPORT_PROGRESS_INTERVAL);
        if due {
            backlog.reported_at = Some(Instant::now());
        }
        due
    }

    fn note_backlog_published(&self, generation: u64) {
        let mut backlog = self.backlog.lock().expect("backlog lock");
        backlog.published_generation = Some(
            backlog
                .published_generation
                .map_or(generation, |current| current.max(generation)),
        );
    }

    /// Marks one worker's busy→idle transition. Returns the drained total
    /// and its elapsed time when this was the last busy worker. A failed
    /// first item must also end the busy stretch.
    fn note_backlog_idle_with_generation(&self) -> Option<(usize, u64, Option<u64>)> {
        let mut backlog = self.backlog.lock().expect("backlog lock");
        if backlog.active == 0 {
            return None;
        }
        backlog.active -= 1;
        if backlog.active == 0 {
            let elapsed_ms = backlog
                .started_at
                .map(|started_at| started_at.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let processed = backlog.processed;
            let published_generation = backlog.published_generation.take();
            backlog.processed = 0;
            backlog.started_at = None;
            backlog.reported_at = None;
            Some((processed, elapsed_ms, published_generation))
        } else {
            None
        }
    }

    #[cfg(test)]
    fn note_backlog_idle(&self) -> Option<(usize, u64, bool)> {
        self.note_backlog_idle_with_generation()
            .map(|(processed, elapsed_ms, generation)| {
                (processed, elapsed_ms, generation.is_some())
            })
    }

    /// Whether the pool has at least one worker busy on the backlog right
    /// now. Backs the `get_insights_backlog` command's initial read.
    pub fn backlog_active(&self) -> bool {
        self.backlog.lock().expect("backlog lock").active > 0
    }
}

struct BacklogActivity<'a> {
    busy: AtomicBool,
    handle: &'a WorkerHandle,
    signals: &'a WorkerLoopSignals<'a>,
    publication_drained: &'a (dyn Fn(u64) + Send + Sync),
}

impl BacklogActivity<'_> {
    fn claimed(&self, store: &Store) {
        if !self.busy.swap(true, Ordering::SeqCst)
            && let Some(pending) = self.handle.note_backlog_busy(store)
        {
            ::tracing::info!(event = "insights_backlog_started", pending);
            (self.signals.backlog)(true);
        }
    }

    fn idle(&self) -> bool {
        if !self.busy.swap(false, Ordering::SeqCst) {
            return false;
        }
        if let Some((processed, elapsed_ms, published_generation)) =
            self.handle.note_backlog_idle_with_generation()
        {
            ::tracing::info!(event = "insights_backlog_drained", processed, elapsed_ms);
            (self.signals.backlog)(false);
            if let Some(generation) = published_generation {
                (self.publication_drained)(generation);
            }
        }
        true
    }
}

impl Drop for BacklogActivity<'_> {
    fn drop(&mut self) {
        if catch_unwind(AssertUnwindSafe(|| {
            if self.idle() {
                (self.signals.report_changed)();
            }
        }))
        .is_err()
        {
            ::tracing::error!(event = "insights_backlog_shutdown_announcement_failed");
        }
    }
}

pub(crate) type PassFuture = Pin<Box<dyn Future<Output = EvidencePass> + Send>>;
/// The `i64` is the claim's fence: [`process_next_with_progress`] already holds the claim
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
    if record.key.remote_host_id().is_some() {
        return crate::remote_cache::run_pass(record.clone(), signal, claim_fence, store);
    }
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
    let signal = if journal::owns_session(record)
        || crate::store::native_tool_results::source_delivery::native_file::owns_session(record)
    {
        signal.with_journal_delivery()
    } else {
        signal
    };
    let writer: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
        store,
        record.key.clone(),
        claim_fence,
    ));
    run_pass_with_writer(signal.clone(), writer, |writer| {
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
    })
}

struct CancelPassOnDrop(PassSignal);

impl CancelPassOnDrop {
    fn cancel(&self) {
        self.0.cancel();
    }
}

impl Drop for CancelPassOnDrop {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(crate) fn run_pass_with_writer(
    signal: PassSignal,
    writer: Arc<dyn TurnRowStore>,
    run: impl FnOnce(Arc<dyn TurnRowStore>) -> PassFuture,
) -> PassFuture {
    let cancellation = CancelPassOnDrop(signal);
    let pass = catch_unwind(AssertUnwindSafe(|| run(writer)));
    Box::pin(async move {
        let result = match pass {
            Ok(pass) => catch_pass_panic(pass).await,
            Err(panic) => Err(panic),
        };
        cancellation.cancel();
        cancellation.0.join_parsers().await;
        match result {
            Ok(pass) => pass,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

async fn catch_pass_panic(
    pass: impl Future<Output = EvidencePass>,
) -> std::thread::Result<EvidencePass> {
    tokio::pin!(pass);
    std::future::poll_fn(|context| {
        match catch_unwind(AssertUnwindSafe(|| pass.as_mut().poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(pass)) => Poll::Ready(Ok(pass)),
            Err(panic) => Poll::Ready(Err(panic)),
        }
    })
    .await
}

/// The first minute after a cold launch runs one worker, so the main
/// window's first paint competes with a single parse thread rather than
/// [`WORKER_CONCURRENCY`]. Full concurrency resumes as soon as the main
/// window reports its content ready, or after [`WORKER_RAMP_SECS`],
/// whichever comes first.
pub fn spawn(app: &tauri::AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let restore_store = app.state::<Store>().inner().clone();
        if let Err(error) = restore_store.recover_opencode_journal_claims() {
            ::tracing::error!(event = "journal_claim_recovery_failed", error = %error);
            return;
        }
        if let Err(error) = tauri::async_runtime::spawn_blocking(move || {
            crate::remote_cache::restore_fork_companions(&restore_store)
        })
        .await
        .unwrap_or_else(|error| Err(error.into()))
        {
            ::tracing::warn!(event = "remote_fork_companion_restore_failed", error = %error);
        }
        let mut workers = JoinSet::new();
        if let Err(error) = app.state::<Store>().recover_native_file_claims() {
            ::tracing::error!(event = "native_file_claim_recovery_failed", error = %error);
            return;
        }
        if let Err(error) = app
            .state::<Store>()
            .recover_opencode_v2_capture_claims(unix_now())
        {
            ::tracing::error!(event = "opencode_v2_capture_recovery_failed", error = %error);
            return;
        }
        let live_app = app.clone();
        workers.spawn(supervision::run("live_delivery", move || {
            run_live_worker(live_app.clone())
        }));
        let capture_app = app.clone();
        workers.spawn(supervision::run("opencode_v2_capture", move || {
            opencode_v2::run(capture_app.clone())
        }));
        let evidence_app = app.clone();
        workers.spawn(supervision::run("evidence", move || {
            run_worker(evidence_app.clone())
        }));
        let handle = app.state::<WorkerHandle>();
        let (reason, elapsed) =
            await_ramp(&handle.ramp, Duration::from_secs(WORKER_RAMP_SECS)).await;
        for _ in 1..WORKER_CONCURRENCY {
            let evidence_app = app.clone();
            workers.spawn(supervision::run("evidence", move || {
                run_worker(evidence_app.clone())
            }));
        }
        ::tracing::info!(
            event = "insights_worker_ramped",
            workers = WORKER_CONCURRENCY,
            reason,
            elapsed_ms = elapsed.as_millis() as u64
        );
        while let Some(result) = workers.join_next().await {
            ::tracing::error!(event = "insights_supervisor_stopped", result = ?result);
        }
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
    let scan_app = app.clone();
    let run_pass = move |record: &SessionRecord, signal: PassSignal, claim_fence: i64| {
        let key = record.key.clone();
        let scan_app = scan_app.clone();
        let pass = run_record_pass(record, signal, claim_fence, store_handle.clone());
        Box::pin(async move {
            let pass = pass.await;
            if let Some(agent) = source_refresh_agent_for_outcome(&pass.outcome, &key.agent) {
                ::tracing::debug!(
                    event = "insights_source_changed_scan_requested",
                    agent = %key.agent,
                );
                scan_app
                    .state::<ScanController>()
                    .request_source_refresh(agent);
            }
            pass
        }) as PassFuture
    };
    let announce_app = app.clone();
    // The worker reports a typed fact only. The projection worker
    // rebuilds the rich row and emits the frontend events.
    let announce = move |key: &SessionKey| {
        crate::installed_checks::wake(&announce_app);
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
    let report_changed = move || {
        ::tracing::debug!(
            event = "checks_report_changed_emitted",
            source = "insights_worker_batch"
        );
        let _ = report_app.emit(commands::CHECKS_REPORT_CHANGED_EVENT, ());
        // Catches up the history progress indicator while the worker
        // settles history evidence after the historical pass itself has
        // already finished. `push_progress` throttles this call.
        crate::scan::history::push_progress(&report_app, false);
    };
    let backlog_app = app.clone();
    let announce_backlog = move |active: bool| {
        let _ = backlog_app.emit(
            commands::INSIGHTS_BACKLOG_CHANGED_EVENT,
            crate::dto::InsightsBacklog { active },
        );
        // Refresh history progress and retry enrollment when the backlog drains.
        if !active {
            crate::scan::history::push_progress(&backlog_app, true);
            crate::scan::history::maybe_start_automatic_pass(&backlog_app);
        }
    };
    let publish_app = app.clone();
    let publication_drained =
        move |generation| crate::usage_alerts::learn_after_first_publish(&publish_app, generation);
    let analytics_app = app.clone();
    let report_ingested = move |agent: AgentKind, ingested: IngestedIncidents| {
        crate::analytics::record_provider_incidents_ingested(&analytics_app, agent, &ingested);
    };
    // The worker does nothing until the reader's own "Run session checks"
    // step starts it. Cloned once here so every call below only clones a
    // cheap handle, not a state lookup, into its own future.
    let gate = (*app.state::<FirstRunGate>()).clone();
    let generation_gate = gate.clone();
    let generation = move || generation_gate.generation();
    let wait_for_check = move || -> GateFuture {
        let gate = gate.clone();
        Box::pin(async move {
            gate.wait_until(FirstRunStage::Checks, || false).await;
        })
    };
    let clock = || unix_now();
    let store = app.state::<Store>();
    let handle = app.state::<WorkerHandle>();
    let signals = WorkerLoopSignals {
        report_changed: &report_changed,
        backlog: &announce_backlog,
        gate: &wait_for_check,
    };
    worker_loop_with_reports(
        &store,
        &handle,
        &clock,
        &run_pass,
        &announce,
        &signals,
        &WorkerLoopReports {
            ingested: &report_ingested,
            publication_drained: &publication_drained,
            generation: &generation,
        },
    )
    .await;
}

fn source_refresh_agent_for_outcome(outcome: &PassOutcome, agent_slug: &str) -> Option<AgentKind> {
    if !matches!(outcome, PassOutcome::SourceChanged) {
        return None;
    }
    crate::agents::kind_from_slug(agent_slug)
}

async fn run_live_worker(app: tauri::AppHandle) {
    let store = app.state::<Store>();
    let gate = app.state::<FirstRunGate>();
    let mut next_lane = 0;
    let announce = |key: &SessionKey| announce_live_result(&app, key);
    loop {
        gate.wait_until(FirstRunStage::Checks, || false).await;
        if !crate::installed_checks::live_delivery_ready(&app) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let result = process_next_live(
            &mut next_lane,
            &|| journal::process_next(&store, &unix_now, &announce, &|| {}),
            &|| file_live::process_next(&store, &unix_now, &announce),
        )
        .await;
        match result {
            Ok(true) => tokio::task::yield_now().await,
            Ok(false) => tokio::time::sleep(Duration::from_secs(1)).await,
            Err(error) => {
                ::tracing::error!(event = "native_delivery_failed", error = %error);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

pub(super) fn announce_live_result(app: &tauri::AppHandle, key: &SessionKey) {
    crate::installed_checks::wake(app);
    crate::session_lifecycle::report(
        app,
        crate::session_lifecycle::SyncObservation::RowChanged {
            session: key.clone(),
            facets: crate::session_lifecycle::UpdateFacets {
                analysis: true,
                ..Default::default()
            },
            at: unix_now(),
        },
    );
    let _ = app.emit(commands::CHECKS_REPORT_CHANGED_EVENT, ());
}

async fn process_next_live<J, F, JF, FF>(
    next_lane: &mut usize,
    journal: &J,
    file: &F,
) -> anyhow::Result<bool>
where
    J: Fn() -> JF,
    F: Fn() -> FF,
    JF: Future<Output = anyhow::Result<bool>>,
    FF: Future<Output = anyhow::Result<bool>>,
{
    let first = *next_lane;
    *next_lane = (first + 1) % 2;
    for offset in 0..2 {
        let worked = if (first + offset).is_multiple_of(2) {
            journal().await?
        } else {
            file().await?
        };
        if worked {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn wake(app: &tauri::AppHandle) {
    let handle = app.state::<WorkerHandle>();
    handle.wake.notify_one();
    handle.capture_wake.notify_one();
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
    apply_outcome_with_attribution(store, claim, pass, now, None)
}

fn apply_outcome_with_attribution(
    store: &Store,
    claim: &EvidenceClaim,
    pass: &EvidencePass,
    now: i64,
    config_attribution: Option<&crate::remediation::PublicationConfigAttribution>,
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
            // Compare accepted evidence to find new incidents. The fenced
            // write below decides whether this pass can report them.
            let previous = store
                .evidence(&claim.key)?
                .and_then(|row| row.evidence_json)
                .and_then(|json| serde_json::from_str::<SessionEvidence>(&json).ok());
            let verified_source_fingerprint = pass.source_fingerprint.as_deref();
            let source_format = pass.native_tool_result_source_format.ok_or_else(|| {
                anyhow::anyhow!("published evidence pass has no admitted source format")
            })?;
            let tool_results = ToolResultBatch {
                source_format,
                results: pass.tool_result_observations.clone(),
                complete: pass.tool_result_observations_complete,
            };
            let native_delivery = store.session(&claim.key)?.as_ref().is_some_and(|record| {
                journal::owns_session(record)
                    || crate::store::native_tool_results::source_delivery::native_file::owns_session(record)
            });
            let fallback;
            let config_attribution = match config_attribution {
                Some(attribution) => attribution,
                None => {
                    fallback = crate::remediation::publication_config_attribution(
                        store,
                        &record.key,
                        completion.status,
                        &completion.evidence_json,
                    )?;
                    &fallback
                }
            };
            let applied = store.publish_projections_with_config_attribution(
                &record,
                pass.analysis.started_at_epoch,
                &completion,
                &relations,
                &pass.source_outcomes,
                ToolResultPublishOptions {
                    source_fingerprint: verified_source_fingerprint,
                    batch: (!native_delivery).then_some(&tool_results),
                },
                config_attribution,
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
                    next_attempt_at_epoch: now
                        + backoff_secs(claim.retry_count).max(crate::scan::TICK.as_secs() as i64),
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
            let applied = store.fail_evidence(
                claim,
                evidence_failure(claim, now, reason != UnreadableReason::Cancelled),
                &last_error,
            )?;
            Ok(AppliedOutcome {
                applied,
                ingested: None,
            })
        }
    }
}

fn evidence_failure(claim: &EvidenceClaim, now: i64, counts_as_attempt: bool) -> EvidenceFailure {
    if !counts_as_attempt || claim.retry_count < MAX_EVIDENCE_ATTEMPTS {
        EvidenceFailure::Retry {
            next_attempt_at_epoch: now + backoff_secs(claim.retry_count),
            counts_as_attempt,
        }
    } else {
        EvidenceFailure::Failed {
            revisions: analysis::projection_revisions(),
        }
    }
}

async fn apply_outcome_blocking(
    store: &Store,
    claim: &EvidenceClaim,
    pass: EvidencePass,
    now: i64,
    publish: impl FnOnce(&Store, &EvidenceClaim, &EvidencePass, i64) -> anyhow::Result<AppliedOutcome>
    + Send
    + 'static,
) -> anyhow::Result<AppliedOutcome> {
    let store = store.clone();
    let claim = claim.clone();
    tokio::task::spawn_blocking(move || {
        let outcome = publish(&store, &claim, &pass, now)?;
        if pass.outcome != PassOutcome::Published {
            let row = store.evidence(&claim.key)?;
            ::tracing::info!(
                event = "insights_evidence_not_published",
                agent = %claim.key.agent,
                session_id = %claim.key.session_id,
                outcome = ?pass.outcome,
                applied = outcome.applied,
                retry_count = claim.retry_count,
                status = ?row.as_ref().map(|row| row.status),
                next_attempt_at_epoch = ?row.and_then(|row| row.next_attempt_at_epoch),
            );
        } else if outcome.applied {
            fork_lineage::link_claude_fork(&store, &claim.key)?;
        }
        Ok(outcome)
    })
    .await
    .map_err(|error| anyhow::anyhow!("evidence publication task failed: {error}"))?
}

async fn settle_failed_claim(
    store: &Store,
    claim: &EvidenceClaim,
    now: i64,
    last_error: &'static str,
    counts_as_attempt: bool,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    on_published: &(dyn Fn() + Send + Sync),
) -> anyhow::Result<bool> {
    let task_store = store.clone();
    let task_claim = claim.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
        let committed = task_store
            .evidence(&task_claim.key)?
            .is_some_and(|row| row.published_fence == Some(task_claim.claim_fence));
        // A task error can follow a commit. Keep its accepted projections.
        if !committed {
            let failure = evidence_failure(&task_claim, now, counts_as_attempt);
            task_store.fail_evidence(&task_claim, failure, last_error)?;
        }
        Ok(committed)
    })
    .await
    .map_err(|error| anyhow::anyhow!("evidence recovery task failed: {error}"))?;
    if matches!(result, Ok(true)) {
        on_published();
    }
    announce(&claim.key);
    result?;
    Ok(true)
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

#[cfg(test)]
pub(crate) async fn process_next(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
    on_claimed: &(dyn Fn() + Send + Sync),
) -> anyhow::Result<bool> {
    process_next_with_progress(
        store,
        clock,
        run_pass,
        announce,
        report_ingested,
        &WorkProgress {
            claimed: on_claimed,
            published: &|| {},
            authorities: Arc::new(tokio::sync::Mutex::new(None)),
        },
    )
    .await
}

struct WorkProgress<'a> {
    claimed: &'a (dyn Fn() + Send + Sync),
    published: &'a (dyn Fn() + Send + Sync),
    authorities: Arc<tokio::sync::Mutex<Option<crate::remediation::PublicationAuthorityCache>>>,
}

async fn process_next_with_progress(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
    progress: &WorkProgress<'_>,
) -> anyhow::Result<bool> {
    // Newest-active session first, so a current session never waits behind
    // a history one in the same backlog.
    let Some(claim) = store.claim_next_evidence_by_recency(
        &crate::agents::evidence_cohort(),
        clock(),
        LEASE_SECS,
    )?
    else {
        return Ok(false);
    };
    // Mark the pool busy now, before this claim's pass runs. The pass
    // itself can take a while, and a published pass's `announce` event
    // reaches the frontend as soon as the pass returns — waiting for that
    // return to also flip the backlog signal would let the event arrive
    // while the backlog still read idle.
    (progress.claimed)();
    let Some(record) = store.session(&claim.key)? else {
        return Ok(true);
    };
    let Some(_agent) = crate::agents::kind_from_slug(&record.key.agent) else {
        let pass = analysis::unsupported_evidence_pass();
        apply_outcome_blocking(store, &claim, pass, clock(), apply_outcome).await?;
        return Ok(true);
    };
    let signal = PassSignal::new();
    // Construct the runner inside the panic boundary as well as polling it.
    let pass =
        catch_pass_panic(async { run_pass(&record, signal.clone(), claim.claim_fence).await });
    tokio::pin!(pass);
    let mut lease_progress = signal.progress();
    let result = loop {
        tokio::select! {
            result = &mut pass => break result,
            () = tokio::time::sleep(lease_renew_interval()) => {
                let observed = signal.progress();
                if observed == lease_progress {
                    continue;
                }
                lease_progress = observed;
                match store.renew_evidence_lease(&claim, clock(), LEASE_SECS) {
                    Ok(true) => {}
                    renewal => {
                        signal.cancel();
                        let _ = pass.await;
                        if let Err(error) = renewal {
                            ::tracing::error!(event = "insights_evidence_renewal_failed", error = %error);
                        }
                        return settle_failed_claim(
                            store, &claim, clock(), "worker-claim-lost", false, announce, progress.published,
                        ).await;
                    }
                }
            }
        }
    };
    let Ok(mut pass) = result else {
        signal.cancel();
        ::tracing::error!(event = "insights_evidence_pass_panicked", agent = %claim.key.agent);
        return settle_failed_claim(
            store,
            &claim,
            clock(),
            "worker-pass-panicked",
            true,
            announce,
            progress.published,
        )
        .await;
    };
    pass.analysis.analyzed_generation = claim.source_generation;
    let published = pass.outcome == PassOutcome::Published;
    let outcome = async {
        let config_attribution = if published {
            let evidence = pass
                .evidence
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("published pass has no evidence projection"))?;
            let evidence_json = serde_json::to_string(evidence)?;
            let mut authorities = progress.authorities.lock().await;
            let authorities =
                authorities.get_or_insert_with(crate::remediation::PublicationAuthorityCache::new);
            Some(
                crate::remediation::publication_config_attribution_async(
                    authorities,
                    store,
                    &claim.key,
                    published_status(evidence),
                    &evidence_json,
                )
                .await?,
            )
        } else {
            None
        };
        apply_outcome_blocking(
            store,
            &claim,
            pass,
            clock(),
            move |store, claim, pass, now| {
                apply_outcome_with_attribution(store, claim, pass, now, config_attribution.as_ref())
            },
        )
        .await
    }
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            ::tracing::error!(event = "insights_evidence_publication_failed", error = %error);
            return settle_failed_claim(
                store,
                &claim,
                clock(),
                "worker-publication-failed",
                true,
                announce,
                progress.published,
            )
            .await;
        }
    };
    if published && outcome.applied {
        (progress.published)();
        announce(&claim.key);
    }
    if let Some((agent, ingested)) = outcome.ingested {
        report_ingested(agent, ingested);
    }
    Ok(true)
}

#[cfg(test)]
pub(crate) async fn process_next_work(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
    on_claimed: &(dyn Fn() + Send + Sync),
) -> anyhow::Result<bool> {
    process_next_work_with_journal(
        store,
        clock,
        run_pass,
        announce,
        report_ingested,
        &WorkProgress {
            claimed: on_claimed,
            published: &|| {},
            authorities: Arc::new(tokio::sync::Mutex::new(None)),
        },
        &|| journal::process_next(store, clock, announce, on_claimed),
    )
    .await
}

async fn process_next_work_with_journal<F, Fut>(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
    progress: &WorkProgress<'_>,
    run_journal: &F,
) -> anyhow::Result<bool>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = anyhow::Result<bool>> + Send,
{
    let now = clock();
    if let Some(recovery) = store.next_remediation_write_recovery(now)? {
        (progress.claimed)();
        let store = store.clone();
        let authorities = progress.authorities.clone();
        tokio::task::spawn_blocking(move || {
            let mut authorities = authorities.blocking_lock();
            let authorities =
                authorities.get_or_insert_with(crate::remediation::PublicationAuthorityCache::new);
            crate::remediation::recover_uncertain_write_with_cache(
                &store,
                &recovery,
                now,
                authorities,
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("remediation recovery task failed: {error}"))??;
        return Ok(true);
    }
    let first = store.take_insights_work_turn();
    for offset in 0..3 {
        match (first + offset) % 3 {
            0 => {
                if let Some(remediation) = store.next_dirty_remediation()? {
                    (progress.claimed)();
                    let store = store.clone();
                    let now = clock();
                    tokio::task::spawn_blocking(move || {
                        crate::remediation::evaluate_dirty_remediation(
                            store.state_dir(),
                            &store,
                            &remediation,
                            now,
                        )
                    })
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("remediation evaluation task failed: {error}")
                    })??;
                    return Ok(true);
                }
            }
            1 => {
                if run_journal().await? {
                    return Ok(true);
                }
            }
            _ => {
                if process_next_with_progress(
                    store,
                    clock,
                    run_pass,
                    announce,
                    report_ingested,
                    progress,
                )
                .await?
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// A future [`worker_loop_with_reports`] awaits before every claim. See
/// [`WorkerLoopSignals::gate`].
pub(crate) type GateFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A test-only gate that is always open, for a test that is not exercising
/// the first-run gate itself.
#[cfg(test)]
pub(crate) fn always_open_gate() -> GateFuture {
    Box::pin(async {})
}

/// Report refresh and pool-wide backlog signals.
pub(crate) struct WorkerLoopSignals<'a> {
    pub report_changed: &'a (dyn Fn() + Send + Sync),
    pub backlog: &'a (dyn Fn(bool) + Send + Sync),
    /// Resolves once the first-run gate has reached `Check`. Awaited inside
    /// the loop, before every claim this worker makes: the debug reset can
    /// set the stage back to `Welcome` while this loop is running, and the
    /// worker must not claim anything in the gap. A test that wants no
    /// gating passes a closure that returns an already-resolved future.
    pub gate: &'a (dyn Fn() -> GateFuture + Send + Sync),
}

#[cfg(test)]
pub(crate) async fn worker_loop(
    store: &Store,
    handle: &WorkerHandle,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    signals: &WorkerLoopSignals<'_>,
    report_ingested: &(dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
) {
    worker_loop_with_reports(
        store,
        handle,
        clock,
        run_pass,
        announce,
        signals,
        &WorkerLoopReports {
            ingested: report_ingested,
            publication_drained: &|_| {},
            generation: &|| 0,
        },
    )
    .await;
}

struct WorkerLoopReports<'a> {
    ingested: &'a (dyn Fn(AgentKind, IngestedIncidents) + Send + Sync),
    publication_drained: &'a (dyn Fn(u64) + Send + Sync),
    generation: &'a (dyn Fn() -> u64 + Send + Sync),
}

async fn worker_loop_with_reports(
    store: &Store,
    handle: &WorkerHandle,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    run_pass: &PassRunner<'_>,
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    signals: &WorkerLoopSignals<'_>,
    reports: &WorkerLoopReports<'_>,
) {
    let activity = BacklogActivity {
        busy: AtomicBool::new(false),
        handle,
        signals,
        publication_drained: reports.publication_drained,
    };
    let mut report_dirty = false;
    let authorities = Arc::new(tokio::sync::Mutex::new(None));
    let mut authority_generation = (reports.generation)();
    let on_claimed = || activity.claimed(store);
    loop {
        // Checked every iteration, not once before the loop starts: the
        // debug reset can lower the stage while this loop is already
        // running, and the next claim must wait for it to reopen.
        let generation = (reports.generation)();
        if generation != authority_generation {
            *authorities.lock().await = None;
            authority_generation = generation;
        }
        (signals.gate)().await;
        if generation != (reports.generation)() {
            continue;
        }
        let on_published = || handle.note_backlog_published(generation);
        let progress = WorkProgress {
            claimed: &on_claimed,
            published: &on_published,
            authorities: authorities.clone(),
        };
        match process_next_work_with_journal(
            store,
            clock,
            run_pass,
            announce,
            reports.ingested,
            &progress,
            &|| async { Ok(false) },
        )
        .await
        {
            Ok(true) => {
                if handle.note_backlog_processed() {
                    report_dirty = false;
                    (signals.report_changed)();
                } else {
                    report_dirty = true;
                }
                continue;
            }
            Ok(false) => {
                *authorities.lock().await = None;
                if activity.idle() || report_dirty {
                    report_dirty = false;
                    (signals.report_changed)();
                }
                let wait = match store.next_opencode_journal_attempt(clock()) {
                    Ok(Some(at)) => Duration::from_secs(at.saturating_sub(clock()).max(1) as u64)
                        .min(Duration::from_secs(IDLE_POLL_SECS)),
                    Ok(None) => Duration::from_secs(IDLE_POLL_SECS),
                    Err(error) => {
                        ::tracing::warn!(event = "journal_schedule_read_failed", error = %error);
                        Duration::from_secs(IDLE_POLL_SECS)
                    }
                };
                tokio::select! {
                    () = handle.wake.notified() => {}
                    () = tokio::time::sleep(wait) => {}
                }
            }
            Err(error) => {
                *authorities.lock().await = None;
                if activity.idle() || report_dirty {
                    report_dirty = false;
                    (signals.report_changed)();
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
