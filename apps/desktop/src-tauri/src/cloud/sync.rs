//! Session upload scheduling. One controller-owned scheduler shares bounded work
//! with auth and cleanup. Renderers supply explicit consent and read safe status;
//! they do not drive capture turns or own retries.

use super::{
    Controller, DestinationBinding, Folder, auth, capture, engine::Engine, ensure_cloud_setup, now,
    upload, work::Priority,
};
use crate::store::{
    Store,
    cloud_capture::{
        SourceKind, SourceSelection,
        sync::{Job, JobState, ONGOING_CONSENT_VERSION, Policy, ProjectSelection},
    },
};
use antiburn_cloud_capture::{
    Candidate, CaptureState, Limits, MetadataSections, QueueEntry, TransferProgress,
    Transport as _, discover_claude, discover_claude_page, discover_opencode,
    discover_opencode_page,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tokio::sync::Notify;

const RESERVATION_RETRY_DELAY: Duration = Duration::from_secs(15);

/// Explicit full session content confirmation for initial and future sessions.
/// Old source selections and upload preferences cannot deserialize as consent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Consent {
    version: String,
    account_id: String,
    destination_id: String,
    selected_sources: Vec<SourceSelection>,
    projects: Vec<ProjectSelection>,
    /// Full confirmed UI group snapshot. Only roots and projects grant admission.
    project_choices: BTreeMap<String, bool>,
    full_content: bool,
    ongoing: bool,
    attachments: bool,
}

/// One Settings change. Adding projects requires a new full session confirmation.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectChange {
    account_id: String,
    destination_id: String,
    group_id: String,
    selected: bool,
    projects: Vec<ProjectSelection>,
    folder_paths: Vec<String>,
    expected_policy_revision: String,
}

/// Closed upload outcomes. Byte progress alone never reports acceptance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    #[default]
    ConsentRequired,
    Ready,
    Syncing,
    Paused,
    Offline,
    ReauthRequired,
    UploadUnavailable,
    Fenced,
    Interrupted,
    StorageUnavailable,
}

/// Confirmed compressed-byte counters for the current account's saved entries.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Progress {
    negotiated: bool,
    total_compressed_bytes: u64,
    required_transfer_bytes: u64,
    reused_bytes: u64,
    confirmed_upload_bytes: u64,
}

impl Progress {
    fn add(&mut self, value: TransferProgress) {
        self.negotiated &= value.negotiated;
        self.total_compressed_bytes += value.total_compressed_bytes;
        self.required_transfer_bytes += value.required_transfer_bytes;
        self.reused_bytes += value.reused_bytes;
        self.confirmed_upload_bytes += value.confirmed_upload_bytes;
    }
}

/// Account-bound renderer state. It contains no capture IDs, payload, signed
/// URLs, service diagnostics, credentials, or authority to read content.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    status: State,
    /// Counts and progress are current only when the complete storage read succeeds.
    storage_known: bool,
    account_id: Option<String>,
    destination_id: Option<String>,
    consent_version: Option<String>,
    policy_revision: Option<String>,
    paused: bool,
    selected_sources: Vec<SourceSelection>,
    projects: Vec<ProjectSelection>,
    project_choices: BTreeMap<String, bool>,
    queued_count: u32,
    interrupted_count: u32,
    recovery_required_count: u32,
    fenced_count: u32,
    accepted_count: u32,
    suppressed_count: u32,
    progress: Progress,
}

/// Recorded project choices. Counts refer to native candidates, not acceptance.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectChoice {
    #[serde(flatten)]
    selection: ProjectSelection,
    session_count: u32,
}

/// One admitted candidate with only native recorded project evidence.
struct Selection {
    candidate: Candidate,
    scope: String,
    project: String,
    observation: String,
}

/// Memory-only scheduling hints. Durable policy, jobs, and spool entries remain
/// authoritative after restart. Each bounded discovery page stays until drained.
#[derive(Default)]
struct Runner {
    candidates: VecDeque<Selection>,
    next_reconcile: Option<Instant>,
    next_reservation: Option<Instant>,
    /// Bound retries of saved requests without delaying unrelated captures.
    retry_deadlines: BTreeMap<String, (Instant, upload::Failure)>,
    /// Replay submitted finalization once per sign-in, restart, or explicit retry.
    finalization_submissions: BTreeSet<String>,
    /// A new sign-in clears conflict cooldowns, not saved account or policy bindings.
    retry_grant_id: Option<String>,
    binding: Option<(String, String, Option<String>, String)>,
    discovery_cursor: Option<DiscoveryCursor>,
    cursor: Option<String>,
    discovery_blocked: bool,
    state: State,
    last_status: Option<Status>,
    last_status_origin: Option<String>,
}

/// Memory-only position in the current policy's source sweep. A restart repeats
/// the sweep; durable source keys and accepted state prevent duplicate jobs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DiscoveryCursor {
    root: usize,
    after: Option<String>,
}

/// Controller-owned wakeup and content fence. Pause invalidates sensitive work
/// before waiting for the shared blocking permit.
#[derive(Default)]
pub(super) struct Scheduler {
    wake: Arc<Notify>,
    pub generation: AtomicU64,
    runner: Mutex<Runner>,
}

/// Start once from Cloud initialization, regardless of renderer lifetime.
/// Only one blocking closure may run at a time. Busy turns wait for the next tick.
pub(super) fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let wake = app.state::<Controller>().sync.wake.clone();
            tokio::select! {
                _ = wake.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            }
            let work = app.state::<Controller>().blocking.clone();
            let worker_app = app.clone();
            let _ = work
                .run(Priority::Normal, move || {
                    let controller = worker_app.state::<Controller>();
                    let mut engine = controller
                        .engine
                        .lock()
                        .map_err(|_| "Cloud is unavailable.")?;
                    let mut runner = controller
                        .sync
                        .runner
                        .lock()
                        .map_err(|_| "Cloud sync is unavailable.")?;
                    let store = worker_app.state::<Store>();
                    let setup_failed = ensure_cloud_setup(&controller.setup_ready, &store).is_err();
                    let account_generation = controller.account_generation.load(Ordering::Acquire);
                    let content_generation = controller.sync.generation.load(Ordering::Acquire);
                    let engine_generation = engine.generation;
                    let current = || {
                        controller.account_generation.load(Ordering::Acquire) == account_generation
                            && controller.sync.generation.load(Ordering::Acquire)
                                == content_generation
                    };
                    let account_current = || {
                        engine_generation == account_generation
                            && controller.account_generation.load(Ordering::Acquire)
                                == account_generation
                    };
                    let mut host = controller
                        .capture
                        .lock()
                        .map_err(|_| "Session capture is unavailable.")?;
                    let mut project_cache = controller
                        .project_cache
                        .lock()
                        .map_err(|_| "Project discovery is unavailable.")?;
                    let failed = setup_failed
                        || runner
                            .turn(
                                &store,
                                &mut host,
                                &mut engine,
                                &current,
                                &account_current,
                                &mut project_cache,
                            )
                            .is_err();
                    if current() {
                        runner.report_status(
                            &store,
                            &mut host,
                            &engine,
                            failed,
                            &account_current,
                            |status| {
                                emit_status(&worker_app, status);
                            },
                        );
                        super::emit_snapshot(&worker_app, super::snapshot(&worker_app, &engine));
                    }
                    Ok(())
                })
                .await;
        }
    });
}

/// Notify only account surfaces. Other windows do not receive local project paths.
fn emit_status(app: &tauri::AppHandle, status: Status) -> Status {
    for window in ["onboarding", "main", "settings"] {
        let _ = app.emit_to(window, "cloud:sync-changed", status.clone());
    }
    status
}

impl Runner {
    /// Fill only an empty queue. Partial sweeps continue on the next available
    /// turn; completed sweeps use the normal refresh interval.
    fn refresh_candidates(
        &mut self,
        policy: &Policy,
        current: &dyn Fn() -> bool,
        cache: &mut super::projects::Cache,
    ) {
        if !current() {
            return;
        }
        let binding = (
            policy.account_id.clone(),
            policy.destination_id.clone(),
            policy.resource_origin.clone(),
            policy.revision.clone(),
        );
        if self.binding.as_ref() != Some(&binding) {
            self.candidates.clear();
            self.discovery_cursor = None;
            self.next_reconcile = None;
            self.next_reservation = None;
            self.binding = Some(binding);
        }
        if !self.candidates.is_empty()
            || self
                .next_reconcile
                .is_some_and(|time| time > Instant::now())
        {
            return;
        }
        if self.discovery_cursor.is_none() {
            self.discovery_blocked = false;
        }
        let cursor = self.discovery_cursor.clone().unwrap_or_default();
        match discover_policy_page(policy, current, cache, &cursor) {
            Ok((candidates, next_cursor, blocked)) => {
                self.candidates = candidates.into();
                self.discovery_cursor = next_cursor;
                self.discovery_blocked |= blocked;
                self.next_reconcile = self
                    .discovery_cursor
                    .is_none()
                    .then(|| Instant::now() + Duration::from_secs(60));
            }
            Err(_) => {
                self.discovery_blocked = true;
                self.next_reconcile = Some(Instant::now() + Duration::from_secs(60));
            }
        }
    }

