//! The IPC surface exposed to the webview.
//!
//! Commands stay thin: they translate a request into an engine, store, or
//! window-system call and map the result into something serializable. Anything
//! that needs real logic belongs in the engine, the store, or one of the
//! shell's own modules.
//!
//! Errors cross the boundary as strings. A command that fails because something
//! is simply *absent* — a transcript the user deleted, a session that aged out —
//! returns an empty success instead, because the views have states for those and
//! an error banner would be a lie.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION, ProviderHint, SessionEvidence,
    SourceAcceptance, price_breakdown,
};
use antiburn_local::insights::{
    BadgeId, BadgeStatus, NotAssessedReason, ReportCatalogs, session_badges,
};
use antiburn_local::paths::scan_roots as engine_scan_roots;
use antiburn_local::paths::{home_dir, protected};
use antiburn_local::pricing::ModelTokens;
use antiburn_local::repositories as repositories_engine;
use antiburn_local::repositories::platform::{PlatformDiscovery as _, platform};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::agents::kind_from_slug;
use crate::analysis;
use crate::consent;
use crate::dto::{
    ActivityEntry, AgentScanState, AggregateWinsPayload, AppInfo,
    ApplyPreparedBurnCheckOperationOutcome, AutoFixUnavailableReason, BurnCheckDetectorId,
    BurnCheckRemediationProgressPayload, BurnCheckSnoozePayload, BurnCheckTargetListPayload,
    ChecksCategoryLifecyclePayload, ChecksReportPayload, CopyPromptFixBurnCheckOutcome,
    CopyPromptFixBurnCheckTargetOutcome, DeferredPermissionDir, HygieneSummaryPayload,
    InsightsBacklog, LiveUsageSummary, OrchestrationStatus, PrepareAutoFixBurnCheckTargetOutcome,
    PromptFixUnavailableReason, ProviderUsageSummary, RepositoryItem, ScanStatus, SessionAnalysis,
    SessionHygienePayload, SessionHygieneRequest, SessionIdentity, SessionLimitAllocation,
    SessionLimitAllocationSummary, SessionRelation, SessionRelations, SubagentMember,
};
pub(crate) mod local_usage;
pub(crate) mod quota;
#[cfg(test)]
mod reader_routing_tests;

use crate::insights_ipc::InsightsController;
use crate::insights_report::ReportRequest;
use crate::popover;
use crate::provider_usage;
use crate::remediation::{
    BurnCheckTargetContext, BurnCheckTargetEvidence, ControllerError, RemediationController,
};
use crate::repositories;
use crate::scan::{self, ScanController, ScanTrigger};
use crate::settings;
use crate::store::model::environment_key;
use crate::store::{
    AppSettings, RelationKind, RepositoryRecord, SessionKey, SessionRecord, Store, iso_from_epoch,
};

/// Anything that goes wrong becomes a string the webview can show.
pub(crate) type CommandResult<T> = Result<T, String>;

pub(crate) fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub(crate) async fn run_blocking<T, F>(operation: F) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CommandResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(fail)?
}

/// Logs how long one of the Overview's `UiReadStore`-routed commands spent
/// in its blocking body. A read at or past this threshold is worth calling
/// out in the ordinary log; a faster one stays at `debug` so it does not
/// crowd it.
const OVERVIEW_READ_SLOW_MS: u64 = 250;

pub(crate) fn log_overview_read_timing(command: &'static str, elapsed: std::time::Duration) {
    let elapsed_ms = elapsed.as_millis() as u64;
    if elapsed_ms >= OVERVIEW_READ_SLOW_MS {
        ::tracing::info!(event = "overview_read_timing", command, elapsed_ms);
    } else {
        ::tracing::debug!(event = "overview_read_timing", command, elapsed_ms);
    }
}

pub(crate) use local_usage::provider_priced_models;
#[cfg(test)]
pub(crate) use local_usage::session_limit_allocations;
pub(crate) mod usage;

/// Version stamp of the active runtime pricing catalog.
#[tauri::command]
pub fn engine_catalog_version(app: tauri::AppHandle) -> String {
    crate::runtime_pricing::catalog_version(&app)
}

/// Reveal a native window after its invoking renderer commits its shell.
#[tauri::command]
pub fn window_ready(window: tauri::WebviewWindow, generation: u64) {
    match window.label() {
        crate::popover::LABEL => {
            crate::popover::renderer_ready(&window, generation);
        }
        crate::settings::LABEL => crate::settings::renderer_ready(&window, generation),
        crate::onboarding::LABEL => crate::onboarding::renderer_ready(&window, generation),
        label => {
            ::tracing::debug!(event = "window_ready_ignored", window = label);
        }
    }
}

/// Reveal the main window after its renderer commits its shell.
#[tauri::command]
pub fn main_window_ready(window: tauri::WebviewWindow, generation: u64) {
    if window.label() == crate::main_window::LABEL {
        crate::main_window::renderer_ready(&window, generation);
    } else {
        ::tracing::debug!(event = "main_window_ready_ignored", window = window.label());
    }
}

/// Record when the popover's first activity and cached usage state settle.
#[tauri::command]
pub fn popover_content_ready(window: tauri::WebviewWindow, generation: u64) {
    if window.label() == crate::popover::LABEL {
        crate::popover::content_ready(&window, generation);
    }
}

/// Record when the main window's first activity and cached usage state
/// settle. Mirrors [`popover_content_ready`].
#[tauri::command]
pub fn main_window_content_ready(window: tauri::WebviewWindow, generation: u64) {
    if window.label() == crate::main_window::LABEL {
        crate::main_window::content_ready(&window, generation);
    }
}

/// Opens, or refocuses, the standalone settings window.
///
/// `pane` is optional and is a *request*: the frontend owns the pane list, so
/// an id it does not recognize simply leaves the window where it was.
///
/// `async` is load-bearing, not style. A synchronous command runs on the main
/// thread *inside* the calling webview's IPC callback. On Windows a webview
/// created from there never finishes: WebView2 cannot create a controller
/// while the thread is still inside one of its own callbacks, so
/// `WebviewWindowBuilder::build` keeps the main thread and the window never
/// loads its page. `async` moves the command to the async runtime, and
/// [`main_window::on_main_value`] then reaches the main thread through an
/// ordinary event-loop turn. Every command that can create a window owes the
/// same treatment.
#[tauri::command]
pub async fn open_settings_window(
    app: tauri::AppHandle,
    pane: Option<String>,
) -> CommandResult<()> {
    crate::main_window::on_main_value(&app, move |app| settings::open(app, pane).map_err(fail))
        .await?
}

/// The pane a caller asked for, taken once, as the settings window mounts.
#[tauri::command]
pub fn take_settings_pane(app: tauri::AppHandle) -> Option<String> {
    app.try_state::<settings::PendingPane>()
        .and_then(|pending| pending.take())
}

/// Quit antiburn.
///
/// The Settings sidebar's quit action and the tray menu's both land here, so
/// there is one exit path: `exit(0)` is what distinguishes a deliberate quit
/// from the window closes the shell suppresses (see `on_window_event`), and the
/// background tasks are aborted on the way out.
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    crate::main_window::on_main(&app, |app| {
        crate::main_window::exit_after_placement_flush(app);
    });
}

/// Post the settings pane's test notification.
///
/// The one notification the webview can cause, and only by this explicit
/// command: it goes through the same delivery path as every real kind (so a
/// reader sees exactly what they will get), bypassing only the master
/// preference — pressing the button *is* the permission.
#[tauri::command]
pub fn post_test_notification(app: tauri::AppHandle) {
    crate::notifications::note_test(&app);
}

/// Post a sample notification of one kind, for copy work.
///
/// Debug builds only: a release build refuses, so the row that sends this can
/// never become a way around the preferences.
#[tauri::command]
pub async fn post_sample_notification(app: tauri::AppHandle, kind: String) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("sample notifications are for debug builds only".to_string());
    }
    let kind = crate::notifications::Kind::from_id(&kind)
        .ok_or_else(|| format!("unknown notification kind: {kind}"))?;
    if kind == crate::notifications::Kind::UpdateAvailable {
        crate::updates::start_simulation(&app)
            .await
            .map_err(str::to_string)?;
    }
    crate::notifications::note_sample(&app, kind);
    Ok(())
}

/// Whether the local database is still accepting writes.
#[tauri::command]
pub fn get_storage_health(app: tauri::AppHandle) -> crate::storage_health::StorageHealthStatus {
    crate::storage_health::status(&app)
}

/* -------------------------------------------------------------------------
 * Popover window
 * ---------------------------------------------------------------------- */

/// Dismiss the popover — the Escape key's destination.
///
/// A shell command rather than the webview hiding its own window, because the
/// scan scheduler is gated on the popover being visible; hiding it behind the
/// shell's back would leave that gate stuck open.
#[tauri::command]
pub fn hide_popover(app: tauri::AppHandle) {
    popover::hide(&app);
}

/// Resize the popover to the height the view now on screen needs.
///
/// Clamped shell-side, so a webview bug cannot produce a window taller than the
/// display or shorter than its own chrome. `animate` is the *webview's* call:
/// the reduced-motion preference lives there, and a height change is motion.
/// Returns `true` only when this request reaches its target.
#[tauri::command]
pub async fn set_popover_height(app: tauri::AppHandle, height: f64, animate: Option<bool>) -> bool {
    popover::set_height(&app, height, animate.unwrap_or(true)).await
}

/// Keep the popover on screen while a native dialog it opened holds focus.
///
/// Paired with [`end_popover_hold`]; the webview wraps its folder-picker call
/// in the two so losing focus to the picker does not dismiss the surface the
/// reader is using.
#[tauri::command]
pub fn begin_popover_hold(app: tauri::AppHandle) {
    popover::begin_focus_hold(&app);
}

/// Release the dialog hold and hand focus back to the popover.
#[tauri::command]
pub fn end_popover_hold(app: tauri::AppHandle) {
    popover::end_focus_hold(&app);
}

/// Read bounded live rows with exact registry counts. Readers must subscribe before
/// requesting this snapshot.
#[tauri::command]
pub fn get_live_sessions(
    app: tauri::AppHandle,
    limit: Option<usize>,
) -> crate::session_lifecycle::LiveSnapshot {
    app.state::<crate::session_lifecycle::SessionEvents>()
        .snapshot(limit.unwrap_or(crate::session_lifecycle::DEFAULT_SNAPSHOT_LIMIT))
}

/// Read every named identity at one registry sequence. Requests cannot exceed
/// [`MAX_ACTIVITY_ROWS`].
#[tauri::command]
pub fn get_live_sessions_for(
    app: tauri::AppHandle,
    sessions: Vec<crate::session_lifecycle::SessionRef>,
) -> CommandResult<crate::session_lifecycle::LivePresence> {
    let sessions = bounded_presence_request(&sessions)?;
    Ok(app
        .state::<crate::session_lifecycle::SessionEvents>()
        .presence(sessions))
}

/// Reject oversized presence requests before acquiring the registry lock.
fn bounded_presence_request(
    sessions: &[crate::session_lifecycle::SessionRef],
) -> CommandResult<&[crate::session_lifecycle::SessionRef]> {
    if sessions.len() > MAX_ACTIVITY_ROWS {
        return Err("too many live session requests".to_owned());
    }
    Ok(sessions)
}

/// Where the app came from and what it is running against.
#[tauri::command]
pub async fn app_info(app: tauri::AppHandle) -> CommandResult<AppInfo> {
    run_blocking(move || {
        let store = app.state::<Store>();
        Ok(AppInfo {
            app_version: app.package_info().version.to_string(),
            debug_build: cfg!(debug_assertions),
            arch: std::env::consts::ARCH.to_string(),
            pricing_catalog_version: crate::runtime_pricing::catalog_version(&app),
            schema_version: store.schema_version().map_err(fail)?,
            data_dir: store.state_dir().to_string_lossy().to_string(),
            indexed_sessions: store.session_count().map_err(fail)?,
            database_bytes: store.database_bytes(),
            // Real registration state, not a compile-time guess: a release build
            // whose signing key was never configured has no working updater, and
            // every piece of copy downstream is derived from this one flag.
            updates_supported: crate::updates::supported(&app),
            // Same rule, same reason: derived from the build that is actually
            // running rather than from a `cfg!`, so no copy downstream can offer
            // a control this binary cannot honour.
            analytics_supported: crate::analytics::available(),
            analytics_environment_disabled: crate::analytics::environment_disabled(),
            analytics_operator: crate::analytics::operator().map(str::to_string),
        })
    })
    .await
}

/// Ask the release feed for a newer version.
#[tauri::command]
pub async fn check_for_updates(app: tauri::AppHandle) -> crate::updates::UpdateStatus {
    crate::updates::manual_check(&app).await
}

/// Return the latest updater state for a pane that mounted after its event.
#[tauri::command]
pub fn get_update_status(app: tauri::AppHandle) -> Option<crate::updates::UpdateStatus> {
    crate::updates::current_status(&app)
}

/// Start the fixed local update lifecycle used for interface testing.
#[tauri::command]
pub async fn start_update_simulation(
    app: tauri::AppHandle,
) -> CommandResult<crate::updates::UpdateStatus> {
    crate::updates::start_simulation(&app).await.map_err(fail)
}

/// Download, verify, and install the version the reader selected.
#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    expected_version: String,
) -> crate::updates::UpdateStatus {
    crate::updates::install(&app, &expected_version).await
}

/// Restart the application after an update installs.
#[tauri::command]
pub fn restart_to_update(app: tauri::AppHandle) -> CommandResult<()> {
    crate::updates::restart(&app).map_err(fail)
}

/* -------------------------------------------------------------------------
 * Settings
 * ---------------------------------------------------------------------- */

/// Every persisted preference.
#[tauri::command]
pub async fn get_settings(app: tauri::AppHandle) -> CommandResult<AppSettings> {
    let store = app.state::<Store>().inner().clone();
    run_blocking(move || store.settings().map_err(fail)).await
}

/// Event carrying the stored settings to every window after a write.
///
/// Preferences are written from the settings window but *rendered* in the
/// popover too (theme, the activity window, the pause state). The event is
/// what keeps a long-lived popover webview honest without a poll.
pub const SETTINGS_CHANGED_EVENT: &str = "settings:changed";
static SETTINGS_COMMAND_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tauri::command]
pub async fn set_settings(
    app: tauri::AppHandle,
    settings: AppSettings,
) -> CommandResult<AppSettings> {
    let _settings_command = SETTINGS_COMMAND_LOCK.lock().await;
    let remote_host_ids = crate::remote_sessions::lifecycle_host_ids(&app);
    let database_app = app.clone();
    let (previous, saved) = run_blocking(move || {
        crate::remote_sync::with_lifecycle_guard(&database_app, &remote_host_ids, || {
            let store = database_app.state::<Store>();
            let (previous, saved, removed) = {
                let _analytics_transition = crate::analytics::lock_settings_transition();
                let result = store.replace_settings_preserving_interface_scale(
                    &settings,
                    |tx, previous, saved| {
                        // The preference must still save when analytics serialization or
                        // queue storage fails. The withdrawal signal is best effort.
                        let _ = crate::analytics::prepare_opt_out_in_transaction(
                            &database_app,
                            tx,
                            previous,
                            saved,
                        );
                        crate::store::apply_session_retention_in(
                            tx,
                            saved.session_data_retention_days,
                            crate::retention::unix_now(),
                        )
                    },
                )?;
                crate::analytics::handle_settings_transition(&database_app, &result.0, &result.1);
                result
            };
            // This revision covers the completed retention commit. No report holds the
            // Store guard.
            let revision = store.revision();
            crate::retention::note_removed(&database_app, removed, revision);
            if let Ok(root) = crate::remote_sessions::directory(&database_app) {
                crate::remote_cache::prune_after_commit(&store, &root);
            }
            Ok((previous, saved))
        })
        .map_err(fail)
    })
    .await?;
    apply_settings_transition_on_main(&app, &previous, &saved).await?;
    let analytics_app = app.clone();
    let analytics_previous = previous.clone();
    let analytics_saved = saved.clone();
    run_blocking(move || {
        record_settings_transition(&analytics_app, &analytics_previous, &analytics_saved);
        Ok(())
    })
    .await?;
    Ok(saved)
}

