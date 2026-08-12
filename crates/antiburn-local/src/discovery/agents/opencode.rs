// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! OpenCode log discovery and normalization.
//!
//! Native discovery prefers a read-only `rusqlite` connection to
//! `opencode.db`; older installations store conversation fragments across:
//! - `storage/session/**.json`
//! - `storage/message/**.json`
//! - `storage/part/**.json`
//!
//! Mounted WSL discovery never opens that live database through UNC. It invokes
//! the distro's OpenCode CLI for one bounded metadata query and per-cluster
//! exports. Some CLI builds truncate otherwise successful exports, so malformed
//! in-limit output is reconstructed with bounded 16 KiB read-only DB chunks.
//! Legacy fragments are the final fallback. Every path normalizes a root and
//! its descendants into the same synthetic JSONL consumed downstream.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use std::time::UNIX_EPOCH;

use crate::discovery::scanner::{AgentKind, TitleSource};
use crate::discovery::{
    AgentExplorer, DesktopPlatform, DirectSessionSource, FORK_OBSERVATION_KEY, ForkObservation,
    ResolvedTitle, SessionLog, SessionSource, SessionTitleAndSurface, SurfacePaths,
    TitleLookupKind, current_desktop_platform, env_path_when_real_home, home_dir,
};
use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags, params, params_from_iter};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::platform::environment::{DiscoveryEnvironment, WslEnvironmentInfo};

const CLI_TIMEOUT: Duration = Duration::from_secs(15);
const CLI_METADATA_MAX_BYTES: usize = 8 * 1024 * 1024;
const CLI_EXPORT_MAX_BYTES: usize = 32 * 1024 * 1024;
const CLI_DATA_CHUNK_BYTES: usize = 16 * 1024;
const CLI_EXPORT_CONCURRENCY: usize = 4;
const CLI_EXPORT_CACHE_LIMIT: usize = 256;
const CLI_EXPORT_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

static CLI_EXPORT_CACHE: OnceLock<Mutex<CliExportCache>> = OnceLock::new();

type FragmentRecords = (
    BTreeMap<String, Vec<MessageRecord>>,
    BTreeMap<String, Vec<PartRecord>>,
);

#[derive(Default)]
struct CliExportCache {
    entries: HashMap<String, (i64, String)>,
    order: VecDeque<String>,
    retained_bytes: usize,
}

impl CliExportCache {
    fn get(&mut self, key: &str, updated_at: i64) -> Option<String> {
        let (cached_updated, content) = self.entries.get(key)?;
        (*cached_updated == updated_at).then(|| content.clone())
    }

    fn insert(&mut self, key: String, updated_at: i64, content: String) {
        let content_bytes = content.len();
        if content_bytes > CLI_EXPORT_CACHE_MAX_BYTES {
            return;
        }
        if let Some((_, existing)) = self.entries.get(&key) {
            self.retained_bytes = self.retained_bytes.saturating_sub(existing.len());
        }
        self.order.retain(|existing| existing != &key);
        self.order.push_back(key.clone());
        self.retained_bytes = self.retained_bytes.saturating_add(content_bytes);
        self.entries.insert(key, (updated_at, content));
        while self.order.len() > CLI_EXPORT_CACHE_LIMIT
            || self.retained_bytes > CLI_EXPORT_CACHE_MAX_BYTES
        {
            if let Some(oldest) = self.order.pop_front()
                && let Some((_, removed)) = self.entries.remove(&oldest)
            {
                self.retained_bytes = self.retained_bytes.saturating_sub(removed.len());
            }
        }
    }
}

pub fn clear_cli_export_cache() {
    if let Some(cache) = CLI_EXPORT_CACHE.get() {
        *cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = CliExportCache::default();
    }
}

pub fn cli_export_cache_stats() -> (usize, usize) {
    CLI_EXPORT_CACHE
        .get()
        .map(|cache| {
            let cache = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (cache.entries.len(), cache.retained_bytes)
        })
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct CliSessionRow {
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    time_created: Option<i64>,
    #[serde(default)]
    time_updated: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CliRecordRow {
    kind: String,
    id: String,
    #[serde(default)]
    message_id: Option<String>,
    session_id: String,
    #[serde(default)]
    time_created: Option<i64>,
    #[serde(default)]
    time_updated: Option<i64>,
    data_len: usize,
}

/// Failure classes at the WSL CLI trust boundary. Discovery logs the class,
/// falls back to legacy fragments for this scan, and retries the CLI naturally
/// on a later recurring scan.
#[derive(Debug)]
enum CliBridgeError {
    Unavailable(String),
    Timeout,
    Oversized,
    InvalidOutput(String),
}

impl std::fmt::Display for CliBridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(message) | Self::InvalidOutput(message) => f.write_str(message),
            Self::Timeout => f.write_str("command timed out"),
            Self::Oversized => f.write_str("command output exceeded the configured limit"),
        }
    }
}

pub struct OpenCodeExplorer;

#[async_trait]
impl AgentExplorer for OpenCodeExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let roots = data_roots();
        discover_recent_in(&roots, now, since_secs).await
    }

    async fn direct_session_source(&self, session_id: &str) -> DirectSessionSource {
        locate_db_session_source(session_id.to_string()).await
    }

    async fn provider_db_fingerprint(
        &self,
        db_path: &Path,
        session_id: &str,
    ) -> Option<(u64, u64)> {
        db_session_fingerprint(db_path.to_path_buf(), session_id.to_string()).await
    }

    async fn discover_cwds(&self, now: i64, since_secs: i64) -> Vec<String> {
        let roots = data_roots();
        discover_cwds_in(&roots, now, since_secs).await
    }

    /// OpenCode override of the batched title+surface scan. Pulls every
    /// session's title from `opencode.db` in a single SQL query rather
    /// than reading transcript files (which don't carry titles). Surface
    /// is always `"cli"` — OpenCode is a single binary serving both CLI
    /// and TUI from the same store.
    async fn session_titles_and_surfaces(&self) -> Vec<SessionTitleAndSurface> {
        let mut out = Vec::new();
        for root in data_roots() {
            let db_path = root.join("opencode.db");
            if !db_path.exists() {
                continue;
            }
            let rows =
                tokio::task::spawn_blocking(move || query_all_session_titles_from_db(&db_path))
                    .await
                    .unwrap_or_default();
            for (session_id, title) in rows {
                // OpenCode's SQLite schema has no rename concept — every
                // non-empty `session.title` is tagged `Explicit`.
                let title_source = title.as_ref().map(|_| TitleSource::Explicit);
                out.push(SessionTitleAndSurface {
                    session_id,
                    title,
                    title_source,
                    surface: "cli",
                });
            }
        }
        out
    }

    fn title_lookup_kind(&self) -> TitleLookupKind {
        TitleLookupKind::Direct
    }

    /// Point-query lookup against `opencode.db#session.title` keyed by id.
    /// Avoids the default's full-table scan when the frontend only needs a
    /// handful of titles. OpenCode has no rename concept, so every hit is
    /// tagged [`TitleSource::Explicit`].
    async fn session_title(&self, agent_session_id: &str) -> Option<ResolvedTitle> {
        for root in data_roots() {
            let db_path = root.join("opencode.db");
            if !tokio::fs::try_exists(&db_path).await.unwrap_or(false) {
                continue;
            }
            let session_id = agent_session_id.to_string();
            let title = tokio::task::spawn_blocking(move || {
                query_session_title_by_id(&db_path, &session_id)
            })
            .await
            .ok()
            .flatten();
            if let Some(text) = title {
                return Some(ResolvedTitle::new(text, TitleSource::Explicit));
            }
        }
        None
    }

    /// Owns the per-platform OpenCode data dir (XDG `share/opencode/`, macOS
    /// `Application Support/opencode/`, Windows `AppData/Roaming/opencode/`)
    /// plus the SQLite store `opencode.db`. Single-surface — CLI+TUI share
    /// one tree.
    ///
    /// Substring → `surface_paths` bucket: *(all → `cli`)*
    /// - `/.local/share/opencode/`                 (Linux XDG)
    /// - `/opencode/storage/`                      (storage subtree)
    /// - `/opencode/log/`                          (log subtree)
    /// - `/opencode/opencode.db` (ends_with)       (SQLite store)
    /// - `/library/application support/opencode/`  (macOS)
    /// - `/appdata/roaming/opencode/`              (Windows)
    ///
    /// No IDE substring — OpenCode is a single binary serving both CLI
    /// and TUI from the same store.
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/.local/share/opencode/")
            || path_lower.contains("/opencode/storage/")
            || path_lower.contains("/opencode/log/")
            || path_lower.ends_with("/opencode/opencode.db")
            || path_lower.contains("/library/application support/opencode/")
            || path_lower.contains("/appdata/roaming/opencode/")
    }

    // OpenCode is a single binary serving both CLI and TUI from the same
    // on-disk store. Per the 2026-05-25 audit.
    fn unmatched_surface(&self) -> &'static str {
        "cli"
    }

    /// CLI-only: per-platform OpenCode data dir candidates. No IDE surface
    /// (the binary serves both CLI and TUI from the same store).
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        let appdata = env_path_when_real_home(home, "APPDATA");
        let xdg_data_home = env_path_when_real_home(home, "XDG_DATA_HOME");
        SurfacePaths {
            cli: data_roots_for_platform(
                home,
                current_desktop_platform(),
                appdata.as_deref(),
                xdg_data_home.as_deref(),
            ),
            ide_desktop: Vec::new(),
            mirror: Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    home.join(".local")
        .join("share")
        .join("opencode")
        .join("storage")
        .join("session")
        .join("global")
        .join("ses_abc.json")
}

/// Read every `(id, title)` row from the OpenCode `session` table. Used by
/// the batched title+surface path to fill the activity-list cache in one
/// trip rather than O(N) per-session queries. Empty / whitespace-only
/// titles are returned as `None` so the frontend's existing "no title"
/// rendering still applies.
/// Point-query for a single session's title. Returns `None` when the DB
/// can't be opened, the row is missing, or the title is blank/whitespace.
fn query_session_title_by_id(path: &Path, session_id: &str) -> Option<String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let title: String = conn
        .query_row(
            "SELECT title FROM session WHERE id = ?1 LIMIT 1",
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

fn query_all_session_titles_from_db(path: &Path) -> Vec<(String, Option<String>)> {
    let Ok(conn) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT id, title FROM session") else {
        return Vec::new();
    };
    let rows_iter = match stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        Ok((id, title))
    }) {
        Ok(iter) => iter,
        Err(_) => return Vec::new(),
    };
    rows_iter
        .filter_map(|row| row.ok())
        .map(|(id, title)| {
            let trimmed = title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
            (id, trimmed)
        })
        .collect()
}

