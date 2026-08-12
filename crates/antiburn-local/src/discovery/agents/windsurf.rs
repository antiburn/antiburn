// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windsurf log discovery (VS Code style workspace storage).
//!
//! Windsurf stores chat sessions under:
//! - macOS: `~/Library/Application Support/Windsurf/User/workspaceStorage/*/chatSessions/*.json`
//! - Linux: `~/.config/Windsurf/User/workspaceStorage/*/chatSessions/*.json`
//! - Windows: `%APPDATA%\Windsurf\User\workspaceStorage\*\chatSessions\*.json`
//!
//! Some Windsurf conversations never land on disk. An embedding application
//! that obtains them another way can write them into a [`SessionMirror`]
//! directory and this adapter will walk it alongside workspace storage;
//! discovery itself never populates a mirror.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionMirror, SessionSource, SurfacePaths, app_config_dir_in,
    dir_has_json_files, find_chat_session_dirs, home_dir, recent_files_with_exts,
};
use async_trait::async_trait;

/// Windsurf discovery over the vendor's own layout, plus whatever the embedding
/// application has configured.
pub struct WindsurfExplorer {
    /// An application-maintained directory of conversation JSON.
    pub mirror: SessionMirror,
}

/// The unconfigured adapter: workspace storage only, no mirror.
pub static DISK_WINDSURF: WindsurfExplorer = WindsurfExplorer {
    mirror: SessionMirror::NONE,
};

impl WindsurfExplorer {
    /// The mirror directory, only when it actually holds conversation JSON.
    async fn populated_mirror_dir(&self, home: &Path) -> Option<PathBuf> {
        let dir = self.mirror.dir_in(home)?;
        dir_has_json_files(&dir).await.then_some(dir)
    }

    /// Workspace storage plus the configured mirror.
    ///
    /// Used by `discover_recent` to enumerate directories containing session
    /// files.
    async fn log_dirs_in(&self, home: &Path) -> Vec<PathBuf> {
        let ws_root = app_config_dir_in("Windsurf", home)
            .join("User")
            .join("workspaceStorage");
        let user_root = app_config_dir_in("Windsurf", home).join("User");

        let mut dirs = BTreeSet::new();
        for dir in find_chat_session_dirs(&ws_root).await {
            dirs.insert(dir);
        }
        for dir in find_chat_session_dirs(&user_root).await {
            dirs.insert(dir);
        }

        if let Some(mirror) = self.populated_mirror_dir(home).await {
            dirs.insert(mirror);
        }

        dirs.into_iter().collect()
    }
}

#[async_trait]
impl AgentExplorer for WindsurfExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        let dirs = self.log_dirs_in(&home).await;
        recent_files_with_exts(&dirs, now, since_secs, &["json"])
            .await
            .into_iter()
            .map(|file| SessionLog {
                agent_type: AgentKind::Windsurf,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns Windsurf IDE state across all platforms (`<app-config>/Windsurf/
    /// User/workspaceStorage/`), the `.codeium/windsurf/` cascade tree, and the
    /// configured mirror.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.codeium/windsurf/`                       → `ide_desktop` (cascade tree)
    /// - the mirror's `path_marker`                  → `mirror` (classifier maps
    ///                                                           to `ide_desktop`)
    /// - `/library/application support/windsurf/`    → `ide_desktop` (macOS app-config)
    /// - `/.config/windsurf/`                        → `ide_desktop` (Linux app-config)
    /// - `/appdata/roaming/windsurf/`                → `ide_desktop` (Windows app-config)
    /// - `/windsurf/user/workspacestorage/`          → `ide_desktop` (IDE chat)
    ///
    /// No CLI substring — Devin CLI bundled in Windsurf 2.0 has an
    /// undocumented path (P1 spike in the 2026-05-25 audit).
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/.codeium/windsurf/")
            || path_lower.contains("/library/application support/windsurf/")
            || path_lower.contains("/.config/windsurf/")
            || path_lower.contains("/appdata/roaming/windsurf/")
            || path_lower.contains("/windsurf/user/workspacestorage/")
            || self.mirror.owns(path_lower)
    }

    /// CWD discovery straight from workspace storage and the mirror, skipping
    /// the `discover_recent` round-trip.
    async fn discover_cwds(&self, now: i64, since_secs: i64) -> Vec<String> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        let mut dirs = Vec::new();
        let ws_root = app_config_dir_in("Windsurf", &home)
            .join("User")
            .join("workspaceStorage");
        dirs.extend(find_chat_session_dirs(&ws_root).await);
        if let Some(mirror) = self.populated_mirror_dir(&home).await {
            dirs.push(mirror);
        }
        // Fall back to default: read files for CWDs.
        let logs = recent_files_with_exts(&dirs, now, since_secs, &["json"]).await;
        let mut set = tokio::task::JoinSet::new();
        for file in logs {
            let path = file.path;
            set.spawn(async move {
                crate::discovery::scanner::parse_session_metadata(&path)
                    .await
                    .cwd
            });
        }
        let mut cwds = Vec::new();
        while let Some(result) = set.join_next().await {
            if let Ok(Some(cwd)) = result {
                cwds.push(cwd);
            }
        }
        cwds
    }

    // Windsurf IDE chatSessions only. Devin CLI bundled into Windsurf 2.0 is a
    // P1 spike in the 2026-05-25 audit.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// IDE-only: `<app-config>/Windsurf/User/workspaceStorage/` chat sessions
    /// and the `~/.codeium/windsurf/cascade/` tree. `mirror`: the configured
    /// mirror directory, if any.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: Vec::new(),
            ide_desktop: vec![
                app_config_dir_in("Windsurf", home)
                    .join("User")
                    .join("workspaceStorage"),
                home.join(".codeium").join("windsurf").join("cascade"),
            ],
            mirror: self.mirror.roots_in(home),
        }
    }
}

