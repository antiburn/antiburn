//! Reads one verified OpenCode V2 event feed and publishes ordered results.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use antiburn_local::analysis::{
    SourceFormat, ToolResultObservation,
    opencode_v2_capture::{
        BoundaryKind, CaptureReconciliation, read_capture_reconciliation_for_scope,
    },
    opencode_v2_capture::{CaptureGap, CapturedEvent, OpenCodeV2CaptureEpoch},
    opencode_v2_events::{Gap, OpenCodeV2EventJoiner},
};
use anyhow::{Context, bail};
use serde_json::Value;
use tauri::async_runtime::JoinHandle;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;

use super::{LEASE_SECS, SessionKey, Store, unix_now};
use crate::store::native_tool_results::opencode_v2_status::OpenCodeV2SourceStatus;
use crate::{
    commands,
    first_run_gate::{FirstRunGate, FirstRunStage},
    opencode_live::{self, stream::EventStream},
    store::native_tool_results::source_delivery::opencode_v2::{
        OpenCodeV2AdmittedSource, OpenCodeV2CaptureClaim, OpenCodeV2CaptureStart,
        OpenCodeV2ReconciliationPreparation,
    },
};

const CAPTURE_SESSION_LIMIT: usize = 256;
const CANDIDATE_SESSION_LIMIT: usize = 1024;
const CAPTURE_DATABASE_LIMIT: usize = 8;
const CAPTURE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct CaptureTiming {
    settings_check: Duration,
    recheck: Duration,
    retry_base: Duration,
    retry_max: Duration,
    connect_timeout: Duration,
    empty_scope_wait: Duration,
    stream: opencode_live::stream::StreamTiming,
}

impl Default for CaptureTiming {
    fn default() -> Self {
        Self {
            settings_check: Duration::from_secs(1),
            recheck: Duration::from_secs(30),
            retry_base: Duration::from_secs(2),
            retry_max: Duration::from_secs(60),
            connect_timeout: CAPTURE_CONNECT_TIMEOUT,
            empty_scope_wait: Duration::from_secs(super::IDLE_POLL_SECS),
            stream: opencode_live::stream::StreamTiming::default(),
        }
    }
}

#[async_trait::async_trait]
trait CaptureHost: Send + Sync {
    fn store(&self) -> &Store;
    fn ready(&self) -> bool;
    fn stopping(&self) -> bool;
    fn registration_path(&self) -> anyhow::Result<PathBuf>;
    fn announce(&self, key: &SessionKey);
    fn report_changed(&self);
    async fn wait_for_checks(&self);
    async fn wait_for_sources(&self) {
        std::future::pending::<()>().await;
    }
    async fn locate_sources(
        &self,
        records: &[crate::store::SessionRecord],
        cancelled: &AtomicBool,
    ) -> anyhow::Result<Vec<(crate::store::SessionRecord, PathBuf)>>;
    fn read_baselines(
        &self,
        groups: &[DatabaseGroup],
        targets: &[ToolResultObservation],
        cancel: &dyn Fn() -> bool,
    ) -> anyhow::Result<CaptureReconciliation> {
        read_baselines(groups, targets, cancel)
    }
}

struct AppCaptureHost {
    app: tauri::AppHandle,
    store: Store,
}

#[async_trait::async_trait]
impl CaptureHost for AppCaptureHost {
    fn store(&self) -> &Store {
        &self.store
    }
    fn ready(&self) -> bool {
        crate::installed_checks::live_delivery_ready(&self.app)
    }
    fn stopping(&self) -> bool {
        crate::installed_checks::live_delivery_stopping(&self.app)
    }
    fn registration_path(&self) -> anyhow::Result<PathBuf> {
        opencode_live::default_registration_path().context("locating OpenCode registration")
    }
    fn announce(&self, key: &SessionKey) {
        super::announce_live_result(&self.app, key);
    }
    fn report_changed(&self) {
        let _ = self.app.emit(commands::CHECKS_REPORT_CHANGED_EVENT, ());
    }
    async fn wait_for_checks(&self) {
        self.app
            .state::<FirstRunGate>()
            .wait_until(FirstRunStage::Checks, || self.stopping())
            .await;
    }
    async fn wait_for_sources(&self) {
        self.app
            .state::<super::WorkerHandle>()
            .capture_wake
            .notified()
            .await;
    }
    async fn locate_sources(
        &self,
        records: &[crate::store::SessionRecord],
        cancelled: &AtomicBool,
    ) -> anyhow::Result<Vec<(crate::store::SessionRecord, PathBuf)>> {
        locate_v2_sources(records, cancelled).await
    }
}

pub(super) async fn run(app: tauri::AppHandle) {
    let store = app.state::<Store>().inner().clone();
    run_capture_driver(
        Arc::new(AppCaptureHost { app, store }),
        CaptureTiming::default(),
    )
    .await;
}

