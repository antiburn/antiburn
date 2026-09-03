//! Filesystem watcher: turns raw OS events into debounced pass requests.
//!
//! An event under a watched root does not describe a session directly — it
//! only tells the debouncer that something changed. After a quiet period the
//! debouncer asks the scheduler for one pass, and that pass walks discovery
//! the same way a tick does. This keeps one code path for discovery, and it
//! makes reconciliation trivial: a missed or coalesced event costs nothing
//! beyond the delay to the next tick.
//!
//! # Roots
//!
//! Each agent's [`AgentExplorer::watch_roots`](antiburn_local::discovery::AgentExplorer::watch_roots)
//! lists the directories its own discovery reads. Only roots that exist are
//! watched at start. A periodic re-check adds roots that appear later (a
//! freshly installed agent), on the same cadence as [`super::TICK`].
//!
//! # Debouncing
//!
//! [`QUIET_WINDOW`] and [`MAX_WAIT`] are defaults tuned by feel, not measured
//! constants: a quiet window short enough that a reader who opens the popover
//! right after a burst of writes still sees them, and a maximum wait so an
//! actively-writing session still gets a pass at a bounded interval even
//! under a steady stream of events.
//!
//! # Noise
//!
//! `Access`-only events (a file opened for reading — including antiburn's own
//! pass) never reach the debouncer. Events under a WSL mount are dropped too:
//! WSL sessions run through a separate discovery walk on the tick, per
//! [`crate::scan`]'s module doc.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use antiburn_local::discovery::{Explorers, WatchRoot};
use antiburn_local::model::AgentKind;
use antiburn_local::paths::home_dir;
use antiburn_local::platform::environment::environment_from_mounted_path;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::Instant;

/// Quiet period after the last relevant event before a pass is requested.
pub const QUIET_WINDOW: Duration = Duration::from_millis(1500);

/// Upper bound on how long an unbroken stream of events can delay a pass.
pub const MAX_WAIT: Duration = Duration::from_secs(5);

/// Tick while the watcher is not fully healthy: it failed to start, or one or
/// more existing roots could not be watched. Polling alone must still find
/// new sessions at a reasonable cadence.
pub const FALLBACK_TICK: Duration = Duration::from_secs(15);

/// How often the watcher re-lists every agent's roots and watches any that
/// now exist but did not when it started (or at the last re-check).
const ROOT_RECHECK_INTERVAL: Duration = super::TICK;

/// What starting the watcher produced. The scheduler reads this once, right
/// after start, to pick its tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatcherStatus {
    /// Whether the OS watcher itself started.
    pub active: bool,
    /// Existing roots the watcher could not watch. Discovery still reads
    /// them on the tick; only the acceleration is missing.
    pub failed_roots: Vec<PathBuf>,
}

impl WatcherStatus {
    /// Whether the scheduler should use [`super::TICK`] rather than
    /// [`FALLBACK_TICK`].
    pub fn is_healthy(&self) -> bool {
        self.active && self.failed_roots.is_empty()
    }
}

/// Start the filesystem watcher and its debounce task. Returns immediately;
/// the debounce loop and the periodic root re-check outlive this call and run
/// until the process exits.
pub fn spawn_watcher(app: &AppHandle) -> WatcherStatus {
    let Some(home) = home_dir() else {
        // No resolvable home means discovery itself finds nothing either;
        // report inactive so the scheduler falls back to the faster tick.
        return WatcherStatus::default();
    };
    let app = app.clone();
    spawn_watcher_over(home, move |burst: WatchBurst| {
        app.state::<crate::scan::ScanController>()
            .request(crate::scan::ScanTrigger::Watcher {
                events: burst.events,
                paths: burst.paths,
            });
    })
}

/// The roots every agent's discovery reads, for a given home. One list, so
/// the watcher and its periodic re-check share one source of truth.
fn all_watch_roots(home: &Path) -> Vec<WatchRoot> {
    AgentKind::ALL
        .iter()
        .flat_map(|agent| Explorers::DISK.watch_roots_for(agent, home))
        .collect()
}