/// Change the application interface size against the latest stored preset.
#[tauri::command]
pub async fn set_interface_scale(
    app: tauri::AppHandle,
    change: crate::interface_scale::InterfaceScaleChange,
    source: crate::interface_scale::InterfaceScaleSource,
) -> CommandResult<AppSettings> {
    let _settings_command = SETTINGS_COMMAND_LOCK.lock().await;
    let store = app.state::<Store>().inner().clone();
    let (previous, saved, target) = run_blocking(move || {
        store
            .update_settings_with(|settings| {
                let percent = crate::interface_scale::resolve_change(
                    settings.interface_scale_percent,
                    change,
                )
                .map_err(anyhow::Error::msg)?;
                settings.interface_scale_percent = percent;
                Ok(percent)
            })
            .map_err(fail)
    })
    .await?;
    let changed = saved.interface_scale_percent != previous.interface_scale_percent;
    debug_assert_eq!(saved.interface_scale_percent, target);
    let scale = crate::interface_scale::from_settings(&saved);
    let hud_app = app.clone();
    let hud_error = run_blocking(move || {
        Ok(
            crate::hud::reconcile_interface_scale(&hud_app, scale.factor())
                .err()
                .map(|error| format!("HUD: {error}")),
        )
    })
    .await?;
    let main_saved = saved.clone();
    let errors = crate::main_window::on_main_value(&app, move |app| {
        let mut errors: Vec<String> = hud_error.into_iter().collect();
        if let Err(error) = crate::interface_scale::reconcile_existing(app, scale) {
            ::tracing::error!(event = "interface_scale_reconcile_failed", percent = target, %error);
            errors.push(error);
        }
        if let Err(error) = crate::interface_scale::emit_settings_changed(app, &main_saved) {
            ::tracing::error!(event = "interface_scale_broadcast_failed", percent = target, %error);
            errors.push(error);
        }
        errors
    })
    .await?;
    if changed {
        let analytics_app = app.clone();
        run_blocking(move || {
            crate::analytics::record(
                &analytics_app,
                crate::analytics::event::EventName::InterfaceScaleChanged,
                crate::analytics::event::Facts {
                    label: crate::interface_scale::analytics_preset(target),
                    detail: Some(source.analytics_value()),
                    ..Default::default()
                },
            );
            Ok(())
        })
        .await?;
    }
    if errors.is_empty() {
        Ok(saved)
    } else {
        Err(errors.join("; "))
    }
}

/// Make setup pending, open it at Welcome, and keep all other local state.
#[tauri::command]
pub async fn restart_onboarding(app: tauri::AppHandle) -> CommandResult<()> {
    let _settings_command = SETTINGS_COMMAND_LOCK.lock().await;
    let store = app.state::<Store>().inner().clone();
    let (previous, saved) = run_blocking(move || store.restart_onboarding().map_err(fail)).await?;
    let main_previous = previous.clone();
    let main_saved = saved.clone();
    crate::main_window::on_main_value(&app, move |app| {
        crate::analytics::prepare_onboarding_restart();
        apply_settings_transition(app, &main_previous, &main_saved);
        restart_onboarding_surfaces(
            || crate::popover::hide_for_onboarding(app),
            || crate::onboarding::restart(app).map_err(fail),
        )
    })
    .await??;
    let analytics_app = app.clone();
    run_blocking(move || {
        record_settings_transition(&analytics_app, &previous, &saved);
        Ok(())
    })
    .await
}

fn restart_onboarding_surfaces(
    hide_popover: impl FnOnce(),
    open_onboarding: impl FnOnce() -> CommandResult<()>,
) -> CommandResult<()> {
    hide_popover();
    open_onboarding()
}

/// Commit the first-run choices and finish onboarding as one transition.
///
/// The webview treats these values as a draft until the final button. Keeping
/// the merge here means an unrelated preference written elsewhere cannot be
/// replaced by an older whole-settings snapshot from the onboarding window.
#[tauri::command]
pub async fn finish_onboarding(
    app: tauri::AppHandle,
    activity_window_days: u32,
    launch_at_login: bool,
    disabled_agents: Option<Vec<String>>,
    nudges_respect_dnd: Option<bool>,
) -> CommandResult<AppSettings> {
    let _settings_command = SETTINGS_COMMAND_LOCK.lock().await;
    let store = app.state::<Store>().inner().clone();
    let (previous, saved) = run_blocking(move || {
        store
            .update_settings(|settings| {
                settings.activity_window_days = activity_window_days;
                settings.launch_at_login = launch_at_login;
                if let Some(disabled) = disabled_agents {
                    settings.disabled_agents = crate::store::DisabledAgents::selected(disabled);
                }
                if let Some(respect) = nudges_respect_dnd {
                    settings.nudges_respect_dnd = respect;
                }
                settings.onboarding_completed = true;
            })
            .map_err(fail)
    })
    .await?;
    apply_settings_transition_on_main(&app, &previous, &saved).await?;
    let analytics_app = app.clone();
    let analytics_previous = previous.clone();
    let analytics_saved = saved.clone();
    run_blocking(move || {
        record_settings_transition(&analytics_app, &analytics_previous, &analytics_saved);
        if !analytics_previous.onboarding_completed && analytics_saved.onboarding_completed {
            crate::analytics::record_onboarding_finished(&analytics_app);
        }
        Ok(())
    })
    .await?;
    Ok(saved)
}

/// Report one interaction from the renderer.
///
/// Infallible and silent: analytics that could fail an action the reader
/// actually asked for would have their priorities inverted. The parameter is a
/// closed enum rather than a name and a property map — see
/// [`analytics::event::Interaction`](crate::analytics::event::Interaction).
#[tauri::command]
pub async fn note_interaction(
    app: tauri::AppHandle,
    interaction: crate::analytics::event::Interaction,
) -> CommandResult<()> {
    run_blocking(move || {
        crate::analytics::record_interaction(&app, interaction);
        Ok(())
    })
    .await
}

fn apply_settings_transition(app: &tauri::AppHandle, previous: &AppSettings, saved: &AppSettings) {
    // This transition means the current setup run is over. It can repeat only
    // after an explicit restart. Each completion refreshes data and explains
    // where the menu-bar app went.
    let finished_onboarding = !previous.onboarding_completed && saved.onboarding_completed;

    if crate::startup_registration::should_reconcile_after_save(previous, saved) {
        crate::startup_registration::reconcile(app, saved.launch_at_login);
    }
    crate::app_presence::apply_transition(app, previous, saved);

    // Finishing onboarding, widening the window past what the store holds,
    // resuming discovery, and changing the folder gate all want fresh data
    // immediately rather than at the next tick.
    let wants_scan = finished_onboarding
        || saved.activity_window_days > previous.activity_window_days
        || (previous.discovery_paused && !saved.discovery_paused)
        || previous.include_non_repo_folders != saved.include_non_repo_folders;
    if wants_scan && !saved.discovery_paused {
        app.state::<ScanController>()
            .request(ScanTrigger::SettingsTransition);
    }

    // Put the first-run window away and say where the app went. Done here
    // rather than in the webview because the window closing and the
    // notification arriving are one gesture, and only the shell can perform
    // both halves of it.
    if finished_onboarding {
        crate::onboarding::finish(app);
    }

    if !saved.live_usage_active() {
        crate::tray::clear_usage(app);
    } else if !previous.live_usage_active()
        && let Some(live) = app.try_state::<crate::usage_alerts::LiveUsage>()
    {
        crate::tray::sync_usage(app, &live.snapshot(), true, false);
    }

    // The webviews restyle themselves from the event; the native side of the
    // theme (window chrome, scrollbars, the `prefers-color-scheme` each
    // webview reports) follows AppHandle::set_theme, which covers every
    // current and future window.
    if saved.theme != previous.theme {
        app.set_theme(match saved.theme.as_str() {
            "light" => Some(tauri::Theme::Light),
            "dark" => Some(tauri::Theme::Dark),
            _ => None,
        });
    }

    // The working week moves the pace marker on every weekly bar. Republish
    // the held readings so the menu bar and each window follow the control,
    // rather than waiting for the next collection.
    if saved.working_week != previous.working_week && saved.live_usage_active() {
        crate::usage_alerts::republish_after_settings_change(app);
    }

    // The menu-bar free-space number follows its display preference on the
    // next poll tick; repainting here makes the toggle feel wired rather than
    // eventually-consistent.
    if saved.disk_space_display != previous.disk_space_display
        || saved.disk_space_threshold_gb != previous.disk_space_threshold_gb
    {
        crate::disk_monitor::refresh_title(app);
    }

    // On macOS, the Focus-status authorization waits for a completed setup,
    // the master notification switch, and the Do Not Disturb opt-in. The
    // function checks all three itself, and repeat calls are free (the gate
    // keeps a once-flag), so no transition edge needs to be computed here.
    crate::notifications::maybe_initialize_authorization(app);

    let _ = app.emit(SETTINGS_CHANGED_EVENT, &saved);
}

async fn apply_settings_transition_on_main(
    app: &tauri::AppHandle,
    previous: &AppSettings,
    saved: &AppSettings,
) -> CommandResult<()> {
    let previous = previous.clone();
    let saved = saved.clone();
    crate::main_window::on_main_value(app, move |app| {
        apply_settings_transition(app, &previous, &saved);
    })
    .await
}

fn record_settings_transition(app: &tauri::AppHandle, previous: &AppSettings, saved: &AppSettings) {
    // Which switch moved, never what it moved to, and only from this closed
    // list. A key alone answers "is this control being found at all"; the
    // value would start describing the reader's setup.
    for (changed, key) in [
        (
            previous.live_usage_enabled != saved.live_usage_enabled,
            "live_usage",
        ),
        (
            previous.notifications_enabled != saved.notifications_enabled,
            "notifications",
        ),
        (
            previous.launch_at_login != saved.launch_at_login,
            "launch_at_login",
        ),
        (
            previous.tray_icon_visible != saved.tray_icon_visible,
            "tray_icon",
        ),
        (
            previous.dock_icon_visible != saved.dock_icon_visible,
            "dock_icon",
        ),
        (
            previous.discovery_paused != saved.discovery_paused,
            "discovery_paused",
        ),
        (
            previous.include_non_repo_folders != saved.include_non_repo_folders,
            "include_non_repo_folders",
        ),
    ] {
        if changed {
            crate::analytics::record(
                app,
                crate::analytics::event::EventName::SettingToggled,
                crate::analytics::event::Facts {
                    label: Some(key),
                    ..Default::default()
                },
            );
        }
    }
}

/* -------------------------------------------------------------------------
 * Activity
 * ---------------------------------------------------------------------- */

/// The sessions to show in the popover, newest first.
///
/// `window_days` overrides the stored preference, so the list can be widened
/// without writing a setting first.
#[tauri::command]
pub async fn list_recent_sessions(
    app: tauri::AppHandle,
    window_days: Option<u32>,
    local_only: Option<bool>,
) -> CommandResult<Vec<ActivityEntry>> {
    run_blocking(move || {
        #[cfg(feature = "memory-probe")]
        if let Some(entries) = crate::memory_probe::synthetic_sessions()? {
            return Ok(entries);
        }
        let store = app.state::<Store>();
        let settings = store.settings().map_err(fail)?;
        let days = match window_days {
            Some(days) => days.clamp(
                crate::store::MIN_ACTIVITY_DAYS,
                crate::store::MAX_ACTIVITY_DAYS,
            ),
            None => settings.activity_window_days,
        };
        let now = scan::unix_now();
        let since = now - i64::from(days) * 86_400;
        let sessions = if local_only.unwrap_or(false) {
            store.recent_local_sessions_excluding(
                since,
                MAX_ACTIVITY_ROWS,
                &settings.disabled_agents,
            )
        } else {
            store.recent_sessions_excluding(since, MAX_ACTIVITY_ROWS, &settings.disabled_agents)
        }
        .map_err(fail)?;
        let repositories = store.repositories().map_err(fail)?;

        let mut entries = Vec::with_capacity(sessions.len());
        for session in sessions {
            entries.push(activity_entry(&store, &repositories, session, now).map_err(fail)?);
        }
        if entries
            .iter()
            .any(|entry| entry.cost.is_none() && !entry.models.is_empty())
        {
            crate::runtime_pricing::request_refresh(&app);
        }
        Ok(entries)
    })
    .await
}

/// Upper bound on rows one list request returns. Well past what any window can
/// show, and small enough that a machine with years of history cannot make the
/// popover's first paint unbounded.
const MAX_ACTIVITY_ROWS: usize = 500;

pub(crate) fn activity_entry(
    store: &Store,
    repositories: &[RepositoryRecord],
    session: SessionRecord,
    now: i64,
) -> anyhow::Result<ActivityEntry> {
    let analysis = store.analysis(&session.key)?;
    let (cost, models) = analysis
        .as_ref()
        .map(|record| {
            analysis::price_cached_breakdown(
                &record.model_breakdown_json,
                &record.pricing_breakdown_json,
            )
        })
        .unwrap_or((None, Vec::new()));
    let model_runs = analysis
        .as_ref()
        .map(|record| analysis::cached_inclusive_model_runs(&record.inclusive_models_json))
        .unwrap_or_default();
    let total_tokens = analysis
        .as_ref()
        .map(|record| analysis::cached_total_tokens(&record.model_breakdown_json))
        .unwrap_or(0);

    Ok(ActivityEntry {
        agent: session.key.agent.clone(),
        session_id: session.key.session_id.clone(),
        repo: repository_label(
            if session.key.remote_host_id().is_some() {
                &[]
            } else {
                repositories
            },
            session.cwd.as_deref(),
        ),
        timestamp: iso_from_epoch(session.updated_at_epoch),
        is_active: session.key.remote_host_id().is_none()
            && analysis::is_active(session.updated_at_epoch, now),
        surface: session.surface.clone(),
        wsl_distro: session.wsl_distro.clone(),
        remote_host_id: session.key.remote_host_id().map(str::to_owned),
        title: session.title.clone(),
        has_fork_parent: session.fork_parent_session_id.is_some(),
        fork_child_count: store.fork_children(&session.key)?.len() as u32,
        cost,
        total_tokens,
        models,
        model_runs,
    })
}

