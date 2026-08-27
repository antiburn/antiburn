//! Update checks against the public GitHub Releases feed.
//!
//! The updater plugin is the only surface in the whole application that talks
//! to a service of ours, and this module owns every decision about when it is
//! allowed to speak:
//!
//! - **Whether it can speak at all.** [`supported`] answers from *real*
//!   registration state — a flag set only after the plugin actually built and
//!   installed — plus a non-empty signing public key in the bundled
//!   configuration. A compile-time `cfg!` would say "yes" in a release build
//!   whose key was never configured, and every piece of copy downstream would
//!   inherit that lie.
//! - **When it speaks on its own.** [`spawn_scheduler`] checks once shortly
//!   after launch and then every [`CHECK_INTERVAL`], and only while the reader's
//!   "check automatically" preference is on. It is the thing that makes that
//!   preference real; without it the switch would be decoration.
//! - **What it says afterwards.** Every check — automatic or not — ends in an
//!   [`EVENT_UPDATE`] event carrying an [`UpdateStatus`], which is what the
//!   Updates pane renders. Nothing is inferred from silence. An *automatic*
//!   check that finds a version also asks [`crate::notifications`] to say so
//!   once, since nobody is watching the pane when the schedule runs.
//!
//! Nothing here contacts a server in a development build: the plugin is never
//! registered there, so [`supported`] is false and the scheduler does nothing.
//!
//! # The signing key is a release requirement, not a nicety
//!
//! `plugins.updater.pubkey` in `tauri.conf.json` carries the updater key's
//! public half (minted 2026-08-14; custody in
//! `docs/runbooks/updater-key-recovery.md`), and
//! `bundle.createUpdaterArtifacts` is **true** — so a release build produces
//! signed updater bundles, and this build claims update support only while a
//! key is present. The two halves are one decision: an updater with artifacts
//! but no key would download something it cannot verify, and an updater with a
//! key but no artifacts would have nothing to check.
//!
//! A real release therefore **requires** the key pair to exist: the private
//! half in the release environment as `TAURI_SIGNING_PRIVATE_KEY`, the public
//! half committed here. `.github/workflows/release-app.yml` fails the release
//! outright while the field is empty rather than shipping a build whose update
//! path is decoration. The key was minted 2026-08-14 — custody, rotation, and
//! what a reader has to do if it is ever lost are in
//! `docs/runbooks/updater-key-recovery.md`.
//!
//! (The warning lives here rather than beside the field because `tauri.conf.json`
//! is parsed as strict JSON — `tauri-build`'s default features exclude
//! `config-json5` — so a comment in that file would fail the build.)

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::store::Store;

/// Event the shell emits as the update lifecycle changes.
pub const EVENT_UPDATE: &str = "update:status";

/// How long after launch the first automatic check runs.
///
/// Launch is the busiest moment in the app's life — the first scan, the store
/// migration, the webview boot — and an update check is the least urgent thing
/// competing for it.
pub const STARTUP_DELAY: Duration = Duration::from_secs(30);

/// How often the automatic check repeats.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Fixed version used by the debug-only update simulator.
pub const SIMULATED_VERSION: &str = "99.0.0";

