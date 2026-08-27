//! Cursor agent log discovery.
//!
//! Cursor stores sessions in a few different local formats:
//! - Legacy/VS Code style `chatSessions/*.json`
//! - Agent transcript JSONL files under `~/.cursor/projects/*/agent-transcripts/**`
//! - Desktop composer state split across `workspaceStorage/*` and `globalStorage/state.vscdb`

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};

use super::path_codec::decode_cursor_workspace_path_buf;
use crate::discovery::scanner::{self, AgentKind};
use crate::discovery::{
    AgentExplorer, DuplicateForkDetector, FORK_OBSERVATION_KEY, ForkObservation, SessionLog,
    SessionSource, SurfacePaths, app_config_dir_in, find_chat_session_dirs, home_dir,
    recent_files_with_exts,
};

const CURSOR_PROJECT_SKIP_DIRS: &[&str] = &["mcps", "agent-tools", "terminals"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CursorSourcePriority {
    LegacyChatSession = 0,
    StoreDb = 1,
    AgentTranscript = 2,
    DesktopComposer = 3,
}

#[derive(Debug, Clone, Default)]
struct CursorChatMetadata {
    title: Option<String>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
}

struct CursorSyntheticMetadata<'a> {
    cwd: Option<&'a str>,
    model: Option<&'a str>,
    title: Option<&'a str>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
    fork_observation: Option<&'a ForkObservation>,
    source_kind: &'a str,
}

#[derive(Debug, Clone)]
struct CursorDiscoveredSession {
    canonical_id: Option<String>,
    workspace_hint: Option<String>,
    content_len: usize,
    priority: CursorSourcePriority,
    log: SessionLog,
}

/// Cursor discovery over the vendor's own stores.
pub struct CursorExplorer {
    /// How to decide that one desktop composer was duplicated from another.
    ///
    /// Cursor's desktop composers record no parent id, so the only evidence is
    /// a copied conversation prefix and how much overlap counts as a fork is a
    /// policy call. Without a detector the adapter emits no duplicate-fork
    /// observation; the `store.db` sources, whose lineage the vendor's own blob
    /// ids make unambiguous, are unaffected.
    pub duplicate_fork_detector: Option<DuplicateForkDetector>,
}

/// The unconfigured adapter: no duplicate-fork detection.
pub static DISK_CURSOR: CursorExplorer = CursorExplorer {
    duplicate_fork_detector: None,
};

#[async_trait]
impl AgentExplorer for CursorExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        discover_recent_in(&home, now, since_secs, self.duplicate_fork_detector).await
    }

    /// Owns `~/.cursor/{projects,chats}/**` (CLI) and `<app-config>/Cursor/User/**`
    /// (IDE). The loose `/cursor/` substring is intentionally avoided — it
    /// catches unrelated user directories named `cursor`.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.cursor/`       → `cli` (`cursor-agent` projects + chats)
    /// - `/cursor/user/`   → `ide_desktop` (IDE workspaceStorage + state.vscdb)
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/.cursor/") || path_lower.contains("/cursor/user/")
    }

    // Cursor is bi-modal:
    //   - CLI (`cursor-agent`): `~/.cursor/projects/**/agent-transcripts/**`
    //                           and `~/.cursor/chats/**`.
    //   - IDE chat:             `<app_config>/Cursor/User/workspaceStorage/**`
    //                           and store.db cascades labelled `cursor-store:` /
    //                           `cursor-desktop:`.
    // Trait default classifies via `surface_paths`; inline labels without
    // an embedded ws-root substring (e.g. `cursor-desktop:<ws>:<composer>`)
    // fall through to the IDE default below.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// CLI: `~/.cursor/{projects,chats}/**` (cursor-agent). IDE:
    /// `<app-config>/Cursor/User/workspaceStorage/**` (chat sessions, store.db
    /// cascades surfaced via inline labels).
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: vec![
                home.join(".cursor").join("projects"),
                home.join(".cursor").join("chats"),
            ],
            ide_desktop: vec![
                app_config_dir_in("Cursor", home)
                    .join("User")
                    .join("workspaceStorage"),
            ],
            mirror: Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    app_config_dir_in("Cursor", home)
        .join("User")
        .join("workspaceStorage")
        .join("x")
        .join("chatSessions")
        .join("session.json")
}

async fn discover_recent_in(
    home: &Path,
    now: i64,
    since_secs: i64,
    duplicate_fork_detector: Option<DuplicateForkDetector>,
) -> Vec<SessionLog> {
    let mut discovered: HashMap<String, CursorDiscoveredSession> = HashMap::new();

    let ws_root = app_config_dir_in("Cursor", home)
        .join("User")
        .join("workspaceStorage");
    let chat_session_dirs = find_chat_session_dirs(&ws_root).await;
    for file in recent_files_with_exts(&chat_session_dirs, now, since_secs, &["json"]).await {
        let metadata = scanner::parse_session_metadata(&file.path).await;
        let candidate = CursorDiscoveredSession {
            canonical_id: metadata.session_id,
            workspace_hint: metadata.cwd,
            content_len: 0,
            priority: CursorSourcePriority::LegacyChatSession,
            log: SessionLog {
                environment: Default::default(),
                agent_type: AgentKind::Cursor,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
            },
        };
        merge_cursor_session(&mut discovered, candidate);
    }

    for candidate in discover_agent_transcripts(home, now, since_secs).await {
        merge_cursor_session(&mut discovered, candidate);
    }

    for candidate in discover_store_db_sessions(home, now, since_secs).await {
        merge_cursor_session(&mut discovered, candidate);
    }

    let desktop_logs =
        discover_desktop_sessions(home, now, since_secs, duplicate_fork_detector).await;
    for candidate in desktop_logs {
        merge_cursor_session(&mut discovered, candidate);
    }

    let mut logs = discovered
        .into_values()
        .map(|candidate| candidate.log)
        .collect::<Vec<_>>();

    logs.sort_by(|a, b| {
        a.updated_at
            .unwrap_or_default()
            .cmp(&b.updated_at.unwrap_or_default())
            .then_with(|| a.source_label().cmp(&b.source_label()))
    });
    logs
}

#[cfg(test)]
async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    let ws_root = app_config_dir_in("Cursor", home)
        .join("User")
        .join("workspaceStorage");
    dirs.extend(find_chat_session_dirs(&ws_root).await);

    let projects_dir = home.join(".cursor").join("projects");
    dirs.extend(collect_agent_transcript_dirs(&projects_dir).await);

    dirs
}

async fn collect_agent_transcript_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
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

            if !file_type.is_dir() {
                continue;
            }

            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };

            if CURSOR_PROJECT_SKIP_DIRS.contains(&name) {
                continue;
            }

            if name == "agent-transcripts" {
                out.push(path);
                continue;
            }

            stack.push(path);
        }
    }

    out
}

async fn discover_agent_transcripts(
    home: &Path,
    now: i64,
    since_secs: i64,
) -> Vec<CursorDiscoveredSession> {
    let cutoff = now - since_secs;
    let projects_dir = home.join(".cursor").join("projects");
    let transcript_dirs = collect_agent_transcript_dirs(&projects_dir).await;
    let workspace_map = collect_workspace_paths(home).await;
    let chat_metadata = collect_cursor_chat_metadata(home).await;
    let mut workspace_decode_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut out = Vec::new();

    for dir in transcript_dirs {
        let mut stack = vec![dir.clone()];
        while let Some(current) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&current).await {
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
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }

                let metadata = match tokio::fs::metadata(&path).await {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                let mtime_epoch = match metadata.modified() {
                    Ok(mtime) => match mtime.duration_since(UNIX_EPOCH) {
                        Ok(d) => d.as_secs() as i64,
                        Err(_) => continue,
                    },
                    Err(_) => continue,
                };
                if mtime_epoch < cutoff {
                    continue;
                }

                let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let workspace_key = path
                    .strip_prefix(&projects_dir)
                    .ok()
                    .and_then(|relative| relative.components().next())
                    .map(|component| component.as_os_str().to_string_lossy().to_string());
                let cwd = if let Some(workspace_key) = workspace_key.as_deref() {
                    if let Some(path) = workspace_map.get(workspace_key).cloned() {
                        Some(path)
                    } else if let Some(cached) = workspace_decode_cache.get(workspace_key) {
                        cached.clone()
                    } else {
                        let decoded = decode_cursor_workspace_path(workspace_key, home).await;
                        workspace_decode_cache.insert(workspace_key.to_string(), decoded.clone());
                        decoded
                    }
                } else {
                    None
                };
                let content = match tokio::fs::read_to_string(&path).await {
                    Ok(content) => content,
                    Err(_) => continue,
                };
                let metadata = chat_metadata.get(session_id).cloned().unwrap_or_default();
                let label = path.to_string_lossy().to_string();
                let source = SessionSource::Inline {
                    label,
                    content: prepend_cursor_metadata_line(
                        &content,
                        session_id,
                        CursorSyntheticMetadata {
                            cwd: cwd.as_deref(),
                            model: None,
                            title: metadata.title.as_deref(),
                            created_at: metadata.created_at,
                            updated_at: metadata.updated_at,
                            fork_observation: None,
                            source_kind: "agent_transcript",
                        },
                    ),
                };
                out.push(CursorDiscoveredSession {
                    canonical_id: Some(session_id.to_string()),
                    workspace_hint: cwd,
                    content_len: content.len(),
                    priority: CursorSourcePriority::AgentTranscript,
                    log: SessionLog {
                        environment: Default::default(),
                        agent_type: AgentKind::Cursor,
                        source,
                        updated_at: Some(mtime_epoch),
                    },
                });
            }
        }
    }

    annotate_cursor_agent_forks(&mut out);
    out
}

async fn collect_cursor_chat_metadata(home: &Path) -> HashMap<String, CursorChatMetadata> {
    let mut out = HashMap::new();
    let mut stack = vec![home.join(".cursor").join("chats")];
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) != Some("meta.json") {
                continue;
            }
            let Some(session_id) = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
            else {
                continue;
            };
            let Ok(content) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&content) else {
                continue;
            };
            out.insert(
                session_id.to_string(),
                CursorChatMetadata {
                    title: value
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                        .map(str::to_string),
                    created_at: value.get("createdAtMs").and_then(Value::as_i64),
                    updated_at: value.get("updatedAtMs").and_then(Value::as_i64),
                },
            );
        }
    }
    out
}

async fn collect_workspace_paths(home: &Path) -> HashMap<String, String> {
    let ws_root = app_config_dir_in("Cursor", home)
        .join("User")
        .join("workspaceStorage");
    let mut out = HashMap::new();
    let mut entries = match tokio::fs::read_dir(&ws_root).await {
        Ok(entries) => entries,
        Err(_) => return out,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let workspace_dir = entry.path();
        if !entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let workspace_json = workspace_dir.join("workspace.json");
        let Ok(content) = tokio::fs::read_to_string(&workspace_json).await else {
            continue;
        };
        let Some(path) = parse_workspace_json_path(&content) else {
            continue;
        };
        let encoded_key = encode_cursor_project_workspace_key(Path::new(&path));
        out.entry(encoded_key).or_insert(path);
    }

    out
}