fn query_recent_session_directories_from_db(path: &Path, cutoff: i64) -> Vec<String> {
    let Ok(conn) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for query in [
        "SELECT directory FROM session WHERE COALESCE(time_updated, time_created) >= ?1",
        "SELECT directory FROM session WHERE time_updated >= ?1 OR time_created >= ?1",
    ] {
        let Ok(mut stmt) = conn.prepare(query) else {
            continue;
        };
        let Ok(rows) = stmt.query_map(params![cutoff * 1000], |row| row.get::<_, String>(0)) else {
            continue;
        };
        for directory in rows.flatten() {
            let trimmed = directory.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

pub fn data_roots() -> Vec<PathBuf> {
    if let Ok(override_dir) = std::env::var("OPENCODE_DATA_DIR") {
        return vec![PathBuf::from(override_dir)];
    }

    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    data_roots_for_home(&home)
}

/// Read recent assistant-message records straight from the OpenCode store,
/// newest first and capped at `limit`.
///
/// Returns each record as the raw JSON object OpenCode wrote; nothing is
/// interpreted or persisted here.
pub async fn recent_assistant_message_records(cutoff: i64, limit: usize) -> Vec<String> {
    let mut records = Vec::new();
    for root in data_roots() {
        let db_path = root.join("opencode.db");
        if tokio::fs::try_exists(&db_path).await.unwrap_or(false) {
            let remaining = limit.saturating_sub(records.len());
            if remaining == 0 {
                break;
            }
            let rows = tokio::task::spawn_blocking(move || {
                query_recent_assistant_messages_from_db(&db_path, cutoff, remaining)
            })
            .await
            .unwrap_or_default();
            records.extend(rows);
            continue;
        }

        let message_dir = root.join("storage").join("message");
        let remaining = limit.saturating_sub(records.len());
        records.extend(recent_assistant_messages_from_json(&message_dir, cutoff, remaining).await);
    }
    records.truncate(limit);
    records
}

async fn recent_assistant_messages_from_json(
    message_dir: &Path,
    cutoff: i64,
    limit: usize,
) -> Vec<String> {
    let mut records = Vec::new();
    for candidate in collect_recent_json_files(message_dir, cutoff).await {
        if records.len() >= limit {
            break;
        }
        let Some(value) = read_json(&candidate.path).await else {
            continue;
        };
        if value.get("role").and_then(Value::as_str) == Some("assistant")
            && let Ok(record) = serde_json::to_string(&value)
        {
            records.push(record);
        }
    }
    records
}

fn query_recent_assistant_messages_from_db(path: &Path, cutoff: i64, limit: usize) -> Vec<String> {
    let Ok(conn) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(mut statement) = conn.prepare(
        "SELECT data FROM message WHERE time_created >= ?1 AND json_extract(data, '$.role') = 'assistant' ORDER BY time_created DESC LIMIT ?2",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![cutoff.saturating_mul(1000), limit as i64], |row| {
        row.get::<_, String>(0)
    }) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

pub async fn locate_db_session_source(root_session_id: String) -> DirectSessionSource {
    let mut saw_db = false;
    for root in data_roots() {
        let db_path = root.join("opencode.db");
        if !tokio::fs::try_exists(&db_path).await.unwrap_or(false) {
            continue;
        }
        saw_db = true;
        let session_id = root_session_id.clone();
        let exists = tokio::task::spawn_blocking(move || db_session_exists(&db_path, &session_id))
            .await
            .unwrap_or(false);
        if exists {
            return DirectSessionSource::Found(SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: root.join("opencode.db"),
                session_id: root_session_id,
            });
        }
    }
    if saw_db {
        DirectSessionSource::Missing
    } else {
        DirectSessionSource::Unsupported
    }
}

/// Discover OpenCode sessions in one mounted WSL distribution. The CLI owns
/// access to its live SQLite store; fragmented JSON is only used when the CLI
/// bridge is unavailable, unsupported, or returns unusable output.
pub async fn discover_recent_in_wsl(
    info: &WslEnvironmentInfo,
    now: i64,
    since_secs: i64,
) -> Vec<SessionLog> {
    match discover_recent_via_cli(info, now - since_secs).await {
        Ok(logs) => logs,
        Err(error) => {
            tracing::warn!(
                event = "opencode_wsl_cli_unavailable",
                distro = info.distribution,
                error = %error,
                "OpenCode WSL CLI discovery failed; trying fragmented storage"
            );
            let roots = [info.context.home.join(".local/share/opencode")];
            let mut logs = discover_recent_in_impl(&roots, now, since_secs, false).await;
            for log in &mut logs {
                log.environment = info.context.environment.clone();
            }
            logs
        }
    }
}

/// Discovers one CWD per recent root session without exporting transcripts.
///
/// Repository discovery needs only location/count metadata. Keeping it on the
/// single CLI metadata query avoids the much more expensive export and chunked
/// reconstruction path used by upload, detail, and analytics. Legacy fragmented
/// storage remains the fallback when the distro CLI cannot provide metadata.
pub async fn discover_cwds_in_wsl(
    info: &WslEnvironmentInfo,
    now: i64,
    since_secs: i64,
) -> Vec<String> {
    let executable = opencode_executable(info).await;
    let cutoff = now.saturating_sub(since_secs.max(0));
    let query = cli_session_metadata_query(cutoff);
    let cli_cwds = async {
        let metadata = run_opencode_cli(
            &info.context.environment,
            &executable,
            &["db", query.as_str(), "--format", "json"],
            CLI_METADATA_MAX_BYTES,
        )
        .await?;
        let rows = parse_cli_session_rows(&metadata)?;
        Ok::<_, CliBridgeError>(
            cli_root_directories(rows)
                .into_iter()
                .filter_map(|cwd| {
                    crate::platform::environment::wsl_to_windows_path(&info.distribution, &cwd)
                        .ok()
                        .map(|path| path.to_string_lossy().to_string())
                })
                .collect::<Vec<_>>(),
        )
    }
    .await;
    match cli_cwds {
        Ok(cwds) => cwds,
        Err(error) => {
            tracing::warn!(
                event = "opencode_wsl_cli_cwd_unavailable",
                distro = info.distribution,
                error = %error,
                "OpenCode WSL metadata-only CWD discovery failed; trying fragmented storage"
            );
            let roots = [info.context.home.join(".local/share/opencode")];
            let mut logs = discover_recent_in_impl(&roots, now, since_secs, false).await;
            let mut cwds = Vec::new();
            for log in &mut logs {
                log.environment = info.context.environment.clone();
                if let Some(cwd) = crate::discovery::session_log_metadata(log)
                    .await
                    .and_then(|metadata| metadata.cwd)
                {
                    cwds.push(cwd);
                }
            }
            cwds
        }
    }
}

/// Resolves requested WSL OpenCode titles with one metadata query and no
/// transcript exports. Missing IDs are omitted from the result.
pub async fn session_titles_in_wsl(
    info: &WslEnvironmentInfo,
    requested_ids: &HashSet<String>,
) -> HashMap<String, Option<String>> {
    let Some(query) = cli_title_query(requested_ids) else {
        return HashMap::new();
    };
    let executable = opencode_executable(info).await;
    let cli_titles = async {
        let output = run_opencode_cli(
            &info.context.environment,
            &executable,
            &["db", query.as_str(), "--format", "json"],
            CLI_METADATA_MAX_BYTES,
        )
        .await?;
        let rows = parse_cli_session_rows(&output)?;
        Ok::<_, CliBridgeError>(
            rows.into_iter()
                .filter(|row| requested_ids.contains(&row.id))
                .map(|row| (row.id, row.title.filter(|title| !title.trim().is_empty())))
                .collect(),
        )
    }
    .await;
    match cli_titles {
        Ok(titles) => titles,
        Err(error) => {
            tracing::warn!(
                event = "opencode_wsl_cli_title_unavailable",
                distro = info.distribution,
                error = %error,
                "OpenCode WSL title query failed; trying fragmented storage"
            );
            let session_dir = info
                .context
                .home
                .join(".local/share/opencode/storage/session");
            let mut titles = HashMap::new();
            for id in requested_ids {
                let Some(path) = find_session_json_file(&session_dir, id).await else {
                    continue;
                };
                let Some(value) = read_json(&path).await else {
                    continue;
                };
                let title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .map(str::to_string);
                titles.insert(id.clone(), title);
            }
            titles
        }
    }
}

fn cli_title_query(requested_ids: &HashSet<String>) -> Option<String> {
    let mut ids = requested_ids
        .iter()
        .filter(|id| valid_session_id(id))
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>();
    ids.sort();
    (!ids.is_empty()).then(|| {
        format!(
            "SELECT id, parent_id, directory, title, time_created, time_updated \
             FROM session WHERE id IN ({})",
            ids.join(",")
        )
    })
}

/// Root directories represent the same clustered session units emitted by full
/// OpenCode discovery; descendants must not inflate Local Repos session counts.
fn cli_root_directories(rows: Vec<CliSessionRow>) -> Vec<String> {
    rows.into_iter()
        .filter(|row| row.parent_id.is_none())
        .filter_map(|row| row.directory)
        .collect()
}

/// Loads all recently changed clusters with one metadata query, then exports
/// roots concurrently under a fixed semaphore and output limits.
async fn discover_recent_via_cli(
    info: &WslEnvironmentInfo,
    cutoff: i64,
) -> Result<Vec<SessionLog>, CliBridgeError> {
    let executable = opencode_executable(info).await;
    let query = cli_session_metadata_query(cutoff);
    let metadata = run_opencode_cli(
        &info.context.environment,
        &executable,
        &["db", query.as_str(), "--format", "json"],
        CLI_METADATA_MAX_BYTES,
    )
    .await?;
    let rows = parse_cli_session_rows(&metadata)?;
    render_cli_clusters(&info.context.environment, &executable, rows, cutoff).await
}

/// Fixed read-only query shared by full transcript discovery and the Local
/// Repos metadata-only path. It promotes recently changed descendants to their
/// root and returns the complete cluster metadata without message/part blobs.
fn cli_session_metadata_query(cutoff: i64) -> String {
    let cutoff_ms = cutoff.saturating_mul(1000);
    format!(
        "WITH RECURSIVE ancestors(id, parent_id) AS (\
         SELECT id, parent_id FROM session WHERE time_updated >= {cutoff_ms} \
         UNION \
         SELECT parent.id, parent.parent_id FROM session parent \
         JOIN ancestors child ON child.parent_id = parent.id\
         ), roots(id) AS (SELECT id FROM ancestors WHERE parent_id IS NULL), \
         cluster(id) AS (SELECT id FROM roots UNION ALL SELECT child.id FROM session child \
         JOIN cluster parent ON child.parent_id = parent.id) \
         SELECT session.id, session.parent_id, session.directory, session.title, \
         session.time_created, session.time_updated FROM session JOIN cluster USING (id)"
    )
}

/// Resolves common per-user installs before falling back to distro `PATH`.
/// Paths are built in the Linux namespace and never interpolated through a shell.
async fn opencode_executable(info: &WslEnvironmentInfo) -> String {
    for relative in [".opencode/bin/opencode", ".local/bin/opencode"] {
        if tokio::fs::metadata(info.context.home.join(relative))
            .await
            .is_ok()
        {
            return format!(
                "{}/{}",
                info.context
                    .platform_home
                    .to_string_lossy()
                    .replace('\\', "/")
                    .trim_end_matches('/'),
                relative
            );
        }
    }
    "opencode".to_string()
}

fn parse_cli_session_rows(output: &[u8]) -> Result<Vec<CliSessionRow>, CliBridgeError> {
    let rows: Vec<CliSessionRow> = serde_json::from_slice(output)
        .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))?;
    if rows.iter().any(|row| !valid_session_id(&row.id)) {
        return Err(CliBridgeError::InvalidOutput(
            "OpenCode metadata contained an invalid session id".to_string(),
        ));
    }
    Ok(rows)
}

async fn render_cli_clusters(
    environment: &DiscoveryEnvironment,
    executable: &str,
    rows: Vec<CliSessionRow>,
    cutoff: i64,
) -> Result<Vec<SessionLog>, CliBridgeError> {
    let sessions = session_records_from_cli_rows(environment, rows);
    let wanted = recent_cli_clusters(&sessions, cutoff);
    let exports = spawn_cli_exports(environment, executable, &sessions, &wanted);
    let exported = collect_cli_exports(exports).await?;
    render_exported_cli_clusters(environment, cutoff, sessions, &wanted, &exported)
}

fn session_records_from_cli_rows(
    environment: &DiscoveryEnvironment,
    rows: Vec<CliSessionRow>,
) -> HashMap<String, SessionRecord> {
    let source_prefix = format!("opencode-cli:{}", environment.key());
    rows.into_iter()
        .map(|row| {
            let created_at = milliseconds_to_seconds(row.time_created);
            let updated_at = milliseconds_to_seconds(row.time_updated).or(created_at);
            let raw = json!({
                "id": row.id,
                "parentID": row.parent_id,
                "directory": row.directory,
                "title": row.title,
                "time": { "created": created_at, "updated": updated_at },
            });
            let id = raw["id"].as_str().unwrap_or_default().to_string();
            (
                id.clone(),
                SessionRecord {
                    directory: raw["directory"].as_str().map(str::to_string),
                    title: raw["title"].as_str().map(str::to_string),
                    parent_session_id: raw["parentID"].as_str().map(str::to_string),
                    source_file: format!("{source_prefix}:{id}"),
                    created_at,
                    updated_at,
                    file_mtime: updated_at,
                    raw,
                },
            )
        })
        .collect()
}

fn recent_cli_clusters(
    sessions: &HashMap<String, SessionRecord>,
    cutoff: i64,
) -> Vec<OpenCodeSessionCluster> {
    let clusters = build_session_clusters(sessions, &BTreeMap::new(), &BTreeMap::new());
    clusters
        .into_iter()
        .filter(|cluster| {
            cluster.session_ids.iter().any(|id| {
                sessions
                    .get(id)
                    .and_then(|record| record.updated_at.or(record.created_at))
                    .is_some_and(|updated| updated >= cutoff)
            })
        })
        .collect()
}

type CliExportTask = Result<(String, i64, String, String), CliBridgeError>;

fn spawn_cli_exports(
    environment: &DiscoveryEnvironment,
    executable: &str,
    sessions: &HashMap<String, SessionRecord>,
    wanted: &[OpenCodeSessionCluster],
) -> tokio::task::JoinSet<CliExportTask> {
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(CLI_EXPORT_CONCURRENCY));
    let mut exports = tokio::task::JoinSet::new();
    for cluster in wanted {
        for session_id in &cluster.session_ids {
            spawn_cli_export(
                &mut exports,
                environment,
                executable,
                sessions,
                session_id,
                semaphore.clone(),
            );
        }
    }
    exports
}