/// The repository a working directory belongs to, as a short display name.
///
/// Falls back to the directory's own last segment so a session outside every
/// known repository still says where it ran, and to empty when there is nothing
/// to say — which is what the list renders as "no repository".
fn repository_label(repositories: &[RepositoryRecord], cwd: Option<&str>) -> String {
    let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) else {
        return String::new();
    };
    let matched = repositories
        .iter()
        .filter(|record| {
            record
                .repo_root
                .as_deref()
                .is_some_and(|root| path_is_under(cwd, root))
        })
        // The deepest matching root wins, so a nested clone is not reported
        // under its parent.
        .max_by_key(|record| record.repo_root.as_deref().map(str::len).unwrap_or(0));
    match matched {
        Some(record) => record.repo_name.clone(),
        None => Path::new(cwd)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

/// Requeues one session's evidence row and wakes the durable worker, so a
/// gap the drilldown just found (rows not ready, or ready but stale) closes
/// on its own without the caller waiting on it. Errors are swallowed: this
/// is a best-effort nudge, not a step the drilldown's own response depends
/// on — the worker's next pass either way is what actually closes the gap.
fn requeue_and_wake_worker(app: &tauri::AppHandle, store: &Store, key: &SessionKey) {
    let _ = store.requeue_session_evidence(key);
    crate::insights_worker::wake(app);
}

/// Whether a served, replayed analysis is stale: a fresher pass is queued
/// (`pending`) or running (`processing`) behind the served fence.
///
/// `failed` is deliberately excluded: a failed pass has given up, so nothing
/// fresher is queued or running behind the served fence until something
/// requeues it, at which point it reads `pending` again. The scan layer is
/// what marks evidence `pending` on a real transcript change — see
/// `upsert_sessions` — so this status check is the single source of
/// staleness; it needs no separate fingerprint poll of the transcript.
/// Split out from [`get_session_analysis`] and [`get_subagent_analysis`] so
/// the rule itself is testable without a store or an app handle.
fn analysis_is_stale(evidence_status: Option<crate::store::EvidenceStatus>) -> bool {
    matches!(
        evidence_status,
        Some(crate::store::EvidenceStatus::Pending | crate::store::EvidenceStatus::Processing)
    )
}

fn path_is_under(path: &str, root: &str) -> bool {
    let path = path.replace('\\', "/");
    let path = path.trim_end_matches('/');
    let root = root.replace('\\', "/");
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

/* -------------------------------------------------------------------------
 * Session analysis
 * ---------------------------------------------------------------------- */

/// Everything the session-analysis surface renders for one session.
///
/// Returns a payload with no summary rather than an error when the transcript
/// is gone: a deleted conversation is an ordinary state, and the view says so.
#[tauri::command]
pub async fn get_session_analysis(
    app: tauri::AppHandle,
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
    remote_host_id: Option<String>,
) -> CommandResult<SessionAnalysis> {
    run_blocking(move || session_analysis(&app, agent, session_id, wsl_distro, remote_host_id))
        .await
}

fn session_analysis(
    app: &tauri::AppHandle,
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
    remote_host_id: Option<String>,
) -> CommandResult<SessionAnalysis> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    let key = SessionKey::for_origin(
        &agent,
        &session_id,
        wsl_distro.as_deref(),
        remote_host_id.as_deref(),
    )
    .map_err(str::to_owned)?;
    let store = app.state::<Store>();

    // Rows are the only way this command computes an analysis: every agent
    // is in the evidence cohort, so this always serves the worker's last
    // published pass — the last one that ever finished, even while a fresh
    // pass is requeued or in flight over an actively-written session. A
    // session that has never published at all reports a pending payload
    // instead of re-parsing the transcript in-process, and nudges the
    // worker so the gap closes on its own. Publishing a fresh pass is the
    // worker's job — see its announce callback in `insights_worker::spawn`
    // — so this command never caches one itself or reports a row fact
    // for it.
    let (analysis, analysis_pending, analysis_stale) =
        match analysis::analysis_from_rows(&store, &key, &session_id, kind) {
            Some(replayed) => {
                let evidence_status = store.evidence(&key).ok().flatten().map(|row| row.status);
                (replayed, false, analysis_is_stale(evidence_status))
            }
            None => {
                requeue_and_wake_worker(app, &store, &key);
                (analysis::SessionAnalysis::unavailable(), true, false)
            }
        };
    let relations = resolve_lineage(app, &key, wsl_distro.as_deref());

    let stored = store.session(&key).ok().flatten();

    let orchestration = match &analysis.orchestration {
        Some(orchestration) => Some(orchestration.clone()),
        // The listing came back empty. That is usually the truth, but it is
        // also what a momentarily unreadable transcript looks like, so a roster
        // the store already recorded is shown rather than silently dropped.
        None => cached_orchestration(&store, &key),
    };

    Ok(SessionAnalysis {
        summary: analysis.summary.clone(),
        supports_analysis: analysis::analysis_supported(kind),
        title: stored.as_ref().and_then(|record| record.title.clone()),
        wsl_distro,
        remote_host_id: remote_host_id.clone(),
        is_active: remote_host_id.is_none()
            && analysis::is_active(
                stored.as_ref().and_then(|record| record.updated_at_epoch),
                scan::unix_now(),
            ),
        cost: analysis.cost,
        top_level_cost: analysis.top_level_cost,
        subagents_cost: analysis.subagents_cost,
        inclusive_tokens: analysis.inclusive_tokens,
        subagents_tokens: analysis.subagents_tokens,
        efficiency: analysis.efficiency,
        models: analysis.models.clone(),
        model_runs: analysis.model_runs.clone(),
        orchestration,
        relations: (!relations.is_empty()).then_some(relations),
        started_at_epoch: analysis.started_at_epoch,
        source_path: remote_host_id
            .is_none()
            .then(|| stored_source_path(stored.as_ref()))
            .flatten(),
        project_path: remote_host_id
            .is_none()
            .then(|| stored_project_path(stored.as_ref()))
            .flatten(),
        analysis_pending,
        analysis_stale,
    })
}

/// The transcript path to reveal, from the store's own record of the source
/// — `None` for anything but a file, mirroring the old `analysis::source_path`
/// helper, which returned a path only for a file-backed `SessionSource`.
fn stored_source_path(stored: Option<&SessionRecord>) -> Option<String> {
    stored
        .filter(|record| record.source_kind == "file")
        .map(|record| record.source_label.clone())
}

/// Keep the recorded path available when a worktree no longer exists.
fn stored_project_path(stored: Option<&SessionRecord>) -> Option<String> {
    stored
        .and_then(|record| record.cwd.as_ref())
        .filter(|path| Path::new(path).is_absolute())
        .cloned()
}

/// One sub-agent's own analysis, opened from the roster.
#[tauri::command]
pub async fn get_subagent_analysis(
    app: tauri::AppHandle,
    agent: String,
    parent_session_id: String,
    subagent_id: String,
    wsl_distro: Option<String>,
    remote_host_id: Option<String>,
) -> CommandResult<SessionAnalysis> {
    run_blocking(move || {
        subagent_analysis(
            &app,
            agent,
            parent_session_id,
            subagent_id,
            wsl_distro,
            remote_host_id,
        )
    })
    .await
}

fn subagent_analysis(
    app: &tauri::AppHandle,
    agent: String,
    parent_session_id: String,
    subagent_id: String,
    wsl_distro: Option<String>,
    remote_host_id: Option<String>,
) -> CommandResult<SessionAnalysis> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    // Rows are the only way this command computes an analysis — see the
    // matching comment in `get_session_analysis`. A parent session that has
    // never published at all reports a pending payload and nudges the
    // worker, instead of re-parsing the sub-agent's own transcript
    // in-process.
    let store = app.state::<Store>();
    let parent_key = SessionKey::for_origin(
        &agent,
        &parent_session_id,
        wsl_distro.as_deref(),
        remote_host_id.as_deref(),
    )
    .map_err(str::to_owned)?;
    let (analysis, analysis_pending, analysis_stale) = match analysis::subagent_analysis_from_rows(
        &store,
        &parent_key,
        &parent_session_id,
        &subagent_id,
        kind,
    ) {
        Some(replayed) => {
            let evidence_status = store
                .evidence(&parent_key)
                .ok()
                .flatten()
                .map(|row| row.status);
            (replayed, false, analysis_is_stale(evidence_status))
        }
        None => {
            requeue_and_wake_worker(app, &store, &parent_key);
            (analysis::SessionAnalysis::unavailable(), true, false)
        }
    };
    Ok(SessionAnalysis {
        summary: analysis.summary.clone(),
        supports_analysis: analysis::analysis_supported(kind),
        title: None,
        wsl_distro,
        remote_host_id: remote_host_id.clone(),
        is_active: false,
        cost: analysis.cost,
        top_level_cost: analysis.top_level_cost,
        subagents_cost: analysis.subagents_cost,
        inclusive_tokens: analysis.inclusive_tokens,
        subagents_tokens: analysis.subagents_tokens,
        efficiency: analysis.efficiency,
        models: analysis.models.clone(),
        model_runs: analysis.model_runs.clone(),
        orchestration: None,
        relations: None,
        started_at_epoch: analysis.started_at_epoch,
        source_path: remote_host_id
            .is_none()
            .then(|| analysis.source_path.clone())
            .flatten(),
        project_path: None,
        analysis_pending,
        analysis_stale,
    })
}

/// The sub-agent roster the store already recorded, rebuilt as an orchestration status.
fn cached_orchestration(store: &Store, key: &SessionKey) -> Option<OrchestrationStatus> {
    let members: Vec<SubagentMember> = store
        .relations(key)
        .unwrap_or_default()
        .into_iter()
        .filter(|relation| relation.kind == RelationKind::Subagent)
        .map(|relation| SubagentMember {
            agent: key.agent.clone(),
            label: relation
                .label
                .clone()
                .unwrap_or_else(|| "Sub-agent".to_string()),
            subagent_id: relation.related_id,
            // The store's roster carries no cost, token, or model figures —
            // only the analysis pass computes those, and this path is the
            // fallback for when that pass came back empty this time.
            cost: None,
            tokens: None,
            model_runs: Vec::new(),
            started_at_epoch: None,
        })
        .collect();
    if members.is_empty() {
        return None;
    }
    Some(OrchestrationStatus {
        orchestrating: members.len() as u32 >= analysis::MIN_ORCHESTRATED_SUBAGENTS,
        orchestrator_agent: key.agent.clone(),
        orchestrator_session_id: key.session_id.clone(),
        subagent_count: members.len() as u32,
        members,
    })
}

/// One session's fork lineage, read from the store's own record of it.
///
/// Describe already resolves and writes the fork parent it finds — see
/// `describe_one_with_activity` and the `session_relation` row
/// `upsert_session_in` writes for it — so this reads that back instead of
/// re-discovering it: no `locate`, no transcript read, on a drilldown open.
fn resolve_lineage(
    app: &tauri::AppHandle,
    key: &SessionKey,
    wsl_distro: Option<&str>,
) -> SessionRelations {
    let store = app.state::<Store>();
    let parent_id = store.fork_parent(key).ok().flatten();

    let mut relations = SessionRelations {
        title: store
            .session(key)
            .ok()
            .flatten()
            .and_then(|record| record.title),
        ..SessionRelations::default()
    };

    if let Some(parent_id) = parent_id {
        let record = store
            .session(&SessionKey::new(
                &key.environment_key,
                &key.agent,
                &parent_id,
            ))
            .ok()
            .flatten();
        relations.parent = Some(SessionRelation {
            identity: SessionIdentity {
                agent: key.agent.clone(),
                session_id: parent_id,
                wsl_distro: wsl_distro.map(str::to_string),
                remote_host_id: key.remote_host_id().map(str::to_owned),
            },
            title: record.as_ref().and_then(|record| record.title.clone()),
            // A parent we still have a row for is on this machine, mirroring
            // the children loop below: retention removes the row when its
            // session data expires.
            available: record.is_some(),
        });
    }

    for child_id in store.fork_children(key).unwrap_or_default() {
        let child_key = SessionKey::new(&key.environment_key, &key.agent, &child_id);
        let record = store.session(&child_key).ok().flatten();
        relations.children.push(SessionRelation {
            identity: SessionIdentity {
                agent: key.agent.clone(),
                session_id: child_id,
                wsl_distro: wsl_distro.map(str::to_string),
                remote_host_id: key.remote_host_id().map(str::to_owned),
            },
            title: record.as_ref().and_then(|record| record.title.clone()),
            // A child we still have a row for is on this machine. The retention
            // policy removes the row when its session data expires.
            available: record.is_some(),
        });
    }

    relations
}

/* -------------------------------------------------------------------------
 * Scanning
 * ---------------------------------------------------------------------- */

/// Run a scan now, unless one is already in flight.
///
/// Explicit, so it runs even while background discovery is paused: pausing
/// stops antiburn from scanning on its own, not from being asked.
#[tauri::command]
pub async fn scan_now(
    app: tauri::AppHandle,
    activity_window_days: Option<u32>,
) -> CommandResult<ScanStatus> {
    Ok(scan::run_pass(
        &app,
        activity_window_days,
        ScanTrigger::ManualRescan,
        scan::PassScope::Full,
    )
    .await)
}

/// Run the dedicated historical pass now: Settings › General › Historical
/// scan. Widens discovery past the current window, up to the retention
/// limit — see `scan::history::window_secs`. Unlike [`scan_now`], a request
/// dropped because a pass is already running is queued rather than lost,
/// since no later routine pass would cover the same ground.
#[tauri::command]
pub async fn scan_history(app: tauri::AppHandle) -> CommandResult<ScanStatus> {
    Ok(scan::run_pass(
        &app,
        None,
        ScanTrigger::HistoricalScan,
        scan::PassScope::Full,
    )
    .await)
}

/// Ask the scan in flight to stop at its next phase boundary.
///
/// Everything it already persisted stays: a cancelled pass is a shorter pass,
/// not an undone one.
#[tauri::command]
pub fn cancel_scan(app: tauri::AppHandle) -> ScanStatus {
    let controller = app.state::<ScanController>();
    controller.request_cancel();
    controller.status()
}

/// What the current or last scan is doing, plus what each agent last saw.
#[tauri::command]
pub async fn get_scan_status(app: tauri::AppHandle) -> CommandResult<ScanStatus> {
    run_blocking(move || {
        let mut status = app.state::<ScanController>().status();
        status.agents = app
            .state::<Store>()
            .scan_state()
            .unwrap_or_default()
            .into_iter()
            .map(|(agent, last_completed_at, sessions_seen)| AgentScanState {
                agent,
                last_completed_at,
                sessions_seen,
            })
            .collect();
        Ok(status)
    })
    .await
}

/* -------------------------------------------------------------------------
 * Insights
 * ---------------------------------------------------------------------- */

/// Whether the insights worker pool has a backlog to drain right now. The
/// frontend's initial read for [`INSIGHTS_BACKLOG_CHANGED_EVENT`]'s state.
#[tauri::command]
pub fn get_insights_backlog(app: tauri::AppHandle) -> InsightsBacklog {
    InsightsBacklog {
        active: app
            .state::<crate::insights_worker::WorkerHandle>()
            .backlog_active(),
    }
}

/// Days of history the insights report covers. Shares
/// [`crate::store::model::CURRENT_WINDOW_DAYS`] with discovery, so the report
/// window and the discovery window can never drift apart.
const INSIGHTS_WINDOW_DAYS: i64 = crate::store::model::CURRENT_WINDOW_DAYS as i64;

fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Builds the one report request the pane can ask for.
fn insights_report_request(now_epoch: i64) -> ReportRequest {
    // One environment key per report, and the host report covers the
    // native scope only. It does not combine native and WSL scopes: the
    // reduction queries are pinned to single-scope semantics, and
    // detector statuses cannot be recombined from two finished reports
    // (clean and not-assessed do not merge). On macOS and Linux the
    // native scope is total, so nothing is excluded there. Windows hosts
    // do not have separate reports for each WSL environment.
    ReportRequest {
        environment_key: environment_key(None),
        window: antiburn_local::insights::ReportWindow {
            start_epoch: now_epoch - INSIGHTS_WINDOW_DAYS * 86_400,
            // The end bound is exclusive, so one past now keeps a session
            // that started this very second inside the window.
            end_epoch: now_epoch + 1,
        },
        computed_at_epoch: now_epoch,
    }
}