async fn discover_store_db_sessions(
    home: &Path,
    now: i64,
    since_secs: i64,
) -> Vec<CursorDiscoveredSession> {
    let chats_root = home.join(".cursor").join("chats");
    if !tokio::fs::try_exists(&chats_root).await.unwrap_or(false) {
        return Vec::new();
    }

    let cutoff = now - since_secs;
    let chats_root_clone = chats_root.clone();
    let mut candidates = tokio::task::spawn_blocking(move || {
        query_cursor_store_db_sessions(&chats_root_clone, cutoff)
    })
    .await
    .unwrap_or_default();

    // store.db's `meta` blob doesn't carry the workspace path. When the same
    // session also has an agent-transcript on disk (same UUID), backfill cwd
    // from the agent-transcript project dir so dedup against AgentTranscript
    // collapses to one row. Also rewrites the inline synthetic metadata line
    // so the scanner sees `cwd` / `workspacePath` populated.
    let projects_dir = home.join(".cursor").join("projects");
    let workspace_map = collect_workspace_paths(home).await;
    let mut workspace_decode_cache: HashMap<String, Option<String>> = HashMap::new();
    for candidate in candidates.iter_mut() {
        if candidate.workspace_hint.is_some() {
            continue;
        }
        let Some(session_id) = candidate.canonical_id.as_deref() else {
            continue;
        };
        let Some(workspace_key) =
            find_agent_transcript_workspace_key(&projects_dir, session_id).await
        else {
            continue;
        };
        let resolved = if let Some(path) = workspace_map.get(&workspace_key).cloned() {
            Some(path)
        } else if let Some(cached) = workspace_decode_cache.get(&workspace_key).cloned() {
            cached
        } else {
            let decoded = decode_cursor_workspace_path(&workspace_key, home).await;
            workspace_decode_cache.insert(workspace_key.clone(), decoded.clone());
            decoded
        };
        let Some(cwd) = resolved else { continue };
        candidate.workspace_hint = Some(cwd.clone());
        if let SessionSource::Inline { content, .. } = &mut candidate.log.source {
            *content = patch_session_cwd_inline(content, &cwd);
        }
    }
    candidates
}

/// Rewrite the first line of an inline synthetic session payload so its
/// `cwd` / `workspacePath` reflect a backfilled value. The scanner picks
/// the cwd off this line; nothing in the conversation bubbles touches it.
///
/// Splits the original content at the first newline (O(1) string slice)
/// rather than collecting/joining every line — important because the
/// `content` for a long Cursor session can be multiple MB and only the
/// first line needs editing.
fn patch_session_cwd_inline(content: &str, cwd: &str) -> String {
    let (first_line, tail) = match content.find('\n') {
        Some(idx) => (&content[..idx], &content[idx..]),
        None => (content, ""),
    };
    let Ok(mut header) = serde_json::from_str::<Value>(first_line) else {
        return content.to_string();
    };
    let Some(object) = header.as_object_mut() else {
        return content.to_string();
    };
    object.insert("cwd".into(), Value::String(cwd.to_string()));
    object.insert("workspacePath".into(), Value::String(cwd.to_string()));
    let rebuilt = header.to_string();
    let mut out = String::with_capacity(rebuilt.len() + tail.len());
    out.push_str(&rebuilt);
    out.push_str(tail);
    out
}

/// Walk `~/.cursor/projects/*/agent-transcripts/` looking for a session
/// folder named `session_id`. Returns the parent project-dir name (the
/// workspace key) so the caller can decode it to a workspace path.
///
/// `session_id` is validated against a positive allowlist (alphanumeric +
/// hyphen + underscore) before any `Path::join` — the value is sourced from
/// `store.db#meta.value.agentId`, which is on-disk and could in principle be
/// crafted to escape via `..`. Mirrors the same guard Claude uses on
/// transcript ids ([`resolve_cli_transcript_path`]).
async fn find_agent_transcript_workspace_key(
    projects_dir: &Path,
    session_id: &str,
) -> Option<String> {
    if !is_safe_session_id(session_id) {
        return None;
    }
    let mut entries = tokio::fs::read_dir(projects_dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let project_path = entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let candidate = project_path.join("agent-transcripts").join(session_id);
        if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            return project_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned);
        }
    }
    None
}

/// Positive allowlist for an on-disk-sourced session id used in a
/// `Path::join`. Accepts ASCII alphanumerics, hyphen, and underscore —
/// covers every UUID Cursor and Claude write while rejecting `..`, path
/// separators, null bytes, and anything else that could escape a parent.
fn is_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn discover_desktop_sessions(
    home: &Path,
    now: i64,
    since_secs: i64,
    duplicate_fork_detector: Option<DuplicateForkDetector>,
) -> Vec<CursorDiscoveredSession> {
    let ws_root = app_config_dir_in("Cursor", home)
        .join("User")
        .join("workspaceStorage");
    let global_db = app_config_dir_in("Cursor", home)
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");

    if !global_db.exists() || !ws_root.exists() {
        return Vec::new();
    }

    let ws_root = ws_root.clone();
    let global_db = global_db.clone();
    tokio::task::spawn_blocking(move || {
        query_cursor_desktop_sessions(
            &ws_root,
            &global_db,
            now,
            since_secs,
            duplicate_fork_detector,
        )
    })
    .await
    .unwrap_or_default()
}

fn query_cursor_desktop_sessions(
    ws_root: &Path,
    global_db: &Path,
    now: i64,
    since_secs: i64,
    duplicate_fork_detector: Option<DuplicateForkDetector>,
) -> Vec<CursorDiscoveredSession> {
    let cutoff_ms = (now - since_secs) * 1000;
    let global = match open_sqlite_readonly(global_db) {
        Ok(conn) => conn,
        Err(_) => return Vec::new(),
    };
    let mut logs = Vec::new();
    let workspace_dirs = match std::fs::read_dir(ws_root) {
        Ok(entries) => entries,
        Err(_) => return logs,
    };

    for workspace_entry in workspace_dirs.flatten() {
        let workspace_dir = workspace_entry.path();
        if !workspace_dir.is_dir() {
            continue;
        }

        let workspace_db = workspace_dir.join("state.vscdb");
        let workspace_json = workspace_dir.join("workspace.json");
        if !workspace_db.exists() || !workspace_json.exists() {
            continue;
        }

        let cwd = std::fs::read_to_string(&workspace_json)
            .ok()
            .and_then(|content| parse_workspace_json_path(&content));

        let workspace = match open_sqlite_readonly(&workspace_db) {
            Ok(conn) => conn,
            Err(_) => continue,
        };
        let Some(composer_data_raw) = sqlite_query_string(
            &workspace,
            "SELECT value FROM ItemTable WHERE key = 'composer.composerData'",
            params![],
        ) else {
            continue;
        };
        let composers = parse_workspace_composers(&composer_data_raw);
        for mut composer in composers.iter().cloned() {
            // Cursor's migrated workspace blob (`hasMigratedComposerData: true`)
            // no longer carries per-composer timestamps — only id lists.
            // Pull the authoritative `lastUpdatedAt` from the global
            // `cursorDiskKV` `composerData:<id>` row in that case.
            if composer.last_updated_at == 0
                && let Some(ts) = fetch_composer_last_updated_at_ms(&global, &composer.composer_id)
            {
                composer.last_updated_at = ts;
            }
            if composer.last_updated_at < cutoff_ms {
                continue;
            }
            let fork_observation = duplicate_fork_detector.and_then(|detect| {
                cursor_duplicate_observation(detect, &global, &composers, &composer)
            });
            let Some(content) = build_desktop_cursor_session_content(
                &global,
                &composer.composer_id,
                cwd.as_deref(),
                fork_observation.as_ref(),
            ) else {
                continue;
            };
            logs.push(CursorDiscoveredSession {
                canonical_id: Some(composer.composer_id.clone()),
                workspace_hint: cwd.clone(),
                content_len: content.len(),
                priority: CursorSourcePriority::DesktopComposer,
                log: SessionLog {
                    environment: Default::default(),
                    agent_type: AgentKind::Cursor,
                    source: SessionSource::Inline {
                        label: format!(
                            "cursor-desktop:{}:{}",
                            workspace_dir
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("workspace"),
                            composer.composer_id
                        ),
                        content,
                    },
                    updated_at: Some(composer.last_updated_at / 1000),
                },
            });
        }
    }

    logs
}

/// Point query into the global `cursorDiskKV` table for a composer's
/// `lastUpdatedAt` (or `createdAt`) in epoch milliseconds. Used to recover
/// timestamps when the migrated workspace blob omits them.
fn fetch_composer_last_updated_at_ms(global: &Connection, composer_id: &str) -> Option<i64> {
    let raw = sqlite_query_string(
        global,
        "SELECT value FROM cursorDiskKV WHERE key = ?1",
        params![format!("composerData:{composer_id}")],
    )?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    value
        .get("lastUpdatedAt")
        .and_then(Value::as_i64)
        .or_else(|| value.get("createdAt").and_then(Value::as_i64))
}

fn open_sqlite_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let _ = conn.busy_timeout(Duration::from_millis(200));
    Ok(conn)
}

fn sqlite_query_string<P>(conn: &Connection, sql: &str, params: P) -> Option<String>
where
    P: rusqlite::Params,
{
    conn.query_row(sql, params, |row| row.get::<_, String>(0))
        .ok()
}

fn parse_workspace_json_path(content: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(content).ok()?;
    value
        .get("folder")
        .or_else(|| value.get("path"))
        .and_then(Value::as_str)
        .map(normalize_cursor_workspace_path)
}

fn encode_cursor_project_workspace_key(path: &Path) -> String {
    use std::path::Component;

    let mut encoded = String::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                let raw = prefix.as_os_str().to_string_lossy();
                if let Some(drive) = raw.strip_suffix(':') {
                    encoded.push_str(drive);
                    encoded.push_str("--");
                } else if !raw.is_empty() {
                    if !encoded.is_empty() && !encoded.ends_with('-') {
                        encoded.push('-');
                    }
                    encoded.push_str(&raw.replace(['/', '\\', ':'], "-"));
                }
            }
            Component::RootDir => {}
            Component::Normal(segment) => {
                let segment = segment.to_string_lossy();
                if segment.is_empty() {
                    continue;
                }
                if !encoded.is_empty() && !encoded.ends_with('-') {
                    encoded.push('-');
                }
                encoded.push_str(&segment);
            }
            Component::CurDir | Component::ParentDir => {}
        }
    }
    encoded
}

fn normalize_cursor_workspace_path(path: &str) -> String {
    let trimmed = path.strip_prefix("file://").unwrap_or(path);
    if cfg!(target_os = "windows") && trimmed.starts_with('/') && trimmed.len() > 3 {
        trimmed[1..].to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone)]
struct WorkspaceComposerHead {
    composer_id: String,
    last_updated_at: i64,
    is_subagent: Option<bool>,
}

