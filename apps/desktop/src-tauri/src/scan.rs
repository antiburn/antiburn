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
//! antiburn is an always-running background utility. CPU time, memory, open
//! files, and disk I/O are therefore correctness constraints, not optional
//! optimizations: a scan must do no more work, retain no more data in memory,
//! and run no more often than the visible feature requires.
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
//! # Pausing
//!
//! `AppSettings::discovery_paused` stops every *scheduled* pass — the launch
//! pass, the tick, and the passes requested when the popover opens or the
//! sources change. It deliberately does not stop [`run_pass`] itself, so the
//! rescan control still works while discovery is paused and the popover keeps
//! browsing everything already indexed. "Paused" is a statement about
//! background work, not a lock on the app.
//!
//! # Cancelling
//!
//! A pass in flight can be asked to stop ([`ScanController::request_cancel`]).
//! The engine's discovery walk is not itself interruptible, so a cancel lands
//! at the next phase boundary: nothing further is analyzed, and the status the
//! views render says `cancelled` rather than pretending the pass completed.
//!
//! # Failing
//!
//! A pass that ends in an error says so in three places, each for a different
//! reader: [`ScanStatus::error`] for the status line, [`crate::storage_health`]
//! when the failure was a *write* (which the popover surfaces as a banner with
//! a retry), and [`crate::notifications`] once per run of the app, for someone
//! who is not looking at antiburn at all.
//!
//! Every pass is bounded: discovery is windowed to the widest activity view, the
//! per-session metadata reads run at a fixed concurrency, and analysis is
//! capped at [`MAX_ANALYSES_PER_PASS`] sessions so one pass cannot grow with
//! the size of the machine. Sessions already indexed are retained until the
//! reader explicitly clears them; the bounded discovery window is not a data
//! expiry policy. The scheduler is a single task whose handle the app aborts on
//! exit, so nothing outlives the process.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::discovery::scanner::TitleSource;
use antiburn_local::discovery::{
    Explorers, ResolvedTitle, SessionLog, SessionSource, TitleLookupKind, session_log_metadata,
    session_source_preview,
};
use antiburn_local::model::AgentKind;
use antiburn_local::paths::{home_dir, ignored_paths};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::analytics;
use crate::dto::ScanStatus;
use crate::repositories;
use crate::storage_health::{self, checked};
use crate::store::{SessionKey, SessionRecord, Store};

/// How often the scheduler wakes up.
pub const TICK: Duration = Duration::from_secs(60);

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
    cancel: AtomicBool,
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

    /// Ask the pass in flight to stop at its next phase boundary.
    ///
    /// A no-op when nothing is running: the flag is cleared at the start of
    /// every pass, so a stale request cannot cancel a future one.
    pub fn request_cancel(&self) {
        if self.running.load(Ordering::SeqCst) {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
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
        if scheduled_scanning_allowed(&app) {
            run_pass(&app).await;
        }
        loop {
            let controller = app.state::<ScanController>();
            tokio::select! {
                () = controller.kick.notified() => {}
                () = tokio::time::sleep(TICK) => {
                    if !controller.popover_visible() {
                        continue;
                    }
                }
            }
            // Checked after the wake-up rather than before the wait, so
            // resuming discovery takes effect at the next request or tick
            // instead of needing the app restarted.
            if !scheduled_scanning_allowed(&app) {
                continue;
            }
            run_pass(&app).await;
        }
    })
}

/// Whether the scheduler may start a pass of its own right now.
///
/// Two gates, both of them the reader's: onboarding has to be finished (before
/// that there are no chosen sources to scan), and discovery must not be paused.
/// Neither gate applies to an explicitly requested [`run_pass`].
fn scheduled_scanning_allowed(app: &AppHandle) -> bool {
    app.state::<Store>()
        .settings()
        .map(|settings| settings.onboarding_completed && !settings.discovery_paused)
        .unwrap_or(false)
}

