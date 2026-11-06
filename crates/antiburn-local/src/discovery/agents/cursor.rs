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

mod header_log;
mod json_encoding;
mod tool_call_binary;

use self::json_encoding::{decode_hex_jsonish, looks_like_jsonish};
use self::tool_call_binary::{DecodeError, NativeStatus, decode_completed_call};
use super::path_codec::decode_cursor_workspace_path_buf;
use crate::discovery::scanner::{self, AgentKind};
use crate::discovery::{
    AgentExplorer, DuplicateForkDetector, FORK_OBSERVATION_KEY, ForkObservation, SessionLog,
    SessionSource, SurfacePaths, WatchRoot, app_config_dir_in, env_path_when_real_home,
    find_chat_session_dirs, home_dir, recent_files_with_exts,
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

    /// Owns `~/.cursor/{projects,chats}/**` (CLI) and each selected Cursor
    /// user-data root's `User/**` tree (IDE). The loose `/cursor/` substring
    /// is intentionally avoided — it catches unrelated user directories.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.cursor/`       → `cli` (`cursor-agent` projects + chats)
    /// - `/cursor/user/`   → `ide_desktop` (canonical Cursor User roots)
    /// - selected `<user-data>/User/` → `ide_desktop` (workspace and state DBs)
    fn owns_path(&self, path_lower: &str) -> bool {
        let roots = home_dir().map_or_else(Vec::new, |home| cursor_user_data_roots_in(&home));
        cursor_owns_path(path_lower, &roots)
    }

    // Cursor is bi-modal:
    //   - CLI (`cursor-agent`): `~/.cursor/projects/**/agent-transcripts/**`
    //                           and `~/.cursor/chats/**`.
    //   - IDE chat:             `<user_data>/User/workspaceStorage/**`
    //                           and store.db cascades labelled `cursor-store:` /
    //                           `cursor-desktop:`.
    // Trait default classifies via `surface_paths`; inline labels without
    // an embedded ws-root substring (e.g. `cursor-desktop:<ws>:<composer>`)
    // fall through to the IDE default below.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// CLI: `~/.cursor/{projects,chats}/**` (cursor-agent). IDE:
    /// Each selected user-data root's `User/workspaceStorage/**` (chat
    /// sessions, store.db cascades surfaced via inline labels).
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        cursor_surface_paths(home, &cursor_user_data_roots_in(home))
    }

    /// The same CLI and IDE roots discovery walks.
    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        cursor_watch_roots(home, &cursor_user_data_roots_in(home))
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

/// The platform default plus an explicit isolated Cursor Desktop profile.
/// Cursor must still be launched with `--user-data-dir` for this override to
/// refer to the profile it writes.
fn cursor_user_data_roots_in(home: &Path) -> Vec<PathBuf> {
    let process_home = home_dir();
    let explicit = env_path_when_real_home(home, "ANTIBURN_CURSOR_USER_DATA_DIR");
    cursor_user_data_roots_for_home(home, process_home.as_deref(), explicit.as_deref())
}

fn cursor_user_data_roots_for_home(
    home: &Path,
    process_home: Option<&Path>,
    configured: Option<&Path>,
) -> Vec<PathBuf> {
    let default = app_config_dir_in("Cursor", home);
    let explicit = configured
        .filter(|_| process_home == Some(home))
        .filter(|path| {
            path.is_absolute() && path.to_str().is_none_or(|value| !value.trim().is_empty())
        })
        .map(Path::to_path_buf);
    let mut roots = vec![default];
    if let Some(explicit) = explicit {
        let canonical =
            |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let explicit_canonical = canonical(&explicit);
        if !roots
            .iter()
            .any(|root| canonical(root) == explicit_canonical)
        {
            roots.push(explicit);
        }
    }
    roots
}

fn cursor_owns_path(path_lower: &str, roots: &[PathBuf]) -> bool {
    path_lower.contains("/.cursor/")
        || path_lower.contains("/cursor/user/")
        || roots.iter().any(|root| {
            let root = root.to_string_lossy().replace('\\', "/").to_lowercase();
            path_lower.starts_with(&format!("{}/user/", root.trim_end_matches('/')))
        })
}