fn parse_workspace_composers(raw: &str) -> Vec<WorkspaceComposerHead> {
    let value = match serde_json::from_str::<Value>(raw) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };

    // Legacy shape: `{"allComposers":[{composerId, lastUpdatedAt, createdAt, ...}]}`.
    // Carries the timestamp inline.
    let legacy: Vec<WorkspaceComposerHead> = value
        .get("allComposers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(WorkspaceComposerHead {
                composer_id: entry.get("composerId")?.as_str()?.to_string(),
                last_updated_at: entry
                    .get("lastUpdatedAt")
                    .and_then(Value::as_i64)
                    .or_else(|| entry.get("createdAt").and_then(Value::as_i64))?,
                is_subagent: entry.get("isSubagent").and_then(|value| {
                    value
                        .as_bool()
                        .or_else(|| value.as_i64().map(|value| value != 0))
                }),
            })
        })
        .collect();
    if !legacy.is_empty() {
        return legacy;
    }

    // Migrated shape (`hasMigratedComposerData: true`): only id lists in the
    // workspace blob — `lastUpdatedAt` lives on each global
    // `cursorDiskKV.composerData:<id>` row. Emit ids with sentinel `0` and
    // let the caller fill timestamps from global before applying cutoff.
    let mut ids: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for key in ["selectedComposerIds", "lastFocusedComposerIds"] {
        let Some(arr) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for entry in arr {
            let Some(id) = entry.as_str() else { continue };
            if seen.insert(id.to_string()) {
                ids.push(id.to_string());
            }
        }
    }
    ids.into_iter()
        .map(|composer_id| WorkspaceComposerHead {
            composer_id,
            last_updated_at: 0,
            is_subagent: None,
        })
        .collect()
}

fn cursor_duplicate_observation(
    detect: DuplicateForkDetector,
    global: &Connection,
    composers: &[WorkspaceComposerHead],
    child: &WorkspaceComposerHead,
) -> Option<ForkObservation> {
    if child.is_subagent != Some(false) {
        return None;
    }
    let child_data = sqlite_query_string(
        global,
        "SELECT value FROM cursorDiskKV WHERE key = ?1",
        params![format!("composerData:{}", child.composer_id)],
    )?;
    let child_store = json!({
        "composer_header": {
            "composerId": child.composer_id,
            "isSubagent": false,
        },
        "composer_data": serde_json::from_str::<Value>(&child_data).ok()?,
    })
    .to_string();
    composers
        .iter()
        .filter(|parent| {
            parent.composer_id != child.composer_id
                && parent.is_subagent == Some(false)
                && parent.last_updated_at <= child.last_updated_at
        })
        .filter_map(|parent| {
            let parent_data = sqlite_query_string(
                global,
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                params![format!("composerData:{}", parent.composer_id)],
            )?;
            let parent_store = json!({
                "composer_header": {
                    "composerId": parent.composer_id,
                    "isSubagent": false,
                },
                "composer_data": serde_json::from_str::<Value>(&parent_data).ok()?,
            })
            .to_string();
            detect(&parent_store, &child_store)
        })
        .max_by_key(|observation| observation.inherited_item_count.unwrap_or_default())
}

fn build_desktop_cursor_session_content(
    global: &Connection,
    composer_id: &str,
    cwd: Option<&str>,
    fork_observation: Option<&ForkObservation>,
) -> Option<String> {
    let composer_raw = sqlite_query_string(
        global,
        "SELECT value FROM cursorDiskKV WHERE key = ?1",
        params![format!("composerData:{composer_id}")],
    )?;
    let composer = serde_json::from_str::<Value>(&composer_raw).ok()?;
    let model = composer
        .pointer("/modelConfig/modelName")
        .or_else(|| composer.pointer("/lastUsedModel/name"))
        .or_else(|| composer.pointer("/model/name"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());

    let mut header_ids = composer
        .get("fullConversationHeadersOnly")
        .and_then(Value::as_array)
        .map(|headers| {
            headers
                .iter()
                .filter_map(|header| header.get("bubbleId").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    header_ids.extend(extract_bubble_ids_from_value(
        composer.get("conversation").unwrap_or(&Value::Null),
    ));
    header_ids.extend(extract_bubble_ids_from_value(
        composer.get("conversationMap").unwrap_or(&Value::Null),
    ));
    dedupe_preserving_order(&mut header_ids);

    let mut bubbles = query_cursor_bubbles(global, composer_id);
    if header_ids.is_empty() {
        header_ids.extend(bubbles.keys().cloned());
        header_ids.sort();
    }

    // Cursor IDE stores the composer title (both AI-set and user-renamed)
    // in `composer.name` on the global `cursorDiskKV` row. Surface it on the
    // synthetic metadata line as `title` so the scanner's Tier 2
    // ExplicitField picks it up — same mechanism Codex uses for its
    // SQLite-resolved title, except Cursor's `name` is dual-purpose
    // (no schema-level distinction between AI and rename), so the
    // resulting provenance is `Explicit` rather than `UserRename`.
    let composer_name = composer
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut lines = Vec::new();
    lines.push(
        json!({
            "sessionId": composer_id,
            "session_id": composer_id,
            "cwd": cwd,
            "workspacePath": cwd,
            "model": model,
            "title": composer_name,
            "createdAt": composer.get("createdAt").and_then(Value::as_i64),
            "lastUpdatedAt": composer.get("lastUpdatedAt").and_then(Value::as_i64),
            "cursor_source": "desktop_state_vscdb",
            "workspaceId": composer.get("workspaceId").and_then(Value::as_str),
            FORK_OBSERVATION_KEY: fork_observation,
        })
        .to_string(),
    );

    for bubble_id in header_ids {
        let Some(bubble) = bubbles.remove(&bubble_id) else {
            continue;
        };
        let Some(role) = cursor_role_for_value(&bubble) else {
            continue;
        };
        let Some(content) = extract_cursor_bubble_text(&bubble) else {
            continue;
        };
        lines.push(
            json!({
                "role": role,
                "content": content,
                "timestamp": bubble.get("createdAt").and_then(Value::as_str),
                "model": bubble.pointer("/modelInfo/modelName").and_then(Value::as_str).or(model),
                "bubbleId": bubble_id,
            })
            .to_string(),
        );
    }

    if lines.len() == 1 {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn query_cursor_bubbles(global: &Connection, composer_id: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Ok(mut stmt) =
        global.prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE ?1 ORDER BY key")
    else {
        return out;
    };
    let pattern = format!("bubbleId:{composer_id}:%");
    let Ok(rows) = stmt.query_map(params![pattern], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return out;
    };

    for row in rows.flatten() {
        let Some(bubble_id) = row.0.rsplit(':').next() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&row.1) else {
            continue;
        };
        out.insert(bubble_id.to_string(), value);
    }
    out
}

fn cursor_role_for_type(kind: Option<i64>) -> Option<&'static str> {
    match kind {
        Some(1) => Some("user"),
        Some(2) => Some("assistant"),
        _ => None,
    }
}

fn cursor_role_for_value(value: &Value) -> Option<&'static str> {
    cursor_role_for_type(value.get("type").and_then(Value::as_i64)).or_else(|| {
        value
            .get("role")
            .and_then(Value::as_str)
            .and_then(|role| match role {
                "user" => Some("user"),
                "assistant" => Some("assistant"),
                _ => None,
            })
    })
}

fn extract_cursor_bubble_text(bubble: &Value) -> Option<String> {
    for key in ["text", "markdown", "content", "description"] {
        if let Some(text) = bubble.get(key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    for pointer in [
        "/toolFormerData/rawArgs",
        "/toolFormerData/result",
        "/toolFormerData/output",
        "/toolResult/output",
        "/errorDetails/message",
    ] {
        if let Some(text) = bubble.pointer(pointer).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    if let Some(rich_text) = bubble.get("richText").and_then(Value::as_str)
        && let Ok(parsed) = serde_json::from_str::<Value>(rich_text)
    {
        let mut fragments = Vec::new();
        collect_rich_text_fragments(&parsed, &mut fragments);
        let joined = fragments.join(" ").trim().to_string();
        if !joined.is_empty() {
            return Some(joined);
        }
    }

    None
}

fn extract_bubble_ids_from_value(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_bubble_ids(value, &mut out);
    out
}

fn dedupe_preserving_order(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn collect_bubble_ids(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for key in ["bubbleId", "id"] {
                if let Some(id) = map.get(key).and_then(Value::as_str)
                    && !id.trim().is_empty()
                {
                    out.push(id.to_string());
                }
            }
            for child in map.values() {
                collect_bubble_ids(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_bubble_ids(item, out);
            }
        }
        _ => {}
    }
}

fn collect_rich_text_fragments(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            if let Some(children) = map.get("children").and_then(Value::as_array) {
                for child in children {
                    collect_rich_text_fragments(child, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_rich_text_fragments(item, out);
            }
        }
        _ => {}
    }
}

fn prepend_cursor_metadata_line(
    content: &str,
    session_id: &str,
    metadata: CursorSyntheticMetadata<'_>,
) -> String {
    let metadata = json!({
        "sessionId": session_id,
        "session_id": session_id,
        "cwd": metadata.cwd,
        "workspacePath": metadata.cwd,
        "model": metadata.model,
        "title": metadata.title,
        "createdAt": metadata.created_at,
        "lastUpdatedAt": metadata.updated_at,
        FORK_OBSERVATION_KEY: metadata.fork_observation,
        "cursor_source": metadata.source_kind,
    })
    .to_string();
    if content.is_empty() {
        metadata
    } else {
        format!("{metadata}\n{content}")
    }
}

fn annotate_cursor_agent_forks(candidates: &mut [CursorDiscoveredSession]) {
    let snapshots = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            if candidate.priority != CursorSourcePriority::AgentTranscript {
                return None;
            }
            let SessionSource::Inline { content, .. } = &candidate.log.source else {
                return None;
            };
            let metadata = content
                .lines()
                .next()
                .and_then(|line| serde_json::from_str::<Value>(line).ok())?;
            Some((
                index,
                metadata
                    .get("sessionId")
                    .and_then(Value::as_str)?
                    .to_string(),
                metadata
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                metadata
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                metadata.get("createdAt").and_then(Value::as_i64),
                cursor_agent_visible_items(content),
            ))
        })
        .collect::<Vec<_>>();

    for (child_index, child_id, child_cwd, child_title, child_created, child_items) in &snapshots {
        let Some(parent_title) = child_title.as_deref().and_then(cursor_fork_parent_title) else {
            continue;
        };
        let mut matches = snapshots
            .iter()
            .filter(
                |(parent_index, parent_id, parent_cwd, title, parent_created, parent_items)| {
                    parent_index != child_index
                        && parent_id != child_id
                        && parent_cwd == child_cwd
                        && title.as_deref() == Some(parent_title)
                        && parent_created
                            .zip(*child_created)
                            .is_none_or(|(parent, child)| parent <= child)
                        && strict_visible_prefix(parent_items, child_items)
                },
            )
            .collect::<Vec<_>>();
        matches.sort_by_key(|(_, _, _, _, _, items)| std::cmp::Reverse(items.len()));
        let Some(parent) = matches.first() else {
            continue;
        };
        if matches
            .get(1)
            .is_some_and(|other| other.5.len() == parent.5.len())
        {
            continue;
        }
        let observation = ForkObservation {
            parent_agent: "cursor".to_string(),
            parent_agent_session_id: parent.1.clone(),
            fork_kind: "fork".to_string(),
            provider_fork_point_id: None,
            detection_source: "normalized_content".to_string(),
            confidence: 85,
            inherited_item_count: parent.5.len().try_into().ok(),
            extractor_version: "cursor-agent-transcript-v1".to_string(),
        };
        if let SessionSource::Inline { content, .. } = &mut candidates[*child_index].log.source {
            set_embedded_fork_observation(content, &observation);
        }
    }
}

fn cursor_fork_parent_title(title: &str) -> Option<&str> {
    title
        .trim()
        .strip_suffix(" (forked)")
        .filter(|title| !title.is_empty())
}

fn cursor_agent_visible_items(content: &str) -> Vec<(String, String)> {
    content
        .lines()
        .skip(1)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            let role = value.get("role")?.as_str()?;
            if !matches!(role, "user" | "assistant") {
                return None;
            }
            let text = value
                .pointer("/message/content")?
                .as_array()?
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            Some((role.to_string(), normalize_visible_text(&text)))
        })
        .filter(|(_, text)| !text.is_empty())
        .collect()
}

fn normalize_visible_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strict_visible_prefix(parent: &[(String, String)], child: &[(String, String)]) -> bool {
    parent.len() >= 2 && parent.len() < child.len() && child.starts_with(parent)
}

fn set_embedded_fork_observation(content: &mut String, observation: &ForkObservation) {
    let Some((first, rest)) = content.split_once('\n') else {
        return;
    };
    let Ok(mut metadata) = serde_json::from_str::<Value>(first) else {
        return;
    };
    metadata[FORK_OBSERVATION_KEY] = serde_json::to_value(observation).unwrap_or(Value::Null);
    *content = format!("{metadata}\n{rest}");
}

async fn decode_cursor_workspace_path(encoded: &str, home_boundary: &Path) -> Option<String> {
    let decoded = decode_cursor_workspace_path_buf(encoded, home_boundary).await?;
    Some(decoded.to_string_lossy().to_string())
}

fn merge_cursor_session(
    discovered: &mut HashMap<String, CursorDiscoveredSession>,
    incoming: CursorDiscoveredSession,
) {
    let key = cursor_discovered_key(&incoming);
    let Some(existing) = discovered.remove(&key) else {
        discovered.insert(key, incoming);
        return;
    };
    let (mut selected, other) = if should_replace_cursor_candidate(&existing, &incoming) {
        (incoming, existing)
    } else {
        (existing, incoming)
    };
    merge_cursor_candidate_metadata(&mut selected, &other);
    discovered.insert(key, selected);
}

fn merge_cursor_candidate_metadata(
    selected: &mut CursorDiscoveredSession,
    other: &CursorDiscoveredSession,
) {
    let (
        SessionSource::Inline {
            content: selected_content,
            ..
        },
        SessionSource::Inline {
            content: other_content,
            ..
        },
    ) = (&mut selected.log.source, &other.log.source)
    else {
        return;
    };
    let Some((selected_first, selected_rest)) = selected_content.split_once('\n') else {
        return;
    };
    let Some(other_first) = other_content.lines().next() else {
        return;
    };
    let (Ok(mut selected_metadata), Ok(other_metadata)) = (
        serde_json::from_str::<Value>(selected_first),
        serde_json::from_str::<Value>(other_first),
    ) else {
        return;
    };
    for key in [
        "title",
        "model",
        "createdAt",
        "lastUpdatedAt",
        FORK_OBSERVATION_KEY,
    ] {
        if selected_metadata.get(key).is_none_or(Value::is_null)
            && let Some(value) = other_metadata.get(key)
            && !value.is_null()
        {
            selected_metadata[key] = value.clone();
        }
    }
    *selected_content = format!("{selected_metadata}\n{selected_rest}");
}

fn cursor_discovered_key(candidate: &CursorDiscoveredSession) -> String {
    match (&candidate.canonical_id, &candidate.workspace_hint) {
        (Some(session_id), Some(cwd)) => format!("{session_id}::{cwd}"),
        (Some(session_id), None) => format!("{session_id}::"),
        (None, Some(cwd)) => format!("::<{cwd}>"),
        (None, None) => candidate.log.source_label(),
    }
}

fn should_replace_cursor_candidate(
    existing: &CursorDiscoveredSession,
    incoming: &CursorDiscoveredSession,
) -> bool {
    if incoming.priority != existing.priority {
        return incoming.priority > existing.priority;
    }
    if incoming.content_len != existing.content_len {
        return incoming.content_len > existing.content_len;
    }
    incoming.log.updated_at.unwrap_or_default() > existing.log.updated_at.unwrap_or_default()
}

fn query_cursor_store_db_sessions(chats_root: &Path, cutoff: i64) -> Vec<CursorDiscoveredSession> {
    let mut snapshots = Vec::new();
    let mut stack = vec![chats_root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) != Some("store.db") {
                continue;
            }
            let Some(snapshot) = read_cursor_store_db_snapshot(&path) else {
                continue;
            };
            snapshots.push(snapshot);
        }
    }

    let observations = snapshots
        .iter()
        .map(|child| cursor_store_db_fork_observation(&snapshots, child))
        .collect::<Vec<_>>();
    snapshots
        .into_iter()
        .zip(observations)
        .filter_map(|(snapshot, observation)| {
            let candidate = snapshot.into_candidate(observation.as_ref())?;
            (candidate.log.updated_at.unwrap_or_default() >= cutoff).then_some(candidate)
        })
        .collect()
}