    /// Publish a closed failure without rereading failed storage. Retained
    /// counters are last-known values for the same account and service only.
    /// `storage_known: false` makes their current values explicitly unknown.
    fn report_status(
        &mut self,
        store: &Store,
        host: &mut capture::Host,
        engine: &Engine,
        failed: bool,
        account_current: &dyn Fn() -> bool,
        publish: impl FnOnce(Status),
    ) -> Status {
        let read = if failed {
            Err("Capture storage is unavailable.".to_owned())
        } else if !account_current() {
            Ok(Status {
                status: State::ReauthRequired,
                ..Status::default()
            })
        } else {
            status(store, host, engine, self.state)
        };
        let mut result = match read {
            Ok(value) => {
                self.last_status = Some(value.clone());
                self.last_status_origin = engine.resource_origin().map(str::to_owned);
                value
            }
            Err(_) => {
                self.state = State::StorageUnavailable;
                let previous = self.last_status.as_ref().filter(|previous| {
                    account_current()
                        && self.last_status_origin.as_deref() == engine.resource_origin()
                        && engine.session().is_some_and(|session| {
                            previous.account_id.as_deref() == Some(&session.user.id)
                                && previous.destination_id.as_deref() == Some(&session.workspace.id)
                        })
                });
                let mut value = previous.cloned().unwrap_or_default();
                if account_current()
                    && let Some(session) = engine.session()
                {
                    value.account_id = Some(session.user.id.clone());
                    value.destination_id = Some(session.workspace.id.clone());
                }
                value.status = State::StorageUnavailable;
                value.storage_known = false;
                value
            }
        };
        if !account_current() {
            result = Status {
                status: if self.state == State::StorageUnavailable {
                    State::StorageUnavailable
                } else {
                    State::ReauthRequired
                },
                ..Status::default()
            };
        }
        publish(result.clone());
        result
    }