async fn run_capture_driver(host: Arc<dyn CaptureHost>, timing: CaptureTiming) {
    let mut retry_count = 0_u32;
    let trace = Arc::new(CaptureFailureTrace::default());
    let mut status_report_failed = false;
    while !host.stopping() {
        host.wait_for_checks().await;
        if !host.ready() {
            tokio::time::sleep(timing.settings_check).await;
            continue;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel_on_drop = CancelCapture(cancelled.clone());
        let epoch = run_capture_epoch(&host, timing, cancelled.clone(), trace.clone());
        tokio::pin!(epoch);
        let outcome = tokio::select! {
            outcome = &mut epoch => outcome,
            () = wait_until_paused(&*host, timing.settings_check) => {
                cancelled.store(true, Ordering::SeqCst);
                let _ = epoch.await;
                retry_count = 0;
                trace.healthy();
                continue;
            }
        };
        match outcome {
            Ok(()) => {
                retry_count = 0;
                trace.healthy();
            }
            Err(error) => {
                let status = capture_failure_status(&error);
                let limit = capture_limit_context(&error);
                let report_host = host.clone();
                let report = tokio::task::spawn_blocking(move || {
                    let records = live_candidates(report_host.store())?;
                    report_source_status_with_limit(&*report_host, &records, status, limit)
                })
                .await;
                let report_failed = !matches!(report, Ok(Ok(())));
                if report_failed && !status_report_failed {
                    ::tracing::warn!(event = "opencode_v2_source_status_failed");
                }
                status_report_failed = report_failed;
                trace.failed(&error, retry_count);
                retry_count = retry_count.saturating_add(1).min(10);
                let delay = timing
                    .retry_base
                    .saturating_mul(1_u32 << retry_count.min(5))
                    .min(timing.retry_max);
                tokio::select! {
                    () = tokio::time::sleep(delay) => {},
                    () = wait_until_paused(&*host, timing.settings_check) => retry_count = 0,
                }
            }
        }
    }
}

#[derive(Default)]
struct CaptureFailureTrace {
    stopped: std::sync::Mutex<Option<CaptureFailureKey>>,
    recovery: std::sync::Mutex<Option<CaptureFailureKey>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CaptureFailureKey {
    status: OpenCodeV2SourceStatus,
    phase: &'static str,
    reason: &'static str,
    cause: crate::store::burn_checks::trace_failure::FailureCause,
}

impl CaptureFailureKey {
    fn new(error: &anyhow::Error) -> Self {
        let (phase, reason) = capture_limit_context(error)
            .map(|limit| limit.kind.codes())
            .unwrap_or_else(|| capture_failure_codes(error));
        Self {
            status: capture_failure_status(error),
            phase,
            reason,
            cause: crate::store::burn_checks::trace_failure::FailureCause::classify(error),
        }
    }
}

impl CaptureFailureTrace {
    fn healthy(&self) {
        *self.stopped.lock().expect("capture trace lock") = None;
        self.recovered();
    }

    fn recovered(&self) {
        *self.recovery.lock().expect("recovery trace lock") = None;
    }

    fn failed(&self, error: &anyhow::Error, retry_count: u32) {
        let limit = capture_limit_context(error);
        let key = CaptureFailureKey::new(error);
        let mut previous = self.stopped.lock().expect("capture trace lock");
        if *previous != Some(key) {
            ::tracing::warn!(
                event = "opencode_v2_capture_stopped",
                status = key.status.code(),
                phase = key.phase,
                reason = key.reason,
                sqlite_code = key.cause.sqlite_code,
                observed = limit.and_then(|limit| limit.observed),
                limit = limit.and_then(|limit| limit.limit),
                retry_count
            );
            *previous = Some(key);
        }
    }

    fn recovery_limited(&self, error: &anyhow::Error) {
        let Some(limit) = capture_limit_context(error) else {
            return;
        };
        let key = CaptureFailureKey::new(error);
        let mut previous = self.recovery.lock().expect("recovery trace lock");
        if *previous != Some(key) {
            ::tracing::warn!(
                event = "opencode_v2_recovery_limited",
                status = key.status.code(),
                phase = key.phase,
                reason = key.reason,
                observed = limit.observed,
                limit = limit.limit,
            );
            *previous = Some(key);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureLimitKind {
    CandidateSessions,
    BoundSessions,
    Databases,
    RecoverySessions,
    RetainedSessions,
    CaptureBuffer,
    RecoveryBuffer,
    FeedBuffer,
    SseFrame,
    Snapshot,
    RecoverySnapshot,
    IdleSnapshot,
    Registration,
}

impl CaptureLimitKind {
    fn codes(self) -> (&'static str, &'static str) {
        match self {
            Self::CandidateSessions => ("source_selection", "candidate_session_limit"),
            Self::BoundSessions => ("source_selection", "bound_session_limit"),
            Self::Databases => ("source_selection", "database_limit"),
            Self::RecoverySessions => ("recovery", "session_limit"),
            Self::RetainedSessions => ("scope_refresh", "session_limit"),
            Self::CaptureBuffer => ("capture_buffer", "capture_buffer_limit"),
            Self::RecoveryBuffer => ("recovery", "capture_buffer_limit"),
            Self::FeedBuffer => ("event_feed", "feed_buffer_limit"),
            Self::SseFrame => ("event_feed", "sse_frame_limit"),
            Self::Snapshot => ("snapshot", "snapshot_budget"),
            Self::RecoverySnapshot => ("recovery", "snapshot_budget"),
            Self::IdleSnapshot => ("idle_frontier", "snapshot_budget"),
            Self::Registration => ("registration", "registration_size_limit"),
        }
    }
}

#[derive(Clone, Copy)]
struct CaptureLimitContext {
    kind: CaptureLimitKind,
    observed: Option<usize>,
    limit: Option<usize>,
}

impl std::fmt::Display for CaptureLimitContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(OpenCodeV2SourceStatus::SourceLimit.code())
    }
}
impl std::fmt::Debug for CaptureLimitContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}
impl std::error::Error for CaptureLimitContext {}

fn source_limit_error(
    kind: CaptureLimitKind,
    observed: Option<usize>,
    limit: Option<usize>,
) -> anyhow::Error {
    anyhow::Error::new(CaptureFailure(OpenCodeV2SourceStatus::SourceLimit)).context(
        CaptureLimitContext {
            kind,
            observed,
            limit,
        },
    )
}

fn snapshot_limit_context(error: anyhow::Error, kind: CaptureLimitKind) -> anyhow::Error {
    if capture_failure_status(&error) == OpenCodeV2SourceStatus::SourceLimit {
        error.context(CaptureLimitContext {
            kind,
            observed: None,
            limit: None,
        })
    } else {
        error
    }
}

fn capture_limit_context(error: &anyhow::Error) -> Option<CaptureLimitContext> {
    use opencode_live::stream::EventStreamError;
    if let Some(context) = error.downcast_ref::<CaptureLimitContext>() {
        return Some(*context);
    }
    for cause in error.chain() {
        let kind = match cause.downcast_ref::<EventStreamError>() {
            Some(EventStreamError::CapacityExceeded) => Some(CaptureLimitKind::FeedBuffer),
            Some(EventStreamError::Sse(opencode_live::SseError::FrameTooLarge)) => {
                Some(CaptureLimitKind::SseFrame)
            }
            _ if cause.downcast_ref::<antiburn_local::analysis::OpenCodeV2SnapshotError>()
                == Some(&antiburn_local::analysis::OpenCodeV2SnapshotError::SourceLimit) =>
            {
                Some(CaptureLimitKind::Snapshot)
            }
            _ if matches!(
                cause.downcast_ref::<opencode_live::RegistrationError>(),
                Some(opencode_live::RegistrationError::TooLarge)
            ) =>
            {
                Some(CaptureLimitKind::Registration)
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return Some(CaptureLimitContext {
                kind,
                observed: None,
                limit: None,
            });
        }
    }
    None
}

fn capture_failure_codes(error: &anyhow::Error) -> (&'static str, &'static str) {
    use antiburn_local::analysis::OpenCodeV2SnapshotError;
    use opencode_live::stream::EventStreamError;
    use opencode_live::{DatabaseAssociationError, ProbeError, RegistrationError, SseError};
    for cause in error.chain() {
        if let Some(boundary) = cause.downcast_ref::<CaptureBoundaryFailure>() {
            return (
                "capture_boundary",
                match boundary.0 {
                    CaptureGap::SubscriptionNotConfirmed => "subscription_not_confirmed",
                    CaptureGap::InvalidBoundary => "boundary_invalid",
                    CaptureGap::InvalidEvent => "event_invalid",
                    CaptureGap::CapacityExceeded => "capture_buffer_limit",
                    CaptureGap::ContinuityLost => "continuity_lost",
                },
            );
        }
        if let Some(stream) = cause.downcast_ref::<EventStreamError>() {
            let reason = match stream {
                EventStreamError::Registration(_) | EventStreamError::Association(_) => continue,
                EventStreamError::Request => "request_failed",
                EventStreamError::Redirect => "redirect_rejected",
                EventStreamError::HttpStatus(401 | 403) => "authentication_rejected",
                EventStreamError::HttpStatus(_) => "http_status_rejected",
                EventStreamError::NotEventStream => "content_type_rejected",
                EventStreamError::RegistrationChanged => "registration_changed",
                EventStreamError::Body => "body_read_failed",
                EventStreamError::StreamEnded => "stream_ended",
                EventStreamError::Sse(SseError::MalformedJson) => "malformed_json",
                EventStreamError::Cancelled | EventStreamError::Sse(SseError::Cancelled) => {
                    "cancelled"
                }
                EventStreamError::IdleTimeout => "heartbeat_timeout",
                EventStreamError::CapacityExceeded
                | EventStreamError::Sse(SseError::FrameTooLarge) => "source_limit",
            };
            return ("event_feed", reason);
        }
        if let Some(registration) = cause.downcast_ref::<RegistrationError>() {
            let reason = match registration {
                RegistrationError::Io => "read_failed",
                RegistrationError::TooLarge => "registration_size_limit",
                RegistrationError::InvalidEndpoint => "endpoint_rejected",
                _ => "registration_invalid",
            };
            return ("registration", reason);
        }
        if let Some(probe) = cause.downcast_ref::<ProbeError>() {
            let reason = match probe {
                ProbeError::Request => "request_failed",
                ProbeError::Redirect => "redirect_rejected",
                ProbeError::HttpStatus(401 | 403) => "authentication_rejected",
                ProbeError::HttpStatus(_) => "http_status_rejected",
                ProbeError::ResponseTooLarge => "response_size_limit",
                ProbeError::MalformedInfo => "response_invalid",
                ProbeError::ProcessMismatch => "process_mismatch",
                ProbeError::VersionMismatch => "version_mismatch",
            };
            return ("service_probe", reason);
        }
        if let Some(association) = cause.downcast_ref::<DatabaseAssociationError>() {
            match association {
                DatabaseAssociationError::Binding => return ("source_binding", "binding_unproved"),
                DatabaseAssociationError::RegistrationChanged => {
                    return ("source_binding", "registration_changed");
                }
                _ => continue,
            }
        }
        if let Some(snapshot) = cause.downcast_ref::<OpenCodeV2SnapshotError>() {
            return (
                "snapshot",
                match snapshot {
                    OpenCodeV2SnapshotError::UnsupportedSchema => "schema_unsupported",
                    OpenCodeV2SnapshotError::UnsupportedSource => "source_unsupported",
                    OpenCodeV2SnapshotError::ScopeChanged => "scope_changed",
                    OpenCodeV2SnapshotError::SourceLimit => "snapshot_budget",
                    OpenCodeV2SnapshotError::Cancelled => "cancelled",
                },
            );
        }
    }
    let cause = crate::store::burn_checks::trace_failure::FailureCause::classify(error);
    (
        "capture",
        if cause.category == "runtime_invariant" {
            capture_failure_status(error).code()
        } else {
            cause.category
        },
    )
}

struct CancelCapture(Arc<AtomicBool>);

impl Drop for CancelCapture {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn wait_until_paused(host: &dyn CaptureHost, poll: Duration) {
    while host.ready() && !host.stopping() {
        tokio::time::sleep(poll).await;
    }
}

async fn run_capture_epoch(
    host: &Arc<dyn CaptureHost>,
    timing: CaptureTiming,
    cancelled: Arc<AtomicBool>,
    trace: Arc<CaptureFailureTrace>,
) -> anyhow::Result<()> {
    let host = host.clone();
    // Store transactions and source identity reads must not block app runtime threads.
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(run_capture_epoch_inner(&host, timing, cancelled, &trace))
    })
    .await
    .context("joining OpenCode V2 capture task")?
}

async fn run_capture_epoch_inner(
    host: &Arc<dyn CaptureHost>,
    timing: CaptureTiming,
    cancelled: Arc<AtomicBool>,
    trace: &CaptureFailureTrace,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    ensure_capture_active(&cancelled)?;
    let store = host.store();
    let records = live_candidates(store)?;
    if records.is_empty() {
        tokio::select! {
            () = tokio::time::sleep(timing.empty_scope_wait) => {},
            () = host.wait_for_sources() => {},
            () = wait_until_paused(&**host, timing.settings_check) => {},
            () = cancel_poll(&cancelled) => {},
        }
        return Ok(());
    }
    let registration_path = host.registration_path()?;
    let (mut groups, mut records) =
        bind_live_sources(host, &records, &registration_path, timing, &cancelled).await?;
    if groups.is_empty() {
        return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
    }
    let database_path = groups[0].0.clone();
    let database_identity = DatabaseIdentity::read_admitted(&database_path, &records)?;
    let mut stream = tokio::select! {
        stream = opencode_live::stream::open_verified_event_stream(
            &registration_path, &database_path, timing.connect_timeout, timing.stream,
        ) => stream.context("opening the verified OpenCode V2 event stream")?,
        () = cancel_poll(&cancelled) => return Ok(()),
    };
    let first = stream.next_event(&cancelled).await?;
    if first.data.get("type").and_then(Value::as_str) != Some("server.connected") {
        bail!("OpenCode V2 event feed did not confirm its subscription");
    }
    if !opencode_live::is_v2_version(&stream.service_info().version) {
        return Err(CaptureFailure(OpenCodeV2SourceStatus::VersionUnsupported).into());
    }
    let registration = stream.registration().clone();
    verify_bound_databases(
        &registration_path,
        &groups,
        &registration,
        timing.connect_timeout,
        &cancelled,
    )
    .await?;
    database_identity.verify()?;

    let session_ids: Vec<_> = records
        .iter()
        .map(|record| record.key.session_id.clone())
        .collect();
    let mut capture =
        OpenCodeV2CaptureEpoch::new(session_ids.clone()).map_err(capture_gap_error)?;
    capture.confirm_subscription().map_err(capture_gap_error)?;
    let mut pump = FeedPump::start(
        stream,
        cancelled.clone(),
        session_ids.iter().cloned().collect(),
    );
    let detached = store.detached_opencode_v2_capture_claims(&records)?;
    retire_detached_scope_claims(host, &database_identity, &mut pump, &mut capture, &detached)
        .await?;
    if !prepare_capture_reconciliation(
        host,
        &records,
        &database_identity,
        &mut pump,
        &mut capture,
        None,
    )
    .await?
    {
        return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
    }
    let targets = store
        .opencode_v2_reconciliation_targets(&records)
        .map_err(|error| snapshot_limit_context(error, CaptureLimitKind::Snapshot))?;
    let mut baseline_task = BaselineTask::start(
        host.clone(),
        groups.clone(),
        targets,
        cancelled.clone(),
        CaptureLimitKind::Snapshot,
    );
    let mut snapshot = loop {
        tokio::select! {
            result = &mut baseline_task.task => {
                break result.context("reading OpenCode V2 capture baseline")??;
            }
            event = pump.next() => {
                let event = event.context("OpenCode V2 feed ended during baseline hydration")??;
                capture.observe(event).map_err(capture_gap_error)?;
            }
        }
    };
    if cancelled.load(Ordering::SeqCst) {
        return Ok(());
    }
    let invalid_scopes = snapshot.invalid_scopes.clone();
    let mut hydration_baseline = snapshot.baseline.clone();
    if !invalid_scopes.is_empty() {
        let invalid_records = records
            .iter()
            .filter(|record| invalid_scopes.contains(&record.key.session_id))
            .cloned()
            .collect::<Vec<_>>();
        for session_id in &invalid_scopes {
            capture.retire_session(session_id);
            hydration_baseline.remove(session_id);
            pump.exclude_session(session_id)?;
        }
        let invalid_keys = invalid_records
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>();
        let invalid_claims = store.opencode_v2_capture_claims(&invalid_keys)?;
        retire_detached_scope_claims(
            host,
            &database_identity,
            &mut pump,
            &mut capture,
            &invalid_claims,
        )
        .await?;
        report_source_status(
            &**host,
            &invalid_records,
            OpenCodeV2SourceStatus::SourceMismatch,
        )?;
        records.retain(|record| !invalid_scopes.contains(&record.key.session_id));
        for (_, scoped_records) in &mut groups {
            scoped_records.retain(|record| !invalid_scopes.contains(&record.key.session_id));
        }
        groups.retain(|(_, scoped_records)| !scoped_records.is_empty());
        if records.is_empty() {
            pump.stop().await;
            return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
        }
        snapshot
            .bindings
            .retain(|binding| !invalid_scopes.contains(&binding.observation.session_id));
        snapshot
            .baseline
            .retain(|session_id, _| !invalid_scopes.contains(session_id));
        snapshot.invalid_scopes.clear();
    }
    pump.verify_registration(&registration_path)?;
    database_identity.verify()?;
    let baseline = &snapshot.baseline;
    let buffered = capture
        .finish_hydration(std::mem::take(&mut hydration_baseline))
        .map_err(capture_gap_error)?;
    let starts = capture_starts(store, &records, baseline)?;
    let capture_epoch = capture.id().to_string();
    let Some(claims) = store.start_opencode_v2_capture_epoch_reconciled(
        &starts,
        &snapshot,
        &capture_epoch,
        unix_now(),
        LEASE_SECS,
    )?
    else {
        pump.stop().await;
        bail!("OpenCode V2 capture is leased by another worker");
    };
    let lease = CaptureLease::new(store, claims);
    trace.healthy();
    ::tracing::debug!(
        event = "opencode_v2_capture_attached",
        sessions = lease.claims.len(),
        sessions_with_frontier = lease
            .claims
            .iter()
            .filter(|claim| claim.last_sequence.is_some())
            .count(),
        maximum_last_sequence = lease
            .claims
            .iter()
            .filter_map(|claim| claim.last_sequence)
            .max(),
        history_gap_sessions = lease
            .claims
            .iter()
            .filter(|claim| claim.history_gap)
            .count(),
        databases = groups.len(),
        buffered_events = buffered.len(),
        invalid_scopes = invalid_scopes.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
    );
    let claims_by_session = lease
        .claims
        .iter()
        .map(|claim| (claim.key.session_id.clone(), claim.clone()))
        .collect::<HashMap<_, _>>();
    let joiners: HashMap<String, OpenCodeV2EventJoiner> = lease
        .claims
        .iter()
        .map(|claim| {
            (
                claim.key.session_id.clone(),
                live_joiner(claim.baseline_sequence),
            )
        })
        .collect();
    let mut active = ActiveCaptures {
        capture,
        lease,
        claims_by_session,
        joiners,
        gapped_sessions: HashSet::new(),
        gapped_claims: HashMap::new(),
        retirement_reasons: HashMap::new(),
    };
    active.record_gaps()?;

    active.replay_buffered(&**host, &database_identity, &pump, buffered)?;
    drain_scope_retirements(host, &database_identity, &mut pump, &mut active).await?;

    let mut recheck = tokio::time::interval(timing.recheck);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    recheck.tick().await;
    let mut settings_check = tokio::time::interval(timing.settings_check);
    settings_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    settings_check.tick().await;
    loop {
        tokio::select! {
            event = pump.next() => {
                let Some(event) = event else {
                    active.lease.mark_gap("event-feed-ended")?;
                    bail!("OpenCode V2 event feed ended");
                };
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        active.lease.mark_gap("event-feed-error")?;
                        return Err(error).context("reading OpenCode V2 event feed");
                    }
                };
                let mut batch = vec![event];
                let mut feed_error = None;
                for _ in 1..64 {
                    let Some(event) = pump.try_next() else { break; };
                    match event {
                        Ok(event) => batch.push(event),
                        Err(error) => {
                            feed_error = Some(error);
                            break;
                        }
                    }
                }
                for event in batch {
                    pump.verify_registration(&registration_path)?;
                    database_identity.verify()?;
                    if let Some(key) = active.observe_verified(event, store, &database_identity, &pump)? { host.announce(&key); }
                }
                if let Some(error) = feed_error {
                    active.lease.mark_gap("event-feed-error")?;
                    return Err(error).context("reading OpenCode V2 event feed");
                }
                drain_scope_retirements(host, &database_identity, &mut pump, &mut active).await?;
            }
            _ = settings_check.tick() => {
                if cancelled.load(Ordering::SeqCst) || !host.ready() || host.stopping() {
                    active.lease.mark_gap("checks-disabled")?;
                    pump.stop().await;
                    return Ok(());
                }
                pump.verify_registration(&registration_path)?;
                database_identity.verify()?;
            }
            _ = recheck.tick() => {
                active.renew_claims(&database_identity, &pump)?;
                let current = active.recheck_candidates(store, &records)?;
                let (current_groups, current_bound_records) = bind_live_sources(host, &current, &registration_path, timing, &cancelled).await?;
                database_identity.verify_records(&current_bound_records)?;
                let mut changed_sessions = changed_scope_sessions(&records, &current_bound_records);
                changed_sessions.extend(active.gapped_sessions.iter().filter(|session_id| {
                    !current_bound_records.iter().any(|record| record.key.session_id == **session_id)
                }).cloned());
                for session_id in &changed_sessions {
                    let previous = records.iter().find(|record| record.key.session_id == *session_id);
                    let current = store.session(&SessionKey::new("native", "opencode", session_id))?;
                    if previous.zip(current.as_ref()).is_some_and(|(previous, current)| same_source_scope(previous, current)) {
                        active.retire_inactive_session(session_id)?;
                    } else {
                        active.retire_session(session_id, "source-scope-changed")?;
                    }
                    pump.exclude_session(session_id)?;
                }
                records.retain(|record| !changed_sessions.contains(&record.key.session_id));
                verify_bound_databases(&registration_path, &current_groups, &registration, timing.connect_timeout, &cancelled).await?;
                database_identity.verify()?;
                drain_scope_retirements(host, &database_identity, &mut pump, &mut active).await?;
                active.retain_sessions(&current_bound_records);
                if current_bound_records.is_empty() {
                    pump.stop().await;
                    return Ok(());
                }
                if store.needs_opencode_v2_idle_frontier(unix_now() * 1000)? {
                    let mut task = BaselineTask::start(host.clone(), current_groups.clone(), Vec::new(), cancelled.clone(), CaptureLimitKind::IdleSnapshot);
                    let snapshot = (&mut task.task).await.context("reading V2 idle frontier")??;
                    drain_buffered_feed(host, &database_identity, &mut pump, |event, pump| {
                        if let Some(key) = active.observe_verified(event, store, &database_identity, pump)? { host.announce(&key); }
                        Ok(())
                    })?;
                    if snapshot.invalid_scopes.is_empty() && pump.receiver.is_empty() {
                        pump.verify_registration(&host.registration_path()?)?;
                        database_identity.verify()?;
                        if store.validate_opencode_v2_idle_frontier(&active.lease.claims, &snapshot.baseline, unix_now())? {
                            for claim in &active.lease.claims { host.announce(&claim.key); }
                        }
                    }
                }
                for record in &current_bound_records {
                    if !active.claims_by_session.contains_key(&record.key.session_id) {
                        active.gapped_sessions.insert(record.key.session_id.clone());
                    }
                }
                if !active.gapped_sessions.is_empty() {
                    recover_gapped_sessions(host, store, &current_groups, &database_identity, &mut pump, &mut active, trace).await?;
                    drain_scope_retirements(host, &database_identity, &mut pump, &mut active).await?;
                    for record in &current_bound_records {
                        if active.claims_by_session.contains_key(&record.key.session_id)
                            && !records.iter().any(|previous| previous.key == record.key)
                        {
                            records.push(record.clone());
                        }
                    }
                }
                let attached_records = current_bound_records.into_iter().filter(|record| {
                    active.claims_by_session.contains_key(&record.key.session_id)
                }).collect::<Vec<_>>();
                pump.replace_sessions(&attached_records)?;
            }
        }
        if active.lease.claims.is_empty()
            && store
                .opencode_v2_live_candidates(unix_now(), CANDIDATE_SESSION_LIMIT)?
                .is_empty()
        {
            pump.stop().await;
            return Ok(());
        }
    }
}