fn spawn_cli_export(
    exports: &mut tokio::task::JoinSet<CliExportTask>,
    environment: &DiscoveryEnvironment,
    executable: &str,
    sessions: &HashMap<String, SessionRecord>,
    session_id: &str,
    semaphore: std::sync::Arc<tokio::sync::Semaphore>,
) {
    let updated_at = sessions
        .get(session_id)
        .and_then(|record| record.updated_at.or(record.created_at))
        .unwrap_or_default();
    let cache_key = format!("{}:{session_id}", environment.key());
    let cached = CLI_EXPORT_CACHE
        .get_or_init(|| Mutex::new(CliExportCache::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&cache_key, updated_at);
    if let Some(content) = cached {
        let session_id = session_id.to_string();
        exports.spawn(async move { Ok((session_id, updated_at, content, cache_key)) });
        return;
    }
    let environment = environment.clone();
    let executable = executable.to_string();
    let session_id = session_id.to_string();
    exports.spawn(async move {
        let _permit = semaphore.acquire_owned().await.map_err(|error| {
            CliBridgeError::Unavailable(format!("export semaphore closed: {error}"))
        })?;
        let content = load_cli_export_content(&environment, &executable, &session_id).await?;
        Ok((session_id, updated_at, content, cache_key))
    });
}

async fn load_cli_export_content(
    environment: &DiscoveryEnvironment,
    executable: &str,
    session_id: &str,
) -> Result<String, CliBridgeError> {
    let output = run_opencode_cli(
        environment,
        executable,
        &["export", session_id],
        CLI_EXPORT_MAX_BYTES,
    )
    .await?;
    let content = String::from_utf8(output)
        .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))?;
    if parse_cli_export(&content).is_ok() {
        Ok(content)
    } else {
        export_session_via_db(environment, executable, session_id).await
    }
}

async fn collect_cli_exports(
    mut exports: tokio::task::JoinSet<CliExportTask>,
) -> Result<HashMap<String, String>, CliBridgeError> {
    let mut exported = HashMap::new();
    let mut first_error = None;
    while let Some(result) = exports.join_next().await {
        if let Err(error) = accept_cli_export(result, &mut exported) {
            tracing::warn!(
                target: "opencode_wsl_cli",
                error = %error,
                "skipping one invalid OpenCode CLI session export",
            );
            first_error.get_or_insert(error);
        }
    }
    if exported.is_empty()
        && let Some(error) = first_error
    {
        return Err(error);
    }
    Ok(exported)
}

fn accept_cli_export(
    result: Result<CliExportTask, tokio::task::JoinError>,
    exported: &mut HashMap<String, String>,
) -> Result<(), CliBridgeError> {
    let (session_id, updated_at, content, cache_key) =
        result.map_err(|error| CliBridgeError::Unavailable(error.to_string()))??;
    parse_cli_export(&content)?;
    CLI_EXPORT_CACHE
        .get_or_init(|| Mutex::new(CliExportCache::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(cache_key, updated_at, content.clone());
    exported.insert(session_id, content);
    Ok(())
}

fn render_exported_cli_clusters(
    environment: &DiscoveryEnvironment,
    cutoff: i64,
    sessions: HashMap<String, SessionRecord>,
    wanted: &[OpenCodeSessionCluster],
    exported: &HashMap<String, String>,
) -> Result<Vec<SessionLog>, CliBridgeError> {
    let (messages, parts) = records_from_cli_exports(exported)?;
    let wanted_roots: HashSet<String> = wanted
        .iter()
        .map(|cluster| cluster.root_session_id.clone())
        .collect();
    let mut logs = render_session_logs(cutoff, sessions, messages, parts);
    logs.retain(|log| {
        log.source_label()
            .strip_prefix("opencode:")
            .is_some_and(|id| wanted_roots.contains(id))
    });
    for log in &mut logs {
        log.environment = environment.clone();
    }
    Ok(logs)
}

/// Reconstructs one export through read-only CLI DB queries when direct export
/// output is malformed. Aggregate record bytes are validated before allocation
/// or chunk commands, and each record is checked for concurrent mutation.
async fn export_session_via_db(
    environment: &DiscoveryEnvironment,
    executable: &str,
    session_id: &str,
) -> Result<String, CliBridgeError> {
    if !valid_session_id(session_id) {
        return Err(CliBridgeError::InvalidOutput(
            "OpenCode export fallback received an invalid session id".into(),
        ));
    }
    let query = format!(
        "SELECT 'message' AS kind, id, NULL AS message_id, session_id, time_created, \
         time_updated, length(CAST(data AS BLOB)) AS data_len FROM message \
         WHERE session_id = '{session_id}' UNION ALL \
         SELECT 'part' AS kind, id, message_id, session_id, time_created, time_updated, \
         length(CAST(data AS BLOB)) AS data_len FROM part WHERE session_id = '{session_id}' \
         ORDER BY time_created, id"
    );
    let output = run_opencode_cli(
        environment,
        executable,
        &["db", query.as_str(), "--format", "json"],
        CLI_METADATA_MAX_BYTES,
    )
    .await?;
    let rows: Vec<CliRecordRow> = serde_json::from_slice(&output)
        .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))?;
    validate_cli_record_rows(&rows, session_id)?;
    let records = read_cli_records(environment, executable, rows).await?;
    assemble_reconstructed_export(session_id, records)
}

fn validate_cli_record_rows(rows: &[CliRecordRow], session_id: &str) -> Result<(), CliBridgeError> {
    if rows.iter().any(|row| {
        row.session_id != session_id
            || !valid_session_id(&row.id)
            || row
                .message_id
                .as_deref()
                .is_some_and(|id| !valid_session_id(id))
            || !matches!(row.kind.as_str(), "message" | "part")
    }) {
        return Err(CliBridgeError::InvalidOutput(
            "OpenCode DB fallback returned invalid record metadata".into(),
        ));
    }
    if !cli_record_data_within_limit(rows) {
        return Err(CliBridgeError::Oversized);
    }
    Ok(())
}

enum ReconstructedRecord {
    Message(Value),
    Part { message_id: String, value: Value },
    Skip,
}

async fn read_cli_records(
    environment: &DiscoveryEnvironment,
    executable: &str,
    rows: Vec<CliRecordRow>,
) -> Result<Vec<ReconstructedRecord>, CliBridgeError> {
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        let data = read_cli_record_data(environment, executable, &row).await?;
        records.push(normalize_cli_record(row, &data)?);
    }
    Ok(records)
}

fn normalize_cli_record(
    row: CliRecordRow,
    data: &str,
) -> Result<ReconstructedRecord, CliBridgeError> {
    let mut value: Value = serde_json::from_str(data)
        .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))?;
    let object = value.as_object_mut().ok_or_else(|| {
        CliBridgeError::InvalidOutput("OpenCode record data was not an object".into())
    })?;
    object.insert("id".into(), Value::String(row.id));
    object.insert("sessionID".into(), Value::String(row.session_id));
    object.insert(
        "time".into(),
        json!({ "created": row.time_created, "updated": row.time_updated }),
    );
    if row.kind != "part" {
        return Ok(ReconstructedRecord::Message(value));
    }
    let Some(message_id) = row.message_id else {
        return Ok(ReconstructedRecord::Skip);
    };
    object.insert("messageID".into(), Value::String(message_id.clone()));
    Ok(ReconstructedRecord::Part { message_id, value })
}

fn assemble_reconstructed_export(
    session_id: &str,
    records: Vec<ReconstructedRecord>,
) -> Result<String, CliBridgeError> {
    let mut messages = Vec::new();
    let mut parts_by_message: HashMap<String, Vec<Value>> = HashMap::new();
    for record in records {
        match record {
            ReconstructedRecord::Message(value) => messages.push(value),
            ReconstructedRecord::Part { message_id, value } => {
                parts_by_message.entry(message_id).or_default().push(value);
            }
            ReconstructedRecord::Skip => {}
        }
    }
    for message in &mut messages {
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parts = parts_by_message.remove(id).unwrap_or_default();
        *message = json!({ "info": message.take(), "parts": parts });
    }
    serde_json::to_string(&json!({
        "info": { "id": session_id },
        "messages": messages,
    }))
    .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))
}

/// Bounds both allocation and WSL process count for chunked DB reconstruction.
/// Checked addition also rejects malicious or corrupt lengths that overflow.
fn cli_record_data_within_limit(rows: &[CliRecordRow]) -> bool {
    rows.iter()
        .try_fold(0usize, |total, row| {
            total
                .checked_add(row.data_len)
                .filter(|size| *size <= CLI_EXPORT_MAX_BYTES)
        })
        .is_some()
}

/// Reads one JSON blob as fixed-size hex chunks so CLI stdout truncation cannot
/// split a JSON string. Metadata IDs select a fixed table and are validated by
/// the caller before they reach the query text.
async fn read_cli_record_data(
    environment: &DiscoveryEnvironment,
    executable: &str,
    row: &CliRecordRow,
) -> Result<String, CliBridgeError> {
    if row.data_len > CLI_EXPORT_MAX_BYTES {
        return Err(CliBridgeError::Oversized);
    }
    let table = row.kind.as_str();
    let mut bytes = Vec::with_capacity(row.data_len);
    for offset in (0..row.data_len).step_by(CLI_DATA_CHUNK_BYTES) {
        let query = cli_data_chunk_query(table, &row.id, offset);
        let output = run_opencode_cli(
            environment,
            executable,
            &["db", query.as_str(), "--format", "json"],
            CLI_METADATA_MAX_BYTES,
        )
        .await?;
        bytes.extend(parse_cli_data_chunk(&output)?);
    }
    finish_cli_record_data(bytes, row.data_len)
}