    /// Reconcile receipts locally even after logout. Network and source work need
    /// current personal authority and the separate ongoing content policy.
    fn turn(
        &mut self,
        store: &Store,
        host: &mut capture::Host,
        engine: &mut Engine,
        current: &dyn Fn() -> bool,
        account_current: &dyn Fn() -> bool,
        cache: &mut super::projects::Cache,
    ) -> Result<(), String> {
        cache.begin_scan();
        let entries = host
            .spool()?
            .entries()
            .map_err(|_| "Could not read saved captures.")?;
        for entry in entries
            .iter()
            .filter(|entry| entry.state == CaptureState::Accepted)
        {
            if let Some(job) = store
                .cloud_sync_job(&entry.identity.capture_id)
                .map_err(|_| "Could not read saved capture jobs.")?
                && job.identity == entry.identity
            {
                store
                    .accept_cloud_sync(entry)
                    .map_err(|_| "Could not save accepted capture state.")?;
                host.spool()?
                    .acknowledge_accepted(&entry.identity)
                    .map_err(|_| "Could not acknowledge accepted capture state.")?;
            }
        }
        if !account_current() || !current() {
            self.state = State::ReauthRequired;
            return Ok(());
        }
        if let Err(error) = engine.refresh(now(), account_current) {
            engine.status = error.into();
            self.state = match error {
                auth::Failure::Offline | auth::Failure::OfflineBeforeSend => State::Offline,
                _ => State::ReauthRequired,
            };
            return Ok(());
        }
        if !current() {
            self.state = State::Paused;
            return Ok(());
        }
        let Some(session) = engine.session() else {
            self.state = State::ReauthRequired;
            self.candidates.clear();
            self.discovery_cursor = None;
            self.next_reconcile = None;
            return Ok(());
        };
        let sign_in_changed = self.retry_grant_id.as_deref() != Some(&session.grant_id);
        if sign_in_changed {
            self.retry_grant_id = Some(session.grant_id.clone());
            self.retry_deadlines
                .retain(|_, (_, reason)| *reason != upload::Failure::Conflict);
            self.finalization_submissions.clear();
            tracing::info!(
                event = "cloud_sync_sign_in_changed",
                conflict_rechecks_cleared = true
            );
        }
        let Some(policy) = store
            .cloud_sync_policy(&session.user.id, &session.workspace.id, &session.resource)
            .map_err(|_| "Could not read Cloud sync consent.")?
        else {
            self.state = State::ConsentRequired;
            self.candidates.clear();
            self.discovery_cursor = None;
            self.next_reconcile = None;
            self.next_reservation = None;
            return Ok(());
        };
        if policy.paused || !current() {
            self.state = State::Paused;
            self.candidates.clear();
            self.discovery_cursor = None;
            self.next_reconcile = None;
            self.next_reservation = None;
            return Ok(());
        }
        let mut jobs = store
            .cloud_sync_jobs(
                &policy.account_id,
                &policy.destination_id,
                policy
                    .resource_origin
                    .as_deref()
                    .ok_or("Confirm Cloud sync for this service.")?,
            )
            .map_err(|_| "Could not read saved capture jobs.")?;
        restore_jobs(store, &mut jobs, &entries)?;
        for job in &mut jobs {
            if !current() {
                return Ok(());
            }
            let entry = entries.iter().find(|entry| entry.identity == job.identity);
            let next = if entry.is_none()
                && !matches!(
                    job.state,
                    JobState::Recovering | JobState::WaitingForLocalCapacity
                ) {
                JobState::Interrupted
            } else if sign_in_changed
                && job.state == JobState::Interrupted
                && entry.is_some_and(|entry| {
                    matches!(
                        entry.state,
                        CaptureState::Queued | CaptureState::AwaitingIntegrity
                    )
                })
                && job.allowed(&policy)
            {
                JobState::Queued
            } else {
                continue;
            };
            if next != job.state {
                store
                    .set_cloud_sync_job_state(job, next)
                    .map_err(|_| "Could not save capture recovery state.")?;
                job.state = next;
            }
        }
        self.retry_deadlines
            .retain(|id, _| jobs.iter().any(|job| &job.identity.capture_id == id));
        self.finalization_submissions
            .retain(|id| jobs.iter().any(|job| &job.identity.capture_id == id));
        let pending = entries
            .iter()
            .filter(|entry| {
                entry.identity.account_id == policy.account_id
                    && entry.identity.destination_id == policy.destination_id
                    && matches!(
                        entry.state,
                        CaptureState::Queued | CaptureState::AwaitingIntegrity
                    )
                    && jobs.iter().any(|job| {
                        !matches!(job.state, JobState::Recovering | JobState::Interrupted)
                            && self
                                .retry_deadlines
                                .get(&job.identity.capture_id)
                                .is_none_or(|(deadline, _)| *deadline <= Instant::now())
                            && job.identity == entry.identity
                            && job.allowed(&policy)
                    })
            })
            .collect::<Vec<_>>();
        if !pending.is_empty() {
            let entry = pending
                .iter()
                .copied()
                .find(|entry| {
                    self.cursor
                        .as_ref()
                        .is_none_or(|cursor| entry.identity.capture_id > *cursor)
                })
                .unwrap_or(pending[0]);
            self.cursor = Some(entry.identity.capture_id.clone());
            let Some(job) = jobs.iter().find(|job| job.identity == entry.identity) else {
                return Err("Saved capture binding is unavailable.".into());
            };
            let origin = job
                .resource_origin
                .as_deref()
                .ok_or("Confirm Cloud sync for this service.")?;
            if job.state == JobState::Fenced {
                self.retry_deadlines.insert(
                    job.identity.capture_id.clone(),
                    (
                        Instant::now() + Duration::from_secs(60),
                        upload::Failure::Conflict,
                    ),
                );
            }
            let mut transport =
                upload::HttpTransport::new(engine, account_current, current, origin, None);
            let started = Instant::now();
            let mut result = host.spool()?.resume_step(&entry.identity, &mut transport);
            if let Ok(step) = &result
                && step.entry.state == CaptureState::AwaitingIntegrity
                && step.entry.completion_submitted
                && entry.completion_submitted
                && self
                    .finalization_submissions
                    .insert(entry.identity.capture_id.clone())
            {
                let saved = &step.entry;
                let replay = match (&saved.reservation_id, &saved.manifest, &saved.upload_plan) {
                    (Some(reservation), Some(manifest), Some(plan)) => transport
                        .complete(reservation, manifest, plan)
                        .map_err(antiburn_cloud_capture::Error::Transport),
                    _ => Err(antiburn_cloud_capture::Error::CorruptSpool),
                };
                tracing::info!(
                    event = "cloud_saved_finalization_replayed",
                    succeeded = replay.is_ok(),
                );
                if let Err(error) = replay {
                    self.finalization_submissions
                        .remove(&entry.identity.capture_id);
                    result = Err(error);
                }
            }
            if result
                .as_ref()
                .is_ok_and(|step| step.entry.completion_submitted)
            {
                self.finalization_submissions
                    .insert(entry.identity.capture_id.clone());
            }
            self.state = transport_state(transport.failure, &result);
            if result.is_ok() && job.state == JobState::Fenced && current() {
                store
                    .set_cloud_sync_job_state(job, JobState::Queued)
                    .map_err(|_| "Could not save recovered capture progress.")?;
                self.retry_deadlines.remove(&job.identity.capture_id);
            }
            if result.is_err()
                && self.state == State::Fenced
                && let Some(job) = jobs.iter().find(|job| job.identity == entry.identity)
            {
                store
                    .set_cloud_sync_job_state(job, JobState::Fenced)
                    .map_err(|_| "Could not save fenced capture state.")?;
                self.retry_deadlines.insert(
                    job.identity.capture_id.clone(),
                    (
                        Instant::now() + Duration::from_secs(60),
                        upload::Failure::Conflict,
                    ),
                );
            }
            if result.is_err() && matches!(self.state, State::Offline | State::Syncing) {
                let delay = transport.retry_after.unwrap_or(RESERVATION_RETRY_DELAY);
                self.retry_deadlines.insert(
                    job.identity.capture_id.clone(),
                    (
                        Instant::now() + delay,
                        transport.failure.unwrap_or(upload::Failure::Offline),
                    ),
                );
                if transport.failure == Some(upload::Failure::Backpressure) {
                    self.next_reservation = Some(Instant::now() + delay);
                }
            } else if result.is_err()
                && matches!(self.state, State::Interrupted | State::UploadUnavailable)
            {
                store
                    .set_cloud_sync_job_state(job, JobState::Interrupted)
                    .map_err(|_| "Could not save interrupted capture state.")?;
            }
            if job.state == JobState::Fenced || result.is_err() {
                tracing::info!(
                    event = "cloud_saved_upload_resume",
                    capture_state = ?entry.state,
                    job_state = ?job.state,
                    upload_state = ?self.state,
                    has_plan = entry.upload_plan.is_some(),
                    succeeded = result.is_ok(),
                    transport_failure = ?transport.failure,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    retry_after_seconds = self.retry_deadlines
                        .get(&job.identity.capture_id)
                        .map(|(deadline, _)| deadline.saturating_duration_since(Instant::now()).as_secs())
                        .unwrap_or_default(),
                );
            }
            if let Ok(step) = result
                && step.entry.state == CaptureState::Accepted
            {
                store
                    .accept_cloud_sync(&step.entry)
                    .map_err(|_| "Could not save accepted capture state.")?;
                host.spool()?
                    .acknowledge_accepted(&step.entry.identity)
                    .map_err(|_| "Could not acknowledge accepted capture state.")?;
            }
            return Ok(());
        }
        self.refresh_candidates(&policy, current, cache);
        if self
            .next_reservation
            .is_some_and(|time| time > Instant::now())
        {
            if self.state != State::Offline {
                self.state = State::Syncing;
            }
            return Ok(());
        }
        self.next_reservation = None;
        for job in &mut jobs {
            if !job.allowed(&policy) && job.state != JobState::Fenced {
                store
                    .set_cloud_sync_job_state(job, JobState::Fenced)
                    .map_err(|_| "Could not save fenced capture state.")?;
                job.state = JobState::Fenced;
                job.reservation_retry_at = None;
            }
        }
        // Reopen one due job directly. A filtered or incomplete discovery page
        // cannot prove that its saved source has moved or disappeared.
        if let Some(job) = jobs.iter().find(|job| {
            matches!(
                job.state,
                JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
            ) && job
                .reservation_retry_at
                .is_some_and(|deadline| deadline <= now())
        }) {
            if !current() {
                return Ok(());
            }
            let candidate = reopen_waiting_source(&policy, job);
            if !current() {
                return Ok(());
            }
            match candidate {
                Ok(candidate) => {
                    self.candidates.retain(|selection| {
                        selection.candidate.source_id() != candidate.source_id()
                    });
                    self.candidates.push_front(Selection {
                        candidate,
                        scope: job.source_scope_id.clone(),
                        project: job.project_path.clone(),
                        observation: job.observation_key.clone(),
                    });
                }
                Err(_) => {
                    store
                        .set_cloud_sync_job_state(job, JobState::Interrupted)
                        .map_err(|_| "Could not save the changed capture source state.")?;
                    self.state = State::Interrupted;
                    return Ok(());
                }
            }
        }
        while let Some(selection) = self.candidates.pop_front() {
            if !current() {
                return Ok(());
            }
            let saved = jobs
                .iter()
                .find(|job| job.identity.source_id == selection.candidate.source_id());
            if saved.is_some_and(|job| {
                !job.allowed(&policy)
                    || !matches!(
                        job.state,
                        JobState::Pending
                            | JobState::WaitingForCapacity
                            | JobState::WaitingForLocalCapacity
                    )
            }) {
                continue;
            }
            if let Some(job) = saved
                && let Some(retry_at) = job.reservation_retry_at
                && retry_at > now()
            {
                let seconds = retry_at
                    .saturating_sub(now())
                    .unsigned_abs()
                    .min(RESERVATION_RETRY_DELAY.as_secs());
                self.next_reservation = Some(Instant::now() + Duration::from_secs(seconds));
                self.candidates.push_front(selection);
                self.state = State::Syncing;
                return Ok(());
            }
            if store
                .cloud_sync_source(
                    &policy.account_id,
                    &policy.destination_id,
                    policy
                        .resource_origin
                        .as_deref()
                        .ok_or("Confirm Cloud sync for this service.")?,
                    selection.candidate.source_id(),
                )
                .map_err(|_| "Could not read accepted source state.")?
                .is_some_and(|source| {
                    source.suppressed
                        || (saved.is_none_or(|job| {
                            !matches!(
                                job.state,
                                JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
                            )
                        }) && source.observation_key.as_deref() == Some(&selection.observation))
                })
            {
                continue;
            }
            let limits = match upload::capabilities(
                engine,
                &policy.account_id,
                &policy.destination_id,
                account_current,
                current,
            ) {
                Ok(limits) => limits,
                Err(failure) => {
                    self.state = match failure.failure {
                        upload::Failure::Backpressure => {
                            self.next_reservation = Some(
                                Instant::now()
                                    + failure.retry_after.unwrap_or(RESERVATION_RETRY_DELAY),
                            );
                            State::Syncing
                        }
                        upload::Failure::Offline => {
                            self.next_reservation = Some(
                                Instant::now()
                                    + failure.retry_after.unwrap_or(RESERVATION_RETRY_DELAY),
                            );
                            State::Offline
                        }
                        upload::Failure::Unauthorized => State::ReauthRequired,
                        _ => State::UploadUnavailable,
                    };
                    self.candidates.push_front(selection);
                    return Ok(());
                }
            };
            host.configure_limits(limits.clone())?;
            let waiting = saved.filter(|job| {
                matches!(
                    job.state,
                    JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
                )
            });
            let folders = if let Some(job) = waiting {
                job.project_folders
                    .iter()
                    .map(Path::new)
                    .map(Path::to_owned)
                    .collect()
            } else {
                policy
                    .projects
                    .iter()
                    .filter(|project| {
                        Path::new(&selection.project).starts_with(&project.project_path)
                            && policy.roots.iter().any(|root| {
                                root.source_scope_id == selection.scope
                                    && root.source_kind == project.source_kind
                                    && root.path == project.source_path
                            })
                    })
                    .map(|project| Path::new(&project.project_path).to_owned())
                    .collect()
            };
            let project_selection = antiburn_cloud_capture::ProjectSelection::new(folders);
            let projects = project_selection
                .as_ref()
                .map_err(|_| ())
                .and_then(|projects| {
                    projects
                        .admit_with_limits(&selection.candidate, &limits)
                        .map_err(|_| ())
                });
            if !current() {
                self.state = State::Paused;
                return Ok(());
            }
            let candidate = match (projects, waiting) {
                (Ok(Some(candidate)), _) => candidate,
                (_, Some(job)) => {
                    store
                        .set_cloud_sync_job_state(job, JobState::Interrupted)
                        .map_err(|_| "Could not save the changed capture project state.")?;
                    self.state = State::Interrupted;
                    return Ok(());
                }
                (Ok(None), None) => {
                    self.state = State::Fenced;
                    continue;
                }
                (Err(_), None) => {
                    return Err("Project admission failed. No session content was uploaded.".into());
                }
            };
            if !current() {
                self.candidates.push_front(selection);
                self.state = State::Paused;
                return Ok(());
            }
            let mut job = if let Some(job) = saved {
                job.clone()
            } else {
                let Some(job) = store
                    .enqueue_cloud_sync(
                        &policy,
                        &selection.scope,
                        &selection.project,
                        selection.candidate.source_id(),
                        &selection.observation,
                    )
                    .map_err(|_| "Could not save a capture job. Saved bytes are kept.")?
                else {
                    continue;
                };
                job
            };
            let retry_reservation = job.state == JobState::WaitingForCapacity;
            if matches!(
                job.state,
                JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
            ) {
                let root = policy
                    .roots
                    .iter()
                    .find(|root| root.source_scope_id == selection.scope)
                    .ok_or("The capture source changed. Saved state is kept.")?;
                let source = SourceSelection {
                    source_kind: root.source_kind,
                    path: root.path.clone(),
                };
                let observation = cache
                    .evidence(
                        &policy.account_id,
                        &policy.destination_id,
                        &source,
                        &selection.candidate,
                        &limits,
                    )
                    .and_then(|evidence| {
                        evidence
                            .map(|evidence| {
                                if !current() {
                                    return Err("Cloud upload controls changed.".into());
                                }
                                candidate_observation(
                                    &selection.candidate,
                                    &evidence,
                                    project_selection
                                        .as_ref()
                                        .map_err(|_| "Cloud project selection changed.")?,
                                )
                            })
                            .transpose()
                    });
                if !current() {
                    self.state = State::Paused;
                    return Ok(());
                }
                if matches!(observation, Ok(None)) {
                    self.candidates.push_front(selection);
                    self.state = State::Syncing;
                    return Ok(());
                };
                if job.identity.source_id != candidate.source_id()
                    || observation.ok().flatten().as_deref() != Some(&job.observation_key)
                {
                    store
                        .set_cloud_sync_job_state(&job, JobState::Interrupted)
                        .map_err(|_| "Could not save the changed capture source state.")?;
                    self.state = State::Interrupted;
                    return Ok(());
                }
                // Flush away capacity-only retry permission before reserve can run.
                store
                    .set_cloud_sync_job_state(&job, JobState::Pending)
                    .map_err(|_| "Could not save reservation retry intent.")?;
                job.state = JobState::Pending;
                job.reservation_retry_at = None;
            }
            if !current() {
                self.candidates.push_front(selection);
                self.state = State::Paused;
                return Ok(());
            }
            let source = upload::Source::from_candidate(&candidate);
            let mut transport = upload::HttpTransport::new(
                engine,
                account_current,
                current,
                job.resource_origin
                    .as_deref()
                    .ok_or("Confirm Cloud sync for this service.")?,
                Some(source),
            );
            let result = if retry_reservation {
                host.spool()?.retry_reservation(
                    job.identity.clone(),
                    &candidate,
                    MetadataSections::new(),
                    &mut transport,
                )
            } else {
                host.spool()?.capture(
                    job.identity.clone(),
                    &candidate,
                    MetadataSections::new(),
                    &mut transport,
                )
            };
            if transport.failure == Some(upload::Failure::SourceSuppressed) {
                store.suppress_cloud_sync(&job).map_err(
                    |_| "Could not save the Cloud deletion outcome. Saved captures are kept.",
                )?;
                host.spool()?.cancel(&job.identity).map_err(
                    |_| "Cloud deletion is saved, but its empty local entry needs cleanup.",
                )?;
                self.state = State::Ready;
                return Ok(());
            }
            self.state = transport_state(transport.failure, &result);
            let capacity_refused = matches!(
                result,
                Err(antiburn_cloud_capture::Error::Transport(
                    antiburn_cloud_capture::TransportError::Backpressure
                ))
            );
            let saved_entries = host
                .spool()?
                .entries()
                .map_err(|_| "Could not read interrupted capture state.")?;
            let state = match saved_entries
                .iter()
                .find(|entry| entry.identity == job.identity)
            {
                Some(entry)
                    if matches!(
                        entry.state,
                        CaptureState::Queued | CaptureState::AwaitingIntegrity
                    ) =>
                {
                    JobState::Queued
                }
                Some(entry)
                    if capacity_refused
                        && entry.state == CaptureState::Preparing
                        && entry.reservation_id.is_none()
                        && entry.manifest.is_none()
                        && entry.upload_plan.is_none()
                        && entry.server_status.is_none()
                        && !entry.completion_submitted =>
                {
                    JobState::WaitingForCapacity
                }
                Some(_) => JobState::Interrupted,
                None if matches!(result, Err(antiburn_cloud_capture::Error::SpoolCapacity)) => {
                    JobState::WaitingForLocalCapacity
                }
                None => JobState::Interrupted,
            };
            if state == JobState::WaitingForCapacity {
                let delay = transport.retry_after.unwrap_or(RESERVATION_RETRY_DELAY);
                self.next_reservation = Some(Instant::now() + delay);
                self.candidates.push_front(selection);
                // Round up persisted seconds so a restart cannot shorten the delay.
                let retry_at = now().saturating_add(delay.as_secs() as i64 + 1);
                store
                    .defer_cloud_sync_reservation(&job, retry_at)
                    .map_err(|_| "Could not save reservation backpressure.")?;
            } else if state == JobState::WaitingForLocalCapacity {
                let delay = RESERVATION_RETRY_DELAY;
                self.next_reservation = Some(Instant::now() + delay);
                self.candidates.push_front(selection);
                store
                    .defer_cloud_sync_local_capture(
                        &job,
                        now().saturating_add(delay.as_secs() as i64 + 1),
                    )
                    .map_err(|_| "Could not save local capture backpressure.")?;
                self.state = State::Syncing;
            } else {
                store
                    .set_cloud_sync_job_state(&job, state)
                    .map_err(|_| "Could not save capture progress.")?;
            }
            if result.is_err()
                && transport.failure.is_none()
                && state != JobState::WaitingForLocalCapacity
            {
                self.state = State::Interrupted;
            }
            return Ok(());
        }
        self.state = if jobs.iter().any(|job| {
            job.allowed(&policy)
                && matches!(job.state, JobState::Interrupted | JobState::Recovering)
        }) {
            State::Interrupted
        } else if self.discovery_blocked
            || jobs
                .iter()
                .any(|job| job.allowed(&policy) && job.state == JobState::Fenced)
        {
            State::Fenced
        } else if jobs.iter().any(|job| {
            job.allowed(&policy)
                && matches!(
                    job.state,
                    JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
                )
        }) {
            State::Syncing
        } else {
            State::Ready
        };
        Ok(())
    }
}

