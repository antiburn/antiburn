//! Copilot session log discovery.
//!
//! Two source kinds, both surfaced as `AgentKind::Copilot`:
//!
//! 1. **VS Code chat sessions** (the original variant). Each VS Code-family
//!    install stores chat sessions per-workspace:
//!    - macOS: `~/Library/Application Support/Code/User/workspaceStorage/*/chatSessions/*.json`
//!    - Linux: `~/.config/Code/User/workspaceStorage/*/chatSessions/*.json`
//!    - Windows: `%APPDATA%\Code\User\workspaceStorage\*\chatSessions\*.json`
//!
//! 2. **Copilot CLI session-state** (GA February 2026). The `copilot` CLI and
//!    the Copilot Desktop App (technical preview 2026-05-14) share a single
//!    on-disk store:
//!    - macOS/Linux: `~/.copilot/session-state/<session-id>/...` (honors
//!      `XDG_CONFIG_HOME` if set, falling back to `~/.config/copilot/...`)
//!    - Windows: `%LOCALAPPDATA%\GitHub Copilot CLI\session-state\...`
//!
//! Each session is its own subdirectory under `session-state/` containing one
//! or more `*.json` / `*.jsonl` files. The walker is layout-tolerant: it
//! recursively collects any directory containing matching files.

use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, DesktopPlatform, SessionLog, SessionSource, SurfacePaths, WatchRoot,
    app_config_dir_in, collect_dirs_with_exts, current_desktop_platform, env_path_when_real_home,
    find_chat_session_dirs, home_dir, recent_files_with_exts,
};
use async_trait::async_trait;

const CLI_FILE_EXTS: &[&str] = &["json", "jsonl"];

/// Substrings of the VS Code-family on-disk layout used by Copilot Chat.
/// Both must appear in the (lowercased, forward-slashed) path to claim it
/// as Copilot — `/code/user/` alone would catch unrelated user directories
/// like `~/projects/code/user/...`. Narrower agents (Cline / Roo Code) are
/// matched before Copilot by the dispatcher.
const VS_CODE_USER_FRAGMENT: &str = "/code/user/";
const VS_CODE_WORKSPACE_STORAGE_FRAGMENT: &str = "/workspacestorage/";
const VS_CODE_GLOBAL_STORAGE_FRAGMENT: &str = "/globalstorage/";

/// Return all Copilot log directories for use by the post-commit hook.
pub async fn log_dirs() -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    log_dirs_in(&home).await
}

/// Return all Copilot log directories for backfill (not repo-scoped).
pub async fn all_log_dirs() -> Vec<PathBuf> {
    log_dirs().await
}

pub struct CopilotExplorer;

#[async_trait]
impl AgentExplorer for CopilotExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let dirs = all_log_dirs().await;
        recent_files_with_exts(&dirs, now, since_secs, CLI_FILE_EXTS)
            .await
            .into_iter()
            .map(|file| SessionLog {
                agent_type: AgentKind::Copilot,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns CLI `session-state/` trees (`~/.copilot/`, XDG_CONFIG, or
    /// `GitHub Copilot CLI/`) plus any VS Code `User/{workspace,global}Storage`
    /// chat session. The VS Code arm is fork-tolerant — works for forks not
    /// enumerated in `surface_paths`.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.copilot/session-state/`              → `cli` (CLI + Copilot Desktop App
    ///                                                    inherit the same store)
    /// - `/copilot/session-state/`               → `cli` (`$XDG_CONFIG_HOME/copilot/...`)
    /// - `/github copilot cli/session-state/`    → `cli` (Windows `%LOCALAPPDATA%`)
    /// - VS Code `User/` + `workspaceStorage/`   → `ide_desktop` (chatSessions)
    /// - VS Code `User/` + `globalStorage/`      → `ide_desktop`
    fn owns_path(&self, path_lower: &str) -> bool {
        if path_lower.contains("/.copilot/session-state/")
            || path_lower.contains("/copilot/session-state/")
            || path_lower.contains("/github copilot cli/session-state/")
        {
            return true;
        }
        path_lower.contains(VS_CODE_USER_FRAGMENT)
            && (path_lower.contains(VS_CODE_WORKSPACE_STORAGE_FRAGMENT)
                || path_lower.contains(VS_CODE_GLOBAL_STORAGE_FRAGMENT))
    }

    // Copilot is bi-modal:
    //   - CLI:    `~/.copilot/session-state/`, `<XDG_CONFIG>/copilot/session-state/`,
    //             `<LOCALAPPDATA>/GitHub Copilot CLI/session-state/`.
    //   - IDE:    VS Code workspaceStorage/globalStorage chatSessions.
    // Copilot is bi-modal: CLI under platform-specific session-state roots,
    // IDE under VS Code workspaceStorage. Trait default classifies via
    // `surface_paths`; unmatched paths default to "ide_desktop" to preserve
    // the historical permissive default.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// CLI: per-platform `session-state/` roots (unix `~/.copilot/`, XDG, or
    /// `<LOCALAPPDATA>/GitHub Copilot CLI/`). IDE: `<app-config>/Code/User/
    /// workspaceStorage/**` only — VS Code forks are classified via
    /// `owns_path`'s substring matcher rather than enumeration here.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        let local_appdata = env_path_when_real_home(home, "LOCALAPPDATA")
            .or_else(|| env_path_when_real_home(home, "APPDATA"));
        let xdg_config_home = env_path_when_real_home(home, "XDG_CONFIG_HOME");
        SurfacePaths {
            cli: cli_root_candidates_for_platform(
                home,
                current_desktop_platform(),
                local_appdata.as_deref(),
                xdg_config_home.as_deref(),
            ),
            ide_desktop: vec![
                app_config_dir_in("Code", home)
                    .join("User")
                    .join("workspaceStorage"),
            ],
            mirror: Vec::new(),
        }
    }

    /// The same CLI and IDE roots discovery walks.
    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        let paths = self.surface_paths(home);
        paths
            .cli
            .into_iter()
            .chain(paths.ide_desktop)
            .map(WatchRoot::recursive)
            .collect()
    }
}

