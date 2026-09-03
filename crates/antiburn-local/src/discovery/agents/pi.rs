//! Pi agent log discovery.
//!
//! Pi stores session transcripts at `~/.pi/agent/sessions/`. Each
//! subdirectory encodes the working directory in its name (double-dash
//! bookends, hyphen between components, e.g. `--Users-foo-projects-bar--`)
//! and contains JSONL session files named `{ISO-timestamp}_{UUIDv7}.jsonl`.
//!
//! CWD is read directly from the `{"type":"session","cwd":"..."}` record on
//! the first line of each transcript rather than decoded from the folder
//! name. The in-content `cwd` is unambiguous; the folder name is not (a
//! repo name containing `-` is indistinguishable from a path separator).
//! No `path_codec` entry is needed for Pi.

use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionSource, SurfacePaths, WatchRoot, env_path_when_real_home,
    home_dir, recent_files_with_exts,
};
use async_trait::async_trait;

pub struct PiExplorer;

#[async_trait]
impl AgentExplorer for PiExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let dirs = all_log_dirs().await;
        recent_files_with_exts(&dirs, now, since_secs, &["jsonl"])
            .await
            .into_iter()
            .map(|file| SessionLog {
                agent_type: AgentKind::Pi,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns the Pi agent sessions tree at `~/.pi/agent/sessions/**`. Single
    /// substring covers macOS / Linux; Windows Pi support is unverified but
    /// would still match if the path were normalised to forward slashes
    /// under `%USERPROFILE%/.pi/agent/`.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.pi/agent/`  → `cli` (Pi is a CLI-only agent)
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/.pi/agent/")
    }

    fn unmatched_surface(&self) -> &'static str {
        "cli"
    }

    /// CLI-only: `~/.pi/agent/sessions/`. Pi has no IDE companion today.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: vec![
                agent_dir_in(
                    home,
                    env_path_when_real_home(home, "PI_AGENT_DIR").as_deref(),
                )
                .join("sessions"),
            ],
            ide_desktop: Vec::new(),
            mirror: Vec::new(),
        }
    }

    /// The same sessions root discovery walks, `PI_AGENT_DIR` included.
    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        self.surface_paths(home)
            .cli
            .into_iter()
            .map(WatchRoot::recursive)
            .collect()
    }

    // Title lookup: inherits the `Scan` default. Pi has no per-session index
    // (the file-per-session layout still requires a `read_dir` walk to locate
    // a session by id), so a Direct point query is no cheaper than the
    // batched scan path — and Scan additionally populates
    // `AppState::session_surfaces` as a side effect, which Direct skips.

    /// Pi filenames are `{ISO-timestamp}_{UUIDv7}.jsonl`. The header also has
    /// an ID, but the filename remains the stable discovery identity source.
    /// Recover the UUID suffix so the scanner can populate `session_id`.
    ///
    /// Requires a `_` separator: a stem with no underscore is not a valid Pi
    /// session filename and returns `None` rather than guessing.
    fn recover_session_id_from_path(&self, file: &Path) -> Option<String> {
        file.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| stem.rsplit_once('_'))
            .map(|(_, uuid)| uuid)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    home.join(".pi")
        .join("agent")
        .join("sessions")
        .join("--Users-test-projects-foo--")
        .join("2026-05-26T01-02-03-000Z_019e61cd-aaaa-bbbb-cccc-dddddddddddd.jsonl")
}

async fn all_log_dirs() -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    log_dirs_in(&home).await
}

fn agent_dir_in(home: &Path, override_dir: Option<&Path>) -> PathBuf {
    override_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".pi").join("agent"))
}

async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let sessions_dir = agent_dir_in(
        home,
        env_path_when_real_home(home, "PI_AGENT_DIR").as_deref(),
    )
    .join("sessions");
    let mut entries = match tokio::fs::read_dir(&sessions_dir).await {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut dirs = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_type = match entry.file_type().await {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs
}

/// Find a session file by UUID suffix across all log directories.
///
/// Test-only helper retained for the session-id / title-extraction tests that
/// model the on-disk layout end-to-end. Pi has no durable title index, so
/// production titles come from transcript metadata recorded at ingest time,
/// not from a lookup through this helper.
#[cfg(test)]
async fn find_session_file_by_id(dirs: &[PathBuf], session_id: &str) -> Option<PathBuf> {
    let suffix = format!("_{session_id}.jsonl");
    for dir in dirs {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let matches = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(&suffix))
                .unwrap_or(false);
            if matches {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/pi_tests.rs"]
mod tests;