/// Run one pass, unless one is already in flight.
pub async fn run_pass(app: &AppHandle) -> ScanStatus {
    {
        let controller = app.state::<ScanController>();
        if controller.running.swap(true, Ordering::SeqCst) {
            return controller.status();
        }
        // A cancel request only ever applies to the pass it was made during.
        controller.cancel.store(false, Ordering::SeqCst);
        let started = controller.update(|status| {
            status.running = true;
            status.completed_agents = 0;
            status.total_agents = AgentKind::ALL.len();
            status.sessions = 0;
            status.error = None;
            status.cancelled = false;
        });
        let _ = app.emit(EVENT_STARTED, started);
    }

    let outcome = pass(app).await;

    let controller = app.state::<ScanController>();
    let cancelled = controller.cancelled();
    let finished = controller.update(|status| {
        status.running = false;
        status.cancelled = cancelled;
        status.finished_at = Some(crate::store::now_rfc3339());
        match &outcome {
            Ok(sessions) => {
                status.sessions = *sessions;
                // A cancelled pass did not finish every agent, and saying it
                // did would make the progress line lie on its last frame.
                if !cancelled {
                    status.completed_agents = status.total_agents;
                }
                status.error = None;
            }
            Err(error) => status.error = Some(error.to_string()),
        }
    });
    controller.running.store(false, Ordering::SeqCst);
    controller.cancel.store(false, Ordering::SeqCst);
    // A pass that got all the way through wrote to the store several times, so
    // it is also the proof that a previously reported storage failure is over.
    if outcome.is_ok() {
        storage_health::note_ok(app);
    }
    let _ = app.emit(EVENT_FINISHED, finished.clone());
    crate::notifications::note_scan_outcome(app, &finished);
    finished
}

/// The body of one pass. Split out so [`run_pass`] owns only the in-flight
/// bookkeeping and the events.
async fn pass(app: &AppHandle) -> anyhow::Result<usize> {
    let store = app.state::<Store>();
    let settings = store.settings()?;
    let now = unix_now();
    // Discovery always covers the widest list the UI can request, so changing
    // the display window is instant. Previously indexed sessions outside this
    // lookback remain in the store indefinitely.
    let window_days = i64::from(crate::store::MAX_ACTIVITY_DAYS);
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

    let Described { records, rejected } = describe(logs, &home, &ignored).await;
    // Every write below is routed through the storage-health check, so a
    // database that has stopped accepting writes becomes a banner in the
    // popover rather than a list that silently stops changing.
    checked(app, "The session index", store.upsert_sessions(&records))?;

    // A transcript the gate rejected may have been indexed by an earlier
    // version of the app that did not gate; the row is removed rather than
    // left to mislead indefinitely.
    for key in &rejected {
        checked(
            app,
            "The session index",
            store.delete_session(key).map(|_| ()),
        )?;
    }

    for (agent, seen, cursor) in per_agent_totals(&records) {
        checked(
            app,
            "The scan bookkeeping",
            store.record_agent_scan(&agent, cursor, seen),
        )?;
    }

    // Everything discovered so far is already persisted, so a cancel here keeps
    // the reader's results and only skips the work still ahead.
    if app.state::<ScanController>().cancelled() {
        return Ok(records.len());
    }

    // Derived analysis for the newest sessions in the visible window, so the
    // list's cost and time pills are populated without opening every row.
    top_up_analysis(app, now, i64::from(settings.activity_window_days)).await?;

    if app.state::<ScanController>().cancelled() {
        return Ok(records.len());
    }

    repositories::refresh(app).await?;

    Ok(records.len())
}

/// What one scan pass learned: rows for the index, and previously indexable
/// transcripts the sub-agent gate now refuses.
struct Described {
    records: Vec<SessionRecord>,
    rejected: Vec<SessionKey>,
}

/// Read metadata for every discovered log, at a bounded concurrency, and drop
/// the ones the reader opted out of.
async fn describe(
    logs: Vec<SessionLog>,
    home: &std::path::Path,
    ignored: &std::collections::HashSet<String>,
) -> Described {
    let indexed_titles = indexed_titles_for_logs(&logs).await;
    let mut records = Vec::with_capacity(logs.len());
    let mut rejected = Vec::new();
    for chunk in logs.chunks(METADATA_CONCURRENCY) {
        let mut set = JoinSet::new();
        for log in chunk {
            let log = log.clone();
            let home = home.to_path_buf();
            let indexed_title = recovered_id(&log)
                .and_then(|session_id| indexed_titles.get(&(log.agent_type, session_id)).cloned());
            set.spawn(async move { describe_one(log, &home, indexed_title).await });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(DescribeOutcome::Session(record)) => {
                    let cwd = record.cwd.as_deref();
                    // The engine's opt-out gate, applied once here so every
                    // surface that reads the store inherits it.
                    if cwd.is_some_and(|cwd| ignored_paths::set_contains(ignored, cwd)) {
                        continue;
                    }
                    records.push(*record);
                }
                Ok(DescribeOutcome::Subagent(key)) => rejected.push(key),
                Ok(DescribeOutcome::Skip) | Err(_) => {}
            }
        }
    }
    Described { records, rejected }
}

