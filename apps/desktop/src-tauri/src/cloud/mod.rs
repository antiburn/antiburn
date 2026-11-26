//! Feature-gated Cloud IPC for the existing onboarding and Settings surfaces.
//! The controller owns authentication and source-listing side effects. Renderer
//! snapshots contain no credentials and do not authorize content capture or upload.
//! One admission slot owns synchronous I/O. Cleanup waits asynchronously; browser
//! socket waiting runs separately and must recheck generations after admission.

mod auth;
mod capture;
#[cfg(unix)]
mod device_broker;
mod engine;
mod file_sources;
#[cfg(test)]
#[cfg(debug_assertions)]
mod http_tests;
mod loopback;
pub(crate) mod projects;
mod repository_checkout;
pub(crate) mod repository_origin;
mod state;
pub(crate) mod sync;
#[cfg(test)]
mod tests;
mod upload;
mod work;
pub(crate) mod workspace_grants;

use crate::store::cloud_capture::SourceKind;
use auth::{Config, Failure, HttpTransport, Identity, KeyringVault, Workspace};
use engine::Engine;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use work::{BlockingWork, Priority};

/// Closed renderer-safe authentication outcomes, not permission to capture content.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    #[default]
    Idle,
    Opening,
    Waiting,
    Returned,
    SignedIn,
    SignedOut,
    Cancelled,
    Expired,
    BrowserFailed,
    Offline,
    Rejected,
    InvalidResponse,
    CredentialFailed,
    Unavailable,
    ReauthRequired,
    RevocationUnconfirmed,
}

/// Status of a browser access update, including when local access expires.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AccessUpdateStatus {
    Waiting,
    Denied,
    Failed,
}

/// Extra actions a Desktop control can request. The native engine binds the account.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessUpdate {
    SessionReads,
    Capture,
}

impl AccessUpdate {
    /// The CLI broker and Desktop capture require Unix.
    fn platform_available(&self) -> bool {
        cfg!(unix)
    }

    fn actions(&self) -> &'static [auth::DeviceAction] {
        use auth::DeviceAction::*;
        match self {
            Self::SessionReads => &[SessionList, SessionRead],
            Self::Capture => &[CaptureWrite],
        }
    }
}

impl From<Failure> for AuthStatus {
    fn from(value: Failure) -> Self {
        match value {
            Failure::Offline | Failure::OfflineBeforeSend => Self::Offline,
            Failure::Rejected => Self::Rejected,
            Failure::InvalidResponse => Self::InvalidResponse,
            Failure::CredentialFailed => Self::CredentialFailed,
            Failure::Cancelled => Self::Cancelled,
            Failure::Unavailable => Self::Unavailable,
            Failure::ReauthRequired => Self::ReauthRequired,
        }
    }
}

/// An explicit native source-root preference. Display counts do not bind consent.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    path: String,
    selected: bool,
    #[serde(default, skip_serializing_if = "SourceKind::is_claude")]
    source_kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_count: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    session_count_is_partial: bool,
}

/// Persisted nested setup position, separate from the host onboarding step.
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    #[default]
    Handoff,
    Identity,
    Folders,
    Complete,
}

/// Preferences for one account and personal destination, never durable content consent.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    screen: Screen,
    folders: Vec<Folder>,
    #[serde(default = "existing_folder_choices")]
    folders_initialized: bool,
}

/// Untrusted renderer intent that must match the engine's current personal grant.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DestinationBinding {
    account_id: String,
    destination_id: String,
}

impl DestinationBinding {
    fn matches_grant(&self, session: &auth::Session) -> bool {
        self.account_id == session.user.id && self.destination_id == session.workspace.id
    }

    /// Reject a changed account, destination, or explicit team without fallback.
    fn matches(&self, session: &auth::Session) -> bool {
        self.matches_grant(session) && session.workspace.kind == auth::WorkspaceKind::Personal
    }
}

/// Preserve the stored draft's legacy initialization default when its field is absent.
fn existing_folder_choices() -> bool {
    true
}

