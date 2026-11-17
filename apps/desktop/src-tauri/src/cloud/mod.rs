mod auth;
mod capture;
mod engine;
#[cfg(test)]
mod http_tests;
mod loopback;
mod state;
#[cfg(test)]
mod tests;

use auth::{Config, Failure, HttpTransport, Identity, KeyringVault, Workspace};
use engine::Engine;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

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
            Failure::Offline => Self::Offline,
            Failure::Rejected => Self::Rejected,
            Failure::InvalidResponse => Self::InvalidResponse,
            Failure::CredentialFailed => Self::CredentialFailed,
            Failure::Cancelled => Self::Cancelled,
            Failure::Unavailable => Self::Unavailable,
            Failure::ReauthRequired => Self::ReauthRequired,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    path: String,
    selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_count: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    session_count_is_partial: bool,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    #[default]
    Handoff,
    Identity,
    Folders,
    Complete,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    screen: Screen,
    folders: Vec<Folder>,
    #[serde(default = "existing_folder_choices")]
    folders_initialized: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DestinationBinding {
    account_id: String,
    destination_id: String,
}

impl DestinationBinding {
    fn matches(&self, session: &auth::Session) -> bool {
        self.account_id == session.user.id
            && self.destination_id == session.workspace.id
            && session.workspace.kind == auth::WorkspaceKind::Personal
    }
}

fn existing_folder_choices() -> bool {
    true
}

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