/// Testable core of [`spawn_watcher`]: build the roots, start the OS watcher,
/// and spawn the tasks that debounce events into `on_relevant_change` calls.
/// Kept separate from Tauri so the debounce timing can be driven directly in
/// tests, under `tokio::time::pause()`, without a running app.
fn spawn_watcher_over(
    home: PathBuf,
    on_relevant_change: impl Fn(WatchBurst) + Send + Sync + 'static,
) -> WatcherStatus {
    let roots = all_watch_roots(&home);
    let (tx, rx) = unbounded_channel::<Event>();
    let mut watcher = match RecommendedWatcher::new(
        move |result: notify::Result<Event>| {
            if let Ok(event) = result {
                // The debounce task may already be gone at shutdown; a send
                // failure there is not this callback's problem.
                let _ = tx.send(event);
            }
        },
        notify::Config::default(),
    ) {
        Ok(watcher) => watcher,
        Err(_) => return WatcherStatus::default(),
    };

    let mut watched = HashSet::new();
    let failed_roots = watch_new_roots(&mut watcher, &roots, &mut watched);

    // Plain `tokio::spawn`, not `tauri::async_runtime::spawn`: this function
    // is called from inside the scheduler's own task, so the ambient runtime
    // is already the app's, and a plain spawn keeps this testable with
    // `#[tokio::test(start_paused = true)]` under its own runtime.
    tokio::spawn(run_debounce_loop(rx, on_relevant_change));
    tokio::spawn(run_root_recheck_loop(watcher, home, watched));

    WatcherStatus {
        active: true,
        failed_roots,
    }
}

/// Watch every root in `roots` that exists and is not already in `watched`.
/// Returns the roots that exist but could not be watched.
fn watch_new_roots(
    watcher: &mut RecommendedWatcher,
    roots: &[WatchRoot],
    watched: &mut HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for root in roots {
        if watched.contains(&root.path) || !root.path.exists() {
            continue;
        }
        let mode = if root.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        match watcher.watch(&root.path, mode) {
            Ok(()) => {
                watched.insert(root.path.clone());
            }
            Err(_) => failed.push(root.path.clone()),
        }
    }
    failed
}

/// Owns the [`RecommendedWatcher`] for the process's life. On each
/// [`ROOT_RECHECK_INTERVAL`], re-lists every agent's roots and watches any
/// that appeared since the last check — a freshly installed agent, or a root
/// that failed to watch earlier because it did not exist yet.
async fn run_root_recheck_loop(
    mut watcher: RecommendedWatcher,
    home: PathBuf,
    mut watched: HashSet<PathBuf>,
) {
    let mut ticks = tokio::time::interval(ROOT_RECHECK_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick fires immediately; the roots are already watched.
    ticks.tick().await;
    loop {
        ticks.tick().await;
        let roots = all_watch_roots(&home);
        watch_new_roots(&mut watcher, &roots, &mut watched);
    }
}

/// W2: the most paths one burst stores. Past this bound the burst keeps
/// counting events but stops storing paths, so a runaway burst cannot grow
/// the debouncer's memory with the size of the change. A consumer that sees
/// a full sample must treat the burst as possibly incomplete.
pub const MAX_BURST_PATHS: usize = 64;

/// One coalesced run of relevant filesystem events, handed to
/// `on_relevant_change` after the burst's quiet period ends.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WatchBurst {
    /// Relevant paths seen in this burst, deduplicated, in first-seen order,
    /// bounded at [`MAX_BURST_PATHS`].
    pub paths: Vec<PathBuf>,
    /// Every relevant event coalesced into this burst. Counts past
    /// [`MAX_BURST_PATHS`] even after the path sample stops growing.
    pub events: usize,
}

impl WatchBurst {
    /// Fold one relevant event's paths into the burst.
    fn record(&mut self, event: &Event) {
        self.events += 1;
        for path in &event.paths {
            if self.paths.len() >= MAX_BURST_PATHS {
                break;
            }
            if !self.paths.contains(path) {
                self.paths.push(path.clone());
            }
        }
    }
}

/// Consume events until the channel closes, requesting one pass after each
/// quiet period. See the module doc for the debounce shape.
async fn run_debounce_loop(
    mut events: UnboundedReceiver<Event>,
    on_relevant_change: impl Fn(WatchBurst),
) {
    while let Some(first) = events.recv().await {
        if !is_relevant(&first) {
            continue;
        }
        let mut burst = WatchBurst::default();
        burst.record(&first);
        wait_for_quiet(&mut events, QUIET_WINDOW, MAX_WAIT, &mut burst).await;
        on_relevant_change(burst);
    }
}

