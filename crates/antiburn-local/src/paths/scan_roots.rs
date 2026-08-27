//! Persists the directories the user points the scanner at.
//!
//! These are *extra scan roots* beyond the built-in common code directories —
//! folders a user declares so repositories cloned in non-standard locations
//! (and with no agent sessions yet) are still found. They are machine-global:
//! a folder like `~/work` may hold repositories from several sources, and
//! later filtering is the caller's job.
//!
//! The caller supplies the state directory; the file inside it is
//! `scan-roots.json` (debug builds: `scan-roots-debug.json`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

use super::ignored_paths;
use super::state_files::{now_rfc3339, write_json_atomic};

/// Serializes the load-modify-write cycle so concurrent edits can't race.
static SCAN_ROOTS_LOCK: Mutex<()> = Mutex::const_new(());

fn scan_roots_file() -> &'static str {
    if cfg!(debug_assertions) {
        "scan-roots-debug.json"
    } else {
        "scan-roots.json"
    }
}

/// Path of the scan-roots file inside `state_dir`.
pub fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join(scan_roots_file())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanRootEntry {
    path: String,
    #[serde(default)]
    added_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ScanRootsState {
    #[serde(default)]
    roots: Vec<ScanRootEntry>,
}

/// Normalize a path for comparison/dedup. Delegates to the shared
/// [`ignored_paths::normalize`] so scan roots trim a trailing separator of
/// either kind — including a Windows `\` — exactly as the ignored/tracked sets
/// do. Without this, `C:\work\` and `C:\work` would dedup differently and a
/// trailing backslash could slip past the home-directory too-broad guard.
fn normalize(path: &str) -> String {
    ignored_paths::normalize(path)
}

/// True if a path is too broad to scan as a root: the filesystem root, or the
/// home directory itself (which would make a scan walk the entire home tree).
fn is_too_broad(norm: &str) -> bool {
    if norm == "/" {
        return true;
    }
    super::home_dir()
        .map(|home| normalize(&home.to_string_lossy()) == norm)
        .unwrap_or(false)
}

fn load_state(state_dir: &Path) -> ScanRootsState {
    match std::fs::read_to_string(state_path(state_dir)) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => ScanRootsState::default(),
    }
}

async fn save_state(state_dir: &Path, state: &ScanRootsState) -> Result<()> {
    write_json_atomic(&state_path(state_dir), state).await
}

/// Scan roots, as `PathBuf`s, for a scan to walk.
pub fn load_scan_roots(state_dir: &Path) -> Vec<PathBuf> {
    load_state(state_dir)
        .roots
        .into_iter()
        .map(|e| PathBuf::from(e.path))
        .collect()
}

/// Record a scan root (idempotent). Rejects the home directory and `/` as too
/// broad.
///
/// Written when the user locates an otherwise-not-found repository (the
/// repository's parent directory is persisted), so the next scan re-finds it.
pub async fn add_scan_root(state_dir: &Path, path: &str) -> Result<()> {
    let norm = normalize(path);
    if norm.is_empty() || is_too_broad(&norm) {
        ::tracing::warn!(event = "scan_root_rejected_broad", path = %path);
        return Ok(());
    }
    let _guard = SCAN_ROOTS_LOCK.lock().await;
    let mut state = load_state(state_dir);
    if !state.roots.iter().any(|e| normalize(&e.path) == norm) {
        state.roots.push(ScanRootEntry {
            path: norm,
            added_at: now_rfc3339(),
        });
        save_state(state_dir, &state).await?;
    }
    Ok(())
}

/// Clear all scan roots.
pub async fn clear(state_dir: &Path) -> Result<()> {
    match tokio::fs::remove_file(state_path(state_dir)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("failed to remove scan-roots file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_trailing_separator() {
        assert_eq!(normalize("/a/b/"), "/a/b");
        assert_eq!(normalize("/a/b"), "/a/b");
        assert_eq!(normalize("/"), "/");
        // A Windows trailing backslash is trimmed too, matching ignored_paths so
        // dedup and the too-broad guard stay consistent across platforms.
        assert_eq!(normalize(r"C:\work\"), r"C:\work");
    }

    #[test]
    fn rejects_root_and_home_as_too_broad() {
        assert!(is_too_broad("/"));
        let home = super::super::home_dir().unwrap();
        assert!(is_too_broad(&normalize(&home.to_string_lossy())));
        assert!(!is_too_broad("/Users/avery/work"));
    }

    #[tokio::test]
    async fn roots_dedup_and_round_trip_through_disk() {
        let dir = tempfile::TempDir::new().unwrap();

        add_scan_root(dir.path(), "/Users/avery/work/")
            .await
            .unwrap();
        // Second add of the same (trailing-slash) path is a no-op.
        add_scan_root(dir.path(), "/Users/avery/work")
            .await
            .unwrap();

        assert_eq!(
            load_scan_roots(dir.path()),
            vec![PathBuf::from("/Users/avery/work")]
        );

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(state_path(dir.path())).unwrap())
                .unwrap();
        assert_eq!(raw["roots"][0]["path"], "/Users/avery/work");
        assert!(
            raw["roots"][0]["addedAt"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }

    #[tokio::test]
    async fn too_broad_roots_are_not_persisted_and_clear_removes_the_file() {
        let dir = tempfile::TempDir::new().unwrap();

        add_scan_root(dir.path(), "/").await.unwrap();
        assert!(load_scan_roots(dir.path()).is_empty());
        assert!(!state_path(dir.path()).exists());

        add_scan_root(dir.path(), "/Users/avery/work")
            .await
            .unwrap();
        assert!(state_path(dir.path()).exists());

        clear(dir.path()).await.unwrap();
        assert!(!state_path(dir.path()).exists());
        // Clearing twice is a no-op.
        clear(dir.path()).await.unwrap();
    }

    #[test]
    fn missing_state_directory_reads_as_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(load_scan_roots(&dir.path().join("does-not-exist")).is_empty());
    }
}
