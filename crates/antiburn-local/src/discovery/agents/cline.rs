//! Cline log discovery.
//!
//! Cline stores task/session artifacts under extension global storage and
//! backup folders.

use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionSource, SurfacePaths, WatchRoot, collect_dirs_with_exts,
    home_dir, recent_files_with_exts, vs_code_global_storage_task_roots,
};
use async_trait::async_trait;

const CLINE_EXT_IDS: &[&str] = &["saoudrizwan.claude-dev", "cline.cline"];

pub async fn all_log_dirs() -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    log_dirs_in(&home).await
}

pub struct ClineExplorer;

#[async_trait]
impl AgentExplorer for ClineExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let dirs = all_log_dirs().await;
        recent_files_with_exts(&dirs, now, since_secs, &["json"])
            .await
            .into_iter()
            // Cline 2.0+ writes `<id>.json` (metadata) alongside
            // `<id>.messages.json` (transcript) in the same dir. The metadata
            // file carries the title, cwd, and session id we need — the
            // messages file would just produce a duplicate SessionLog that
            // dedupes downstream by id. Skip it here so we don't pay the parse.
            .filter(|file| !is_cline_messages_file(&file.path))
            .map(|file| SessionLog {
                agent_type: AgentKind::Cline,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns `~/.cline/**` (CLI + JetBrains mirror) and any path containing a
    /// known Cline extension id. Substring scan over `CLINE_EXT_IDS` keeps
    /// `owns_path` in sync with `surface_paths` as new publishers are added.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.cline/`                  → `cli` (covers Cline 2.0 CLI + JetBrains plugin mirror)
    /// - `saoudrizwan.claude-dev`    → `ide_desktop` (VS Code-family globalStorage)
    /// - `cline.cline`               → `ide_desktop` (VS Code-family globalStorage)
    fn owns_path(&self, path_lower: &str) -> bool {
        if path_lower.contains("/.cline/") {
            return true;
        }
        CLINE_EXT_IDS.iter().any(|ext| path_lower.contains(ext))
    }

    // Cline is bi-modal: `~/.cline/{tasks,task-history,data/tasks,data/sessions}/`
    // is CLI (covers Cline 2.0 CLI and the JetBrains mirror; `data/sessions/`
    // is the current Cline 2.0+ layout, the others are legacy); VS Code
    // globalStorage for `saoudrizwan.claude-dev` / `cline.cline` is IDE.
    // Trait default classifies via `surface_paths`; unmatched paths default
    // to IDE to preserve the historical "anything-not-CLI is IDE" behavior —
    // IDE roots can vary across forked VS Code-family installs we don't
    // enumerate.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// CLI: `~/.cline/{tasks,task-history,data/tasks,data/sessions}/`. IDE:
    /// VS Code-family `globalStorage/<CLINE_EXT_IDS>/tasks` across every fork
    /// in `VS_CODE_FAMILY_APPS`.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: cli_dirs_in(home),
            ide_desktop: vs_code_global_storage_task_roots(home, CLINE_EXT_IDS),
            mirror: Vec::new(),
        }
    }

    /// The same CLI and extension roots discovery walks.
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

/// Cline 2.0+ pairs every `<id>.json` (metadata, has title/cwd/session_id)
/// with `<id>.messages.json` (transcript). Both live in the same session
/// directory, so `recent_files_with_exts` returns them both. Only the
/// metadata file is useful for `discover_recent`; filter the transcript out
/// here to avoid a redundant SessionLog per session.
fn is_cline_messages_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".messages.json"))
}

fn cli_dirs_in(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".cline").join("tasks"),
        home.join(".cline").join("task-history"),
        home.join(".cline").join("data").join("tasks"),
        home.join(".cline").join("data").join("sessions"),
    ]
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    crate::discovery::app_config_dir_in("Code", home)
        .join("User")
        .join("globalStorage")
        .join(CLINE_EXT_IDS[0])
        .join("tasks")
        .join("task-1")
        .join("task.json")
}