/// Renderer state for one current account. `auth_enabled` describes configured
/// service availability, not the binary's `cloud` feature or upload authority.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    auth_enabled: bool,
    dashboard_available: bool,
    session_reads_allowed: bool,
    session_read_update_available: bool,
    capture_update_available: bool,
    access_update: Option<AccessUpdateStatus>,
    capture_enabled: bool,
    sync_enabled: bool,
    status: AuthStatus,
    identity: Option<Identity>,
    expires_at: Option<i64>,
    grant_expires_at: Option<i64>,
    grant_id: Option<String>,
    destination: Option<Workspace>,
    revocation_pending: bool,
    local_cleanup_pending: bool,
    local_reset_available: bool,
    remote_revocation_unconfirmed: bool,
    setup_error: Option<String>,
    draft: Draft,
}

/// One desktop owner serializes auth and capture operations. Atomic generations
/// invalidate queued work before it can acquire a mutex or publish old authority.
pub(crate) struct Controller {
    blocking: BlockingWork,
    setup_ready: AtomicBool,
    engine: Mutex<Engine>,
    workspace_cleanup_failed: AtomicBool,
    capture: Mutex<capture::Host>,
    sync: sync::Scheduler,
    project_cache: Mutex<projects::Cache>,
    project_listing: BlockingWork,
    project_listing_cache: Mutex<projects::Cache>,
    project_listing_authority: Mutex<Option<projects::ListingAuthority>>,
    attempt_generation: AtomicU64,
    account_generation: AtomicU64,
    recorded_status: Mutex<AuthStatus>,
}

/// Supply UTC seconds for fixed grant deadlines and browser-attempt expiry.
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Initialize feature tables only inside admitted blocking work. Failed attempts
/// leave readiness false so a later read can retry; auth cleanup does not need them.
fn ensure_cloud_setup(ready: &AtomicBool, store: &crate::store::Store) -> Result<(), String> {
    if !ready.load(Ordering::Acquire) {
        store
            .initialize_cloud_setup()
            .map_err(|_| "Could not initialize Cloud setup. Try again.".to_owned())?;
        ready.store(true, Ordering::Release);
    }
    Ok(())
}

/// Read account-bound preferences in a blocking worker, never as content consent.
fn draft(app: &tauri::AppHandle, session: Option<&auth::Session>) -> Result<Draft, String> {
    let Some(session) = session else {
        return Ok(Draft::default());
    };
    let store = app.state::<crate::store::Store>();
    ensure_cloud_setup(&app.state::<Controller>().setup_ready, &store)?;
    let saved = store
        .cloud_draft(&session.user.id, &session.workspace.id)
        .map_err(|_| "Could not read Cloud setup.")?
        .map(|value| serde_json::from_str::<Draft>(&value))
        .transpose()
        .map_err(|_| "Could not read Cloud setup.")?;
    Ok(saved.unwrap_or_default())
}

