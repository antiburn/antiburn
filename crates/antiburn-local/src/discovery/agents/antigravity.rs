//! Antigravity log discovery.
//!
//! Three source kinds, all surfaced as `AgentKind::Antigravity`:
//!
//! 1. **IDE workspace storage** (Antigravity v1 layout only).
//!    Paths:
//!    - macOS: `~/Library/Application Support/Antigravity/User/workspaceStorage/*/chatSessions/*.json`
//!    - Linux: `~/.config/Antigravity/User/workspaceStorage/*/chatSessions/*.json`
//!    - Windows: `%APPDATA%\Antigravity\User\workspaceStorage\*\chatSessions\*.json`
//!
//!    Antigravity IDE 2.0 (verified on a live install 2026-05-25) moved its
//!    app-data directory to `Antigravity IDE/` (note the space + "IDE" suffix)
//!    and **no longer writes `chatSessions/`** under workspaceStorage. v2 IDE
//!    chat data lives in the Gemini brain layout described in (3) instead.
//!
//! 2. **A mirror directory**, when the embedding application registers one.
//!    Antigravity keeps some IDE conversations only in memory of a running
//!    process, so an application that obtains them another way can write them
//!    as `<cascadeId>.json` into a [`SessionMirror`] directory; this adapter
//!    then walks it like any vendor directory. Discovery itself never
//!    populates a mirror.
//!
//! 3. **Gemini native sessions** — per-session databases and brain traces
//!    under three sibling roots inside `$GEMINI_HOME` (default `~/.gemini`):
//!    - `antigravity-cli/brain/<uuid>/` — Antigravity CLI (the Go binary that
//!      replaced Gemini CLI as of 2026-06-18).
//!    - `antigravity-ide/brain/<uuid>/` — Antigravity IDE 2.0 (where v2 chat
//!      sessions actually live, including `.system_generated/logs/transcript.jsonl`).
//!    - `antigravity/brain/<uuid>/` — legacy single-binary Antigravity layout,
//!      preserved for back-compat with users on older builds.
//!
//!    Each root can hold `conversations/<uuid>.db` and
//!    `brain/<uuid>/.system_generated/logs/transcript.jsonl`. The database is
//!    the primary session source. `GEMINI_HOME` overrides all three roots.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use crate::analysis::{BoundedJsonlReader, FramedRecord};
use crate::discovery::scanner::AgentKind;
use crate::discovery::scanner::{SessionMetadata, TitleSource};
use crate::discovery::{
    AgentExplorer, SessionLog, SessionMirror, SessionSource, SurfacePaths, WatchRoot,
    app_config_dir_in, collect_dirs_with_exts, dir_has_json_files, env_path_when_real_home,
    find_chat_session_dirs, home_dir, recent_files_with_exts,
};
use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags};

const SESSION_FILE_EXTS: &[&str] = &["json", "jsonl"];

/// Antigravity discovery over the vendor's own layout, plus whatever the
/// embedding application has configured.
pub struct AntigravityExplorer {
    /// An application-maintained directory of `<cascadeId>.json` conversations.
    pub mirror: SessionMirror,
    /// Where the application records orchestrator → worker spawn edges for the
    /// cascades in [`Self::mirror`]. Antigravity does not persist the
    /// relationship itself (see [`antigravity_subagents`](super::antigravity_subagents)),
    /// so without this the adapter reports no sub-agents.
    pub spawn_edges_path: fn(&Path) -> Option<PathBuf>,
}

/// The unconfigured adapter: vendor layouts only, no mirror, no spawn edges.
pub static DISK_ANTIGRAVITY: AntigravityExplorer = AntigravityExplorer {
    mirror: SessionMirror::NONE,
    spawn_edges_path: |_| None,
};

impl AntigravityExplorer {
    /// The mirror directory, only when it actually holds cascade JSON. An
    /// empty mirror stays out of the walk.
    async fn populated_mirror_dir(&self, home: &Path) -> Option<PathBuf> {
        let dir = self.mirror.dir_in(home)?;
        dir_has_json_files(&dir).await.then_some(dir)
    }

    /// Sub-agent inputs for `home`: the spawn-edge sidecar and the mirror
    /// directory holding each worker's cascade. `None` when either is unset.
    fn subagent_inputs(&self, home: &Path) -> Option<(PathBuf, PathBuf)> {
        Some(((self.spawn_edges_path)(home)?, self.mirror.dir_in(home)?))
    }
}