/// The bounded report data used by the popover All checks summary.
#[tauri::command]
pub async fn get_checks_report(
    window: tauri::WebviewWindow,
    consumer_id: String,
) -> CommandResult<ChecksReportPayload> {
    let started_at = Instant::now();
    if !matches!(window.label(), popover::LABEL | crate::main_window::LABEL) {
        return Err(fail("only Checks surfaces can read the Checks report"));
    }
    if consumer_id.is_empty() || consumer_id.len() > 128 {
        return Err(fail("the Checks consumer ID is invalid"));
    }
    let report_consumer_id = consumer_id.clone();
    let report_window = window.label().to_owned();
    let app = window.app_handle();
    crate::insights_worker::wake(app);
    let data_dir = app.state::<Store>().state_dir().to_path_buf();
    let request = insights_report_request(epoch_now());
    let reduced = app
        .state::<InsightsController>()
        .checks_report(data_dir, request.clone(), consumer_id)
        .await?;
    let reduction_ms = started_at.elapsed().as_millis() as u64;
    // The report carries three measurements that no other command reduces:
    // unknown record vocabulary, quota incidents, and provider incidents.
    // Each recorder compares the outcome against the last one it sent, so
    // repeated reports of the same state record nothing.
    crate::analytics::record_unrecognized_records(app, &reduced.report.unrecognized_records);
    crate::analytics::record_quota_incidents(app, &reduced.report.quota_pressure);
    crate::analytics::record_provider_incidents(app, &reduced.report.provider_incidents);
    let mut payload = ChecksReportPayload::from_reduced_report(&reduced);
    payload.smart_checks_available = app
        .state::<crate::jev::worker::WorkerHandle>()
        .is_available();
    for (id, check_id) in [
        (
            BurnCheckDetectorId::SkillOpportunities,
            "skill_opportunities",
        ),
        (BurnCheckDetectorId::OverExploring, "over_exploring"),
        (BurnCheckDetectorId::ScopeCreep, "scope_creep"),
    ] {
        if let Some(category) = payload
            .categories
            .iter_mut()
            .find(|category| category.id == id)
        {
            category.sampled = category.finding > 0 || category.clean > 0;
            if app
                .state::<Store>()
                .burn_check_in_progress_count(check_id)
                .map_err(fail)?
                > 0
            {
                apply_ignored_instruction_progress(category);
            }
        }
    }
    if let Some(category) = payload
        .categories
        .iter_mut()
        .find(|category| category.id == BurnCheckDetectorId::IgnoredInstructions)
    {
        category.sampled = crate::insights_report::has_published_sampled_instruction_assessment(
            app.state::<Store>().state_dir(),
            &request,
        )
        .map_err(fail)?;
    }
    app.state::<RemediationController>()
        .apply_category_lifecycles(
            &app.state::<Store>(),
            &mut payload,
            &request.environment_key,
        )
        .map_err(fail)?;
    let ignored_instruction_work = app
        .state::<Store>()
        .burn_check_in_progress_count("ignored_instructions")
        .map_err(fail)?;
    if ignored_instruction_work > 0
        && let Some(category) = payload
            .categories
            .iter_mut()
            .find(|category| category.id == BurnCheckDetectorId::IgnoredInstructions)
    {
        apply_ignored_instruction_progress(category);
    }
    #[cfg(debug_assertions)]
    if let Some(category) = payload
        .categories
        .iter()
        .find(|category| category.id == BurnCheckDetectorId::IgnoredInstructions)
    {
        ::tracing::debug!(
            event = "ignored_instruction_report_quality",
            in_progress_sessions = ignored_instruction_work,
            finding_sessions = category.finding,
            clean_sessions = category.clean,
            unavailable_sessions = category.unavailable,
            lifecycle = ?category.lifecycle,
        );
    }
    if !payload.smart_checks_available {
        payload.categories.retain(|category| {
            !matches!(
                category.id,
                BurnCheckDetectorId::IgnoredInstructions
                    | BurnCheckDetectorId::SkillOpportunities
                    | BurnCheckDetectorId::OverExploring
                    | BurnCheckDetectorId::ScopeCreep
            )
        });
    }
    let finding_count = payload
        .categories
        .iter()
        .map(|category| category.finding)
        .sum::<u64>();
    let clean_count = payload
        .categories
        .iter()
        .map(|category| category.clean)
        .sum::<u64>();
    ::tracing::debug!(
        event = "checks_report_finished",
        consumer_id = %report_consumer_id,
        window = %report_window,
        categories = payload.categories.len(),
        findings = finding_count,
        clean = clean_count,
        worker_woken = true,
        duration_ms = started_at.elapsed().as_millis() as u64,
        reduction_ms,
        lifecycle_ms = started_at.elapsed().as_millis() as u64 - reduction_ms,
    );
    #[cfg(debug_assertions)]
    let payload = {
        let mut payload = payload;
        crate::tray::simulate_burn_checks(app, &mut payload);
        payload
    };
    Ok(payload)
}

fn apply_ignored_instruction_progress(category: &mut crate::dto::ChecksCategoryPayload) {
    category.lifecycle = if category.finding > 0 {
        Some(ChecksCategoryLifecyclePayload::Failing)
    } else if category.unavailable > 0 {
        None
    } else {
        Some(ChecksCategoryLifecyclePayload::Passing)
    };
}

fn current_burn_check_snoozes(store: &Store) -> CommandResult<Vec<BurnCheckSnoozePayload>> {
    let now_ms = epoch_now() * 1_000;
    Ok(store
        .burn_check_snoozes()
        .map_err(fail)?
        .into_iter()
        .filter(|snooze| snooze.until.is_none_or(|until| until > now_ms))
        .collect())
}

/// List active reader-owned burn-check snoozes.
#[tauri::command]
pub async fn list_burn_check_snoozes(
    app: tauri::AppHandle,
) -> CommandResult<Vec<BurnCheckSnoozePayload>> {
    let store = app.state::<Store>().inner().clone();
    run_blocking(move || current_burn_check_snoozes(&store)).await
}

/// Save or replace one check-level snooze.
#[tauri::command]
pub async fn set_burn_check_snooze(
    app: tauri::AppHandle,
    snooze: BurnCheckSnoozePayload,
) -> CommandResult<Vec<BurnCheckSnoozePayload>> {
    let store = app.state::<Store>().inner().clone();
    let saved = run_blocking(move || {
        let mut snoozes = current_burn_check_snoozes(&store)?;
        snoozes.retain(|current| current.detector != snooze.detector);
        snoozes.push(snooze);
        store
            .save_burn_check_snoozes(&serde_json::to_string(&snoozes).map_err(fail)?)
            .map_err(fail)?;
        Ok(snoozes)
    })
    .await?;
    let _ = app.emit(BURN_CHECK_SNOOZES_CHANGED_EVENT, &saved);
    Ok(saved)
}

/// Remove one check-level snooze.
#[tauri::command]
pub async fn clear_burn_check_snooze(
    app: tauri::AppHandle,
    detector: BurnCheckDetectorId,
) -> CommandResult<Vec<BurnCheckSnoozePayload>> {
    let store = app.state::<Store>().inner().clone();
    let saved = run_blocking(move || {
        let mut snoozes = current_burn_check_snoozes(&store)?;
        snoozes.retain(|snooze| snooze.detector != detector);
        store
            .save_burn_check_snoozes(&serde_json::to_string(&snoozes).map_err(fail)?)
            .map_err(fail)?;
        Ok(snoozes)
    })
    .await?;
    let _ = app.emit(BURN_CHECK_SNOOZES_CHANGED_EVENT, &saved);
    Ok(saved)
}

/// Restricts burn-check remediation to the current Checks surface.
fn ensure_checks_window(label: &str) -> CommandResult<()> {
    if matches!(label, popover::LABEL | crate::main_window::LABEL) {
        Ok(())
    } else {
        Err(fail(
            "only the main window and popover can use burn check remediation",
        ))
    }
}

#[tauri::command]
pub async fn list_burn_check_targets(
    window: tauri::WebviewWindow,
    detector: BurnCheckDetectorId,
) -> CommandResult<BurnCheckTargetListPayload> {
    ensure_checks_window(window.label())?;
    let window_label = window.label().to_owned();
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started_at = Instant::now();
        let request = insights_report_request(epoch_now());
        let list = app
            .state::<RemediationController>()
            .list_burn_check_targets(
                &app.state::<Store>(),
                detector.into(),
                BurnCheckTargetContext {
                    environment_key: request.environment_key,
                    window: request.window,
                },
            )
            .map_err(|_| "unable to list burn check targets".to_owned())?;
        let payload = burn_check_target_list_payload(
            &app.state::<crate::main_window::MainWindowState>(),
            &app.state::<Store>(),
            list,
            epoch_now(),
            app.state::<crate::jev::worker::WorkerHandle>()
                .is_available(),
        )?;
        ::tracing::debug!(
            event = "burn_check_targets_finished",
            detector = ?detector,
            window = %window_label,
            targets = payload.targets.len(),
            samples = payload.samples.len(),
            truncated = payload.truncated,
            duration_ms = started_at.elapsed().as_millis() as u64,
        );
        Ok(payload)
    })
    .await
    .map_err(|_| "unable to list burn check targets".to_owned())?
}

#[tauri::command]
pub async fn get_burn_check_target_evidence(
    window: tauri::WebviewWindow,
    action_id: String,
) -> CommandResult<BurnCheckTargetEvidence> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started_at = Instant::now();
        let evidence = app
            .state::<RemediationController>()
            .burn_check_target_evidence(&app.state::<Store>(), &action_id)
            .map_err(|_| "unable to load burn check evidence".to_owned())?;
        ::tracing::debug!(
            event = "burn_check_evidence_finished",
            status = if evidence.status == crate::remediation::BurnCheckEvidenceStatus::Available {
                "available"
            } else {
                "unavailable"
            },
            items = evidence.items.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
        );
        Ok(evidence)
    })
    .await
    .map_err(|_| "unable to load burn check evidence".to_owned())?
}

#[tauri::command]
pub async fn get_burn_check_remediation_progress(
    window: tauri::WebviewWindow,
) -> CommandResult<BurnCheckRemediationProgressPayload> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<RemediationController>()
            .burn_check_remediation_progress(&app.state::<Store>())
            .map(Into::into)
            .map_err(|_| "unable to read burn check remediation progress".to_owned())
    })
    .await
    .map_err(|_| "unable to read burn check remediation progress".to_owned())?
}

fn burn_check_target_list_payload(
    state: &crate::main_window::MainWindowState,
    store: &Store,
    list: crate::remediation::BurnCheckTargetList,
    now: i64,
    smart_checks_available: bool,
) -> CommandResult<BurnCheckTargetListPayload> {
    let repositories = store.repositories().map_err(fail)?;
    let enrich = |samples: &[crate::remediation::BurnCheckSampleSession]| {
        crate::main_window::sample_payloads_from_store(
            state,
            store,
            &repositories,
            samples,
            now,
            smart_checks_available,
        )
    };
    let check_samples = enrich(&list.sample_sessions)?;
    let samples = list
        .targets
        .iter()
        .map(|target| enrich(&target.sample_sessions))
        .collect::<Result<Vec<_>, _>>()?;
    let mut payload: BurnCheckTargetListPayload = list.into();
    payload.samples = check_samples;
    for (target, samples) in payload.targets.iter_mut().zip(samples) {
        target.samples = samples;
    }
    Ok(payload)
}

fn prepare_auto_fix_outcome(
    result: Result<crate::remediation::AutoFixReview, ControllerError>,
) -> CommandResult<PrepareAutoFixBurnCheckTargetOutcome> {
    match result {
        Ok(review) => Ok(PrepareAutoFixBurnCheckTargetOutcome::ReviewReady {
            review: review.into(),
        }),
        Err(ControllerError::TargetExpired) => Ok(PrepareAutoFixBurnCheckTargetOutcome::Expired),
        Err(ControllerError::TargetChanged) => Ok(PrepareAutoFixBurnCheckTargetOutcome::Stale),
        Err(ControllerError::Conflict) => Ok(PrepareAutoFixBurnCheckTargetOutcome::Conflict),
        Err(ControllerError::TargetNotFound) => {
            Ok(PrepareAutoFixBurnCheckTargetOutcome::Unavailable {
                reason: AutoFixUnavailableReason::TargetNotFound,
            })
        }
        Err(ControllerError::AutoFixUnavailable(reason)) => {
            Ok(PrepareAutoFixBurnCheckTargetOutcome::Unavailable {
                reason: reason.into(),
            })
        }
        Err(ControllerError::PromptUnavailable(_))
        | Err(ControllerError::CheckPromptUnavailable)
        | Err(ControllerError::ApplyFailed(_))
        | Err(ControllerError::RecoveryNeeded { .. })
        | Err(ControllerError::PersistenceFailed)
        | Err(ControllerError::Internal) => Err("unable to prepare burn check fix".to_owned()),
    }
}

fn apply_prepared_outcome(
    result: Result<crate::remediation::AutoFixResult, ControllerError>,
) -> CommandResult<ApplyPreparedBurnCheckOperationOutcome> {
    use crate::agent_config::ApplyError;

    match result {
        Ok(result) => Ok(if result.verification_available {
            ApplyPreparedBurnCheckOperationOutcome::AppliedAwaitingVerification {
                watch_id: result.watch_id,
            }
        } else {
            ApplyPreparedBurnCheckOperationOutcome::AppliedVerificationUnavailable {
                watch_id: result.watch_id,
            }
        }),
        Err(ControllerError::RecoveryNeeded { watch_id }) => {
            Ok(ApplyPreparedBurnCheckOperationOutcome::RecoveryNeeded { watch_id })
        }
        Err(ControllerError::TargetExpired) => Ok(ApplyPreparedBurnCheckOperationOutcome::Expired),
        Err(ControllerError::TargetChanged) => Ok(ApplyPreparedBurnCheckOperationOutcome::Stale),
        Err(ControllerError::Conflict) => Ok(ApplyPreparedBurnCheckOperationOutcome::Conflict),
        Err(ControllerError::TargetNotFound) => {
            Ok(ApplyPreparedBurnCheckOperationOutcome::Unavailable {
                reason: AutoFixUnavailableReason::TargetNotFound,
            })
        }
        Err(ControllerError::AutoFixUnavailable(reason)) => {
            Ok(ApplyPreparedBurnCheckOperationOutcome::Unavailable {
                reason: reason.into(),
            })
        }
        Err(ControllerError::ApplyFailed(ApplyError::Conflict(_))) => {
            Ok(ApplyPreparedBurnCheckOperationOutcome::Conflict)
        }
        Err(ControllerError::ApplyFailed(ApplyError::Unavailable(_))) => {
            Ok(ApplyPreparedBurnCheckOperationOutcome::Unavailable {
                reason: AutoFixUnavailableReason::SafetyCheckFailed,
            })
        }
        Err(ControllerError::PromptUnavailable(_))
        | Err(ControllerError::CheckPromptUnavailable)
        | Err(ControllerError::ApplyFailed(ApplyError::Readback(_)))
        | Err(ControllerError::PersistenceFailed)
        | Err(ControllerError::Internal) => Err("unable to auto fix burn check target".to_owned()),
    }
}

/// Prepares one exact automatic change for review without reserving a write.
#[tauri::command]
pub async fn prepare_auto_fix_burn_check_target(
    window: tauri::WebviewWindow,
    action_id: String,
) -> CommandResult<PrepareAutoFixBurnCheckTargetOutcome> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        prepare_auto_fix_outcome(
            app.state::<RemediationController>()
                .prepare_auto_fix_burn_check_target(&app.state::<Store>(), &action_id),
        )
    })
    .await
    .map_err(|_| "unable to prepare burn check fix".to_owned())?
}

/// Applies only the exact operation returned by the review command.
#[tauri::command]
pub async fn apply_prepared_burn_check_operation(
    window: tauri::WebviewWindow,
    prepared_operation_id: String,
) -> CommandResult<ApplyPreparedBurnCheckOperationOutcome> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    let action_app = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        apply_prepared_outcome(
            action_app
                .state::<RemediationController>()
                .apply_prepared_burn_check_operation(
                    &action_app.state::<Store>(),
                    &prepared_operation_id,
                ),
        )
    })
    .await
    .map_err(|_| "unable to apply prepared burn check operation".to_owned())??;
    if matches!(
        outcome,
        ApplyPreparedBurnCheckOperationOutcome::AppliedAwaitingVerification { .. }
            | ApplyPreparedBurnCheckOperationOutcome::RecoveryNeeded { .. }
    ) {
        crate::insights_worker::wake(&app);
    }
    Ok(outcome)
}

