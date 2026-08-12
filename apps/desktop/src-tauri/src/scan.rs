// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The background scan: what antiburn knows about this machine, refreshed.
//!
//! # Scan policy
//!
//! One pass asks every agent explorer for the sessions it can see, reads each
//! one's metadata, writes the result to the store, tops up derived analysis for
//! the newest sessions, and refreshes the repository list. Passes never overlap:
//! a request that arrives while one is running is dropped rather than queued,
//! because the next tick would produce the same answer.
//!
//! When a pass runs:
//!
//! - **At launch**, once, if onboarding is finished. A first-run install has no
//!   sources selected yet, so scanning before the flow completes would only
//!   spend disk on a window nobody can see.
//! - **Every [`TICK`] while the popover is open.** The popover *is* the view of
//!   this data; refreshing behind a closed popover would burn disk on a machine
//!   whose owner is not looking.
//! - **Paused entirely while the popover is hidden.** The scheduler keeps
//!   ticking (it is one timer) but does no work, so reopening the popover
//!   refreshes within a tick rather than after a cold start.
//! - **The moment the popover is opened**, so a reader never looks at a stale
//!   list while waiting out a tick.
//! - **On demand**, from the rescan control and after any change to the source
//!   selection.
//!
//! Every pass is bounded: discovery is windowed to the retention horizon, the
//! per-session metadata reads run at a fixed concurrency, and analysis is
//! capped at [`MAX_ANALYSES_PER_PASS`] sessions so one pass cannot grow with
//! the size of the machine. The scheduler is a single task whose handle the app
//! aborts on exit, so nothing outlives the process.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::discovery::{Explorers, SessionLog, SessionSource, session_log_metadata};
use antiburn_local::model::AgentKind;
use antiburn_local::paths::{home_dir, ignored_paths};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::analytics;
use crate::dto::ScanStatus;
use crate::repositories;
use crate::store::{SessionKey, SessionRecord, Store};

/// How often the scheduler wakes up.
pub const TICK: Duration = Duration::from_secs(60);

/// Days of session history the store retains, regardless of how narrow the
/// user's activity window is. Widening the window then has data to show
/// immediately instead of waiting for a scan.
pub const RETENTION_DAYS: i64 = 14;

/// How many session logs have their metadata read at once. Bounds open files
/// and blocking-pool pressure during a whole-machine pass.
const METADATA_CONCURRENCY: usize = 16;

/// Sessions analyzed per pass. Analysis reads whole transcripts, so a machine
/// with hundreds of recent sessions catches up over several passes rather than
/// spending one very long pass on all of them. Newest first, so the rows a
/// reader actually sees are the ones that fill in first.
const MAX_ANALYSES_PER_PASS: usize = 60;

/// Scope key for the engine's ignored-path store. The engine namespaces opt-outs
/// so one machine can hold several independent sets; this app keeps one.
pub const IGNORE_SCOPE: &str = "local";

/// Events the scan emits. The webview listens for these rather than polling.
pub const EVENT_STARTED: &str = "scan:started";
pub const EVENT_PROGRESS: &str = "scan:progress";
pub const EVENT_FINISHED: &str = "scan:finished";

/// The scheduler's shared state, registered as Tauri managed state.
#[derive(Default)]
pub struct ScanController {
    running: AtomicBool,
    popover_visible: AtomicBool,
    status: Mutex<ScanStatus>,
    kick: Notify,
}

impl ScanController {
    /// Ask for a pass as soon as the scheduler can start one.
    pub fn request(&self) {
        self.kick.notify_one();
    }

    /// Record whether the popover is on screen, which is what gates the timer.
    pub fn set_popover_visible(&self, visible: bool) {
        self.popover_visible.store(visible, Ordering::Relaxed);
    }

    fn popover_visible(&self) -> bool {
        self.popover_visible.load(Ordering::Relaxed)
    }

    /// The current or last pass.
    pub fn status(&self) -> ScanStatus {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn update(&self, mutate: impl FnOnce(&mut ScanStatus)) -> ScanStatus {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mutate(&mut status);
        status.clone()
    }
}

/// Start the scheduler. The returned handle is aborted when the app exits.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A fresh install has nothing to scan until the reader picks sources.
        if onboarding_completed(&app) {
            run_pass(&app).await;
        }
        loop {
            let controller = app.state::<ScanController>();
            tokio::select! {
                () = controller.kick.notified() => {}
                () = tokio::time::sleep(TICK) => {
                    if !controller.popover_visible() || !onboarding_completed(&app) {
                        continue;
                    }
                }
            }
            run_pass(&app).await;
        }
    })
}

