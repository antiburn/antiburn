//! Amp Code log discovery.
//!
//! Primary source:
//! - `~/.local/share/amp/threads/*.json`
//!
//! Fallback source (if no threads are found):
//! - `~/.amp/file-changes/**/*.{json,jsonl}`

use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, DesktopPlatform, SessionLog, SessionSource, SurfacePaths,
    collect_dirs_with_exts, current_desktop_platform, env_path_when_real_home, home_dir,
    recent_files_with_exts,
};
use async_trait::async_trait;

pub async fn all_log_dirs() -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    log_dirs_in(&home).await
}

pub struct AmpCodeExplorer;

#[async_trait]
impl AgentExplorer for AmpCodeExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let dirs = all_log_dirs().await;
        recent_files_with_exts(&dirs, now, since_secs, &["json", "jsonl"])
            .await
            .into_iter()
            .map(|file| SessionLog {
                agent_type: AgentKind::AmpCode,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns the Amp Code state dir per platform: `~/.local/share/amp/threads/`
    /// (unix), `%APPDATA%/amp/threads/` (Windows), and the
    /// `~/.amp/file-changes/` fallback tree.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.local/share/amp/threads/`      → `cli` (Linux state dir)
    /// - `/.amp/file-changes/`             → `cli` (fallback file-diffs tree)
    /// - `/appdata/roaming/amp/threads/`   → `cli` (Windows state dir)
    ///
    /// No IDE substring — the Amp VS Code extension keeps no local
    /// transcripts (per the 2026-05-25 audit). `unmatched_surface` returns
    /// `"cli"` for the same reason.
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/.local/share/amp/threads/")
            || path_lower.contains("/.amp/file-changes/")
            || path_lower.contains("/appdata/roaming/amp/threads/")
    }

    // Transcripts/file-changes walkers are CLI-side; the Amp VS Code extension
    // keeps no local transcripts. Per the 2026-05-25 audit.
    fn unmatched_surface(&self) -> &'static str {
        "cli"
    }

    /// CLI-only: platform state dir's `threads/` plus the
    /// `~/.amp/file-changes/` fallback. The Amp VS Code extension keeps no
    /// local transcripts (per the 2026-05-25 audit).
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        let appdata = env_path_when_real_home(home, "APPDATA");
        let state_root =
            amp_state_dir_for_platform(home, current_desktop_platform(), appdata.as_deref());
        SurfacePaths {
            cli: vec![
                state_root.join("threads"),
                home.join(".amp").join("file-changes"),
            ],
            ide_desktop: Vec::new(),
            mirror: Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    amp_state_dir_for_platform(home, DesktopPlatform::Linux, None)
        .join("threads")
        .join("T-abc.json")
}

async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let appdata = env_path_when_real_home(home, "APPDATA");
    log_dirs_for_platform(home, current_desktop_platform(), appdata.as_deref()).await
}

async fn log_dirs_for_platform(
    home: &Path,
    platform: DesktopPlatform,
    appdata: Option<&Path>,
) -> Vec<PathBuf> {
    let primary_threads = amp_state_dir_for_platform(home, platform, appdata).join("threads");
    let mut primary_dirs = Vec::new();
    collect_dirs_with_exts(&primary_threads, &mut primary_dirs, &["json"]).await;

    if !primary_dirs.is_empty() {
        primary_dirs.sort();
        primary_dirs.dedup();
        return primary_dirs;
    }

    let mut fallback_dirs = Vec::new();
    collect_dirs_with_exts(
        &home.join(".amp").join("file-changes"),
        &mut fallback_dirs,
        &["json", "jsonl"],
    )
    .await;
    fallback_dirs.sort();
    fallback_dirs.dedup();
    fallback_dirs
}

