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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::discovery::scanner::{self, TitleSource};
use antiburn_local::discovery::{
    Explorers, ResolvedTitle, SessionLog, SessionSource, SourceDescriptor, TitleLookupKind,
    session_log_read, session_source_tail,
};
use antiburn_local::model::AgentKind;
use antiburn_local::paths::{home_dir, ignored_paths};
use antiburn_local::titles::TitleInput;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::analysis;
use crate::dto::ScanStatus;
use crate::repositories;
use crate::storage_health::{self, checked};
use crate::store::{SessionActivityKey, SessionActivityState, SessionKey, SessionRecord, Store};
use crate::titles::LocalSummaryCandidate;

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
    cancel: Arc<AtomicBool>,
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

    pub fn cancel_flag(&self) -> analysis::CancelFlag {
        analysis::CancelFlag::from_flag(Arc::clone(&self.cancel))
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

#[derive(Clone, Copy)]
pub(crate) enum Wake {
    Launch,
    Tick,
    Kick,
}

pub(crate) fn should_run_scheduled_pass(wake: Wake, popover_visible: bool, allowed: bool) -> bool {
    allowed && (!matches!(wake, Wake::Tick) || popover_visible)
}

pub(crate) fn on_demand_start(controller: &ScanController) -> bool {
    if controller.running.swap(true, Ordering::SeqCst) {
        return false;
    }
    controller.cancel.store(false, Ordering::SeqCst);
    true
}

/// Start the scheduler. The returned handle is aborted when the app exits.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A fresh install has nothing to scan until the reader picks sources.
        if should_run_scheduled_pass(Wake::Launch, false, scheduled_scanning_allowed(&app)) {
            run_pass(&app, None).await;
        }
        loop {
            let controller = app.state::<ScanController>();
            let wake = tokio::select! {
                () = controller.kick.notified() => Wake::Kick,
                () = tokio::time::sleep(TICK) => Wake::Tick,
            };
            // Checked after the wake-up rather than before the wait, so
            // resuming discovery takes effect at the next request or tick
            // instead of needing the app restarted.
            if !should_run_scheduled_pass(
                wake,
                controller.popover_visible(),
                scheduled_scanning_allowed(&app),
            ) {
                continue;
            }
            run_pass(&app, None).await;
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
pub async fn run_pass(app: &AppHandle, activity_window_days: Option<u32>) -> ScanStatus {
    {
        let controller = app.state::<ScanController>();
        if !on_demand_start(&controller) {
            return controller.status();
        }
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

    let outcome = pass(app, activity_window_days).await;

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
    // The outcome, not a shaped event: whether this pass is worth reporting at
    // all is an analytics question, and this scheduler runs a pass a minute
    // while the popover is open. `None` is a failure, which travels as a bare
    // category — an error string can hold a path.
    crate::analytics::record_scan(app, outcome.as_ref().ok().map(|n| *n as u64));
    crate::notifications::note_scan_outcome(app, &finished);
    finished
}

/// The body of one pass. Split out so [`run_pass`] owns only the in-flight
/// bookkeeping and the events.
async fn pass(app: &AppHandle, activity_window_days: Option<u32>) -> anyhow::Result<usize> {
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

    let activity_states = store.session_activity_states()?;
    let Described {
        records,
        rejected,
        summary_candidates,
    } = describe_with_states(logs, &home, &ignored, &activity_states).await;
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

    // Generate titles on device for sessions stuck on the first-message
    // fallback. Newest sessions first: the pass is capped, and the rows a
    // reader sees are the ones that deserve a name first. The write is
    // guarded, so a title that arrived after the upsert above still wins.
    if settings.local_summary_titles
        && let Some(summarizer) = crate::titles::platform_summarizer()
    {
        let updated_at: std::collections::HashMap<&SessionKey, i64> = records
            .iter()
            .map(|record| (&record.key, record.updated_at_epoch.unwrap_or(0)))
            .collect();
        let mut summary_candidates = summary_candidates;
        summary_candidates.sort_by_key(|candidate| {
            std::cmp::Reverse(updated_at.get(&candidate.key).copied().unwrap_or(0))
        });
        crate::titles::local_summary_pass(&store, summarizer.as_ref(), &summary_candidates).await;
    }

    // Everything discovered so far is already persisted, so a cancel here keeps
    // the reader's results and only skips the work still ahead.
    let controller = app.state::<ScanController>();
    if controller.cancelled() {
        return Ok(records.len());
    }

    // Derived analysis for the newest sessions in the visible window, so the
    // list's cost and time pills are populated without opening every row.
    let activity_window_days = activity_window_days
        .unwrap_or(settings.activity_window_days)
        .clamp(
            crate::store::MIN_ACTIVITY_DAYS,
            crate::store::MAX_ACTIVITY_DAYS,
        );
    top_up_analysis(
        &store,
        &controller,
        now,
        i64::from(activity_window_days),
        |agent, session_id, wsl_distro| async move {
            analysis::locate(agent, &session_id, wsl_distro.as_deref()).await
        },
        |agent, session_id, wsl_distro, claimed, cancel| async move {
            analysis::analyze(agent, &session_id, wsl_distro.as_deref(), claimed, cancel).await
        },
    )
    .await?;

    if controller.cancelled() {
        return Ok(records.len());
    }

    repositories::refresh(app).await?;

    Ok(records.len())
}

/// What one scan pass learned: rows for the index, previously indexable
/// transcripts the sub-agent gate now refuses, and sessions that qualify for
/// a locally generated title.
struct Described {
    records: Vec<SessionRecord>,
    rejected: Vec<SessionKey>,
    summary_candidates: Vec<LocalSummaryCandidate>,
}

/// Read metadata for every discovered log, at a bounded concurrency, and drop
/// the ones the reader opted out of.
#[cfg(test)]
async fn describe(
    logs: Vec<SessionLog>,
    home: &std::path::Path,
    ignored: &std::collections::HashSet<String>,
) -> Described {
    describe_with_states(logs, home, ignored, &std::collections::HashMap::new()).await
}

async fn describe_with_states(
    logs: Vec<SessionLog>,
    home: &std::path::Path,
    ignored: &std::collections::HashSet<String>,
    activity_states: &std::collections::HashMap<SessionActivityKey, SessionActivityState>,
) -> Described {
    let indexed_titles = indexed_titles_for_logs(&logs).await;
    let mut records = Vec::with_capacity(logs.len());
    let mut rejected = Vec::new();
    let mut summary_candidates = Vec::new();
    for chunk in logs.chunks(METADATA_CONCURRENCY) {
        let mut set = JoinSet::new();
        for log in chunk {
            let log = log.clone();
            let home = home.to_path_buf();
            let activity_key = SessionActivityKey::new(
                log.environment.key(),
                log.agent_type.slug(),
                log.source_label(),
            );
            let activity_state = activity_states.get(&activity_key).cloned();
            let indexed_title = recovered_id(&log)
                .and_then(|session_id| indexed_titles.get(&(log.agent_type, session_id)).cloned());
            set.spawn(async move {
                describe_one_with_activity(log, &home, indexed_title, activity_state).await
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((DescribeOutcome::Session(record), candidate)) => {
                    let cwd = record.cwd.as_deref();
                    // The engine's opt-out gate, applied once here so every
                    // surface that reads the store inherits it.
                    if cwd.is_some_and(|cwd| ignored_paths::set_contains(ignored, cwd)) {
                        continue;
                    }
                    if let Some(candidate) = candidate {
                        summary_candidates.push(candidate);
                    }
                    records.push(*record);
                }
                Ok((DescribeOutcome::Subagent(key), _)) => rejected.push(key),
                Ok((DescribeOutcome::Skip, _)) | Err(_) => {}
            }
        }
    }
    Described {
        records,
        rejected,
        summary_candidates,
    }
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

/// Resolve the display timestamp from transcript events while using the
/// persisted aggregate cursor as a cheap unchanged-source gate.
async fn semantic_activity_for_log(
    log: &SessionLog,
    previous: Option<&SessionActivityState>,
    children: &[std::path::PathBuf],
    preview: Option<&str>,
) -> (Option<i64>, String, String) {
    let SessionSource::File(path) = &log.source else {
        return (log.updated_at, "unknown".to_string(), String::new());
    };

    let Some(size) = tokio::fs::metadata(path).await.ok().map(|meta| meta.len()) else {
        return (log.updated_at, "mtime".to_string(), String::new());
    };

    // Include the complete source set in the cursor. Parent + child sizes and
    // identities make an unchanged orchestrator as cheap as a leaf while a
    // child append naturally invalidates the gate.
    let mut cursor_parts = vec![[
        "parent".to_string(),
        path.to_string_lossy().into_owned(),
        size.to_string(),
    ]];
    for child in children {
        let child_size = tokio::fs::metadata(child)
            .await
            .ok()
            .map(|meta| meta.len())
            .map_or_else(|| "missing".to_string(), |size| size.to_string());
        cursor_parts.push([
            "child".to_string(),
            child.to_string_lossy().into_owned(),
            child_size,
        ]);
    }
    cursor_parts.sort_unstable();
    let cursor = serde_json::to_string(&cursor_parts).expect("activity cursor is serializable");

    let unchanged_event = previous
        .is_some_and(|state| state.activity_source == "event" && state.activity_cursor == cursor);
    if unchanged_event {
        let state = previous.expect("checked above");
        return (
            state.updated_at_epoch,
            state.activity_source.clone(),
            cursor,
        );
    }

    // Preserve a known event timestamp before looking at changed content. A
    // size-growing append containing only housekeeping must never fall back to
    // its new mtime or promote an otherwise idle session.
    let mut latest = previous
        .filter(|state| state.activity_source == "event")
        .and_then(|state| state.updated_at_epoch);
    let mut consider = |content: &str| {
        if let Some(epoch) = scanner::max_activity_event_epoch(content, log.agent_type) {
            latest = Some(latest.map_or(epoch, |current| current.max(epoch)));
        }
    };

    // `describe_one_with_activity` already fetched the bounded preview for
    // metadata/title work. Reuse it for normal-sized files; larger files use
    // both the preview and a line-aligned tail so old activity can heal a
    // migrated mtime row even when the tail is housekeeping-only.
    if let Some(preview) = preview {
        consider(preview);
    }
    let preview_is_complete = preview.is_some_and(|content| size <= content.len() as u64);
    if !preview_is_complete && let Some(tail) = session_source_tail(&log.source).await {
        consider(&tail);
    }

    // Child transcripts are separate append-only files. Their semantic work
    // belongs to the orchestrator's activity row; child mtimes are discovery
    // hints only.
    for child in children {
        let child_source = SessionSource::File(child.clone());
        if let Some(epoch) = session_source_tail(&child_source)
            .await
            .and_then(|tail| scanner::max_activity_event_epoch(&tail, log.agent_type))
        {
            latest = Some(latest.map_or(epoch, |current| current.max(epoch)));
        }
    }

    match latest {
        Some(epoch) => (Some(epoch), "event".to_string(), cursor),
        None => (log.updated_at, "mtime".to_string(), cursor),
    }
}

#[cfg(test)]
async fn describe_one(
    log: SessionLog,
    home: &std::path::Path,
    indexed_title: Option<ResolvedTitle>,
) -> DescribeOutcome {
    describe_one_with_activity(log, home, indexed_title, None)
        .await
        .0
}

async fn describe_one_with_activity(
    log: SessionLog,
    home: &std::path::Path,
    indexed_title: Option<ResolvedTitle>,
    activity_state: Option<SessionActivityState>,
) -> (DescribeOutcome, Option<LocalSummaryCandidate>) {
    let read = session_log_read(&log).await;
    let metadata = read.as_ref().map(|read| &read.metadata);
    let Some(session_id) = metadata
        .and_then(|metadata| metadata.session_id.clone())
        .or_else(|| recovered_id(&log))
    else {
        return (DescribeOutcome::Skip, None);
    };
    if session_id.is_empty() {
        return (DescribeOutcome::Skip, None);
    }

    let key = SessionKey::new(
        log.environment.key(),
        log.agent_type.slug(),
        session_id.clone(),
    );
    let preview = match log.agent_type {
        AgentKind::Claude | AgentKind::Codex => {
            read.as_ref().and_then(|read| read.content.as_deref())
        }
        _ => None,
    };
    if is_subagent_transcript(&log, preview) {
        return (DescribeOutcome::Subagent(key), None);
    }

    let resolved_title = if should_lookup_indexed_title(&log) {
        indexed_title
    } else {
        None
    };
    let (title, title_source) = select_title_pair(
        resolved_title,
        metadata.and_then(|metadata| metadata.title.clone()),
        metadata.and_then(|metadata| metadata.title_source),
        &log.agent_type,
        preview,
    );
    let summary_candidate = local_summary_candidate(
        &log,
        &key,
        metadata.and_then(|metadata| metadata.cwd.as_deref()),
        preview,
        title_source.as_deref(),
    );

    // A dir listing per orchestrator-capable session; vendors that record no
    // orchestration return empty without touching the disk.
    let children = match &log.source {
        SessionSource::File(path)
            if matches!(log.agent_type, AgentKind::Claude | AgentKind::Codex) =>
        {
            Explorers::DISK
                .list_subagents_for_transcript(&log.agent_type, path)
                .await
        }
        _ => Vec::new(),
    };
    let subagent_count = children.len() as u32;
    let fork_parent_session_id = preview.and_then(analysis::fork_parent_from_content);

    let (updated_at_epoch, activity_source, activity_cursor) =
        semantic_activity_for_log(&log, activity_state.as_ref(), &children, preview).await;
    let descriptor = SourceDescriptor {
        agent: log.agent_type,
        session_id: session_id.clone(),
        environment: log.environment.clone(),
        source: log.source.clone(),
        updated_at_epoch: log.updated_at,
    };
    let source_fingerprint = Explorers::DISK
        .source_version(&descriptor, read.as_ref())
        .await
        .map(|version| version.fingerprint);

    let record = DescribeOutcome::Session(Box::new(SessionRecord {
        key,
        source_kind: source_kind(&log.source).to_string(),
        source_label: log.source_label(),
        wsl_distro: log.environment.wsl_distro().map(str::to_string),
        title,
        title_source,
        cwd: metadata.and_then(|metadata| metadata.cwd.clone()),
        surface: log.surface_label(home).to_string(),
        updated_at_epoch,
        activity_cursor,
        activity_source,
        subagent_count,
        fork_parent_session_id,
        source_fingerprint,
    }));
    (record, summary_candidate)
}

/// User messages after the first that travel to the summarizer as context. A
/// few early prompts improve the title; the whole transcript would not.
const LOCAL_SUMMARY_CONTEXT_MESSAGES: usize = 3;

/// True when a user-role turn holds text that Codex injects, not text the
/// user wrote. Codex records project instructions ("# AGENTS.md instructions
/// for …") and synthetic elements ("<environment_context>…") as user turns.
/// The check is prefix-only because the extractor truncates long turns.
/// A real prompt that starts with "<" is rare, and the cost of a skip is
/// small: the summarizer anchors on the next user message instead.
fn is_injected_codex_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('<') || trimmed.starts_with("# AGENTS.md instructions")
}

/// Collect a candidate for local title generation. Only a native Codex
/// session stuck on the `firstMessage` fallback qualifies; every other
/// provenance already carries a better name.
fn local_summary_candidate(
    log: &SessionLog,
    key: &SessionKey,
    cwd: Option<&str>,
    preview: Option<&str>,
    title_source: Option<&str>,
) -> Option<LocalSummaryCandidate> {
    if title_source != Some(TitleSource::FirstMessage.as_str()) {
        return None;
    }
    if !matches!(log.agent_type, AgentKind::Codex) || !log.environment.is_native() {
        return None;
    }
    let mut messages = scanner::user_message_titles_from_content(preview?);
    messages.retain(|message| !is_injected_codex_text(message));
    if messages.is_empty() {
        return None;
    }
    let first_message = messages.remove(0);
    messages.truncate(LOCAL_SUMMARY_CONTEXT_MESSAGES);
    let repo = cwd
        .map(std::path::Path::new)
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    Some(LocalSummaryCandidate {
        key: key.clone(),
        input: TitleInput {
            repo,
            first_message,
            context: messages,
        },
    })
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
                .and_then(|title| scanner::clean_first_message_title(&title))
        } else {
            Some(resolved.text)
        };
        let source = title.as_ref().map(|_| source.as_str().to_string());
        return (title, source);
    }

    let title = sanitized_title(fallback_title, agent, preview);
    // A first-message fallback is raw prose. Clean it for display; the
    // provenance stays `firstMessage`.
    let title = if fallback_source == Some(TitleSource::FirstMessage) {
        title.and_then(|title| scanner::clean_first_message_title(&title))
    } else {
        title
    };
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

/// Per-agent `(slug, sessions seen, newest activity)` for the scan-state table.
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
async fn top_up_analysis<F, Fut, A, AFut>(
    store: &Store,
    controller: &ScanController,
    now: i64,
    activity_days: i64,
    mut locate: F,
    mut analyze: A,
) -> anyhow::Result<()>
where
    F: FnMut(AgentKind, String, Option<String>) -> Fut,
    Fut: std::future::Future<Output = Option<SessionSource>>,
    A: FnMut(
        AgentKind,
        String,
        Option<String>,
        analysis::ClaimedSource,
        analysis::CancelFlag,
    ) -> AFut,
    AFut: std::future::Future<Output = analysis::SessionAnalysis>,
{
    let since = now - activity_days.max(1) * 86_400;
    let candidates = store.recent_sessions(since, MAX_ANALYSES_PER_PASS)?;

    for record in candidates {
        // Analysis is the long tail of a pass — one whole transcript read per
        // session — so this is where a cancel is felt.
        if controller.cancelled() {
            return Ok(());
        }
        let Some(agent) = crate::agents::kind_from_slug(&record.key.agent) else {
            continue;
        };
        if !analysis::analysis_supported(agent) {
            // A generically-parsed transcript would produce a half-confident
            // metric; the view says so instead of showing one.
            continue;
        }
        let Some(source) = locate(
            agent,
            record.key.session_id.clone(),
            record.wsl_distro.clone(),
        )
        .await
        else {
            continue;
        };
        let fingerprint = analysis::fingerprint_with_subagents(
            agent,
            &record.key.session_id,
            record.wsl_distro.as_deref(),
            &source,
        )
        .await;
        if let Some(cached) = store.analysis(&record.key)?
            && analysis::cache_is_fresh(&cached, &fingerprint)
        {
            continue;
        }

        let source_state = store.session_source_state(&record.key)?;
        let claimed = source_state
            .map(|state| analysis::ClaimedSource {
                fingerprint: state.source_fingerprint,
                generation: state.source_generation,
            })
            .unwrap_or(analysis::ClaimedSource {
                fingerprint: None,
                generation: 0,
            });
        let analysis = analyze(
            agent,
            record.key.session_id.clone(),
            record.wsl_distro.clone(),
            claimed,
            controller.cancel_flag(),
        )
        .await;
        if let Some(cache) = analysis.record(&record.key) {
            store.save_analysis(&cache, analysis.started_at_epoch)?;
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
    use std::io::Write;

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

    fn write_opencode_provider_db(home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
        let path = home.join("opencode.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                     id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT,
                     directory TEXT NOT NULL, title TEXT NOT NULL, version TEXT NOT NULL,
                     time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
                 );
                 CREATE TABLE message (
                     id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                     time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
                 );
                 CREATE TABLE part (
                     id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
                     time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES (?1, 'synthetic-project', NULL, '/repo',
                                              'Synthetic session', '1', 100, 120, '{}')",
                [session_id],
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

    fn write_codex_fork_session(
        home: &std::path::Path,
        session_id: &str,
        parent_session_id: &str,
    ) -> std::path::PathBuf {
        let path = write_codex_session(home, session_id);
        let header = serde_json::json!({
            "timestamp": "2026-08-01T10:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "forked_from_id": parent_session_id,
                "cwd": "/home/avery/code/gadgets",
                "source": "cli",
                "thread_source": "user",
            }
        });
        std::fs::write(&path, format!("{header}\n")).unwrap();
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
    fn first_message_titles_are_cleaned_for_display() {
        // A resolved first-message title loses its attachment marker and
        // truncates at a word boundary; the provenance stays firstMessage.
        let resolved = select_title_pair(
            Some(ResolvedTitle::new(
                "[Image #1] in this pane, it should be possible to click on the claude/codex/whatever section",
                TitleSource::FirstMessage,
            )),
            None,
            None,
            &AgentKind::Codex,
            None,
        );
        let (title, source) = resolved;
        let title = title.expect("cleaned title");
        assert!(title.starts_with("In this pane, it should be possible"));
        assert!(title.ends_with('…'));
        assert_eq!(source.as_deref(), Some("firstMessage"));

        // The transcript-fallback arm cleans too.
        let fallback = select_title_pair(
            None,
            Some("[Pasted text #2 +12 lines] please review this".into()),
            Some(TitleSource::FirstMessage),
            &AgentKind::Codex,
            None,
        );
        assert_eq!(
            fallback,
            (
                Some("Please review this".into()),
                Some("firstMessage".into())
            )
        );

        // An explicit-source fallback stays untouched.
        let explicit = select_title_pair(
            None,
            Some("[Image #1] literal explicit title. Second sentence.".into()),
            Some(TitleSource::Explicit),
            &AgentKind::Codex,
            None,
        );
        assert_eq!(
            explicit.0.as_deref(),
            Some("[Image #1] literal explicit title. Second sentence.")
        );
    }

    #[test]
    fn only_native_codex_first_message_sessions_become_summary_candidates() {
        let codex = log(
            AgentKind::Codex,
            std::path::PathBuf::from("/tmp/rollout.jsonl"),
            1_800_000_000,
        );
        let key = SessionKey::new("native", "codex", "session-1");
        let preview = concat!(
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"[Image #1] make the pane clickable"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"also fix the hover state"}]}}"#,
            "\n",
        );

        let candidate = local_summary_candidate(
            &codex,
            &key,
            Some("/home/avery/code/gadgets"),
            Some(preview),
            Some("firstMessage"),
        )
        .expect("a native codex fallback title qualifies");
        assert_eq!(candidate.key, key);
        assert_eq!(candidate.input.repo.as_deref(), Some("gadgets"));
        assert!(
            candidate
                .input
                .first_message
                .contains("make the pane clickable")
        );
        assert_eq!(
            candidate.input.context,
            vec!["also fix the hover state".to_string()]
        );

        // A better provenance never becomes a candidate.
        assert!(
            local_summary_candidate(&codex, &key, None, Some(preview), Some("userRename"))
                .is_none()
        );
        // Other agents keep their existing title chain.
        let claude = log(
            AgentKind::Claude,
            std::path::PathBuf::from("/tmp/session.jsonl"),
            1_800_000_000,
        );
        assert!(
            local_summary_candidate(&claude, &key, None, Some(preview), Some("firstMessage"))
                .is_none()
        );
        // A WSL session must not receive a native-store generated title.
        let in_wsl = SessionLog {
            environment: DiscoveryEnvironment::Wsl {
                distribution: "SyntheticLinux".into(),
                user: "avery".into(),
            },
            ..codex
        };
        assert!(
            local_summary_candidate(&in_wsl, &key, None, Some(preview), Some("firstMessage"))
                .is_none()
        );
    }

    #[test]
    fn summary_candidates_skip_injected_codex_turns() {
        let codex = log(
            AgentKind::Codex,
            std::path::PathBuf::from("/tmp/rollout.jsonl"),
            1_800_000_000,
        );
        let key = SessionKey::new("native", "codex", "session-1");
        // Codex records project instructions and environment context as
        // user-role turns before the first real prompt.
        let preview = concat!(
            r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /home/avery/code/gadgets\n\n<INSTRUCTIONS>\nUse the makefile.\n</INSTRUCTIONS>"}]}}"##,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/home/avery/code/gadgets</cwd>\n</environment_context>"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"make the pane clickable"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"also fix the hover state"}]}}"#,
            "\n",
        );

        let candidate = local_summary_candidate(
            &codex,
            &key,
            Some("/home/avery/code/gadgets"),
            Some(preview),
            Some("firstMessage"),
        )
        .expect("the real prompt still qualifies");
        assert_eq!(candidate.input.first_message, "make the pane clickable");
        assert_eq!(
            candidate.input.context,
            vec!["also fix the hover state".to_string()]
        );

        // A session with only injected turns yields no candidate.
        let injected_only = concat!(
            r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /home/avery/code/gadgets"}]}}"##,
            "\n",
        );
        assert!(
            local_summary_candidate(
                &codex,
                &key,
                None,
                Some(injected_only),
                Some("firstMessage")
            )
            .is_none()
        );
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
    async fn a_codex_fork_records_its_parent_during_the_scan() {
        let home = tempfile::TempDir::new().unwrap();
        let parent_session_id = "parent-session";
        let child_session_id = "child-session";
        let path = write_codex_fork_session(home.path(), child_session_id, parent_session_id);

        let DescribeOutcome::Session(child) = describe_one(
            log(AgentKind::Codex, path, 1_800_000_000),
            home.path(),
            None,
        )
        .await
        else {
            panic!("Codex fork should be described");
        };
        assert_eq!(
            child.fork_parent_session_id.as_deref(),
            Some(parent_session_id)
        );

        let store = crate::store::Store::open_in_memory(home.path()).unwrap();
        store
            .upsert_sessions(&[record("codex", parent_session_id, Some(1_799_999_000))])
            .unwrap();
        store.upsert_sessions(std::slice::from_ref(&child)).unwrap();

        assert_eq!(
            store
                .fork_children(&SessionKey::new("native", "codex", parent_session_id))
                .unwrap(),
            vec![child_session_id.to_string()]
        );
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
        assert!(
            claude
                .source_fingerprint
                .as_deref()
                .is_some_and(|value| value.starts_with("sv1:"))
        );

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
    async fn describing_a_claude_session_reads_the_head_once() {
        let home = tempfile::TempDir::new().unwrap();
        let path = write_claude_session(home.path(), "one-head-read");
        antiburn_local::discovery::track_head_reads(&path);

        let outcome = describe_one(
            log(AgentKind::Claude, path.clone(), 1_800_000_000),
            home.path(),
            None,
        )
        .await;

        assert!(matches!(outcome, DescribeOutcome::Session(_)));
        assert_eq!(antiburn_local::discovery::take_tracked_head_reads(&path), 1);
    }

    #[tokio::test]
    async fn describing_an_opencode_provider_db_does_not_render_the_transcript() {
        let home = tempfile::TempDir::new().unwrap();
        let session_id = "opencode-provider-db";
        let db_path = write_opencode_provider_db(home.path(), session_id);
        let log = SessionLog {
            agent_type: AgentKind::OpenCode,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: db_path.clone(),
                session_id: session_id.to_string(),
            },
            updated_at: Some(120),
            environment: DiscoveryEnvironment::Native,
        };
        antiburn_local::discovery::track_provider_db_renders(&db_path);

        let outcome = describe_one(log, home.path(), None).await;

        assert!(matches!(outcome, DescribeOutcome::Session(_)));
        assert_eq!(
            antiburn_local::discovery::take_tracked_provider_db_renders(&db_path),
            0
        );
    }

    #[tokio::test]
    async fn a_consumed_provider_db_preview_is_rendered() {
        let home = tempfile::TempDir::new().unwrap();
        let session_id = "consumed-provider-db";
        let db_path = write_opencode_provider_db(home.path(), session_id);
        let log = SessionLog {
            agent_type: AgentKind::Claude,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: db_path.clone(),
                session_id: session_id.to_string(),
            },
            updated_at: Some(120),
            environment: DiscoveryEnvironment::Native,
        };
        antiburn_local::discovery::track_provider_db_renders(&db_path);

        let read = session_log_read(&log).await.expect("source read");

        assert!(read.content.is_some());
        assert_eq!(
            antiburn_local::discovery::take_tracked_provider_db_renders(&db_path),
            1
        );
    }

    #[tokio::test]
    async fn an_inline_claude_subagent_is_rejected_on_the_scan_path() {
        let content = concat!(
            r#"{"type":"user","sessionId":"inline-subagent","isSidechain":true,"agentId":"agent-child","message":{"role":"user","content":"Investigate the failed deployment"}}"#,
            "\n",
        );
        let log = SessionLog {
            agent_type: AgentKind::Claude,
            source: SessionSource::Inline {
                label: "inline-subagent".to_string(),
                content: content.to_string(),
            },
            updated_at: Some(1_800_000_000),
            environment: DiscoveryEnvironment::Native,
        };

        assert!(matches!(
            describe_one(log, std::path::Path::new("/tmp"), None).await,
            DescribeOutcome::Subagent(key)
                if key == SessionKey::new("native", "claude-code", "inline-subagent")
        ));
    }

    #[tokio::test]
    async fn a_descriptor_takes_the_metadata_session_id() {
        let home = tempfile::TempDir::new().unwrap();
        let path = home.path().join("recovered-file-name.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"user","sessionId":"metadata-id","cwd":"/repo"}
"#,
        )
        .unwrap();

        let DescribeOutcome::Session(record) =
            describe_one(log(AgentKind::Claude, path, 100), home.path(), None).await
        else {
            panic!("session should be described");
        };

        assert_eq!(record.key.session_id, "metadata-id");
        assert!(record.source_fingerprint.is_some());
    }

    #[tokio::test]
    async fn a_descriptor_falls_back_to_the_recovered_id() {
        let home = tempfile::TempDir::new().unwrap();
        let path = home.path().join("recovered-id.jsonl");
        std::fs::write(&path, "not json\n").unwrap();

        let DescribeOutcome::Session(record) =
            describe_one(log(AgentKind::Pi, path, 100), home.path(), None).await
        else {
            panic!("session should be described");
        };

        assert_eq!(record.key.session_id, "recovered-id");
        assert!(record.source_fingerprint.is_some());
    }

    #[tokio::test]
    async fn an_empty_session_id_is_skipped() {
        let log = SessionLog {
            agent_type: AgentKind::Claude,
            source: SessionSource::Inline {
                label: String::new(),
                content: "{}".to_string(),
            },
            updated_at: None,
            environment: DiscoveryEnvironment::Native,
        };

        assert!(matches!(
            describe_one(log, std::path::Path::new("/tmp"), None).await,
            DescribeOutcome::Skip
        ));
    }

    #[tokio::test]
    async fn an_appended_transcript_produces_a_different_fingerprint() {
        let home = tempfile::TempDir::new().unwrap();
        let path = write_claude_session(home.path(), "changing-session");
        let DescribeOutcome::Session(first) =
            describe_one(log(AgentKind::Claude, path.clone(), 100), home.path(), None).await
        else {
            panic!("session should be described");
        };
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":\"assistant\"}\n")
            .unwrap();
        let DescribeOutcome::Session(second) =
            describe_one(log(AgentKind::Claude, path, 101), home.path(), None).await
        else {
            panic!("session should be described");
        };

        assert_ne!(first.source_fingerprint, second.source_fingerprint);
    }

    #[tokio::test]
    async fn a_non_native_codex_title_survives_the_scan_path() {
        let home = tempfile::TempDir::new().unwrap();
        let session_id = "wsl-indexed-title";
        let path = write_codex_session(home.path(), session_id);
        std::fs::write(
            home.path().join(".codex/session_index.jsonl"),
            format!(
                r#"{{"id":"{session_id}","thread_name":"Indexed WSL title"}}
"#
            ),
        )
        .unwrap();
        let log = SessionLog {
            environment: DiscoveryEnvironment::Wsl {
                distribution: "SyntheticLinux".into(),
                user: "avery".into(),
            },
            ..log(AgentKind::Codex, path, 100)
        };

        let DescribeOutcome::Session(record) = describe_one(log, home.path(), None).await else {
            panic!("session should be described");
        };

        assert_eq!(record.title.as_deref(), Some("Indexed WSL title"));
        assert_eq!(record.title_source.as_deref(), Some("aiGenerated"));
    }

    #[tokio::test]
    async fn a_wsl_cwd_is_mapped_to_a_windows_path_in_the_scan_path() {
        let home = tempfile::TempDir::new().unwrap();
        let path = write_codex_session(home.path(), "wsl-cwd");
        let log = SessionLog {
            environment: DiscoveryEnvironment::Wsl {
                distribution: "SyntheticLinux".into(),
                user: "avery".into(),
            },
            ..log(AgentKind::Codex, path, 100)
        };

        let DescribeOutcome::Session(record) = describe_one(log, home.path(), None).await else {
            panic!("session should be described");
        };

        assert_eq!(
            record.cwd.as_deref(),
            Some(r"\\wsl.localhost\SyntheticLinux\home\avery\code\gadgets")
        );
    }

    #[tokio::test]
    async fn cache_freshness_ignores_the_session_source_fingerprint() {
        let home = tempfile::TempDir::new().unwrap();
        let session_id = "cache-contract-session";
        let source = SessionSource::File(write_claude_session(home.path(), session_id));
        let mut session = record("claude-code", session_id, Some(100));
        session.source_fingerprint = Some("sv1:session-source".to_string());
        let SessionSource::File(source_path) = &source else {
            unreachable!("the fixture uses a file source");
        };
        session.source_label = source_path.to_string_lossy().into_owned();
        let store = Store::open_in_memory(home.path()).unwrap();
        store
            .upsert_sessions(std::slice::from_ref(&session))
            .unwrap();

        let legacy_fingerprint =
            analysis::fingerprint_with_subagents(AgentKind::Claude, session_id, None, &source)
                .await;
        let cached = crate::store::AnalysisRecord {
            key: session.key.clone(),
            model_breakdown_json: r#"{"cached":true}"#.to_string(),
            inclusive_models_json: "[]".to_string(),
            source_fingerprint: legacy_fingerprint.clone(),
            pricing_generation: antiburn_local::analysis::pricing_generation() as i64,
            analyzed_generation: 0,
            parser_revision: 0,
            analyzer_revision: 0,
            metrics_schema_revision: 0,
        };
        store.save_analysis(&cached, None).unwrap();

        let persisted_session = store
            .session(&session.key)
            .unwrap()
            .expect("persisted session");
        assert_eq!(
            persisted_session.source_fingerprint.as_deref(),
            Some("sv1:session-source")
        );
        assert!(analysis::cache_is_fresh(
            &store
                .analysis(&session.key)
                .unwrap()
                .expect("persisted analysis"),
            &legacy_fingerprint
        ));

        let locate_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_locates = locate_calls.clone();
        let analysis_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_analyses = analysis_calls.clone();
        let located_source = source.clone();
        let expected_id = session_id.to_string();
        top_up_analysis(
            &store,
            &ScanController::default(),
            200,
            1,
            move |agent, candidate_id, wsl_distro| {
                observed_locates.fetch_add(1, Ordering::SeqCst);
                let source = located_source.clone();
                let matches = agent == AgentKind::Claude
                    && candidate_id == expected_id
                    && wsl_distro.is_none();
                async move { matches.then_some(source) }
            },
            move |_, _, _, _, _| {
                observed_analyses.fetch_add(1, Ordering::SeqCst);
                async { analysis::SessionAnalysis::unavailable() }
            },
        )
        .await
        .unwrap();

        assert_eq!(locate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(analysis_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.analysis(&session.key).unwrap().as_ref(),
            Some(&cached)
        );
    }

    #[tokio::test]
    async fn top_up_analysis_analyzes_a_candidate_whose_source_changed_one_second_ago() {
        let home = tempfile::TempDir::new().unwrap();
        let store = Store::open_in_memory(home.path()).unwrap();
        let mut session = record("claude-code", "recent-change", Some(100));
        session.source_fingerprint = Some("sv1:recent".to_string());
        store.upsert_sessions(&[session]).unwrap();
        let analysis_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&analysis_calls);

        top_up_analysis(
            &store,
            &ScanController::default(),
            101,
            1,
            |_, _, _| async {
                Some(SessionSource::Inline {
                    label: "recent-change".to_string(),
                    content: String::new(),
                })
            },
            move |_, _, _, _, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                async { analysis::SessionAnalysis::unavailable() }
            },
        )
        .await
        .unwrap();

        assert_eq!(analysis_calls.load(Ordering::SeqCst), 1);
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
            "newest activity first"
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
    async fn an_idle_touched_transcript_heals_mtime_recency_and_then_uses_size_gate() {
        let home = tempfile::TempDir::new().unwrap();
        let path = home
            .path()
            .join(".claude/projects/-home-avery-code-widgets/old.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","sessionId":"old","cwd":"/home/avery/code/widgets","timestamp":"2026-06-26T21:20:00Z"}"#,
                "\n",
                r#"{"type":"custom-title","customTitle":"Renamed","timestamp":"2026-08-19T17:07:32Z"}"#,
                "\n",
                r#"{"type":"permission-mode","mode":"default","timestamp":"2026-08-19T17:07:33Z"}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-06-26T21:30:15Z"}"#,
                "\n",
            ),
        )
        .unwrap();
        let store = crate::store::Store::open_in_memory(home.path()).unwrap();

        // Append a housekeeping record larger than the bounded tail. The
        // preview still contains the old activity and should heal this
        // migrated mtime row on its first semantic scan.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(
                format!(
                    r#"{{"type":"permission-mode","mode":"default","timestamp":"2026-08-19T17:08:00Z","padding":"{}"}}
"#,
                    "x".repeat(300_000)
                )
                .as_bytes(),
            )
            .unwrap();

        // Simulate a row written by the old mtime-based scanner. The semantic
        // pass must replace it with the old meaningful transcript activity.
        let mut stale = record("claude-code", "old", Some(1_787_155_652));
        stale.source_label = path.to_string_lossy().into_owned();
        stale.activity_cursor = "legacy".into();
        stale.activity_source = "mtime".into();
        store.upsert_sessions(&[stale]).unwrap();

        let states = store.session_activity_states().unwrap();
        let described = describe_with_states(
            vec![log(AgentKind::Claude, path.clone(), 1_787_155_652)],
            home.path(),
            &HashSet::new(),
            &states,
        )
        .await;
        let expected = time::OffsetDateTime::parse(
            "2026-06-26T21:30:15Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
        .unix_timestamp();
        assert_eq!(described.records[0].updated_at_epoch, Some(expected));
        assert_eq!(described.records[0].activity_source, "event");
        store.upsert_sessions(&described.records).unwrap();
        let stored = store
            .session_activity_states()
            .unwrap()
            .remove(&SessionActivityKey::new(
                "native",
                AgentKind::Claude.slug(),
                path.to_string_lossy().into_owned(),
            ))
            .expect("healed activity cursor");
        assert_eq!(stored.updated_at_epoch, Some(expected));
        assert_eq!(stored.activity_source, "event");

        // A harness appends housekeeping only. The changed size invalidates
        // the cursor, but the previous event seed survives the suffix parse
        // and prevents the new mtime from promoting the session.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(
                format!(
                    r#"{{"type":"permission-mode","mode":"default","timestamp":"2026-08-19T17:08:00Z","padding":"{}"}}
"#,
                    "x".repeat(300_000)
                )
                .as_bytes(),
            )
            .unwrap();
        let states = store.session_activity_states().unwrap();
        let touched = describe_with_states(
            vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
            home.path(),
            &HashSet::new(),
            &states,
        )
        .await;
        assert_eq!(touched.records[0].updated_at_epoch, Some(expected));
        assert_eq!(touched.records[0].activity_source, "event");

        // A later mtime-only touch now hits the unchanged-size cursor gate.
        let states = {
            store.upsert_sessions(&touched.records).unwrap();
            store.session_activity_states().unwrap()
        };
        let gated = describe_with_states(
            vec![log(AgentKind::Claude, path, 1_800_000_001)],
            home.path(),
            &HashSet::new(),
            &states,
        )
        .await;
        assert_eq!(gated.records[0].updated_at_epoch, Some(expected));
    }

    #[tokio::test]
    async fn an_orchestrator_cursor_gates_unchanged_children_and_advances_on_child_growth() {
        let home = tempfile::TempDir::new().unwrap();
        let parent = write_claude_session(home.path(), "orchestrator");
        let child_dir = parent
            .parent()
            .unwrap()
            .join("orchestrator")
            .join("subagents");
        std::fs::create_dir_all(&child_dir).unwrap();
        let child = child_dir.join("agent-child.jsonl");
        std::fs::write(
            &child,
            r#"{"type":"assistant","timestamp":"2026-08-01T10:02:00Z"}
"#,
        )
        .unwrap();
        let store = crate::store::Store::open_in_memory(home.path()).unwrap();

        let first = describe_with_states(
            vec![log(AgentKind::Claude, parent.clone(), 1_800_000_000)],
            home.path(),
            &HashSet::new(),
            &store.session_activity_states().unwrap(),
        )
        .await;
        assert_eq!(first.records[0].subagent_count, 1);
        let first_epoch = first.records[0].updated_at_epoch.unwrap();
        store.upsert_sessions(&first.records).unwrap();

        // An mtime-only parent touch with an unchanged parent+child cursor is
        // served from the cached semantic event without reading either tail.
        let gated = describe_with_states(
            vec![log(AgentKind::Claude, parent.clone(), 1_900_000_000)],
            home.path(),
            &HashSet::new(),
            &store.session_activity_states().unwrap(),
        )
        .await;
        assert_eq!(gated.records[0].updated_at_epoch, Some(first_epoch));

        // Appending genuine child work changes the aggregate cursor and
        // promotes the parent to the child's semantic event time.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&child)
            .unwrap()
            .write_all(
                br#"{"type":"assistant","timestamp":"2026-08-01T10:03:00Z"}
"#,
            )
            .unwrap();
        let advanced = describe_with_states(
            vec![log(AgentKind::Claude, parent, 1_900_000_001)],
            home.path(),
            &HashSet::new(),
            &store.session_activity_states().unwrap(),
        )
        .await;
        let expected = time::OffsetDateTime::parse(
            "2026-08-01T10:03:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
        .unix_timestamp();
        assert_eq!(advanced.records[0].updated_at_epoch, Some(expected));
        assert!(advanced.records[0].updated_at_epoch.unwrap() > first_epoch);
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
            activity_cursor: String::new(),
            activity_source: "mtime".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    #[test]
    fn per_agent_totals_count_sessions_and_keep_the_newest_activity() {
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
    fn the_scheduled_trigger_set_is_unchanged() {
        for wake in [Wake::Launch, Wake::Kick] {
            assert!(should_run_scheduled_pass(wake, false, true));
            assert!(should_run_scheduled_pass(wake, true, true));
            assert!(!should_run_scheduled_pass(wake, true, false));
        }
        assert!(should_run_scheduled_pass(Wake::Tick, true, true));
        assert!(!should_run_scheduled_pass(Wake::Tick, false, true));
        assert!(!should_run_scheduled_pass(Wake::Tick, true, false));
    }

    #[test]
    fn an_on_demand_pass_starts_without_the_scheduler_gate() {
        let controller = ScanController::default();
        assert!(on_demand_start(&controller));
        assert!(!on_demand_start(&controller));
        controller.running.store(false, Ordering::SeqCst);
        assert!(on_demand_start(&controller));
    }

    #[test]
    fn a_cloned_cancel_flag_observes_request_cancel_and_the_pass_reset() {
        let controller = ScanController::default();
        let flag = controller.cancel_flag();
        controller.running.store(true, Ordering::SeqCst);
        controller.request_cancel();
        controller.running.store(false, Ordering::SeqCst);
        assert!(flag.cancelled());

        assert!(on_demand_start(&controller));
        assert!(!flag.cancelled());
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
