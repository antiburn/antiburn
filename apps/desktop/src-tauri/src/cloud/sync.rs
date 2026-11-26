//! Session upload scheduling. One controller-owned scheduler shares bounded work
//! with auth and cleanup. Renderers supply explicit consent and read safe status;
//! they do not drive capture turns or own retries.

use super::{
    Controller, DestinationBinding, Folder, auth, capture, engine::Engine, ensure_cloud_setup,
    file_sources, now, upload, work::Priority,
};
#[cfg(test)]
use crate::store::cloud_capture::SourceKind;
use crate::store::{
    Store,
    cloud_capture::{
        SourceSelection,
        sync::{
            Job, JobState, MAX_RECORDED_PROJECT_PATH_BYTES, MAX_SYNC_PROJECT_BINDINGS,
            MAX_SYNC_PROJECT_PATHS, MAX_SYNC_SOURCE_ROOTS, ONGOING_CONSENT_VERSION, Policy,
            ProjectSelection,
        },
    },
};
use antiburn_cloud_capture::{CaptureState, QueueEntry, Transport as _};
#[cfg(all(test, unix))]
use antiburn_cloud_capture::{Limits, MetadataSections, discover_claude, discover_opencode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tokio::sync::Notify;

#[path = "backfill_progress.rs"]
mod backfill_progress;
mod device_upload;
mod discovery;
mod folder_move;
#[path = "git_checkout.rs"]
mod git_checkout;
mod project_directories;
mod repository_capture;
mod repository_policy;
mod runtime_progress;
mod scheduler_tick;
mod transfer_progress;
use repository_policy::{
    has_folders, revoke_repository, saved_folders, saved_team_folder_id, team_policy_status,
};

use backfill_progress::{BackfillProgress, SourceProblem, UploadProgress};
#[cfg(all(test, unix, debug_assertions))]
use discovery::legacy_file_observation;
use discovery::{
    DiscoveryCursor, Selection, check_retry_observation, discover_policy_page,
    reopen_waiting_source,
};
#[cfg(all(test, unix))]
use discovery::{discover_policy, discover_root};
pub(super) use discovery::{discover_root_page, metadata_limits, source_observation};
use folder_move::{verify_project_move, verify_project_move_to_new_policy};
use project_directories::{exact_project_directories, validate_project_directories};
pub(super) use scheduler_tick::start;
use transfer_progress::{Progress, UploadTransfer, saved_transfer_progress};

const RESERVATION_RETRY_DELAY: Duration = Duration::from_secs(15);
const INTEGRITY_POLL_DELAY: Duration = Duration::from_secs(5);
const FINALIZATION_PROBE_LIMIT: u8 = 3;
const ROUTE_DENIAL_RETRY_DELAY: Duration = Duration::from_secs(60);
const SYNC_CONTROL_ADMISSION_WAIT: Duration = Duration::from_secs(5);

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
    move_from: Option<MoveFrom>,
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
    /// For a move, revoke only these exact saved selections.
    expected_removals: Option<Vec<ProjectSelection>>,
    move_from: Option<MoveFrom>,
}

/// The source workspace and its policy revision after the move's removal step.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MoveFrom {
    destination_id: String,
    policy_revision: String,
}

/// Confirm the exact saved folder list and any explicit additions for one Team.
/// This confirmation never authorizes old jobs from a different membership.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TeamConfirmation {
    account_id: String,
    destination_id: String,
    expected_policy_revision: String,
    folder_paths: Vec<String>,
    #[serde(default)]
    additions: BTreeMap<String, Vec<ProjectSelection>>,
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
    RouteDenied,
    Fenced,
    Interrupted,
    StorageUnavailable,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    team_statuses: Vec<Status>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_folders: Option<Vec<SavedFolder>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    repository_group_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    repository_folders: Vec<repository_policy::RepositoryFolder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exact_projects: Option<Vec<ProjectSelection>>,
    /// Current Team eligibility is verified, but saved folder consent is stale.
    folder_confirmation_required: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    recovery: Option<super::workspace_grants::Recovery>,
    device_upload_enabled: bool,
    paused: bool,
    source_confirmation_required: bool,
    /// At least one saved job can be retried for this exact destination.
    retry_available: bool,
    /// Desktop still has a requested sweep for this saved policy.
    backfill_in_progress: bool,
    /// The explicit scan still has discovery, upload, or automatic verification work.
    full_sync_in_progress: bool,
    /// Normal discovery or resumable saved work remains, even without scan counters.
    background_sync_in_progress: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    upload_progress: Option<UploadProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    background_progress: Option<UploadProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upload_transfer: Option<UploadTransfer>,
    selected_sources: Vec<SourceSelection>,
    projects: Vec<ProjectSelection>,
    project_choices: BTreeMap<String, bool>,
    queued_count: u32,
    interrupted_count: u32,
    recovery_required_count: u32,
    /// Saved jobs the current scheduler can advance without an explicit Retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    automatic_recovery_count: Option<u32>,
    fenced_count: u32,
    accepted_count: u32,
    suppressed_count: u32,
    progress: Progress,
}

impl Status {
    fn with_device_upload_enabled(mut self, enabled: bool) -> Self {
        self.apply_device_upload_enabled(enabled);
        self
    }

    fn apply_device_upload_enabled(&mut self, enabled: bool) {
        self.device_upload_enabled = enabled;
        self.retry_available &= enabled;
        self.backfill_in_progress &= enabled;
        self.full_sync_in_progress &= enabled;
        self.background_sync_in_progress &= enabled;
        if !enabled {
            self.upload_progress = None;
            self.background_progress = None;
            self.upload_transfer = None;
            self.automatic_recovery_count = None;
        }
        if !enabled && self.consent_version.is_some() {
            self.paused = true;
            if !matches!(
                self.status,
                State::StorageUnavailable | State::ReauthRequired
            ) {
                self.status = State::Paused;
            }
        }
        for team in &mut self.team_statuses {
            team.apply_device_upload_enabled(enabled);
        }
    }
}

/// One exact saved folder that the renderer can confirm or turn off without discovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedFolder {
    group_id: String,
    path: String,
}

/// Memory-only scheduling hints. Durable policy, jobs, and spool entries remain
/// authoritative after restart. Each bounded discovery page stays until drained.
#[derive(Default)]
struct Runner {
    /// Team destination verified for this turn. None is the Personal runner.
    team: Option<(String, String)>,
    candidates: VecDeque<Selection>,
    next_reconcile: Option<Instant>,
    next_reservation: Option<Instant>,
    route_denied_until: Option<Instant>,
    /// Bound retries of saved requests without delaying unrelated captures.
    retry_deadlines: BTreeMap<String, (Instant, upload::Failure)>,
    /// A successful capability lookup did not advertise this saved job's format.
    unsupported_saved_uploads: BTreeSet<String>,
    /// Rejections of both the saved request and its recovery. Stop probing a
    /// persistent fence until the user retries, signs in again, or restarts.
    conflict_attempts: BTreeMap<String, u8>,
    /// Replay submitted finalization once per sign-in, restart, or explicit retry.
    finalization_submissions: BTreeSet<String>,
    /// Track replay separately from status conflict probes. A zero count keeps
    /// a denied replay pending without using a transient attempt.
    conflict_finalization_replays: BTreeMap<String, u8>,
    /// The first observed or last reconciled grant for this route.
    retry_grant: Option<RetryGrant>,
    binding: Option<(String, String, Option<String>, String)>,
    discovery_cursor: Option<DiscoveryCursor>,
    continue_discovery: bool,
    backfill_revision: Option<String>,
    backfill_progress: Option<BackfillProgress>,
    /// Keep explicit run ownership until its scheduled uploads and verification drain.
    full_sync_pending: bool,
    background_progress: Option<BackfillProgress>,
    /// The normal sweep has read a page that has not drained through its final cursor.
    background_page: bool,
    backfill_page: bool,
    /// Give backfill a discovery turn before another integrity poll, even if the
    /// time between scheduler turns exceeds the poll delay.
    backfill_discovery_due: bool,
    cursor: Option<String>,
    next_integrity_poll: Option<Instant>,
    discovery_blocked: bool,
    /// A failed provider read or capability lookup cannot advance discovery now.
    /// This hint affects status only, not scheduling or upload admission.
    discovery_unavailable: bool,
    unsupported_format: bool,
    state: State,
    last_status: Option<Status>,
    last_status_origin: Option<String>,
    repository_migration: Option<(String, Instant)>,
}

enum DiscoveryRefresh {
    Unchanged,
    ReadPage,
    YieldedOpenCodeSlice,
}

struct RetryGrant {
    account_id: String,
    destination_id: String,
    grant_id: String,
}

/// Controller-owned wakeup and content fence. Pause invalidates sensitive work
/// before waiting for the shared blocking permit.
#[derive(Default)]
pub(super) struct Scheduler {
    wake: Arc<Notify>,
    pub generation: AtomicU64,
    // An odd value stops admission immediately, even if saving OFF fails.
    // Each OFF changes the value so an older ON cannot undo a newer pause.
    device_pause: AtomicU64,
    runner: Mutex<Runner>,
    team_runners: Mutex<BTreeMap<(String, String), Runner>>,
    team_cursor: Mutex<Option<String>>,
}

impl Scheduler {
    fn device_admission_open(&self) -> bool {
        self.device_pause.load(Ordering::Acquire) & 1 == 0
    }

    fn fence_device_uploads(&self) -> u64 {
        let previous = self
            .device_pause
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.wrapping_add(2) | 1)
            })
            .expect("device pause always advances");
        self.generation.fetch_add(1, Ordering::AcqRel);
        previous.wrapping_add(2) | 1
    }

    /// Release this fence only for the current request. A newer OFF replaces it.
    fn finish_device_change(&self, expected: u64) -> Result<(), String> {
        self.device_pause
            .compare_exchange(expected, expected & !1, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "Cloud upload controls changed. Try again.".into())
    }

    /// Read the durable preference unless a newer OFF still fences work locally.
    pub(super) fn device_upload_enabled(&self, store: &Store) -> Result<bool, String> {
        if !self.device_admission_open() {
            return Ok(false);
        }
        store
            .cloud_device_upload_enabled()
            .map_err(|_| "Could not read Cloud upload controls.".into())
    }
}

/// Run one exact Team policy per tick. On a short tick, run only a pending
/// discovery continuation and check fresh membership before using it.
fn turn_team_workspace(
    app: &tauri::AppHandle,
    store: &Store,
    engine: &mut Engine,
    host: &mut capture::Host,
    cache: &mut super::projects::Cache,
    fences: (&dyn Fn() -> bool, &dyn Fn() -> bool),
    regular_tick: bool,
) -> Result<bool, String> {
    let (current, account_current) = fences;
    let controller = app.state::<Controller>();
    let Some(session) = engine.session().cloned() else {
        return Ok(false);
    };
    let mut runners = controller
        .sync
        .team_runners
        .lock()
        .map_err(|_| "Cloud sync is unavailable.")?;
    if !regular_tick && !runners.values().any(Runner::discovery_continuation_due) {
        return Ok(false);
    }
    let policies = store
        .saved_team_sync_policies(
            &session.user.id,
            &session.workspace.id,
            &session.resource,
            &session.iss,
        )
        .map_err(|_| "Could not read team folder consent.")?;
    if !policies.iter().any(|policy| {
        policy.membership_generation.is_some() && !policy.paused && has_folders(policy)
    }) {
        runners.clear();
        return Ok(false);
    }
    let binding = DestinationBinding {
        account_id: session.user.id.clone(),
        destination_id: session.workspace.id.clone(),
    };
    let engine_generation = engine.generation;
    let eligible = engine.eligible_workspaces(&binding, account_current);
    if !current() {
        return Ok(false);
    }
    if eligible.is_err() {
        // A Personal continuation must not repeat a failed Team proof.
        for runner in runners.values_mut() {
            runner.continue_discovery = false;
        }
    }
    let mut destinations = verified_team_turns(
        &policies,
        &session,
        &mut runners,
        eligible.as_deref().map_err(|error| *error),
        |status| {
            if current() {
                emit_status(app, status.with_device_upload_enabled(true));
            }
        },
    );
    if !regular_tick {
        destinations.retain(|policy| {
            runners
                .get(&(session.user.id.clone(), policy.destination_id.clone()))
                .is_some_and(Runner::discovery_continuation_due)
        });
    }
    if destinations.is_empty() {
        return Ok(false);
    }
    let mut cursor = controller
        .sync
        .team_cursor
        .lock()
        .map_err(|_| "Cloud sync is unavailable.")?;
    let policy = destinations
        .iter()
        .copied()
        .find(|policy| {
            cursor
                .as_ref()
                .is_none_or(|previous| &policy.destination_id > previous)
        })
        .unwrap_or(destinations[0]);
    if regular_tick {
        *cursor = Some(policy.destination_id.clone());
    }
    let Some(generation) = &policy.membership_generation else {
        return Ok(false);
    };
    let runner = runners
        .entry((session.user.id.clone(), policy.destination_id.clone()))
        .or_default();
    let team = (policy.destination_id.clone(), generation.clone());
    if runner.team.as_ref() != Some(&team) {
        *runner = Runner {
            team: Some(team),
            ..Runner::default()
        };
    }
    let proof = eligible.as_deref().ok().and_then(|entries| {
        TeamTurnProof::for_policy(&session, engine_generation, policy, entries)
    });
    let failed = runner
        .turn(
            store,
            host,
            engine,
            current,
            (account_current, proof),
            cache,
        )
        .is_err();
    let live = runner.report_status(store, host, engine, failed, account_current, |_| {});
    if current()
        && engine
            .session()
            .is_some_and(|current_anchor| current_anchor.same_grant(&session))
    {
        emit_status(
            app,
            team_policy_status(policy, Some(live), eligible.as_deref().ok())
                .with_device_upload_enabled(true),
        );
    }
    Ok(!failed && runner.discovery_continuation_due())
}

