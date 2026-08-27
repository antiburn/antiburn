//! Codex sub-agent (orchestration) discovery.
//!
//! Native Codex differs from Claude on disk. Claude writes each spawned sub-agent as
//! its own transcript under a `<sessionId>/subagents/` subtree, so detection is
//! a directory walk. Codex instead records the parent→child spawn relationship
//! in its state database (`~/.codex/state_5.sqlite`), table `thread_spawn_edges`:
//!
//! ```sql
//! CREATE TABLE thread_spawn_edges (
//!     parent_thread_id TEXT NOT NULL,
//!     child_thread_id  TEXT NOT NULL PRIMARY KEY,
//!     status           TEXT NOT NULL
//! );
//! ```
//!
//! Every thread — parent or spawned child — still gets its own rollout file in
//! the flat `~/.codex/sessions/YYYY/MM/DD/` tree, and the child's path is
//! recorded in `threads.rollout_path`. So a sub-agent looks like an ordinary
//! top-level rollout on disk; only the spawn edge marks it as a child. Two
//! consequences this module handles:
//!
//! 1. **Roster:** a session is an orchestrator when it has rows in
//!    `thread_spawn_edges` as `parent_thread_id`. We list its *direct* children
//!    (mirroring Claude's one-level roster), resolving each to its rollout via
//!    `threads.rollout_path` (falling back to a filename-suffix scan).
//! 2. **No leaking:** because children are ordinary rollouts in the same tree,
//!    discovery must drop any rollout whose thread id is a `child_thread_id`
//!    or whose `threads.thread_source` is `subagent` (see
//!    [`top_level_excluded_thread_ids`], used by `CodexExplorer::discover_recent`)
//!    so a sub-agent never surfaces as a top-level session or uploads.
//!
//! Mounted WSL stores are deliberately file-only: modern rollout
//! `parent_thread_id` / `source.subagent.thread_spawn` metadata supplies the
//! roster and labels, so discovery never opens `state_5.sqlite` through UNC.
//!
//! These helpers back the [`AgentExplorer`](crate::discovery::AgentExplorer) sub-agent
//! hooks on `CodexExplorer`, kept isolated here so `codex.rs` stays focused on
//! session/title discovery — mirroring [`claude_subagents`](super::claude_subagents).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};

use super::codex::{
    all_log_dirs, find_session_file_by_id, session_title_text, state_db_paths_resolved,
};
use crate::discovery::extract_json_string_field;

/// Max characters kept for a sub-agent label (matches `claude_subagents`).
const SUBAGENT_LABEL_MAX_CHARS: usize = 80;

/// List a parent session's spawned sub-agents as rollout paths, sorted for a
/// stable roster order. Empty when the parent spawned nothing (not an
/// orchestrator) or Codex's state DB is absent.
pub(super) async fn list_subagents(parent_transcript: &Path) -> Vec<PathBuf> {
    if !crate::platform::environment::DiscoveryEnvironment::from_mounted_path(parent_transcript)
        .is_native()
    {
        return list_subagents_from_rollouts(parent_transcript).await;
    }
    let dirs = all_log_dirs().await;
    list_subagents_in(parent_transcript, &state_db_paths_resolved(), &dirs).await
}

/// Resolve a single sub-agent's rollout by `(parent_transcript, subagent_id)`,
/// validating `subagent_id` and confirming the spawn edge actually links it to
/// this parent before returning a path (children share the flat rollout tree,
/// so an unrelated id must not resolve).
pub(super) async fn locate_subagent(
    parent_transcript: &Path,
    subagent_id: &str,
) -> Option<PathBuf> {
    if !crate::platform::environment::DiscoveryEnvironment::from_mounted_path(parent_transcript)
        .is_native()
    {
        return locate_subagent_from_rollouts(parent_transcript, subagent_id).await;
    }
    let dirs = all_log_dirs().await;
    locate_subagent_in(
        parent_transcript,
        subagent_id,
        &state_db_paths_resolved(),
        &dirs,
    )
    .await
}

/// The Codex thread UUID that identifies a sub-agent for deep-linking, parsed
/// from its rollout filename.
pub(super) fn subagent_id(path: &Path) -> Option<String> {
    thread_id_from_rollout_path(path)
}