/// Reopen a saved locator only under its current approved source root.
fn reopen_waiting_source(policy: &Policy, job: &Job) -> antiburn_cloud_capture::Result<Candidate> {
    use antiburn_cloud_capture::{Error, reopen_claude, reopen_opencode};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    let root = policy
        .roots
        .iter()
        .find(|root| root.source_scope_id == job.source_scope_id)
        .ok_or(Error::InvalidIdentity)?;
    let (scope, key) = job
        .identity
        .source_id
        .split_once('/')
        .ok_or(Error::InvalidIdentity)?;
    if scope != root.source_scope_id {
        return Err(Error::InvalidIdentity);
    }
    let path = Path::new(&root.path);
    let limits = metadata_limits();
    let candidate = match root.source_kind {
        SourceKind::ClaudeJsonl => {
            let (session, agent) = match key.split_once("/subagents/agent-") {
                Some((session, agent)) => (session, Some(agent)),
                None => (key, None),
            };
            reopen_claude(path, session, agent, &limits)?
        }
        SourceKind::OpenCodeSqliteV2 => {
            let (_, encoded_id) = key
                .strip_prefix("opencode/")
                .and_then(|key| key.split_once('/'))
                .ok_or(Error::InvalidIdentity)?;
            let id = URL_SAFE_NO_PAD
                .decode(encoded_id)
                .map_err(|_| Error::InvalidIdentity)?;
            let id = std::str::from_utf8(&id).map_err(|_| Error::InvalidIdentity)?;
            reopen_opencode(path, id, &limits)?
        }
    }
    .in_source_scope(scope)?;
    if candidate.source_id() != job.identity.source_id {
        return Err(Error::SourceChanged);
    }
    Ok(candidate)
}