/// A failed lookup is not evidence of removal. Keep scheduling state only for
/// the saved Team binding, but do not return any Team to run without fresh proof.
fn verified_team_turns<'a>(
    policies: &'a [Policy],
    session: &auth::Session,
    runners: &mut BTreeMap<(String, String), Runner>,
    eligible: Result<&[auth::EligibleWorkspace], auth::Failure>,
    mut publish: impl FnMut(Status),
) -> Vec<&'a Policy> {
    let destinations = policies
        .iter()
        .filter(|policy| {
            !policy.paused
                && has_folders(policy)
                && eligible.is_ok_and(|entries| team_has_proof(policy, entries))
        })
        .collect::<Vec<_>>();
    runners.retain(|(account, id), runner| {
        if account != &session.user.id {
            return false;
        }
        if destinations
            .iter()
            .any(|policy| &policy.destination_id == id)
        {
            return true;
        }
        if let Some(policy) = policies.iter().find(|policy| &policy.destination_id == id) {
            if let Err(error) = eligible
                && !policy.paused
                && has_folders(policy)
                && runner.team.as_ref().is_some_and(|(team, generation)| {
                    team == id && Some(generation) == policy.membership_generation.as_ref()
                })
            {
                let live = runner.last_status.clone().filter(|_| {
                    runner.last_status_origin.as_deref() == Some(&session.resource)
                        && runner.has_retry_grant(session, id)
                });
                let status = runner.backfill_status(
                    team_lookup_failed(policy, live, error),
                    Some(&session.grant_id),
                );
                runner.last_status = Some(status.clone());
                publish(status);
                return true;
            }
            publish(team_policy_status(policy, None, eligible.ok()));
        }
        false
    });
    destinations
}

fn team_has_proof(policy: &Policy, entries: &[auth::EligibleWorkspace]) -> bool {
    policy
        .membership_generation
        .as_ref()
        .is_some_and(|generation| {
            entries.iter().any(|entry| {
                entry.workspace.kind == auth::WorkspaceKind::Team
                    && entry.workspace.id == policy.destination_id
                    && entry.membership_generation.as_ref() == Some(generation)
            })
        })
}

/// Reuse the selected Team's membership only within the turn that fetched it.
struct TeamTurnProof<'a> {
    session: &'a auth::Session,
    engine_generation: u64,
    policy: &'a Policy,
}

impl<'a> TeamTurnProof<'a> {
    /// Create proof only for the selected destination and membership generation.
    fn for_policy(
        session: &'a auth::Session,
        engine_generation: u64,
        policy: &'a Policy,
        entries: &[auth::EligibleWorkspace],
    ) -> Option<Self> {
        (policy.account_id == session.user.id
            && policy.resource_origin.as_deref() == Some(session.resource.as_str())
            && policy.issuer_origin.as_deref() == Some(session.iss.as_str())
            && team_has_proof(policy, entries))
        .then_some(Self {
            session,
            engine_generation,
            policy,
        })
    }

    /// Check the grant and exact route again after refresh and policy reload.
    fn matches(&self, session: &auth::Session, engine_generation: u64, policy: &Policy) -> bool {
        self.engine_generation == engine_generation
            && self.session.same_grant(session)
            && self.policy.account_id == policy.account_id
            && self.policy.resource_origin == policy.resource_origin
            && self.policy.issuer_origin == policy.issuer_origin
            && self.policy.destination_id == policy.destination_id
            && self.policy.membership_generation == policy.membership_generation
    }
}

/// Do not present retained counters as authority to retry after a failed lookup.
fn team_lookup_failed(policy: &Policy, live: Option<Status>, error: auth::Failure) -> Status {
    let mut status = team_policy_status(policy, live, None);
    status.status = match error {
        auth::Failure::Offline | auth::Failure::OfflineBeforeSend => State::Offline,
        _ => State::ReauthRequired,
    };
    status.retry_available = false;
    status.backfill_in_progress = false;
    status.full_sync_in_progress = false;
    status.background_sync_in_progress = false;
    status.upload_progress = None;
    status.background_progress = None;
    status.upload_transfer = None;
    status
}

/// Read saved Team consent and recovery for the unexpired Personal principal.
/// Explicit status reads can check eligibility. No provider files are read.
fn saved_team_statuses(
    store: &Store,
    anchor: Option<&auth::Session>,
    runners: &BTreeMap<(String, String), Runner>,
    installation: (&mut Engine, &str),
    current: Option<&dyn Fn() -> bool>,
) -> Result<Vec<Status>, String> {
    let Some(anchor) =
        anchor.filter(|session| session.workspace.kind == auth::WorkspaceKind::Personal)
    else {
        return Ok(Vec::new());
    };
    let policies = store
        .saved_team_sync_policies(
            &anchor.user.id,
            &anchor.workspace.id,
            &anchor.resource,
            &anchor.iss,
        )
        .map_err(|_| "Could not read saved team consent.")?;
    let eligible = current
        .filter(|_| policies.iter().any(has_folders))
        .map(|current| {
            let binding = DestinationBinding {
                account_id: anchor.user.id.clone(),
                destination_id: anchor.workspace.id.clone(),
            };
            installation.0.eligible_workspaces(&binding, current)
        });
    let registered = store
        .cloud_workspace_slots(&anchor.iss, &anchor.resource)
        .map_err(|_| "Could not read saved workspace connections.")?;
    let mut statuses: Vec<_> = policies
        .iter()
        .map(|policy| {
            let key = (policy.account_id.clone(), policy.destination_id.clone());
            let live = runners
                .get(&key)
                .filter(|runner| {
                    runner.last_status_origin.as_deref() == Some(&anchor.resource)
                        && runner.has_retry_grant(anchor, &policy.destination_id)
                        && runner.team.as_ref().is_some_and(|(id, generation)| {
                            id == &policy.destination_id
                                && Some(generation) == policy.membership_generation.as_ref()
                        })
                })
                .and_then(|runner| runner.last_status.clone());
            let mut status = match &eligible {
                Some(Ok(entries)) => {
                    let live = live.filter(|_| team_has_proof(policy, entries));
                    team_policy_status(policy, live, Some(entries))
                }
                Some(Err(error)) => team_lookup_failed(policy, live, *error),
                None => team_policy_status(policy, live, None),
            };
            status.recovery = Some(if registered.contains(&key) {
                super::workspace_grants::recovery(installation.0, installation.1, &key)
            } else {
                Default::default()
            });
            status
        })
        .collect();
    // The browser can lose its callback before the user saves any folder consent.
    // Report recovery for that slot without creating or implying a policy.
    for key in registered {
        if key.0 != anchor.user.id
            || key.1 == anchor.workspace.id
            || policies.iter().any(|policy| policy.destination_id == key.1)
        {
            continue;
        }
        let recovery = super::workspace_grants::recovery(installation.0, installation.1, &key);
        if recovery.local_reset_available || recovery.remote_revocation_unconfirmed {
            statuses.push(Status {
                status: State::ReauthRequired,
                account_id: Some(key.0),
                destination_id: Some(key.1),
                recovery: Some(recovery),
                ..Status::default()
            });
        }
    }
    Ok(statuses)
}

/// Notify only account surfaces. Other windows do not receive local project paths.
fn emit_status(app: &tauri::AppHandle, status: Status) -> Status {
    for window in ["onboarding", "main", "settings"] {
        let _ = app.emit_to(window, "cloud:sync-changed", status.clone());
    }
    status
}

impl Runner {
    /// Continue only a drained slice with an unread source. A control change can
    /// clear the cursor between the turn that requested this and the short tick.
    fn discovery_continuation_due(&self) -> bool {
        self.continue_discovery
            && self.discovery_cursor.is_some()
            && self.candidates.is_empty()
            && self.state != State::StorageUnavailable
    }

    fn has_retry_grant(&self, session: &auth::Session, destination: &str) -> bool {
        self.retry_grant.as_ref().is_some_and(|grant| {
            grant.account_id == session.user.id
                && grant.destination_id == destination
                && grant.grant_id == session.grant_id
        })
    }

    /// Remember the first grant before a hold can stop reconciliation.
    /// A later grant on this route stays pending until saved jobs are reconciled.
    fn observe_retry_grant(&mut self, session: &auth::Session) -> bool {
        let destination = self
            .team
            .as_ref()
            .map_or(&session.workspace.id, |(id, _)| id);
        if let Some(grant) = &self.retry_grant
            && grant.account_id == session.user.id
            && &grant.destination_id == destination
        {
            return grant.grant_id != session.grant_id;
        }
        let grant = RetryGrant {
            account_id: session.user.id.clone(),
            destination_id: destination.clone(),
            grant_id: session.grant_id.clone(),
        };
        if self.retry_grant.is_some() {
            // Do not carry scheduling hints from another account or destination.
            *self = Self {
                team: self.team.take(),
                ..Self::default()
            };
        }
        self.retry_grant = Some(grant);
        false
    }

    /// Clear retry timers only for saved jobs admitted by the current policy.
    fn retry(&mut self, saved: &BTreeSet<String>) -> State {
        if !saved.is_empty() {
            self.retry_deadlines.retain(|id, _| !saved.contains(id));
            self.conflict_attempts.retain(|id, _| !saved.contains(id));
            self.finalization_submissions
                .retain(|id| !saved.contains(id));
            self.conflict_finalization_replays
                .retain(|id, _| !saved.contains(id));
            self.next_integrity_poll = None;
            self.route_denied_until = None;
            self.state = State::Ready;
        }
        self.state
    }

    /// Start at the first retained source key, regardless of the normal sweep
    /// position. A second click with the same policy and grant keeps the page.
    fn schedule_backfill(&mut self, policy: &Policy, grant_id: &str) {
        if self.backfill_revision.as_deref() == Some(&policy.revision)
            && self
                .backfill_progress
                .as_ref()
                .is_some_and(|progress| progress.matches_grant(grant_id))
        {
            return;
        }
        self.backfill_revision = Some(policy.revision.clone());
        self.backfill_progress = Some(BackfillProgress::new(policy, grant_id));
        self.full_sync_pending = true;
        self.background_progress = None;
        self.background_page = false;
        self.discovery_unavailable = false;
        self.backfill_page = false;
        self.backfill_discovery_due = false;
        self.candidates.clear();
        self.discovery_cursor = None;
        self.next_reconcile = None;
    }

    /// Return the current backfill progress without treating Retry as a new scan.
    fn retry_status(
        &mut self,
        saved: &BTreeSet<String>,
        grant_id: &str,
        read_status: impl FnOnce(State, Option<&mut Self>) -> Result<Status, String>,
    ) -> Result<Status, String> {
        let state = self.retry(saved);
        let status = read_status(state, Some(self))?;
        Ok(self.backfill_status(status, Some(grant_id)))
    }

    /// Start a full normal refresh so an unresolved source cannot be behind its cursor.
    /// Keep the cursor of an explicit scan until it finishes.
    fn request_backfill_refresh(&mut self) {
        self.next_reconcile = None;
        if self.backfill_revision.is_none() && !self.backfill_page {
            self.discovery_cursor = None;
        }
    }

    fn can_probe_saved_upload(&self, capture_id: &str, now: Instant) -> bool {
        self.has_saved_probe_budget(capture_id)
            && self
                .retry_deadlines
                .get(capture_id)
                .is_none_or(|(deadline, _)| *deadline <= now)
    }

    fn has_saved_probe_budget(&self, capture_id: &str) -> bool {
        self.conflict_attempts.get(capture_id).copied().unwrap_or(0) < 3
    }

    /// Count saved work with an automatic recovery path, including future
    /// retry deadlines. An ordinary queued capture is not recovery.
    fn will_recover_automatically(
        &self,
        job: &Job,
        entry: Option<&QueueEntry>,
        state: State,
    ) -> bool {
        if job.state == JobState::Recovering {
            return saved_retry(job, entry) == Some(SavedRetry::RecoverEmpty);
        }
        if matches!(state, State::RouteDenied | State::UploadUnavailable) {
            return false;
        }
        let waiting_for_capacity = match job.state {
            JobState::WaitingForCapacity => entry.is_some_and(|entry| {
                entry.state == CaptureState::Preparing && empty_preparation(entry)
            }),
            JobState::WaitingForLocalCapacity => entry.is_none(),
            _ => false,
        };
        if waiting_for_capacity
            && job.reservation_retry_at.is_some()
            && job.recorded_project_paths.is_some()
        {
            return true;
        }
        let Some(entry) = entry else { return false };
        let capture_id = &job.identity.capture_id;
        let retry = self
            .retry_deadlines
            .get(capture_id)
            .map(|(_, reason)| *reason);
        let transient = matches!(
            retry,
            Some(
                upload::Failure::Offline
                    | upload::Failure::Backpressure
                    | upload::Failure::Conflict
            )
        );
        job.state != JobState::Interrupted
            && self.has_saved_probe_budget(capture_id)
            && !self.unsupported_saved_uploads.contains(capture_id)
            && !matches!(
                retry,
                Some(
                    upload::Failure::Unavailable
                        | upload::Failure::Unauthorized
                        | upload::Failure::Rejected
                )
            )
            && (entry.state == CaptureState::AwaitingIntegrity
                || (entry.state == CaptureState::Queued
                    && (job.state == JobState::Fenced || transient)))
    }

