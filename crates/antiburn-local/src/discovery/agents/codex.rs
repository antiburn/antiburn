// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Codex agent log discovery.
//!
//! Codex stores session logs under `~/.codex/sessions/`.
//! Unlike Claude Code, Codex session directories are not scoped to a
//! specific repo path -- all sessions live in a flat directory.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::discovery::scanner::{self, AgentKind, TitleSource};
use crate::discovery::{
    AgentExplorer, DirectSessionSource, ResolvedTitle, SessionLog, SessionSource, SurfacePaths,
    TitleLookupKind, extract_json_string_field, home_dir, recent_files_with_exts,
};
use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use serde_json::Value;

use crate::platform::environment::WslEnvironmentInfo;

/// Return ALL directories containing `.jsonl` files under `~/.codex/sessions/`.
///
/// Codex stores session logs in a date-based directory hierarchy:
/// `~/.codex/sessions/YYYY/MM/DD/*.jsonl`. This function recursively
/// traverses the hierarchy to find the leaf directories that actually
/// contain session log files.
///
/// Used by the `backfill` command to scan all sessions regardless of repo.
pub async fn all_log_dirs() -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    log_dirs_in(&home).await
}

pub struct CodexExplorer;

#[async_trait]
impl AgentExplorer for CodexExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let dirs = all_log_dirs().await;
        // Spawned sub-agents are ordinary rollouts in this same tree. Drop them
        // so they never surface as top-level sessions or upload (Claude gets
        // this for free via its `subagents/` subdir). Empty set → no-op in the
        // common no-sub-agent case.
        //
        // Note: `locate_session_source` also resolves ids through this method,
        // so a thread that is itself a sub-agent can't be resolved here. That's
        // correct for a leaf child (reached via `locate_subagent_source`), but
        // means a *nested* orchestrator (a child that also spawns its own
        // children) won't resolve as a parent — its roster would be empty.
        // Codex's default `max_depth` is 1 (no such middle nodes), and our
        // roster is one-level like Claude's, so this is a known limitation, not
        // a regression for the shipping case.
        let excluded_thread_ids = super::codex_subagents::top_level_excluded_thread_ids().await;
        recent_files_with_exts(&dirs, now, since_secs, &["jsonl"])
            .await
            .into_iter()
            .filter(|file| !is_excluded_subagent(&file.path, &excluded_thread_ids))
            .map(|file| SessionLog {
                agent_type: AgentKind::Codex,
                source: SessionSource::File(file.path),
                updated_at: Some(file.mtime_epoch),
                environment: Default::default(),
            })
            .collect()
    }

    /// Owns `~/.codex/**` plus `CODEX_HOME` overrides that still nest under
    /// `.codex/sessions/`. Bounded fragments avoid false hits like
    /// `.codex_backup` or `notes.codex.md`.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/.codex/`           → shared CLI + IDE extension tree
    /// - `.codex/sessions/`   → shared CLI + IDE extension tree
    ///
    /// Path alone is insufficient: the Codex VS Code / JetBrains extension
    /// writes into the same `~/.codex/sessions/**` tree as the CLI. Surface
    /// is disambiguated by the `session_meta.source` content marker, not by
    /// path. See `session_surface_label` below.
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/.codex/") || path_lower.contains(".codex/sessions/")
    }

    fn title_lookup_kind(&self) -> TitleLookupKind {
        TitleLookupKind::Direct
    }

    fn recover_session_id_from_path(&self, file: &Path) -> Option<String> {
        super::codex_subagents::thread_id_from_rollout_path(file)
    }

    /// Resolve from Codex's indexed stores without opening a rollout:
    /// `state_5.sqlite#threads.title`, then `session_index.jsonl#thread_name`.
    async fn indexed_session_title(&self, agent_session_id: &str) -> Option<ResolvedTitle> {
        if let Some(text) = session_title_from_sqlite(agent_session_id).await {
            return Some(ResolvedTitle::new(text, TitleSource::UserRename));
        }
        if let Some(text) = session_title_from_index(agent_session_id).await {
            return Some(ResolvedTitle::new(text, TitleSource::AiGenerated));
        }
        None
    }

    async fn indexed_session_titles(
        &self,
        agent_session_ids: &[String],
    ) -> HashMap<String, ResolvedTitle> {
        let Some(home) = home_dir() else {
            return HashMap::new();
        };
        let codex_home = std::env::var("CODEX_HOME").ok().map(PathBuf::from);
        indexed_session_titles_in(&home, codex_home.as_deref(), agent_session_ids).await
    }

    /// Resolve a Codex session title from Codex's own stores, in order of
    /// authority:
    ///
    /// 1. `~/.codex/state_5.sqlite` `threads.title` — Codex Desktop's source
    ///    of truth (the same field rendered in its own sidebar; user renames
    ///    land here). Tagged [`TitleSource::UserRename`].
    /// 2. `~/.codex/session_index.jsonl` `thread_name` — flat-file projection
    ///    that may carry the AI-rendered name. Tagged [`TitleSource::AiGenerated`].
    /// 3. Rollout JSONL scan — last-resort fallback for CLI-only users with
    ///    neither store. Source provenance flows through from the scanner.
    async fn session_title(&self, agent_session_id: &str) -> Option<ResolvedTitle> {
        if let Some(title) = self.indexed_session_title(agent_session_id).await {
            return Some(title);
        }

        let dirs = all_log_dirs().await;
        let path = find_session_file_by_id(&dirs, agent_session_id).await?;
        let metadata = scanner::parse_session_metadata(&path).await;
        let text = metadata.title?;
        let source = metadata.title_source.unwrap_or(TitleSource::Explicit);
        Some(ResolvedTitle::new(text, source))
    }

    /// Point query: every rollout embeds its thread id as the filename suffix
    /// (`rollout-<ts>-<id>.jsonl`), so a filename scan replaces the default
    /// full-tree discover (which also stats every file). Preserves
    /// `discover_recent`'s child-thread exclusion — a spawned sub-agent's
    /// rollout must not resolve as a top-level session (see the comment there).
    /// An excluded id returns `Missing` (authoritative: the fallback discover
    /// filters it out too, so there's nothing to fall back to), a by-id hit
    /// returns `Found`, and a miss returns `Unsupported` so manifest-less /
    /// rotated ids still fall back. The decision is factored into
    /// [`fast_locate`] so it's unit testable with an injected exclusion set and
    /// log dirs (no env / HOME redirection).
    async fn direct_session_source(&self, session_id: &str) -> DirectSessionSource {
        let excluded = super::codex_subagents::top_level_excluded_thread_ids().await;
        let dirs = all_log_dirs().await;
        match fast_locate(session_id, &excluded, &dirs).await {
            FastLocate::Excluded => DirectSessionSource::Missing,
            FastLocate::Found(path) => DirectSessionSource::Found(SessionSource::File(path)),
            FastLocate::Fallback => DirectSessionSource::Unsupported,
        }
    }

    // Codex CLI and the Codex Desktop app / IDE extension share the same
    // `~/.codex/sessions/**` rollout tree — path alone can't tell them
    // apart. Each rollout's first `session_meta` record carries the
    // distinguishing fields: `"source":"cli"` for the TUI/CLI, anything
    // else (`"vscode"`, …) for Desktop / IDE extension.
    // Codex's CLI and IDE/Desktop variants share `~/.codex/sessions/**`,
    // so `surface_paths` exposes the tree under `cli` only and per-session
    // disambiguation comes from the `session_meta.source` content marker.
    // Without content we return `"unknown"` rather than guess.
    fn session_surface_label(
        &self,
        _log: &SessionLog,
        content: Option<&str>,
        _home: &Path,
    ) -> &'static str {
        match content.and_then(session_meta_source) {
            Some("cli") => "cli",
            Some(_) => "ide_desktop",
            // No content available, or no `session_meta.source` field —
            // can't disambiguate from the slug alone.
            None => "unknown",
        }
    }

    /// Shared CLI + IDE/Desktop tree at `~/.codex/sessions/**`. Exposed only
    /// under `cli`; `session_surface_label` flips to `ide_desktop` via the
    /// `session_meta.source` content marker.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: vec![home.join(".codex").join("sessions")],
            ide_desktop: Vec::new(),
            mirror: Vec::new(),
        }
    }

    // ---- Orchestration: Codex records each spawned sub-agent as a row in the
    // `thread_spawn_edges` table of `state_5.sqlite` (parent → child), with the
    // child's own rollout in the flat `sessions/` tree. These hooks expose that
    // to the vendor-agnostic orchestration layer; the implementation lives in
    // the `codex_subagents` module.

    fn supports_subagents(&self) -> bool {
        true
    }

    async fn list_subagents(&self, parent_transcript: &Path) -> Vec<PathBuf> {
        super::codex_subagents::list_subagents(parent_transcript).await
    }

    async fn locate_subagent(
        &self,
        parent_transcript: &Path,
        subagent_id: &str,
    ) -> Option<PathBuf> {
        super::codex_subagents::locate_subagent(parent_transcript, subagent_id).await
    }

    fn subagent_id(&self, path: &Path) -> Option<String> {
        super::codex_subagents::subagent_id(path)
    }

    async fn subagent_label(&self, path: &Path) -> String {
        super::codex_subagents::subagent_label(path).await
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RolloutOrigin {
    /// Thread identity from `session_meta`, falling back to the filename UUID.
    pub thread_id: Option<String>,
    /// Explicit modern parent identity; absent for unattached legacy children.
    pub parent_thread_id: Option<String>,
    /// Whether Codex marks this rollout as spawned rather than user top-level.
    pub subagent: bool,
    /// Whether the source is an internal review/guardian/compaction rollout.
    pub internal: bool,
    /// Optional spawned-agent role used as a WSL-safe display label.
    pub role: Option<String>,
    /// Optional spawned-agent nickname, preferred over the generic role.
    pub nickname: Option<String>,
}

/// WSL Codex discovery is rollout-first and derives exclusions from the files
/// themselves. It intentionally never consults `state_5.sqlite` over UNC.
pub async fn discover_recent_in_wsl(
    info: &WslEnvironmentInfo,
    now: i64,
    since_secs: i64,
) -> Vec<SessionLog> {
    let dirs = log_dirs_in(&info.context.home).await;
    let files = recent_files_with_exts(&dirs, now, since_secs, &["jsonl"]).await;
    let mut origins = HashMap::with_capacity(files.len());
    for file in &files {
        origins.insert(file.path.clone(), rollout_origin(&file.path).await);
    }

    files
        .into_iter()
        .filter(|file| {
            origins
                .get(&file.path)
                .is_none_or(|origin| !origin.subagent && !origin.internal)
        })
        .map(|file| SessionLog {
            agent_type: AgentKind::Codex,
            source: SessionSource::File(file.path),
            updated_at: Some(file.mtime_epoch),
            environment: info.context.environment.clone(),
        })
        .collect()
}

/// Reads bounded first-line metadata from a rollout. Filename identity remains
/// the fail-open fallback for older, malformed, or concurrently written files.
pub(super) async fn rollout_origin(path: &Path) -> RolloutOrigin {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let fallback_id = super::codex_subagents::thread_id_from_rollout_path(path);
    let Ok(file) = tokio::fs::File::open(path).await else {
        return RolloutOrigin {
            thread_id: fallback_id,
            ..Default::default()
        };
    };
    let mut first_line = String::new();
    let mut reader = tokio::io::BufReader::new(file).take(256 * 1024);
    if reader.read_line(&mut first_line).await.is_err() {
        return RolloutOrigin {
            thread_id: fallback_id,
            ..Default::default()
        };
    }
    parse_rollout_origin(&first_line, fallback_id)
}

fn parse_rollout_origin(line: &str, fallback_id: Option<String>) -> RolloutOrigin {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return RolloutOrigin {
            thread_id: fallback_id,
            ..Default::default()
        };
    };
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return RolloutOrigin {
            thread_id: fallback_id,
            ..Default::default()
        };
    }
    let payload = value.get("payload").unwrap_or(&value);
    let source = payload.get("source").unwrap_or(&Value::Null);
    let spawn = source.pointer("/subagent/thread_spawn");
    let parent_thread_id = payload
        .get("parent_thread_id")
        .or_else(|| payload.get("parentThreadId"))
        .or_else(|| spawn.and_then(|value| value.get("parent_thread_id")))
        .or_else(|| spawn.and_then(|value| value.get("parentThreadId")))
        .and_then(Value::as_str)
        .filter(|id| is_valid_thread_id(id))
        .map(str::to_string);
    let source_text = serde_json::to_string(source)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let subagent = source.get("subagent").is_some()
        || source.as_str() == Some("subagent")
        || parent_thread_id.is_some();
    let internal = ["guardian", "review", "compact", "compaction"]
        .iter()
        .any(|marker| source_text.contains(marker));
    let role = spawn
        .and_then(|value| value.get("role").or_else(|| value.get("agent_role")))
        .or_else(|| source.pointer("/subagent/role"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let nickname = spawn
        .and_then(|value| {
            value
                .get("nickname")
                .or_else(|| value.get("agent_nickname"))
        })
        .or_else(|| source.pointer("/subagent/nickname"))
        .and_then(Value::as_str)
        .map(str::to_string);
    RolloutOrigin {
        thread_id: payload
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_valid_thread_id(id))
            .map(str::to_string)
            .or(fallback_id),
        parent_thread_id,
        subagent,
        internal,
        role,
        nickname,
    }
}

