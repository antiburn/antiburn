//! The app's local database.
//!
//! One SQLite file under the app data directory holds everything antiburn
//! remembers between launches: preferences, the directories the user pointed
//! the scanner at, a metadata cache of the sessions discovery has seen, and the
//! analysis the engine derived from them.
//!
//! The database is allowed to retain any local session content needed for
//! visibility and analysis. The current schema stores identities, locations,
//! derived numbers, a session title, and capped skill descriptions; future
//! migrations may add transcript content deliberately. All such data remains
//! app-controlled and on-device, and clear/delete/retention paths apply to it.
//! Provider source transcripts are never modified or deleted. [`schema`] carries the
//! detailed contract. Exports have their own, narrower content policy in
//! [`crate::export`].
//!
//! # Concurrency
//!
//! The connection lives behind a mutex and database methods are synchronous.
//! Native callbacks read a separate last-committed settings snapshot. Callers
//! run long work outside the connection lock and come here to write the result.
//!
//! # Write order evidence
//!
//! [`Store::open`] holds the one writing connection. [`open_read_only`]
//! connections never write and supply no order evidence. A [`Revision`] is
//! the writing connection's `total_changes()`, read under the mutex after a
//! commit or together with the rows a read returns. An [`Incarnation`] is
//! the persisted creation identity of one session row.

mod check_preferences;
pub(crate) use check_preferences::{check_preferences_snapshot_in, enabled_checks_in};
pub(crate) mod codex_rollout_checkpoint;
mod evidence_queue;
pub(crate) mod memories;
pub(crate) mod native_tool_results;
pub use native_tool_results::{ToolResultBatch, ToolResultPublishOptions};
pub(crate) mod burn_checks;
pub mod model;
pub(crate) mod provider_limit;
pub(crate) mod provider_usage_history;
mod remediation;
mod retired_checks_migration;
pub(crate) mod revision_sql;
mod schema;
mod selected_content;
pub(crate) mod selected_source;
pub(crate) use selected_source::native_opencode_sqlite_format;
mod session_persistence;
mod session_write;
pub(crate) use session_write::SessionTitleRefreshGuard;
use session_write::SessionWriteState;
pub use session_write::{SessionCommitRejection, SessionWriteGuard};

/// Result of a guarded session write at the Store boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommitOutcome {
    Upserted(SessionUpsertResult),
    Deleted(Option<(Incarnation, Revision)>),
    Rejected(SessionCommitRejection),
}
mod source_fingerprint;
pub(crate) use source_fingerprint::PendingSourceFingerprint;
mod settings;
mod usage_accounting;
pub use usage_accounting::BurnCheckUsageSummary;

#[cfg(test)]
mod privacy_tests;
mod publication;
#[cfg(test)]
mod publish_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, TryLockError};
use std::time::Duration;

use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, FenceScope, ModelRun, PARSER_REVISION,
    PublishedContent, PublishedScope, ResumeRevisions, SessionCoverageRecord, StoredResume,
    TurnFacts, TurnRow, TurnRowError, TurnRowStore, TurnSessionKey, count_turn_rows,
    delete_source_resume, delete_source_rows_at_fence, delete_stale_source_resume,
    delete_turn_rows, delete_turn_rows_except_fence, delete_turn_rows_for_fence,
    insert_coverage_record, insert_source_resume, insert_turn_rows, latest_turn_execution,
    query_coverage_record, query_model_breakdown, query_model_runs, query_pricing_breakdown,
    query_source_resume, query_turn_content_offset_selected, query_turn_facts, query_turn_rows,
};
use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, named_params, params, params_from_iter};

use crate::dto::{BurnCheckSnoozePayload, DeferredPermissionDir};
pub(crate) use session_persistence::apply_session_retention_in;
use session_persistence::*;
pub use session_persistence::{FencedTurnRowStore, HygieneSummaryRow, iso_from_epoch, now_rfc3339};
use settings::read_settings;

pub use model::{
    ActiveCursor, AnalysisRecord, AppSettings, DisabledAgents, DiskSpaceDisplay, EvidenceClaim,
    EvidenceCompletion, EvidenceFailure, EvidenceRow, EvidenceStatus, HiddenMeters, Incarnation,
    MAX_ACTIVITY_DAYS, MILESTONE_OPTIONS, MIN_ACTIVITY_DAYS, Milestones, NudgePlacement,
    OwningSession, Presence, ProjectionRevisions, PublishedEvidence, PublishedModel,
    RETAIN_SESSION_DATA_FOREVER, RelationKind, RelationRecord, Remediation,
    RemediationEvidenceGuard, RemediationRecord, RemediationResult, RemediationState,
    RepositoryRecord, Revision, SessionActivityKey, SessionBadgeMetric, SessionKey, SessionRecord,
    SourcePublishMode, SourcePublishOutcome, SourceVersionState, ThemePreference,
    UsageEvidenceRecord, WorkingWeek,
};
pub(crate) use remediation::{
    PassiveRemediation, RemediationContribution, RemediationDisplaySnapshot,
};

/// Evidence rows that still wait for, or sit in, processing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvidenceBacklogCounts {
    pub pending: u64,
    pub processing: u64,
}

/// Replay inputs from one publication, read under one connection lock.
pub(crate) struct PublishedReplay {
    pub rows: Vec<TurnRow>,
    pub record: AnalysisRecord,
    pub relations: Vec<RelationRecord>,
}

/// Internal-scalar key holding the protected directories the last pass declined
/// to read.
pub const DEFERRED_PERMISSION_DIRS_KEY: &str = "internal:deferredPermissionDirs";
const PROVIDER_ACCOUNT_SECRET_KEY: &str = "internal:providerAccountHmacSecretV1";
const BURN_CHECK_SNOOZES_KEY: &str = "internal:burnCheckSnoozesV1";
const REPOSITORY_POLICY_GENERATION_KEY: &str = "internal:repositoryPolicyGenerationV1";

fn encode_secret(secret: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in secret {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn decode_secret(encoded: &str) -> Option<[u8; 32]> {
    if encoded.len() != 64 || !encoded.is_ascii() {
        return None;
    }
    let mut secret = [0_u8; 32];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)? as u8;
        let low = (pair[1] as char).to_digit(16)? as u8;
        secret[index] = high << 4 | low;
    }
    Some(secret)
}

const EVIDENCE_BY_KEY_SQL: &str = "SELECT environment_key, agent, session_id, status,
            analyzed_generation, processed_fingerprint,
            parser_revision, analyzer_revision, evidence_schema_revision,
            evidence_json, retry_count, claim_fence,
            claimed_at_epoch, lease_expires_at_epoch,
            next_attempt_at_epoch, analyzed_at_epoch, last_error, published_fence
       FROM session_evidence
      WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3";

/// How many undelivered analytics events are kept before the oldest are
/// dropped. At the flusher's 50-per-15-minutes this is several hours of
/// backlog, which is more than an ordinary outage needs and far less than an
/// unbounded table on a reader's disk.
const ANALYTICS_QUEUE_LIMIT: u32 = 500;

/// Maximum activity or session keys bound by one scan-history query.
///
/// Three values per key keep each statement below SQLite's legacy 999-value
/// limit. A scan can issue more batches, but it never loads unrelated rows.
const SCAN_HISTORY_KEY_BATCH_SIZE: usize = 256;

fn session_activity_key_predicates(key_count: usize) -> String {
    (0..key_count)
        .map(|index| {
            let first = index * 3 + 1;
            format!(
                "(s.source_label = ?{} AND s.environment_key = ?{first} AND s.agent = ?{})",
                first + 2,
                first + 1
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn session_records_for_activity_keys_sql(key_count: usize) -> String {
    format!(
        "{SESSION_SELECT_SQL}\n WHERE {}",
        session_activity_key_predicates(key_count)
    )
}

fn session_keys_for_activity_keys_sql(key_count: usize) -> String {
    format!(
        "SELECT s.environment_key, s.agent, s.session_id, s.source_label, s.incarnation\n       FROM session s\n      WHERE {}",
        session_activity_key_predicates(key_count)
    )
}

/// The session identity and incarnation behind each activity key one
/// watcher burst resolved.
pub type ActivityIdentities = HashMap<SessionActivityKey, (SessionKey, Incarnation)>;

/// Session identities and the writer revision returned by one upsert batch.
pub type SessionUpsertResult = (Vec<(SessionKey, Incarnation)>, Revision);

/// Persisted policy inputs used by scans and repository refreshes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPolicySnapshot {
    pub generation: u64,
    pub ignored_paths: HashSet<String>,
    pub granted_dirs: HashSet<String>,
    pub deferred_dirs: Vec<DeferredPermissionDir>,
}

/// A repository refresh either publishes as one policy snapshot or loses to a
/// newer policy change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryPolicyPublishOutcome {
    Published,
    Superseded,
}

/// Maximum session keys one [`Store::session_presence_for_keys`] call
/// answers. Three values per key keep the one statement below SQLite's
/// legacy 999-value limit. A caller pages larger sets itself.
pub const PRESENCE_LOOKUP_CAP: usize = 256;

/// The presence lookup: one primary-key search per requested identity.
fn session_presence_for_keys_sql(key_count: usize) -> String {
    let predicates = (0..key_count)
        .map(|index| {
            let first = index * 3 + 1;
            format!(
                "(environment_key = ?{first} AND agent = ?{} AND session_id = ?{})",
                first + 1,
                first + 2
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("{PRESENCE_SELECT_SQL}\n      WHERE {predicates}")
}

/// The column list every [`Presence`] reader uses, in [`presence_from_row`]'s
/// order.
const PRESENCE_SELECT_SQL: &str = "SELECT environment_key, agent, session_id, incarnation,
            COALESCE(updated_at_epoch, 0)
       FROM session";

fn presence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Presence> {
    Ok(Presence {
        key: SessionKey::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ),
        incarnation: Incarnation(row.get::<_, u64>(3)?),
        epoch: row.get::<_, i64>(4)?,
    })
}

/// File name of the database inside the app data directory.
///
/// Debug builds use their own file so a half-finished migration cannot damage
/// an installed copy — the same split the engine applies to its own state
/// files. The `dev` scripts already land in a different *directory* (they carry
/// `tauri.debug.conf.json`, which gives a development build its own bundle
/// identifier), so this branch is the backstop for the one path that does not:
/// a bare `cargo run` inside `src-tauri`, which compiles the release
/// identifier.
fn database_file() -> &'static str {
    if cfg!(debug_assertions) {
        "antiburn-debug.sqlite3"
    } else {
        "antiburn.sqlite3"
    }
}

/// Path of the database inside `data_dir`.
pub fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join(database_file())
}

/// Opens a second connection for reads only. It never writes.
pub fn open_read_only(data_dir: &Path, busy_timeout: Duration) -> Result<Connection> {
    let path = database_path(data_dir);
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {} for reporting", path.display()))?;
    connection.busy_timeout(busy_timeout)?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

/// The app's local database.
///
/// `connection` is behind an `Arc` so a cheap [`Store::clone`] shares the
/// same underlying connection rather than opening a second one. The worker
/// uses this to hand a [`FencedTurnRowStore`] a handle to the same database
/// without threading `Store` state through every call site as `Arc<Store>`.
#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    destructive_write_epoch: Arc<AtomicU64>,
    session_write_state: Arc<Mutex<SessionWriteState>>,
    settings_snapshot: Arc<RwLock<AppSettings>>,
    limit_factor_learn: Arc<Mutex<()>>,
    insights_work_turn: Arc<AtomicUsize>,
    #[cfg(test)]
    burn_check_test_hooks: burn_checks::test_hooks::Hooks,
    /// The directory the engine's own state files (scan roots, ignored paths)
    /// live in. The engine never chooses this; the shell does.
    state_dir: PathBuf,
}

fn recent_sessions_sql(origin_filter: &str, exclusion_filter: &str) -> String {
    format!(
        "SELECT environment_key, agent, session_id, source_kind,
                source_label, wsl_distro, title, title_source, cwd, surface, updated_at_epoch,
                activity_cursor, activity_source, subagent_count,
                (SELECT related_id FROM session_relation r
                   WHERE r.environment_key = s.environment_key
                     AND r.agent = s.agent
                     AND r.session_id = s.session_id
                     AND r.kind = 'forkParent'
                   LIMIT 1),
                s.source_fingerprint
           FROM session s
          WHERE COALESCE(updated_at_epoch, 0) >= :since_epoch{origin_filter}{exclusion_filter}
          ORDER BY COALESCE(updated_at_epoch, 0) DESC, session_id DESC
          LIMIT :limit"
    )
}

/// A schema test checks that [`Store::sessions_active_since_page`]'s first page uses the
/// keyset index. `?1` is the window start and `?2` the row limit. The order
/// is a strict total order: its last three columns are the primary key.
const SESSIONS_ACTIVE_PAGE_FIRST_SQL: &str =
    "SELECT environment_key, agent, session_id, incarnation,
            COALESCE(updated_at_epoch, 0)
       FROM session
      WHERE COALESCE(updated_at_epoch, 0) >= ?1
      ORDER BY COALESCE(updated_at_epoch, 0) DESC, session_id DESC,
               environment_key DESC, agent DESC
      LIMIT ?2";

/// [`Store::sessions_active_since_page`]'s later pages. `?3` to `?6` are the
/// previous page's last row in page order; the page starts strictly after
/// it.
///
/// The nested predicate is the form the planner matches to the index as a
/// range bounded on both sides, with no temporary sort. A row-value
/// comparison seeks on the window bound only. The scan starts at the
/// cursor's epoch, so rows that tie on that epoch and precede the cursor
/// are read and filtered: one page costs at most `limit` returned rows plus
/// the already visited rows at the cursor's epoch.
const SESSIONS_ACTIVE_PAGE_NEXT_SQL: &str =
    "SELECT environment_key, agent, session_id, incarnation,
            COALESCE(updated_at_epoch, 0)
       FROM session
      WHERE COALESCE(updated_at_epoch, 0) >= ?1
        AND COALESCE(updated_at_epoch, 0) <= ?3
        AND (COALESCE(updated_at_epoch, 0) < ?3
             OR session_id < ?4
             OR (session_id = ?4 AND environment_key < ?5)
             OR (session_id = ?4 AND environment_key = ?5 AND agent < ?6))
      ORDER BY COALESCE(updated_at_epoch, 0) DESC, session_id DESC,
               environment_key DESC, agent DESC
      LIMIT ?2";

/// Ceiling on how many uuids [`Store::sessions_owning_turn_uuids`] matches
/// in one call, applied to the `IN (...)` list it builds.
const FORK_LINEAGE_UUID_CAP: usize = 8;

/// Keep each watcher lookup below SQLite's host-parameter limit.
const SOURCE_LABEL_LOOKUP_CHUNK_SIZE: usize = 500;

/// A schema test checks that [`Store::active_native_file_source_labels`]'s query uses the
/// `session_recency_keyset` index. `?1` is the active window's cutoff epoch,
/// exclusive, and the leading `WHERE` term names the index's own `COALESCE`
/// expression so the planner can seek the range instead of scanning every row.
///
/// `INDEXED BY session_recency_keyset` is deliberate, not just a test
/// convenience, for the same reason as `sessions_owning_turn_uuids_sql`'s own
/// hint: on almost every install `environment_key = 'native'` matches nearly
/// every row, so the equality-first `session_insights_window` index the
/// planner would otherwise prefer barely narrows the scan. The active window
/// is only a few minutes wide, so seeking the recency index straight to its
/// cutoff is far cheaper regardless of how many sessions a reader has.
const ACTIVE_NATIVE_FILE_SOURCE_LABELS_SQL: &str = "SELECT source_label
       FROM session INDEXED BY session_recency_keyset
      WHERE COALESCE(updated_at_epoch, 0) > ?1
        AND environment_key = 'native'
        AND source_kind = 'file'";

/// Build the indexed query for one chunk of native file source labels.
fn native_file_session_activity_keys_sql(source_label_count: usize) -> String {
    let placeholders = vec!["?"; source_label_count].join(", ");
    format!(
        "SELECT agent, source_label
           FROM session
          WHERE source_label IN ({placeholders})
            AND environment_key = 'native'
            AND source_kind = 'file'"
    )
}

/// A scalar subquery counting one session's published turn rows, correlated
/// to an outer `session s` row. Shared by [`sessions_owning_turn_uuids_sql`]
/// and [`Store::owning_session_stats`]'s query so the two definitions of
/// "published turn rows" cannot drift apart.
const PUBLISHED_TURN_ROW_COUNT_SQL: &str = "(SELECT COUNT(*)
                   FROM turn pt
                   JOIN session_evidence pe
                     ON pe.environment_key = pt.environment_key
                    AND pe.agent = pt.agent
                    AND pe.session_id = pt.session_id
                    AND pe.published_fence = pt.claim_fence
                  WHERE pt.environment_key = s.environment_key
                    AND pt.agent = s.agent
                    AND pt.session_id = s.session_id)";

/// Build [`Store::sessions_owning_turn_uuids`]'s query for `uuid_count` bound UUIDs.
/// A schema test checks that this query uses the `turn_uuid` index.
///
/// `INDEXED BY turn_uuid` is deliberate, not just a test convenience: a
/// uuid is far more selective than `environment_key`/`agent`, but SQLite's
/// planner cannot know that without `ANALYZE` statistics, and could
/// otherwise pick `turn_session_thread` instead. Forcing `turn_uuid` keeps
/// this lookup cheap regardless of how a reader's own database is shaped.
fn sessions_owning_turn_uuids_sql(uuid_count: usize) -> String {
    let placeholders = vec!["?"; uuid_count].join(", ");
    format!(
        "WITH matched AS (
             SELECT DISTINCT t.session_id
               FROM turn t INDEXED BY turn_uuid
               JOIN session_evidence e
                 ON e.environment_key = t.environment_key
                AND e.agent = t.agent
                AND e.session_id = t.session_id
                AND e.published_fence = t.claim_fence
              WHERE t.environment_key = ?
                AND t.agent = ?
                AND t.session_id <> ?
                AND t.uuid IN ({placeholders})
         )
         SELECT s.session_id, s.first_seen_at,
                {PUBLISHED_TURN_ROW_COUNT_SQL} AS published_turn_rows
           FROM session s
           JOIN matched m ON m.session_id = s.session_id
          WHERE s.environment_key = ?
            AND s.agent = ?"
    )
}

