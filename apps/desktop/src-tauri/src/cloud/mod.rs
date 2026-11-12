mod auth;
#[cfg(test)]
mod tests;

use auth::{
    Attempt, Callback, Config, Failure, HttpTransport, Identity, KeyringVault, Secrets, Transport,
    Vault,
};
use serde::{Deserialize, Serialize};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use url::Url;

const DRAFT_KEY: &str = "internal:cloudOnboardingDraft";

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
    draft: Draft,
}
struct Access {
    token: String,
    expires_at: i64,
}
struct Engine {
    config: Option<Config>,
    scheme: String,
    vault: Box<dyn Vault>,
    transport: Box<dyn Transport>,
    loaded: bool,
    attempt_generation: u64,
    account_generation: u64,
    secrets: Secrets,
    access: Option<Access>,
    identity: Option<Identity>,
    status: AuthStatus,
}
impl Engine {
    fn load(&mut self, now: i64) -> Result<(), Failure> {
        if self.loaded {
            return Ok(());
        }
        self.secrets = self.vault.read()?;
        if self
            .secrets
            .pending
            .as_ref()
            .is_some_and(|pending| pending.verifier.is_none())
        {
            self.secrets.pending = None;
            self.vault.write(&self.secrets)?;
        }
        if let Some(pending) = &self.secrets.pending {
            if pending.expires_at <= now {
                self.invalidate(AuthStatus::Expired)?;
            } else {
                self.status = AuthStatus::Waiting;
            }
        } else if self.status == AuthStatus::CredentialFailed {
            self.status = AuthStatus::Idle;
        }
        self.loaded = true;
        Ok(())
    }
    fn invalidate(&mut self, status: AuthStatus) -> Result<(), Failure> {
        if self.secrets.pending.is_none() {
            return Ok(());
        }
        self.secrets.pending = None;
        self.status = status;
        self.vault.write(&self.secrets)
    }
    fn begin(&mut self, now: i64, generation: u64) -> Result<String, Failure> {
        if self.config.is_none() {
            self.status = AuthStatus::Idle;
            return Ok("https://antiburn.com/cloud".to_owned());
        }
        self.load(now)?;
        if self
            .secrets
            .pending
            .as_ref()
            .is_some_and(|attempt| attempt.expires_at > now)
        {
            return Err(Failure::Unavailable);
        }
        let state = auth::random()?;
        let verifier = self.config.as_ref().map(|_| auth::random()).transpose()?;
        let attempt = Attempt {
            state,
            verifier,
            expires_at: now + 600,
        };
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let mut url = config.authorization.clone();
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &config.client_id),
            ("redirect_uri", &config.callback),
            ("scope", "openid profile offline_access"),
            ("state", &attempt.state),
            (
                "code_challenge",
                &auth::challenge(attempt.verifier.as_deref().ok_or(Failure::Unavailable)?),
            ),
            ("code_challenge_method", "S256"),
        ]);
        self.attempt_generation = generation;
        self.secrets.pending = Some(attempt);
        self.vault.write(&self.secrets)?;
        self.status = AuthStatus::Opening;
        Ok(url.to_string())
    }
    fn browser_result(&mut self, launched: bool) -> Result<(), Failure> {
        if self.config.is_none() {
            self.status = if launched {
                AuthStatus::Idle
            } else {
                AuthStatus::BrowserFailed
            };
            return Ok(());
        }
        if launched {
            self.status = AuthStatus::Waiting;
            Ok(())
        } else {
            self.invalidate(AuthStatus::BrowserFailed)
        }
    }
    fn accept_tokens(
        &mut self,
        tokens: auth::Tokens,
        now: i64,
        rotation: bool,
        current: impl Fn() -> bool,
    ) -> Result<(), Failure> {
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        if !current() {
            let _ = self.transport.revoke(config, &tokens.refresh_token);
            return Err(Failure::Cancelled);
        }
        // Save rotated credentials before profile requests so offline errors do not lose the session.
        if rotation {
            self.secrets.refresh_token = Some(tokens.refresh_token.clone());
            if self.vault.write(&self.secrets).is_err() {
                let _ = self.transport.revoke(config, &tokens.refresh_token);
                self.secrets.refresh_token = None;
                return Err(Failure::CredentialFailed);
            }
        }
        let identity = self.transport.profile(config, &tokens.access_token);
        if !current() {
            let _ = self.transport.revoke(config, &tokens.refresh_token);
            self.access = None;
            self.identity = None;
            if rotation {
                self.secrets.refresh_token = None;
                self.vault.write(&self.secrets)?;
            }
            return Err(Failure::Cancelled);
        }
        let identity = match identity {
            Ok(identity) => identity,
            Err(error) => {
                if !rotation || error == Failure::Rejected {
                    let _ = self.transport.revoke(config, &tokens.refresh_token);
                }
                if rotation && error == Failure::Rejected {
                    self.secrets.refresh_token = None;
                    self.vault.write(&self.secrets)?;
                }
                return Err(error);
            }
        };
        if !rotation {
            let previous = self
                .secrets
                .refresh_token
                .replace(tokens.refresh_token.clone());
            if self.vault.write(&self.secrets).is_err() {
                self.secrets.refresh_token = previous;
                let _ = self.transport.revoke(config, &tokens.refresh_token);
                return Err(Failure::CredentialFailed);
            }
            if !current() {
                let _ = self.transport.revoke(config, &tokens.refresh_token);
                self.secrets.refresh_token = previous;
                self.vault.write(&self.secrets)?;
                return Err(Failure::Cancelled);
            }
        }
        self.access = Some(Access {
            token: tokens.access_token,
            expires_at: now + tokens.expires_in as i64,
        });
        self.identity = Some(identity);
        self.status = AuthStatus::SignedIn;
        Ok(())
    }
    fn callback(
        &mut self,
        raw: &str,
        now: i64,
        generation: u64,
        current: impl Fn() -> bool,
    ) -> Result<bool, Failure> {
        if self.config.is_none()
            || !raw.starts_with(&format!("{}://cloud/auth?", self.scheme))
            || self.load(now).is_err()
        {
            return Ok(false);
        }
        if self.attempt_generation != generation || !current() {
            return Ok(false);
        }
        let Some(attempt) = self.secrets.pending.clone() else {
            return Ok(false);
        };
        if attempt.verifier.is_none() {
            return Ok(false);
        }
        let Some(callback) = auth::validate_callback(raw, &self.scheme, &attempt, now) else {
            return Ok(false);
        };
        self.invalidate(AuthStatus::Waiting)?;
        match callback {
            Callback::Marketing => return Ok(false),
            Callback::Cancelled => self.status = AuthStatus::Cancelled,
            Callback::Code(code) => {
                let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
                let tokens = self.transport.exchange(
                    config,
                    &[
                        ("grant_type", "authorization_code"),
                        ("code", &code),
                        (
                            "code_verifier",
                            attempt.verifier.as_deref().ok_or(Failure::Unavailable)?,
                        ),
                        ("client_id", &config.client_id),
                        ("redirect_uri", &config.callback),
                    ],
                )?;
                if let Err(error) = self.accept_tokens(tokens, now, false, current) {
                    self.status = error.into();
                }
            }
        }
        Ok(true)
    }
    fn refresh(&mut self, now: i64, current: impl Fn() -> bool) -> Result<(), Failure> {
        if self.config.is_none() {
            return Ok(());
        }
        self.load(now)?;
        if self
            .secrets
            .pending
            .as_ref()
            .is_some_and(|pending| pending.expires_at <= now)
        {
            self.invalidate(AuthStatus::Expired)?;
        }
        if self
            .access
            .as_ref()
            .is_some_and(|access| access.expires_at > now + 60 && !access.token.is_empty())
            || self.secrets.pending.is_some()
        {
            return Ok(());
        }
        let (Some(config), Some(refresh)) = (&self.config, &self.secrets.refresh_token) else {
            return Ok(());
        };
        self.identity = None;
        self.access = None;
        let tokens = self.transport.exchange(
            config,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("client_id", &config.client_id),
            ],
        );
        match tokens {
            Ok(tokens) => self.accept_tokens(tokens, now, true, current),
            Err(Failure::Rejected) => {
                self.secrets.refresh_token = None;
                self.access = None;
                self.identity = None;
                self.vault.write(&self.secrets)?;
                Err(Failure::Rejected)
            }
            Err(error) => Err(error),
        }
    }
    fn sign_out(&mut self) -> Result<(), Failure> {
        self.load(now())?;
        let refresh = self.secrets.refresh_token.clone();
        self.access = None;
        self.identity = None;
        let cleared = self.vault.write(&Secrets::default());
        if let (Some(config), Some(refresh)) = (&self.config, refresh) {
            let _ = self.transport.revoke(config, &refresh);
        }
        cleared?;
        self.secrets = Secrets::default();
        self.status = AuthStatus::SignedOut;
        Ok(())
    }
}
#[derive(Default)]
pub(crate) struct PendingLinks {
    urls: Mutex<std::collections::VecDeque<(String, u64)>>,
    running: AtomicBool,
}

