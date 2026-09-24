//! Oh My Pi agent log discovery.
//!
//! OMP stores session transcripts at `~/.omp/agent/sessions/`. Each
//! subdirectory encodes the working directory in its name and contains JSONL
//! session files named `{ISO-timestamp}_{sessionId}.jsonl`.
//!
//! `PI_CONFIG_DIR` renames the `.omp` dotdir, and `PI_CODING_AGENT_DIR`
//! replaces the agent directory for the default profile. Named profiles and
//! the XDG redirects move the tree to roots that this explorer does not
//! discover.
//!
//! CWD is read from the `{"type":"session","cwd":"..."}` record after the
//! 256-byte title slot. The folder name is not used.

use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionSource, SurfacePaths, WatchRoot, env_path_when_real_home,
    home_dir, recent_files_with_exts,
};
use async_trait::async_trait;

pub struct OmpExplorer;

#[async_trait]
impl AgentExplorer for OmpExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let dirs = all_log_dirs().await;
        recent_files_with_exts(&dirs, now, since_secs, &["jsonl"])
            .await
            .into_iter()
            .map(|file| SessionLog {
                agent_type: AgentKind::Omp,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns the OMP agent sessions tree at `~/.omp/agent/sessions/**`.
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/.omp/agent/")
    }

    fn unmatched_surface(&self) -> &'static str {
        "cli"
    }

    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: vec![agent_dir_in(home).join("sessions")],
            ide_desktop: Vec::new(),
            mirror: Vec::new(),
        }
    }

    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        self.surface_paths(home)
            .cli
            .into_iter()
            .map(WatchRoot::recursive)
            .collect()
    }

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
    home.join(".omp")
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

/// The OMP config root under `home`.
///
/// `PI_CONFIG_DIR` renames the `.omp` dotdir. It names a directory under the
/// home directory, so an empty value keeps the default.
pub(crate) fn config_root_in(home: &Path) -> PathBuf {
    env_path_when_real_home(home, "PI_CONFIG_DIR")
        .filter(|name| !name.as_os_str().is_empty())
        .map(|name| home.join(name))
        .unwrap_or_else(|| home.join(".omp"))
}

/// The OMP agent directory for the default profile.
///
/// OMP reads `PI_CODING_AGENT_DIR` for the default profile only. Pi reads the
/// same variable, so antiburn gives it to OMP only when it points into the OMP
/// config root. A named profile (`OMP_PROFILE` / `PI_PROFILE`) moves the agent
/// directory to `<root>/profiles/<name>/agent`, and the XDG redirects can move
/// it again. antiburn does not discover those roots; see
/// `docs/session-coverage.md`.
pub(crate) fn agent_dir_in(home: &Path) -> PathBuf {
    let root = config_root_in(home);
    match env_path_when_real_home(home, "PI_CODING_AGENT_DIR") {
        Some(dir) if dir.starts_with(&root) => dir,
        _ => root.join("agent"),
    }
}

async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let sessions_dir = agent_dir_in(home).join("sessions");
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

#[cfg(test)]
#[path = "tests/omp_tests.rs"]
mod tests;
