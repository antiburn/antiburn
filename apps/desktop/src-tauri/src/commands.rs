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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION, SessionEvidence, SourceAcceptance,
};
use antiburn_local::insights::{NotAssessedReason, ReportCatalogs, session_badges};
use antiburn_local::model::AgentKind;
use antiburn_local::paths::scan_roots as engine_scan_roots;
use antiburn_local::paths::{home_dir, protected};
use antiburn_local::repositories as repositories_engine;
use antiburn_local::repositories::ConsentGrants as _;
use antiburn_local::repositories::platform::{PlatformDiscovery as _, platform};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::agents::kind_from_slug;
use crate::analysis;
use crate::consent;
use crate::dto::{
    ActivityEntry, AgentScanState, AppInfo, DeferredPermissionDir, InsightsReportPayload,
    InsightsStatusPayload, LiveUsageSummary, OrchestrationStatus, ProviderUsageSummary,
    RepositoryItem, ScanStatus, SessionAnalysis, SessionHygienePayload, SessionHygieneRequest,
    SessionIdentity, SessionRelation, SessionRelations, SubagentMember,
};
use crate::export::{ExportedSession, SessionExport};
use crate::insights_ipc::InsightsController;
use crate::insights_report::ReportRequest;
use crate::popover;
use crate::provider_usage;
use crate::repositories;
use crate::scan::{self, ScanController};
use crate::settings;
use crate::store::model::environment_key;
use crate::store::{
    AppSettings, RelationKind, RelationRecord, RepositoryRecord, SessionKey, SessionRecord, Store,
    iso_from_epoch,
};

/// Anything that goes wrong becomes a string the webview can show.
type CommandResult<T> = Result<T, String>;

fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Version stamp of the engine's bundled pricing catalog (`YYYY-MM-DD`).
///
/// Small on purpose, and load-bearing: it is the shell's end-to-end proof that
/// the webview, the IPC bridge, and the linked engine all work.
#[tauri::command]
pub fn engine_catalog_version() -> &'static str {
    antiburn_local::pricing::PRICING_CATALOG_VERSION
}

/// Reveal a native window after its invoking renderer commits its shell.
#[tauri::command]
pub fn window_ready(window: tauri::WebviewWindow, generation: u64) {
    match window.label() {
        crate::popover::LABEL => crate::popover::renderer_ready(&window, generation),
        crate::settings::LABEL => crate::settings::renderer_ready(&window, generation),
        crate::onboarding::LABEL => crate::onboarding::renderer_ready(&window, generation),
        label => {
            ::tracing::debug!(event = "window_ready_ignored", window = label);
        }
    }
}

/// Record when the popover's first activity and cached usage state settle.
#[tauri::command]
pub fn popover_content_ready(window: tauri::WebviewWindow, generation: u64) {
    if window.label() == crate::popover::LABEL {
        crate::popover::content_ready(&window, generation);
    }
}