fn onboarding_completed(app: &AppHandle) -> bool {
    app.state::<Store>()
        .settings()
        .map(|settings| settings.onboarding_completed)
        .unwrap_or(false)
}

/// Run one pass, unless one is already in flight.
pub async fn run_pass(app: &AppHandle) -> ScanStatus {
    {
        let controller = app.state::<ScanController>();
        if controller.running.swap(true, Ordering::SeqCst) {
            return controller.status();
        }
        let started = controller.update(|status| {
            status.running = true;
            status.completed_agents = 0;
            status.total_agents = AgentKind::ALL.len();
            status.sessions = 0;
            status.error = None;
        });
        let _ = app.emit(EVENT_STARTED, started);
    }

    let outcome = pass(app).await;

    let controller = app.state::<ScanController>();
    let finished = controller.update(|status| {
        status.running = false;
        status.finished_at = Some(crate::store::now_rfc3339());
        match &outcome {
            Ok(sessions) => {
                status.sessions = *sessions;
                status.completed_agents = status.total_agents;
                status.error = None;
            }
            Err(error) => status.error = Some(error.to_string()),
        }
    });
    controller.running.store(false, Ordering::SeqCst);
    let _ = app.emit(EVENT_FINISHED, finished.clone());
    finished
}

/// The body of one pass. Split out so [`run_pass`] owns only the in-flight
/// bookkeeping and the events.
async fn pass(app: &AppHandle) -> anyhow::Result<usize> {
    let store = app.state::<Store>();
    let settings = store.settings()?;
    let now = unix_now();
    let window_days = i64::from(settings.activity_window_days).max(RETENTION_DAYS);
    let since_secs = window_days * 86_400;

    let ignored = ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE);
    let home = home_dir().unwrap_or_default();

    let progress_app = app.clone();
    let logs = Explorers::DISK
        .discover_recent_sessions_with_progress(
            now,
            since_secs,
            move |agent, found, completed, total| {
                let controller = progress_app.state::<ScanController>();
                let status = controller.update(|status| {
                    status.completed_agents = completed;
                    status.total_agents = total;
                    status.sessions += found;
                });
                let _ = progress_app.emit(EVENT_PROGRESS, status);
                let _ = agent;
            },
        )
        .await;

    let records = describe(logs, &home, &ignored).await;
    store.upsert_sessions(&records)?;

    for (agent, seen, cursor) in per_agent_totals(&records) {
        store.record_agent_scan(&agent, cursor, seen)?;
    }

    store.prune_sessions_before(now - RETENTION_DAYS * 86_400)?;

    // Derived analysis for the newest sessions in the visible window, so the
    // list's cost and time pills are populated without opening every row.
    top_up_analysis(app, now, i64::from(settings.activity_window_days)).await?;

    repositories::refresh(app).await?;

    Ok(records.len())
}

/// Read metadata for every discovered log, at a bounded concurrency, and drop
/// the ones the reader opted out of.
async fn describe(
    logs: Vec<SessionLog>,
    home: &std::path::Path,
    ignored: &std::collections::HashSet<String>,
) -> Vec<SessionRecord> {
    let mut records = Vec::with_capacity(logs.len());
    for chunk in logs.chunks(METADATA_CONCURRENCY) {
        let mut set = JoinSet::new();
        for log in chunk {
            let log = log.clone();
            let home = home.to_path_buf();
            set.spawn(async move { describe_one(log, &home).await });
        }
        while let Some(joined) = set.join_next().await {
            if let Ok(Some(record)) = joined {
                let cwd = record.cwd.as_deref();
                // The engine's opt-out gate, applied once here so every surface
                // that reads the store inherits it.
                if cwd.is_some_and(|cwd| ignored_paths::set_contains(ignored, cwd)) {
                    continue;
                }
                records.push(record);
            }
        }
    }
    records
}