    fn defer_conflict(&mut self, capture_id: &str) {
        let attempts = self
            .conflict_attempts
            .entry(capture_id.to_owned())
            .or_default();
        *attempts = attempts.saturating_add(1);
        self.retry_deadlines.insert(
            capture_id.to_owned(),
            (
                Instant::now() + Duration::from_secs(60),
                upload::Failure::Conflict,
            ),
        );
    }

    /// Limit capture requests for this destination without pausing the device.
    fn defer_route_denial(&mut self, retry_after: Option<Duration>) {
        let delay = retry_after.map_or(ROUTE_DENIAL_RETRY_DELAY, |hint| {
            hint.max(ROUTE_DENIAL_RETRY_DELAY)
        });
        self.route_denied_until = Some(Instant::now() + delay);
        self.state = State::RouteDenied;
    }

    /// Fill only an empty queue. Partial sweeps continue on the next available
    /// turn; completed sweeps use the normal refresh interval.
    fn refresh_candidates(
        &mut self,
        policy: &Policy,
        current: &dyn Fn() -> bool,
        cache: &mut super::projects::Cache,
    ) -> DiscoveryRefresh {
        if !current() {
            return DiscoveryRefresh::Unchanged;
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
            self.backfill_page = false;
            self.next_reconcile = None;
            self.next_reservation = None;
            self.discovery_unavailable = false;
            self.binding = Some(binding);
            if self
                .backfill_progress
                .as_ref()
                .is_some_and(|progress| !progress.matches(policy))
            {
                self.backfill_progress = None;
                self.full_sync_pending = false;
            }
            if self
                .background_progress
                .as_ref()
                .is_some_and(|progress| !progress.matches(policy))
            {
                self.background_progress = None;
                self.background_page = false;
            }
        }
        if self.backfill_revision.as_deref() != Some(&policy.revision) {
            self.backfill_revision = None;
            self.backfill_page = false;
        }
        self.finish_backfill_scan();
        if !self.candidates.is_empty()
            || self
                .next_reconcile
                .is_some_and(|time| time > Instant::now())
        {
            return DiscoveryRefresh::Unchanged;
        }
        if self.discovery_cursor.is_none() {
            self.discovery_blocked = false;
            self.unsupported_format = false;
            self.start_background_scan(policy);
        }
        let cursor = self.discovery_cursor.clone().unwrap_or_default();
        match discover_policy_page(policy, current, cache, &cursor) {
            Ok((candidates, next_cursor, coverage)) => {
                self.discovery_unavailable = false;
                let slice_advanced = coverage.opencode_slice_yielded
                    && next_cursor.as_ref().is_some_and(|next| next != &cursor);
                self.backfill_page = self.backfill_revision.is_some();
                self.background_page =
                    !self.full_sync_pending && self.background_progress.is_some();
                if self.backfill_page
                    && let Some(progress) = &mut self.backfill_progress
                {
                    if coverage.partial || coverage.blocked {
                        progress.record_decision("discovery_page_incomplete");
                        progress.mark_incomplete();
                    } else {
                        progress.record_decision("discovery_page_complete");
                    }
                }
                if self.background_page
                    && let Some(progress) = &mut self.background_progress
                    && (coverage.partial || coverage.blocked)
                {
                    progress.mark_incomplete();
                }
                tracing::debug!(
                    event = "cloud_sync_discovery_page",
                    backfill = self.backfill_page,
                    selected = candidates.len(),
                    has_next_page = next_cursor.is_some(),
                    blocked = coverage.blocked,
                    partial = coverage.partial
                );
                self.candidates = candidates.into();
                self.discovery_cursor = next_cursor;
                self.discovery_blocked |= coverage.blocked;
                self.next_reconcile = self
                    .discovery_cursor
                    .is_none()
                    .then(|| Instant::now() + Duration::from_secs(60));
                if slice_advanced {
                    DiscoveryRefresh::YieldedOpenCodeSlice
                } else {
                    DiscoveryRefresh::ReadPage
                }
            }
            Err(_) if !current() => DiscoveryRefresh::Unchanged,
            Err(_) => {
                self.discovery_unavailable = true;
                self.backfill_revision = None;
                self.backfill_page = false;
                self.discovery_blocked = true;
                if let Some(progress) = &mut self.backfill_progress {
                    progress.abort("discovery_unavailable");
                }
                self.background_page = false;
                if let Some(progress) = &mut self.background_progress {
                    progress.abort("discovery_unavailable");
                }
                self.next_reconcile = Some(Instant::now() + Duration::from_secs(60));
                DiscoveryRefresh::Unchanged
            }
        }
    }

    /// Record a problem only in runs that already counted the source.
    fn observe_backfill_problem(&mut self, source_id: &str, problem: Option<SourceProblem>) {
        for progress in self.progress_trackers() {
            progress.observe_problem(source_id, problem);
        }
    }