/// A sub-agent's display label: the Codex session title (state DB → index →
/// rollout scan, via [`session_title_text`]), truncated; generic otherwise.
pub(super) async fn subagent_label(path: &Path) -> String {
    if !crate::platform::environment::DiscoveryEnvironment::from_mounted_path(path).is_native() {
        let origin = super::codex::rollout_origin(path).await;
        if let Some(label) = origin.nickname.or(origin.role) {
            return truncate_chars(label.trim(), SUBAGENT_LABEL_MAX_CHARS);
        }
        if let Some(id) = origin.thread_id.as_deref()
            && let Some(title) = super::codex::index_title_for_rollout(path, id).await
        {
            return truncate_chars(title.trim(), SUBAGENT_LABEL_MAX_CHARS);
        }
        let title = crate::discovery::scanner::parse_session_metadata(path)
            .await
            .title;
        return label_from_title(title);
    }
    let title = match thread_id_from_rollout_path(path) {
        Some(id) => session_title_text(&id).await,
        None => None,
    };
    label_from_title(title)
}

async fn list_subagents_from_rollouts(parent_transcript: &Path) -> Vec<PathBuf> {
    let parent = super::codex::rollout_origin(parent_transcript).await;
    let Some(parent_id) = parent.thread_id else {
        return Vec::new();
    };
    let mut children = Vec::new();
    for dir in super::codex::log_dirs_for_rollout(parent_transcript).await {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            let origin = super::codex::rollout_origin(&path).await;
            if origin.parent_thread_id.as_deref() == Some(parent_id.as_str()) {
                children.push(path);
            }
        }
    }
    children.sort();
    children.dedup();
    children
}

async fn locate_subagent_from_rollouts(
    parent_transcript: &Path,
    subagent_id: &str,
) -> Option<PathBuf> {
    if !is_valid_thread_id(subagent_id) {
        return None;
    }
    let parent_id = super::codex::rollout_origin(parent_transcript)
        .await
        .thread_id?;
    for path in list_subagents_from_rollouts(parent_transcript).await {
        let origin = super::codex::rollout_origin(&path).await;
        if origin.thread_id.as_deref() == Some(subagent_id)
            && origin.parent_thread_id.as_deref() == Some(parent_id.as_str())
        {
            return Some(path);
        }
    }
    None
}

/// Every thread id that must be excluded from top-level discovery/upload across
/// Codex's state DB(s). Includes regular spawned children from
/// `thread_spawn_edges.child_thread_id` and guardian/approval-review threads
/// that Codex marks directly as `threads.thread_source = 'subagent'` without a
/// spawn edge. Empty when Codex has no sub-agent rows (the common case) or the
/// table/DB is absent.
///
/// Exclusion is therefore **best-effort**: this reads a DB another process
/// writes, so a read that fails (rather than the long busy_timeout in
/// [`open_ro`] succeeding) yields an empty set and that tick's sub-agents could
/// surface as top-level sessions. The generous busy_timeout makes that window
/// small.
pub(super) async fn top_level_excluded_thread_ids() -> HashSet<String> {
    top_level_excluded_thread_ids_in(&state_db_paths_resolved()).await
}

async fn list_subagents_in(
    parent_transcript: &Path,
    db_paths: &[PathBuf],
    log_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    let Some(parent_id) = parent_thread_id(parent_transcript).await else {
        return Vec::new();
    };
    let rows = children_with_paths_in(&parent_id, db_paths).await;
    let mut out = Vec::new();
    for (child_id, db_path) in rows {
        if let Some(path) = db_path
            && tokio::fs::try_exists(&path).await.unwrap_or(false)
        {
            out.push(path);
            continue;
        }
        if let Some(path) = find_session_file_by_id(log_dirs, &child_id).await {
            out.push(path);
        }
    }
    out
}