/// Read each durable vendor title store once for the sessions in this pass.
/// Transcript fallback stays inside `describe_one`, which already has bounded
/// metadata and preview reads for the same log.
async fn indexed_titles_for_logs(
    logs: &[SessionLog],
) -> std::collections::HashMap<(AgentKind, String), ResolvedTitle> {
    let mut indexed = std::collections::HashMap::new();
    for agent in AgentKind::ALL {
        let mut session_ids: Vec<String> = logs
            .iter()
            .filter(|log| log.agent_type == *agent && should_lookup_indexed_title(log))
            .filter_map(recovered_id)
            .collect();
        session_ids.sort_unstable();
        session_ids.dedup();
        if session_ids.is_empty() {
            continue;
        }
        for (session_id, title) in Explorers::DISK
            .indexed_session_titles_for(agent, &session_ids)
            .await
        {
            indexed.insert((*agent, session_id), title);
        }
    }
    indexed
}

enum DescribeOutcome {
    Session(Box<SessionRecord>),
    /// A sub-agent transcript: never listed, and evicted if an earlier,
    /// ungated version of the app indexed it.
    Subagent(SessionKey),
    Skip,
}

async fn describe_one(
    log: SessionLog,
    home: &std::path::Path,
    indexed_title: Option<ResolvedTitle>,
) -> DescribeOutcome {
    let metadata = session_log_metadata(&log).await;
    let Some(session_id) = metadata
        .as_ref()
        .and_then(|metadata| metadata.session_id.clone())
        .or_else(|| recovered_id(&log))
    else {
        return DescribeOutcome::Skip;
    };
    if session_id.is_empty() {
        return DescribeOutcome::Skip;
    }

    let key = SessionKey::new(
        log.environment.key(),
        log.agent_type.slug(),
        session_id.clone(),
    );
    // One bounded content read serves both content checks below.
    let preview = match log.agent_type {
        AgentKind::Claude | AgentKind::Codex => session_source_preview(&log.source).await,
        _ => None,
    };
    if is_subagent_transcript(&log, preview.as_deref()) {
        return DescribeOutcome::Subagent(key);
    }

    let resolved_title = if should_lookup_indexed_title(&log) {
        indexed_title
    } else {
        None
    };
    let (title, title_source) = select_title_pair(
        resolved_title,
        metadata
            .as_ref()
            .and_then(|metadata| metadata.title.clone()),
        metadata.as_ref().and_then(|metadata| metadata.title_source),
        &log.agent_type,
        preview.as_deref(),
    );

    // A dir listing per orchestrator-capable session; vendors that record no
    // orchestration return empty without touching the disk.
    let subagent_count = match &log.source {
        SessionSource::File(path) => Explorers::DISK
            .list_subagents_for_transcript(&log.agent_type, path)
            .await
            .len() as u32,
        _ => 0,
    };

    DescribeOutcome::Session(Box::new(SessionRecord {
        key,
        source_kind: source_kind(&log.source).to_string(),
        source_label: log.source_label(),
        wsl_distro: log.environment.wsl_distro().map(str::to_string),
        title,
        title_source,
        cwd: metadata.and_then(|metadata| metadata.cwd),
        surface: log.surface_label(home).to_string(),
        updated_at_epoch: log.updated_at,
        subagent_count,
        // Lineage is resolved when a session is opened: the observation lives
        // inside the transcript, and reading every transcript on every pass
        // would cost far more than the relationship is worth here.
        fork_parent_session_id: None,
    }))
}

/// Native stores with a point-query index are authoritative for session names.
/// Mounted WSL sessions deliberately stay on their transcript-local metadata:
/// a native store may contain the same vendor id but belongs to another
/// environment.
fn should_lookup_indexed_title(log: &SessionLog) -> bool {
    log.environment.is_native()
        && matches!(
            Explorers::DISK.title_lookup_kind_for(&log.agent_type),
            TitleLookupKind::Direct
        )
}

