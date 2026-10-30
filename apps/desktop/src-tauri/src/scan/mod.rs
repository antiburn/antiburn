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
//! - **At launch**, once, unless discovery is paused. A fresh install runs
//!   this pass too: the first-run Overview follows it, rather than waiting
//!   for an explicit scan request.
//! - **Every [`TICK`], unconditionally.** R1/R2: the watcher is the primary
//!   freshness path now, so the tick is 5 minutes of reconciliation for what
//!   it cannot see — a WSL session (never watched) or a rare dropped OS
//!   event — not the path a reader waits on. A pass over sources that have
//!   not changed reads no transcript, so ticking while the popover is hidden
//!   costs stat calls, not disk reads.
//! - **On demand**, from the rescan control and after any change to the source
//!   selection.
//! - **Shortly after a watched file changes — narrowly, not as a full pass.**
//!   [`watch::spawn_watcher`] starts an OS filesystem watcher over every
//!   agent's watch roots (`AgentExplorer::watch_roots`), debounces the events
//!   it sees, and hands the scheduler's loop one [`watch::WatchBurst`] after
//!   each quiet period — see the `watch` module doc for the debounce shape.
//!   Both handoffs are bounded. An overflow requests one full reconciliation
//!   instead of retaining an event-sized backlog.
//!   The `scoped` module classifies that burst into four lanes: a known
//!   session refresh, an indexed-title refresh, a plain agent rediscovery,
//!   or a database-backed agent rediscovery. Each lane has its own minimum
//!   re-run interval, and a burst
//!   that arrives inside one is folded into the next admitted run rather
//!   than dropped — see the `scoped` module doc for the full contract.
//!   [`TICK`] stays as reconciliation for what the watcher cannot cover (a
//!   root the OS refuses to watch, a change the debounce window swallowed),
//!   and it runs a full pass, which supersedes any scoped work still
//!   waiting on its floor. When the watcher is not fully healthy the
//!   scheduler ticks at [`watch::FALLBACK_TICK`] instead, so polling alone
//!   still finds new sessions at a reasonable cadence.
//!
//! # Pausing
//!
//! `AppSettings::discovery_paused` stops every *scheduled* pass — the launch
//! pass, the tick, and the passes requested after a source selection change.
//! It deliberately does not stop [`run_pass`] itself, so the
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
//! Every routine pass is bounded: discovery is windowed to
//! [`crate::store::model::CURRENT_WINDOW_DAYS`], and the per-session metadata reads
//! run at a fixed concurrency, so one pass cannot grow with the size of the
//! machine. [`crate::insights_worker`] bounds its own analysis concurrency
//! separately. The separate retention policy expires indexed sessions; the
//! bounded discovery window does not. The dedicated historical pass widens
//! the window instead — see its own module section below. The scheduler is a
//! single handle the app aborts on exit, so nothing outlives
//! the process.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use antiburn_local::discovery::scanner::{self, TitleSource};
use antiburn_local::discovery::{
    Explorers, ResolvedTitle, SessionLog, SessionSource, SourceDescriptor, TitleLookupKind,
    session_log_read, session_source_preview, session_source_tail,
};
use antiburn_local::model::AgentKind;
use antiburn_local::paths::{home_dir, ignored_paths};
use antiburn_local::platform::environment::{self, DiscoveryEnvironment};
use antiburn_local::platform::git;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::agents;
use crate::analysis;
use crate::dto::{AgentFoundCount, ReadGateCounts, ReadProgress, ScanPhase, ScanStatus};
use crate::first_run_gate::{FirstRunGate, FirstRunStage};
use crate::repositories;
use crate::session_lifecycle::{self, AnonymousCover, AnonymousGen};
use crate::storage_health::{self, checked};
use crate::store::{
    SessionActivityKey, SessionCommitOutcome, SessionKey, SessionRecord, SessionWriteGuard, Store,
};

mod fingerprint;
pub mod history;
pub mod live_poll;
pub mod scoped;
pub mod watch;

/// How often the scheduler wakes up for its unconditional full pass.
///
/// R2: 5 minutes. The watcher is the primary freshness path (T1-T7 in the
/// `scoped` module), so this tick only has to reconcile what it cannot see —
/// a WSL session, or a rare dropped OS event — not answer for an active
/// session's own row updates. [`watch::FALLBACK_TICK`] stays 15 seconds for a
/// degraded watcher, where polling really is the only freshness path.
pub const TICK: Duration = Duration::from_secs(300);

/// How many session logs have their metadata read at once. Bounds open files
/// and blocking-pool pressure during a whole-machine pass.
#[cfg(test)]
const METADATA_CONCURRENCY: usize = 16;

/// How often the read stage's `scan:progress` event may repeat while the
/// stage is still running. The stage always emits one more event right after
/// its last file, whatever this interval says.
const READ_PROGRESS_THROTTLE: Duration = Duration::from_millis(150);

/// Scope key for the engine's ignored-path store. The engine namespaces opt-outs
/// so one machine can hold several independent sets; this app keeps one.
pub const IGNORE_SCOPE: &str = "local";

/// Events the scan emits. The webview listens for these rather than polling.
pub const EVENT_STARTED: &str = "scan:started";
pub const EVENT_PROGRESS: &str = "scan:progress";
pub const EVENT_FINISHED: &str = "scan:finished";

/// W3: why a pass was requested. Every request site names its own purpose, so
/// a log line answers "why is the machine scanning right now" without having
/// to correlate timestamps against reader actions after the fact.
#[derive(Debug, Clone)]
pub enum ScanTrigger {
    /// The one pass run at startup, unless discovery is paused.
    Launch,
    /// The scheduler's unconditional [`TICK`].
    Tick,
    /// T3/T5: a burst named one or more agents to rediscover, with no full
    /// discovery walk over the rest.
    WatcherAgents {
        /// The rediscovered agents' slugs, for the log line.
        agents: Vec<&'static str>,
    },
    /// Refresh retained source metadata for agents whose evidence read changed
    /// mid-pass.
    SourceRefresh {
        /// The agents whose stored source claims need a fresh discovery.
        agents: Vec<&'static str>,
    },
    /// A burst reached [`watch::MAX_BURST_PATHS`] and may have dropped
    /// paths, so only a full pass can be sure to cover it.
    WatcherOverflow,
    /// A settings save that finished onboarding, widened the activity
    /// window, or resumed discovery.
    SettingsTransition,
    /// A repository was included or ignored.
    RepositoryToggle,
    /// A scan root was added.
    ScanRootAdded,
    /// A protected folder's access was granted or discovered.
    FolderAccessGranted,
    /// The local index was cleared.
    IndexCleared,
    /// The reader asked for a rescan explicitly. Stays current-window only —
    /// see [`Self::HistoricalScan`] for the trigger that widens.
    ManualRescan,
    /// The reader asked for the dedicated historical pass (Sessions step ›
    /// Scanning › Older sessions), or the scheduler started the one-time
    /// automatic pass for the current retention. Widens discovery past the
    /// current window — see `history::window_secs`.
    HistoricalScan,
}

impl ScanTrigger {
    /// A stable snake_case name for logs.
    pub fn label(&self) -> &'static str {
        match self {
            ScanTrigger::Launch => "launch",
            ScanTrigger::Tick => "tick",
            ScanTrigger::WatcherAgents { .. } => "watcher_agents",
            ScanTrigger::SourceRefresh { .. } => "source_refresh",
            ScanTrigger::WatcherOverflow => "watcher_overflow",
            ScanTrigger::SettingsTransition => "settings_transition",
            ScanTrigger::RepositoryToggle => "repository_toggle",
            ScanTrigger::ScanRootAdded => "scan_root_added",
            ScanTrigger::FolderAccessGranted => "folder_access_granted",
            ScanTrigger::IndexCleared => "index_cleared",
            ScanTrigger::ManualRescan => "manual_rescan",
            ScanTrigger::HistoricalScan => "historical_scan",
        }
    }

    /// R4: whether a full pass should pay to refresh the repository list for
    /// this trigger, on top of the `list_changed` check every trigger gets.
    ///
    /// The tick and the watcher triggers cannot plausibly have introduced a
    /// repository the list has not already seen — a new `cwd` only arrives
    /// through a session's own upsert, and `list_changed` already covers
    /// that case for them. Every other trigger names an action that can
    /// change the repository set directly (a toggle, a new scan root, a
    /// settings transition), so it refreshes unconditionally. Matched
    /// exhaustively, with no `_` arm, so a new variant forces this decision
    /// rather than defaulting into it silently.
    pub fn refreshes_repositories(&self) -> bool {
        match self {
            ScanTrigger::Tick
            | ScanTrigger::WatcherAgents { .. }
            | ScanTrigger::SourceRefresh { .. }
            | ScanTrigger::WatcherOverflow => false,
            ScanTrigger::Launch
            | ScanTrigger::SettingsTransition
            | ScanTrigger::RepositoryToggle
            | ScanTrigger::ScanRootAdded
            | ScanTrigger::FolderAccessGranted
            | ScanTrigger::IndexCleared
            | ScanTrigger::ManualRescan
            | ScanTrigger::HistoricalScan => true,
        }
    }
}

fn source_refresh_scope(agents: &BTreeSet<AgentKind>) -> PassScope {
    PassScope::Agents(agents.clone())
}

fn uses_history_window(trigger: &ScanTrigger) -> bool {
    matches!(
        trigger,
        ScanTrigger::HistoricalScan | ScanTrigger::SourceRefresh { .. }
    )
}

/// What one pass discovers: every agent, or only a burst-named subset.
///
/// T3/T5: an agent-scoped pass reuses [`run_pass`] and [`pass`] wholesale —
/// same status machinery, same events — so the only thing it changes is which
/// agents discovery asks and which agents' rows the upsert and per-agent scan
/// bookkeeping touch.
#[derive(Debug, Clone)]
pub enum PassScope {
    Full,
    Agents(BTreeSet<AgentKind>),
}

impl PassScope {
    fn includes(&self, agent: AgentKind) -> bool {
        match self {
            PassScope::Full => true,
            PassScope::Agents(agents) => agents.contains(&agent),
        }
    }
}

/// The scheduler's ledger of anonymous activity it has reported but no
/// pass has accounted for yet.
///
/// Every anonymous touch gets the next generation from a checked monotonic
/// counter, and the agent's outstanding generation moves to it. A pass
/// captures the outstanding generations for its scope when it starts, and
/// settles them only when it succeeds. Generations express causality:
/// a touch reported during a pass gets a generation above the capture, so
/// the pass cannot cover it, whatever the clock says. Only the scheduler
/// task owns this ledger, so a pass a command asks for covers nothing.
#[derive(Debug, Default)]
pub(crate) struct AnonymousLedger {
    next: u64,
    outstanding: BTreeMap<AgentKind, AnonymousGen>,
}

impl AnonymousLedger {
    /// The next generation for one anonymous touch of `agent`. `None` when
    /// the counter is exhausted: the touch is then not reported, and a log
    /// line says so, rather than reusing a generation.
    pub(crate) fn issue(&mut self, agent: AgentKind) -> Option<AnonymousGen> {
        let Some(next) = self.next.checked_add(1) else {
            ::tracing::error!(event = "scan_anonymous_generation_exhausted");
            return None;
        };
        self.next = next;
        let generation = AnonymousGen(next);
        self.outstanding.insert(agent, generation);
        Some(generation)
    }

    /// The covers a pass of `scope` can settle: every outstanding
    /// generation of an agent in the scope, as of now.
    pub(crate) fn capture(&self, scope: &PassScope) -> Vec<AnonymousCover> {
        self.outstanding
            .iter()
            .filter(|(agent, _)| scope.includes(**agent))
            .map(|(agent, through)| AnonymousCover {
                agent: *agent,
                through: *through,
            })
            .collect()
    }

    /// Forget every outstanding generation at or below its cover. A newer
    /// generation issued after the capture stays outstanding.
    pub(crate) fn settle(&mut self, covers: &[AnonymousCover]) {
        for cover in covers {
            if self
                .outstanding
                .get(&cover.agent)
                .is_some_and(|outstanding| *outstanding <= cover.through)
            {
                self.outstanding.remove(&cover.agent);
            }
        }
    }
}

/// Whether a finished pass accounted for everything it set out to see. A
/// failed pass may have stopped before discovery; a cancelled one did not
/// finish every phase and says so, so neither covers.
fn pass_covers(status: &ScanStatus) -> bool {
    status.error.is_none() && !status.cancelled
}