fn is_valid_thread_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// Recovers the owning `.codex` directory without consulting process-global
/// home variables, which keeps native and mounted WSL lookups isolated.
pub(super) fn codex_home_from_rollout(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.file_name().and_then(|name| name.to_str()) == Some("sessions"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// Enumerates rollout leaf directories beneath the same Codex home as `path`.
pub(super) async fn log_dirs_for_rollout(path: &Path) -> Vec<PathBuf> {
    let Some(codex_home) = codex_home_from_rollout(path) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    collect_dirs_with_jsonl(&codex_home.join("sessions"), &mut dirs).await;
    dirs.sort();
    dirs
}

/// Reads the latest matching title from the rollout's sibling session index.
/// This path-relative lookup is the WSL alternative to native state SQLite.
pub async fn index_title_for_rollout(path: &Path, session_id: &str) -> Option<String> {
    let codex_home = codex_home_from_rollout(path)?;
    read_session_title_from_index(&codex_home.join("session_index.jsonl"), session_id).await
}

/// Codex thread ids that should not appear in a top-level session list.
///
/// Exposed separately from discovery so a consumer holding records written by
/// an older version can filter them out after the fact.
pub async fn top_level_excluded_thread_ids() -> HashSet<String> {
    super::codex_subagents::top_level_excluded_thread_ids().await
}

/// True when `path` is a spawned sub-agent rollout — its thread id appears as a
/// known excluded Codex sub-agent id. Used to keep sub-agents out of top-level
/// discovery (see [`CodexExplorer::discover_recent`]). A path whose
/// thread id can't be parsed is never excluded (fail-open keeps real sessions).
fn is_excluded_subagent(path: &Path, child_ids: &HashSet<String>) -> bool {
    super::codex_subagents::thread_id_from_rollout_path(path)
        .is_some_and(|id| child_ids.contains(&id))
}

/// Extract the `payload.source` string from the first `session_meta` record
/// in a Codex rollout file. Returns `None` when the marker isn't present
/// (e.g. older rollout format, or the first line isn't a `session_meta`).
///
/// The session_meta record is always the first line of the rollout. The
/// shared substring extractor in [`crate::discovery::extract_json_string_field`] does
/// the field peek without a full JSON parse.
fn session_meta_source(content: &str) -> Option<&str> {
    let first_line = content.lines().next()?;
    if !first_line.contains("\"session_meta\"") {
        return None;
    }
    extract_json_string_field(first_line, "source")
}

/// Extract Codex-specific working-directory hints from rollout JSONL content.
///
/// This intentionally lives in the Codex adapter instead of the generic
/// scanner: Codex function-call arguments are often a JSON-encoded string, and
/// treating every agent's arbitrary string field that way would widen
/// cross-vendor path extraction too much.
pub fn repo_hint_cwds_from_transcript(content: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        collect_codex_repo_hints(&value, &mut seen, &mut candidates, 0, false);
    }

    candidates
}

const CODEX_REPO_HINT_MAX_DEPTH: usize = 16;

fn collect_codex_repo_hints(
    value: &serde_json::Value,
    seen: &mut HashSet<String>,
    candidates: &mut Vec<String>,
    depth: usize,
    in_arguments: bool,
) {
    if depth > CODEX_REPO_HINT_MAX_DEPTH {
        return;
    }

    match value {
        serde_json::Value::Object(map) => {
            if in_arguments {
                for key in ["cwd", "workdir", "working_directory"] {
                    if let Some(path) = map.get(key).and_then(|v| v.as_str()) {
                        add_dir_candidate(path, seen, candidates);
                    }
                }
                for key in ["cmd", "command"] {
                    if let Some(command) = map.get(key).and_then(|v| v.as_str()) {
                        collect_command_path_hints(command, seen, candidates);
                    }
                }
            }

            for (key, val) in map {
                let argument_value = matches!(key.as_str(), "arguments" | "input" | "params");
                if let Some(raw) = val.as_str()
                    && argument_value
                    && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw)
                {
                    collect_codex_repo_hints(&parsed, seen, candidates, depth + 1, true);
                    continue;
                }
                collect_codex_repo_hints(
                    val,
                    seen,
                    candidates,
                    depth + 1,
                    in_arguments || argument_value,
                );
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_codex_repo_hints(item, seen, candidates, depth + 1, in_arguments);
            }
        }
        _ => {}
    }
}

