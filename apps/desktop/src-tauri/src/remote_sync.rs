//! Serial, completion-based scheduling for remote session scans.

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

pub const STATUS_EVENT: &str = "remote-sync-status";

pub type ScanOrigin = crate::analytics::event::RemoteSyncOrigin;

#[derive(Clone, Debug)]
struct Job {
    host_id: String,
    origin: ScanOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub host_id: String,
    pub completed: usize,
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub interval_secs: u64,
    pub active: Option<Progress>,
    pub pending_host_ids: Vec<String>,
}

struct State {
    interval_secs: u64,
    active: Option<Progress>,
    pending: VecDeque<Job>,
    queued: HashSet<String>,
    generations: HashMap<String, u64>,
    finished: HashMap<String, Instant>,
    lifecycle_write: bool,
}

pub struct Scheduler {
    state: Mutex<State>,
    lifecycle: Mutex<()>,
    wake: Notify,
}

impl Scheduler {
    fn new(interval_secs: u64) -> Self {
        Self {
            state: Mutex::new(State {
                interval_secs,
                active: None,
                pending: VecDeque::new(),
                queued: HashSet::new(),
                generations: HashMap::new(),
                finished: HashMap::new(),
                lifecycle_write: false,
            }),
            lifecycle: Mutex::new(()),
            wake: Notify::new(),
        }
    }
}

pub fn validate_interval(seconds: u64) -> Result<(), String> {
    if [0, 60, 300, 900, 1800, 3600].contains(&seconds) {
        Ok(())
    } else {
        Err("Unsupported remote scan interval".into())
    }
}

fn snapshot(state: &State) -> Status {
    Status {
        interval_secs: state.interval_secs,
        active: state.active.clone(),
        pending_host_ids: state
            .pending
            .iter()
            .map(|job| job.host_id.clone())
            .collect(),
    }
}

fn emit(app: &AppHandle, status: &Status) {
    let _ = app.emit(STATUS_EVENT, status);
}

#[tauri::command]
pub fn get_remote_sync_status(app: AppHandle) -> Status {
    let scheduler = app.state::<Scheduler>();
    let state = scheduler
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    snapshot(&state)
}

#[tauri::command]
pub fn set_remote_sync_interval(app: AppHandle, seconds: u64) -> Result<Status, String> {
    validate_interval(seconds)?;
    crate::remote_sessions::write_interval(&app, seconds)?;
    let scheduler = app.state::<Scheduler>();
    let status = {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.interval_secs = seconds;
        snapshot(&state)
    };
    emit(&app, &status);
    scheduler.wake.notify_one();
    Ok(status)
}

fn enqueue(app: &AppHandle, host_id: &str, origin: ScanOrigin) {
    let scheduler = app.state::<Scheduler>();
    let status = {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.queued.insert(host_id.to_owned()) {
            state.pending.push_back(Job {
                host_id: host_id.to_owned(),
                origin,
            });
        } else if origin == ScanOrigin::Manual
            && let Some(job) = state.pending.iter_mut().find(|job| job.host_id == host_id)
        {
            job.origin = ScanOrigin::Manual;
        }
        snapshot(&state)
    };
    emit(app, &status);
    scheduler.wake.notify_one();
}

pub fn enqueue_manual(app: &AppHandle, host_id: &str) {
    enqueue(app, host_id, ScanOrigin::Manual);
}

pub fn enqueue_automatic(app: &AppHandle, host_id: &str) {
    if crate::remote_sessions::automatic_host_ids(app).is_ok_and(|ids| ids.contains(host_id)) {
        enqueue(app, host_id, ScanOrigin::Automatic);
    }
}

fn disable_host(state: &mut State, host_id: &str) {
    state.pending.retain(|job| job.host_id != host_id);
    state.queued.remove(host_id);
    *state.generations.entry(host_id.to_owned()).or_default() += 1;
}