fn prompt_fix_outcome(
    result: Result<crate::remediation::PromptFixResult, ControllerError>,
) -> CommandResult<CopyPromptFixBurnCheckTargetOutcome> {
    use antiburn_local::remediation::RemediationUnavailableReason;

    match result {
        Ok(result) => Ok(CopyPromptFixBurnCheckTargetOutcome::PromptReady {
            prompt: result.prompt,
            watch: result.watch.map(Into::into),
        }),
        Err(ControllerError::TargetExpired) => Ok(CopyPromptFixBurnCheckTargetOutcome::Expired),
        Err(ControllerError::TargetChanged) => Ok(CopyPromptFixBurnCheckTargetOutcome::Stale),
        Err(ControllerError::TargetNotFound) => {
            Ok(CopyPromptFixBurnCheckTargetOutcome::Unavailable {
                reason: PromptFixUnavailableReason::TargetNotFound,
            })
        }
        Err(ControllerError::PromptUnavailable(reason)) => {
            let reason = match reason {
                RemediationUnavailableReason::PromptSizeLimit => {
                    PromptFixUnavailableReason::PromptSizeLimit
                }
                RemediationUnavailableReason::EssentialIdentityUnavailable => {
                    PromptFixUnavailableReason::EssentialIdentityUnavailable
                }
                RemediationUnavailableReason::ProtectedBuiltInTool => {
                    PromptFixUnavailableReason::ProtectedBuiltInTool
                }
                RemediationUnavailableReason::DeferredAgent => {
                    PromptFixUnavailableReason::DeferredAgent
                }
                RemediationUnavailableReason::UnsupportedSourceFormat => {
                    PromptFixUnavailableReason::UnsupportedSourceFormat
                }
                RemediationUnavailableReason::CheckUnsupportedForAgent => {
                    PromptFixUnavailableReason::CheckUnsupportedForAgent
                }
            };
            Ok(CopyPromptFixBurnCheckTargetOutcome::Unavailable { reason })
        }
        Err(ControllerError::AutoFixUnavailable(_))
        | Err(ControllerError::CheckPromptUnavailable)
        | Err(ControllerError::Conflict)
        | Err(ControllerError::ApplyFailed(_))
        | Err(ControllerError::RecoveryNeeded { .. })
        | Err(ControllerError::PersistenceFailed)
        | Err(ControllerError::Internal) => Err("unable to copy prompt fix target".to_owned()),
    }
}

/// Returns a bounded prompt and starts or reuses its verification watch.
#[tauri::command]
pub async fn copy_prompt_fix_burn_check_target(
    window: tauri::WebviewWindow,
    action_id: String,
) -> CommandResult<CopyPromptFixBurnCheckTargetOutcome> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    let action_app = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        prompt_fix_outcome(
            action_app
                .state::<RemediationController>()
                .copy_prompt_fix_burn_check_target(&action_app.state::<Store>(), &action_id),
        )
    })
    .await
    .map_err(|_| "unable to copy prompt fix target".to_owned())??;
    if matches!(
        outcome,
        CopyPromptFixBurnCheckTargetOutcome::PromptReady { .. }
    ) {
        crate::insights_worker::wake(&app);
    }
    Ok(outcome)
}

/// Returns one bounded prompt for selectable current targets.
#[tauri::command]
pub async fn copy_prompt_fix_burn_check(
    window: tauri::WebviewWindow,
    detector: BurnCheckDetectorId,
) -> CommandResult<CopyPromptFixBurnCheckOutcome> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let request = insights_report_request(epoch_now());
        match app
            .state::<RemediationController>()
            .copy_prompt_fix_burn_check(
                &app.state::<Store>(),
                detector.into(),
                BurnCheckTargetContext {
                    environment_key: request.environment_key,
                    window: request.window,
                },
            ) {
            Ok(result) => Ok(CopyPromptFixBurnCheckOutcome::PromptReady {
                prompt: result.prompt,
            }),
            Err(ControllerError::CheckPromptUnavailable) => {
                Ok(CopyPromptFixBurnCheckOutcome::Unavailable)
            }
            Err(_) => Err("unable to copy burn check prompt".to_owned()),
        }
    })
    .await
    .map_err(|_| "unable to copy burn check prompt".to_owned())?
}

/// Returns one bounded prompt for all selected current targets in one check.
#[tauri::command]
pub async fn copy_prompt_fix_burn_check_targets(
    window: tauri::WebviewWindow,
    action_ids: Vec<String>,
) -> CommandResult<CopyPromptFixBurnCheckOutcome> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    let action_app = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        match action_app
            .state::<RemediationController>()
            .copy_prompt_fix_burn_check_targets(&action_app.state::<Store>(), &action_ids)
        {
            Ok(result) => Ok(CopyPromptFixBurnCheckOutcome::PromptReady {
                prompt: result.prompt,
            }),
            Err(ControllerError::TargetExpired)
            | Err(ControllerError::TargetChanged)
            | Err(ControllerError::TargetNotFound)
            | Err(ControllerError::CheckPromptUnavailable)
            | Err(ControllerError::PromptUnavailable(_)) => {
                Ok(CopyPromptFixBurnCheckOutcome::Unavailable)
            }
            Err(_) => Err("unable to copy burn check prompt".to_owned()),
        }
    })
    .await
    .map_err(|_| "unable to copy burn check prompt".to_owned())??;
    if matches!(outcome, CopyPromptFixBurnCheckOutcome::PromptReady { .. }) {
        crate::insights_worker::wake(&app);
    }
    Ok(outcome)
}

/// Returns bounded durable wins without reading current findings.
#[tauri::command]
pub async fn get_burn_check_aggregate_wins(
    window: tauri::WebviewWindow,
) -> CommandResult<AggregateWinsPayload> {
    ensure_checks_window(window.label())?;
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<RemediationController>()
            .aggregate_wins(&app.state::<Store>())
            .map(Into::into)
            .map_err(|_| "unable to read burn check aggregate wins".to_owned())
    })
    .await
    .map_err(|_| "unable to read burn check aggregate wins".to_owned())?
}

/// Release one Checks consumer without affecting another visible surface.
#[tauri::command]
pub fn cancel_checks_report(
    window: tauri::WebviewWindow,
    consumer_id: String,
) -> CommandResult<()> {
    if !matches!(window.label(), popover::LABEL | crate::main_window::LABEL) {
        return Err(fail("only Checks surfaces can cancel the Checks report"));
    }
    if consumer_id.is_empty() || consumer_id.len() > 128 {
        return Err(fail("the Checks consumer ID is invalid"));
    }
    window
        .app_handle()
        .state::<InsightsController>()
        .release_checks(&consumer_id);
    Ok(())
}

/// The aggregate hygiene numbers for the sessions in the activity window.
///
/// Same window and disabled-agent filter as `list_recent_sessions`, so the
/// summary describes the sessions the list shows.
#[tauri::command]
pub async fn get_hygiene_summary(app: tauri::AppHandle) -> CommandResult<HygieneSummaryPayload> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Store>();
        let settings = store.settings().map_err(fail)?;
        let since = scan::unix_now() - i64::from(settings.activity_window_days) * 86_400;
        let rows = store
            .hygiene_summary_rows(&environment_key(None), since, &settings.disabled_agents)
            .map_err(fail)?;
        Ok(hygiene_summary_payload(rows))
    })
    .await
    .map_err(fail)?
}

fn hygiene_summary_payload(rows: Vec<crate::store::HygieneSummaryRow>) -> HygieneSummaryPayload {
    let catalogs = ReportCatalogs::default();
    let total_sessions = rows.len() as u64;
    let mut settled_sessions = 0;
    let mut analyzed_sessions = 0;
    let mut failing_sessions = 0;
    let mut finding_counts = [0u64; BadgeId::ALL.len()];
    for row in rows {
        if row.settled {
            settled_sessions += 1;
        }
        let Some(evidence_json) = row.evidence_json else {
            continue;
        };
        let Ok(evidence) = serde_json::from_str::<SessionEvidence>(&evidence_json) else {
            continue;
        };
        analyzed_sessions += 1;
        let mut failed = false;
        for (index, badge) in session_badges(&evidence, &catalogs).iter().enumerate() {
            if badge.status == BadgeStatus::Finding {
                failed = true;
                finding_counts[index] += 1;
            }
        }
        if failed {
            failing_sessions += 1;
        }
    }
    // Ties keep the first badge in `BadgeId::ALL` order.
    let most_common_finding = finding_counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .max_by(|left, right| left.1.cmp(right.1).then(right.0.cmp(&left.0)))
        .map(|(index, _)| crate::dto::badge_id_str(BadgeId::ALL[index]));
    HygieneSummaryPayload {
        total_sessions,
        settled_sessions,
        analyzed_sessions,
        failing_sessions,
        most_common_finding,
    }
}