fn cursor_surface_paths(home: &Path, user_data_roots: &[PathBuf]) -> SurfacePaths {
    SurfacePaths {
        cli: vec![
            home.join(".cursor").join("projects"),
            home.join(".cursor").join("chats"),
        ],
        ide_desktop: user_data_roots
            .iter()
            .flat_map(|root| {
                [
                    root.join("User/workspaceStorage"),
                    root.join("User/globalStorage"),
                ]
            })
            .collect(),
        mirror: Vec::new(),
    }
}

fn cursor_watch_roots(home: &Path, user_data_roots: &[PathBuf]) -> Vec<WatchRoot> {
    let paths = cursor_surface_paths(home, user_data_roots);
    paths
        .cli
        .into_iter()
        .chain(paths.ide_desktop)
        .map(WatchRoot::recursive)
        .collect()
}

async fn discover_recent_in(
    home: &Path,
    now: i64,
    since_secs: i64,
    duplicate_fork_detector: Option<DuplicateForkDetector>,
) -> Vec<SessionLog> {
    let roots = cursor_user_data_roots_in(home);
    discover_recent_in_roots(home, &roots, now, since_secs, duplicate_fork_detector).await
}

async fn discover_recent_in_roots(
    home: &Path,
    roots: &[PathBuf],
    now: i64,
    since_secs: i64,
    duplicate_fork_detector: Option<DuplicateForkDetector>,
) -> Vec<SessionLog> {
    let mut discovered: HashMap<String, CursorDiscoveredSession> = HashMap::new();

    for root in roots {
        let ws_root = root.join("User/workspaceStorage");
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
    }

    for candidate in discover_agent_transcripts(home, now, since_secs).await {
        merge_cursor_session(&mut discovered, candidate);
    }

    for candidate in discover_store_db_sessions(home, now, since_secs).await {
        merge_cursor_session(&mut discovered, candidate);
    }

    let desktop_logs =
        discover_desktop_sessions(roots, now, since_secs, duplicate_fork_detector).await;
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

    for root in cursor_user_data_roots_in(home) {
        dirs.extend(find_chat_session_dirs(&root.join("User/workspaceStorage")).await);
    }

    let projects_dir = home.join(".cursor").join("projects");
    dirs.extend(collect_agent_transcript_dirs(&projects_dir).await);

    dirs
}

/// How many directory levels [`collect_agent_transcript_dirs`] enters below
/// `root`. Cursor's CLI layout puts `agent-transcripts` two levels below the
/// projects root: `projects/<project>/agent-transcripts`. The walk never
/// enters a directory past this depth, so it never reads a project's other
/// subdirectories, such as `canvases`.
///
/// A directory's mtime cannot gate this walk instead (F2): a new session
/// file bumps only its immediate parent, `agent-transcripts/`, never the
/// project directory above it. An mtime gate on the project directory would
/// hide every session added to an old project from then on.
const MAX_TRANSCRIPT_WALK_DEPTH: usize = 2;

