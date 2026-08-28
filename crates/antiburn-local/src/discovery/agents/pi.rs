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
use std::time::UNIX_EPOCH;

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionSource, SurfacePaths, home_dir, recent_files_with_exts,
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

    async fn discover_cwds(&self, now: i64, since_secs: i64) -> Vec<String> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        discover_cwds_in(&home, now, since_secs).await
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
            cli: vec![home.join(".pi").join("agent").join("sessions")],
            ide_desktop: Vec::new(),
            mirror: Vec::new(),
        }
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

/// Return bounded recent native session payloads, newest first.
///
/// Session identity comes from the Pi filename contract, never content. Files
/// larger than `max_bytes` are skipped rather than truncated.
pub async fn recent_session_payloads(
    cutoff: i64,
    max_files: usize,
    max_bytes: u64,
) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    for dir in all_log_dirs().await {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs() as i64)
                .unwrap_or_default();
            if modified >= cutoff && metadata.len() <= max_bytes {
                candidates.push((modified, path));
            }
        }
    }
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let mut records = Vec::new();
    for (_, path) in candidates.into_iter().take(max_files) {
        let Some(session_key) = PiExplorer.recover_session_id_from_path(&path) else {
            continue;
        };
        if let Ok(content) = tokio::fs::read_to_string(path).await {
            records.push((session_key, content));
        }
    }
    records
}

async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let sessions_dir = home.join(".pi").join("agent").join("sessions");
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

/// Fast CWD discovery: read the first line of the newest session file in each
/// subdirectory to extract the `cwd` field from the `{type: "session"}` record.
async fn discover_cwds_in(home: &Path, now: i64, since_secs: i64) -> Vec<String> {
    let cutoff = now - since_secs;
    let dirs = log_dirs_in(home).await;
    let mut cwds = Vec::new();

    for dir in &dirs {
        if let Some(path) = newest_recent_jsonl(dir, cutoff).await
            && let Some(cwd) = cwd_from_first_line(&path).await
        {
            cwds.push(cwd);
        }
    }
    cwds
}

/// Return the newest `.jsonl` file in `dir` modified after `cutoff`, if any.
async fn newest_recent_jsonl(dir: &Path, cutoff: i64) -> Option<PathBuf> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return None,
    };

    let mut best: Option<(PathBuf, i64)> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let is_jsonl = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("jsonl"))
            .unwrap_or(false);
        if !is_jsonl {
            continue;
        }
        let mtime = match tokio::fs::metadata(&path).await {
            Ok(m) => match m.modified() {
                Ok(t) => match t.duration_since(UNIX_EPOCH) {
                    Ok(d) => d.as_secs() as i64,
                    Err(_) => continue,
                },
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        if mtime >= cutoff && best.as_ref().is_none_or(|(_, prev)| mtime > *prev) {
            best = Some((path, mtime));
        }
    }
    best.map(|(p, _)| p)
}

/// Read the first line of a JSONL file and extract the `cwd` field.
///
/// I/O and JSON-parse failures are traced and converted to `None` rather than
/// propagated — a truncated or malformed first line just means we don't
/// contribute this directory's CWD to repo discovery, which the upstream
/// `discover_cwds_in` is designed to tolerate.
async fn cwd_from_first_line(path: &Path) -> Option<String> {
    use tokio::io::AsyncBufReadExt;
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(err) => {
            ::tracing::trace!(path = %path.display(), error = %err, "pi: cwd_from_first_line open failed");
            return None;
        }
    };
    let mut reader = tokio::io::BufReader::new(file);
    let mut line = String::new();
    if let Err(err) = reader.read_line(&mut line).await {
        ::tracing::trace!(path = %path.display(), error = %err, "pi: cwd_from_first_line read failed");
        return None;
    }
    let value: serde_json::Value = match serde_json::from_str(&line) {
        Ok(v) => v,
        Err(err) => {
            ::tracing::trace!(path = %path.display(), error = %err, "pi: cwd_from_first_line json parse failed");
            return None;
        }
    };
    value.get("cwd").and_then(|v| v.as_str()).map(String::from)
}

/// Find a session file by UUID suffix across all log directories.
///
/// Test-only helper retained for the session-id / title-extraction tests that
/// model the on-disk layout end-to-end. Production lookups go through the
/// shared `default_session_titles_and_surfaces` scan in `agents::mod`.
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