#[cfg(test)]
fn build_cursor_store_db_session(path: &Path) -> Option<CursorDiscoveredSession> {
    read_cursor_store_db_snapshot(path)?.into_candidate(None)
}

#[derive(Debug, Clone)]
struct CursorStoreDbSnapshot {
    path: PathBuf,
    workspace_dir: PathBuf,
    metadata: CursorStoreDbMetadata,
    records: Vec<(String, String)>,
    blob_ids: HashSet<String>,
    file_mtime: i64,
}

impl CursorStoreDbSnapshot {
    fn into_candidate(
        self,
        fork_observation: Option<&ForkObservation>,
    ) -> Option<CursorDiscoveredSession> {
        let content =
            build_cursor_store_db_content(&self.metadata, &self.records, fork_observation)?;
        let metadata_updated_at = self
            .metadata
            .updated_at
            .or(self.metadata.created_at)
            .map(normalize_epoch_guess)
            .unwrap_or_default();
        let updated_at = self.file_mtime.max(metadata_updated_at);
        Some(CursorDiscoveredSession {
            canonical_id: Some(self.metadata.session_id.clone()),
            workspace_hint: self.metadata.cwd.clone(),
            content_len: content.len(),
            priority: CursorSourcePriority::StoreDb,
            log: SessionLog {
                environment: Default::default(),
                agent_type: AgentKind::Cursor,
                source: SessionSource::Inline {
                    label: format!("cursor-store:{}", self.path.display()),
                    content,
                },
                updated_at: Some(updated_at),
            },
        })
    }
}

fn read_cursor_store_db_snapshot(path: &Path) -> Option<CursorStoreDbSnapshot> {
    let conn = open_sqlite_readonly(path).ok()?;
    let mut records = Vec::new();
    let mut meta_values = Vec::new();
    for table in ["meta", "blobs"] {
        if !sqlite_table_exists(&conn, table) {
            continue;
        }
        let rows = sqlite_table_string_rows(&conn, table);
        if table == "meta" {
            meta_values.extend(rows.iter().map(|(_, value)| value.clone()));
        }
        records.extend(rows);
    }
    if records.is_empty() {
        return None;
    }

    let metadata = parse_cursor_store_db_metadata(&meta_values)?;
    let file_mtime = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let workspace_dir = path.parent()?.parent()?.to_path_buf();
    Some(CursorStoreDbSnapshot {
        path: path.to_path_buf(),
        workspace_dir,
        metadata,
        records,
        blob_ids: cursor_store_blob_ids(&conn),
        file_mtime,
    })
}

fn cursor_store_blob_ids(conn: &Connection) -> HashSet<String> {
    for column in ["id", "key"] {
        let sql = format!("SELECT {column} FROM blobs");
        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
            continue;
        };
        return rows
            .flatten()
            .filter(|id| id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .collect();
    }
    HashSet::new()
}

fn cursor_store_db_fork_observation(
    snapshots: &[CursorStoreDbSnapshot],
    child: &CursorStoreDbSnapshot,
) -> Option<ForkObservation> {
    let child_created_at = child.metadata.created_at?;
    if child.blob_ids.is_empty() {
        return None;
    }
    let eligible = snapshots
        .iter()
        .filter(|parent| {
            parent.workspace_dir == child.workspace_dir
                && parent.metadata.session_id != child.metadata.session_id
                && parent
                    .metadata
                    .created_at
                    .is_some_and(|created_at| created_at <= child_created_at)
                && !parent.blob_ids.is_empty()
                && parent.blob_ids.len() < child.blob_ids.len()
                && parent.blob_ids.is_subset(&child.blob_ids)
        })
        .collect::<Vec<_>>();
    let maximal = eligible
        .iter()
        .copied()
        .filter(|parent| {
            !eligible.iter().copied().any(|other| {
                parent.metadata.session_id != other.metadata.session_id
                    && parent.blob_ids.len() < other.blob_ids.len()
                    && parent.blob_ids.is_subset(&other.blob_ids)
            })
        })
        .collect::<Vec<_>>();
    let [parent] = maximal.as_slice() else {
        return None;
    };
    Some(ForkObservation {
        parent_agent: "cursor".to_string(),
        parent_agent_session_id: parent.metadata.session_id.clone(),
        fork_kind: "fork".to_string(),
        provider_fork_point_id: None,
        detection_source: "stable_id_prefix".to_string(),
        confidence: 100,
        inherited_item_count: parent.blob_ids.len().try_into().ok(),
        extractor_version: "cursor-store-db-v1".to_string(),
    })
}

#[derive(Debug, Clone)]
struct CursorStoreDbMetadata {
    session_id: String,
    cwd: Option<String>,
    model: Option<String>,
    /// Title written by `cursor-agent`. Both `/rename` and the CLI's
    /// AI-generated initial label land in the `meta.value.name` field
    /// of `~/.cursor/chats/<ws>/<agent>/store.db`. Surfaced on the
    /// synthetic metadata line so the scanner picks it up at Tier 2.
    title: Option<String>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
}

fn parse_cursor_store_db_metadata(values: &[String]) -> Option<CursorStoreDbMetadata> {
    let mut session_id = None;
    let mut cwd = None;
    let mut model = None;
    let mut title = None;
    let mut created_at = None;
    let mut updated_at = None;

    for value in values {
        let Ok(json) = serde_json::from_str::<Value>(value) else {
            continue;
        };
        session_id = session_id
            .or_else(|| find_string_in_value(&json, &["agentId", "sessionId", "composerId", "id"]));
        cwd = cwd.or_else(|| {
            find_string_in_value(
                &json,
                &[
                    "workspacePath",
                    "cwd",
                    "workingDirectory",
                    "working_directory",
                    "path",
                ],
            )
        });
        model =
            model.or_else(|| find_string_in_value(&json, &["lastUsedModel", "modelName", "model"]));
        title = title.or_else(|| find_string_in_value(&json, &["name", "title", "customTitle"]));
        created_at = created_at.or_else(|| find_i64_in_value(&json, &["createdAt", "created_at"]));
        updated_at = updated_at
            .or_else(|| find_i64_in_value(&json, &["lastUpdatedAt", "updatedAt", "updated_at"]));
    }

    Some(CursorStoreDbMetadata {
        session_id: session_id?,
        cwd,
        model,
        title,
        created_at,
        updated_at,
    })
}

