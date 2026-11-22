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
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
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
    capture_enabled: bool,
    sync_enabled: bool,
    status: AuthStatus,
    identity: Option<Identity>,
    expires_at: Option<i64>,
    grant_expires_at: Option<i64>,
    destination: Option<Workspace>,
    revocation_pending: bool,
    local_cleanup_pending: bool,
    local_reset_available: bool,
    remote_revocation_unconfirmed: bool,
    setup_error: Option<String>,
    pending_sync: Option<capture::PendingSync>,
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
    let pending_sync = session
        .filter(|_| setup_error.is_none())
        .and_then(|session| {
            match app
                .state::<crate::store::Store>()
                .current_cloud_capture_request(
                    &session.user.id,
                    &session.workspace.id,
                    &capture::selected_roots(&draft.folders),
                ) {
                Ok(request) => request.map(Into::into),
                Err(_) => {
                    setup_error = Some("Could not read saved capture requests.".into());
                    None
                }
            }
        });
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
    let pending_sync = pending_sync.filter(|_| current && session.is_some());
    if !current || session.is_none() {
        draft = Draft::default();
        setup_error = None;
    }
    Snapshot {
        auth_enabled: engine.config.is_some(),
        capture_enabled: cfg!(unix) && session.is_some() && setup_error.is_none(),
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
        destination: session.map(|session| session.workspace.clone()),
        revocation_pending: matches!(engine.cleanup, state::Cleanup::Revoke { .. }),
        local_cleanup_pending: matches!(engine.cleanup, state::Cleanup::DeleteCredential { .. }),
        local_reset_available: engine.local_reset_available,
        remote_revocation_unconfirmed: team_cleanup_unconfirmed
            || app
                .state::<Controller>()
                .workspace_cleanup_failed
                .load(Ordering::Acquire)
            || engine.cleanup.prior_unconfirmed(),
        setup_error,
        pending_sync,
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
    app.manage(Controller {
        blocking: BlockingWork::default(),
        setup_ready: AtomicBool::new(false),
        capture: Mutex::new(capture::Host::new(directory)),
        sync: sync::Scheduler::default(),
        project_cache: Mutex::new(projects::Cache::default()),
        engine: Mutex::new(Engine::new(
            Config::from_build(),
            Box::new(KeyringVault::new(
                app.config().identifier.clone(),
                directory,
            )),
            Box::new(HttpTransport),
            state::StateStore::new(directory),
        )),
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
            .map_err(|_| "Could not verify Cloud workspaces.".into())
    })
    .await
}

/// Open explicit browser approval after normal admission. A busy rejection leaves
/// the existing attempt intact. Cleanup can invalidate an admitted replacement
/// before its blocking closure starts, so it must claim the observed generation.
#[tauri::command]
pub async fn cloud_begin(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let expected_generation = app
        .state::<Controller>()
        .attempt_generation
        .load(Ordering::Acquire);
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let generation = expected_generation + 1;
        controller
            .attempt_generation
            .compare_exchange(
                expected_generation,
                generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| "Cloud sign-in was cancelled.".to_owned())?;
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
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
        let (url, listener, attempt) = match engine.begin(now(), generation) {
            Ok(value) => value,
            Err(error) => {
                engine.status = error.into();
                return Ok(publish(&app, &engine));
            }
        };
        if let Err(error) = app.opener().open_url(url, None::<&str>) {
            if browser_launch_did_not_start(&error) {
                engine.abort_browser_launch(&attempt)
            } else {
                engine.cancel(generation)
            }
            .map_err(|_| "Could not cancel Cloud sign-in.")?;
            engine.status = AuthStatus::BrowserFailed;
            return Ok(publish(&app, &engine));
        }
        engine.status = AuthStatus::Waiting;
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
                    if let Some(callback) = callback {
                        if let Err(error) = engine.callback(&attempt, callback, now(), current) {
                            engine.status = error.into();
                        }
                    } else {
                        if let Err(error) = engine.cancel(generation) {
                            engine.status = error.into();
                        } else {
                            engine.status = if engine.cleanup.blocks_connection() {
                                AuthStatus::RevocationUnconfirmed
                            } else {
                                AuthStatus::Expired
                            };
                        }
                    }
                    if current() {
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

/// Cancel a browser attempt with cleanup priority, retaining uncertain approval
/// for explicit recovery. Established auth and saved source requests survive.
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
/// closes access before revocation and secure deletion; source bytes stay intact.
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
        if let Err(error) = engine.sign_out() {
            engine.status = error.into();
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
/// Invalidate late work immediately and preserve the remote-revocation warning.
#[tauri::command]
pub async fn cloud_reset_local(app: tauri::AppHandle) -> Result<Snapshot, String> {
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
        if let Err(error) = engine.reset_local() {
            engine.status = error.into();
        }
        engine.generation = generation;
        Ok(publish(&app, &engine))
    })
    .await
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

/// Suggest a picker directory without enumerating stores or granting source access.
/// Only explicit file selection and content consent can create an OpenCode request.
#[tauri::command]
pub(crate) async fn cloud_source_picker_directory(
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, || {
        Ok(antiburn_local::discovery::agents::opencode::data_roots()
            .into_iter()
            .filter(|path| path.is_absolute())
            .filter_map(|path| path.to_str().map(str::to_owned))
            .find(|path| path.len() <= 4096 && !path.chars().any(char::is_control)))
    })
    .await
}

/// Persist exact full-content consent and roots before source-name discovery.
/// Reusing a request ID must preserve its original account/destination selection.
#[tauri::command]
pub async fn cloud_prepare_sync(
    app: tauri::AppHandle,
    request_id: String,
    folders: Vec<Folder>,
    consent: capture::Consent,
) -> Result<capture::SyncResult, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        if !cfg!(unix) {
            return Err("Desktop capture is not supported on this platform.".into());
        }
        if !capture::valid_request_id(&request_id) || folders.len() > 100 {
            return Err("Choose a valid capture request and source selection.".into());
        }
        let controller = app.state::<Controller>();
        let generation = controller.account_generation.load(Ordering::Acquire);
        let current = || controller.account_generation.load(Ordering::Acquire) == generation;
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.generation != generation {
            return Err("Cloud account changed. Choose sources again.".into());
        }
        if let Err(error) = engine.refresh(now(), current) {
            engine.status = error.into();
            publish(&app, &engine);
            return Err("Connect to Cloud before preparing capture.".into());
        }
        let Some(session) = engine.session() else {
            publish(&app, &engine);
            return Err("Connect to Cloud before preparing capture.".into());
        };
        let roots =
            capture::validate_selection(session, &draft(&app, Some(session))?, &folders, &consent)?;
        if !current() {
            return Err("Cloud account changed. Choose sources again.".into());
        }
        let request = app
            .state::<crate::store::Store>()
            .prepare_cloud_capture(
                &request_id,
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                now(),
            )
            .map_err(|_| "Could not save the capture request. No source content was read.")?;
        publish(&app, &engine);
        Ok(capture::SyncResult::from(&request))
    })
    .await
}