fn cli_data_chunk_query(table: &str, id: &str, offset: usize) -> String {
    format!(
        "SELECT hex(substr(CAST(data AS BLOB), {}, {})) AS chunk FROM {table} WHERE id = '{id}'",
        offset + 1,
        CLI_DATA_CHUNK_BYTES,
    )
}

fn parse_cli_data_chunk(output: &[u8]) -> Result<Vec<u8>, CliBridgeError> {
    let value: Value = serde_json::from_slice(output)
        .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))?;
    let chunk = value
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("chunk"))
        .and_then(Value::as_str)
        .ok_or_else(|| CliBridgeError::InvalidOutput("OpenCode data chunk was missing".into()))?;
    decode_hex(chunk)
}

fn finish_cli_record_data(bytes: Vec<u8>, expected_len: usize) -> Result<String, CliBridgeError> {
    if bytes.len() != expected_len {
        return Err(CliBridgeError::InvalidOutput(
            "OpenCode data changed while it was being read".into(),
        ));
    }
    String::from_utf8(bytes).map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))
}

fn decode_hex(value: &str) -> Result<Vec<u8>, CliBridgeError> {
    if !value.len().is_multiple_of(2) {
        return Err(CliBridgeError::InvalidOutput(
            "OpenCode returned malformed hex data".into(),
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair)
                .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))?;
            u8::from_str_radix(text, 16)
                .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))
        })
        .collect()
}

fn parse_cli_export(content: &str) -> Result<Value, CliBridgeError> {
    let value: Value = serde_json::from_str(content)
        .map_err(|error| CliBridgeError::InvalidOutput(error.to_string()))?;
    if !value.get("info").is_some_and(Value::is_object)
        || !value.get("messages").is_some_and(Value::is_array)
    {
        return Err(CliBridgeError::InvalidOutput(
            "OpenCode export did not contain info and messages".to_string(),
        ));
    }
    Ok(value)
}

fn records_from_cli_exports(
    exports: &HashMap<String, String>,
) -> Result<FragmentRecords, CliBridgeError> {
    let mut messages_by_session = BTreeMap::new();
    let mut parts_by_session = BTreeMap::new();
    for (expected_session_id, content) in exports {
        let value = parse_cli_export(content)?;
        let export_session_id = value
            .pointer("/info/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliBridgeError::InvalidOutput("OpenCode export omitted its id".into())
            })?;
        if export_session_id != expected_session_id || !valid_session_id(export_session_id) {
            return Err(CliBridgeError::InvalidOutput(
                "OpenCode export id did not match the requested session".into(),
            ));
        }
        for message in value["messages"].as_array().into_iter().flatten() {
            let Some(info) = message.get("info") else {
                continue;
            };
            let Some(message_id) = info.get("id").and_then(Value::as_str) else {
                continue;
            };
            let session_id = info
                .get("sessionID")
                .and_then(Value::as_str)
                .unwrap_or(expected_session_id)
                .to_string();
            let created_at = info
                .pointer("/time/created")
                .and_then(parse_epoch_from_json_value);
            messages_by_session
                .entry(session_id.clone())
                .or_insert_with(Vec::new)
                .push(MessageRecord {
                    id: message_id.to_string(),
                    session_id: session_id.clone(),
                    role: info.get("role").and_then(Value::as_str).map(str::to_string),
                    source_file: format!("opencode-cli:{expected_session_id}"),
                    created_at,
                    file_mtime: created_at,
                    raw: info.clone(),
                });
            for part in message
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let part_id = part
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown-part")
                    .to_string();
                let part_created = part
                    .pointer("/time/created")
                    .and_then(parse_epoch_from_json_value)
                    .or(created_at);
                parts_by_session
                    .entry(session_id.clone())
                    .or_insert_with(Vec::new)
                    .push(PartRecord {
                        id: part_id,
                        message_id: Some(message_id.to_string()),
                        session_id: Some(session_id.clone()),
                        part_type: part.get("type").and_then(Value::as_str).map(str::to_string),
                        source_file: format!("opencode-cli:{expected_session_id}"),
                        created_at: part_created,
                        file_mtime: part_created,
                        raw: part.clone(),
                    });
            }
        }
    }
    Ok((messages_by_session, parts_by_session))
}

