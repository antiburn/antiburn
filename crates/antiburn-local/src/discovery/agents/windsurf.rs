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
use crate::discovery::source_version::devin_content_fingerprint;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionMirror, SessionSource, SurfacePaths, WatchRoot,
    app_config_dir_in, dir_has_json_files, find_chat_session_dirs, home_dir,
    recent_files_with_exts,
};
use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags};

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

        // Cascade stores protobuf sessions directly in this root.
        dirs.insert(home.join(".codeium").join("windsurf").join("cascade"));

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
        let mut logs: Vec<SessionLog> =
            recent_files_with_exts(&dirs, now, since_secs, &["json", "pb"])
                .await
                .into_iter()
                .map(|file| SessionLog {
                    agent_type: AgentKind::Windsurf,
                    source: SessionSource::File(file.path),
                    updated_at: Some(file.mtime_epoch),
                    environment: Default::default(),
                })
                .collect();
        let database_path = devin_database_path(&home);
        let devin_logs = tokio::task::spawn_blocking(move || {
            discover_devin_sessions(&database_path, now, since_secs)
        })
        .await
        .unwrap_or_default();
        logs.extend(devin_logs);
        logs
    }

    async fn direct_session_source(
        &self,
        session_id: &str,
    ) -> crate::discovery::DirectSessionSource {
        let Some(home) = home_dir() else {
            return crate::discovery::DirectSessionSource::Unsupported;
        };
        let path = devin_database_path(&home);
        let id = session_id.to_owned();
        let lookup_path = path.clone();
        let found = tokio::task::spawn_blocking(move || devin_session_exists(&lookup_path, &id))
            .await
            .unwrap_or(false);
        if found {
            crate::discovery::DirectSessionSource::Found(SessionSource::ProviderDb {
                agent: AgentKind::Windsurf,
                db_path: path,
                session_id: session_id.to_owned(),
            })
        } else {
            // A Devin miss must still allow legacy Windsurf file lookup.
            crate::discovery::DirectSessionSource::Unsupported
        }
    }

    async fn provider_db_fingerprint(
        &self,
        db_path: &Path,
        session_id: &str,
    ) -> Option<(u64, u64)> {
        let path = db_path.to_owned();
        let id = session_id.to_owned();
        tokio::task::spawn_blocking(move || devin_session_fingerprint(&path, &id))
            .await
            .ok()
            .flatten()
    }

    /// Owns legacy Windsurf and current Devin Desktop IDE state across all platforms (`<app-config>/Windsurf/
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
        let owns_devin_cli = home_dir()
            .map(|home| {
                devin_database_path(&home)
                    .parent()
                    .map(|root| {
                        let root = format!("{}/", lower_path(root).trim_end_matches('/'));
                        path_lower.starts_with(&root)
                    })
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        path_lower.contains("/.codeium/windsurf/")
            || path_lower.contains("/library/application support/windsurf/")
            || path_lower.contains("/.config/windsurf/")
            || path_lower.contains("/appdata/roaming/windsurf/")
            || path_lower.contains("/windsurf/user/workspacestorage/")
            || owns_devin_cli
            || self.mirror.owns(path_lower)
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
            cli: vec![
                devin_database_path(home)
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| home.join(".local").join("share").join("devin").join("cli")),
            ],
            ide_desktop: vec![
                app_config_dir_in("Windsurf", home)
                    .join("User")
                    .join("workspaceStorage"),
                home.join(".codeium").join("windsurf").join("cascade"),
            ],
            mirror: self.mirror.roots_in(home),
        }
    }

    /// The IDE and Cascade roots discovery walks. The configured mirror
    /// directory is not watched: it is an embedding application's own copy,
    /// not a root this agent writes to.
    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        let surface = self.surface_paths(home);
        surface
            .cli
            .into_iter()
            .chain(surface.ide_desktop)
            .map(WatchRoot::recursive)
            .collect()
    }
}

fn devin_database_path(home: &Path) -> PathBuf {
    let data_home = crate::discovery::env_path_when_real_home(home, "XDG_DATA_HOME")
        .unwrap_or_else(|| home.join(".local").join("share"));
    data_home.join("devin").join("cli").join("sessions.db")
}

fn lower_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn discover_devin_sessions(path: &Path, now: i64, since_secs: i64) -> Vec<SessionLog> {
    if !path.is_file() {
        return Vec::new();
    }
    let path = path.to_owned();
    let Ok(connection) = Connection::open_with_flags(
        path.clone(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(version) = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
    else {
        return Vec::new();
    };
    if version != 17 {
        return Vec::new();
    }
    let cutoff = now.saturating_sub(since_secs.max(0));
    let Ok(mut statement) =
        connection.prepare("SELECT id, last_activity_at, created_at, hidden FROM sessions")
    else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|(id, updated, created, hidden)| {
            if hidden != 0 {
                return None;
            }
            let timestamp = updated.or(created)?;
            (timestamp >= cutoff).then_some(SessionLog {
                agent_type: AgentKind::Windsurf,
                source: SessionSource::ProviderDb {
                    agent: AgentKind::Windsurf,
                    db_path: path.clone(),
                    session_id: id,
                },
                updated_at: Some(timestamp),
                environment: Default::default(),
            })
        })
        .collect()
}

fn devin_session_exists(path: &Path, session_id: &str) -> bool {
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    connection
        .query_row(
            "SELECT 1 FROM sessions WHERE id = ?1 AND COALESCE(hidden, 0) = 0",
            [session_id],
            |_| Ok(()),
        )
        .is_ok()
}

fn devin_session_fingerprint(path: &Path, session_id: &str) -> Option<(u64, u64)> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let latest: i64 = connection.query_row(
        "SELECT COALESCE((SELECT last_activity_at FROM sessions WHERE id = ?1), (SELECT created_at FROM sessions WHERE id = ?1), 0), COUNT(*) FROM message_nodes WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    ).ok()?;
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM message_nodes WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .ok()?;
    let content = devin_content_fingerprint(&connection, path, session_id)?;
    Some((content ^ latest.max(0) as u64, rows.max(0) as u64))
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
        assert!(
            dirs.contains(
                &home
                    .path()
                    .join(".codeium")
                    .join("windsurf")
                    .join("cascade")
            )
        );
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

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn custom_xdg_devin_cli_path_is_owned() {
        let previous = std::env::var_os("XDG_DATA_HOME");
        let result = std::panic::catch_unwind(|| {
            unsafe { std::env::set_var("XDG_DATA_HOME", "/tmp/antiburn-test-xdg-data") };

            let home = home_dir().expect("the test environment must have a home directory");
            let path = devin_database_path(&home);
            assert!(DISK_WINDSURF.owns_path(&lower_path(&path)));
        });
        match previous {
            Some(value) => unsafe { std::env::set_var("XDG_DATA_HOME", value) },
            None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
        }
        result.unwrap();
    }
}