/// Recover a crash between spool publication and host progress. A preparing
/// entry remains interrupted; repeating `capture` cannot resume its native reads.
fn restore_jobs(store: &Store, jobs: &mut [Job], entries: &[QueueEntry]) -> Result<(), String> {
    for job in jobs {
        if matches!(job.state, JobState::Fenced | JobState::Recovering) {
            continue;
        }
        if let Some(entry) = entries.iter().find(|entry| entry.identity == job.identity) {
            let state = match entry.state {
                CaptureState::Preparing if job.state == JobState::WaitingForCapacity => {
                    JobState::WaitingForCapacity
                }
                CaptureState::Preparing => JobState::Interrupted,
                CaptureState::Queued | CaptureState::AwaitingIntegrity
                    if job.state == JobState::Interrupted =>
                {
                    continue;
                }
                CaptureState::Queued | CaptureState::AwaitingIntegrity => JobState::Queued,
                CaptureState::Accepted | CaptureState::Cancelled => continue,
            };
            if state != job.state {
                store
                    .set_cloud_sync_job_state(job, state)
                    .map_err(|_| "Could not restore saved capture progress.")?;
                job.state = state;
                if state != JobState::WaitingForCapacity {
                    job.reservation_retry_at = None;
                }
            }
        }
    }
    Ok(())
}

/// Apply an explicit Retry without changing saved upload bytes. A record without
/// a reservation is before payload capture. Keep every later interrupted record.
/// Recovery intent survives a crash between spool cancellation and the terminal
/// host write; the old capture ID can never be reused.
fn retry_saved_jobs(
    store: &Store,
    spool: &mut antiburn_cloud_capture::Spool,
    policy: &Policy,
    current: &dyn Fn() -> bool,
) -> Result<(), String> {
    let origin = policy
        .resource_origin
        .as_deref()
        .ok_or("Confirm Cloud sync for this service.")?;
    let entries = spool
        .entries()
        .map_err(|_| "Could not read saved captures.")?;
    for mut job in store
        .cloud_sync_jobs(&policy.account_id, &policy.destination_id, origin)
        .map_err(|_| "Could not read saved capture jobs.")?
    {
        if !current() {
            return Err("Cloud upload controls changed. Saved captures are kept.".into());
        }
        if !job.allowed(policy) {
            continue;
        }
        let entry = entries.iter().find(|entry| entry.identity == job.identity);
        if entry.is_none() && job.state != JobState::Recovering {
            continue;
        }
        let empty_preparing = entry.is_some_and(empty_preparation);
        if matches!(
            job.state,
            JobState::Interrupted | JobState::Pending | JobState::Recovering | JobState::Fenced
        ) && (empty_preparing || (job.state == JobState::Recovering && entry.is_none()))
        {
            if job.state != JobState::Recovering {
                store
                    .set_cloud_sync_job_state(&job, JobState::Recovering)
                    .map_err(|_| "Could not save capture recovery intent.")?;
                job.state = JobState::Recovering;
            }
            if entry.is_some() {
                spool.cancel(&job.identity).map_err(
                    |_| "Could not finish empty capture recovery. Saved captures are kept.",
                )?;
            }
            store
                .abandon_cloud_sync_preparing(&job)
                .map_err(|_| "Could not finish capture recovery. Retry again.")?;
        } else if matches!(job.state, JobState::Fenced | JobState::Interrupted)
            && entry.is_some_and(|entry| {
                matches!(
                    entry.state,
                    CaptureState::Queued | CaptureState::AwaitingIntegrity
                )
            })
        {
            store
                .set_cloud_sync_job_state(&job, JobState::Queued)
                .map_err(|_| "Could not retry saved capture state.")?;
        }
    }
    Ok(())
}

/// Only this saved shape permits explicit retry to replace an empty preparation.
/// It never authorizes automatic rereading of source.
fn empty_preparation(entry: &QueueEntry) -> bool {
    matches!(
        entry.state,
        CaptureState::Preparing | CaptureState::Cancelled
    ) && entry.reservation_id.is_none()
        && entry.manifest.is_none()
        && entry.upload_plan.is_none()
        && entry.server_status.is_none()
        && !entry.completion_submitted
}

fn transport_state<T>(
    failure: Option<upload::Failure>,
    result: &Result<T, antiburn_cloud_capture::Error>,
) -> State {
    let failure = failure.or(match result {
        Err(antiburn_cloud_capture::Error::Transport(error)) => Some(match error {
            antiburn_cloud_capture::TransportError::Backpressure => upload::Failure::Backpressure,
            antiburn_cloud_capture::TransportError::Offline => upload::Failure::Offline,
            antiburn_cloud_capture::TransportError::Unauthorized => upload::Failure::Unauthorized,
            antiburn_cloud_capture::TransportError::Conflict => upload::Failure::Conflict,
            antiburn_cloud_capture::TransportError::Rejected => upload::Failure::Rejected,
        }),
        _ => None,
    });
    match failure {
        Some(upload::Failure::Backpressure) => State::Syncing,
        Some(upload::Failure::Offline) => State::Offline,
        Some(upload::Failure::Unauthorized) => State::ReauthRequired,
        Some(upload::Failure::Unavailable) => State::UploadUnavailable,
        Some(upload::Failure::Conflict) => State::Fenced,
        Some(upload::Failure::Rejected) => State::Interrupted,
        Some(upload::Failure::SourceSuppressed) => State::Ready,
        None if result.is_ok() => State::Syncing,
        None => State::Interrupted,
    }
}

/// Inspect only admitted native source candidates. Project matching uses directory components;
/// paths in tool calls, attachments, and transcript text cannot grant access.
#[cfg(test)]
fn discover_policy(policy: &Policy, current: &dyn Fn() -> bool) -> Result<Vec<Selection>, String> {
    if !current() {
        return Ok(Vec::new());
    }
    discover_policy_page(
        policy,
        current,
        &mut super::projects::Cache::default(),
        &DiscoveryCursor::default(),
    )
    .map(|(candidates, _, _)| candidates)
}

pub(super) fn metadata_limits() -> Limits {
    Limits {
        source_bytes: 128 * 1024 * 1024,
        ..Limits::default()
    }
}

/// Inspect at most 512 candidates across the remaining roots in one sweep page.
/// Keep bad sessions visible without discarding healthy selections or the cursor.
fn discover_policy_page(
    policy: &Policy,
    current: &dyn Fn() -> bool,
    cache: &mut super::projects::Cache,
    cursor: &DiscoveryCursor,
) -> Result<(Vec<Selection>, Option<DiscoveryCursor>, bool), String> {
    cache.begin_scan();
    let mut selected = Vec::new();
    let mut discovered = 0usize;
    let mut blocked = false;
    for (index, root) in policy.roots.iter().enumerate().skip(cursor.root) {
        if !current() {
            return Err("Cloud account changed.".into());
        }
        let (candidates, next_cursor) = discover_root_page(
            root.source_kind,
            &root.path,
            (index == cursor.root)
                .then_some(cursor.after.as_deref())
                .flatten(),
            512usize.saturating_sub(discovered),
        )?;
        discovered += candidates.len();
        let projects = antiburn_cloud_capture::ProjectSelection::new(
            policy
                .projects
                .iter()
                .filter(|project| {
                    project.source_kind == root.source_kind && project.source_path == root.path
                })
                .map(|project| Path::new(&project.project_path).to_owned())
                .collect(),
        )
        .map_err(|_| "A selected project directory is unavailable. Saved captures are kept.")?;
        for candidate in candidates {
            if !current() {
                return Err("Cloud account changed.".into());
            }
            let root_selection = SourceSelection {
                source_kind: root.source_kind,
                path: root.path.clone(),
            };
            let evidence = cache.evidence(
                &policy.account_id,
                &policy.destination_id,
                &root_selection,
                &candidate,
                &metadata_limits(),
            );
            let evidence = match evidence {
                Ok(Some(evidence)) => evidence,
                Ok(None) => continue,
                Err(_) => {
                    blocked = true;
                    continue;
                }
            };
            match projects.matches(&evidence) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    blocked = true;
                    continue;
                }
            }
            let project = evidence
                .project_paths()
                .iter()
                .find(|path| {
                    path.to_str()
                        .is_some_and(|path| policy.allows(&root.source_scope_id, path))
                })
                .map(|path| path.as_path())
                .and_then(Path::to_str)
                .ok_or("Could not bind the recorded project.")?
                .to_owned();
            if !current() {
                return Err("Cloud upload controls changed.".into());
            }
            let observation = match candidate_observation(&candidate, &evidence, &projects) {
                Ok(observation) => observation,
                Err(_) => {
                    blocked = true;
                    continue;
                }
            };
            if policy.allows(&root.source_scope_id, &project) {
                selected.push(Selection {
                    candidate: candidate
                        .in_source_scope(&root.source_scope_id)
                        .map_err(|_| "Could not bind a session source.")?,
                    scope: root.source_scope_id.clone(),
                    project,
                    observation,
                });
            }
        }
        if !current() {
            return Err("Cloud account changed.".into());
        }
        if let Some(after) = next_cursor {
            return Ok((
                selected,
                Some(DiscoveryCursor {
                    root: index,
                    after: Some(after),
                }),
                blocked,
            ));
        }
        if discovered == 512 && index + 1 < policy.roots.len() {
            return Ok((
                selected,
                Some(DiscoveryCursor {
                    root: index + 1,
                    after: None,
                }),
                blocked,
            ));
        }
    }
    Ok((selected, None, blocked))
}