fn milliseconds_to_seconds(value: Option<i64>) -> Option<i64> {
    value.filter(|value| *value > 0).map(|value| {
        if value > 1_000_000_000_000 {
            value / 1000
        } else {
            value
        }
    })
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

async fn run_opencode_cli(
    environment: &DiscoveryEnvironment,
    executable: &str,
    args: &[&str],
    max_bytes: usize,
) -> Result<Vec<u8>, CliBridgeError> {
    let mut command = build_opencode_cli_command(environment, executable, args)?;
    let mut child = command
        .spawn()
        .map_err(|error| CliBridgeError::Unavailable(error.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        CliBridgeError::Unavailable("OpenCode stdout pipe was unavailable".into())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        CliBridgeError::Unavailable("OpenCode stderr pipe was unavailable".into())
    })?;
    let result = tokio::time::timeout(
        CLI_TIMEOUT,
        collect_opencode_cli_output(child, stdout, stderr, max_bytes),
    )
    .await
    .map_err(|_| CliBridgeError::Timeout)?;
    classify_opencode_cli_output(result, max_bytes)
}

fn build_opencode_cli_command(
    environment: &DiscoveryEnvironment,
    executable: &str,
    args: &[&str],
) -> Result<tokio::process::Command, CliBridgeError> {
    let mut command =
        crate::platform::environment::configure_command_for_environment(environment, "env", None)
            .map_err(|error| CliBridgeError::Unavailable(error.to_string()))?;
    command.args([
        "OPENCODE_DISABLE_AUTOUPDATE=true",
        "OPENCODE_DISABLE_MODELS_FETCH=true",
        "OPENCODE_DISABLE_DEFAULT_PLUGINS=true",
        "OPENCODE_SKIP_MIGRATIONS=true",
        executable,
        "--pure",
    ]);
    command.args(args);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(command)
}

type CliProcessOutput = (
    std::io::Result<Vec<u8>>,
    std::io::Result<Vec<u8>>,
    std::io::Result<std::process::ExitStatus>,
);

async fn collect_opencode_cli_output(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    max_bytes: usize,
) -> CliProcessOutput {
    let stdout_task = read_bounded(stdout, max_bytes + 1);
    let stderr_task = read_bounded(stderr, 64 * 1024 + 1);
    tokio::join!(stdout_task, stderr_task, child.wait())
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map(|_| bytes)
}

fn classify_opencode_cli_output(
    (stdout, stderr, status): CliProcessOutput,
    max_bytes: usize,
) -> Result<Vec<u8>, CliBridgeError> {
    let stdout = stdout.map_err(|error| CliBridgeError::Unavailable(error.to_string()))?;
    if stdout.len() > max_bytes {
        return Err(CliBridgeError::Oversized);
    }
    let status = status.map_err(|error| CliBridgeError::Unavailable(error.to_string()))?;
    if !status.success() {
        let _ = stderr.map_err(|error| CliBridgeError::Unavailable(error.to_string()))?;
        return Err(CliBridgeError::Unavailable(format!(
            "OpenCode exited unsuccessfully ({status})"
        )));
    }
    Ok(stdout)
}

pub async fn db_session_fingerprint(
    db_path: PathBuf,
    root_session_id: String,
) -> Option<(u64, u64)> {
    tokio::task::spawn_blocking(move || db_session_fingerprint_blocking(&db_path, &root_session_id))
        .await
        .ok()
        .flatten()
}

fn data_roots_for_home(home: &Path) -> Vec<PathBuf> {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let xdg_data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    data_roots_for_platform(
        home,
        current_desktop_platform(),
        appdata.as_deref(),
        xdg_data_home.as_deref(),
    )
}

fn data_roots_for_platform(
    home: &Path,
    platform: DesktopPlatform,
    appdata: Option<&Path>,
    xdg_data_home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // OpenCode's CLI resolves data storage through xdg-basedir, so the primary
    // session root is `$XDG_DATA_HOME/opencode` or `~/.local/share/opencode`
    // across platforms. Keep older native app roots as fallbacks.
    if let Some(xdg_data_home) = xdg_data_home {
        roots.push(xdg_data_home.join("opencode"));
    } else {
        roots.push(home.join(".local").join("share").join("opencode"));
    }

    if platform == DesktopPlatform::Macos {
        roots.push(
            home.join("Library")
                .join("Application Support")
                .join("opencode"),
        );
    } else if platform == DesktopPlatform::Windows {
        if let Some(appdata) = appdata {
            roots.push(appdata.join("opencode"));
        } else {
            roots.push(home.join("AppData").join("Roaming").join("opencode"));
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

#[derive(Debug, Clone)]
struct SessionRecord {
    directory: Option<String>,
    title: Option<String>,
    parent_session_id: Option<String>,
    source_file: String,
    created_at: Option<i64>,
    updated_at: Option<i64>,
    file_mtime: Option<i64>,
    raw: Value,
}

#[derive(Debug, Clone)]
struct MessageRecord {
    id: String,
    session_id: String,
    role: Option<String>,
    source_file: String,
    created_at: Option<i64>,
    file_mtime: Option<i64>,
    raw: Value,
}

#[derive(Debug, Clone)]
struct PartRecord {
    id: String,
    message_id: Option<String>,
    session_id: Option<String>,
    part_type: Option<String>,
    source_file: String,
    created_at: Option<i64>,
    file_mtime: Option<i64>,
    raw: Value,
}

async fn discover_recent_in(roots: &[PathBuf], now: i64, since_secs: i64) -> Vec<SessionLog> {
    discover_recent_in_impl(roots, now, since_secs, true).await
}

async fn discover_recent_in_impl(
    roots: &[PathBuf],
    now: i64,
    since_secs: i64,
    allow_sqlite: bool,
) -> Vec<SessionLog> {
    let cutoff = now - since_secs;
    let mut output = Vec::new();

    let mut sessions: HashMap<String, SessionRecord> = HashMap::new();
    let mut messages_by_session: BTreeMap<String, Vec<MessageRecord>> = BTreeMap::new();
    let mut parts_by_session: BTreeMap<String, Vec<PartRecord>> = BTreeMap::new();
    let mut message_to_session: HashMap<String, String> = HashMap::new();

    for root in roots {
        if allow_sqlite
            && let Ok(Some(logs)) = tokio::task::spawn_blocking({
                let root = root.clone();
                move || query_recent_db_session_sources(&root, cutoff)
            })
            .await
        {
            output.extend(logs);
            continue;
        }

        let session_dir = root.join("storage").join("session");
        let message_dir = root.join("storage").join("message");
        let part_dir = root.join("storage").join("part");

        let mut recent_file_session_ids = HashSet::new();

        for candidate in collect_recent_json_files(&session_dir, cutoff).await {
            let path = candidate.path;
            let Some(value) = read_json(&path).await else {
                continue;
            };
            let Some((session_id, record)) =
                parse_session_record(value, path, candidate.mtime_epoch)
            else {
                continue;
            };
            recent_file_session_ids.insert(session_id.clone());
            merge_session_records(&mut sessions, vec![(session_id, record)]);
        }

        load_session_file_ancestors(&session_dir, &mut sessions, &recent_file_session_ids).await;

        for candidate in collect_recent_json_files(&message_dir, cutoff).await {
            let path = candidate.path;
            let Some(value) = read_json(&path).await else {
                continue;
            };
            let Some(session_id) = value.get("sessionID").and_then(Value::as_str) else {
                continue;
            };
            let Some(message_id) = value.get("id").and_then(Value::as_str) else {
                continue;
            };

            let record = MessageRecord {
                id: message_id.to_string(),
                session_id: session_id.to_string(),
                role: value
                    .get("role")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source_file: path.to_string_lossy().to_string(),
                created_at: value
                    .pointer("/time/created")
                    .and_then(parse_epoch_from_json_value),
                file_mtime: candidate.mtime_epoch,
                raw: value,
            };

            message_to_session.insert(record.id.clone(), record.session_id.clone());
            messages_by_session
                .entry(record.session_id.clone())
                .or_default()
                .push(record);
        }

        for candidate in collect_recent_json_files(&part_dir, cutoff).await {
            let path = candidate.path;
            let Some(value) = read_json(&path).await else {
                continue;
            };
            let part_id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown-part")
                .to_string();

            let message_id = value
                .get("messageID")
                .and_then(Value::as_str)
                .map(str::to_string);
            let session_id = value
                .get("sessionID")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    message_id
                        .as_ref()
                        .and_then(|id| message_to_session.get(id).cloned())
                });

            let record = PartRecord {
                id: part_id,
                message_id,
                session_id: session_id.clone(),
                part_type: value
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source_file: path.to_string_lossy().to_string(),
                created_at: value
                    .pointer("/time/created")
                    .and_then(parse_epoch_from_json_value),
                file_mtime: candidate.mtime_epoch,
                raw: value,
            };

            if let Some(session_id) = session_id {
                parts_by_session.entry(session_id).or_default().push(record);
            }
        }
    }

    let file_logs = tokio::task::spawn_blocking(move || {
        render_session_logs(cutoff, sessions, messages_by_session, parts_by_session)
    })
    .await
    .unwrap_or_default();
    output.extend(file_logs);
    output
}

async fn discover_cwds_in(roots: &[PathBuf], now: i64, since_secs: i64) -> Vec<String> {
    let cutoff = now - since_secs;
    let mut cwds = HashSet::new();

    for root in roots {
        let db_path = root.join("opencode.db");
        if tokio::fs::try_exists(&db_path).await.unwrap_or(false) {
            let db_path_for_query = db_path.clone();
            let directories = tokio::task::spawn_blocking(move || {
                query_recent_session_directories_from_db(&db_path_for_query, cutoff)
            })
            .await
            .unwrap_or_default();
            for directory in directories {
                cwds.insert(directory);
            }
        }

        let session_dir = root.join("storage").join("session");
        for candidate in collect_recent_json_files(&session_dir, cutoff).await {
            let Some(value) = read_json(&candidate.path).await else {
                continue;
            };
            let cwd = value
                .get("directory")
                .or_else(|| value.get("cwd"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(cwd) = cwd {
                cwds.insert(cwd.to_string());
            }
        }
    }

    let mut out: Vec<String> = cwds.into_iter().collect();
    out.sort();
    out
}

fn parse_session_record(
    value: Value,
    path: PathBuf,
    file_mtime: Option<i64>,
) -> Option<(String, SessionRecord)> {
    let session_id = value
        .get("id")
        .or_else(|| value.get("sessionID"))
        .and_then(Value::as_str)?
        .to_string();

    let created_at = value
        .pointer("/time/created")
        .and_then(parse_epoch_from_json_value);
    let updated_at = value
        .pointer("/time/updated")
        .and_then(parse_epoch_from_json_value)
        .or(created_at);

    Some((
        session_id,
        SessionRecord {
            directory: value
                .get("directory")
                .and_then(Value::as_str)
                .map(str::to_string),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            parent_session_id: value
                .get("parentID")
                .or_else(|| value.get("parentSessionID"))
                .or_else(|| value.get("parent_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
            source_file: path.to_string_lossy().to_string(),
            created_at,
            updated_at,
            file_mtime,
            raw: value,
        },
    ))
}

async fn load_session_file_ancestors(
    session_dir: &Path,
    sessions: &mut HashMap<String, SessionRecord>,
    initial_session_ids: &HashSet<String>,
) {
    let mut pending: Vec<String> = initial_session_ids.iter().cloned().collect();
    let mut seen = HashSet::new();

    while let Some(session_id) = pending.pop() {
        if !seen.insert(session_id.clone()) {
            continue;
        }

        let Some(parent_session_id) = sessions
            .get(&session_id)
            .and_then(session_parent_link)
            .filter(|parent| !sessions.contains_key(parent))
        else {
            continue;
        };

        let Some(path) = find_session_json_file(session_dir, &parent_session_id).await else {
            continue;
        };
        let mtime_epoch = file_mtime_epoch(&path).await;
        let Some(value) = read_json(&path).await else {
            continue;
        };
        let Some((loaded_session_id, record)) = parse_session_record(value, path, mtime_epoch)
        else {
            continue;
        };

        if loaded_session_id != parent_session_id {
            continue;
        }

        merge_session_records(sessions, vec![(loaded_session_id.clone(), record)]);
        pending.push(loaded_session_id);
    }
}

fn render_session_logs(
    cutoff: i64,
    sessions: HashMap<String, SessionRecord>,
    mut messages_by_session: BTreeMap<String, Vec<MessageRecord>>,
    mut parts_by_session: BTreeMap<String, Vec<PartRecord>>,
) -> Vec<SessionLog> {
    let clusters = build_session_clusters(&sessions, &messages_by_session, &parts_by_session);
    let mut output = Vec::new();
    for cluster in clusters {
        let session_record = sessions.get(&cluster.root_session_id);
        let mut lines = Vec::new();
        let mut max_updated = 0i64;

        let session_created = session_record.and_then(|s| s.created_at);
        let session_updated = session_record.and_then(|s| s.updated_at);
        let cwd = cluster.cwd.clone();
        let title = session_record.and_then(|s| s.title.clone());
        let source_file = session_record.map(|s| s.source_file.clone());

        if let Some(ts) = session_updated
            .or(session_created)
            .or_else(|| session_record.and_then(|s| s.file_mtime))
        {
            max_updated = max_updated.max(ts);
        }

        let session_meta = json!({
            "type": "session_meta",
            "source": "opencode",
            "sessionID": cluster.root_session_id,
            "session_id": cluster.root_session_id,
            "rootSessionID": cluster.root_session_id,
            "sessionRole": "root",
            "directory": cwd,
            "cwd": cwd,
            "title": title,
            "source_file": source_file,
            "clusterSessionCount": cluster.session_ids.len(),
            "childSessionIDs": cluster.child_session_ids,
            "stitchedFromSessionIDs": cluster.session_ids,
            FORK_OBSERVATION_KEY: session_record
                .and_then(|record| record.raw.get(FORK_OBSERVATION_KEY))
                .cloned(),
            "time": {
                "created": session_created,
                "updated": session_updated,
            },
            "payload": session_record.map(|s| s.raw.clone()),
        });
        lines.push(session_meta.to_string());

        let mut ordered_records = Vec::new();

        for session_id in &cluster.session_ids {
            if let Some(record) = sessions.get(session_id)
                && let Some(ts) = record
                    .updated_at
                    .or(record.created_at)
                    .or(record.file_mtime)
            {
                max_updated = max_updated.max(ts);
            }

            if session_id == &cluster.root_session_id {
                continue;
            }

            if let Some(record) = sessions.get(session_id) {
                ordered_records.push(RenderedClusterRecord {
                    timestamp: record
                        .created_at
                        .or(record.updated_at)
                        .or(record.file_mtime),
                    kind_rank: 0,
                    session_id: session_id.clone(),
                    record_id: session_id.clone(),
                    line: json!({
                        "type": "session_member",
                        "source": "opencode",
                        "rootSessionID": cluster.root_session_id,
                        "originSessionID": session_id,
                        "sessionRole": "child",
                        "parentSessionID": session_parent_link(record),
                        "time": {
                            "created": record.created_at,
                            "updated": record.updated_at,
                        },
                        "source_file": record.source_file,
                        "payload": record.raw,
                    })
                    .to_string(),
                });
            }
        }

        for session_id in &cluster.session_ids {
            let mut messages = messages_by_session.remove(session_id).unwrap_or_default();
            messages.sort_by(|a, b| {
                a.created_at
                    .unwrap_or_default()
                    .cmp(&b.created_at.unwrap_or_default())
                    .then(a.id.cmp(&b.id))
            });

            let parent_session_id = sessions.get(session_id).and_then(session_parent_link);
            let session_role = if session_id == &cluster.root_session_id {
                "root"
            } else {
                "child"
            };

            for message in messages {
                if let Some(ts) = message.created_at.or(message.file_mtime) {
                    max_updated = max_updated.max(ts);
                }
                ordered_records.push(RenderedClusterRecord {
                    timestamp: message.created_at.or(message.file_mtime),
                    kind_rank: 1,
                    session_id: message.session_id.clone(),
                    record_id: message.id.clone(),
                    line: json!({
                        "type": "message",
                        "source": "opencode",
                        "rootSessionID": cluster.root_session_id,
                        "originSessionID": message.session_id,
                        "sessionRole": session_role,
                        "parentSessionID": parent_session_id,
                        "sessionID": message.session_id,
                        "messageID": message.id,
                        "role": message.role,
                        "time": {
                            "created": message.created_at,
                        },
                        "source_file": message.source_file,
                        "payload": message.raw,
                    })
                    .to_string(),
                });
            }
        }

        for session_id in &cluster.session_ids {
            let mut parts = parts_by_session.remove(session_id).unwrap_or_default();
            parts.sort_by(|a, b| {
                a.created_at
                    .unwrap_or_default()
                    .cmp(&b.created_at.unwrap_or_default())
                    .then(a.id.cmp(&b.id))
            });

            let parent_session_id = sessions.get(session_id).and_then(session_parent_link);
            let session_role = if session_id == &cluster.root_session_id {
                "root"
            } else {
                "child"
            };

            for part in parts {
                if let Some(ts) = part.created_at.or(part.file_mtime) {
                    max_updated = max_updated.max(ts);
                }
                ordered_records.push(RenderedClusterRecord {
                    timestamp: part.created_at.or(part.file_mtime),
                    kind_rank: 2,
                    session_id: part.session_id.clone().unwrap_or_default(),
                    record_id: part.id.clone(),
                    line: json!({
                        "type": "part",
                        "source": "opencode",
                        "rootSessionID": cluster.root_session_id,
                        "originSessionID": part.session_id,
                        "sessionRole": session_role,
                        "parentSessionID": parent_session_id,
                        "sessionID": part.session_id,
                        "messageID": part.message_id,
                        "partID": part.id,
                        "partType": part.part_type,
                        "time": {
                            "created": part.created_at,
                        },
                        "source_file": part.source_file,
                        "payload": part.raw,
                    })
                    .to_string(),
                });
            }
        }

        ordered_records.sort_by(|a, b| {
            a.timestamp
                .unwrap_or_default()
                .cmp(&b.timestamp.unwrap_or_default())
                .then(a.kind_rank.cmp(&b.kind_rank))
                .then(a.session_id.cmp(&b.session_id))
                .then(a.record_id.cmp(&b.record_id))
        });

        for record in ordered_records {
            lines.push(record.line);
        }

        if max_updated == 0 {
            continue;
        }
        if max_updated < cutoff {
            continue;
        }

        output.push(SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::OpenCode,
            source: SessionSource::Inline {
                label: format!("opencode:{}", cluster.root_session_id),
                content: lines.join("\n"),
            },
            updated_at: Some(max_updated),
        });
    }

    annotate_opencode_inline_forks(&mut output);
    output
}

#[derive(Debug, Clone)]
struct OpenCodeSessionCluster {
    root_session_id: String,
    session_ids: Vec<String>,
    child_session_ids: Vec<String>,
    cwd: Option<String>,
}

#[derive(Debug, Clone)]
struct RenderedClusterRecord {
    timestamp: Option<i64>,
    kind_rank: u8,
    session_id: String,
    record_id: String,
    line: String,
}

fn build_session_clusters(
    sessions: &HashMap<String, SessionRecord>,
    messages_by_session: &BTreeMap<String, Vec<MessageRecord>>,
    parts_by_session: &BTreeMap<String, Vec<PartRecord>>,
) -> Vec<OpenCodeSessionCluster> {
    let mut session_ids: HashSet<String> = HashSet::new();
    session_ids.extend(sessions.keys().cloned());
    session_ids.extend(messages_by_session.keys().cloned());
    session_ids.extend(parts_by_session.keys().cloned());

    let mut root_to_sessions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut resolved_roots: HashMap<String, String> = HashMap::new();

    for session_id in session_ids {
        let root_session_id = resolve_root_session_id(&session_id, sessions, &mut resolved_roots);
        root_to_sessions
            .entry(root_session_id)
            .or_default()
            .push(session_id);
    }

    let mut clusters = Vec::new();
    for (root_session_id, mut cluster_session_ids) in root_to_sessions {
        cluster_session_ids.sort_by_key(|session_id| session_sort_tuple(session_id, sessions));
        if let Some(root_idx) = cluster_session_ids
            .iter()
            .position(|id| id == &root_session_id)
        {
            cluster_session_ids.swap(0, root_idx);
        }

        let cwd = sessions
            .get(&root_session_id)
            .and_then(|record| record.directory.clone())
            .or_else(|| {
                cluster_session_ids.iter().find_map(|session_id| {
                    sessions
                        .get(session_id)
                        .and_then(|record| record.directory.clone())
                })
            });

        let child_session_ids = cluster_session_ids
            .iter()
            .filter(|session_id| *session_id != &root_session_id)
            .cloned()
            .collect();

        clusters.push(OpenCodeSessionCluster {
            root_session_id,
            session_ids: cluster_session_ids,
            child_session_ids,
            cwd,
        });
    }

    clusters.sort_by(|a, b| {
        cluster_sort_tuple(a, sessions)
            .cmp(&cluster_sort_tuple(b, sessions))
            .then(a.root_session_id.cmp(&b.root_session_id))
    });
    clusters
}

fn resolve_root_session_id(
    session_id: &str,
    sessions: &HashMap<String, SessionRecord>,
    resolved_roots: &mut HashMap<String, String>,
) -> String {
    if let Some(root) = resolved_roots.get(session_id) {
        return root.clone();
    }

    let mut lineage = Vec::new();
    let mut seen = HashSet::new();
    let mut current = session_id.to_string();

    loop {
        if let Some(root) = resolved_roots.get(&current) {
            let root = root.clone();
            for id in lineage {
                resolved_roots.insert(id, root.clone());
            }
            return root;
        }

        if !seen.insert(current.clone()) {
            let mut cycle: Vec<_> = seen.into_iter().collect();
            cycle.sort_by(|a, b| {
                session_sort_tuple(a, sessions).cmp(&session_sort_tuple(b, sessions))
            });
            let root = cycle
                .into_iter()
                .next()
                .unwrap_or_else(|| session_id.to_string());
            for id in lineage {
                resolved_roots.insert(id, root.clone());
            }
            return root;
        }

        lineage.push(current.clone());
        let Some(parent_session_id) = sessions
            .get(&current)
            .and_then(session_parent_link)
            .filter(|parent| parent != &current)
        else {
            for id in lineage {
                resolved_roots.insert(id, current.clone());
            }
            return current;
        };

        if !sessions.contains_key(&parent_session_id) {
            for id in lineage {
                resolved_roots.insert(id, current.clone());
            }
            return current;
        }

        current = parent_session_id;
    }
}

fn session_parent_link(record: &SessionRecord) -> Option<String> {
    record.parent_session_id.clone().or_else(|| {
        record
            .raw
            .get("parentID")
            .or_else(|| record.raw.get("parentSessionID"))
            .or_else(|| record.raw.get("parent_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn session_sort_tuple(
    session_id: &str,
    sessions: &HashMap<String, SessionRecord>,
) -> (i64, i64, String) {
    let created = sessions
        .get(session_id)
        .and_then(|record| record.created_at)
        .unwrap_or_default();
    let updated = sessions
        .get(session_id)
        .and_then(|record| record.updated_at.or(record.file_mtime))
        .unwrap_or_default();
    (created, updated, session_id.to_string())
}

fn cluster_sort_tuple(
    cluster: &OpenCodeSessionCluster,
    sessions: &HashMap<String, SessionRecord>,
) -> (i64, i64) {
    let created = sessions
        .get(&cluster.root_session_id)
        .and_then(|record| record.created_at)
        .unwrap_or_default();
    let updated = cluster
        .session_ids
        .iter()
        .filter_map(|session_id| {
            sessions.get(session_id).and_then(|record| {
                record
                    .updated_at
                    .or(record.created_at)
                    .or(record.file_mtime)
            })
        })
        .max()
        .unwrap_or_default();
    (created, updated)
}

fn merge_session_records(
    sessions: &mut HashMap<String, SessionRecord>,
    incoming: Vec<(String, SessionRecord)>,
) {
    for (session_id, record) in incoming {
        match sessions.get(&session_id) {
            None => {
                sessions.insert(session_id, record);
            }
            Some(existing) => {
                let record_recency = record.updated_at.or(record.file_mtime).unwrap_or_default();
                let existing_recency = existing
                    .updated_at
                    .or(existing.file_mtime)
                    .unwrap_or_default();

                let (mut merged, fallback) = if record_recency > existing_recency {
                    (record, existing)
                } else {
                    (existing.clone(), &record)
                };

                if merged.parent_session_id.is_none() {
                    merged.parent_session_id = fallback.parent_session_id.clone();
                }
                if merged.directory.is_none() {
                    merged.directory = fallback.directory.clone();
                }
                if merged.title.is_none() {
                    merged.title = fallback.title.clone();
                }

                sessions.insert(session_id, merged);
            }
        }
    }
}

#[derive(Debug, Default)]
struct DbRecords {
    sessions: Vec<(String, SessionRecord)>,
    messages: Vec<MessageRecord>,
    parts: Vec<PartRecord>,
}

struct DbSessionRowData {
    session_id: String,
    parent_session_id: Option<String>,
    directory: Option<String>,
    title: Option<String>,
    time_created: Option<i64>,
    time_updated: Option<i64>,
    raw_json: Option<String>,
}

struct DbSessionMetadataRow {
    session_id: String,
    directory: Option<String>,
    title: Option<String>,
}

fn query_recent_db_session_sources(root: &Path, cutoff: i64) -> Option<Vec<SessionLog>> {
    let db_path = root.join("opencode.db");
    if !db_path.exists() {
        return None;
    }

    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let recent_session_ids = recent_db_session_ids(&conn, cutoff);
    if recent_session_ids.is_empty() {
        return Some(Vec::new());
    }
    let cluster_session_ids = db_session_ids_with_ancestors(&conn, &recent_session_ids);
    let sessions: HashMap<String, SessionRecord> =
        fetch_db_sessions_light(&conn, &db_path, &cluster_session_ids)
            .into_iter()
            .collect();
    let activity_by_session = fetch_db_activity_by_session(&conn, &cluster_session_ids);
    let clusters = build_session_clusters(&sessions, &BTreeMap::new(), &BTreeMap::new());

    let mut out = Vec::new();
    for cluster in clusters {
        let mut max_updated = 0i64;
        for session_id in &cluster.session_ids {
            if let Some(record) = sessions.get(session_id)
                && let Some(ts) = record
                    .updated_at
                    .or(record.created_at)
                    .or(record.file_mtime)
            {
                max_updated = max_updated.max(ts);
            }
            if let Some(ts) = activity_by_session.get(session_id) {
                max_updated = max_updated.max(*ts);
            }
        }
        if max_updated == 0 || max_updated < cutoff {
            continue;
        }
        out.push(SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::OpenCode,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: db_path.clone(),
                session_id: cluster.root_session_id,
            },
            updated_at: Some(max_updated),
        });
    }
    out.sort_by(|a, b| {
        a.updated_at
            .unwrap_or_default()
            .cmp(&b.updated_at.unwrap_or_default())
            .then(a.source_label().cmp(&b.source_label()))
    });
    Some(out)
}

pub async fn render_db_session(db_path: PathBuf, root_session_id: String) -> Option<String> {
    tokio::task::spawn_blocking(move || render_db_session_blocking(&db_path, &root_session_id))
        .await
        .ok()
        .flatten()
}

pub async fn db_session_metadata(
    db_path: PathBuf,
    root_session_id: String,
) -> Option<crate::discovery::scanner::SessionMetadata> {
    tokio::task::spawn_blocking(move || db_session_metadata_blocking(&db_path, &root_session_id))
        .await
        .ok()
        .flatten()
}

fn render_db_session_blocking(db_path: &Path, root_session_id: &str) -> Option<String> {
    let mut records = query_db_records_for_root(db_path, root_session_id)?;
    if let Some(observation) = infer_db_fork_observation(db_path, root_session_id, &records)
        && let Some((_, session)) = records
            .sessions
            .iter_mut()
            .find(|(session_id, _)| session_id == root_session_id)
    {
        session.raw[FORK_OBSERVATION_KEY] =
            serde_json::to_value(observation).unwrap_or(Value::Null);
    }
    let logs = render_session_logs(
        0,
        records.sessions.into_iter().collect(),
        group_messages_by_session(records.messages),
        group_parts_by_session(records.parts),
    );
    let expected_label = format!("opencode:{root_session_id}");
    logs.into_iter().find_map(|log| {
        if log.source_label() != expected_label {
            return None;
        }
        match log.source {
            SessionSource::Inline { content, .. } => Some(content),
            SessionSource::File(_) | SessionSource::ProviderDb { .. } => None,
        }
    })
}

fn db_session_metadata_blocking(
    db_path: &Path,
    root_session_id: &str,
) -> Option<crate::discovery::scanner::SessionMetadata> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let row = query_db_session_metadata_row(&conn, root_session_id)?;
    let title_source = row.title.as_ref().map(|_| TitleSource::Explicit);
    Some(crate::discovery::scanner::SessionMetadata {
        session_id: Some(row.session_id),
        cwd: row.directory,
        agent_type: Some(AgentKind::OpenCode),
        title: row.title,
        title_source,
    })
}

fn query_db_records_for_root(db_path: &Path, root_session_id: &str) -> Option<DbRecords> {
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    query_db_records_for_root_connection(&conn, db_path, root_session_id)
}

fn query_db_records_for_root_connection(
    conn: &Connection,
    db_path: &Path,
    root_session_id: &str,
) -> Option<DbRecords> {
    let cluster_session_ids = db_session_ids_for_root(conn, root_session_id);
    if cluster_session_ids.is_empty() {
        return None;
    }
    Some(DbRecords {
        sessions: fetch_db_sessions(conn, db_path, &cluster_session_ids),
        messages: fetch_db_messages(conn, db_path, &cluster_session_ids),
        parts: fetch_db_parts(conn, db_path, &cluster_session_ids),
    })
}

fn infer_db_fork_observation(
    db_path: &Path,
    child_session_id: &str,
    child_records: &DbRecords,
) -> Option<ForkObservation> {
    let (_, child_session) = child_records
        .sessions
        .iter()
        .find(|(session_id, _)| session_id == child_session_id)?;
    let parent_title = child_session
        .title
        .as_deref()
        .and_then(opencode_fork_parent_title)?;
    let child_items = opencode_record_visible_items(child_records, child_session_id);
    if child_items.len() < 3 {
        return None;
    }

    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT id FROM session WHERE id != ?1 AND directory = ?2 AND title = ?3 AND parent_id IS NULL LIMIT 101",
        )
        .ok()?;
    let rows = stmt
        .query_map(
            params![child_session_id, child_session.directory, parent_title],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    let mut matches = rows
        .flatten()
        .filter_map(|parent_session_id| {
            let records = query_db_records_for_root_connection(&conn, db_path, &parent_session_id)?;
            let (_, parent_session) = records
                .sessions
                .iter()
                .find(|(session_id, _)| session_id == &parent_session_id)?;
            if parent_session
                .created_at
                .zip(child_session.created_at)
                .is_some_and(|(parent, child)| parent > child)
            {
                return None;
            }
            let items = opencode_record_visible_items(&records, &parent_session_id);
            strict_opencode_visible_prefix(&items, &child_items)
                .then_some((parent_session_id, items.len()))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(_, inherited)| std::cmp::Reverse(*inherited));
    let (parent_session_id, inherited) = matches.first()?;
    if matches.get(1).is_some_and(|(_, other)| other == inherited) {
        return None;
    }
    Some(opencode_fork_observation(parent_session_id, *inherited))
}

fn opencode_record_visible_items(
    records: &DbRecords,
    root_session_id: &str,
) -> Vec<(String, String)> {
    let mut messages = records
        .messages
        .iter()
        .filter(|message| message.session_id == root_session_id)
        .collect::<Vec<_>>();
    messages.sort_by(|a, b| {
        a.created_at
            .unwrap_or_default()
            .cmp(&b.created_at.unwrap_or_default())
            .then(a.id.cmp(&b.id))
    });
    let mut out = Vec::new();
    for message in messages {
        let Some(role) = message
            .role
            .as_deref()
            .filter(|role| matches!(*role, "user" | "assistant"))
        else {
            continue;
        };
        let mut parts = records
            .parts
            .iter()
            .filter(|part| part.message_id.as_deref() == Some(message.id.as_str()))
            .filter(|part| part.part_type.as_deref() == Some("text"))
            .collect::<Vec<_>>();
        parts.sort_by(|a, b| {
            a.created_at
                .unwrap_or_default()
                .cmp(&b.created_at.unwrap_or_default())
                .then(a.id.cmp(&b.id))
        });
        for part in parts {
            let Some(text) = part.raw.get("text").and_then(Value::as_str) else {
                continue;
            };
            let text = normalize_opencode_visible_text(text);
            if !text.is_empty() {
                out.push((role.to_string(), text));
            }
        }
    }
    out
}

fn annotate_opencode_inline_forks(logs: &mut [SessionLog]) {
    let snapshots = logs
        .iter()
        .enumerate()
        .filter_map(|(index, log)| {
            let SessionSource::Inline { content, .. } = &log.source else {
                return None;
            };
            let metadata = content
                .lines()
                .next()
                .and_then(|line| serde_json::from_str::<Value>(line).ok())?;
            Some((
                index,
                metadata.get("sessionID")?.as_str()?.to_string(),
                metadata
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                metadata
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                metadata.pointer("/time/created").and_then(Value::as_i64),
                opencode_content_visible_items(content),
            ))
        })
        .collect::<Vec<_>>();

    for (child_index, child_id, child_cwd, child_title, child_created, child_items) in &snapshots {
        let Some(parent_title) = child_title.as_deref().and_then(opencode_fork_parent_title) else {
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
                        && (*parent_created)
                            .zip(*child_created)
                            .is_none_or(|(parent, child)| parent <= child)
                        && strict_opencode_visible_prefix(parent_items, child_items)
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
        let observation = opencode_fork_observation(&parent.1, parent.5.len());
        let SessionSource::Inline { content, .. } = &mut logs[*child_index].source else {
            continue;
        };
        set_opencode_fork_observation(content, &observation);
    }
}

fn opencode_content_visible_items(content: &str) -> Vec<(String, String)> {
    let values = content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let roles = values
        .iter()
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("message"))
        .filter(|value| value.get("sessionRole").and_then(Value::as_str) == Some("root"))
        .filter_map(|value| {
            Some((
                value.get("messageID")?.as_str()?.to_string(),
                value.get("role")?.as_str()?.to_string(),
            ))
        })
        .collect::<HashMap<_, _>>();
    values
        .iter()
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("part"))
        .filter(|value| value.get("sessionRole").and_then(Value::as_str) == Some("root"))
        .filter(|value| value.get("partType").and_then(Value::as_str) == Some("text"))
        .filter_map(|value| {
            let role = roles.get(value.get("messageID")?.as_str()?)?;
            if !matches!(role.as_str(), "user" | "assistant") {
                return None;
            }
            let text = value.pointer("/payload/text")?.as_str()?;
            let text = normalize_opencode_visible_text(text);
            (!text.is_empty()).then(|| (role.clone(), text))
        })
        .collect()
}

fn opencode_fork_parent_title(title: &str) -> Option<&str> {
    let (parent, suffix) = title.trim().rsplit_once(" (fork #")?;
    let number = suffix.strip_suffix(')')?;
    (!parent.is_empty() && !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some(parent)
}

fn normalize_opencode_visible_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strict_opencode_visible_prefix(parent: &[(String, String)], child: &[(String, String)]) -> bool {
    parent.len() >= 2 && parent.len() < child.len() && child.starts_with(parent)
}

fn opencode_fork_observation(parent_session_id: &str, inherited: usize) -> ForkObservation {
    ForkObservation {
        parent_agent: "opencode".to_string(),
        parent_agent_session_id: parent_session_id.to_string(),
        fork_kind: "fork".to_string(),
        provider_fork_point_id: None,
        detection_source: "normalized_content".to_string(),
        confidence: 85,
        inherited_item_count: inherited.try_into().ok(),
        extractor_version: "opencode-title-prefix-v1".to_string(),
    }
}

fn set_opencode_fork_observation(content: &mut String, observation: &ForkObservation) {
    let Some((first, rest)) = content.split_once('\n') else {
        return;
    };
    let Ok(mut metadata) = serde_json::from_str::<Value>(first) else {
        return;
    };
    metadata[FORK_OBSERVATION_KEY] = serde_json::to_value(observation).unwrap_or(Value::Null);
    *content = format!("{metadata}\n{rest}");
}

fn db_session_exists(db_path: &Path, root_session_id: &str) -> bool {
    if !db_path.exists() {
        return false;
    }
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    conn.query_row(
        "SELECT 1 FROM session WHERE id = ?1 LIMIT 1",
        params![root_session_id],
        |_| Ok(()),
    )
    .is_ok()
}

fn db_session_fingerprint_blocking(db_path: &Path, root_session_id: &str) -> Option<(u64, u64)> {
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let cluster_session_ids = db_session_ids_for_root(&conn, root_session_id);
    if cluster_session_ids.is_empty() {
        return None;
    }

    let placeholders = sql_in_placeholders(cluster_session_ids.len());
    let (session_count, session_updated) = count_and_max_updated(
        &conn,
        &format!(
            "SELECT COUNT(*), MAX(COALESCE(time_updated, time_created, 0)) FROM session WHERE id IN ({placeholders})"
        ),
        &cluster_session_ids,
    );
    let (message_count, message_updated) = count_and_max_updated(
        &conn,
        &format!(
            "SELECT COUNT(*), MAX(COALESCE(time_updated, time_created, 0)) FROM message WHERE session_id IN ({placeholders})"
        ),
        &cluster_session_ids,
    );
    let (part_count, part_updated) = count_and_max_updated(
        &conn,
        &format!(
            "SELECT COUNT(*), MAX(COALESCE(time_updated, time_created, 0)) FROM part WHERE session_id IN ({placeholders})"
        ),
        &cluster_session_ids,
    );

    let latest = session_updated.max(message_updated).max(part_updated);
    let rows = session_count + message_count + part_count;
    Some((latest.max(0) as u64, rows))
}

fn count_and_max_updated(
    conn: &Connection,
    sql: &str,
    session_ids: &HashSet<String>,
) -> (u64, i64) {
    conn.query_row(sql, params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, i64>(0).unwrap_or(0).max(0) as u64,
            row.get::<_, Option<i64>>(1).unwrap_or(None).unwrap_or(0),
        ))
    })
    .unwrap_or((0, 0))
}

fn recent_db_session_ids(conn: &Connection, cutoff: i64) -> HashSet<String> {
    let mut ids = HashSet::new();

    for query in [
        "SELECT id FROM session WHERE COALESCE(time_updated, time_created) >= ?1",
        "SELECT DISTINCT session_id FROM message WHERE COALESCE(time_updated, time_created) >= ?1",
        "SELECT DISTINCT session_id FROM part WHERE COALESCE(time_updated, time_created) >= ?1",
    ] {
        let Ok(mut stmt) = conn.prepare(query) else {
            continue;
        };
        let Ok(rows) = stmt.query_map(params![cutoff * 1000], |row| row.get::<_, String>(0)) else {
            continue;
        };
        for id in rows.flatten() {
            ids.insert(id);
        }
    }

    ids
}

fn group_messages_by_session(messages: Vec<MessageRecord>) -> BTreeMap<String, Vec<MessageRecord>> {
    let mut grouped: BTreeMap<String, Vec<MessageRecord>> = BTreeMap::new();
    for message in messages {
        grouped
            .entry(message.session_id.clone())
            .or_default()
            .push(message);
    }
    grouped
}

fn group_parts_by_session(parts: Vec<PartRecord>) -> BTreeMap<String, Vec<PartRecord>> {
    let mut grouped: BTreeMap<String, Vec<PartRecord>> = BTreeMap::new();
    for part in parts {
        if let Some(session_id) = part.session_id.clone() {
            grouped.entry(session_id).or_default().push(part);
        }
    }
    grouped
}

fn db_session_ids_with_ancestors(
    conn: &Connection,
    session_ids: &HashSet<String>,
) -> HashSet<String> {
    let mut all_ids = session_ids.clone();
    let mut frontier = session_ids.clone();

    while !frontier.is_empty() {
        let placeholders = sql_in_placeholders(frontier.len());
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT id, parent_id FROM session WHERE id IN ({placeholders})"
        )) else {
            break;
        };

        let Ok(rows) = stmt.query_map(params_from_iter(frontier.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
            ))
        }) else {
            break;
        };

        let mut next_frontier = HashSet::new();
        for (_, parent_id) in rows.flatten() {
            if let Some(parent_id) = parent_id
                && all_ids.insert(parent_id.clone())
            {
                next_frontier.insert(parent_id);
            }
        }
        frontier = next_frontier;
    }

    all_ids
}