fn build_cursor_store_db_content(
    metadata: &CursorStoreDbMetadata,
    records: &[(String, String)],
    fork_observation: Option<&ForkObservation>,
) -> Option<String> {
    let mut lines = vec![
        json!({
            "sessionId": metadata.session_id,
            "session_id": metadata.session_id,
            "cwd": metadata.cwd,
            "workspacePath": metadata.cwd,
            "model": metadata.model,
            "title": metadata.title,
            "createdAt": metadata.created_at,
            "lastUpdatedAt": metadata.updated_at,
            "cursor_source": "store_db",
            FORK_OBSERVATION_KEY: fork_observation,
        })
        .to_string(),
    ];
    let mut seen = HashSet::new();
    for (_, value) in records {
        let Ok(json) = serde_json::from_str::<Value>(value) else {
            continue;
        };
        for message in extract_cursor_message_candidates(&json) {
            let fingerprint = format!("{}:{}", message.role, message.content);
            if !seen.insert(fingerprint) {
                continue;
            }
            lines.push(
                json!({
                    "role": message.role,
                    "content": message.content,
                    "timestamp": message.timestamp,
                    "model": message.model,
                })
                .to_string(),
            );
        }
    }
    if lines.len() <= 1 {
        None
    } else {
        Some(lines.join("\n"))
    }
}

#[derive(Debug, Clone)]
struct CursorMessageCandidate {
    role: String,
    content: String,
    timestamp: Option<i64>,
    model: Option<String>,
}

fn extract_cursor_message_candidates(value: &Value) -> Vec<CursorMessageCandidate> {
    let mut out = Vec::new();
    collect_cursor_message_candidates(value, &mut out);
    out
}

fn collect_cursor_message_candidates(value: &Value, out: &mut Vec<CursorMessageCandidate>) {
    match value {
        Value::Object(map) => {
            let role = map
                .get("role")
                .and_then(Value::as_str)
                .or_else(|| map.get("type").and_then(Value::as_str));
            let content = map
                .get("content")
                .and_then(Value::as_str)
                .or_else(|| map.get("text").and_then(Value::as_str))
                .or_else(|| map.get("markdown").and_then(Value::as_str));
            if let (Some(role), Some(content)) = (role, content)
                && matches!(role, "user" | "assistant")
                && !content.trim().is_empty()
            {
                out.push(CursorMessageCandidate {
                    role: role.to_string(),
                    content: content.trim().to_string(),
                    timestamp: map
                        .get("createdAt")
                        .and_then(Value::as_i64)
                        .or_else(|| map.get("timestamp").and_then(Value::as_i64)),
                    model: map
                        .get("model")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .or_else(|| {
                            value
                                .pointer("/modelInfo/modelName")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned)
                        }),
                });
            }
            for child in map.values() {
                collect_cursor_message_candidates(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_cursor_message_candidates(item, out);
            }
        }
        _ => {}
    }
}

fn find_string_in_value(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(string) = map.get(*key).and_then(Value::as_str)
                    && !string.trim().is_empty()
                {
                    return Some(string.to_string());
                }
            }
            for child in map.values() {
                if let Some(found) = find_string_in_value(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_string_in_value(item, keys)),
        _ => None,
    }
}

fn find_i64_in_value(value: &Value, keys: &[&str]) -> Option<i64> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(number) = map.get(*key).and_then(Value::as_i64) {
                    return Some(number);
                }
            }
            for child in map.values() {
                if let Some(found) = find_i64_in_value(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_i64_in_value(item, keys)),
        _ => None,
    }
}

fn normalize_epoch_guess(value: i64) -> i64 {
    if value > 100_000_000_000 {
        value / 1000
    } else {
        value
    }
}

fn sqlite_table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        params![table],
        |_| Ok(()),
    )
    .is_ok()
}

fn sqlite_table_string_rows(conn: &Connection, table: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let direct_sql = format!("SELECT key, CAST(value AS TEXT) FROM {table}");
    if let Ok(mut stmt) = conn.prepare(&direct_sql)
        && let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
    {
        out.extend(rows.flatten().filter_map(|(key, value)| {
            // Cursor `~/.cursor/chats/*/store.db#meta.value` is a TEXT column
            // whose content is hex-encoded JSON (literal ASCII hex like
            // `7b22...` rather than `{"...`). When the raw text doesn't look
            // JSON-ish, try a hex-decode pass before giving up.
            if looks_like_jsonish(&value) {
                Some((key, value))
            } else {
                decode_hex_jsonish(&value).map(|decoded| (key, decoded))
            }
        }));
        if !out.is_empty() {
            return out;
        }
    }

    let pragma = format!("PRAGMA table_info({table})");
    let Ok(mut info_stmt) = conn.prepare(&pragma) else {
        return out;
    };
    let Ok(columns) = info_stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return out;
    };
    let columns = columns.flatten().collect::<Vec<_>>();
    if columns.is_empty() {
        return out;
    }
    let sql = format!("SELECT * FROM {table}");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return out;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return out;
    };
    while let Ok(Some(row)) = rows.next() {
        let mut key = None;
        let mut value = None;
        for (idx, column) in columns.iter().enumerate() {
            if key.is_none()
                && (column == "key" || column == "id" || column.ends_with("Id"))
                && let Ok(text) = row.get::<_, String>(idx)
            {
                key = Some(text);
            }
            if value.is_none()
                && let Ok(text) = row.get::<_, String>(idx)
            {
                if looks_like_jsonish(&text) {
                    value = Some(text);
                } else if let Some(decoded) = decode_hex_jsonish(&text) {
                    value = Some(decoded);
                }
            }
            if value.is_none()
                && let Ok(bytes) = row.get::<_, Vec<u8>>(idx)
                && let Ok(text) = String::from_utf8(bytes)
            {
                if looks_like_jsonish(&text) {
                    value = Some(text);
                } else if let Some(decoded) = decode_hex_jsonish(&text) {
                    value = Some(decoded);
                }
            }
        }
        if let Some(value) = value {
            out.push((key.unwrap_or_else(|| table.to_string()), value));
        }
    }
    out
}

fn looks_like_jsonish(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

/// Cursor's `~/.cursor/chats/*/store.db#meta.value` is a TEXT column whose
/// stored content is hex-encoded JSON (literal ASCII hex characters
/// representing the raw bytes of a `{...}` document). Decode and return the
/// JSON text if `value` looks like hex-encoded JSON; otherwise `None`.
///
/// Cheap pre-checks bail before any allocation: even length, first two
/// chars decode to `{` or `[`, and the rest is all ASCII hex. Only after
/// every check passes do we allocate the output buffer.
fn decode_hex_jsonish(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.len() < 4 || !trimmed.len().is_multiple_of(2) {
        return None;
    }
    let bytes = trimmed.as_bytes();
    // Cheapest discriminating check first: the leading byte. If it doesn't
    // decode to `{` / `[`, this isn't a hex-encoded JSON document and we
    // bail before scanning the full string for hex-ness.
    let head = hex_byte(bytes[0], bytes[1])?;
    if head != b'{' && head != b'[' {
        return None;
    }
    // Now confirm the tail is all hex, then decode into a single
    // pre-sized buffer. `chunks_exact(2)` is bounds-check-free.
    if !bytes[2..].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    out.push(head);
    for pair in bytes[2..].chunks_exact(2) {
        out.push(hex_byte(pair[0], pair[1])?);
    }
    String::from_utf8(out).ok()
}

/// Decode a two-char ASCII hex pair (e.g. `b'7'`, `b'b'`) to one byte.
/// Returns `None` if either char isn't a valid hex digit.
fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    let h = (hi as char).to_digit(16)?;
    let l = (lo as char).to_digit(16)?;
    Some(((h << 4) | l) as u8)
}

#[cfg(test)]
mod tests {

    /// Read back the [`ForkObservation`] an adapter embedded in a synthetic
    /// metadata header. Mirrors what a consumer of the rendered content does.
    fn embedded_fork_observation(content: &str) -> Option<ForkObservation> {
        let first = content.lines().next()?;
        let value: Value = serde_json::from_str(first).ok()?;
        serde_json::from_value(value.get(FORK_OBSERVATION_KEY)?.clone()).ok()
    }

    /// Stand-in for the duplicate detector an embedding application supplies:
    /// accepts a parent whose conversation items are a structural prefix of the
    /// child's, ignoring per-item ids. That copied prefix is the only evidence
    /// Cursor's desktop composers leave behind.
    fn prefix_duplicate_detector(parent_store: &str, child_store: &str) -> Option<ForkObservation> {
        fn structural(mut value: Value) -> Value {
            if let Value::Object(object) = &mut value {
                object.retain(|key, _| key != "bubbleId");
            }
            value
        }
        fn parts(store: &str) -> Option<(String, Vec<Value>)> {
            let value: Value = serde_json::from_str(store).ok()?;
            let id = value
                .pointer("/composer_header/composerId")?
                .as_str()?
                .to_string();
            let items = value
                .pointer("/composer_data/fullConversationHeadersOnly")?
                .as_array()?
                .iter()
                .cloned()
                .map(structural)
                .collect();
            Some((id, items))
        }
        let (parent_id, parent_items) = parts(parent_store)?;
        let (child_id, child_items) = parts(child_store)?;
        if parent_id == child_id
            || parent_items.is_empty()
            || child_items.len() <= parent_items.len()
            || !child_items.starts_with(&parent_items)
        {
            return None;
        }
        Some(ForkObservation {
            parent_agent: "cursor".to_string(),
            parent_agent_session_id: parent_id,
            fork_kind: "duplicate".to_string(),
            provider_fork_point_id: None,
            detection_source: "stable_id_prefix".to_string(),
            confidence: 100,
            inherited_item_count: parent_items.len().try_into().ok(),
            extractor_version: "test".to_string(),
        })
    }
    use super::app_config_dir_in;
    use super::*;
    use crate::discovery::scanner;
    use tempfile::TempDir;

