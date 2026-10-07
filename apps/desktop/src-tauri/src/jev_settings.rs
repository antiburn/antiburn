//! Native TypeSafe credentials and Settings-only authorization.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::jev_worker::WorkerHandle;
use crate::store::{BurnCheckHistoryStatus, BurnCheckUsageSummary, Store};

const CHECK_ID: &str = "ignored_instructions";
const ENABLED_AT_KEY: &str = "internal:burnChecksEnabledAtEpochV1";
const SAVED_KEY_KEY: &str = "internal:typesafeKeySavedV1";
const HISTORY_DAYS_KEY: &str = "internal:jevBurnCheckHistoryDaysV1";
const AUTH_REJECTED_KEY: &str = "internal:typesafeAuthRejectedV1";
const CREDENTIAL_CHANGE_PENDING_KEY: &str = "internal:typesafeCredentialChangePendingV1";
#[cfg(debug_assertions)]
const SERVICE: &str = "ai.antiburn.desktop.debug.typesafe";
#[cfg(not(debug_assertions))]
const SERVICE: &str = "ai.antiburn.desktop.typesafe";
const ACCOUNT: &str = "ignored-instructions";
pub(crate) const AVAILABILITY_EVENT: &str = "checks:availability-changed";
static CREDENTIAL_CHANGE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static STARTUP_ERROR: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

#[cfg(feature = "analytics")]
fn history_window_label(days: u8) -> Option<&'static str> {
    match days {
        0 => Some("future"),
        7 => Some("7_days"),
        30 => Some("30_days"),
        _ => None,
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckAvailability {
    configured: bool,
    saved_key: bool,
    error: Option<&'static str>,
    usage: BurnCheckUsageSummary,
    history_days: u8,
    backfill: BurnCheckBackfillSummary,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", content = "snapshot", rename_all = "snake_case")]
enum CheckAvailabilityEvent {
    Updated(CheckAvailability),
    Failed,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BurnCheckBackfillSummary {
    total: usize,
    waiting_for_data: usize,
    waiting_for_idle: usize,
    ready: usize,
    queued: usize,
    running: usize,
    completed: usize,
    skipped: usize,
    failed: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackfillRunResult {
    queued: usize,
    availability: CheckAvailability,
}

fn entry() -> Result<keyring::Entry, &'static str> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|_| "Credential storage is unavailable.")
}

fn read_key() -> Result<Option<String>, &'static str> {
    match entry()?.get_password() {
        Ok(key) if !key.trim().is_empty() => Ok(Some(key)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("Credential storage is unavailable."),
    }
}

fn saved_key_marker(store: &Store) -> bool {
    store.internal_value(SAVED_KEY_KEY).as_deref() == Some("true")
        || store.internal_value(ENABLED_AT_KEY).is_some()
        || store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref()
            == Some("true")
}

fn preserve_saved_key_marker(store: &Store) -> Result<(), &'static str> {
    if store.internal_value(SAVED_KEY_KEY).as_deref() == Some("true")
        || store.internal_value(ENABLED_AT_KEY).is_none()
    {
        return Ok(());
    }
    store
        .set_internal_value_checked(SAVED_KEY_KEY, "true")
        .map_err(|_| "Could not preserve the saved credential state.")
}

fn restore_saved_key(
    store: &Store,
    worker: &WorkerHandle,
    read: impl FnOnce() -> Result<Option<String>, &'static str>,
) -> Result<(), &'static str> {
    if store
        .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
        .as_deref()
        == Some("true")
    {
        return Err("TypeSafe credential removal is incomplete. Retry it in Settings → Checks.");
    }
    if !saved_key_marker(store) {
        return Ok(());
    }
    let key =
        read()?.ok_or("The saved TypeSafe API key is missing. Replace it in Settings → Checks.")?;
    if store.internal_value(AUTH_REJECTED_KEY).as_deref() != Some("true") {
        worker.set_api_key(Some(key));
    }
    Ok(())
}

/// Restore only a previously enabled key. Serialize startup with Settings changes.
pub(crate) fn restore_at_launch(app: &AppHandle) {
    if app
        .state::<Store>()
        .internal_value(ENABLED_AT_KEY)
        .is_none()
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _change = CREDENTIAL_CHANGE.lock().await;
        let store = app.state::<Store>().inner().clone();
        let worker_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let worker = worker_app.state::<WorkerHandle>();
            restore_saved_key(&store, &worker, read_key)
        })
        .await;
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(_) => Some("Credential storage is unavailable."),
        };
        *STARTUP_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
        changed(&app);
    });
}

