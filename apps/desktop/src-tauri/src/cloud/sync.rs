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
    discover_claude, discover_opencode,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tokio::sync::Notify;

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
    paused: bool,
    selected_sources: Vec<SourceSelection>,
    projects: Vec<ProjectSelection>,
    project_choices: BTreeMap<String, bool>,
    queued_count: u32,
    interrupted_count: u32,
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
/// authoritative after restart. A fresh reconcile replaces this bounded cache.
#[derive(Default)]
struct Runner {
    candidates: VecDeque<Selection>,
    next_reconcile: Option<Instant>,
    binding: Option<(String, String, String)>,
    cursor: Option<String>,
    capture_turn: bool,
    discovery_blocked: bool,
    state: State,
    last_status: Option<Status>,
    last_status_origin: Option<String>,
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
            return Ok(());
        };
        let Some(policy) = store
            .cloud_sync_policy(&session.user.id, &session.workspace.id, &session.resource)
            .map_err(|_| "Could not read Cloud sync consent.")?
        else {
            self.state = State::ConsentRequired;
            self.candidates.clear();
            return Ok(());
        };
        if policy.paused || !current() {
            self.state = State::Paused;
            self.candidates.clear();
            return Ok(());
        }
        let binding = (
            policy.account_id.clone(),
            policy.destination_id.clone(),
            policy.revision.clone(),
        );
        if self.binding.as_ref() != Some(&binding) {
            self.candidates.clear();
            self.next_reconcile = None;
            self.binding = Some(binding);
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
        if jobs.iter().any(|job| {
            job.state != JobState::Recovering
                && !entries.iter().any(|entry| entry.identity == job.identity)
        }) {
            return Err(
                "Saved capture state is missing. Restore it under its original ID before retrying."
                    .into(),
            );
        }
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
                        !matches!(job.state, JobState::Fenced | JobState::Recovering)
                            && job.identity == entry.identity
                            && job.allowed(&policy)
                    })
            })
            .collect::<Vec<_>>();
        if self
            .next_reconcile
            .is_none_or(|time| time <= Instant::now())
        {
            match discover_policy_cached(&policy, current, cache) {
                Ok(candidates) => {
                    self.candidates = candidates.into();
                    self.discovery_blocked = false;
                }
                Err(_) => {
                    self.candidates.clear();
                    self.discovery_blocked = true;
                }
            }
            self.next_reconcile = Some(Instant::now() + Duration::from_secs(60));
        }
        self.capture_turn = !self.capture_turn;
        if !pending.is_empty() && (!self.capture_turn || self.candidates.is_empty()) {
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
            let mut transport =
                upload::HttpTransport::new(engine, account_current, current, origin, None);
            let result = host.spool()?.resume_step(&entry.identity, &mut transport);
            self.state = transport_state(transport.failure, &result);
            if result.is_err()
                && self.state == State::Fenced
                && let Some(job) = jobs.iter().find(|job| job.identity == entry.identity)
            {
                store
                    .set_cloud_sync_job_state(job, JobState::Fenced)
                    .map_err(|_| "Could not save fenced capture state.")?;
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
        for job in &jobs {
            if !job.allowed(&policy) && job.state != JobState::Fenced {
                store
                    .set_cloud_sync_job_state(job, JobState::Fenced)
                    .map_err(|_| "Could not save fenced capture state.")?;
            }
        }
        while let Some(selection) = self.candidates.pop_front() {
            if !current() {
                return Ok(());
            }
            let saved = jobs
                .iter()
                .find(|job| job.identity.source_id == selection.candidate.source_id());
            if saved.is_some_and(|job| job.state != JobState::Pending) {
                continue;
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
                        || source.observation_key.as_deref() == Some(&selection.observation)
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
                    self.state = match failure {
                        upload::Failure::Offline => State::Offline,
                        upload::Failure::Unauthorized => State::ReauthRequired,
                        _ => State::UploadUnavailable,
                    };
                    self.candidates.push_front(selection);
                    return Ok(());
                }
            };
            host.configure_limits(limits.clone())?;
            let folders = policy
                .projects
                .iter()
                .filter(|project| {
                    policy.roots.iter().any(|root| {
                        root.source_scope_id == selection.scope
                            && root.source_kind == project.source_kind
                            && root.path == project.source_path
                    })
                })
                .map(|project| Path::new(&project.project_path).to_owned())
                .collect();
            let projects = antiburn_cloud_capture::ProjectSelection::new(folders)
                .map_err(|_| "A selected project folder is unavailable.")?;
            let Some(candidate) = projects
                .admit_with_limits(&selection.candidate, &limits)
                .map_err(|_| "Project admission failed. No session content was uploaded.")?
            else {
                self.state = State::Fenced;
                continue;
            };
            if !current() {
                self.candidates.push_front(selection);
                self.state = State::Paused;
                return Ok(());
            }
            let job = if let Some(job) = saved {
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
            let result = host.spool()?.capture(
                job.identity.clone(),
                &candidate,
                MetadataSections::new(),
                &mut transport,
            );
            if transport.failure == Some(upload::Failure::SourceSuppressed) {
                store.suppress_cloud_sync(&job).map_err(
                    |_| "Could not save the Cloud deletion outcome. Saved captures are kept.",
                )?;
                self.state = State::Ready;
                return Ok(());
            }
            self.state = transport_state(transport.failure, &result);
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
                Some(_) => JobState::Interrupted,
                None => JobState::Pending,
            };
            store
                .set_cloud_sync_job_state(&job, state)
                .map_err(|_| "Could not save capture progress.")?;
            if result.is_err() && transport.failure.is_none() {
                self.state = State::Interrupted;
            }
            return Ok(());
        }
        self.state = if jobs
            .iter()
            .any(|job| matches!(job.state, JobState::Interrupted | JobState::Recovering))
        {
            State::Interrupted
        } else if self.discovery_blocked || jobs.iter().any(|job| job.state == JobState::Fenced) {
            State::Fenced
        } else {
            State::Ready
        };
        Ok(())
    }
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
                CaptureState::Preparing => JobState::Interrupted,
                CaptureState::Queued | CaptureState::AwaitingIntegrity => JobState::Queued,
                CaptureState::Accepted | CaptureState::Cancelled => continue,
            };
            if state != job.state {
                store
                    .set_cloud_sync_job_state(job, state)
                    .map_err(|_| "Could not restore saved capture progress.")?;
                job.state = state;
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
            return Err(
                "Saved capture state is missing. Restore it under its original ID before retrying."
                    .into(),
            );
        }
        let empty_preparing = entry.is_some_and(|entry| {
            matches!(
                entry.state,
                CaptureState::Preparing | CaptureState::Cancelled
            ) && entry.reservation_id.is_none()
                && entry.manifest.is_none()
                && entry.upload_plan.is_none()
                && entry.server_status.is_none()
                && !entry.completion_submitted
        });
        if matches!(
            job.state,
            JobState::Interrupted | JobState::Pending | JobState::Recovering
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
        } else if job.state == JobState::Fenced
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

