//! Poll active native transcript files for changes that the watcher does not report.
//!
//! Some writers keep transcript files open between records. File metadata
//! can change before the operating system sends a watcher notification.
//! Changed paths use the existing scan queue and its admission limits.
//! The poll waits longer when no native file session is active.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Manager};

use crate::store::Store;

use super::ScanController;
use super::watch::{MAX_BURST_PATHS, WatchBurst};

/// Poll cadence while the last poll found at least one active native file
/// session.
pub const LIVE_POLL_ACTIVE: Duration = Duration::from_secs(5);

/// Poll cadence while the last poll found none.
pub const LIVE_POLL_IDLE: Duration = Duration::from_secs(15);

/// One path's last-seen size and modification time.
type FileStamp = (u64, SystemTime);

/// Start the live poll. The returned handle is aborted when the app exits,
/// alongside the other schedulers.
pub fn spawn_live_poll(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut seen: HashMap<PathBuf, FileStamp> = HashMap::new();
        loop {
            let next = poll_once(&app, &mut seen);
            tokio::time::sleep(next).await;
        }
    })
}

/// Run one poll and return how long to sleep before the next one.
fn poll_once(app: &AppHandle, seen: &mut HashMap<PathBuf, FileStamp>) -> Duration {
    // Checked on every wake, not once at spawn, so resuming discovery takes
    // effect at the next poll instead of needing the app restarted — the
    // same reasoning as the scheduler's own check in `spawn_scheduler`.
    if !super::scheduled_scanning_allowed(app) {
        return LIVE_POLL_IDLE;
    }
    let now = crate::retention::unix_now();
    let labels = match app.state::<Store>().active_native_file_source_labels(now) {
        Ok(labels) => labels,
        Err(error) => {
            ::tracing::warn!(event = "scan_live_poll_query_failed", error = %error);
            return LIVE_POLL_IDLE;
        }
    };
    let active: Vec<PathBuf> = labels.into_iter().map(PathBuf::from).collect();
    let changed = changed_paths(seen, &active);
    if active.is_empty() {
        return LIVE_POLL_IDLE;
    }
    if !changed.is_empty() {
        let burst = build_burst(changed);
        ::tracing::debug!(event = "scan_live_poll_pushed", paths = burst.paths.len());
        app.state::<ScanController>().push_burst(burst);
    }
    LIVE_POLL_ACTIVE
}

/// Compare `active`'s current stat against what `seen` last recorded, and
/// return the paths whose size or modification time changed.
///
/// A path seen for the first time is recorded but not returned: the watcher
/// or the last full pass already ingested it, so reporting it again here
/// would only start a redundant refresh. A path missing from `active` is
/// dropped from `seen`, so a later return of that path is a first sighting
/// again. A stat error drops the path from `seen` silently; the next full
/// tick reconciles the removal.
fn changed_paths(seen: &mut HashMap<PathBuf, FileStamp>, active: &[PathBuf]) -> Vec<PathBuf> {
    let active_set: std::collections::HashSet<&PathBuf> = active.iter().collect();
    seen.retain(|path, _| active_set.contains(path));

    let mut changed = Vec::new();
    for path in active {
        let Some(stamp) = stat(path) else {
            seen.remove(path);
            continue;
        };
        match seen.insert(path.clone(), stamp) {
            None => {}
            Some(previous) if previous == stamp => {}
            Some(_) => changed.push(path.clone()),
        }
    }
    changed
}

fn stat(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    Some((metadata.len(), modified))
}

/// Build a burst from one poll's changed paths, bounded the same way the
/// watcher bounds its own bursts: past [`MAX_BURST_PATHS`], keep the first
/// paths and mark the burst overflowed instead of growing it further.
fn build_burst(mut changed: Vec<PathBuf>) -> WatchBurst {
    let overflowed = changed.len() > MAX_BURST_PATHS;
    if overflowed {
        changed.truncate(MAX_BURST_PATHS);
    }
    WatchBurst {
        events: changed.len(),
        paths: changed,
        overflowed,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn a_first_sighting_is_recorded_without_being_pushed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"first record\n").unwrap();
        let mut seen = HashMap::new();

        let changed = changed_paths(&mut seen, std::slice::from_ref(&path));

        assert!(changed.is_empty());
        assert!(seen.contains_key(&path));
    }

    #[test]
    fn an_append_is_pushed_once_and_not_again_until_it_changes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"first record\n").unwrap();
        let mut seen = HashMap::new();
        changed_paths(&mut seen, std::slice::from_ref(&path));

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"second record\n").unwrap();
        drop(file);
        let first_poll = changed_paths(&mut seen, std::slice::from_ref(&path));
        let second_poll = changed_paths(&mut seen, std::slice::from_ref(&path));

        assert_eq!(first_poll, vec![path]);
        assert!(second_poll.is_empty());
    }

    #[test]
    fn a_path_leaving_the_active_set_is_forgotten() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"first record\n").unwrap();
        let mut seen = HashMap::new();
        changed_paths(&mut seen, std::slice::from_ref(&path));

        // The session goes inactive: an empty active set forgets its stamp.
        changed_paths(&mut seen, &[]);
        assert!(seen.is_empty());

        // Its return is a first sighting again, not a change.
        let changed = changed_paths(&mut seen, std::slice::from_ref(&path));
        assert!(changed.is_empty());
        assert!(seen.contains_key(&path));
    }

    #[test]
    fn a_missing_file_is_dropped_and_not_pushed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("gone.jsonl");
        fs::write(&path, b"first record\n").unwrap();
        let mut seen = HashMap::new();
        changed_paths(&mut seen, std::slice::from_ref(&path));

        fs::remove_file(&path).unwrap();
        let changed = changed_paths(&mut seen, std::slice::from_ref(&path));

        assert!(changed.is_empty());
        assert!(!seen.contains_key(&path));
    }

    #[test]
    fn build_burst_bounds_and_overflows_past_the_watcher_limit() {
        let changed: Vec<PathBuf> = (0..MAX_BURST_PATHS + 5)
            .map(|index| PathBuf::from(format!("/session-{index}.jsonl")))
            .collect();

        let burst = build_burst(changed);

        assert_eq!(burst.paths.len(), MAX_BURST_PATHS);
        assert_eq!(burst.events, MAX_BURST_PATHS);
        assert!(burst.overflowed);
    }
}