#[async_trait]
impl AgentExplorer for AntigravityExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        let dirs = self.log_dirs_in(&home).await;
        let files = recent_files_with_exts(&dirs, now, since_secs, SESSION_FILE_EXTS).await;
        // Agent-Manager workers are ordinary mirrored cascades (and brain
        // transcripts) in the same trees this walk covers, linked to their
        // orchestrator only by a sidecar edge. Drop them so a worker never
        // surfaces as a top-level session. Empty set → no-op in the common
        // no-orchestration case. Like Codex, this single chokepoint covers
        // every consumer, since they all funnel through `discover_recent`.
        let child_ids = match (self.spawn_edges_path)(&home) {
            Some(sidecar) => super::antigravity_subagents::child_cascade_ids_in(&sidecar).await,
            None => HashSet::new(),
        };

        let cutoff = now.saturating_sub(since_secs.max(0));
        let databases = conversation_databases_in(&home, cutoff).await;
        let database_paths: HashSet<PathBuf> =
            databases.iter().map(|(path, _, _)| path.clone()).collect();
        let mut logs: Vec<SessionLog> = Vec::with_capacity(files.len() + databases.len());
        for (db_path, session_id, mtime_epoch) in databases {
            if child_ids.contains(&session_id) {
                continue;
            }
            logs.push(SessionLog {
                environment: Default::default(),
                agent_type: AgentKind::Antigravity,
                source: SessionSource::ProviderDb {
                    agent: AgentKind::Antigravity,
                    db_path,
                    session_id,
                },
                updated_at: Some(mtime_epoch),
            });
        }
        for file in files {
            if is_excluded_subagent(&file.path, &child_ids) {
                continue;
            }
            if conversation_database_for_brain_file(&file.path)
                .is_some_and(|path| database_paths.contains(&path))
            {
                continue;
            }
            match classify_session_file(&file.path) {
                SessionFileDecision::File => logs.push(SessionLog {
                    environment: Default::default(),
                    agent_type: AgentKind::Antigravity,
                    source: SessionSource::File(file.path),
                    updated_at: Some(file.mtime_epoch),
                }),
                SessionFileDecision::Skip => {}
            }
        }
        logs
    }

    async fn direct_session_source(
        &self,
        session_id: &str,
    ) -> crate::discovery::DirectSessionSource {
        let Some(home) = home_dir() else {
            return crate::discovery::DirectSessionSource::Unsupported;
        };
        if !is_safe_session_id(session_id) {
            return crate::discovery::DirectSessionSource::Unsupported;
        }
        for root in gemini_subroots_in(&home) {
            let db_path = root.join("conversations").join(format!("{session_id}.db"));
            let fingerprint_path = db_path.clone();
            let usable =
                tokio::task::spawn_blocking(move || database_has_usage_tables(&fingerprint_path))
                    .await
                    .unwrap_or(false);
            if usable {
                return crate::discovery::DirectSessionSource::Found(SessionSource::ProviderDb {
                    agent: AgentKind::Antigravity,
                    db_path,
                    session_id: session_id.to_owned(),
                });
            }
        }
        crate::discovery::DirectSessionSource::Unsupported
    }

    async fn provider_db_fingerprint(
        &self,
        db_path: &Path,
        session_id: &str,
    ) -> Option<(u64, u64)> {
        let db_path = db_path.to_owned();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || db_fingerprint(&db_path, &session_id))
            .await
            .ok()
            .flatten()
    }

    /// Owns every Antigravity tree: legacy `/antigravity/` brain + v1 IDE
    /// workspaceStorage, v2 IDE `/antigravity-ide/brain/`, CLI
    /// `/antigravity-cli/brain/`, and the configured mirror.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/antigravity/`           → `ide_desktop` (legacy v1 IDE workspaceStorage + brain)
    /// - `/antigravity-ide/brain/` → `ide_desktop` (v2 IDE brain)
    /// - `/antigravity-cli/brain/` → `cli` (CLI brain — replaces Gemini CLI 2026-06-18)
    /// - the mirror's `path_marker` → `mirror` (classifier maps to `ide_desktop`)
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/antigravity/")
            || path_lower.contains("/antigravity-cli/brain/")
            || path_lower.contains("/antigravity-ide/brain/")
            || self.mirror.owns(path_lower)
    }

    /// Reuses `discover_recent` so brain transcripts use the same file-backed
    /// metadata path as the activity list.
    async fn discover_cwds(&self, now: i64, since_secs: i64) -> Vec<String> {
        let logs = self.discover_recent(now, since_secs).await;
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        let gemini_root =
            env_path_when_real_home(&home, "GEMINI_HOME").unwrap_or_else(|| home.join(".gemini"));
        let history = read_cli_history(&gemini_root).await;
        let mut cwds = Vec::new();
        for log in logs {
            let path = match log.source {
                SessionSource::File(path) => path,
                SessionSource::ProviderDb {
                    db_path,
                    session_id,
                    ..
                } => match sibling_brain_transcript(&db_path, &session_id) {
                    Some(path) => path,
                    None => continue,
                },
                SessionSource::Inline { .. } => continue,
            };
            let Some(preview) =
                crate::discovery::session_source_preview(&SessionSource::File(path.clone())).await
            else {
                continue;
            };
            let mut metadata = crate::discovery::scanner::parse_session_metadata_str(&preview);
            augment_brain_metadata_with_history(&path, &preview, &mut metadata, history.as_deref());
            if let Some(cwd) = metadata.cwd {
                cwds.push(cwd);
            }
        }
        cwds
    }

    // Antigravity is bi-modal:
    //   - CLI:    `~/.gemini/antigravity-cli/brain/**`.
    //   - IDE:    workspaceStorage, the mirror, legacy `antigravity/brain/`,
    //             v2 `antigravity-ide/brain/`.
    // Trait default classifies via `surface_paths`; inline
    // `antigravity-brain:<absolute-path>` labels classify correctly because
    // the embedded path contains one of the `surface_paths` root substrings.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// CLI: `<gemini>/antigravity-cli/brain/`. IDE: v1 + v2 brain trees and
    /// Antigravity IDE workspaceStorage. `mirror`: the configured mirror
    /// directory, if any. `GEMINI_HOME` overrides the gemini root.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        let gemini_root =
            env_path_when_real_home(home, "GEMINI_HOME").unwrap_or_else(|| home.join(".gemini"));
        SurfacePaths {
            cli: vec![gemini_root.join("antigravity-cli")],
            ide_desktop: vec![
                gemini_root.join("antigravity-ide"),
                gemini_root.join("antigravity"),
                app_config_dir_in("Antigravity", home)
                    .join("User")
                    .join("workspaceStorage"),
            ],
            mirror: self.mirror.roots_in(home),
        }
    }

    /// Each brain subroot (`antigravity-cli`, `antigravity-ide`,
    /// `antigravity`), recursive: a conversation's `.db`, its `-wal` file,
    /// and its sibling transcript all move under the same subroot.
    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        gemini_subroots_in(home)
            .into_iter()
            .map(WatchRoot::recursive)
            .collect()
    }

    fn recover_session_id_from_path(&self, file: &Path) -> Option<String> {
        brain_uuid_dir(file)?
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    }

    // ---- Orchestration: Antigravity's Agent Manager spawns each worker as its
    // own cascade (own `<cascadeId>.json`), linked to the orchestrator by
    // `trajectoryMetadata.parentConversationId`. That link is not on disk, so
    // these hooks read it from the sidecar the embedding application records.
    // Implementation lives in `antigravity_subagents`.

    fn supports_subagents(&self) -> bool {
        true
    }

    async fn list_subagents(&self, parent_transcript: &Path) -> Vec<PathBuf> {
        let Some((sidecar, cascades)) = home_dir().and_then(|home| self.subagent_inputs(&home))
        else {
            return Vec::new();
        };
        super::antigravity_subagents::list_subagents_in(parent_transcript, &sidecar, &cascades)
            .await
    }

    async fn locate_subagent(
        &self,
        parent_transcript: &Path,
        subagent_id: &str,
    ) -> Option<PathBuf> {
        let (sidecar, cascades) = self.subagent_inputs(&home_dir()?)?;
        super::antigravity_subagents::locate_subagent_in(
            parent_transcript,
            subagent_id,
            &sidecar,
            &cascades,
        )
        .await
    }

    fn subagent_id(&self, path: &Path) -> Option<String> {
        super::antigravity_subagents::subagent_id(path)
    }

    async fn subagent_label(&self, path: &Path) -> String {
        let Some(sidecar) = home_dir().and_then(|home| (self.spawn_edges_path)(&home)) else {
            return "Sub-agent".to_string();
        };
        super::antigravity_subagents::subagent_label_in(path, &sidecar).await
    }
}

