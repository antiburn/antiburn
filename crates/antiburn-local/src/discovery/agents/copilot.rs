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
//!    Each session is its own subdirectory under `session-state/`. The CLI
//!    writes `events.jsonl` there as the transcript. The rest of the
//!    directory holds unrelated files: `workspace.yaml`, `plan.md`, a
//!    `checkpoints/` directory of markdown, a `files/` directory of user
//!    attachments (any type, including `.json`), and, when autopilot is
//!    used, `autopilot-objective.json`. Only `events.jsonl` is a session; the
//!    walker looks for that exact file name and ignores every sibling.

use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, DesktopPlatform, SessionLog, SessionSource, SurfacePaths, WatchRoot,
    app_config_dir_in, collect_dirs_with_file_named, current_desktop_platform,
    env_path_when_real_home, find_chat_session_dirs, home_dir, recent_files_named,
    recent_files_with_exts,
};
use async_trait::async_trait;

/// The only file name the CLI's `session-state/<session-id>/` directory
/// holds that antiburn treats as a session transcript.
const CLI_TRANSCRIPT_FILE_NAME: &str = "events.jsonl";

/// [`CLI_TRANSCRIPT_FILE_NAME`] as a path suffix, for matching a lowercased
/// full path in [`CopilotExplorer::owns_path`].
const CLI_TRANSCRIPT_PATH_SUFFIX: &str = "/events.jsonl";

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
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };

        // The two source kinds match on different terms: VS Code chat
        // sessions are any `*.json` file, but a CLI session directory holds
        // several `.json` / `.jsonl` files and only `events.jsonl` is the
        // transcript. Each dir set uses the matcher that fits its layout.
        let vs_code_dirs = vs_code_chat_session_dirs_in(&home).await;
        let cli_dirs = cli_session_dirs_in(&home).await;

        let mut files = recent_files_with_exts(&vs_code_dirs, now, since_secs, &["json"]).await;
        files
            .extend(recent_files_named(&cli_dirs, now, since_secs, CLI_TRANSCRIPT_FILE_NAME).await);

        files
            .into_iter()
            .map(|file| SessionLog {
                agent_type: AgentKind::Copilot,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns `events.jsonl` under a CLI `session-state/` tree (`~/.copilot/`,
    /// XDG_CONFIG, or `GitHub Copilot CLI/`) plus any VS Code
    /// `User/{workspace,global}Storage` chat session. The VS Code arm is
    /// fork-tolerant — works for forks not enumerated in `surface_paths`.
    ///
    /// A CLI session directory also holds `workspace.yaml`, `plan.md`,
    /// `checkpoints/`, `files/`, and `autopilot-objective.json`; none of
    /// those is a session, so only the `events.jsonl` path is claimed.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.copilot/session-state/.../events.jsonl`           → `cli` (CLI + Copilot
    ///                                                                 Desktop App inherit
    ///                                                                 the same store)
    /// - `/copilot/session-state/.../events.jsonl`            → `cli` (`$XDG_CONFIG_HOME/copilot/...`)
    /// - `/github copilot cli/session-state/.../events.jsonl` → `cli` (Windows `%LOCALAPPDATA%`)
    /// - VS Code `User/` + `workspaceStorage/`                → `ide_desktop` (chatSessions)
    /// - VS Code `User/` + `globalStorage/`                   → `ide_desktop`
    fn owns_path(&self, path_lower: &str) -> bool {
        let is_cli_root = path_lower.contains("/.copilot/session-state/")
            || path_lower.contains("/copilot/session-state/")
            || path_lower.contains("/github copilot cli/session-state/");
        if is_cli_root {
            return path_lower.ends_with(CLI_TRANSCRIPT_PATH_SUFFIX);
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
        .join(CLI_TRANSCRIPT_FILE_NAME)
}

#[cfg(test)]
pub(crate) fn sample_cli_xdg_log_path(home: &Path) -> PathBuf {
    home.join(".config")
        .join("copilot")
        .join("session-state")
        .join("abc")
        .join(CLI_TRANSCRIPT_FILE_NAME)
}

#[cfg(test)]
pub(crate) fn sample_cli_windows_log_path(local_appdata: &Path) -> PathBuf {
    local_appdata
        .join("GitHub Copilot CLI")
        .join("session-state")
        .join("abc")
        .join(CLI_TRANSCRIPT_FILE_NAME)
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
        collect_dirs_with_file_named(&root, &mut results, CLI_TRANSCRIPT_FILE_NAME).await;
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
        tokio::fs::write(cli_session_dir.join(CLI_TRANSCRIPT_FILE_NAME), "{}")
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
            cli_session_dir.join(CLI_TRANSCRIPT_FILE_NAME),
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
        tokio::fs::write(xdg_session_dir.join(CLI_TRANSCRIPT_FILE_NAME), "{}")
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
        tokio::fs::write(cli_session_dir.join(CLI_TRANSCRIPT_FILE_NAME), "{}")
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
        let transcript = cli_session_dir.join(CLI_TRANSCRIPT_FILE_NAME);
        tokio::fs::write(&transcript, "{\"session_id\":\"x\"}\n")
            .await
            .unwrap();

        // discover_recent uses log_dirs() which reads the real HOME, so we
        // exercise the platform helper directly to verify the dir-walking and
        // name-matching path without depending on env state.
        let dirs =
            cli_session_dirs_for_platform(home.path(), DesktopPlatform::Linux, None, None).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let files = recent_files_named(&dirs, now, 3600, CLI_TRANSCRIPT_FILE_NAME).await;
        assert!(
            files.iter().any(|f| f.path == transcript),
            "expected events.jsonl to be discovered, got {:?}",
            files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    /// A real CLI session directory holds `events.jsonl` next to
    /// `workspace.yaml`, `plan.md`, a `checkpoints/` directory, a `files/`
    /// directory of attachments (any type, including `.json`), and,
    /// optionally, `autopilot-objective.json`. Only `events.jsonl` is a
    /// session; the sibling files and the `files/` attachment must not turn
    /// into their own discovered sessions.
    #[tokio::test]
    async fn test_copilot_cli_session_dir_yields_only_events_jsonl() {
        let home = TempDir::new().unwrap();
        let session_dir = home
            .path()
            .join(".copilot")
            .join("session-state")
            .join("synth-session");
        let checkpoints_dir = session_dir.join("checkpoints");
        let files_dir = session_dir.join("files");
        tokio::fs::create_dir_all(&checkpoints_dir).await.unwrap();
        tokio::fs::create_dir_all(&files_dir).await.unwrap();

        let events = session_dir.join(CLI_TRANSCRIPT_FILE_NAME);
        tokio::fs::write(&events, "{\"session_id\":\"x\"}\n")
            .await
            .unwrap();
        tokio::fs::write(session_dir.join("workspace.yaml"), "root: /tmp/demo\n")
            .await
            .unwrap();
        tokio::fs::write(session_dir.join("plan.md"), "# Plan\n")
            .await
            .unwrap();
        let autopilot_objective = session_dir.join("autopilot-objective.json");
        tokio::fs::write(&autopilot_objective, "{}").await.unwrap();
        tokio::fs::write(checkpoints_dir.join("one.md"), "# Checkpoint\n")
            .await
            .unwrap();
        let attachment = files_dir.join("attachment.json");
        tokio::fs::write(&attachment, "{}").await.unwrap();

        let dirs =
            cli_session_dirs_for_platform(home.path(), DesktopPlatform::Linux, None, None).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let files = recent_files_named(&dirs, now, 3600, CLI_TRANSCRIPT_FILE_NAME).await;
        assert_eq!(
            files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
            vec![events.clone()],
            "expected exactly one discovered file, the events.jsonl transcript"
        );

        let explorer = CopilotExplorer;
        // `owns_path` takes a lowercased, forward-slashed path (see its doc
        // comment) — `normalize_for_matching` is what every real caller
        // uses, so a Windows-style backslash path matches too.
        let events_lower = crate::discovery::normalize_for_matching(&events.to_string_lossy());
        let autopilot_lower =
            crate::discovery::normalize_for_matching(&autopilot_objective.to_string_lossy());
        let attachment_lower =
            crate::discovery::normalize_for_matching(&attachment.to_string_lossy());
        assert!(explorer.owns_path(&events_lower));
        assert!(!explorer.owns_path(&autopilot_lower));
        assert!(!explorer.owns_path(&attachment_lower));
    }
}