const SIMULATED_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
const SIMULATION_STEP_DELAY: Duration = Duration::from_millis(250);
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(2 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimulationPhase {
    Available,
    Failed,
    Installed,
}

impl SimulationPhase {
    fn after_install(self) -> SimulationPhase {
        match self {
            SimulationPhase::Available => SimulationPhase::Failed,
            SimulationPhase::Failed | SimulationPhase::Installed => SimulationPhase::Installed,
        }
    }
}

fn download_stall_wait(elapsed: Duration) -> Option<Duration> {
    if elapsed >= DOWNLOAD_STALL_TIMEOUT {
        None
    } else {
        Some(DOWNLOAD_STALL_TIMEOUT - elapsed)
    }
}

/// Whether the updater plugin actually registered.
///
/// Registered as Tauri managed state and set exactly once, from
/// `install_updater`. Its default — false — is what a development build, a
/// build with no signing key, and a build whose plugin failed to install all
/// report, which is the truth in every one of those cases.
#[derive(Default)]
pub struct UpdaterState {
    registered: AtomicBool,
    operation: tokio::sync::Mutex<()>,
    installed_version: Mutex<Option<String>>,
    latest_status: Mutex<Option<UpdateStatus>>,
    next_revision: AtomicU64,
    simulation: Mutex<Option<SimulationPhase>>,
}

impl UpdaterState {
    /// Called after the plugin installed successfully. Not public guesswork:
    /// the only caller is the registration site itself.
    ///
    /// That site is compiled into release builds only, so in a development
    /// build nothing calls this outside the tests — which is precisely the
    /// state it exists to describe.
    #[cfg(any(not(debug_assertions), test))]
    pub fn note_registered(&self) {
        self.registered.store(true, Ordering::SeqCst);
    }

    pub fn registered(&self) -> bool {
        self.registered.load(Ordering::SeqCst)
    }

    fn installed_version(&self) -> Option<String> {
        self.installed_version
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn note_installed(&self, version: &str) {
        *self
            .installed_version
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(version.to_string());
    }

    fn note_status(&self, status: &UpdateStatus) {
        *self
            .latest_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(status.clone());
    }

    fn latest_status(&self) -> Option<UpdateStatus> {
        self.latest_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn simulation_phase(&self) -> Option<SimulationPhase> {
        *self
            .simulation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn note_simulation_phase(&self, phase: SimulationPhase) {
        *self
            .simulation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(phase);
    }

    fn finish_simulated_restart(&self) -> bool {
        let mut simulation = self
            .simulation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *simulation != Some(SimulationPhase::Installed) {
            return false;
        }
        *simulation = None;
        true
    }
}

/// Whether this build can check for updates *at all*.
///
/// Both halves matter. Without registration there is no plugin to ask; without
/// a signing key the plugin cannot verify what it downloads, so a check that
/// appeared to work would be a check whose answer could not be trusted.
pub fn supported(app: &AppHandle) -> bool {
    app.try_state::<UpdaterState>()
        .is_some_and(|state| state.registered())
        && signing_key_configured(app)
        && installation_supported()
}

#[cfg(target_os = "linux")]
fn installation_supported() -> bool {
    linux_bundle_supported(std::env::var_os("APPIMAGE").is_some())
}

#[cfg(not(target_os = "linux"))]
fn installation_supported() -> bool {
    true
}

#[cfg(any(target_os = "linux", test))]
fn linux_bundle_supported(appimage_runtime: bool) -> bool {
    appimage_runtime
}

/// Whether the bundled configuration carries an updater public key.
pub fn signing_key_configured(app: &AppHandle) -> bool {
    let plugins = serde_json::to_value(&app.config().plugins).unwrap_or(serde_json::Value::Null);
    has_signing_key(&plugins)
}

/// The pure half of [`signing_key_configured`], over the serialized plugin
/// configuration.
fn has_signing_key(plugins: &serde_json::Value) -> bool {
    plugins
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
}

/// One update lifecycle state, as the Updates pane renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// The update lifecycle state rendered by the Updates pane.
    pub kind: &'static str,
    /// The version waiting to be installed, when there is one.
    pub version: Option<String>,
    /// Why an update operation failed, in the plugin's own words.
    pub message: Option<String>,
    /// ISO-8601 stamp for this update state.
    pub checked_at: String,
    /// True when the check ran on the schedule rather than because a reader
    /// pressed a button — so the pane can say "checked automatically" without
    /// inventing the distinction.
    pub automatic: bool,
    /// Bytes received for an active or completed download.
    pub downloaded_bytes: Option<u64>,
    /// Expected download size, when the server supplied one.
    pub total_bytes: Option<u64>,
    /// The operation that failed, so the pane can offer the correct retry.
    pub failure_operation: Option<&'static str>,
    /// Monotonic process-local order for events and status snapshots.
    pub revision: u64,
}

impl UpdateStatus {
    fn new(kind: &'static str, automatic: bool) -> UpdateStatus {
        UpdateStatus {
            kind,
            version: None,
            message: None,
            checked_at: crate::store::now_rfc3339(),
            automatic,
            downloaded_bytes: None,
            total_bytes: None,
            failure_operation: None,
            revision: 0,
        }
    }

    fn current(automatic: bool) -> UpdateStatus {
        UpdateStatus::new("current", automatic)
    }

    fn available(version: String, automatic: bool) -> UpdateStatus {
        UpdateStatus {
            version: Some(version),
            ..UpdateStatus::new("available", automatic)
        }
    }

    fn downloading(
        version: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    ) -> UpdateStatus {
        UpdateStatus {
            version: Some(version),
            downloaded_bytes: Some(downloaded_bytes),
            total_bytes,
            ..UpdateStatus::new("downloading", false)
        }
    }

    fn installing(
        version: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    ) -> UpdateStatus {
        UpdateStatus {
            version: Some(version),
            downloaded_bytes: Some(downloaded_bytes),
            total_bytes,
            ..UpdateStatus::new("installing", false)
        }
    }

    fn installed(version: String) -> UpdateStatus {
        UpdateStatus {
            version: Some(version),
            ..UpdateStatus::new("installed", false)
        }
    }

    fn failed(
        message: String,
        automatic: bool,
        failure_operation: &'static str,
        version: Option<String>,
    ) -> UpdateStatus {
        UpdateStatus {
            message: Some(message),
            failure_operation: Some(failure_operation),
            version,
            ..UpdateStatus::new("failed", automatic)
        }
    }
}

/// Start the automatic check schedule. The returned handle is aborted on exit.
///
/// The loop runs in every build; what it *does* is decided every pass by
/// [`supported`] and the reader's preference, so there is no compile-time
/// branch to get wrong and the whole path is covered by the normal checks.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            if should_check(&app) {
                let Some(status) = automatic_check(&app).await else {
                    tokio::time::sleep(CHECK_INTERVAL).await;
                    continue;
                };
                // The Updates pane learns from the event; someone who is not
                // looking at antiburn learns from a notification, at most once
                // per version. A *manual* check is deliberately not notified:
                // the reader is already looking at the answer.
                let status = emit_status(&app, status);
                crate::notifications::note_update_status(&app, &status);
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    })
}