/// The hygiene badges for a bounded set of stored session evidence rows.
#[tauri::command]
pub async fn get_session_hygiene(
    app: tauri::AppHandle,
    sessions: Vec<SessionHygieneRequest>,
) -> CommandResult<Vec<SessionHygienePayload>> {
    if sessions.len() > MAX_ACTIVITY_ROWS {
        return Err("too many session hygiene requests".to_owned());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let keys = sessions
            .iter()
            .map(|session| {
                SessionKey::for_origin(
                    &session.agent,
                    &session.session_id,
                    session.wsl_distro.as_deref(),
                    session.remote_host_id.as_deref(),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_owned())?;
        let store = app.state::<Store>();
        let rows = store.evidence_batch(&keys).map_err(fail)?;
        let source_generations = store.source_generation_batch(&keys).map_err(fail)?;
        let checks_enabled = store
            .internal_value("internal:burnChecksEnabledAtEpochV1")
            .is_some();
        let findings = if checks_enabled
            && app
                .state::<crate::jev::worker::WorkerHandle>()
                .is_available()
        {
            crate::insights_report::ignored_instruction_session_statuses(store.state_dir(), &keys)
                .map_err(fail)?
        } else if checks_enabled {
            vec![
                crate::dto::IgnoredInstructionSessionStatus {
                    status: crate::dto::SessionHygieneStatus::CouldntCheck,
                    reason: Some("The TypeSafe API key is unavailable. Replace it in Settings."),
                };
                keys.len()
            ]
        } else {
            vec![
                crate::dto::IgnoredInstructionSessionStatus {
                    status: crate::dto::SessionHygieneStatus::NotAssessed,
                    reason: None,
                };
                keys.len()
            ]
        };
        let mut payloads = session_hygiene_payloads(rows, source_generations);
        attach_ignored_instruction_statuses(&mut payloads, findings);
        if checks_enabled {
            for (detector, id) in [
                (
                    antiburn_local::insights::DetectorId::SkillOpportunities,
                    "skillOpportunities",
                ),
                (
                    antiburn_local::insights::DetectorId::OverExploring,
                    "overExploring",
                ),
                (
                    antiburn_local::insights::DetectorId::ScopeCreep,
                    "scopeCreep",
                ),
            ] {
                attach_smart_check_statuses(
                    &mut payloads,
                    crate::insights_report::smart_session_statuses(
                        store.state_dir(),
                        &keys,
                        detector,
                        app.state::<crate::jev::worker::WorkerHandle>()
                            .is_available(),
                    )
                    .map_err(fail)?,
                    id,
                );
            }
        }
        Ok(payloads)
    })
    .await
    .map_err(fail)?
}

pub(crate) fn attach_ignored_instruction_statuses(
    payloads: &mut [SessionHygienePayload],
    statuses: impl IntoIterator<Item = crate::dto::IgnoredInstructionSessionStatus>,
) {
    attach_smart_check_statuses(payloads, statuses, "ignoredInstructions");
}

pub(crate) fn attach_smart_check_statuses(
    payloads: &mut [SessionHygienePayload],
    statuses: impl IntoIterator<Item = crate::dto::IgnoredInstructionSessionStatus>,
    id: &'static str,
) {
    for (payload, outcome) in payloads.iter_mut().zip(statuses) {
        payload.badges.retain(|badge| badge.id != id);
        if outcome.status != crate::dto::SessionHygieneStatus::NotAssessed {
            payload.badges.push(crate::dto::SessionHygieneBadgePayload {
                id,
                status: outcome.status,
                not_assessed_reason: None,
                check_reason: outcome.reason,
                accounting: None,
                finding_evidence: None,
            });
        }
    }
}

fn session_hygiene_payloads(
    rows: Vec<Option<crate::store::EvidenceRow>>,
    source_generations: Vec<Option<i64>>,
) -> Vec<SessionHygienePayload> {
    rows.into_iter()
        .zip(source_generations)
        .map(|(row, source_generation)| session_hygiene_payload(row, source_generation))
        .collect()
}

/// Parses `evidence_json` and, when it parses, serves its badges under
/// `evidence_state`. Returns `None` when there is no prior evidence to
/// serve, so the caller falls back to a `not_assessed` payload.
fn hygiene_payload_from_prior_evidence(
    evidence_json: Option<String>,
    evidence_state: &'static str,
) -> Option<SessionHygienePayload> {
    let evidence_json = evidence_json?;
    let evidence = serde_json::from_str::<SessionEvidence>(&evidence_json).ok()?;
    let catalogs = ReportCatalogs::default();
    Some(SessionHygienePayload::for_evidence(
        session_badges(&evidence, &catalogs),
        &evidence,
        &catalogs,
        evidence_state,
    ))
}

pub(crate) fn session_hygiene_payload(
    row: Option<crate::store::EvidenceRow>,
    source_generation: Option<i64>,
) -> SessionHygienePayload {
    let Some(row) = row else {
        return SessionHygienePayload::not_assessed(
            "pending",
            NotAssessedReason::IncompleteEvidence,
        );
    };

    match row.status {
        crate::store::EvidenceStatus::Ready => {
            let revisions_are_current = row.parser_revision == Some(PARSER_REVISION)
                && row.analyzer_revision == Some(ANALYZER_REVISION)
                && row.evidence_schema_revision == Some(EVIDENCE_SCHEMA_REVISION);
            // A session whose source grew a new generation, with the
            // requeue not yet run or still pending, must not serve badges
            // from the previous generation's evidence. This mirrors
            // `CURRENT_EVIDENCE_PREDICATE` in `insights_report.rs`.
            let generation_is_current =
                row.analyzed_generation.is_some() && row.analyzed_generation == source_generation;
            if !revisions_are_current || !generation_is_current {
                // The session UI deliberately shows the previous
                // generation's verdict while the requeue runs, marked
                // "stale" so the caller knows it is not current.
                // `CURRENT_EVIDENCE_PREDICATE` in `insights_report.rs` is
                // unchanged: the report cohort still excludes stale
                // evidence.
                return hygiene_payload_from_prior_evidence(row.evidence_json, "stale")
                    .unwrap_or_else(|| {
                        SessionHygienePayload::not_assessed(
                            "stale",
                            NotAssessedReason::IncompleteEvidence,
                        )
                    });
            }
            let Some(evidence_json) = row.evidence_json else {
                return SessionHygienePayload::not_assessed(
                    "failed",
                    NotAssessedReason::IncompleteEvidence,
                );
            };
            let Ok(evidence) = serde_json::from_str::<SessionEvidence>(&evidence_json) else {
                return SessionHygienePayload::not_assessed(
                    "failed",
                    NotAssessedReason::IncompleteEvidence,
                );
            };
            let evidence_state = if matches!(
                evidence.provenance.source_acceptance,
                SourceAcceptance::AcceptedPrefix { .. }
            ) {
                "activelyGrowing"
            } else {
                "ready"
            };
            let catalogs = ReportCatalogs::default();
            SessionHygienePayload::for_evidence(
                session_badges(&evidence, &catalogs),
                &evidence,
                &catalogs,
                evidence_state,
            )
        }
        crate::store::EvidenceStatus::Unsupported => {
            SessionHygienePayload::not_assessed("unsupported", NotAssessedReason::CapabilityMissing)
        }
        crate::store::EvidenceStatus::Pending | crate::store::EvidenceStatus::Processing => {
            // A row that is pending or processing again (fingerprint
            // churn on an actively growing session) keeps its last
            // published evidence_json until the requeue lands. Serve that
            // last verdict as "stale" instead of a blank not-assessed
            // state; only a row with no prior evidence falls back to the
            // status label.
            let status_label = row.status.as_str();
            hygiene_payload_from_prior_evidence(row.evidence_json, "stale").unwrap_or_else(|| {
                SessionHygienePayload::not_assessed(
                    status_label,
                    NotAssessedReason::IncompleteEvidence,
                )
            })
        }
        crate::store::EvidenceStatus::Failed => {
            SessionHygienePayload::not_assessed("failed", NotAssessedReason::IncompleteEvidence)
        }
    }
}

/* -------------------------------------------------------------------------
 * Sources
 * ---------------------------------------------------------------------- */

/// Every repository antiburn knows about on this machine.
#[tauri::command]
pub async fn list_repositories(app: tauri::AppHandle) -> CommandResult<Vec<RepositoryItem>> {
    run_blocking(move || repositories::list(&app.state::<Store>()).map_err(fail)).await
}

/// Include or ignore one repository.
#[tauri::command]
pub async fn set_repository_enabled(
    app: tauri::AppHandle,
    key: String,
    enabled: bool,
) -> CommandResult<Vec<RepositoryItem>> {
    let revision = {
        let store = app.state::<Store>();
        repositories::set_enabled(&store, &key, enabled)
            .await
            .map_err(fail)?;
        // Read a revision that covers the completed purge before reporting the removal.
        store.revision()
    };
    // A broad removal makes the registry check purged rows. The index invalidation
    // refreshes lists. Re-enabling requests discovery.
    if !enabled {
        crate::session_lifecycle::report(
            &app,
            crate::session_lifecycle::SyncObservation::Removed {
                scope: crate::session_lifecycle::RemovalScope::Broad,
                reason: crate::session_lifecycle::RemovalReason::Purged,
                revision,
            },
        );
    }
    crate::session_lifecycle::report(
        &app,
        crate::session_lifecycle::SyncObservation::IndexChanged {
            reason: crate::session_lifecycle::IndexChangeReason::Invalidated,
        },
    );
    crate::jev::settings::changed(&app);
    if enabled {
        app.state::<ScanController>()
            .request(ScanTrigger::RepositoryToggle);
    }
    list_repositories(app).await
}

/// Only the projection bridge emits this lifecycle scope with canonical sequences.
pub const SESSION_LIFECYCLE_EVENT: &str = "session:lifecycle";

/// Only the projection bridge emits enriched rows on this scope.
pub const SESSION_UPDATED_EVENT: &str = "session:updated";

/// Only the projection bridge emits membership changes and invalidations on this scope.
pub const SESSION_INDEX_CHANGED_EVENT: &str = "session:index-changed";
pub const CHECKS_REPORT_CHANGED_EVENT: &str = "checks:report-changed";
/// Only the insights worker pool emits this on a pool-wide backlog start/drain.
pub const INSIGHTS_BACKLOG_CHANGED_EVENT: &str = "insights-backlog-changed";
pub const BURN_CHECK_SNOOZES_CHANGED_EVENT: &str = "checks:snoozes-changed";

/// Re-derive the repository list from what is on disk right now.
#[tauri::command]
pub async fn refresh_repositories(app: tauri::AppHandle) -> CommandResult<Vec<RepositoryItem>> {
    repositories::refresh(&app).await.map_err(fail)?;
    list_repositories(app).await
}

/// The extra directories the reader pointed the scanner at.
#[tauri::command]
pub async fn list_scan_roots(app: tauri::AppHandle) -> CommandResult<Vec<String>> {
    let store = app.state::<Store>().inner().clone();
    run_blocking(move || store.scan_roots().map_err(fail)).await
}

/// The directories the engine already searches without being asked, shown in
/// onboarding so a reader can see that the common cases are covered.
#[tauri::command]
pub fn default_scan_roots() -> Vec<String> {
    let Some(home) = antiburn_local::paths::home_dir() else {
        return Vec::new();
    };
    platform()
        .common_code_dirs()
        .iter()
        .map(|dir| home.join(dir).to_string_lossy().to_string())
        .collect()
}

/// Add a directory to scan, and mirror the list into the engine's own store.
#[tauri::command]
pub async fn add_scan_root(app: tauri::AppHandle, path: String) -> CommandResult<Vec<String>> {
    let store = app.state::<Store>().inner().clone();
    let roots = run_blocking(move || {
        store.add_scan_root(&path).map_err(fail)?;
        store.scan_roots().map_err(fail)
    })
    .await?;
    mirror_scan_roots(&app, &roots).await.map_err(fail)?;
    app.state::<ScanController>()
        .request(ScanTrigger::ScanRootAdded);
    Ok(roots)
}

/// Stop scanning a directory, and mirror the list into the engine's own store.
#[tauri::command]
pub async fn remove_scan_root(app: tauri::AppHandle, path: String) -> CommandResult<Vec<String>> {
    let store = app.state::<Store>().inner().clone();
    let roots = run_blocking(move || {
        store.remove_scan_root(&path).map_err(fail)?;
        store.scan_roots().map_err(fail)
    })
    .await?;
    mirror_scan_roots(&app, &roots).await.map_err(fail)?;
    Ok(roots)
}

/// Rewrite the engine's `scan-roots.json` from the store's list.
///
/// The store is the source of truth because it can order and *remove* a root;
/// the engine's file is append-or-clear only, so the two are kept in step by
/// rewriting it wholesale rather than by editing it in place.
async fn mirror_scan_roots(app: &tauri::AppHandle, roots: &[String]) -> anyhow::Result<()> {
    let state_dir: PathBuf = app.state::<Store>().state_dir().to_path_buf();
    engine_scan_roots::clear(&state_dir).await?;
    for root in roots {
        engine_scan_roots::add_scan_root(&state_dir, root).await?;
    }
    Ok(())
}

/* -------------------------------------------------------------------------
 * Session actions
 * ---------------------------------------------------------------------- */

/// Write the privacy-scoped support diagnostics to `dest_path` as JSON.
///
/// The export omits transcript content, titles, paths, working directories,
/// account keys, analytics identifiers, and every `turn_content` value.
#[tauri::command]
pub async fn export_diagnostics(app: tauri::AppHandle, dest_path: String) -> CommandResult<String> {
    let data_dir = app.state::<Store>().state_dir().to_path_buf();
    let app_version = app.package_info().version.to_string();
    let json = tauri::async_runtime::spawn_blocking(move || {
        crate::diagnostics_export::build(&data_dir, app_version)?.to_json()
    })
    .await
    .map_err(fail)?
    .map_err(fail)?;
    tokio::fs::write(&dest_path, json).await.map_err(fail)?;
    Ok(dest_path)
}

/// Delete antiburn's own records for one session.
///
/// **Only antiburn's records.** The agent's transcript is the agent's file and
/// is never touched — deleting a conversation is that vendor's affair, not
/// this app's. What this removes is the cached metadata, the derived analysis,
/// and the relations, so the session disappears from antiburn's views until
/// a future scan rediscovers it on disk.
#[tauri::command]
pub async fn delete_session_data(
    app: tauri::AppHandle,
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
    remote_host_id: Option<String>,
) -> CommandResult<bool> {
    let key = SessionKey::for_origin(
        &agent,
        &session_id,
        wsl_distro.as_deref(),
        remote_host_id.as_deref(),
    )
    .map_err(str::to_owned)?;
    let action_app = app.clone();
    let delete_key = key.clone();
    let host_id = key.remote_host_id().map(str::to_owned);
    let removed = run_blocking(move || {
        if let Some(host_id) = host_id {
            crate::remote_sync::with_destructive_lifecycle_guard(
                &action_app,
                std::slice::from_ref(&host_id),
                || {
                    crate::remote_cache::delete_session(
                        &action_app.state::<Store>(),
                        crate::remote_sessions::directory(&action_app)
                            .ok()
                            .as_deref(),
                        &delete_key,
                    )
                },
            )
            .map_err(fail)
        } else {
            action_app
                .state::<Store>()
                .delete_session(&delete_key)
                .map_err(fail)
        }
    })
    .await?;
    if let Some((incarnation, revision)) = removed {
        crate::session_lifecycle::report(
            &app,
            crate::session_lifecycle::SyncObservation::Removed {
                scope: crate::session_lifecycle::RemovalScope::One(key, incarnation),
                reason: crate::session_lifecycle::RemovalReason::Deleted,
                revision,
            },
        );
    }
    Ok(removed.is_some())
}

/// Forget all session data in antiburn's local store.
///
/// **antiburn's own records only.** Not one provider file is touched: the
/// the agents' source transcripts stay exactly where they are, and a later
/// scan rebuilds everything this removed.
/// Preferences, scan folders, and repository include choices are kept — this is
/// "forget what you worked out", not "forget who I am".
///
/// Returns how many sessions were dropped, so the confirmation can report a
/// number rather than a shrug.
#[tauri::command]
pub async fn clear_local_index(app: tauri::AppHandle) -> CommandResult<usize> {
    let host_ids = crate::remote_sessions::host_ids(&app)?;
    let action_app = app.clone();
    let (removed, revision) = run_blocking(move || {
        crate::remote_sync::with_destructive_lifecycle_guard(&action_app, &host_ids, || {
            crate::remote_sessions::clear_cached_sessions_fenced(&action_app)
                .map_err(anyhow::Error::msg)?;
            action_app.state::<Store>().clear_local_session_data()
        })
        .map_err(fail)
    })
    .await?;
    // A fresh index has not earned its historical pass yet, even under a
    // retention that already covered the one this just dropped.
    crate::scan::history::reset_done(&app.state::<Store>());
    app.state::<ScanController>().reset_history_auto_request();
    // Report the broad removal and list invalidation before requesting index refill.
    crate::session_lifecycle::report(
        &app,
        crate::session_lifecycle::SyncObservation::Removed {
            scope: crate::session_lifecycle::RemovalScope::Broad,
            reason: crate::session_lifecycle::RemovalReason::Deleted,
            revision,
        },
    );
    crate::session_lifecycle::report(
        &app,
        crate::session_lifecycle::SyncObservation::IndexChanged {
            reason: crate::session_lifecycle::IndexChangeReason::Invalidated,
        },
    );
    // The index is empty and the popover is showing it. Refill it rather than
    // leaving a reader looking at an empty list until the next tick.
    app.state::<ScanController>()
        .request(ScanTrigger::IndexCleared);
    for host_id in crate::remote_sessions::host_ids(&app)? {
        crate::remote_sync::enqueue_automatic(&app, &host_id);
    }
    Ok(removed)
}

/* --------------------------------------------------------------------------
 * Folder permissions
 * ----------------------------------------------------------------------- */

/// What the last pass could and could not read.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderPermissions {
    /// Protected directories the last pass declined to read, in the order it
    /// met them.
    pub deferred: Vec<DeferredPermissionDir>,
    /// Directory names the user has already granted.
    pub granted: Vec<String>,
    /// Whether this platform guards directories behind consent at all. False
    /// everywhere but macOS, and the interface hides the whole surface when so.
    pub supported: bool,
}

/// The result of asking the operating system for a directory.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderAccessOutcome {
    /// `granted`, `denied`, or `recorded-denial`.
    pub outcome: String,
    /// How long the system took to answer. See
    /// [`consent::RECORDED_DENIAL_MS`] for why this is worth reporting.
    pub elapsed_ms: u64,
}

/// Which directories need permission, and which already have it.
#[tauri::command]
pub async fn get_folder_permissions(app: tauri::AppHandle) -> CommandResult<FolderPermissions> {
    run_blocking(move || {
        let store = app.state::<Store>();
        let mut granted: Vec<String> = store.granted_dirs().map_err(fail)?.into_iter().collect();
        granted.sort();
        Ok(FolderPermissions {
            deferred: store.deferred_permission_dirs().map_err(fail)?,
            granted,
            supported: !protected::protected_dir_names().is_empty(),
        })
    })
    .await
}

/// Ask the operating system for one protected directory.
///
/// **This is the call that raises the consent dialog**, and it is deliberate:
/// it runs only when the reader asks for it, after the interface has explained
/// what is about to happen. Two details are load-bearing and easy to lose:
///
/// 1. **The window is focused first.** antiburn has no dock icon and often no
///    visible window, and a consent dialog raised by an unfocused accessory app
///    can open behind everything else — the reader then waits on a prompt they
///    cannot see. The popover is held open across the call for the same reason
///    the folder picker holds it.
/// 2. **The elapsed time is measured around the probe alone.** It is what
///    separates "the reader answered a dialog" from "the system answered from a
///    decision it already had", so no other work may be folded into it.
///
/// A grant kicks a rescan: the directory's repositories were skipped by every
/// pass until now, and the reader who just granted it is watching for them.
#[tauri::command]
pub async fn request_folder_access(
    app: tauri::AppHandle,
    dir: String,
) -> CommandResult<FolderAccessOutcome> {
    let Some(home) = home_dir() else {
        return Err("no home directory".to_string());
    };
    if !protected::protected_dir_names().contains(&dir.as_str()) {
        return Err(format!("{dir} is not a consent-protected directory"));
    }

    popover::begin_focus_hold(&app);
    if let Some(window) = app.get_webview_window(popover::LABEL) {
        let _ = window.set_focus();
    }

    let outcome = {
        let store = app.state::<Store>();
        let consent = consent::StoreConsentGrants::new(&store);
        consent.probe_and_record(&home.join(&dir)).await
    };

    // Released before anything can fail. An early `?` between the two would
    // leave the hold in place for the rest of the run, and the popover would
    // stop dismissing on focus loss with nothing on screen to explain why.
    popover::end_focus_hold(&app);

    if matches!(outcome, consent::ProbeOutcome::Granted { .. }) {
        let store = app.state::<Store>().inner().clone();
        let granted_dir = dir.clone();
        run_blocking(move || {
            consent::StoreConsentGrants::new(&store)
                .grant(&granted_dir)
                .map_err(fail)
        })
        .await?;
    }

    if matches!(outcome, consent::ProbeOutcome::Granted { .. }) {
        app.state::<ScanController>()
            .request(ScanTrigger::FolderAccessGranted);
    }

    Ok(FolderAccessOutcome {
        outcome: outcome.label().to_string(),
        elapsed_ms: outcome.elapsed_ms(),
    })
}

/// Open the system pane where folder permissions are granted.
#[tauri::command]
pub fn open_folder_access_settings(app: tauri::AppHandle) -> CommandResult<()> {
    let url = repositories_engine::permission_settings_url()
        .ok_or_else(|| "no permission settings on this platform".to_string())?;
    app.opener().open_url(url, None::<&str>).map_err(fail)
}

/// Open the antiburn GitHub repository in the system browser.
#[tauri::command]
pub fn open_github_repo(app: tauri::AppHandle) -> CommandResult<()> {
    app.opener()
        .open_url("https://github.com/antiburn/antiburn", None::<&str>)
        .map_err(fail)
}

/// Open the official releases page used by the manual remote-helper setup.
#[tauri::command]
pub fn open_remote_helper_downloads(app: tauri::AppHandle) -> CommandResult<()> {
    app.opener()
        .open_url(
            "https://github.com/antiburn/antiburn/releases",
            None::<&str>,
        )
        .map_err(fail)
}

/// Open the public analytics documentation in the system browser.
#[tauri::command]
pub fn open_analytics_documentation(app: tauri::AppHandle) -> CommandResult<()> {
    let url = analytics_documentation_url(&app.package_info().version.to_string());
    app.opener().open_url(url, None::<&str>).map_err(fail)
}

fn analytics_documentation_url(version: &str) -> String {
    format!("https://github.com/antiburn/antiburn/blob/antiburn-v{version}/docs/analytics.md")
}

/// Open the public privacy policy in the system browser.
#[tauri::command]
pub fn open_privacy_policy(app: tauri::AppHandle) -> CommandResult<()> {
    app.opener()
        .open_url(
            "https://github.com/antiburn/antiburn/blob/main/docs/privacy-policy.md",
            None::<&str>,
        )
        .map_err(fail)
}

/// Probe outcomes from this run, for the reader to copy into a bug report.
///
/// Every entry uses the same vocabulary — `granted`, `denied`, `recorded-denial`
/// — whichever layer observed it, because the reader pasting this into an issue
/// should not have to know which one did.
#[tauri::command]
pub fn get_consent_diagnostics() -> Vec<consent::ProbeRecord> {
    consent::recent_probes()
}