/// Keep the title and its provenance coupled. Indexed, renamed, and explicit
/// vendor titles win directly; first-message provenance remains transcript
/// fallback and still passes through sanitization.
fn select_title_pair(
    resolved: Option<ResolvedTitle>,
    fallback_title: Option<String>,
    fallback_source: Option<TitleSource>,
    agent: &AgentKind,
    preview: Option<&str>,
) -> (Option<String>, Option<String>) {
    if let Some(resolved) = resolved {
        let source = resolved.source;
        let title = if source == TitleSource::FirstMessage {
            sanitized_title(Some(resolved.text), agent, preview)
        } else {
            Some(resolved.text)
        };
        let source = title.as_ref().map(|_| source.as_str().to_string());
        return (title, source);
    }

    let title = sanitized_title(fallback_title, agent, preview);
    let source = title
        .as_ref()
        .and(fallback_source)
        .map(|source| source.as_str().to_string());
    (title, source)
}

/// Whether this transcript belongs to a sub-agent rather than a top-level
/// session. Sub-agent work is presented on the parent row (the roster and the
/// fan-out pill), so listing the transcript as its own session would double
/// what the reader already sees.
///
/// The evidence is in the content, not the path: some agent versions write
/// sidechain transcripts beside top-level ones in the same directory.
///
/// - **Claude Code**: a sidechain transcript marks its records with
///   `isSidechain: true` and carries a top-level `agentId` string. The check
///   parses up to [`SUBAGENT_SCAN_LINES`] leading records — the marker is on
///   the very first record in every observed format, so the bound is
///   generosity, not risk.
/// - **Codex**: a sub-agent thread's `session_meta` payload names a parent
///   (`parent_thread_id`), an object `source`, or `thread_source:
///   "subagent"`. Discovery already excludes the known shapes; this catches
///   records written by earlier app versions and format drift.
fn is_subagent_transcript(log: &SessionLog, preview: Option<&str>) -> bool {
    let claude = matches!(log.agent_type, AgentKind::Claude);
    let codex = matches!(log.agent_type, AgentKind::Codex);
    if !claude && !codex {
        return false;
    }
    let Some(content) = preview else {
        return false;
    };
    let lines = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(SUBAGENT_SCAN_LINES);
    for line in lines {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if claude
            && (value.get("isSidechain").and_then(|v| v.as_bool()) == Some(true)
                || value.get("agentId").is_some_and(|v| v.is_string()))
        {
            return true;
        }
        if codex && value.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
            let payload = value.get("payload").unwrap_or(&serde_json::Value::Null);
            if payload
                .get("parent_thread_id")
                .is_some_and(|v| !v.is_null())
                || payload.get("source").is_some_and(|v| v.is_object())
                || payload.get("thread_source").and_then(|v| v.as_str()) == Some("subagent")
            {
                return true;
            }
        }
    }
    false
}

/// Leading records the sub-agent check reads before deciding a transcript is
/// top-level. The markers sit on the first record in practice.
const SUBAGENT_SCAN_LINES: usize = 40;

/// Longest title the list stores. Matches what one activity row can show with
/// room for truncation, and keeps a pasted wall of text from becoming the row.
const MAX_TITLE_CHARS: usize = 200;

/// Whether a title is injected context rather than something the reader wrote:
/// harness blocks like `<recommended_plugins>` / `<system-reminder>` ride the
/// transcript as user-role messages, and a title fallback that picks the first
/// user message picks them up.
fn is_injected_title(title: &str) -> bool {
    let trimmed = title.trim_start();
    trimmed.starts_with('<') || trimmed.starts_with("Caveat:")
}

