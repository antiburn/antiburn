// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Kiro log discovery.
//!
//! Kiro stores agent session files in:
//! - `<Kiro app config>/User/globalStorage/kiro.kiroagent/workspace-sessions/**/<sessionId>.json`
//! - `<Kiro app config>/User/globalStorage/kiro.kiroagent/*.chat`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionSource, SurfacePaths, app_config_dir_in, home_dir,
};
use async_trait::async_trait;
use serde_json::Value;

pub async fn all_log_dirs() -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    vec![kiro_storage_root_in(&home)]
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
    /// No CLI substring — Kiro CLI path is undocumented (P1 spike in the
    /// 2026-05-25 audit).
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/library/application support/kiro/user/globalstorage/kiro.kiroagent/")
            || path_lower.contains("/.config/kiro/user/globalstorage/kiro.kiroagent/")
            || path_lower.contains("/appdata/roaming/kiro/user/globalstorage/kiro.kiroagent/")
    }

    // Kiro IDE globalStorage only on `main` — Kiro CLI path layout is a P1
    // spike in the 2026-05-25 audit.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// IDE-only: `<app-config>/Kiro/User/globalStorage/kiro.kiroagent/`. Kiro
    /// CLI layout is still a P1 spike (2026-05-25 audit).
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: Vec::new(),
            ide_desktop: vec![kiro_storage_root_in(home)],
            mirror: Vec::new(),
        }
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

fn is_kiro_session_file(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return false,
    };

    if file_name.ends_with(".chat") {
        return true;
    }

    if file_name.ends_with(".json") && file_name != "sessions.json" {
        let path_str = path.to_string_lossy().replace('\\', "/");
        return path_str.contains("/workspace-sessions/");
    }

    false
}

async fn session_id_for_file(path: &Path, canonical: bool) -> String {
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
}