/// Settings and the main window's Checks step can change check settings. The
/// `checks-settings` capability grants the same two windows.
fn checks_settings_window(window: &WebviewWindow) -> Result<(), &'static str> {
    if is_checks_settings_window(window.label()) {
        Ok(())
    } else {
        Err("This action is available only in Settings or the Checks step.")
    }
}

fn is_checks_settings_window(label: &str) -> bool {
    label == crate::settings::LABEL || label == crate::main_window::LABEL
}

pub(crate) fn read_check_availability(
    store: &Store,
    configured: bool,
    saved_key: bool,
    error: Option<&'static str>,
) -> Result<CheckAvailability, &'static str> {
    let status = store
        .historical_burn_check_status(time::OffsetDateTime::now_utc().unix_timestamp(), 180)
        .map_err(|_| "Could not read check history from the local database.")?;
    let backfill = BurnCheckBackfillSummary::from(status);
    Ok(CheckAvailability {
        configured,
        saved_key,
        error,
        usage: store
            .burn_check_usage_summary()
            .map_err(|_| "Could not read check usage from the local database.")?,
        history_days: store
            .internal_value(HISTORY_DAYS_KEY)
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|days| matches!(days, 0 | 7 | 30))
            .unwrap_or(0),
        backfill,
    })
}