fn remove_native_tool_results_from_other_formats_in(
    transaction: &rusqlite::Transaction<'_>,
    session: &SessionKey,
    source_format: &str,
    source_generation: i64,
    claim_fence: i64,
) -> Result<()> {
    let mut statement = transaction.prepare(
        "SELECT source_format, native_identity, native_position_json, native_revision,
                payload_json
           FROM native_tool_result_current
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND source_format <> ?4 AND is_removed = 0",
    )?;
    let stale_results = statement
        .query_map(
            params![
                session.environment_key,
                session.agent,
                session.session_id,
                source_format
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    for (old_format, identity, position, revision, payload) in stale_results {
        transaction.execute(
            "INSERT INTO native_tool_result_publication (
                 outcome, environment_key, agent, session_id, source_format,
                 native_identity, native_position_json, native_revision, payload_json,
                 complete, source_generation, claim_fence)
             VALUES ('removal', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10)",
            params![
                session.environment_key,
                session.agent,
                session.session_id,
                old_format,
                identity,
                position,
                revision,
                payload,
                source_generation,
                claim_fence
            ],
        )?;
        let publication_sequence = transaction.last_insert_rowid();
        transaction.execute(
            "UPDATE native_tool_result_current SET is_removed = 1,
                    publication_sequence = ?6, source_generation = ?7, claim_fence = ?8
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND source_format = ?4 AND native_identity = ?5 AND is_removed = 0",
            params![
                session.environment_key,
                session.agent,
                session.session_id,
                old_format,
                identity,
                publication_sequence,
                source_generation,
                claim_fence
            ],
        )?;
    }
    Ok(())
}

impl Store {
    /// Open (creating if absent) and migrate the database under `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("failed to create {}", data_dir.display()))?;
        let path = database_path(data_dir);
        let connection = Connection::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        Store::from_connection(connection, data_dir.to_path_buf())
    }

    /// An in-memory database, for tests.
    #[cfg(test)]
    pub fn open_in_memory(state_dir: &Path) -> Result<Store> {
        Store::from_connection(Connection::open_in_memory()?, state_dir.to_path_buf())
    }

    /// A [`Store`] whose connection is a fresh [`open_read_only`] WAL reader,
    /// sharing every other Arc with `self`.
    ///
    /// Call this only on an already migrated store: [`Store::open`] always
    /// migrates before returning, so this holds by construction for every
    /// caller. A write through the returned store fails with SQLite's
    /// read-only error, on purpose — the connection carries
    /// `SQLITE_OPEN_READ_ONLY`.
    pub fn open_reader(&self, busy_timeout: Duration) -> Result<Store> {
        let connection = open_read_only(&self.state_dir, busy_timeout)?;
        Ok(Store {
            connection: Arc::new(Mutex::new(connection)),
            destructive_write_epoch: Arc::clone(&self.destructive_write_epoch),
            session_write_state: Arc::clone(&self.session_write_state),
            settings_snapshot: Arc::clone(&self.settings_snapshot),
            limit_factor_learn: Arc::clone(&self.limit_factor_learn),
            insights_work_turn: Arc::clone(&self.insights_work_turn),
            #[cfg(test)]
            burn_check_test_hooks: Arc::clone(&self.burn_check_test_hooks),
            state_dir: self.state_dir.clone(),
        })
    }

    fn from_connection(connection: Connection, state_dir: PathBuf) -> Result<Store> {
        // WAL keeps a read during a scan write from blocking, and `NORMAL` is
        // the documented companion: a crash can lose the last commit, which for
        // a rebuildable cache is cheaper than an fsync per transaction.
        connection.pragma_update(None, "journal_mode", "WAL").ok();
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let store = Store {
            connection: Arc::new(Mutex::new(connection)),
            destructive_write_epoch: Arc::new(AtomicU64::new(0)),
            session_write_state: Arc::new(Mutex::new(SessionWriteState::default())),
            settings_snapshot: Arc::new(RwLock::new(AppSettings::default())),
            limit_factor_learn: Arc::new(Mutex::new(())),
            insights_work_turn: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            burn_check_test_hooks: Arc::new(Mutex::new(None)),
            state_dir,
        };
        store.migrate()?;
        store.lock().execute(
            "WITH legacy AS (
                SELECT rowid, session_id, CASE WHEN json_valid(substr(admission_revision, ?1))
                    THEN json_extract(substr(admission_revision, ?1), '$[0]') END AS physical
                  FROM session WHERE fingerprint_source_path IS NULL
                    AND environment_key = 'native' AND agent = 'opencode'
                    AND admission_revision LIKE 'opencode-admission-v1:%'
             ) UPDATE session SET fingerprint_source_path = (
                 SELECT substr(physical, 12, length(physical) - 12 - length(legacy.session_id))
                   FROM legacy WHERE legacy.rowid = session.rowid)
               WHERE rowid IN (SELECT rowid FROM legacy WHERE substr(physical, 1, 11) = 'providerDb:'
                 AND substr(physical, -length(session_id) - 1) = ':' || session_id)",
            ["opencode-admission-v1:".len() + 1],
        )?;
        store.lock().execute(
            "UPDATE session SET fingerprint_in_flight = 0, fingerprint_next_attempt_at_epoch = NULL
              WHERE fingerprint_in_flight = 1",
            [],
        )?;
        store.check_preferences_snapshot()?;
        store.update_settings_snapshot(&store.settings()?);
        Ok(store)
    }

    /// The directory the engine's state files live in.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub(crate) fn take_insights_work_turn(&self) -> usize {
        self.insights_work_turn.fetch_add(1, Ordering::Relaxed) % 3
    }

    /// Apply every migration the database has not seen yet.
    ///
    /// Idempotent: running it twice is a no-op, which is what makes reopening
    /// an already-current database free.
    fn migrate(&self) -> Result<()> {
        let mut guard = self.lock();
        let mut current: i64 = guard.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let is_new_database = current == 0;
        // PR builds used v49 for incarnation before main assigned v49 to remediation.
        if current == 49
            && guard.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session') WHERE name = 'incarnation')",
                [],
                |row| row.get::<_, bool>(0),
            )?
        {
            let tx = guard.transaction()?;
            let complete: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_incarnation_seq WHERE id = 1 AND value >= 0)
                    AND EXISTS(SELECT 1 FROM sqlite_master
                        WHERE type = 'index' AND name = 'session_recency_keyset')",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(complete, "incomplete prerelease lifecycle schema at version 49");
            // Preserve incarnation values and apply the missing main migrations atomically.
            for sql in &schema::MIGRATIONS[48..50] {
                tx.execute_batch(sql)?;
            }
            tx.pragma_update(None, "user_version", 51)?;
            tx.commit()?;
            current = 51;
        }
        // Branch builds assigned v52 to the refusal column before main used
        // v52 for model lanes. Apply the main migrations to that shape.
        if (current == 52 || current == 53)
            && guard.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('provider_usage_observation')
                    WHERE name = 'refusal_kind')",
                [],
                |row| row.get::<_, bool>(0),
            )?
        {
            let tx = guard.transaction()?;
            tx.execute_batch(schema::MIGRATIONS[51])?;
            tx.execute_batch(schema::MIGRATIONS[52])?;
            tx.pragma_update(None, "user_version", 54)?;
            tx.commit()?;
            current = 54;
        }
        // Main used v54 and v55 for allowance changes before this branch used
        // those versions for remediation changes. Complete the remediation
        // shape before applying the appended migrations.
        if (current == 54 || current == 55)
            && guard.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('provider_usage_observation')
                    WHERE name = 'refusal_kind')",
                [],
                |row| row.get::<_, bool>(0),
            )?
        {
            let tx = guard.transaction()?;
            let remediation_ready: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('remediation')
                    WHERE name = 'prompt_group_id')",
                [],
                |row| row.get(0),
            )?;
            if !remediation_ready {
                tx.execute_batch(schema::MIGRATIONS[53])?;
                tx.execute_batch(schema::MIGRATIONS[54])?;
            }
            tx.pragma_update(None, "user_version", 56)?;
            tx.commit()?;
            current = 56;
        }
        current = schema::repair_branch_version(&mut guard, current)?;
        for (index, sql) in schema::MIGRATIONS.iter().enumerate() {
            let version = index as i64 + 1;
            if version <= current {
                continue;
            }
            let tx = guard.transaction()?;
            if version == retired_checks_migration::VERSION {
                retired_checks_migration::prepare(
                    &tx,
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .with_context(|| format!("migration {version} preparation failed"))?;
            }
            if version == 85 {
                schema::prepare_source_metadata(&tx)
                    .with_context(|| format!("migration {version} preparation failed"))?;
            }
            // The SQLite error is in the source chain either way, but a caller
            // that formats with `{}` rather than `{:#}` prints this line alone
            // — and tauri's setup hook is such a caller. A bare "migration 4
            // failed" is exactly what a developer saw when a database built
            // from an earlier numbering of these migrations met a table that
            // already existed; naming the conflict is what makes it
            // actionable rather than a trip to this file to guess.
            tx.execute_batch(sql)
                .map_err(|error| anyhow::anyhow!("migration {version} failed: {error}"))?;
            if is_new_database && version == 1 {
                tx.execute(
                    "INSERT INTO setting (key, value) VALUES (?1, 'true')",
                    [retired_checks_migration::FRESH_INITIALIZATION_KEY],
                )?;
            }
            // `pragma_update` cannot be parameterized, and `version` is derived
            // from the compiled-in migration list, never from input.
            tx.pragma_update(None, "user_version", version)?;
            tx.commit()?;
        }
        // Early V80 builds stored the admission revision without retry state.
        if !guard.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session') WHERE name = 'fingerprint_source_path')",
            [], |row| row.get::<_, bool>(0),
        )? {
            let tx = guard.transaction()?;
            tx.execute_batch(schema::FINGERPRINT_WORK)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// How many sessions the index currently holds.
    pub fn session_count(&self) -> Result<u32> {
        let connection = self.lock();
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM session", [], |row| row.get(0))?;
        Ok(count.max(0) as u32)
    }

    /// Size of the database file on disk, in bytes.
    ///
    /// Zero rather than an error when it has not been written yet: "nothing on
    /// disk" is a real state on a fresh install and in the in-memory store the
    /// tests use, and it is not worth a failure path in a settings row.
    pub fn database_bytes(&self) -> u64 {
        std::fs::metadata(database_path(&self.state_dir))
            .map(|metadata| metadata.len())
            .unwrap_or(0)
    }

    /// The applied schema version.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .lock()
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// The current write order position. Every committed write that changed
    /// a row ended above this value if it committed before this call.
    pub fn revision(&self) -> Revision {
        revision_of(&self.lock())
    }

    /// A poisoned lock still holds a usable connection: the panic that poisoned
    /// it happened in a caller, not inside SQLite.
    ///
    /// `pub(crate)` rather than private: [`crate::provider_usage::factor`]'s
    /// tests build synthetic turns, periods, and observations directly, the
    /// same way the store's own lifecycle tests do, from outside the `store`
    /// module tree.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start one shared limit-factor learning pass, or skip work another
    /// clone already owns.
    pub(crate) fn try_begin_limit_factor_learn(&self) -> Option<std::sync::MutexGuard<'_, ()>> {
        match self.limit_factor_learn.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /* --------------------------------------------------------------------
     * Settings
     * ----------------------------------------------------------------- */

    /// A non-preference scalar the shell persists for itself, by key.
    ///
    /// Same table as the settings, different audience: these rows carry state
    /// (a last-fired timestamp, a delivered-milestone ledger) rather than a
    /// choice, so they are namespaced under `internal:` and never surface in
    /// [`Store::settings`]. `None` covers both "never written" and an
    /// unreadable store — callers treat absence as "act as if new".
    pub fn internal_value(&self, key: &str) -> Option<String> {
        let connection = self.lock();
        connection
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .ok()
    }

    /// Write (or replace) an internal scalar. Errors are swallowed by design:
    /// every caller is a background loop for which "the seed did not persist"
    /// degrades to a duplicate notification after a relaunch, which is a
    /// better failure than a loop that stops.
    pub fn set_internal_value(&self, key: &str, value: &str) {
        let connection = self.lock();
        let _ = connection.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    pub fn set_internal_value_checked(&self, key: &str, value: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn set_internal_values_checked(&self, values: &[(&str, &str)]) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        for (key, value) in values {
            tx.execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Read the burn-check snooze ledger. A missing ledger is empty.
    pub fn burn_check_snoozes(&self) -> Result<Vec<BurnCheckSnoozePayload>> {
        let connection = self.lock();
        let stored = connection
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                params![BURN_CHECK_SNOOZES_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        stored
            .map(|value| serde_json::from_str(&value).context("invalid burn-check snooze ledger"))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Replace the bounded burn-check snooze ledger.
    pub fn save_burn_check_snoozes(&self, value: &str) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![BURN_CHECK_SNOOZES_KEY, value],
        )?;
        Ok(())
    }

    /// Read both reader ledgers. Native consumers keep the native-only method above.
    pub fn check_snoozes(&self) -> Result<Vec<crate::dto::CheckSnoozePayload>> {
        use crate::dto::{CheckSnoozeId, CheckSnoozePayload};
        let mut records = self
            .burn_check_snoozes()?
            .into_iter()
            .map(|record| CheckSnoozePayload {
                detector: CheckSnoozeId::Native(record.detector),
                scope: record.scope,
                until: record.until,
            })
            .collect::<Vec<_>>();
        let connection = self.lock();
        let stored: Option<String> = connection
            .query_row(
                "SELECT value FROM setting WHERE key = 'internal:installedCheckSnoozesV1'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(stored) = stored {
            let installed: Vec<CheckSnoozePayload> =
                serde_json::from_str(&stored).context("invalid installed check snooze ledger")?;
            anyhow::ensure!(
                installed
                    .iter()
                    .all(|record| matches!(record.detector, CheckSnoozeId::Installed(_))),
                "native identity in installed snooze ledger"
            );
            records.extend(installed);
        }
        Ok(records)
    }

    /// Replace one reader suppression without changing check execution preferences.
    pub fn set_check_snooze(&self, snooze: &crate::dto::CheckSnoozePayload) -> Result<()> {
        use crate::dto::CheckSnoozeId;
        match &snooze.detector {
            CheckSnoozeId::Native(detector) => {
                let mut records = self.burn_check_snoozes()?;
                records.retain(|record| record.detector != *detector);
                records.push(BurnCheckSnoozePayload {
                    detector: *detector,
                    scope: snooze.scope,
                    until: snooze.until,
                });
                self.save_burn_check_snoozes(&serde_json::to_string(&records)?)
            }
            CheckSnoozeId::Installed(_) => {
                let mut records = self
                    .check_snoozes()?
                    .into_iter()
                    .filter(|record| {
                        matches!(record.detector, CheckSnoozeId::Installed(_))
                            && record.detector != snooze.detector
                    })
                    .collect::<Vec<_>>();
                records.push(snooze.clone());
                self.set_internal_value_checked(
                    "internal:installedCheckSnoozesV1",
                    &serde_json::to_string(&records)?,
                )
            }
        }
    }

    /// Clear one suppression in its original ledger.
    pub fn clear_check_snooze(&self, detector: &crate::dto::CheckSnoozeId) -> Result<()> {
        use crate::dto::CheckSnoozeId;
        match detector {
            CheckSnoozeId::Native(detector) => {
                let mut records = self.burn_check_snoozes()?;
                records.retain(|record| record.detector != *detector);
                self.save_burn_check_snoozes(&serde_json::to_string(&records)?)
            }
            CheckSnoozeId::Installed(_) => {
                let records = self
                    .check_snoozes()?
                    .into_iter()
                    .filter(|record| {
                        matches!(record.detector, CheckSnoozeId::Installed(_))
                            && &record.detector != detector
                    })
                    .collect::<Vec<_>>();
                self.set_internal_value_checked(
                    "internal:installedCheckSnoozesV1",
                    &serde_json::to_string(&records)?,
                )
            }
        }
    }

    /// Return the durable random secret used for provider account keys.
    pub fn provider_account_secret(&self) -> Result<[u8; 32]> {
        let connection = self.lock();
        if let Some(encoded) = connection
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                params![PROVIDER_ACCOUNT_SECRET_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return decode_secret(&encoded).context("invalid provider account secret");
        }

        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        connection.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)",
            params![PROVIDER_ACCOUNT_SECRET_KEY, encode_secret(&secret)],
        )?;
        Ok(secret)
    }

    /// Apply the stored session-data retention policy.
    ///
    /// Returns how many sessions it removed and the revision after the
    /// commit. Every removed row is absent at that revision.
    pub fn apply_session_retention(&self, now_epoch: i64) -> Result<(usize, Revision)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let settings = read_settings(&tx)?;
        let removed =
            apply_session_retention_in(&tx, settings.session_data_retention_days, now_epoch)?;
        let surviving_keys = if removed > 0 {
            let mut statement =
                tx.prepare("SELECT environment_key, agent, session_id FROM session")?;
            statement
                .query_map([], |row| {
                    Ok(SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<HashSet<_>>>()?
        } else {
            HashSet::new()
        };
        tx.commit()?;
        if removed > 0 {
            self.destructive_write_epoch.fetch_add(1, Ordering::Release);
            let mut state = self
                .session_write_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.key_fences.clear();
            state
                .physical_sources
                .retain(|key, _| surviving_keys.contains(key));
        }
        Ok((removed, revision_of(&connection)))
    }

    /* --------------------------------------------------------------------
     * Scan roots
     * ----------------------------------------------------------------- */

    /// The extra directories the user pointed the scanner at, oldest first.
    pub fn scan_roots(&self) -> Result<Vec<String>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT path FROM scan_root ORDER BY added_at, path")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Record a scan root. Idempotent, and a no-op for an empty path.
    pub fn add_scan_root(&self, path: &str) -> Result<()> {
        let path = path.trim_end_matches(['/', '\\']);
        if path.is_empty() {
            return Ok(());
        }
        self.lock().execute(
            "INSERT INTO scan_root (path, added_at) VALUES (?1, ?2)
             ON CONFLICT(path) DO NOTHING",
            params![path, now_rfc3339()],
        )?;
        Ok(())
    }

    /// Forget a scan root. Idempotent.
    pub fn remove_scan_root(&self, path: &str) -> Result<()> {
        let path = path.trim_end_matches(['/', '\\']);
        self.lock()
            .execute("DELETE FROM scan_root WHERE path = ?1", params![path])?;
        Ok(())
    }

    /* --------------------------------------------------------------------
     * Consent grants
     * ----------------------------------------------------------------- */

    /// Protected directory names the user has granted access to.
    ///
    /// Read at the start of every pass that might touch the filesystem, and
    /// trusted as-is: confirming a grant means reading the directory, which is
    /// the very thing that prompts.
    pub fn granted_dirs(&self) -> Result<HashSet<String>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT dir_name FROM consent_grant")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
    }

    /// Record that the user granted access to a protected directory.
    /// Idempotent; re-granting refreshes when the decision was made.
    pub fn grant_dir(&self, dir_name: &str) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let changed = tx.execute(
            "INSERT INTO consent_grant (dir_name, granted_at) VALUES (?1, ?2)
             ON CONFLICT(dir_name) DO UPDATE SET granted_at = excluded.granted_at
                 WHERE consent_grant.granted_at <> excluded.granted_at",
            params![dir_name, now_rfc3339()],
        )?;
        if changed > 0 {
            bump_repository_policy_generation_in(&tx)?;
        }
        tx.commit()?;
        if changed > 0 {
            self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    /// Drop a recorded grant, after a read under it came back denied.
    /// Idempotent.
    pub fn revoke_dir_grant(&self, dir_name: &str) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let changed = tx.execute(
            "DELETE FROM consent_grant WHERE dir_name = ?1",
            params![dir_name],
        )?;
        if changed > 0 {
            bump_repository_policy_generation_in(&tx)?;
        }
        tx.commit()?;
        if changed > 0 {
            self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    /// Snapshot the persisted repository and folder policy with its generation.
    pub fn repository_policy_snapshot(&self) -> Result<RepositoryPolicySnapshot> {
        let connection = self.lock();
        let generation = read_repository_policy_generation(&connection)?;
        let mut ignored_statement = connection.prepare(
            "SELECT COALESCE(repo_root, suspected_path) FROM repository WHERE enabled = 0",
        )?;
        let ignored_paths = ignored_statement
            .query_map([], |row| row.get::<_, Option<String>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .map(|path| antiburn_local::paths::ignored_paths::normalize(&path))
            .collect();
        let mut grant_statement = connection.prepare("SELECT dir_name FROM consent_grant")?;
        let granted_dirs = grant_statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        let deferred_dirs = connection
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                params![DEFERRED_PERMISSION_DIRS_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        Ok(RepositoryPolicySnapshot {
            generation,
            ignored_paths,
            granted_dirs,
            deferred_dirs,
        })
    }

    /* Anonymised application events. */

    /// Queue one event for delivery. Callers hold the consent check; this is
    /// storage, and a queue that decided policy for itself would be a second
    /// place for the gate to drift out of step with the reader's choice.
    ///
    /// Bounded, and the bound is load-bearing rather than defensive. Events
    /// now include interactions, which a reader can generate as fast as they
    /// can click; a machine offline for a week would otherwise accumulate
    /// them without limit in the reader's own database. The oldest go first —
    /// the newest events are the ones still worth having, and a queue that
    /// dropped the newest would report a machine's distant past forever.
    pub fn queue_analytics_event(&self, name: &str, payload: &str) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO analytics_event (name, payload, queued_at) VALUES (?1, ?2, ?3)",
            params![name, payload, now_rfc3339()],
        )?;
        connection.execute(
            "DELETE FROM analytics_event WHERE id NOT IN
                 (SELECT id FROM analytics_event ORDER BY id DESC LIMIT ?1)",
            params![ANALYTICS_QUEUE_LIMIT],
        )?;
        Ok(())
    }

    /// Queue an analytics event in a transaction that also changes settings.
    #[cfg(feature = "analytics")]
    pub(crate) fn queue_analytics_event_in(
        transaction: &rusqlite::Transaction<'_>,
        name: &str,
        payload: &str,
    ) -> Result<()> {
        transaction.execute(
            "INSERT INTO analytics_event (name, payload, queued_at) VALUES (?1, ?2, ?3)",
            params![name, payload, now_rfc3339()],
        )?;
        transaction.execute(
            "DELETE FROM analytics_event WHERE id NOT IN
                 (SELECT id FROM analytics_event ORDER BY id DESC LIMIT ?1)",
            params![ANALYTICS_QUEUE_LIMIT],
        )?;
        Ok(())
    }

    /// Read the current analytics identity inside a settings transaction.
    #[cfg(feature = "analytics")]
    pub(crate) fn analytics_identity_in(
        transaction: &rusqlite::Transaction<'_>,
    ) -> Result<Option<(String, String)>> {
        let mut statement = transaction
            .prepare("SELECT install_id, minted_at FROM analytics_identity WHERE id = 1")?;
        let mut rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Whether a durable opt-out event still waits for its final flush.
    pub fn analytics_opt_out_pending(&self) -> Result<bool> {
        Ok(self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM analytics_event WHERE name = 'antiburn.analytics_opted_out')",
            [],
            |row| row.get::<_, bool>(0),
        )?)
    }

    /// Return the current delivery backlog depth.
    pub fn analytics_event_count(&self) -> Result<u32> {
        Ok(self
            .lock()
            .query_row("SELECT COUNT(*) FROM analytics_event", [], |row| {
                row.get::<_, u32>(0)
            })?)
    }

    /// The next batch to attempt, oldest first, as `(id, payload)`.
    pub fn pending_analytics_events(&self, limit: u32) -> Result<Vec<(i64, String)>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, payload FROM analytics_event
                 ORDER BY CASE WHEN name = 'antiburn.analytics_opted_out' THEN 0 ELSE 1 END, id
                 LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Forget events that were delivered.
    pub fn drop_analytics_events(&self, ids: &[i64]) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM analytics_event WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Count a failed delivery, and drop whatever has now failed too often.
    ///
    /// Returns how many rows were given up on. The flusher ignores it — a
    /// dropped event is not worth a line of user-facing text, and analytics
    /// that reported their own failures would have their priorities inverted.
    /// It is returned so the give-up threshold is assertable from a test,
    /// which is the only way that arm is exercised at all.
    pub fn fail_analytics_events(&self, ids: &[i64], max_attempts: u32) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE analytics_event SET attempts = attempts + 1 WHERE id = ?1",
                params![id],
            )?;
        }
        let dropped = tx.execute(
            "DELETE FROM analytics_event WHERE attempts >= ?1",
            params![max_attempts],
        )?;
        tx.commit()?;
        Ok(dropped)
    }

    /// The current installation identifier and when it was minted, if one has
    /// been created. Absent until the reader's first consented event.
    pub fn analytics_identity(&self) -> Result<Option<(String, String)>> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT install_id, minted_at FROM analytics_identity WHERE id = 1")?;
        let mut rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Mint or rotate the installation identifier.
    pub fn set_analytics_identity(&self, install_id: &str) -> Result<()> {
        self.set_analytics_identity_with_time(install_id, &now_rfc3339())
    }

    fn set_analytics_identity_with_time(&self, install_id: &str, minted_at: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO analytics_identity (id, install_id, minted_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET install_id = excluded.install_id,
                                           minted_at  = excluded.minted_at",
            params![install_id, minted_at],
        )?;
        Ok(())
    }

    #[cfg(all(test, feature = "analytics"))]
    pub(crate) fn set_analytics_identity_at(
        &self,
        install_id: &str,
        minted_at: &str,
    ) -> Result<()> {
        self.set_analytics_identity_with_time(install_id, minted_at)
    }

    /// Opting out: the queue and the identity go together, in one transaction.
    ///
    /// Both halves matter. Leaving the queue would send, on a later opt-in,
    /// events the reader withdrew consent for; leaving the identity would let
    /// a later opt-in be joined to the earlier one, which is the whole thing
    /// the rotation exists to prevent.
    pub fn clear_analytics(&self) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM analytics_event", [])?;
        tx.execute("DELETE FROM analytics_identity", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Record which protected directories the last pass declined to read.
    ///
    /// An internal scalar rather than a table: it is derived state, replaced
    /// whole on every pass, and only ever read back as one list.
    pub fn set_deferred_permission_dirs(&self, dirs: &[DeferredPermissionDir]) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        tx.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![DEFERRED_PERMISSION_DIRS_KEY, serde_json::to_string(dirs)?],
        )?;
        bump_repository_policy_generation_in(&tx)?;
        tx.commit()?;
        self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// The protected directories the last pass declined to read.
    ///
    /// An unreadable or malformed value reads as "nothing deferred": the
    /// consequence is a missing prompt, where the alternative — failing the
    /// whole permissions query — would take the recovery interface down with it.
    pub fn deferred_permission_dirs(&self) -> Result<Vec<DeferredPermissionDir>> {
        Ok(self
            .internal_value(DEFERRED_PERMISSION_DIRS_KEY)
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default())
    }

    /* --------------------------------------------------------------------
     * Sessions
     * ----------------------------------------------------------------- */

    /// Write the session batch in one transaction. Return each row’s incarnation and
    /// the committed writer revision under the same lock.
    pub fn upsert_sessions(
        &self,
        records: &[SessionRecord],
        evidence_agents: &[&str],
    ) -> Result<SessionUpsertResult> {
        let mut connection = self.lock();
        upsert_sessions_locked(&mut connection, records, evidence_agents)
    }

    /// Sessions whose activity falls at or after `since_epoch`, newest first.
    pub fn recent_sessions(&self, since_epoch: i64, limit: usize) -> Result<Vec<SessionRecord>> {
        self.recent_sessions_excluding(since_epoch, limit, &DisabledAgents::default())
    }

    /// [`Self::recent_sessions`], without the sessions of the excluded agents.
    ///
    /// This is the display filter for [`DisabledAgents`]. Scan and analysis
    /// callers use [`Self::recent_sessions`], because a disabled agent stays
    /// indexed and analyzed.
    pub fn recent_sessions_excluding(
        &self,
        since_epoch: i64,
        limit: usize,
        excluded_agents: &DisabledAgents,
    ) -> Result<Vec<SessionRecord>> {
        self.recent_sessions_with_origin(since_epoch, limit, excluded_agents, false)
    }

    /// Filter local origins before the row limit so remote rows cannot hide them.
    pub fn recent_local_sessions_excluding(
        &self,
        since_epoch: i64,
        limit: usize,
        excluded_agents: &DisabledAgents,
    ) -> Result<Vec<SessionRecord>> {
        self.recent_sessions_with_origin(since_epoch, limit, excluded_agents, true)
    }

    fn recent_sessions_with_origin(
        &self,
        since_epoch: i64,
        limit: usize,
        excluded_agents: &DisabledAgents,
        local_only: bool,
    ) -> Result<Vec<SessionRecord>> {
        let excluded = excluded_agents.slugs();
        let origin_filter = if local_only {
            " AND (environment_key = 'native' OR environment_key LIKE 'wsl:%')"
        } else {
            ""
        };
        let exclusion_placeholders = (0..excluded.len())
            .map(|index| format!(":excluded_agent_{index}"))
            .collect::<Vec<_>>();
        let exclusion_filter = if exclusion_placeholders.is_empty() {
            String::new()
        } else {
            format!(" AND agent NOT IN ({})", exclusion_placeholders.join(", "))
        };
        let connection = self.lock();
        let sql = recent_sessions_sql(origin_filter, &exclusion_filter);
        let mut statement = connection.prepare(&sql)?;
        let limit = limit as i64;
        let mut bindings = named_params![":since_epoch": since_epoch, ":limit": limit].to_vec();
        bindings.extend(
            exclusion_placeholders
                .iter()
                .zip(excluded)
                .map(|(name, agent)| (name.as_str(), agent as &dyn rusqlite::ToSql)),
        );
        let rows = statement.query_map(bindings.as_slice(), session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Read at most `limit` active rows in full cursor order. Read their revision under
    /// the same writer lock. Cursor ties can require scanning earlier rows.
    pub fn sessions_active_since_page(
        &self,
        since_epoch: i64,
        after: Option<&ActiveCursor>,
        limit: usize,
    ) -> Result<(Vec<Presence>, Revision)> {
        let connection = self.lock();
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = match after {
            None => {
                let mut statement = connection.prepare(SESSIONS_ACTIVE_PAGE_FIRST_SQL)?;
                let rows = statement.query_map(params![since_epoch, limit], presence_from_row)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            }
            Some(cursor) => {
                let mut statement = connection.prepare(SESSIONS_ACTIVE_PAGE_NEXT_SQL)?;
                let rows = statement.query_map(
                    params![
                        since_epoch,
                        limit,
                        cursor.epoch,
                        cursor.session_id,
                        cursor.environment_key,
                        cursor.agent
                    ],
                    presence_from_row,
                )?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok((rows, revision_of(&connection)))
    }

    /// Read presence for at most [`PRESENCE_LOOKUP_CAP`] keys. One writer lock protects
    /// the rows and their revision. Missing keys are absent at that revision.
    pub fn session_presence_for_keys(
        &self,
        keys: &[SessionKey],
    ) -> Result<(Vec<Presence>, Revision)> {
        anyhow::ensure!(
            keys.len() <= PRESENCE_LOOKUP_CAP,
            "presence lookup of {} keys exceeds the cap of {PRESENCE_LOOKUP_CAP}",
            keys.len()
        );
        let connection = self.lock();
        if keys.is_empty() {
            return Ok((Vec::new(), revision_of(&connection)));
        }
        let mut values = Vec::with_capacity(keys.len() * 3);
        for key in keys {
            values.push(rusqlite::types::Value::Text(key.environment_key.clone()));
            values.push(rusqlite::types::Value::Text(key.agent.clone()));
            values.push(rusqlite::types::Value::Text(key.session_id.clone()));
        }
        let mut statement = connection.prepare(&session_presence_for_keys_sql(keys.len()))?;
        let rows = statement.query_map(params_from_iter(values.iter()), presence_from_row)?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((rows, revision_of(&connection)))
    }

    /// The analysis state of every session in the activity window, for the
    /// aggregate hygiene summary.
    ///
    /// The window and the agent exclusion match
    /// [`Self::recent_sessions_excluding`], so the summary describes the
    /// same sessions the list shows. A row carries its evidence JSON only
    /// when the evidence is `ready` for the current source generation and
    /// revisions — the same currency rule `insights_report.rs` applies.
    pub fn hygiene_summary_rows(
        &self,
        environment_key: &str,
        since_epoch: i64,
        excluded_agents: &DisabledAgents,
    ) -> Result<Vec<HygieneSummaryRow>> {
        let excluded = excluded_agents.slugs();
        let exclusion_names = (0..excluded.len())
            .map(|index| format!(":excluded_agent_{index}"))
            .collect::<Vec<_>>();
        let exclusion_predicate = if excluded.is_empty() {
            String::new()
        } else {
            let placeholders = exclusion_names.join(", ");
            format!(" AND s.agent NOT IN ({placeholders})")
        };
        let current_ready = format!(
            "e.status = 'ready' AND {}",
            revision_sql::current_evidence("e", "s")
        );
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT COALESCE(e.status IN ('failed', 'unsupported')
                             OR ({current_ready}), 0),
                    CASE WHEN {current_ready} THEN e.evidence_json END
               FROM session s
               LEFT JOIN session_evidence e
                 ON e.environment_key = s.environment_key
                AND e.agent = s.agent
                AND e.session_id = s.session_id
               WHERE s.environment_key = :environment_key
                 AND COALESCE(s.updated_at_epoch, 0) >= :since_epoch{exclusion_predicate}",
        ))?;
        let mut bindings = named_params![
            ":parser_revision": PARSER_REVISION,
            ":analyzer_revision": ANALYZER_REVISION,
            ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
            ":environment_key": environment_key,
            ":since_epoch": since_epoch,
        ]
        .to_vec();
        bindings.extend(
            exclusion_names
                .iter()
                .zip(excluded)
                .map(|(name, agent)| (name.as_str(), agent as &dyn rusqlite::ToSql)),
        );
        let rows = statement.query_map(bindings.as_slice(), |row| {
            Ok(HygieneSummaryRow {
                settled: row.get::<_, bool>(0)?,
                evidence_json: row.get::<_, Option<String>>(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return every session's full cached record.
    #[cfg(test)]
    pub fn session_records(&self) -> Result<HashMap<SessionActivityKey, SessionRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(SESSION_SELECT_SQL)?;
        let rows = statement.query_map([], session_from_row)?;
        let mut records = HashMap::new();
        for row in rows {
            let record = row?;
            let key = SessionActivityKey::new(
                record.key.environment_key.clone(),
                record.key.agent.clone(),
                record.source_label.clone(),
            );
            records.insert(key, record);
        }
        Ok(records)
    }

    /// Return cached records for the activity sources in one discovery pass.
    ///
    /// Each predicate includes the environment and agent. Native and WSL rows
    /// can therefore share a source label without sharing cached state.
    pub fn session_records_for_activity_keys(
        &self,
        keys: &[SessionActivityKey],
    ) -> Result<HashMap<SessionActivityKey, SessionRecord>> {
        let connection = self.lock();
        let mut records = HashMap::with_capacity(keys.len());
        for keys in keys.chunks(SCAN_HISTORY_KEY_BATCH_SIZE) {
            let mut values = Vec::with_capacity(keys.len() * 3);
            for key in keys {
                values.push(rusqlite::types::Value::Text(key.environment_key.clone()));
                values.push(rusqlite::types::Value::Text(key.agent.clone()));
                values.push(rusqlite::types::Value::Text(key.source_label.clone()));
            }
            let mut statement =
                connection.prepare(&session_records_for_activity_keys_sql(keys.len()))?;
            let rows = statement.query_map(params_from_iter(values.iter()), session_from_row)?;
            for row in rows {
                let record = row?;
                let key = SessionActivityKey::new(
                    record.key.environment_key.clone(),
                    record.key.agent.clone(),
                    record.source_label.clone(),
                );
                records.insert(key, record);
            }
        }
        Ok(records)
    }

    /// Return only session identities for the activity sources of one
    /// watcher burst. The touch report needs keys, not full records, so
    /// this query does not load a row's metadata columns. Each identity
    /// carries its incarnation, and the revision is read with the rows.
    pub fn session_keys_for_activity_keys(
        &self,
        keys: &[SessionActivityKey],
    ) -> Result<(ActivityIdentities, Revision)> {
        let connection = self.lock();
        let mut identities = HashMap::with_capacity(keys.len());
        for keys in keys.chunks(SCAN_HISTORY_KEY_BATCH_SIZE) {
            let mut values = Vec::with_capacity(keys.len() * 3);
            for key in keys {
                values.push(rusqlite::types::Value::Text(key.environment_key.clone()));
                values.push(rusqlite::types::Value::Text(key.agent.clone()));
                values.push(rusqlite::types::Value::Text(key.source_label.clone()));
            }
            let mut statement =
                connection.prepare(&session_keys_for_activity_keys_sql(keys.len()))?;
            let rows = statement.query_map(params_from_iter(values.iter()), |row| {
                let environment_key = row.get::<_, String>(0)?;
                let agent = row.get::<_, String>(1)?;
                let session_id = row.get::<_, String>(2)?;
                let source_label = row.get::<_, String>(3)?;
                let incarnation = Incarnation(row.get::<_, u64>(4)?);
                Ok((
                    SessionActivityKey::new(environment_key.clone(), agent.clone(), source_label),
                    (
                        SessionKey::new(environment_key, agent, session_id),
                        incarnation,
                    ),
                ))
            })?;
            for row in rows {
                let (activity_key, identity) = row?;
                identities.insert(activity_key, identity);
            }
        }
        Ok((identities, revision_of(&connection)))
    }

    /// Return native session identities for one agent title index.
    pub fn native_session_ids_for_agent(&self, agent: &str) -> Result<Vec<String>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT session_id
               FROM session
              WHERE environment_key = 'native' AND agent = ?1
              ORDER BY session_id",
        )?;
        let rows = statement.query_map(params![agent], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return cached records for exact session identities.
    pub fn session_records_for_session_keys(
        &self,
        keys: &[SessionKey],
    ) -> Result<Vec<SessionRecord>> {
        let connection = self.lock();
        let mut records = Vec::with_capacity(keys.len());
        for keys in keys.chunks(SCAN_HISTORY_KEY_BATCH_SIZE) {
            let predicates = (0..keys.len())
                .map(|index| {
                    let first = index * 3 + 1;
                    format!(
                        "(s.environment_key = ?{first} AND s.agent = ?{} AND s.session_id = ?{})",
                        first + 1,
                        first + 2
                    )
                })
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut values = Vec::with_capacity(keys.len() * 3);
            for key in keys {
                values.push(rusqlite::types::Value::Text(key.environment_key.clone()));
                values.push(rusqlite::types::Value::Text(key.agent.clone()));
                values.push(rusqlite::types::Value::Text(key.session_id.clone()));
            }
            let mut statement =
                connection.prepare(&format!("{SESSION_SELECT_SQL}\n WHERE {predicates}"))?;
            let rows = statement.query_map(params_from_iter(values.iter()), session_from_row)?;
            records.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        Ok(records)
    }

    /// One session's cached metadata, when it has been seen.
    pub fn session(&self, key: &SessionKey) -> Result<Option<SessionRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "{SESSION_SELECT_SQL}
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3"
        ))?;
        Ok(statement
            .query_row(
                params![key.environment_key, key.agent, key.session_id],
                session_from_row,
            )
            .optional()?)
    }

    /// Return native file activity keys grouped by their requested transcript
    /// paths. One lock and bounded query chunks cover the whole watcher burst.
    ///
    /// A path can match more than one agent row. Keep every full activity key
    /// so the classifier does not merge agents or native and WSL identities.
    pub fn native_file_session_activity_keys(
        &self,
        source_labels: &BTreeSet<String>,
    ) -> Result<HashMap<String, BTreeSet<SessionActivityKey>>> {
        if source_labels.is_empty() {
            return Ok(HashMap::new());
        }
        let connection = self.lock();
        let labels = source_labels.iter().collect::<Vec<_>>();
        let mut matches: HashMap<String, BTreeSet<SessionActivityKey>> = HashMap::new();
        for chunk in labels.chunks(SOURCE_LABEL_LOOKUP_CHUNK_SIZE) {
            let mut statement =
                connection.prepare(&native_file_session_activity_keys_sql(chunk.len()))?;
            let rows = statement.query_map(
                params_from_iter(chunk.iter().map(|label| label.as_str())),
                |row| {
                    let agent = row.get::<_, String>(0)?;
                    let source_label = row.get::<_, String>(1)?;
                    Ok((
                        source_label.clone(),
                        SessionActivityKey::new("native", agent, source_label),
                    ))
                },
            )?;
            for row in rows {
                let (source_label, key) = row?;
                matches.entry(source_label).or_default().insert(key);
            }
        }
        Ok(matches)
    }

    /// Source labels — native file sessions' own transcript paths — of every
    /// native file session [`crate::analysis::is_active`] still counts as
    /// active as of `now`.
    ///
    /// Takes the lock only for this query; a caller stats the returned paths
    /// outside it.
    pub fn active_native_file_source_labels(&self, now: i64) -> Result<Vec<String>> {
        let connection = self.lock();
        let cutoff = now.saturating_sub(ACTIVE_SESSION_WINDOW_SECS);
        let mut statement = connection.prepare(ACTIVE_NATIVE_FILE_SOURCE_LABELS_SQL)?;
        let rows = statement.query_map(params![cutoff], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return one file session by its complete activity identity.
    pub fn session_record_by_activity_key(
        &self,
        key: &SessionActivityKey,
    ) -> Result<Option<SessionRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "{SESSION_SELECT_SQL}
              WHERE source_label = ?3
                AND environment_key = ?1
                AND agent = ?2
                AND source_kind = 'file'"
        ))?;
        Ok(statement
            .query_row(
                params![key.environment_key, key.agent, key.source_label],
                session_from_row,
            )
            .optional()?)
    }

    /// Keys in `sessions` whose evidence last failed because its source was
    /// missing.
    ///
    /// R3: a full pass reuses an unchanged row without rewriting it, which
    /// would otherwise leave a session stuck once its source returns —
    /// nothing else would notice and re-queue its evidence. The scan pass
    /// persists these rows through `upsert_sessions` even when the row
    /// itself is unchanged, so its own `source_returned` check runs and
    /// clears the failure.
    pub fn sessions_with_missing_source_for(
        &self,
        sessions: &[SessionKey],
    ) -> Result<Vec<SessionKey>> {
        let connection = self.lock();
        let mut missing = Vec::new();
        for sessions in sessions.chunks(SCAN_HISTORY_KEY_BATCH_SIZE) {
            let predicates = (0..sessions.len())
                .map(|index| {
                    let first = index * 3 + 2;
                    format!(
                        "(environment_key = ?{first} AND agent = ?{} AND session_id = ?{})",
                        first + 1,
                        first + 2
                    )
                })
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut values = Vec::with_capacity(sessions.len() * 3 + 1);
            values.push(rusqlite::types::Value::Text(
                crate::insights_worker::EVIDENCE_ERROR_SOURCE_MISSING.to_string(),
            ));
            for key in sessions {
                values.push(rusqlite::types::Value::Text(key.environment_key.clone()));
                values.push(rusqlite::types::Value::Text(key.agent.clone()));
                values.push(rusqlite::types::Value::Text(key.session_id.clone()));
            }
            let mut statement = connection.prepare(&format!(
                "SELECT environment_key, agent, session_id
                   FROM session_evidence
                  WHERE status = 'failed' AND last_error = ?1 AND ({predicates})"
            ))?;
            let rows = statement.query_map(params_from_iter(values.iter()), |row| {
                Ok(SessionKey::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            missing.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        Ok(missing)
    }

    /// One session's persisted source version and optional start time.
    pub fn session_source_state(&self, key: &SessionKey) -> Result<Option<SourceVersionState>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT source_fingerprint, source_generation, started_at_epoch
                   FROM session
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![key.environment_key, key.agent, key.session_id],
                |row| {
                    Ok(SourceVersionState {
                        source_fingerprint: row.get(0)?,
                        source_generation: row.get(1)?,
                        started_at_epoch: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// One session's persisted evidence and queue state.
    pub fn evidence(&self, key: &SessionKey) -> Result<Option<EvidenceRow>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                EVIDENCE_BY_KEY_SQL,
                params![key.environment_key, key.agent, key.session_id],
                evidence_from_row,
            )
            .optional()?)
    }

    /// Persisted evidence for each key, in request order.
    pub fn evidence_batch(&self, keys: &[SessionKey]) -> Result<Vec<Option<EvidenceRow>>> {
        let connection = self.lock();
        let mut statement = connection.prepare(EVIDENCE_BY_KEY_SQL)?;
        keys.iter()
            .map(|key| {
                statement
                    .query_row(
                        params![key.environment_key, key.agent, key.session_id],
                        evidence_from_row,
                    )
                    .optional()
                    .map_err(Into::into)
            })
            .collect()
    }

    /// Each session's current source generation, in request order.
    ///
    /// A caller compares this against an evidence row's own
    /// `analyzed_generation` to find evidence analyzed against a generation
    /// the source has since moved past — see `session_hygiene_payload` in
    /// `commands.rs`. `None` marks a key with no `session` row.
    pub fn source_generation_batch(&self, keys: &[SessionKey]) -> Result<Vec<Option<i64>>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT source_generation FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        )?;
        keys.iter()
            .map(|key| {
                statement
                    .query_row(
                        params![key.environment_key, key.agent, key.session_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(Into::into)
            })
            .collect()
    }

    /// Count the evidence backlog for one environment.
    ///
    /// The counts feed the Insights pane's processing status. They are
    /// scoped to one environment key so they describe the same population
    /// as the report for that scope.
    pub fn evidence_backlog_counts(&self, environment_key: &str) -> Result<EvidenceBacklogCounts> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT status, COUNT(*)
               FROM session_evidence
              WHERE environment_key = ?1 AND status IN ('pending', 'processing')
              GROUP BY status",
        )?;
        let mut rows = statement.query(params![environment_key])?;
        let mut counts = EvidenceBacklogCounts::default();
        while let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let count = u64::try_from(row.get::<_, i64>(1)?)?;
            match status.as_str() {
                "pending" => counts.pending = count,
                "processing" => counts.processing = count,
                _ => {}
            }
        }
        Ok(counts)
    }

    /// Enroll missing evidence rows and requeue stale transcript projections.
    pub fn reconcile_evidence_revisions(
        &self,
        agents: &[&str],
        revisions: ProjectionRevisions,
    ) -> Result<usize> {
        if agents.is_empty() {
            return Ok(0);
        }

        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let agent_names = (0..agents.len())
            .map(|index| format!(":agent_{index}"))
            .collect::<Vec<_>>();
        let agent_placeholders = agent_names.join(", ");
        let agent_bindings = agent_names
            .iter()
            .zip(agents)
            .map(|(name, agent)| (name.as_str(), agent as &dyn rusqlite::ToSql))
            .collect::<Vec<_>>();
        let enroll_sql = format!(
            "INSERT INTO session_evidence (environment_key, agent, session_id)
                 SELECT session.environment_key, session.agent, session.session_id
                   FROM session
                  WHERE session.agent IN ({agent_placeholders})
                    AND NOT EXISTS (
                        SELECT 1 FROM session_evidence
                         WHERE session_evidence.environment_key = session.environment_key
                           AND session_evidence.agent = session.agent
                            AND session_evidence.session_id = session.session_id
                     )
                 RETURNING environment_key, agent"
        );
        let mut enroll_statement = transaction.prepare(&enroll_sql)?;
        let enrolled_scopes = enroll_statement
            .query_map(agent_bindings.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let enrolled = enrolled_scopes.len();
        drop(enroll_statement);

        let evidence_current = revision_sql::current_evidence("evidence", "session");
        let analysis_current = revision_sql::current_analysis("analysis", "session");
        let update_sql = format!(
            "UPDATE session_evidence AS evidence
                SET status = 'pending', last_error = NULL,
                    next_attempt_at_epoch = NULL, retry_count = 0
              WHERE evidence.agent IN ({agent_placeholders})
                AND (
                    evidence.status <> 'pending'
                    OR evidence.last_error IS NOT NULL
                    OR evidence.next_attempt_at_epoch IS NOT NULL
                    OR evidence.retry_count <> 0
                )
                AND EXISTS (
                    SELECT 1 FROM session
                     WHERE session.environment_key = evidence.environment_key
                       AND session.agent = evidence.agent
                       AND session.session_id = evidence.session_id
                       AND (
                             evidence.analyzed_generation IS NOT session.source_generation
                            OR (evidence.status NOT IN ('failed', 'unsupported')
                                AND evidence.processed_fingerprint IS NOT session.source_fingerprint)
                             OR NOT ({evidence_current})
                           OR (evidence.status NOT IN ('failed', 'unsupported')
                               AND NOT EXISTS (
                               SELECT 1 FROM session_analysis AS analysis
                                WHERE analysis.environment_key = session.environment_key
                                  AND analysis.agent = session.agent
                                  AND analysis.session_id = session.session_id
                                   AND {analysis_current}
                           ))
                       )
                 )
             RETURNING environment_key, agent"
        );
        let mut update_bindings = agent_bindings;
        update_bindings.extend(named_params![
            ":parser_revision": revisions.parser_revision,
            ":analyzer_revision": revisions.analyzer_revision,
            ":evidence_schema_revision": revisions.evidence_schema_revision,
            ":metrics_schema_revision": revisions.metrics_schema_revision,
        ]);
        let mut update_statement = transaction.prepare(&update_sql)?;
        let requeued_scopes = update_statement
            .query_map(update_bindings.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let requeued = requeued_scopes.len();
        drop(update_statement);
        transaction.commit()?;
        Ok(enrolled + requeued)
    }

    /// Deletes every `source_resume` snapshot whose revisions do not match
    /// `current`. A parser, analyzer, metrics, evidence, or coverage
    /// revision bump invalidates every persisted snapshot at once — the
    /// stale rows a build with the old revisions left behind are decided at
    /// read time, so a bumped build simply never resumes from them; this
    /// sweeps them out so they do not sit unused. Call this from the same
    /// startup path that calls [`Self::reconcile_evidence_revisions`].
    pub fn purge_stale_source_resume(&self, current: ResumeRevisions) -> Result<usize> {
        let connection = self.lock();
        Ok(delete_stale_source_resume(&connection, &current)?)
    }

    /// Forget all locally stored session data: every session, its analysis, its
    /// relations, and the per-agent scan bookkeeping. Returns how many sessions
    /// were dropped.
    ///
    /// **antiburn's own tables only.** Not one provider file is opened, let
    /// alone written — the agents' source transcripts stay exactly where they
    /// are, which is why a later scan finds all of it again.
    ///
    /// Deliberately spared, because it represents preferences and source
    /// configuration rather than indexed session data:
    ///
    /// - `setting` — the reader's preferences, including whether onboarding is
    ///   done. The provider account HMAC key is attribution data, so it is cleared.
    /// - `scan_root` — the folders the reader pointed the scanner at.
    /// - `repository` — the include/ignore choices the reader made. Their
    ///   session counts *are* derived, so those are zeroed here and refilled by
    ///   the next pass.
    ///
    /// `session_incarnation_seq` is kept: a session discovered again after a
    /// clear must get a higher incarnation than its cleared row.
    /// `evidence_claim_fence_seq` is kept for the same reason: a pass that
    /// started before the clear must not share a fence with a new claim.
    ///
    /// Returns how many sessions it removed and the revision after the
    /// commit. Every session row is absent at that revision.
    pub fn clear_local_session_data(&self) -> Result<(usize, Revision)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM session_relation", [])?;
        tx.execute("DELETE FROM remediation_contribution", [])?;
        tx.execute("DELETE FROM remediation", [])?;
        tx.execute(
            "DELETE FROM setting WHERE key = ?1",
            [usage_accounting::USAGE_LEDGER_KEY],
        )?;
        tx.execute("DELETE FROM burn_check_decision_usage", [])?;
        tx.execute("DELETE FROM session_analysis", [])?;
        tx.execute("DELETE FROM session_evidence", [])?;
        tx.execute("DELETE FROM turn_content", [])?;
        tx.execute("DELETE FROM session_coverage", [])?;
        tx.execute("DELETE FROM source_resume", [])?;
        tx.execute("DELETE FROM turn", [])?;
        // These reference `provider_usage_period` and must go before it, the
        // same way `provider_usage_observation` already does below.
        tx.execute("DELETE FROM provider_limit_residual", [])?;
        tx.execute("DELETE FROM provider_limit_learn_cursor", [])?;
        tx.execute("DELETE FROM provider_limit_factor_sample", [])?;
        tx.execute("DELETE FROM provider_limit_factor_point", [])?;
        tx.execute("DELETE FROM provider_usage_observation", [])?;
        tx.execute("DELETE FROM provider_usage_period", [])?;
        tx.execute("DELETE FROM opencode_v2_capture", [])?;
        tx.execute("DELETE FROM opencode_v2_selection", [])?;
        let sessions = tx.execute("DELETE FROM session", [])?;
        native_tool_results::clear_local_data_in(&tx)?;
        Store::clear_burn_check_session_data_in(&tx)?;
        tx.execute("DELETE FROM provider_account_seen", [])?;
        tx.execute(
            "DELETE FROM setting
              WHERE key IN (?1, 'internal:liveUsageSnapshotV2')",
            params![PROVIDER_ACCOUNT_SECRET_KEY],
        )?;
        tx.execute("DELETE FROM scan_state", [])?;
        tx.execute("UPDATE repository SET session_count = 0", [])?;
        tx.commit()?;
        self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        let mut state = self
            .session_write_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.key_fences.clear();
        state.physical_sources.clear();
        crate::provider_accounts::clear_cache();
        Ok((sessions, revision_of(&connection)))
    }

    /// Delete local content and analysis for one session. Keep native V2 selection
    /// history so retained V1 migration copies stay hidden.
    ///
    /// Local records only. The provider's transcript is never touched — see
    /// [`crate::commands::delete_session_data`].
    ///
    /// Returns the deleted row's incarnation and the revision after the
    /// commit when the row existed, and `None` when there was no row.
    pub fn delete_session(&self, key: &SessionKey) -> Result<Option<(Incarnation, Revision)>> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let removed = delete_session_in(&tx, key)?;
        tx.commit()?;
        self.bump_session_key_fence(key);
        self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        Ok(removed.map(|incarnation| (incarnation, revision_of(&connection))))
    }

    /// Count saved sessions by immutable host identity, without a discovery window.
    pub(crate) fn remote_session_counts(&self) -> Result<HashMap<String, u32>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT substr(environment_key, 5), COUNT(*) FROM session
             WHERE environment_key LIKE 'ssh:%' GROUP BY environment_key",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// Delete every indexed row owned by one immutable remote-host identity.
    pub fn delete_remote_host(&self, host_id: &str) -> Result<(usize, Revision)> {
        anyhow::ensure!(
            !host_id.is_empty() && !host_id.contains(['/', '\\']),
            "invalid remote host ID"
        );
        let environment = format!("ssh:{host_id}");
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let keys = {
            let mut statement =
                tx.prepare("SELECT agent, session_id FROM session WHERE environment_key = ?1")?;
            statement
                .query_map([&environment], |row| {
                    Ok(SessionKey::new(
                        &environment,
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut removed = 0;
        for key in &keys {
            if delete_session_in(&tx, key)?.is_some() {
                removed += 1;
            }
        }
        tx.commit()?;
        for key in &keys {
            self.bump_session_key_fence(key);
        }
        Ok((removed, revision_of(&connection)))
    }

    /* --------------------------------------------------------------------
     * Derived analysis
     * ----------------------------------------------------------------- */

    /// Publish metrics, evidence, and the optional start time as one pass.
    ///
    /// `sources` names how each source in this pass fared — resumed or
    /// read fully — and the resume snapshot (if any) to persist for the
    /// next pass. A source with no entry is treated as
    /// [`SourcePublishMode::Full`] with no resume bookkeeping; a caller
    /// that never claims resume support for any source passes an empty
    /// slice and gets a plain full publish.
    pub fn publish_projections(
        &self,
        record: &AnalysisRecord,
        started_at_epoch: Option<i64>,
        completion: &EvidenceCompletion,
        relations: &[RelationRecord],
        sources: &[SourcePublishOutcome],
    ) -> Result<bool> {
        self.publish_projections_with_source_fingerprint(
            record,
            started_at_epoch,
            completion,
            relations,
            sources,
            Some(record.source_fingerprint.as_str()),
        )
    }

    pub fn publish_projections_with_source_fingerprint(
        &self,
        record: &AnalysisRecord,
        started_at_epoch: Option<i64>,
        completion: &EvidenceCompletion,
        relations: &[RelationRecord],
        sources: &[SourcePublishOutcome],
        source_fingerprint: Option<&str>,
    ) -> Result<bool> {
        self.publish_projections_with_native_tool_results(
            record,
            started_at_epoch,
            completion,
            relations,
            sources,
            ToolResultPublishOptions {
                source_fingerprint,
                batch: None,
            },
        )
    }

    /// Publishes projections and an optional native-result snapshot in the
    /// same claim-fenced transaction.
    ///
    /// Native observations commit only after the evidence claim update wins.
    /// `None` leaves native-result state untouched for existing callers.
    pub fn publish_projections_with_native_tool_results(
        &self,
        record: &AnalysisRecord,
        started_at_epoch: Option<i64>,
        completion: &EvidenceCompletion,
        relations: &[RelationRecord],
        sources: &[SourcePublishOutcome],
        options: ToolResultPublishOptions<'_>,
    ) -> Result<bool> {
        let config_attribution = crate::remediation::publication_config_attribution(
            self,
            &record.key,
            completion.status,
            &completion.evidence_json,
        )?;
        self.publish_projections_with_config_attribution(
            record,
            started_at_epoch,
            completion,
            relations,
            sources,
            options,
            &config_attribution,
        )
    }

    /// Publishes projections with configuration attribution prepared before the write.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_projections_with_config_attribution(
        &self,
        record: &AnalysisRecord,
        started_at_epoch: Option<i64>,
        completion: &EvidenceCompletion,
        relations: &[RelationRecord],
        sources: &[SourcePublishOutcome],
        options: ToolResultPublishOptions<'_>,
        config_attribution: &crate::remediation::PublicationConfigAttribution,
    ) -> Result<bool> {
        let source_sets = publication::SourceSets::new(sources)?;
        let remediation_secret = (completion.status == PublishedEvidence::Ready)
            .then(|| self.provider_account_secret())
            .transpose()?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        // The fence every source's rows will share once this pass
        // publishes: the session's existing `published_fence` when it has
        // one (a resumed source's appended rows join the rows already
        // there), or this pass's own claim fence for a session that has
        // never published. Read before the claim-race UPDATE below so this
        // still names the *pre*-publish value.
        let existing_published_fence = read_published_fence(&transaction, &record.key)?;
        let correction_replay = existing_published_fence.is_some()
            && transaction.query_row(
                "SELECT analyzed_generation FROM session_evidence
                      WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id
                ],
                |row| row.get::<_, Option<i64>>(0),
            )? == Some(record.analyzed_generation);
        let target_fence = existing_published_fence.unwrap_or(completion.claim_fence);
        transaction.execute(
            "INSERT INTO session_analysis (
                 environment_key, agent, session_id, model_breakdown_json,
                 pricing_breakdown_json,
                 inclusive_models_json, initial_context_json, source_summaries_json,
                 provider_hints_json,
                 source_fingerprint, pricing_generation, analyzed_generation,
                 parser_revision, analyzer_revision, metrics_schema_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 model_breakdown_json = excluded.model_breakdown_json,
                 pricing_breakdown_json = excluded.pricing_breakdown_json,
                 inclusive_models_json = excluded.inclusive_models_json,
                 initial_context_json = excluded.initial_context_json,
                 source_summaries_json = excluded.source_summaries_json,
                 provider_hints_json = excluded.provider_hints_json,
                 source_fingerprint = excluded.source_fingerprint,
                 pricing_generation = excluded.pricing_generation,
                 analyzed_generation = excluded.analyzed_generation,
                 parser_revision = excluded.parser_revision,
                 analyzer_revision = excluded.analyzer_revision,
                 metrics_schema_revision = excluded.metrics_schema_revision",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                record.model_breakdown_json,
                record.pricing_breakdown_json,
                record.inclusive_models_json,
                record.initial_context_json,
                record.source_summaries_json,
                record.provider_hints_json,
                record.source_fingerprint,
                record.pricing_generation,
                record.analyzed_generation,
                record.parser_revision,
                record.analyzer_revision,
                record.metrics_schema_revision,
            ],
        )?;
        if let Some(started_at_epoch) = started_at_epoch {
            transaction.execute(
                "UPDATE session SET started_at_epoch = ?4
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id,
                    started_at_epoch,
                ],
            )?;
        }
        let updated = transaction.execute(
            "UPDATE session_evidence AS evidence
                SET status = ?4, analyzed_generation = ?5,
                    processed_fingerprint = ?6, parser_revision = ?7,
                    analyzer_revision = ?8, evidence_schema_revision = ?9,
                    evidence_json = ?10,
                    analyzed_at_epoch = ?11, retry_count = 0, last_error = NULL,
                    claimed_at_epoch = NULL, lease_expires_at_epoch = NULL,
                     next_attempt_at_epoch = NULL, published_fence = ?12,
                     effective_model_target_hash = ?14,
                      effective_model_scope = ?15, effective_model = ?16,
                      effective_reasoning_target_hash = ?17,
                      effective_reasoning_scope = ?18, effective_reasoning = ?19,
                      effective_config_path = ?20, effective_config_selector = ?21,
                      effective_config_precedence_hash = ?22,
                      effective_config_resource_name = NULL, effective_config_value_json = ?23
              WHERE evidence.environment_key = ?1
                AND evidence.agent = ?2 AND evidence.session_id = ?3
                AND evidence.status = 'processing' AND evidence.claim_fence = ?13
                AND EXISTS (
                    SELECT 1 FROM session
                     WHERE session.environment_key = evidence.environment_key
                       AND session.agent = evidence.agent
                        AND session.session_id = evidence.session_id
                        AND session.source_generation = ?5
                        AND session.source_fingerprint IS ?24
                )",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                completion.status.as_str(),
                record.analyzed_generation,
                options.source_fingerprint,
                record.parser_revision,
                record.analyzer_revision,
                completion.evidence_schema_revision,
                completion.evidence_json,
                time::OffsetDateTime::now_utc().unix_timestamp(),
                target_fence,
                completion.claim_fence,
                config_attribution
                    .model
                    .as_ref()
                    .map(|value| value.0.as_str()),
                config_attribution
                    .model
                    .as_ref()
                    .map(|value| value.1.as_str()),
                config_attribution
                    .model
                    .as_ref()
                    .map(|value| value.2.as_str()),
                config_attribution
                    .reasoning
                    .as_ref()
                    .map(|value| value.0.as_str()),
                config_attribution
                    .reasoning
                    .as_ref()
                    .map(|value| value.1.as_str()),
                config_attribution
                    .reasoning
                    .as_ref()
                    .map(|value| value.2.as_str()),
                config_attribution_json(&config_attribution.records, |record| &record.path),
                config_attribution_json(&config_attribution.records, |record| &record.selector),
                config_attribution_json(&config_attribution.records, |record| {
                    &record.precedence_hash
                }),
                config_attribution_values_json(&config_attribution.records),
                options.source_fingerprint,
            ],
        )?;
        if updated == 0 {
            // Roll back this pass's projections. A repeated completion can
            // already own the published fence, so preserve published rows.
            drop(transaction);
            let cleanup = connection.transaction()?;
            evidence_queue::discard_evidence_claim_rows_in(
                &cleanup,
                &record.key,
                completion.claim_fence,
            )?;
            cleanup.commit()?;
            return Ok(false);
        }
        // Stored V2 snapshots cannot prove live event continuity or corrections.
        if let Some(batch) = options.batch.filter(|batch| {
            batch.source_format != antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
        }) {
            let source_format = serde_json::to_string(&batch.source_format)?;
            if batch.complete {
                remove_native_tool_results_from_other_formats_in(
                    &transaction,
                    &record.key,
                    &source_format,
                    record.analyzed_generation,
                    completion.claim_fence,
                )?;
            }
            native_tool_results::reconcile_batch_in(
                &transaction,
                &record.key,
                record.analyzed_generation,
                completion.claim_fence,
                batch,
            )?;
        }
        if completion.status == PublishedEvidence::Ready {
            let publication_epoch = time::OffsetDateTime::now_utc();
            let boundary_ms = i64::try_from(publication_epoch.unix_timestamp_nanos() / 1_000_000)
                .unwrap_or(i64::MAX);
            remediation::activate_waiting_prompt_remediations_in(
                &transaction,
                &record.key.environment_key,
                &record.key.agent,
                &record.key.session_id,
                completion.claim_fence,
                boundary_ms,
                publication_epoch.unix_timestamp(),
            )?;
        }
        let key = turn_session_key(&record.key);
        publication::publish_turn_rows(
            &transaction,
            &key,
            completion.claim_fence,
            target_fence,
            &source_sets,
        )?;
        // Each named source replaces its stored resume snapshot. The
        // set-based publication above already removes snapshots for sources
        // that vanished from this pass.
        for source in sources {
            match &source.resume {
                Some(stored) => {
                    insert_source_resume(&transaction, &key, &source.source_key, stored)?
                }
                None => delete_source_resume(&transaction, &key, &source.source_key)?,
            }
        }
        // The coverage record this pass wrote under the claim fence (R3
        // rebuilds it every pass, resumed or full) becomes the published
        // record, replacing whatever was there before.
        if let Some(coverage_record) = query_coverage_record(
            &transaction,
            &key,
            &FenceScope::single(completion.claim_fence),
        )? {
            insert_coverage_record(&transaction, &key, target_fence, &coverage_record)?;
        }
        // Every row and coverage record from an earlier, superseded fence
        // is dropped now that this pass's evidence is what published. Every
        // source's rows above were already re-stamped onto `target_fence`,
        // so on a resumed pass this finds nothing left to delete.
        delete_turn_rows_except_fence(&transaction, &key, target_fence)?;
        replace_relations_in(&transaction, &record.key, RelationKind::Subagent, relations)?;
        if completion.status == PublishedEvidence::Ready {
            let publication_epoch = time::OffsetDateTime::now_utc();
            let boundary_ms = i64::try_from(publication_epoch.unix_timestamp_nanos() / 1_000_000)
                .unwrap_or(i64::MAX);
            remediation::mark_remediations_dirty_in(
                &transaction,
                &record.key.environment_key,
                &record.key.agent,
                publication_epoch.unix_timestamp(),
                correction_replay,
            )?;
            let enabled_detectors = antiburn_local::insights::DetectorSelection::from_enabled(
                enabled_checks_in(&transaction)?,
            );
            let findings = crate::insights_report::publication_findings_in(
                &transaction,
                &record.key,
                &enabled_detectors,
            )?;
            let candidates = crate::remediation::passive_remediations(
                &transaction,
                remediation_secret
                    .as_ref()
                    .expect("ready publication has a remediation secret"),
                findings,
                boundary_ms,
            )?;
            remediation::enroll_passive_remediations_in(
                &transaction,
                &candidates,
                publication_epoch.unix_timestamp(),
            )?;
        }
        transaction.commit()?;
        if options.batch.is_some_and(|batch| {
            batch.source_format == antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
        }) {
            ::tracing::debug!(
                event = "stored_metrics_committed",
                publication_origin = "stored_metrics",
                live_evidence_ownership = "excluded",
                metrics_claim_fence = completion.claim_fence,
                metrics_source_generation = record.analyzed_generation,
            );
        }
        Ok(true)
    }

    /// Requeues one session's evidence row for the durable worker.
    ///
    /// Wraps [`mark_evidence_pending_in`] in its own connection lock, for a
    /// caller outside a transaction this module already holds open — the
    /// drilldown command switch nudges the worker this way when it finds
    /// the stored analysis fingerprint no longer matches the live
    /// transcript's. Idempotent: requeuing a session already `pending` (or
    /// already claimed) just clears its retry state again.
    pub fn requeue_session_evidence(&self, key: &SessionKey) -> Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        mark_evidence_pending_in(&transaction, key)?;
        transaction.commit()?;
        Ok(())
    }

    /// Counts one session's turn rows stamped with `claim_fence`.
    pub fn count_turn_rows_for_session(&self, key: &SessionKey, claim_fence: i64) -> Result<u64> {
        let connection = self.lock();
        Ok(count_turn_rows(
            &connection,
            &turn_session_key(key),
            claim_fence,
        )?)
    }

    /// The model one session's newest published turn ran, or `None` when
    /// this session has never published a turn with a model.
    ///
    /// This reads at `published_fence`, for the reason
    /// [`Self::published_turn_rows`] gives: a claim in flight writes rows
    /// under its own fence, and only a complete publish moves
    /// `published_fence`. The evidence lookup and the turn query run under
    /// one lock, so a concurrent claim cannot swap the fence in between.
    #[cfg(test)]
    pub fn latest_session_model(&self, key: &SessionKey) -> Result<Option<String>> {
        Ok(self
            .published_models_for_keys(std::slice::from_ref(key))?
            .0
            .into_iter()
            .next()
            .and_then(|row| row.model))
    }

    /// Read compact published model evidence and its revision under one writer lock.
    pub(crate) fn published_models_for_keys(
        &self,
        keys: &[SessionKey],
    ) -> Result<(Vec<PublishedModel>, Revision)> {
        anyhow::ensure!(keys.len() <= 256, "model page exceeds 256 identities");
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT s.incarnation, e.published_fence FROM session s
             LEFT JOIN session_evidence e USING (environment_key, agent, session_id)
             WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3",
        )?;
        let mut rows = Vec::with_capacity(keys.len());
        for key in keys {
            let identity = statement
                .query_row(
                    params![key.environment_key, key.agent, key.session_id],
                    |row| {
                        Ok((
                            Incarnation(row.get::<_, u64>(0)?),
                            row.get::<_, Option<i64>>(1)?,
                        ))
                    },
                )
                .optional()?;
            let Some((incarnation, published_fence)) = identity else {
                continue;
            };
            let execution = match published_fence {
                Some(fence) => latest_turn_execution(&connection, &turn_session_key(key), fence)?,
                None => None,
            };
            rows.push(PublishedModel {
                key: key.clone(),
                incarnation,
                published_fence,
                provider: execution.as_ref().and_then(|value| value.provider.clone()),
                model: execution.map(|value| value.model),
            });
        }
        Ok((rows, revision_of(&connection)))
    }

    /// One session's last published turn rows, or `None` when this session
    /// has never published.
    ///
    /// A claim bumps `session_evidence.claim_fence` and writes new rows
    /// under that fence while the last published pass's rows still sit
    /// under `published_fence`. Only a winning publish moves
    /// `published_fence`, and it always moves it to a complete row set —
    /// see `EvidenceRow::published_fence`'s doc comment and the `v21`
    /// migration in `store::schema` for the full contract. So a claim in
    /// flight, a requeue back to `pending`, or a run that later fails
    /// changes `status` and `claim_fence` but never touches
    /// `published_fence`, and this method keeps serving the same rows it
    /// served before that started. The evidence lookup and the row query
    /// run under one lock, so a concurrent claim cannot swap
    /// `published_fence` in between them.
    ///
    /// This reads at `published_fence` alone, with no `FenceScope`
    /// widening: it wants only what the last publish left behind, not the
    /// in-flight pass's own claim fence. [`FencedTurnRowStore`] is the one
    /// that widens, and only for a mid-pass read of a source that resumed
    /// — see the `v30` migration comment in `store::schema`.
    pub fn published_turn_rows(&self, key: &SessionKey) -> Result<Option<Vec<TurnRow>>> {
        let connection = self.lock();
        published_turn_rows_in(&connection, key)
    }

    /// Reads rows, analysis, and relations without mixing concurrent publications.
    pub(crate) fn published_replay(&self, key: &SessionKey) -> Result<Option<PublishedReplay>> {
        let connection = self.lock();
        let Some(rows) = published_turn_rows_in(&connection, key)? else {
            return Ok(None);
        };
        let Some(record) = analysis_in(&connection, key)? else {
            return Ok(None);
        };
        let relations = relations_in(&connection, key)?;
        Ok(Some(PublishedReplay {
            rows,
            record,
            relations,
        }))
    }

    /// Reads bounded private content from a publication that matches the
    /// current source generation and parser revision. The evidence lookup,
    /// freshness check, and content query share one lock.
    pub fn published_turn_content(&self, key: &SessionKey) -> Result<Option<PublishedContent>> {
        self.published_turn_content_after(key, None, &Default::default())
    }

    pub fn published_turn_content_after(
        &self,
        key: &SessionKey,
        after_ms: Option<i64>,
        positions: &std::collections::BTreeMap<String, u64>,
    ) -> Result<Option<PublishedContent>> {
        self.published_turn_content_page(key, after_ms, positions, 0)
    }

    pub fn published_turn_content_page(
        &self,
        key: &SessionKey,
        after_ms: Option<i64>,
        positions: &std::collections::BTreeMap<String, u64>,
        page: usize,
    ) -> Result<Option<PublishedContent>> {
        self.published_turn_content_offset(
            key,
            after_ms,
            positions,
            page.saturating_mul(antiburn_local::analysis::MAX_CONTENT_QUERY_PARTS),
        )
    }

    pub fn published_turn_content_offset(
        &self,
        key: &SessionKey,
        after_ms: Option<i64>,
        positions: &std::collections::BTreeMap<String, u64>,
        offset: usize,
    ) -> Result<Option<PublishedContent>> {
        self.published_turn_content_offset_selected(
            key,
            after_ms,
            positions,
            offset,
            antiburn_local::analysis::JevInputSelection::ALL,
        )
    }

    pub fn published_turn_content_offset_selected(
        &self,
        key: &SessionKey,
        after_ms: Option<i64>,
        positions: &std::collections::BTreeMap<String, u64>,
        offset: usize,
        selection: antiburn_local::analysis::JevInputSelection,
    ) -> Result<Option<PublishedContent>> {
        let connection = self.lock();
        let Some(evidence) = connection
            .query_row(
                EVIDENCE_BY_KEY_SQL,
                params![key.environment_key, key.agent, key.session_id],
                evidence_from_row,
            )
            .optional()?
        else {
            return Ok(None);
        };
        let Some(published_fence) = evidence.published_fence else {
            return Ok(None);
        };
        let current: Option<(i64, Option<String>)> = connection
            .query_row(
                "SELECT source_generation, source_fingerprint FROM session
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![key.environment_key, key.agent, key.session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((source_generation, source_fingerprint)) = current else {
            return Ok(None);
        };
        if evidence.status.as_str() != "ready"
            || evidence.analyzed_generation != Some(source_generation)
            || evidence.processed_fingerprint != source_fingerprint
            || evidence.parser_revision != Some(PARSER_REVISION)
            || evidence.analyzer_revision != Some(ANALYZER_REVISION)
            || evidence.evidence_schema_revision != Some(EVIDENCE_SCHEMA_REVISION)
        {
            return Ok(None);
        }
        let mut content = query_turn_content_offset_selected(
            &connection,
            &turn_session_key(key),
            &FenceScope::single(published_fence),
            after_ms,
            positions,
            offset,
            selection,
        )?;
        content.source_generation = Some(source_generation);
        Ok(Some(content))
    }

    /// One session's last published [`SessionCoverageRecord`], or `None`
    /// when this session has never published.
    ///
    /// Mirrors [`Self::published_turn_rows`]'s contract exactly, over
    /// `session_coverage` instead of `turn`: `published_fence` names a
    /// complete coverage record for the same reason it names a complete row
    /// set, and the evidence lookup and the record query run under one
    /// lock for the same race-free guarantee.
    pub fn published_coverage_record(
        &self,
        key: &SessionKey,
    ) -> Result<Option<SessionCoverageRecord>> {
        let connection = self.lock();
        let Some(evidence) = connection
            .query_row(
                EVIDENCE_BY_KEY_SQL,
                params![key.environment_key, key.agent, key.session_id],
                evidence_from_row,
            )
            .optional()?
        else {
            return Ok(None);
        };
        let Some(published_fence) = evidence.published_fence else {
            return Ok(None);
        };
        Ok(query_coverage_record(
            &connection,
            &turn_session_key(key),
            &FenceScope::single(published_fence),
        )?)
    }

    /// Write one session's analysis columns directly, without the evidence
    /// claim `publish_projections` requires.
    ///
    /// Test scaffolding only. Every production write goes through
    /// `publish_projections`, which also settles the session's evidence
    /// claim; this method exists so fixture setup can seed
    /// `session_analysis` without first driving a claim through the worker.
    #[cfg(test)]
    pub fn save_analysis(
        &self,
        record: &AnalysisRecord,
        started_at_epoch: Option<i64>,
    ) -> Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO session_analysis (
                 environment_key, agent, session_id, model_breakdown_json,
                 pricing_breakdown_json,
                 inclusive_models_json, initial_context_json, source_summaries_json,
                 provider_hints_json,
                 source_fingerprint, pricing_generation, analyzed_generation,
                 parser_revision, analyzer_revision, metrics_schema_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 model_breakdown_json = excluded.model_breakdown_json,
                 pricing_breakdown_json = excluded.pricing_breakdown_json,
                 inclusive_models_json = excluded.inclusive_models_json,
                 initial_context_json = excluded.initial_context_json,
                 source_summaries_json = excluded.source_summaries_json,
                 provider_hints_json = excluded.provider_hints_json,
                 source_fingerprint = excluded.source_fingerprint,
                 pricing_generation = excluded.pricing_generation,
                 analyzed_generation = excluded.analyzed_generation,
                 parser_revision = excluded.parser_revision,
                 analyzer_revision = excluded.analyzer_revision,
                 metrics_schema_revision = excluded.metrics_schema_revision",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                record.model_breakdown_json,
                record.pricing_breakdown_json,
                record.inclusive_models_json,
                record.initial_context_json,
                record.source_summaries_json,
                record.provider_hints_json,
                record.source_fingerprint,
                record.pricing_generation,
                record.analyzed_generation,
                record.parser_revision,
                record.analyzer_revision,
                record.metrics_schema_revision,
            ],
        )?;
        if let Some(started_at_epoch) = started_at_epoch {
            transaction.execute(
                "UPDATE session SET started_at_epoch = ?4
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id,
                    started_at_epoch,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Every listed session's cached analysis, in one query rather than one
    /// per session. A session with no analysis row is absent from the map.
    pub fn analyses(&self, keys: &[SessionKey]) -> Result<HashMap<SessionKey, AnalysisRecord>> {
        let mut analyses = HashMap::new();
        if keys.is_empty() {
            return Ok(analyses);
        }
        let mut clauses = Vec::with_capacity(keys.len());
        let mut values = Vec::with_capacity(keys.len() * 3);
        for key in keys.iter().take(500) {
            clauses.push("(environment_key = ? AND agent = ? AND session_id = ?)");
            values.push(rusqlite::types::Value::from(key.environment_key.clone()));
            values.push(rusqlite::types::Value::from(key.agent.clone()));
            values.push(rusqlite::types::Value::from(key.session_id.clone()));
        }
        let sql = format!(
            "SELECT environment_key, agent, session_id, model_breakdown_json,
                    pricing_breakdown_json,
                    inclusive_models_json, initial_context_json, source_summaries_json,
                    provider_hints_json,
                    source_fingerprint, pricing_generation, analyzed_generation,
                    parser_revision, analyzer_revision, metrics_schema_revision
               FROM session_analysis
              WHERE {}",
            clauses.join(" OR ")
        );
        let connection = self.lock();
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        while let Some(row) = rows.next()? {
            let record = AnalysisRecord {
                key: SessionKey::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ),
                model_breakdown_json: row.get(3)?,
                pricing_breakdown_json: row.get(4)?,
                inclusive_models_json: row.get(5)?,
                initial_context_json: row.get(6)?,
                source_summaries_json: row.get(7)?,
                provider_hints_json: row.get(8)?,
                source_fingerprint: row.get(9)?,
                pricing_generation: row.get(10)?,
                analyzed_generation: row.get(11)?,
                parser_revision: row.get(12)?,
                analyzer_revision: row.get(13)?,
                metrics_schema_revision: row.get(14)?,
            };
            analyses.insert(record.key.clone(), record);
        }
        Ok(analyses)
    }

    /// One session's cached analysis, when it has been computed.
    pub fn analysis(&self, key: &SessionKey) -> Result<Option<AnalysisRecord>> {
        let connection = self.lock();
        analysis_in(&connection, key)
    }

    /// Agent, activity timestamp, token breakdown, and provider hints for every
    /// session at or after `since_epoch`.
    ///
    /// One join rather than a listing plus a lookup per row: provider usage
    /// walks every retained session, and the N+1 shape would take the
    /// connection lock once per session.
    ///
    /// A session with no analysis row still comes back, with `None` for its
    /// breakdown — the aggregation counts it as an unattributed session rather
    /// than pretending it spent nothing.
    pub fn usage_evidence(&self, since_epoch: i64) -> Result<Vec<UsageEvidenceRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT s.agent, COALESCE(s.updated_at_epoch, 0), a.model_breakdown_json,
                    a.pricing_breakdown_json, a.provider_hints_json,
                    COALESCE((
                        SELECT json_group_array(json_object(
                            'provider', spa.provider,
                            'accountKey', spa.account_key
                        ))
                          FROM session_provider_account spa
                         WHERE spa.environment_key = s.environment_key
                           AND spa.agent = s.agent
                           AND spa.session_id = s.session_id
                    ), '[]')
               FROM session s
               LEFT JOIN session_analysis a
                 ON a.environment_key = s.environment_key
                AND a.agent = s.agent
                AND a.session_id = s.session_id
              WHERE COALESCE(s.updated_at_epoch, 0) >= ?1
                AND s.environment_key NOT LIKE 'ssh:%'
              ORDER BY COALESCE(s.updated_at_epoch, 0) DESC",
        )?;
        let rows = statement.query_map(params![since_epoch], |row| {
            Ok(UsageEvidenceRecord {
                agent: row.get(0)?,
                updated_at_epoch: row.get(1)?,
                model_breakdown_json: row.get(2)?,
                pricing_breakdown_json: row.get(3)?,
                provider_hints_json: row.get(4)?,
                provider_accounts_json: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Bind recent sessions after the account-attribution rollout.
    pub fn observe_provider_account(
        &self,
        agent: &str,
        provider: &str,
        account_key: &str,
        observed_at_epoch: i64,
        provenance: &str,
    ) -> Result<()> {
        if account_key.len() != 64
            || provider.is_empty()
            || agent.is_empty()
            || !matches!(provenance, "provider_live" | "tool_oauth")
        {
            anyhow::bail!("invalid provider account observation");
        }
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let updated = tx.execute(
            "UPDATE provider_account_seen
                SET last_seen_epoch = MAX(last_seen_epoch, ?4)
              WHERE agent = ?1 AND provider = ?2 AND account_key = ?3",
            params![agent, provider, account_key, observed_at_epoch],
        )?;
        let inserted = if updated == 0 {
            tx.execute(
                "INSERT OR IGNORE INTO provider_account_seen (
                agent, provider, account_key, first_seen_epoch, last_seen_epoch
             )
             SELECT ?1, ?2, ?3, ?4, ?4
               WHERE (SELECT COUNT(*) FROM provider_account_seen
                       WHERE agent = ?1 AND provider = ?2) < 32",
                params![agent, provider, account_key, observed_at_epoch],
            )?
        } else {
            0
        };
        if updated == 0
            && inserted == 0
            && !tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM provider_account_seen
                     WHERE agent = ?1 AND provider = ?2 AND account_key = ?3
                 )",
                params![agent, provider, account_key],
                |row| row.get::<_, bool>(0),
            )?
        {
            tx.commit()?;
            return Ok(());
        }
        let rollout: i64 = tx
            .query_row(
                "SELECT value FROM setting
                  WHERE key = 'internal:providerAccountRolloutV1'",
                [],
                |row| row.get::<_, String>(0),
            )?
            .parse()
            .context("invalid provider account rollout")?;
        tx.execute(
            "INSERT OR IGNORE INTO session_provider_account (
                environment_key, agent, session_id, provider, account_key,
                provenance, confidence, first_seen_at
             )
             SELECT environment_key, agent, session_id, ?2, ?3, ?7, 'direct', ?4
              FROM session
              WHERE agent = ?1
                AND environment_key NOT LIKE 'ssh:%'
                AND unixepoch(first_seen_at) >= ?5
                AND COALESCE(updated_at_epoch, 0) BETWEEN MAX(?5, ?6 - 600) AND ?6
                AND COALESCE(updated_at_epoch, 0) > COALESCE((
                    SELECT MAX(seen.last_seen_epoch)
                      FROM provider_account_seen AS seen
                     WHERE seen.agent = ?1
                       AND seen.provider = ?2
                       AND seen.account_key <> ?3
                       AND seen.last_seen_epoch <= ?6
                ), ?5 - 1)
                AND (SELECT COUNT(*) FROM session_provider_account spa
                      WHERE spa.environment_key = session.environment_key
                        AND spa.agent = session.agent
                        AND spa.session_id = session.session_id
                        AND spa.provider = ?2) < 8",
            params![
                agent,
                provider,
                account_key,
                now_rfc3339(),
                rollout,
                observed_at_epoch,
                provenance
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /* --------------------------------------------------------------------
     * Relations
     * ----------------------------------------------------------------- */

    /// Replace every relation of one kind for a session.
    pub fn replace_relations(
        &self,
        key: &SessionKey,
        kind: RelationKind,
        relations: &[RelationRecord],
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        replace_relations_in(&tx, key, kind, relations)?;
        tx.commit()?;
        Ok(())
    }

    /// Every relation recorded for a session.
    pub fn relations(&self, key: &SessionKey) -> Result<Vec<RelationRecord>> {
        let connection = self.lock();
        relations_in(&connection, key)
    }

    /// The session id this session recorded as its own fork parent, when
    /// describe found one. Describe writes this once, from the transcript's
    /// own bounded preview — see `upsert_session_in`'s `forkParent` insert —
    /// so a reader never re-derives it from disk.
    pub fn fork_parent(&self, key: &SessionKey) -> Result<Option<String>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT related_id FROM session_relation
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND kind = 'forkParent'
                  LIMIT 1",
                params![key.environment_key, key.agent, key.session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    }

    /// The sessions that recorded `session_id` as their fork parent, within the
    /// same environment and agent.
    pub fn fork_children(&self, key: &SessionKey) -> Result<Vec<String>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT session_id FROM session_relation
              WHERE environment_key = ?1 AND agent = ?2 AND kind = 'forkParent' AND related_id = ?3
              ORDER BY session_id",
        )?;
        let rows = statement.query_map(
            params![key.environment_key, key.agent, key.session_id],
            |row| row.get::<_, String>(0),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Sessions that already hold a published turn row for one of `uuids`,
    /// within `key`'s own environment and agent, excluding `key` itself.
    ///
    /// Serves the Claude fork-lineage lookup: a "resume as fork" session
    /// copies its parent's leading turn records byte for byte, keeping
    /// their `uuid`. A session this finds owning one of them is a
    /// fork-parent candidate.
    ///
    /// `uuids` is capped at [`FORK_LINEAGE_UUID_CAP`] entries — the
    /// caller's own extraction stays far under this, and the cap only
    /// guards the `IN (...)` list against an oversized call.
    pub fn sessions_owning_turn_uuids(
        &self,
        key: &SessionKey,
        uuids: &[String],
    ) -> Result<Vec<OwningSession>> {
        let uuids = &uuids[..uuids.len().min(FORK_LINEAGE_UUID_CAP)];
        if uuids.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.lock();
        let sql = sessions_owning_turn_uuids_sql(uuids.len());
        let mut statement = connection.prepare(&sql)?;
        let mut values: Vec<rusqlite::types::Value> = vec![
            rusqlite::types::Value::Text(key.environment_key.clone()),
            rusqlite::types::Value::Text(key.agent.clone()),
            rusqlite::types::Value::Text(key.session_id.clone()),
        ];
        values.extend(uuids.iter().cloned().map(rusqlite::types::Value::Text));
        values.push(rusqlite::types::Value::Text(key.environment_key.clone()));
        values.push(rusqlite::types::Value::Text(key.agent.clone()));
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(OwningSession {
                session_id: row.get(0)?,
                first_seen_at: row.get(1)?,
                published_turn_rows: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The uuid of `key`'s own earliest published turn row, scope `main`.
    ///
    /// This is the uuid a Claude "resume as fork" session's own leading
    /// record carries, since a fork copies its parent's early records
    /// byte for byte. `None` when the session has no published turn row
    /// with a uuid yet — a describe pass has not published one, or the
    /// source assigns no uuids at all.
    pub fn first_turn_uuid(&self, key: &SessionKey) -> Result<Option<String>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT t.uuid
                   FROM turn t
                   JOIN session_evidence e
                     ON e.environment_key = t.environment_key
                    AND e.agent = t.agent
                    AND e.session_id = t.session_id
                    AND e.published_fence = t.claim_fence
                  WHERE t.environment_key = ?1
                    AND t.agent = ?2
                    AND t.session_id = ?3
                    AND t.scope = 'main'
                    AND t.uuid IS NOT NULL
                  ORDER BY t.turn_index ASC
                  LIMIT 1",
                params![key.environment_key, key.agent, key.session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    }

    /// `key`'s own `first_seen_at` and published turn row count, in the
    /// same shape [`Self::sessions_owning_turn_uuids`] returns for a
    /// candidate — so a caller can rank itself against those candidates
    /// with the same comparison. `None` when `key` has no session row.
    pub fn owning_session_stats(&self, key: &SessionKey) -> Result<Option<OwningSession>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                &format!(
                    "SELECT s.session_id, s.first_seen_at,
                            {PUBLISHED_TURN_ROW_COUNT_SQL} AS published_turn_rows
                       FROM session s
                      WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3"
                ),
                params![key.environment_key, key.agent, key.session_id],
                |row| {
                    Ok(OwningSession {
                        session_id: row.get(0)?,
                        first_seen_at: row.get(1)?,
                        published_turn_rows: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Record `parent` as `key`'s fork parent. Returns whether this call
    /// inserted the row.
    ///
    /// This does not check whether `key` already has a different parent —
    /// see [`insert_fork_parent_in`]. A caller that must keep one parent
    /// per session checks first with [`Self::fork_parent`].
    pub fn record_fork_parent(&self, key: &SessionKey, parent: &str) -> Result<bool> {
        let connection = self.lock();
        insert_fork_parent_in(&connection, key, parent)
    }

    /// Test scaffolding only. Production code sets `first_seen_at` once, on
    /// a session's first insert, and never updates it again. A fork-lineage
    /// ranking test uses this to force two sessions to tie, or to order,
    /// on `first_seen_at` without depending on wall-clock timing.
    #[cfg(test)]
    pub fn set_first_seen_at_for_test(&self, key: &SessionKey, first_seen_at: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE session SET first_seen_at = ?1
              WHERE environment_key = ?2 AND agent = ?3 AND session_id = ?4",
            params![
                first_seen_at,
                key.environment_key,
                key.agent,
                key.session_id
            ],
        )?;
        Ok(())
    }

    /* --------------------------------------------------------------------
     * Scan state
     * ----------------------------------------------------------------- */

    /// Record what one agent's pass of a scan saw.
    pub fn record_agent_scan(
        &self,
        agent: &str,
        cursor_epoch: Option<i64>,
        seen: i64,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO scan_state (agent, last_completed_at, cursor_epoch, sessions_seen)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(agent) DO UPDATE SET
                 last_completed_at = excluded.last_completed_at,
                 cursor_epoch = MAX(COALESCE(scan_state.cursor_epoch, 0),
                                    COALESCE(excluded.cursor_epoch, 0)),
                 sessions_seen = excluded.sessions_seen",
            params![agent, now_rfc3339(), cursor_epoch, seen],
        )?;
        Ok(())
    }

    /// When each agent was last scanned, and what it saw.
    pub fn scan_state(&self) -> Result<Vec<(String, Option<String>, i64)>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT agent, last_completed_at, sessions_seen FROM scan_state ORDER BY agent",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /* --------------------------------------------------------------------
     * Repositories
     * ----------------------------------------------------------------- */

    /// Remove excluded sessions in one transaction. A failed purge can be retried.
    pub fn purge_ignored_sessions(&self) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let ignored = ignored_repository_paths_in(&tx)?;
        let keys = {
            let mut statement = tx.prepare(
                "SELECT environment_key, agent, session_id, cwd FROM session WHERE cwd IS NOT NULL",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ),
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut keys = Vec::new();
            for row in rows {
                let (key, cwd) = row?;
                if key.remote_host_id().is_none()
                    && antiburn_local::paths::ignored_paths::set_contains(&ignored, &cwd)
                {
                    keys.push(key);
                }
            }
            keys
        };
        for key in &keys {
            delete_session_in(&tx, key)?;
        }
        tx.commit()?;
        for key in &keys {
            self.bump_session_key_fence(key);
        }
        if !ignored.is_empty() {
            self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        }
        Ok(keys.len())
    }

    /// Update counts without replacing repository paths or include choices.
    pub fn update_repository_session_counts(&self, counts: &[(String, u32)]) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        for (key, count) in counts {
            tx.execute(
                "UPDATE repository SET session_count = ?2 WHERE key = ?1",
                params![key, count],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Refresh the located repositories, preserving each one's include choice.
    ///
    /// A repository the user turned off stays off across rescans: the incoming
    /// records describe what is on disk, never what the user decided.
    pub fn replace_repositories(&self, records: &[RepositoryRecord]) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let generation = read_repository_policy_generation(&tx)?;
        replace_repositories_in(&tx, records)?;
        let policy_changed = read_repository_policy_generation(&tx)? != generation;
        tx.commit()?;
        if policy_changed {
            self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    /// Publish repository and deferred-folder results only if the policy used
    /// to build them is still current. Both result sets share one commit.
    pub fn publish_repository_refresh(
        &self,
        expected_generation: u64,
        records: &[RepositoryRecord],
        deferred: &[DeferredPermissionDir],
    ) -> Result<RepositoryPolicyPublishOutcome> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        if read_repository_policy_generation(&tx)? != expected_generation {
            return Ok(RepositoryPolicyPublishOutcome::Superseded);
        }
        replace_repositories_in(&tx, records)?;
        let deferred_json = serde_json::to_string(deferred)?;
        let previous_deferred: Option<String> = tx
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                params![DEFERRED_PERMISSION_DIRS_KEY],
                |row| row.get(0),
            )
            .optional()?;
        if previous_deferred.as_deref() != Some(&deferred_json) {
            bump_repository_policy_generation_in(&tx)?;
        }
        tx.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![DEFERRED_PERMISSION_DIRS_KEY, deferred_json],
        )?;
        let policy_changed = read_repository_policy_generation(&tx)? != expected_generation;
        tx.commit()?;
        if policy_changed {
            self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        }
        Ok(RepositoryPolicyPublishOutcome::Published)
    }

    /// Every known repository, by owner-qualified name.
    pub fn repositories(&self) -> Result<Vec<RepositoryRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT key, repo_name, full_name, status, repo_root, suspected_path,
                    worktree_count, session_count, wsl_distro, enabled
               FROM repository ORDER BY full_name, key",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(RepositoryRecord {
                key: row.get(0)?,
                repo_name: row.get(1)?,
                full_name: row.get(2)?,
                status: row.get(3)?,
                repo_root: row.get(4)?,
                suspected_path: row.get(5)?,
                worktree_count: row.get(6)?,
                session_count: row.get(7)?,
                wsl_distro: row.get(8)?,
                enabled: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Include or ignore one repository.
    pub fn set_repository_enabled(&self, key: &str, enabled: bool) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM repository WHERE key = ?1)",
            params![key],
            |row| row.get(0),
        )?;
        if !exists {
            tx.commit()?;
            return Ok(false);
        }
        let changed = tx.execute(
            "UPDATE repository SET enabled = ?2 WHERE key = ?1 AND enabled <> ?2",
            params![key, enabled],
        )?;
        if changed > 0 {
            bump_repository_policy_generation_in(&tx)?;
        }
        tx.commit()?;
        if changed > 0 {
            self.destructive_write_epoch.fetch_add(1, Ordering::Release);
        }
        Ok(true)
    }
}

fn published_turn_rows_in(
    connection: &Connection,
    key: &SessionKey,
) -> Result<Option<Vec<TurnRow>>> {
    let Some(published_fence) = read_published_fence(connection, key)? else {
        return Ok(None);
    };
    Ok(Some(query_turn_rows(
        connection,
        &turn_session_key(key),
        &FenceScope::single(published_fence),
    )?))
}

fn analysis_in(connection: &Connection, key: &SessionKey) -> Result<Option<AnalysisRecord>> {
    let mut statement = connection.prepare(
        "SELECT environment_key, agent, session_id, model_breakdown_json,
                pricing_breakdown_json,
                inclusive_models_json, initial_context_json, source_summaries_json,
                provider_hints_json,
                source_fingerprint, pricing_generation, analyzed_generation,
                parser_revision, analyzer_revision, metrics_schema_revision
           FROM session_analysis
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
    )?;
    Ok(statement
        .query_row(
            params![key.environment_key, key.agent, key.session_id],
            |row| {
                Ok(AnalysisRecord {
                    key: SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ),
                    model_breakdown_json: row.get(3)?,
                    pricing_breakdown_json: row.get(4)?,
                    inclusive_models_json: row.get(5)?,
                    initial_context_json: row.get(6)?,
                    source_summaries_json: row.get(7)?,
                    provider_hints_json: row.get(8)?,
                    source_fingerprint: row.get(9)?,
                    pricing_generation: row.get(10)?,
                    analyzed_generation: row.get(11)?,
                    parser_revision: row.get(12)?,
                    analyzer_revision: row.get(13)?,
                    metrics_schema_revision: row.get(14)?,
                })
            },
        )
        .optional()?)
}

fn relations_in(connection: &Connection, key: &SessionKey) -> Result<Vec<RelationRecord>> {
    let mut statement = connection.prepare(
        "SELECT kind, related_id, label FROM session_relation
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
          ORDER BY kind, related_id",
    )?;
    let rows = statement.query_map(
        params![key.environment_key, key.agent, key.session_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    let mut relations = Vec::new();
    for row in rows {
        let (kind, related_id, label) = row?;
        if let Some(kind) = RelationKind::parse(&kind) {
            relations.push(RelationRecord {
                kind,
                related_id,
                label,
            });
        }
    }
    Ok(relations)
}

fn read_repository_policy_generation(connection: &Connection) -> Result<u64> {
    let value = connection
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            params![REPOSITORY_POLICY_GENERATION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    Ok(value)
}

fn bump_repository_policy_generation_in(connection: &Connection) -> Result<()> {
    let generation = read_repository_policy_generation(connection)?.wrapping_add(1);
    connection.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![REPOSITORY_POLICY_GENERATION_KEY, generation.to_string()],
    )?;
    Ok(())
}

fn replace_repositories_in(
    transaction: &rusqlite::Transaction<'_>,
    records: &[RepositoryRecord],
) -> Result<()> {
    let previous_policy = ignored_repository_paths_in(transaction)?;
    let seen_at = now_rfc3339();
    for record in records {
        transaction.execute(
            "INSERT INTO repository (
                 key, repo_name, full_name, status, repo_root, suspected_path,
                 worktree_count, session_count, wsl_distro, enabled, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(key) DO UPDATE SET
                 repo_name = excluded.repo_name,
                 full_name = excluded.full_name,
                 status = excluded.status,
                 repo_root = excluded.repo_root,
                 suspected_path = excluded.suspected_path,
                 worktree_count = excluded.worktree_count,
                 session_count = excluded.session_count,
                 wsl_distro = excluded.wsl_distro,
                 last_seen_at = excluded.last_seen_at",
            params![
                record.key,
                record.repo_name,
                record.full_name,
                record.status,
                record.repo_root,
                record.suspected_path,
                record.worktree_count,
                record.session_count,
                record.wsl_distro,
                record.enabled,
                seen_at,
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM repository WHERE last_seen_at <> ?1",
        params![seen_at],
    )?;
    if previous_policy != ignored_repository_paths_in(transaction)? {
        bump_repository_policy_generation_in(transaction)?;
    }
    Ok(())
}

fn ignored_repository_paths_in(connection: &Connection) -> Result<HashSet<String>> {
    let mut statement = connection
        .prepare("SELECT COALESCE(repo_root, suspected_path) FROM repository WHERE enabled = 0")?;
    let paths = statement
        .query_map([], |row| row.get::<_, Option<String>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(paths
        .into_iter()
        .flatten()
        .map(|path| antiburn_local::paths::ignored_paths::normalize(&path))
        .collect())
}

fn config_attribution_json(
    records: &[crate::remediation::PublicationSettingAttribution],
    value: impl Fn(&crate::remediation::PublicationSettingAttribution) -> &str,
) -> Option<String> {
    (!records.is_empty()).then(|| {
        let mut values = serde_json::Map::new();
        for record in records {
            values.insert(
                config_setting_name(record.setting).to_owned(),
                serde_json::Value::String(value(record).to_owned()),
            );
        }
        serde_json::Value::Object(values).to_string()
    })
}

fn config_attribution_values_json(
    records: &[crate::remediation::PublicationSettingAttribution],
) -> Option<String> {
    (!records.is_empty()).then(|| {
        let mut values = serde_json::Map::new();
        for record in records {
            let value = serde_json::from_str(&record.expected_value_json)
                .expect("attribution serializes its typed expected value");
            values.insert(config_setting_name(record.setting).to_owned(), value);
        }
        serde_json::Value::Object(values).to_string()
    })
}

fn config_setting_name(setting: crate::agent_config::ConfigSetting) -> &'static str {
    match setting {
        crate::agent_config::ConfigSetting::Model => "model",
        crate::agent_config::ConfigSetting::Reasoning => "reasoning",
        crate::agent_config::ConfigSetting::Compaction => "compaction",
        crate::agent_config::ConfigSetting::SubagentModel => "subagentModel",
        crate::agent_config::ConfigSetting::McpServer => "mcpServer",
        crate::agent_config::ConfigSetting::BuiltInTool => "builtInTool",
        crate::agent_config::ConfigSetting::Skill => "skill",
        crate::agent_config::ConfigSetting::FastMode => "fastMode",
    }
}