impl AntigravityExplorer {
    /// Every directory that can hold an Antigravity session: IDE workspace
    /// storage, the configured mirror, and the Gemini brain trees.
    async fn log_dirs_in(&self, home: &Path) -> Vec<PathBuf> {
        let ws_root = app_config_dir_in("Antigravity", home)
            .join("User")
            .join("workspaceStorage");
        let user_root = app_config_dir_in("Antigravity", home).join("User");

        let mut dirs = BTreeSet::new();
        for dir in find_chat_session_dirs(&ws_root).await {
            dirs.insert(dir);
        }
        for dir in find_chat_session_dirs(&user_root).await {
            dirs.insert(dir);
        }

        if let Some(mirror) = self.populated_mirror_dir(home).await {
            dirs.insert(mirror);
        }

        // Antigravity brain memory across CLI, IDE 2.0, and legacy roots.
        let brain_dirs = gemini_brain_dirs_in(home).await;
        let brain_dirs_count = brain_dirs.len();
        for dir in brain_dirs {
            dirs.insert(dir);
        }

        ::tracing::debug!(
            target: "antiburn::discovery::antigravity",
            total_dirs = dirs.len(),
            brain_dirs = brain_dirs_count,
            "Antigravity session directories discovered"
        );

        dirs.into_iter().collect()
    }
}

/// Collect Antigravity brain directories under `$GEMINI_HOME` (or
/// `~/.gemini` if `GEMINI_HOME` is unset), across all three known sibling
/// roots:
/// - `antigravity-cli/brain/` — Antigravity CLI (the Go binary)
/// - `antigravity-ide/brain/` — Antigravity IDE 2.0
/// - `antigravity/brain/`     — legacy single-binary layout
///
/// Brain memory is stored under uuid/numeric subdirectories that contain
/// `.json` and/or `.jsonl` files. The walker is layout-tolerant: any
/// directory in any of the three trees with at least one matching file is
/// returned.
async fn gemini_brain_dirs_in(home: &Path) -> Vec<PathBuf> {
    let gemini_home = env_path_when_real_home(home, "GEMINI_HOME");
    gemini_brain_dirs_for_overrides(home, gemini_home.as_deref()).await
}

const GEMINI_BRAIN_SUBROOTS: &[&str] = &["antigravity-cli", "antigravity-ide", "antigravity"];
const MAX_FINGERPRINT_BLOB_BYTES: usize = 1024 * 1024;