fn live_candidates(store: &Store) -> anyhow::Result<Vec<crate::store::SessionRecord>> {
    let now = unix_now();
    let mut records = store.opencode_v2_live_candidates(now, CANDIDATE_SESSION_LIMIT)?;
    records.retain(|record| crate::analysis::is_active(record.updated_at_epoch, now));
    Ok(records)
}

async fn bind_live_sources(
    host: &Arc<dyn CaptureHost>,
    records: &[crate::store::SessionRecord],
    registration_path: &std::path::Path,
    timing: CaptureTiming,
    cancelled: &AtomicBool,
) -> anyhow::Result<(Vec<DatabaseGroup>, Vec<crate::store::SessionRecord>)> {
    if records.len() >= CANDIDATE_SESSION_LIMIT {
        return Err(source_limit_error(
            CaptureLimitKind::CandidateSessions,
            Some(records.len()),
            Some(CANDIDATE_SESSION_LIMIT),
        ));
    }
    let sources = host.locate_sources(records, cancelled).await?;
    ensure_capture_active(cancelled)?;
    let groups = select_bound_databases(
        registration_path,
        sources,
        timing.connect_timeout,
        cancelled,
    )
    .await?;
    ensure_capture_active(cancelled)?;
    let bound = groups
        .iter()
        .flat_map(|(_, records)| records.iter().cloned())
        .collect::<Vec<_>>();
    report_unbound_sources(&**host, records, &bound)?;
    if bound.len() > CAPTURE_SESSION_LIMIT {
        return Err(source_limit_error(
            CaptureLimitKind::BoundSessions,
            Some(bound.len()),
            Some(CAPTURE_SESSION_LIMIT),
        ));
    }
    Ok((groups, bound))
}

fn report_source_status(
    host: &dyn CaptureHost,
    records: &[crate::store::SessionRecord],
    status: OpenCodeV2SourceStatus,
) -> anyhow::Result<()> {
    report_source_status_with_limit(host, records, status, None)
}

fn report_source_status_with_limit(
    host: &dyn CaptureHost,
    records: &[crate::store::SessionRecord],
    status: OpenCodeV2SourceStatus,
    limit: Option<CaptureLimitContext>,
) -> anyhow::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let changed = host
        .store()
        .set_opencode_v2_source_status(records, status, unix_now())?;
    if changed > 0 {
        ::tracing::debug!(
            event = "opencode_v2_source_state",
            status = status.code(),
            phase = limit.map(|limit| limit.kind.codes().0),
            reason = limit.map(|limit| limit.kind.codes().1),
            observed = limit.and_then(|limit| limit.observed),
            limit = limit.and_then(|limit| limit.limit),
            affected_sessions = changed
        );
        host.report_changed();
    }
    Ok(())
}

fn report_unbound_sources(
    host: &dyn CaptureHost,
    candidates: &[crate::store::SessionRecord],
    bound: &[crate::store::SessionRecord],
) -> anyhow::Result<()> {
    let bound_keys: HashSet<_> = bound.iter().map(|record| &record.key).collect();
    let unbound: Vec<_> = candidates
        .iter()
        .filter(|record| !bound_keys.contains(&record.key))
        .cloned()
        .collect();
    report_source_status(host, &unbound, OpenCodeV2SourceStatus::SourceMismatch)
}

#[derive(Debug)]
struct CaptureFailure(OpenCodeV2SourceStatus);

impl std::fmt::Display for CaptureFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0.code())
    }
}

impl std::error::Error for CaptureFailure {}