/// Build safe view state in a blocking worker. Account generations hide stale
/// identity. Recheck expiry after store reads; setup failures do not hide cleanup.
fn snapshot(app: &tauri::AppHandle, engine: &Engine) -> Snapshot {
    let current = || {
        engine.generation
            == app
                .state::<Controller>()
                .account_generation
                .load(Ordering::Acquire)
    };
    let session = if current() { engine.session() } else { None };
    let (mut draft, mut setup_error) = match draft(app, session) {
        Ok(draft) => (draft, None),
        Err(error) => (Draft::default(), Some(error)),
    };
    if session.is_none() {
        draft.screen = Screen::Handoff;
    }
    let sync_enabled = session
        .filter(|_| setup_error.is_none())
        .is_some_and(|session| {
            let store = app.state::<crate::store::Store>();
            let result = (|| {
                if !app
                    .state::<Controller>()
                    .sync
                    .device_upload_enabled(&store)?
                {
                    return Ok(false);
                }
                store
                    .cloud_sync_policy_for_origins(
                        &session.user.id,
                        &session.workspace.id,
                        &session.resource,
                        &session.iss,
                    )
                    .map(|policy| policy.is_some_and(|policy| !policy.paused))
                    .map_err(|_| "Could not read Cloud sync consent.".to_owned())
            })();
            match result {
                Ok(enabled) => enabled,
                Err(_) => {
                    setup_error = Some("Could not read Cloud sync consent.".into());
                    false
                }
            }
        });
    if session.is_some() && !cfg!(unix) {
        setup_error = Some("Desktop capture is not supported on this platform.".into());
    }
    let current = current();
    let session = session.filter(|_| current && engine.session().is_some());
    let team_cleanup_unconfirmed = app
        .state::<Controller>()
        .setup_ready
        .load(Ordering::Acquire)
        && engine.config.as_ref().is_some_and(|config| {
            app.state::<crate::store::Store>()
                .cloud_workspace_slots(&config.issuer, &config.resource)
                .map_or(true, |slots| {
                    slots.iter().any(|(account, workspace)| {
                        engine
                            .workspace_cleanup_unconfirmed(account, workspace)
                            .unwrap_or(true)
                    })
                })
        });
    if !current || session.is_none() {
        draft = Draft::default();
        setup_error = None;
    }
    if let Ok(mut authority) = app.state::<Controller>().project_listing_authority.lock() {
        // Listing uses this proof without holding the engine during local scans.
        // A stale snapshot cannot extend the grant or authorize uploads.
        *authority = session
            .filter(|session| {
                setup_error.is_none() && session.workspace.kind == auth::WorkspaceKind::Personal
            })
            .zip(engine.listing_expiry())
            .map(|(session, expires_at)| projects::ListingAuthority {
                binding: DestinationBinding {
                    account_id: session.user.id.clone(),
                    destination_id: session.workspace.id.clone(),
                },
                generation: engine.generation,
                expires_at,
            });
    }
    let session_reads_allowed = session.is_some_and(|session| {
        session.allows(auth::DeviceAction::SessionList)
            && session.allows(auth::DeviceAction::SessionRead)
    });
    Snapshot {
        auth_enabled: engine.config.is_some(),
        dashboard_available: engine
            .config
            .as_ref()
            .is_some_and(|config| config.dashboard.is_some()),
        session_reads_allowed,
        session_read_update_available: AccessUpdate::SessionReads.platform_available()
            && session.is_some()
            && !session_reads_allowed,
        capture_update_available: cfg!(unix)
            && setup_error.is_none()
            && session.is_some_and(|session| !session.allows(auth::DeviceAction::CaptureWrite)),
        access_update: engine.access_update.filter(|status| {
            current && (session.is_some() || matches!(status, AccessUpdateStatus::Waiting))
        }),
        capture_enabled: cfg!(unix)
            && setup_error.is_none()
            && session.is_some_and(|session| session.allows(auth::DeviceAction::CaptureWrite)),
        sync_enabled: sync_enabled && current && session.is_some(),
        status: if !current {
            AuthStatus::Cancelled
        } else if engine.status == AuthStatus::SignedIn && session.is_none() {
            AuthStatus::ReauthRequired
        } else {
            engine.status
        },
        identity: session.map(Identity::from),
        expires_at: engine
            .attempt
            .as_ref()
            .filter(|_| current)
            .map(|attempt| attempt.expires_at),
        grant_expires_at: session.map(|session| session.expires_at),
        grant_id: session.map(|session| session.grant_id.clone()),
        destination: session.map(|session| session.workspace.clone()),
        revocation_pending: matches!(engine.cleanup, state::Cleanup::Revoke { .. }),
        local_cleanup_pending: matches!(engine.cleanup, state::Cleanup::DeleteCredential { .. }),
        local_reset_available: engine.local_reset_available,
        remote_revocation_unconfirmed: team_cleanup_unconfirmed
            || app
                .state::<Controller>()
                .workspace_cleanup_failed
                .load(Ordering::Acquire)
            || engine.remote_logout_unconfirmed(),
        setup_error,
        draft,
    }
}

/// Notify views without replaying operation outcomes when stored state is read.
fn emit_snapshot(app: &tauri::AppHandle, result: Snapshot) -> Snapshot {
    for window in ["onboarding", "main", "settings"] {
        let _ = app.emit_to(window, "cloud:changed", result.clone());
    }
    result
}