fn gemini_subroots_in(home: &Path) -> Vec<PathBuf> {
    let gemini_root =
        env_path_when_real_home(home, "GEMINI_HOME").unwrap_or_else(|| home.join(".gemini"));
    GEMINI_BRAIN_SUBROOTS
        .iter()
        .map(|subroot| gemini_root.join(subroot))
        .collect()
}

/// Conversation databases within `cutoff`, each with its combined db + sibling
/// transcript mtime. `cutoff` is applied from cheap stats alone, before
/// [`database_has_usage_tables`] opens the database — an old, quiet
/// conversation is never opened just to find out it is old and quiet.
async fn conversation_databases_in(home: &Path, cutoff: i64) -> Vec<(PathBuf, String, i64)> {
    let mut databases = Vec::new();
    for root in gemini_subroots_in(home) {
        let Ok(mut entries) = tokio::fs::read_dir(root.join("conversations")).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("db"))
            {
                continue;
            }
            let Some(mtime_epoch) = database_mtime_epoch(&path).await else {
                continue;
            };
            let Some(session_id) = path
                .file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| is_safe_session_id(value))
                .map(str::to_owned)
            else {
                continue;
            };
            // The sibling transcript can be newer than the database; a stat
            // is cheap, so check it before deciding this entry is stale.
            let transcript_mtime = match sibling_brain_transcript(&path, &session_id) {
                Some(transcript) => database_mtime_epoch(&transcript)
                    .await
                    .unwrap_or(mtime_epoch),
                None => mtime_epoch,
            };
            let combined_mtime = mtime_epoch.max(transcript_mtime);
            if combined_mtime < cutoff {
                continue;
            }
            let fingerprint_path = path.clone();
            if !tokio::task::spawn_blocking(move || database_has_usage_tables(&fingerprint_path))
                .await
                .unwrap_or(false)
            {
                continue;
            }
            databases.push((path, session_id, combined_mtime));
        }
    }
    databases.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
    databases
}

fn is_safe_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && !matches!(session_id, "." | "..")
        && matches!(
            Path::new(session_id)
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [Component::Normal(_)]
        )
}

async fn database_mtime_epoch(path: &Path) -> Option<i64> {
    async fn mtime(path: &Path) -> Option<i64> {
        tokio::fs::metadata(path)
            .await
            .ok()?
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs() as i64)
    }

    let main = mtime(path).await?;
    let mut wal_path = path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal = mtime(Path::new(&wal_path)).await.unwrap_or(main);
    Some(main.max(wal))
}

fn open_database(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

fn database_has_usage_tables(path: &Path) -> bool {
    #[cfg(any(test, feature = "test-instrumentation"))]
    record_tracked_database_open(path);
    let Ok(connection) = open_database(path) else {
        return false;
    };
    [
        "SELECT idx, metadata FROM steps LIMIT 0",
        "SELECT idx, data FROM gen_metadata LIMIT 0",
    ]
    .into_iter()
    .any(|query| connection.prepare(query).is_ok())
}

/// How many times [`database_has_usage_tables`] opened a tracked path since
/// the matching [`track_database_opens`] call. Backs the discovery-pruning
/// test that an old, quiet conversation is never opened.
#[cfg(any(test, feature = "test-instrumentation"))]
static TRACKED_DATABASE_OPENS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Arm open-counting for `path`. [`take_tracked_database_opens`] reports the
/// count since this call.
#[doc(hidden)]
#[cfg(any(test, feature = "test-instrumentation"))]
pub fn track_database_opens(path: &Path) {
    TRACKED_DATABASE_OPENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(path.to_path_buf(), 0);
}

#[cfg(any(test, feature = "test-instrumentation"))]
fn record_tracked_database_open(path: &Path) {
    let mut opens = TRACKED_DATABASE_OPENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(count) = opens.get_mut(path) {
        *count += 1;
    }
}

/// Take the tracked open count for `path` and stop tracking it.
#[doc(hidden)]
#[cfg(any(test, feature = "test-instrumentation"))]
pub fn take_tracked_database_opens(path: &Path) -> usize {
    TRACKED_DATABASE_OPENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(path)
        .unwrap_or(0)
}

pub(crate) fn db_fingerprint(path: &Path, session_id: &str) -> Option<(u64, u64)> {
    let connection = open_database(path).ok()?;
    connection.execute_batch("BEGIN").ok()?;
    let database = db_fingerprint_connection(&connection)?;
    let transcript =
        sibling_brain_transcript(path, session_id).and_then(|path| file_fingerprint(&path));
    Some(combine_db_fingerprint(database, transcript.as_deref()))
}

pub(crate) fn combine_db_fingerprint(
    database: (u64, u64),
    transcript_fingerprint: Option<&str>,
) -> (u64, u64) {
    fn mix(hash: u64, value: u64) -> u64 {
        (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3)
    }

    let transcript_hash = transcript_fingerprint.map_or(0, |fingerprint| {
        fingerprint
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
                mix(hash, u64::from(byte))
            })
    });
    (
        mix(database.0, transcript_hash),
        mix(database.1, u64::from(transcript_fingerprint.is_some())),
    )
}