/// After a relevant event, keep consuming events while more relevant ones
/// keep arriving inside the quiet window, up to the maximum wait. Only a
/// relevant event resets the quiet window; noise never extends it. Every
/// relevant event folds its paths into `burst` (W2).
async fn wait_for_quiet(
    events: &mut UnboundedReceiver<Event>,
    quiet: Duration,
    max_wait: Duration,
    burst: &mut WatchBurst,
) {
    let deadline = Instant::now() + max_wait;
    let mut quiet_deadline = Instant::now() + quiet;
    loop {
        let wait_until = quiet_deadline.min(deadline);
        tokio::select! {
            maybe_event = events.recv() => {
                let Some(event) = maybe_event else {
                    return;
                };
                if is_relevant(&event) {
                    burst.record(&event);
                    quiet_deadline = Instant::now() + quiet;
                }
                if Instant::now() >= deadline {
                    return;
                }
            }
            () = tokio::time::sleep_until(wait_until) => {
                return;
            }
        }
    }
}

/// Drop `Access`-only events (including antiburn's own reads), events whose
/// every path sits under a WSL mount — WSL sessions run through the tick's
/// own discovery walk, not this watcher — and events whose every path is a
/// SQLite `-shm` sidecar.
///
/// W1: a WAL reader takes read marks in the `-shm` file, which bumps its
/// mtime. antiburn's own read-only open of a vendor database (Cursor's
/// `state.vscdb`) therefore modifies `-shm` on every pass, and without this
/// rule the watcher would re-kick a pass the moment the previous one finished
/// reading. `-wal` stays relevant: only a real writer appends to it, and a
/// WAL database's committed rows live there until a checkpoint.
fn is_relevant(event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        return true;
    }
    if event.paths.iter().all(|path| is_shm_sidecar_path(path)) {
        return false;
    }
    !event.paths.iter().all(|path| is_wsl_mount_path(path))
}

fn is_wsl_mount_path(path: &Path) -> bool {
    environment_from_mounted_path(path).is_some()
}