async fn locate_subagent_in(
    parent_transcript: &Path,
    subagent_id: &str,
    db_paths: &[PathBuf],
    log_dirs: &[PathBuf],
) -> Option<PathBuf> {
    if !is_valid_thread_id(subagent_id) {
        return None;
    }
    let parent_id = parent_thread_id(parent_transcript).await?;
    if !edge_exists_in(&parent_id, subagent_id, db_paths).await {
        return None;
    }
    if let Some(path) = rollout_path_from_db_in(subagent_id, db_paths).await
        && tokio::fs::try_exists(&path).await.unwrap_or(false)
    {
        return Some(path);
    }
    find_session_file_by_id(log_dirs, subagent_id).await
}

/// Extract a Codex thread UUID from a rollout file path.
///
/// Rollouts are named `rollout-<ISO8601>-<uuid>.jsonl`, and the ISO timestamp
/// itself contains hyphens, so we can't split naively. A thread id is a UUID —
/// five hyphen-separated hex groups (8-4-4-4-12) — so we take the *trailing*
/// five groups of the file stem and validate their shape. We also anchor on the
/// `rollout-` prefix so a non-Codex file that merely *ends* in a UUID-shaped tail
/// isn't mistaken for a rollout. `None` when either check fails (non-Codex /
/// malformed names are ignored).
pub(super) fn thread_id_from_rollout_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if !stem.starts_with("rollout-") {
        return None;
    }
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 5 {
        return None;
    }
    let tail = &parts[parts.len() - 5..];
    const GROUP_LENS: [usize; 5] = [8, 4, 4, 4, 12];
    for (seg, want) in tail.iter().zip(GROUP_LENS.iter()) {
        if seg.len() != *want || !seg.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
    }
    Some(tail.join("-"))
}

/// Derive the parent's thread id from its transcript path, falling back to the
/// rollout's `session_meta` `payload.id` when the filename isn't UUID-shaped.
async fn parent_thread_id(parent_transcript: &Path) -> Option<String> {
    if let Some(id) = thread_id_from_rollout_path(parent_transcript) {
        return Some(id);
    }
    let content = tokio::fs::read_to_string(parent_transcript).await.ok()?;
    let first_line = content.lines().next()?;
    extract_json_string_field(first_line, "id").map(str::to_string)
}

/// Positive allowlist for a sub-agent id reaching a SQL lookup / filesystem
/// resolution: UUID characters only. Parameterized queries already prevent
/// injection; this also rejects path-traversal ids before any fs work.
fn is_valid_thread_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn label_from_title(title: Option<String>) -> String {
    if let Some(text) = title {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return truncate_chars(trimmed, SUBAGENT_LABEL_MAX_CHARS);
        }
    }
    "Sub-agent".to_string()
}

/// Truncate to at most `max` characters (not bytes — safe for UTF-8), appending
/// an ellipsis when clipped (matches `claude_subagents`).
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn open_ro(path: &Path) -> Option<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    // Codex Desktop holds a write lock on `state_5.sqlite` while it spawns
    // sub-agents — exactly when the leak-prevention read in
    // `top_level_excluded_thread_ids` matters most. A generous busy_timeout
    // makes a transient lock wait rather than fail (a failed read would let
    // that tick's sub-agents leak into discovery). Runs on the blocking pool,
    // so the wait is cheap.
    let _ = conn.busy_timeout(Duration::from_millis(1000));
    Some(conn)
}

/// Direct children of `parent_id` paired with their recorded `rollout_path`
/// (`None` when the `threads` row or path is absent). Listed across every state
/// DB, de-duplicated, ordered by child id for a stable roster.
async fn children_with_paths_in(
    parent_id: &str,
    db_paths: &[PathBuf],
) -> Vec<(String, Option<PathBuf>)> {
    let parent_id = parent_id.to_string();
    let db_paths = db_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        // Merge across state DBs into a map keyed by child id. The `BTreeMap`
        // gives a single globally-sorted roster (the per-DB `ORDER BY` would only
        // sort within each DB, leaving the concatenation unsorted), and on a
        // collision we prefer a `Some(rollout_path)` over a `None` so a DB that
        // recorded the path wins over one that didn't.
        let mut merged: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();
        for db in &db_paths {
            let Some(conn) = open_ro(db) else { continue };
            let Ok(mut stmt) = conn.prepare(
                "SELECT e.child_thread_id, t.rollout_path \
                 FROM thread_spawn_edges e \
                 LEFT JOIN threads t ON t.id = e.child_thread_id \
                 WHERE e.parent_thread_id = ?1",
            ) else {
                continue;
            };
            let mapped = stmt.query_map(params![parent_id], |row| {
                let id: String = row.get(0)?;
                let path: Option<String> = row.get(1)?;
                Ok((id, path))
            });
            if let Ok(rows) = mapped {
                for (id, path) in rows.flatten() {
                    let pb = path
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .map(PathBuf::from);
                    match merged.get_mut(&id) {
                        Some(existing) if existing.is_none() => *existing = pb,
                        Some(_) => {}
                        None => {
                            merged.insert(id, pb);
                        }
                    }
                }
            }
        }
        merged.into_iter().collect()
    })
    .await
    .unwrap_or_default()
}