fn file_fingerprint(path: &Path) -> Option<String> {
    use crate::discovery::source_version::{
        FINGERPRINT_HEAD_BYTES, FingerprintInputs, SourceStat, head_hash_of,
    };

    let mut file = File::open(path).ok()?;
    let stat = SourceStat::from_open_std_file(&file)?;
    let mut head = Vec::new();
    file.by_ref()
        .take(FINGERPRINT_HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    Some(
        FingerprintInputs {
            stat,
            head_hash: Some(head_hash_of(&head)),
        }
        .fingerprint(),
    )
}

pub(crate) fn db_fingerprint_connection(connection: &Connection) -> Option<(u64, u64)> {
    fn mix(hash: u64, value: u64) -> u64 {
        (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3)
    }

    fn table_state(connection: &Connection, table: &str, blob: &str) -> Option<[u64; 5]> {
        let mut statement = connection
            .prepare(&format!(
                "SELECT idx, length(CAST({blob} AS BLOB)),
                        CASE WHEN length(CAST({blob} AS BLOB)) <= ?1
                             THEN CAST({blob} AS BLOB) END
                   FROM {table}
                  ORDER BY idx"
            ))
            .ok()?;
        let mut rows = statement.query([MAX_FINGERPRINT_BLOB_BYTES as i64]).ok()?;
        let mut count = 0_u64;
        let mut max = u64::MAX;
        let mut total_bytes = 0_u64;
        let mut max_bytes = 0_u64;
        let mut content_hash = 0xcbf2_9ce4_8422_2325;
        while let Some(row) = rows.next().ok()? {
            let idx = row.get::<_, i64>(0).ok()? as u64;
            let length = row.get::<_, Option<i64>>(1).ok()?.unwrap_or(0).max(0) as u64;
            let data = row.get::<_, Option<Vec<u8>>>(2).ok().flatten();
            count = count.checked_add(1)?;
            max = if max == u64::MAX { idx } else { max.max(idx) };
            total_bytes = total_bytes.checked_add(length)?;
            max_bytes = max_bytes.max(length);
            content_hash = mix(mix(content_hash, idx), length);
            if let Some(data) = data {
                for byte in data {
                    content_hash = mix(content_hash, u64::from(byte));
                }
            }
        }
        Some([count, max, total_bytes, max_bytes, content_hash])
    }

    fn table_exists(connection: &Connection, table: &str) -> Option<bool> {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .ok()
    }

    fn hash(values: impl IntoIterator<Item = u64>) -> u64 {
        values.into_iter().fold(0xcbf2_9ce4_8422_2325, mix)
    }

    let has_steps = table_exists(connection, "steps")?;
    let has_generations = table_exists(connection, "gen_metadata")?;
    if !has_steps && !has_generations {
        return None;
    }
    let steps = has_steps
        .then(|| table_state(connection, "steps", "metadata"))
        .flatten()
        .unwrap_or([u64::MAX; 5]);
    let generations = has_generations
        .then(|| table_state(connection, "gen_metadata", "data"))
        .flatten()
        .unwrap_or([u64::MAX; 5]);
    Some((hash(steps), hash(generations)))
}

pub(crate) fn sibling_brain_transcript(db_path: &Path, session_id: &str) -> Option<PathBuf> {
    let subroot = db_path.parent()?.parent()?;
    Some(
        subroot
            .join("brain")
            .join(session_id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl"),
    )
}

fn conversation_database_for_brain_file(path: &Path) -> Option<PathBuf> {
    let uuid_dir = brain_uuid_dir(path)?;
    let session_id = uuid_dir.file_name()?;
    let subroot = uuid_dir.parent()?.parent()?;
    Some(
        subroot
            .join("conversations")
            .join(Path::new(session_id).with_extension("db")),
    )
}

pub(crate) async fn db_session_metadata(
    db_path: PathBuf,
    session_id: String,
) -> Option<SessionMetadata> {
    let fallback = || SessionMetadata {
        session_id: Some(session_id.clone()),
        agent_type: Some(AgentKind::Antigravity),
        ..SessionMetadata::default()
    };
    let Some(transcript) = sibling_brain_transcript(&db_path, &session_id) else {
        return Some(fallback());
    };
    let Some(preview) =
        crate::discovery::session_source_preview(&SessionSource::File(transcript.clone())).await
    else {
        return Some(fallback());
    };
    let mut metadata = crate::discovery::scanner::parse_session_metadata_str(&preview);
    augment_brain_metadata(&transcript, &preview, &mut metadata).await;
    Some(metadata)
}

async fn gemini_brain_dirs_for_overrides(home: &Path, gemini_home: Option<&Path>) -> Vec<PathBuf> {
    let gemini_root = gemini_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".gemini"));
    let mut results = Vec::new();
    for subroot in GEMINI_BRAIN_SUBROOTS {
        let brain_root = gemini_root.join(subroot).join("brain");
        collect_dirs_with_exts(&brain_root, &mut results, SESSION_FILE_EXTS).await;
    }
    results
}

/// What `discover_recent` should do with one discovered session file.
enum SessionFileDecision {
    /// Surface the file as-is.
    File,
    /// Drop it — a brain sidecar / `transcript_full.jsonl` that isn't a session.
    Skip,
}