/// Share current-account checks across saved-request read, discovery, and cancellation.
enum CaptureAction {
    Discover,
    Read,
    Cancel,
}

/// Run under blocking admission. Resolve the original account's durable request
/// before touching library/storage state; never capture content or start upload.
fn capture_request(
    app: &tauri::AppHandle,
    request_id: &str,
    action: CaptureAction,
) -> Result<capture::SyncResult, String> {
    if !capture::valid_request_id(request_id) {
        return Err("Capture request is unavailable.".into());
    }
    let controller = app.state::<Controller>();
    let generation = controller.account_generation.load(Ordering::Acquire);
    let current = || controller.account_generation.load(Ordering::Acquire) == generation;
    let mut engine = controller
        .engine
        .lock()
        .map_err(|_| "Cloud is unavailable.")?;
    if engine.generation != generation {
        return Err("Cloud account changed. Your saved request is kept.".into());
    }
    if let Err(error) = engine.refresh(now(), current) {
        engine.status = error.into();
        publish(app, &engine);
        return Err("Connect to the original Cloud account to view this request.".into());
    }
    let Some(session) = engine.session() else {
        publish(app, &engine);
        return Err("Connect to the original Cloud account to view this request.".into());
    };
    let store = app.state::<crate::store::Store>();
    ensure_cloud_setup(&controller.setup_ready, &store)?;
    let request = store
        .cloud_capture_request(request_id, &session.user.id, &session.workspace.id)
        .map_err(|_| "Capture request is unavailable.")?;
    let request = match action {
        CaptureAction::Read => request,
        CaptureAction::Cancel => store
            .cancel_cloud_capture_request(&request)
            .map_err(|_| "Could not cancel the saved request.")?,
        CaptureAction::Discover => controller
            .capture
            .lock()
            .map_err(|_| "Desktop capture is unavailable.")?
            .discover(&store, &request, current)?,
    };
    publish(app, &engine);
    Ok(capture::SyncResult::from(&request))
}

/// List names for a saved selection once. Retries keep IDs and completed results;
/// `UploadUnavailable` confirms neither payload reading nor service acceptance.
#[tauri::command]
pub async fn cloud_start_sync(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<capture::SyncResult, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        capture_request(&app, &request_id, CaptureAction::Discover)
    })
    .await
}

/// Read saved request state only, without rescan, payload reads, or upload submission.
#[tauri::command]
pub async fn cloud_reconcile_sync(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<capture::SyncResult, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        capture_request(&app, &request_id, CaptureAction::Read)
    })
    .await
}

/// Mark the original account's request cancelled with cleanup admission.
/// This does not delete spools, native source, or remote content.
#[tauri::command]
pub async fn cloud_cancel_sync(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<capture::SyncResult, String> {
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Cleanup, move || {
        capture_request(&app, &request_id, CaptureAction::Cancel)
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