    fn create_cursor_db(path: &Path) -> Connection {
        let conn = Connection::open(path).expect("test Cursor database should open");
        conn.execute_batch(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB);
             CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);",
        )
        .expect("test Cursor database schema should initialize");
        conn
    }

    fn create_cursor_store_db(path: &Path) -> Connection {
        let conn = Connection::open(path).expect("test Cursor store database should open");
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value BLOB);
             CREATE TABLE blobs (key TEXT PRIMARY KEY, value BLOB);",
        )
        .expect("test Cursor store database schema should initialize");
        conn
    }

    fn synthetic_cursor_blob_id(index: usize) -> String {
        format!("{index:064x}")
    }

    fn seed_cursor_store_db_with_blobs(
        chats_root: &Path,
        workspace: &str,
        session_id: &str,
        created_at: i64,
        blob_indexes: impl IntoIterator<Item = usize>,
    ) {
        let path = chats_root.join(workspace).join(session_id).join("store.db");
        std::fs::create_dir_all(
            path.parent()
                .expect("test Cursor store path should have a parent"),
        )
        .expect("test Cursor store directory should be created");
        let db = Connection::open(path).expect("test Cursor store should open");
        db.execute_batch(
            "CREATE TABLE meta (key TEXT, value TEXT);
             CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);",
        )
        .expect("test Cursor store schema should initialize");
        let blob_ids = blob_indexes
            .into_iter()
            .map(synthetic_cursor_blob_id)
            .collect::<Vec<_>>();
        let meta = json!({
            "agentId": session_id,
            "latestRootBlobId": blob_ids.last(),
            "name": format!("Synthetic {session_id}"),
            "createdAt": created_at,
            "mode": "agent",
            "lastUsedModel": "synthetic-model",
        })
        .to_string();
        let hex = meta
            .bytes()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params!["0", hex],
        )
        .expect("test Cursor store metadata should be inserted");
        for (index, blob_id) in blob_ids.iter().enumerate() {
            let data = if index == 0 {
                json!({ "role": "user", "content": "Synthetic prompt" }).to_string()
            } else {
                "{}".to_string()
            };
            db.execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                params![blob_id, data.as_bytes()],
            )
            .expect("test Cursor store blob should be inserted");
        }
    }

    fn cursor_store_observation(session: &CursorDiscoveredSession) -> Option<ForkObservation> {
        let SessionSource::Inline { content, .. } = &session.log.source else {
            panic!("expected inline Cursor store session")
        };
        serde_json::from_str::<Value>(
            content
                .lines()
                .next()
                .expect("inline Cursor store session should contain metadata"),
        )
        .expect("inline Cursor store metadata should be valid JSON")
        .get(FORK_OBSERVATION_KEY)
        .cloned()
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value(value).ok())
    }

    #[test]
    fn cursor_store_db_reader_detects_the_19_of_27_fork_boundary() {
        let fixture: Value =
            serde_json::from_str(include_str!("tests/fixtures/cursor-cli-store-fork.json"))
                .unwrap();
        let parent_blob_ids = fixture["parent"]["blob_ids"].as_array().unwrap();
        let child_blob_ids = fixture["child"]["blob_ids"].as_array().unwrap();
        assert_eq!(parent_blob_ids.len(), 19);
        assert_eq!(child_blob_ids.len(), 27);
        assert!(parent_blob_ids.iter().all(|id| child_blob_ids.contains(id)));

        let temp = TempDir::new().unwrap();
        let chats_root = temp.path().join("chats");
        seed_cursor_store_db_with_blobs(
            &chats_root,
            "workspace-a",
            "parent-session",
            1_000,
            0..parent_blob_ids.len(),
        );
        seed_cursor_store_db_with_blobs(
            &chats_root,
            "workspace-a",
            "child-session",
            2_000,
            0..child_blob_ids.len(),
        );

        let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
        let child = sessions
            .iter()
            .find(|session| session.canonical_id.as_deref() == Some("child-session"))
            .unwrap();
        let observation = cursor_store_observation(child).expect("strict blob subset lineage");
        assert_eq!(observation.parent_agent, "cursor");
        assert_eq!(observation.parent_agent_session_id, "parent-session");
        assert_eq!(observation.fork_kind, "fork");
        assert_eq!(observation.detection_source, "stable_id_prefix");
        assert_eq!(observation.inherited_item_count, Some(19));
        assert_eq!(observation.confidence, 100);
        assert_eq!(observation.extractor_version, "cursor-store-db-v1");
    }

    #[test]
    fn cursor_store_db_reader_rejects_independent_partial_overlap() {
        let temp = TempDir::new().unwrap();
        let chats_root = temp.path().join("chats");
        seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "independent-a", 1_000, 0..19);
        seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "independent-b", 2_000, 10..37);

        let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
        assert!(
            sessions
                .iter()
                .all(|session| cursor_store_observation(session).is_none())
        );
    }

    #[test]
    fn cursor_store_db_reader_rejects_ambiguous_maximal_parents() {
        let temp = TempDir::new().unwrap();
        let chats_root = temp.path().join("chats");
        seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "parent-a", 1_000, 0..19);
        seed_cursor_store_db_with_blobs(
            &chats_root,
            "workspace-a",
            "parent-b",
            1_000,
            (0..18).chain(std::iter::once(19)),
        );
        seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "child-session", 2_000, 0..27);

        let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
        let child = sessions
            .iter()
            .find(|session| session.canonical_id.as_deref() == Some("child-session"))
            .unwrap();
        assert!(cursor_store_observation(child).is_none());
    }

    #[test]
    fn cursor_store_db_reader_never_links_same_session_id_to_itself() {
        let temp = TempDir::new().unwrap();
        let chats_root = temp.path().join("chats");
        seed_cursor_store_db_with_blobs(
            &chats_root,
            "workspace-a",
            "resumed-session",
            1_000,
            0..19,
        );
        seed_cursor_store_db_with_blobs(
            &chats_root,
            "workspace-a",
            "resumed-session-copy",
            2_000,
            0..27,
        );
        let copied_path = chats_root
            .join("workspace-a")
            .join("resumed-session-copy")
            .join("store.db");
        let db = Connection::open(copied_path).unwrap();
        let meta = json!({
            "agentId": "resumed-session",
            "createdAt": 2_000,
            "name": "Synthetic resume",
        })
        .to_string();
        let hex = meta
            .bytes()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        db.execute("UPDATE meta SET value = ?1 WHERE key = '0'", params![hex])
            .unwrap();

        let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
        assert!(
            sessions
                .iter()
                .all(|session| cursor_store_observation(session).is_none())
        );
    }

    #[test]
    #[ignore = "requires a recent local Cursor Agent /fork"]
    fn live_cursor_cli_fork_store_matches_reader_contract() {
        let home = crate::paths::home_dir().expect("home directory");
        let sessions = query_cursor_store_db_sessions(&home.join(".cursor/chats"), i64::MIN);
        let observation = sessions
            .iter()
            .find_map(cursor_store_observation)
            .expect("a fixture-proven Cursor CLI fork");
        assert_eq!(observation.parent_agent, "cursor");
        assert_eq!(observation.fork_kind, "fork");
        assert_eq!(observation.inherited_item_count, Some(19));
    }

    async fn write_workspace(path: &Path, folder: &str, composer_id: &str, updated_at: i64) {
        tokio::fs::create_dir_all(path)
            .await
            .expect("test Cursor workspace directory should be created");
        tokio::fs::write(
            path.join("workspace.json"),
            json!({ "folder": folder }).to_string(),
        )
        .await
        .expect("test Cursor workspace metadata should be written");
        let db = create_cursor_db(&path.join("state.vscdb"));
        db.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            params![
                "composer.composerData",
                json!({
                    "allComposers": [{
                        "composerId": composer_id,
                        "lastUpdatedAt": updated_at,
                        "createdAt": updated_at - 1000,
                    }]
                })
                .to_string()
            ],
        )
        .expect("test Cursor workspace composer data should be inserted");
    }

    fn seed_global_cursor_session(path: &Path, composer_id: &str) {
        let db = create_cursor_db(path);
        db.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            params![
                format!("composerData:{composer_id}"),
                json!({
                    "composerId": composer_id,
                    "createdAt": 1_775_083_355_225i64,
                    "lastUpdatedAt": 1_775_083_382_256i64,
                    "modelConfig": { "modelName": "composer-2" },
                    "fullConversationHeadersOnly": [
                        { "bubbleId": "u1", "type": 1 },
                        { "bubbleId": "a1", "type": 2 }
                    ]
                })
                .to_string()
            ],
        )
        .expect("test Cursor composer data should be inserted");
        db.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            params![
                format!("bubbleId:{composer_id}:u1"),
                json!({
                    "type": 1,
                    "text": "Explain the architecture",
                    "createdAt": "2026-04-01T22:42:55.658Z",
                    "modelInfo": { "modelName": "composer-2" }
                })
                .to_string()
            ],
        )
        .expect("test Cursor user bubble should be inserted");
        db.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            params![
                format!("bubbleId:{composer_id}:a1"),
                json!({
                    "type": 2,
                    "markdown": "Here is the architecture.",
                    "createdAt": "2026-04-01T22:42:57.468Z"
                })
                .to_string()
            ],
        )
        .expect("test Cursor assistant bubble should be inserted");
    }

    #[test]
    fn cursor_desktop_reader_embeds_only_verified_duplicate_lineage() {
        let temp = TempDir::new().unwrap();
        let ws_root = temp.path().join("workspaceStorage");
        let workspace = ws_root.join("workspace-1");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            workspace.join("workspace.json"),
            json!({ "folder": "/tmp/repo" }).to_string(),
        )
        .unwrap();
        let workspace_db = create_cursor_db(&workspace.join("state.vscdb"));
        workspace_db
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                params![
                    "composer.composerData",
                    json!({
                        "allComposers": [
                            { "composerId": "parent", "lastUpdatedAt": 2_000_000, "isSubagent": 0 },
                            { "composerId": "child", "lastUpdatedAt": 3_000_000, "isSubagent": 0 }
                        ]
                    })
                    .to_string()
                ],
            )
            .unwrap();
        let global_path = temp.path().join("global.vscdb");
        let global = create_cursor_db(&global_path);
        for (id, headers) in [
            (
                "parent",
                json!([
                    { "bubbleId": "parent-u", "type": 1, "grouping": { "hasText": true } },
                    { "bubbleId": "parent-a", "type": 2, "grouping": { "hasText": true } }
                ]),
            ),
            (
                "child",
                json!([
                    { "bubbleId": "child-u", "type": 1, "grouping": { "hasText": true } },
                    { "bubbleId": "child-a", "type": 2, "grouping": { "hasText": true } },
                    { "bubbleId": "child-u2", "type": 1, "grouping": { "hasText": true } }
                ]),
            ),
        ] {
            global
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    params![
                        format!("composerData:{id}"),
                        json!({
                            "composerId": id,
                            "fullConversationHeadersOnly": headers,
                            "isBestOfNSubcomposer": false
                        })
                        .to_string()
                    ],
                )
                .unwrap();
        }
        for (id, bubble, kind, text) in [
            ("parent", "parent-u", 1, "question"),
            ("parent", "parent-a", 2, "answer"),
            ("child", "child-u", 1, "question"),
            ("child", "child-a", 2, "answer"),
            ("child", "child-u2", 1, "follow up"),
        ] {
            global
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    params![
                        format!("bubbleId:{id}:{bubble}"),
                        json!({ "type": kind, "text": text }).to_string()
                    ],
                )
                .unwrap();
        }
        drop(global);

        let sessions = query_cursor_desktop_sessions(
            &ws_root,
            &global_path,
            4_000,
            4_000,
            Some(prefix_duplicate_detector),
        );
        let child = sessions
            .iter()
            .find(|session| session.canonical_id.as_deref() == Some("child"))
            .unwrap();
        let SessionSource::Inline { content, .. } = &child.log.source else {
            panic!("expected inline Cursor session")
        };
        let observation = embedded_fork_observation(content).expect("duplicate lineage");
        assert_eq!(observation.parent_agent_session_id, "parent");
        assert_eq!(observation.inherited_item_count, Some(2));
    }

    fn seed_cursor_store_db(path: &Path, session_id: &str) {
        let db = create_cursor_store_db(path);
        db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![
                "sessionMeta",
                json!({
                    "agentId": session_id,
                    "workspacePath": "/Users/avery/dev/demo-cli",
                    "lastUsedModel": "claude-sonnet-4",
                    "createdAt": 1_775_083_355_225i64,
                    "lastUpdatedAt": 1_775_083_382_256i64,
                })
                .to_string()
            ],
        )
        .expect("test Cursor store session metadata should be inserted");
        db.execute(
            "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
            params![
                "message-1",
                json!({
                    "messages": [
                        {"role": "user", "content": "Investigate the outage", "createdAt": 1_775_083_355_225i64},
                        {"role": "assistant", "content": "Here is the outage analysis.", "createdAt": 1_775_083_382_256i64}
                    ]
                })
                .to_string()
            ],
        )
        .expect("test Cursor store messages should be inserted");
    }

    #[tokio::test]
    async fn test_cursor_log_dirs_collects_chat_sessions_and_agent_transcripts() {
        let home = TempDir::new().unwrap();

        let ws_root = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("abc")
            .join("chatSessions");
        tokio::fs::create_dir_all(&ws_root).await.unwrap();

        let transcripts_dir = home
            .path()
            .join(".cursor")
            .join("projects")
            .join("Users-avery-dev-demo-cli")
            .join("agent-transcripts")
            .join("conversation");
        tokio::fs::create_dir_all(&transcripts_dir).await.unwrap();
        tokio::fs::write(transcripts_dir.join("conversation.jsonl"), "{}\n")
            .await
            .unwrap();

        let dirs = log_dirs_in(home.path()).await;

        assert!(dirs.contains(&ws_root));
        assert!(dirs.iter().any(|dir| dir.ends_with("agent-transcripts")));
    }

    #[tokio::test]
    async fn test_cursor_log_dirs_ignore_non_session_project_dirs() {
        let home = TempDir::new().unwrap();
        let agent_tools_dir = home
            .path()
            .join(".cursor")
            .join("projects")
            .join("p1")
            .join("agent-tools");
        tokio::fs::create_dir_all(&agent_tools_dir).await.unwrap();
        tokio::fs::write(agent_tools_dir.join("tool.txt"), "content")
            .await
            .unwrap();

        let terminals_dir = home
            .path()
            .join(".cursor")
            .join("projects")
            .join("p1")
            .join("terminals");
        tokio::fs::create_dir_all(&terminals_dir).await.unwrap();
        tokio::fs::write(terminals_dir.join("1.txt"), "content")
            .await
            .unwrap();

        let dirs = log_dirs_in(home.path()).await;
        assert!(dirs.is_empty());
    }

    #[tokio::test]
    async fn test_cursor_discovers_agent_transcript_and_recovers_metadata() {
        let home = TempDir::new().unwrap();
        let repo_root = home
            .path()
            .join("Users")
            .join("zack")
            .join("dev")
            .join("demo-cli");
        tokio::fs::create_dir_all(&repo_root).await.unwrap();

        let transcript = home
            .path()
            .join(".cursor")
            .join("projects")
            .join(encode_cursor_project_workspace_key(&repo_root))
            .join("agent-transcripts")
            .join("abc")
            .join("abc.jsonl");
        tokio::fs::create_dir_all(transcript.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &transcript,
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
        )
        .await
        .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let logs = discover_recent_in(home.path(), now, 3600, None).await;
        assert_eq!(logs.len(), 1);

        let content = match &logs[0].source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline transcript"),
            SessionSource::ProviderDb { .. } => panic!("expected inline transcript"),
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(metadata.session_id.as_deref(), Some("abc"));
        assert_eq!(
            metadata.cwd.as_deref(),
            Some(repo_root.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn test_cursor_discovers_desktop_state_vscdb_sessions() {
        let home = TempDir::new().unwrap();
        let global_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("globalStorage");
        tokio::fs::create_dir_all(&global_dir).await.unwrap();
        seed_global_cursor_session(&global_dir.join("state.vscdb"), "composer-1");

        let workspace_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("workspace-1");
        write_workspace(
            &workspace_dir,
            if cfg!(windows) {
                "file:///C:/cursor-test-workspace"
            } else {
                "file:///tmp/cursor-test-workspace"
            },
            "composer-1",
            1_775_083_382_256i64,
        )
        .await;

        let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
        assert_eq!(logs.len(), 1);
        let content = match &logs[0].source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline desktop log"),
            SessionSource::ProviderDb { .. } => panic!("expected inline desktop log"),
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(metadata.session_id.as_deref(), Some("composer-1"));
        assert_eq!(
            metadata.cwd.as_deref(),
            Some(if cfg!(windows) {
                "C:/cursor-test-workspace"
            } else {
                "/tmp/cursor-test-workspace"
            })
        );
        assert!(content.contains("Explain the architecture"));
        assert!(content.contains("Here is the architecture."));
    }

    /// Helper for the migrated-blob test. Mirrors `write_workspace` but seeds
    /// `composer.composerData` in the post-migration shape
    /// (`selectedComposerIds[]` + `hasMigratedComposerData: true`), which is
    /// what Cursor writes today.
    async fn write_workspace_migrated(path: &Path, folder: &str, composer_ids: &[&str]) {
        tokio::fs::create_dir_all(path).await.unwrap();
        tokio::fs::write(
            path.join("workspace.json"),
            json!({ "folder": folder }).to_string(),
        )
        .await
        .expect("migrated test Cursor workspace metadata should be written");
        let db = create_cursor_db(&path.join("state.vscdb"));
        db.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            params![
                "composer.composerData",
                json!({
                    "selectedComposerIds": composer_ids,
                    "lastFocusedComposerIds": composer_ids,
                    "hasMigratedComposerData": true,
                    "hasMigratedMultipleComposers": true,
                })
                .to_string()
            ],
        )
        .expect("migrated test Cursor composer data should be inserted");
    }

    /// Seed a composer with an explicit `name` field (Cursor's title slot,
    /// used by both AI-set initial names and user renames).
    fn seed_global_cursor_session_with_name(path: &Path, composer_id: &str, name: &str) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value BLOB);
             CREATE TABLE IF NOT EXISTS cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);",
        )
        .expect("named test Cursor database schema should initialize");
        db.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            params![
                format!("composerData:{composer_id}"),
                json!({
                    "composerId": composer_id,
                    "name": name,
                    "createdAt": 1_775_083_355_225i64,
                    "lastUpdatedAt": 1_775_083_382_256i64,
                    "fullConversationHeadersOnly": [
                        { "bubbleId": "u1", "type": 1 }
                    ]
                })
                .to_string()
            ],
        )
        .expect("named test Cursor composer data should be inserted");
        db.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            params![
                format!("bubbleId:{composer_id}:u1"),
                json!({
                    "type": 1,
                    "text": "any prompt",
                    "createdAt": "2026-04-01T22:42:55.658Z",
                })
                .to_string()
            ],
        )
        .expect("named test Cursor user bubble should be inserted");
    }

    #[tokio::test]
    async fn test_cursor_desktop_session_uses_composer_name_as_title() {
        // Composer's `name` field is Cursor IDE's title slot. AI-generated
        // initial names and user-renamed titles both land here. Assert the
        // scanner picks it up via Tier 2 (ExplicitField) so it beats the
        // first-message fallback, and the provenance is `Explicit`.
        use crate::discovery::scanner::TitleSource;

        let home = TempDir::new().unwrap();
        let global_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("globalStorage");
        tokio::fs::create_dir_all(&global_dir).await.unwrap();
        seed_global_cursor_session_with_name(
            &global_dir.join("state.vscdb"),
            "composer-1",
            "(RENAME) Hello world test from Cursor IDE",
        );

        let workspace_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("workspace-1");
        write_workspace_migrated(
            &workspace_dir,
            if cfg!(windows) {
                "file:///C:/cursor-test-workspace"
            } else {
                "file:///tmp/cursor-test-workspace"
            },
            &["composer-1"],
        )
        .await;

        let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
        assert_eq!(logs.len(), 1);
        let content = match &logs[0].source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline desktop log"),
            SessionSource::ProviderDb { .. } => panic!("expected inline desktop log"),
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("(RENAME) Hello world test from Cursor IDE")
        );
        assert_eq!(metadata.title_source, Some(TitleSource::Explicit));
    }

    #[tokio::test]
    async fn test_cursor_discovers_desktop_sessions_with_migrated_workspace_blob() {
        // Real on-disk shape from a current Cursor install: the workspace
        // `composer.composerData` blob only carries id lists; the per-composer
        // `lastUpdatedAt` lives in the global `cursorDiskKV.composerData:<id>`
        // row. Verifies the fallback path through
        // `fetch_composer_last_updated_at_ms`.
        let home = TempDir::new().unwrap();
        let global_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("globalStorage");
        tokio::fs::create_dir_all(&global_dir).await.unwrap();
        seed_global_cursor_session(&global_dir.join("state.vscdb"), "composer-1");

        let workspace_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("workspace-1");
        write_workspace_migrated(
            &workspace_dir,
            if cfg!(windows) {
                "file:///C:/cursor-test-workspace"
            } else {
                "file:///tmp/cursor-test-workspace"
            },
            &["composer-1"],
        )
        .await;

        let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
        assert_eq!(
            logs.len(),
            1,
            "expected one Cursor IDE session from migrated workspace blob"
        );
        let content = match &logs[0].source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline desktop log"),
            SessionSource::ProviderDb { .. } => panic!("expected inline desktop log"),
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(metadata.session_id.as_deref(), Some("composer-1"));
        assert!(content.contains("Explain the architecture"));
    }

    #[test]
    fn test_cursor_dedupe_preserves_bubble_order() {
        let mut ids = vec![
            "bubble-2".to_string(),
            "bubble-10".to_string(),
            "bubble-2".to_string(),
            "bubble-1".to_string(),
        ];

        dedupe_preserving_order(&mut ids);

        assert_eq!(ids, vec!["bubble-2", "bubble-10", "bubble-1"]);
    }

    #[tokio::test]
    async fn test_cursor_discovers_store_db_sessions() {
        let home = TempDir::new().unwrap();
        let store_dir = home
            .path()
            .join(".cursor")
            .join("chats")
            .join("a")
            .join("b");
        tokio::fs::create_dir_all(&store_dir).await.unwrap();
        seed_cursor_store_db(&store_dir.join("store.db"), "store-session-1");

        let logs =
            discover_store_db_sessions(home.path(), current_unix_epoch_for_tests(), 3600).await;
        assert_eq!(logs.len(), 1);
        let content = match &logs[0].log.source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline store db session"),
            SessionSource::ProviderDb { .. } => panic!("expected inline store db session"),
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(metadata.session_id.as_deref(), Some("store-session-1"));
        assert_eq!(metadata.cwd.as_deref(), Some("/Users/avery/dev/demo-cli"));
        assert!(content.contains("Investigate the outage"));
        assert!(content.contains("Here is the outage analysis."));
    }

    /// Seed a `store.db` with a `meta` blob carrying a `name` field but no
    /// workspace path — matches the actual on-disk shape of
    /// `~/.cursor/chats/<ws>/<agent>/store.db` written by `cursor-agent`
    /// (workspace path is recoverable only via the agent-transcripts side).
    fn seed_cursor_store_db_named_no_cwd(path: &Path, session_id: &str, name: &str) {
        let db = create_cursor_store_db(path);
        db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![
                "sessionMeta",
                json!({
                    "agentId": session_id,
                    "name": name,
                    "lastUsedModel": "composer-2.5",
                    "createdAt": current_unix_epoch_for_tests() * 1000 - 60_000,
                    "lastUpdatedAt": current_unix_epoch_for_tests() * 1000,
                })
                .to_string()
            ],
        )
        .expect("named test Cursor store metadata should be inserted");
        db.execute(
            "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
            params![
                "message-1",
                json!({
                    "messages": [
                        {"role": "user", "content": "doesn't matter — store.db `name` should win", "createdAt": 1i64},
                    ]
                })
                .to_string()
            ],
        )
        .expect("named test Cursor store message should be inserted");
    }

    #[test]
    fn test_decode_hex_jsonish_decodes_real_meta_blob_shape() {
        // Mirrors the exact bytes Cursor writes into
        // `~/.cursor/chats/*/store.db#meta.value`: literal ASCII hex of a
        // JSON document, stored in a TEXT column.
        let json = r#"{"agentId":"x","name":"renamed"}"#;
        let hex: String = json.bytes().map(|b| format!("{:02x}", b)).collect();
        let decoded = decode_hex_jsonish(&hex).expect("hex-encoded json must decode");
        assert_eq!(decoded, json);
    }

    #[test]
    fn test_decode_hex_jsonish_rejects_plain_json_and_garbage() {
        assert!(decode_hex_jsonish("{\"already\":\"json\"}").is_none());
        assert!(decode_hex_jsonish("not-hex").is_none());
        // First byte decodes to 'x' (`78`), not `{` / `[`.
        assert!(decode_hex_jsonish("78797a").is_none());
        // Odd length.
        assert!(decode_hex_jsonish("7b22").is_some()); // even
        assert!(decode_hex_jsonish("7b223").is_none()); // odd
    }

    /// Seed a store.db whose `meta.value` is hex-encoded JSON, matching the
    /// real on-disk shape Cursor writes (see `decode_hex_jsonish`).
    fn seed_cursor_store_db_hex_meta(path: &Path, session_id: &str, name: &str) {
        let db = create_cursor_store_db(path);
        let meta_json = json!({
            "agentId": session_id,
            "name": name,
            "createdAt": current_unix_epoch_for_tests() * 1000 - 60_000,
            "lastUpdatedAt": current_unix_epoch_for_tests() * 1000,
        })
        .to_string();
        let hex: String = meta_json.bytes().map(|b| format!("{:02x}", b)).collect();
        db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params!["sessionMeta", hex],
        )
        .unwrap();
        db.execute(
            "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
            params![
                "msg-1",
                json!({"messages":[{"role":"user","content":"first prompt","createdAt":1i64}]})
                    .to_string()
            ],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_cursor_cli_store_db_with_hex_encoded_meta_reads_rename_name() {
        // Production Cursor stores `meta.value` as a TEXT column whose content
        // is hex-encoded JSON. Verifies the reader's hex-decode fallback
        // surfaces the `name` field (the `/rename` slot) so it lands as the
        // tray title.
        use crate::discovery::scanner::TitleSource;

        let home = TempDir::new().unwrap();
        let session_id = "abc12345-6789-0000-aaaa-cccccccccccc";
        let store_dir = home
            .path()
            .join(".cursor")
            .join("chats")
            .join("workspacehash")
            .join(session_id);
        tokio::fs::create_dir_all(&store_dir).await.unwrap();
        seed_cursor_store_db_hex_meta(
            &store_dir.join("store.db"),
            session_id,
            "(RENAME) hello from real-shape store.db",
        );

        let logs =
            discover_recent_in(home.path(), current_unix_epoch_for_tests(), 3600, None).await;
        assert_eq!(logs.len(), 1);
        let content = match &logs[0].source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline log"),
            SessionSource::ProviderDb { .. } => panic!("expected inline log"),
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("(RENAME) hello from real-shape store.db"),
        );
        assert_eq!(metadata.title_source, Some(TitleSource::Explicit));
    }

    #[test]
    fn cursor_cli_store_metadata_ignores_nested_blob_names() {
        let temp = TempDir::new().unwrap();
        let db = create_cursor_store_db(&temp.path().join("store.db"));
        db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params!["session", json!({ "agentId": "session-1" }).to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
            params![
                "tool-result",
                json!({
                    "name": "Nested tool blob must not become title",
                    "role": "user",
                    "content": "Actual first prompt"
                })
                .to_string()
            ],
        )
        .unwrap();
        drop(db);

        let candidate = build_cursor_store_db_session(&temp.path().join("store.db")).unwrap();
        let SessionSource::Inline { content, .. } = candidate.log.source else {
            panic!("expected inline store session")
        };
        let metadata = scanner::parse_session_metadata_str(&content);
        assert_eq!(metadata.title.as_deref(), Some("Actual first prompt"));
    }

    #[tokio::test]
    async fn test_cursor_cli_session_uses_store_db_name_and_backfills_cwd() {
        // Real on-disk shape: `cursor-agent` writes its session metadata to
        // `~/.cursor/chats/<ws-hash>/<agent_id>/store.db` and its transcript
        // to `~/.cursor/projects/<ws>/agent-transcripts/<id>/<id>.jsonl`.
        // The two stores share the agent UUID; only the agent-transcripts
        // side knows the workspace path. Asserts:
        // 1. `meta.value.name` becomes the title.
        // 2. cwd is backfilled from the agent-transcripts project dir, so
        //    the StoreDb + AgentTranscript candidates merge to a single row.
        let home = TempDir::new().unwrap();
        let repo_root = home
            .path()
            .join("Users")
            .join("zack")
            .join("dev")
            .join("demo-cli");
        tokio::fs::create_dir_all(&repo_root).await.unwrap();
        let session_id = "abc12345-6789-0000-aaaa-bbbbbbbbbbbb";

        // Agent-transcripts side: gives cwd via the workspace-key dir name.
        let transcript = home
            .path()
            .join(".cursor")
            .join("projects")
            .join(encode_cursor_project_workspace_key(&repo_root))
            .join("agent-transcripts")
            .join(session_id)
            .join(format!("{session_id}.jsonl"));
        tokio::fs::create_dir_all(transcript.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &transcript,
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<user_query>fallback prompt</user_query>\"}]}}\n",
        )
        .await
        .unwrap();

        // store.db side: same UUID, has the title, has no cwd.
        let store_dir = home
            .path()
            .join(".cursor")
            .join("chats")
            .join("workspacehash")
            .join(session_id);
        tokio::fs::create_dir_all(&store_dir).await.unwrap();
        seed_cursor_store_db_named_no_cwd(
            &store_dir.join("store.db"),
            session_id,
            "(RENAME) hello world from Cursor CLI",
        );

        let logs =
            discover_recent_in(home.path(), current_unix_epoch_for_tests(), 3600, None).await;
        assert_eq!(
            logs.len(),
            1,
            "expected the two candidates (transcript + store.db) to merge"
        );
        let content = match &logs[0].source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline log"),
            SessionSource::ProviderDb { .. } => panic!("expected inline log"),
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(metadata.session_id.as_deref(), Some(session_id));
        assert_eq!(
            metadata.title.as_deref(),
            Some("(RENAME) hello world from Cursor CLI"),
            "title must come from store.db `name`, not the transcript first-message"
        );
        assert_eq!(
            metadata.cwd.as_deref(),
            Some(repo_root.to_string_lossy().as_ref()),
            "cwd must be backfilled from the agent-transcripts side"
        );
    }

    #[tokio::test]
    async fn agent_fixture_keeps_distinct_sessions_and_detects_marked_fork() {
        let fixture: Value =
            serde_json::from_str(include_str!("tests/fixtures/cursor-cli-agent-fork.json"))
                .unwrap();
        let home = TempDir::new().unwrap();
        let repo_root = home.path().join("home/avery/projects/demo-app");
        tokio::fs::create_dir_all(&repo_root).await.unwrap();
        let project_dir = home
            .path()
            .join(".cursor/projects")
            .join(encode_cursor_project_workspace_key(&repo_root));

        for session in fixture["sessions"].as_array().unwrap() {
            let session_id = session["id"].as_str().unwrap();
            let transcript_path = project_dir
                .join("agent-transcripts")
                .join(session_id)
                .join(format!("{session_id}.jsonl"));
            tokio::fs::create_dir_all(transcript_path.parent().unwrap())
                .await
                .unwrap();
            let transcript = session["transcript"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            tokio::fs::write(&transcript_path, format!("{transcript}\n"))
                .await
                .unwrap();

            let chat_dir = home
                .path()
                .join(".cursor/chats/fixture-workspace")
                .join(session_id);
            tokio::fs::create_dir_all(&chat_dir).await.unwrap();
            tokio::fs::write(chat_dir.join("meta.json"), session["meta"].to_string())
                .await
                .unwrap();
            seed_cursor_store_db_named_no_cwd(
                &chat_dir.join("store.db"),
                session_id,
                session["meta"]["title"].as_str().unwrap(),
            );
        }

        let logs =
            discover_recent_in(home.path(), current_unix_epoch_for_tests(), 3600, None).await;
        assert_eq!(logs.len(), 3, "all three Cursor Agent IDs stay distinct");
        let content_by_id = logs
            .iter()
            .filter_map(|log| {
                let SessionSource::Inline { content, .. } = &log.source else {
                    return None;
                };
                let id = scanner::parse_session_metadata_str(content).session_id?;
                Some((id, content))
            })
            .collect::<HashMap<_, _>>();
        let child_id = "00000000-0000-4000-8000-000000000102";
        assert_eq!(
            embedded_fork_observation(content_by_id[child_id])
                .expect("fork observation")
                .parent_agent_session_id,
            "00000000-0000-4000-8000-000000000101"
        );
        let unrelated_id = "00000000-0000-4000-8000-000000000103";
        assert!(
            embedded_fork_observation(content_by_id[unrelated_id]).is_none(),
            "a fork title without an exact copied prefix must fail closed"
        );
    }

    #[tokio::test]
    async fn test_cursor_dedupes_duplicate_transcript_and_desktop_session_ids() {
        let home = TempDir::new().unwrap();
        let repo_root = PathBuf::from("/Users/avery/dev/demo-cli");

        let global_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("globalStorage");
        tokio::fs::create_dir_all(&global_dir).await.unwrap();
        seed_global_cursor_session(&global_dir.join("state.vscdb"), "shared-session");

        let workspace_dir = app_config_dir_in("Cursor", home.path())
            .join("User")
            .join("workspaceStorage")
            .join("workspace-1");
        write_workspace(
            &workspace_dir,
            &format!("file://{}", repo_root.display()),
            "shared-session",
            1_775_083_382_256i64,
        )
        .await;

        let encoded = encode_cursor_project_workspace_key(&repo_root);
        let transcript = home
            .path()
            .join(".cursor")
            .join("projects")
            .join(encoded)
            .join("agent-transcripts")
            .join("shared-session")
            .join("shared-session.jsonl");
        tokio::fs::create_dir_all(transcript.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &transcript,
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"short transcript\"}]}}\n",
        )
        .await
        .unwrap();

        let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
        assert_eq!(logs.len(), 1);
        let content = match &logs[0].source {
            SessionSource::Inline { content, .. } => content,
            SessionSource::File(_) => panic!("expected inline session"),
            SessionSource::ProviderDb { .. } => panic!("expected inline session"),
        };
        assert!(content.contains("Explain the architecture"));
        assert!(!content.contains("short transcript"));
    }

    fn current_unix_epoch_for_tests() -> i64 {
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn test_encode_cursor_project_workspace_key_windows_drive_format() {
        let encoded =
            encode_cursor_project_workspace_key(Path::new(r"C:\Users\avery\dev\demo-cli"));

        if cfg!(windows) {
            assert_eq!(encoded, "C--Users-avery-dev-demo-cli");
        }
    }
}