async fn describe_one(log: SessionLog, home: &std::path::Path) -> Option<SessionRecord> {
    let metadata = session_log_metadata(&log).await;
    let session_id = metadata
        .as_ref()
        .and_then(|metadata| metadata.session_id.clone())
        .or_else(|| recovered_id(&log))?;
    if session_id.is_empty() {
        return None;
    }

    // A dir listing per orchestrator-capable session; vendors that record no
    // orchestration return empty without touching the disk.
    let subagent_count = match &log.source {
        SessionSource::File(path) => Explorers::DISK
            .list_subagents_for_transcript(&log.agent_type, path)
            .await
            .len() as u32,
        _ => 0,
    };

    Some(SessionRecord {
        key: SessionKey::new(
            log.environment.key(),
            log.agent_type.slug(),
            session_id.clone(),
        ),
        source_kind: source_kind(&log.source).to_string(),
        source_label: log.source_label(),
        wsl_distro: log.environment.wsl_distro().map(str::to_string),
        title: metadata
            .as_ref()
            .and_then(|metadata| metadata.title.clone()),
        title_source: metadata
            .as_ref()
            .and_then(|metadata| metadata.title_source)
            .map(|source| source.as_str().to_string()),
        cwd: metadata.and_then(|metadata| metadata.cwd),
        surface: log.surface_label(home).to_string(),
        updated_at_epoch: log.updated_at,
        subagent_count,
        // Lineage is resolved when a session is opened: the observation lives
        // inside the transcript, and reading every transcript on every pass
        // would cost far more than the relationship is worth here.
        fork_parent_session_id: None,
    })
}

/// The id an agent embeds in the transcript's filename, for vendors whose
/// content parse does not surface one.
fn recovered_id(log: &SessionLog) -> Option<String> {
    match &log.source {
        SessionSource::File(path) => Explorers::DISK
            .recover_session_id_from_path(&log.agent_type, path)
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
            }),
        SessionSource::ProviderDb { session_id, .. } => Some(session_id.clone()),
        SessionSource::Inline { label, .. } => Some(label.clone()),
    }
}

fn source_kind(source: &SessionSource) -> &'static str {
    match source {
        SessionSource::File(_) => "file",
        SessionSource::Inline { .. } => "inline",
        SessionSource::ProviderDb { .. } => "providerDb",
    }
}

/// Per-agent `(slug, sessions seen, newest heartbeat)` for the scan-state table.
fn per_agent_totals(records: &[SessionRecord]) -> Vec<(String, i64, Option<i64>)> {
    let mut totals: std::collections::BTreeMap<String, (i64, Option<i64>)> =
        std::collections::BTreeMap::new();
    for record in records {
        let entry = totals.entry(record.key.agent.clone()).or_insert((0, None));
        entry.0 += 1;
        entry.1 = match (entry.1, record.updated_at_epoch) {
            (Some(seen), Some(candidate)) => Some(seen.max(candidate)),
            (seen, candidate) => seen.or(candidate),
        };
    }
    totals
        .into_iter()
        .map(|(agent, (seen, cursor))| (agent, seen, cursor))
        .collect()
}

/// Analyze the newest sessions whose cached analysis is missing or stale.
async fn top_up_analysis(app: &AppHandle, now: i64, activity_days: i64) -> anyhow::Result<()> {
    let store = app.state::<Store>();
    let since = now - activity_days.max(1) * 86_400;
    let candidates = store.recent_sessions(since, MAX_ANALYSES_PER_PASS)?;

    for record in candidates {
        let Some(agent) = crate::agents::kind_from_slug(&record.key.agent) else {
            continue;
        };
        if !analytics::analytics_supported(agent) {
            // A generically-parsed transcript would produce a half-confident
            // metric; the view says so instead of showing one.
            continue;
        }
        let Some(source) =
            analytics::locate(agent, &record.key.session_id, record.wsl_distro.as_deref()).await
        else {
            continue;
        };
        let fingerprint = analytics::fingerprint_of(&source);
        if let Some(cached) = store.analysis(&record.key)?
            && analytics::cache_is_fresh(&cached, &fingerprint)
        {
            continue;
        }

        let analysis =
            analytics::analyze(agent, &record.key.session_id, record.wsl_distro.as_deref()).await;
        if let Some(cache) = analysis.record(&record.key) {
            store.save_analysis(&cache)?;
        }
        if let Some(orchestration) = &analysis.orchestration {
            let relations: Vec<_> = orchestration
                .members
                .iter()
                .map(|member| crate::store::RelationRecord {
                    kind: crate::store::RelationKind::Subagent,
                    related_id: member.subagent_id.clone(),
                    label: Some(member.label.clone()),
                })
                .collect();
            store.replace_relations(
                &record.key,
                crate::store::RelationKind::Subagent,
                &relations,
            )?;
        }
    }
    Ok(())
}