#[cfg(test)]
pub(crate) fn sample_vs_code_log_path(home: &Path) -> PathBuf {
    app_config_dir_in("Code", home)
        .join("User")
        .join("workspaceStorage")
        .join("x")
        .join("chatSessions")
        .join("session.json")
}

#[cfg(test)]
pub(crate) fn sample_cli_home_log_path(home: &Path) -> PathBuf {
    home.join(".copilot")
        .join("session-state")
        .join("abc")
        .join("transcript.jsonl")
}

#[cfg(test)]
pub(crate) fn sample_cli_xdg_log_path(home: &Path) -> PathBuf {
    home.join(".config")
        .join("copilot")
        .join("session-state")
        .join("abc")
        .join("state.json")
}

#[cfg(test)]
pub(crate) fn sample_cli_windows_log_path(local_appdata: &Path) -> PathBuf {
    local_appdata
        .join("GitHub Copilot CLI")
        .join("session-state")
        .join("abc")
        .join("state.json")
}

async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let vs_code_dirs = vs_code_chat_session_dirs_in(home).await;
    let cli_dirs = cli_session_dirs_in(home).await;

    ::tracing::debug!(
        target: "antiburn::discovery::copilot",
        vs_code_dirs = vs_code_dirs.len(),
        cli_dirs = cli_dirs.len(),
        "Copilot session directories discovered"
    );

    let mut out = vs_code_dirs;
    out.extend(cli_dirs);
    out
}

async fn vs_code_chat_session_dirs_in(home: &Path) -> Vec<PathBuf> {
    let ws_root = app_config_dir_in("Code", home)
        .join("User")
        .join("workspaceStorage");
    find_chat_session_dirs(&ws_root).await
}

async fn cli_session_dirs_in(home: &Path) -> Vec<PathBuf> {
    let appdata = env_path_when_real_home(home, "LOCALAPPDATA")
        .or_else(|| env_path_when_real_home(home, "APPDATA"));
    let xdg_config_home = env_path_when_real_home(home, "XDG_CONFIG_HOME");

    cli_session_dirs_for_platform(
        home,
        current_desktop_platform(),
        appdata.as_deref(),
        xdg_config_home.as_deref(),
    )
    .await
}

async fn cli_session_dirs_for_platform(
    home: &Path,
    platform: DesktopPlatform,
    local_appdata: Option<&Path>,
    xdg_config_home: Option<&Path>,
) -> Vec<PathBuf> {
    let roots = cli_root_candidates_for_platform(home, platform, local_appdata, xdg_config_home);
    let mut results = Vec::new();
    for root in roots {
        collect_dirs_with_exts(&root, &mut results, CLI_FILE_EXTS).await;
    }
    results
}