/// Whether an automatic check should run right now.
fn should_check(app: &AppHandle) -> bool {
    supported(app)
        && auto_update_enabled(app)
        && app
            .try_state::<UpdaterState>()
            .is_some_and(|state| state.installed_version().is_none())
}

/// The reader's "check automatically" preference, defaulting to *not* checking
/// when the store cannot be read: a failed read is not consent.
fn auto_update_enabled(app: &AppHandle) -> bool {
    app.try_state::<Store>()
        .and_then(|store| store.settings().ok())
        .is_some_and(|settings| settings.auto_update)
}

/// Ask the release feed whether there is a newer version.
///
/// Guarded by [`supported`] rather than by a `cfg`, because the plugin's own
/// extension trait reads managed state that only exists once the plugin has
/// registered.
pub async fn check(app: &AppHandle, automatic: bool) -> UpdateStatus {
    if !supported(app) {
        return UpdateStatus::new("unsupported", automatic);
    }
    check_with_plugin(app, automatic).await
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn check_with_plugin(app: &AppHandle, automatic: bool) -> UpdateStatus {
    match find_update(app).await {
        Ok(Some(update)) => UpdateStatus::available(update.version.clone(), automatic),
        Ok(None) => UpdateStatus::current(automatic),
        Err(error) => UpdateStatus::failed(error, automatic, "check", None),
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
async fn check_with_plugin(_app: &AppHandle, automatic: bool) -> UpdateStatus {
    UpdateStatus::new("unsupported", automatic)
}

/// Run a reader-requested check without racing another updater operation.
pub async fn manual_check(app: &AppHandle) -> UpdateStatus {
    let Some(state) = app.try_state::<UpdaterState>() else {
        return UpdateStatus::new("unsupported", false);
    };
    let _operation = state.operation.lock().await;
    if let Some(version) = state.installed_version() {
        let status = UpdateStatus::installed(version);
        return emit_status(app, status);
    }
    let status = check(app, false).await;
    emit_status(app, status)
}

/// Return the latest state so a pane mounted after an event can catch up.
pub fn current_status(app: &AppHandle) -> Option<UpdateStatus> {
    app.try_state::<UpdaterState>()
        .and_then(|state| state.latest_status())
}

/// Start the fixed local update lifecycle used by debug builds.
pub async fn start_simulation(app: &AppHandle) -> Result<UpdateStatus, &'static str> {
    if !cfg!(debug_assertions) {
        return Err("The update simulator is available only in debug builds");
    }
    let Some(state) = app.try_state::<UpdaterState>() else {
        return Err("The updater state is unavailable");
    };
    let _operation = state.operation.lock().await;
    state.note_simulation_phase(SimulationPhase::Available);
    Ok(emit_status(
        app,
        UpdateStatus::available(SIMULATED_VERSION.to_string(), false),
    ))
}

/// Download, verify, and install the version the reader approved.
pub async fn install(app: &AppHandle, expected_version: &str) -> UpdateStatus {
    let Some(state) = app.try_state::<UpdaterState>() else {
        return UpdateStatus::new("unsupported", false);
    };
    let _operation = state.operation.lock().await;
    if let Some(phase) = state.simulation_phase() {
        return install_simulation(app, &state, phase, expected_version).await;
    }
    if !supported(app) {
        return UpdateStatus::new("unsupported", false);
    }
    if let Some(version) = state.installed_version() {
        let status = UpdateStatus::installed(version);
        return emit_status(app, status);
    }
    install_with_plugin(app, expected_version).await
}

/// Restart only after an update has installed successfully.
pub fn restart(app: &AppHandle) -> Result<(), &'static str> {
    if app
        .try_state::<UpdaterState>()
        .is_some_and(|state| state.finish_simulated_restart())
    {
        emit_status(app, UpdateStatus::new("unsupported", false));
        return Ok(());
    }
    let installed = app
        .try_state::<UpdaterState>()
        .is_some_and(|state| state.installed_version().is_some());
    if !installed {
        return Err("No installed update is waiting for a restart");
    }
    app.restart()
}

async fn install_simulation(
    app: &AppHandle,
    state: &UpdaterState,
    phase: SimulationPhase,
    expected_version: &str,
) -> UpdateStatus {
    if expected_version != SIMULATED_VERSION {
        return emit_status(
            app,
            UpdateStatus::available(SIMULATED_VERSION.to_string(), false),
        );
    }
    if phase == SimulationPhase::Installed {
        return emit_status(app, UpdateStatus::installed(SIMULATED_VERSION.to_string()));
    }

    emit_status(
        app,
        UpdateStatus::downloading(SIMULATED_VERSION.to_string(), 0, None),
    );
    tokio::time::sleep(SIMULATION_STEP_DELAY).await;
    emit_status(
        app,
        UpdateStatus::downloading(
            SIMULATED_VERSION.to_string(),
            SIMULATED_TOTAL_BYTES / 2,
            Some(SIMULATED_TOTAL_BYTES),
        ),
    );
    tokio::time::sleep(SIMULATION_STEP_DELAY).await;

    let next_phase = phase.after_install();
    state.note_simulation_phase(next_phase);
    if next_phase == SimulationPhase::Failed {
        return emit_status(
            app,
            UpdateStatus::failed(
                "The simulated download failed. Try the install again.".to_string(),
                false,
                "install",
                Some(SIMULATED_VERSION.to_string()),
            ),
        );
    }

    emit_status(
        app,
        UpdateStatus::installing(
            SIMULATED_VERSION.to_string(),
            SIMULATED_TOTAL_BYTES,
            Some(SIMULATED_TOTAL_BYTES),
        ),
    );
    tokio::time::sleep(SIMULATION_STEP_DELAY).await;
    emit_status(app, UpdateStatus::installed(SIMULATED_VERSION.to_string()))
}

async fn automatic_check(app: &AppHandle) -> Option<UpdateStatus> {
    let state = app.try_state::<UpdaterState>()?;
    let _operation = state.operation.try_lock().ok()?;
    if state.installed_version().is_some() {
        return None;
    }
    Some(check(app, true).await)
}

fn emit_status(app: &AppHandle, mut status: UpdateStatus) -> UpdateStatus {
    if let Some(state) = app.try_state::<UpdaterState>() {
        status.revision = state.next_revision.fetch_add(1, Ordering::Relaxed) + 1;
        state.note_status(&status);
    }
    let _ = app.emit(EVENT_UPDATE, status.clone());
    status
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn find_update(app: &AppHandle) -> Result<Option<tauri_plugin_updater::Update>, String> {
    use tauri_plugin_updater::UpdaterExt as _;

    let updater = app.updater().map_err(|error| error.to_string())?;
    updater.check().await.map_err(|error| error.to_string())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn install_with_plugin(app: &AppHandle, expected_version: &str) -> UpdateStatus {
    let update = match find_update(app).await {
        Ok(Some(update)) if update.version == expected_version => update,
        Ok(Some(update)) => {
            let status = UpdateStatus::available(update.version, false);
            return emit_status(app, status);
        }
        Ok(None) => {
            let status = UpdateStatus::current(false);
            return emit_status(app, status);
        }
        Err(message) => {
            let status = UpdateStatus::failed(
                message,
                false,
                "install",
                Some(expected_version.to_string()),
            );
            return emit_status(app, status);
        }
    };

    let version = update.version.clone();
    let downloaded = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(u64::MAX));
    emit_status(app, UpdateStatus::downloading(version.clone(), 0, None));

    let progress_downloaded = Arc::clone(&downloaded);
    let progress_total = Arc::clone(&total);
    let progress_app = app.clone();
    let progress_version = version.clone();
    let last_progress = Arc::new(Mutex::new(Instant::now()));
    let callback_last_progress = Arc::clone(&last_progress);
    let mut last_emit = Instant::now();
    let bytes = {
        let download = update.download(
            move |chunk_length, content_length| {
                *callback_last_progress
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
                let current = progress_downloaded
                    .fetch_add(chunk_length as u64, Ordering::Relaxed)
                    .saturating_add(chunk_length as u64);
                if let Some(content_length) = content_length {
                    progress_total.store(content_length, Ordering::Relaxed);
                }
                if last_emit.elapsed() >= Duration::from_millis(100) {
                    last_emit = Instant::now();
                    emit_status(
                        &progress_app,
                        UpdateStatus::downloading(
                            progress_version.clone(),
                            current,
                            content_length,
                        ),
                    );
                }
            },
            || {},
        );
        tokio::pin!(download);
        loop {
            let elapsed = last_progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .elapsed();
            let Some(wait) = download_stall_wait(elapsed) else {
                let status = UpdateStatus::failed(
                    "The update download stopped making progress. Check your connection and try again."
                        .to_string(),
                    false,
                    "install",
                    Some(version.clone()),
                );
                return emit_status(app, status);
            };
            tokio::select! {
                biased;
                result = &mut download => match result {
                    Ok(bytes) => break bytes,
                    Err(error) => {
                        let status = UpdateStatus::failed(
                            error.to_string(),
                            false,
                            "install",
                            Some(version.clone()),
                        );
                        return emit_status(app, status);
                    }
                },
                () = tokio::time::sleep(wait) => {}
            }
        }
    };

    let downloaded_bytes = bytes.len() as u64;
    let total_bytes = match total.load(Ordering::Relaxed) {
        u64::MAX => None,
        value => Some(value),
    };
    emit_status(
        app,
        UpdateStatus::installing(version.clone(), downloaded_bytes, total_bytes),
    );
    let status = match update.install(bytes) {
        Ok(()) => {
            if let Some(state) = app.try_state::<UpdaterState>() {
                state.note_installed(&version);
            }
            UpdateStatus::installed(version)
        }
        Err(error) => UpdateStatus::failed(error.to_string(), false, "install", Some(version)),
    };
    emit_status(app, status)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
async fn install_with_plugin(_app: &AppHandle, _expected_version: &str) -> UpdateStatus {
    UpdateStatus::new("unsupported", false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_public_key_is_not_a_configured_one() {
        let plugins = serde_json::json!({ "updater": { "pubkey": "" } });
        assert!(!has_signing_key(&plugins));

        let blank = serde_json::json!({ "updater": { "pubkey": "   " } });
        assert!(!has_signing_key(&blank));
    }

    #[test]
    fn a_real_public_key_is_configured() {
        let plugins = serde_json::json!({
            "updater": { "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk=" }
        });
        assert!(has_signing_key(&plugins));
    }

    #[test]
    fn a_configuration_with_no_updater_block_carries_no_key() {
        assert!(!has_signing_key(&serde_json::json!({})));
        assert!(!has_signing_key(&serde_json::json!({ "updater": {} })));
        assert!(!has_signing_key(&serde_json::Value::Null));
        // A non-string key is malformed, not a key.
        assert!(!has_signing_key(
            &serde_json::json!({ "updater": { "pubkey": 42 } })
        ));
    }

    #[test]
    fn a_fresh_updater_state_reports_the_truth_about_a_development_build() {
        let state = UpdaterState::default();
        assert!(
            !state.registered(),
            "nothing has registered, so nothing may claim update support"
        );
        state.note_registered();
        assert!(state.registered());
    }

    #[test]
    fn every_outcome_serializes_as_the_shape_the_pane_renders() {
        let available = UpdateStatus::available("0.2.0".into(), true);
        let json = serde_json::to_value(&available).unwrap();
        assert_eq!(json["kind"], "available");
        assert_eq!(json["version"], "0.2.0");
        assert_eq!(json["automatic"], true);
        assert!(json["checkedAt"].as_str().is_some_and(|at| at.len() >= 20));

        let failed = serde_json::to_value(UpdateStatus::failed(
            "no network".into(),
            false,
            "install",
            Some("0.2.0".into()),
        ))
        .unwrap();
        assert_eq!(failed["kind"], "failed");
        assert_eq!(failed["message"], "no network");
        assert_eq!(failed["failureOperation"], "install");
        assert_eq!(failed["automatic"], false);

        let current = serde_json::to_value(UpdateStatus::current(false)).unwrap();
        assert_eq!(current["kind"], "current");
        assert!(current["version"].is_null());

        let downloading =
            serde_json::to_value(UpdateStatus::downloading("0.2.0".into(), 512, Some(1_024)))
                .unwrap();
        assert_eq!(downloading["kind"], "downloading");
        assert_eq!(downloading["downloadedBytes"], 512);
        assert_eq!(downloading["totalBytes"], 1_024);

        let installing =
            serde_json::to_value(UpdateStatus::installing("0.2.0".into(), 1_024, None)).unwrap();
        assert_eq!(installing["kind"], "installing");
        assert!(installing["totalBytes"].is_null());

        let installed = serde_json::to_value(UpdateStatus::installed("0.2.0".into())).unwrap();
        assert_eq!(installed["kind"], "installed");
        assert_eq!(installed["version"], "0.2.0");
    }

    #[test]
    fn updater_operations_are_single_flight() {
        let state = UpdaterState::default();
        let _operation = state.operation.try_lock().unwrap();
        assert!(state.operation.try_lock().is_err());
    }

    #[test]
    fn an_installed_version_stays_available_until_restart() {
        let state = UpdaterState::default();
        assert!(state.installed_version().is_none());
        state.note_installed("0.2.0");
        assert_eq!(state.installed_version().as_deref(), Some("0.2.0"));
    }

    #[test]
    fn the_simulator_tracks_failure_retry_and_restart_separately() {
        let state = UpdaterState::default();
        assert!(state.simulation_phase().is_none());

        state.note_simulation_phase(SimulationPhase::Available);
        assert_eq!(state.simulation_phase(), Some(SimulationPhase::Available));
        state.note_simulation_phase(SimulationPhase::Failed);
        assert_eq!(state.simulation_phase(), Some(SimulationPhase::Failed));
        assert!(!state.finish_simulated_restart());

        state.note_simulation_phase(SimulationPhase::Installed);
        assert!(state.finish_simulated_restart());
        assert!(state.simulation_phase().is_none());
        assert!(state.installed_version().is_none());
    }

    #[test]
    fn the_simulator_fails_once_and_then_installs() {
        assert_eq!(
            SimulationPhase::Available.after_install(),
            SimulationPhase::Failed
        );
        assert_eq!(
            SimulationPhase::Failed.after_install(),
            SimulationPhase::Installed
        );
        assert_eq!(
            SimulationPhase::Installed.after_install(),
            SimulationPhase::Installed
        );
    }

    #[test]
    fn the_download_watchdog_waits_only_while_progress_is_recent() {
        assert_eq!(
            download_stall_wait(Duration::from_secs(30)),
            Some(Duration::from_secs(90))
        );
        assert_eq!(download_stall_wait(DOWNLOAD_STALL_TIMEOUT), None);
        assert_eq!(download_stall_wait(Duration::from_secs(121)), None);
    }

    #[test]
    fn only_an_appimage_runtime_can_replace_a_linux_bundle() {
        assert!(linux_bundle_supported(true));
        assert!(!linux_bundle_supported(false));
    }

    #[test]
    fn the_latest_status_is_retained_for_a_pane_that_mounts_late() {
        let state = UpdaterState::default();
        assert!(state.latest_status().is_none());
        state.note_status(&UpdateStatus::available("0.2.0".into(), true));
        assert_eq!(
            state.latest_status().unwrap().version.as_deref(),
            Some("0.2.0")
        );
    }

    #[test]
    fn the_schedule_is_the_one_the_updates_pane_describes() {
        // The pane tells the reader an automatic check happens a moment after
        // launch and then every few hours; these are those numbers.
        assert_eq!(STARTUP_DELAY, Duration::from_secs(30));
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(21_600));
    }
}