/// Record changed operation outcomes and notify views from admitted blocking work.
/// Read-only notifications use `emit_snapshot` so they do not replay analytics.
fn publish(app: &tauri::AppHandle, engine: &Engine) -> Snapshot {
    let result = snapshot(app, engine);
    let controller = app.state::<Controller>();
    if let Ok(mut previous) = controller.recorded_status.lock()
        && *previous != engine.status
    {
        use crate::analytics::event::OnboardingCloudStatus as Event;
        let event = match engine.status {
            AuthStatus::Idle => None,
            AuthStatus::Opening => Some(Event::Opening),
            AuthStatus::Waiting => Some(Event::Waiting),
            AuthStatus::Returned => Some(Event::Returned),
            AuthStatus::SignedIn => Some(Event::SignedIn),
            AuthStatus::SignedOut => Some(Event::SignedOut),
            AuthStatus::Cancelled => Some(Event::Cancelled),
            AuthStatus::Expired => Some(Event::Expired),
            AuthStatus::BrowserFailed => Some(Event::BrowserFailed),
            AuthStatus::Offline => Some(Event::Offline),
            AuthStatus::CredentialFailed => Some(Event::CredentialFailed),
            AuthStatus::Unavailable => Some(Event::Unavailable),
            AuthStatus::Rejected
            | AuthStatus::InvalidResponse
            | AuthStatus::ReauthRequired
            | AuthStatus::RevocationUnconfirmed => Some(Event::Failed),
        };
        if let Some(event) = event {
            crate::analytics::record_cloud_status(app, event);
        }
        *previous = engine.status;
    }
    emit_snapshot(app, result)
}

/// Register capabilities and memory-only state during Tauri setup. The first
/// account-bound store access initializes SQLite in admitted blocking work.
pub fn initialize(
    app: &tauri::AppHandle,
    directory: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    app.add_capability(include_str!("../../cloud-capabilities/account.json"))?;
    #[cfg(debug_assertions)]
    app.add_capability(include_str!("../../cloud-capabilities/dev.json"))?;
    let logout_app = app.clone();
    app.manage(Controller {
        blocking: BlockingWork::default(),
        setup_ready: AtomicBool::new(false),
        capture: Mutex::new(capture::Host::new(directory)),
        sync: sync::Scheduler::default(),
        project_cache: Mutex::new(projects::Cache::default()),
        project_listing: BlockingWork::default(),
        project_listing_cache: Mutex::new(projects::Cache::default()),
        project_listing_authority: Mutex::new(None),
        engine: Mutex::new(
            Engine::new(
                Config::from_build(),
                Box::new(KeyringVault::new(
                    app.config().identifier.clone(),
                    directory,
                )),
                Box::new(HttpTransport),
                state::StateStore::new(directory),
            )
            .with_remote_logout_notification(move || {
                notify_remote_logout_failure(logout_app.clone())
            }),
        ),
        workspace_cleanup_failed: AtomicBool::new(false),
        attempt_generation: AtomicU64::new(0),
        account_generation: AtomicU64::new(0),
        recorded_status: Mutex::new(AuthStatus::Idle),
    });
    #[cfg(unix)]
    device_broker::start(app.clone(), directory.to_path_buf());
    sync::start(app.clone());
    Ok(())
}

/// Publish the durable warning with whichever account is current, without refreshing it.
fn notify_remote_logout_failure(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let work = app.state::<Controller>().blocking.clone();
        let _ = work
            .run(Priority::Cleanup, move || {
                let controller = app.state::<Controller>();
                let engine = controller
                    .engine
                    .lock()
                    .map_err(|_| "Cloud is unavailable.")?;
                Ok(emit_snapshot(&app, snapshot(&app, &engine)))
            })
            .await;
    });
}

/// Refresh native authority and notify all account surfaces, including when a
/// status read closes access. Callers must attach their event listener first.
#[tauri::command]
pub async fn cloud_status(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let generation = controller.account_generation.load(Ordering::Acquire);
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.generation != generation {
            return Ok(snapshot(&app, &engine));
        }
        if let Err(error) = engine.refresh(now(), || {
            controller.account_generation.load(Ordering::Acquire) == generation
        }) {
            engine.status = error.into();
        }
        Ok(emit_snapshot(&app, snapshot(&app, &engine)))
    })
    .await
}