fn capture_failure_status(error: &anyhow::Error) -> OpenCodeV2SourceStatus {
    use antiburn_local::analysis::OpenCodeV2SnapshotError;
    use opencode_live::stream::EventStreamError;
    use opencode_live::{DatabaseAssociationError, ProbeError};
    for cause in error.chain() {
        if let Some(failure) = cause.downcast_ref::<CaptureFailure>() {
            return failure.0;
        }
        if let Some(snapshot) = cause.downcast_ref::<OpenCodeV2SnapshotError>() {
            return match snapshot {
                OpenCodeV2SnapshotError::UnsupportedSchema
                | OpenCodeV2SnapshotError::UnsupportedSource => {
                    OpenCodeV2SourceStatus::VersionUnsupported
                }
                OpenCodeV2SnapshotError::ScopeChanged => OpenCodeV2SourceStatus::SourceMismatch,
                OpenCodeV2SnapshotError::SourceLimit => OpenCodeV2SourceStatus::SourceLimit,
                OpenCodeV2SnapshotError::Cancelled => OpenCodeV2SourceStatus::CaptureGap,
            };
        }
        if let Some(registration) = cause.downcast_ref::<opencode_live::RegistrationError>() {
            return match registration {
                opencode_live::RegistrationError::TooLarge => OpenCodeV2SourceStatus::SourceLimit,
                opencode_live::RegistrationError::InvalidEndpoint => {
                    OpenCodeV2SourceStatus::SourceMismatch
                }
                _ => OpenCodeV2SourceStatus::ServiceUnavailable,
            };
        }
        if let Some(probe) = cause.downcast_ref::<ProbeError>() {
            return match probe {
                ProbeError::HttpStatus(401 | 403) => OpenCodeV2SourceStatus::AuthenticationRejected,
                ProbeError::VersionMismatch => OpenCodeV2SourceStatus::VersionUnsupported,
                ProbeError::ProcessMismatch => OpenCodeV2SourceStatus::SourceMismatch,
                _ => OpenCodeV2SourceStatus::ServiceUnavailable,
            };
        }
        if let Some(
            DatabaseAssociationError::Binding | DatabaseAssociationError::RegistrationChanged,
        ) = cause.downcast_ref::<DatabaseAssociationError>()
        {
            return OpenCodeV2SourceStatus::SourceMismatch;
        }
        if let Some(stream) = cause.downcast_ref::<EventStreamError>() {
            match stream {
                EventStreamError::HttpStatus(401 | 403) => {
                    return OpenCodeV2SourceStatus::AuthenticationRejected;
                }
                EventStreamError::CapacityExceeded
                | EventStreamError::Sse(opencode_live::SseError::FrameTooLarge) => {
                    return OpenCodeV2SourceStatus::SourceLimit;
                }
                EventStreamError::RegistrationChanged => {
                    return OpenCodeV2SourceStatus::SourceMismatch;
                }
                EventStreamError::Registration(_) => {}
                EventStreamError::Request => {
                    return OpenCodeV2SourceStatus::ServiceUnavailable;
                }
                _ => {}
            }
        }
    }
    OpenCodeV2SourceStatus::CaptureGap
}

struct ActiveCaptures<'a> {
    capture: OpenCodeV2CaptureEpoch,
    lease: CaptureLease<'a>,
    claims_by_session: HashMap<String, OpenCodeV2CaptureClaim>,
    joiners: HashMap<String, OpenCodeV2EventJoiner>,
    gapped_sessions: HashSet<String>,
    gapped_claims: HashMap<String, OpenCodeV2CaptureClaim>,
    retirement_reasons: HashMap<String, &'static str>,
}