fn db_session_ids_for_root(conn: &Connection, root_session_id: &str) -> HashSet<String> {
    let mut all_ids = HashSet::from([root_session_id.to_string()]);
    let mut frontier = HashSet::from([root_session_id.to_string()]);

    while !frontier.is_empty() {
        let placeholders = sql_in_placeholders(frontier.len());
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT id FROM session WHERE parent_id IN ({placeholders})"
        )) else {
            break;
        };

        let Ok(rows) = stmt.query_map(params_from_iter(frontier.iter()), |row| {
            row.get::<_, String>(0)
        }) else {
            break;
        };

        let mut next_frontier = HashSet::new();
        for child_id in rows.flatten() {
            if all_ids.insert(child_id.clone()) {
                next_frontier.insert(child_id);
            }
        }
        frontier = next_frontier;
    }

    all_ids
}

fn fetch_db_sessions(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<(String, SessionRecord)> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, parent_id, directory, title, time_created, time_updated, data FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
                row.get::<_, String>(2).ok(),
                row.get::<_, String>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, i64>(5).ok(),
                row.get::<_, String>(6).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, parent_session_id, directory, title, time_created, time_updated, raw_json)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json,
                        },
                    )
                },
            )
            .collect();
    }

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, directory, title, time_created, time_updated, data FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1).ok(),
                row.get::<_, String>(2).ok(),
                row.get::<_, i64>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, String>(5).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, directory, title, time_created, time_updated, raw_json)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id: None,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json,
                        },
                    )
                },
            )
            .collect();
    }

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, parent_id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
                row.get::<_, String>(2).ok(),
                row.get::<_, String>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, i64>(5).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, parent_session_id, directory, title, time_created, time_updated)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json: None,
                        },
                    )
                },
            )
            .collect();
    }

    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1).ok(),
            row.get::<_, String>(2).ok(),
            row.get::<_, i64>(3).ok(),
            row.get::<_, i64>(4).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(
            |(session_id, directory, title, time_created, time_updated)| {
                build_db_session_record(
                    db_path,
                    DbSessionRowData {
                        session_id,
                        parent_session_id: None,
                        directory,
                        title,
                        time_created,
                        time_updated,
                        raw_json: None,
                    },
                )
            },
        )
        .collect()
}