/// Re-check protected directories for grants made outside antiburn.
///
/// **This can raise the consent dialog**, so it is reachable only from an
/// explicit action in settings — never from a background pass.
#[tauri::command]
pub async fn recheck_folder_permissions(app: tauri::AppHandle) -> CommandResult<Vec<String>> {
    let store = app.state::<Store>().inner().clone();
    let deferred: HashSet<String> = run_blocking(move || {
        Ok(store
            .deferred_permission_dirs()
            .map_err(fail)?
            .into_iter()
            .map(|entry| entry.dir)
            .collect())
    })
    .await?;
    if deferred.is_empty() {
        return Ok(Vec::new());
    }

    let Some(home) = home_dir() else {
        return Ok(Vec::new());
    };
    let mut discovered = HashSet::new();
    for dir in deferred {
        let outcome = {
            let store = app.state::<Store>();
            let consent = consent::StoreConsentGrants::new(&store);
            consent.probe_and_record(&home.join(&dir)).await
        };
        if !matches!(outcome, consent::ProbeOutcome::Granted { .. }) {
            continue;
        }
        let store = app.state::<Store>().inner().clone();
        let granted_dir = dir.clone();
        let recorded = run_blocking(move || {
            consent::StoreConsentGrants::new(&store)
                .grant(&granted_dir)
                .map_err(fail)
        })
        .await;
        if recorded.is_ok() {
            discovered.insert(dir);
        }
    }

    if !discovered.is_empty() {
        app.state::<ScanController>()
            .request(ScanTrigger::FolderAccessGranted);
    }
    let mut discovered: Vec<String> = discovered.into_iter().collect();
    discovered.sort();
    Ok(discovered)
}

/// Reveal a transcript in the platform's file manager.
///
/// The path is canonicalized and checked to exist before it reaches the
/// platform opener. The webview loads only this app's own bundle under a
/// restrictive CSP, so a hostile path cannot get here today — but "cannot get
/// here today" is a property of the *rest* of the app, and the one call that
/// hands a string to the operating system should not depend on it.
#[tauri::command]
pub fn reveal_source(
    app: tauri::AppHandle,
    path: String,
    remote_host_id: Option<String>,
) -> CommandResult<()> {
    if remote_host_id.is_some() {
        return Err("Cached remote transcripts cannot be opened from this Mac".into());
    }
    let target = revealable_path(&path)?;
    reject_remote_cache_path(&app, &target)?;
    app.opener().reveal_item_in_dir(target).map_err(fail)
}

#[derive(Debug, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProjectFolderTarget {
    Session {
        environment_key: String,
        agent: String,
        session_id: String,
    },
    BurnCheck {
        action_id: String,
    },
}

fn session_project_directory(store: &Store, key: &SessionKey) -> CommandResult<PathBuf> {
    let record = store
        .session(key)
        .map_err(fail)?
        .ok_or_else(|| "The session is no longer available".to_owned())?;
    let environment = &record.key.environment_key;
    if environment != "native"
        && !environment
            .strip_prefix("wsl:")
            .is_some_and(|distro| !distro.is_empty())
    {
        return Err("This session's project folder cannot be opened on this machine".into());
    }
    let path = stored_project_path(Some(&record))
        .ok_or_else(|| "The session has no local project directory".to_owned())?;
    project_directory(&path)
}

/// Resolve the folder from a stored session or an issued check action.
#[tauri::command]
pub async fn open_project_folder(
    app: tauri::AppHandle,
    target: ProjectFolderTarget,
) -> CommandResult<()> {
    run_blocking(move || {
        let target = match target {
            ProjectFolderTarget::Session {
                environment_key,
                agent,
                session_id,
            } => session_project_directory(
                &app.state::<Store>(),
                &SessionKey::new(environment_key, agent, session_id),
            )?,
            ProjectFolderTarget::BurnCheck { action_id } => {
                let path = app
                    .state::<RemediationController>()
                    .project_folder(&app.state::<Store>(), &action_id)
                    .map_err(|_| "The check's project folder is no longer available".to_owned())?;
                project_directory(&path)?
            }
        };
        reject_remote_cache_path(&app, &target)?;
        let target = target
            .into_os_string()
            .into_string()
            .map_err(|_| "The project path is not valid Unicode".to_string())?;
        app.opener().open_path(target, None::<&str>).map_err(fail)
    })
    .await
}

fn reject_remote_cache_path(app: &tauri::AppHandle, target: &Path) -> CommandResult<()> {
    let root = crate::remote_sessions::directory(app)?;
    let root = std::fs::canonicalize(root)
        .map_err(|_| "Remote session storage is unavailable".to_owned())?;
    if is_remote_cache_path(target, &root) {
        Err("Cached remote paths cannot be opened from this Mac".into())
    } else {
        Ok(())
    }
}

fn is_remote_cache_path(target: &Path, remote_root: &Path) -> bool {
    presentable(target.to_path_buf()).starts_with(presentable(remote_root.to_path_buf()))
}

fn project_directory(path: &str) -> CommandResult<PathBuf> {
    let target = revealable_path(path)?;
    if !target.is_dir() {
        return Err("The project path is not a directory".into());
    }
    Ok(target)
}

/// Validate and resolve a path before it is handed to the platform opener.
///
/// Absolute, existing, and canonical — in that order. Relative paths are
/// rejected outright rather than resolved, because "relative to what" has no
/// answer a command handler should be inventing.
fn revealable_path(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    if path.is_empty() || !candidate.is_absolute() {
        return Err(format!("{path} is not an absolute path"));
    }
    let resolved =
        std::fs::canonicalize(candidate).map_err(|_| format!("{path} is not on this machine"))?;
    Ok(presentable(resolved))
}

/// Windows canonicalization returns an extended-length (`\\?\`) path, which
/// several shells and file managers refuse to open. Strip the prefix back off
/// for presentation; everything else is unchanged.
#[cfg(windows)]
fn presentable(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => match rest.strip_prefix(r"UNC\") {
            Some(share) => PathBuf::from(format!(r"\\{share}")),
            None => PathBuf::from(rest),
        },
        None => path,
    }
}