/// The scheduler's shared state, registered as Tauri managed state.
#[derive(Default)]
pub struct ScanController {
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
    status: Mutex<ScanStatus>,
    kick: Notify,
    /// The trigger for the next pass the scheduler will run, set by
    /// [`ScanController::request`] and taken by the scheduler loop when it
    /// wakes. W3: a second request while one is already pending is coalesced
    /// rather than queued, because the waiting request already covers
    /// "scan again soon".
    pending_trigger: Mutex<Option<ScanTrigger>>,
    /// The one watcher burst waiting for the scheduler loop. New bursts merge
    /// into its fixed path budgets while a pass is running.
    pending_burst: Mutex<Option<watch::WatchBurst>>,
    /// One refresh bit per agent whose evidence reader rejected a stale claim.
    /// This stays separate from the coalesced one-shot trigger.
    pending_source_refreshes: Mutex<BTreeSet<AgentKind>>,
    /// Set once the first current-window pass finishes. The automatic
    /// historical pass (`history::maybe_start_automatic_pass`) checks this
    /// before it asks for anything, so it never races a fresh install's
    /// first pass.
    first_current_pass_done: AtomicBool,
    /// When [`history::push_progress`] last emitted its event, for the
    /// roughly-one-per-second throttle.
    history_last_emit: Mutex<Option<Instant>>,
    /// The retention for which this launch already asked for the automatic
    /// historical pass. A failed or cancelled pass does not ask again until
    /// the next launch, so a lasting failure cannot repeat it after every
    /// pass, and a cancel holds. The reader's own "Older sessions" scan ignores it.
    history_auto_requested_for: Mutex<Option<i32>>,
    /// True while a historical pass runs its discovery or describe phase.
    history_pass_running: AtomicBool,
}

impl ScanController {
    /// Ask for a pass as soon as the scheduler can start one, naming why.
    ///
    /// If a trigger is already pending, the earlier one is kept: the pending
    /// trigger already means a pass is coming, so the second ask changes
    /// nothing about when that happens, only which label would explain it.
    pub fn request(&self, trigger: ScanTrigger) {
        let mut pending = self
            .pending_trigger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match pending.as_mut() {
            // The historical pass is the only trigger that widens discovery
            // past the current window, so a pending automatic trigger
            // yields to it. Coalescing the other way would drop the only
            // pass that covers the retained history.
            Some(existing) if matches!(trigger, ScanTrigger::HistoricalScan) => {
                ::tracing::debug!(
                    event = "scan_request_coalesced",
                    kept = trigger.label(),
                    dropped = existing.label(),
                );
                *pending = Some(trigger);
            }
            Some(existing) => {
                ::tracing::debug!(
                    event = "scan_request_coalesced",
                    kept = existing.label(),
                    dropped = trigger.label(),
                );
            }
            None => *pending = Some(trigger),
        }
        drop(pending);
        self.kick.notify_one();
    }

    pub fn request_source_refresh(&self, agent: AgentKind) {
        self.pending_source_refreshes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(agent);
        self.kick.notify_one();
    }