async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    for root in vs_code_global_storage_task_roots(home, CLINE_EXT_IDS) {
        collect_dirs_with_exts(&root, &mut dirs, &["json"]).await;
    }

    for backup in [
        home.join(".cline").join("task-history"),
        home.join(".cline").join("tasks"),
        home.join(".cline").join("data").join("tasks"),
        home.join(".cline").join("data").join("sessions"),
    ] {
        collect_dirs_with_exts(&backup, &mut dirs, &["json"]).await;
    }

    dirs.sort();
    dirs.dedup();
    dirs
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::app_config_dir_in;

    use tempfile::TempDir;

    #[tokio::test]
    async fn test_cline_log_dirs_collects_global_storage_and_backup_dirs() {
        let home = TempDir::new().unwrap();

        let task_dir = app_config_dir_in("Code", home.path())
            .join("User")
            .join("globalStorage")
            .join("saoudrizwan.claude-dev")
            .join("tasks")
            .join("task-1");
        tokio::fs::create_dir_all(&task_dir).await.unwrap();
        tokio::fs::write(task_dir.join("task-1.json"), "{}")
            .await
            .unwrap();

        let backup_dir = home.path().join(".cline").join("task-history").join("abc");
        tokio::fs::create_dir_all(&backup_dir).await.unwrap();
        tokio::fs::write(backup_dir.join("ui_messages.json"), "{}")
            .await
            .unwrap();

        let dirs = log_dirs_in(home.path()).await;

        assert!(dirs.contains(&task_dir));
        assert!(dirs.contains(&backup_dir));
    }

    #[tokio::test]
    async fn test_cline_log_dirs_collects_cline_2_sessions_layout() {
        let home = TempDir::new().unwrap();

        // Cline 2.0+ stores `<home>/.cline/data/sessions/<id>/<id>.json`
        // (metadata) alongside `<id>.messages.json` (transcript).
        let session_dir = home
            .path()
            .join(".cline")
            .join("data")
            .join("sessions")
            .join("1779760221048_s43gt");
        tokio::fs::create_dir_all(&session_dir).await.unwrap();
        tokio::fs::write(
            session_dir.join("1779760221048_s43gt.json"),
            r#"{"session_id":"1779760221048_s43gt","provider":"cline"}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(session_dir.join("1779760221048_s43gt.messages.json"), "[]")
            .await
            .unwrap();

        let dirs = log_dirs_in(home.path()).await;
        assert!(
            dirs.contains(&session_dir),
            "expected {} in {:?}",
            session_dir.display(),
            dirs,
        );
    }

    #[test]
    fn test_is_cline_messages_file() {
        use std::path::PathBuf;
        // The dedup filter in discover_recent relies on this predicate to
        // drop `<id>.messages.json` companions while keeping the metadata
        // file. Keep the legacy `ui_messages.json` shape (Cline 1.x) NOT
        // matched so backfill of older sessions still surfaces them.
        assert!(is_cline_messages_file(&PathBuf::from(
            "/x/sess-1/sess-1.messages.json"
        )));
        assert!(!is_cline_messages_file(&PathBuf::from(
            "/x/sess-1/sess-1.json"
        )));
        assert!(!is_cline_messages_file(&PathBuf::from(
            "/x/sess-1/ui_messages.json"
        )));
    }

    #[tokio::test]
    async fn test_cline_cli_dirs_in_includes_data_sessions() {
        let home = TempDir::new().unwrap();
        let cli_dirs = cli_dirs_in(home.path());
        assert!(
            cli_dirs.contains(&home.path().join(".cline").join("data").join("sessions")),
            "cli_dirs_in must include data/sessions so surface_paths classifies \
             Cline 2.0+ session paths as CLI (not the unmatched_surface IDE default). \
             got: {cli_dirs:?}",
        );
    }

    #[tokio::test]
    async fn test_cline_log_dirs_ignores_non_json_files() {
        let home = TempDir::new().unwrap();

        let task_dir = app_config_dir_in("Code", home.path())
            .join("User")
            .join("globalStorage")
            .join("saoudrizwan.claude-dev")
            .join("tasks")
            .join("task-1");
        tokio::fs::create_dir_all(&task_dir).await.unwrap();
        tokio::fs::write(task_dir.join("note.txt"), "nope")
            .await
            .unwrap();

        let dirs = log_dirs_in(home.path()).await;
        assert!(!dirs.contains(&task_dir));
    }
}