/// Decide how to surface one discovered file. Split out of `discover_recent` so
/// the brain-transcript fallback is unit-testable without the discovery walk.
fn classify_session_file(file: &Path) -> SessionFileDecision {
    if is_brain_transcript_main(file) {
        return SessionFileDecision::File;
    }
    // Under a brain `<uuid>/` tree the only session is the main transcript
    // (handled above). `transcript_full.jsonl` and the sidecar artifacts that
    // share the UUID dir — `task.md.metadata.json`,
    // `implementation_plan.md.metadata.json`, memory files, … — are not sessions.
    // Emitting them would create spurious SessionLogs and, more damagingly, let a
    // sidecar `.json` shadow the real transcript when a session is located by its
    // brain UUID (`locate_session_source` substring-matches the UUID and would
    // otherwise pick the sidecar), leaving the session with no analyzable
    // transcript.
    if brain_origin_of(file).is_some() {
        return SessionFileDecision::Skip;
    }
    SessionFileDecision::File
}

/// True when `path` is a brain transcript whose contents we want to surface
/// as a SessionLog. Today that's the user-facing `transcript.jsonl`; every
/// other artifact under the brain `<uuid>/` tree (including
/// `transcript_full.jsonl`) is skipped by the `brain_origin_of` guard in
/// `discover_recent` to avoid emitting non-session files.
fn is_brain_transcript_main(path: &Path) -> bool {
    brain_uuid_dir(path).is_some()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("transcript.jsonl"))
            .unwrap_or(false)
}

/// Returns the `<uuid>` segment from a brain transcript path shaped like
/// `…/brain/<uuid>/.system_generated/logs/<file>`. Layout-strict so it
/// doesn't accidentally claim unrelated brain artifacts.
fn brain_uuid_dir(file: &Path) -> Option<&Path> {
    let logs = file.parent()?;
    if logs.file_name().and_then(|n| n.to_str()) != Some("logs") {
        return None;
    }
    let sysgen = logs.parent()?;
    if sysgen.file_name().and_then(|n| n.to_str()) != Some(".system_generated") {
        return None;
    }
    let uuid_dir = sysgen.parent()?;
    let brain = uuid_dir.parent()?;
    if brain.file_name().and_then(|n| n.to_str()) != Some("brain") {
        return None;
    }
    Some(uuid_dir)
}

/// True when `path` is a spawned Agent-Manager worker — the cascadeId it encodes
/// is in the sidecar's child set. Covers both mirrored cascades
/// (`<cascadeId>.json`) and brain transcripts (`…/brain/<cascadeId>/…`). Used by
/// `discover_recent` to keep workers out of the top-level list. A path whose
/// cascadeId can't be derived is never excluded (fail-open keeps real sessions),
/// matching `codex::is_excluded_subagent`.
fn is_excluded_subagent(path: &Path, child_ids: &HashSet<String>) -> bool {
    antigravity_cascade_id_of(path).is_some_and(|id| child_ids.contains(&id))
}