fn availability(app: &AppHandle) -> Result<CheckAvailability, &'static str> {
    let store = app.state::<Store>();
    let removal_pending = store
        .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
        .as_deref()
        == Some("true");
    let authentication_rejected = app.state::<WorkerHandle>().authentication_rejected()
        || (saved_key_marker(&store)
            && store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true"));
    let error = if removal_pending {
        Some("TypeSafe credential removal is incomplete. Retry it in Settings → Checks.")
    } else if authentication_rejected {
        Some("TypeSafe rejected this API key. Replace it in Settings → Checks.")
    } else {
        *STARTUP_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    read_check_availability(
        &store,
        app.state::<WorkerHandle>().is_available(),
        saved_key_marker(&store),
        error,
    )
}

impl From<BurnCheckHistoryStatus> for BurnCheckBackfillSummary {
    fn from(status: BurnCheckHistoryStatus) -> Self {
        Self {
            total: status.total,
            waiting_for_data: status.waiting_for_data,
            waiting_for_idle: status.waiting_for_idle,
            ready: status.ready,
            queued: status.queued,
            running: status.running,
            completed: status.completed,
            skipped: status.skipped,
            failed: status.failed,
        }
    }
}

pub(crate) fn changed(app: &AppHandle) {
    let event = match availability(app) {
        Ok(snapshot) => CheckAvailabilityEvent::Updated(snapshot),
        Err(_) => CheckAvailabilityEvent::Failed,
    };
    let _ = app.emit(AVAILABILITY_EVENT, event);
    let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
}

pub(crate) fn progress_changed(app: &AppHandle) {
    let event = match availability(app) {
        Ok(snapshot) => CheckAvailabilityEvent::Updated(snapshot),
        Err(_) => CheckAvailabilityEvent::Failed,
    };
    let _ = app.emit(AVAILABILITY_EVENT, event);
}

#[tauri::command]
pub(crate) async fn set_smart_burn_checks_enabled(
    app: AppHandle,
    window: WebviewWindow,
    enabled: bool,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let store = app.state::<Store>().inner().clone();
    if !enabled {
        preserve_saved_key_marker(&store).map_err(str::to_owned)?;
        app.state::<WorkerHandle>().set_api_key(None);
        store
            .disable_burn_checks()
            .map_err(|_| "Could not pause Smart Burn Checks.".to_owned())?;
    } else {
        if store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref()
            == Some("true")
        {
            return Err(
                "TypeSafe credential removal is incomplete. Retry it in Settings → Checks."
                    .to_owned(),
            );
        }
        if !saved_key_marker(&store) {
            return Err("Save a TypeSafe API key before enabling checks.".to_owned());
        }
        if store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true") {
            return Err(
                "TypeSafe rejected this API key. Replace it in Settings → Checks.".to_owned(),
            );
        }
        let app_for_worker = app.clone();
        let key = tauri::async_runtime::spawn_blocking(read_key)
            .await
            .map_err(|_| "Credential storage is unavailable.".to_owned())?
            .map_err(str::to_owned)?
            .ok_or_else(|| {
                "The saved TypeSafe API key is missing. Replace it in Settings → Checks.".to_owned()
            })?;
        if store.internal_value(ENABLED_AT_KEY).is_none() {
            store
                .capture_burn_check_boundaries(
                    &[CHECK_ID],
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .map_err(|_| "Could not enable checks.".to_owned())?;
        }
        app_for_worker
            .state::<WorkerHandle>()
            .set_api_key(Some(key));
        store.set_internal_value(AUTH_REJECTED_KEY, "false");
    }
    #[cfg(feature = "analytics")]
    crate::analytics::record(
        &app,
        crate::analytics::event::EventName::IgnoredInstructionLifecycle,
        crate::analytics::event::Facts {
            label: Some("enablement"),
            detail: Some(if enabled { "enabled" } else { "disabled" }),
            ..Default::default()
        },
    );
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn get_check_availability(app: AppHandle) -> Result<CheckAvailability, String> {
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) async fn set_typesafe_api_key(
    app: AppHandle,
    window: WebviewWindow,
    key: Option<String>,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    if key.is_none()
        && app
            .state::<Store>()
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref()
            == Some("true")
    {
        return Err(
            "TypeSafe credential removal is incomplete. Retry it in Settings → Checks.".to_owned(),
        );
    }
    if key.is_none() && !saved_key_marker(&app.state::<Store>()) {
        return Err("Enter a TypeSafe API key first.".to_owned());
    }
    if key.is_none()
        && app
            .state::<Store>()
            .internal_value(AUTH_REJECTED_KEY)
            .as_deref()
            == Some("true")
    {
        return Err("TypeSafe rejected this API key. Enter a replacement key.".to_owned());
    }
    let key = key.map(|key| key.trim().to_owned());
    let replacing = key.is_some();
    let was_enabled = app
        .state::<Store>()
        .internal_value(ENABLED_AT_KEY)
        .is_some();
    if key
        .as_ref()
        .is_some_and(|key| key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control))
    {
        return Err("Enter a valid TypeSafe API key.".to_owned());
    }
    // Stop work before replacing or loading a credential.
    app.state::<WorkerHandle>().set_api_key(None);
    let store = app.state::<Store>().inner().clone();
    #[cfg(feature = "analytics")]
    let first_enablement = replacing && !saved_key_marker(&store);
    if replacing && was_enabled {
        store
            .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "true")
            .map_err(|_| "Could not protect the credential update state.".to_owned())?;
    }
    changed(&app);
    let key = tauri::async_runtime::spawn_blocking(move || {
        if let Some(key) = key {
            entry()?
                .set_password(&key)
                .map_err(|_| "Could not save the API key in credential storage.")?;
            Ok(key)
        } else {
            read_key()?.ok_or("The saved TypeSafe API key is missing. Enter a new key.")
        }
    })
    .await
    .map_err(|_| "Credential storage is unavailable.".to_owned())?
    .map_err(str::to_owned)?;
    if replacing && !was_enabled {
        // Capture current cursors before the worker can observe the key.
        store
            .capture_burn_check_boundaries(
                &[CHECK_ID],
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .map_err(|_| "Could not enable checks.".to_owned())?;
    }
    if replacing {
        store
            .set_internal_value_checked(SAVED_KEY_KEY, "true")
            .map_err(|_| "Could not save the credential state.".to_owned())?;
    }
    store
        .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "false")
        .map_err(|_| "Could not save the credential update state.".to_owned())?;
    if replacing {
        store
            .retry_rejected_burn_checks()
            .map_err(|_| "Could not schedule checks after replacing the API key.".to_owned())?;
    }
    store.set_internal_value(AUTH_REJECTED_KEY, "false");
    app.state::<WorkerHandle>().set_api_key(Some(key));
    #[cfg(feature = "analytics")]
    if first_enablement {
        crate::analytics::record(
            &app,
            crate::analytics::event::EventName::IgnoredInstructionLifecycle,
            crate::analytics::event::Facts {
                label: Some("enablement"),
                detail: Some("enabled"),
                ..Default::default()
            },
        );
    }
    *STARTUP_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) async fn remove_typesafe_api_key(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    if !saved_key_marker(&app.state::<Store>()) && !app.state::<WorkerHandle>().is_available() {
        return availability(&app).map_err(str::to_owned);
    }
    // Revoke in-memory authorization before any blocking native vault operation.
    app.state::<WorkerHandle>().set_api_key(None);
    app.state::<Store>()
        .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "true")
        .map_err(|_| "Could not protect the credential removal state.".to_owned())?;
    app.state::<Store>()
        .disable_burn_checks()
        .map_err(|_| "Could not disable checks.".to_owned())?;
    app.state::<Store>()
        .set_internal_value(AUTH_REJECTED_KEY, "false");
    changed(&app);
    #[cfg(feature = "analytics")]
    crate::analytics::record(
        &app,
        crate::analytics::event::EventName::IgnoredInstructionLifecycle,
        crate::analytics::event::Facts {
            label: Some("enablement"),
            detail: Some("disabled"),
            ..Default::default()
        },
    );
    let removal = tauri::async_runtime::spawn_blocking(|| match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("Could not remove the API key from credential storage."),
    })
    .await
    .map_err(|_| "Credential storage is unavailable.".to_owned())?
    .map_err(str::to_owned);
    if let Err(error) = removal {
        *STARTUP_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("Could not remove the API key from credential storage.");
        changed(&app);
        return Err(error);
    }
    app.state::<Store>()
        .set_internal_value_checked(SAVED_KEY_KEY, "false")
        .map_err(|_| "Could not save the credential removal state.".to_owned())?;
    app.state::<Store>()
        .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "false")
        .map_err(|_| "Could not save the credential removal state.".to_owned())?;
    *STARTUP_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn set_check_history_days(
    app: AppHandle,
    window: WebviewWindow,
    days: u8,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    if !matches!(days, 0 | 7 | 30) {
        return Err("Choose future checks, 7 days, or 30 days.".to_owned());
    }
    let store = app.state::<Store>();
    #[cfg(feature = "analytics")]
    let previous = store
        .internal_value(HISTORY_DAYS_KEY)
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| matches!(value, 0 | 7 | 30))
        .unwrap_or(0);
    store
        .set_internal_value_checked(HISTORY_DAYS_KEY, &days.to_string())
        .map_err(|_| "Could not save the check history window.".to_owned())?;
    #[cfg(feature = "analytics")]
    if previous != days
        && let Some(detail) = history_window_label(days)
    {
        crate::analytics::record(
            &app,
            crate::analytics::event::EventName::IgnoredInstructionLifecycle,
            crate::analytics::event::Facts {
                label: Some("history_window"),
                detail: Some(detail),
                ..Default::default()
            },
        );
    }
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn run_check_backfill(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<BackfillRunResult, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    if !app.state::<WorkerHandle>().is_available()
        || app.state::<WorkerHandle>().authentication_rejected()
        || app
            .state::<Store>()
            .internal_value(AUTH_REJECTED_KEY)
            .as_deref()
            == Some("true")
    {
        return Err("Add a working TypeSafe API key before running checks.".to_owned());
    }
    let days = app
        .state::<Store>()
        .internal_value(HISTORY_DAYS_KEY)
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|days| matches!(days, 7 | 30))
        .ok_or_else(|| "Choose the last 7 or 30 days first.".to_owned())?;
    let store = app.state::<Store>();
    store
        .reconcile_evidence_revisions(
            &crate::agents::evidence_cohort(),
            crate::analysis::projection_revisions(),
        )
        .map_err(|_| "Could not refresh session evidence for this check.".to_owned())?;
    let queued = app
        .state::<Store>()
        .enqueue_burn_checks(time::OffsetDateTime::now_utc().unix_timestamp(), days)
        .map_err(|_| "Could not queue checks for this period.".to_owned())?;
    let progress = store
        .historical_burn_check_status(time::OffsetDateTime::now_utc().unix_timestamp(), 180)
        .map_err(|_| "Could not read check progress.".to_owned())?;
    ::tracing::info!(
        event = "ignored_instruction_history_requested",
        days,
        queued,
        total = progress.total,
        waiting_for_data = progress.waiting_for_data,
        waiting_for_idle = progress.waiting_for_idle,
        ready = progress.ready,
        running = progress.running,
        completed = progress.completed,
        skipped = progress.skipped,
        failed = progress.failed,
    );
    crate::insights_worker::wake(&app);
    crate::jev_worker::wake(&app);
    #[cfg(feature = "analytics")]
    crate::analytics::record(
        &app,
        crate::analytics::event::EventName::IgnoredInstructionLifecycle,
        crate::analytics::event::Facts {
            label: Some("backfill"),
            detail: Some("requested"),
            ..Default::default()
        },
    );
    progress_changed(&app);
    Ok(BackfillRunResult {
        queued,
        availability: availability(&app).map_err(str::to_owned)?,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        AUTH_REJECTED_KEY, CREDENTIAL_CHANGE_PENDING_KEY, CheckAvailabilityEvent, ENABLED_AT_KEY,
        is_checks_settings_window, preserve_saved_key_marker, restore_saved_key, saved_key_marker,
    };
    use crate::jev_worker::WorkerHandle;
    use crate::store::Store;

    #[test]
    fn settings_and_the_main_window_can_change_check_settings() {
        assert!(is_checks_settings_window(crate::settings::LABEL));
        assert!(is_checks_settings_window(crate::main_window::LABEL));
        assert!(!is_checks_settings_window(crate::popover::LABEL));
    }

    #[test]
    fn availability_failure_event_has_a_typed_failure_status() {
        assert_eq!(
            serde_json::to_value(CheckAvailabilityEvent::Failed).unwrap(),
            serde_json::json!({"status":"failed"})
        );
    }

    #[test]
    fn startup_skips_credential_storage_without_the_enabled_marker() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-unconfigured")).expect("store");
        let worker = WorkerHandle::default();
        restore_saved_key(&store, &worker, || {
            panic!("unconfigured startup read the keychain")
        })
        .expect("no key needed");
        assert!(!worker.is_available());
    }

    #[test]
    fn pausing_keeps_the_saved_key_marker_separate_from_enablement() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-paused")).expect("store");
        store.set_internal_value(ENABLED_AT_KEY, "123");

        preserve_saved_key_marker(&store).expect("migrate saved key marker");
        store.disable_burn_checks().expect("pause checks");

        assert!(saved_key_marker(&store));
        assert!(store.internal_value(ENABLED_AT_KEY).is_none());
    }

    #[test]
    fn startup_restores_only_a_saved_valid_key() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-restored")).expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
        restore_saved_key(&store, &worker, || Ok(Some("synthetic-key".to_owned())))
            .expect("key restored");
        assert!(worker.is_available());

        worker.set_api_key(None);
        store.set_internal_value(AUTH_REJECTED_KEY, "true");
        restore_saved_key(&store, &worker, || Ok(Some("rejected-key".to_owned())))
            .expect("rejected key read");
        assert!(!worker.is_available());
    }

    #[test]
    fn removal_before_startup_restore_prevents_a_keychain_read() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-removed")).expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
        store.disable_burn_checks().expect("marker removed");
        restore_saved_key(&store, &worker, || panic!("removed key read from keychain"))
            .expect("restore skipped");
        assert!(!worker.is_available());
    }

    #[test]
    fn missing_saved_key_does_not_start_checks() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-missing")).expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
        assert!(restore_saved_key(&store, &worker, || Ok(None)).is_err());
        assert!(!worker.is_available());
    }

    #[test]
    fn pending_credential_removal_blocks_startup_restore() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-pending-remove"))
            .expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
        store.set_internal_value(CREDENTIAL_CHANGE_PENDING_KEY, "true");
        assert!(
            restore_saved_key(&store, &worker, || {
                panic!("pending removal read the old credential")
            })
            .is_err()
        );
        assert!(!worker.is_available());
    }
}