impl ActiveCaptures<'_> {
    fn gap_session(&mut self, session_id: &str, reason: &'static str) -> anyhow::Result<()> {
        self.capture.mark_session_gap(session_id);
        if let Some(claim) = self.lease.mark_session_gap(session_id, reason)? {
            self.gapped_claims.insert(session_id.to_owned(), claim);
        }
        self.claims_by_session.remove(session_id);
        self.joiners.remove(session_id);
        self.gapped_sessions.insert(session_id.to_owned());
        Ok(())
    }

    fn remove_session_admission(&mut self, session_id: &str) {
        self.capture.retire_session(session_id);
        self.gapped_sessions.remove(session_id);
        self.claims_by_session.remove(session_id);
        self.joiners.remove(session_id);
    }

    fn retire_session(&mut self, session_id: &str, reason: &'static str) -> anyhow::Result<()> {
        self.remove_session_admission(session_id);
        if let Some(claim) = self.lease.mark_session_gap(session_id, reason)? {
            self.gapped_claims.insert(session_id.to_owned(), claim);
        } else if let Some(claim) = self.gapped_claims.get(session_id) {
            self.lease.store.mark_opencode_v2_capture_gap(
                std::slice::from_ref(claim),
                reason,
                unix_now(),
            )?;
        }
        if self.gapped_claims.contains_key(session_id) {
            self.retirement_reasons
                .insert(session_id.to_owned(), reason);
        }
        Ok(())
    }

    fn retire_inactive_session(&mut self, session_id: &str) -> anyhow::Result<()> {
        self.gap_session(session_id, "source-detached")?;
        self.remove_session_admission(session_id);
        self.gapped_claims.remove(session_id);
        Ok(())
    }

    fn admit_session(
        &mut self,
        session_id: &str,
        store: &Store,
        identity: &DatabaseIdentity,
        pump: &FeedPump,
    ) -> anyhow::Result<bool> {
        if !identity.verify_session(store, session_id)? {
            self.retire_session(session_id, "source-scope-changed")?;
            pump.exclude_session(session_id)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn observe_verified(
        &mut self,
        event: Value,
        store: &Store,
        identity: &DatabaseIdentity,
        pump: &FeedPump,
    ) -> anyhow::Result<Option<SessionKey>> {
        if !pump.admits(&event)? {
            return Ok(None);
        }
        if let Some(session_id) = event_session(&event)
            && !self.admit_session(session_id, store, identity, pump)?
        {
            return Ok(None);
        }
        self.observe(event, store)
    }

    fn renew_claims(&mut self, identity: &DatabaseIdentity, pump: &FeedPump) -> anyhow::Result<()> {
        let store = self.lease.store;
        let now = unix_now();
        if self.lease.claims.is_empty()
            || store.renew_opencode_v2_capture_epoch(&self.lease.claims, now, LEASE_SECS)?
        {
            return Ok(());
        }
        // Local deletion can invalidate one claim after the last scope read.
        for claim in self.lease.claims.clone() {
            if store.renew_opencode_v2_capture_epoch(
                std::slice::from_ref(&claim),
                now,
                LEASE_SECS,
            )? {
                continue;
            }
            let attached = identity.verify_session(store, &claim.key.session_id)?
                && store.session(&claim.key)?.is_some_and(|record| {
                    record.source_label == claim.source_label
                        && record.cwd.as_deref() == Some(claim.cwd.as_str())
                });
            if attached {
                self.gap_session(&claim.key.session_id, "capture-lease-lost")?;
            } else {
                self.retire_session(&claim.key.session_id, "source-scope-changed")?;
            }
            pump.exclude_session(&claim.key.session_id)?;
        }
        Ok(())
    }

    fn recheck_candidates(
        &self,
        store: &Store,
        previous: &[crate::store::SessionRecord],
    ) -> anyhow::Result<Vec<crate::store::SessionRecord>> {
        let mut records = live_candidates(store)?;
        let mut included = records
            .iter()
            .map(|record| record.key.clone())
            .collect::<HashSet<_>>();
        let pending_keys = self
            .claims_by_session
            .values()
            .filter(|claim| {
                !included.contains(&claim.key)
                    && self
                        .joiners
                        .get(&claim.key.session_id)
                        .is_some_and(OpenCodeV2EventJoiner::has_pending_work)
            })
            .map(|claim| claim.key.clone())
            .collect::<Vec<_>>();
        if !pending_keys.is_empty() {
            for record in store.opencode_v2_continuation_candidates(&pending_keys)? {
                if previous
                    .iter()
                    .any(|old| old.key == record.key && same_source_scope(old, &record))
                    && included.insert(record.key.clone())
                {
                    records.push(record);
                }
            }
        }
        Ok(records)
    }

    fn record_gaps(&mut self) -> anyhow::Result<()> {
        let sessions = self
            .capture
            .gapped_sessions()
            .filter(|id| !self.gapped_sessions.contains(*id))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for session in sessions {
            self.gap_session(&session, "session-event-sequence-gap")?;
        }
        Ok(())
    }

    fn retain_sessions(&mut self, records: &[crate::store::SessionRecord]) {
        let retained: HashSet<_> = records
            .iter()
            .map(|record| record.key.session_id.clone())
            .collect();
        self.capture.retain_sessions(&retained);
        self.gapped_sessions
            .retain(|session| retained.contains(session));
        self.gapped_claims
            .retain(|session, _| retained.contains(session));
        self.retirement_reasons
            .retain(|session, _| retained.contains(session));
    }

    fn replay_buffered(
        &mut self,
        host: &dyn CaptureHost,
        identity: &DatabaseIdentity,
        pump: &FeedPump,
        buffered: Vec<CapturedEvent>,
    ) -> anyhow::Result<()> {
        for event in buffered {
            pump.verify_registration(&host.registration_path()?)?;
            identity.verify()?;
            if !self.claims_by_session.contains_key(&event.session_id) {
                continue;
            }
            if !self.admit_session(&event.session_id, host.store(), identity, pump)? {
                continue;
            }
            if let Some(key) = self.apply_captured(event, host.store())? {
                host.announce(&key);
            }
        }
        Ok(())
    }

    fn observe(&mut self, event: Value, store: &Store) -> anyhow::Result<Option<SessionKey>> {
        if event_session(&event).is_some_and(|id| !self.claims_by_session.contains_key(id)) {
            return Ok(None);
        }
        match self.capture.observe(event).map_err(capture_gap_error)? {
            Some(event) => self.apply_captured(event, store),
            None => {
                self.record_gaps()?;
                Ok(None)
            }
        }
    }

    fn apply_captured(
        &mut self,
        event: CapturedEvent,
        store: &Store,
    ) -> anyhow::Result<Option<SessionKey>> {
        let scope_loss = match event.event().get("type").and_then(Value::as_str) {
            Some("session.moved") => Some("session-moved"),
            Some("session.deleted") => Some("session-deleted"),
            _ => None,
        };
        let outcome = if event.kind == BoundaryKind::Seed {
            process_seed_event(event, &mut self.joiners)
        } else {
            process_captured_event(event, &mut self.joiners, &self.claims_by_session, store)?
        };
        match outcome {
            ProcessResult::Advanced => Ok(None),
            ProcessResult::ResultPublished(key) => Ok(Some(key)),
            ProcessResult::SessionGap(session_id) => {
                let reason = scope_loss.unwrap_or_else(|| {
                    self.joiners
                        .get(&session_id)
                        .map_or("event-join-gap", |joiner| {
                            if joiner.gaps().contains(&Gap::UnsupportedVersion) {
                                "event-version-unsupported"
                            } else if joiner.gaps().contains(&Gap::CapacityExceeded) {
                                ::tracing::warn!(
                                    event = "opencode_v2_session_limited",
                                    status = OpenCodeV2SourceStatus::SourceLimit.code(),
                                    phase = "event_join",
                                    reason = "joiner_capacity_limit",
                                    affected_sessions = 1,
                                );
                                "source-limit"
                            } else {
                                "event-join-gap"
                            }
                        })
                });
                if scope_loss.is_some() {
                    self.retire_session(&session_id, reason)?;
                } else {
                    self.gap_session(&session_id, reason)?;
                }
                Ok(None)
            }
        }
    }
}

fn retire_scope_changed(
    host: &dyn CaptureHost,
    active: &mut ActiveCaptures<'_>,
    pump: &FeedPump,
    records: &[crate::store::SessionRecord],
) -> anyhow::Result<()> {
    for record in records {
        let session = &record.key.session_id;
        active.retire_session(session, "source-scope-changed")?;
        pump.exclude_session(session)?;
    }
    report_source_status(host, records, OpenCodeV2SourceStatus::SourceMismatch)
}

async fn recover_gapped_sessions(
    host: &Arc<dyn CaptureHost>,
    store: &Store,
    groups: &[DatabaseGroup],
    identity: &DatabaseIdentity,
    pump: &mut FeedPump,
    active: &mut ActiveCaptures<'_>,
    trace: &CaptureFailureTrace,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let groups: Vec<_> = groups
        .iter()
        .filter_map(|(path, records)| {
            let records: Vec<_> = records
                .iter()
                .filter(|record| active.gapped_sessions.contains(&record.key.session_id))
                .cloned()
                .collect();
            (!records.is_empty()).then(|| (path.clone(), records))
        })
        .collect();
    let mut records: Vec<_> = groups
        .iter()
        .flat_map(|(_, records)| records.iter().cloned())
        .collect();
    if records.is_empty() {
        return Ok(());
    }
    let mut recovery = OpenCodeV2CaptureEpoch::new(
        records
            .iter()
            .map(|record| record.key.session_id.clone())
            .collect(),
    )
    .map_err(capture_gap_error)?;
    recovery.confirm_subscription().map_err(capture_gap_error)?;
    identity.verify()?;
    if !prepare_capture_reconciliation(
        host,
        &records,
        identity,
        pump,
        &mut recovery,
        Some(&mut *active),
    )
    .await?
    {
        return Ok(());
    }
    pump.include_sessions(&records)?;
    let targets = store
        .opencode_v2_reconciliation_targets(&records)
        .map_err(|error| snapshot_limit_context(error, CaptureLimitKind::RecoverySnapshot))?;
    let mut baseline_task = BaselineTask::start(
        host.clone(),
        groups.clone(),
        targets,
        pump.cancelled.clone(),
        CaptureLimitKind::RecoverySnapshot,
    );
    let snapshot = loop {
        tokio::select! {
            result = &mut baseline_task.task => break result,
            event = pump.next() => {
                let event = event.context("OpenCode V2 feed ended during session recovery")??;
                pump.verify_registration(&host.registration_path()?)?;
                identity.verify()?;
                if let Some(session_id) = event_session(&event)
                    && !active.admit_session(session_id, store, identity, pump)?
                {
                    continue;
                }
                if let Err(gap) = recovery.observe(event.clone()) {
                    trace.recovery_limited(&snapshot_limit_context(capture_gap_error(gap), CaptureLimitKind::RecoveryBuffer));
                    return Ok(());
                }
                if let Some(key) = active.observe(event, store)? { host.announce(&key); }
            }
        }
    };
    ensure_capture_active(&pump.cancelled)?;
    pump.verify_registration(&host.registration_path()?)?;
    identity.verify()?;
    let mut snapshot = match snapshot {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(error)) => {
            trace.recovery_limited(&error);
            if error.downcast_ref::<antiburn_local::analysis::OpenCodeV2SnapshotError>()
                == Some(&antiburn_local::analysis::OpenCodeV2SnapshotError::ScopeChanged)
            {
                retire_scope_changed(&**host, active, pump, &records)?;
            }
            let status = capture_failure_status(&error);
            report_source_status_with_limit(
                &**host,
                &records,
                status,
                capture_limit_context(&error),
            )?;
            return Ok(());
        }
        Err(error) => return Err(error).context("reading the OpenCode V2 recovery snapshot"),
    };
    for record in &records {
        let current = store.session(&record.key)?;
        if current.as_ref().is_none_or(|current| {
            !same_source_scope(record, current) || current.activity_cursor != record.activity_cursor
        }) {
            snapshot
                .invalid_scopes
                .insert(record.key.session_id.clone());
        }
    }
    if !snapshot.invalid_scopes.is_empty() {
        let invalid_records = records
            .iter()
            .filter(|record| snapshot.invalid_scopes.contains(&record.key.session_id))
            .cloned()
            .collect::<Vec<_>>();
        retire_scope_changed(&**host, active, pump, &invalid_records)?;
    }
    let gapped_sessions = recovery
        .gapped_sessions()
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let gapped_records = records
        .iter()
        .filter(|record| {
            gapped_sessions.contains(&record.key.session_id)
                && !snapshot.invalid_scopes.contains(&record.key.session_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    for record in &gapped_records {
        active.gap_session(&record.key.session_id, "session-event-sequence-gap")?;
    }
    report_source_status(&**host, &gapped_records, OpenCodeV2SourceStatus::CaptureGap)?;
    records.retain(|record| {
        !snapshot.invalid_scopes.contains(&record.key.session_id)
            && !gapped_sessions.contains(&record.key.session_id)
    });
    if records.is_empty() {
        return Ok(());
    }
    let retained = records
        .iter()
        .map(|record| record.key.session_id.clone())
        .collect::<HashSet<_>>();
    recovery.retain_sessions(&retained);
    snapshot
        .baseline
        .retain(|session, _| retained.contains(session));
    snapshot
        .bindings
        .retain(|binding| retained.contains(&binding.observation.session_id));
    snapshot.invalid_scopes.clear();
    let baseline = &snapshot.baseline;
    let buffered = match recovery.finish_hydration(baseline.clone()) {
        Ok(buffered) => buffered,
        Err(gap) => {
            trace.recovery_limited(&snapshot_limit_context(
                capture_gap_error(gap),
                CaptureLimitKind::RecoveryBuffer,
            ));
            return Ok(());
        }
    };
    let starts = match capture_starts(store, &records, baseline) {
        Ok(starts) => starts,
        Err(error)
            if matches!(
                error.downcast_ref::<CaptureFailure>(),
                Some(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch))
            ) =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let Some(claims) = store.start_opencode_v2_capture_epoch_reconciled(
        &starts,
        &snapshot,
        &recovery.id().to_string(),
        unix_now(),
        LEASE_SECS,
    )?
    else {
        return Ok(());
    };
    if let Err(gap) = active.capture.resume_sessions(recovery) {
        trace.recovery_limited(&snapshot_limit_context(
            capture_gap_error(gap),
            CaptureLimitKind::RecoverySessions,
        ));
        store.mark_opencode_v2_capture_gap(&claims, "recovery-capacity", unix_now())?;
        return Ok(());
    }
    for claim in &claims {
        active.gapped_sessions.remove(&claim.key.session_id);
        active.gapped_claims.remove(&claim.key.session_id);
        active.retirement_reasons.remove(&claim.key.session_id);
        active.joiners.insert(
            claim.key.session_id.clone(),
            live_joiner(claim.baseline_sequence),
        );
        active
            .claims_by_session
            .insert(claim.key.session_id.clone(), claim.clone());
    }
    let recovered_claims = claims.len();
    let buffered_events = buffered.len();
    active.lease.claims.extend(claims);
    active.lease.active = true;
    active.replay_buffered(&**host, identity, pump, buffered)?;
    trace.recovered();
    ::tracing::debug!(
        event = "opencode_v2_sessions_recovered",
        status = "attached",
        recovered_claims,
        buffered_events,
        remaining_gaps = active.gapped_sessions.len(),
        elapsed_ms = started.elapsed().as_millis() as u64
    );
    Ok(())
}

async fn prepare_capture_reconciliation(
    host: &Arc<dyn CaptureHost>,
    records: &[crate::store::SessionRecord],
    identity: &DatabaseIdentity,
    pump: &mut FeedPump,
    capture: &mut OpenCodeV2CaptureEpoch,
    mut active: Option<&mut ActiveCaptures<'_>>,
) -> anyhow::Result<bool> {
    let started = std::time::Instant::now();
    let mut batches = 0usize;
    loop {
        ensure_capture_active(&pump.cancelled)?;
        match host
            .store()
            .prepare_opencode_v2_reconciliation(records, unix_now())?
        {
            OpenCodeV2ReconciliationPreparation::Ready => {
                ::tracing::debug!(
                    event = "opencode_v2_reconciliation_prepared",
                    status = "ready",
                    sessions = records.len(),
                    batches,
                    elapsed_ms = started.elapsed().as_millis() as u64
                );
                return Ok(true);
            }
            OpenCodeV2ReconciliationPreparation::Fenced => {
                ::tracing::debug!(
                    event = "opencode_v2_reconciliation_prepared",
                    status = "fenced",
                    reason = "source_scope_or_lease_changed",
                    sessions = records.len(),
                    batches,
                    elapsed_ms = started.elapsed().as_millis() as u64
                );
                return Ok(false);
            }
            OpenCodeV2ReconciliationPreparation::Progress => {
                batches += 1;
                tokio::task::yield_now().await;
                ensure_capture_active(&pump.cancelled)?;
                drain_buffered_feed(host, identity, pump, |event, pump| {
                    if let Some(session_id) = event_session(&event)
                        && !identity.verify_session(host.store(), session_id)?
                    {
                        capture.mark_session_gap(session_id);
                        pump.exclude_session(session_id)?;
                        if let Some(active) = active.as_deref_mut() {
                            active.retire_session(session_id, "source-scope-changed")?;
                        }
                    } else {
                        capture.observe(event.clone()).map_err(capture_gap_error)?;
                        if let Some(active) = active.as_deref_mut()
                            && let Some(key) = active.observe(event, host.store())?
                        {
                            host.announce(&key);
                        }
                    }
                    Ok(())
                })?;
            }
        }
    }
}

async fn retire_detached_scope_claims(
    host: &Arc<dyn CaptureHost>,
    identity: &DatabaseIdentity,
    pump: &mut FeedPump,
    capture: &mut OpenCodeV2CaptureEpoch,
    claims: &[OpenCodeV2CaptureClaim],
) -> anyhow::Result<()> {
    for claim in claims {
        ensure_capture_active(&pump.cancelled)?;
        host.store().mark_opencode_v2_capture_gap(
            std::slice::from_ref(claim),
            "source-scope-changed",
            unix_now(),
        )?;
        loop {
            ensure_capture_active(&pump.cancelled)?;
            match host.store().continue_opencode_v2_scope_retirement(
                std::slice::from_ref(claim),
                "source-scope-changed",
                unix_now(),
            )? {
                OpenCodeV2ReconciliationPreparation::Ready
                | OpenCodeV2ReconciliationPreparation::Fenced => break,
                OpenCodeV2ReconciliationPreparation::Progress => {
                    tokio::task::yield_now().await;
                    drain_buffered_feed(host, identity, pump, |event, pump| {
                        if let Some(session_id) = event_session(&event)
                            && !identity.verify_session(host.store(), session_id)?
                        {
                            capture.mark_session_gap(session_id);
                            pump.exclude_session(session_id)?;
                        } else {
                            capture.observe(event).map_err(capture_gap_error)?;
                        }
                        Ok(())
                    })?;
                }
            }
        }
    }
    Ok(())
}

async fn drain_scope_retirements(
    host: &Arc<dyn CaptureHost>,
    identity: &DatabaseIdentity,
    pump: &mut FeedPump,
    active: &mut ActiveCaptures<'_>,
) -> anyhow::Result<()> {
    while !active.retirement_reasons.is_empty() {
        let pending: Vec<_> = active
            .retirement_reasons
            .iter()
            .map(|(session, reason)| (session.clone(), *reason))
            .collect();
        for (session, reason) in pending {
            ensure_capture_active(&pump.cancelled)?;
            active.remove_session_admission(&session);
            pump.exclude_session(&session)?;
            let claim = active
                .gapped_claims
                .get(&session)
                .context("OpenCode V2 retirement has no exact claim")?;
            match host.store().continue_opencode_v2_scope_retirement(
                std::slice::from_ref(claim),
                reason,
                unix_now(),
            )? {
                OpenCodeV2ReconciliationPreparation::Ready
                | OpenCodeV2ReconciliationPreparation::Fenced => {
                    active.retirement_reasons.remove(&session);
                    active.gapped_claims.remove(&session);
                }
                OpenCodeV2ReconciliationPreparation::Progress => {
                    tokio::task::yield_now().await;
                    ensure_capture_active(&pump.cancelled)?;
                    drain_buffered_feed(host, identity, pump, |event, pump| {
                        if let Some(key) =
                            active.observe_verified(event, host.store(), identity, pump)?
                        {
                            host.announce(&key);
                        }
                        Ok(())
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn drain_buffered_feed(
    host: &Arc<dyn CaptureHost>,
    identity: &DatabaseIdentity,
    pump: &mut FeedPump,
    mut observe: impl FnMut(Value, &FeedPump) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    for _ in 0..64 {
        ensure_capture_active(&pump.cancelled)?;
        let Some(event) = pump.try_next() else {
            break;
        };
        let event = event?;
        pump.verify_registration(&host.registration_path()?)?;
        identity.verify()?;
        observe(event, pump)?;
    }
    Ok(())
}

fn admitted_source(record: &crate::store::SessionRecord) -> Option<OpenCodeV2AdmittedSource> {
    if record.key.environment_key != "native"
        || record.key.agent != "opencode"
        || record.source_kind != "providerDb"
        || record.wsl_distro.is_some()
    {
        return None;
    }
    OpenCodeV2AdmittedSource::from_activity_cursor(&record.activity_cursor, &record.key.session_id)
}

struct DatabaseIdentity {
    path: PathBuf,
    file: std::fs::File,
    identity: String,
}

impl DatabaseIdentity {
    fn read_admitted(
        path: &std::path::Path,
        records: &[crate::store::SessionRecord],
    ) -> anyhow::Result<Self> {
        let identity = Self::read(path)?;
        identity.verify_records(records)?;
        Ok(identity)
    }

    fn read(path: &std::path::Path) -> anyhow::Result<Self> {
        let file =
            std::fs::File::open(path).context("opening the OpenCode V2 database identity")?;
        let identity = antiburn_local::discovery::SourceStat::from_open_std_file(&file)
            .and_then(|stat| stat.identity)
            .context("OpenCode V2 database identity is unavailable")?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity,
        })
    }

    fn verify(&self) -> anyhow::Result<()> {
        let current = Self::read(&self.path)?;
        let held = antiburn_local::discovery::SourceStat::from_open_std_file(&self.file)
            .and_then(|stat| stat.identity);
        if current.identity != self.identity || held.as_ref() != Some(&self.identity) {
            return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
        }
        Ok(())
    }

    fn verify_records(&self, records: &[crate::store::SessionRecord]) -> anyhow::Result<()> {
        for record in records {
            if !self.matches_record(record) {
                return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
            }
        }
        Ok(())
    }

    fn matches_record(&self, record: &crate::store::SessionRecord) -> bool {
        let Some(admitted) = admitted_source(record) else {
            return false;
        };
        admitted.file_identity == self.identity
            && std::fs::canonicalize(admitted.database_path).is_ok_and(|path| path == self.path)
    }

    fn verify_session(&self, store: &Store, session_id: &str) -> anyhow::Result<bool> {
        let Some(record) = store.session(&SessionKey::new("native", "opencode", session_id))?
        else {
            return Ok(false);
        };
        Ok(self.matches_record(&record))
    }
}

async fn locate_v2_sources(
    records: &[crate::store::SessionRecord],
    cancelled: &AtomicBool,
) -> anyhow::Result<Vec<(crate::store::SessionRecord, PathBuf)>> {
    let mut sources = Vec::with_capacity(records.len());
    let mut identities = HashMap::new();
    for record in records {
        ensure_capture_active(cancelled)?;
        let Some(admitted) = admitted_source(record) else {
            continue;
        };
        let Some(identity) = identities
            .entry(admitted.database_path.clone())
            .or_insert_with(|| {
                DatabaseIdentity::read(&admitted.database_path)
                    .map(|current| current.identity)
                    .ok()
            })
        else {
            continue;
        };
        if *identity != admitted.file_identity {
            continue;
        }
        sources.push((record.clone(), admitted.database_path));
    }
    Ok(sources)
}

async fn cancel_poll(cancelled: &AtomicBool) {
    while !cancelled.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

type DatabaseGroup = (PathBuf, Vec<crate::store::SessionRecord>);

struct BaselineTask {
    task: tokio::task::JoinHandle<anyhow::Result<CaptureReconciliation>>,
    cancelled: Arc<AtomicBool>,
}

impl BaselineTask {
    fn start(
        host: Arc<dyn CaptureHost>,
        groups: Vec<DatabaseGroup>,
        targets: Vec<ToolResultObservation>,
        capture_cancelled: Arc<AtomicBool>,
        limit_kind: CaptureLimitKind,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let read_cancelled = cancelled.clone();
        let task = tokio::task::spawn_blocking(move || {
            host.read_baselines(&groups, &targets, &|| {
                read_cancelled.load(Ordering::SeqCst) || capture_cancelled.load(Ordering::SeqCst)
            })
            .map_err(|error| snapshot_limit_context(error, limit_kind))
        });
        Self { task, cancelled }
    }
}

impl Drop for BaselineTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.task.abort();
    }
}

fn ensure_capture_active(cancelled: &AtomicBool) -> anyhow::Result<()> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(opencode_live::stream::EventStreamError::Cancelled.into());
    }
    Ok(())
}

async fn select_bound_databases(
    registration_path: &std::path::Path,
    sources: Vec<(crate::store::SessionRecord, PathBuf)>,
    connect_timeout: Duration,
    cancelled: &AtomicBool,
) -> anyhow::Result<Vec<DatabaseGroup>> {
    let mut grouped = BTreeMap::<PathBuf, Vec<crate::store::SessionRecord>>::new();
    for (record, path) in sources {
        let Ok(path) = std::fs::canonicalize(path) else {
            continue;
        };
        grouped.entry(path).or_default().push(record);
    }
    if grouped.len() > CAPTURE_DATABASE_LIMIT {
        return Err(source_limit_error(
            CaptureLimitKind::Databases,
            Some(grouped.len()),
            Some(CAPTURE_DATABASE_LIMIT),
        ));
    }

    let mut bound = Vec::new();
    for (path, records) in grouped {
        ensure_capture_active(cancelled)?;
        let is_bound = tokio::select! {
            result = opencode_live::stream::verify_binding(
                registration_path.to_path_buf(), path.clone(), connect_timeout,
            ) => match result {
                Ok(_) => true,
                Err(opencode_live::stream::EventStreamError::Association(opencode_live::DatabaseAssociationError::Binding)) => false,
                Err(error) => return Err(error).context("checking OpenCode V2 source binding"),
            },
            () = cancel_poll(cancelled) => return Err(opencode_live::stream::EventStreamError::Cancelled.into()),
        };
        ensure_capture_active(cancelled)?;
        if is_bound {
            bound.push((path, records));
        }
    }
    if bound.len() > 1 {
        return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
    }
    Ok(bound)
}

fn read_baselines(
    groups: &[DatabaseGroup],
    targets: &[ToolResultObservation],
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<CaptureReconciliation> {
    let mut all = CaptureReconciliation::default();
    for (path, records) in groups {
        if cancel() {
            bail!("OpenCode V2 baseline read was cancelled");
        }
        let scopes: Vec<_> = records
            .iter()
            .map(|record| {
                Ok((
                    record.key.session_id.clone(),
                    record
                        .cwd
                        .clone()
                        .context("OpenCode V2 baseline session has no CWD")?,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let group_targets: Vec<_> = targets
            .iter()
            .filter(|target| scopes.iter().any(|(id, _)| *id == target.session_id))
            .cloned()
            .collect();
        let snapshot = read_capture_reconciliation_for_scope(
            path,
            SourceFormat::OpenCodeSqliteV2,
            &scopes,
            &group_targets,
            cancel,
        )?;
        all.invalid_scopes.extend(snapshot.invalid_scopes);
        for (session_id, sequence) in snapshot.baseline {
            if all.baseline.insert(session_id, sequence).is_some() {
                bail!("OpenCode V2 baseline has duplicate session identities");
            }
        }
        all.bindings.extend(snapshot.bindings);
    }
    Ok(all)
}

fn capture_starts(
    store: &Store,
    records: &[crate::store::SessionRecord],
    baseline: &HashMap<String, Option<u64>>,
) -> anyhow::Result<Vec<OpenCodeV2CaptureStart>> {
    use rusqlite::{OptionalExtension, params};

    let connection = store.lock();
    let mut binding = connection.prepare(
        "SELECT incarnation, source_generation FROM session
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND source_kind = 'providerDb' AND source_label = ?4 AND cwd = ?5
            AND wsl_distro IS NULL AND activity_cursor = ?6",
    )?;
    records
        .iter()
        .map(|record| {
            let (incarnation, source_generation) = binding
                .query_row(
                    params![
                        record.key.environment_key,
                        record.key.agent,
                        record.key.session_id,
                        record.source_label,
                        record.cwd,
                        record.activity_cursor,
                    ],
                    |row| Ok((crate::store::Incarnation(row.get(0)?), row.get(1)?)),
                )
                .optional()?
                .ok_or(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch))?;
            let baseline_sequence = baseline
                .get(&record.key.session_id)
                .copied()
                .with_context(|| "OpenCode V2 baseline omitted an admitted session")?;
            let admitted_source = OpenCodeV2AdmittedSource::from_activity_cursor(
                &record.activity_cursor,
                &record.key.session_id,
            )
            .ok_or(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch))?;
            Ok(OpenCodeV2CaptureStart {
                key: record.key.clone(),
                incarnation,
                source_generation,
                source_label: record.source_label.clone(),
                cwd: record
                    .cwd
                    .clone()
                    .context("OpenCode V2 live session has no CWD")?,
                admitted_source,
                baseline_sequence,
            })
        })
        .collect()
}

fn live_joiner(baseline: Option<u64>) -> OpenCodeV2EventJoiner {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(baseline.unwrap_or(0));
    joiner
}

fn process_seed_event(
    event: CapturedEvent,
    joiners: &mut HashMap<String, OpenCodeV2EventJoiner>,
) -> ProcessResult {
    let Some(joiner) = joiners.get_mut(&event.session_id) else {
        return ProcessResult::SessionGap(event.session_id);
    };
    joiner.observe_captured_evidence(&event);
    if has_hard_join_gap(joiner) {
        ProcessResult::SessionGap(event.session_id)
    } else {
        ProcessResult::Advanced
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProcessResult {
    Advanced,
    ResultPublished(SessionKey),
    SessionGap(String),
}

fn process_captured_event(
    event: CapturedEvent,
    joiners: &mut HashMap<String, OpenCodeV2EventJoiner>,
    claims: &HashMap<String, OpenCodeV2CaptureClaim>,
    store: &Store,
) -> anyhow::Result<ProcessResult> {
    let Some(joiner) = joiners.get_mut(&event.session_id) else {
        return Ok(ProcessResult::SessionGap(event.session_id));
    };
    let evidence = joiner.observe_captured_evidence(&event);
    if has_hard_join_gap(joiner) {
        return Ok(ProcessResult::SessionGap(event.session_id));
    }
    let Some(claim) = claims.get(&event.session_id) else {
        return Ok(ProcessResult::SessionGap(event.session_id));
    };
    let applied = store.advance_opencode_v2_evidence(
        claim,
        event.sequence,
        &evidence,
        unix_now(),
        LEASE_SECS,
    )?;
    if !applied {
        return Ok(ProcessResult::SessionGap(event.session_id));
    }
    Ok(
        if evidence.events.iter().any(|event| {
            matches!(
                event.fact,
                antiburn_local::analysis::NativeEvidenceFact::Message(_)
                    | antiburn_local::analysis::NativeEvidenceFact::ResponseCycle(_)
                    | antiburn_local::analysis::NativeEvidenceFact::MessageRemoval(_)
            )
        }) {
            ProcessResult::ResultPublished(claim.key.clone())
        } else {
            ProcessResult::Advanced
        },
    )
}

fn has_hard_join_gap(joiner: &OpenCodeV2EventJoiner) -> bool {
    joiner.gaps().iter().any(|gap| {
        matches!(
            gap,
            Gap::MissingSequence
                | Gap::SequenceDiscontinuity
                | Gap::ContinuityLost
                | Gap::ConflictingDuplicate
                | Gap::CapacityExceeded
                | Gap::UnknownShape
                | Gap::UnsupportedVersion
        )
    })
}

fn capture_gap_error(gap: CaptureGap) -> anyhow::Error {
    if gap == CaptureGap::CapacityExceeded {
        source_limit_error(CaptureLimitKind::CaptureBuffer, None, None)
    } else {
        anyhow::Error::new(CaptureBoundaryFailure(gap))
    }
}

#[derive(Debug)]
struct CaptureBoundaryFailure(CaptureGap);

impl std::fmt::Display for CaptureBoundaryFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "OpenCode V2 capture boundary failed: {:?}",
            self.0
        )
    }
}

impl std::error::Error for CaptureBoundaryFailure {}

fn changed_scope_sessions(
    previous: &[crate::store::SessionRecord],
    current: &[crate::store::SessionRecord],
) -> HashSet<String> {
    previous
        .iter()
        .filter(|record| {
            current
                .iter()
                .find(|candidate| candidate.key == record.key)
                .is_none_or(|candidate| !same_source_scope(record, candidate))
        })
        .map(|record| record.key.session_id.clone())
        .collect()
}

fn same_source_scope(
    previous: &crate::store::SessionRecord,
    current: &crate::store::SessionRecord,
) -> bool {
    current.source_label == previous.source_label
        && current.cwd == previous.cwd
        && admitted_source(current) == admitted_source(previous)
}

struct CaptureLease<'a> {
    store: &'a Store,
    claims: Vec<OpenCodeV2CaptureClaim>,
    active: bool,
}

impl<'a> CaptureLease<'a> {
    fn new(store: &'a Store, claims: Vec<OpenCodeV2CaptureClaim>) -> Self {
        Self {
            store,
            claims,
            active: true,
        }
    }

    fn mark_gap(&mut self, reason: &str) -> anyhow::Result<()> {
        self.store
            .mark_opencode_v2_capture_gap(&self.claims, reason, unix_now())?;
        self.active = false;
        Ok(())
    }

    fn mark_session_gap(
        &mut self,
        session_id: &str,
        reason: &str,
    ) -> anyhow::Result<Option<OpenCodeV2CaptureClaim>> {
        let claim = if let Some(index) = self
            .claims
            .iter()
            .position(|claim| claim.key.session_id == session_id)
        {
            let claim = self.claims[index].clone();
            self.store.mark_opencode_v2_capture_gap(
                std::slice::from_ref(&claim),
                reason,
                unix_now(),
            )?;
            Some(self.claims.remove(index))
        } else {
            None
        };
        if self.claims.is_empty() {
            self.active = false;
        }
        Ok(claim)
    }
}

impl Drop for CaptureLease<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ =
                self.store
                    .mark_opencode_v2_capture_gap(&self.claims, "worker-stopped", unix_now());
        }
    }
}

async fn verify_bound_databases(
    registration_path: &std::path::Path,
    groups: &[DatabaseGroup],
    expected: &opencode_live::ServiceRegistration,
    connect_timeout: Duration,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    for (database_path, _) in groups {
        ensure_capture_active(cancelled)?;
        if opencode_live::read_registration(registration_path)? != *expected {
            return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
        }
        tokio::select! {
            result = opencode_live::stream::verify_binding(
                registration_path.to_path_buf(), database_path.clone(), connect_timeout,
            ) => { result.context("checking OpenCode capture binding")?; },
            () = cancel_poll(cancelled) => return Err(opencode_live::stream::EventStreamError::Cancelled.into()),
        }
        ensure_capture_active(cancelled)?;
    }
    Ok(())
}

fn event_session(event: &Value) -> Option<&str> {
    event
        .pointer("/durable/aggregateID")
        .and_then(Value::as_str)
        .or_else(|| event.pointer("/data/sessionID").and_then(Value::as_str))
}

struct FeedPump {
    receiver: mpsc::Receiver<anyhow::Result<Value>>,
    cancelled: Arc<AtomicBool>,
    task: JoinHandle<()>,
    admitted: Arc<RwLock<HashSet<String>>>,
    registration: opencode_live::ServiceRegistration,
}

impl FeedPump {
    fn start(
        mut stream: EventStream,
        cancelled: Arc<AtomicBool>,
        admitted: HashSet<String>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(64);
        let task_cancelled = cancelled.clone();
        let admitted = Arc::new(RwLock::new(admitted));
        let task_admitted = admitted.clone();
        let registration = stream.registration().clone();
        let task = tauri::async_runtime::spawn(async move {
            loop {
                match stream.next_event(&task_cancelled).await {
                    Ok(event) => {
                        if !event.data.get("durable").is_some_and(Value::is_object) {
                            continue;
                        }
                        let session = event_session(&event.data);
                        let is_admitted = match task_admitted.read() {
                            Ok(admitted) => {
                                session.is_some_and(|session| admitted.contains(session))
                            }
                            Err(_) => break,
                        };
                        if !is_admitted {
                            continue;
                        }
                        tokio::select! {
                            result = sender.send(Ok(event.data)) => if result.is_err() { break; },
                            () = cancel_poll(&task_cancelled) => break,
                        }
                    }
                    Err(opencode_live::stream::EventStreamError::Cancelled) => break,
                    Err(error) => {
                        tokio::select! {
                            _ = sender.send(Err(error.into())) => {},
                            () = cancel_poll(&task_cancelled) => {},
                        }
                        break;
                    }
                }
            }
        });
        Self {
            receiver,
            cancelled,
            task,
            admitted,
            registration,
        }
    }

    fn include_sessions(&self, records: &[crate::store::SessionRecord]) -> anyhow::Result<()> {
        let mut admitted = self
            .admitted
            .write()
            .map_err(|_| anyhow::anyhow!("OpenCode V2 admitted-session lock is poisoned"))?;
        let included = admitted
            .iter()
            .cloned()
            .chain(records.iter().map(|record| record.key.session_id.clone()))
            .collect::<HashSet<_>>();
        if included.len() > CAPTURE_SESSION_LIMIT {
            return Err(source_limit_error(
                CaptureLimitKind::RecoverySessions,
                Some(included.len()),
                Some(CAPTURE_SESSION_LIMIT),
            ));
        }
        *admitted = included;
        Ok(())
    }

    fn replace_sessions(&self, records: &[crate::store::SessionRecord]) -> anyhow::Result<()> {
        if records.len() > CAPTURE_SESSION_LIMIT {
            return Err(source_limit_error(
                CaptureLimitKind::RetainedSessions,
                Some(records.len()),
                Some(CAPTURE_SESSION_LIMIT),
            ));
        }
        *self
            .admitted
            .write()
            .map_err(|_| anyhow::anyhow!("OpenCode V2 admitted-session lock is poisoned"))? =
            records
                .iter()
                .map(|record| record.key.session_id.clone())
                .collect();
        Ok(())
    }

    fn exclude_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.admitted
            .write()
            .map_err(|_| anyhow::anyhow!("OpenCode V2 admitted-session lock is poisoned"))?
            .remove(session_id);
        Ok(())
    }

    fn admits(&self, event: &Value) -> anyhow::Result<bool> {
        let admitted = self
            .admitted
            .read()
            .map_err(|_| anyhow::anyhow!("OpenCode V2 admitted-session lock is poisoned"))?;
        Ok(event_session(event).is_some_and(|session| admitted.contains(session)))
    }

    fn verify_registration(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if opencode_live::read_registration(path)? != self.registration {
            return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
        }
        Ok(())
    }

    async fn next(&mut self) -> Option<anyhow::Result<Value>> {
        loop {
            let event = match self.receiver.recv().await? {
                Ok(event) => event,
                Err(error) => return Some(Err(error)),
            };
            match self.admits(&event) {
                Ok(true) => return Some(Ok(event)),
                Ok(false) => {}
                Err(error) => return Some(Err(error)),
            }
        }
    }

    fn try_next(&mut self) -> Option<anyhow::Result<Value>> {
        loop {
            let event = match self.receiver.try_recv() {
                Ok(Ok(event)) => event,
                Ok(Err(error)) => return Some(Err(error)),
                Err(mpsc::error::TryRecvError::Empty) => return None,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Some(Err(
                        opencode_live::stream::EventStreamError::StreamEnded.into()
                    ));
                }
            };
            match self.admits(&event) {
                Ok(true) => return Some(Ok(event)),
                Ok(false) => {}
                Err(error) => return Some(Err(error)),
            }
        }
    }

    async fn stop(mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut self.task).await;
    }
}