    fn take_source_refreshes(&self) -> BTreeSet<AgentKind> {
        std::mem::take(
            &mut *self
                .pending_source_refreshes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn requeue_source_refreshes(&self, agents: BTreeSet<AgentKind>) {
        if agents.is_empty() {
            return;
        }
        self.pending_source_refreshes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend(agents);
        self.kick.notify_one();
    }

    /// Take the pending trigger, if any, for the scheduler to run next.
    fn take_pending_trigger(&self) -> Option<ScanTrigger> {
        self.pending_trigger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// Hand the scheduler loop a debounced burst, and wake it. Called from
    /// the watcher's own task, not the scheduler's, so this only queues the
    /// burst — classification and admission happen on the scheduler's loop.
    fn push_burst(&self, burst: watch::WatchBurst) {
        let mut pending = self
            .pending_burst
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match pending.as_mut() {
            Some(existing) => existing.merge(burst),
            None => {
                let mut bounded = watch::WatchBurst::default();
                bounded.merge(burst);
                *pending = Some(bounded);
            }
        }
        drop(pending);
        self.kick.notify_one();
    }

    /// Take the merged burst queued since the last scheduler drain.
    fn take_burst(&self) -> Option<watch::WatchBurst> {
        self.pending_burst
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn peek_burst(&self) -> Option<watch::WatchBurst> {
        self.pending_burst
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
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

    /// Whether the first current-window pass has finished since launch.
    pub(crate) fn first_current_pass_done(&self) -> bool {
        self.first_current_pass_done.load(Ordering::SeqCst)
    }

    /// Records that a current-window pass finished. Idempotent: a later
    /// pass's finish leaves this set.
    /// Whether a historical pass runs its discovery or describe phase now.
    pub(crate) fn history_pass_running(&self) -> bool {
        self.history_pass_running.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_current_pass_done(&self) {
        self.first_current_pass_done.store(true, Ordering::SeqCst);
    }

    /// Claims this launch's one automatic historical request for
    /// `retention_days`. True the first time for each retention.
    pub(crate) fn claim_history_auto_request(&self, retention_days: i32) -> bool {
        let mut requested = self
            .history_auto_requested_for
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *requested == Some(retention_days) {
            return false;
        }
        *requested = Some(retention_days);
        true
    }

    /// Lets the automatic historical pass ask again, for a fresh index.
    pub(crate) fn reset_history_auto_request(&self) {
        *self
            .history_auto_requested_for
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// Whether [`history::push_progress`] may emit now, under the
    /// roughly-one-per-second throttle. Advances the throttle's clock only
    /// when it answers yes.
    pub(crate) fn throttle_history_emit(&self) -> bool {
        let mut last_emit = self
            .history_last_emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let due =
            last_emit.is_none_or(|previous| now.duration_since(previous) >= Duration::from_secs(1));
        if due {
            *last_emit = Some(now);
        }
        due
    }
}

pub(crate) fn on_demand_start(controller: &ScanController) -> bool {
    if controller.running.swap(true, Ordering::SeqCst) {
        return false;
    }
    controller.cancel.store(false, Ordering::SeqCst);
    true
}

/// Release the scan slot if the caller drops an in-flight future.
pub(super) struct ScanOwnership<'a> {
    controller: Option<tauri::State<'a, ScanController>>,
}

impl<'a> ScanOwnership<'a> {
    fn new(controller: tauri::State<'a, ScanController>) -> Self {
        Self {
            controller: Some(controller),
        }
    }

    fn release(&mut self) {
        if let Some(controller) = self.controller.take() {
            controller.update(|status| status.running = false);
            controller
                .history_pass_running
                .store(false, Ordering::SeqCst);
            controller.cancel.store(false, Ordering::SeqCst);
            controller.running.store(false, Ordering::SeqCst);
            controller.kick.notify_one();
        }
    }
}

impl Drop for ScanOwnership<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// Which `tokio::select!` arm woke the scheduler loop. The loop needs to
/// tell its own unconditional [`TICK`] apart from a burst-triggered or
/// retry-triggered wake, since only the tick (or an explicit request) runs a
/// full pass; a plain burst or retry wake only advances scoped work.
enum Wake {
    Kick,
    Tick,
    Deferred,
    Health(Option<watch::WatcherStatus>),
    FingerprintRetry,
}

/// Start the scheduler. The returned handle is aborted when the app exits.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::runtime_pricing::wait_until_ready(&app).await;
        // The watcher starts here, before the launch pass: a session written
        // between this line and the first tick still reaches the debouncer.
        let mut watcher_health = watch::spawn_watcher(&app).await;
        // The ledger of anonymous activity this task has reported. Only
        // passes this task runs can cover it.
        let mut ledger = AnonymousLedger::default();
        // A fresh install runs this pass too: the first-run Overview follows
        // it, rather than waiting for an explicit scan request.
        if scheduled_scanning_allowed(&app) {
            run_covered_pass(&app, &mut ledger, ScanTrigger::Launch, PassScope::Full).await;
        }

        // T1-T7: the backlog a watcher burst can answer without a full
        // discovery walk. `pending_work` is floor-gated (T2/T4/T5, T7);
        // `retry_work` already cleared its floor but found a command's pass
        // running, and is retried directly at `scoped::SCOPED_RETRY` — see
        // the `scoped` module doc.
        let mut floors = scoped::Floors::default();
        let mut pending_work = scoped::ScopedWork::default();
        let mut retry_work = scoped::ScopedWork::default();
        let mut deferred_due: Option<tokio::time::Instant> = None;
        let mut last_full_pass = tokio::time::Instant::now();
        // The tick is a fixed deadline, not a sleep restarted on every
        // wake. A scoped wake every few seconds must not push the
        // reconciliation pass back forever.
        let mut watcher_is_healthy = watcher_health.borrow_and_update().is_healthy();
        let mut next_tick = tokio::time::Instant::now() + tick_for_health(watcher_is_healthy);
        let mut watcher_health_open = true;

        loop {
            let controller = app.state::<ScanController>();
            let fingerprint_due = if scheduled_scanning_allowed(&app) {
                match app.state::<Store>().next_source_fingerprint_attempt() {
                    Ok(due) => due.map(|epoch| {
                        tokio::time::Instant::now()
                            + Duration::from_secs(epoch.saturating_sub(unix_now()).max(0) as u64)
                    }),
                    Err(error) => {
                        ::tracing::warn!(event = "scan_fingerprint_schedule_failed", error = %error);
                        None
                    }
                }
            } else {
                None
            };
            let woke = tokio::select! {
                () = controller.kick.notified() => Wake::Kick,
                () = tokio::time::sleep_until(next_tick) => Wake::Tick,
                () = sleep_until_due(deferred_due) => Wake::Deferred,
                () = sleep_until_due(fingerprint_due) => Wake::FingerprintRetry,
                result = watcher_health.changed(), if watcher_health_open => {
                    let status = result
                        .ok()
                        .map(|()| watcher_health.borrow_and_update().clone());
                    Wake::Health(status)
                },
            };
            if matches!(woke, Wake::FingerprintRetry) {
                match owner::recover_fingerprints(&app).await {
                    Ok(true) => {}
                    Ok(false) => tokio::time::sleep(Duration::from_secs(1)).await,
                    Err(error) => {
                        ::tracing::warn!(event = "scan_fingerprint_recovery_failed", error = %error);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                continue;
            }
            if let Wake::Health(status) = &woke {
                let healthy = status
                    .as_ref()
                    .is_some_and(watch::WatcherStatus::is_healthy);
                next_tick = deadline_after_health_change(
                    next_tick,
                    last_full_pass,
                    watcher_is_healthy,
                    healthy,
                );
                watcher_is_healthy = healthy;
                watcher_health_open = status.is_some();
                continue;
            }
            if matches!(woke, Wake::Tick) {
                next_tick = tokio::time::Instant::now() + tick_for_health(watcher_is_healthy);
            }
            // Checked after the wake-up rather than before the wait, so
            // resuming discovery takes effect at the next request or tick
            // instead of needing the app restarted.
            if !scheduled_scanning_allowed(&app) {
                let dropped = controller.take_burst();
                if let Some(dropped) = dropped {
                    ::tracing::debug!(
                        event = "scan_bursts_dropped_while_paused",
                        events = dropped.events,
                        overflowed = dropped.overflowed,
                    );
                }
                // `Floors` keeps real last-run timestamps, so admission
                // recomputes correct due times once scanning resumes. A
                // stale `deferred_due` left set here would otherwise wake
                // this loop again immediately, on every iteration, for as
                // long as discovery stays paused.
                deferred_due = None;
                continue;
            }

            // Fold in any bursts queued since the last wake before deciding
            // what runs: a full pass below covers them for free, and a
            // scoped wake needs them for admission.
            let burst = controller.take_burst();
            let mut overflowed = false;
            if let Some(burst) = burst {
                let home = home_dir().unwrap_or_default();
                let store = app.state::<Store>();
                if burst.overflowed {
                    ::tracing::debug!(
                        event = "scan_burst_overflowed",
                        events = burst.events,
                        path_count = burst.paths.len(),
                    );
                    overflowed = true;
                } else {
                    let work = scoped::classify_burst(&burst.paths, &home, &|source_labels| {
                        store
                            .native_file_session_activity_keys(source_labels)
                            .unwrap_or_default()
                    });
                    // Before any floor or describe: this is the realtime
                    // path to the lifecycle bus. The wait is for inbox room
                    // only; no store guard is held across it.
                    report_touched(&app, &store, &mut ledger, &work).await;
                    pending_work.merge(work);
                }
            }

            // A trigger is pending unless this woke from the tick's own
            // sleep arm with nothing requested. Either way a full pass
            // supersedes every floor-gated or retrying scoped item: it
            // re-describes everything they would have, so their floors are
            // stamped as satisfied rather than left to run again soon.
            let source_refreshes = controller.take_source_refreshes();
            if !source_refreshes.is_empty() {
                let trigger = ScanTrigger::SourceRefresh {
                    agents: source_refreshes.iter().map(|agent| agent.slug()).collect(),
                };
                if run_covered_pass(
                    &app,
                    &mut ledger,
                    trigger,
                    source_refresh_scope(&source_refreshes),
                )
                .await
                .is_none()
                {
                    controller.requeue_source_refreshes(source_refreshes);
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
            }
            let full_trigger = controller.take_pending_trigger();
            if full_trigger.is_some() || overflowed || matches!(woke, Wake::Tick) {
                let trigger = full_trigger.unwrap_or(if overflowed {
                    ScanTrigger::WatcherOverflow
                } else {
                    ScanTrigger::Tick
                });
                run_covered_pass(&app, &mut ledger, trigger, PassScope::Full).await;
                let now = tokio::time::Instant::now();
                last_full_pass = now;
                floors.stamp(&pending_work, now);
                floors.stamp(&retry_work, now);
                pending_work = scoped::ScopedWork::default();
                retry_work = scoped::ScopedWork::default();
                deferred_due = None;
                // A full pass just ran, so the next tick can wait a whole
                // interval.
                next_tick = now + tick_for_health(watcher_is_healthy);
                continue;
            }

            // T7: retry work a prior admission already stamped as run, but
            // that found a command's pass in the way, before admitting more
            // — it does not need to clear its floor again, only wait for
            // the pass that was busy.
            if !retry_work.is_empty() {
                retry_work =
                    run_admitted_work(&app, &mut ledger, std::mem::take(&mut retry_work)).await;
            }

            let now = tokio::time::Instant::now();
            let (run_now, deferred, earliest_due) =
                floors.admit(std::mem::take(&mut pending_work), now);
            pending_work = deferred;
            if !run_now.is_empty() {
                let busy = run_admitted_work(&app, &mut ledger, run_now).await;
                retry_work.merge(busy);
            }

            deferred_due = match (earliest_due, retry_work.is_empty()) {
                (Some(due), true) => Some(due),
                (Some(due), false) => Some(due.min(now + scoped::SCOPED_RETRY)),
                (None, true) => None,
                (None, false) => Some(now + scoped::SCOPED_RETRY),
            };
        }
    })
}

/// Sleep until `due`, or forever when nothing is due. A `tokio::select!` arm
/// with nothing pending must never fire, so this never resolves on `None`
/// rather than resolving immediately.
async fn sleep_until_due(due: Option<tokio::time::Instant>) {
    match due {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

/// Run one pass this scheduler owns. When it ran and succeeded, the
/// anonymous generations captured at its start are settled and covered on
/// the lifecycle bus, after every report the pass itself made. A busy,
/// failed, or cancelled pass covers nothing.
async fn run_covered_pass(
    app: &AppHandle,
    ledger: &mut AnonymousLedger,
    trigger: ScanTrigger,
    scope: PassScope,
) -> Option<ScanStatus> {
    let covers = ledger.capture(&scope);
    let status = try_run_pass(app, None, trigger, scope).await?;
    if pass_covers(&status) {
        ledger.settle(&covers);
        report_covered(app, covers).await;
    }
    Some(status)
}

/// Run each lane admitted for one scheduler wake.
/// Return work that found another pass running, so the scheduler can retry it.
async fn run_admitted_work(
    app: &AppHandle,
    ledger: &mut AnonymousLedger,
    work: scoped::ScopedWork,
) -> scoped::ScopedWork {
    let mut busy = scoped::ScopedWork::default();

    if !work.sessions.is_empty() {
        match scoped::refresh_sessions(app, &work.sessions).await {
            Ok(Some(summary)) => {
                ::tracing::debug!(
                    event = "scan_targeted_admitted",
                    sessions = work.sessions.len(),
                    re_described = summary.re_described,
                );
            }
            Ok(None) => busy.sessions = work.sessions,
            Err(error) => {
                ::tracing::debug!(event = "scan_targeted_failed", error = %error);
            }
        }
    }

    if !work.title_agents.is_empty() {
        match scoped::refresh_indexed_titles(app, &work.title_agents).await {
            Ok(Some(summary)) => {
                ::tracing::debug!(
                    event = "scan_titles_admitted",
                    agents = work.title_agents.len(),
                    changed = summary.re_described,
                );
            }
            Ok(None) => busy.title_agents = work.title_agents,
            Err(error) => {
                ::tracing::debug!(event = "scan_titles_failed", error = %error);
            }
        }
    }

    let rediscover: BTreeSet<AgentKind> = work
        .agents
        .iter()
        .chain(work.db_agents.iter())
        .chain(work.quiet_agents.iter())
        .copied()
        .collect();
    if !rediscover.is_empty() {
        let labels: Vec<&'static str> = rediscover.iter().map(|agent| agent.slug()).collect();
        let trigger = ScanTrigger::WatcherAgents { agents: labels };
        // T3, T5: rediscover exactly these agents, reusing the pass so
        // `scan:started` / `scan:finished` and its log lines come for free.
        if run_covered_pass(app, ledger, trigger, PassScope::Agents(rediscover))
            .await
            .is_none()
        {
            busy.agents = work.agents;
            busy.db_agents = work.db_agents;
            busy.quiet_agents = work.quiet_agents;
        }
    }

    busy
}

fn tick_for_health(healthy: bool) -> Duration {
    if healthy { TICK } else { watch::FALLBACK_TICK }
}

/// Update the next full-pass deadline only when watcher health changes.
fn deadline_after_health_change(
    current: tokio::time::Instant,
    last_full_pass: tokio::time::Instant,
    was_healthy: bool,
    is_healthy: bool,
) -> tokio::time::Instant {
    if was_healthy == is_healthy {
        current
    } else if is_healthy {
        last_full_pass + TICK
    } else {
        current.min(last_full_pass + watch::FALLBACK_TICK)
    }
}

/// Whether the scheduler may start a pass of its own right now.
///
/// One gate, the reader's: discovery must not be paused. Does not apply to an
/// explicitly requested [`run_pass`].
fn scheduled_scanning_allowed(app: &AppHandle) -> bool {
    app.state::<Store>()
        .settings()
        .map(|settings| !settings.discovery_paused)
        .unwrap_or(false)
}

/// Run one pass, unless one is already in flight.
pub async fn run_pass(
    app: &AppHandle,
    activity_window_days: Option<u32>,
    trigger: ScanTrigger,
    scope: PassScope,
) -> ScanStatus {
    try_run_pass(app, activity_window_days, trigger, scope)
        .await
        .unwrap_or_else(|| app.state::<ScanController>().status())
}

/// [`run_pass`]'s body, returning `None` rather than a stale status when
/// [`on_demand_start`] finds one already running. The scoped lanes in
/// `scoped.rs` use this directly to tell "ran" from "skipped, retry soon"
/// (T7) — a distinction [`run_pass`]'s callers do not need, since they always
/// treat "already running" as "nothing more to do here".
pub(crate) async fn try_run_pass(
    app: &AppHandle,
    activity_window_days: Option<u32>,
    trigger: ScanTrigger,
    scope: PassScope,
) -> Option<ScanStatus> {
    log_scan_pass_requested(&trigger);
    {
        let controller = app.state::<ScanController>();
        if !on_demand_start(&controller) {
            // W4: a request dropped here is otherwise silent, and a hidden
            // feedback loop then shows up only as a wall of full passes on
            // the tick. Logging the trigger turns that into a visible stream
            // of drops instead.
            ::tracing::debug!(event = "scan_request_dropped", trigger = trigger.label());
            // A dropped automatic request is fine, because the scheduler runs
            // a pass on its own tick. A dropped historical pass is not: it is
            // the only request that widens discovery past the current
            // window, so no later routine pass would cover the same ground.
            // Queue it for the scheduler instead of losing it.
            if matches!(trigger, ScanTrigger::HistoricalScan) {
                controller.request(ScanTrigger::HistoricalScan);
            }
            return None;
        }
        let full_pass = progress_pass(&trigger, &scope);
        let started = controller.update(|status| {
            status.running = true;
            status.completed_agents = 0;
            status.total_agents = AgentKind::ALL.len();
            status.sessions = 0;
            status.list_changed = false;
            status.re_described = 0;
            status.error = None;
            status.cancelled = false;
            // FTUE's first-run steps read `phase`/`found_by_agent`/`read`/
            // `gate` as one full pass's outcome (see `pass`'s own `full_pass`
            // gate). A scoped pass (T3/T5) must not blank them either: a
            // reader who looks up only after such a pass started would
            // otherwise see a finished full pass's result go blank.
            if full_pass {
                status.phase = ScanPhase::Finding;
                // Every agent this pass searches, in a fixed order, so a
                // reader sees which agents are still searching.
                status.found_by_agent = AgentKind::ALL
                    .iter()
                    .map(|agent| AgentFoundCount {
                        agent: agent.slug().to_string(),
                        sessions: 0,
                        done: false,
                    })
                    .collect();
                status.read = ReadProgress::default();
                status.gate = None;
            }
        });
        let _ = app.emit(EVENT_STARTED, started);
    }
    let mut ownership = ScanOwnership::new(app.state::<ScanController>());
    app.state::<ScanController>().history_pass_running.store(
        matches!(trigger, ScanTrigger::HistoricalScan),
        Ordering::SeqCst,
    );
    history::push_progress(app, true);
    ::tracing::debug!(event = "scan_pass_started", trigger = trigger.label());
    let pass_started_at = Instant::now();

    let outcome = pass(app, activity_window_days, &trigger, &scope).await;

    let controller = app.state::<ScanController>();
    let cancelled = controller.cancelled();
    controller.update(|status| {
        status.running = false;
        status.cancelled = cancelled;
        status.finished_at = Some(crate::store::now_rfc3339());
        match &outcome {
            Ok(summary) => {
                status.sessions = summary.sessions;
                status.list_changed = summary.list_changed;
                // R5: lets a reader tell an idle pass from a productive one.
                status.re_described = summary.re_described;
                // A cancelled pass did not finish every agent, and saying it
                // did would make the progress line lie on its last frame.
                if !cancelled {
                    status.completed_agents = status.total_agents;
                }
                status.error = None;
            }
            Err(error) => {
                status.error = Some(error.to_string());
                status.re_described = 0;
            }
        }
    });
    ownership.release();
    // A pass that got all the way through wrote to the store several times, so
    // it is also the proof that a previously reported storage failure is over.
    if outcome.is_ok() {
        storage_health::note_ok(app);
    }
    let covered = outcome.is_ok() && !cancelled;
    if covered {
        match (&trigger, &scope) {
            // The first current-window pass to cover everything clears the
            // automatic historical trigger's own gate.
            (trigger, PassScope::Full) if !matches!(trigger, ScanTrigger::HistoricalScan) => {
                controller.mark_current_pass_done();
            }
            (ScanTrigger::HistoricalScan, _) => {
                let store = app.state::<Store>();
                let retention_days = store.settings_snapshot().session_data_retention_days;
                if history::window_secs(retention_days, unix_now()).is_some() {
                    history::mark_done(&store, retention_days);
                }
            }
            _ => {}
        }
    }
    controller
        .history_pass_running
        .store(false, Ordering::SeqCst);
    history::push_progress(app, true);
    history::maybe_start_automatic_pass(app);
    let finished = controller.status();
    let duration_ms = pass_started_at.elapsed().as_millis() as u64;
    match &outcome {
        Ok(summary) => {
            ::tracing::debug!(
                event = "scan_pass_finished",
                trigger = trigger.label(),
                duration_ms,
                sessions = summary.sessions,
                re_described = summary.re_described,
                list_changed = summary.list_changed,
                cancelled,
            );
        }
        Err(error) => {
            ::tracing::debug!(
                event = "scan_pass_finished",
                trigger = trigger.label(),
                duration_ms,
                sessions = 0,
                re_described = 0,
                list_changed = false,
                cancelled,
                error = %error,
            );
        }
    }
    let _ = app.emit(EVENT_FINISHED, finished.clone());
    // The outcome, not a shaped event: whether this pass is worth reporting at
    // all is an analytics question, and this scheduler runs a full pass every
    // five minutes plus a scoped pass on every watcher burst. `None` is a
    // failure, which travels as a bare category — an error string can hold a
    // path. Only a full pass counts every session on the install, so only a
    // full pass may report. See [`scan_report`].
    if let Some(report) = scan_report(
        &scope,
        outcome.as_ref().ok().map(|summary| summary.sessions as u64),
    ) {
        crate::analytics::record_scan(app, report);
    }
    crate::notifications::note_scan_outcome(app, &finished);
    Some(finished)
}

/// Decide whether a finished pass should reach analytics, and with what count.
///
/// A full pass counts every session on the install. A scoped pass (T3/T5, a
/// watcher-burst retry of a handful of agents) counts only those agents, so
/// its number is not comparable to a full pass's number. Reporting it made
/// the bucket flap between the full total and a small scoped count on every
/// burst, which produced about 90% of all analytics events. A scoped pass
/// therefore reports nothing, on success or on failure: a scoped failure
/// must not flip the last-reported outcome to `scan_failed` either.
///
/// `outcome` is the pass's session count, or `None` for a failed pass — the
/// same shape [`crate::analytics::record_scan`] takes. The return value is
/// `None` when nothing should be reported, or `Some(outcome)` to pass on.
fn scan_report(scope: &PassScope, outcome: Option<u64>) -> Option<Option<u64>> {
    match scope {
        PassScope::Full => Some(outcome),
        PassScope::Agents(_) => None,
    }
}

/// Log `scan_pass_requested`. Every call to [`run_pass`] gets one, whether it
/// goes on to run or is dropped by [`on_demand_start`] — the drop is a
/// separate `scan_request_dropped` line (W4), and a coalesced ask never
/// reaches here at all: it never became a pending trigger a scheduler wake
/// picked up (see [`ScanController::request`]).
fn log_scan_pass_requested(trigger: &ScanTrigger) {
    match trigger {
        ScanTrigger::WatcherAgents { agents } | ScanTrigger::SourceRefresh { agents } => {
            ::tracing::debug!(
                event = "scan_pass_requested",
                trigger = trigger.label(),
                agents = %agents.join(","),
                agent_count = agents.len(),
            );
        }
        other => {
            ::tracing::debug!(event = "scan_pass_requested", trigger = other.label());
        }
    }
}

/// What one pass persisted, for [`run_pass`] to fold into the reported status.
struct PassSummary {
    sessions: usize,
    /// True when a reader's list needs a full refetch rather than a row
    /// patch: this pass indexed a session the list has never shown, or
    /// evicted a rejected one.
    list_changed: bool,
    /// [`Described::changed`]'s length: new or refreshed rows.
    re_described: usize,
}

/// The age limit discovery applies to a routine pass, in seconds.
///
/// Every routine trigger gets exactly this window — launch, the tick, a
/// watcher burst, a settings transition, a repository toggle, an explicit
/// `scan_now` rescan, and so on. None of them widen it: a recurring pass
/// doing more work than this would grow with the size of the machine, and a
/// pass that discovered more than the checks report covers would make the
/// two counts disagree. [`crate::store::model::CURRENT_WINDOW_DAYS`] is the shared
/// constant, so discovery and the report can never drift apart. The
/// dedicated historical pass uses its own, retention-based window instead —
/// see `history_window_secs`.
const CURRENT_WINDOW_SECS: i64 = crate::store::model::CURRENT_WINDOW_DAYS as i64 * 86_400;

/// Tells if a pass moves the Overview progress fields of [`ScanStatus`]:
/// only a full pass over the current window does.
fn progress_pass(trigger: &ScanTrigger, scope: &PassScope) -> bool {
    matches!(scope, PassScope::Full) && !matches!(trigger, ScanTrigger::HistoricalScan)
}

/// The first-run takeover's own gate on a full pass: wait for `stage`
/// before letting the pass continue. A pass [`progress_pass`] says is not
/// gated (a scoped watcher pass, or the dedicated historical pass) returns
/// `true` at once.
///
/// While the first run is still at an earlier stage, this is also what the
/// launch pass holds the scan slot on: no other full pass can start until
/// it returns, which keeps a scoped watcher pass (T3/T5) from writing
/// sessions before the reader's own Sessions step starts.
///
/// Returns `false` only when the wait is itself cancelled first, which the
/// caller must treat as an ordinary cancel: end the pass with no writes.
async fn wait_for_first_run_stage(app: &AppHandle, full_pass: bool, stage: FirstRunStage) -> bool {
    if !full_pass {
        return true;
    }
    let controller = app.state::<ScanController>();
    let gate = app.state::<FirstRunGate>();
    gate.wait_until(stage, || controller.cancelled()).await
}

/// The summary [`pass`] returns when a first-run gate wait ends as
/// cancelled: the same empty, uncovering outcome a plain scan cancel gives
/// today, because nothing has been discovered or written yet.
fn gate_cancelled_summary() -> PassSummary {
    PassSummary {
        sessions: 0,
        list_changed: false,
        re_described: 0,
    }
}

/// The body of one pass. Split out so [`run_pass`] owns only the in-flight
/// bookkeeping and the events.
///
/// `scope` narrows discovery, the upsert's evidence cohort, and per-agent
/// scan bookkeeping to a burst-named subset of agents (T3/T5); everything
/// else runs exactly as a full pass. See [`PassScope`]. `trigger` decides
/// the discovery window, whether this pass refreshes the repository list
/// (R4), and whether it moves the progress fields (see [`progress_pass`]).
async fn pass(
    app: &AppHandle,
    _activity_window_days: Option<u32>,
    trigger: &ScanTrigger,
    scope: &PassScope,
) -> anyhow::Result<PassSummary> {
    let store = app.state::<Store>();
    let now = unix_now();
    let is_history_pass = uses_history_window(trigger);
    // The Overview progress row reads `phase`, `found_by_agent`, `read`, and
    // `gate` as the outcome of one full pass over the current window. A
    // scoped pass (T3/T5) must not move them: it can start and finish while
    // an unrelated watched agent is still writing, and a reader who looks up
    // in that window would otherwise see a finished pass's result go blank.
    // A history pass must not move them either, because it covers a wider
    // window than the row shows.
    let full_pass = progress_pass(trigger, scope);
    let write_fence = store.capture_destructive_write_epoch();
    let since_secs = if is_history_pass {
        let retention_days = store.settings_snapshot().session_data_retention_days;
        history::window_secs(retention_days, now).unwrap_or(CURRENT_WINDOW_SECS)
    } else {
        CURRENT_WINDOW_SECS
    };

    let ignored = store.repository_policy_snapshot()?.ignored_paths;
    let include_non_repo_folders = store.settings_snapshot().include_non_repo_folders;
    let home = home_dir().unwrap_or_default();

    // The first-run takeover's "Agents" step: discovery does not
    // start until the reader leaves the welcome step.
    if !wait_for_first_run_stage(app, full_pass, FirstRunStage::Agents).await {
        return Ok(gate_cancelled_summary());
    }

    let mut incremental = IncrementalAdmission {
        io: AdmissionIo::App(app.clone()),
        store: &store,
        previous_records: std::collections::HashMap::new(),
        evidence_agents: evidence_agents_for_scope(scope),
        write_fence,
        error: None,
        outcomes: std::collections::HashMap::new(),
        list_changed: false,
    };
    let described = owner::discover_and_run(
        app,
        scope,
        owner::PassInputs {
            now,
            since_secs,
            history: is_history_pass,
            progress: full_pass,
            home,
            ignored,
            include_non_repo_folders,
            refresh_repositories: trigger.refreshes_repositories(),
            refresh_file_sources: matches!(trigger, ScanTrigger::SourceRefresh { .. }),
        },
        &mut incremental,
    )
    .await?;
    let Described {
        records,
        rejected,
        changed,
        list_changed,
        gate,
    } = &described;
    ::tracing::debug!(
        event = "scan_admission_finished",
        accepted = records.len(),
        rejected = rejected.len()
    );
    if full_pass {
        let kept = records.len();
        let gate_counts = read_gate_counts(gate, kept);
        let controller = app.state::<ScanController>();
        let status = controller.update(|status| {
            status.phase = ScanPhase::Saving;
            status.gate = Some(gate_counts);
            apply_admitted_agent_counts(&mut status.found_by_agent, records);
        });
        let _ = app.emit(EVENT_PROGRESS, status);
    }
    incremental.finish()?;

    // `records` already holds only the scoped agents' sessions when `scope`
    // is [`PassScope::Agents`], since discovery itself was scoped.
    for (agent, seen, cursor) in per_agent_totals(records) {
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
            list_changed: *list_changed,
            re_described: changed.len(),
        });
    }

    Ok(PassSummary {
        sessions: records.len(),
        list_changed: *list_changed,
        re_described: changed.len(),
    })
}

/// Report keyed touches before anonymous agent activity. Title-only writes and quiet
/// paths report no activity. Missing keys never produce anonymous substitutes.
async fn report_touched(
    app: &AppHandle,
    store: &Store,
    ledger: &mut AnonymousLedger,
    work: &scoped::ScopedWork,
) {
    if work.sessions.is_empty() && work.agents.is_empty() && work.db_agents.is_empty() {
        return;
    }
    let at = unix_now();
    let activity_keys = work.sessions.iter().cloned().collect::<Vec<_>>();
    // Read only identity evidence before awaiting any report. A failed lookup reports
    // no keyed touch. Later checked writes can report storage-health failures.
    let identities = match store.session_keys_for_activity_keys(&activity_keys) {
        Ok(identities) => Some(identities),
        Err(error) => {
            ::tracing::warn!(event = "scan_touch_lookup_failed", error = %error);
            None
        }
    };
    for observation in touch_observations(ledger, work, identities, at) {
        session_lifecycle::report_async(app, observation).await;
    }
}

/// Build keyed touches from writer evidence. Then assign fresh generations to anonymous
/// agent touches.
fn touch_observations(
    ledger: &mut AnonymousLedger,
    work: &scoped::ScopedWork,
    identities: Option<(crate::store::ActivityIdentities, crate::store::Revision)>,
    at: i64,
) -> Vec<session_lifecycle::Observation> {
    let mut out = Vec::new();
    if let Some((identities, seen)) = identities {
        for key in &work.sessions {
            let Some(agent) = AgentKind::from_slug(&key.agent) else {
                continue;
            };
            let Some((key, incarnation)) = identities.get(key) else {
                continue;
            };
            out.push(session_lifecycle::Observation::Touched {
                session: session_lifecycle::TouchedSession {
                    key: key.clone(),
                    incarnation: *incarnation,
                    seen,
                },
                agent,
                at,
            });
        }
    }
    for agent in work.agents.iter().chain(work.db_agents.iter()) {
        let Some(generation) = ledger.issue(*agent) else {
            continue;
        };
        out.push(session_lifecycle::Observation::Anonymous {
            agent: *agent,
            at,
            generation,
        });
    }
    out
}

/// Report successful pass covers after all its `Indexed` facts on the same ordered
/// path.
async fn report_covered(app: &AppHandle, covers: Vec<AnonymousCover>) {
    if covers.is_empty() {
        return;
    }
    session_lifecycle::report_async(
        app,
        session_lifecycle::Observation::AnonymousCovered { covers },
    )
    .await;
}

/// Derive bounded discovery facts without loading rich rows or accessing the registry.
fn discovery_report(
    now: i64,
    records: &[SessionRecord],
    incarnations: &[(SessionKey, crate::store::Incarnation)],
    changed: &[SessionKey],
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
    revision: crate::store::Revision,
) -> impl Iterator<Item = session_lifecycle::Observation> {
    let mut sessions =
        indexed_sessions_for_report(now, records, changed, previous_records, incarnations)
            .into_iter();
    let indexed = std::iter::from_fn(move || {
        let sessions: Vec<_> = sessions
            .by_ref()
            .take(session_lifecycle::INDEXED_CHUNK)
            .collect();
        (!sessions.is_empty())
            .then_some(session_lifecycle::Observation::Indexed { sessions, revision })
    });
    let rows = row_change_facets(records, changed, previous_records)
        .into_iter()
        .map(
            move |(session, facets)| session_lifecycle::Observation::RowChanged {
                session,
                facets,
                at: now,
            },
        );
    indexed.chain(rows)
}

/// Report only changed records inside the active window with returned incarnations.
/// Full identity determines `is_new`, not a reused source label.
fn indexed_sessions_for_report(
    now: i64,
    records: &[SessionRecord],
    changed: &[SessionKey],
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
    incarnations: &[(SessionKey, crate::store::Incarnation)],
) -> Vec<session_lifecycle::IndexedSession> {
    let changed: std::collections::HashSet<&SessionKey> = changed.iter().collect();
    let incarnations: std::collections::HashMap<&SessionKey, crate::store::Incarnation> =
        incarnations
            .iter()
            .map(|(key, incarnation)| (key, *incarnation))
            .collect();
    let previously_known = previously_known_keys(previous_records);
    let mut sessions = Vec::new();
    for record in records {
        if !changed.contains(&record.key) {
            continue;
        }
        let Some(at) = record.updated_at_epoch else {
            continue;
        };
        if now - at >= ACTIVE_SESSION_WINDOW_SECS {
            continue;
        }
        let Some(agent) = AgentKind::from_slug(&record.key.agent) else {
            continue;
        };
        let Some(incarnation) = incarnations.get(&record.key).copied() else {
            continue;
        };
        sessions.push(session_lifecycle::IndexedSession {
            key: record.key.clone(),
            agent,
            incarnation,
            at,
            is_new: !previously_known.contains(&record.key),
        });
    }
    sessions
}

fn wake_session_workers(app: &AppHandle) {
    crate::insights_worker::wake(app);
}

/// Which facets changed on each refreshed, previously known row. The scan
/// reports compact facts only; the projection layer rebuilds rich rows.
fn row_change_facets(
    records: &[SessionRecord],
    changed: &[SessionKey],
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
) -> Vec<(SessionKey, session_lifecycle::UpdateFacets)> {
    let changed: std::collections::HashSet<&SessionKey> = changed.iter().collect();
    let previous_by_identity: std::collections::HashMap<&SessionKey, &SessionRecord> =
        previous_records
            .values()
            .map(|record| (&record.key, record))
            .collect();
    let mut out = Vec::new();
    for record in records {
        if !changed.contains(&record.key) {
            continue;
        }
        let Some(&previous) = previous_by_identity.get(&record.key) else {
            continue;
        };
        let facets = facets_between(previous, record);
        if facets == session_lifecycle::UpdateFacets::default() {
            continue;
        }
        out.push((record.key.clone(), facets));
    }
    out
}

/// Compare one refreshed record with its stored predecessor and name what
/// changed: the title facet, the metadata facet, or both.
fn facets_between(
    previous: &SessionRecord,
    record: &SessionRecord,
) -> session_lifecycle::UpdateFacets {
    let title = previous.title != record.title || previous.title_source != record.title_source;
    let metadata = {
        let mut without_title = record.clone();
        without_title.title = previous.title.clone();
        without_title.title_source = previous.title_source.clone();
        &without_title != previous
    };
    session_lifecycle::UpdateFacets {
        title,
        metadata,
        project_scope: previous.cwd != record.cwd,
        ..Default::default()
    }
}

/// What one scan pass learned: rows for the index, and previously indexable
/// transcripts the sub-agent gate now refuses.
struct Described {
    records: Vec<SessionRecord>,
    rejected: Vec<SessionKey>,
    /// Keys of every new or refreshed record.
    changed: Vec<SessionKey>,
    /// True when a reader's list needs a full refetch rather than a row
    /// patch: this pass indexed a session absent from `previous_records`, or
    /// rejected a sub-agent transcript.
    list_changed: bool,
    /// How the repository gate resolved every read session.
    gate: GateCounts,
}

enum AdmissionIo {
    App(AppHandle),
    #[cfg(test)]
    Test {
        cancel: Arc<AtomicBool>,
        reports: Arc<Mutex<Vec<session_lifecycle::Observation>>>,
        now: Arc<std::sync::atomic::AtomicI64>,
        cancel_after_report: bool,
    },
}

impl AdmissionIo {
    fn cancelled(&self) -> bool {
        match self {
            Self::App(app) => app.state::<ScanController>().cancelled(),
            #[cfg(test)]
            Self::Test { cancel, .. } => cancel.load(Ordering::SeqCst),
        }
    }

    fn now(&self) -> i64 {
        match self {
            Self::App(_) => unix_now(),
            #[cfg(test)]
            Self::Test { now, .. } => now.load(Ordering::SeqCst),
        }
    }

    fn checked<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        match self {
            Self::App(app) => checked(app, "The session index", result),
            #[cfg(test)]
            Self::Test { .. } => result,
        }
    }

    fn wake_workers(&self) {
        match self {
            Self::App(app) => wake_session_workers(app),
            #[cfg(test)]
            Self::Test { .. } => {}
        }
    }

    async fn report(&self, observation: session_lifecycle::Observation) {
        match self {
            Self::App(app) => session_lifecycle::report_async(app, observation).await,
            #[cfg(test)]
            Self::Test {
                reports,
                cancel,
                cancel_after_report,
                ..
            } => {
                reports.lock().unwrap().push(observation);
                if *cancel_after_report {
                    cancel.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    async fn report_index_changed(&self) {
        self.report(session_lifecycle::Observation::IndexChanged {
            reason: session_lifecycle::IndexChangeReason::ScanPass,
        })
        .await;
    }
}

struct IncrementalAdmission<'a> {
    io: AdmissionIo,
    store: &'a Store,
    previous_records: std::collections::HashMap<SessionActivityKey, SessionRecord>,
    evidence_agents: Vec<&'a str>,
    write_fence: u64,
    error: Option<anyhow::Error>,
    outcomes: std::collections::HashMap<SessionKey, AttemptOutcome>,
    list_changed: bool,
}

enum AttemptOutcome {
    Accepted(Box<SessionRecord>, bool),
    Rejected,
    Failed,
}

impl IncrementalAdmission<'_> {
    fn finish(&mut self) -> anyhow::Result<()> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        if !self.io.cancelled()
            && self
                .outcomes
                .values()
                .any(|outcome| matches!(outcome, AttemptOutcome::Failed))
        {
            anyhow::bail!("Some session writes did not complete");
        }
        Ok(())
    }

    fn fail(&mut self, error: anyhow::Error) {
        ::tracing::error!(event = "scan_attempt_failed", error = %error);
        self.error.get_or_insert(error);
    }

    async fn commit(
        &mut self,
        guard: &SessionWriteGuard,
        record: &SessionRecord,
        changed: bool,
    ) -> bool {
        self.outcomes
            .insert(record.key.clone(), AttemptOutcome::Failed);
        if self.io.cancelled() || self.store.capture_destructive_write_epoch() != self.write_fence {
            return false;
        }
        let keys = [record.key.clone()];
        let returned = match self.store.sessions_with_missing_source_for(&keys) {
            Ok(returned) => returned,
            Err(error) => {
                self.fail(error);
                return false;
            }
        };
        if !changed && !returned.contains(&record.key) {
            return match self.io.checked(self.store.accept_session_if_current(guard)) {
                Ok(true) => {
                    self.outcomes.insert(
                        record.key.clone(),
                        AttemptOutcome::Accepted(Box::new(record.clone()), false),
                    );
                    true
                }
                Ok(false) => false,
                Err(error) => {
                    self.fail(error);
                    false
                }
            };
        }
        let membership_added = match self.io.checked(self.store.session_presence_for_keys(&keys)) {
            Ok((presence, _)) => presence.is_empty(),
            Err(error) => {
                self.fail(error);
                return false;
            }
        };
        let Some((incarnations, revision)) = (match self.io.checked(
            self.store
                .upsert_session_if_current(guard, record, &self.evidence_agents),
        ) {
            Ok(SessionCommitOutcome::Upserted(result)) => Some(result),
            Ok(SessionCommitOutcome::Deleted(_)) => None,
            Ok(SessionCommitOutcome::Rejected(_)) => None,
            Err(error) => {
                self.fail(error);
                return false;
            }
        }) else {
            ::tracing::warn!(
                event = "scan_incremental_commit_stale",
                session_id = %record.key.session_id,
            );
            return false;
        };
        self.io.wake_workers();
        for observation in discovery_report(
            self.io.now(),
            std::slice::from_ref(record),
            &incarnations,
            std::slice::from_ref(&record.key),
            &self.previous_records,
            revision,
        ) {
            self.io.report(observation).await;
        }
        if membership_added {
            self.io.report_index_changed().await;
            self.list_changed = true;
        }
        self.outcomes.insert(
            record.key.clone(),
            AttemptOutcome::Accepted(Box::new(record.clone()), changed),
        );
        self.previous_records.insert(
            SessionActivityKey::new(
                record.key.environment_key.clone(),
                record.key.agent.clone(),
                record.source_label.clone(),
            ),
            record.clone(),
        );
        true
    }

    async fn delete_rejected(&mut self, guard: &SessionWriteGuard, key: &SessionKey) -> bool {
        self.outcomes.insert(key.clone(), AttemptOutcome::Failed);
        if self.io.cancelled() {
            return false;
        }
        match self.io.checked(self.store.delete_session_if_current(guard)) {
            Ok(SessionCommitOutcome::Deleted(removed)) => {
                if let Some((incarnation, revision)) = removed {
                    self.list_changed = true;
                    self.io.wake_workers();
                    self.io
                        .report(session_lifecycle::Observation::Removed {
                            scope: session_lifecycle::RemovalScope::One(key.clone(), incarnation),
                            reason: session_lifecycle::RemovalReason::Rejected,
                            revision,
                        })
                        .await;
                    self.io.report_index_changed().await;
                }
                self.previous_records.retain(|_, record| &record.key != key);
                self.outcomes.insert(key.clone(), AttemptOutcome::Rejected);
                true
            }
            Ok(_) => false,
            Err(error) => {
                self.fail(error);
                false
            }
        }
    }
}

fn physical_source(log: &SessionLog) -> String {
    match &log.source {
        SessionSource::File(path) => format!("file:{}", path.display()),
        SessionSource::ProviderDb {
            db_path,
            session_id,
            ..
        } => format!("providerDb:{}:{session_id}", db_path.display()),
        SessionSource::Inline { label, .. } => format!("inline:{label}"),
    }
}

struct DescriptionLog {
    log: SessionLog,
    key: SessionKey,
    read: Option<antiburn_local::discovery::SourceRead>,
    // Keep the refresh policy when the identity read fails.
    refresh_source: bool,
}

async fn prepare_description_logs(
    mut logs: Vec<SessionLog>,
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
    refresh_file_sources: bool,
) -> anyhow::Result<Vec<DescriptionLog>> {
    sort_logs_newest_first(&mut logs);
    let mut prepared = Vec::with_capacity(logs.len());
    for log in logs {
        let activity_key = SessionActivityKey::new(
            log.environment.key(),
            log.agent_type.slug(),
            log.source_label(),
        );
        let previous = previous_records.get(&activity_key);
        let mut read = None;
        let refresh_source = refresh_file_sources && matches!(&log.source, SessionSource::File(_));
        let id = match &log.source {
            SessionSource::File(_) => {
                if !refresh_source
                    && let Some(record) = reuse_unchanged_record(&log, previous, None).await
                {
                    Some(record.key.session_id)
                } else {
                    read = session_log_read(&log).await;
                    read.as_ref()
                        .and_then(|read| read.metadata.session_id.clone())
                        .or_else(|| recovered_id(&log))
                }
            }
            SessionSource::Inline { content, .. } => scanner::parse_session_metadata_str(content)
                .session_id
                .or_else(|| recovered_id(&log)),
            SessionSource::ProviderDb { session_id, .. } => Some(session_id.clone()),
        };
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            let key = SessionKey::new(log.environment.key(), log.agent_type.slug(), id);
            prepared.push(DescriptionLog {
                log,
                key,
                read,
                refresh_source,
            });
        }
    }
    Ok(prepared)
}

fn session_write_guard(
    store: &Store,
    log: &SessionLog,
    key: &SessionKey,
    epoch: u64,
) -> anyhow::Result<SessionWriteGuard> {
    store.begin_session_write_for_source_at(epoch, key, &log.source_label(), &physical_source(log))
}

fn evidence_agents_for_scope(scope: &PassScope) -> Vec<&'static str> {
    match scope {
        PassScope::Full => agents::evidence_cohort().to_vec(),
        PassScope::Agents(agents) => agents.iter().copied().map(|agent| agent.slug()).collect(),
    }
}

/// One candidate's last activity, computed by [`filter_current_window`]
/// before describe runs. Carrying it forward means describe never asks
/// [`semantic_activity_for_log`] the same question twice.
#[derive(Clone)]
struct CandidateActivity {
    updated_at_epoch: Option<i64>,
    activity_source: String,
    activity_cursor: String,
}

/// The sub-agent transcripts one orchestrator-capable log lists, or an empty
/// list for an agent that records no orchestration. Shared by the current-
/// window filter and by describe, so both ask the engine the same question.
async fn subagent_children_for(log: &SessionLog) -> Vec<std::path::PathBuf> {
    match &log.source {
        SessionSource::File(path)
            if matches!(log.agent_type, AgentKind::Claude | AgentKind::Codex) =>
        {
            Explorers::DISK
                .list_subagents_for_transcript(&log.agent_type, path)
                .await
        }
        _ => Vec::new(),
    }
}

/// Drop a discovery candidate whose last activity predates the current
/// window, so a current pass never admits a session on the strength of a
/// housekeeping-only append to its transcript.
///
/// Shares [`semantic_activity_for_log`]'s event-timestamp rules rather than
/// forking them: a candidate whose source carries no event timestamp at all
/// ("unknown" — a non-file source) is always kept, as today. One agent's
/// candidates at a time, so a later per-agent "found" count can report after
/// this filter runs, and so the first-run UI can report per agent.
///
/// Returns the survivors together with the activity this pass already
/// computed for them, so describe reuses it instead of reading the source a
/// second time.
#[cfg(test)]
async fn filter_current_window(
    logs: Vec<SessionLog>,
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
    cutoff: i64,
) -> Vec<(SessionLog, CandidateActivity)> {
    let mut kept = Vec::with_capacity(logs.len());
    for log in logs {
        let activity_key = SessionActivityKey::new(
            log.environment.key(),
            log.agent_type.slug(),
            log.source_label(),
        );
        let previous = previous_records.get(&activity_key);
        let children = subagent_children_for(&log).await;
        let (updated_at_epoch, activity_source, activity_cursor) =
            semantic_activity_for_log(&log, previous, &children, None).await;
        let current =
            activity_source == "unknown" || updated_at_epoch.is_none_or(|at| at >= cutoff);
        if current {
            kept.push((
                log,
                CandidateActivity {
                    updated_at_epoch,
                    activity_source,
                    activity_cursor,
                },
            ));
        }
    }
    kept
}

/// Apply [`filter_current_window`] to every agent's candidates, and split the
/// survivors back into a plain log list plus the activity describe reuses.
///
/// Discovery returns one merged list across every agent; this groups it back
/// by agent before filtering, so the filter itself stays the one-agent
/// function the design calls for.
/// [`current_window_candidates`], and `on_found` gets each agent's count of
/// current-window candidate files when the filter finishes that agent. An
/// agent that discovery found nothing for gets 0 before the filter starts.
/// The Agents step's card shows this candidate count while discovery runs;
/// once the read stage finishes, `pass` replaces it with the sessions that
/// stage actually admits, so the finished count agrees with what the Read
/// step reads.
#[cfg(test)]
async fn current_window_candidates_with_progress(
    mut logs: Vec<SessionLog>,
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
    now: i64,
    on_found: &mut (dyn FnMut(AgentKind, usize) + Send),
) -> (
    Vec<SessionLog>,
    std::collections::HashMap<SessionActivityKey, CandidateActivity>,
) {
    sort_logs_newest_first(&mut logs);
    let cutoff = now - CURRENT_WINDOW_SECS;
    let mut by_agent: BTreeMap<AgentKind, Vec<SessionLog>> = BTreeMap::new();
    for log in logs {
        by_agent.entry(log.agent_type).or_default().push(log);
    }
    for agent in AgentKind::ALL {
        if !by_agent.contains_key(agent) {
            on_found(*agent, 0);
        }
    }
    let mut survivors = Vec::new();
    for (agent, agent_logs) in by_agent {
        let kept = filter_current_window(agent_logs, previous_records, cutoff).await;
        on_found(agent, kept.len());
        survivors.extend(kept);
    }
    let mut logs = Vec::with_capacity(survivors.len());
    let mut precomputed = std::collections::HashMap::with_capacity(survivors.len());
    for (log, activity) in survivors {
        let activity_key = SessionActivityKey::new(
            log.environment.key(),
            log.agent_type.slug(),
            log.source_label(),
        );
        precomputed.insert(activity_key, activity);
        logs.push(log);
    }
    (logs, precomputed)
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

#[cfg(test)]
async fn describe_with_states(
    logs: Vec<SessionLog>,
    home: &std::path::Path,
    ignored: &std::collections::HashSet<String>,
    previous_records: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
) -> Described {
    describe_with_gate(
        logs,
        DescribeInputs {
            home,
            ignored,
            previous_records,
            precomputed: &std::collections::HashMap::new(),
            include_non_repo_folders: false,
            probe_app: None,
            incremental: None,
        },
        &mut |_, _, _| {},
    )
    .await
}

/// Describe `logs` and apply the repository scan gate.
///
/// `include_non_repo_folders` keeps a session whose CWD has no repository
/// under that CWD. See [`repo_admission`]. `precomputed` holds the activity
/// [`filter_current_window`] already worked out for a candidate, keyed the
/// same way as `previous_records`; a history pass (which skips that filter)
/// passes an empty map, and every candidate falls back to describe's own
/// computation exactly as before.
///
/// `on_progress(completed, total, force)` reports read progress as it runs.
/// `total` starts at `logs.len()` and drops by one for every sub-agent
/// transcript this pass recognizes — a transcript can only be told apart from
/// a session by reading it, so a call never raises `total` back up, and
/// `completed` never exceeds it. `force` is true only for the call after the
/// last log, so a caller that throttles still sees a final, accurate frame.
#[cfg(test)]
struct DescribeInputs<'a, 'incremental> {
    home: &'a std::path::Path,
    ignored: &'a std::collections::HashSet<String>,
    previous_records: &'a std::collections::HashMap<SessionActivityKey, SessionRecord>,
    precomputed: &'a std::collections::HashMap<SessionActivityKey, CandidateActivity>,
    include_non_repo_folders: bool,
    probe_app: Option<&'a AppHandle>,
    incremental: Option<&'incremental mut IncrementalAdmission<'a>>,
}

#[cfg(test)]
async fn describe_with_gate(
    logs: Vec<SessionLog>,
    inputs: DescribeInputs<'_, '_>,
    on_progress: &mut (dyn FnMut(usize, usize, bool) + Send),
) -> Described {
    let DescribeInputs {
        home,
        ignored,
        previous_records,
        precomputed,
        include_non_repo_folders,
        probe_app,
        incremental,
    } = inputs;
    // Scan unit fixtures skip the repository gate. The `repo_admission` tests
    // cover the setting.
    #[cfg(test)]
    let _ = include_non_repo_folders;
    let mut logs = match prepare_description_logs(logs, previous_records, false).await {
        Ok(logs) => logs,
        Err(error) => {
            if let Some(writer) = incremental {
                writer.fail(error);
            }
            return Described {
                records: Vec::new(),
                rejected: Vec::new(),
                changed: Vec::new(),
                list_changed: false,
                gate: GateCounts::default(),
            };
        }
    };
    deduplicate_session_sources(&mut logs);
    let mut total_logs = logs.len();
    let mut records = Vec::with_capacity(logs.len());
    let mut rejected = Vec::new();
    let mut changed = Vec::new();
    let mut list_changed = false;
    let mut gate = GateCounts::default();
    let mut incremental = incremental;
    let mut git_hold = GitHold::default();
    let mut read_completed = 0_usize;
    let mut read_total = total_logs;
    let mut read_done = 0_usize;
    let mut pending = logs;
    let mut scheduled = pending
        .iter()
        .map(|prepared| {
            let log = &prepared.log;
            (
                SessionActivityKey::new(
                    log.environment.key(),
                    log.agent_type.slug(),
                    log.source_label(),
                ),
                log.updated_at,
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut set = JoinSet::new();
    let mut in_flight = std::collections::HashMap::new();
    let mut probes: JoinSet<(Vec<SessionLog>, u64)> = JoinSet::new();
    let mut last_probe_burst = None;
    let mut probe_interval = tokio::time::interval(Duration::from_secs(10));
    probe_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    probe_interval.tick().await;
    let description_concurrency = if probe_app.is_some() {
        METADATA_CONCURRENCY.saturating_sub(1)
    } else {
        METADATA_CONCURRENCY
    };
    let queue_context = DescriptionQueueContext {
        home,
        previous_records,
        precomputed,
        concurrency: description_concurrency,
    };
    while describe_loop_has_work(&set, &pending, &probes) {
        let cancelled = probe_app.is_some_and(|app| app.state::<ScanController>().cancelled())
            || incremental
                .as_ref()
                .is_some_and(|writer| writer.io.cancelled());
        if cancelled {
            pending.clear();
        } else {
            spawn_pending_descriptions(
                &mut set,
                &mut in_flight,
                &mut pending,
                &queue_context,
                incremental.as_deref_mut(),
            );
        }
        if !describe_loop_has_work(&set, &pending, &probes) {
            break;
        }
        let descriptions_done = set.is_empty() && pending.is_empty();
        let event = if let Some(app) = probe_app {
            let controller = app.state::<ScanController>();
            tokio::select! {
                joined = set.join_next(), if !set.is_empty() => joined,
                result = probes.join_next(), if !probes.is_empty() => {
                    if let Some(Ok((found, elapsed_ms))) = result {
                        ::tracing::debug!(event = "scan_opencode_probe_finished", sessions = found.len(), elapsed_ms);
                        for log in found.into_iter().filter(|_| !cancelled) {
                            if enqueue_opencode_probe_log(&mut pending, &mut scheduled, log.clone()) {
                                total_logs += 1;
                                read_total += 1;
                            }
                        }
                    }
                    None
                },
                _ = controller.kick.notified(), if !descriptions_done && !cancelled => {
                    if let Some(burst) = controller.peek_burst() {
                        let signature = (burst.events, burst.paths.clone());
                        if scoped::has_opencode_database_change(&burst.paths)
                            && last_probe_burst.as_ref() != Some(&signature)
                            && probes.is_empty()
                        {
                            last_probe_burst = Some(signature);
                            spawn_opencode_probe(&mut probes);
                        }
                    }
                    None
                },
                _ = probe_interval.tick(), if !descriptions_done && !cancelled => {
                    if probes.is_empty() {
                        spawn_opencode_probe(&mut probes);
                    }
                    None
                },
            }
        } else {
            set.join_next().await
        };
        let Some(joined) = event else {
            continue;
        };
        match &joined {
            Ok((key, _, _, _)) => {
                in_flight.remove(key);
                records.retain(|record: &SessionRecord| &record.key != key);
                rejected.retain(|rejected| rejected != key);
                changed.retain(|changed| changed != key);
            }
            Err(error) => {
                in_flight.retain(|_, id| *id != error.id());
                if let Some(writer) = incremental.as_deref_mut() {
                    writer.fail(anyhow::anyhow!("Session description task failed: {error}"));
                }
            }
        }
        // A sub-agent transcript is a companion, not a session: it
        // shrinks the read total instead of advancing `read_completed`.
        // Checked by reference, and counted before the match below, so
        // every `continue` inside that match still reports progress.
        let is_subagent = matches!(&joined, Ok((_, DescribeOutcome::Subagent(_), _, _)));
        read_done += 1;
        if is_subagent {
            read_total = read_total.saturating_sub(1);
        } else {
            read_completed += 1;
        }
        on_progress(read_completed, read_total, read_done == total_logs);
        if cancelled
            || incremental
                .as_ref()
                .is_some_and(|writer| writer.io.cancelled())
        {
            continue;
        }
        if let Ok((key, outcome, _, _)) = &joined {
            let parsed_key = match outcome {
                DescribeOutcome::Session(record) => Some(&record.key),
                DescribeOutcome::Subagent(key) => Some(key),
                DescribeOutcome::Skip => None,
            };
            if parsed_key.is_some_and(|parsed| parsed != key) && incremental.is_some() {
                if let Some(writer) = incremental.as_deref_mut() {
                    writer.fail(anyhow::anyhow!(
                        "Session identity changed during description"
                    ));
                }
                continue;
            }
        }
        match joined {
            Ok((_, DescribeOutcome::Session(record), changed_record, guard)) => {
                if record.cwd.is_none() {
                    #[cfg(test)]
                    if incremental.is_none() {
                        if changed_record {
                            changed.push(record.key.clone());
                        }
                        records.push(*record);
                        continue;
                    }
                    {
                        gate.missing_cwd += 1;
                        if let Some(writer) = incremental.as_deref_mut() {
                            writer
                                .delete_rejected(
                                    guard.as_ref().expect("a dispatched write has a guard"),
                                    &record.key,
                                )
                                .await;
                        }
                        rejected.push(record.key.clone());
                        continue;
                    }
                }
                let cwd = record.cwd.as_deref().expect("the CWD was checked above");
                if ignored_paths::set_contains(ignored, cwd) {
                    gate.ignored += 1;
                    if let Some(writer) = incremental.as_deref_mut() {
                        writer
                            .delete_rejected(
                                guard.as_ref().expect("a dispatched write has a guard"),
                                &record.key,
                            )
                            .await;
                    }
                    rejected.push(record.key.clone());
                    continue;
                }
                // Scan unit fixtures use synthetic paths instead of Git
                // repositories. Production always resolves the repository.
                #[cfg(test)]
                if incremental.is_none() {
                    if changed_record {
                        changed.push(record.key.clone());
                    }
                    records.push(*record);
                    continue;
                }
                {
                    let cwd = cwd.to_string();
                    let mut record = record;
                    let mut changed_record = changed_record;
                    let admission = repo_admission(&record, &cwd, include_non_repo_folders).await;
                    let ControlFlow::Continue(root) = apply_repo_admission(
                        admission,
                        &mut record,
                        &mut changed_record,
                        &mut gate,
                        &mut rejected,
                        &mut git_hold,
                    ) else {
                        if rejected.contains(&record.key)
                            && let Some(writer) = incremental.as_deref_mut()
                        {
                            writer
                                .delete_rejected(
                                    guard.as_ref().expect("a dispatched write has a guard"),
                                    &record.key,
                                )
                                .await;
                        }
                        continue;
                    };
                    if let Some(root) = root {
                        let root = git::canonical_main_repo_root(&root).await;
                        // Apply the shared opt-out gate to both the working
                        // directory and the canonical main root. This also
                        // covers linked worktrees. Use `record.cwd`: it holds
                        // the inferred repository when the scan moved the
                        // session from a parent folder.
                        if ignored_paths::is_session_ignored(
                            ignored,
                            record.cwd.as_deref().or(Some(&cwd)),
                            &root.to_string_lossy(),
                        ) {
                            gate.ignored += 1;
                            if let Some(writer) = incremental.as_deref_mut() {
                                writer
                                    .delete_rejected(
                                        guard.as_ref().expect("a dispatched write has a guard"),
                                        &record.key,
                                    )
                                    .await;
                            }
                            rejected.push(record.key.clone());
                            continue;
                        }
                    }
                    if changed_record {
                        changed.push(record.key.clone());
                    }
                    if let Some(writer) = incremental.as_deref_mut()
                        && !writer
                            .commit(
                                guard.as_ref().expect("a dispatched write has a guard"),
                                &record,
                                changed_record,
                            )
                            .await
                    {
                        continue;
                    }
                    records.push(*record);
                }
            }
            Ok((_, DescribeOutcome::Subagent(key), _, guard)) => {
                gate.subagent += 1;
                if let Some(writer) = incremental.as_deref_mut() {
                    writer
                        .delete_rejected(
                            guard.as_ref().expect("a dispatched write has a guard"),
                            &key,
                        )
                        .await;
                }
                rejected.push(key);
            }
            Ok((_, DescribeOutcome::Skip, _, _)) | Err(_) => {}
        }
    }
    if let Some(error) = &git_hold.error {
        ::tracing::warn!(
            event = "scan_git_unavailable",
            sessions = git_hold.sessions,
            error = %error,
        );
        if let Some(writer) = incremental.as_deref_mut() {
            writer.fail(anyhow::anyhow!(
                "Git cannot resolve session repositories: {error}"
            ));
        }
    }
    if gate != GateCounts::default() {
        ::tracing::debug!(
            event = "scan_repo_gate",
            missing_cwd = gate.missing_cwd,
            ignored = gate.ignored,
            subagent = gate.subagent,
            no_repo = gate.no_repo,
            folder = gate.folder,
        );
    }
    // Fixtures without a writer still expose the gate's membership decision.
    if !rejected.is_empty() {
        list_changed = true;
    }
    let previously_known = previously_known_keys(previous_records);
    if changed.iter().any(|key| !previously_known.contains(key)) {
        list_changed = true;
    }
    if let Some(writer) = incremental {
        records.clear();
        changed.clear();
        rejected.clear();
        for (key, outcome) in &writer.outcomes {
            match outcome {
                AttemptOutcome::Accepted(record, refreshed) => {
                    records.push((**record).clone());
                    if *refreshed {
                        changed.push(key.clone());
                    }
                }
                AttemptOutcome::Rejected => rejected.push(key.clone()),
                AttemptOutcome::Failed => {}
            }
        }
        records.sort_by(|left, right| left.key.cmp(&right.key));
        changed.sort();
        rejected.sort();
        list_changed = writer.list_changed;
    }
    Described {
        records,
        rejected,
        changed,
        list_changed,
        gate,
    }
}

#[cfg(test)]
fn describe_loop_has_work<T, U>(
    descriptions: &JoinSet<T>,
    pending: &[DescriptionLog],
    probes: &JoinSet<U>,
) -> bool {
    !descriptions.is_empty() || !pending.is_empty() || !probes.is_empty()
}

/// Map the read stage's internal gate tally to the IPC payload shape. `kept`
/// is not a [`GateCounts`] field: it is the caller's own count of records
/// that passed the gate, since [`GateCounts`] only tracks exclusions.
fn read_gate_counts(gate: &GateCounts, kept: usize) -> ReadGateCounts {
    ReadGateCounts {
        kept,
        outside_repository: gate.no_repo,
        excluded: gate.ignored,
        unreadable: gate.missing_cwd,
    }
}

/// Why one scan pass rejected sessions, and how many it kept as folders. The
/// counts go to the log, so a missing session has a reason.
#[derive(Debug, Default, PartialEq, Eq)]
struct GateCounts {
    missing_cwd: usize,
    ignored: usize,
    subagent: usize,
    no_repo: usize,
    folder: usize,
}

/// How the repository scan gate files a session.
#[derive(Debug, PartialEq, Eq)]
enum RepoAdmission {
    /// The CWD is in this repository.
    Repository(std::path::PathBuf),
    /// The CWD is a parent folder, and the transcript worked in this
    /// repository below it.
    InferredRepository(std::path::PathBuf),
    /// No repository applies, and the settings keep the session under its CWD.
    Folder,
    /// No repository applies.
    Rejected,
    /// Git cannot run, so the gate cannot decide. The string holds the error.
    GitUnavailable(String),
}

/// Apply the repository scan gate to one session with the CWD `cwd`.
async fn repo_admission(
    record: &SessionRecord,
    cwd: &str,
    include_non_repo_folders: bool,
) -> RepoAdmission {
    let cwd = std::path::Path::new(cwd);
    match git::repo_root_if_any_at(cwd).await {
        Ok(Some(root)) => return RepoAdmission::Repository(root),
        // Only a CWD that Git reports as outside every repository can move
        // to a repository below it or stay as a folder.
        Ok(None) => {}
        Err(error) => return git_error_admission(&error, git_runs(cwd).await),
    }
    if record.source_kind == "file"
        && let Some(root) =
            scanner::infer_repo_root_below_cwd(std::path::Path::new(&record.source_label), cwd)
                .await
    {
        return RepoAdmission::InferredRepository(root);
    }
    if include_non_repo_folders {
        RepoAdmission::Folder
    } else {
        RepoAdmission::Rejected
    }
}

/// Sessions that the repository gate holds because Git cannot run.
#[derive(Debug, Default, PartialEq, Eq)]
struct GitHold {
    sessions: usize,
    /// The first Git error of the pass.
    error: Option<String>,
}

/// Apply one [`RepoAdmission`] to `record`.
///
/// `Continue` keeps the record, with the repository root to check against the
/// opt-out list, if any. `Break` drops the record from this pass. A rejected
/// record also goes into `rejected`, and the scan deletes its stored row. A
/// held record does not: the scan keeps its stored row, if any, adds no new
/// row, and admits the session on a later pass when Git runs again.
fn apply_repo_admission(
    admission: RepoAdmission,
    record: &mut SessionRecord,
    changed_record: &mut bool,
    gate: &mut GateCounts,
    rejected: &mut Vec<SessionKey>,
    git_hold: &mut GitHold,
) -> ControlFlow<(), Option<std::path::PathBuf>> {
    match admission {
        RepoAdmission::Repository(root) => ControlFlow::Continue(Some(root)),
        RepoAdmission::InferredRepository(root) => {
            record.cwd = Some(root.to_string_lossy().into_owned());
            // Persist the new CWD, also for a reused record.
            *changed_record = true;
            ControlFlow::Continue(Some(root))
        }
        RepoAdmission::Folder => {
            gate.folder += 1;
            ControlFlow::Continue(None)
        }
        RepoAdmission::Rejected => {
            gate.no_repo += 1;
            rejected.push(record.key.clone());
            ControlFlow::Break(())
        }
        RepoAdmission::GitUnavailable(error) => {
            git_hold.sessions += 1;
            git_hold.error.get_or_insert(error);
            ControlFlow::Break(())
        }
    }
}

/// File a session when Git fails for its CWD. A Git that cannot run at all
/// (for example, macOS blocks `/usr/bin/git` until the Xcode license is
/// accepted) says nothing about the CWD, so the gate does not reject it.
fn git_error_admission(error: &anyhow::Error, git_runs: bool) -> RepoAdmission {
    if git_runs {
        RepoAdmission::Rejected
    } else {
        RepoAdmission::GitUnavailable(format!("{error:#}"))
    }
}

/// The environment that runs Git for `cwd`. This is the same selection that
/// `git::repo_root_if_any_at` uses, so the probe tests the Git that failed.
fn git_probe_environment(cwd: &std::path::Path) -> DiscoveryEnvironment {
    environment::environment_from_mounted_path(cwd).unwrap_or_default()
}

/// True when `git --version` runs and succeeds in the environment for `cwd`.
/// The probe does not pass `cwd` to Git, so a missing CWD cannot fail it.
async fn git_runs(cwd: &std::path::Path) -> bool {
    git::run_git_output_in_environment(&git_probe_environment(cwd), None, &["--version"], &[])
        .await
        .is_ok_and(|output| output.status.success())
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
#[cfg(test)]
async fn indexed_titles_for_logs(
    logs: &[SessionLog],
) -> std::collections::HashMap<(AgentKind, String), ResolvedTitle> {
    let mut indexed = std::collections::HashMap::new();
    for agent in AgentKind::ALL {
        let mut session_ids: Vec<String> = logs
            .iter()
            .filter(|log| log.agent_type == *agent && should_batch_indexed_title(log))
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

fn sort_logs_newest_first(logs: &mut [SessionLog]) {
    logs.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.agent_type.slug().cmp(right.agent_type.slug()))
            .then_with(|| left.environment.key().cmp(&right.environment.key()))
            .then_with(|| physical_source(left).cmp(&physical_source(right)))
    });
}

#[cfg(test)]
fn sort_description_logs(logs: &mut [DescriptionLog]) {
    logs.sort_by(|left, right| {
        right
            .log
            .updated_at
            .cmp(&left.log.updated_at)
            .then_with(|| left.log.agent_type.slug().cmp(right.log.agent_type.slug()))
            .then_with(|| left.log.environment.key().cmp(&right.log.environment.key()))
            .then_with(|| physical_source(&left.log).cmp(&physical_source(&right.log)))
    });
}

#[cfg(test)]
fn deduplicate_session_sources(logs: &mut Vec<DescriptionLog>) {
    sort_description_logs(logs);
    let mut seen = std::collections::HashSet::new();
    logs.retain(|log| seen.insert(log.key.clone()));
}

#[cfg(test)]
fn enqueue_opencode_probe_log(
    pending: &mut Vec<DescriptionLog>,
    scheduled: &mut std::collections::HashMap<SessionActivityKey, Option<i64>>,
    log: SessionLog,
) -> bool {
    if log.agent_type != AgentKind::OpenCode
        || !matches!(&log.source, SessionSource::ProviderDb { .. })
    {
        return false;
    }
    let key = SessionActivityKey::new(
        log.environment.key(),
        log.agent_type.slug(),
        log.source_label(),
    );
    let SessionSource::ProviderDb { session_id, .. } = &log.source else {
        return false;
    };
    let session_key = SessionKey::new(log.environment.key(), log.agent_type.slug(), session_id);
    let prepared = DescriptionLog {
        log,
        key: session_key,
        read: None,
        refresh_source: false,
    };
    let log = &prepared.log;
    let added = match scheduled.get_mut(&key) {
        Some(previous_at) if log.updated_at > *previous_at => {
            *previous_at = log.updated_at;
            if let Some(queued) = pending.iter_mut().find(|queued| queued.key == prepared.key) {
                *queued = prepared;
                false
            } else {
                pending.push(prepared);
                true
            }
        }
        Some(_) => false,
        None => {
            scheduled.insert(key, log.updated_at);
            pending.push(prepared);
            true
        }
    };
    sort_description_logs(pending);
    added
}

fn should_batch_indexed_title(log: &SessionLog) -> bool {
    should_lookup_indexed_title(log)
        && !matches!(
            (&log.agent_type, &log.source),
            (AgentKind::OpenCode, SessionSource::ProviderDb { .. })
        )
}

#[cfg(test)]
struct DescriptionQueueContext<'a> {
    home: &'a std::path::Path,
    previous_records: &'a std::collections::HashMap<SessionActivityKey, SessionRecord>,
    precomputed: &'a std::collections::HashMap<SessionActivityKey, CandidateActivity>,
    concurrency: usize,
}

#[cfg(test)]
fn spawn_pending_descriptions(
    set: &mut JoinSet<(SessionKey, DescribeOutcome, bool, Option<SessionWriteGuard>)>,
    in_flight: &mut std::collections::HashMap<SessionKey, tokio::task::Id>,
    pending: &mut Vec<DescriptionLog>,
    context: &DescriptionQueueContext<'_>,
    mut incremental: Option<&mut IncrementalAdmission<'_>>,
) {
    while set.len() < context.concurrency && !pending.is_empty() {
        let Some(index) = pending
            .iter()
            .position(|log| !in_flight.contains_key(&log.key))
        else {
            break;
        };
        let description = pending.remove(index);
        let log = &description.log;
        let key = description.key.clone();
        let guard = if let Some(writer) = incremental.as_deref_mut() {
            writer.outcomes.insert(key.clone(), AttemptOutcome::Failed);
            match session_write_guard(writer.store, log, &key, writer.write_fence) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    writer.fail(error);
                    continue;
                }
            }
        } else {
            None
        };
        let activity_key = SessionActivityKey::new(
            log.environment.key(),
            log.agent_type.slug(),
            log.source_label(),
        );
        let previous = incremental
            .as_ref()
            .and_then(|writer| writer.previous_records.get(&activity_key))
            .or_else(|| context.previous_records.get(&activity_key))
            .cloned();
        let activity = context
            .precomputed
            .get(&activity_key)
            .filter(|activity| {
                previous
                    .as_ref()
                    .is_none_or(|record| record.activity_cursor == activity.activity_cursor)
            })
            .cloned();
        let home = context.home.to_path_buf();
        let task_key = key.clone();
        let handle = set.spawn(async move {
            let indexed_titles =
                indexed_titles_for_logs(std::slice::from_ref(&description.log)).await;
            let indexed_title = recovered_id(&description.log).and_then(|id| {
                indexed_titles
                    .get(&(description.log.agent_type, id))
                    .cloned()
            });
            let (_, outcome, changed) = describe_candidate_with_read(
                description,
                home,
                previous,
                activity,
                indexed_title,
                Instant::now(),
            )
            .await;
            (task_key, outcome, changed, guard)
        });
        in_flight.insert(key, handle.id());
    }
}

#[cfg(test)]
fn spawn_opencode_probe(probes: &mut JoinSet<(Vec<SessionLog>, u64)>) {
    let explorer = Explorers::DISK.get(&AgentKind::OpenCode);
    probes.spawn(async move {
        let started = Instant::now();
        ::tracing::debug!(event = "scan_opencode_probe_started");
        let now = unix_now();
        let (now, since_secs) = opencode_probe_window(now);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        let discover =
            explorer.discover_recent_batches(AgentKind::OpenCode, now, since_secs, 64, sender);
        let receive = async move {
            let mut logs = Vec::new();
            while let Some(batch) = receiver.recv().await {
                logs.extend(batch.candidates);
            }
            logs
        };
        let (_, logs) = tokio::join!(discover, receive);
        (logs, started.elapsed().as_millis() as u64)
    });
}

/// The explorer API takes a lookback duration, not an absolute cutoff.
#[cfg(test)]
fn opencode_probe_window(now: i64) -> (i64, i64) {
    (now, CURRENT_WINDOW_SECS)
}

enum DescribeOutcome {
    Session(Box<SessionRecord>),
    /// A sub-agent transcript: never listed, and evicted if an earlier,
    /// ungated version of the app indexed it.
    Subagent(SessionKey),
    Skip,
}

async fn describe_candidate_with_read(
    description: DescriptionLog,
    home: PathBuf,
    previous: Option<SessionRecord>,
    activity: Option<CandidateActivity>,
    indexed_title: Option<ResolvedTitle>,
    queued_at: Instant,
) -> (SessionActivityKey, DescribeOutcome, bool) {
    let DescriptionLog {
        log,
        read,
        refresh_source,
        ..
    } = description;
    let agent = log.agent_type.slug();
    let source_label = log.source_label();
    let activity_key = SessionActivityKey::new(
        log.environment.key(),
        log.agent_type.slug(),
        source_label.clone(),
    );
    let activity_at = log.updated_at;
    let started_at = Instant::now();
    ::tracing::debug!(
        event = "scan_description_started",
        agent,
        source_label = %source_label,
        activity_at,
        queue_wait_ms = queued_at.elapsed().as_millis() as u64,
    );
    let result = if !refresh_source
        && let Some(reused) =
            reuse_unchanged_record(&log, previous.as_ref(), indexed_title.as_ref()).await
    {
        let changed = previous.as_ref().is_some_and(|stored| stored != &reused);
        (DescribeOutcome::Session(Box::new(reused)), changed)
    } else {
        let outcome =
            describe_one_with_activity(log, &home, indexed_title, previous, activity, read).await;
        (outcome, true)
    };
    ::tracing::debug!(
        event = "scan_description_finished",
        agent,
        source_label = %source_label,
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        outcome = match &result.0 {
            DescribeOutcome::Session(_) => "session",
            DescribeOutcome::Subagent(_) => "subagent",
            DescribeOutcome::Skip => "skip",
        },
    );
    (activity_key, result.0, result.1)
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
    source_activity_cursor(log.agent_type, path, size, child_sizes).await
}

/// Include Claude delegation metadata without changing other agents' activity cursors.
async fn source_activity_cursor(
    agent: AgentKind,
    parent: &std::path::Path,
    size: u64,
    children: Vec<(std::path::PathBuf, Option<u64>)>,
) -> Option<String> {
    let cursor = activity_cursor(parent, size, &children);
    if agent != AgentKind::Claude || children.is_empty() {
        return Some(cursor);
    }
    tokio::task::spawn_blocking(move || {
        let sidecars = children
            .iter()
            .map(|(path, _)| {
                antiburn_local::discovery::source_version::claude_sidecar_fingerprint(path)
                    .map(|fingerprint| (path, fingerprint))
            })
            .collect::<std::io::Result<Vec<_>>>()
            .ok()?;
        Some(serde_json::to_string(&(cursor, sidecars)).expect("source cursor is serializable"))
    })
    .await
    .ok()
    .flatten()
}

/// Reuse a previous record when its source has not changed.
/// Apply a fresh indexed title without opening the transcript.
async fn reuse_unchanged_record(
    log: &SessionLog,
    previous: Option<&SessionRecord>,
    indexed_title: Option<&ResolvedTitle>,
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
    if !mtime_matches {
        return None;
    }
    let mut reused = previous.clone();
    if let Some(indexed_title) = indexed_title {
        apply_indexed_title(&mut reused, &log.agent_type, indexed_title.clone());
    }
    Some(reused)
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
    let cursor = source_activity_cursor(log.agent_type, path, size, child_sizes)
        .await
        .unwrap_or_default();

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
    describe_one_with_activity(log, home, indexed_title, None, None, None).await
}

async fn describe_one_with_activity(
    log: SessionLog,
    home: &std::path::Path,
    indexed_title: Option<ResolvedTitle>,
    previous: Option<SessionRecord>,
    precomputed_activity: Option<CandidateActivity>,
    read: Option<antiburn_local::discovery::SourceRead>,
) -> DescribeOutcome {
    let bounded_revision = if fingerprint::uses_bounded_admission(&log) {
        let Some(revision) = fingerprint::admission_revision(&log).await else {
            return DescribeOutcome::Skip;
        };
        Some(revision)
    } else {
        None
    };
    let metadata_started = Instant::now();
    let read = match read {
        Some(read) => Some(read),
        None => session_log_read(&log).await,
    };
    ::tracing::debug!(
        event = "scan_metadata_finished",
        agent = log.agent_type.slug(),
        elapsed_ms = metadata_started.elapsed().as_millis() as u64,
        available = read.is_some()
    );
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
    let lineage_started = Instant::now();
    let children = subagent_children_for(&log).await;
    let subagent_count = children.len() as u32;
    let fork_parent_session_id = fork_parent_session_id_for(&log, preview).await;
    ::tracing::debug!(
        event = "scan_lineage_finished",
        agent = log.agent_type.slug(),
        elapsed_ms = lineage_started.elapsed().as_millis() as u64,
        children = children.len()
    );

    // Reuse the current-window filter's answer only when it found an event
    // timestamp. The filter reads the tail only, so its mtime fallback can
    // miss events that the preview holds.
    let (updated_at_epoch, activity_source, activity_cursor) =
        match precomputed_activity.filter(|activity| activity.activity_source == "event") {
            Some(activity) => (
                activity.updated_at_epoch,
                activity.activity_source,
                activity.activity_cursor,
            ),
            None => semantic_activity_for_log(&log, previous.as_ref(), &children, preview).await,
        };
    let surface = session_surface(&log, preview, home);
    let descriptor = SourceDescriptor {
        agent: log.agent_type,
        session_id: session_id.clone(),
        environment: log.environment.clone(),
        source: log.source.clone(),
        source_format: crate::analysis::source_format(log.agent_type, &log.source),
        surface: surface.to_string(),
        updated_at_epoch: log.updated_at,
    };
    let source_fingerprint = if bounded_revision.is_some() {
        None
    } else {
        Explorers::DISK
            .source_version(&descriptor, read.as_ref())
            .await
            .map(|version| version.fingerprint)
    };
    let activity_cursor = if let Some(revision) = bounded_revision {
        if fingerprint::admission_revision(&log).await.as_ref() != Some(&revision) {
            return DescribeOutcome::Skip;
        }
        revision
    } else {
        activity_cursor
    };

    DescribeOutcome::Session(Box::new(SessionRecord {
        key,
        source_kind: source_kind(&log.source).to_string(),
        source_label: log.source_label(),
        wsl_distro: log.environment.wsl_distro().map(str::to_string),
        title,
        title_source,
        cwd: metadata.and_then(|metadata| metadata.cwd.clone()),
        surface: surface.to_string(),
        updated_at_epoch,
        activity_cursor,
        activity_source,
        subagent_count,
        fork_parent_session_id,
        source_fingerprint,
    }))
}

/// The session's surface label. Claude Code, the VS Code extension, and the
/// Claude Desktop Code tab all write to `~/.claude/projects`, so for Claude
/// the path alone gives `cli`. The `entrypoint` marker in the transcript
/// head gives the correct surface.
fn session_surface(
    log: &SessionLog,
    preview: Option<&str>,
    home: &std::path::Path,
) -> &'static str {
    match (log.agent_type, preview) {
        (AgentKind::Claude, Some(content)) => log.surface_label_with_content(content, home),
        _ => log.surface_label(home),
    }
}

/// The fork parent this session's own source declares, if any.
///
/// Every vendor's evidence is bounded: the transcript preview already read
/// above for a file or an inline source, a direct database check for
/// OpenCode (`db_fork_parent` never renders the full transcript), and a
/// bounded preview read for every other provider database. This runs only
/// when describe runs, which the size cursor already gates, so it costs
/// nothing for a session that has not changed.
///
/// Claude's own "resume as fork" writes no such marker. The insights worker
/// links those fork sessions from published turn rows instead, once rows
/// exist — see `fork_lineage`.
async fn fork_parent_session_id_for(log: &SessionLog, preview: Option<&str>) -> Option<String> {
    match &log.source {
        SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path,
            session_id,
            ..
        } => {
            antiburn_local::discovery::agents::opencode::db_fork_parent(
                db_path.clone(),
                session_id.clone(),
            )
            .await
        }
        SessionSource::ProviderDb { .. } => {
            let content = session_source_preview(&log.source).await?;
            analysis::fork_parent_from_content(&content)
        }
        SessionSource::Inline { content, .. } => analysis::fork_parent_from_content(content),
        SessionSource::File(_)
            if matches!(log.agent_type, AgentKind::Claude | AgentKind::Codex) =>
        {
            preview.and_then(analysis::fork_parent_from_content)
        }
        _ => None,
    }
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

fn apply_indexed_title(
    record: &mut SessionRecord,
    agent: &AgentKind,
    indexed_title: ResolvedTitle,
) -> bool {
    let (title, title_source) = select_title_pair(Some(indexed_title), None, None, agent, None);
    if record.title == title && record.title_source == title_source {
        return false;
    }
    record.title = title;
    record.title_source = title_source;
    true
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

/// Replace discovery's candidate-file count in every `found_by_agent` entry
/// with the sessions this pass actually admits for that agent — 0 for an
/// agent with none. `records` is the read stage's output, after it has
/// dropped every sub-agent transcript and gate-rejected session, so this is
/// the count the Agents list and its icon row must agree on. `done` was
/// already true from discovery, and stays true.
fn apply_admitted_agent_counts(found_by_agent: &mut [AgentFoundCount], records: &[SessionRecord]) {
    let totals = per_agent_totals(records);
    let admitted: std::collections::BTreeMap<&str, usize> = totals
        .iter()
        .map(|(agent, seen, _cursor)| (agent.as_str(), *seen as usize))
        .collect();
    for entry in found_by_agent.iter_mut() {
        entry.sessions = admitted.get(entry.agent.as_str()).copied().unwrap_or(0);
        entry.done = true;
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

mod owner;
mod prepare;
mod queue;
#[cfg(test)]
mod tests;