/// Compare a fresh source generation and bounded metadata digest with the fixed
/// job hint. This hint never replaces full project admission or capture checks.
fn candidate_observation(
    candidate: &Candidate,
    evidence: &antiburn_cloud_capture::ProjectEvidence,
    projects: &antiburn_cloud_capture::ProjectSelection,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    if candidate.source_kind() == antiburn_cloud_capture::CaptureSourceKind::OpenCodeSqliteV2 {
        return candidate
            .opencode_content_digest(&metadata_limits(), projects)
            .map(|digest| format!("opencode-native-v1:{digest}"))
            .map_err(|_| "Could not check the approved session's native content.".into());
    }
    let observation = source_observation(candidate)?;
    let bytes = serde_json::to_vec(&(observation, evidence.source_digest()))
        .map_err(|_| "Could not record a source observation.")?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Read one bounded provider page using stable native keys and fresh source checks.
pub(super) fn discover_root_page(
    kind: SourceKind,
    path: &str,
    after: Option<&str>,
    remaining: usize,
) -> Result<(Vec<Candidate>, Option<String>), String> {
    if remaining == 0 {
        return Err("Session discovery is still loading.".into());
    }
    match kind {
        SourceKind::ClaudeJsonl => discover_claude_page(
            Path::new(path),
            after,
            &Limits {
                discovery_sessions: remaining,
                ..Limits::default()
            },
        )
        .map(|page| (page.candidates, page.next_cursor))
        .map_err(|_| "Could not list the selected Claude source.".into()),
        SourceKind::OpenCodeSqliteV2 => discover_opencode_page(
            Path::new(path),
            after,
            &Limits {
                discovery_sessions: remaining,
                ..Limits::default()
            },
        )
        .map(|page| (page.candidates, page.next_cursor))
        .map_err(|_| {
            "Could not list the selected session source. Check its access and supported format."
                .into()
        }),
    }
}

pub(super) fn discover_root(
    kind: SourceKind,
    path: &str,
    remaining: usize,
) -> Result<Vec<Candidate>, String> {
    let limits = Limits {
        discovery_sessions: remaining.max(1),
        ..Limits::default()
    };
    let candidates = match kind {
        SourceKind::ClaudeJsonl => discover_claude(Path::new(path), &limits),
        SourceKind::OpenCodeSqliteV2 => discover_opencode(Path::new(path), &limits),
    }
    .map_err(
        |_| "Could not list the selected session source. Check its access and supported format.",
    )?;
    if candidates.len() > remaining {
        return Err("Choose fewer session sources.".into());
    }
    Ok(candidates)
}

/// Use file generations only as scheduling hints. A periodic full reconcile
/// bounds stale hints; capture still checks admitted handles and exact bytes.
pub(super) fn source_observation(candidate: &Candidate) -> Result<String, String> {
    #[cfg(unix)]
    {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::MetadataExt;
        let mut paths = vec![candidate.source_path().to_owned()];
        match candidate.source_kind() {
            antiburn_cloud_capture::CaptureSourceKind::ClaudeJsonl
                if candidate.native_agent_id().is_some() =>
            {
                paths.push(candidate.source_path().with_extension("meta.json"));
            }
            antiburn_cloud_capture::CaptureSourceKind::OpenCodeSqliteV2 => {
                let mut wal = candidate.source_path().as_os_str().to_owned();
                wal.push("-wal");
                paths.push(wal.into());
            }
            _ => {}
        }
        let mut generations = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_file() && !metadata.is_symlink() => {
                    generations.push(Some((
                        metadata.dev(),
                        metadata.ino(),
                        metadata.len(),
                        metadata.mtime(),
                        metadata.mtime_nsec(),
                        metadata.ctime(),
                        metadata.ctime_nsec(),
                    )));
                }
                Err(error) if index > 0 && error.kind() == std::io::ErrorKind::NotFound => {
                    generations.push(None)
                }
                _ => return Err("Session source metadata is unavailable.".into()),
            }
        }
        let bytes = serde_json::to_vec(&generations)
            .map_err(|_| "Could not record a source observation.")?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    #[cfg(not(unix))]
    {
        let _ = candidate;
        Err("Session capture is not supported on this platform.".into())
    }
}

fn status(
    store: &Store,
    host: &mut capture::Host,
    engine: &Engine,
    state: State,
) -> Result<Status, String> {
    let Some(session) = engine.session() else {
        return Ok(Status {
            status: State::ReauthRequired,
            ..Status::default()
        });
    };
    let policy = store
        .cloud_sync_policy(&session.user.id, &session.workspace.id, &session.resource)
        .map_err(|_| "Could not read Cloud sync consent.")?;
    let mut status = Status {
        status: state,
        account_id: Some(session.user.id.clone()),
        destination_id: Some(session.workspace.id.clone()),
        ..Status::default()
    };
    let Some(policy) = policy else {
        status.status = State::ConsentRequired;
        return Ok(status);
    };
    status.paused = policy.paused;
    if policy.paused {
        status.status = State::Paused;
    }
    status.selected_sources = policy
        .roots
        .iter()
        .map(|root| SourceSelection {
            source_kind: root.source_kind,
            path: root.path.clone(),
        })
        .collect();
    let jobs = store
        .cloud_sync_jobs(&session.user.id, &session.workspace.id, &session.resource)
        .map_err(|_| "Could not read capture progress.")?;
    let entries = host
        .spool()?
        .entries()
        .map_err(|_| "Could not read saved capture progress.")?;
    for job in &jobs {
        if !job.allowed(&policy) {
            continue;
        }
        let entry = entries.iter().find(|entry| entry.identity == job.identity);
        if (entry.is_none()
            && !matches!(
                job.state,
                JobState::Recovering | JobState::WaitingForLocalCapacity
            ))
            || entry.is_some_and(|entry| {
                matches!(
                    entry.state,
                    CaptureState::Preparing | CaptureState::Cancelled
                ) && !empty_preparation(entry)
            })
        {
            status.interrupted_count += 1;
            status.recovery_required_count += 1;
            continue;
        }
        match job.state {
            JobState::Pending
            | JobState::WaitingForCapacity
            | JobState::WaitingForLocalCapacity
            | JobState::Queued => status.queued_count += 1,
            JobState::Interrupted | JobState::Recovering => status.interrupted_count += 1,
            JobState::Fenced => status.fenced_count += 1,
            JobState::Accepted | JobState::Suppressed | JobState::Abandoned => {}
        }
    }
    status.consent_version = Some(policy.consent_version.clone());
    status.policy_revision = Some(policy.revision.clone());
    status.projects = policy.projects.clone();
    status.project_choices = policy.project_choices.clone();
    (status.accepted_count, status.suppressed_count) = store
        .cloud_sync_outcome_counts(&session.user.id, &session.workspace.id, &session.resource)
        .map_err(|_| "Could not read accepted source state.")?;
    status.progress.negotiated = true;
    let mut has_progress = false;
    for entry in entries {
        if entry.identity.account_id == session.user.id
            && entry.identity.destination_id == session.workspace.id
            && jobs
                .iter()
                .any(|job| job.identity == entry.identity && job.allowed(&policy))
        {
            has_progress = true;
            status.progress.add(
                entry
                    .progress()
                    .map_err(|_| "Could not read saved capture progress.")?,
            );
        }
    }
    if !has_progress {
        status.progress.negotiated = false;
    }
    if engine.session().is_none() {
        return Ok(Status {
            status: State::ReauthRequired,
            ..Status::default()
        });
    }
    status.storage_known = true;
    Ok(status)
}

/// Restore ongoing consent and counters, without requesting an upload turn.
#[tauri::command]
pub(crate) async fn cloud_sync_status(app: tauri::AppHandle) -> Result<Status, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let generation = controller.account_generation.load(Ordering::Acquire);
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.generation != generation {
            return Ok(Status {
                status: State::ReauthRequired,
                ..Status::default()
            });
        }
        let current = || controller.account_generation.load(Ordering::Acquire) == generation;
        if let Err(error) = engine.refresh(now(), current) {
            engine.status = error.into();
        }
        super::emit_snapshot(&app, super::snapshot(&app, &engine));
        let store = app.state::<Store>();
        let setup_failed = ensure_cloud_setup(&controller.setup_ready, &store).is_err();
        let mut host = controller
            .capture
            .lock()
            .map_err(|_| "Session capture is unavailable.")?;
        let mut runner = controller
            .sync
            .runner
            .lock()
            .map_err(|_| "Cloud sync is unavailable.")?;
        let result = runner.report_status(
            &store,
            &mut host,
            &engine,
            setup_failed,
            &current,
            |value| {
                emit_status(&app, value);
            },
        );
        if !current() {
            return Ok(Status {
                status: State::ReauthRequired,
                ..Status::default()
            });
        }
        Ok(result)
    })
    .await
}

