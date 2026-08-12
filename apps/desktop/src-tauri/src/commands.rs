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

use std::path::{Path, PathBuf};

use antiburn_local::model::AgentKind;
use antiburn_local::paths::scan_roots as engine_scan_roots;
use antiburn_local::repositories::platform::{PlatformDiscovery as _, platform};
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

use crate::agents::kind_from_slug;
use crate::analytics;
use crate::dto::{
    ActivityEntry, AgentScanState, AppInfo, OrchestrationStatus, RepositoryItem, ScanStatus,
    SessionAnalytics, SessionIdentity, SessionRelation, SessionRelations, SubagentMember,
};
use crate::export::{ExportedSession, SessionExport};
use crate::repositories;
use crate::scan::{self, ScanController};
use crate::settings;
use crate::store::{
    AppSettings, RelationKind, RelationRecord, RepositoryRecord, SessionKey, SessionRecord, Store,
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
#[tauri::command]
pub fn open_settings_window(app: tauri::AppHandle) -> CommandResult<()> {
    settings::open(&app).map_err(fail)
}

/// Where the app came from and what it is running against.
#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> CommandResult<AppInfo> {
    let store = app.state::<Store>();
    Ok(AppInfo {
        app_version: app.package_info().version.to_string(),
        pricing_catalog_version: antiburn_local::pricing::PRICING_CATALOG_VERSION.to_string(),
        schema_version: store.schema_version().map_err(fail)?,
        data_dir: store.state_dir().to_string_lossy().to_string(),
        // The updater plugin is registered in release builds only, so a
        // development run says so rather than pretending a check happened.
        updates_supported: !cfg!(debug_assertions),
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

/// Replace every preference, returning what was actually stored.
///
/// `launchAtLogin` is recorded but not enforced: registering a login item needs
/// the autostart plugin, which this build does not carry. The settings pane
/// says so next to the control rather than silently doing nothing.
#[tauri::command]
pub fn set_settings(app: tauri::AppHandle, settings: AppSettings) -> CommandResult<AppSettings> {
    let store = app.state::<Store>();
    let previous = store.settings().map_err(fail)?;
    let saved = store.save_settings(&settings).map_err(fail)?;

    // Finishing onboarding, or widening the window past what the store holds,
    // both want fresh data immediately rather than at the next tick.
    let wants_scan = (!previous.onboarding_completed && saved.onboarding_completed)
        || saved.activity_window_days > previous.activity_window_days;
    if wants_scan {
        app.state::<ScanController>().request();
    }
    Ok(saved)
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

/// An epoch stamp as the ISO-8601 string the activity list parses.
fn iso_from_epoch(epoch: Option<i64>) -> String {
    let epoch = epoch.unwrap_or(0);
    time::OffsetDateTime::from_unix_timestamp(epoch)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
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
#[tauri::command]
pub async fn scan_now(app: tauri::AppHandle) -> CommandResult<ScanStatus> {
    Ok(scan::run_pass(&app).await)
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
    list_repositories(app)
}

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

/// Reveal a transcript in the platform's file manager.
#[tauri::command]
pub fn reveal_source(app: tauri::AppHandle, path: String) -> CommandResult<()> {
    app.opener().reveal_item_in_dir(path).map_err(fail)
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
        // A session with no heartbeat still yields a parseable stamp rather
        // than an empty string the list would drop.
        assert_eq!(iso_from_epoch(None), "1970-01-01T00:00:00Z");
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
