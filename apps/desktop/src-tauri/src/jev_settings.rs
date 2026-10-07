//! Native TypeSafe credentials and authorized window controls.

use antiburn_local::checks::DetectorId;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::dto::BurnCheckDetectorId;
use crate::jev_worker::WorkerHandle;
use crate::store::{BurnCheckHistoryStatus, BurnCheckUsageSummary, Store};

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
static CHECK_CHANGE: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    checks: Vec<CheckChoice>,
    revision: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckChoice {
    id: BurnCheckDetectorId,
    enabled: bool,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", content = "snapshot", rename_all = "snake_case")]
enum CheckAvailabilityEvent {
    Updated(Box<CheckAvailability>),
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

fn checks_settings_window(window: &WebviewWindow) -> Result<(), &'static str> {
    checks_settings_label(window.label())
}

fn checks_settings_label(label: &str) -> Result<(), &'static str> {
    if matches!(label, crate::settings::LABEL | crate::main_window::LABEL) {
        Ok(())
    } else {
        Err("This action is available only in the main window or Settings.")
    }
}

pub(crate) fn read_check_availability(
    store: &Store,
    configured: bool,
    saved_key: bool,
    error: Option<&'static str>,
) -> Result<CheckAvailability, &'static str> {
    let (enabled_checks, revision) = store
        .check_preferences_snapshot()
        .map_err(|_| "Could not read check preferences from the local database.")?;
    let history_checks = enabled_registered_history_checks(store)?;
    let status = store
        .historical_burn_check_status_for(
            time::OffsetDateTime::now_utc().unix_timestamp(),
            180,
            &history_checks,
        )
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
        checks: DetectorId::ALL
            .into_iter()
            .map(|detector| CheckChoice {
                id: detector.into(),
                enabled: enabled_checks.contains(&detector),
            })
            .collect(),
        revision,
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
        Ok(snapshot) => CheckAvailabilityEvent::Updated(Box::new(snapshot)),
        Err(_) => CheckAvailabilityEvent::Failed,
    };
    let _ = app.emit(AVAILABILITY_EVENT, event);
    let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
}

pub(crate) fn progress_changed(app: &AppHandle) {
    let event = match availability(app) {
        Ok(snapshot) => CheckAvailabilityEvent::Updated(Box::new(snapshot)),
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
            let enabled = enabled_registered_check_ids(&store).map_err(str::to_owned)?;
            store
                .capture_burn_check_boundaries(
                    &enabled,
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

fn enabled_registered_check_ids(store: &Store) -> Result<Vec<&'static str>, &'static str> {
    let enabled = store
        .enabled_checks()
        .map_err(|_| "Could not read check preferences from the local database.")?;
    Ok(crate::jev_worker::registered_check_ids()
        .into_iter()
        .filter(|detector| enabled.contains(detector))
        .map(DetectorId::key)
        .collect())
}

fn enabled_registered_history_checks(
    store: &Store,
) -> Result<Vec<crate::store::BurnCheckHistoryCheck>, &'static str> {
    let enabled = store
        .enabled_checks()
        .map_err(|_| "Could not read check preferences from the local database.")?;
    Ok(crate::jev_worker::registered_history_checks()
        .into_iter()
        .filter(|check| {
            DetectorId::from_key(&check.check_id).is_some_and(|id| enabled.contains(&id))
        })
        .collect())
}

fn resume_after_key_save(was_enabled: bool, replacing: bool, was_configured: bool) -> bool {
    was_enabled || (replacing && !was_configured)
}

#[tauri::command]
pub(crate) fn set_check_enabled(
    app: AppHandle,
    window: WebviewWindow,
    detector: BurnCheckDetectorId,
    enabled: bool,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CHECK_CHANGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let detector = DetectorId::from(detector);
    let store = app.state::<Store>();
    if store
        .check_enabled(detector)
        .map_err(|_| "Could not read the check preference.".to_owned())?
        == enabled
    {
        return availability(&app).map_err(str::to_owned);
    }

    let smart = crate::jev_worker::registered_check_ids().contains(&detector);
    if smart {
        app.state::<WorkerHandle>()
            .advance_check_generation(detector.key());
    }

    let changed_preference = store
        .set_check_enabled_with_smart_transition(
            detector,
            enabled,
            smart,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .map_err(|_| "Could not save the check preference.".to_owned())?;
    if !changed_preference {
        return availability(&app).map_err(str::to_owned);
    }
    app.state::<crate::insights_ipc::InsightsController>()
        .cancel();
    crate::session_lifecycle::report(
        &app,
        crate::session_lifecycle::SyncObservation::IndexChanged {
            reason: crate::session_lifecycle::IndexChangeReason::Invalidated,
        },
    );
    crate::jev_worker::wake(&app);
    changed(&app);
    crate::analytics::record_check_enablement_saved(&app, detector, enabled);
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
    let was_configured = saved_key_marker(&app.state::<Store>());
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
    let first_enablement = replacing && !was_configured;
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
    let resume_after_save = resume_after_key_save(was_enabled, replacing, was_configured);
    if replacing && !was_configured {
        // Capture current cursors before the worker can observe the key.
        let enabled = enabled_registered_check_ids(&store).map_err(str::to_owned)?;
        store
            .capture_burn_check_boundaries(
                &enabled,
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
    if resume_after_save {
        app.state::<WorkerHandle>().set_api_key(Some(key));
    }
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
    let history_checks = enabled_registered_history_checks(&store).map_err(str::to_owned)?;
    if history_checks.is_empty() {
        return Err("Turn on a Smart Burn Check before checking past sessions.".to_owned());
    }
    store
        .reconcile_evidence_revisions(
            &crate::agents::evidence_cohort(),
            crate::analysis::projection_revisions(),
        )
        .map_err(|_| "Could not refresh session evidence for this check.".to_owned())?;
    let queued = app
        .state::<Store>()
        .enqueue_burn_checks_for(
            time::OffsetDateTime::now_utc().unix_timestamp(),
            days,
            &history_checks,
        )
        .map_err(|_| "Could not queue checks for this period.".to_owned())?;
    let progress = store
        .historical_burn_check_status_for(
            time::OffsetDateTime::now_utc().unix_timestamp(),
            180,
            &history_checks,
        )
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
        checks_settings_label, preserve_saved_key_marker, restore_saved_key, resume_after_key_save,
        saved_key_marker,
    };
    use crate::jev_worker::WorkerHandle;
    use crate::store::Store;

    #[test]
    fn availability_failure_event_has_a_typed_failure_status() {
        assert_eq!(
            serde_json::to_value(CheckAvailabilityEvent::Failed).unwrap(),
            serde_json::json!({"status":"failed"})
        );
    }

    #[test]
    fn check_mutations_accept_only_main_and_settings_windows() {
        assert!(checks_settings_label(crate::main_window::LABEL).is_ok());
        assert!(checks_settings_label(crate::settings::LABEL).is_ok());
        assert!(checks_settings_label("popover").is_err());
    }

    #[test]
    fn replacing_a_saved_key_preserves_a_paused_master_switch() {
        assert!(!resume_after_key_save(false, true, true));
        assert!(resume_after_key_save(true, true, true));
        assert!(resume_after_key_save(false, true, false));
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