pub(crate) struct Controller {
    engine: Mutex<Engine>,
    capture: Mutex<capture::Host>,
    attempt_generation: AtomicU64,
    account_generation: AtomicU64,
    recorded_status: Mutex<AuthStatus>,
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn draft(app: &tauri::AppHandle, session: Option<&auth::Session>) -> Result<Draft, String> {
    let Some(session) = session else {
        return Ok(Draft::default());
    };
    let store = app.state::<crate::store::Store>();
    let saved = store
        .cloud_draft(&session.user.id, &session.workspace.id)
        .map_err(|_| "Could not read Cloud setup.")?
        .map(|value| serde_json::from_str::<Draft>(&value))
        .transpose()
        .map_err(|_| "Could not read Cloud setup.")?;
    Ok(saved.unwrap_or_default())
}

fn snapshot(app: &tauri::AppHandle, engine: &Engine) -> Snapshot {
    let current = engine.generation
        == app
            .state::<Controller>()
            .account_generation
            .load(Ordering::Acquire);
    let session = if current { engine.session() } else { None };
    let (mut draft, mut setup_error) = match draft(app, session) {
        Ok(draft) => (draft, None),
        Err(error) => (Draft::default(), Some(error)),
    };
    if session.is_none() {
        draft.screen = Screen::Handoff;
    }
    let pending_sync = session.and_then(|session| {
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
    if session.is_some() && !cfg!(unix) {
        setup_error = Some("Native capture is not supported on this platform.".into());
    }
    Snapshot {
        auth_enabled: engine.config.is_some(),
        capture_enabled: cfg!(unix) && session.is_some() && setup_error.is_none(),
        // Authentication alone does not authorize capture.
        sync_enabled: false,
        status: if current {
            engine.status
        } else {
            AuthStatus::Cancelled
        },
        identity: session.map(Identity::from),
        expires_at: engine.attempt.as_ref().map(|attempt| attempt.expires_at),
        grant_expires_at: session.map(|session| session.expires_at),
        destination: session.map(|session| session.workspace.clone()),
        revocation_pending: matches!(engine.cleanup, state::Cleanup::Revoke { .. }),
        local_cleanup_pending: matches!(engine.cleanup, state::Cleanup::DeleteCredential { .. }),
        local_reset_available: engine.local_reset_available,
        remote_revocation_unconfirmed: engine.cleanup.prior_unconfirmed(),
        setup_error,
        pending_sync,
        draft,
    }
}

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
    for window in ["onboarding", "main", "settings"] {
        let _ = app.emit_to(window, "cloud:changed", result.clone());
    }
    result
}

pub fn initialize(
    app: &tauri::AppHandle,
    directory: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    app.state::<crate::store::Store>()
        .initialize_cloud_setup()?;
    app.add_capability(include_str!("../../cloud-capabilities/account.json"))?;
    #[cfg(debug_assertions)]
    app.add_capability(include_str!("../../cloud-capabilities/dev.json"))?;
    app.manage(Controller {
        capture: Mutex::new(capture::Host::new(directory)),
        engine: Mutex::new(Engine::new(
            Config::from_build(),
            Box::new(KeyringVault::new(
                app.config().identifier.clone(),
                directory,
            )),
            Box::new(HttpTransport),
            state::StateStore::new(directory),
        )),
        attempt_generation: AtomicU64::new(0),
        account_generation: AtomicU64::new(0),
        recorded_status: Mutex::new(AuthStatus::Idle),
    });
    Ok(())
}

#[tauri::command]
pub async fn cloud_status(app: tauri::AppHandle) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
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
        Ok(snapshot(&app, &engine))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

#[tauri::command]
pub async fn cloud_begin(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let generation = app
        .state::<Controller>()
        .attempt_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    tauri::async_runtime::spawn_blocking(move || {
        let controller = app.state::<Controller>();
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
        if app.opener().open_url(url, None::<&str>).is_err() {
            engine
                .cancel(generation)
                .map_err(|_| "Could not cancel Cloud sign-in.")?;
            engine.status = AuthStatus::BrowserFailed;
            return Ok(publish(&app, &engine));
        }
        engine.status = AuthStatus::Waiting;
        let config = engine.config.clone().ok_or("Cloud is unavailable.")?;
        let result = publish(&app, &engine);
        let callback_app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let current = || {
                let controller = callback_app.state::<Controller>();
                controller.attempt_generation.load(Ordering::Acquire) == generation
                    && controller.account_generation.load(Ordering::Acquire) == account_generation
            };
            let callback = listener.wait(&config, &attempt, current);
            let controller = callback_app.state::<Controller>();
            let Ok(mut engine) = controller.engine.lock() else {
                return;
            };
            if !current() {
                return;
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
        });
        Ok(result)
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

pub fn invalidate_pending(app: &tauri::AppHandle) {
    if let Some(controller) = app.try_state::<Controller>() {
        controller.attempt_generation.fetch_add(1, Ordering::AcqRel);
    }
}

#[tauri::command]
pub async fn cloud_cancel(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let generation = app
        .state::<Controller>()
        .attempt_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    tauri::async_runtime::spawn_blocking(move || {
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
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

#[tauri::command]
pub async fn cloud_sign_out(app: tauri::AppHandle) -> Result<Snapshot, String> {
    invalidate_pending(&app);
    let generation = app
        .state::<Controller>()
        .account_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    tauri::async_runtime::spawn_blocking(move || {
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
        engine.generation = generation;
        Ok(publish(&app, &engine))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

#[tauri::command]
pub async fn cloud_reset_local(app: tauri::AppHandle) -> Result<Snapshot, String> {
    invalidate_pending(&app);
    let generation = app
        .state::<Controller>()
        .account_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    tauri::async_runtime::spawn_blocking(move || {
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
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

#[tauri::command]
pub async fn cloud_save_draft(
    app: tauri::AppHandle,
    mut value: Draft,
    binding: DestinationBinding,
    folders_edited: Option<bool>,
) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
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
        let previous = store
            .cloud_draft(&session.user.id, &session.workspace.id)
            .map_err(|_| "Could not read Cloud setup.")?
            .map(|raw| serde_json::from_str::<Draft>(&raw))
            .transpose()
            .map_err(|_| "Could not read Cloud setup.")?;
        value.folders_initialized = folders_edited.unwrap_or(true)
            || previous.is_some_and(|draft| draft.folders_initialized);
        for folder in &mut value.folders {
            folder.session_count = None;
            folder.session_count_is_partial = false;
        }
        let encoded = serde_json::to_string(&value).map_err(|_| "Could not save Cloud setup.")?;
        store
            .save_cloud_draft(&session.user.id, &session.workspace.id, &encoded)
            .map_err(|_| "Could not save Cloud setup.")?;
        Ok(publish(&app, &engine))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

#[tauri::command]
pub async fn cloud_prepare_sync(
    app: tauri::AppHandle,
    request_id: String,
    folders: Vec<Folder>,
    consent: capture::Consent,
) -> Result<capture::SyncResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !cfg!(unix) {
            return Err("Native capture is not supported on this platform.".into());
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
        let session = engine
            .session()
            .ok_or("Connect to Cloud before preparing capture.")?;
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
                now(),
            )
            .map_err(|_| "Could not save the capture request. No source content was read.")?;
        publish(&app, &engine);
        Ok(capture::SyncResult::from(&request))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

enum CaptureAction {
    Discover,
    Read,
    Cancel,
}

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
    let session = engine
        .session()
        .ok_or("Connect to the original Cloud account to view this request.")?;
    let store = app.state::<crate::store::Store>();
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
            .map_err(|_| "Native capture is unavailable.")?
            .discover(&store, &request, current)?,
    };
    publish(app, &engine);
    Ok(capture::SyncResult::from(&request))
}

#[tauri::command]
pub async fn cloud_start_sync(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<capture::SyncResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        capture_request(&app, &request_id, CaptureAction::Discover)
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

#[tauri::command]
pub async fn cloud_reconcile_sync(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<capture::SyncResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        capture_request(&app, &request_id, CaptureAction::Read)
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}

#[tauri::command]
pub async fn cloud_cancel_sync(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<capture::SyncResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        capture_request(&app, &request_id, CaptureAction::Cancel)
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}