fn transport_state<T>(
    failure: Option<upload::Failure>,
    result: &Result<T, antiburn_cloud_capture::Error>,
) -> State {
    let failure = failure.or(match result {
        Err(antiburn_cloud_capture::Error::Transport(error)) => Some(match error {
            antiburn_cloud_capture::TransportError::Offline => upload::Failure::Offline,
            antiburn_cloud_capture::TransportError::Unauthorized => upload::Failure::Unauthorized,
            antiburn_cloud_capture::TransportError::Conflict => upload::Failure::Conflict,
            antiburn_cloud_capture::TransportError::Rejected => upload::Failure::Rejected,
        }),
        _ => None,
    });
    match failure {
        Some(upload::Failure::Offline) => State::Offline,
        Some(upload::Failure::Unauthorized) => State::ReauthRequired,
        Some(upload::Failure::Unavailable) => State::UploadUnavailable,
        Some(upload::Failure::Conflict | upload::Failure::Rejected) => State::Fenced,
        Some(upload::Failure::SourceSuppressed) => State::Ready,
        None if result.is_ok() => State::Syncing,
        None => State::Fenced,
    }
}

/// Inspect only admitted native source candidates. Project matching uses directory components;
/// paths in tool calls, attachments, and transcript text cannot grant access.
#[cfg(test)]
fn discover_policy(policy: &Policy, current: &dyn Fn() -> bool) -> Result<Vec<Selection>, String> {
    discover_policy_cached(policy, current, &mut super::projects::Cache::default())
}

