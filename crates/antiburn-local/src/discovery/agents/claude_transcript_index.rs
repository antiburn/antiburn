//! Per-scan index of Claude CLI transcripts by session id.
//!
//! One discovery scan resolves many Claude Desktop manifests and fork jobs to
//! their CLI transcripts at `<project dir>/<session id>.jsonl`. A probe of each
//! project directory for each id does one path check per directory per id. A
//! large profile has hundreds of directories and hundreds of ids, so the scan
//! does hundreds of thousands of checks.
//!
//! [`TranscriptIndex`] lists each project directory one time per scan and maps
//! session ids to transcript paths. A lookup then reads the map. The usual
//! source safety check (`source_fs::try_exists`) runs only on the paths that
//! the map gives, so symlink and containment rules stay the same as for a
//! probe.

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::OnceCell;

/// Whether `session_id` is safe to use as a transcript file stem.
///
/// The allowlist is ASCII letters, digits, and hyphens. Claude session ids are
/// UUIDs. The check stops path traversal from a crafted `sessionId` or
/// `cliSessionId` value.
pub(super) fn is_cli_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Map from Claude session id to CLI transcript path, built one time per scan.
///
/// The index lists the project directories on the first [`Self::resolve`]
/// call, in one blocking task. A scan that resolves no ids lists nothing.
/// Create one index for each scan. Do not keep an index across scans, because
/// it does not see transcripts that appear after it lists the directories.
pub(super) struct TranscriptIndex {
    project_dirs: Vec<PathBuf>,
    by_id: OnceCell<HashMap<String, Vec<PathBuf>>>,
}

impl TranscriptIndex {
    /// Make an index over `project_dirs`. The directory order sets which
    /// transcript wins when two directories hold the same id: the first one.
    pub(super) fn new(project_dirs: &[PathBuf]) -> Self {
        Self {
            project_dirs: project_dirs.to_vec(),
            by_id: OnceCell::new(),
        }
    }

    /// Find the CLI transcript for `session_id`.
    ///
    /// The match ignores ASCII letter case in the id and in the `.jsonl`
    /// extension, as a probe does on the default macOS and Windows file
    /// systems. A file whose name is exactly `<session_id>.jsonl` wins over a
    /// file that matches only when case is ignored, so a case-sensitive file
    /// system gives the same result as an exact-name probe. Returns `None`
    /// for an unsafe id, an audit log name, or an id with no transcript. Each
    /// returned path passed the source safety check.
    pub(super) async fn resolve(&self, session_id: &str) -> Option<PathBuf> {
        if !is_cli_session_id(session_id) {
            return None;
        }
        let by_id = self
            .by_id
            .get_or_init(|| {
                let project_dirs = self.project_dirs.clone();
                async move {
                    tokio::task::spawn_blocking(move || index_blocking(&project_dirs))
                        .await
                        .unwrap_or_default()
                }
            })
            .await;
        let mut candidates = by_id.get(&session_id.to_ascii_lowercase())?.clone();
        let exact_name = format!("{session_id}.jsonl");
        // A stable sort keeps the project directory order inside each group.
        candidates.sort_by_key(|candidate| {
            candidate.file_name().and_then(|name| name.to_str()) != Some(exact_name.as_str())
        });
        tokio::task::spawn_blocking(move || {
            candidates.into_iter().find(|candidate| {
                crate::discovery::source_fs::try_exists(candidate).unwrap_or(false)
            })
        })
        .await
        .ok()
        .flatten()
    }
}

/// List each project directory one time and group `<id>.jsonl` entries by
/// ASCII-lowercase id. Each path keeps the file name from the directory.
/// Run this only in a blocking context.
fn index_blocking(project_dirs: &[PathBuf]) -> HashMap<String, Vec<PathBuf>> {
    let mut by_id: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for project_dir in project_dirs {
        #[cfg(test)]
        record_tracked_listing(project_dir);
        let Ok(entries) = crate::discovery::source_fs::read_dir(project_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().map(str::to_ascii_lowercase) else {
                continue;
            };
            let Some(session_id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            let path = project_dir.join(&file_name);
            if !is_cli_session_id(session_id) || super::claude::is_audit_log(&path) {
                continue;
            }
            by_id.entry(session_id.to_owned()).or_default().push(path);
        }
    }
    by_id
}

/// How many times [`index_blocking`] listed a tracked project directory since
/// the matching [`track_listings`] call. Backs the test that a scan lists each
/// directory one time, not one time per manifest.
#[cfg(test)]
static TRACKED_LISTINGS: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, usize>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// Start to count index listings of `project_dir`.
#[cfg(test)]
pub(super) fn track_listings(project_dir: &std::path::Path) {
    TRACKED_LISTINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(project_dir.to_path_buf(), 0);
}

#[cfg(test)]
fn record_tracked_listing(project_dir: &std::path::Path) {
    let mut listings = TRACKED_LISTINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(count) = listings.get_mut(project_dir) {
        *count += 1;
    }
}

/// Give the listing count for `project_dir` and stop the count.
#[cfg(test)]
pub(super) fn take_tracked_listings(project_dir: &std::path::Path) -> usize {
    TRACKED_LISTINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(project_dir)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/claude_transcript_index_tests.rs"]
mod tests;