fn collect_command_path_hints(
    command: &str,
    seen: &mut HashSet<String>,
    candidates: &mut Vec<String>,
) {
    for segment in command.split("&&").flat_map(|part| part.split(';')) {
        let trimmed = segment.trim();
        if let Some(rest) = trimmed.strip_prefix("cd ") {
            let dir = rest.trim().trim_matches('"').trim_matches('\'');
            add_dir_candidate(dir, seen, candidates);
        }
    }

    for token in command.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
            )
    }) {
        let token = token
            .trim()
            .trim_matches(':')
            .trim_end_matches([':', ',', '.']);
        add_path_candidate(token, seen, candidates);
    }
}

fn add_path_candidate(path: &str, seen: &mut HashSet<String>, candidates: &mut Vec<String>) {
    if !is_absolute_path(path) {
        return;
    }
    add_dir_candidate(path, seen, candidates);

    let path = Path::new(path);
    if looks_like_file(path)
        && let Some(parent) = path.parent()
    {
        add_dir_candidate(&parent.to_string_lossy(), seen, candidates);
    }
}

fn add_dir_candidate(dir: &str, seen: &mut HashSet<String>, candidates: &mut Vec<String>) {
    if is_absolute_path(dir) && seen.insert(dir.to_string()) {
        candidates.push(dir.to_string());
    }
}