/// Read current workspace choices without creating a grant or changing consent.
/// A missing service route fails closed.
#[tauri::command]
pub async fn cloud_workspaces(
    app: tauri::AppHandle,
    binding: DestinationBinding,
) -> Result<Vec<Workspace>, String> {
    let controller = app.state::<Controller>();
    let generation = controller.account_generation.load(Ordering::Acquire);
    let work = controller.blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let current = || controller.account_generation.load(Ordering::Acquire) == generation;
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if !current() || engine.generation != generation {
            return Err("Cloud account changed.".into());
        }
        let result = engine.eligible_workspaces(&binding, &current);
        emit_snapshot(&app, snapshot(&app, &engine));
        result
            .map(|items| items.into_iter().map(|item| item.workspace).collect())
            .map_err(|failure| {
                match failure {
                    Failure::Cancelled => "Cloud account changed.",
                    Failure::ReauthRequired => "Cloud workspace sign-in required.",
                    Failure::Rejected => "Cloud workspace access denied.",
                    _ => "Could not verify Cloud workspaces.",
                }
                .into()
            })
    })
    .await
}

/// Open the configured browser dashboard without starting device approval.
/// The renderer can choose only the dashboard root or the fixed API setup entry point.
#[tauri::command]
pub async fn cloud_open_dashboard(
    app: tauri::AppHandle,
    setup: Option<bool>,
) -> Result<(), String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        let dashboard = engine
            .config
            .as_ref()
            .and_then(|config| {
                if setup == Some(true) {
                    Some(&config.desktop_setup)
                } else {
                    config.dashboard.as_ref()
                }
            })
            .cloned()
            .ok_or("The Cloud dashboard is not configured.")?;
        drop(engine);
        app.opener()
            .open_url(dashboard.as_str(), None::<&str>)
            .map_err(|_| "Could not open the Cloud dashboard.".into())
    })
    .await
}

/// Open explicit browser approval after normal admission. A busy rejection leaves
/// the existing attempt intact. Cleanup can invalidate an admitted replacement
/// before its blocking closure starts, so it must claim the observed generation.
#[tauri::command]
pub async fn cloud_begin(
    app: tauri::AppHandle,
    access_update: Option<AccessUpdate>,
) -> Result<Snapshot, String> {
    if access_update
        .as_ref()
        .is_some_and(|update| !update.platform_available())
    {
        return Err("Cloud device access updates are not supported on this platform.".into());
    }
    let expected_generation = app
        .state::<Controller>()
        .attempt_generation
        .load(Ordering::Acquire);
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.generation != controller.account_generation.load(Ordering::Acquire) {
            return Err("Cloud account cleanup is still in progress.".to_owned());
        }
        let started_at = now();
        let generation = claim_browser_attempt(
            &mut engine,
            &controller.attempt_generation,
            expected_generation,
            started_at,
            access_update.is_some(),
        )?;
        if controller.attempt_generation.load(Ordering::Acquire) != generation {
            return Err("Cloud sign-in was cancelled.".to_owned());
        }
        let account_generation = controller.account_generation.load(Ordering::Acquire);
        if engine.generation != account_generation {
            return Err("Cloud account cleanup is still in progress.".to_owned());
        }
        if engine.config.is_none() {
            engine.status = if app
                .opener()
                .open_url("https://antiburn.com/cloud", None::<&str>)
                .is_ok()
            {
                AuthStatus::Idle
            } else {
                AuthStatus::BrowserFailed
            };
            return Ok(publish(&app, &engine));
        }
        let account_current =
            || controller.account_generation.load(Ordering::Acquire) == account_generation;
        let begun = match &access_update {
            Some(update) => engine
                .begin_access_update(started_at, generation, update.actions(), account_current)
                .map(Some),
            None => engine.begin_account_sign_in(started_at, generation, account_current),
        };
        let (url, listener, attempt) = match begun {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(publish(&app, &engine)),
            Err(error) => {
                if access_update.is_some() && engine.session().is_some() {
                    engine.access_update = Some(AccessUpdateStatus::Failed);
                } else {
                    engine.status = error.into();
                }
                return Ok(publish(&app, &engine));
            }
        };
        // A replacement sign-in cannot inherit queued account work from the
        // previous connection, even for the same user.
        let account_generation = if attempt.upgrade.is_some() {
            account_generation
        } else {
            if !advance_account_generation(
                &controller.account_generation,
                &mut engine,
                account_generation,
            ) {
                return Ok(snapshot(&app, &engine));
            }
            engine.generation
        };
        if !launch_account_browser(
            &mut engine,
            &attempt,
            || {
                controller.attempt_generation.load(Ordering::Acquire) == generation
                    && controller.account_generation.load(Ordering::Acquire) == account_generation
            },
            || app.opener().open_url(url, None::<&str>),
        )
        .map_err(|_| "Could not cancel Cloud sign-in.")?
        {
            return Ok(publish(&app, &engine));
        }
        if attempt.upgrade.is_none() {
            engine.status = AuthStatus::Waiting;
        }
        let config = engine.config.clone().ok_or("Cloud is unavailable.")?;
        let result = publish(&app, &engine);
        let callback_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let current = || {
                let controller = callback_app.state::<Controller>();
                controller.attempt_generation.load(Ordering::Acquire) == generation
                    && controller.account_generation.load(Ordering::Acquire) == account_generation
            };
            let callback = listener.wait(&config, &attempt, current).await;
            if !current() {
                return;
            }
            let work = callback_app.state::<Controller>().blocking.clone();
            let result = work
                .run(Priority::Cleanup, move || {
                    let controller = callback_app.state::<Controller>();
                    let current = || {
                        controller.attempt_generation.load(Ordering::Acquire) == generation
                            && controller.account_generation.load(Ordering::Acquire)
                                == account_generation
                    };
                    let mut engine = controller
                        .engine
                        .lock()
                        .map_err(|_| "Cloud is unavailable.")?;
                    if !current() {
                        return Ok(());
                    }
                    let replaced = if let Some(callback) = callback {
                        complete_browser_return(
                            &mut engine,
                            &attempt,
                            callback,
                            now(),
                            current,
                            |engine| {
                                publish(&callback_app, engine);
                            },
                        )
                    } else {
                        if let Err(error) = engine.cancel(generation) {
                            engine.status = error.into();
                        } else if attempt.upgrade.is_some() {
                            engine.access_update = Some(AccessUpdateStatus::Failed);
                        } else {
                            engine.status = if engine.cleanup.blocks_connection() {
                                AuthStatus::RevocationUnconfirmed
                            } else {
                                AuthStatus::Expired
                            };
                        }
                        false
                    };
                    if (replaced
                        && advance_account_generation(
                            &controller.account_generation,
                            &mut engine,
                            account_generation,
                        ))
                        || (!replaced && current())
                    {
                        publish(&callback_app, &engine);
                    }
                    Ok(())
                })
                .await;
            if result.is_err() {
                tracing::warn!(event = "cloud_callback_worker_unavailable");
            }
        });
        Ok(result)
    })
    .await
}