fn is_shm_sidecar_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("-shm"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, ModifyKind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn modify_event(path: &Path) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.to_path_buf())
    }

    fn access_event(path: &Path) -> Event {
        Event::new(EventKind::Access(AccessKind::Read)).add_path(path.to_path_buf())
    }

    #[tokio::test(start_paused = true)]
    async fn a_relevant_event_resets_the_quiet_window() {
        let (tx, rx) = unbounded_channel();
        let fires = Arc::new(AtomicUsize::new(0));
        let counter = fires.clone();
        tokio::spawn(run_debounce_loop(rx, move |_burst: WatchBurst| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        tx.send(modify_event(Path::new("/tmp/a"))).unwrap();
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert_eq!(
            fires.load(Ordering::SeqCst),
            0,
            "still inside the quiet window"
        );

        // A second relevant event at t=1000ms pushes the quiet deadline to
        // t=2500ms; without the reset it would have fired at t=1500ms.
        tx.send(modify_event(Path::new("/tmp/a"))).unwrap();
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert_eq!(
            fires.load(Ordering::SeqCst),
            0,
            "the reset window is not over yet"
        );

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(fires.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unbroken_stream_of_events_still_fires_at_the_maximum_wait() {
        let (tx, rx) = unbounded_channel();
        let fires = Arc::new(AtomicUsize::new(0));
        let counter = fires.clone();
        tokio::spawn(run_debounce_loop(rx, move |_burst: WatchBurst| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        // Events every 400ms never let the 1500ms quiet window elapse, so
        // only the 5s maximum wait can end this burst.
        for _ in 0..14 {
            tx.send(modify_event(Path::new("/tmp/a"))).unwrap();
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        assert_eq!(
            fires.load(Ordering::SeqCst),
            1,
            "the maximum wait should force exactly one pass despite the steady stream"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn access_only_events_never_reach_the_debouncer() {
        let (tx, rx) = unbounded_channel();
        let fires = Arc::new(AtomicUsize::new(0));
        let counter = fires.clone();
        tokio::spawn(run_debounce_loop(rx, move |_burst: WatchBurst| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        tx.send(access_event(Path::new("/tmp/a"))).unwrap();
        tokio::time::sleep(MAX_WAIT + Duration::from_secs(1)).await;
        assert_eq!(fires.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn two_events_for_the_same_path_yield_one_path_and_two_events() {
        let (tx, rx) = unbounded_channel();
        let bursts: Arc<std::sync::Mutex<Vec<WatchBurst>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = bursts.clone();
        tokio::spawn(run_debounce_loop(rx, move |burst: WatchBurst| {
            collected.lock().unwrap().push(burst);
        }));

        tx.send(modify_event(Path::new("/tmp/a"))).unwrap();
        tx.send(modify_event(Path::new("/tmp/a"))).unwrap();
        tokio::time::sleep(QUIET_WINDOW + Duration::from_millis(100)).await;

        let bursts = bursts.lock().unwrap();
        assert_eq!(bursts.len(), 1);
        assert_eq!(bursts[0].paths, vec![PathBuf::from("/tmp/a")]);
        assert_eq!(bursts[0].events, 2);
    }

    #[test]
    fn events_confined_to_a_wsl_mount_are_not_relevant() {
        let event = modify_event(Path::new(
            r"\\wsl.localhost\Ubuntu\home\avery\.claude\projects\p\s.jsonl",
        ));
        assert!(!is_relevant(&event));

        let native = modify_event(Path::new("/home/avery/.claude/projects/p/s.jsonl"));
        assert!(is_relevant(&native));
    }

    #[test]
    fn an_event_confined_to_shm_sidecars_is_not_relevant() {
        let shm_only = modify_event(Path::new("/home/avery/.cursor/state.vscdb-shm"));
        assert!(!is_relevant(&shm_only));
    }

    #[test]
    fn an_event_with_one_shm_path_and_one_relevant_path_is_relevant() {
        let mixed = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(PathBuf::from("/home/avery/.cursor/state.vscdb-shm"))
            .add_path(PathBuf::from("/home/avery/.claude/projects/p/s.jsonl"));
        assert!(is_relevant(&mixed));
    }

    #[test]
    fn a_wal_event_is_relevant() {
        let wal = modify_event(Path::new("/home/avery/.cursor/state.vscdb-wal"));
        assert!(is_relevant(&wal));
    }

    #[test]
    fn watcher_status_is_healthy_only_when_active_with_no_failed_roots() {
        assert!(!WatcherStatus::default().is_healthy());
        assert!(
            !WatcherStatus {
                active: true,
                failed_roots: vec![PathBuf::from("/home/avery/.codex/sessions")],
            }
            .is_healthy()
        );
        assert!(
            WatcherStatus {
                active: true,
                failed_roots: Vec::new(),
            }
            .is_healthy()
        );
    }

    #[test]
    fn a_root_that_does_not_exist_yet_is_skipped_rather_than_failed() {
        let home = tempfile::TempDir::new().unwrap();
        let (tx, _rx) = unbounded_channel::<Event>();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                let _ = tx.send(result.unwrap_or_else(|error| {
                    Event::new(EventKind::Other).add_path(PathBuf::from(error.to_string()))
                }));
            },
            notify::Config::default(),
        )
        .unwrap();
        let roots = all_watch_roots(home.path());
        let mut watched = HashSet::new();

        let failed = watch_new_roots(&mut watcher, &roots, &mut watched);

        assert!(watched.is_empty(), "an empty home has no existing roots");
        assert!(
            failed.is_empty(),
            "a missing root is skipped, not reported as a failure"
        );
    }

    #[tokio::test]
    async fn the_watcher_requests_a_pass_after_a_claude_transcript_is_created() {
        let home = tempfile::TempDir::new().unwrap();
        let project = home.path().join(".claude").join("projects").join("demo");
        std::fs::create_dir_all(&project).unwrap();

        let fired = Arc::new(AtomicUsize::new(0));
        let counter = fired.clone();
        let status = spawn_watcher_over(home.path().to_path_buf(), move |_burst: WatchBurst| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert!(status.active);
        assert!(status.failed_roots.is_empty());

        std::fs::write(project.join("session.jsonl"), b"{\"type\":\"user\"}\n").unwrap();

        let mut waited = Duration::ZERO;
        let step = Duration::from_millis(100);
        while fired.load(Ordering::SeqCst) == 0 && waited < Duration::from_secs(10) {
            tokio::time::sleep(step).await;
            waited += step;
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "creating a transcript under a watched root should request one pass"
        );
    }
}