pub(crate) struct Controller {
    engine: Mutex<Engine>,
    generation: AtomicU64,
    account_generation: AtomicU64,
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn draft(app: &tauri::AppHandle, discover: bool) -> Draft {
    let store = app.state::<crate::store::Store>();
    let saved = store
        .internal_value(DRAFT_KEY)
        .and_then(|value| serde_json::from_str::<Draft>(&value).ok());
    if !discover {
        return saved.unwrap_or_default();
    }
    let repositories = store.repositories().unwrap_or_default();
    let mut result = saved.unwrap_or_default();
    if !result.folders_initialized {
        let mut folders = Vec::<Folder>::new();
        for repository in &repositories {
            if let Some(path) = &repository.repo_root
                && repository.enabled
                && std::path::Path::new(path).is_absolute()
                && !folders.iter().any(|folder| &folder.path == path)
                && folders.len() < 100
            {
                folders.push(Folder {
                    path: path.clone(),
                    selected: true,
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
    result
}
fn snapshot(app: &tauri::AppHandle, engine: &Engine) -> Snapshot {
    let mut draft = draft(app, engine.identity.is_some());
    if engine.identity.is_none() {
        draft.screen = Screen::Handoff;
    }
    let result = Snapshot {
        auth_enabled: engine.config.is_some(),
        sync_enabled: false,
        status: engine.status,
        identity: engine.identity.clone(),
        expires_at: engine
            .secrets
            .pending
            .as_ref()
            .map(|pending| pending.expires_at),
        draft,
    };
    snapshot_for_account(
        result,
        engine.account_generation,
        app.state::<Controller>()
            .account_generation
            .load(Ordering::Acquire),
    )
}
fn snapshot_for_account(
    mut result: Snapshot,
    account_generation: u64,
    current_generation: u64,
) -> Snapshot {
    if account_generation != current_generation {
        result.identity = None;
        result.draft.screen = Screen::Handoff;
        if result.status != AuthStatus::CredentialFailed {
            result.status = AuthStatus::Unavailable;
        }
    }
    result
}
fn snapshot_for_window(mut result: Snapshot, window: &str) -> Snapshot {
    if window != "onboarding" {
        result.draft = Draft::default();
    }
    result
}
fn publish(app: &tauri::AppHandle, engine: &Engine) -> Snapshot {
    let result = snapshot(app, engine);
    for window in ["onboarding", "main", "settings"] {
        let _ = app.emit_to(
            window,
            "cloud:changed",
            snapshot_for_window(result.clone(), window),
        );
    }
    result
}
fn completed(app: &tauri::AppHandle) -> bool {
    app.state::<crate::store::Store>()
        .onboarding_state()
        .map_or(true, |state| {
            state == crate::store::OnboardingState::Completed
        })
}
pub fn initialize(app: &tauri::AppHandle) {
    let identifier = &app.config().identifier;
    let scheme = if identifier == "ai.antiburn.desktop" {
        "antiburn".to_owned()
    } else {
        identifier.replace('.', "-")
    };
    app.manage(Controller {
        engine: Mutex::new(Engine {
            config: Config::from_build(&scheme),
            scheme,
            vault: Box::new(KeyringVault(identifier.clone())),
            transport: Box::new(HttpTransport),
            loaded: false,
            attempt_generation: 0,
            account_generation: 0,
            secrets: Secrets::default(),
            access: None,
            identity: None,
            status: AuthStatus::Idle,
        }),
        generation: AtomicU64::new(0),
        account_generation: AtomicU64::new(0),
    });
    schedule_callbacks(app);
    for raw in std::env::args_os()
        .skip(1)
        .filter_map(|arg| arg.into_string().ok())
    {
        receive(app, raw);
    }
}
fn callback_route(app: &tauri::AppHandle, raw: &str) -> bool {
    let identifier = &app.config().identifier;
    let scheme = if identifier == "ai.antiburn.desktop" {
        "antiburn".to_owned()
    } else {
        identifier.replace('.', "-")
    };
    if raw.len() > 8192 {
        return false;
    }
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    url.scheme() == scheme
        && url.host_str() == Some("cloud")
        && url.path() == "/auth"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none()
}
pub fn receive(app: &tauri::AppHandle, raw: String) {
    if !callback_route(app, &raw) {
        return;
    }
    let generation = app.try_state::<Controller>().map_or(0, |controller| {
        controller.generation.load(Ordering::Acquire)
    });
    {
        let pending = app.state::<PendingLinks>();
        let Ok(mut queue) = pending.urls.lock() else {
            return;
        };
        if queue.len() >= 16 {
            return;
        }
        queue.push_back((raw, generation));
    }
    schedule_callbacks(app);
}
fn schedule_callbacks(app: &tauri::AppHandle) {
    let pending = app.state::<PendingLinks>();
    let Ok(queue) = pending.urls.lock() else {
        return;
    };
    if app.try_state::<Controller>().is_none()
        || queue.is_empty()
        || pending.running.swap(true, Ordering::AcqRel)
    {
        return;
    }
    drop(queue);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        loop {
            let next = {
                let pending = app.state::<PendingLinks>();
                let Ok(mut queue) = pending.urls.lock() else {
                    return;
                };
                let next = queue.pop_front();
                if next.is_none() {
                    pending.running.store(false, Ordering::Release);
                }
                next
            };
            let Some((raw, generation)) = next else {
                return;
            };
            if completed(&app) {
                continue;
            }
            let controller = app.state::<Controller>();
            let Ok(mut engine) = controller.engine.lock() else {
                continue;
            };
            let valid =
                || controller.generation.load(Ordering::Acquire) == generation && !completed(&app);
            if !valid() {
                continue;
            }
            let prior = engine.status;
            let accepted = match engine.callback(&raw, now(), generation, valid) {
                Ok(value) => value,
                Err(error) => {
                    engine.status = error.into();
                    engine.secrets.pending.is_none()
                }
            };
            if accepted {
                record_transition(&app, prior, engine.status);
            }
            if accepted && valid() {
                if engine.status == AuthStatus::SignedIn {
                    engine.account_generation =
                        controller.account_generation.load(Ordering::Acquire);
                }
                publish(&app, &engine);
                let open = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if !completed(&open)
                        && open
                            .state::<Controller>()
                            .generation
                            .load(Ordering::Acquire)
                            == generation
                    {
                        let _ = crate::onboarding_window::open(&open);
                    }
                });
            }
        }
    });
}

#[tauri::command]
pub async fn cloud_status(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Snapshot, String> {
    let generation = app
        .state::<Controller>()
        .account_generation
        .load(Ordering::Acquire);
    tauri::async_runtime::spawn_blocking(move || {
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if generation != controller.account_generation.load(Ordering::Acquire)
            || engine.account_generation != generation
        {
            let mut result = snapshot(&app, &engine);
            result.identity = None;
            if result.status != AuthStatus::CredentialFailed {
                result.status = AuthStatus::Unavailable;
            }
            return Ok(snapshot_for_window(result, window.label()));
        }
        let prior = engine.status;
        if let Err(error) = engine.refresh(now(), || {
            controller.account_generation.load(Ordering::Acquire) == generation
        }) {
            engine.status = error.into();
        }
        if prior == AuthStatus::Waiting && engine.status == AuthStatus::Expired {
            record_transition(&app, prior, engine.status);
        }
        Ok(snapshot_for_window(snapshot(&app, &engine), window.label()))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}
#[tauri::command]
pub async fn cloud_begin(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let generation = app.state::<Controller>().generation.load(Ordering::Acquire);
    tauri::async_runtime::spawn_blocking(move || {
        if completed(&app) {
            return Err("The introduction is already complete.".to_owned());
        }
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.config.is_some()
            && engine
                .secrets
                .pending
                .as_ref()
                .is_some_and(|attempt| attempt.expires_at > now())
        {
            return Ok(snapshot(&app, &engine));
        }
        if completed(&app) || controller.generation.load(Ordering::Acquire) != generation {
            return Err("Cloud setup was cancelled.".to_owned());
        }
        let prior = engine.status;
        match engine.begin(now(), generation) {
            Ok(url) => {
                if completed(&app) || controller.generation.load(Ordering::Acquire) != generation {
                    let _ = engine.invalidate(AuthStatus::Cancelled);
                    return Ok(snapshot(&app, &engine));
                }
                record_transition(&app, prior, engine.status);
                publish(&app, &engine);
                let launched = app.opener().open_url(url, None::<&str>).is_ok();
                if let Err(error) = engine.browser_result(launched) {
                    engine.status = error.into();
                }
            }
            Err(error) => engine.status = error.into(),
        }
        record_transition(&app, prior, engine.status);
        Ok(publish(&app, &engine))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
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
        let prior = engine.status;
        if engine.config.is_some()
            && let Err(error) = engine
                .load(now())
                .and_then(|()| engine.invalidate(AuthStatus::Cancelled))
        {
            engine.status = error.into();
        }
        record_transition(&app, prior, engine.status);
        Ok(publish(&app, &engine))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}
pub fn invalidate_pending(app: &tauri::AppHandle) {
    if let Some(controller) = app.try_state::<Controller>() {
        controller.generation.fetch_add(1, Ordering::AcqRel);
    }
}
#[tauri::command]
pub async fn cloud_sign_out(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Snapshot, String> {
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
        let prior = engine.status;
        match engine.sign_out() {
            Ok(()) => engine.account_generation = generation,
            Err(error) => engine.status = error.into(),
        }
        record_transition(&app, prior, engine.status);
        Ok(snapshot_for_window(publish(&app, &engine), window.label()))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}
#[tauri::command]
pub async fn cloud_save_draft(
    app: tauri::AppHandle,
    mut value: Draft,
    folders_edited: Option<bool>,
) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if value.folders.len() > 100
            || value.folders.iter().any(|folder| {
                folder.path.is_empty()
                    || folder.path.len() > 4096
                    || !std::path::Path::new(&folder.path).is_absolute()
            })
        {
            return Err("Choose valid local folders.".to_owned());
        }
        if completed(&app) {
            return Err("The introduction is already complete.".to_owned());
        }
        let controller = app.state::<Controller>();
        let engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.identity.is_none() {
            value.screen = Screen::Handoff;
        }
        if completed(&app) {
            return Err("The introduction is already complete.".to_owned());
        }
        let store = app.state::<crate::store::Store>();
        let previous = store
            .internal_value(DRAFT_KEY)
            .and_then(|raw| serde_json::from_str::<Draft>(&raw).ok());
        value.folders_initialized = folders_edited.unwrap_or(true)
            || previous.is_some_and(|draft| draft.folders_initialized);
        for folder in &mut value.folders {
            folder.session_count = None;
            folder.session_count_is_partial = false;
        }
        let encoded = serde_json::to_string(&value).map_err(|_| "Could not save Cloud setup.")?;
        app.state::<crate::store::Store>()
            .set_internal_value_checked(DRAFT_KEY, &encoded)
            .map_err(|_| "Could not save Cloud setup.")?;
        Ok(publish(&app, &engine))
    })
    .await
    .map_err(|_| "Cloud is unavailable.".to_owned())?
}
#[tauri::command]
pub fn cloud_start_sync() -> Result<(), String> {
    Err("Cloud sync is unavailable until the service and upload consent are ready.".to_owned())
}

fn record_transition(app: &tauri::AppHandle, previous: AuthStatus, next: AuthStatus) {
    use crate::analytics::event::OnboardingCloudStatus as Event;
    if previous == next {
        return;
    }
    let event = match next {
        AuthStatus::Idle => return,
        AuthStatus::Opening => Event::Opening,
        AuthStatus::Waiting => Event::Waiting,
        AuthStatus::Returned => Event::Returned,
        AuthStatus::SignedIn => Event::SignedIn,
        AuthStatus::SignedOut => Event::SignedOut,
        AuthStatus::Cancelled => Event::Cancelled,
        AuthStatus::Expired => Event::Expired,
        AuthStatus::BrowserFailed => Event::BrowserFailed,
        AuthStatus::Offline => Event::Offline,
        AuthStatus::CredentialFailed => Event::CredentialFailed,
        AuthStatus::Unavailable => Event::Unavailable,
        AuthStatus::Rejected | AuthStatus::InvalidResponse => Event::Failed,
    };
    crate::analytics::record_cloud_status(app, event);
}