/// The cascadeId a discovered Antigravity path encodes: the brain `<uuid>` dir
/// for a brain transcript, else the file stem for a `<cascadeId>.json`.
fn antigravity_cascade_id_of(path: &Path) -> Option<String> {
    if let Some(uuid_dir) = brain_uuid_dir(path) {
        return uuid_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

/// Which Antigravity subroot a brain transcript lives under. Determines
/// which structured cwd source to consult.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrainOrigin {
    /// `~/.gemini/antigravity-cli/brain/<uuid>/...` — has structured
    /// `history.jsonl` workspace records.
    Cli,
    /// `~/.gemini/antigravity-ide/brain/<uuid>/...` — chats embed `Active
    /// Document: <path>` inside USER_INPUT `<ADDITIONAL_METADATA>` blocks
    /// (an IDE-defined contract).
    Ide,
    /// `~/.gemini/antigravity/brain/<uuid>/...` — legacy single-binary
    /// layout. No structured cwd source; best-effort prose sniff only.
    Legacy,
}

/// Classify a brain transcript path by its `.gemini/<subroot>/brain/...`
/// parent. Returns `None` if the path isn't under one of the known subroots.
fn brain_origin_of(file: &Path) -> Option<BrainOrigin> {
    let lower = file
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if lower.contains("/antigravity-cli/brain/") {
        Some(BrainOrigin::Cli)
    } else if lower.contains("/antigravity-ide/brain/") {
        Some(BrainOrigin::Ide)
    } else if lower.contains("/antigravity/brain/") {
        Some(BrainOrigin::Legacy)
    } else {
        None
    }
}

/// One entry from `~/.gemini/antigravity-cli/history.jsonl`. The CLI writes
/// one of these per user-launched session (no UUID — we correlate to the
/// brain transcript via the first step's `created_at`).
#[derive(Debug, Clone)]
struct CliHistoryEntry {
    /// User's first prompt — usable as the session title.
    display: String,
    /// Absolute working directory of the CLI when the session started.
    workspace: String,
    /// Session start time in **epoch seconds** (file stores ms; we
    /// normalize on read).
    timestamp_secs: i64,
}

/// Read `<gemini_root>/antigravity-cli/history.jsonl` and return its
/// entries. Returns `None` if the file is absent or unreadable; callers
/// fall back to prose sniffing in that case.
async fn read_cli_history(gemini_root: &Path) -> Option<Vec<CliHistoryEntry>> {
    const HISTORY_MAX_RECORD_BYTES: usize = 64 * 1024;
    const HISTORY_MAX_ENTRIES: usize = 4_096;
    const HISTORY_FIELD_MAX_BYTES: usize = 4 * 1024;
    let path = gemini_root.join("antigravity-cli").join("history.jsonl");
    #[cfg(any(test, feature = "test-instrumentation"))]
    if HISTORY_READ_PATH
        .lock()
        .is_ok_and(|watched| watched.as_deref() == Some(path.as_path()))
    {
        HISTORY_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    tokio::task::spawn_blocking(move || {
        let file = File::open(path).ok()?;
        let mut reader = BoundedJsonlReader::with_max_record_bytes(
            BufReader::new(file),
            HISTORY_MAX_RECORD_BYTES,
        );
        let mut out = VecDeque::with_capacity(HISTORY_MAX_ENTRIES);
        while let Some(record) = reader.next_record(&|| false) {
            let FramedRecord::Complete { bytes, .. } = record else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
                continue;
            };
            let (Some(display), Some(workspace), Some(timestamp_ms)) = (
                value.get("display").and_then(|value| value.as_str()),
                value.get("workspace").and_then(|value| value.as_str()),
                value.get("timestamp").and_then(|value| value.as_i64()),
            ) else {
                continue;
            };
            if display.len() > HISTORY_FIELD_MAX_BYTES || workspace.len() > HISTORY_FIELD_MAX_BYTES
            {
                continue;
            }
            if out.len() == HISTORY_MAX_ENTRIES {
                out.pop_front();
            }
            out.push_back(CliHistoryEntry {
                display: display.to_owned(),
                workspace: workspace.to_owned(),
                timestamp_secs: timestamp_ms / 1000,
            });
        }
        Some(out.into_iter().collect())
    })
    .await
    .ok()
    .flatten()
}

#[cfg(any(test, feature = "test-instrumentation"))]
static HISTORY_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(test, feature = "test-instrumentation"))]
static HISTORY_READ_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn watch_history_reads(path: PathBuf) {
    *HISTORY_READ_PATH.lock().expect("history read path locks") = Some(path);
    HISTORY_READS.store(0, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
fn take_history_reads() -> usize {
    *HISTORY_READ_PATH.lock().expect("history read path locks") = None;
    HISTORY_READS.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// Find the history entry whose `timestamp_secs` matches `created_at_secs`
/// within ±5 seconds (the CLI logs the session-start timestamp in
/// `history.jsonl` and uses the same instant as the brain transcript's
/// first step's `created_at`, but the two writes can lag slightly).
fn find_cli_history_entry(
    history: &[CliHistoryEntry],
    created_at_secs: i64,
) -> Option<&CliHistoryEntry> {
    history
        .iter()
        .min_by_key(|entry| entry.timestamp_secs.abs_diff(created_at_secs))
        .filter(|entry| entry.timestamp_secs.abs_diff(created_at_secs) <= 5)
}

/// Add brain metadata from the UUID path, a bounded transcript preview, and
/// the bounded CLI history stream. The transcript remains a file source.
pub(crate) async fn augment_brain_metadata(
    file: &Path,
    preview: &str,
    metadata: &mut SessionMetadata,
) {
    let history = if brain_origin_of(file) == Some(BrainOrigin::Cli) {
        let gemini_root = brain_uuid_dir(file)
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::parent);
        match gemini_root {
            Some(root) => read_cli_history(root).await,
            None => None,
        }
    } else {
        None
    };
    augment_brain_metadata_with_history(file, preview, metadata, history.as_deref());
}

fn augment_brain_metadata_with_history(
    file: &Path,
    preview: &str,
    metadata: &mut SessionMetadata,
    history: Option<&[CliHistoryEntry]>,
) {
    let Some(uuid_dir) = brain_uuid_dir(file) else {
        return;
    };
    if metadata.session_id.is_none() {
        metadata.session_id = uuid_dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
    }
    let Some(origin) = brain_origin_of(file) else {
        return;
    };

    let (cwd, title) = match origin {
        BrainOrigin::Cli => cli_cwd_and_title_from_history(preview, history),
        BrainOrigin::Ide | BrainOrigin::Legacy => brain_cwd_and_title_from_prose(preview),
    };
    if metadata.cwd.is_none() {
        metadata.cwd = cwd;
    }
    if metadata.title.is_none() {
        metadata.title = title.map(|title| crate::discovery::scanner::normalize_title(&title));
        if metadata.title.is_some() {
            metadata.title_source = Some(TitleSource::FirstMessage);
        }
    }
}

/// CLI origin: look up the workspace + display in `history.jsonl` by
/// correlating to the brain transcript's first step `created_at`.
fn cli_cwd_and_title_from_history(
    raw: &str,
    history: Option<&[CliHistoryEntry]>,
) -> (Option<String>, Option<String>) {
    let history = match history {
        Some(h) => h,
        None => return (None, None),
    };
    let created = match first_step_created_at_secs(raw) {
        Some(ts) => ts,
        None => return (None, None),
    };
    match find_cli_history_entry(history, created) {
        Some(entry) => (Some(entry.workspace.clone()), Some(entry.display.clone())),
        None => (None, None),
    }
}

/// IDE / legacy: extract cwd from prose `Active Document:` or
/// `[label](file:///path)` patterns, and title from the first USER_INPUT
/// step's `<USER_REQUEST>` block.
fn brain_cwd_and_title_from_prose(raw: &str) -> (Option<String>, Option<String>) {
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    for line in raw.lines() {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let raw_content = value.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if cwd.is_none()
            && let Some(uri) = sniff_base_uri_from_step(raw_content)
        {
            cwd = Some(uri_to_directory(&uri));
        }
        if title.is_none() && value.get("type").and_then(|v| v.as_str()) == Some("USER_INPUT") {
            let prose = strip_user_input_wrappers(raw_content);
            if !prose.is_empty() {
                title = Some(prose);
            }
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }
    (cwd, title)
}

/// First parseable JSONL line's `created_at` → epoch seconds. The brain
/// transcript's step 0 always carries this — it's the session-start
/// instant we correlate to CLI history.
fn first_step_created_at_secs(raw: &str) -> Option<i64> {
    raw.lines().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let ts = value.get("created_at").and_then(|v| v.as_str())?;
        parse_iso8601_to_epoch_secs(ts)
    })
}

/// Strip a leading `file://` and, if the path looks like a file (has an
/// extension), return its parent directory. The scanner stores `cwd`
/// verbatim, so we normalize here.
fn uri_to_directory(uri: &str) -> String {
    let path_str = uri.strip_prefix("file://").unwrap_or(uri);
    let path = Path::new(path_str);
    if path.extension().is_some() {
        path.parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.to_string())
    } else {
        path_str.to_string()
    }
}