/// Replace an injected-context title with the first thing the reader actually
/// typed, or drop the title entirely so the row falls back to its path label.
///
/// Only Claude transcripts need this: their harness injects context blocks as
/// user-role messages, and the engine's title fallback (first user message)
/// cannot tell those from human input without re-reading content — which the
/// scan has already done for the sub-agent gate.
fn sanitized_title(
    title: Option<String>,
    agent: &AgentKind,
    preview: Option<&str>,
) -> Option<String> {
    let title = title?;
    if !is_injected_title(&title) {
        return Some(title);
    }
    if !matches!(agent, AgentKind::Claude) {
        return None;
    }
    let content = preview?;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("user")
            || value.get("isMeta").and_then(|v| v.as_bool()) == Some(true)
            || value.get("isSidechain").and_then(|v| v.as_bool()) == Some(true)
        {
            continue;
        }
        let message = value.get("message");
        let text = message
            .and_then(|m| m.get("content"))
            .and_then(|content| match content {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Array(parts) => parts.iter().find_map(|part| {
                    if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                        part.get("text")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    } else {
                        None
                    }
                }),
                _ => None,
            });
        let Some(text) = text else {
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() || is_injected_title(trimmed) {
            continue;
        }
        let first_line = trimmed.lines().next().unwrap_or_default().trim();
        if first_line.is_empty() {
            continue;
        }
        return Some(first_line.chars().take(MAX_TITLE_CHARS).collect());
    }
    None
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
        // Analysis is the long tail of a pass — one whole transcript read per
        // session — so this is where a cancel is felt.
        if app.state::<ScanController>().cancelled() {
            return Ok(());
        }
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
                    r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"Fallback transcript request"}}]}}}}"#,
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

    #[test]
    fn only_native_direct_agents_are_eligible_for_indexed_title_lookups() {
        let direct = [AgentKind::Claude, AgentKind::Codex, AgentKind::OpenCode];
        for agent in AgentKind::ALL {
            let native = SessionLog {
                agent_type: *agent,
                source: SessionSource::Inline {
                    label: "synthetic".into(),
                    content: String::new(),
                },
                updated_at: None,
                environment: DiscoveryEnvironment::Native,
            };
            assert_eq!(
                should_lookup_indexed_title(&native),
                direct.contains(agent),
                "unexpected lookup route for {agent}"
            );

            let in_wsl = SessionLog {
                environment: DiscoveryEnvironment::Wsl {
                    distribution: "SyntheticLinux".into(),
                    user: "avery".into(),
                },
                ..native
            };
            assert!(
                !should_lookup_indexed_title(&in_wsl),
                "WSL {agent} must not query native stores"
            );
        }
    }

    #[test]
    fn direct_titles_are_authoritative_and_keep_their_source() {
        let first = select_title_pair(
            Some(ResolvedTitle::new(
                "Generated session name",
                TitleSource::AiGenerated,
            )),
            Some("<injected transcript context>".into()),
            Some(TitleSource::FirstMessage),
            &AgentKind::Codex,
            None,
        );
        assert_eq!(
            first,
            (
                Some("Generated session name".into()),
                Some("aiGenerated".into())
            )
        );

        let renamed = select_title_pair(
            Some(ResolvedTitle::new(
                "Reader renamed session",
                TitleSource::UserRename,
            )),
            Some("old transcript fallback".into()),
            Some(TitleSource::FirstMessage),
            &AgentKind::Codex,
            None,
        );
        assert_eq!(
            renamed,
            (
                Some("Reader renamed session".into()),
                Some("userRename".into())
            )
        );

        let transcript_fallback = select_title_pair(
            Some(ResolvedTitle::new(
                "<recommended_plugins> injected context",
                TitleSource::FirstMessage,
            )),
            None,
            None,
            &AgentKind::Claude,
            Some(concat!(
                r#"{"type":"user","message":{"role":"user","content":"<recommended_plugins> injected context"}}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":"Reader's actual request"}}"#,
                "\n",
            )),
        );
        assert_eq!(
            transcript_fallback,
            (
                Some("Reader's actual request".into()),
                Some("firstMessage".into())
            )
        );
    }

    #[test]
    fn a_direct_lookup_miss_keeps_the_transcript_fallback_pair() {
        assert_eq!(
            select_title_pair(
                None,
                Some("First reader request".into()),
                Some(TitleSource::FirstMessage),
                &AgentKind::Codex,
                None,
            ),
            (
                Some("First reader request".into()),
                Some("firstMessage".into())
            )
        );
    }

    #[tokio::test]
    async fn a_native_codex_title_refreshes_while_wsl_keeps_its_own_fallback() {
        let home = tempfile::TempDir::new().unwrap();
        let session_id = "same-id-in-two-environments";
        let path = write_codex_session(home.path(), session_id);
        let native_log = log(AgentKind::Codex, path.clone(), 1_800_000_000);
        let store = crate::store::Store::open_in_memory(home.path()).unwrap();

        for (title, source) in [
            ("Indexed session name", TitleSource::AiGenerated),
            ("Reader renamed session", TitleSource::UserRename),
        ] {
            let DescribeOutcome::Session(record) = describe_one(
                native_log.clone(),
                home.path(),
                Some(ResolvedTitle::new(title, source)),
            )
            .await
            else {
                panic!("native Codex session should be described");
            };
            store.upsert_sessions(&[*record]).unwrap();
        }

        let native = store
            .session(&SessionKey::new("native", "codex", session_id))
            .unwrap()
            .expect("native session");
        assert_eq!(native.title.as_deref(), Some("Reader renamed session"));
        assert_eq!(native.title_source.as_deref(), Some("userRename"));

        let wsl_log = SessionLog {
            environment: DiscoveryEnvironment::Wsl {
                distribution: "SyntheticLinux".into(),
                user: "avery".into(),
            },
            ..log(AgentKind::Codex, path, 1_800_000_100)
        };
        let DescribeOutcome::Session(wsl) = describe_one(
            wsl_log,
            home.path(),
            // A same-id native hit is available but must be ignored for WSL.
            Some(ResolvedTitle::new(
                "Native title must not leak",
                TitleSource::UserRename,
            )),
        )
        .await
        else {
            panic!("WSL Codex session should be described");
        };
        assert_eq!(wsl.key.environment_key, "wsl:syntheticlinux");
        assert_eq!(wsl.title.as_deref(), Some("Fallback transcript request"));
        assert_eq!(wsl.title_source.as_deref(), Some("firstMessage"));
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

        assert_eq!(records.records.len(), 2);
        assert!(records.rejected.is_empty());

        let claude = records
            .records
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
            .records
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

        assert_eq!(records.records.len(), 1);
        assert_eq!(records.records[0].key.agent, "codex");
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
            store.upsert_sessions(&records.records).unwrap();
            for (agent, seen, cursor) in per_agent_totals(&records.records) {
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

    /// A synthetic Claude sidechain transcript: `agentId` on every record and
    /// `isSidechain: true`, written beside top-level sessions the way current
    /// agent versions do.
    fn write_claude_sidechain(home: &std::path::Path, agent_id: &str) -> std::path::PathBuf {
        let project = home
            .join(".claude")
            .join("projects")
            .join("-home-avery-code-widgets");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("agent-{agent_id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                concat!(
                    r#"{{"type":"user","isSidechain":true,"agentId":"{id}","#,
                    r#""sessionId":"{id}","cwd":"/home/avery/code/widgets","#,
                    r#""timestamp":"2026-08-01T10:00:00Z","#,
                    r#""message":{{"role":"user","content":"subtask"}}}}"#,
                    "\n",
                ),
                id = agent_id
            ),
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn a_sidechain_transcript_is_rejected_not_listed() {
        let home = tempfile::TempDir::new().unwrap();
        let parent = write_claude_session(home.path(), "11111111-2222-3333-4444-555555555555");
        let sidechain = write_claude_sidechain(home.path(), "aaaa-1111");

        let described = describe(
            vec![
                log(AgentKind::Claude, parent, 1_800_000_000),
                log(AgentKind::Claude, sidechain, 1_800_000_050),
            ],
            home.path(),
            &HashSet::new(),
        )
        .await;

        assert_eq!(described.records.len(), 1, "only the parent is listable");
        assert_eq!(described.rejected.len(), 1);
        assert_eq!(described.rejected[0].session_id, "aaaa-1111");
    }

    #[tokio::test]
    async fn a_codex_subagent_thread_is_rejected_not_listed() {
        let home = tempfile::TempDir::new().unwrap();
        let day = home.path().join(".codex/sessions/2026/08/01");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-2026-08-01T10-00-00-child-1.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"timestamp":"2026-08-01T10:00:00Z","type":"session_meta","#,
                r#""payload":{"id":"child-1","cwd":"/home/avery/code/gadgets","#,
                r#""parent_thread_id":"parent-9","thread_source":"subagent"}}"#,
                "\n",
            ),
        )
        .unwrap();

        let described = describe(
            vec![log(AgentKind::Codex, path, 1_800_000_000)],
            home.path(),
            &HashSet::new(),
        )
        .await;

        assert!(described.records.is_empty());
        assert_eq!(described.rejected.len(), 1);
        assert_eq!(described.rejected[0].session_id, "child-1");
    }

    #[tokio::test]
    async fn a_rejected_transcript_evicts_its_stale_row_from_the_store() {
        let home = tempfile::TempDir::new().unwrap();
        let store = crate::store::Store::open_in_memory(home.path()).unwrap();
        // An earlier, ungated version of the app indexed the sidechain.
        store
            .upsert_sessions(&[record("claude-code", "aaaa-1111", Some(1_800_000_000))])
            .unwrap();
        assert_eq!(store.recent_sessions(0, 10).unwrap().len(), 1);

        let sidechain = write_claude_sidechain(home.path(), "aaaa-1111");
        let described = describe(
            vec![log(AgentKind::Claude, sidechain, 1_800_000_050)],
            home.path(),
            &HashSet::new(),
        )
        .await;
        for key in &described.rejected {
            store.delete_session(key).unwrap();
        }

        assert!(store.recent_sessions(0, 10).unwrap().is_empty());
    }

    /// A transcript whose first user message is an injected harness block:
    /// the title must come from the first thing the reader actually typed.
    #[tokio::test]
    async fn an_injected_context_block_never_becomes_the_title() {
        let home = tempfile::TempDir::new().unwrap();
        let project = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-home-avery-code-widgets");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("cccc-dddd.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"sessionId":"cccc-dddd","cwd":"/home/avery/code/widgets","type":"user","#,
                r#""timestamp":"2026-08-01T10:00:00Z","message":{"role":"user","#,
                r#""content":"<recommended_plugins> Here is a list of plugins that are recommended."}}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-08-01T10:00:10Z","#,
                r#""message":{"role":"user","content":"Fix the tray popover anchoring"}}"#,
                "\n",
            ),
        )
        .unwrap();

        let described = describe(
            vec![log(AgentKind::Claude, path, 1_800_000_000)],
            home.path(),
            &HashSet::new(),
        )
        .await;

        assert_eq!(described.records.len(), 1);
        assert_eq!(
            described.records[0].title.as_deref(),
            Some("Fix the tray popover anchoring")
        );
    }

    /// A transcript that is nothing but injected context gets no title at all
    /// — the row falls back to its path label rather than showing harness
    /// text as if the reader wrote it.
    #[tokio::test]
    async fn a_transcript_with_only_injected_context_gets_no_title() {
        assert_eq!(
            sanitized_title(
                Some("<recommended_plugins> Here is a list".to_string()),
                &AgentKind::Claude,
                Some(concat!(
                    r#"{"type":"user","message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}"#,
                    "\n",
                )),
            ),
            None
        );
        // Non-injected titles pass through untouched.
        assert_eq!(
            sanitized_title(Some("Fix the bug".to_string()), &AgentKind::Claude, None),
            Some("Fix the bug".to_string())
        );
        // "Caveat:" is the harness's resumed-session preamble, not the reader.
        assert_eq!(
            sanitized_title(
                Some("Caveat: the messages below were generated".to_string()),
                &AgentKind::Claude,
                None
            ),
            None
        );

        let home = tempfile::TempDir::new().unwrap();
        let project = home
            .path()
            .join(".claude/projects/-home-avery-code-widgets");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("only-context.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"sessionId":"only-context","cwd":"/home/avery/code/widgets","type":"user","message":{"role":"user","content":"<recommended_plugins> list"}}"#,
                "\n",
            ),
        )
        .unwrap();
        let described = describe(
            vec![log(AgentKind::Claude, path, 1_800_000_000)],
            home.path(),
            &HashSet::new(),
        )
        .await;
        assert_eq!(described.records.len(), 1);
        assert_eq!(described.records[0].title, None);
        assert_eq!(described.records[0].title_source, None);
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
        assert_eq!(records.records.len(), 1);
        assert_eq!(records.records[0].key.session_id, "orphan-session");
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
    fn a_codex_rollout_recovers_its_canonical_uuid_before_title_prefetch() {
        let session_id = "01a01251-9875-7121-ac24-0d99fd8ccbe1";
        let log = SessionLog {
            agent_type: AgentKind::Codex,
            source: SessionSource::File(
                format!(
                    "/home/avery/.codex/sessions/2026/08/18/rollout-2026-08-18T10-42-12-{session_id}.jsonl"
                )
                .into(),
            ),
            updated_at: Some(1_000),
            environment: Default::default(),
        };

        assert_eq!(recovered_id(&log).as_deref(), Some(session_id));
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
        assert!(!status.cancelled);
    }

    #[test]
    fn a_cancel_request_only_applies_while_a_pass_is_running() {
        let controller = ScanController::default();

        // Nothing is running: a cancel would otherwise be remembered and would
        // kill the *next* pass, which is not what the reader asked for.
        controller.request_cancel();
        assert!(!controller.cancelled());

        controller.running.store(true, Ordering::SeqCst);
        controller.request_cancel();
        assert!(controller.cancelled());
    }
}
