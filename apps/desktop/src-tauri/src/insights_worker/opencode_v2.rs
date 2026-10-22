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
            empty_scope_wait: Duration::from_secs(5),
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
        announce(&self.app, key);
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
    while !host.stopping() {
        host.wait_for_checks().await;
        if !host.ready() {
            tokio::time::sleep(timing.settings_check).await;
            continue;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel_on_drop = CancelCapture(cancelled.clone());
        let epoch = run_capture_epoch(&host, timing, cancelled.clone());
        tokio::pin!(epoch);
        let outcome = tokio::select! {
            outcome = &mut epoch => outcome,
            () = wait_until_paused(&*host, timing.settings_check) => {
                cancelled.store(true, Ordering::SeqCst);
                let _ = epoch.await;
                retry_count = 0;
                continue;
            }
        };
        match outcome {
            Ok(()) => retry_count = 0,
            Err(error) => {
                let status = capture_failure_status(&error);
                let store = host.store();
                match store
                    .opencode_v2_live_candidates(unix_now(), CANDIDATE_SESSION_LIMIT)
                    .and_then(|records| {
                        store.set_opencode_v2_source_status(&records, status, unix_now())
                    }) {
                    Ok(changed) if changed > 0 => host.report_changed(),
                    Err(_) => ::tracing::warn!(event = "opencode_v2_source_status_failed"),
                    _ => {}
                }
                ::tracing::warn!(
                    event = "opencode_v2_capture_stopped",
                    status = status.code(),
                    retry_count,
                );
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
) -> anyhow::Result<()> {
    let store = host.store();
    let records = store.opencode_v2_live_candidates(unix_now(), CANDIDATE_SESSION_LIMIT)?;
    if records.len() == CANDIDATE_SESSION_LIMIT {
        return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into());
    }
    if records.is_empty() {
        tokio::select! {
            () = tokio::time::sleep(timing.empty_scope_wait) => {},
            () = wait_until_paused(&**host, timing.settings_check) => {},
        }
        return Ok(());
    }
    let sources = host.locate_sources(&records, &cancelled).await?;
    ensure_capture_active(&cancelled)?;
    let registration_path = host.registration_path()?;
    let mut groups = select_bound_databases(
        &registration_path,
        sources,
        timing.connect_timeout,
        &cancelled,
    )
    .await?;
    ensure_capture_active(&cancelled)?;
    if groups.is_empty() {
        return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
    }
    let bound_records: Vec<_> = groups
        .iter()
        .flat_map(|(_, records)| records.iter().cloned())
        .collect();
    report_unbound_sources(&**host, &records, &bound_records)?;
    let mut records = bound_records;
    if records.len() > CAPTURE_SESSION_LIMIT {
        return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into());
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
    let detached = store.detached_opencode_v2_capture_claims(&records, unix_now())?;
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
    let targets = store.opencode_v2_reconciliation_targets(&records)?;
    let mut baseline_task =
        BaselineTask::start(host.clone(), groups.clone(), targets, cancelled.clone());
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
            capture.mark_session_gap(session_id);
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
        if store.set_opencode_v2_source_status(
            &invalid_records,
            OpenCodeV2SourceStatus::SourceMismatch,
            unix_now(),
        )? > 0
        {
            host.report_changed();
        }
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
    for claim in &lease.claims {
        ::tracing::debug!(
            event = "opencode_v2_capture_attached",
            session_id = %claim.key.session_id,
            capture_epoch = %claim.capture_epoch,
            baseline_sequence = ?claim.baseline_sequence,
            last_sequence = ?claim.last_sequence,
            history_gap = claim.history_gap,
        );
    }
    let claims_by_session = lease
        .claims
        .iter()
        .map(|claim| (claim.key.session_id.clone(), claim.clone()))
        .collect::<HashMap<_, _>>();
    let joiners: HashMap<String, OpenCodeV2EventJoiner> = session_ids
        .iter()
        .map(|session| (session.clone(), OpenCodeV2EventJoiner::new()))
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

    for event in buffered {
        database_identity.verify()?;
        database_identity.verify_session(store, &event.session_id)?;
        if active.gapped_sessions.contains(&event.session_id) {
            continue;
        }
        if let Some(key) = active.apply_captured(event, store)? {
            host.announce(&key);
        }
    }
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
                if opencode_live::read_registration(&registration_path)? != registration {
                    active.lease.mark_gap("service-registration-changed")?;
                    pump.stop().await;
                    return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
                }
                database_identity.verify()?;
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        active.lease.mark_gap("event-feed-error")?;
                        return Err(error).context("reading OpenCode V2 event feed");
                    }
                };
                database_identity.verify_event(store, &event)?;
                if let Some(key) = active.observe(event, store)? { host.announce(&key); }
                drain_scope_retirements(host, &database_identity, &mut pump, &mut active).await?;
            }
            _ = settings_check.tick() => {
                if cancelled.load(Ordering::SeqCst) || !host.ready() || host.stopping() {
                    active.lease.mark_gap("checks-disabled")?;
                    pump.stop().await;
                    return Ok(());
                }
            }
            _ = recheck.tick() => {
                if !active.lease.claims.is_empty() && !store.renew_opencode_v2_capture_epoch(&active.lease.claims, unix_now(), LEASE_SECS)? {
                    active.lease.mark_gap("capture-lease-lost")?;
                    pump.stop().await;
                    bail!("OpenCode V2 capture lease was lost");
                }
                let current =
                    store.opencode_v2_live_candidates(unix_now(), CANDIDATE_SESSION_LIMIT)?;
                if current.len() == CANDIDATE_SESSION_LIMIT {
                    active.lease.mark_gap("live-scope-overflow")?;
                    pump.stop().await;
                    return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into());
                }
                let current_sources = host.locate_sources(&current, &cancelled).await?;
                let current_groups = select_bound_databases(&registration_path, current_sources, timing.connect_timeout, &cancelled)
                    .await?;
                let current_bound_records: Vec<_> = current_groups
                    .iter()
                    .flat_map(|(_, records)| records.iter().cloned())
                    .collect();
                report_unbound_sources(&**host, &current, &current_bound_records)?;
                if current_bound_records.len() > CAPTURE_SESSION_LIMIT {
                    active.lease.mark_gap("live-scope-overflow")?;
                    pump.stop().await;
                    return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into());
                }
                database_identity.verify_records(&current_bound_records)?;
                let changed_sessions = changed_scope_sessions(&records, &current_bound_records);
                for session_id in &changed_sessions {
                    active.capture.mark_session_gap(session_id);
                    if let Some(claim) = active
                        .lease
                        .mark_session_gap(session_id, "source-scope-changed")?
                    {
                        active.gapped_claims.insert(session_id.clone(), claim);
                        active
                            .retirement_reasons
                            .insert(session_id.clone(), "source-scope-changed");
                    }
                    active.claims_by_session.remove(session_id);
                    active.joiners.remove(session_id);
                    active.gapped_sessions.insert(session_id.clone());
                }
                records.retain(|record| !changed_sessions.contains(&record.key.session_id));
                verify_bound_databases(&registration_path, &groups, &registration, timing.connect_timeout, &cancelled).await?;
                database_identity.verify()?;
                drain_scope_retirements(host, &database_identity, &mut pump, &mut active).await?;
                for record in &current_bound_records {
                    if !records.iter().any(|previous| previous.key == record.key) {
                        active.gapped_sessions.insert(record.key.session_id.clone());
                    }
                }
                if !active.gapped_sessions.is_empty() {
                    pump.include_sessions(&current_bound_records)?;
                    recover_gapped_sessions(host, store, &current_groups, &database_identity, &mut pump, &mut active).await?;
                    drain_scope_retirements(host, &database_identity, &mut pump, &mut active).await?;
                    for record in current_bound_records {
                        if active.claims_by_session.contains_key(&record.key.session_id)
                            && !records.iter().any(|previous| previous.key == record.key)
                        {
                            records.push(record);
                        }
                    }
                }
            }
        }
    }
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
    if !unbound.is_empty()
        && host.store().set_opencode_v2_source_status(
            &unbound,
            OpenCodeV2SourceStatus::SourceMismatch,
            unix_now(),
        )? > 0
    {
        host.report_changed();
    }
    Ok(())
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
    fn record_gaps(&mut self) -> anyhow::Result<()> {
        mark_gapped_sessions(
            &self.capture,
            &mut self.gapped_sessions,
            &mut self.lease,
            &mut self.claims_by_session,
            &mut self.joiners,
            &mut self.gapped_claims,
        )
    }

    fn observe(&mut self, event: Value, store: &Store) -> anyhow::Result<Option<SessionKey>> {
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
        let scope_loss = match event.event.get("type").and_then(Value::as_str) {
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
                                "source-limit"
                            } else {
                                "event-join-gap"
                            }
                        })
                });
                if let Some(claim) = self.lease.mark_session_gap(&session_id, reason)? {
                    self.gapped_claims.insert(session_id.clone(), claim);
                    if let Some(reason) = scope_loss {
                        self.retirement_reasons.insert(session_id.clone(), reason);
                    }
                }
                self.capture.mark_session_gap(&session_id);
                self.claims_by_session.remove(&session_id);
                self.joiners.remove(&session_id);
                self.gapped_sessions.insert(session_id);
                Ok(None)
            }
        }
    }
}