/// Parse an ISO 8601 / RFC 3339 timestamp string to epoch seconds. Returns
/// `None` on parse failure. We use this only for correlating brain
/// transcripts to CLI history entries.
fn parse_iso8601_to_epoch_secs(s: &str) -> Option<i64> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|dt| dt.unix_timestamp())
}

/// Pull a `file://`-form cwd hint out of one step's prose `content`.
///
/// Two known emitters:
/// - IDE 2.0 USER_INPUT: `Active Document: /abs/path/file.ext (LANGUAGE_RUST)`
///   inside `<ADDITIONAL_METADATA>`. The parent dir is the cwd; we still
///   return the file path here and let the scanner's `extract_cwd_from_value`
///   → `normalize_cwd_path` collapse it to the parent (`looks_like_file`
///   based on extension).
/// - CLI PLANNER_RESPONSE: a markdown link `[label](file:///abs/path)` that
///   echoes the project directory.
fn sniff_base_uri_from_step(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Active Document: ") {
            let path = match rest.rfind(" (LANGUAGE_") {
                Some(idx) => &rest[..idx],
                None => rest,
            }
            .trim();
            if !path.is_empty() {
                return Some(format!("file://{}", path));
            }
        }
    }
    if let Some(start) = content.find("](file://") {
        let after = &content[start + 2..];
        if let Some(end) = after.find(')') {
            let uri = &after[..end];
            if !uri.is_empty() {
                return Some(uri.to_string());
            }
        }
    }
    None
}

/// Strip the `<USER_REQUEST>...</USER_REQUEST>` wrapper (and adjacent
/// `<ADDITIONAL_METADATA>` / `<USER_SETTINGS_CHANGE>` blocks) from a
/// USER_INPUT step's content so the scanner's title extractor sees clean
/// user prose.
fn strip_user_input_wrappers(content: &str) -> String {
    let mut out = String::new();
    let mut search_start = 0usize;
    while let Some(open_rel) = content[search_start..].find("<USER_REQUEST>") {
        let open = search_start + open_rel + "<USER_REQUEST>".len();
        let Some(close_rel) = content[open..].find("</USER_REQUEST>") else {
            break;
        };
        let close = open + close_rel;
        let segment = content[open..close].trim();
        if !segment.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(segment);
        }
        search_start = close + "</USER_REQUEST>".len();
    }
    if out.is_empty() {
        // No wrapper found — return the original prose (trimmed) so we don't
        // accidentally swallow the user's text on a non-standard layout.
        content.trim().to_string()
    } else {
        out
    }
}

#[cfg(test)]
pub(crate) fn sample_workspace_log_path(home: &Path) -> PathBuf {
    app_config_dir_in("Antigravity", home)
        .join("User")
        .join("workspaceStorage")
        .join("x")
        .join("chatSessions")
        .join("session.json")
}

#[cfg(test)]
pub(crate) fn sample_cli_brain_log_path(home: &Path) -> PathBuf {
    home.join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join("12345")
        .join("trace.jsonl")
}

#[cfg(test)]
pub(crate) fn sample_ide_brain_log_path(home: &Path) -> PathBuf {
    home.join(".gemini")
        .join("antigravity-ide")
        .join("brain")
        .join("3afb6691-6ba3-4a01-bd37-df54a0c1ee82")
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl")
}

#[cfg(test)]
#[path = "tests/antigravity_tests.rs"]
mod tests;