fn fetch_db_sessions_light(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<(String, SessionRecord)> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, parent_id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
                row.get::<_, String>(2).ok(),
                row.get::<_, String>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, i64>(5).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, parent_session_id, directory, title, time_created, time_updated)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json: None,
                        },
                    )
                },
            )
            .collect();
    }

    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1).ok(),
            row.get::<_, String>(2).ok(),
            row.get::<_, i64>(3).ok(),
            row.get::<_, i64>(4).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(
            |(session_id, directory, title, time_created, time_updated)| {
                build_db_session_record(
                    db_path,
                    DbSessionRowData {
                        session_id,
                        parent_session_id: None,
                        directory,
                        title,
                        time_created,
                        time_updated,
                        raw_json: None,
                    },
                )
            },
        )
        .collect()
}

fn fetch_db_activity_by_session(
    conn: &Connection,
    session_ids: &HashSet<String>,
) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    if session_ids.is_empty() {
        return out;
    }
    let placeholders = sql_in_placeholders(session_ids.len());
    for table in ["message", "part"] {
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT session_id, time_created, time_updated FROM {table} WHERE session_id IN ({placeholders})"
        )) else {
            continue;
        };
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1).ok(),
                row.get::<_, i64>(2).ok(),
            ))
        }) else {
            continue;
        };
        for (session_id, time_created, time_updated) in rows.flatten() {
            if let Some(ts) =
                normalize_db_epoch(time_created).or_else(|| normalize_db_epoch(time_updated))
            {
                out.entry(session_id)
                    .and_modify(|current| *current = (*current).max(ts))
                    .or_insert(ts);
            }
        }
    }
    out
}