/// Reject live overlaps before making a listener stale. Retire an expired
/// attempt through normal cancellation to preserve revocation uncertainty.
/// The caller holds the engine lock across the generation claim and attempt start.
fn claim_browser_attempt(
    engine: &mut Engine,
    attempt_generation: &AtomicU64,
    expected: u64,
    started_at: i64,
    access_update: bool,
) -> Result<u64, String> {
    let expired = match &engine.attempt {
        Some(attempt) if attempt.expires_at <= started_at => Some(attempt.generation),
        Some(attempt) if access_update || attempt.upgrade.is_some() => {
            return Err("Cloud browser approval is already in progress.".into());
        }
        _ => None,
    };
    let generation = expected + 1;
    attempt_generation
        .compare_exchange(expected, generation, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "Cloud sign-in was cancelled.".to_owned())?;
    if let Some(expired) = expired {
        engine
            .cancel(expired)
            .map_err(|_| "Could not cancel Cloud sign-in.".to_owned())?;
    }
    Ok(generation)
}

fn advance_account_generation(
    account_generation: &AtomicU64,
    engine: &mut Engine,
    current: u64,
) -> bool {
    if account_generation
        .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    engine.generation = current + 1;
    true
}

/// Report a validated browser return before the token exchange can block.
fn complete_browser_return(
    engine: &mut Engine,
    attempt: &auth::Attempt,
    callback: auth::Callback,
    returned_at: i64,
    current: impl Fn() -> bool,
    notify_returned: impl FnOnce(&Engine),
) -> bool {
    if !current() {
        return false;
    }
    if attempt.upgrade.is_none() && matches!(callback, auth::Callback::Code(_)) {
        engine.status = AuthStatus::Returned;
        notify_returned(engine);
    }
    match engine.callback(attempt, callback, returned_at, current) {
        Ok(()) => attempt.upgrade.is_some() && engine.access_update.is_none(),
        Err(_) if attempt.upgrade.is_some() => {
            engine.access_update = Some(AccessUpdateStatus::Failed);
            false
        }
        Err(error) => {
            engine.status = error.into();
            false
        }
    }
}