    /// Reconcile receipts locally even after logout. Network and source work need
    /// the current account and a separate, exact destination policy.
    fn turn(
        &mut self,
        store: &Store,
        host: &mut capture::Host,
        engine: &mut Engine,
        current: &dyn Fn() -> bool,
        authority: (&dyn Fn() -> bool, Option<TeamTurnProof<'_>>),
        cache: &mut super::projects::Cache,
    ) -> Result<(), String> {
        let (account_current, team_proof) = authority;
        self.continue_discovery = false;
        let device_enabled = store
            .cloud_device_upload_enabled()
            .map_err(|_| "Could not read Cloud upload controls.")?;
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
                self.observe_accepted_transfer(&job, entry)?;
                self.observe_backfill_acceptance(store, &job);
                host.spool()?
                    .acknowledge_accepted(&entry.identity)
                    .map_err(|_| "Could not acknowledge accepted capture state.")?;
            }
        }
        let account_is_current = account_current();
        if !account_is_current || !current() {
            self.state = if account_is_current {
                State::Paused
            } else {
                State::ReauthRequired
            };
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
        let session = engine.session().cloned();
        let new_sign_in = session
            .as_ref()
            .is_some_and(|session| self.observe_retry_grant(session));
        if let Some(session) = &session
            && self
                .backfill_progress
                .as_ref()
                .is_some_and(|progress| !progress.matches_grant(&session.grant_id))
        {
            self.backfill_progress = None;
            self.full_sync_pending = false;
            self.backfill_revision = None;
            self.backfill_page = false;
            self.backfill_discovery_due = false;
            self.candidates.clear();
            self.discovery_cursor = None;
            self.next_reconcile = None;
        }
        if let Some(session) = &session
            && self
                .background_progress
                .as_ref()
                .is_some_and(|progress| !progress.matches_grant(&session.grant_id))
        {
            self.background_progress = None;
            self.background_page = false;
        }
        // Auth and acknowledged receipts are independent of upload admission.
        if !device_enabled || !current() {
            self.state = State::Paused;
            return Ok(());
        }
        cache.begin_scan();
        let Some(session) = session else {
            self.state = State::ReauthRequired;
            self.candidates.clear();
            self.discovery_cursor = None;
            self.next_reconcile = None;
            return Ok(());
        };
        let (destination, generation) = self
            .team
            .as_ref()
            .map_or((session.workspace.id.as_str(), None), |(id, generation)| {
                (id.as_str(), Some(generation.as_str()))
            });
        let Some(policy) = store
            .cloud_sync_policy_for_origins(
                &session.user.id,
                destination,
                &session.resource,
                &session.iss,
            )
            .map_err(|_| "Could not read Cloud sync consent.")?
        else {
            self.state = State::ConsentRequired;
            self.candidates.clear();
            self.discovery_cursor = None;
            self.next_reconcile = None;
            self.next_reservation = None;
            return Ok(());
        };
        if generation != policy.membership_generation.as_deref()
            || policy.is_team() != generation.is_some()
            || policy.team_grant_id.is_some()
        {
            self.state = State::Fenced;
            self.candidates.clear();
            return Ok(());
        }
        let policy = repository_policy::try_migration(
            store,
            policy,
            &mut self.repository_migration,
            current,
        );
        if policy.paused || !current() {
            self.state = State::Paused;
            self.candidates.clear();
            self.discovery_cursor = None;
            if policy.paused {
                self.backfill_revision = None;
                self.backfill_page = false;
            }
            self.next_reconcile = None;
            self.next_reservation = None;
            return Ok(());
        }
        if let Some(generation) = policy.membership_generation.as_deref()
            && !team_proof.is_some_and(|proof| proof.matches(&session, engine.generation, &policy))
            && !eligible_team(
                engine,
                &policy.account_id,
                &policy.destination_id,
                account_current,
            )
            .is_ok_and(|(live_generation, _)| live_generation == generation)
        {
            self.state = State::ReauthRequired;
            return Ok(());
        }
        if !current() {
            self.state = State::Paused;
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
        jobs.retain(|job| {
            job.issuer_origin == policy.issuer_origin
                && job.membership_generation == policy.membership_generation
                && job.team_grant_id.is_none()
        });
        restore_jobs(store, &mut jobs, &entries)?;
        // Finish one saved recovery decision per turn. Do not treat other
        // missing entries as empty captures.
        if let Some(job) = jobs.iter().find(|job| {
            job.state == JobState::Recovering
                && job.allowed(&policy)
                && saved_retry(
                    job,
                    entries.iter().find(|entry| entry.identity == job.identity),
                ) == Some(SavedRetry::RecoverEmpty)
        }) && current()
        {
            let entry = entries.iter().find(|entry| entry.identity == job.identity);
            finish_empty_recovery(store, host.spool()?, job, entry)?;
            return Ok(());
        }
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
            } else if new_sign_in
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
        // Keep the grant change pending until this route reconciles its jobs.
        if new_sign_in {
            if let Some(grant) = &mut self.retry_grant {
                grant.grant_id = session.grant_id.clone();
            }
            self.retry_deadlines
                .retain(|_, (_, reason)| *reason != upload::Failure::Conflict);
            self.conflict_attempts.clear();
            self.finalization_submissions.clear();
            self.conflict_finalization_replays.clear();
            self.next_integrity_poll = None;
            self.route_denied_until = None;
            tracing::info!(
                event = "cloud_sync_sign_in_changed",
                conflict_rechecks_cleared = true
            );
        }
        self.retry_deadlines
            .retain(|id, _| jobs.iter().any(|job| &job.identity.capture_id == id));
        self.unsupported_saved_uploads
            .retain(|id| jobs.iter().any(|job| &job.identity.capture_id == id));
        self.conflict_attempts
            .retain(|id, _| jobs.iter().any(|job| &job.identity.capture_id == id));
        self.finalization_submissions
            .retain(|id| jobs.iter().any(|job| &job.identity.capture_id == id));
        self.conflict_finalization_replays
            .retain(|id, _| jobs.iter().any(|job| &job.identity.capture_id == id));
        let now_instant = Instant::now();
        if self
            .route_denied_until
            .is_some_and(|deadline| deadline > now_instant)
        {
            self.state = State::RouteDenied;
            return Ok(());
        }
        self.route_denied_until = None;
        let backfill_active = self.backfill_revision.as_deref() == Some(&policy.revision);
        let can_poll_integrity = !(backfill_active && self.backfill_discovery_due)
            && self
                .next_integrity_poll
                .is_none_or(|deadline| deadline <= now_instant);
        let pending = entries
            .iter()
            .filter(|entry| {
                entry.identity.account_id == policy.account_id
                    && entry.identity.destination_id == policy.destination_id
                    && matches!(
                        entry.state,
                        CaptureState::Queued | CaptureState::AwaitingIntegrity
                    )
                    && (entry.state != CaptureState::AwaitingIntegrity || can_poll_integrity)
                    && jobs.iter().any(|job| {
                        !matches!(job.state, JobState::Recovering | JobState::Interrupted)
                            && self.can_probe_saved_upload(&job.identity.capture_id, now_instant)
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
            if entry.state == CaptureState::Queued
                && !repository_capture::saved_job_origin_current(job)
            {
                store
                    .set_cloud_sync_job_state(job, JobState::Fenced)
                    .map_err(|_| "Could not save the changed repository state.")?;
                self.retry_deadlines.insert(
                    job.identity.capture_id.clone(),
                    (
                        now_instant + ROUTE_DENIAL_RETRY_DELAY,
                        upload::Failure::Unavailable,
                    ),
                );
                self.state = State::Fenced;
                return Ok(());
            }
            let origin = job
                .resource_origin
                .as_deref()
                .ok_or("Confirm Cloud sync for this service.")?;
            if let Some(root) = policy.roots.iter().find(|root| {
                root.source_scope_id == job.source_scope_id && root.source_kind.is_project_only()
            }) {
                let capability = upload::capabilities(
                    engine,
                    &policy.account_id,
                    (&policy.destination_id, job.membership_generation.as_deref()),
                    account_current,
                    current,
                );
                if !capability
                    .as_ref()
                    .is_ok_and(|caps| caps.supports(root.source_kind))
                {
                    let unsupported = capability.is_ok();
                    if unsupported {
                        self.unsupported_saved_uploads
                            .insert(job.identity.capture_id.clone());
                    } else {
                        self.unsupported_saved_uploads
                            .remove(&job.identity.capture_id);
                    }
                    let failure = capability.err();
                    let kind = failure
                        .as_ref()
                        .map_or(upload::Failure::Unavailable, |err| err.failure);
                    if !unsupported
                        && matches!(
                            kind,
                            upload::Failure::Unavailable
                                | upload::Failure::Unauthorized
                                | upload::Failure::Rejected
                        )
                    {
                        self.discovery_unavailable = true;
                    }
                    if kind == upload::Failure::RouteDenied {
                        self.defer_route_denial(failure.and_then(|err| err.retry_after));
                        return Ok(());
                    }
                    let delay = failure
                        .and_then(|err| err.retry_after)
                        .unwrap_or(RESERVATION_RETRY_DELAY);
                    self.retry_deadlines.insert(
                        job.identity.capture_id.clone(),
                        (Instant::now() + delay, kind),
                    );
                    self.observe_backfill_problem(
                        &job.identity.source_id,
                        if unsupported {
                            Some(SourceProblem::Unsupported)
                        } else {
                            (kind == upload::Failure::Unavailable).then_some(SourceProblem::Blocked)
                        },
                    );
                    self.state = transport_state(Some(kind), &Ok(()));
                    return Ok(());
                }
                self.retry_deadlines.remove(&job.identity.capture_id);
                self.unsupported_saved_uploads
                    .remove(&job.identity.capture_id);
            }
            let source_authority = || repository_capture::saved_job_origin_current(job);
            let mut transport =
                upload::HttpTransport::new(engine, account_current, current, origin, None)
                    .with_source_authority(&source_authority)
                    .with_membership_generation(job.membership_generation.as_deref());
            let started = Instant::now();
            let mut result = host.spool()?.resume_step(&entry.identity, &mut transport);
            if result.is_err() && !current() {
                // Keep the saved job resumable when local admission closes.
                self.state = State::Paused;
                return Ok(());
            }
            let saved_replay_attempts = self
                .conflict_finalization_replays
                .get(&entry.identity.capture_id)
                .copied();
            let replay_attempts = saved_replay_attempts.unwrap_or_default();
            let status_conflict = transport.status_renewal_conflict;
            let status_pending = result.as_ref().is_ok_and(|step| {
                step.entry.state == CaptureState::AwaitingIntegrity
                    && step.entry.completion_submitted
            });
            if let Some(progress) = &mut self.backfill_progress {
                if status_conflict {
                    progress.record_decision("saved_status_and_renewal_conflict");
                }
                if status_pending {
                    progress.record_decision("saved_integrity_pending");
                }
            }
            if backfill_active && (entry.state == CaptureState::AwaitingIntegrity || status_pending)
            {
                self.backfill_discovery_due = true;
            }
            if (status_conflict || (saved_replay_attempts.is_some() && status_pending))
                && entry.state == CaptureState::AwaitingIntegrity
                && let (Some(reservation), Some(manifest), Some(plan)) =
                    (&entry.reservation_id, &entry.manifest, &entry.upload_plan)
                && current()
                && replay_attempts < FINALIZATION_PROBE_LIMIT
            {
                let replay = transport.replay_saved_finalization(reservation, manifest, plan);
                if let Some(progress) = &mut self.backfill_progress {
                    progress.record_decision(if replay.is_ok() {
                        "conflict_finalize_replay_submitted"
                    } else {
                        "conflict_finalize_replay_failed"
                    });
                }
                let transient = matches!(
                    transport.failure,
                    Some(upload::Failure::Offline | upload::Failure::Backpressure)
                );
                let route_denied = transport.failure == Some(upload::Failure::RouteDenied);
                let attempts = if route_denied {
                    replay_attempts
                } else if transient {
                    replay_attempts + 1
                } else {
                    FINALIZATION_PROBE_LIMIT
                };
                self.conflict_finalization_replays
                    .insert(entry.identity.capture_id.clone(), attempts);
                tracing::info!(
                    event = "cloud_saved_finalization_replayed_after_status_conflict",
                    succeeded = replay.is_ok(),
                    failure = ?transport.failure,
                    transient,
                    attempts,
                );
                if attempts == FINALIZATION_PROBE_LIMIT {
                    self.finalization_submissions
                        .insert(entry.identity.capture_id.clone());
                }
                if (transient
                    || route_denied
                    || matches!(transport.failure, Some(upload::Failure::Unauthorized)))
                    && let Err(error) = replay
                {
                    // Preserve the finalize failure instead of the earlier status result.
                    result = Err(antiburn_cloud_capture::Error::Transport(error));
                } else if status_conflict {
                    // Even HTTP 202 does not prove acceptance.
                    transport.failure = Some(upload::Failure::Conflict);
                } else {
                    transport.failure = None;
                }
            }
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
                    if transport.failure == Some(upload::Failure::RouteDenied) {
                        self.finalization_submissions
                            .remove(&entry.identity.capture_id);
                    } else if matches!(
                        transport.failure,
                        Some(upload::Failure::Offline | upload::Failure::Backpressure)
                    ) {
                        let attempts = replay_attempts.saturating_add(1);
                        self.conflict_finalization_replays
                            .insert(entry.identity.capture_id.clone(), attempts);
                        if attempts < FINALIZATION_PROBE_LIMIT {
                            self.finalization_submissions
                                .remove(&entry.identity.capture_id);
                        }
                    } else {
                        self.conflict_finalization_replays
                            .insert(entry.identity.capture_id.clone(), FINALIZATION_PROBE_LIMIT);
                    }
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
            if result.is_err()
                && let Some(progress) = &mut self.backfill_progress
            {
                progress.record_decision(match transport.failure {
                    Some(upload::Failure::Offline) => "saved_upload_offline",
                    Some(upload::Failure::Backpressure) => "saved_upload_capacity",
                    Some(upload::Failure::Conflict) => "saved_upload_conflict",
                    Some(upload::Failure::RouteDenied) => "saved_upload_denied",
                    _ => "saved_upload_stopped",
                });
            }
            if transport.failure == Some(upload::Failure::RouteDenied) {
                self.defer_route_denial(transport.retry_after);
                return Ok(());
            }
            if result.is_ok() {
                self.observe_backfill_problem(&job.identity.source_id, None);
                self.conflict_attempts.remove(&job.identity.capture_id);
                self.retry_deadlines.remove(&job.identity.capture_id);
                if status_pending {
                    // Pace the whole runner so many pending captures cannot
                    // fill every turn with status checks.
                    self.next_integrity_poll = Some(Instant::now() + INTEGRITY_POLL_DELAY);
                }
            }
            if result.is_ok() && job.state == JobState::Fenced && current() {
                store
                    .set_cloud_sync_job_state(job, JobState::Queued)
                    .map_err(|_| "Could not save recovered capture progress.")?;
            }
            if result.is_err()
                && self.state == State::Fenced
                && let Some(job) = jobs.iter().find(|job| job.identity == entry.identity)
            {
                store
                    .set_cloud_sync_job_state(job, JobState::Fenced)
                    .map_err(|_| "Could not save fenced capture state.")?;
                self.defer_conflict(&job.identity.capture_id);
                self.observe_backfill_problem(
                    &job.identity.source_id,
                    Some(SourceProblem::Blocked),
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
                self.observe_backfill_problem(
                    &job.identity.source_id,
                    Some(SourceProblem::Blocked),
                );
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
                    conflict_probe_limit_reached = self
                        .conflict_attempts
                        .get(&job.identity.capture_id)
                        .is_some_and(|attempts| *attempts >= 3),
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
                self.observe_accepted_transfer(job, &step.entry)?;
                self.observe_backfill_acceptance(store, job);
                host.spool()?
                    .acknowledge_accepted(&step.entry.identity)
                    .map_err(|_| "Could not acknowledge accepted capture state.")?;
            }
            return Ok(());
        }
        let discovery = self.refresh_candidates(&policy, current, cache);
        if !matches!(discovery, DiscoveryRefresh::Unchanged)
            && let Some(progress) = &mut self.backfill_progress
        {
            progress.observe_page(store, &self.candidates, !self.backfill_page);
            if !self.backfill_page && self.discovery_cursor.is_none() {
                progress.finish_refresh();
            }
        }
        if !matches!(discovery, DiscoveryRefresh::Unchanged)
            && self.background_page
            && let Some(progress) = &mut self.background_progress
        {
            progress.observe_page(store, &self.candidates, false);
        }
        if current() {
            self.backfill_discovery_due = false;
        }
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
                        repository_origin: job.repository_origin.clone(),
                        observation: job.observation_key.clone(),
                        folder_group_id: None,
                    });
                }
                Err(_) => {
                    store
                        .set_cloud_sync_job_state(job, JobState::Interrupted)
                        .map_err(|_| "Could not save the changed capture source state.")?;
                    self.state = State::Interrupted;
                    self.observe_backfill_problem(
                        &job.identity.source_id,
                        Some(SourceProblem::Blocked),
                    );
                    return Ok(());
                }
            }
        }
        let mut capabilities_for_turn = None;
        let mut file_refresh_due = false;
        while let Some(selection) = self.candidates.pop_front() {
            if !current() {
                return Ok(());
            }
            let saved = jobs
                .iter()
                .find(|job| job.identity.source_id == selection.candidate.source_id());
            if let Some(job) = saved
                && (!job.allowed(&policy)
                    || !matches!(
                        job.state,
                        JobState::Pending
                            | JobState::WaitingForCapacity
                            | JobState::WaitingForLocalCapacity
                    ))
            {
                let allowed = job.allowed(&policy);
                let blocked = matches!(
                    job.state,
                    JobState::Interrupted | JobState::Fenced | JobState::Recovering
                ) || self
                    .retry_deadlines
                    .get(&job.identity.capture_id)
                    .is_some_and(|(_, failure)| *failure == upload::Failure::Unavailable);
                let problem = if !allowed {
                    Some(SourceProblem::Blocked)
                } else if self
                    .unsupported_saved_uploads
                    .contains(&job.identity.capture_id)
                {
                    Some(SourceProblem::Unsupported)
                } else {
                    blocked.then_some(SourceProblem::Blocked)
                };
                self.observe_backfill_problem(&job.identity.source_id, problem);
                if self.backfill_page
                    && let Some(progress) = &mut self.backfill_progress
                {
                    progress.record_decision(match problem {
                        Some(SourceProblem::Blocked) => "saved_job_blocked",
                        Some(SourceProblem::Unsupported) => "saved_job_unsupported",
                        None => "saved_job_pending",
                    });
                }
                tracing::debug!(
                    event = "cloud_backfill_saved_job_decision",
                    backfill = self.backfill_page,
                    job_state = ?job.state,
                    policy_allows_job = allowed,
                    problem = ?problem,
                );
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
            let waiting = saved.filter(|job| {
                matches!(
                    job.state,
                    JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
                )
            });
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
                        || (waiting.is_none()
                            && source.observation_key.as_deref() == Some(&selection.observation))
                })
            {
                continue;
            }
            let capabilities = match &mut capabilities_for_turn {
                Some(capabilities) => capabilities,
                slot @ None => match upload::capabilities(
                    engine,
                    &policy.account_id,
                    (
                        &policy.destination_id,
                        policy.membership_generation.as_deref(),
                    ),
                    account_current,
                    current,
                ) {
                    Ok(capabilities) => {
                        self.discovery_unavailable = false;
                        slot.insert(capabilities)
                    }
                    Err(failure) if failure.failure == upload::Failure::RouteDenied => {
                        self.defer_route_denial(failure.retry_after);
                        self.candidates.push_front(selection);
                        return Ok(());
                    }
                    Err(failure) => {
                        self.state = match failure.failure {
                            upload::Failure::Backpressure | upload::Failure::Offline => {
                                self.next_reservation = Some(
                                    Instant::now()
                                        + failure.retry_after.unwrap_or(RESERVATION_RETRY_DELAY),
                                );
                                transport_state(Some(failure.failure), &Ok(()))
                            }
                            upload::Failure::Unauthorized => State::ReauthRequired,
                            _ => State::UploadUnavailable,
                        };
                        if matches!(self.state, State::UploadUnavailable | State::ReauthRequired) {
                            self.discovery_unavailable = true;
                            self.backfill_revision = None;
                            self.backfill_page = false;
                            if let Some(progress) = &mut self.backfill_progress {
                                progress.abort(if self.state == State::ReauthRequired {
                                    "capabilities_need_sign_in"
                                } else {
                                    "capabilities_unavailable"
                                });
                            }
                            self.background_page = false;
                            if let Some(progress) = &mut self.background_progress {
                                progress.abort("capabilities_unavailable");
                            }
                        }
                        self.candidates.push_front(selection);
                        return Ok(());
                    }
                },
            };
            if !capabilities.supports(selection.candidate.source_kind().into()) {
                self.unsupported_format = true;
                self.observe_backfill_problem(
                    selection.candidate.source_id(),
                    Some(SourceProblem::Unsupported),
                );
                continue;
            }
            let limits = capabilities.limits.clone();
            host.configure_limits(limits.clone())?;
            let folders = if let Some(job) = waiting {
                waiting_project_directories(job)
            } else if selection.repository_origin.is_some() {
                vec![Path::new(&selection.project).to_owned()]
            } else {
                exact_project_directories(
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
                        .map(|project| project.project_path.as_str()),
                )
            };
            let project_selection = antiburn_cloud_capture::ProjectSelection::new(folders);
            let projects = project_selection
                .as_ref()
                .map_err(|_| ())
                .and_then(|projects| {
                    projects
                        .admit_for_routing(&selection.candidate, &limits)
                        .map_err(|_| ())
                });
            if !current() {
                self.state = State::Paused;
                return Ok(());
            }
            let (candidate, recorded_paths) = match (projects, waiting) {
                (Ok(Some(candidate)), _) => candidate,
                (_, Some(job)) => {
                    store
                        .set_cloud_sync_job_state(job, JobState::Interrupted)
                        .map_err(|_| "Could not save the changed capture project state.")?;
                    self.state = State::Interrupted;
                    self.observe_backfill_problem(
                        &job.identity.source_id,
                        Some(SourceProblem::Blocked),
                    );
                    return Ok(());
                }
                (Ok(None), None) => {
                    let excluded = selection
                        .candidate
                        .project_evidence_with_limits(&limits)
                        .is_ok_and(|evidence| {
                            evidence.scan_complete()
                                && !evidence.project_paths().is_empty()
                                && evidence.project_paths().iter().all(|path| {
                                    path.to_str()
                                        .is_some_and(|path| !policy.allows(&selection.scope, path))
                                })
                        });
                    if !current() {
                        self.state = State::Paused;
                        return Ok(());
                    }
                    if excluded {
                        for progress in self.progress_trackers() {
                            progress.observe_exclusion(selection.candidate.source_id());
                        }
                        continue;
                    }
                    if self.backfill_page
                        && let Some(progress) = &mut self.backfill_progress
                    {
                        progress.mark_incomplete();
                    }
                    if let Some(progress) = &mut self.background_progress {
                        progress.mark_incomplete();
                    }
                    self.discovery_blocked = true;
                    self.observe_backfill_problem(
                        selection.candidate.source_id(),
                        Some(SourceProblem::Blocked),
                    );
                    continue;
                }
                (Err(_), None) => {
                    self.discovery_blocked = true;
                    self.observe_backfill_problem(
                        selection.candidate.source_id(),
                        Some(SourceProblem::Blocked),
                    );
                    tracing::debug!(
                        event = "cloud_sync_project_admission_failed",
                        backfill = self.backfill_page,
                        source_kind = ?selection.candidate.source_kind()
                    );
                    continue;
                }
            };
            if recorded_paths
                .iter()
                .map(|path| path.as_os_str().len())
                .sum::<usize>()
                > MAX_RECORDED_PROJECT_PATH_BYTES
            {
                self.discovery_blocked = true;
                self.observe_backfill_problem(candidate.source_id(), Some(SourceProblem::Blocked));
                continue;
            }
            if let Some(origin) = &selection.repository_origin
                && !repository_capture::recheck_repository_project(
                    &policy,
                    &selection.scope,
                    origin,
                    &selection.project,
                    &recorded_paths,
                )
            {
                if let Some(job) = waiting {
                    store
                        .set_cloud_sync_job_state(job, JobState::Interrupted)
                        .map_err(|_| "Could not save the changed capture project state.")?;
                }
                self.discovery_blocked = true;
                self.observe_backfill_problem(candidate.source_id(), Some(SourceProblem::Blocked));
                continue;
            }
            let recorded_paths = recorded_paths
                .iter()
                .map(|path| path.to_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or("Could not freeze the recorded workspace folders.")?;
            if let Some(job) = waiting
                && job.recorded_project_paths.as_ref() != Some(&recorded_paths)
            {
                store
                    .set_cloud_sync_job_state(job, JobState::Interrupted)
                    .map_err(|_| "Could not save the changed capture project state.")?;
                self.state = State::Interrupted;
                self.observe_backfill_problem(
                    &job.identity.source_id,
                    Some(SourceProblem::Blocked),
                );
                return Ok(());
            }
            if !current() {
                self.candidates.push_front(selection);
                self.state = State::Paused;
                return Ok(());
            }
            let mut job = if let Some(job) = saved {
                job.clone()
            } else {
                let queued = match &selection.repository_origin {
                    Some(origin) => store.enqueue_cloud_sync_for_repository(
                        &policy,
                        &selection.scope,
                        &selection.project,
                        selection.candidate.source_id(),
                        &selection.observation,
                        (origin, &recorded_paths),
                    ),
                    None => store.enqueue_cloud_sync_with_paths(
                        &policy,
                        &selection.scope,
                        &selection.project,
                        selection.candidate.source_id(),
                        &selection.observation,
                        Some(&recorded_paths),
                    ),
                };
                let Some(job) =
                    queued.map_err(|_| "Could not save a capture job. Saved bytes are kept.")?
                else {
                    continue;
                };
                job
            };
            let retry_reservation = job.state == JobState::WaitingForCapacity;
            let retry_observation = if waiting.is_some() {
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
                                check_retry_observation(
                                    &selection.candidate,
                                    &evidence,
                                    project_selection
                                        .as_ref()
                                        .map_err(|_| "Cloud project selection changed.")?,
                                    &job.observation_key,
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
                if let Ok(Some(observation)) = observation
                    && job.identity.source_id == candidate.source_id()
                {
                    Some(observation)
                } else {
                    store
                        .set_cloud_sync_job_state(&job, JobState::Interrupted)
                        .map_err(|_| "Could not save the changed capture source state.")?;
                    self.state = State::Interrupted;
                    self.observe_backfill_problem(
                        &job.identity.source_id,
                        Some(SourceProblem::Blocked),
                    );
                    return Ok(());
                }
            } else {
                None
            };
            let sections = git_checkout::capture_sections(&candidate, current, &|root| {
                root.to_str()
                    .is_some_and(|root| policy.allows(&selection.scope, root))
            });
            if !current() {
                self.candidates.push_front(selection);
                self.state = State::Paused;
                return Ok(());
            }
            if let Some(observation) = retry_observation {
                // Flush away capacity-only retry permission before reserve can run.
                store
                    .begin_cloud_sync_reservation_retry(&job, &observation)
                    .map_err(|_| "Could not save reservation retry intent.")?;
                job.state = JobState::Pending;
                job.reservation_retry_at = None;
                job.observation_key = observation;
            }
            if !current() {
                self.candidates.push_front(selection);
                self.state = State::Paused;
                return Ok(());
            }
            let source = upload::Source::from_candidate(&candidate);
            let source_authority = || repository_capture::saved_job_origin_current(&job);
            let mut transport = upload::HttpTransport::new(
                engine,
                account_current,
                current,
                job.resource_origin
                    .as_deref()
                    .ok_or("Confirm Cloud sync for this service.")?,
                Some(source),
            )
            .with_membership_generation(job.membership_generation.as_deref())
            .with_source_authority(&source_authority)
            .with_expected_file_generation(&job.observation_key);
            let result = if retry_reservation {
                host.spool()?.retry_reservation(
                    job.identity.clone(),
                    &candidate,
                    sections,
                    &mut transport,
                )
            } else {
                host.spool()?
                    .capture(job.identity.clone(), &candidate, sections, &mut transport)
            };
            if matches!(
                result,
                Err(antiburn_cloud_capture::Error::FileGenerationChanged)
            ) {
                if retry_reservation {
                    // The spool checked the empty entry and stopped before reserve.
                    let entry = entries
                        .iter()
                        .find(|entry| entry.identity == job.identity)
                        .ok_or("The empty capture is unavailable.")?;
                    store
                        .set_cloud_sync_job_state(&job, JobState::Recovering)
                        .map_err(|_| "Could not save empty capture recovery.")?;
                    job.state = JobState::Recovering;
                    finish_empty_recovery(store, host.spool()?, &job, Some(entry))?;
                } else {
                    store.abandon_cloud_sync_unreserved(&job).map_err(
                        |_| "Could not retire a job whose source changed before reservation.",
                    )?;
                }
                file_refresh_due = true;
                if self.full_sync_pending
                    && let Some(progress) = &mut self.backfill_progress
                    && progress.refresh_changed_source(&job.identity.source_id)
                {
                    self.request_backfill_refresh();
                }
                if let Some(progress) = &mut self.background_progress {
                    progress.refresh_changed_source(&job.identity.source_id);
                }
                tracing::debug!(
                    event = "cloud_sync_capture_decision",
                    backfill = self.backfill_page,
                    outcome = "file_generation_changed_before_reservation"
                );
                continue;
            }
            if transport.failure == Some(upload::Failure::SourceSuppressed) {
                store.suppress_cloud_sync(&job).map_err(
                    |_| "Could not save the Cloud deletion outcome. Saved captures are kept.",
                )?;
                for progress in self.progress_trackers() {
                    progress.observe_suppression(&job);
                }
                host.spool()?.cancel(&job.identity).map_err(
                    |_| "Cloud deletion is saved, but its empty local entry needs cleanup.",
                )?;
                self.state = State::Ready;
                return Ok(());
            }
            self.state = transport_state(transport.failure, &result);
            if transport.failure == Some(upload::Failure::RouteDenied) {
                self.defer_route_denial(transport.retry_after);
            }
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
                if let Some(progress) = &mut self.backfill_progress {
                    progress.record_decision("reservation_capacity");
                }
                let delay = transport.retry_after.unwrap_or(RESERVATION_RETRY_DELAY);
                self.next_reservation = Some(Instant::now() + delay);
                self.candidates.push_front(selection);
                // Round up persisted seconds so a restart cannot shorten the delay.
                let retry_at = now().saturating_add(delay.as_secs() as i64 + 1);
                store
                    .defer_cloud_sync_reservation(&job, retry_at)
                    .map_err(|_| "Could not save reservation backpressure.")?;
            } else if state == JobState::WaitingForLocalCapacity {
                if let Some(progress) = &mut self.backfill_progress {
                    progress.record_decision("spool_capacity");
                }
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
            self.observe_backfill_problem(
                &job.identity.source_id,
                (state == JobState::Interrupted).then_some(SourceProblem::Blocked),
            );
            tracing::debug!(
                event = "cloud_sync_capture_decision",
                backfill = self.backfill_page,
                job_state = ?state,
                succeeded = result.is_ok(),
                transport_failure = ?transport.failure
            );
            return Ok(());
        }
        self.state = if jobs.iter().any(|job| {
            job.allowed(&policy)
                && matches!(job.state, JobState::Interrupted | JobState::Recovering)
        }) {
            State::Interrupted
        } else if self.unsupported_format
            || jobs.iter().any(|job| {
                job.allowed(&policy)
                    && self
                        .retry_deadlines
                        .get(&job.identity.capture_id)
                        .is_some_and(|(_, reason)| *reason == upload::Failure::Unavailable)
            })
        {
            State::UploadUnavailable
        } else if self.discovery_blocked
            || jobs
                .iter()
                .any(|job| job.allowed(&policy) && job.state == JobState::Fenced)
        {
            State::Fenced
        } else if file_refresh_due
            || !can_poll_integrity
                && entries.iter().any(|entry| {
                    entry.state == CaptureState::AwaitingIntegrity
                        && jobs
                            .iter()
                            .any(|job| job.identity == entry.identity && job.allowed(&policy))
                })
            || jobs.iter().any(|job| {
                job.allowed(&policy)
                    && matches!(
                        job.state,
                        JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
                    )
            })
        {
            State::Syncing
        } else {
            State::Ready
        };
        self.continue_discovery = matches!(discovery, DiscoveryRefresh::YieldedOpenCodeSlice);
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
) -> Result<BTreeSet<String>, String> {
    if policy.paused {
        return Err("Confirm the changed Cloud folders before retrying saved work.".into());
    }
    let origin = policy
        .resource_origin
        .as_deref()
        .ok_or("Confirm Cloud sync for this service.")?;
    let entries = spool
        .entries()
        .map_err(|_| "Could not read saved captures.")?;
    let mut retried = BTreeSet::new();
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
        let action = saved_retry(&job, entry);
        if action == Some(SavedRetry::RecoverEmpty) {
            if job.state != JobState::Recovering {
                store
                    .set_cloud_sync_job_state(&job, JobState::Recovering)
                    .map_err(|_| "Could not save capture recovery intent.")?;
                job.state = JobState::Recovering;
            }
            finish_empty_recovery(store, spool, &job, entry)?;
            retried.insert(job.identity.capture_id);
        } else if action == Some(SavedRetry::Requeue) {
            store
                .set_cloud_sync_job_state(&job, JobState::Queued)
                .map_err(|_| "Could not retry saved capture state.")?;
            retried.insert(job.identity.capture_id);
        } else if action == Some(SavedRetry::ClearDelay) {
            retried.insert(job.identity.capture_id);
        }
    }
    Ok(retried)
}