fn cli_root_candidates_for_platform(
    home: &Path,
    platform: DesktopPlatform,
    local_appdata: Option<&Path>,
    xdg_config_home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    match platform {
        DesktopPlatform::Windows => {
            if let Some(appdata) = local_appdata {
                roots.push(appdata.join("GitHub Copilot CLI").join("session-state"));
            } else {
                roots.push(
                    home.join("AppData")
                        .join("Local")
                        .join("GitHub Copilot CLI")
                        .join("session-state"),
                );
            }
        }
        DesktopPlatform::Macos | DesktopPlatform::Linux => {
            roots.push(home.join(".copilot").join("session-state"));
            if let Some(xdg) = xdg_config_home {
                roots.push(xdg.join("copilot").join("session-state"));
            }
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_copilot_log_dirs_collects_chat_sessions() {
        let home = TempDir::new().unwrap();
        let ws_root = app_config_dir_in("Code", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("abc")
            .join("chatSessions");
        tokio::fs::create_dir_all(&ws_root).await.unwrap();

        let dirs = log_dirs_in(home.path()).await;
        assert!(dirs.contains(&ws_root));
    }

    #[tokio::test]
    async fn test_copilot_log_dirs_includes_cli_session_state_unix() {
        let home = TempDir::new().unwrap();
        let cli_session_dir = home
            .path()
            .join(".copilot")
            .join("session-state")
            .join("synth-session");
        tokio::fs::create_dir_all(&cli_session_dir).await.unwrap();
        tokio::fs::write(cli_session_dir.join("state.json"), "{}")
            .await
            .unwrap();

        let dirs =
            cli_session_dirs_for_platform(home.path(), DesktopPlatform::Macos, None, None).await;
        assert!(dirs.contains(&cli_session_dir));

        let dirs_linux =
            cli_session_dirs_for_platform(home.path(), DesktopPlatform::Linux, None, None).await;
        assert!(dirs_linux.contains(&cli_session_dir));
    }

    #[tokio::test]
    async fn test_copilot_cli_walks_jsonl_in_session_state() {
        let home = TempDir::new().unwrap();
        let cli_session_dir = home
            .path()
            .join(".copilot")
            .join("session-state")
            .join("synth-session");
        tokio::fs::create_dir_all(&cli_session_dir).await.unwrap();
        tokio::fs::write(
            cli_session_dir.join("transcript.jsonl"),
            "{\"session_id\":\"x\"}\n",
        )
        .await
        .unwrap();

        let dirs =
            cli_session_dirs_for_platform(home.path(), DesktopPlatform::Linux, None, None).await;
        assert!(dirs.contains(&cli_session_dir));
    }

    #[tokio::test]
    async fn test_copilot_cli_honors_xdg_config_home() {
        let home = TempDir::new().unwrap();
        let xdg = TempDir::new().unwrap();
        let xdg_session_dir = xdg.path().join("copilot").join("session-state").join("s1");
        tokio::fs::create_dir_all(&xdg_session_dir).await.unwrap();
        tokio::fs::write(xdg_session_dir.join("state.json"), "{}")
            .await
            .unwrap();

        let dirs = cli_session_dirs_for_platform(
            home.path(),
            DesktopPlatform::Linux,
            None,
            Some(xdg.path()),
        )
        .await;
        assert!(dirs.contains(&xdg_session_dir));
    }

    #[tokio::test]
    async fn test_copilot_cli_uses_local_appdata_on_windows() {
        let home = TempDir::new().unwrap();
        let appdata = TempDir::new().unwrap();
        let cli_session_dir = appdata
            .path()
            .join("GitHub Copilot CLI")
            .join("session-state")
            .join("s1");
        tokio::fs::create_dir_all(&cli_session_dir).await.unwrap();
        tokio::fs::write(cli_session_dir.join("state.json"), "{}")
            .await
            .unwrap();

        let dirs = cli_session_dirs_for_platform(
            home.path(),
            DesktopPlatform::Windows,
            Some(appdata.path()),
            None,
        )
        .await;
        assert!(dirs.contains(&cli_session_dir));
    }

    #[tokio::test]
    async fn test_copilot_log_dirs_handles_missing_cli_dir() {
        let home = TempDir::new().unwrap();
        let ws_root = app_config_dir_in("Code", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("abc")
            .join("chatSessions");
        tokio::fs::create_dir_all(&ws_root).await.unwrap();

        // No ~/.copilot/session-state seeded; should not panic.
        let dirs = log_dirs_in(home.path()).await;
        assert!(dirs.contains(&ws_root));
    }

    #[tokio::test]
    async fn test_copilot_discover_recent_finds_cli_session() {
        let home = TempDir::new().unwrap();
        let cli_session_dir = home
            .path()
            .join(".copilot")
            .join("session-state")
            .join("synth-session");
        tokio::fs::create_dir_all(&cli_session_dir).await.unwrap();
        let transcript = cli_session_dir.join("transcript.jsonl");
        tokio::fs::write(&transcript, "{\"session_id\":\"x\"}\n")
            .await
            .unwrap();

        // discover_recent uses log_dirs() which reads the real HOME, so we
        // exercise the platform helper directly to verify the dir-walking and
        // extension-matching path without depending on env state.
        let dirs =
            cli_session_dirs_for_platform(home.path(), DesktopPlatform::Linux, None, None).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let files = recent_files_with_exts(&dirs, now, 3600, CLI_FILE_EXTS).await;
        assert!(
            files.iter().any(|f| f.path == transcript),
            "expected transcript.jsonl to be discovered, got {:?}",
            files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }
}