/// A capability probe can finish after cancel or sign-out starts. If the
/// opener has not run, close this pending attempt without a revocation warning.
fn launch_account_browser(
    engine: &mut Engine,
    attempt: &auth::Attempt,
    current: impl FnOnce() -> bool,
    open: impl FnOnce() -> Result<(), tauri_plugin_opener::Error>,
) -> Result<bool, Failure> {
    if !current() {
        engine.abort_browser_launch(attempt)?;
        if attempt.upgrade.is_none() {
            engine.status = AuthStatus::Cancelled;
        }
        return Ok(false);
    }
    if let Err(error) = open() {
        if browser_launch_did_not_start(&error) {
            engine.abort_browser_launch(attempt)?;
        } else {
            engine.cancel(attempt.generation)?;
        }
        if attempt.upgrade.is_some() {
            engine.access_update = Some(AccessUpdateStatus::Failed);
        } else {
            engine.status = AuthStatus::BrowserFailed;
        }
        return Ok(false);
    }
    Ok(true)
}

/// A native missing-launcher error proves no handoff. Launcher exit failures and
/// other errors can follow browser launch, so they must retain remote uncertainty.
fn browser_launch_did_not_start(error: &tauri_plugin_opener::Error) -> bool {
    matches!(
        error,
        tauri_plugin_opener::Error::Io(error)
            if error.kind() == std::io::ErrorKind::NotFound && error.raw_os_error().is_some()
    )
}

/// Invalidate only browser work immediately, before any cleanup admission wait.
/// This memory-only operation does not revoke established auth or remove requests.
pub fn invalidate_pending(app: &tauri::AppHandle) {
    if let Some(controller) = app.try_state::<Controller>() {
        controller.attempt_generation.fetch_add(1, Ordering::AcqRel);
    }
}

/// Cancel a browser attempt with cleanup priority. A lost approval leaves a
/// warning but does not block reconnect. Established auth and requests survive.
#[tauri::command]
pub async fn cloud_cancel(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let generation = app
        .state::<Controller>()
        .attempt_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Cleanup, move || {
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if controller.attempt_generation.load(Ordering::Acquire) != generation {
            return Ok(snapshot(&app, &engine));
        }
        if let Err(error) = engine.cancel(generation) {
            engine.status = error.into();
        }
        Ok(publish(&app, &engine))
    })
    .await
}

/// Fence account/browser work before waiting for cleanup admission. Desktop state
/// closes access and removes saved local proof without waiting for Cloud.
/// Source bytes stay intact.
#[tauri::command]
pub async fn cloud_sign_out(app: tauri::AppHandle) -> Result<Snapshot, String> {
    invalidate_pending(&app);
    let generation = app
        .state::<Controller>()
        .account_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Cleanup, move || {
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if !engine.allows_account_cleanup(
            generation,
            controller.account_generation.load(Ordering::Acquire),
        ) {
            return Ok(snapshot(&app, &engine));
        }
        match engine.sign_out() {
            Ok(Some(proof)) => {
                engine.try_remote_logout(proof);
            }
            Ok(None) => {}
            Err(error) => engine.status = error.into(),
        }
        let store = app.state::<crate::store::Store>();
        let workspace_cleanup_failed = ensure_cloud_setup(&controller.setup_ready, &store).is_err()
            || workspace_grants::close(&store, &app.config().identifier, &engine).is_err();
        controller
            .workspace_cleanup_failed
            .store(workspace_cleanup_failed, Ordering::Release);
        engine.generation = generation;
        Ok(publish(&app, &engine))
    })
    .await
}

/// Apply an explicitly confirmed reset only when no secure refresh proof remains.
/// Fence late work only after the reset succeeds. Keep revocation uncertainty.
#[tauri::command]
pub async fn cloud_reset_local(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let expected = app
        .state::<Controller>()
        .account_generation
        .load(Ordering::Acquire);
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Cleanup, move || {
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if reset_local_account(
            &mut engine,
            &controller.account_generation,
            &controller.attempt_generation,
            expected,
        )
        .map_err(|_| "Could not reset Cloud sign-in.".to_owned())?
        {
            Ok(publish(&app, &engine))
        } else {
            Ok(snapshot(&app, &engine))
        }
    })
    .await
}