fn selected_sources(folders: &[Folder]) -> Result<Vec<SourceSelection>, String> {
    let roots = capture::selected_roots(folders);
    if roots.is_empty() || roots.len() > 100 || roots.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("Choose valid session sources before confirming Cloud sync.".into());
    }
    for root in &roots {
        let path = Path::new(&root.path);
        if !path.is_absolute()
            || root.path.len() > 4096
            || root.path.chars().any(char::is_control)
            || path.canonicalize().ok().as_deref() != Some(path)
            || match root.source_kind {
                SourceKind::ClaudeJsonl => !path.is_dir(),
                SourceKind::OpenCodeSqliteV2 => !path.is_file(),
            }
        {
            return Err("Choose an existing session source without path aliases.".into());
        }
    }
    Ok(roots)
}

/// List bounded recorded project choices only after an explicit native source
/// selection. This does not grant content consent or create upload jobs.
#[tauri::command]
pub(crate) async fn cloud_sync_projects(
    app: tauri::AppHandle,
    folders: Vec<Folder>,
    binding: DestinationBinding,
) -> Result<Vec<ProjectChoice>, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let generation = controller.account_generation.load(Ordering::Acquire);
        let current = || controller.account_generation.load(Ordering::Acquire) == generation;
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.generation != generation {
            return Err("Cloud account changed.".into());
        }
        let refreshed = engine.refresh(now(), current);
        if let Err(error) = refreshed { engine.status = error.into(); }
        super::emit_snapshot(&app, super::snapshot(&app, &engine));
        refreshed.map_err(|_| "Connect to Cloud before choosing projects.")?;
        if !engine
            .session()
            .is_some_and(|session| binding.matches(session))
        {
            return Err("Cloud account changed.".into());
        }
        let roots = selected_sources(&folders)?;
        let mut projects = std::collections::BTreeMap::new();
        let mut discovered = 0usize;
        let mut cache = controller.project_cache.lock().map_err(|_| "Project discovery is unavailable.")?;
        cache.begin_scan();
        for root in roots {
            if !current() {
                return Err("Cloud account changed.".into());
            }
            let candidates = discover_root(
                root.source_kind,
                &root.path,
                512usize.saturating_sub(discovered),
            )?;
            discovered += candidates.len();
            for candidate in candidates {
                if !current() { return Err("Cloud account changed.".into()); }
                let evidence = cache.evidence(&binding.account_id, &binding.destination_id, &root,
                    &candidate, &metadata_limits())?.ok_or("Project metadata is still loading. Retry discovery.")?;
                if !evidence.scan_complete() { return Err("Some project metadata is unreadable. Restore access or use a supported source.".into()); }
                for project_path in evidence.project_paths().iter().filter_map(|path| path.to_str()) {
                    let selection = ProjectSelection {
                        source_kind: root.source_kind,
                        source_path: root.path.clone(),
                        project_path: project_path.to_owned(),
                    };
                    *projects.entry(selection).or_insert(0u32) += 1;
                }
            }
        }
        if !current() || engine.session().is_none() {
            return Err("Cloud account changed.".into());
        }
        Ok(projects
            .into_iter()
            .map(|(selection, session_count)| ProjectChoice {
                selection,
                session_count,
            })
            .collect())
    })
    .await
}