#[cfg(not(windows))]
fn presentable(path: PathBuf) -> PathBuf {
    path
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod project_folder_tests {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::*;

    #[test]
    fn project_folder_requires_a_complete_trusted_target() {
        for value in [
            serde_json::json!({"path":"/tmp"}),
            serde_json::json!({"kind":"session","agent":"claude-code","sessionId":"same"}),
            serde_json::json!({"kind":"session","environmentKey":"ssh:host","agent":"claude-code","sessionId":"same","path":"/tmp"}),
        ] {
            assert!(serde_json::from_value::<ProjectFolderTarget>(value).is_err());
        }
    }

    #[test]
    fn an_ignored_instruction_result_adds_a_finding_to_the_session_badges() {
        let mut payloads = [SessionHygienePayload {
            badges: Vec::new(),
            evidence_state: "ready",
            unused_resources: None,
        }];

        attach_ignored_instruction_statuses(
            &mut payloads,
            [crate::dto::IgnoredInstructionSessionStatus {
                status: crate::dto::SessionHygieneStatus::Finding,
                reason: None,
            }],
        );

        assert_eq!(payloads[0].badges.len(), 1);
        assert_eq!(payloads[0].badges[0].id, "ignoredInstructions");
        assert!(matches!(
            payloads[0].badges[0].status,
            crate::dto::SessionHygieneStatus::Finding
        ));
    }

    #[test]
    fn an_in_progress_instruction_check_stays_unassessed_until_evidence_is_available() {
        let mut category = crate::dto::ChecksCategoryPayload {
            id: BurnCheckDetectorId::IgnoredInstructions,
            sampled: false,
            lifecycle: None,
            finding: 0,
            agents: Vec::new(),
            clean: 0,
            unavailable: 7,
            estimated_token_burn_basis_points: None,
        };
        apply_ignored_instruction_progress(&mut category);
        assert_eq!(category.lifecycle, None);

        category.unavailable = 0;
        apply_ignored_instruction_progress(&mut category);
        assert_eq!(
            category.lifecycle,
            Some(ChecksCategoryLifecyclePayload::Passing)
        );

        category.unavailable = 7;
        category.finding = 1;
        category.lifecycle = Some(ChecksCategoryLifecyclePayload::AwaitingVerification);
        apply_ignored_instruction_progress(&mut category);
        assert_eq!(
            category.lifecycle,
            Some(ChecksCategoryLifecyclePayload::Failing)
        );
    }

    #[test]
    fn every_enabled_instruction_check_gets_a_truthful_session_state() {
        let statuses = [
            crate::dto::IgnoredInstructionSessionStatus {
                status: crate::dto::SessionHygieneStatus::Checking,
                reason: Some("Waiting for current session evidence."),
            },
            crate::dto::IgnoredInstructionSessionStatus {
                status: crate::dto::SessionHygieneStatus::Clean,
                reason: None,
            },
            crate::dto::IgnoredInstructionSessionStatus {
                status: crate::dto::SessionHygieneStatus::CouldntCheck,
                reason: Some("The assessment limit was reached."),
            },
        ];
        let mut payloads = statuses
            .iter()
            .map(|_| SessionHygienePayload {
                badges: Vec::new(),
                evidence_state: "ready",
                unused_resources: None,
            })
            .collect::<Vec<_>>();

        attach_ignored_instruction_statuses(&mut payloads, statuses);

        assert_eq!(payloads.len(), 3);
        assert!(payloads.iter().all(|payload| {
            payload
                .badges
                .iter()
                .any(|badge| badge.id == "ignoredInstructions")
        }));
        assert_eq!(
            payloads[0].badges[0].check_reason,
            Some("Waiting for current session evidence.")
        );
        assert_eq!(
            payloads[2].badges[0].check_reason,
            Some("The assessment limit was reached.")
        );
    }

    #[test]
    fn hud_locking_commands_dispatch_to_blocking_workers() {
        let source = include_str!("../hud_commands.rs");
        for name in [
            "hide_overlay_window",
            "resize_overlay_window",
            "set_hud_detail_size",
            "tear_off_overlay",
            "set_hud_island",
        ] {
            let signature = format!("pub async fn {name}(");
            let body = source
                .split_once(&signature)
                .unwrap_or_else(|| panic!("{name} must not run on the UI thread"))
                .1
                .split_once("\n}")
                .expect("the command has a body")
                .0;
            let dispatch = body
                .find("run_blocking(move ||")
                .expect("a blocking worker");
            let hud_call = body[dispatch..]
                .find("antiburn_hud::")
                .expect("a HUD operation")
                + dispatch;
            assert!(dispatch < hud_call, "{name} dispatches before locking");
            assert!(body.contains(".await"), "{name} awaits completion");
        }
    }

    #[test]
    fn hud_notch_reads_do_not_dispatch_mutations_to_the_main_thread() {
        let source = include_str!("../hud_commands.rs");
        assert!(!source.contains("on_main_value(&app, antiburn_hud::settle_after_drag)"));
        assert!(!source.contains("move |app| antiburn_hud::restore_dock(app, dock)"));
        let restore = include_str!("../hud.rs")
            .split_once("pub fn restore_at_launch(")
            .unwrap()
            .1;
        let restore = restore.split_once("\n}").unwrap().0;
        let dispatch = restore
            .find("spawn_blocking")
            .expect("startup dispatches to a worker");
        let open = restore
            .find("antiburn_hud::open")
            .expect("startup opens the HUD");
        assert!(dispatch < open);
    }

    #[test]
    fn hud_hover_intent_stays_synchronous_and_outside_the_resize_lock() {
        let commands = include_str!("../hud_commands.rs");
        let hud = include_str!("../../crates/hud/src/lib.rs");
        for name in ["show_hud_detail", "hide_hud_detail"] {
            assert!(commands.contains(&format!("pub fn {name}(")));
        }
        let show = hud.split_once("pub fn show_detail(").unwrap().1;
        let show = show.split_once("\n}").unwrap().0;
        assert!(!show.contains("resize_apply_guard"));
        assert!(!show.contains("spawn"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hud_lock_contention_leaves_native_query_dispatch_responsive() {
        let lock = std::sync::Arc::new(std::sync::Mutex::new(()));
        let scale_lock = lock.clone();
        let (query_tx, query_rx) = tokio::sync::oneshot::channel();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let scale = tokio::spawn(run_blocking(move || {
            let _guard = scale_lock.lock().expect("scale lock");
            query_tx.send(()).expect("native query dispatch");
            reply_rx.recv_timeout(Duration::from_secs(5)).map_err(fail)
        }));
        query_rx
            .await
            .expect("scale holds the lock and requests the UI");
        let (resize_tx, resize_rx) = tokio::sync::oneshot::channel();
        let resize = tokio::spawn(run_blocking(move || {
            resize_tx.send(()).expect("resize starts");
            let _guard = lock.lock().expect("resize lock");
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(2), resize_rx)
            .await
            .expect("resize dispatch does not block the UI")
            .expect("resize starts while scale holds the lock");
        assert!(!resize.is_finished());
        reply_tx
            .send(())
            .expect("the UI can answer the native query");
        scale
            .await
            .expect("scale joins")
            .expect("native query succeeds");
        resize
            .await
            .expect("resize joins")
            .expect("resize completes");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_command_work_keeps_the_current_thread_runtime_responsive() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let operation = tokio::spawn(run_blocking(move || {
            let _ = started_tx.send(());
            release_rx
                .recv_timeout(Duration::from_secs(5))
                .map_err(fail)
        }));

        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .expect("the blocking operation starts without occupying the runtime")
            .expect("the blocking operation reports that it started");
        tokio::time::timeout(
            Duration::from_millis(250),
            tokio::time::sleep(Duration::from_millis(1)),
        )
        .await
        .expect("the current-thread runtime advances while blocking work remains");
        assert!(!operation.is_finished());
        release_tx
            .send(())
            .expect("the blocking operation accepts release");
        operation
            .await
            .expect("the command task joins")
            .expect("the blocking operation succeeds");
    }

    #[test]
    fn burn_check_remediation_rejects_unrelated_windows() {
        assert!(ensure_checks_window(popover::LABEL).is_ok());
        assert!(ensure_checks_window(crate::main_window::LABEL).is_ok());
        assert!(ensure_checks_window("settings").is_err());
        assert!(ensure_checks_window("onboarding").is_err());
        assert!(ensure_checks_window(crate::popover_peek::LABEL).is_err());
    }

    #[test]
    fn burn_check_snooze_command_rejects_malformed_stored_state() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-snooze-command-test")).unwrap();
        store.save_burn_check_snoozes("not json").unwrap();

        assert!(current_burn_check_snoozes(&store).is_err());
    }

    #[test]
    fn burn_check_payload_keeps_check_samples_diverse_and_target_samples_independent() {
        use crate::remediation::{
            AutoFixAvailability, BurnCheckDisplayFacts, BurnCheckResourceKind,
            BurnCheckSampleSession, BurnCheckScopeKind, BurnCheckTarget, BurnCheckTargetList,
            BurnCheckVerificationLimit, PromptFixAvailability,
        };
        use crate::store::{AnalysisRecord, EvidenceCompletion, PublishedEvidence};
        use antiburn_local::analysis::SourceFormat;
        use antiburn_local::insights::DetectorId;
        use antiburn_local::model::AgentKind;
        use antiburn_local::remediation::{DisplayFacts, FindingDisplay};

        let data_dir = tempfile::tempdir().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        let state = crate::main_window::MainWindowState::load(&store);
        let agents = [
            "claude-code",
            "claude-code",
            "claude-code",
            "codex",
            "codex",
            "codex",
        ];
        let mut samples = Vec::new();
        for (index, agent) in agents.into_iter().enumerate() {
            let mut record = session_record("file", "/synthetic/private-source.jsonl");
            record.key = SessionKey::new("native", agent, format!("private-session-{index}"));
            record.title = Some(format!("Review {index}"));
            record.cwd = Some("/synthetic/demo".into());
            record.updated_at_epoch = Some(990 - index as i64);
            record.source_fingerprint = Some(format!("fingerprint-{index}"));
            store
                .upsert_sessions(
                    std::slice::from_ref(&record),
                    &crate::agents::evidence_cohort(),
                )
                .unwrap();
            let claim = store
                .claim_next_evidence(&[agent], 1000, 60)
                .unwrap()
                .unwrap();
            assert_eq!(claim.key, record.key);
            let tokens = ModelTokens {
                input_tokens: 1000,
                output_tokens: 100,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cache_creation_1h_tokens: 0,
            };
            let breakdown =
                serde_json::to_string(&HashMap::from([("claude-sonnet-5", tokens)])).unwrap();
            let mut evidence = super::tests::synthetic_evidence();
            evidence.identity.agent = agent.into();
            evidence.identity.session_id = record.key.session_id.clone();
            assert!(
                store
                    .publish_projections(
                        &AnalysisRecord {
                            key: record.key.clone(),
                            model_breakdown_json: breakdown.clone(),
                            pricing_breakdown_json: breakdown,
                            inclusive_models_json: "[]".into(),
                            initial_context_json: None,
                            source_summaries_json: None,
                            provider_hints_json: None,
                            source_fingerprint: record.source_fingerprint.clone().unwrap(),
                            pricing_generation: 0,
                            analyzed_generation: claim.source_generation,
                            parser_revision: PARSER_REVISION,
                            analyzer_revision: ANALYZER_REVISION,
                            metrics_schema_revision: 0,
                        },
                        None,
                        &EvidenceCompletion {
                            claim_fence: claim.claim_fence,
                            status: PublishedEvidence::Ready,
                            evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                            evidence_json: serde_json::to_string(&evidence).unwrap(),
                        },
                        &[],
                        &[],
                    )
                    .unwrap()
            );
            samples.push(BurnCheckSampleSession {
                environment_key: record.key.environment_key,
                agent: record.key.agent,
                session_id: record.key.session_id,
                observed_at_ms: 990_000 - index as i64,
                incarnation: None,
            });
        }
        let target = BurnCheckTarget {
            over_exploring_reason: None,
            decision_proof: None,
            finding_id: "finding".into(),
            action_id: "action".into(),
            finding: FindingDisplay {
                detector: DetectorId::SessionsOverDepth,
                agent: AgentKind::Claude,
                source_format: SourceFormat::ClaudeJsonl,
                observation: "Long session".into(),
                certainty: None,
                instruction_provenance: None,
                facts: DisplayFacts {
                    labels: Vec::new(),
                    omitted: 0,
                },
            },
            display: BurnCheckDisplayFacts {
                resource_kind: BurnCheckResourceKind::Session,
                resource_identity: None,
                instruction_title: None,
                current_value: None,
                replacement_value: None,
                scope_kind: BurnCheckScopeKind::Session,
                quantity: None,
                quantity_unit: None,
                observation_count: 3,
                first_observed_at_ms: 1,
                last_observed_at_ms: 990_000,
                estimate_method: None,
                estimated_opportunity: None,
                estimated_token_burn_basis_points: None,
                verification_limit: BurnCheckVerificationLimit::CurrentEvidenceCannotProveFix,
            },
            occurrences: 3,
            affected_sessions: Some(3),
            project_name: None,
            project_location: None,
            project_path: None,
            config_file: None,
            auto_fix: AutoFixAvailability::Unavailable(
                crate::remediation::AutoFixUnavailableReason::UnsupportedOrUnprovenTarget,
            ),
            prompt_fix: PromptFixAvailability::Available,
            watch: None,
            evidence_available: false,
            coverage_limits: Vec::new(),
            sample_sessions: samples[..3].to_vec(),
            expires_at_epoch: 1600,
        };
        let mut overlapping_target = target.clone();
        overlapping_target.sample_sessions = vec![samples[0].clone()];
        let payload = burn_check_target_list_payload(
            &state,
            &store,
            BurnCheckTargetList {
                targets: vec![target, overlapping_target],
                sample_sessions: samples,
                truncated: false,
            },
            1000,
            false,
        )
        .unwrap();
        assert_eq!(payload.samples.len(), 6);
        assert_eq!(
            payload
                .samples
                .iter()
                .map(|sample| sample.agent.as_str())
                .collect::<Vec<_>>(),
            [
                "claude-code",
                "claude-code",
                "claude-code",
                "codex",
                "codex",
                "codex"
            ]
        );
        assert_eq!(payload.targets[0].samples.len(), 3);
        assert!(
            payload.targets[0]
                .samples
                .iter()
                .all(|sample| sample.agent == "claude-code")
        );
        assert_eq!(payload.targets[1].samples.len(), 1);
        assert_eq!(
            payload.samples[0].navigation_handle,
            payload.targets[1].samples[0].navigation_handle
        );
        for sample in &payload.samples {
            assert!(sample.title.starts_with("Review "));
            assert_eq!(sample.repo, "demo");
            assert!(sample.is_active);
            assert!(!sample.timestamp.is_empty());
            assert!(sample.cost.is_some());
            assert!(!sample.models.is_empty());
            assert_eq!(sample.hygiene.evidence_state, "ready");
            assert!(
                state
                    .resolve_sample_handle(&sample.navigation_handle, std::time::Instant::now())
                    .is_ok()
            );
        }
        let encoded = serde_json::to_string(&payload).unwrap();
        for private in [
            "sessionId",
            "environmentKey",
            "wslDistro",
            "/synthetic/",
            "private-session-",
        ] {
            assert!(!encoded.contains(private), "the payload exposes {private}");
        }
    }

    #[test]
    fn expected_auto_fix_failures_map_to_closed_outcomes() {
        assert!(matches!(
            apply_prepared_outcome(Err(ControllerError::TargetExpired)).unwrap(),
            ApplyPreparedBurnCheckOperationOutcome::Expired
        ));
        assert!(matches!(
            apply_prepared_outcome(Err(ControllerError::TargetChanged)).unwrap(),
            ApplyPreparedBurnCheckOperationOutcome::Stale
        ));
        assert!(matches!(
            apply_prepared_outcome(Err(ControllerError::ApplyFailed(
                crate::agent_config::ApplyError::Conflict(
                    crate::agent_config::ApplyConflict::ChangedContent
                )
            )))
            .unwrap(),
            ApplyPreparedBurnCheckOperationOutcome::Conflict
        ));
        assert!(apply_prepared_outcome(Err(ControllerError::Internal)).is_err());
        assert!(matches!(
            prepare_auto_fix_outcome(Err(ControllerError::TargetExpired)).unwrap(),
            PrepareAutoFixBurnCheckTargetOutcome::Expired
        ));
    }

    #[test]
    fn auto_fix_success_reports_verification_availability() {
        assert!(matches!(
            apply_prepared_outcome(Ok(crate::remediation::AutoFixResult {
                watch_id: "watch".into(),
                verification_available: true,
            }))
            .unwrap(),
            ApplyPreparedBurnCheckOperationOutcome::AppliedAwaitingVerification { .. }
        ));
        assert!(matches!(
            apply_prepared_outcome(Ok(crate::remediation::AutoFixResult {
                watch_id: "watch".into(),
                verification_available: false,
            }))
            .unwrap(),
            ApplyPreparedBurnCheckOperationOutcome::AppliedVerificationUnavailable { .. }
        ));
    }

    #[test]
    fn expected_prompt_failures_map_to_closed_outcomes() {
        assert!(matches!(
            prompt_fix_outcome(Err(ControllerError::TargetNotFound)).unwrap(),
            CopyPromptFixBurnCheckTargetOutcome::Unavailable {
                reason: PromptFixUnavailableReason::TargetNotFound
            }
        ));
        assert!(matches!(
            prompt_fix_outcome(Err(ControllerError::PromptUnavailable(
                antiburn_local::remediation::RemediationUnavailableReason::PromptSizeLimit
            )))
            .unwrap(),
            CopyPromptFixBurnCheckTargetOutcome::Unavailable {
                reason: PromptFixUnavailableReason::PromptSizeLimit
            }
        ));
        assert!(matches!(
            prompt_fix_outcome(Err(ControllerError::PromptUnavailable(
                antiburn_local::remediation::RemediationUnavailableReason::ProtectedBuiltInTool
            )))
            .unwrap(),
            CopyPromptFixBurnCheckTargetOutcome::Unavailable {
                reason: PromptFixUnavailableReason::ProtectedBuiltInTool
            }
        ));
        assert!(prompt_fix_outcome(Err(ControllerError::PersistenceFailed)).is_err());
    }

    #[test]
    fn analytics_documentation_matches_the_installed_release() {
        assert_eq!(
            analytics_documentation_url("0.1.0-rc.5"),
            "https://github.com/antiburn/antiburn/blob/antiburn-v0.1.0-rc.5/docs/analytics.md"
        );
    }

    fn repository(key: &str, name: &str, root: &str) -> RepositoryRecord {
        RepositoryRecord {
            key: key.into(),
            repo_name: name.into(),
            full_name: format!("avery/{name}"),
            status: "accessible".into(),
            repo_root: Some(root.into()),
            suspected_path: None,
            worktree_count: 1,
            session_count: 0,
            wsl_distro: None,
            enabled: true,
        }
    }

    #[test]
    fn restarting_onboarding_retires_the_popover_before_opening_setup() {
        let actions = RefCell::new(Vec::new());

        restart_onboarding_surfaces(
            || actions.borrow_mut().push("hide_popover"),
            || {
                actions.borrow_mut().push("open_onboarding");
                Ok(())
            },
        )
        .expect("the test transition succeeds");

        assert_eq!(*actions.borrow(), ["hide_popover", "open_onboarding"]);
    }

    /// The report request covers thirty days, ends one past now (the end
    /// bound is exclusive), and asks for the native scope only.
    #[test]
    fn the_insights_request_spans_thirty_days_of_the_native_scope() {
        let request = insights_report_request(1_000_000_000);
        assert_eq!(request.environment_key, "native");
        assert_eq!(request.computed_at_epoch, 1_000_000_000);
        assert_eq!(request.window.end_epoch, 1_000_000_001);
        assert_eq!(
            request.window.end_epoch - request.window.start_epoch,
            30 * 86_400 + 1
        );
    }

    #[test]
    fn a_working_directory_is_labelled_by_the_repository_that_contains_it() {
        let repositories = vec![repository("a", "widgets", "/home/avery/code/widgets")];
        assert_eq!(
            repository_label(&repositories, Some("/home/avery/code/widgets/src/api")),
            "widgets"
        );
    }

    #[test]
    fn a_nested_clone_wins_over_the_repository_above_it() {
        let repositories = vec![
            repository("a", "widgets", "/home/avery/code/widgets"),
            repository(
                "b",
                "vendored",
                "/home/avery/code/widgets/third_party/vendored",
            ),
        ];
        assert_eq!(
            repository_label(
                &repositories,
                Some("/home/avery/code/widgets/third_party/vendored/src")
            ),
            "vendored"
        );
    }

    #[test]
    fn a_directory_outside_every_repository_falls_back_to_its_own_name() {
        assert_eq!(repository_label(&[], Some("/tmp/scratch")), "scratch");
        assert_eq!(repository_label(&[], Some("")), "");
        assert_eq!(repository_label(&[], None), "");
    }

    #[test]
    fn a_sibling_directory_is_not_mistaken_for_the_repository() {
        let repositories = vec![repository("a", "widgets", "/home/avery/code/widgets")];
        assert_eq!(
            repository_label(&repositories, Some("/home/avery/code/widgets-legacy")),
            "widgets-legacy",
            "the fallback, not the neighbouring repository"
        );
    }

    #[test]
    fn epochs_render_as_the_iso_stamps_the_activity_list_parses() {
        assert_eq!(iso_from_epoch(Some(0)), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_epoch(Some(1_800_000_000)), "2027-01-15T08:00:00Z");
        // A session with no activity still yields a parseable stamp rather
        // than an empty string the list would drop.
        assert_eq!(iso_from_epoch(None), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_ready_or_unsupported_fence_is_not_stale() {
        assert!(!analysis_is_stale(Some(
            crate::store::EvidenceStatus::Ready
        )));
        assert!(!analysis_is_stale(Some(
            crate::store::EvidenceStatus::Unsupported
        )));
    }

    #[test]
    fn a_fence_left_by_a_requeue_or_a_running_pass_is_stale() {
        // The served rows are the last winning publish's, but the evidence
        // row itself is not terminal: a fresher pass is queued or running
        // behind them.
        assert!(analysis_is_stale(Some(
            crate::store::EvidenceStatus::Pending
        )));
        assert!(analysis_is_stale(Some(
            crate::store::EvidenceStatus::Processing
        )));
    }

    #[test]
    fn a_failed_pass_behind_an_earlier_publish_is_not_stale_on_its_own() {
        // Nothing fresher is queued or running: the worker gave up. The
        // served rows stay marked fresh until something requeues this row,
        // at which point it reads `pending` again.
        assert!(!analysis_is_stale(Some(
            crate::store::EvidenceStatus::Failed
        )));
    }

    fn session_record(source_kind: &str, source_label: &str) -> SessionRecord {
        SessionRecord {
            key: SessionKey::for_session("claude-code", "session-1", None),
            source_kind: source_kind.into(),
            source_label: source_label.into(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            updated_at_epoch: None,
            activity_cursor: String::new(),
            activity_source: "unknown".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    #[test]
    fn session_project_folder_uses_stored_origin_and_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(dir.path()).unwrap();
        let mut local = SessionRecord {
            key: SessionKey::new("native", "claude-code", "same"),
            source_kind: "file".into(),
            source_label: "/synthetic/local.jsonl".into(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            updated_at_epoch: None,
            activity_cursor: String::new(),
            activity_source: "unknown".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        };
        local.cwd = Some(dir.path().to_string_lossy().into_owned());
        let mut remote = local.clone();
        remote.key.environment_key = "ssh:host".into();
        remote.source_label = "/synthetic/remote.jsonl".into();
        let mut unknown = local.clone();
        unknown.key.environment_key = "unknown".into();
        unknown.source_label = "/synthetic/unknown.jsonl".into();
        let mut wsl = local.clone();
        wsl.key.environment_key = "wsl:ubuntu".into();
        wsl.wsl_distro = Some("Ubuntu".into());
        wsl.source_label = "/synthetic/wsl.jsonl".into();
        store
            .upsert_sessions(
                &[local.clone(), remote.clone(), unknown.clone(), wsl.clone()],
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
        let expected = project_directory(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(
            session_project_directory(&store, &local.key).unwrap(),
            expected
        );
        assert_eq!(
            session_project_directory(&store, &wsl.key).unwrap(),
            expected
        );
        assert!(session_project_directory(&store, &remote.key).is_err());
        assert!(session_project_directory(&store, &unknown.key).is_err());
        store.delete_session(&local.key).unwrap();
        assert!(session_project_directory(&store, &local.key).is_err());
        wsl.cwd = Some(dir.path().join("missing").to_string_lossy().into_owned());
        store
            .upsert_sessions(&[wsl.clone()], &crate::agents::evidence_cohort())
            .unwrap();
        assert!(session_project_directory(&store, &wsl.key).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_folder_cache_guard_checks_canonical_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("remote");
        std::fs::create_dir(&cache).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&cache, &alias).unwrap();
        let root = std::fs::canonicalize(&cache).unwrap();
        for path in [&cache, &alias] {
            assert!(is_remote_cache_path(
                &project_directory(path.to_str().unwrap()).unwrap(),
                &root
            ));
        }
    }

    #[test]
    fn project_directory_accepts_directories_and_rejects_other_inputs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(project_directory(dir.path().to_str().unwrap()).is_ok());
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "fixture").unwrap();
        assert!(project_directory(file.to_str().unwrap()).is_err());
        assert!(project_directory(dir.path().join("missing").to_str().unwrap()).is_err());
        for path in ["", "relative/folder", "https://example.com", "file:///tmp"] {
            assert!(project_directory(path).is_err());
        }
    }

    #[test]
    fn project_folder_analytics_rejects_path_fields_and_unknown_values() {
        use crate::analytics::event::Interaction;
        for value in [
            serde_json::json!({"kind":"projectFolderAction","action":"open","outcome":"succeeded"}),
            serde_json::json!({"kind":"projectFolderAction","action":"copy","outcome":"failed"}),
        ] {
            assert!(serde_json::from_value::<Interaction>(value).is_ok());
        }
        for value in [
            serde_json::json!({"kind":"projectFolderAction","action":"open","outcome":"succeeded","path":"/private/work"}),
            serde_json::json!({"kind":"projectFolderAction","action":"delete","outcome":"succeeded"}),
            serde_json::json!({"kind":"projectFolderAction","action":"copy","outcome":"unknown"}),
        ] {
            assert!(serde_json::from_value::<Interaction>(value).is_err());
        }
    }
}