/// Collect `agent-transcripts` directories under `root`, entering a
/// directory no deeper than [`MAX_TRANSCRIPT_WALK_DEPTH`].
async fn collect_agent_transcript_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];

    while let Some((dir, depth)) = stack.pop() {
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

            let entry_depth = depth + 1;
            if entry_depth < MAX_TRANSCRIPT_WALK_DEPTH {
                stack.push((path, entry_depth));
            }
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
                let fork_observation =
                    cursor_agent_subagent_parent(&path).map(|parent_id| ForkObservation {
                        parent_agent: "cursor".to_owned(),
                        parent_agent_session_id: parent_id,
                        fork_kind: "subagent".to_owned(),
                        provider_fork_point_id: None,
                        detection_source: "subagent_path".to_owned(),
                        confidence: 100,
                        inherited_item_count: None,
                        extractor_version: "cursor-agent-subagent-v1".to_owned(),
                    });
                let source = SessionSource::Inline {
                    label,
                    source_format: None,
                    content: prepend_cursor_metadata_line(
                        &content,
                        session_id,
                        CursorSyntheticMetadata {
                            cwd: cwd.as_deref(),
                            model: None,
                            title: metadata.title.as_deref(),
                            created_at: metadata.created_at,
                            updated_at: metadata.updated_at,
                            fork_observation: fork_observation.as_ref(),
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

/// How many directory levels [`collect_cursor_chat_metadata`] enters below
/// `~/.cursor/chats`. The layout is `chats/<workspace>/<chat>/meta.json`: a
/// workspace directory is depth 1, a chat directory is depth 2, and
/// `meta.json` is read from inside the chat directory. The walk never enters
/// a directory past this depth.
const MAX_CHAT_WALK_DEPTH: usize = 2;

/// Collect `meta.json` sidecars under `~/.cursor/chats`, entering a
/// directory no deeper than [`MAX_CHAT_WALK_DEPTH`].
async fn collect_cursor_chat_metadata(home: &Path) -> HashMap<String, CursorChatMetadata> {
    let mut out = HashMap::new();
    let mut stack = vec![(home.join(".cursor").join("chats"), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
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
                let entry_depth = depth + 1;
                if entry_depth <= MAX_CHAT_WALK_DEPTH {
                    stack.push((path, entry_depth));
                }
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
    collect_workspace_paths_in_roots(&cursor_user_data_roots_in(home)).await
}

async fn collect_workspace_paths_in_roots(roots: &[PathBuf]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for root in roots {
        let ws_root = root.join("User/workspaceStorage");
        let Ok(mut entries) = tokio::fs::read_dir(ws_root).await else {
            continue;
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

/// Cursor stores a child transcript at
/// `agent-transcripts/<parent-id>/subagents/<child-id>.jsonl`. The parent
/// directory is a vendor-assigned session id, so this link is stronger than a
/// content-prefix fork guess and needs no title heuristic.
fn cursor_agent_subagent_parent(path: &Path) -> Option<String> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
        || path.parent()?.file_name().and_then(|name| name.to_str()) != Some("subagents")
    {
        return None;
    }
    let transcript_root = path
        .parent()?
        .parent()?
        .parent()?
        .file_name()
        .and_then(|name| name.to_str())?;
    if transcript_root != "agent-transcripts" {
        return None;
    }
    let parent_id = path
        .parent()?
        .parent()?
        .file_name()
        .and_then(|name| name.to_str())?;
    is_safe_session_id(parent_id).then(|| parent_id.to_owned())
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
    roots: &[PathBuf],
    now: i64,
    since_secs: i64,
    duplicate_fork_detector: Option<DuplicateForkDetector>,
) -> Vec<CursorDiscoveredSession> {
    let mut logs = Vec::new();
    for root in roots {
        let ws_root = root.join("User/workspaceStorage");
        let global_db = root.join("User/globalStorage/state.vscdb");
        if !global_db.exists() || !ws_root.exists() {
            continue;
        }
        let detector = duplicate_fork_detector;
        let result = tokio::task::spawn_blocking(move || {
            query_cursor_desktop_sessions(&ws_root, &global_db, now, since_secs, detector)
        })
        .await
        .unwrap_or_default();
        logs.extend(result);
    }
    logs
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
                        source_format: None,
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
    let snapshot = global.unchecked_transaction().ok()?;
    let global = &*snapshot;
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
    let inline_ordering_proven = composer
        .get("fullConversationHeadersOnly")
        .and_then(Value::as_array)
        .is_some_and(|headers| {
            !headers.is_empty()
                && headers.iter().all(|header| {
                    header
                        .get("bubbleId")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
                })
                && header_ids.iter().collect::<HashSet<_>>().len() == header_ids.len()
        });
    let header_ordering_proven = if let Some(metadata) = composer.get("headerLog") {
        match header_log::read_header_ids(global, composer_id, metadata) {
            Some(ids)
                if composer
                    .get("fullConversationHeadersOnly")
                    .is_none_or(|value| {
                        value.is_null()
                            || value.as_array().is_some_and(Vec::is_empty)
                            || (inline_ordering_proven && header_ids == ids)
                    }) =>
            {
                header_ids = ids;
                true
            }
            _ => false,
        }
    } else {
        inline_ordering_proven
    };
    let scope_ordering_proven = header_ordering_proven
        && composer
            .get("conversation")
            .is_none_or(|value| value.is_null() || value.as_array().is_some_and(Vec::is_empty))
        && composer.get("conversationMap").is_none_or(|value| {
            value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty)
        });

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
            "cursor_scope_ordering": if scope_ordering_proven { "recorded_headers" } else { "unproven" },
            "workspaceId": composer.get("workspaceId").and_then(Value::as_str),
            FORK_OBSERVATION_KEY: fork_observation,
        })
        .to_string(),
    );

    for bubble_id in header_ids {
        let Some(bubble) = bubbles.remove(&bubble_id) else {
            lines.push(json!({"type":"__missing_bubble__","bubbleId":bubble_id}).to_string());
            continue;
        };
        let Some(role) = cursor_role_for_value(&bubble) else {
            lines.push(json!({"type":"__unknown_bubble_role__","bubbleId":bubble_id}).to_string());
            continue;
        };
        if role == "assistant"
            && bubble.get("toolFormerData").is_some()
            && bubble.get("bubbleId").and_then(Value::as_str) != Some(bubble_id.as_str())
        {
            lines
                .push(json!({"type":"__invalid_tool_identity__","bubbleId":bubble_id}).to_string());
            continue;
        }
        let content = extract_cursor_bubble_text(&bubble);
        if content.is_none() && (role == "user" || bubble.get("toolFormerData").is_none()) {
            if role == "user" {
                lines.push(
                    json!({"type":"__unavailable_user_content__","bubbleId":bubble_id}).to_string(),
                );
            }
            continue;
        };
        lines.push(
            json!({
                "role": role,
                "content": content.unwrap_or_default(),
                "cursor_tool_result": cursor_native_tool_fields(&bubble),
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

fn cursor_native_tool_fields(bubble: &Value) -> Value {
    let Some(tool) = bubble.get("toolFormerData").and_then(Value::as_object) else {
        return Value::Null;
    };
    let mut fields = serde_json::Map::new();
    for key in ["name", "toolCallId", "status", "params", "result"] {
        if let Some(value) = tool.get(key).and_then(Value::as_str)
            && value.len() <= crate::analysis::MAX_CONTENT_PART_BYTES
        {
            fields.insert(key.into(), Value::String(value.into()));
        }
    }
    if let Some(status) = tool
        .get("additionalData")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .filter(|status| matches!(*status, "success" | "error"))
    {
        fields.insert("native_status".into(), Value::String(status.into()));
    }
    if !matches!(
        tool.get("name").and_then(Value::as_str),
        Some("read_file_v2" | "ripgrep_raw_search")
    ) {
        return Value::Object(fields);
    }
    let native_binding = (|| {
        let binary = tool
            .get("toolCallBinary")
            .and_then(Value::as_str)
            .filter(|binary| binary.len() <= 384 * 1024)
            .ok_or(DecodeError::Malformed)?;
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .ok_or(DecodeError::Malformed)?;
        let call_id = tool
            .get("toolCallId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= crate::analysis::EVIDENCE_STRING_CAP)
            .ok_or(DecodeError::UnsafeIdentity)?;
        let life_status = tool
            .get("status")
            .and_then(Value::as_str)
            .ok_or(DecodeError::Malformed)?;
        let params = tool
            .get("params")
            .and_then(Value::as_str)
            .filter(|params| params.len() <= crate::analysis::MAX_CONTENT_PART_BYTES)
            .ok_or(DecodeError::Malformed)?;
        let params = serde_json::from_str::<Value>(params).map_err(|_| DecodeError::Malformed)?;
        let status = decode_completed_call(name, call_id, life_status, &params, binary)?;
        Ok((name, call_id, status))
    })();
    match native_binding {
        Ok((name, call_id, status)) => {
            fields.insert("native_result_binding".into(), json!({
                "tool_name": name,
                "tool_call_id": call_id,
                "status": match status { NativeStatus::Success => "success", NativeStatus::Error => "error" },
                "completed": true,
                "args_verified": true,
            }));
        }
        Err(error) => {
            fields.insert(
                "native_result_binding_error".into(),
                Value::String(
                    match error {
                        DecodeError::UnsafeIdentity => "unsafe_identity",
                        DecodeError::Malformed => "malformed",
                    }
                    .into(),
                ),
            );
        }
    }
    Value::Object(fields)
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
        let prefix = format!("bubbleId:{composer_id}:");
        let Some(bubble_id) = row.0.strip_prefix(&prefix) else {
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
        if let Some(text) = bubble.get(key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return Some(text.to_string());
        }
    }

    if let Some(rich_text) = bubble.get("richText").and_then(Value::as_str)
        && serde_json::from_str::<Value>(rich_text).is_ok()
    {
        return Some(rich_text.to_owned());
    }

    if cursor_role_for_value(bubble) == Some("user") {
        return None;
    }

    for pointer in [
        "/toolFormerData/rawArgs",
        "/toolFormerData/result",
        "/toolFormerData/output",
        "/toolResult/output",
        "/errorDetails/message",
    ] {
        if let Some(text) = bubble.pointer(pointer).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return Some(text.to_string());
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
    if metadata
        .get(FORK_OBSERVATION_KEY)
        .is_some_and(|value| !value.is_null())
    {
        return;
    }
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
    if selected.workspace_hint != other.workspace_hint {
        return;
    }
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
        (Some(session_id), _) => session_id.clone(),
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
    if incoming.log.updated_at != existing.log.updated_at {
        return incoming.log.updated_at.unwrap_or_default()
            > existing.log.updated_at.unwrap_or_default();
    }
    incoming.log.source_label() < existing.log.source_label()
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
        let explicit_observation = cursor_store_db_subagent_observation(&self.metadata);
        let observation = explicit_observation.as_ref().or(fork_observation);
        let content = build_cursor_store_db_content(&self.metadata, &self.records, observation)?;
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
                    source_format: None,
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
        let (rows, unreadable) = sqlite_table_string_rows(&conn, table);
        if table == "meta" {
            meta_values.extend(rows.iter().map(|(_, value)| value.clone()));
        } else {
            records.extend(rows);
        }
        if table == "blobs" && unreadable {
            records.push((
                "cursor-unreadable-blob".to_owned(),
                r#"{"type":"__unreadable_blob__"}"#.to_owned(),
            ));
        }
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
    parent_session_id: Option<String>,
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
    let mut parent_session_id = None;
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
        parent_session_id = parent_session_id.or_else(|| {
            json.pointer("/subagentInfo/parentAgentId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| is_safe_session_id(id))
                .map(str::to_owned)
        });
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
        parent_session_id,
        cwd,
        model,
        title,
        created_at,
        updated_at,
    })
}

fn cursor_store_db_subagent_observation(
    metadata: &CursorStoreDbMetadata,
) -> Option<ForkObservation> {
    metadata
        .parent_session_id
        .as_ref()
        .filter(|parent_id| *parent_id != &metadata.session_id)
        .map(|parent_id| ForkObservation {
            parent_agent: "cursor".to_owned(),
            parent_agent_session_id: parent_id.clone(),
            fork_kind: "subagent".to_owned(),
            provider_fork_point_id: None,
            detection_source: "store_db_subagent_info".to_owned(),
            confidence: 100,
            inherited_item_count: None,
            extractor_version: "cursor-store-db-v1".to_owned(),
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
            "cursor_scope_ordering": "unproven",
            FORK_OBSERVATION_KEY: fork_observation,
        })
        .to_string(),
    ];
    for (blob_id, value) in records {
        let Ok(json) = serde_json::from_str::<Value>(value) else {
            lines.push(r#"{"type":"__unreadable_blob__"}"#.to_owned());
            continue;
        };
        if json.get("type").and_then(Value::as_str) == Some("__unreadable_blob__") {
            lines.push(json.to_string());
            continue;
        }
        let messages = extract_cursor_message_candidates(&json);
        if messages.is_empty() && json != json!({"messages": []}) {
            lines.push(
                json!({"type":"__unrecognized_store_blob__", "blobId":blob_id, "native":json})
                    .to_string(),
            );
        }
        for message in messages {
            lines.push(
                json!({
                    "role": message.role,
                    "content": message.content,
                    "timestamp": message.timestamp,
                    "model": message.model,
                    "bubbleId": message.record_id.as_deref().or_else(|| json.get("role").is_some().then_some(blob_id.as_str())),
                    "providerOptions": message.provider_options,
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
    content: Value,
    provider_options: Option<Value>,
    timestamp: Option<i64>,
    model: Option<String>,
    record_id: Option<String>,
}

fn extract_cursor_message_candidates(value: &Value) -> Vec<CursorMessageCandidate> {
    if let Some(message) = cursor_transport_message(value) {
        return vec![message];
    }
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    if map.len() != 1 {
        return Vec::new();
    }
    map.get("messages")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .map(cursor_transport_message)
                .collect::<Option<Vec<_>>>()
        })
        .unwrap_or_default()
}

fn cursor_transport_message(value: &Value) -> Option<CursorMessageCandidate> {
    let map = value.as_object()?;
    if map.contains_key("type") {
        return None;
    }
    let role = map.get("role")?.as_str()?;
    let content = map.get("content")?;
    if !matches!(role, "user" | "assistant" | "tool" | "system")
        || !(content.is_string() || content.is_array())
        || content.as_str().is_some_and(|text| text.trim().is_empty())
    {
        return None;
    }
    Some(CursorMessageCandidate {
        role: role.to_string(),
        content: content.clone(),
        provider_options: map.get("providerOptions").cloned(),
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
        record_id: ["bubbleId", "messageId", "id"]
            .into_iter()
            .find_map(|key| map.get(key).and_then(Value::as_str))
            .map(ToOwned::to_owned),
    })
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

fn sqlite_table_string_rows(conn: &Connection, table: &str) -> (Vec<(String, String)>, bool) {
    let mut out = Vec::new();
    let direct_sql = if table == "blobs" && conn.prepare("SELECT id, data FROM blobs").is_ok() {
        "SELECT id, CAST(data AS TEXT) FROM blobs ORDER BY rowid".to_owned()
    } else {
        format!("SELECT key, CAST(value AS TEXT) FROM {table} ORDER BY rowid")
    };
    if let Ok(mut stmt) = conn.prepare(&direct_sql)
        && let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
    {
        let mut unreadable = false;
        for row in rows {
            let Ok((key, value)) = row else {
                unreadable = true;
                continue;
            };
            // Cursor stores may contain binary or truncated blobs. Keep the
            // readable rows, but retain a partial marker when a relevant row
            // cannot be decoded.
            if looks_like_jsonish(&value) {
                out.push((key, value));
            } else if let Some(decoded) = decode_hex_jsonish(&value) {
                out.push((key, decoded));
            } else {
                unreadable = true;
            }
        }
        if !out.is_empty() {
            return (out, unreadable);
        }
    }

    let pragma = format!("PRAGMA table_info({table})");
    let Ok(mut info_stmt) = conn.prepare(&pragma) else {
        return (out, true);
    };
    let Ok(columns) = info_stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return (out, true);
    };
    let columns = columns.flatten().collect::<Vec<_>>();
    if columns.is_empty() {
        return (out, true);
    }
    let sql = format!("SELECT * FROM {table}");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return (out, true);
    };
    let Ok(mut rows) = stmt.query([]) else {
        return (out, true);
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
    (out, true)
}

#[cfg(test)]
#[path = "tests/cursor_tests.rs"]
mod tests;