/// Run under the engine lock. Return true to publish a successful reset or a
/// storage failure for current closed recovery. Rejected resets do not fence work.
fn reset_local_account(
    engine: &mut Engine,
    account_generation: &AtomicU64,
    attempt_generation: &AtomicU64,
    expected: u64,
) -> Result<bool, Failure> {
    if engine.generation != expected || account_generation.load(Ordering::Acquire) != expected {
        return Ok(false);
    }
    if let Err(error) = engine.reset_local() {
        if account_generation.load(Ordering::Acquire) != expected {
            return Ok(false);
        }
        if error == Failure::CredentialFailed
            && engine.local_reset_available
            && engine.session().is_none()
            && engine.attempt.is_none()
        {
            engine.status = error.into();
            return Ok(true);
        }
        return Err(error);
    }
    if !advance_account_generation(account_generation, engine, expected) {
        return Ok(false);
    }
    attempt_generation.fetch_add(1, Ordering::AcqRel);
    Ok(true)
}

/// Save account-bound preferences in admitted blocking work. Counts are display
/// data, and no preference write creates content consent or retargets requests.
#[tauri::command]
pub async fn cloud_save_draft(
    app: tauri::AppHandle,
    mut value: Draft,
    binding: DestinationBinding,
    folders_edited: Option<bool>,
) -> Result<Snapshot, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        if value.folders.len() > 100
            || value.folders.iter().any(|folder| {
                folder.path.is_empty()
                    || folder.path.len() > 4096
                    || !Path::new(&folder.path).is_absolute()
            })
        {
            return Err("Choose valid local folders.".to_owned());
        }
        let controller = app.state::<Controller>();
        let engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        let session = engine
            .session()
            .ok_or("Connect to Cloud before choosing folders.")?;
        if engine.generation != controller.account_generation.load(Ordering::Acquire)
            || !binding.matches(session)
        {
            return Err("Cloud account changed. Choose folders again.".to_owned());
        }
        let store = app.state::<crate::store::Store>();
        let previous = draft(&app, Some(session))?;
        value.folders_initialized = folders_edited.unwrap_or(true) || previous.folders_initialized;
        for folder in &mut value.folders {
            folder.session_count = None;
            folder.session_count_is_partial = false;
        }
        if capture::selected_roots(&previous.folders) != capture::selected_roots(&value.folders)
            && let Some(policy) = store
                .cloud_sync_policy_for_origins(
                    &session.user.id,
                    &session.workspace.id,
                    &session.resource,
                    &session.iss,
                )
                .map_err(|_| "Could not read Cloud sync consent.")?
        {
            controller.sync.generation.fetch_add(1, Ordering::AcqRel);
            store
                .set_cloud_sync_paused(&policy, true)
                .map_err(|_| "Could not pause Cloud sync for the changed sources.")?;
        }
        let encoded = serde_json::to_string(&value).map_err(|_| "Could not save Cloud setup.")?;
        store
            .save_cloud_draft(&session.user.id, &session.workspace.id, &encoded)
            .map_err(|_| "Could not save Cloud setup.")?;
        Ok(publish(&app, &engine))
    })
    .await
}

/// Read whether leaving onboarding must cancel its browser attempt. SQLite waits
/// use cleanup admission, so they never block the UI or a Tokio runtime thread.
pub(crate) async fn onboarding_requires_cancel(
    app: tauri::AppHandle,
    requested: crate::store::OnboardingState,
) -> Result<bool, String> {
    if requested == crate::store::OnboardingState::CloudUnavailable {
        return Ok(false);
    }
    let work = app.state::<Controller>().blocking.clone();
    let store = app.state::<crate::store::Store>().inner().clone();
    work.run(Priority::Cleanup, move || {
        store
            .onboarding_state()
            .map(|state| state == crate::store::OnboardingState::CloudUnavailable)
            .map_err(|_| "Could not read onboarding progress.".to_owned())
    })
    .await
}
