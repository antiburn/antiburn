// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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

use antiburn_local::model::AgentKind;
use antiburn_local::paths::scan_roots as engine_scan_roots;
use antiburn_local::paths::{home_dir, protected};
use antiburn_local::repositories as repositories_engine;
use antiburn_local::repositories::ConsentGrants as _;
use antiburn_local::repositories::platform::{PlatformDiscovery as _, platform};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::agents::kind_from_slug;
use crate::analytics;
use crate::consent;
use crate::dto::{
    ActivityEntry, AgentScanState, AppInfo, DeferredPermissionDir, LiveUsageSummary,
    OrchestrationStatus, ProviderUsageSummary, RepositoryItem, ScanStatus, SessionAnalytics,
    SessionIdentity, SessionRelation, SessionRelations, SubagentMember,
};
use crate::export::{ExportedSession, SessionExport};
use crate::popover;
use crate::provider_usage;
use crate::repositories;
use crate::scan::{self, ScanController};
use crate::settings;
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

/// Where the app came from and what it is running against.
#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> CommandResult<AppInfo> {
    let store = app.state::<Store>();
    Ok(AppInfo {
        app_version: app.package_info().version.to_string(),
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
    })
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
    Ok(saved)
}

fn apply_settings_transition(app: &tauri::AppHandle, previous: &AppSettings, saved: &AppSettings) {
    // The one transition that means the first run is over. Named rather than
    // inlined because two things now hang off it, and because it is the app's
    // only "exactly once, ever" event: nothing writes this flag back to false,
    // so neither consequence below needs a marker of its own to avoid repeating.
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

fn activity_entry(
    store: &Store,
    repositories: &[RepositoryRecord],
    session: SessionRecord,
    now: i64,
) -> anyhow::Result<ActivityEntry> {
    let analysis = store.analysis(&session.key)?;
    let (cost, models) = analysis
        .as_ref()
        .map(|record| analytics::price_cached_breakdown(&record.model_breakdown_json))
        .unwrap_or((None, Vec::new()));

    Ok(ActivityEntry {
        agent: session.key.agent.clone(),
        session_id: session.key.session_id.clone(),
        repo: repository_label(repositories, session.cwd.as_deref()),
        timestamp: iso_from_epoch(session.updated_at_epoch),
        is_active: analytics::is_active(session.updated_at_epoch, now),
        surface: session.surface.clone(),
        wsl_distro: session.wsl_distro.clone(),
        title: session.title.clone(),
        has_fork_parent: session.fork_parent_session_id.is_some(),
        fork_child_count: store.fork_children(&session.key)?.len() as u32,
        subagent_count: session.subagent_count,
        cost,
        models,
        active_secs: analysis.as_ref().map(|record| record.active_secs as u64),
        duration_secs: analysis.as_ref().map(|record| record.duration_secs as u64),
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

/// How fresh a reading this command asks each source's cooldown for.
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
/// takes back over (see `usage_alerts::MILESTONE_MAX_AGE`).
const POPOVER_LIVE_USAGE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(50);

/// The provider's own limit figures, when a registered source can prove them.
///
/// Deliberately a second command rather than a field on
/// [`get_provider_usage`]: that payload's guarantee is that it carries no
/// percentage, allowance, or reset anywhere, and a test proves it by
/// serializing the whole thing. Keeping the two apart means a limit surface
/// can exist without weakening the estimate surface's contract, and the views
/// layer them.
///
/// An empty summary is the ordinary answer — no source with anything to say —
/// and is not an error. Sources that *failed* report separately, so "nothing
/// found" and "something broke" never look alike on screen.
/// `utc_offset_minutes` travels for one reason only: "used today" is a claim
/// about the reader's calendar day. The windows themselves are the provider's
/// own boundaries, stated as absolute instants, and owe nothing to it.
#[tauri::command]
pub async fn get_live_usage(
    app: tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<LiveUsageSummary> {
    // The sources deliberately expose a synchronous interface and include
    // blocking HTTP, Keychain, and subprocess work. An async command keeps the
    // IPC handler responsive; the blocking pool is the boundary for that work.
    tauri::async_runtime::spawn_blocking(move || {
        let now = scan::unix_now();
        let Some(live) = app.try_state::<crate::usage_alerts::LiveUsage>() else {
            return Ok(LiveUsageSummary {
                generated_at: crate::store::iso_from_epoch(Some(now)),
                ..LiveUsageSummary::default()
            });
        };
        Ok(provider_usage::live::summarize(
            &live.sources,
            app.try_state::<Store>().as_deref(),
            now,
            utc_offset_minutes.unwrap_or(0),
            POPOVER_LIVE_USAGE_MAX_AGE,
        ))
    })
    .await
    .map_err(fail)?
}

/* -------------------------------------------------------------------------
 * Session analytics
 * ---------------------------------------------------------------------- */

/// Everything the session-analytics surface renders for one session.
///
/// Returns a payload with no summary rather than an error when the transcript
/// is gone: a deleted conversation is an ordinary state, and the view says so.
#[tauri::command]
pub async fn get_session_analytics(
    app: tauri::AppHandle,
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
) -> CommandResult<SessionAnalytics> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    let key = SessionKey::for_session(&agent, &session_id, wsl_distro.as_deref());

    let analysis = analytics::analyze(kind, &session_id, wsl_distro.as_deref()).await;
    let relations = resolve_lineage(&app, kind, &key, wsl_distro.as_deref()).await;

    let store = app.state::<Store>();
    let stored = store.session(&key).ok().flatten();

    if let Some(record) = analysis.record(&key) {
        let _ = store.save_analysis(&record);
    }
    let orchestration = match &analysis.orchestration {
        Some(orchestration) => {
            let members: Vec<RelationRecord> = orchestration
                .members
                .iter()
                .map(|member| RelationRecord {
                    kind: RelationKind::Subagent,
                    related_id: member.subagent_id.clone(),
                    label: Some(member.label.clone()),
                })
                .collect();
            let _ = store.replace_relations(&key, RelationKind::Subagent, &members);
            Some(orchestration.clone())
        }
        // The listing came back empty. That is usually the truth, but it is
        // also what a momentarily unreadable transcript looks like, so a roster
        // the store already recorded is shown rather than silently dropped.
        None => cached_orchestration(&store, &key),
    };

    Ok(SessionAnalytics {
        summary: analysis.summary.clone(),
        supports_analytics: analytics::analytics_supported(kind),
        title: stored.as_ref().and_then(|record| record.title.clone()),
        wsl_distro,
        is_active: analytics::is_active(
            stored.as_ref().and_then(|record| record.updated_at_epoch),
            scan::unix_now(),
        ),
        cost: analysis.cost,
        models: analysis.models.clone(),
        skills: analysis.skills.clone(),
        orchestration,
        relations: (!relations.is_empty()).then_some(relations),
        source_path: analysis.source_path.clone(),
    })
}

/// One sub-agent's own analysis, for the roster and the spawn markers.
#[tauri::command]
pub async fn get_subagent_analytics(
    app: tauri::AppHandle,
    agent: String,
    parent_session_id: String,
    subagent_id: String,
    wsl_distro: Option<String>,
) -> CommandResult<SessionAnalytics> {
    let Some(kind) = kind_from_slug(&agent) else {
        return Err(format!("unknown agent {agent}"));
    };
    let _ = &app;
    let analysis = analytics::analyze_subagent(
        kind,
        &parent_session_id,
        &subagent_id,
        wsl_distro.as_deref(),
    )
    .await;
    Ok(SessionAnalytics {
        summary: analysis.summary.clone(),
        supports_analytics: analytics::analytics_supported(kind),
        title: None,
        wsl_distro,
        is_active: false,
        cost: analysis.cost,
        models: analysis.models.clone(),
        skills: analysis.skills.clone(),
        orchestration: None,
        relations: None,
        source_path: analysis.source_path.clone(),
    })
}

/// The sub-agent roster the store already recorded, rebuilt as an orchestration
/// status. Health scores and spawn positions are absent — those come from
/// analyzing the children, which is exactly what did not happen this time.
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
            pattern_score: 0,
            spawn_progress: None,
        })
        .collect();
    if members.is_empty() {
        return None;
    }
    Some(OrchestrationStatus {
        orchestrating: members.len() as u32 >= analytics::MIN_ORCHESTRATED_SUBAGENTS,
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
    let parent_id = match analytics::locate(kind, &key.session_id, wsl_distro).await {
        Some(source) => analytics::fork_parent(&source).await,
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
        let available = analytics::locate(kind, &parent_id, wsl_distro)
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
    let analysis = analytics::analyze(kind, &session_id, wsl_distro.as_deref()).await;

    let key = SessionKey::for_session(&agent, &session_id, wsl_distro.as_deref());
    let stored = app.state::<Store>().session(&key).ok().flatten();

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
    use super::*;

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