pub fn host_sync_changed(app: &AppHandle, host_id: &str, enabled: bool) {
    let scheduler = app.state::<Scheduler>();
    let _lifecycle = scheduler
        .lifecycle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let status = {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !enabled {
            disable_host(&mut state, host_id);
        }
        snapshot(&state)
    };
    emit(app, &status);
    scheduler.wake.notify_one();
}

fn begin_lifecycle_write(state: &mut State, host_ids: &[String], drop_pending: bool) {
    state.lifecycle_write = true;
    let mut affected = host_ids.iter().cloned().collect::<HashSet<_>>();
    if let Some(active) = &state.active {
        affected.insert(active.host_id.clone());
    }
    for host_id in affected {
        *state.generations.entry(host_id.clone()).or_default() += 1;
        state.finished.remove(&host_id);
        if drop_pending {
            state.pending.retain(|job| job.host_id != host_id);
            state.queued.remove(&host_id);
        }
    }
}

pub fn progress(app: &AppHandle, host_id: &str, completed: usize, total: usize) {
    let scheduler = app.state::<Scheduler>();
    let status = {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.host_id == host_id)
        {
            state.active = Some(Progress {
                host_id: host_id.to_owned(),
                completed,
                total,
            });
        }
        snapshot(&state)
    };
    emit(app, &status);
}

pub fn generation(app: &AppHandle, host_id: &str) -> u64 {
    let scheduler = app.state::<Scheduler>();
    let state = scheduler
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.generations.get(host_id).copied().unwrap_or(0)
}

pub fn is_current(app: &AppHandle, host_id: &str, generation: u64) -> bool {
    self::generation(app, host_id) == generation
        && crate::remote_sessions::host_exists(app, host_id).unwrap_or(false)
}

fn commit_allowed(state: &State, host_id: &str, generation: u64) -> bool {
    !state.lifecycle_write && state.generations.get(host_id).copied().unwrap_or(0) == generation
}

/// Runs one final cache/database commit while holding the lifecycle fence.
/// Removal and deletion advance the same generation under this lock, so they
/// either happen before this closure or delete its completed write afterward.
pub fn with_commit_guard<T>(
    app: &AppHandle,
    host_id: &str,
    generation: u64,
    commit: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let scheduler = app.state::<Scheduler>();
    let _lifecycle = scheduler
        .lifecycle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let allowed = {
        let state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        commit_allowed(&state, host_id, generation)
    };
    anyhow::ensure!(allowed, "scan cancelled");
    commit()
}

/// Runs one authoritative lifecycle write while remote commits are fenced.
pub fn with_lifecycle_guard<T>(
    app: &AppHandle,
    host_ids: &[String],
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    with_lifecycle_guard_inner(app, host_ids, false, operation)
}

pub fn with_destructive_lifecycle_guard<T>(
    app: &AppHandle,
    host_ids: &[String],
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    with_lifecycle_guard_inner(app, host_ids, true, operation)
}

fn with_lifecycle_guard_inner<T>(
    app: &AppHandle,
    host_ids: &[String],
    drop_pending: bool,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let scheduler = app.state::<Scheduler>();
    with_scheduler_lifecycle_guard(&scheduler, host_ids, drop_pending, operation)
}

struct LifecycleWrite<'a>(&'a Scheduler);

impl Drop for LifecycleWrite<'_> {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lifecycle_write = false;
        self.0.wake.notify_one();
    }
}

fn with_scheduler_lifecycle_guard<T>(
    scheduler: &Scheduler,
    host_ids: &[String],
    drop_pending: bool,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _lifecycle = scheduler
        .lifecycle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        begin_lifecycle_write(&mut state, host_ids, drop_pending);
    }
    let _write = LifecycleWrite(scheduler);
    operation()
}

fn due(interval: u64, finished: Option<Instant>, now: Instant) -> bool {
    interval != 0
        && finished.is_none_or(|time| now.duration_since(time) >= Duration::from_secs(interval))
}