/// The current time in unix seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::platform::environment::DiscoveryEnvironment;
    use std::collections::HashSet;

    /// A synthetic Claude store: `<home>/.claude/projects/<encoded>/<id>.jsonl`.
    /// Every value is fictional; the shapes are what the engine's scanner reads.
    fn write_claude_session(home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
        let project = home
            .join(".claude")
            .join("projects")
            .join("-home-avery-code-widgets");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{session_id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                concat!(
                    r#"{{"type":"summary","summary":"Wire the tray popover"}}"#,
                    "\n",
                    r#"{{"session_id":"{id}","cwd":"/home/avery/code/widgets","type":"user","#,
                    r#""timestamp":"2026-08-01T10:00:00Z"}}"#,
                    "\n",
                    r#"{{"type":"assistant","timestamp":"2026-08-01T10:01:00Z","#,
                    r#""message":{{"role":"assistant","model":"claude-opus-4-6","#,
                    r#""usage":{{"input_tokens":120,"output_tokens":40}}}}}}"#,
                    "\n",
                ),
                id = session_id
            ),
        )
        .unwrap();
        path
    }

    /// A synthetic Codex rollout: `<home>/.codex/sessions/YYYY/MM/DD/...jsonl`.
    fn write_codex_session(home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
        let day = home
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("08")
            .join("01");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join(format!("rollout-2026-08-01T10-00-00-{session_id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                concat!(
                    r#"{{"timestamp":"2026-08-01T10:00:00Z","type":"session_meta","#,
                    r#""payload":{{"id":"{id}","cwd":"/home/avery/code/gadgets"}}}}"#,
                    "\n",
                ),
                id = session_id
            ),
        )
        .unwrap();
        path
    }

    fn log(agent: AgentKind, path: std::path::PathBuf, updated_at: i64) -> SessionLog {
        SessionLog {
            agent_type: agent,
            source: SessionSource::File(path),
            updated_at: Some(updated_at),
            environment: DiscoveryEnvironment::Native,
        }
    }

    #[tokio::test]
    async fn describing_transcripts_recovers_identity_title_and_working_directory() {
        let home = tempfile::TempDir::new().unwrap();
        let claude = write_claude_session(home.path(), "11111111-2222-3333-4444-555555555555");
        let codex = write_codex_session(home.path(), "codex-abc");

        let records = describe(
            vec![
                log(AgentKind::Claude, claude, 1_800_000_000),
                log(AgentKind::Codex, codex, 1_800_000_100),
            ],
            home.path(),
            &HashSet::new(),
        )
        .await;

        assert_eq!(records.len(), 2);

        let claude = records
            .iter()
            .find(|record| record.key.agent == "claude-code")
            .expect("a claude record");
        assert_eq!(
            claude.key.session_id,
            "11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(claude.key.environment_key, "native");
        assert_eq!(claude.cwd.as_deref(), Some("/home/avery/code/widgets"));
        assert_eq!(claude.title.as_deref(), Some("Wire the tray popover"));
        assert_eq!(claude.source_kind, "file");
        assert_eq!(claude.subagent_count, 0);

        let codex = records
            .iter()
            .find(|record| record.key.agent == "codex")
            .expect("a codex record");
        assert_eq!(codex.key.session_id, "codex-abc");
        assert_eq!(codex.cwd.as_deref(), Some("/home/avery/code/gadgets"));
        assert!(matches!(
            codex.surface.as_str(),
            "cli" | "ide_desktop" | "unknown"
        ));
    }

    #[tokio::test]
    async fn an_opted_out_working_directory_never_reaches_the_store() {
        let home = tempfile::TempDir::new().unwrap();
        let claude = write_claude_session(home.path(), "aaaa-bbbb");
        let codex = write_codex_session(home.path(), "codex-abc");
        let logs = vec![
            log(AgentKind::Claude, claude, 1_800_000_000),
            log(AgentKind::Codex, codex, 1_800_000_100),
        ];

        // The engine's opt-out gate covers the directory and everything under it.
        let ignored = HashSet::from(["/home/avery/code/widgets".to_string()]);
        let records = describe(logs, home.path(), &ignored).await;

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key.agent, "codex");
    }

    #[tokio::test]
    async fn a_described_pass_round_trips_through_the_store_and_is_idempotent() {
        let home = tempfile::TempDir::new().unwrap();
        let store = crate::store::Store::open_in_memory(home.path()).unwrap();
        let claude = write_claude_session(home.path(), "aaaa-bbbb");
        let codex = write_codex_session(home.path(), "codex-abc");

        for _ in 0..2 {
            let records = describe(
                vec![
                    log(AgentKind::Claude, claude.clone(), 1_800_000_000),
                    log(AgentKind::Codex, codex.clone(), 1_800_000_100),
                ],
                home.path(),
                &HashSet::new(),
            )
            .await;
            store.upsert_sessions(&records).unwrap();
            for (agent, seen, cursor) in per_agent_totals(&records) {
                store.record_agent_scan(&agent, cursor, seen).unwrap();
            }
        }

        // A second pass over the same machine updates rather than duplicates.
        let stored = store.recent_sessions(0, 100).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(
            stored[0].key.session_id, "codex-abc",
            "newest heartbeat first"
        );

        let state = store.scan_state().unwrap();
        assert_eq!(state.len(), 2);
        assert!(
            state
                .iter()
                .all(|(_, completed, seen)| { completed.is_some() && *seen == 1 })
        );
    }

    #[tokio::test]
    async fn a_transcript_with_no_embedded_id_falls_back_to_its_filename() {
        let home = tempfile::TempDir::new().unwrap();
        let path = home.path().join("orphan-session.jsonl");
        std::fs::write(&path, "not json at all\n").unwrap();

        let records = describe(
            vec![log(AgentKind::Pi, path, 1_800_000_000)],
            home.path(),
            &HashSet::new(),
        )
        .await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key.session_id, "orphan-session");
    }

    fn record(agent: &str, session_id: &str, updated_at: Option<i64>) -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", agent, session_id),
            source_kind: "file".into(),
            source_label: format!("/tmp/{session_id}.jsonl"),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            updated_at_epoch: updated_at,
            subagent_count: 0,
            fork_parent_session_id: None,
        }
    }

    #[test]
    fn per_agent_totals_count_sessions_and_keep_the_newest_heartbeat() {
        let records = vec![
            record("claude-code", "a", Some(1_000)),
            record("claude-code", "b", Some(3_000)),
            record("codex", "c", Some(2_000)),
            record("codex", "d", None),
        ];
        let totals = per_agent_totals(&records);
        assert_eq!(
            totals,
            vec![
                ("claude-code".to_string(), 2, Some(3_000)),
                ("codex".to_string(), 2, Some(2_000)),
            ]
        );
    }

    #[test]
    fn a_pass_with_nothing_discovered_reports_no_agents() {
        assert!(per_agent_totals(&[]).is_empty());
    }

    #[test]
    fn source_kinds_are_stable_wire_strings() {
        assert_eq!(
            source_kind(&SessionSource::File("/tmp/x.jsonl".into())),
            "file"
        );
        assert_eq!(
            source_kind(&SessionSource::Inline {
                label: "opencode:x".into(),
                content: String::new(),
            }),
            "inline"
        );
        assert_eq!(
            source_kind(&SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: "/tmp/opencode.db".into(),
                session_id: "x".into(),
            }),
            "providerDb"
        );
    }

    #[test]
    fn a_provider_database_session_recovers_its_id_from_the_source() {
        let log = SessionLog {
            agent_type: AgentKind::OpenCode,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: "/tmp/opencode.db".into(),
                session_id: "ses_123".into(),
            },
            updated_at: Some(1_000),
            environment: Default::default(),
        };
        assert_eq!(recovered_id(&log).as_deref(), Some("ses_123"));
    }

    #[test]
    fn a_file_session_falls_back_to_its_filename_stem() {
        let log = SessionLog {
            agent_type: AgentKind::Claude,
            source: SessionSource::File("/home/avery/.claude/projects/demo/abc-123.jsonl".into()),
            updated_at: Some(1_000),
            environment: Default::default(),
        };
        assert_eq!(recovered_id(&log).as_deref(), Some("abc-123"));
    }

    #[test]
    fn the_controller_reports_the_popover_gate_and_a_clean_initial_status() {
        let controller = ScanController::default();
        assert!(!controller.popover_visible());
        controller.set_popover_visible(true);
        assert!(controller.popover_visible());

        let status = controller.status();
        assert!(!status.running);
        assert_eq!(status.sessions, 0);
        assert!(status.error.is_none());
    }

    #[test]
    fn retention_is_never_narrower_than_the_widest_activity_window() {
        // The store must be able to answer a widened window without a rescan.
        assert!(RETENTION_DAYS >= i64::from(crate::store::model::DEFAULT_ACTIVITY_DAYS));
    }
}