fn query_db_session_metadata_row(
    conn: &Connection,
    session_id: &str,
) -> Option<DbSessionMetadataRow> {
    if let Ok(mut stmt) =
        conn.prepare("SELECT id, directory, title FROM session WHERE id = ?1 LIMIT 1")
    {
        return stmt
            .query_row(params![session_id], |row| {
                Ok(DbSessionMetadataRow {
                    session_id: row.get::<_, String>(0)?,
                    directory: row.get::<_, String>(1).ok(),
                    title: row.get::<_, String>(2).ok().and_then(|t| {
                        let trimmed = t.trim().to_string();
                        (!trimmed.is_empty()).then_some(trimmed)
                    }),
                })
            })
            .ok();
    }
    None
}

fn build_db_session_record(db_path: &Path, row: DbSessionRowData) -> (String, SessionRecord) {
    let DbSessionRowData {
        session_id,
        parent_session_id,
        directory,
        title,
        time_created,
        time_updated,
        raw_json,
    } = row;

    let raw = raw_json
        .as_deref()
        .and_then(parse_json)
        .unwrap_or_else(|| json!({}));
    let raw = merge_json_object(
        raw,
        json!({
            "id": session_id.clone(),
            "parentID": parent_session_id.clone(),
            "directory": directory.clone(),
            "title": title.clone(),
            "time": {
                "created": normalize_db_epoch(time_created),
                "updated": normalize_db_epoch(time_updated),
            }
        }),
    );
    let record = SessionRecord {
        directory,
        title,
        parent_session_id,
        source_file: format!("{}#session/{}", db_path.display(), session_id),
        created_at: normalize_db_epoch(time_created),
        updated_at: normalize_db_epoch(time_updated),
        file_mtime: None,
        raw,
    };
    (session_id, record)
}

fn fetch_db_messages(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<MessageRecord> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, session_id, time_created, time_updated, data FROM message WHERE session_id IN ({placeholders})"
    ))
    else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2).ok(),
            row.get::<_, i64>(3).ok(),
            row.get::<_, String>(4).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(|(id, session_id, time_created, time_updated, raw_json)| {
            let raw = raw_json
                .as_deref()
                .and_then(parse_json)
                .unwrap_or_else(|| json!({}));
            let raw = merge_json_object(
                raw,
                json!({
                    "id": id,
                    "sessionID": session_id,
                    "time": {
                        "created": normalize_db_epoch(time_created),
                        "updated": normalize_db_epoch(time_updated),
                    }
                }),
            );

            MessageRecord {
                id: id.clone(),
                session_id: session_id.clone(),
                role: raw.get("role").and_then(Value::as_str).map(str::to_string),
                source_file: format!("{}#message/{}", db_path.display(), id),
                created_at: raw
                    .pointer("/time/created")
                    .and_then(parse_epoch_from_json_value)
                    .or_else(|| normalize_db_epoch(time_created))
                    .or_else(|| normalize_db_epoch(time_updated)),
                file_mtime: None,
                raw,
            }
        })
        .collect()
}

fn fetch_db_parts(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<PartRecord> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, message_id, session_id, time_created, time_updated, data FROM part WHERE session_id IN ({placeholders})"
    ))
    else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3).ok(),
            row.get::<_, i64>(4).ok(),
            row.get::<_, String>(5).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(
            |(id, message_id, session_id, time_created, time_updated, raw_json)| {
                let raw = raw_json
                    .as_deref()
                    .and_then(parse_json)
                    .unwrap_or_else(|| json!({}));
                let raw = merge_json_object(
                    raw,
                    json!({
                        "id": id,
                        "messageID": message_id,
                        "sessionID": session_id,
                        "time": {
                            "created": normalize_db_epoch(time_created),
                            "updated": normalize_db_epoch(time_updated),
                        }
                    }),
                );

                PartRecord {
                    id: id.clone(),
                    message_id: Some(message_id),
                    session_id: Some(session_id),
                    part_type: raw.get("type").and_then(Value::as_str).map(str::to_string),
                    source_file: format!("{}#part/{}", db_path.display(), id),
                    created_at: raw
                        .pointer("/time/created")
                        .and_then(parse_epoch_from_json_value)
                        .or_else(|| normalize_db_epoch(time_created))
                        .or_else(|| normalize_db_epoch(time_updated)),
                    file_mtime: None,
                    raw,
                }
            },
        )
        .collect()
}

fn parse_json(raw_json: &str) -> Option<Value> {
    serde_json::from_str(raw_json).ok()
}

fn sql_in_placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

fn merge_json_object(base: Value, fallback: Value) -> Value {
    match (base, fallback) {
        (Value::Object(mut base), Value::Object(fallback)) => {
            for (key, value) in fallback {
                base.entry(key).or_insert(value);
            }
            Value::Object(base)
        }
        (base, _) => base,
    }
}

fn normalize_db_epoch(value: Option<i64>) -> Option<i64> {
    value.and_then(|epoch| parse_epoch_from_json_value(&Value::from(epoch)))
}

#[derive(Debug)]
struct JsonCandidate {
    path: PathBuf,
    mtime_epoch: Option<i64>,
}

async fn collect_recent_json_files(root: &Path, cutoff: i64) -> Vec<JsonCandidate> {
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
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|e| e.to_str()) == Some("json")
            {
                let mtime_epoch = file_mtime_epoch(&path).await;
                if mtime_epoch.is_some_and(|mtime| mtime < cutoff) {
                    continue;
                }
                out.push(JsonCandidate { path, mtime_epoch });
            }
        }
    }

    out
}

async fn find_session_json_file(root: &Path, session_id: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let expected_file_name = format!("{session_id}.json");

    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await.ok()?;

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

            if file_type.is_file()
                && path.file_name().and_then(|name| name.to_str()) == Some(&expected_file_name)
            {
                return Some(path);
            }
        }
    }

    None
}

async fn read_json(path: &Path) -> Option<Value> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str::<Value>(&content).ok()
}

async fn file_mtime_epoch(path: &Path) -> Option<i64> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_secs() as i64)
}

fn parse_epoch_from_json_value(value: &Value) -> Option<i64> {
    if let Some(v) = value.as_i64() {
        if v > 1_000_000_000_000 {
            return Some(v / 1000);
        }
        if v > 0 {
            return Some(v);
        }
    }

    if let Some(v) = value.as_u64() {
        if v > 1_000_000_000_000 {
            return Some((v / 1000) as i64);
        }
        if v > 0 {
            return Some(v as i64);
        }
    }

    None
}

#[cfg(test)]
#[path = "tests/opencode_tests.rs"]
mod tests;
#[cfg(test)]
fn cli_record(data_len: usize) -> CliRecordRow {
    CliRecordRow {
        kind: "message".into(),
        id: "msg_test".into(),
        message_id: None,
        session_id: "ses_test".into(),
        time_created: None,
        time_updated: None,
        data_len,
    }
}

#[cfg(test)]
#[test]
fn chunked_db_reconstruction_caps_individual_and_aggregate_data() {
    assert!(cli_record_data_within_limit(&[cli_record(
        CLI_EXPORT_MAX_BYTES
    )]));
    assert!(!cli_record_data_within_limit(&[cli_record(
        CLI_EXPORT_MAX_BYTES + 1
    )]));
    assert!(!cli_record_data_within_limit(&[
        cli_record(CLI_EXPORT_MAX_BYTES),
        cli_record(1),
    ]));
}

#[cfg(test)]
#[test]
fn metadata_only_cwds_count_roots_not_descendants() {
    let row = |id: &str, parent_id: Option<&str>, directory: Option<&str>| CliSessionRow {
        id: id.into(),
        parent_id: parent_id.map(str::to_string),
        directory: directory.map(str::to_string),
        title: None,
        time_created: None,
        time_updated: None,
    };
    let directories = cli_root_directories(vec![
        row("ses_root", None, Some("/home/dev/repo")),
        row("ses_child", Some("ses_root"), Some("/home/dev/repo")),
        row("ses_no_cwd", None, None),
    ]);

    assert_eq!(directories, vec!["/home/dev/repo"]);
}

#[cfg(test)]
#[test]
fn title_query_contains_only_valid_requested_ids() {
    let ids = HashSet::from([
        "ses_two".to_string(),
        "ses_one".to_string(),
        "bad'id".to_string(),
    ]);
    let query = cli_title_query(&ids).unwrap();

    assert!(query.contains("'ses_one','ses_two'"));
    assert!(!query.contains("bad'id"));
    assert!(!query.to_ascii_lowercase().contains("export"));
}