/// Finish an already persisted decision using only the spool record, never source.
fn finish_empty_recovery(
    store: &Store,
    spool: &mut antiburn_cloud_capture::Spool,
    job: &Job,
    entry: Option<&QueueEntry>,
) -> Result<(), String> {
    if entry.is_some() {
        spool
            .cancel(&job.identity)
            .map_err(|_| "Could not finish empty capture recovery. Saved captures are kept.")?;
    }
    store
        .abandon_cloud_sync_preparing(job)
        .map_err(|_| "Could not finish capture recovery. Retry again.".into())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SavedRetry {
    RecoverEmpty,
    Requeue,
    ClearDelay,
}

/// Use the same saved evidence to offer Retry and to apply it.
fn saved_retry(job: &Job, entry: Option<&QueueEntry>) -> Option<SavedRetry> {
    match (job.state, entry) {
        (JobState::Recovering, None) => Some(SavedRetry::RecoverEmpty),
        (
            JobState::Interrupted | JobState::Pending | JobState::Recovering | JobState::Fenced,
            Some(entry),
        ) if empty_preparation(entry) => Some(SavedRetry::RecoverEmpty),
        (JobState::Fenced | JobState::Interrupted | JobState::Queued, Some(entry))
            if matches!(
                entry.state,
                CaptureState::Queued | CaptureState::AwaitingIntegrity
            ) =>
        {
            Some(if job.state == JobState::Queued {
                SavedRetry::ClearDelay
            } else {
                SavedRetry::Requeue
            })
        }
        _ => None,
    }
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

/// Retry a waiting capture with the same project directory binding.
fn waiting_project_directories(job: &Job) -> Vec<PathBuf> {
    if job.repository_origin.is_some() {
        vec![PathBuf::from(&job.project_path)]
    } else {
        exact_project_directories(job.project_folders.iter().map(String::as_str))
    }
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
        Some(upload::Failure::RouteDenied) => State::RouteDenied,
        Some(upload::Failure::Unavailable) => State::UploadUnavailable,
        Some(upload::Failure::Conflict) => State::Fenced,
        Some(upload::Failure::Rejected) => State::Interrupted,
        Some(upload::Failure::SourceSuppressed) => State::Ready,
        None if result.is_ok() => State::Syncing,
        None => State::Interrupted,
    }
}

fn status(
    store: &Store,
    host: &mut capture::Host,
    engine: &Engine,
    state: State,
) -> Result<Status, String> {
    status_for_destination(store, host, engine, state, None, None)
}

/// Read one destination and refresh its scan counters from the same spool snapshot.
/// An absent runner leaves scan state unchanged. This does not request upload work.
fn status_for_destination(
    store: &Store,
    host: &mut capture::Host,
    engine: &Engine,
    state: State,
    team: Option<&str>,
    runner: Option<&mut Runner>,
) -> Result<Status, String> {
    let Some(session) = engine.session() else {
        return Ok(Status {
            status: State::ReauthRequired,
            ..Status::default()
        });
    };
    let destination = team.unwrap_or(&session.workspace.id);
    let policy = store
        .cloud_sync_policy_for_origins(
            &session.user.id,
            destination,
            &session.resource,
            &session.iss,
        )
        .map_err(|_| "Could not read Cloud sync consent.")?;
    let mut status = Status {
        status: state,
        account_id: Some(session.user.id.clone()),
        destination_id: Some(destination.to_owned()),
        ..Status::default()
    };
    let Some(policy) = policy else {
        status.status = State::ConsentRequired;
        return Ok(status);
    };
    status.paused = policy.paused;
    status.source_confirmation_required = policy.paused;
    status.saved_folders = Some(saved_folders(&policy));
    if policy.paused {
        status.status = State::Paused;
    } else if state == State::Paused {
        status.status = State::Ready;
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
        .cloud_sync_jobs(&session.user.id, destination, &session.resource)
        .map_err(|_| "Could not read capture progress.")?;
    let entries = host
        .spool()?
        .entries()
        .map_err(|_| "Could not read saved capture progress.")?;
    let can_offer_retry = !policy.paused
        && policy.team_grant_id.is_none()
        && !matches!(state, State::ReauthRequired | State::StorageUnavailable);
    let recovery_runner = runner
        .as_deref()
        .filter(|_| can_offer_retry && state != State::Paused);
    status.automatic_recovery_count = recovery_runner.map(|_| 0);
    let mut retryable = Vec::new();
    let mut pending_upload = false;
    for job in &jobs {
        if !job.allowed(&policy) {
            continue;
        }
        let entry = entries.iter().find(|entry| entry.identity == job.identity);
        pending_upload |= runner
            .as_deref()
            .is_some_and(|runner| runner.has_scheduled_upload(job, entry, state));
        if recovery_runner
            .is_some_and(|runner| runner.will_recover_automatically(job, entry, state))
        {
            if let Some(count) = &mut status.automatic_recovery_count {
                *count += 1;
            }
            status.queued_count += 1;
            continue;
        }
        if can_offer_retry
            && saved_retry(job, entry).is_some_and(|retry| {
                retry != SavedRetry::ClearDelay
                    || matches!(
                        state,
                        State::Offline
                            | State::UploadUnavailable
                            | State::RouteDenied
                            | State::Fenced
                            | State::Interrupted
                    )
            })
        {
            retryable.push(job);
        }
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
    status.retry_available = !retryable.is_empty();
    status.consent_version = Some(policy.consent_version.clone());
    status.policy_revision = Some(policy.revision.clone());
    repository_policy::apply_folder_status(&mut status, &policy);
    status.project_choices = policy.project_choices.clone();
    if team.is_none() {
        (status.accepted_count, status.suppressed_count) = store
            .cloud_sync_outcome_counts(&session.user.id, destination, &session.resource)
            .map_err(|_| "Could not read accepted source state.")?;
    }
    let (progress, transfer) = saved_transfer_progress(&policy, &jobs, &entries)?;
    status.progress = progress;
    if !policy.paused && matches!(state, State::Ready | State::Syncing) {
        status.upload_transfer = transfer;
    }
    if let Some(runner) = runner {
        for progress in runner.progress_trackers() {
            if progress.matches(&policy) && progress.matches_grant(&session.grant_id) {
                progress.observe_transfers(&policy, &jobs, &entries)?;
            }
        }
        status.storage_known = true;
        let pending_upload = pending_upload
            && (status.status != State::Offline || engine.status == super::AuthStatus::SignedIn);
        runner.update_runtime_progress(&policy, &session.grant_id, pending_upload, &mut status);
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
    work.run(Priority::Normal, move || read_sync_status(&app, true))
        .await
}

/// Read the saved view under the shared Cloud permit. Device controls do not
/// make an eligibility request when reading the result of a local change.
fn read_sync_status(app: &tauri::AppHandle, verify_teams: bool) -> Result<Status, String> {
    let controller = app.state::<Controller>();
    let store = app.state::<Store>();
    let content_generation = controller.sync.generation.load(Ordering::Acquire);
    let device_enabled = ensure_cloud_setup(&controller.setup_ready, &store)
        .and_then(|()| controller.sync.device_upload_enabled(&store));
    let generation = controller.account_generation.load(Ordering::Acquire);
    let mut engine = controller
        .engine
        .lock()
        .map_err(|_| "Cloud is unavailable.")?;
    if engine.generation != generation {
        return Ok(Status {
            status: State::ReauthRequired,
            device_upload_enabled: matches!(device_enabled, Ok(true)),
            ..Status::default()
        });
    }
    let account_current = || controller.account_generation.load(Ordering::Acquire) == generation;
    let current = || {
        account_current()
            && controller.sync.generation.load(Ordering::Acquire) == content_generation
    };
    let principal = engine.session().cloned();
    let setup_failed = device_enabled.is_err();
    let mut host = controller
        .capture
        .lock()
        .map_err(|_| "Session capture is unavailable.")?;
    let mut runner = controller
        .sync
        .runner
        .lock()
        .map_err(|_| "Cloud sync is unavailable.")?;
    let mut result = runner.report_status(
        &store,
        &mut host,
        &engine,
        setup_failed,
        &account_current,
        |_| {},
    );
    if !setup_failed {
        let runners = controller
            .sync
            .team_runners
            .lock()
            .map_err(|_| "Cloud sync is unavailable.")?;
        result.team_statuses = saved_team_statuses(
            &store,
            principal.as_ref(),
            &runners,
            (&mut engine, &app.config().identifier),
            verify_teams.then_some(&account_current as &dyn Fn() -> bool),
        )?;
    }
    if !current()
        || principal.as_ref().is_some_and(|saved| {
            engine
                .session()
                .is_none_or(|active| !active.same_grant(saved))
        })
    {
        return Ok(Status {
            status: State::ReauthRequired,
            device_upload_enabled: controller
                .sync
                .device_upload_enabled(&store)
                .unwrap_or(false),
            ..Status::default()
        });
    }
    Ok(result.with_device_upload_enabled(matches!(device_enabled, Ok(true))))
}

fn selected_sources(folders: &[Folder]) -> Result<Vec<SourceSelection>, String> {
    let roots = capture::selected_roots(folders);
    if roots.is_empty()
        || roots.len() > MAX_SYNC_SOURCE_ROOTS
        || roots.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err("Choose valid session sources before confirming Cloud sync.".into());
    }
    let provider_roots = if roots.iter().any(|root| root.source_kind.is_project_only()) {
        file_sources::roots().0
    } else {
        Vec::new()
    };
    for root in &roots {
        let path = Path::new(&root.path);
        if !path.is_absolute()
            || root.path.len() > 4096
            || root.path.chars().any(char::is_control)
            || path.canonicalize().ok().as_deref() != Some(path)
            || (root.source_kind.is_project_only() && !provider_roots.contains(root))
            || if root.source_kind.is_directory() {
                !path.is_dir()
            } else {
                !path.is_file()
            }
        {
            return Err("Choose an existing session source without path aliases.".into());
        }
    }
    Ok(roots)
}

/// Resolve one exact Team through the current account bearer. The Personal
/// binding checks the account adapter only; it never substitutes a destination.
fn eligible_team(
    engine: &mut Engine,
    account: &str,
    destination: &str,
    account_current: &dyn Fn() -> bool,
) -> Result<(String, String), String> {
    let session = engine
        .session()
        .filter(|session| session.user.id == account)
        .ok_or("Cloud account changed.")?;
    let anchor = DestinationBinding {
        account_id: account.to_owned(),
        destination_id: session.workspace.id.clone(),
    };
    engine
        .eligible_workspaces(&anchor, account_current)
        .map_err(|_| "Could not verify Cloud workspaces.")?
        .into_iter()
        .find(|entry| {
            entry.workspace.id == destination && entry.workspace.kind == auth::WorkspaceKind::Team
        })
        .and_then(|entry| {
            entry
                .membership_generation
                .map(|generation| (generation, entry.workspace.name))
        })
        .ok_or_else(|| "The selected team workspace is not available.".into())
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
    let controller = app.state::<Controller>();
    let work = controller.blocking.clone();
    let wake = controller.sync.wake.clone();
    let generation = controller.account_generation.load(Ordering::Acquire);
    let content_generation = controller.sync.generation.load(Ordering::Acquire);
    let result = work
        .run(Priority::Wait(SYNC_CONTROL_ADMISSION_WAIT), move || {
            let controller = app.state::<Controller>();
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
            let binding = DestinationBinding {
                account_id: consent.account_id.clone(),
                destination_id: consent.destination_id.clone(),
            };
            let personal = engine
                .session()
                .is_some_and(|session| binding.matches(session));
            if consent.version != ONGOING_CONSENT_VERSION
                || !consent.full_content
                || !consent.ongoing
                || consent.attachments
                || consent.selected_sources.is_empty()
                || consent.selected_sources.len() > MAX_SYNC_SOURCE_ROOTS
                || consent.projects.is_empty()
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
            validate_project_directories(consent.projects.iter())?;
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
            let anchor = engine
                .session()
                .filter(|session| {
                    session.user.id == binding.account_id
                        && session.workspace.kind == auth::WorkspaceKind::Personal
                })
                .map(|session| DestinationBinding {
                    account_id: session.user.id.clone(),
                    destination_id: session.workspace.id.clone(),
                })
                .ok_or("The selected workspace connection changed.")?;
            let (_, repository_anchors) = super::projects::confirm_project_choices_with_origins(
                &store,
                &mut *controller
                    .project_cache
                    .lock()
                    .map_err(|_| "Project discovery is unavailable.")?,
                &anchor,
                &consent.project_choices,
                &consent.projects.iter().cloned().collect(),
                &current,
            )?;
            let team = if personal {
                None
            } else {
                Some(eligible_team(
                    &mut engine,
                    &binding.account_id,
                    &binding.destination_id,
                    &account_current,
                )?)
            };
            let generation = team.as_ref().map(|(generation, _)| generation.as_str());
            let capabilities = upload::capabilities(
                &mut engine,
                &consent.account_id,
                (&consent.destination_id, generation),
                &account_current,
                &current,
            );
            let capabilities = capabilities
                .map_err(|_| "Cloud upload is unavailable. No source content was uploaded.")?;
            if !consent
                .projects
                .iter()
                .any(|project| capabilities.supports(project.source_kind))
            {
                return Err("Cloud does not support the session formats in these folders.".into());
            }
            controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?
                .configure_limits(capabilities.limits)?;
            if let Some(move_from) = &consent.move_from {
                let session = engine
                    .session()
                    .filter(|session| anchor.matches(session) && account_current())
                    .ok_or("The selected workspace connection changed.")?;
                verify_project_move_to_new_policy(
                    &store,
                    session,
                    &binding.destination_id,
                    move_from,
                    &consent.projects,
                    &repository_anchors,
                )?;
            }
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
            let session = engine
                .session()
                .filter(|session| session.user.id == consent.account_id && account_current())
                .ok_or("The selected workspace connection changed.")?;
            let saved = store
                .save_cloud_sync_policy_with_repositories(
                    (
                        &consent.account_id,
                        &consent.destination_id,
                        &session.resource,
                        &session.iss,
                        team.as_ref()
                            .map(|(generation, name)| (generation.as_str(), name.as_str())),
                    ),
                    &roots,
                    &consent.projects,
                    &consent.project_choices,
                    &repository_anchors,
                    || {
                        account_current()
                            && controller.sync.generation.load(Ordering::Acquire)
                                == content_generation + 1
                            && engine
                                .session()
                                .is_some_and(|active| active.same_grant(session))
                    },
                )
                .map_err(|_| "Could not save Cloud sync consent.")?;
            if personal {
                let mut runner = controller
                    .sync
                    .runner
                    .lock()
                    .map_err(|_| "Cloud sync is unavailable.")?;
                runner.next_reconcile = None;
                runner.state = State::Ready;
            }
            let mut host = controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?;
            let result = status_for_destination(
                &store,
                &mut host,
                &engine,
                State::Ready,
                (!personal).then_some(consent.destination_id.as_str()),
                None,
            )?;
            let result = if personal {
                result
            } else {
                team_policy_status(&saved, Some(result), None)
            }
            .with_device_upload_enabled(controller.sync.device_upload_enabled(&store)?);
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

/// Renew the reviewed Team policy and add only validated new folder choices.
/// Check current membership through the Personal connection.
#[tauri::command]
pub(crate) async fn cloud_reapprove_team_sync(
    app: tauri::AppHandle,
    confirmation: TeamConfirmation,
) -> Result<Status, String> {
    let TeamConfirmation {
        account_id,
        destination_id,
        expected_policy_revision,
        folder_paths,
        additions,
    } = confirmation;
    if folder_paths.is_empty()
        || folder_paths.len() > MAX_SYNC_PROJECT_PATHS
        || !folder_paths
            .iter()
            .all(|path| crate::store::cloud_capture::sync::valid_project_path(path))
    {
        return Err("Reload the saved Team folders before confirming.".into());
    }
    let count = folder_paths.len();
    let confirmed_paths: BTreeSet<_> = folder_paths.into_iter().collect();
    if confirmed_paths.len() != count {
        return Err("Reload the saved Team folders before confirming.".into());
    }
    if additions.len() > MAX_SYNC_PROJECT_PATHS
        || additions.values().any(Vec::is_empty)
        || additions.values().map(Vec::len).sum::<usize>() > MAX_SYNC_PROJECT_BINDINGS
    {
        return Err("Reload projects and choose supported Team folders.".into());
    }
    let choices: BTreeMap<_, _> = additions.keys().map(|id| (id.clone(), true)).collect();
    let projects: BTreeSet<_> = additions.into_values().flatten().collect();
    let controller = app.state::<Controller>();
    let account_generation = controller.account_generation.load(Ordering::Acquire);
    let content_generation = controller.sync.generation.load(Ordering::Acquire);
    let work = controller.blocking.clone();
    let wake = controller.sync.wake.clone();
    let result = work
        .run(Priority::Wait(SYNC_CONTROL_ADMISSION_WAIT), move || {
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
            if !current() || engine.generation != account_generation {
                return Err("Cloud account or upload controls changed. Confirm again.".into());
            }
            engine
                .refresh(now(), account_current)
                .map_err(|_| "Connect to Cloud before confirming saved Team folders.")?;
            let (generation, name) =
                eligible_team(&mut engine, &account_id, &destination_id, &account_current)?;
            let session = engine
                .session()
                .filter(|session| {
                    session.workspace.kind == auth::WorkspaceKind::Personal
                        && session.user.id == account_id
                        && session.workspace.id != destination_id
                })
                .cloned()
                .ok_or("The selected workspace connection changed.")?;
            let store = app.state::<Store>();
            ensure_cloud_setup(&controller.setup_ready, &store)?;
            let mut repository_anchors = BTreeMap::new();
            if !projects.is_empty() {
                validate_project_directories(projects.iter())?;
                let anchor = DestinationBinding {
                    account_id: session.user.id.clone(),
                    destination_id: session.workspace.id.clone(),
                };
                (_, repository_anchors) = super::projects::confirm_project_choices_with_origins(
                    &store,
                    &mut *controller
                        .project_cache
                        .lock()
                        .map_err(|_| "Project discovery is unavailable.")?,
                    &anchor,
                    &choices,
                    &projects,
                    &current,
                )?;
                let capabilities = upload::capabilities(
                    &mut engine,
                    &account_id,
                    (&destination_id, Some(&generation)),
                    &account_current,
                    &current,
                )
                .map_err(|_| "Cloud upload is unavailable. No source content was uploaded.")?;
                if !projects
                    .iter()
                    .any(|project| capabilities.supports(project.source_kind))
                {
                    return Err(
                        "Cloud does not support the session formats in these folders.".into(),
                    );
                }
                controller
                    .capture
                    .lock()
                    .map_err(|_| "Session capture is unavailable.")?
                    .configure_limits(capabilities.limits)?;
            }
            let saved = store
                .renew_cloud_team_sync_policy_with_repositories(
                    (
                        &account_id,
                        &destination_id,
                        &session.resource,
                        &session.iss,
                        Some((&generation, &name)),
                    ),
                    &expected_policy_revision,
                    &confirmed_paths,
                    (!projects.is_empty()).then_some((&projects, &choices, &repository_anchors)),
                    || {
                        current()
                            && engine
                                .session()
                                .is_some_and(|active| active.same_grant(&session))
                    },
                )
                .map_err(|_| "Could not renew the saved Team folders. Reload and try again.")?;
            let state = if saved.paused {
                State::Paused
            } else {
                State::Ready
            };
            let status = team_policy_status(
                &saved,
                Some(Status {
                    status: state,
                    account_id: Some(account_id),
                    destination_id: Some(destination_id),
                    policy_revision: Some(saved.revision.clone()),
                    ..Status::default()
                }),
                None,
            )
            .with_device_upload_enabled(controller.sync.device_upload_enabled(&store)?);
            if !current() || engine.session().is_none() {
                return Err("Cloud account or upload controls changed. Saved work is kept.".into());
            }
            Ok(emit_status(&app, status))
        })
        .await;
    if result.is_ok() {
        wake.notify_one();
    }
    result
}

/// Change one approved Settings group without replacing a paged policy or resuming uploads.
#[tauri::command]
pub(crate) async fn cloud_set_sync_project(
    app: tauri::AppHandle,
    mut change: ProjectChange,
) -> Result<Status, String> {
    use super::projects::confirm_project_choices_with_origins;
    use crate::store::cloud_capture::sync::valid_project_path;

    if change.projects.len() > MAX_SYNC_PROJECT_BINDINGS
        || change.folder_paths.len() > MAX_SYNC_PROJECT_PATHS
        || change
            .expected_removals
            .as_ref()
            .is_some_and(|removals| removals.len() > MAX_SYNC_PROJECT_BINDINGS)
    {
        return Err("Reload projects and choose a supported Cloud project.".into());
    }
    let group_key = if !change.selected {
        change
            .group_id
            .strip_prefix("saved-")
            .unwrap_or(&change.group_id)
    } else {
        &change.group_id
    };
    if group_key.len() != 43
        || !group_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || !change
            .folder_paths
            .iter()
            .all(|path| valid_project_path(path))
        || (change.selected && change.expected_removals.is_some())
        || (change.move_from.is_some() && !change.selected)
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
    let priority = if !change.selected {
        Priority::Cleanup
    } else {
        Priority::Wait(SYNC_CONTROL_ADMISSION_WAIT)
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
                account_id: change.account_id.clone(),
                destination_id: change.destination_id.clone(),
            };
            let session = engine
                .session()
                .cloned()
                .ok_or("Cloud account changed. Reload projects.")?;
            let personal = binding.matches(&session);
            if session.workspace.kind != auth::WorkspaceKind::Personal
                || session.user.id != binding.account_id
                || !current()
            {
                return Err("Cloud account or upload controls changed. Reload projects.".into());
            }
            let store = app.state::<Store>();
            ensure_cloud_setup(&controller.setup_ready, &store)?;
            let policy = store
                .cloud_sync_policy_for_origins(
                    &binding.account_id,
                    &binding.destination_id,
                    &session.resource,
                    &session.iss,
                )
                .map_err(|_| "Could not read Cloud sync consent.")?
                .ok_or("Confirm Cloud sync first.")?;
            if policy.revision != change.expected_policy_revision {
                return Err("Cloud project choices changed. Reload projects and try again.".into());
            }
            if !personal && !change.selected {
                let can_publish = || {
                    current()
                        && engine
                            .session()
                            .is_some_and(|active| active.same_grant(&session))
                };
                let saved = disable_team_folder(&store, &policy, &change, &can_publish)?;
                if !can_publish() {
                    return Err(
                        "Cloud account or upload controls changed. Saved work is kept.".into(),
                    );
                }
                let result = team_policy_status(&saved, None, None)
                    .with_device_upload_enabled(controller.sync.device_upload_enabled(&store)?);
                return Ok(emit_status(&app, result));
            }
            let anchor = DestinationBinding {
                account_id: session.user.id.clone(),
                destination_id: session.workspace.id.clone(),
            };
            let team = if !personal {
                if !policy.is_team() {
                    return Err("The saved workspace policy changed. Reload projects.".into());
                }
                Some(eligible_team(
                    &mut engine,
                    &binding.account_id,
                    &binding.destination_id,
                    &account_current,
                )?)
            } else {
                None
            };
            let membership_generation = team.as_ref().map(|(generation, _)| generation.as_str());
            let reapprove = membership_generation
                .is_some_and(|generation| policy.needs_team_membership(generation));
             if reapprove && has_folders(&policy) {
                return Err("Confirm the saved Team folders before adding more.".into());
            }
            if !current() {
                return Err("Cloud account or upload controls changed. Reload projects.".into());
            }
            let mut host = controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?;
             let mut repository_anchors = BTreeMap::new();
             let remove_repository = !change.selected && policy.repository_rule_for_group(&change.group_id).is_some();
             let (projects, fence, clear_source_hold) = if change.selected {
                let projects: BTreeSet<_> = std::mem::take(&mut change.projects).into_iter().collect();
                validate_project_directories(projects.iter())?;
                 let (allowed_roots, verified_anchors) = confirm_project_choices_with_origins(
                    &store,
                    &mut *controller
                        .project_cache
                        .lock()
                        .map_err(|_| "Project discovery is unavailable.")?,
                    &anchor,
                    &[(change.group_id.clone(), true)].into(),
                    &projects,
                    &current,
                 )?;
                 repository_anchors = verified_anchors;
                let clear_source_hold = policy.paused
                    && policy.roots.iter().all(|root| {
                        allowed_roots.iter().any(|allowed| {
                            allowed.source_kind == root.source_kind && allowed.path == root.path
                        })
                    });
                if policy.paused && !clear_source_hold {
                    return Err("Restore access to the saved sources or turn off their folders, then save again.".into());
                }
                let capabilities = upload::capabilities(
                    &mut engine,
                    &binding.account_id,
                    (&binding.destination_id, membership_generation),
                    &account_current,
                    &current,
                );
                super::emit_snapshot(&app, super::snapshot(&app, &engine));
                let capabilities = capabilities
                    .map_err(|_| "Cloud upload is unavailable. No source content was uploaded.")?;
                if !projects
                    .iter()
                    .any(|project| capabilities.supports(project.source_kind))
                {
                    return Err("Cloud does not support the session formats in this folder.".into());
                }
                host.configure_limits(capabilities.limits)?;
                if let Some(move_from) = &change.move_from {
                    let active = engine
                        .session()
                        .filter(|active| active.same_grant(&session))
                        .ok_or("The selected workspace connection changed.")?;
                     verify_project_move(&store, active, &binding.destination_id, move_from, &projects, &repository_anchors)?;
                }
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
                (projects, content_generation + 1, clear_source_hold)
            } else {
                (
                     if remove_repository { BTreeSet::new() } else { project_removals(&policy, &change)? },
                    content_generation,
                    false,
                )
            };
            let can_commit = || {
                account_current()
                    && controller.sync.generation.load(Ordering::Acquire) == fence
                    && engine
                        .session()
                        .is_some_and(|session| anchor.matches(session))
            };
             let saved = if remove_repository {
                 revoke_repository(&store, &policy, &change, &can_commit)
             } else if let Some((generation, name)) = team.as_ref().filter(|_| reapprove) {
                let roots = projects
                    .iter()
                    .map(|project| SourceSelection {
                        source_kind: project.source_kind,
                        path: project.source_path.clone(),
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let projects = projects.into_iter().collect::<Vec<_>>();
                 store.reapprove_cloud_sync_policy_with_repositories(
                    &policy,
                    (
                        &binding.account_id,
                        &binding.destination_id,
                        &session.resource,
                        &session.iss,
                        Some((generation, name)),
                    ),
                    &roots,
                     (&projects, &repository_anchors),
                    &[(change.group_id, true)].into(),
                    can_commit,
                )
            } else {
                 store.set_cloud_sync_project_with_repositories(
                    &policy,
                    &change.group_id,
                    change.selected,
                     (&projects, &repository_anchors),
                    clear_source_hold,
                    can_commit,
                )
            }
            .map_err(
                |_| "Could not save the Cloud project change. Reload projects and try again.",
            )?;
            if !personal {
                if !can_commit() {
                    return Err(
                        "Cloud account or upload controls changed. Saved work is kept.".into(),
                    );
                }
                let result = status_for_destination(
                    &store,
                    &mut host,
                    &engine,
                    State::Ready,
                    Some(&saved.destination_id),
                    None,
                )?;
                return Ok(emit_status(
                    &app,
                    team_policy_status(&saved, Some(result), None)
                        .with_device_upload_enabled(controller.sync.device_upload_enabled(&store)?),
                ));
            }
            let mut runner = controller
                .sync
                .runner
                .lock()
                .map_err(|_| "Cloud sync is unavailable.")?;
            let device_enabled = controller.sync.device_upload_enabled(&store)?;
            if change.selected && !saved.paused && device_enabled {
                runner.next_reconcile = None;
                runner.state = State::Ready;
            }
            let result = status(&store, &mut host, &engine, runner.state)
                .unwrap_or_else(|_| Status {
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
                })
                .with_device_upload_enabled(device_enabled);
            if !can_commit() {
                return Err("Cloud account or upload controls changed. Saved work is kept.".into());
            }
            Ok(emit_status(&app, result))
        })
        .await;
    wake.notify_one();
    result
}

/// Turn off only current saved Team consent. A saved-only row names one exact
/// directory and never needs Git grouping, provider access, or a team bearer.
fn disable_team_folder(
    store: &Store,
    policy: &Policy,
    change: &ProjectChange,
    current: &dyn Fn() -> bool,
) -> Result<Policy, String> {
    if change.selected
        || !change.projects.is_empty()
        || !policy.is_team()
        || change.account_id != policy.account_id
        || change.destination_id != policy.destination_id
        || change.expected_policy_revision != policy.revision
        || !current()
    {
        return Err("Reload the saved team folder and try turning it off again.".into());
    }
    if policy.repository_rule_for_group(&change.group_id).is_some() {
        return revoke_repository(store, policy, change, current).map_err(|_| {
            "Could not turn off team folder uploads. Reload folders and try again.".into()
        });
    }
    let removals = project_removals(policy, change)?;
    store
        .set_cloud_sync_project(policy, &change.group_id, false, &removals, false, current)
        .map_err(|_| "Could not turn off team folder uploads. Reload folders and try again.".into())
}

/// Remove an exact saved path without access to its provider or Git metadata.
fn project_removals(
    policy: &Policy,
    change: &ProjectChange,
) -> Result<BTreeSet<ProjectSelection>, String> {
    let removals: BTreeSet<_> = if change.group_id.starts_with("saved-") {
        let [path] = change.folder_paths.as_slice() else {
            return Err("Choose one exact saved folder.".into());
        };
        if saved_team_folder_id(path) != change.group_id {
            return Err("The saved folder changed. Reload it and try again.".into());
        }
        if policy.projects.iter().any(|project| {
            project.project_path != *path && Path::new(path).starts_with(&project.project_path)
        }) {
            return Err("Turn off the enclosing saved folder first.".into());
        }
        policy
            .projects
            .iter()
            .filter(|project| project.project_path == *path)
            .cloned()
            .collect()
    } else {
        super::projects::group_projects_to_remove(
            &change.group_id,
            &change.folder_paths,
            &policy.projects,
        )?
    };
    if removals.is_empty() {
        return Err("The saved folder changed. Reload it and try again.".into());
    }
    if let Some(expected) = &change.expected_removals
        && expected.iter().collect::<BTreeSet<_>>() != removals.iter().collect()
    {
        return Err("The saved folder paths changed. Reload folders before moving them.".into());
    }
    Ok(removals)
}

/// Change upload admission for every destination on this device. OFF fences
/// in-flight work before waiting. ON requires the caller's current Personal
/// connection and does not retry interrupted captures.
#[tauri::command]
pub(crate) async fn cloud_set_sync_enabled(
    app: tauri::AppHandle,
    enabled: bool,
    binding: Option<DestinationBinding>,
) -> Result<Status, String> {
    let controller = app.state::<Controller>();
    let work = controller.blocking.clone();
    let wake = controller.sync.wake.clone();
    let account_generation = controller.account_generation.load(Ordering::Acquire);
    let (priority, expected_pause) = if enabled {
        (
            Priority::Wait(SYNC_CONTROL_ADMISSION_WAIT),
            controller.sync.device_pause.load(Ordering::Acquire),
        )
    } else {
        (Priority::Cleanup, controller.sync.fence_device_uploads())
    };
    let result = work
        .run(priority, move || {
            let controller = app.state::<Controller>();
            let store = app.state::<Store>();
            ensure_cloud_setup(&controller.setup_ready, &store)?;
            device_upload::save_switch(
                &controller,
                &store,
                enabled,
                binding.as_ref(),
                expected_pause,
                account_generation,
            )?;
            let result = read_sync_status(&app, false).unwrap_or_else(|_| Status {
                status: State::StorageUnavailable,
                device_upload_enabled: controller
                    .sync
                    .device_upload_enabled(&store)
                    .unwrap_or(false),
                ..Status::default()
            });
            Ok(emit_status(&app, result))
        })
        .await;
    wake.notify_one();
    result
}

/// Retry saved work for one exact destination. Keep saved chunks, plans, consent,
/// and the device upload switch. A proved empty failure can get a fresh ID.
#[tauri::command]
pub(crate) async fn cloud_retry_sync(
    app: tauri::AppHandle,
    binding: DestinationBinding,
    expected_policy_revision: String,
) -> Result<Status, String> {
    change_saved_upload_work(app, binding, expected_policy_revision, SyncWork::Retry).await
}

/// Start a fresh scan of one workspace's saved folders. This does not change
/// consent, resume device uploads, retry failed jobs, or clear source outcomes.
/// Skip matching acceptance receipts and keep local deletion markers.
#[tauri::command]
pub(crate) async fn cloud_backfill_sync(
    app: tauri::AppHandle,
    binding: DestinationBinding,
    expected_policy_revision: String,
) -> Result<Status, String> {
    change_saved_upload_work(app, binding, expected_policy_revision, SyncWork::Backfill).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SyncWork {
    Retry,
    Backfill,
}

async fn change_saved_upload_work(
    app: tauri::AppHandle,
    binding: DestinationBinding,
    expected_policy_revision: String,
    action: SyncWork,
) -> Result<Status, String> {
    let controller = app.state::<Controller>();
    let work = controller.blocking.clone();
    let wake = controller.sync.wake.clone();
    let generation = controller.account_generation.load(Ordering::Acquire);
    let content_generation = controller.sync.generation.load(Ordering::Acquire);
    let priority = match action {
        SyncWork::Backfill => Priority::Backfill,
        SyncWork::Retry => Priority::Normal,
    };
    let (result, retry_scheduled) = work
        .run(priority, move || {
            let controller = app.state::<Controller>();
            let account_current =
                || controller.account_generation.load(Ordering::Acquire) == generation;
            let current = || {
                account_current()
                    && controller.sync.generation.load(Ordering::Acquire) == content_generation
                    && controller.sync.device_admission_open()
            };
            let mut engine = controller
                .engine
                .lock()
                .map_err(|_| "Cloud is unavailable.")?;
            if !current() || engine.generation != generation {
                return Err("Cloud account changed. Saved work is kept.".into());
            }
            let session = engine
                .session()
                .filter(|session| session.user.id == binding.account_id)
                .cloned()
                .ok_or("Cloud account changed. Saved work is kept.")?;
            let store = app.state::<Store>();
            ensure_cloud_setup(&controller.setup_ready, &store)?;
            if !controller.sync.device_upload_enabled(&store)? {
                return Err("Turn on uploads on this device first.".into());
            }
            let policy =
                retry_policy_for_revision(&store, &session, &binding, &expected_policy_revision)?;
            if policy.paused {
                return Err("Confirm the changed Cloud folders first.".into());
            }
            if action == SyncWork::Backfill && !has_folders(&policy) {
                return Err("Save folders before starting backfill.".into());
            }
            let team = if binding.matches(&session) {
                if policy.is_team() {
                    return Err("The saved Cloud destination changed.".into());
                }
                None
            } else {
                let saved = policy
                    .membership_generation
                    .as_deref()
                    .filter(|_| policy.team_grant_id.is_none())
                    .ok_or("Confirm the Team folders again before retrying.")?;
                let (live, _) = eligible_team(
                    &mut engine,
                    &binding.account_id,
                    &binding.destination_id,
                    &account_current,
                )?;
                if live != saved {
                    return Err("The Team membership changed. Saved work is kept.".into());
                }
                Some((binding.destination_id.clone(), live))
            };
            let still_authorized = || {
                current()
                    && engine
                        .session()
                        .is_some_and(|active| active.same_grant(&session))
            };
            if !still_authorized() {
                return Err("Cloud account changed. Saved work is kept.".into());
            }
            let mut host = controller
                .capture
                .lock()
                .map_err(|_| "Session capture is unavailable.")?;
            if action == SyncWork::Backfill {
                // Verify storage before queueing the scan so a failed request
                // cannot also report that it started.
                let mut status = status_for_destination(
                    &store,
                    &mut host,
                    &engine,
                    State::Ready,
                    team.as_ref().map(|(id, _)| id.as_str()),
                    None,
                )?;
                if !still_authorized() {
                    return Err("Cloud upload controls changed.".into());
                }
                if let Some(team) = &team {
                    let mut runners = controller
                        .sync
                        .team_runners
                        .lock()
                        .map_err(|_| "Cloud sync is unavailable.")?;
                    let runner = runners
                        .entry((binding.account_id.clone(), binding.destination_id.clone()))
                        .or_default();
                    if runner.team.as_ref() != Some(team) {
                        *runner = Runner {
                            team: Some(team.clone()),
                            ..Runner::default()
                        };
                    }
                    runner.observe_retry_grant(&session);
                    runner.schedule_backfill(&policy, &session.grant_id);
                    status = team_policy_status(&policy, Some(status), None);
                    status = runner.backfill_status(status, Some(&session.grant_id));
                    runner.last_status = Some(status.clone());
                    runner.last_status_origin = Some(session.resource.clone());
                } else {
                    let mut runner = controller
                        .sync
                        .runner
                        .lock()
                        .map_err(|_| "Cloud sync is unavailable.")?;
                    runner.observe_retry_grant(&session);
                    runner.schedule_backfill(&policy, &session.grant_id);
                    status = runner.backfill_status(status, Some(&session.grant_id));
                }
                return Ok((
                    emit_status(&app, status.with_device_upload_enabled(true)),
                    true,
                ));
            }
            let retried = retry_saved_jobs(&store, host.spool()?, &policy, &still_authorized)?;
            let mut read_status = |state, runner: Option<&mut Runner>| {
                status_for_destination(
                    &store,
                    &mut host,
                    &engine,
                    state,
                    team.as_ref().map(|(id, _)| id.as_str()),
                    runner,
                )
            };
            let result = if let Some(team) = &team {
                let mut runners = controller
                    .sync
                    .team_runners
                    .lock()
                    .map_err(|_| "Cloud sync is unavailable.")?;
                if let Some(runner) = runners
                    .get_mut(&(binding.account_id.clone(), binding.destination_id.clone()))
                    .filter(|runner| runner.team.as_ref() == Some(team))
                {
                    runner.retry_status(&retried, &session.grant_id, read_status)?
                } else {
                    read_status(State::Ready, None)?
                }
            } else {
                let mut runner = controller
                    .sync
                    .runner
                    .lock()
                    .map_err(|_| "Cloud sync is unavailable.")?;
                runner.retry_status(&retried, &session.grant_id, read_status)?
            };
            let result = if team.is_some() {
                team_policy_status(&policy, Some(result), None)
            } else {
                result
            }
            .with_device_upload_enabled(true);
            if !still_authorized() {
                return Err("Cloud account changed. Saved work is kept.".into());
            }
            super::emit_snapshot(&app, super::snapshot(&app, &engine));
            Ok((emit_status(&app, result), !retried.is_empty()))
        })
        .await?;
    if retry_scheduled {
        wake.notify_one();
    }
    Ok(result)
}

/// Reject an action from an older view before checking Team membership or
/// touching saved jobs. The Cloud permit serializes policy writes with Retry.
fn retry_policy_for_revision(
    store: &Store,
    session: &auth::Session,
    binding: &DestinationBinding,
    expected_revision: &str,
) -> Result<Policy, String> {
    if session.user.id != binding.account_id {
        return Err("Cloud account changed. Saved work is kept.".into());
    }
    let policy = store
        .cloud_sync_policy_for_origins(
            &binding.account_id,
            &binding.destination_id,
            &session.resource,
            &session.iss,
        )
        .map_err(|_| "Could not read Cloud sync consent.")?
        .ok_or("Confirm Cloud sync first.")?;
    if policy.revision != expected_revision {
        return Err("Cloud folder choices changed. Reload before retrying saved work.".into());
    }
    Ok(policy)
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

#[cfg(test)]
#[path = "tests/sync_capacity_tests.rs"]
mod capacity_tests;

#[cfg(test)]
#[path = "tests/sync_workspace_status_integration_tests.rs"]
mod workspace_status_tests;