impl Drop for FeedPump {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.task.abort();
    }
}

#[cfg(test)]
#[path = "opencode_v2_runtime_tests.rs"]
mod runtime_tests;

#[cfg(test)]
#[path = "opencode_v2_tracing_tests.rs"]
mod tracing_tests;

#[cfg(all(test, unix))]
#[path = "opencode_v2_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use antiburn_local::analysis::{
        ToolResultPosition,
        opencode_v2_capture::{BoundaryKind, CapturedEvent},
    };
    use serde_json::{Value, json};

    use super::*;
    use crate::store::SessionRecord;

    #[test]
    fn database_identity_rejects_replacement_at_the_same_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        std::fs::write(&path, "first").unwrap();
        let identity = DatabaseIdentity::read(&path).unwrap();
        identity.verify().unwrap();
        std::fs::rename(&path, directory.path().join("old.db")).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        assert!(identity.verify().is_err());
    }

    #[test]
    fn source_failures_keep_closed_actionable_statuses() {
        use opencode_live::stream::EventStreamError;
        use opencode_live::{DatabaseAssociationError, ProbeError, RegistrationError, SseError};
        for (error, expected) in [
            (
                anyhow::Error::new(RegistrationError::Io),
                OpenCodeV2SourceStatus::ServiceUnavailable,
            ),
            (
                anyhow::Error::new(ProbeError::HttpStatus(401)),
                OpenCodeV2SourceStatus::AuthenticationRejected,
            ),
            (
                anyhow::Error::new(DatabaseAssociationError::Binding),
                OpenCodeV2SourceStatus::SourceMismatch,
            ),
            (
                anyhow::Error::new(ProbeError::VersionMismatch),
                OpenCodeV2SourceStatus::VersionUnsupported,
            ),
            (
                anyhow::Error::new(EventStreamError::Sse(SseError::FrameTooLarge)),
                OpenCodeV2SourceStatus::SourceLimit,
            ),
        ] {
            assert_eq!(
                capture_failure_status(&error.context("capture failed")),
                expected
            );
        }
    }

    #[test]
    fn event_version_gap_releases_capture_and_saves_typed_status_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let now = unix_now();
        let record = record(now);
        let claim = capture_claim(&store, &record, now);
        let mut capture = OpenCodeV2CaptureEpoch::new(vec![record.key.session_id.clone()]).unwrap();
        capture.confirm_subscription().unwrap();
        capture
            .finish_hydration(HashMap::from([(record.key.session_id.clone(), Some(0))]))
            .unwrap();
        let mut active = ActiveCaptures {
            capture,
            lease: CaptureLease::new(&store, vec![claim.clone()]),
            claims_by_session: HashMap::from([(record.key.session_id.clone(), claim)]),
            joiners: HashMap::from([(record.key.session_id.clone(), OpenCodeV2EventJoiner::new())]),
            gapped_sessions: HashSet::new(),
            gapped_claims: HashMap::new(),
            retirement_reasons: HashMap::new(),
        };
        let mut unknown = event(
            1,
            "unsupported",
            "session.step.started",
            json!({"assistantMessageID":"step"}),
        );
        unknown["durable"]["version"] = json!(99);
        assert!(active.observe(unknown, &store).unwrap().is_none());
        let (status, lease): (String, Option<i64>) = store
            .lock()
            .query_row(
                "SELECT d.code, c.lease_expires_at_epoch FROM opencode_v2_source_status d
              JOIN opencode_v2_capture c USING(environment_key, agent, session_id)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "version-unsupported");
        assert_eq!(lease, None);
        assert!(active.lease.claims.is_empty());
        assert!(active.gapped_sessions.contains(&record.key.session_id));
    }

    pub(super) fn record(now: i64) -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", "opencode", "ses_v2"),
            source_kind: "providerDb".into(),
            source_label: "opencode:ses_v2:open_code_v2_sqlite".into(),
            wsl_distro: None,
            title: Some("V2 test".into()),
            title_source: Some("explicit".into()),
            cwd: Some("/repo".into()),
            surface: "cli".into(),
            updated_at_epoch: Some(now),
            activity_cursor: format!(
                "opencode-admission-v2:{}",
                serde_json::to_string(&(
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("synthetic/opencode.db"),
                    "ses_v2",
                    "synthetic-database-identity",
                    "synthetic-scope"
                ))
                .unwrap()
            ),
            activity_source: "event".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: Some("v2-fingerprint".into()),
        }
    }

    fn event(sequence: u64, id: &str, event_type: &str, extra: Value) -> Value {
        let durable_version =
            if matches!(event_type, "session.tool.success" | "session.tool.failed") {
                2
            } else {
                1
            };
        let mut data = json!({"sessionID":"ses_v2"});
        data.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        json!({
            "id": id,
            "type": event_type,
            "created": sequence as f64,
            "durable":{"aggregateID":"ses_v2", "seq":sequence, "version":durable_version},
            "data": data,
        })
    }

    fn enabled_live_activation(store: &Store, now: i64) {
        store
            .lock()
            .execute(
                "INSERT INTO burn_check_activation (
                    check_id, activation_scope_key, definition_revision, enabled,
                    generation, bootstrap_sequence, applied_sequence, updated_at_epoch)
                 VALUES ('test-check', ?1, 'rev-1', 1, 1, 0, 0, ?2)",
                rusqlite::params![
                    serde_json::to_string(&json!({
                        "environment":"native", "session_cwd":"/repo", "run":"Live"
                    }))
                    .unwrap(),
                    now,
                ],
            )
            .unwrap();
    }

    fn capture_claim(store: &Store, record: &SessionRecord, now: i64) -> OpenCodeV2CaptureClaim {
        store
            .upsert_sessions(
                std::slice::from_ref(record),
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
        enabled_live_activation(store, now);
        let (presence, _) = store
            .session_presence_for_keys(std::slice::from_ref(&record.key))
            .unwrap();
        let source_generation = store
            .source_generation_batch(std::slice::from_ref(&record.key))
            .unwrap()[0]
            .unwrap();
        let baseline = HashMap::from([("ses_v2".to_owned(), Some(0))]);
        let start = OpenCodeV2CaptureStart {
            key: record.key.clone(),
            incarnation: presence[0].incarnation,
            source_generation,
            source_label: record.source_label.clone(),
            cwd: record.cwd.clone().unwrap(),
            admitted_source: OpenCodeV2AdmittedSource::from_activity_cursor(
                &record.activity_cursor,
                &record.key.session_id,
            )
            .unwrap(),
            baseline_sequence: baseline["ses_v2"],
        };
        store
            .start_opencode_v2_capture_epoch(&[start], "worker-test-epoch", now, LEASE_SECS)
            .unwrap()
            .unwrap()
            .remove(0)
    }

    fn active_capture<'a>(store: &'a Store, record: &SessionRecord) -> ActiveCaptures<'a> {
        let claim = capture_claim(store, record, unix_now());
        let mut capture = OpenCodeV2CaptureEpoch::new(vec![record.key.session_id.clone()]).unwrap();
        capture.confirm_subscription().unwrap();
        capture
            .finish_hydration(HashMap::from([(record.key.session_id.clone(), Some(0))]))
            .unwrap();
        ActiveCaptures {
            capture,
            lease: CaptureLease::new(store, vec![claim.clone()]),
            claims_by_session: HashMap::from([(record.key.session_id.clone(), claim)]),
            joiners: HashMap::from([(record.key.session_id.clone(), OpenCodeV2EventJoiner::new())]),
            gapped_sessions: HashSet::new(),
            gapped_claims: HashMap::new(),
            retirement_reasons: HashMap::new(),
        }
    }

    #[test]
    fn unattached_events_do_not_gap_a_healthy_claim() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let record = record(unix_now());
        let mut active = active_capture(&store, &record);
        let mut unrelated = event(1, "unattached", "session.step.started", json!({}));
        unrelated["durable"]["aggregateID"] = json!("unattached");
        unrelated["durable"]["version"] = json!(99);
        assert!(active.observe(unrelated, &store).unwrap().is_none());
        assert_eq!(active.lease.claims.len(), 1);
        assert!(active.gapped_sessions.is_empty());
    }

    #[test]
    fn resultless_events_advance_idle_progress_without_publishing_tool_results() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let record = record(unix_now());
        let mut active = active_capture(&store, &record);
        for sequence in 1..=200 {
            active
                .observe(
                    event(
                        sequence,
                        &format!("update-{sequence}"),
                        "session.updated",
                        json!({"info":{"id":"ses_v2"}}),
                    ),
                    &store,
                )
                .unwrap();
        }
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT last_sequence FROM opencode_v2_capture WHERE session_id = 'ses_v2'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            200
        );
        assert!(
            store
                .native_tool_results_current(&record.key)
                .unwrap()
                .is_empty()
        );
        assert!(active.gapped_sessions.is_empty());
    }

    #[test]
    fn old_capture_claims_do_not_keep_idle_sessions_as_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let record = record(unix_now() - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS - 1);
        let _claim = capture_claim(&store, &record, unix_now());
        assert!(live_candidates(&store).unwrap().is_empty());
    }

    #[test]
    fn continuation_candidates_deduplicate_recent_rows_and_reject_changed_sources() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let record = record(unix_now());
        let mut active = active_capture(&store, &record);
        active
            .observe(
                event(
                    1,
                    "open-step",
                    "session.step.started",
                    json!({"assistantMessageID":"step"}),
                ),
                &store,
            )
            .unwrap();
        let previous = std::slice::from_ref(&record);
        assert_eq!(
            active.recheck_candidates(&store, previous).unwrap().len(),
            1
        );
        store
            .lock()
            .execute(
                "UPDATE session SET updated_at_epoch = ?1 WHERE session_id = 'ses_v2'",
                [unix_now() - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS - 10],
            )
            .unwrap();
        assert_eq!(
            active.recheck_candidates(&store, previous).unwrap().len(),
            1
        );
        let admitted = admitted_source(&record).unwrap();
        let changed_cursor = format!(
            "opencode-admission-v2:{}",
            json!([
                admitted.database_path,
                record.key.session_id,
                "changed-identity",
                "scope"
            ])
        );
        store
            .lock()
            .execute(
                "UPDATE session SET activity_cursor = ?1 WHERE session_id = 'ses_v2'",
                [changed_cursor],
            )
            .unwrap();
        assert!(
            active
                .recheck_candidates(&store, previous)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn admission_uses_the_shared_cursor_identity_limit() {
        let mut record = record(unix_now());
        record.activity_cursor = format!(
            "opencode-admission-v2:{}",
            json!([
                "/synthetic/opencode.db",
                record.key.session_id,
                "x".repeat(513),
                "scope"
            ])
        );
        assert!(admitted_source(&record).is_none());
    }

    #[test]
    fn retained_capture_state_accepts_more_than_a_lifetime_session_limit() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let record = record(unix_now());
        let mut active = active_capture(&store, &record);
        for index in 0..(CAPTURE_SESSION_LIMIT + 1) {
            active.retain_sessions(std::slice::from_ref(&record));
            let session = format!("child-{index}");
            let mut recovered = OpenCodeV2CaptureEpoch::new(vec![session.clone()]).unwrap();
            recovered.confirm_subscription().unwrap();
            recovered
                .finish_hydration(HashMap::from([(session, Some(0))]))
                .unwrap();
            active.capture.resume_sessions(recovered).unwrap();
        }
        active.retain_sessions(std::slice::from_ref(&record));
        assert_eq!(active.lease.claims.len(), 1);
        assert!(active.gapped_sessions.is_empty());
    }

    #[test]
    fn live_event_join_publishes_v2_result_and_sequence_progress() {
        let store = Store::open_in_memory(Path::new("/tmp/opencode-v2-live-worker-test")).unwrap();
        let now = unix_now();
        let record = record(now);
        let claim = capture_claim(&store, &record, now);
        let claims = HashMap::from([(record.key.session_id.clone(), claim)]);
        let mut joiners =
            HashMap::from([(record.key.session_id.clone(), OpenCodeV2EventJoiner::new())]);
        let events = [
            event(
                1,
                "inbox-enqueued",
                "session.inbox.enqueued",
                json!({
                    "inboxID":"msg_user",
                    "item":{"type":"user","payload":{"text":"read this file","files":[],"agents":[],"skills":[]},"delivery":"queue"}
                }),
            ),
            event(
                2,
                "inbox-delivered",
                "session.inbox.delivered",
                json!({"inboxID":"msg_user"}),
            ),
            event(
                3,
                "step-started",
                "session.step.started",
                json!({"assistantMessageID":"msg_assistant"}),
            ),
            event(
                4,
                "tool-started",
                "session.tool.input.started",
                json!({"assistantMessageID":"msg_assistant","id":"call_read","name":"Read"}),
            ),
            event(
                5,
                "tool-called",
                "session.tool.called",
                json!({"assistantMessageID":"msg_assistant","id":"call_read","input":{"path":"src/main.rs"},"executed":false}),
            ),
            event(
                6,
                "tool-terminal",
                "session.tool.success",
                json!({"assistantMessageID":"msg_assistant","id":"call_read","content":[{"type":"text","text":"contents"}],"executed":false}),
            ),
        ];
        let mut terminal_outcome = ProcessResult::Advanced;
        for event in events {
            terminal_outcome = process_captured_event(
                CapturedEvent::new(event, BoundaryKind::Live).unwrap(),
                &mut joiners,
                &claims,
                &store,
            )
            .unwrap();
        }

        assert_eq!(
            terminal_outcome,
            ProcessResult::ResultPublished(record.key.clone())
        );
        let publications = store.native_tool_result_publications_after(0, 100).unwrap();
        let result = publications
            .iter()
            .find(|publication| {
                publication.outcome
                    == crate::store::native_tool_results::ToolResultPublicationOutcome::Update
            })
            .unwrap();
        assert_eq!(
            result.source_format.as_deref(),
            Some("\"open_code_v2_sqlite\"")
        );
        assert_eq!(result.live_eligible, Some(true));
        assert_eq!(
            result.payload.as_ref().unwrap().completeness,
            antiburn_local::analysis::ToolResultCompleteness::Incomplete
        );
        assert!(
            !result
                .payload
                .as_ref()
                .unwrap()
                .context
                .as_ref()
                .unwrap()
                .complete_through_event
        );
        assert_eq!(
            result.native_position,
            Some(ToolResultPosition::OpenCodeV2Event {
                sequence: 6,
                item_index: 0,
            })
        );
        let progress: (i64, i64) = store
            .lock()
            .query_row(
                "SELECT last_sequence, history_gap FROM opencode_v2_capture
                  WHERE environment_key='native' AND agent='opencode' AND session_id='ses_v2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(progress, (6, 1));
    }
}