async fn rollout_path_from_db_in(thread_id: &str, db_paths: &[PathBuf]) -> Option<PathBuf> {
    let thread_id = thread_id.to_string();
    let db_paths = db_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        for db in &db_paths {
            let Some(conn) = open_ro(db) else { continue };
            let path: Option<String> = conn
                .query_row(
                    "SELECT rollout_path FROM threads WHERE id = ?1 LIMIT 1",
                    params![thread_id],
                    |row| row.get(0),
                )
                .ok();
            if let Some(path) = path {
                let trimmed = path.trim();
                if !trimmed.is_empty() {
                    return Some(PathBuf::from(trimmed));
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

async fn edge_exists_in(parent_id: &str, child_id: &str, db_paths: &[PathBuf]) -> bool {
    let parent_id = parent_id.to_string();
    let child_id = child_id.to_string();
    let db_paths = db_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        for db in &db_paths {
            let Some(conn) = open_ro(db) else { continue };
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM thread_spawn_edges \
                     WHERE child_thread_id = ?1 AND parent_thread_id = ?2 LIMIT 1",
                    params![child_id, parent_id],
                    |row| row.get(0),
                )
                .ok();
            if found.is_some() {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

async fn top_level_excluded_thread_ids_in(db_paths: &[PathBuf]) -> HashSet<String> {
    let db_paths = db_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut out = HashSet::new();
        for db in &db_paths {
            let Some(conn) = open_ro(db) else { continue };
            insert_string_column_query(
                &conn,
                "SELECT child_thread_id FROM thread_spawn_edges",
                &mut out,
            );
            insert_string_column_query(
                &conn,
                "SELECT id FROM threads WHERE thread_source = 'subagent'",
                &mut out,
            );
        }
        out
    })
    .await
    .unwrap_or_default()
}

fn insert_string_column_query(conn: &Connection, sql: &str, out: &mut HashSet<String>) {
    let Ok(mut stmt) = conn.prepare(sql) else {
        return;
    };
    if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
        for id in rows.flatten() {
            out.insert(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const PARENT: &str = "019e6c24-5ad0-7610-8d65-fae1fd70b5d3";
    const CHILD_A: &str = "019e0000-0000-7000-8000-000000000001";
    const CHILD_B: &str = "019e0000-0000-7000-8000-000000000002";
    const GUARDIAN: &str = "019e0000-0000-7000-8000-000000000003";

    fn rollout_name(uuid: &str) -> String {
        format!("rollout-2026-05-28T11-13-02-{uuid}.jsonl")
    }

    /// Seed a Codex-shaped state DB with spawn edges and thread rollout paths.
    fn seed_spawn_db(path: &Path, edges: &[(&str, &str, &str)], threads: &[(&str, &str)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE thread_spawn_edges (\
                parent_thread_id TEXT NOT NULL, \
                child_thread_id TEXT NOT NULL PRIMARY KEY, \
                status TEXT NOT NULL); \
             CREATE TABLE threads (\
                id TEXT PRIMARY KEY, \
                rollout_path TEXT NOT NULL, \
                thread_source TEXT NOT NULL DEFAULT '');",
        )
        .unwrap();
        for (p, c, s) in edges {
            conn.execute(
                "INSERT INTO thread_spawn_edges (parent_thread_id, child_thread_id, status) \
                 VALUES (?1, ?2, ?3)",
                params![p, c, s],
            )
            .unwrap();
        }
        for (id, rollout) in threads {
            conn.execute(
                "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                params![id, rollout],
            )
            .unwrap();
        }
    }

    #[test]
    fn thread_id_parsed_from_rollout_filename() {
        let path = PathBuf::from(format!("/s/2026/05/28/{}", rollout_name(PARENT)));
        assert_eq!(thread_id_from_rollout_path(&path).as_deref(), Some(PARENT));
    }

    #[test]
    fn thread_id_none_for_non_uuid_names() {
        for name in [
            "notes.jsonl",
            "rollout-2026-05-28-session.jsonl",
            "abc.jsonl",
            // UUID-shaped tail but NOT a Codex rollout → rejected by the prefix
            // anchor, so a coincidental backup/export can't be parsed as a thread.
            "backup-2026-05-28T11-13-02-019e6c24-5ad0-7610-8d65-fae1fd70b5d3.jsonl",
        ] {
            assert!(
                thread_id_from_rollout_path(&PathBuf::from(name)).is_none(),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn children_merge_across_dbs_sorted_and_prefers_recorded_path() {
        // CHILD_B's edge is in db1 with NO path; the same edge in db2 has a path.
        // The merge must keep the Some(path) and sort the roster globally by id.
        let db1 = TempDir::new().unwrap();
        let db2 = TempDir::new().unwrap();
        let db1_path = db1.path().join("state_5.sqlite");
        let db2_path = db2.path().join("state_5.sqlite");
        seed_spawn_db(
            &db1_path,
            &[(PARENT, CHILD_B, "open"), (PARENT, CHILD_A, "open")],
            &[(CHILD_A, "/rollouts/a.jsonl")], // CHILD_B path absent in db1
        );
        seed_spawn_db(
            &db2_path,
            &[(PARENT, CHILD_B, "open")],
            &[(CHILD_B, "/rollouts/b.jsonl")], // db2 supplies CHILD_B's path
        );

        let rows = children_with_paths_in(PARENT, &[db1_path, db2_path]).await;
        // Globally sorted by child id (A before B), deduped to two entries.
        assert_eq!(
            rows.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec![CHILD_A.to_string(), CHILD_B.to_string()],
        );
        // CHILD_B's None-in-db1 was upgraded to db2's recorded path.
        let child_b = rows.iter().find(|(id, _)| id == CHILD_B).unwrap();
        assert_eq!(child_b.1.as_deref(), Some(Path::new("/rollouts/b.jsonl")));
    }

    #[test]
    fn is_valid_thread_id_rejects_traversal() {
        assert!(is_valid_thread_id(PARENT));
        assert!(!is_valid_thread_id(""));
        assert!(!is_valid_thread_id("../../etc/passwd"));
        assert!(!is_valid_thread_id("a/b"));
    }

    #[test]
    fn label_truncates_and_falls_back() {
        assert_eq!(
            label_from_title(Some("Investigate build".into())),
            "Investigate build"
        );
        assert_eq!(label_from_title(Some("   ".into())), "Sub-agent");
        assert_eq!(label_from_title(None), "Sub-agent");
        let long = "x".repeat(100);
        let label = label_from_title(Some(long));
        assert_eq!(label.chars().count(), SUBAGENT_LABEL_MAX_CHARS + 1); // + ellipsis
        assert!(label.ends_with('…'));
    }

    #[tokio::test]
    async fn list_subagents_resolves_children_via_db_path_and_fallback() {
        let tmp = TempDir::new().unwrap();
        let day = tmp.path().join("2026").join("05").join("28");
        tokio::fs::create_dir_all(&day).await.unwrap();

        let parent_path = day.join(rollout_name(PARENT));
        tokio::fs::write(&parent_path, "{}").await.unwrap();
        // CHILD_A resolved via threads.rollout_path; CHILD_B via filename fallback.
        let child_a_path = day.join(rollout_name(CHILD_A));
        let child_b_path = day.join(rollout_name(CHILD_B));
        tokio::fs::write(&child_a_path, "{}").await.unwrap();
        tokio::fs::write(&child_b_path, "{}").await.unwrap();

        let db = tmp.path().join("state_5.sqlite");
        seed_spawn_db(
            &db,
            &[
                (PARENT, CHILD_A, "completed"),
                (PARENT, CHILD_B, "completed"),
            ],
            // CHILD_A has a recorded rollout_path; CHILD_B intentionally absent.
            &[(CHILD_A, child_a_path.to_str().unwrap())],
        );

        let found = list_subagents_in(&parent_path, &[db], std::slice::from_ref(&day)).await;
        assert_eq!(found.len(), 2);
        assert!(found.contains(&child_a_path));
        assert!(found.contains(&child_b_path));
    }

    #[tokio::test]
    async fn list_subagents_empty_without_edges() {
        let tmp = TempDir::new().unwrap();
        let parent_path = tmp.path().join(rollout_name(PARENT));
        tokio::fs::write(&parent_path, "{}").await.unwrap();
        let db = tmp.path().join("state_5.sqlite");
        seed_spawn_db(&db, &[], &[]);
        assert!(
            list_subagents_in(&parent_path, &[db], &[tmp.path().to_path_buf()])
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn locate_subagent_resolves_valid_rejects_unrelated_and_traversal() {
        let tmp = TempDir::new().unwrap();
        let parent_path = tmp.path().join(rollout_name(PARENT));
        tokio::fs::write(&parent_path, "{}").await.unwrap();
        let child_path = tmp.path().join(rollout_name(CHILD_A));
        tokio::fs::write(&child_path, "{}").await.unwrap();

        let db = tmp.path().join("state_5.sqlite");
        seed_spawn_db(
            &db,
            &[(PARENT, CHILD_A, "completed")],
            &[(CHILD_A, child_path.to_str().unwrap())],
        );
        let dbs = [db];
        let dirs = [tmp.path().to_path_buf()];

        assert_eq!(
            locate_subagent_in(&parent_path, CHILD_A, &dbs, &dirs).await,
            Some(child_path),
        );
        // Unrelated id (no edge to this parent) and traversal id both reject.
        assert!(
            locate_subagent_in(&parent_path, CHILD_B, &dbs, &dirs)
                .await
                .is_none()
        );
        assert!(
            locate_subagent_in(&parent_path, "../../etc/passwd", &dbs, &dirs)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn top_level_excluded_thread_ids_returns_all_subagents() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("state_5.sqlite");
        seed_spawn_db(
            &db,
            &[(PARENT, CHILD_A, "completed"), (PARENT, CHILD_B, "running")],
            &[],
        );
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, thread_source) VALUES (?1, ?2, 'subagent')",
            params![GUARDIAN, "/rollouts/guardian.jsonl"],
        )
        .unwrap();

        let ids = top_level_excluded_thread_ids_in(&[db]).await;
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(CHILD_A));
        assert!(ids.contains(CHILD_B));
        assert!(ids.contains(GUARDIAN));
    }

    #[tokio::test]
    async fn top_level_excluded_thread_ids_empty_when_db_missing() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope.sqlite");
        assert!(
            top_level_excluded_thread_ids_in(&[missing])
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rollout_relationships_attach_only_children_with_an_explicit_parent() {
        let tmp = TempDir::new().unwrap();
        let sessions = tmp.path().join(".codex/sessions/2026/07/01");
        tokio::fs::create_dir_all(&sessions).await.unwrap();
        let parent = sessions.join(rollout_name(PARENT));
        let child = sessions.join(rollout_name(CHILD_A));
        let legacy = sessions.join(rollout_name(CHILD_B));
        tokio::fs::write(
            &parent,
            format!(r#"{{"type":"session_meta","payload":{{"id":"{PARENT}","source":"cli"}}}}"#),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &child,
            format!(r#"{{"type":"session_meta","payload":{{"id":"{CHILD_A}","parent_thread_id":"{PARENT}","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{PARENT}"}}}}}}}}}}"#),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &legacy,
            format!(r#"{{"type":"session_meta","payload":{{"id":"{CHILD_B}","source":{{"subagent":{{"legacy":true}}}}}}}}"#),
        )
        .await
        .unwrap();

        let children = list_subagents_from_rollouts(&parent).await;
        assert_eq!(children, vec![child]);
        assert!(!children.contains(&legacy));
    }
}