fn amp_state_dir_for_platform(
    home: &Path,
    platform: DesktopPlatform,
    appdata: Option<&Path>,
) -> PathBuf {
    match platform {
        DesktopPlatform::Windows => appdata
            .map(|path| path.join("amp"))
            .unwrap_or_else(|| home.join("AppData").join("Roaming").join("amp")),
        DesktopPlatform::Macos | DesktopPlatform::Linux => {
            home.join(".local").join("share").join("amp")
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[tokio::test]
    async fn test_amp_log_dirs_prefers_threads_dir() {
        let home = TempDir::new().unwrap();
        let threads_dir =
            amp_state_dir_for_platform(home.path(), DesktopPlatform::Linux, None).join("threads");
        tokio::fs::create_dir_all(&threads_dir).await.unwrap();
        tokio::fs::write(threads_dir.join("thread-1.json"), "{}")
            .await
            .unwrap();

        let fallback_dir = home.path().join(".amp").join("file-changes").join("x");
        tokio::fs::create_dir_all(&fallback_dir).await.unwrap();
        tokio::fs::write(fallback_dir.join("x.json"), "{}")
            .await
            .unwrap();

        let dirs = log_dirs_for_platform(home.path(), DesktopPlatform::Linux, None).await;
        assert_eq!(dirs, vec![threads_dir]);
    }

    #[tokio::test]
    async fn test_amp_log_dirs_falls_back_when_threads_missing() {
        let home = TempDir::new().unwrap();
        let fallback_dir = home.path().join(".amp").join("file-changes").join("x");
        tokio::fs::create_dir_all(&fallback_dir).await.unwrap();
        tokio::fs::write(fallback_dir.join("x.jsonl"), "{}")
            .await
            .unwrap();

        let dirs = log_dirs_for_platform(home.path(), DesktopPlatform::Linux, None).await;
        assert_eq!(dirs, vec![fallback_dir]);
    }

    #[tokio::test]
    async fn log_dirs_in_does_not_leak_real_env_appdata() {
        // Regression guard for the fix that replaced a raw `std::env::var_os`
        // read in `log_dirs_in` with `env_path_when_real_home`. With a
        // synthetic (TempDir) home, no env var should influence the result —
        // every returned dir must live under the synthetic home, even if
        // APPDATA points somewhere else. Anchors a class of bug, not just
        // this one site: same guard fits any future agent that reads env
        // vars during discovery.
        let home = TempDir::new().unwrap();
        let threads_dir = home
            .path()
            .join(".local")
            .join("share")
            .join("amp")
            .join("threads")
            .join("t");
        tokio::fs::create_dir_all(&threads_dir).await.unwrap();
        tokio::fs::write(threads_dir.join("t.json"), "{}")
            .await
            .unwrap();

        // SAFETY: env mutation in tests is racy with parallel tests touching
        // the same var. APPDATA is windows-only, unlikely to be probed by
        // other tests in this suite; keep the assertion path-anchored so even
        // if a peer test reads APPDATA the resulting paths still pass.
        unsafe { std::env::set_var("APPDATA", "/var/should-never-be-used/amp") };
        let dirs = log_dirs_in(home.path()).await;
        unsafe { std::env::remove_var("APPDATA") };

        for dir in &dirs {
            assert!(
                dir.starts_with(home.path()),
                "log_dirs_in leaked real-env path outside synthetic home: {}",
                dir.display(),
            );
        }
    }

    #[test]
    fn amp_state_dir_covers_windows_and_linux_paths() {
        let windows_home = PathBuf::from("C:\\Users\\tester");
        assert_eq!(
            amp_state_dir_for_platform(
                &windows_home,
                DesktopPlatform::Windows,
                Some(Path::new("D:\\Roaming")),
            ),
            PathBuf::from("D:\\Roaming").join("amp")
        );
        assert_eq!(
            amp_state_dir_for_platform(&windows_home, DesktopPlatform::Windows, None),
            windows_home.join("AppData").join("Roaming").join("amp")
        );

        let linux_home = PathBuf::from("/home/tester");
        assert_eq!(
            amp_state_dir_for_platform(&linux_home, DesktopPlatform::Linux, None),
            linux_home.join(".local").join("share").join("amp")
        );
    }
}