async fn recover_gapped_sessions(
    host: &Arc<dyn CaptureHost>,
    store: &Store,
    groups: &[DatabaseGroup],
    identity: &DatabaseIdentity,
    pump: &mut FeedPump,
    active: &mut ActiveCaptures<'_>,
) -> anyhow::Result<()> {
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
    let records: Vec<_> = groups
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
    let targets = store.opencode_v2_reconciliation_targets(&records)?;
    let mut baseline_task = BaselineTask::start(
        host.clone(),
        groups.clone(),
        targets,
        pump.cancelled.clone(),
    );
    let snapshot = loop {
        tokio::select! {
            result = &mut baseline_task.task => break result,
            event = pump.next() => {
                pump.verify_registration(&host.registration_path()?)?;
                identity.verify()?;
                let event = event.context("OpenCode V2 feed ended during session recovery")??;
                identity.verify_event(store, &event)?;
                if recovery.observe(event.clone()).is_err() {
                    return Ok(());
                }
                if let Some(key) = active.observe(event, store)? { host.announce(&key); }
            }
        }
    };
    ensure_capture_active(&pump.cancelled)?;
    pump.verify_registration(&host.registration_path()?)?;
    identity.verify()?;
    let snapshot = match snapshot {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(error)) => {
            if error.downcast_ref::<antiburn_local::analysis::OpenCodeV2SnapshotError>()
                == Some(&antiburn_local::analysis::OpenCodeV2SnapshotError::ScopeChanged)
            {
                let claims: Vec<_> = records
                    .iter()
                    .filter_map(|record| active.gapped_claims.get(&record.key.session_id).cloned())
                    .collect();
                store.mark_opencode_v2_capture_gap(&claims, "source-scope-changed", unix_now())?;
                active.retirement_reasons.extend(
                    claims
                        .iter()
                        .map(|claim| (claim.key.session_id.clone(), "source-scope-changed")),
                );
            }
            let status = capture_failure_status(&error);
            if store.set_opencode_v2_source_status(&records, status, unix_now())? > 0 {
                host.report_changed();
            }
            return Ok(());
        }
        Err(error) => return Err(error).context("reading the OpenCode V2 recovery snapshot"),
    };
    if !snapshot.invalid_scopes.is_empty() {
        let claims = records
            .iter()
            .filter(|record| snapshot.invalid_scopes.contains(&record.key.session_id))
            .filter_map(|record| active.gapped_claims.get(&record.key.session_id).cloned())
            .collect::<Vec<_>>();
        store.mark_opencode_v2_capture_gap(&claims, "source-scope-changed", unix_now())?;
        active.retirement_reasons.extend(
            claims
                .iter()
                .map(|claim| (claim.key.session_id.clone(), "source-scope-changed")),
        );
        let invalid_records = records
            .iter()
            .filter(|record| snapshot.invalid_scopes.contains(&record.key.session_id))
            .cloned()
            .collect::<Vec<_>>();
        if store.set_opencode_v2_source_status(
            &invalid_records,
            OpenCodeV2SourceStatus::SourceMismatch,
            unix_now(),
        )? > 0
        {
            host.report_changed();
        }
        return Ok(());
    }
    let baseline = &snapshot.baseline;
    let buffered = match recovery.finish_hydration(baseline.clone()) {
        Ok(buffered) if recovery.gapped_sessions().next().is_none() => buffered,
        _ => return Ok(()),
    };
    let starts = capture_starts(store, &records, baseline)?;
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
    if active.capture.resume_sessions(recovery).is_err() {
        store.mark_opencode_v2_capture_gap(&claims, "recovery-capacity", unix_now())?;
        return Ok(());
    }
    for claim in &claims {
        active.gapped_sessions.remove(&claim.key.session_id);
        active.gapped_claims.remove(&claim.key.session_id);
        active.retirement_reasons.remove(&claim.key.session_id);
        active
            .joiners
            .insert(claim.key.session_id.clone(), OpenCodeV2EventJoiner::new());
        active
            .claims_by_session
            .insert(claim.key.session_id.clone(), claim.clone());
    }
    active.lease.claims.extend(claims);
    active.lease.active = true;
    for event in buffered {
        identity.verify()?;
        identity.verify_session(store, &event.session_id)?;
        if active.gapped_sessions.contains(&event.session_id) {
            continue;
        }
        if let Some(key) = active.apply_captured(event, store)? {
            host.announce(&key);
        }
    }
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
    loop {
        ensure_capture_active(&pump.cancelled)?;
        match host
            .store()
            .prepare_opencode_v2_reconciliation(records, unix_now())?
        {
            OpenCodeV2ReconciliationPreparation::Ready => return Ok(true),
            OpenCodeV2ReconciliationPreparation::Fenced => return Ok(false),
            OpenCodeV2ReconciliationPreparation::Progress => {
                tokio::task::yield_now().await;
                ensure_capture_active(&pump.cancelled)?;
                drain_buffered_feed(host, identity, pump, |event| {
                    capture.observe(event.clone()).map_err(capture_gap_error)?;
                    if let Some(active) = active.as_deref_mut()
                        && let Some(key) = active.observe(event, host.store())?
                    {
                        host.announce(&key);
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
                    drain_buffered_feed(host, identity, pump, |event| {
                        capture.observe(event).map_err(capture_gap_error)?;
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
                }
                OpenCodeV2ReconciliationPreparation::Progress => {
                    tokio::task::yield_now().await;
                    ensure_capture_active(&pump.cancelled)?;
                    drain_buffered_feed(host, identity, pump, |event| {
                        if let Some(key) = active.observe(event, host.store())? {
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
    mut observe: impl FnMut(Value) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    for _ in 0..64 {
        ensure_capture_active(&pump.cancelled)?;
        let Some(event) = pump.try_next() else {
            break;
        };
        let event = event?;
        pump.verify_registration(&host.registration_path()?)?;
        identity.verify()?;
        identity.verify_event(host.store(), &event)?;
        observe(event)?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct AdmittedDatabase {
    path: PathBuf,
    identity: String,
}

impl AdmittedDatabase {
    fn from_record(record: &crate::store::SessionRecord) -> Option<Self> {
        if record.key.environment_key != "native"
            || record.key.agent != "opencode"
            || record.source_kind != "providerDb"
            || record.wsl_distro.is_some()
        {
            return None;
        }
        let cursor = record
            .activity_cursor
            .strip_prefix("opencode-admission-v2:")?;
        let (path, session_id, identity, scope): (PathBuf, String, String, String) =
            serde_json::from_str(cursor).ok()?;
        (path.is_absolute()
            && session_id == record.key.session_id
            && !identity.is_empty()
            && !scope.is_empty())
        .then_some(Self { path, identity })
    }
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
            let admitted = AdmittedDatabase::from_record(record)
                .ok_or(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch))?;
            let path = std::fs::canonicalize(admitted.path)
                .map_err(|_| CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch))?;
            if path != self.path || admitted.identity != self.identity {
                return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
            }
        }
        Ok(())
    }

    fn verify_session(&self, store: &Store, session_id: &str) -> anyhow::Result<()> {
        let record = store
            .session(&SessionKey::new("native", "opencode", session_id))?
            .ok_or(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch))?;
        self.verify_records(&[record])
    }

    fn verify_event(&self, store: &Store, event: &Value) -> anyhow::Result<()> {
        if let Some(session_id) = event
            .pointer("/durable/aggregateID")
            .and_then(Value::as_str)
        {
            self.verify_session(store, session_id)?;
        }
        Ok(())
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
        let Some(admitted) = AdmittedDatabase::from_record(record) else {
            continue;
        };
        let Some(identity) = identities.entry(admitted.path.clone()).or_insert_with(|| {
            DatabaseIdentity::read(&admitted.path)
                .map(|current| current.identity)
                .ok()
        }) else {
            continue;
        };
        if *identity != admitted.identity {
            continue;
        }
        sources.push((record.clone(), admitted.path));
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
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let read_cancelled = cancelled.clone();
        let task = tokio::task::spawn_blocking(move || {
            host.read_baselines(&groups, &targets, &|| {
                read_cancelled.load(Ordering::SeqCst) || capture_cancelled.load(Ordering::SeqCst)
            })
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
        return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into());
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

fn process_seed_event(
    event: CapturedEvent,
    joiners: &mut HashMap<String, OpenCodeV2EventJoiner>,
) -> ProcessResult {
    let Some(joiner) = joiners.get_mut(&event.session_id) else {
        return ProcessResult::SessionGap(event.session_id);
    };
    joiner.observe(&event.event);
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
    let result = joiner.observe(&event.event);
    if has_hard_join_gap(joiner) {
        return Ok(ProcessResult::SessionGap(event.session_id));
    }
    let Some(claim) = claims.get(&event.session_id) else {
        return Ok(ProcessResult::SessionGap(event.session_id));
    };
    // The event feed has no replay, and the baseline stores only sequence watermarks.
    let observation = result.map(|result| result.into_observation(false, true));
    let applied = store.advance_opencode_v2_capture(
        claim,
        event.sequence,
        observation.as_ref(),
        unix_now(),
        LEASE_SECS,
    )?;
    if !applied {
        return Ok(ProcessResult::SessionGap(event.session_id));
    }
    Ok(if observation.is_some() {
        ProcessResult::ResultPublished(claim.key.clone())
    } else {
        ProcessResult::Advanced
    })
}

fn mark_gapped_sessions(
    capture: &OpenCodeV2CaptureEpoch,
    reported: &mut HashSet<String>,
    lease: &mut CaptureLease<'_>,
    claims: &mut HashMap<String, OpenCodeV2CaptureClaim>,
    joiners: &mut HashMap<String, OpenCodeV2EventJoiner>,
    gapped_claims: &mut HashMap<String, OpenCodeV2CaptureClaim>,
) -> anyhow::Result<()> {
    let new_gaps: Vec<_> = capture
        .gapped_sessions()
        .filter(|session_id| !reported.contains(*session_id))
        .map(str::to_owned)
        .collect();
    for session_id in new_gaps {
        if let Some(claim) = lease.mark_session_gap(&session_id, "session-event-sequence-gap")? {
            gapped_claims.insert(session_id.clone(), claim);
        }
        claims.remove(&session_id);
        joiners.remove(&session_id);
        reported.insert(session_id);
    }
    Ok(())
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
        CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into()
    } else {
        anyhow::anyhow!("OpenCode V2 capture boundary failed: {gap:?}")
    }
}

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
                .is_none_or(|candidate| {
                    candidate.source_label != record.source_label
                        || candidate.cwd != record.cwd
                        || AdmittedDatabase::from_record(candidate)
                            != AdmittedDatabase::from_record(record)
                })
        })
        .map(|record| record.key.session_id.clone())
        .collect()
}

fn announce(app: &tauri::AppHandle, key: &SessionKey) {
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

struct FeedPump {
    receiver: mpsc::Receiver<anyhow::Result<Value>>,
    cancelled: Arc<AtomicBool>,
    task: JoinHandle<()>,
    overflowed: Arc<AtomicBool>,
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
        let overflowed = Arc::new(AtomicBool::new(false));
        let task_overflowed = overflowed.clone();
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
                        let session = event
                            .data
                            .pointer("/durable/aggregateID")
                            .and_then(Value::as_str)
                            .or_else(|| {
                                event
                                    .data
                                    .pointer("/data/sessionID")
                                    .and_then(Value::as_str)
                            });
                        let admitted = match task_admitted.read() {
                            Ok(admitted) => admitted,
                            Err(_) => break,
                        };
                        if !session.is_some_and(|session| admitted.contains(session)) {
                            continue;
                        }
                        drop(admitted);
                        match sender.try_send(Ok(event.data)) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                task_overflowed.store(true, Ordering::SeqCst);
                                break;
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => break,
                        }
                    }
                    Err(opencode_live::stream::EventStreamError::Cancelled) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error.into())).await;
                        break;
                    }
                }
            }
        });
        Self {
            receiver,
            cancelled,
            task,
            overflowed,
            admitted,
            registration,
        }
    }

    fn include_sessions(&self, records: &[crate::store::SessionRecord]) -> anyhow::Result<()> {
        let mut admitted = self
            .admitted
            .write()
            .map_err(|_| anyhow::anyhow!("OpenCode V2 admitted-session lock is poisoned"))?;
        let added = records
            .iter()
            .filter(|record| !admitted.contains(&record.key.session_id))
            .count();
        if admitted.len().saturating_add(added) > CAPTURE_SESSION_LIMIT {
            return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into());
        }
        admitted.extend(records.iter().map(|record| record.key.session_id.clone()));
        Ok(())
    }

    fn verify_registration(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if opencode_live::read_registration(path)? != self.registration {
            return Err(CaptureFailure(OpenCodeV2SourceStatus::SourceMismatch).into());
        }
        Ok(())
    }

    async fn next(&mut self) -> Option<anyhow::Result<Value>> {
        if self.overflowed.load(Ordering::SeqCst) {
            return Some(Err(
                CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into()
            ));
        }
        self.receiver.recv().await
    }

    fn try_next(&mut self) -> Option<anyhow::Result<Value>> {
        if self.overflowed.load(Ordering::SeqCst) {
            return Some(Err(
                CaptureFailure(OpenCodeV2SourceStatus::SourceLimit).into()
            ));
        }
        match self.receiver.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => Some(Err(
                opencode_live::stream::EventStreamError::StreamEnded.into(),
            )),
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
        for (sequence, event) in events.into_iter().enumerate() {
            terminal_outcome = process_captured_event(
                CapturedEvent {
                    session_id: record.key.session_id.clone(),
                    sequence: sequence as u64 + 1,
                    kind: BoundaryKind::Live,
                    event,
                },
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
