// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Persists which local folders the user has opted out of ("ignored").
//!
//! When a folder is ignored, discovery hides it (and stops prompting for
//! permission) and every consumer skips sessions whose working directory is
//! that folder or a descendant of it. Ignoring is path-level: ignoring one
//! clone of a repository does not ignore other clones at different paths.
//!
//! Entries are grouped under a caller-supplied *scope* string so one machine
//! can keep independent opt-out sets. The caller also supplies the state
//! directory; the file inside it is `ignored-paths.json`
//! (debug builds: `ignored-paths-debug.json`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

use super::state_files::{now_rfc3339, write_json_atomic};

/// Serializes the load-modify-write cycle in [`ignore_path`] / [`unignore_path`]
/// so concurrent edits (e.g. "Restore all" firing one call per folder) can't
/// race and lose updates by overwriting each other's snapshot.
static IGNORED_LOCK: Mutex<()> = Mutex::const_new(());

fn ignored_file() -> &'static str {
    if cfg!(debug_assertions) {
        "ignored-paths-debug.json"
    } else {
        "ignored-paths.json"
    }
}

/// Path of the ignore-list file inside `state_dir`.
pub fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join(ignored_file())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct IgnoredEntry {
    path: String,
    #[serde(default)]
    ignored_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct IgnoredState {
    /// Scope key -> ignored folder entries for that scope. The stored key name
    /// is fixed by the on-disk format and kept for backward compatibility with
    /// existing state files.
    #[serde(default, rename = "orgs")]
    scopes: BTreeMap<String, Vec<IgnoredEntry>>,
}

/// Normalize a path string for comparison: trim a trailing separator so
/// `/a/b` and `/a/b/` compare equal. Does not touch the filesystem (the path
/// may be permission-denied), so it never canonicalizes.
///
/// Callers that build a comparable set of tracked roots must normalize through
/// this function too, so both sides of the comparison agree.
pub fn normalize(path: &str) -> String {
    // Trim a trailing separator of either kind so `/a/b` and `/a/b/` (and, on
    // Windows, `C:\a\b\`) compare equal.
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// True if `target` equals `ignored` or is a descendant directory of it.
///
/// Compares with `/` as the separator regardless of the platform separator in
/// the supplied paths, so descendant matching works on Windows (where paths
/// arrive with `\`) as well as on Unix.
fn is_under(target: &str, ignored: &str) -> bool {
    let target = target.replace('\\', "/");
    let ignored = ignored.replace('\\', "/");
    target == ignored || target.starts_with(&format!("{ignored}/"))
}

fn load_state(state_dir: &Path) -> IgnoredState {
    match std::fs::read_to_string(state_path(state_dir)) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => IgnoredState::default(),
    }
}

async fn save_state(state_dir: &Path, state: &IgnoredState) -> Result<()> {
    write_json_atomic(&state_path(state_dir), state).await
}

/// Load the normalized set of ignored folder paths for a scope. Cache this once
/// per scan and query it with [`set_contains`] to avoid re-reading the file per
/// session.
pub fn load_ignored(state_dir: &Path, scope: &str) -> HashSet<String> {
    load_state(state_dir)
        .scopes
        .get(scope)
        .map(|entries| entries.iter().map(|e| normalize(&e.path)).collect())
        .unwrap_or_default()
}

/// Ignored folder paths for a scope, in stored (display) form.
pub fn list_ignored(state_dir: &Path, scope: &str) -> Vec<String> {
    load_state(state_dir)
        .scopes
        .get(scope)
        .map(|entries| entries.iter().map(|e| e.path.clone()).collect())
        .unwrap_or_default()
}

/// True if `path` equals or is a descendant of any folder in `set` (the
/// already-normalized output of [`load_ignored`]).
pub fn set_contains(set: &HashSet<String>, path: &str) -> bool {
    if set.is_empty() {
        return false;
    }
    let target = normalize(path);
    set.iter().any(|ignored| is_under(&target, ignored))
}

/// Path-segment count of a path, treating both `/` and `\` as separators so the
/// depth of a Windows entry counts correctly. The filesystem root `/` is depth
/// `0`; `/a/b` is depth `2`.
fn path_depth(path: &str) -> usize {
    path.replace('\\', "/")
        .split('/')
        .filter(|seg| !seg.is_empty())
        .count()
}

/// Depth of the deepest entry in `set` that is `path` itself or an ancestor of
/// it — the same self-or-ancestor relation [`set_contains`] tests, but returning
/// *how specific* the match is instead of a bool. `None` when nothing matches.
///
/// Callers that also keep an explicitly-tracked set compare the deepest ignored
/// match against the deepest tracked match so the more-specific one wins,
/// instead of letting a broad ignored ancestor cascade over a repository that is
/// explicitly tracked beneath it.
pub fn deepest_prefix_depth(set: &HashSet<String>, path: &str) -> Option<usize> {
    if set.is_empty() {
        return None;
    }
    let target = normalize(path);
    set.iter()
        .filter(|entry| is_under(&target, entry))
        .map(|entry| path_depth(entry))
        .max()
}

/// True if a session should be skipped because either its working directory
/// (`cwd`) or its resolved repository root is at/under an ignored folder in
/// `set`.
///
/// This is the single opt-out gate, so every consumer enforces the ignore set
/// identically. An empty set short-circuits to `false` via [`set_contains`].
pub fn is_session_ignored(set: &HashSet<String>, cwd: Option<&str>, repo_root: &str) -> bool {
    cwd.is_some_and(|c| set_contains(set, c)) || set_contains(set, repo_root)
}

/// True if `path` is ignored for `scope`. Reads the file each call; prefer
/// [`load_ignored`] + [`set_contains`] in hot loops.
pub fn is_ignored(state_dir: &Path, scope: &str, path: &str) -> bool {
    set_contains(&load_ignored(state_dir, scope), path)
}

/// Mark a folder as ignored for a scope (idempotent).
pub async fn ignore_path(state_dir: &Path, scope: &str, path: &str) -> Result<()> {
    let norm = normalize(path);
    // Defensive: never store an empty or filesystem-root sentinel, which could
    // over-match and silently suppress unrelated sessions.
    if norm.is_empty() || norm == "/" {
        ::tracing::warn!(event = "ignore_path_rejected_root", path = %path);
        return Ok(());
    }
    let _guard = IGNORED_LOCK.lock().await;
    let mut state = load_state(state_dir);
    let entries = state.scopes.entry(scope.to_string()).or_default();
    if !entries.iter().any(|e| normalize(&e.path) == norm) {
        entries.push(IgnoredEntry {
            path: norm,
            ignored_at: now_rfc3339(),
        });
        save_state(state_dir, &state).await?;
    }
    Ok(())
}

/// Remove a folder from a scope's ignore list (restore). Idempotent.
pub async fn unignore_path(state_dir: &Path, scope: &str, path: &str) -> Result<()> {
    let norm = normalize(path);
    let _guard = IGNORED_LOCK.lock().await;
    let mut state = load_state(state_dir);
    let Some(entries) = state.scopes.get_mut(scope) else {
        return Ok(());
    };
    let before = entries.len();
    entries.retain(|e| normalize(&e.path) != norm);
    let changed = entries.len() != before;
    if entries.is_empty() {
        state.scopes.remove(scope);
    }
    if changed {
        save_state(state_dir, &state).await?;
    }
    Ok(())
}

/// Clear every ignored path in every scope.
pub async fn clear(state_dir: &Path) -> Result<()> {
    match tokio::fs::remove_file(state_path(state_dir)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("failed to remove ignored-paths file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_trailing_slash() {
        assert_eq!(normalize("/a/b/"), "/a/b");
        assert_eq!(normalize("/a/b"), "/a/b");
        assert_eq!(normalize("/"), "/");
    }

    #[test]
    fn is_under_matches_self_and_descendants_only() {
        assert!(is_under("/a/b", "/a/b")); // self
        assert!(is_under("/a/b/c", "/a/b")); // descendant
        assert!(is_under("/a/b/c/d", "/a/b")); // deeper descendant
        assert!(!is_under("/a/bc", "/a/b")); // sibling prefix, not under
        assert!(!is_under("/a", "/a/b")); // ancestor, not under
        assert!(!is_under("/x/y", "/a/b")); // unrelated
    }

    #[test]
    fn is_under_handles_windows_separators() {
        // Backslash paths (Windows) still match self + descendants.
        assert!(is_under(r"C:\Users\avery\repo", r"C:\Users\avery\repo"));
        assert!(is_under(
            r"C:\Users\avery\repo\sub\dir",
            r"C:\Users\avery\repo"
        ));
        assert!(!is_under(
            r"C:\Users\avery\repo-two",
            r"C:\Users\avery\repo"
        ));
        // A trailing backslash on the stored entry must not break matching.
        assert!(is_under(
            r"C:\Users\avery\repo\sub",
            &normalize(r"C:\Users\avery\repo\")
        ));
    }

    #[test]
    fn set_contains_handles_descendants_and_trailing_slashes() {
        let set: HashSet<String> = [normalize("/Users/avery/repo/")].into_iter().collect();
        assert!(set_contains(&set, "/Users/avery/repo"));
        assert!(set_contains(&set, "/Users/avery/repo/sub/dir"));
        assert!(!set_contains(&set, "/Users/avery/repo-two"));
        assert!(!set_contains(&HashSet::new(), "/Users/avery/repo"));
    }

    #[test]
    fn path_depth_counts_segments_across_separators() {
        assert_eq!(path_depth("/"), 0);
        assert_eq!(path_depth("/a/b"), 2);
        assert_eq!(path_depth("/a/b/"), 2);
        assert_eq!(path_depth(r"C:\Users\avery\repo"), 4);
    }

    #[test]
    fn deepest_prefix_depth_returns_deepest_matching_entry() {
        let set: HashSet<String> = [
            normalize("/Users/avery"),
            normalize("/Users/avery/proj/repo"),
        ]
        .into_iter()
        .collect();
        // Both entries are ancestors of the cwd; the deeper one wins.
        assert_eq!(
            deepest_prefix_depth(&set, "/Users/avery/proj/repo/src"),
            Some(4)
        );
        // Only the broad ancestor matches here.
        assert_eq!(deepest_prefix_depth(&set, "/Users/avery/other"), Some(2));
        // Nothing matches a sibling outside both entries.
        assert_eq!(deepest_prefix_depth(&set, "/Users/other"), None);
        // Empty set never matches.
        assert_eq!(deepest_prefix_depth(&HashSet::new(), "/Users/avery"), None);
    }

    #[test]
    fn is_session_ignored_checks_cwd_and_repo_root() {
        let set: HashSet<String> = [normalize("/Users/avery/private")].into_iter().collect();
        assert!(is_session_ignored(
            &set,
            Some("/Users/avery/private/app"),
            "/Users/avery/work/app"
        ));
        assert!(is_session_ignored(&set, None, "/Users/avery/private"));
        assert!(!is_session_ignored(
            &set,
            Some("/Users/avery/work"),
            "/Users/avery/work"
        ));
    }

    #[tokio::test]
    async fn scopes_are_isolated_and_round_trip_through_disk() {
        let dir = tempfile::TempDir::new().unwrap();

        ignore_path(dir.path(), "alpha", "/Users/avery/secret/")
            .await
            .unwrap();
        // Second add of the same (trailing-slash) path is a no-op.
        ignore_path(dir.path(), "alpha", "/Users/avery/secret")
            .await
            .unwrap();

        assert_eq!(
            list_ignored(dir.path(), "alpha"),
            vec!["/Users/avery/secret".to_string()]
        );
        assert!(list_ignored(dir.path(), "beta").is_empty());
        assert!(is_ignored(dir.path(), "alpha", "/Users/avery/secret/sub"));
        assert!(!is_ignored(dir.path(), "beta", "/Users/avery/secret/sub"));

        // The on-disk shape keeps the historical container key.
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(state_path(dir.path())).unwrap())
                .unwrap();
        assert_eq!(raw["orgs"]["alpha"][0]["path"], "/Users/avery/secret");
        assert!(
            raw["orgs"]["alpha"][0]["ignoredAt"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );

        unignore_path(dir.path(), "alpha", "/Users/avery/secret")
            .await
            .unwrap();
        assert!(list_ignored(dir.path(), "alpha").is_empty());
    }

    #[tokio::test]
    async fn root_is_rejected_and_clear_removes_the_file() {
        let dir = tempfile::TempDir::new().unwrap();

        ignore_path(dir.path(), "alpha", "/").await.unwrap();
        assert!(list_ignored(dir.path(), "alpha").is_empty());

        ignore_path(dir.path(), "alpha", "/Users/avery/secret")
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
        let missing = dir.path().join("does-not-exist");
        assert!(load_ignored(&missing, "alpha").is_empty());
        assert!(list_ignored(&missing, "alpha").is_empty());
    }
}