fn is_absolute_path(path: &str) -> bool {
    path.starts_with('/')
        || path.as_bytes().get(1) == Some(&b':')
            && path
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic())
}

fn looks_like_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains('.'))
}

/// Internal: find Codex session directories under a given home directory.
///
/// Recursively traverses `~/.codex/sessions/` to find directories that
/// contain `.jsonl` files, since Codex uses a date-based hierarchy
/// (YYYY/MM/DD/) rather than flat directories.
///
/// Separated from `log_dirs` for testability -- tests pass a temp directory
/// instead of the real home, avoiding `unsafe` env var manipulation.
async fn log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let codex_home = std::env::var("CODEX_HOME").ok().map(PathBuf::from);
    log_dirs_in_with_codex_home(home, codex_home.as_deref()).await
}

async fn log_dirs_in_with_codex_home(home: &Path, codex_home: Option<&Path>) -> Vec<PathBuf> {
    let mut session_roots = vec![home.join(".codex").join("sessions")];
    if let Some(custom_home) = codex_home {
        let candidate = custom_home.join("sessions");
        if !session_roots.contains(&candidate) {
            session_roots.push(candidate);
        }
    }

    let mut dirs = Vec::new();
    for root in &session_roots {
        collect_dirs_with_jsonl(root, &mut dirs).await;
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    home.join(".codex")
        .join("sessions")
        .join("2026")
        .join("01")
        .join("01")
        .join("rollout-2026-01-01-abc.jsonl")
}

/// Recursively collect directories that contain at least one `.jsonl` file.
/// The whole walk runs as one `spawn_blocking` task (mirroring
/// [`crate::discovery::collect_dirs_with_exts`]): the sub-agent badge path resolves Codex
/// ids through `direct_session_source` -> `all_log_dirs`, so a deep
/// `~/.codex/sessions/**` tree must not spawn a blocking micro-task per
/// readdir/stat — that was the pool-spawner contention this fix removes.
async fn collect_dirs_with_jsonl(root: &Path, results: &mut Vec<PathBuf>) {
    let root = root.to_path_buf();
    let found = tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };

            let mut has_jsonl = false;
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file()
                    && !has_jsonl
                    && path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                {
                    has_jsonl = true;
                }
            }

            if has_jsonl {
                found.push(dir);
            }
        }
        found
    })
    .await
    .unwrap_or_default();

    results.extend(found);
}

async fn session_title_from_sqlite(session_id: &str) -> Option<String> {
    let home = home_dir()?;
    let codex_home = std::env::var("CODEX_HOME").ok().map(PathBuf::from);
    session_title_from_sqlite_in(&home, codex_home.as_deref(), session_id).await
}

async fn session_title_from_sqlite_in(
    home: &Path,
    codex_home: Option<&Path>,
    session_id: &str,
) -> Option<String> {
    for path in state_db_paths(home, codex_home) {
        if let Some(title) = read_session_title_from_sqlite(&path, session_id).await {
            return Some(title);
        }
    }
    None
}

/// Filename is pinned to the schema version Codex Desktop ships today
/// (`state_5.sqlite`). If a future release writes `state_6.sqlite`, this
/// lookup misses and `session_title` falls through to the JSONL index plus
/// rollout parser — preferable to silently reading from an unverified schema.
fn state_db_paths(home: &Path, codex_home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![home.join(".codex").join("state_5.sqlite")];
    if let Some(custom_home) = codex_home {
        let candidate = custom_home.join("state_5.sqlite");
        if !paths.contains(&candidate) {
            paths.push(candidate);
        }
    }
    paths
}

/// The state-DB paths resolved against the real home + `CODEX_HOME` override.
/// Exposed for sub-agent (orchestration) lookups, which read the same
/// `state_5.sqlite` for `thread_spawn_edges` / `threads.rollout_path`.
pub(super) fn state_db_paths_resolved() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let codex_home = std::env::var("CODEX_HOME").ok().map(PathBuf::from);
    state_db_paths(&home, codex_home.as_deref())
}