fn enqueue_due(app: &AppHandle) {
    let Ok(hosts) = crate::remote_sessions::automatic_host_ids(app) else {
        return;
    };
    let scheduler = app.state::<Scheduler>();
    let now = Instant::now();
    let status = {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for host_id in hosts {
            let active = state
                .active
                .as_ref()
                .is_some_and(|active| active.host_id == host_id);
            if !active
                && !state.queued.contains(&host_id)
                && due(
                    state.interval_secs,
                    state.finished.get(&host_id).copied(),
                    now,
                )
            {
                state.queued.insert(host_id.clone());
                state.pending.push_back(Job {
                    host_id,
                    origin: ScanOrigin::Automatic,
                });
            }
        }
        snapshot(&state)
    };
    emit(app, &status);
}

fn next_job(state: &mut State, enabled: &HashSet<String>) -> Option<Job> {
    while let Some(job) = state.pending.pop_front() {
        state.queued.remove(&job.host_id);
        if enabled.contains(&job.host_id) {
            return Some(job);
        }
    }
    None
}

fn take_next(app: &AppHandle) -> Option<(String, u64, ScanOrigin)> {
    let enabled = crate::remote_sessions::automatic_host_ids(app).unwrap_or_default();
    let scheduler = app.state::<Scheduler>();
    let (job, status) = {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.lifecycle_write {
            return None;
        }
        let job = next_job(&mut state, &enabled)?;
        let host_id = job.host_id;
        state.queued.remove(&host_id);
        let generation = state.generations.get(&host_id).copied().unwrap_or(0);
        state.active = Some(Progress {
            host_id: host_id.clone(),
            completed: 0,
            total: 0,
        });
        (Some((host_id, generation, job.origin)), snapshot(&state))
    };
    emit(app, &status);
    job
}

fn finish(app: &AppHandle, host_id: &str) {
    let scheduler = app.state::<Scheduler>();
    let status = {
        let mut state = scheduler
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.host_id == host_id)
        {
            state.active = None;
            state.finished.insert(host_id.to_owned(), Instant::now());
        }
        snapshot(&state)
    };
    emit(app, &status);
    crate::remote_sessions::emit_hosts(app);
}

