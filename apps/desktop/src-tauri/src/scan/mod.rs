//! The background scan: what antiburn knows about this machine, refreshed.
//!
//! # Scan policy
//!
//! One pass asks every agent explorer for the sessions it can see, reads each
//! one's metadata, writes the result to the store, and refreshes the
//! repository list. It does not analyze sessions: [`crate::insights_worker`]
//! drains that work from its own durable queue. Passes never overlap: a
//! request that arrives while one is running is dropped rather than queued,
//! because the next tick would produce the same answer.
//!
//! antiburn is an always-running background utility. CPU time, memory, open
//! files, and disk I/O are therefore correctness constraints, not optional
//! optimizations: a scan must do no more work, retain no more data in memory,
//! and run no more often than the visible feature requires. A pass describes
//! a file-backed session again only when its parent and child sizes have
//! changed since the stored record; an unchanged source is reused without
//! opening its transcript. This is what makes the unconditional tick in the
//! next section affordable.
//!
//! When a pass runs:
//!
//! - **At launch**, once, if onboarding is finished. A first-run install has no
//!   sources selected yet, so scanning before the flow completes would only
//!   spend disk on a window nobody can see.
//! - **Every [`TICK`], unconditionally.** The popover does not gate the timer:
//!   a pass over sources that have not changed reads no transcript, so ticking
//!   while the popover is hidden costs stat calls, not disk reads.
//! - **The moment the popover is opened**, as reconciliation, so a reader never
//!   waits out a tick behind a missed event.
//! - **On demand**, from the rescan control and after any change to the source
//!   selection.
//! - **Shortly after a watched file changes.** [`watch::spawn_watcher`] starts
//!   an OS filesystem watcher over every agent's watch roots
//!   (`AgentExplorer::watch_roots`), debounces the events it sees, and
//!   requests a pass after a quiet period — see the `watch` module doc for
//!   the debounce shape. [`TICK`] stays as reconciliation for what the
//!   watcher cannot cover (a root the OS refuses to watch, a change the
//!   debounce window swallowed). When the watcher is not fully healthy the
//!   scheduler ticks at [`watch::FALLBACK_TICK`] instead, so polling alone
//!   still finds new sessions at a reasonable cadence.
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
//! Every pass is bounded: discovery is windowed to the widest activity view,
//! and the per-session metadata reads run at a fixed concurrency, so one pass
//! cannot grow with the size of the machine. [`crate::insights_worker`] bounds
//! its own analysis concurrency separately. The separate retention policy
//! expires indexed sessions; the bounded discovery window does not. The
//! scheduler is a single handle the app aborts on exit, so nothing outlives
//! the process.

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
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::agents;
use crate::analysis;
use crate::commands;
use crate::dto::{ActivityEntry, ScanStatus};
use crate::repositories;
use crate::storage_health::{self, checked};
use crate::store::{SessionActivityKey, SessionKey, SessionRecord, Store};

pub mod watch;

/// How often the scheduler wakes up.
pub const TICK: Duration = Duration::from_secs(60);

/// How many session logs have their metadata read at once. Bounds open files
/// and blocking-pool pressure during a whole-machine pass.
const METADATA_CONCURRENCY: usize = 16;

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
    cancel: Arc<AtomicBool>,
    status: Mutex<ScanStatus>,
    kick: Notify,
}

impl ScanController {
    /// Ask for a pass as soon as the scheduler can start one.
    pub fn request(&self) {
        self.kick.notify_one();
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
        crate::runtime_pricing::wait_until_ready(&app).await;
        // The watcher starts here, before the launch pass: a session written
        // between this line and the first tick still reaches the debouncer.
        let tick = tick_for(&watch::spawn_watcher(&app));
        // A fresh install has nothing to scan until the reader picks sources.
        if scheduled_scanning_allowed(&app) {
            run_pass(&app, None).await;
        }
        loop {
            let controller = app.state::<ScanController>();
            tokio::select! {
                () = controller.kick.notified() => {}
                () = tokio::time::sleep(tick) => {}
            }
            // Checked after the wake-up rather than before the wait, so
            // resuming discovery takes effect at the next request or tick
            // instead of needing the app restarted.
            if !scheduled_scanning_allowed(&app) {
                continue;
            }
            run_pass(&app, None).await;
        }
    })
}