/// A Codex session's display title text (no provenance), resolved with the same
/// store precedence as [`CodexExplorer::session_title`]: state DB → index →
/// rollout scan. Exposed for sub-agent label resolution.
pub(super) async fn session_title_text(session_id: &str) -> Option<String> {
    if let Some(text) = session_title_from_sqlite(session_id).await {
        return Some(text);
    }
    if let Some(text) = session_title_from_index(session_id).await {
        return Some(text);
    }
    let dirs = all_log_dirs().await;
    let path = find_session_file_by_id(&dirs, session_id).await?;
    scanner::parse_session_metadata(&path).await.title
}

async fn read_session_title_from_sqlite(path: &Path, session_id: &str) -> Option<String> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return None;
    }
    let path = path.to_path_buf();
    let session_id = session_id.to_string();
    tokio::task::spawn_blocking(move || query_thread_title(&path, &session_id))
        .await
        .ok()
        .flatten()
}

fn query_thread_title(path: &Path, session_id: &str) -> Option<String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_millis(200));
    let title: String = conn
        .query_row(
            "SELECT title FROM threads WHERE id = ?1 LIMIT 1",
            params![session_id],
            |row| row.get(0),
        )
        .ok()?;
    let trimmed = title.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn query_thread_titles(path: &Path, session_ids: &[String]) -> HashMap<String, String> {
    let Ok(conn) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return HashMap::new();
    };
    let _ = conn.busy_timeout(Duration::from_millis(200));
    let Ok(mut statement) = conn.prepare("SELECT title FROM threads WHERE id = ?1 LIMIT 1") else {
        return HashMap::new();
    };
    let mut titles = HashMap::new();
    for session_id in session_ids {
        let Ok(title) = statement.query_row(params![session_id], |row| row.get::<_, String>(0))
        else {
            continue;
        };
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            titles.insert(session_id.clone(), trimmed.to_string());
        }
    }
    titles
}

async fn session_title_from_index(session_id: &str) -> Option<String> {
    let home = home_dir()?;
    let codex_home = std::env::var("CODEX_HOME").ok().map(PathBuf::from);
    session_title_from_index_in(&home, codex_home.as_deref(), session_id).await
}

async fn session_title_from_index_in(
    home: &Path,
    codex_home: Option<&Path>,
    session_id: &str,
) -> Option<String> {
    for path in session_index_paths(home, codex_home) {
        if let Some(title) = read_session_title_from_index(&path, session_id).await {
            return Some(title);
        }
    }
    None
}

fn session_index_paths(home: &Path, codex_home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![home.join(".codex").join("session_index.jsonl")];
    if let Some(custom_home) = codex_home {
        let candidate = custom_home.join("session_index.jsonl");
        if !paths.contains(&candidate) {
            paths.push(candidate);
        }
    }
    paths
}

#[derive(Deserialize)]
struct SessionIndexRow {
    id: String,
    thread_name: Option<String>,
}

async fn read_session_title_from_index(path: &Path, session_id: &str) -> Option<String> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    let mut latest_title = None;

    for line in content.lines() {
        let Ok(row) = serde_json::from_str::<SessionIndexRow>(line) else {
            continue;
        };
        if row.id != session_id {
            continue;
        }

        latest_title = row
            .thread_name
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty());
    }

    latest_title
}

async fn indexed_session_titles_in(
    home: &Path,
    codex_home: Option<&Path>,
    session_ids: &[String],
) -> HashMap<String, ResolvedTitle> {
    let mut resolved = HashMap::new();
    for path in state_db_paths(home, codex_home) {
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            continue;
        }
        let ids = session_ids.to_vec();
        let titles = tokio::task::spawn_blocking(move || query_thread_titles(&path, &ids))
            .await
            .unwrap_or_default();
        for (session_id, text) in titles {
            resolved
                .entry(session_id)
                .or_insert_with(|| ResolvedTitle::new(text, TitleSource::UserRename));
        }
    }

    let wanted: HashSet<String> = session_ids.iter().cloned().collect();
    for path in session_index_paths(home, codex_home) {
        let wanted = wanted.clone();
        let latest = tokio::task::spawn_blocking(move || query_index_titles(&path, &wanted))
            .await
            .unwrap_or_default();
        for (session_id, text) in latest {
            resolved
                .entry(session_id)
                .or_insert_with(|| ResolvedTitle::new(text, TitleSource::AiGenerated));
        }
    }
    resolved
}

fn query_index_titles(path: &Path, wanted: &HashSet<String>) -> HashMap<String, String> {
    let Ok(file) = std::fs::File::open(path) else {
        return HashMap::new();
    };
    let mut latest = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(row) = serde_json::from_str::<SessionIndexRow>(&line) else {
            continue;
        };
        if !wanted.contains(&row.id) {
            continue;
        }
        let title = row
            .thread_name
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty());
        match title {
            Some(title) => {
                latest.insert(row.id, title);
            }
            None => {
                latest.remove(&row.id);
            }
        }
    }
    latest
}