pub fn spawn(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let interval = crate::remote_sessions::read_interval(app).unwrap_or(300);
    app.manage(Scheduler::new(interval));
    if let Ok(root) = crate::remote_sessions::directory(app)
        && let Err(error) = crate::remote_cache::cleanup_staging(&root)
    {
        tracing::warn!(event = "remote_staging_cleanup_failed", error = %error);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for host_id in crate::remote_sessions::automatic_host_ids(&app).unwrap_or_default() {
            enqueue_automatic(&app, &host_id);
        }
        loop {
            enqueue_due(&app);
            while let Some((host_id, generation, origin)) = take_next(&app) {
                let _ =
                    crate::remote_sessions::perform_scan(&app, &host_id, generation, origin).await;
                finish(&app, &host_id);
            }
            let scheduler = app.state::<Scheduler>();
            tokio::select! {
                () = scheduler.wake.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_hosts_skip_automatic_and_manual_jobs() {
        let scheduler = Scheduler::new(300);
        let mut state = scheduler.state.lock().unwrap();
        for (host, origin) in [
            ("paused", ScanOrigin::Automatic),
            ("manual", ScanOrigin::Manual),
            ("enabled", ScanOrigin::Automatic),
        ] {
            state.queued.insert(host.into());
            state.pending.push_back(Job {
                host_id: host.into(),
                origin,
            });
        }
        let enabled = HashSet::from(["enabled".into()]);
        assert_eq!(next_job(&mut state, &enabled).unwrap().host_id, "enabled");
        assert!(!state.queued.contains("paused"));
        assert!(!state.queued.contains("manual"));
        assert!(next_job(&mut state, &enabled).is_none());
    }

    #[test]
    fn disabling_cancels_pending_jobs_and_fences_active_commits() {
        let scheduler = Scheduler::new(300);
        let mut state = scheduler.state.lock().unwrap();
        state.active = Some(Progress {
            host_id: "host".into(),
            completed: 1,
            total: 2,
        });
        state.queued.insert("host".into());
        state.pending.push_back(Job {
            host_id: "host".into(),
            origin: ScanOrigin::Automatic,
        });
        disable_host(&mut state, "host");
        assert!(state.pending.is_empty());
        assert!(state.active.is_some());
        state.pending.push_back(Job {
            host_id: "host".into(),
            origin: ScanOrigin::Manual,
        });
        state.queued.insert("host".into());
        disable_host(&mut state, "host");
        assert!(state.pending.is_empty());
        assert!(!state.queued.contains("host"));
        assert!(!commit_allowed(&state, "host", 0));
        assert!(commit_allowed(&state, "other-host", 0));
    }

    #[test]
    fn schedule_is_completion_based_and_manual_zero_never_runs() {
        let now = Instant::now();
        assert!(due(300, None, now));
        assert!(!due(0, None, now));
        assert!(!due(300, Some(now), now + Duration::from_secs(299)));
        assert!(due(300, Some(now), now + Duration::from_secs(300)));
    }

    #[test]
    fn interval_is_closed() {
        for value in [0, 60, 300, 900, 1800, 3600] {
            assert!(validate_interval(value).is_ok());
        }
        assert!(validate_interval(301).is_err());
    }

    #[test]
    fn lifecycle_writes_and_generation_changes_fence_stale_commits() {
        let scheduler = Scheduler::new(300);
        let mut state = scheduler.state.lock().unwrap();
        assert!(commit_allowed(&state, "host", 0));
        state.lifecycle_write = true;
        assert!(!commit_allowed(&state, "host", 0));
        state.lifecycle_write = false;
        state.generations.insert("host".into(), 1);
        assert!(!commit_allowed(&state, "host", 0));
        assert!(commit_allowed(&state, "host", 1));
    }

    #[tokio::test]
    async fn lifecycle_guard_cleans_up_on_success_error_and_unwind() {
        for destructive in [false, true] {
            for outcome in [0, 1, 2] {
                let scheduler = Scheduler::new(300);
                {
                    let mut state = scheduler.state.lock().unwrap();
                    state.active = Some(Progress {
                        host_id: "host".into(),
                        completed: 0,
                        total: 1,
                    });
                    state.queued.insert("host".into());
                    state.pending.push_back(Job {
                        host_id: "host".into(),
                        origin: ScanOrigin::Manual,
                    });
                }
                let result = std::panic::catch_unwind(|| {
                    with_scheduler_lifecycle_guard(&scheduler, &[], destructive, || {
                        assert!(scheduler.state.lock().unwrap().lifecycle_write);
                        match outcome {
                            0 => Ok(()),
                            1 => anyhow::bail!("fixture error"),
                            _ => panic!("fixture unwind"),
                        }
                    })
                });
                match outcome {
                    0 => assert!(result.unwrap().is_ok()),
                    1 => assert!(result.unwrap().is_err()),
                    _ => assert!(result.is_err()),
                }
                {
                    let state = scheduler.state.lock().unwrap();
                    assert!(!state.lifecycle_write);
                    assert!(!commit_allowed(&state, "host", 0));
                    assert!(commit_allowed(&state, "host", 1));
                    assert_eq!(state.pending.is_empty(), destructive);
                }
                tokio::time::timeout(Duration::from_millis(100), scheduler.wake.notified())
                    .await
                    .unwrap();
                assert!(with_scheduler_lifecycle_guard(&scheduler, &[], false, || Ok(())).is_ok());
            }
        }
    }

    #[test]
    fn lifecycle_write_always_invalidates_an_active_host() {
        let scheduler = Scheduler::new(300);
        let mut state = scheduler.state.lock().unwrap();
        state.active = Some(Progress {
            host_id: "new-host".into(),
            completed: 0,
            total: 1,
        });
        state.queued.insert("new-host".into());
        state.pending.push_back(Job {
            host_id: "new-host".into(),
            origin: ScanOrigin::Automatic,
        });
        begin_lifecycle_write(&mut state, &[], true);
        assert_eq!(state.generations.get("new-host"), Some(&1));
        assert!(!state.queued.contains("new-host"));
        assert!(state.pending.is_empty());
    }
}