/// Opens, or refocuses, the standalone settings window.
///
/// `pane` is optional and is a *request*: the frontend owns the pane list, so
/// an id it does not recognize simply leaves the window where it was.
#[tauri::command]
pub fn open_settings_window(app: tauri::AppHandle, pane: Option<String>) -> CommandResult<()> {
    settings::open(&app, pane).map_err(fail)
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
    app.exit(0);
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
#[tauri::command]
pub fn set_popover_height(app: tauri::AppHandle, height: f64, animate: Option<bool>) {
    popover::set_height(&app, height, animate.unwrap_or(true));
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

/* -------------------------------------------------------------------------
 * Overlay window
 * ---------------------------------------------------------------------- */

/// Open or re-show the always-on-top usage HUD.
#[tauri::command]
pub async fn open_overlay_window(app: tauri::AppHandle) -> CommandResult<()> {
    let entries = crate::hud::load_placements(&app.state::<Store>());
    antiburn_hud::open(&app, &entries).map_err(fail)
}

/// Remember where the HUD is, after a drag moved it.
///
/// No argument: the webview knows a drag ended, the shell knows where the
/// window is, and that split keeps geometry out of the IPC payload.
#[tauri::command]
pub fn record_hud_position(app: tauri::AppHandle) {
    crate::hud::record_position(&app);
}

/// Hide the usage HUD and cancel any pending reveal.
#[tauri::command]
pub fn hide_overlay_window(app: tauri::AppHandle) -> CommandResult<()> {
    antiburn_hud::hide(&app).map_err(fail)
}

/// Match the native HUD frame to the rendered panel.
#[tauri::command]
pub fn resize_overlay_window(
    app: tauri::AppHandle,
    height: f64,
    anchor_bottom: bool,
    animate: bool,
) -> CommandResult<()> {
    antiburn_hud::resize(&app, height, anchor_bottom, animate).map_err(fail)
}

/// Request the hover detail window with the newest usage payload.
///
/// The payload passes through opaque on purpose: the HUD webview produces it
/// and the detail webview consumes it, so the shell does not model its shape.
#[tauri::command]
pub fn show_hud_detail(app: tauri::AppHandle, state: serde_json::Value) {
    antiburn_hud::show_detail(&app, state);
}

/// Hide the hover detail window.
#[tauri::command]
pub fn hide_hud_detail(app: tauri::AppHandle) {
    antiburn_hud::hide_detail(&app);
}

/// Hide the detail window now that its webview cleared the card.
#[tauri::command]
pub fn conceal_hud_detail(app: tauri::AppHandle) {
    antiburn_hud::conceal_detail(&app);
}

/// Return the newest detail payload for a detail webview that mounts late.
#[tauri::command]
pub fn get_hud_detail_state() -> serde_json::Value {
    antiburn_hud::detail_state()
}

/// Size and place the detail window from its webview's measured height.
#[tauri::command]
pub fn set_hud_detail_size(app: tauri::AppHandle, height: f64) {
    antiburn_hud::apply_detail_size(&app, height);
}

/// Return the newest recent transcript write as epoch seconds.
#[tauri::command]
pub async fn get_latest_session_activity() -> Option<i64> {
    crate::hud::latest_session_activity().await
}

/// Where the app came from and what it is running against.
#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> CommandResult<AppInfo> {
    let store = app.state::<Store>();
    Ok(AppInfo {
        app_version: app.package_info().version.to_string(),
        debug_build: cfg!(debug_assertions),
        arch: std::env::consts::ARCH.to_string(),
        pricing_catalog_version: antiburn_local::pricing::PRICING_CATALOG_VERSION.to_string(),
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
pub fn get_settings(app: tauri::AppHandle) -> CommandResult<AppSettings> {
    app.state::<Store>().settings().map_err(fail)
}

/// Event carrying the stored settings to every window after a write.
///
/// Preferences are written from the settings window but *rendered* in the
/// popover too (theme, the activity window, the pause state). The event is
/// what keeps a long-lived popover webview honest without a poll.
pub const SETTINGS_CHANGED_EVENT: &str = "settings:changed";

#[tauri::command]
pub fn set_settings(app: tauri::AppHandle, settings: AppSettings) -> CommandResult<AppSettings> {
    let store = app.state::<Store>();
    let (previous, saved) = store.replace_settings(&settings).map_err(fail)?;
    apply_settings_transition(&app, &previous, &saved);
    Ok(saved)
}

/// Make setup pending, open it at Welcome, and keep all other local state.
#[tauri::command]
pub fn restart_onboarding(app: tauri::AppHandle) -> CommandResult<()> {
    let store = app.state::<Store>();
    let (previous, saved) = store.restart_onboarding().map_err(fail)?;
    apply_settings_transition(&app, &previous, &saved);
    restart_onboarding_surfaces(
        || crate::popover::hide_for_onboarding(&app),
        || crate::onboarding::restart(&app).map_err(fail),
    )
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
pub fn finish_onboarding(
    app: tauri::AppHandle,
    activity_window_days: u32,
    launch_at_login: bool,
) -> CommandResult<AppSettings> {
    let store = app.state::<Store>();
    let (previous, saved) = store
        .update_settings(|settings| {
            settings.activity_window_days = activity_window_days;
            settings.launch_at_login = launch_at_login;
            settings.onboarding_completed = true;
        })
        .map_err(fail)?;
    apply_settings_transition(&app, &previous, &saved);
    // An explicit restart records a new completion because it is a new setup run.
    crate::analytics::record(
        &app,
        crate::analytics::event::EventName::OnboardingFinished,
        crate::analytics::event::Facts::default(),
    );
    Ok(saved)
}

/// Report one interaction from the renderer.
///
/// Infallible and silent: analytics that could fail an action the reader
/// actually asked for would have their priorities inverted. The parameter is a
/// closed enum rather than a name and a property map — see
/// [`analytics::event::Interaction`](crate::analytics::event::Interaction).
#[tauri::command]
pub fn note_interaction(app: tauri::AppHandle, interaction: crate::analytics::event::Interaction) {
    crate::analytics::record_interaction(&app, interaction);
}

fn apply_settings_transition(app: &tauri::AppHandle, previous: &AppSettings, saved: &AppSettings) {
    // This transition means the current setup run is over. It can repeat only
    // after an explicit restart. Each completion refreshes data and explains
    // where the menu-bar app went.
    let finished_onboarding = !previous.onboarding_completed && saved.onboarding_completed;

    if crate::startup_registration::should_reconcile_after_save(previous, saved) {
        crate::startup_registration::reconcile(app, saved.launch_at_login);
    }

    // Finishing onboarding, widening the window past what the store holds, and
    // resuming discovery all want fresh data immediately rather than at the
    // next tick.
    let wants_scan = finished_onboarding
        || saved.activity_window_days > previous.activity_window_days
        || (previous.discovery_paused && !saved.discovery_paused);
    if wants_scan && !saved.discovery_paused {
        app.state::<ScanController>().request();
    }

    // Put the first-run window away and say where the app went. Done here
    // rather than in the webview because the window closing and the
    // notification arriving are one gesture, and only the shell can perform
    // both halves of it.
    if finished_onboarding {
        crate::onboarding::finish(app);
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

    // The menu-bar free-space number follows its display preference on the
    // next poll tick; repainting here makes the toggle feel wired rather than
    // eventually-consistent.
    if saved.disk_space_display != previous.disk_space_display
        || saved.disk_space_threshold_gb != previous.disk_space_threshold_gb
    {
        crate::disk_monitor::refresh_title(app);
    }

    // On macOS, the Focus-status authorization waits for a completed setup
    // and the master notification switch. The function checks both itself,
    // and repeat calls are free (the gate keeps a once-flag), so no
    // transition edge needs to be computed here.
    crate::notifications::maybe_initialize_authorization(app);

    // Consent changing is the queue's business: turning it off withdraws
    // whatever is already queued rather than merely pausing it, and destroys
    // the installation identifier so a later opt-in cannot be joined to this
    // one. Routed through the same hub as every other consequence so the two
    // can never drift apart.
    crate::analytics::handle_settings_transition(app, previous, saved);

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
            previous.discovery_paused != saved.discovery_paused,
            "discovery_paused",
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

    let _ = app.emit(SETTINGS_CHANGED_EVENT, &saved);
}

/* -------------------------------------------------------------------------
 * Activity
 * ---------------------------------------------------------------------- */

/// The sessions to show in the popover, newest first.
///
/// `window_days` overrides the stored preference, so the list can be widened
/// without writing a setting first.
#[tauri::command]
pub fn list_recent_sessions(
    app: tauri::AppHandle,
    window_days: Option<u32>,
) -> CommandResult<Vec<ActivityEntry>> {
    let store = app.state::<Store>();
    let days = match window_days {
        Some(days) => days.clamp(
            crate::store::MIN_ACTIVITY_DAYS,
            crate::store::MAX_ACTIVITY_DAYS,
        ),
        None => store.settings().map_err(fail)?.activity_window_days,
    };
    let now = scan::unix_now();
    let since = now - i64::from(days) * 86_400;
    let sessions = store
        .recent_sessions(since, MAX_ACTIVITY_ROWS)
        .map_err(fail)?;
    let repositories = store.repositories().map_err(fail)?;

    let mut entries = Vec::with_capacity(sessions.len());
    for session in sessions {
        entries.push(activity_entry(&store, &repositories, session, now).map_err(fail)?);
    }
    Ok(entries)
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
        .map(|record| analysis::price_cached_breakdown(&record.model_breakdown_json))
        .unwrap_or((None, Vec::new()));
    let model_runs = analysis
        .as_ref()
        .map(|record| analysis::cached_inclusive_model_runs(&record.inclusive_models_json))
        .unwrap_or_default();

    Ok(ActivityEntry {
        agent: session.key.agent.clone(),
        session_id: session.key.session_id.clone(),
        repo: repository_label(repositories, session.cwd.as_deref()),
        timestamp: iso_from_epoch(session.updated_at_epoch),
        is_active: analysis::is_active(session.updated_at_epoch, now),
        surface: session.surface.clone(),
        wsl_distro: session.wsl_distro.clone(),
        title: session.title.clone(),
        has_fork_parent: session.fork_parent_session_id.is_some(),
        fork_child_count: store.fork_children(&session.key)?.len() as u32,
        cost,
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

/// Nudges the worker when the analysis this pass just served from rows was
/// published against a transcript that has since changed.
///
/// Reuses [`analysis::fingerprint_with_subagents`] — the same cheap
/// `mtime:size` check `get_session_analysis_fingerprint` computes for the
/// webview's own 10s poll — so a fingerprint mismatch found here is exactly
/// the mismatch that poll will see next, and the requeue this triggers
/// covers the gap before then. Does nothing (and requeues nothing) when the
/// stored and live fingerprints already agree.
async fn nudge_if_evidence_stale(
    app: &tauri::AppHandle,
    store: &Store,
    kind: AgentKind,
    key: &SessionKey,
    session_id: &str,
    wsl_distro: Option<&str>,
) {
    let Some(source) = analysis::locate(kind, session_id, wsl_distro).await else {
        return;
    };
    let live_fingerprint =
        analysis::fingerprint_with_subagents(kind, session_id, wsl_distro, &source).await;
    let stored_fingerprint = store
        .analysis(key)
        .ok()
        .flatten()
        .map(|record| record.source_fingerprint);
    if evidence_is_stale(stored_fingerprint.as_deref(), &live_fingerprint) {
        requeue_and_wake_worker(app, store, key);
    }
}

/// Whether the stored analysis's own fingerprint no longer matches the
/// transcript's live one — split out from [`nudge_if_evidence_stale`] so the
/// comparison itself is testable without a located source or an app handle.
fn evidence_is_stale(stored_fingerprint: Option<&str>, live_fingerprint: &str) -> bool {
    stored_fingerprint != Some(live_fingerprint)
}

fn path_is_under(path: &str, root: &str) -> bool {
    let path = path.replace('\\', "/");
    let path = path.trim_end_matches('/');
    let root = root.replace('\\', "/");
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

/* -------------------------------------------------------------------------
 * Local provider usage
 * ---------------------------------------------------------------------- */

/// Per-provider token and cost totals derived from the sessions already on this
/// machine.
///
/// `utc_offset_minutes` is the webview's own offset from UTC. The shell asks
/// for it rather than reading the platform's, because "today" and "this month"
/// are the reader's calendar days, and resolving the local offset inside a
/// multi-threaded process is not reliable on every platform. Omitting it falls
/// back to UTC, which is right for a machine running on it and off by at most
/// one day's boundary for anyone else.
///
/// Nothing here contacts a provider. Every figure comes from
/// [`crate::provider_usage`], which reads the local database and the engine's
/// bundled pricing table and nothing else.
#[tauri::command]
pub fn get_provider_usage(
    app: tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<ProviderUsageSummary> {
    let now = scan::unix_now();
    let offset = utc_offset_minutes.unwrap_or(0);
    let since = provider_usage::lookback_start(now, offset);
    let evidence = app.state::<Store>().usage_evidence(since).map_err(fail)?;
    Ok(provider_usage::summarize(&evidence, now, offset))
}

/// How fresh a reading the refresh command asks each source's cooldown for.
///
/// This command is called from the popover, which polls it roughly once a
/// minute for as long as the popover stays visible — see
/// `scan::TICK` and `popover::note_shown`. Fifty seconds sits just under that
/// polling interval, so an open popover's own ordinary polling is what keeps
/// the reading current: every visible tick is close enough to the cooldown's
/// edge to trigger a real fetch, without this command itself running a timer
/// or a background task. The aggressive freshness is bounded by someone
/// actually looking — once the popover closes, nothing here keeps polling on
/// its behalf, and the background monitor's own, much longer, `max_age`
/// takes back over (see `usage_alerts::BACKGROUND_MAX_AGE`).
const POPOVER_LIVE_USAGE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(50);

/// Return the last provider limit snapshot without reading a provider.
///
/// This remains separate from [`get_provider_usage`]. That payload carries no
/// percentage, allowance, or reset anywhere, and a test proves it by
/// serializing the whole thing. Keeping the two apart means a limit surface
/// can exist without weakening the estimate surface's contract, and the views
/// layer them.
///
/// The live-usage setting gates the cached value too. Turning the feature off
/// removes its figures immediately without waiting for another refresh.
#[tauri::command]
pub fn get_live_usage(
    app: tauri::AppHandle,
    _utc_offset_minutes: Option<i32>,
) -> CommandResult<LiveUsageSummary> {
    let settings = app
        .try_state::<Store>()
        .and_then(|store| store.settings().ok());
    let active = settings
        .as_ref()
        .is_some_and(|settings| settings.live_usage_active());
    let live = app.try_state::<crate::usage_alerts::LiveUsage>();
    if !active {
        // No readings, but keep the roster: Settings shows one switch for each
        // provider antiburn can meter, and the master switch does not remove
        // them. A roster is a list of capabilities, not a reading.
        let hidden = settings
            .map(|settings| settings.live_usage_hidden_providers)
            .unwrap_or_default();
        return Ok(LiveUsageSummary {
            meters: live
                .map(|live| provider_usage::live::roster(&live.sources, &hidden))
                .unwrap_or_default(),
            ..LiveUsageSummary::default()
        });
    }
    Ok(live.map(|live| live.snapshot()).unwrap_or_default())
}

/// Refresh the provider's own limit figures and publish the new snapshot.
///
/// An empty summary is the ordinary answer: no source has anything to say.
/// Sources that fail report separately, so absence and failure stay distinct.
///
/// `utc_offset_minutes` travels for one reason only: "used today" is a claim
/// about the reader's calendar day. The windows themselves are the provider's
/// own boundaries, stated as absolute instants, and owe nothing to it.
///
/// `async`, and every byte of the work handed to a blocking thread, for one
/// reason: a synchronous `#[tauri::command]` is run inline on the thread that
/// delivered the IPC message, and the summary this returns reaches a provider
/// over the network with `reqwest::blocking` — see
/// `provider_usage::live::sources::http::client`. A provider that accepts a
/// connection and then says nothing would hold that thread for the full
/// fifteen-second timeout, once per source, and the popover polls this about
/// once a minute while it is open. The reader would watch the whole app —
/// tray, popover, every window — stop answering. Nothing here needs the main
/// thread, so nothing here stays on it.
#[tauri::command]
pub async fn refresh_live_usage(
    app: tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<LiveUsageSummary> {
    // The sources deliberately expose a synchronous interface and include
    // blocking HTTP, Keychain, and subprocess work. The blocking pool is the
    // boundary for all of it.
    let utc_offset_minutes = utc_offset_minutes.unwrap_or(0);
    tauri::async_runtime::spawn_blocking(move || {
        // Read here rather than before the hop: this summary is stamped with
        // the moment it was produced, and a caller queued behind another
        // summarization can wait a while for its turn.
        let now = scan::unix_now();
        let Some(live) = app.try_state::<crate::usage_alerts::LiveUsage>() else {
            return LiveUsageSummary {
                generated_at: crate::store::iso_from_epoch(Some(now)),
                ..LiveUsageSummary::default()
            };
        };
        let store = app.try_state::<Store>();
        // Held for the whole pass: two of these can now genuinely overlap,
        // and the reading history they append to is not written atomically.
        let _summarizing = live.summarizing();
        live.set_utc_offset_minutes(utc_offset_minutes, store.as_deref());
        let summary = provider_usage::live::summarize(
            &live.sources,
            store.as_deref(),
            now,
            utc_offset_minutes,
            POPOVER_LIVE_USAGE_MAX_AGE,
        );
        live.replace_snapshot(summary.clone(), store.as_deref());
        let _ = app.emit(crate::usage_alerts::EVENT_CHANGED, &summary);
        summary
    })
    .await
    .map_err(fail)
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
) -> CommandResult<SessionAnalysis> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    let key = SessionKey::for_session(&agent, &session_id, wsl_distro.as_deref());
    let store = app.state::<Store>();

    // Rows are the only way this command computes an analysis: every agent
    // is in the evidence cohort, so this always serves the worker's last
    // published pass. A missing or not-yet-published row set reports a
    // pending payload instead of re-parsing the transcript in-process, and
    // nudges the worker so the gap closes on its own. Publishing a fresh
    // pass is the worker's job — see its announce callback in
    // `insights_worker::spawn` — so this command never caches one itself or
    // emits `SESSION_ENTRY_CHANGED_EVENT` for it.
    let (analysis, analysis_pending) =
        match analysis::analysis_from_rows(&store, &key, &session_id, &agent) {
            Some(replayed) => {
                nudge_if_evidence_stale(
                    &app,
                    &store,
                    kind,
                    &key,
                    &session_id,
                    wsl_distro.as_deref(),
                )
                .await;
                (replayed, false)
            }
            None => {
                requeue_and_wake_worker(&app, &store, &key);
                (analysis::SessionAnalysis::unavailable(), true)
            }
        };
    let relations = resolve_lineage(&app, kind, &key, wsl_distro.as_deref()).await;

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
        is_active: analysis::is_active(
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
        source_path: analysis.source_path.clone(),
        analysis_pending,
    })
}

/// The cheap fingerprint of one session's analysis inputs.
///
/// The session-detail popover polls this while it is open, and re-runs the
/// full analysis only when the value changes. This command reads file
/// metadata alone, never a transcript, so a poll costs almost nothing.
#[tauri::command]
pub async fn get_session_analysis_fingerprint(
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
) -> CommandResult<String> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    let Some(source) = analysis::locate(kind, &session_id, wsl_distro.as_deref()).await else {
        return Ok(analysis::MISSING_FINGERPRINT.to_string());
    };
    Ok(
        analysis::fingerprint_with_subagents(kind, &session_id, wsl_distro.as_deref(), &source)
            .await,
    )
}

/// One sub-agent's own analysis, opened from the roster.
#[tauri::command]
pub async fn get_subagent_analysis(
    app: tauri::AppHandle,
    agent: String,
    parent_session_id: String,
    subagent_id: String,
    wsl_distro: Option<String>,
) -> CommandResult<SessionAnalysis> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    // Rows are the only way this command computes an analysis — see the
    // matching comment in `get_session_analysis`. A missing or not-yet-
    // published row set reports a pending payload and nudges the worker,
    // instead of re-parsing the sub-agent's own transcript in-process.
    let store = app.state::<Store>();
    let parent_key = SessionKey::for_session(&agent, &parent_session_id, wsl_distro.as_deref());
    let (analysis, analysis_pending) = match analysis::subagent_analysis_from_rows(
        &store,
        &parent_key,
        &parent_session_id,
        &subagent_id,
        &agent,
    ) {
        Some(replayed) => (replayed, false),
        None => {
            requeue_and_wake_worker(&app, &store, &parent_key);
            (analysis::SessionAnalysis::unavailable(), true)
        }
    };
    Ok(SessionAnalysis {
        summary: analysis.summary.clone(),
        supports_analysis: analysis::analysis_supported(kind),
        title: None,
        wsl_distro,
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
        source_path: analysis.source_path.clone(),
        analysis_pending,
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

/// Resolve and persist one session's fork lineage.
async fn resolve_lineage(
    app: &tauri::AppHandle,
    kind: AgentKind,
    key: &SessionKey,
    wsl_distro: Option<&str>,
) -> SessionRelations {
    let store = app.state::<Store>();
    let parent_id = match analysis::locate(kind, &key.session_id, wsl_distro).await {
        Some(source) => analysis::fork_parent(&source).await,
        None => None,
    };

    if let Some(parent_id) = &parent_id {
        let _ = store.replace_relations(
            key,
            RelationKind::ForkParent,
            &[RelationRecord {
                kind: RelationKind::ForkParent,
                related_id: parent_id.clone(),
                label: None,
            }],
        );
    }

    let mut relations = SessionRelations {
        title: store
            .session(key)
            .ok()
            .flatten()
            .and_then(|record| record.title),
        ..SessionRelations::default()
    };

    if let Some(parent_id) = parent_id {
        let available = analysis::locate(kind, &parent_id, wsl_distro)
            .await
            .is_some();
        let title = store
            .session(&SessionKey::new(
                &key.environment_key,
                &key.agent,
                &parent_id,
            ))
            .ok()
            .flatten()
            .and_then(|record| record.title);
        relations.parent = Some(SessionRelation {
            identity: SessionIdentity {
                agent: key.agent.clone(),
                session_id: parent_id,
                wsl_distro: wsl_distro.map(str::to_string),
            },
            title,
            available,
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
            },
            title: record.as_ref().and_then(|record| record.title.clone()),
            // A child we still have a row for is on this machine; the row is
            // pruned when the transcript ages out.
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
    Ok(scan::run_pass(&app, activity_window_days).await)
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
pub fn get_scan_status(app: tauri::AppHandle) -> ScanStatus {
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
    status
}

/* -------------------------------------------------------------------------
 * Insights
 * ---------------------------------------------------------------------- */

/// Days of history the insights report covers.
const INSIGHTS_WINDOW_DAYS: i64 = 30;

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
    // native scope is total, so nothing is excluded there. Per-environment
    // reports for Windows hosts with WSL sessions are a recorded
    // follow-up in docs/plans/local-insights-followups.md.
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

/// The thirty-day insights report for this machine's native environment.
///
/// Concurrent calls share one reduction (see [`InsightsController`]);
/// none of them cancels a running one. Cancellation is only the explicit
/// [`cancel_insights_report`] signal.
#[tauri::command]
pub async fn get_insights_report(app: tauri::AppHandle) -> CommandResult<InsightsReportPayload> {
    // Opening the Insights pane asks for a scan pass now instead of
    // waiting out a tick. This is a further call site of the shipped
    // on-demand trigger — the same kick the popover and the other
    // commands fire — not a new trigger class and not queue reordering.
    app.state::<ScanController>().request();
    let data_dir = app.state::<Store>().state_dir().to_path_buf();
    let request = insights_report_request(epoch_now());
    let report = app
        .state::<InsightsController>()
        .report(data_dir, request)
        .await?;
    crate::analytics::record_unrecognized_records(&app, &report.unrecognized_records);
    Ok(report.into())
}

/// Report calculation state plus the evidence backlog for the report's scope.
#[tauri::command]
pub fn get_insights_status(app: tauri::AppHandle) -> CommandResult<InsightsStatusPayload> {
    let calculating = app.state::<InsightsController>().is_calculating();
    let backlog = app
        .state::<Store>()
        .evidence_backlog_counts(&environment_key(None))
        .map_err(fail)?;
    Ok(InsightsStatusPayload {
        calculating,
        pending: backlog.pending,
        processing: backlog.processing,
    })
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
                SessionKey::for_session(
                    &session.agent,
                    &session.session_id,
                    session.wsl_distro.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        let store = app.state::<Store>();
        let rows = store.evidence_batch(&keys).map_err(fail)?;
        let source_generations = store.source_generation_batch(&keys).map_err(fail)?;
        Ok(session_hygiene_payloads(rows, source_generations))
    })
    .await
    .map_err(fail)?
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

fn session_hygiene_payload(
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
                return SessionHygienePayload::not_assessed(
                    "stale",
                    NotAssessedReason::IncompleteEvidence,
                );
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
            SessionHygienePayload::for_evidence(
                session_badges(&evidence, &ReportCatalogs::default()),
                &evidence,
                evidence_state,
            )
        }
        crate::store::EvidenceStatus::Unsupported => {
            SessionHygienePayload::not_assessed("unsupported", NotAssessedReason::CapabilityMissing)
        }
        status => SessionHygienePayload::not_assessed(
            status.as_str(),
            NotAssessedReason::IncompleteEvidence,
        ),
    }
}

/// Stop the running report reduction, when one runs.
///
/// The pane fires this when it closes; shutdown fires it too. The
/// reduction is read-only, so a cancelled run leaves the durable
/// evidence state untouched.
#[tauri::command]
pub fn cancel_insights_report(app: tauri::AppHandle) {
    app.state::<InsightsController>().cancel();
}

/* -------------------------------------------------------------------------
 * Sources
 * ---------------------------------------------------------------------- */

/// Every repository antiburn knows about on this machine.
#[tauri::command]
pub fn list_repositories(app: tauri::AppHandle) -> CommandResult<Vec<RepositoryItem>> {
    repositories::list(&app.state::<Store>()).map_err(fail)
}

/// Include or ignore one repository.
#[tauri::command]
pub async fn set_repository_enabled(
    app: tauri::AppHandle,
    key: String,
    enabled: bool,
) -> CommandResult<Vec<RepositoryItem>> {
    {
        let store = app.state::<Store>();
        repositories::set_enabled(&store, &key, enabled)
            .await
            .map_err(fail)?;
    }
    // Disabling purges the repository's rows; the open popover re-reads its
    // list on this event rather than waiting for a scan. Re-enabling asks for
    // a pass so the rows come back without the reader doing anything.
    let _ = app.emit(SESSIONS_INVALIDATED_EVENT, ());
    if enabled {
        app.state::<ScanController>().request();
    }
    list_repositories(app)
}

/// Event the shell emits when stored sessions were removed outside a scan
/// (repository opt-out, index clearing). The popover re-queries on it.
pub const SESSIONS_INVALIDATED_EVENT: &str = "sessions:invalidated";

/// Event the shell emits when one session's cached analysis changes outside a
/// scan. The payload is the fresh [`ActivityEntry`] for that session, so the
/// popover can update the one row without a re-query.
pub const SESSION_ENTRY_CHANGED_EVENT: &str = "sessions:entry-changed";

/// Re-derive the repository list from what is on disk right now.
#[tauri::command]
pub async fn refresh_repositories(app: tauri::AppHandle) -> CommandResult<Vec<RepositoryItem>> {
    repositories::refresh(&app).await.map_err(fail)?;
    list_repositories(app)
}

/// The extra directories the reader pointed the scanner at.
#[tauri::command]
pub fn list_scan_roots(app: tauri::AppHandle) -> CommandResult<Vec<String>> {
    app.state::<Store>().scan_roots().map_err(fail)
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
    let roots = {
        let store = app.state::<Store>();
        store.add_scan_root(&path).map_err(fail)?;
        store.scan_roots().map_err(fail)?
    };
    mirror_scan_roots(&app, &roots).await.map_err(fail)?;
    app.state::<ScanController>().request();
    Ok(roots)
}

/// Stop scanning a directory, and mirror the list into the engine's own store.
#[tauri::command]
pub async fn remove_scan_root(app: tauri::AppHandle, path: String) -> CommandResult<Vec<String>> {
    let roots = {
        let store = app.state::<Store>();
        store.remove_scan_root(&path).map_err(fail)?;
        store.scan_roots().map_err(fail)?
    };
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

/// Write one session's derived analysis to `dest_path` as JSON.
///
/// The transcript is **not** copied: the document carries a reference to where
/// it lives instead. It can still describe real work — titles, paths,
/// repository names — which is why the caller confirms before choosing a
/// destination.
#[tauri::command]
pub async fn export_session(
    app: tauri::AppHandle,
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
    dest_path: String,
) -> CommandResult<String> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    let key = SessionKey::for_session(&agent, &session_id, wsl_distro.as_deref());
    let store = app.state::<Store>();
    let claimed = store
        .session_source_state(&key)
        .ok()
        .flatten()
        .map(|state| analysis::ClaimedSource {
            fingerprint: state.source_fingerprint,
            generation: state.source_generation,
        })
        .unwrap_or(analysis::ClaimedSource {
            fingerprint: None,
            generation: 0,
        });
    let analysis = analysis::analyze(
        kind,
        &session_id,
        wsl_distro.as_deref(),
        claimed,
        analysis::CancelFlag::never(),
    )
    .await;

    let stored = store.session(&key).ok().flatten();

    let document = SessionExport::new(
        app.package_info().version.to_string(),
        ExportedSession {
            agent,
            session_id,
            wsl_distro,
            title: stored.as_ref().and_then(|record| record.title.clone()),
            cwd: stored.as_ref().and_then(|record| record.cwd.clone()),
            surface: stored
                .as_ref()
                .map(|record| record.surface.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            last_activity: stored
                .as_ref()
                .and_then(|record| record.updated_at_epoch)
                .map(|epoch| iso_from_epoch(Some(epoch))),
            source_path: analysis.source_path.clone(),
        },
        &analysis,
    );

    let json = document.to_json().map_err(fail)?;
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
pub fn delete_session_data(
    app: tauri::AppHandle,
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
) -> CommandResult<bool> {
    let key = SessionKey::for_session(&agent, &session_id, wsl_distro.as_deref());
    app.state::<Store>().delete_session(&key).map_err(fail)
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
pub fn clear_local_index(app: tauri::AppHandle) -> CommandResult<usize> {
    let removed = app
        .state::<Store>()
        .clear_local_session_data()
        .map_err(fail)?;
    // The index is empty and the popover is showing it. Refill it rather than
    // leaving a reader looking at an empty list until the next tick.
    app.state::<ScanController>().request();
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
pub fn get_folder_permissions(app: tauri::AppHandle) -> CommandResult<FolderPermissions> {
    let store = app.state::<Store>();
    let mut granted: Vec<String> = store.granted_dirs().map_err(fail)?.into_iter().collect();
    granted.sort();
    Ok(FolderPermissions {
        deferred: store.deferred_permission_dirs().map_err(fail)?,
        granted,
        supported: !protected::protected_dir_names().is_empty(),
    })
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

    let (outcome, recorded) = {
        let store = app.state::<Store>();
        let consent = consent::StoreConsentGrants::new(&store);
        let outcome = consent.probe_and_record(&home.join(&dir)).await;
        let recorded = match outcome {
            consent::ProbeOutcome::Granted { .. } => consent.grant(&dir),
            _ => Ok(()),
        };
        (outcome, recorded)
    };

    // Released before anything can fail. An early `?` between the two would
    // leave the hold in place for the rest of the run, and the popover would
    // stop dismissing on focus loss with nothing on screen to explain why.
    popover::end_focus_hold(&app);
    recorded.map_err(fail)?;

    if matches!(outcome, consent::ProbeOutcome::Granted { .. }) {
        app.state::<ScanController>().request();
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
    let deferred: HashSet<String> = app
        .state::<Store>()
        .deferred_permission_dirs()
        .map_err(fail)?
        .into_iter()
        .map(|entry| entry.dir)
        .collect();
    if deferred.is_empty() {
        return Ok(Vec::new());
    }

    let discovered = {
        let store = app.state::<Store>();
        let consent = consent::StoreConsentGrants::new(&store);
        consent.discover_external_grants(&deferred).await
    };

    if !discovered.is_empty() {
        app.state::<ScanController>().request();
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
pub fn reveal_source(app: tauri::AppHandle, path: String) -> CommandResult<()> {
    let target = revealable_path(&path)?;
    app.opener().reveal_item_in_dir(target).map_err(fail)
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
mod tests {
    use std::cell::RefCell;

    use super::*;

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
    fn the_catalog_version_comes_from_the_engine_and_is_a_review_date() {
        let version = engine_catalog_version();
        assert_eq!(version, antiburn_local::pricing::PRICING_CATALOG_VERSION);
        assert_eq!(version.len(), 10, "expected a YYYY-MM-DD review date");
        assert!(
            version
                .split('-')
                .all(|part| part.chars().all(|c| c.is_ascii_digit()))
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
    fn matching_fingerprints_are_not_stale() {
        assert!(!evidence_is_stale(Some("sv1:a"), "sv1:a"));
    }

    #[test]
    fn a_changed_fingerprint_is_stale() {
        assert!(evidence_is_stale(Some("sv1:a"), "sv1:b"));
    }

    #[test]
    fn no_stored_fingerprint_is_stale() {
        assert!(evidence_is_stale(None, "sv1:a"));
    }

    #[test]
    fn a_relative_path_never_reaches_the_platform_opener() {
        for path in [
            "",
            "relative/session.jsonl",
            "./session.jsonl",
            "../../etc/passwd",
        ] {
            let error = revealable_path(path).expect_err("must be rejected");
            assert!(
                error.contains("absolute"),
                "{path:?} should be refused for not being absolute, got {error:?}"
            );
        }
    }

    #[test]
    fn a_path_that_is_not_on_this_machine_is_refused_rather_than_forwarded() {
        let absent = if cfg!(windows) {
            r"C:\antiburn\does\not\exist\session.jsonl"
        } else {
            "/antiburn/does/not/exist/session.jsonl"
        };
        let error = revealable_path(absent).expect_err("must be rejected");
        assert!(error.contains("not on this machine"), "got {error:?}");
    }

    #[test]
    fn a_real_file_resolves_to_a_canonical_path() {
        let directory = tempfile::TempDir::new().unwrap();
        let file = directory.path().join("session.jsonl");
        std::fs::write(&file, "{}\n").unwrap();

        let resolved = revealable_path(&file.to_string_lossy()).expect("a real file resolves");
        assert!(resolved.is_absolute());
        assert!(resolved.exists());
        assert_eq!(resolved.file_name(), file.file_name());
        // Nothing extended-length reaches the opener, on any platform.
        assert!(!resolved.to_string_lossy().starts_with(r"\\?\"));

        // The data folder is revealed the same way, so directories resolve too.
        let folder = revealable_path(&directory.path().to_string_lossy()).unwrap();
        assert!(folder.is_dir());
    }

    #[test]
    fn a_traversal_dressed_up_as_an_absolute_path_is_resolved_before_it_is_used() {
        let directory = tempfile::TempDir::new().unwrap();
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let file = directory.path().join("session.jsonl");
        std::fs::write(&file, "{}\n").unwrap();

        let sneaky = nested.join("..").join("session.jsonl");
        let resolved = revealable_path(&sneaky.to_string_lossy()).unwrap();
        assert_eq!(
            resolved,
            revealable_path(&file.to_string_lossy()).unwrap(),
            "the opener sees the resolved path, never the one that was typed"
        );
    }

    fn evidence_row(
        status: crate::store::EvidenceStatus,
        evidence: Option<SessionEvidence>,
    ) -> crate::store::EvidenceRow {
        crate::store::EvidenceRow {
            key: SessionKey::new("native", "claude-code", "synthetic-hygiene"),
            status,
            analyzed_generation: Some(1),
            processed_fingerprint: Some("synthetic-fingerprint".to_owned()),
            parser_revision: Some(PARSER_REVISION),
            analyzer_revision: Some(ANALYZER_REVISION),
            evidence_schema_revision: Some(EVIDENCE_SCHEMA_REVISION),
            evidence_json: evidence
                .map(|value| serde_json::to_string(&value).expect("synthetic evidence serializes")),
            diagnostics_json: None,
            retry_count: 0,
            claim_fence: 0,
            claimed_at_epoch: None,
            lease_expires_at_epoch: None,
            next_attempt_at_epoch: None,
            analyzed_at_epoch: Some(1),
            last_error: None,
        }
    }

    fn synthetic_evidence_accumulator() -> antiburn_local::analysis::SessionEvidenceAccumulator {
        antiburn_local::analysis::SessionEvidenceAccumulator::new(
            antiburn_local::analysis::EvidenceSource {
                agent: "claude-code".to_owned(),
                session_id: "synthetic-hygiene".to_owned(),
                kind: antiburn_local::analysis::SourceKind::File,
                capabilities: antiburn_local::analysis::SourceCapabilities::claude(),
            },
        )
    }

    fn synthetic_evidence() -> SessionEvidence {
        synthetic_evidence_accumulator().evidence(&antiburn_local::analysis::TurnFacts::default())
    }

    // The generation `evidence_row` stamps as `analyzed_generation`. Tests
    // that are not exercising a generation mismatch pass this back as the
    // session's current source generation, so the row reads as current.
    const SYNTHETIC_GENERATION: Option<i64> = Some(1);

    #[test]
    fn session_hygiene_preserves_queue_states_without_a_false_clean_result() {
        let missing = session_hygiene_payload(None, SYNTHETIC_GENERATION);
        assert_eq!(missing.evidence_state, "pending");
        assert!(
            missing
                .badges
                .iter()
                .all(|badge| matches!(badge.status, crate::dto::SessionHygieneStatus::NotAssessed))
        );

        let processing = session_hygiene_payload(
            Some(evidence_row(crate::store::EvidenceStatus::Processing, None)),
            SYNTHETIC_GENERATION,
        );
        assert_eq!(processing.evidence_state, "processing");
    }

    #[test]
    fn session_hygiene_marks_old_ready_evidence_as_stale() {
        let mut row = evidence_row(
            crate::store::EvidenceStatus::Ready,
            Some(synthetic_evidence()),
        );
        row.parser_revision = Some(PARSER_REVISION - 1);

        let payload = session_hygiene_payload(Some(row), SYNTHETIC_GENERATION);
        assert_eq!(payload.evidence_state, "stale");
        assert!(
            payload
                .badges
                .iter()
                .all(|badge| matches!(badge.status, crate::dto::SessionHygieneStatus::NotAssessed))
        );
    }

    #[test]
    fn session_hygiene_never_serves_a_requeued_rows_leftover_evidence() {
        // `reconcile_evidence_revisions` flips a stale Ready row's status to
        // Pending but keeps its old `evidence_json` by design (see
        // `store/mod.rs`). This row copies that shape: a non-Ready status
        // next to fully current evidence from a previous pass.
        let mut row = evidence_row(
            crate::store::EvidenceStatus::Pending,
            Some(synthetic_evidence()),
        );
        row.retry_count = 0;

        let payload = session_hygiene_payload(Some(row), SYNTHETIC_GENERATION);
        assert_eq!(payload.evidence_state, "pending");
        assert!(
            payload
                .badges
                .iter()
                .all(|badge| matches!(badge.status, crate::dto::SessionHygieneStatus::NotAssessed)),
            "leftover evidence_json on a requeued row must never surface a Clean or Finding badge"
        );
    }

    #[test]
    fn session_hygiene_marks_evidence_from_an_earlier_source_generation_as_stale() {
        // The source grew a new generation (a requeue not yet run, or still
        // pending) while this row's evidence is still Ready and carries
        // current revisions from the previous generation.
        let row = evidence_row(
            crate::store::EvidenceStatus::Ready,
            Some(synthetic_evidence()),
        );
        assert_eq!(row.analyzed_generation, SYNTHETIC_GENERATION);
        let newer_source_generation = Some(2);

        let payload = session_hygiene_payload(Some(row), newer_source_generation);
        assert_eq!(payload.evidence_state, "stale");
        assert!(
            payload
                .badges
                .iter()
                .all(|badge| matches!(badge.status, crate::dto::SessionHygieneStatus::NotAssessed)),
            "evidence analyzed against a superseded source generation must never surface a Clean or Finding badge"
        );
    }

    #[test]
    fn session_hygiene_batches_preserve_order_and_isolate_invalid_rows() {
        let mut invalid = evidence_row(crate::store::EvidenceStatus::Ready, None);
        invalid.evidence_json = Some("{".to_owned());
        let payloads = session_hygiene_payloads(
            vec![
                None,
                Some(invalid),
                Some(evidence_row(
                    crate::store::EvidenceStatus::Ready,
                    Some(synthetic_evidence()),
                )),
            ],
            vec![
                SYNTHETIC_GENERATION,
                SYNTHETIC_GENERATION,
                SYNTHETIC_GENERATION,
            ],
        );

        assert_eq!(payloads.len(), 3);
        assert_eq!(payloads[0].evidence_state, "pending");
        assert_eq!(payloads[1].evidence_state, "failed");
        assert_eq!(payloads[2].evidence_state, "ready");
    }

    #[test]
    fn session_hygiene_marks_an_accepted_prefix_as_still_growing() {
        let mut accumulator = synthetic_evidence_accumulator();
        accumulator.observe_source_outcome(
            antiburn_local::analysis::VisitOutcome::AcceptedPrefix { boundary: 1 },
        );
        let evidence = accumulator.evidence(&antiburn_local::analysis::TurnFacts::default());
        assert!(matches!(
            evidence.coverage,
            antiburn_local::analysis::EvidenceCoverage::Partial(
                antiburn_local::analysis::CoverageReason::PinnedPrefix
            )
        ));
        let row = evidence_row(crate::store::EvidenceStatus::Ready, Some(evidence));

        let payload = session_hygiene_payload(Some(row), SYNTHETIC_GENERATION);
        assert_eq!(payload.evidence_state, "activelyGrowing");
        assert!(payload.badges.iter().all(|badge| {
            // Model Overthinking / Fast Mode Overuse report a missing
            // signal, because the synthetic evidence carries zero
            // eligible turns. Every other badge — Obsolete Model
            // included, since the reviewed production registry is
            // non-empty and its own rule falls through to the
            // session-wide coverage check — reports the session-wide
            // partial coverage from the accepted-prefix outcome.
            let expected_reason = match badge.id {
                "modelOverthinking" | "fastModeOveruse" => "signalMissing",
                _ => "incompleteEvidence",
            };
            matches!(badge.status, crate::dto::SessionHygieneStatus::NotAssessed)
                && badge.not_assessed_reason == Some(expected_reason)
        }));
    }

    #[test]
    fn the_default_scan_roots_are_absolute_and_under_the_home_directory() {
        let Some(home) = antiburn_local::paths::home_dir() else {
            return;
        };
        for root in default_scan_roots() {
            assert!(
                Path::new(&root).starts_with(&home),
                "{root} should sit under {}",
                home.display()
            );
        }
    }
}