/// Find the rollout file for a given Codex session UUID by filename suffix.
///
/// Searches every dir in `dirs` (non-recursive — caller already collected the
/// leaf dirs via [`all_log_dirs`]) and returns the first file whose name ends
/// with `-{session_id}.jsonl`.
pub(super) async fn find_session_file_by_id(dirs: &[PathBuf], session_id: &str) -> Option<PathBuf> {
    let suffix = format!("-{session_id}.jsonl");
    let dirs = dirs.to_vec();
    // One blocking task for the whole filename scan, not one per readdir step.
    tokio::task::spawn_blocking(move || {
        for dir in &dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let matches = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(&suffix))
                    .unwrap_or(false);
                if matches {
                    return Some(path);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Outcome of the [`CodexExplorer::direct_session_source`] fast path, kept
/// distinct so an *excluded child* (definitive miss, must NOT fall back) can't
/// be confused with a *fast-path miss* (fall back to the full discover).
#[derive(Debug)]
enum FastLocate {
    /// `session_id` is a spawned sub-agent's own id — never a top-level session.
    Excluded,
    /// Resolved directly to a rollout file by id.
    Found(PathBuf),
    /// Not excluded and no rollout matched by id — caller should fall back.
    Fallback,
}

/// Testable core of `direct_session_source`'s fast path: exclusion check first
/// (so a child id resolves to a definitive miss), then a by-id rollout lookup
/// in `dirs`. Takes the exclusion set and dirs by reference so tests inject a
/// seeded set + temp dir without touching the real home. `excluded` is the same
/// `thread_spawn_edges` child-id set `discover_recent` filters with, so the two
/// paths agree on what counts as a top-level session.
async fn fast_locate(session_id: &str, excluded: &HashSet<String>, dirs: &[PathBuf]) -> FastLocate {
    if excluded.contains(session_id) {
        return FastLocate::Excluded;
    }
    match find_session_file_by_id(dirs, session_id).await {
        Some(path) => FastLocate::Found(path),
        None => FastLocate::Fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn modern_rollout_origin_reads_parent_role_and_nickname() {
        let line = r#"{"type":"session_meta","payload":{"id":"019e0000-0000-7000-8000-000000000001","source":{"subagent":{"thread_spawn":{"parent_thread_id":"019e0000-0000-7000-8000-000000000000","agent_role":"explorer","agent_nickname":"Scout"}}}}}"#;
        let origin = parse_rollout_origin(line, None);
        assert_eq!(
            origin.parent_thread_id.as_deref(),
            Some("019e0000-0000-7000-8000-000000000000")
        );
        assert_eq!(origin.role.as_deref(), Some("explorer"));
        assert_eq!(origin.nickname.as_deref(), Some("Scout"));
        assert!(origin.subagent);
        assert!(!origin.internal);
    }

    #[test]
    fn unparented_legacy_subagent_stays_unattached_and_internal_sources_are_hidden() {
        let child = parse_rollout_origin(
            r#"{"type":"session_meta","payload":{"id":"child","source":{"subagent":{"legacy":true}}}}"#,
            None,
        );
        assert!(child.subagent);
        assert!(child.parent_thread_id.is_none());

        for source in ["guardian", "review", "compact"] {
            let line = format!(
                r#"{{"type":"session_meta","payload":{{"id":"internal","source":"{source}"}}}}"#
            );
            assert!(parse_rollout_origin(&line, None).internal, "{source}");
        }
    }

    #[tokio::test]
    async fn originating_session_index_uses_latest_name() {
        let home = TempDir::new().unwrap();
        let codex = home.path().join(".codex");
        let rollout = codex
            .join("sessions/2026/07/01")
            .join("rollout-2026-07-01T00-00-00-019e0000-0000-7000-8000-000000000001.jsonl");
        tokio::fs::create_dir_all(rollout.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            codex.join("session_index.jsonl"),
            "{\"id\":\"thread\",\"thread_name\":\"Old\"}\n{\"id\":\"thread\",\"thread_name\":\"Latest\"}\n",
        )
        .await
        .unwrap();
        assert_eq!(
            index_title_for_rollout(&rollout, "thread").await.as_deref(),
            Some("Latest")
        );
    }

    /// The child-thread exclusion that `discover_recent` applies must also
    /// hold on the `direct_session_source` fast path: a spawned sub-agent's own
    /// id resolves to `Excluded` (a definitive miss, no fallback), while a
    /// top-level id resolves straight to its rollout, and an unrelated id falls
    /// back to the full discover. Guards the regression the storm fix was
    /// warned against.
    #[tokio::test]
    async fn fast_locate_excludes_children_and_finds_top_level() {
        let day = TempDir::new().unwrap();
        let top = "019e0000-0000-7000-8000-00000000top";
        let child = "019e0000-0000-7000-8000-000000child";
        let top_path = day
            .path()
            .join(format!("rollout-2026-05-28T11-13-02-{top}.jsonl"));
        let child_path = day
            .path()
            .join(format!("rollout-2026-05-28T11-13-02-{child}.jsonl"));
        tokio::fs::write(&top_path, "{}").await.unwrap();
        tokio::fs::write(&child_path, "{}").await.unwrap();

        let excluded: HashSet<String> = std::iter::once(child.to_string()).collect();
        let dirs = [day.path().to_path_buf()];

        // Child id is excluded even though its rollout is present on disk.
        assert!(matches!(
            fast_locate(child, &excluded, &dirs).await,
            FastLocate::Excluded
        ));
        // Top-level id resolves to its rollout via the fast path.
        assert!(matches!(
            fast_locate(top, &excluded, &dirs).await,
            FastLocate::Found(p) if p == top_path
        ));
        // Unknown id (not excluded, no rollout) defers to the full discover.
        assert!(matches!(
            fast_locate("019e0000-0000-7000-8000-0000unknown", &excluded, &dirs).await,
            FastLocate::Fallback
        ));
    }

    #[tokio::test]
    async fn test_log_dirs_finds_directories_with_jsonl_files() {
        let home = TempDir::new().unwrap();
        let sessions_dir = home.path().join(".codex").join("sessions");

        let day1 = sessions_dir.join("2026").join("02").join("10");
        let day2 = sessions_dir.join("2026").join("02").join("09");
        tokio::fs::create_dir_all(&day1).await.unwrap();
        tokio::fs::create_dir_all(&day2).await.unwrap();
        tokio::fs::write(day1.join("session-abc.jsonl"), "{}")
            .await
            .unwrap();
        tokio::fs::write(day2.join("session-def.jsonl"), "{}")
            .await
            .unwrap();

        let result = log_dirs_in(home.path()).await;

        assert_eq!(result.len(), 2);
        assert!(result.contains(&day1));
        assert!(result.contains(&day2));
    }

    #[tokio::test]
    async fn test_log_dirs_skips_empty_directories() {
        let home = TempDir::new().unwrap();
        let sessions_dir = home.path().join(".codex").join("sessions");

        // Directory with jsonl file
        let day1 = sessions_dir.join("2026").join("02").join("10");
        tokio::fs::create_dir_all(&day1).await.unwrap();
        tokio::fs::write(day1.join("session.jsonl"), "{}")
            .await
            .unwrap();

        // Empty directory (no jsonl files)
        let day2 = sessions_dir.join("2026").join("02").join("09");
        tokio::fs::create_dir_all(&day2).await.unwrap();

        let result = log_dirs_in(home.path()).await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], day1);
    }

    #[tokio::test]
    async fn test_log_dirs_returns_empty_when_sessions_dir_missing() {
        let home = TempDir::new().unwrap();
        // Don't create .codex/sessions/

        let result = log_dirs_in(home.path()).await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_log_dirs_ignores_non_jsonl_files() {
        let home = TempDir::new().unwrap();
        let sessions_dir = home.path().join(".codex").join("sessions");
        let day_dir = sessions_dir.join("2026").join("01").join("01");
        tokio::fs::create_dir_all(&day_dir).await.unwrap();

        // Only a .txt file, no .jsonl
        tokio::fs::write(day_dir.join("not-a-session.txt"), "content")
            .await
            .unwrap();

        let result = log_dirs_in(home.path()).await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_log_dirs_finds_flat_and_nested_dirs() {
        let home = TempDir::new().unwrap();
        let sessions_dir = home.path().join(".codex").join("sessions");

        // Nested date directory
        let nested = sessions_dir.join("2026").join("02").join("10");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        tokio::fs::write(nested.join("session.jsonl"), "{}")
            .await
            .unwrap();

        // Flat directory at top level (in case Codex format changes)
        let flat = sessions_dir.join("flat-session");
        tokio::fs::create_dir_all(&flat).await.unwrap();
        tokio::fs::write(flat.join("session.jsonl"), "{}")
            .await
            .unwrap();

        let result = log_dirs_in(home.path()).await;

        assert_eq!(result.len(), 2);
        assert!(result.contains(&nested));
        assert!(result.contains(&flat));
    }

    #[tokio::test]
    async fn test_log_dirs_graceful_when_codex_dir_missing() {
        // If ~/.codex/ does not exist at all, log_dirs should return
        // an empty Vec, not error.
        let home = TempDir::new().unwrap();
        // Don't create .codex/ at all

        let result = log_dirs_in(home.path()).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_log_dirs_empty_sessions_directory() {
        let home = TempDir::new().unwrap();
        let sessions_dir = home.path().join(".codex").join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await.unwrap();
        // Sessions dir exists but is empty

        let result = log_dirs_in(home.path()).await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_log_dirs_supports_codex_home_override() {
        let home = TempDir::new().unwrap();
        let codex_home = TempDir::new().unwrap();
        let override_day = codex_home
            .path()
            .join("sessions")
            .join("2026")
            .join("02")
            .join("10");
        tokio::fs::create_dir_all(&override_day).await.unwrap();
        tokio::fs::write(override_day.join("zed-session.jsonl"), "{}")
            .await
            .unwrap();

        let result = log_dirs_in_with_codex_home(home.path(), Some(codex_home.path())).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], override_day);
    }

    #[tokio::test]
    async fn test_log_dirs_dedupes_default_and_override_roots() {
        let home = TempDir::new().unwrap();
        let shared_home = home.path().join(".codex");
        let day = shared_home
            .join("sessions")
            .join("2026")
            .join("02")
            .join("10");
        tokio::fs::create_dir_all(&day).await.unwrap();
        tokio::fs::write(day.join("session.jsonl"), "{}")
            .await
            .unwrap();

        let result = log_dirs_in_with_codex_home(home.path(), Some(&shared_home)).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], day);
    }

    #[test]
    fn codex_repo_hints_extract_stringified_arguments_workdir() {
        let content = r#"{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"pnpm test\",\"workdir\":\"/Users/foo/Developer/Workspace/core-api\"}"}"#;

        let candidates = repo_hint_cwds_from_transcript(content);

        assert_eq!(
            candidates,
            vec!["/Users/foo/Developer/Workspace/core-api".to_string()]
        );
    }

    #[test]
    fn codex_repo_hints_work_for_cli_rollout_arguments() {
        let content = [
            r#"{"type":"session_meta","payload":{"id":"sid","cwd":"/Users/foo/Developer","source":"cli"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":\"cargo test\",\"workdir\":\"/Users/foo/Developer/Workspace/core-api\"}"}}"#,
        ]
        .join("\n");

        let candidates = repo_hint_cwds_from_transcript(&content);

        assert_eq!(
            candidates,
            vec!["/Users/foo/Developer/Workspace/core-api".to_string()]
        );
    }

    #[test]
    fn codex_repo_hints_extract_cmd_absolute_file_parent() {
        let content = r#"{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"sed -n '1,80p' /Users/foo/Developer/Workspace/core-ui/src/App.tsx\"}"}"#;

        let candidates = repo_hint_cwds_from_transcript(content);

        assert_eq!(
            candidates,
            vec![
                "/Users/foo/Developer/Workspace/core-ui/src/App.tsx".to_string(),
                "/Users/foo/Developer/Workspace/core-ui/src".to_string(),
            ]
        );
    }

    #[test]
    fn codex_repo_hints_preserve_dotted_command_directory() {
        let content = r#"{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"ls /Users/foo/Developer/my.repo\"}"}"#;

        let candidates = repo_hint_cwds_from_transcript(content);

        assert_eq!(
            candidates,
            vec![
                "/Users/foo/Developer/my.repo".to_string(),
                "/Users/foo/Developer".to_string(),
            ]
        );
    }

    #[test]
    fn codex_repo_hints_preserve_dotted_workdir_directory() {
        let content = r#"{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"git status --short\",\"workdir\":\"/Users/foo/Developer/my.repo\"}"}"#;

        let candidates = repo_hint_cwds_from_transcript(content);

        assert_eq!(candidates, vec!["/Users/foo/Developer/my.repo".to_string()]);
    }

    #[test]
    fn codex_repo_hints_empty_without_repo_evidence_for_cli_and_app() {
        let content = [
            r#"{"type":"session_meta","payload":{"id":"cli-sid","cwd":"/Users/foo/Developer","source":"cli"}}"#,
            r#"{"type":"session_meta","payload":{"id":"app-sid","cwd":"/Users/foo/Developer","source":"vscode"}}"#,
        ]
        .join("\n");

        let candidates = repo_hint_cwds_from_transcript(&content);

        assert!(candidates.is_empty());
    }

    fn seed_state_db(path: &Path, rows: &[(&str, &str)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT NOT NULL);")
            .unwrap();
        for (id, title) in rows {
            conn.execute(
                "INSERT INTO threads (id, title) VALUES (?1, ?2)",
                params![id, title],
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn test_session_title_from_sqlite_resolves_user_rename() {
        let home = TempDir::new().unwrap();
        let codex_dir = home.path().join(".codex");
        tokio::fs::create_dir_all(&codex_dir).await.unwrap();
        seed_state_db(
            &codex_dir.join("state_5.sqlite"),
            &[("11111111-aaaa-bbbb-cccc-222222222222", "renamed by user")],
        );
        let title =
            session_title_from_sqlite_in(home.path(), None, "11111111-aaaa-bbbb-cccc-222222222222")
                .await;
        assert_eq!(title.as_deref(), Some("renamed by user"));
    }

    #[tokio::test]
    async fn test_session_title_from_sqlite_skips_blank_titles() {
        let home = TempDir::new().unwrap();
        let codex_dir = home.path().join(".codex");
        tokio::fs::create_dir_all(&codex_dir).await.unwrap();
        seed_state_db(
            &codex_dir.join("state_5.sqlite"),
            &[("33333333-aaaa-bbbb-cccc-444444444444", "   ")],
        );
        let title =
            session_title_from_sqlite_in(home.path(), None, "33333333-aaaa-bbbb-cccc-444444444444")
                .await;
        assert!(title.is_none());
    }

    #[test]
    fn explorer_recovers_the_canonical_thread_id_from_a_rollout_path() {
        let session_id = "01a01251-9875-7121-ac24-0d99fd8ccbe1";
        let path = PathBuf::from(format!(
            "/home/avery/.codex/sessions/2026/08/18/rollout-2026-08-18T10-42-12-{session_id}.jsonl"
        ));

        assert_eq!(
            CodexExplorer.recover_session_id_from_path(&path).as_deref(),
            Some(session_id)
        );
    }

    #[tokio::test]
    async fn indexed_title_batch_reads_stores_once_and_keeps_precedence() {
        let home = TempDir::new().unwrap();
        let codex_dir = home.path().join(".codex");
        tokio::fs::create_dir_all(&codex_dir).await.unwrap();
        seed_state_db(
            &codex_dir.join("state_5.sqlite"),
            &[("renamed", "Reader rename")],
        );
        tokio::fs::write(
            codex_dir.join("session_index.jsonl"),
            concat!(
                r#"{"id":"renamed","thread_name":"Generated name"}"#,
                "\n",
                r#"{"id":"indexed","thread_name":"Old generated name"}"#,
                "\n",
                r#"{"id":"ignored","thread_name":"Not requested"}"#,
                "\n",
                r#"{"id":"indexed","thread_name":"Latest generated name"}"#,
                "\n",
                r#"{"id":"cleared","thread_name":"Old generated name"}"#,
                "\n",
                r#"{"id":"cleared","thread_name":null}"#,
                "\n",
            ),
        )
        .await
        .unwrap();

        let titles = indexed_session_titles_in(
            home.path(),
            None,
            &[
                "renamed".into(),
                "indexed".into(),
                "cleared".into(),
                "missing".into(),
            ],
        )
        .await;
        assert_eq!(
            titles.get("renamed"),
            Some(&ResolvedTitle::new(
                "Reader rename",
                TitleSource::UserRename
            ))
        );
        assert_eq!(
            titles.get("indexed"),
            Some(&ResolvedTitle::new(
                "Latest generated name",
                TitleSource::AiGenerated
            ))
        );
        assert!(!titles.contains_key("ignored"));
        assert!(!titles.contains_key("cleared"));
        assert!(!titles.contains_key("missing"));
    }

    #[test]
    fn is_excluded_subagent_drops_only_known_children() {
        let child = "019e0000-0000-7000-8000-000000000001";
        let parent = "019e6c24-5ad0-7610-8d65-fae1fd70b5d3";
        let mut child_ids = HashSet::new();
        child_ids.insert(child.to_string());

        let child_path = PathBuf::from(format!(
            "/s/2026/05/28/rollout-2026-05-28T11-13-02-{child}.jsonl"
        ));
        let parent_path = PathBuf::from(format!(
            "/s/2026/05/28/rollout-2026-05-28T11-13-02-{parent}.jsonl"
        ));

        // The spawned child is excluded; the parent (not a child id) is kept.
        assert!(is_excluded_subagent(&child_path, &child_ids));
        assert!(!is_excluded_subagent(&parent_path, &child_ids));
        // A non-UUID name fails open (kept), and an empty set excludes nothing.
        assert!(!is_excluded_subagent(
            &PathBuf::from("/s/notes.jsonl"),
            &child_ids
        ));
        assert!(!is_excluded_subagent(&child_path, &HashSet::new()));
    }
}
