//! Kiro log discovery.
//!
//! Kiro stores IDE and CLI session files in:
//! - `<Kiro app config>/User/globalStorage/kiro.kiroagent/workspace-sessions/**/<sessionId>.json`
//! - `<Kiro app config>/User/globalStorage/kiro.kiroagent/*.chat`
//! - `~/.kiro/sessions/cli/<uuid>.json` plus `<uuid>.jsonl` (CLI V2)
//! - `~/.kiro/sessions/<workspace>/sess_<uuid>/session.json` plus `messages.jsonl` (CLI V3)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionSource, SurfacePaths, WatchRoot, app_config_dir_in, home_dir,
};
use async_trait::async_trait;
use serde_json::Value;

pub async fn all_log_dirs() -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    vec![
        kiro_storage_root_in(&home),
        kiro_cli_sessions_root_in(&home),
    ]
}

pub struct KiroExplorer;

#[async_trait]
impl AgentExplorer for KiroExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        discover_recent_in(&all_log_dirs().await, now, since_secs).await
    }

    /// Owns the Kiro IDE `globalStorage/kiro.kiroagent/` tree under each
    /// platform's Kiro app-config root (macOS Application Support, Linux
    /// `.config`, Windows AppData/Roaming).
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/library/application support/kiro/user/globalstorage/kiro.kiroagent/`  → `ide_desktop` (macOS)
    /// - `/.config/kiro/user/globalstorage/kiro.kiroagent/`                      → `ide_desktop` (Linux)
    /// - `/appdata/roaming/kiro/user/globalstorage/kiro.kiroagent/`              → `ide_desktop` (Windows)
    ///
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/library/application support/kiro/user/globalstorage/kiro.kiroagent/")
            || path_lower.contains("/.config/kiro/user/globalstorage/kiro.kiroagent/")
            || path_lower.contains("/appdata/roaming/kiro/user/globalstorage/kiro.kiroagent/")
            || path_lower.contains("/.kiro/sessions/cli/")
            || path_lower.contains("/.kiro/sessions/") && path_lower.contains("/sess_")
    }

    fn unmatched_surface(&self) -> &'static str {
        "unknown"
    }

    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: vec![kiro_cli_sessions_root_in(home)],
            ide_desktop: vec![kiro_storage_root_in(home)],
            mirror: Vec::new(),
        }
    }

    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        let paths = self.surface_paths(home);
        paths
            .cli
            .into_iter()
            .chain(paths.ide_desktop)
            .map(WatchRoot::recursive)
            .collect()
    }

    fn supports_subagents(&self) -> bool {
        true
    }

    async fn list_subagents(&self, parent_transcript: &Path) -> Vec<PathBuf> {
        let Some(parent_id) = parent_transcript
            .file_stem()
            .and_then(|value| value.to_str())
        else {
            return Vec::new();
        };
        let Some(directory) = parent_transcript.parent() else {
            return Vec::new();
        };
        if directory.file_name().and_then(|value| value.to_str()) != Some("cli") {
            return Vec::new();
        }
        let mut entries = match tokio::fs::read_dir(directory).await {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };
        let mut children = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !is_kiro_session_file(&path) || path == parent_transcript {
                continue;
            }
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if value.get("parent_session_id").and_then(Value::as_str) == Some(parent_id) {
                children.push(path);
            }
        }
        children.sort();
        children
    }

    fn subagent_id(&self, path: &Path) -> Option<String> {
        path.file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| is_uuid(value))
            .map(str::to_owned)
    }
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    kiro_storage_root_in(home)
        .join("workspace-sessions")
        .join("workspace-1")
        .join("session-a")
        .join("session-a.json")
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    mtime_epoch: i64,
    session_id: String,
    canonical: bool,
}

async fn discover_recent_in(roots: &[PathBuf], now: i64, since_secs: i64) -> Vec<SessionLog> {
    let cutoff = now - since_secs;
    let mut candidates = Vec::new();

    for root in roots {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let file_type = match entry.file_type().await {
                    Ok(file_type) => file_type,
                    Err(_) => continue,
                };
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }

                if !is_kiro_session_file(&path) {
                    continue;
                }

                let metadata = match tokio::fs::metadata(&path).await {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let modified = match metadata.modified() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let mtime_epoch = match modified.duration_since(UNIX_EPOCH) {
                    Ok(d) => d.as_secs() as i64,
                    Err(_) => continue,
                };
                if mtime_epoch < cutoff {
                    continue;
                }

                let canonical = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|name| name.ends_with(".json") && name != "sessions.json")
                    .unwrap_or(false);
                let session_id = session_id_for_file(&path, canonical).await;
                candidates.push(Candidate {
                    path,
                    mtime_epoch,
                    session_id,
                    canonical,
                });
            }
        }
    }

    let mut deduped: HashMap<String, Candidate> = HashMap::new();
    for candidate in candidates {
        match deduped.get(&candidate.session_id) {
            None => {
                deduped.insert(candidate.session_id.clone(), candidate);
            }
            Some(existing) => {
                let replace = (candidate.canonical && !existing.canonical)
                    || (candidate.canonical == existing.canonical
                        && candidate.mtime_epoch > existing.mtime_epoch);
                if replace {
                    deduped.insert(candidate.session_id.clone(), candidate);
                }
            }
        }
    }

    let mut selected: Vec<_> = deduped.into_values().collect();
    selected.sort_by(|a, b| a.path.cmp(&b.path));

    selected
        .into_iter()
        .map(|candidate| SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Kiro,
            source: SessionSource::File(candidate.path),
            updated_at: Some(candidate.mtime_epoch),
        })
        .collect()
}

fn kiro_storage_root_in(home: &Path) -> PathBuf {
    app_config_dir_in("Kiro", home)
        .join("User")
        .join("globalStorage")
        .join("kiro.kiroagent")
}

fn kiro_cli_sessions_root_in(home: &Path) -> PathBuf {
    home.join(".kiro").join("sessions")
}

fn is_kiro_session_file(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return false,
    };

    if file_name.ends_with(".chat") {
        return true;
    }

    if let Some(parent) = path.parent()
        && parent.file_name().and_then(|name| name.to_str()) == Some("cli")
        && file_name.ends_with(".json")
        && let Some(id) = path.file_stem().and_then(|name| name.to_str())
    {
        return is_uuid(id) && parent.join(format!("{id}.jsonl")).is_file();
    }

    if file_name == "session.json"
        && let Some(parent) = path.parent()
        && parent.join("messages.jsonl").is_file()
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("sess_"))
            .is_some_and(is_uuid)
    {
        return true;
    }

    if file_name.ends_with(".json") && file_name != "sessions.json" {
        let path_str = path.to_string_lossy().replace('\\', "/");
        return path_str.contains("/workspace-sessions/");
    }

    false
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

async fn session_id_for_file(path: &Path, canonical: bool) -> String {
    if path.file_name().and_then(|name| name.to_str()) == Some("session.json")
        && let Some(id) = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("sess_"))
            .filter(|id| is_uuid(id))
    {
        return id.to_owned();
    }
    if canonical {
        return path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown-session")
            .to_string();
    }

    if let Ok(content) = tokio::fs::read_to_string(path).await
        && let Ok(value) = serde_json::from_str::<Value>(&content)
    {
        if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
            return id.to_string();
        }
        if let Some(id) = value.get("executionId").and_then(|v| v.as_str()) {
            return id.to_string();
        }
    }

    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown-session")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::discovery::set_file_mtime;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_kiro_prefers_canonical_json_over_chat_for_same_session() {
        let home = TempDir::new().unwrap();
        let root = kiro_storage_root_in(home.path());

        let ws_dir = root
            .join("workspace-sessions")
            .join("workspace-1")
            .join("session-a");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let canonical = ws_dir.join("session-a.json");
        tokio::fs::write(
            &canonical,
            r#"{"sessionId":"session-a","workspaceDirectory":"/tmp/repo"}"#,
        )
        .await
        .unwrap();

        let chat = root.join("chat-a.chat");
        tokio::fs::write(&chat, r#"{"executionId":"session-a"}"#)
            .await
            .unwrap();

        let now = 1_700_000_000;
        set_file_mtime(&canonical, now - 10);
        set_file_mtime(&chat, now - 5);

        let logs = discover_recent_in(&[root], now, 3600).await;
        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::File(path) => assert_eq!(path, &canonical),
            SessionSource::Inline { .. } => panic!("expected file source"),
            SessionSource::ProviderDb { .. } => panic!("expected file source"),
        }
    }

    #[tokio::test]
    async fn test_kiro_uses_chat_when_canonical_missing() {
        let home = TempDir::new().unwrap();
        let root = kiro_storage_root_in(home.path());
        tokio::fs::create_dir_all(&root).await.unwrap();

        let chat = root.join("chat-only.chat");
        tokio::fs::write(&chat, r#"{"executionId":"exec-1"}"#)
            .await
            .unwrap();

        let now = 1_700_000_000;
        set_file_mtime(&chat, now - 10);

        let logs = discover_recent_in(&[root], now, 3600).await;
        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::File(path) => assert_eq!(path, &chat),
            SessionSource::Inline { .. } => panic!("expected file source"),
            SessionSource::ProviderDb { .. } => panic!("expected file source"),
        }
    }

    #[tokio::test]
    async fn test_kiro_canonical_session_id_uses_filename_when_json_invalid() {
        let home = TempDir::new().unwrap();
        let root = kiro_storage_root_in(home.path());

        let ws_dir = root
            .join("workspace-sessions")
            .join("workspace-1")
            .join("session-b");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let canonical = ws_dir.join("session-b.json");
        tokio::fs::write(&canonical, "{not-json").await.unwrap();

        let now = 1_700_000_000;
        set_file_mtime(&canonical, now - 10);

        let logs = discover_recent_in(&[root], now, 3600).await;
        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::File(path) => assert_eq!(path, &canonical),
            SessionSource::Inline { .. } => panic!("expected file source"),
            SessionSource::ProviderDb { .. } => panic!("expected file source"),
        }
    }

    #[tokio::test]
    async fn v2_child_relation_uses_only_parent_session_id() {
        let temp = TempDir::new().unwrap();
        let cli = temp.path().join("cli");
        tokio::fs::create_dir_all(&cli).await.unwrap();
        let root = cli.join("11111111-1111-4111-8111-111111111111.json");
        let child = cli.join("22222222-2222-4222-8222-222222222222.json");
        tokio::fs::write(
            &root,
            include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/root.json"),
        )
        .await
        .unwrap();
        tokio::fs::write(
            root.with_extension("jsonl"),
            include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/root.jsonl"),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &child,
            include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/child.json"),
        )
        .await
        .unwrap();
        tokio::fs::write(
            child.with_extension("jsonl"),
            include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/child.jsonl"),
        )
        .await
        .unwrap();
        let ignored_history = cli.join("11111111-1111-4111-8111-111111111111.history");
        let ignored_lock = cli.join("11111111-1111-4111-8111-111111111111.lock");
        assert!(!is_kiro_session_file(&ignored_history));
        assert!(!is_kiro_session_file(&ignored_lock));
        assert_eq!(KiroExplorer.list_subagents(&root).await, vec![child]);
    }
}