#[cfg(test)]
pub(crate) fn sample_codeium_log_path(home: &Path) -> PathBuf {
    home.join(".codeium")
        .join("windsurf")
        .join("cascade")
        .join("abc.pb")
}

#[cfg(test)]
pub(crate) fn sample_linux_workspace_log_path(home: &Path) -> PathBuf {
    crate::discovery::app_config_dir_for_platform(
        "Windsurf",
        home,
        crate::discovery::DesktopPlatform::Linux,
        None,
        None,
    )
    .join("User")
    .join("workspaceStorage")
    .join("x")
    .join("chatSessions")
    .join("session.json")
}

#[cfg(test)]
pub(crate) fn sample_windows_workspace_log_path(home: &Path, appdata: &Path) -> PathBuf {
    crate::discovery::app_config_dir_for_platform(
        "Windsurf",
        home,
        crate::discovery::DesktopPlatform::Windows,
        Some(appdata),
        None,
    )
    .join("User")
    .join("workspaceStorage")
    .join("x")
    .join("chatSessions")
    .join("session.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn mirror_dir(home: &Path) -> Option<PathBuf> {
        Some(home.join("mirror").join("windsurf"))
    }

    /// An adapter with a mirror registered, mimicking an embedding application.
    static MIRRORED: WindsurfExplorer = WindsurfExplorer {
        mirror: SessionMirror {
            dir: mirror_dir,
            path_marker: Some("/mirror/windsurf/"),
        },
    };

    #[tokio::test]
    async fn test_windsurf_log_dirs_collects_chat_sessions() {
        let home = TempDir::new().unwrap();
        let ws_root = app_config_dir_in("Windsurf", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("abc")
            .join("chatSessions");
        tokio::fs::create_dir_all(&ws_root).await.unwrap();
        let other_root = app_config_dir_in("Windsurf", home.path())
            .join("User")
            .join("other")
            .join("chatSessions");
        tokio::fs::create_dir_all(&other_root).await.unwrap();

        let dirs = DISK_WINDSURF.log_dirs_in(home.path()).await;
        assert!(dirs.contains(&ws_root));
        assert!(dirs.contains(&other_root));
    }

    #[tokio::test]
    async fn test_log_dirs_include_mirror_with_json() {
        let home = TempDir::new().unwrap();
        let mirror = mirror_dir(home.path()).unwrap();
        tokio::fs::create_dir_all(&mirror).await.unwrap();
        tokio::fs::write(mirror.join("cascade-1.json"), "{}")
            .await
            .unwrap();

        let dirs = MIRRORED.log_dirs_in(home.path()).await;
        assert!(
            dirs.contains(&mirror),
            "a mirror holding JSON should be included in discovery"
        );
        // Unconfigured adapters never look at it.
        assert!(
            !DISK_WINDSURF
                .log_dirs_in(home.path())
                .await
                .contains(&mirror)
        );
    }

    #[tokio::test]
    async fn test_log_dirs_exclude_empty_mirror() {
        let home = TempDir::new().unwrap();
        let mirror = mirror_dir(home.path()).unwrap();
        tokio::fs::create_dir_all(&mirror).await.unwrap();

        let dirs = MIRRORED.log_dirs_in(home.path()).await;
        assert!(!dirs.contains(&mirror), "empty mirror should be excluded");
    }

    /// End-to-end: a mirrored file with a recent mtime is discovered by the
    /// same pipeline `discover_recent()` uses internally.
    #[tokio::test]
    async fn test_mirrored_session_discovered_end_to_end() {
        use crate::discovery::set_file_mtime;

        let home = TempDir::new().unwrap();
        let mirror = mirror_dir(home.path()).unwrap();
        tokio::fs::create_dir_all(&mirror).await.unwrap();

        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 7 * 86_400;

        let cascade = mirror.join("abc-123.json");
        let payload = json!({
            "sessionId": "abc-123",
            "cascadeId": "abc-123",
            "source": "windsurf_api",
            "baseUri": { "path": "file:///Users/avery/dev/my-project" },
            "steps": []
        });
        tokio::fs::write(&cascade, payload.to_string())
            .await
            .unwrap();
        set_file_mtime(&cascade, now - 3600); // 1 hour ago

        let dirs = MIRRORED.log_dirs_in(home.path()).await;
        assert!(dirs.contains(&mirror));

        let files = recent_files_with_exts(&dirs, now, since_secs, &["json"]).await;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, cascade);

        let logs: Vec<SessionLog> = files
            .into_iter()
            .map(|file| SessionLog {
                environment: Default::default(),
                agent_type: AgentKind::Windsurf,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
            })
            .collect();
        assert_eq!(logs.len(), 1);
        assert!(matches!(logs[0].agent_type, AgentKind::Windsurf));
    }

    #[test]
    fn mirror_paths_are_owned_only_when_configured() {
        assert!(MIRRORED.owns_path("/home/avery/mirror/windsurf/abc.json"));
        assert!(!DISK_WINDSURF.owns_path("/home/avery/mirror/windsurf/abc.json"));
    }
}