/// Persist separate ongoing content consent. The policy binds only selected
/// native sources and recorded project/CWD directories, never touched files.
#[tauri::command]
pub(crate) async fn cloud_enable_sync(
    app: tauri::AppHandle,
    mut consent: Consent,
) -> Result<Status, String> {
    if !cfg!(unix) {
        return Err("Session capture is not supported on this platform.".into());
    }
    let work = app.state::<Controller>().blocking.clone();
    let wake = app.state::<Controller>().sync.wake.clone();
    let content_generation = app
        .state::<Controller>()
        .sync
        .generation
        .load(Ordering::Acquire);
    let result = work
        .run(Priority::Normal, move || {
            let controller = app.state::<Controller>();
            let generation = controller.account_generation.load(Ordering::Acquire);
            let account_current =
                || controller.account_generation.load(Ordering::Acquire) == generation;
            let current = || {
                account_current()
                    && controller.sync.generation.load(Ordering::Acquire) == content_generation
            };
            let mut engine = controller
                .engine
                .lock()
                .map_err(|_| "Cloud is unavailable.")?;
            if engine.generation != generation {
                return Err("Cloud account changed. Confirm Cloud sync again.".into());
            }
            let refreshed = engine.refresh(now(), account_current);
            if let Err(error) = refreshed {
                engine.status = error.into();
            }
            super::emit_snapshot(&app, super::snapshot(&app, &engine));
            refreshed.map_err(|_| "Connect to Cloud before confirming sync.")?;
            let session = engine
                .session()
                .ok_or("Connect to Cloud before confirming sync.")?;
            let binding = DestinationBinding {
                account_id: consent.account_id.clone(),
                destination_id: consent.destination_id.clone(),
            };
            if !binding.matches(session)
                || consent.version != ONGOING_CONSENT_VERSION
                || !consent.full_content
                || !consent.ongoing
                || consent.attachments
                || consent.selected_sources.is_empty()
                || consent.selected_sources.len() > 100
                || consent.projects.is_empty()
                || consent.projects.len() > 100
                || serde_json::to_vec(&consent.project_choices)
                    .map_err(|_| "Could not read confirmed project choices.")?
                    .len()
                    > 524_288
            {
                return Err(
                    "Confirm full session content and ongoing sync for these projects.".into(),
                );
            }
            consent.selected_sources.sort();
            consent.projects.sort();
            antiburn_cloud_capture::ProjectSelection::new(
                consent
                    .projects
                    .iter()
                    .map(|project| Path::new(&project.project_path).to_owned())
                    .collect(),
            )
            .map_err(|_| "Choose existing project directories without path aliases.")?;
            let folders = consent
                .selected_sources
                .iter()
                .map(|root| Folder {
                    path: root.path.clone(),
                    source_kind: root.source_kind,
                    selected: true,
                    ..Folder::default()
                })
                .collect::<Vec<_>>();
            let roots = selected_sources(&folders)?;
            if roots != consent.selected_sources || !current() {
                return Err("Cloud sources changed. Confirm sync again.".into());
            }
            let store = app.state::<Store>();
            ensure_cloud_setup(&controller.setup_ready, &store)?;
            let capabilities = upload::capabilities(
                &mut engine,
                &consent.account_id,
                &consent.destination_id,
                &account_current,
                &current,
            );
            super::emit_snapshot(&app, super::snapshot(&app, &engine));
            let limits = capabilities
                .map_err(|_| "Cloud upload is unavailable. No source content was uploaded.")?;
            controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?
                .configure_limits(limits)?;
            controller
                .sync
                .generation
                .compare_exchange(
                    content_generation,
                    content_generation + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .map_err(|_| "Cloud upload controls changed. Confirm again.")?;
            store
                .save_cloud_sync_policy(
                    &consent.account_id,
                    &consent.destination_id,
                    engine
                        .resource_origin()
                        .ok_or("Cloud service is unavailable.")?,
                    &roots,
                    &consent.projects,
                    &consent.project_choices,
                )
                .map_err(|_| "Could not save Cloud sync consent.")?;
            let mut runner = controller
                .sync
                .runner
                .lock()
                .map_err(|_| "Cloud sync is unavailable.")?;
            runner.next_reconcile = None;
            runner.state = State::Ready;
            let mut host = controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?;
            let result = status(&store, &mut host, &engine, State::Ready)?;
            if !account_current()
                || controller.sync.generation.load(Ordering::Acquire) != content_generation + 1
            {
                return Err("Cloud account changed. Saved work is kept.".into());
            }
            super::emit_snapshot(&app, super::snapshot(&app, &engine));
            Ok(emit_status(&app, result))
        })
        .await;
    wake.notify_one();
    result
}

/// Change one approved Settings group without replacing a paged policy or resuming uploads.
#[tauri::command]
pub(crate) async fn cloud_set_sync_project(
    app: tauri::AppHandle,
    change: ProjectChange,
) -> Result<Status, String> {
    use super::projects::{confirm_group_projects, group_projects_to_remove};
    use crate::store::cloud_capture::sync::valid_project_path;

    if change.group_id.len() != 43
        || !change
            .group_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || change.projects.len() > 100
        || change.folder_paths.len() > 100
        || !change
            .folder_paths
            .iter()
            .all(|path| valid_project_path(path))
        || (change.selected && change.projects.is_empty())
    {
        return Err("Reload projects and choose a supported Cloud project.".into());
    }
    let controller = app.state::<Controller>();
    let account_generation = controller.account_generation.load(Ordering::Acquire);
    let work = controller.blocking.clone();
    let wake = controller.sync.wake.clone();
    let content_generation = if change.selected {
        controller.sync.generation.load(Ordering::Acquire)
    } else {
        controller.sync.generation.fetch_add(1, Ordering::AcqRel) + 1
    };
    let priority = if change.selected {
        Priority::Normal
    } else {
        Priority::Cleanup
    };
    let result = work
        .run(priority, move || {
            let controller = app.state::<Controller>();
            let account_current =
                || controller.account_generation.load(Ordering::Acquire) == account_generation;
            let current = || {
                account_current()
                    && controller.sync.generation.load(Ordering::Acquire) == content_generation
            };
            let mut engine = controller
                .engine
                .lock()
                .map_err(|_| "Cloud is unavailable.")?;
            if engine.generation != account_generation || !current() {
                return Err("Cloud account or upload controls changed. Reload projects.".into());
            }
            if change.selected {
                let refreshed = engine.refresh(now(), account_current);
                if let Err(error) = refreshed {
                    engine.status = error.into();
                }
                super::emit_snapshot(&app, super::snapshot(&app, &engine));
                refreshed.map_err(|_| "Connect to Cloud before confirming sync.")?;
            }
            let binding = DestinationBinding {
                account_id: change.account_id,
                destination_id: change.destination_id,
            };
            let session = engine
                .session()
                .ok_or("Cloud account changed. Reload projects.")?;
            if !binding.matches(session) || !current() {
                return Err("Cloud account or upload controls changed. Reload projects.".into());
            }
            let store = app.state::<Store>();
            ensure_cloud_setup(&controller.setup_ready, &store)?;
            let policy = store
                .cloud_sync_policy(
                    &binding.account_id,
                    &binding.destination_id,
                    &session.resource,
                )
                .map_err(|_| "Could not read Cloud sync consent.")?
                .ok_or("Confirm Cloud sync first.")?;
            if policy.revision != change.expected_policy_revision {
                return Err("Cloud project choices changed. Reload projects and try again.".into());
            }
            let mut host = controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?;
            let (projects, fence) = if change.selected {
                let projects: BTreeSet<_> = change.projects.into_iter().collect();
                antiburn_cloud_capture::ProjectSelection::new(
                    projects
                        .iter()
                        .map(|project| Path::new(&project.project_path).to_owned())
                        .collect(),
                )
                .map_err(|_| "Choose existing project directories without path aliases.")?;
                confirm_group_projects(
                    &store,
                    &mut *controller
                        .project_cache
                        .lock()
                        .map_err(|_| "Project discovery is unavailable.")?,
                    &binding,
                    &change.group_id,
                    &projects,
                    &current,
                )?;
                let capabilities = upload::capabilities(
                    &mut engine,
                    &binding.account_id,
                    &binding.destination_id,
                    &account_current,
                    &current,
                );
                super::emit_snapshot(&app, super::snapshot(&app, &engine));
                host.configure_limits(capabilities.map_err(
                    |_| "Cloud upload is unavailable. No source content was uploaded.",
                )?)?;
                controller
                    .sync
                    .generation
                    .compare_exchange(
                        content_generation,
                        content_generation + 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .map_err(|_| "Cloud upload controls changed. Confirm again.")?;
                (projects, content_generation + 1)
            } else {
                (
                    group_projects_to_remove(
                        &change.group_id,
                        &change.folder_paths,
                        &policy.projects,
                    )?,
                    content_generation,
                )
            };
            let can_commit = || {
                account_current()
                    && controller.sync.generation.load(Ordering::Acquire) == fence
                    && engine
                        .session()
                        .is_some_and(|session| binding.matches(session))
            };
            let saved = store
                .set_cloud_sync_project(
                    &policy,
                    &change.group_id,
                    change.selected,
                    &projects,
                    can_commit,
                )
                .map_err(
                    |_| "Could not save the Cloud project change. Reload projects and try again.",
                )?;
            let mut runner = controller
                .sync
                .runner
                .lock()
                .map_err(|_| "Cloud sync is unavailable.")?;
            if change.selected && !saved.paused {
                runner.next_reconcile = None;
                runner.retry_deadlines.clear();
                runner.finalization_submissions.clear();
                runner.state = State::Ready;
            }
            let result =
                status(&store, &mut host, &engine, runner.state).unwrap_or_else(|_| Status {
                    status: State::StorageUnavailable,
                    account_id: Some(saved.account_id),
                    destination_id: Some(saved.destination_id),
                    consent_version: Some(saved.consent_version),
                    policy_revision: Some(saved.revision),
                    paused: saved.paused,
                    selected_sources: saved
                        .roots
                        .into_iter()
                        .map(|root| SourceSelection {
                            source_kind: root.source_kind,
                            path: root.path,
                        })
                        .collect(),
                    projects: saved.projects,
                    project_choices: saved.project_choices,
                    ..Status::default()
                });
            if !can_commit() {
                return Err("Cloud account or upload controls changed. Saved work is kept.".into());
            }
            Ok(emit_status(&app, result))
        })
        .await;
    wake.notify_one();
    result
}

/// Fence sensitive work immediately, then persist pause without removing bytes.
#[tauri::command]
pub(crate) async fn cloud_pause_sync(
    app: tauri::AppHandle,
    binding: DestinationBinding,
) -> Result<Status, String> {
    app.state::<Controller>()
        .sync
        .generation
        .fetch_add(1, Ordering::AcqRel);
    control(app, binding, true, Priority::Cleanup).await
}

/// Resume saved chunks under their original bindings. An empty pre-capture
/// failure gets a fresh capture ID. Saved chunks, suppression, and plans remain.
#[tauri::command]
pub(crate) async fn cloud_retry_sync(
    app: tauri::AppHandle,
    binding: DestinationBinding,
) -> Result<Status, String> {
    control(app, binding, false, Priority::Normal).await
}

async fn control(
    app: tauri::AppHandle,
    binding: DestinationBinding,
    paused: bool,
    priority: Priority,
) -> Result<Status, String> {
    let work = app.state::<Controller>().blocking.clone();
    let wake = app.state::<Controller>().sync.wake.clone();
    let content_generation = app
        .state::<Controller>()
        .sync
        .generation
        .load(Ordering::Acquire);
    let result = work
        .run(priority, move || {
            let controller = app.state::<Controller>();
            let generation = controller.account_generation.load(Ordering::Acquire);
            let current = || {
                controller.account_generation.load(Ordering::Acquire) == generation
                    && controller.sync.generation.load(Ordering::Acquire) == content_generation
            };
            let engine = controller
                .engine
                .lock()
                .map_err(|_| "Cloud is unavailable.")?;
            let session = engine.session();
            if !current()
                || engine.generation != generation
                || session.is_some_and(|session| !binding.matches(session))
                || (!paused && session.is_none())
            {
                return Err("Cloud account changed. Saved work is kept.".into());
            }
            let store = app.state::<Store>();
            ensure_cloud_setup(&controller.setup_ready, &store)?;
            let policy = store
                .cloud_sync_policy(
                    &binding.account_id,
                    &binding.destination_id,
                    engine
                        .resource_origin()
                        .ok_or("Cloud service is unavailable.")?,
                )
                .map_err(|_| "Could not read Cloud sync consent.")?
                .ok_or("Confirm Cloud sync first.")?;
            store
                .set_cloud_sync_paused(&policy, paused)
                .map_err(|_| "Could not save Cloud sync controls.")?;
            if !paused {
                retry_saved_jobs(
                    &store,
                    controller
                        .capture
                        .lock()
                        .map_err(|_| "Desktop capture is unavailable.")?
                        .spool()?,
                    &policy,
                    &current,
                )?;
            }
            let mut runner = controller
                .sync
                .runner
                .lock()
                .map_err(|_| "Cloud sync is unavailable.")?;
            runner.next_reconcile = None;
            runner.retry_deadlines.clear();
            runner.finalization_submissions.clear();
            runner.state = if paused { State::Paused } else { State::Ready };
            let mut host = controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?;
            let mut result = status(&store, &mut host, &engine, runner.state)?;
            if paused && session.is_none() {
                result.status = State::Paused;
                result.paused = true;
            }
            if !current() {
                return Err("Cloud account changed. Saved work is kept.".into());
            }
            super::emit_snapshot(&app, super::snapshot(&app, &engine));
            Ok(emit_status(&app, result))
        })
        .await;
    wake.notify_one();
    result
}

#[cfg(all(test, unix))]
#[path = "tests/sync_integration_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "tests/sync_paging_integration_tests.rs"]
mod paging_tests;

#[cfg(all(test, unix, debug_assertions))]
#[path = "tests/sync_scheduler_integration_tests.rs"]
mod scheduler_tests;

#[cfg(test)]
#[path = "tests/sync_consent_tests.rs"]
mod consent_tests;