pub(super) fn metadata_limits() -> Limits {
    Limits {
        source_bytes: 128 * 1024 * 1024,
        ..Limits::default()
    }
}

fn discover_policy_cached(
    policy: &Policy,
    current: &dyn Fn() -> bool,
    cache: &mut super::projects::Cache,
) -> Result<Vec<Selection>, String> {
    cache.begin_scan();
    let mut selected = Vec::new();
    let mut discovered = 0usize;
    for root in &policy.roots {
        if !current() {
            return Ok(Vec::new());
        }
        let candidates = discover_root(
            root.source_kind,
            &root.path,
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
                return Ok(Vec::new());
            }
            let root_selection = SourceSelection {
                source_kind: root.source_kind,
                path: root.path.clone(),
            };
            let Some(evidence) = cache.evidence(
                &policy.account_id,
                &policy.destination_id,
                &root_selection,
                &candidate,
                &metadata_limits(),
            )?
            else {
                continue;
            };
            if !projects
                .matches(&evidence)
                .map_err(|_| "Could not check the recorded project.")?
            {
                continue;
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
            let observation = {
                use sha2::{Digest, Sha256};
                let bytes = serde_json::to_vec(&(
                    source_observation(&candidate)?,
                    evidence.source_digest(),
                ))
                .map_err(|_| "Could not record a source observation.")?;
                Sha256::digest(bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
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
    }
    Ok(selected)
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
    status.consent_version = Some(policy.consent_version);
    status.selected_sources = policy
        .roots
        .iter()
        .map(|root| SourceSelection {
            source_kind: root.source_kind,
            path: root.path.clone(),
        })
        .collect();
    status.projects = policy.projects;
    status.project_choices = policy.project_choices;
    let jobs = store
        .cloud_sync_jobs(&session.user.id, &session.workspace.id, &session.resource)
        .map_err(|_| "Could not read capture progress.")?;
    for job in &jobs {
        match job.state {
            JobState::Pending | JobState::Queued => status.queued_count += 1,
            JobState::Interrupted | JobState::Recovering => status.interrupted_count += 1,
            JobState::Fenced => status.fenced_count += 1,
            JobState::Accepted | JobState::Suppressed | JobState::Abandoned => {}
        }
    }
    (status.accepted_count, status.suppressed_count) = store
        .cloud_sync_outcome_counts(&session.user.id, &session.workspace.id, &session.resource)
        .map_err(|_| "Could not read accepted source state.")?;
    status.progress.negotiated = true;
    let mut has_progress = false;
    let entries = host
        .spool()?
        .entries()
        .map_err(|_| "Could not read saved capture progress.")?;
    if jobs.iter().any(|job| {
        job.state != JobState::Recovering
            && !entries.iter().any(|entry| entry.identity == job.identity)
    }) {
        return Err(
            "Saved capture state is missing. Restore it under its original ID before retrying."
                .into(),
        );
    }
    for entry in entries {
        if entry.identity.account_id == session.user.id
            && entry.identity.destination_id == session.workspace.id
            && jobs.iter().any(|job| job.identity == entry.identity)
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

#[cfg(all(test, unix, debug_assertions))]
#[path = "tests/sync_scheduler_integration_tests.rs"]
mod scheduler_tests;

#[cfg(test)]
#[path = "tests/sync_consent_tests.rs"]
mod consent_tests;
