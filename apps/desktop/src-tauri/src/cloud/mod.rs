mod auth;
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
    draft: Draft,
}

pub(crate) struct Controller {
    engine: Mutex<Engine>,
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
    let repositories = store
        .repositories()
        .map_err(|_| "Could not read Cloud folders.")?;
    let mut result = saved.unwrap_or_default();
    if !result.folders_initialized {
        let mut folders = Vec::<Folder>::new();
        for repository in &repositories {
            if let Some(path) = &repository.repo_root
                && repository.enabled
                && Path::new(path).is_absolute()
                && !folders.iter().any(|folder| &folder.path == path)
                && folders.len() < 100
            {
                folders.push(Folder {
                    path: path.clone(),
                    selected: false,
                    session_count: None,
                    session_count_is_partial: false,
                });
            }
        }
        result.folders = folders;
    }
    for folder in &mut result.folders {
        folder.session_count = repositories
            .iter()
            .filter(|repository| repository.repo_root.as_deref() == Some(folder.path.as_str()))
            .map(|repository| repository.session_count)
            .max();
        folder.session_count_is_partial = folder.session_count.is_some();
    }
    Ok(result)
}

fn snapshot(app: &tauri::AppHandle, engine: &Engine) -> Snapshot {
    let current = engine.generation
        == app
            .state::<Controller>()
            .account_generation
            .load(Ordering::Acquire);
    let session = if current { engine.session() } else { None };
    let (mut draft, setup_error) = match draft(app, session) {
        Ok(draft) => (draft, None),
        Err(error) => (Draft::default(), Some(error)),
    };
    if session.is_none() {
        draft.screen = Screen::Handoff;
    }
    Snapshot {
        auth_enabled: engine.config.is_some(),
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
        engine.generation = generation;
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
        engine.generation = account_generation;
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
    invalidate_pending(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if let Err(error) = engine.cancel(controller.attempt_generation.load(Ordering::Acquire)) {
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
pub fn cloud_start_sync() -> Result<(), String> {
    Err("Cloud capture is not available. No sessions were queued or uploaded.".to_owned())
}