/// The scheduler's fixed poll interval for this run, chosen once from the
/// watcher's start-up status: [`TICK`] when it started clean, or
/// [`watch::FALLBACK_TICK`] when it did not start or could not watch every
/// existing root. The watcher's own periodic re-check (see the `watch`
/// module doc) keeps covering roots that appear later regardless of which
/// tick the scheduler picked here.
fn tick_for(status: &watch::WatcherStatus) -> Duration {
    if status.is_healthy() {
        TICK
    } else {
        watch::FALLBACK_TICK
    }
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
            status.list_changed = false;
            status.error = None;
            status.cancelled = false;
        });
        let _ = app.emit(EVENT_STARTED, started);
    }

    let announce_app = app.clone();
    let announce = move |entry: ActivityEntry| {
        let _ = announce_app.emit(commands::SESSION_ENTRY_CHANGED_EVENT, &entry);
    };
    let outcome = pass(app, activity_window_days, &announce).await;

    let controller = app.state::<ScanController>();
    let cancelled = controller.cancelled();
    let finished = controller.update(|status| {
        status.running = false;
        status.cancelled = cancelled;
        status.finished_at = Some(crate::store::now_rfc3339());
        match &outcome {
            Ok(summary) => {
                status.sessions = summary.sessions;
                status.list_changed = summary.list_changed;
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
    crate::analytics::record_scan(
        app,
        outcome.as_ref().ok().map(|summary| summary.sessions as u64),
    );
    crate::notifications::note_scan_outcome(app, &finished);
    finished
}

/// What one pass persisted, for [`run_pass`] to fold into the reported status.
struct PassSummary {
    sessions: usize,
    /// True when a reader's list needs a full refetch rather than a row
    /// patch: this pass indexed a session the list has never shown, or
    /// evicted a rejected one.
    list_changed: bool,
}

/// The body of one pass. Split out so [`run_pass`] owns only the in-flight
/// bookkeeping and the events.
async fn pass(
    app: &AppHandle,
    _activity_window_days: Option<u32>,
    announce: &(dyn Fn(ActivityEntry) + Send + Sync),
) -> anyhow::Result<PassSummary> {
    let store = app.state::<Store>();
    let now = unix_now();
    // Discovery always covers the widest list the UI can request, so changing
    // the display window is instant. The retention setting controls older rows.
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

    let previous_records = store.session_records()?;
    let Described {
        records,
        rejected,
        changed,
        list_changed,
    } = describe_with_states(logs, &home, &ignored, &previous_records).await;
    // Every write below is routed through the storage-health check, so a
    // database that has stopped accepting writes becomes a banner in the
    // popover rather than a list that silently stops changing.
    checked(
        app,
        "The session index",
        store.upsert_sessions(&records, &agents::evidence_cohort()),
    )?;
    crate::insights_worker::wake(app);

    announce_changed_rows(&store, &changed, &previous_records, now, announce);

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
    let controller = app.state::<ScanController>();
    if controller.cancelled() {
        return Ok(PassSummary {
            sessions: records.len(),
            list_changed,
        });
    }

    repositories::refresh(app).await?;

    Ok(PassSummary {
        sessions: records.len(),
        list_changed,
    })
}

/// Emit `SESSION_ENTRY_CHANGED_EVENT` for every re-described row the reader's
/// list has already shown, so a row already on screen patches in place
/// instead of waiting for the next full refetch. A brand-new session is not
/// announced this way: it has no row to patch, and [`Described::list_changed`]
/// tells the list to refetch and pick it up instead.
fn announce_changed_rows(
    store: &Store,
    changed: &[SessionKey],
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
    now: i64,
    announce: &(dyn Fn(ActivityEntry) + Send + Sync),
) {
    let previously_known = previously_known_keys(previous_records);
    for key in changed {
        if !previously_known.contains(key) {
            continue;
        }
        if let Some(entry) = crate::insights_worker::completion_entry(store, key, now) {
            announce(entry);
        }
    }
}

/// What one scan pass learned: rows for the index, and previously indexable
/// transcripts the sub-agent gate now refuses.
struct Described {
    records: Vec<SessionRecord>,
    rejected: Vec<SessionKey>,
    /// Keys of every record this pass re-described — new, or its cursor
    /// moved — never a row this pass reused verbatim.
    changed: Vec<SessionKey>,
    /// True when a reader's list needs a full refetch rather than a row
    /// patch: this pass indexed a session absent from `previous_records`, or
    /// rejected a sub-agent transcript.
    list_changed: bool,
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
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
) -> Described {
    let indexed_titles = indexed_titles_for_logs(&logs).await;
    let mut records = Vec::with_capacity(logs.len());
    let mut rejected = Vec::new();
    let mut changed = Vec::new();
    let mut list_changed = false;
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
            let previous = previous_records.get(&activity_key).cloned();
            let indexed_title = recovered_id(&log)
                .and_then(|session_id| indexed_titles.get(&(log.agent_type, session_id)).cloned());
            set.spawn(async move {
                if let Some(reused) = reuse_unchanged_record(&log, previous.as_ref()).await {
                    return (DescribeOutcome::Session(Box::new(reused)), false);
                }
                let outcome = describe_one_with_activity(log, &home, indexed_title, previous).await;
                (outcome, true)
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((DescribeOutcome::Session(record), re_described)) => {
                    let cwd = record.cwd.as_deref();
                    // The engine's opt-out gate, applied once here so every
                    // surface that reads the store inherits it.
                    if cwd.is_some_and(|cwd| ignored_paths::set_contains(ignored, cwd)) {
                        continue;
                    }
                    if re_described {
                        changed.push(record.key.clone());
                    }
                    records.push(*record);
                }
                Ok((DescribeOutcome::Subagent(key), _)) => rejected.push(key),
                Ok((DescribeOutcome::Skip, _)) | Err(_) => {}
            }
        }
    }
    // A rejected transcript's stale row, if any, is about to be evicted below
    // `describe_with_states`'s caller — either way the list must refetch to
    // stop showing it.
    if !rejected.is_empty() {
        list_changed = true;
    }
    let previously_known = previously_known_keys(previous_records);
    if changed.iter().any(|key| !previously_known.contains(key)) {
        list_changed = true;
    }
    Described {
        records,
        rejected,
        changed,
        list_changed,
    }
}

/// Every session key `previous_records` already held, keyed the way
/// [`Described::changed`] is: by session identity rather than by activity
/// source. Shared by the `list_changed` check above and by
/// [`announce_changed_rows`], which both ask the same question — was this
/// key already on the reader's list — of the same map.
fn previously_known_keys(
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
) -> std::collections::HashSet<&SessionKey> {
    previous_records
        .values()
        .map(|record| &record.key)
        .collect()
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

/// Build the sizes-only activity cursor for one source: the parent path and
/// size, plus each child's path and size. The format must stay byte-identical
/// across releases — a stored row holds this string, and a format change
/// would read as a change and re-queue evidence for every session.
fn activity_cursor(
    parent: &std::path::Path,
    parent_size: u64,
    children: &[(std::path::PathBuf, Option<u64>)],
) -> String {
    // Include the complete source set in the cursor. Parent + child sizes and
    // identities make an unchanged orchestrator as cheap as a leaf while a
    // child append naturally invalidates the gate.
    let mut cursor_parts = vec![[
        "parent".to_string(),
        parent.to_string_lossy().into_owned(),
        parent_size.to_string(),
    ]];
    for (child, child_size) in children {
        cursor_parts.push([
            "child".to_string(),
            child.to_string_lossy().into_owned(),
            child_size.map_or_else(|| "missing".to_string(), |size| size.to_string()),
        ]);
    }
    cursor_parts.sort_unstable();
    serde_json::to_string(&cursor_parts).expect("activity cursor is serializable")
}

/// Stat the parent and its children the same way [`semantic_activity_for_log`]
/// does, and return the cursor those sizes produce. `None` when the source is
/// not eligible for the unchanged-source skip: not a native file source, or a
/// stat failed.
async fn stat_activity_cursor(log: &SessionLog) -> Option<String> {
    if !log.environment.is_native() {
        // A WSL mount's stat behaviour is not trusted for this skip; describe
        // it every pass, as today.
        return None;
    }
    let SessionSource::File(path) = &log.source else {
        return None;
    };
    let size = tokio::fs::metadata(path).await.ok()?.len();
    let children = match log.agent_type {
        AgentKind::Claude | AgentKind::Codex => {
            Explorers::DISK
                .list_subagents_for_transcript(&log.agent_type, path)
                .await
        }
        _ => Vec::new(),
    };
    let mut child_sizes = Vec::with_capacity(children.len());
    for child in &children {
        let child_size = tokio::fs::metadata(child).await.ok().map(|meta| meta.len());
        child_sizes.push((child.clone(), child_size));
    }
    Some(activity_cursor(path, size, &child_sizes))
}

/// Reuse a previous record verbatim when its source has not changed, so the
/// pass never opens the transcript. Only a native file source with a stored
/// record is eligible; provider-database, inline, and WSL sources always
/// describe.
async fn reuse_unchanged_record(
    log: &SessionLog,
    previous: Option<&SessionRecord>,
) -> Option<SessionRecord> {
    let previous = previous?;
    let cursor = stat_activity_cursor(log).await?;
    if cursor != previous.activity_cursor {
        return None;
    }
    // An "event" row takes its activity from transcript content, and the
    // cursor covers the sizes of that content. Any other row tracks the file
    // mtime. A rewrite can keep the size, so such a row also needs the same
    // discovered mtime before the pass can reuse it.
    let mtime_matches =
        previous.activity_source == "event" || previous.updated_at_epoch == log.updated_at;
    mtime_matches.then(|| previous.clone())
}

/// Resolve the display timestamp from transcript events while using the
/// persisted aggregate cursor as a cheap unchanged-source gate.
async fn semantic_activity_for_log(
    log: &SessionLog,
    previous: Option<&SessionRecord>,
    children: &[std::path::PathBuf],
    preview: Option<&str>,
) -> (Option<i64>, String, String) {
    let SessionSource::File(path) = &log.source else {
        return (log.updated_at, "unknown".to_string(), String::new());
    };

    let Some(size) = tokio::fs::metadata(path).await.ok().map(|meta| meta.len()) else {
        return (log.updated_at, "mtime".to_string(), String::new());
    };

    let mut child_sizes = Vec::with_capacity(children.len());
    for child in children {
        let child_size = tokio::fs::metadata(child).await.ok().map(|meta| meta.len());
        child_sizes.push((child.clone(), child_size));
    }
    let cursor = activity_cursor(path, size, &child_sizes);

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
    describe_one_with_activity(log, home, indexed_title, None).await
}

async fn describe_one_with_activity(
    log: SessionLog,
    home: &std::path::Path,
    indexed_title: Option<ResolvedTitle>,
    previous: Option<SessionRecord>,
) -> DescribeOutcome {
    let read = session_log_read(&log).await;
    let metadata = read.as_ref().map(|read| &read.metadata);
    let Some(session_id) = metadata
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
    let preview = match log.agent_type {
        AgentKind::Claude | AgentKind::Codex => {
            read.as_ref().and_then(|read| read.content.as_deref())
        }
        _ => None,
    };
    if is_subagent_transcript(&log, preview) {
        return DescribeOutcome::Subagent(key);
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
        semantic_activity_for_log(&log, previous.as_ref(), &children, preview).await;
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

    DescribeOutcome::Session(Box::new(SessionRecord {
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

/// The current time in unix seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
