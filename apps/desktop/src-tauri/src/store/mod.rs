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
//! The connection lives behind a mutex and every method is short and
//! synchronous. Callers that hold the runtime's attention (the scan task) run
//! their long work — reading transcripts, analyzing them — outside the lock and
//! come here only to write the result.

pub mod model;
mod schema;

#[cfg(test)]
mod privacy_tests;
#[cfg(test)]
mod publish_tests;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, ModelRun, PARSER_REVISION, SessionCoverageRecord,
    TurnFacts, TurnRow, TurnRowError, TurnRowStore, TurnSessionKey, count_turn_rows,
    delete_turn_rows, delete_turn_rows_except_fence, delete_turn_rows_for_fence,
    insert_coverage_record, insert_turn_rows, query_coverage_record, query_model_breakdown,
    query_model_runs, query_turn_facts, query_turn_rows,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::dto::DeferredPermissionDir;

pub use model::{
    AnalysisRecord, AppSettings, DisabledAgents, DiskSpaceDisplay, EvidenceClaim,
    EvidenceCompletion, EvidenceFailure, EvidenceRow, EvidenceStatus, HiddenMeters,
    MAX_ACTIVITY_DAYS, MILESTONE_OPTIONS, MIN_ACTIVITY_DAYS, Milestones, NudgePlacement,
    ProjectionRevisions, PublishedEvidence, RETAIN_SESSION_DATA_FOREVER, RelationKind,
    RelationRecord, RepositoryRecord, SessionActivityKey, SessionBadgeMetric, SessionKey,
    SessionRecord, SessionUsageRecord, SessionUsageTurnRecord, SourceVersionState, ThemePreference,
    UsageEvidenceRecord,
};

/// Evidence rows that still wait for, or sit in, processing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvidenceBacklogCounts {
    pub pending: u64,
    pub processing: u64,
}

/// Internal-scalar key holding the protected directories the last pass declined
/// to read.
pub const DEFERRED_PERMISSION_DIRS_KEY: &str = "internal:deferredPermissionDirs";
const PROVIDER_ACCOUNT_SECRET_KEY: &str = "internal:providerAccountHmacSecretV1";

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
    /// The directory the engine's own state files (scan roots, ignored paths)
    /// live in. The engine never chooses this; the shell does.
    state_dir: PathBuf,
}

/// [`Store::recent_sessions`]'s query, pulled out to a shared constant so a
/// schema test can run `EXPLAIN QUERY PLAN` against the exact SQL the method
/// runs, instead of a copy that could drift from it.
const RECENT_SESSIONS_SQL: &str = "SELECT environment_key, agent, session_id, source_kind,
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
      WHERE COALESCE(updated_at_epoch, 0) >= ?1
      ORDER BY COALESCE(updated_at_epoch, 0) DESC, session_id DESC
      LIMIT ?2";

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

    fn from_connection(connection: Connection, state_dir: PathBuf) -> Result<Store> {
        // WAL keeps a read during a scan write from blocking, and `NORMAL` is
        // the documented companion: a crash can lose the last commit, which for
        // a rebuildable cache is cheaper than an fsync per transaction.
        connection.pragma_update(None, "journal_mode", "WAL").ok();
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let store = Store {
            connection: Arc::new(Mutex::new(connection)),
            state_dir,
        };
        store.migrate()?;
        Ok(store)
    }

    /// The directory the engine's state files live in.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Apply every migration the database has not seen yet.
    ///
    /// Idempotent: running it twice is a no-op, which is what makes reopening
    /// an already-current database free.
    fn migrate(&self) -> Result<()> {
        let mut guard = self.lock();
        let current: i64 = guard.pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (index, sql) in schema::MIGRATIONS.iter().enumerate() {
            let version = index as i64 + 1;
            if version <= current {
                continue;
            }
            let tx = guard.transaction()?;
            // The SQLite error is in the source chain either way, but a caller
            // that formats with `{}` rather than `{:#}` prints this line alone
            // — and tauri's setup hook is such a caller. A bare "migration 4
            // failed" is exactly what a developer saw when a database built
            // from an earlier numbering of these migrations met a table that
            // already existed; naming the conflict is what makes it
            // actionable rather than a trip to this file to guess.
            tx.execute_batch(sql)
                .map_err(|error| anyhow::anyhow!("migration {version} failed: {error}"))?;
            // `pragma_update` cannot be parameterized, and `version` is derived
            // from the compiled-in migration list, never from input.
            tx.pragma_update(None, "user_version", version)?;
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

    /// A poisoned lock still holds a usable connection: the panic that poisoned
    /// it happened in a caller, not inside SQLite.
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

    /// Every preference, with defaults filled in for keys never written.
    pub fn settings(&self) -> Result<AppSettings> {
        let connection = self.lock();
        read_settings(&connection)
    }

    /// Replace every preference, returning what was there and what was stored.
    ///
    /// Reading and writing share one transaction so callers can decide which
    /// shell side effects a transition owes without another writer changing the
    /// answer between the two operations.
    pub fn replace_settings(&self, settings: &AppSettings) -> Result<(AppSettings, AppSettings)> {
        self.replace_settings_with(settings, |_, _| Ok(()))
            .map(|(previous, saved, ())| (previous, saved))
    }

    /// Replace preferences and apply another database change in one transaction.
    pub fn replace_settings_with<T>(
        &self,
        settings: &AppSettings,
        apply: impl FnOnce(&rusqlite::Transaction<'_>, &AppSettings) -> Result<T>,
    ) -> Result<(AppSettings, AppSettings, T)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let previous = read_settings(&tx)?;
        let saved = settings.clone().normalized();
        write_settings(&tx, &saved)?;
        let result = apply(&tx, &saved)?;
        tx.commit()?;
        Ok((previous, saved, result))
    }

    /// Apply the stored session-data retention policy.
    pub fn apply_session_retention(&self, now_epoch: i64) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let settings = read_settings(&tx)?;
        let removed =
            apply_session_retention_in(&tx, settings.session_data_retention_days, now_epoch)?;
        tx.commit()?;
        Ok(removed)
    }

    /// Change preferences against the latest stored value in one transaction.
    pub fn update_settings(
        &self,
        update: impl FnOnce(&mut AppSettings),
    ) -> Result<(AppSettings, AppSettings)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let previous = read_settings(&tx)?;
        let mut saved = previous.clone();
        update(&mut saved);
        let saved = saved.normalized();
        write_settings(&tx, &saved)?;
        tx.commit()?;
        Ok((previous, saved))
    }

    /// Make setup pending without changing the reader's data or choices.
    pub fn restart_onboarding(&self) -> Result<(AppSettings, AppSettings)> {
        self.update_settings(|settings| settings.onboarding_completed = false)
    }

    /// Replace every preference, returning what was actually stored (clamped).
    #[cfg(test)]
    pub fn save_settings(&self, settings: &AppSettings) -> Result<AppSettings> {
        self.replace_settings(settings).map(|(_, saved)| saved)
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
        self.lock().execute(
            "INSERT INTO consent_grant (dir_name, granted_at) VALUES (?1, ?2)
             ON CONFLICT(dir_name) DO UPDATE SET granted_at = excluded.granted_at",
            params![dir_name, now_rfc3339()],
        )?;
        Ok(())
    }

    /// Drop a recorded grant, after a read under it came back denied.
    /// Idempotent.
    pub fn revoke_dir_grant(&self, dir_name: &str) -> Result<()> {
        self.lock().execute(
            "DELETE FROM consent_grant WHERE dir_name = ?1",
            params![dir_name],
        )?;
        Ok(())
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

    /// The next batch to attempt, oldest first, as `(id, payload)`.
    pub fn pending_analytics_events(&self, limit: u32) -> Result<Vec<(i64, String)>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT id, payload FROM analytics_event ORDER BY id LIMIT ?1")?;
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
        self.lock().execute(
            "INSERT INTO analytics_identity (id, install_id, minted_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET install_id = excluded.install_id,
                                           minted_at  = excluded.minted_at",
            params![install_id, now_rfc3339()],
        )?;
        Ok(())
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
        self.set_internal_value(DEFERRED_PERMISSION_DIRS_KEY, &serde_json::to_string(dirs)?);
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

    /// Insert or refresh a batch of sessions in one transaction.
    ///
    /// `first_seen_at` survives a rescan; everything else is replaced with what
    /// the scan just observed, so a renamed session picks up its new title
    /// without producing a second row.
    pub fn upsert_sessions(
        &self,
        records: &[SessionRecord],
        evidence_agents: &[&str],
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        for record in records {
            let source_returned = tx.query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM session_evidence
                      WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                        AND status = 'failed' AND last_error = 'source-missing'
                 )",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id
                ],
                |row| row.get::<_, i64>(0),
            )? != 0;
            let (previous_generation, source_generation, activity_cursor_changed) =
                upsert_session_in(&tx, record)?;
            let generation_increased =
                previous_generation.is_none_or(|previous| source_generation > previous);
            if evidence_agents.contains(&record.key.agent.as_str())
                && (generation_increased || activity_cursor_changed || source_returned)
            {
                mark_evidence_pending_in(&tx, &record.key)?;
            }
        }
        tx.commit()?;
        Ok(())
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
        let excluded = excluded_agents.slugs();
        let exclusion_predicate = if excluded.is_empty() {
            String::new()
        } else {
            // Parameters 1 and 2 are the window and the limit, so the agent
            // list binds from parameter 3.
            let placeholders = (0..excluded.len())
                .map(|index| format!("?{}", index + 3))
                .collect::<Vec<_>>()
                .join(", ");
            format!(" AND agent NOT IN ({placeholders})")
        };
        let connection = self.lock();
        // Splice the agent exclusion into the shared SQL. The base query and
        // the plan test keep one constant.
        let sql = RECENT_SESSIONS_SQL.replace(
            "WHERE COALESCE(updated_at_epoch, 0) >= ?1",
            &format!("WHERE COALESCE(updated_at_epoch, 0) >= ?1{exclusion_predicate}"),
        );
        let mut statement = connection.prepare(&sql)?;
        let mut values: Vec<rusqlite::types::Value> = vec![
            rusqlite::types::Value::Integer(since_epoch),
            rusqlite::types::Value::Integer(limit as i64),
        ];
        values.extend(
            excluded
                .iter()
                .map(|agent| rusqlite::types::Value::Text(agent.clone())),
        );
        let rows =
            statement.query_map(rusqlite::params_from_iter(values.iter()), session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
        let exclusion_predicate = if excluded.is_empty() {
            String::new()
        } else {
            // Parameters 1-5 are the scope, window and revisions, so the
            // agent list binds from parameter 6.
            let placeholders = (0..excluded.len())
                .map(|index| format!("?{}", index + 6))
                .collect::<Vec<_>>()
                .join(", ");
            format!(" AND s.agent NOT IN ({placeholders})")
        };
        let current_ready = "e.status = 'ready'
                 AND NOT (e.analyzed_generation IS NOT s.source_generation)
                 AND NOT (e.parser_revision IS NOT ?3)
                 AND NOT (e.analyzer_revision IS NOT ?4)
                 AND NOT (e.evidence_schema_revision IS NOT ?5)";
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
              WHERE s.environment_key = ?1
                AND COALESCE(s.updated_at_epoch, 0) >= ?2{exclusion_predicate}",
        ))?;
        let mut values: Vec<rusqlite::types::Value> = vec![
            rusqlite::types::Value::Text(environment_key.to_owned()),
            rusqlite::types::Value::Integer(since_epoch),
            rusqlite::types::Value::Integer(PARSER_REVISION),
            rusqlite::types::Value::Integer(ANALYZER_REVISION),
            rusqlite::types::Value::Integer(EVIDENCE_SCHEMA_REVISION),
        ];
        values.extend(
            excluded
                .iter()
                .map(|agent| rusqlite::types::Value::Text(agent.clone())),
        );
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(HygieneSummaryRow {
                settled: row.get::<_, bool>(0)?,
                evidence_json: row.get::<_, Option<String>>(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return every session's full cached record keyed by environment, agent,
    /// and source label. A single map keeps the scan's cheap unchanged-source
    /// gate outside the SQLite lock without allowing native/WSL rows to
    /// collide, and lets the scan reuse a whole previous record instead of
    /// re-describing a source that has not changed.
    pub fn session_records(&self) -> Result<HashMap<SessionActivityKey, SessionRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, session_id, source_kind, source_label, wsl_distro,
                    title, title_source, cwd, surface, updated_at_epoch,
                    activity_cursor, activity_source, subagent_count,
                    (SELECT related_id FROM session_relation r
                       WHERE r.environment_key = s.environment_key
                         AND r.agent = s.agent
                         AND r.session_id = s.session_id
                         AND r.kind = 'forkParent'
                       LIMIT 1),
                    s.source_fingerprint
               FROM session s",
        )?;
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

    /// One session's cached metadata, when it has been seen.
    pub fn session(&self, key: &SessionKey) -> Result<Option<SessionRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, session_id, source_kind, source_label, wsl_distro,
                    title, title_source, cwd, surface, updated_at_epoch,
                    activity_cursor, activity_source, subagent_count,
                    (SELECT related_id FROM session_relation r
                       WHERE r.environment_key = s.environment_key
                         AND r.agent = s.agent
                         AND r.session_id = s.session_id
                         AND r.kind = 'forkParent'
                       LIMIT 1),
                    s.source_fingerprint
               FROM session s
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        )?;
        Ok(statement
            .query_row(
                params![key.environment_key, key.agent, key.session_id],
                session_from_row,
            )
            .optional()?)
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
        let agent_placeholders = vec!["?"; agents.len()].join(", ");
        let agent_values: Vec<rusqlite::types::Value> = agents
            .iter()
            .map(|agent| rusqlite::types::Value::Text((*agent).to_string()))
            .collect();
        let enrolled = transaction.execute(
            &format!(
                "INSERT INTO session_evidence (environment_key, agent, session_id)
                 SELECT session.environment_key, session.agent, session.session_id
                   FROM session
                  WHERE session.agent IN ({agent_placeholders})
                    AND NOT EXISTS (
                        SELECT 1 FROM session_evidence
                         WHERE session_evidence.environment_key = session.environment_key
                           AND session_evidence.agent = session.agent
                           AND session_evidence.session_id = session.session_id
                    )"
            ),
            rusqlite::params_from_iter(agent_values.iter()),
        )?;

        let parser_parameter = agents.len() + 1;
        let analyzer_parameter = agents.len() + 2;
        let metrics_parameter = agents.len() + 3;
        let evidence_parameter = agents.len() + 4;
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
                           OR evidence.parser_revision IS NOT ?{parser_parameter}
                           OR evidence.analyzer_revision IS NOT ?{analyzer_parameter}
                           OR evidence.evidence_schema_revision IS NOT ?{evidence_parameter}
                           OR (evidence.status NOT IN ('failed', 'unsupported')
                               AND NOT EXISTS (
                               SELECT 1 FROM session_analysis AS analysis
                                WHERE analysis.environment_key = session.environment_key
                                  AND analysis.agent = session.agent
                                  AND analysis.session_id = session.session_id
                                  AND analysis.analyzed_generation = session.source_generation
                                  AND analysis.parser_revision = ?{parser_parameter}
                                  AND analysis.analyzer_revision = ?{analyzer_parameter}
                                  AND analysis.metrics_schema_revision = ?{metrics_parameter}
                           ))
                       )
                )"
        );
        let mut update_values = agent_values;
        update_values.extend([
            rusqlite::types::Value::Integer(revisions.parser_revision),
            rusqlite::types::Value::Integer(revisions.analyzer_revision),
            rusqlite::types::Value::Integer(revisions.metrics_schema_revision),
            rusqlite::types::Value::Integer(revisions.evidence_schema_revision),
        ]);
        let requeued = transaction.execute(
            &update_sql,
            rusqlite::params_from_iter(update_values.iter()),
        )?;
        transaction.commit()?;
        Ok(enrolled + requeued)
    }

    /// Claim the next eligible evidence row for an enabled agent.
    pub fn claim_next_evidence(
        &self,
        agents: &[&str],
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<Option<EvidenceClaim>> {
        if agents.is_empty() {
            return Ok(None);
        }

        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let agent_placeholders = vec!["?"; agents.len()].join(", ");
        let mut values: Vec<rusqlite::types::Value> = agents
            .iter()
            .map(|agent| rusqlite::types::Value::Text((*agent).to_string()))
            .collect();
        values.push(rusqlite::types::Value::Integer(now_epoch));
        let now_parameter = values.len();
        let candidate = transaction
            .query_row(
                &format!(
                    "SELECT evidence.environment_key, evidence.agent, evidence.session_id
                       FROM session_evidence AS evidence
                       JOIN session
                         ON session.environment_key = evidence.environment_key
                        AND session.agent = evidence.agent
                        AND session.session_id = evidence.session_id
                      WHERE evidence.agent IN ({agent_placeholders})
                        AND (
                            evidence.status = 'pending'
                            OR (evidence.status = 'processing'
                                AND evidence.lease_expires_at_epoch <= ?{now_parameter})
                        )
                        AND (evidence.next_attempt_at_epoch IS NULL
                             OR evidence.next_attempt_at_epoch <= ?{now_parameter})
                      ORDER BY evidence.next_attempt_at_epoch,
                               evidence.claimed_at_epoch,
                               evidence.environment_key, evidence.agent, evidence.session_id
                      LIMIT 1"
                ),
                rusqlite::params_from_iter(values.iter()),
                |row| {
                    Ok(SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some(key) = candidate else {
            transaction.commit()?;
            return Ok(None);
        };

        transaction.execute(
            "UPDATE session_evidence
                SET status = 'processing', claim_fence = claim_fence + 1,
                    claimed_at_epoch = ?4, lease_expires_at_epoch = ?5
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                now_epoch,
                now_epoch + lease_secs,
            ],
        )?;
        let (source_generation, claim_fence, retry_count) = transaction.query_row(
            "SELECT session.source_generation, evidence.claim_fence, evidence.retry_count
               FROM session_evidence AS evidence
               JOIN session
                 ON session.environment_key = evidence.environment_key
                AND session.agent = evidence.agent
                AND session.session_id = evidence.session_id
              WHERE evidence.environment_key = ?1
                AND evidence.agent = ?2 AND evidence.session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        transaction.commit()?;
        Ok(Some(EvidenceClaim {
            key,
            source_generation,
            claim_fence,
            retry_count,
        }))
    }

    /// Extend a claim when its fence and source generation remain current.
    pub fn renew_evidence_lease(
        &self,
        claim: &EvidenceClaim,
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<bool> {
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE session_evidence AS evidence
                SET claimed_at_epoch = ?6, lease_expires_at_epoch = ?7
              WHERE evidence.environment_key = ?1
                AND evidence.agent = ?2 AND evidence.session_id = ?3
                AND evidence.status = 'processing' AND evidence.claim_fence = ?4
                AND EXISTS (
                    SELECT 1 FROM session
                     WHERE session.environment_key = evidence.environment_key
                       AND session.agent = evidence.agent
                       AND session.session_id = evidence.session_id
                       AND session.source_generation = ?5
                )",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                claim.claim_fence,
                claim.source_generation,
                now_epoch,
                now_epoch + lease_secs,
            ],
        )?;
        Ok(updated > 0)
    }

    /// Record a retry or terminal failure for a current claim.
    pub fn fail_evidence(
        &self,
        claim: &EvidenceClaim,
        failure: EvidenceFailure,
        last_error: &str,
    ) -> Result<bool> {
        let connection = self.lock();
        let updated = match failure {
            EvidenceFailure::Retry {
                next_attempt_at_epoch,
            } => connection.execute(
                "UPDATE session_evidence AS evidence
                    SET status = 'pending', retry_count = retry_count + 1,
                        last_error = ?6, claimed_at_epoch = NULL,
                        lease_expires_at_epoch = NULL, next_attempt_at_epoch = ?7
                  WHERE evidence.environment_key = ?1
                    AND evidence.agent = ?2 AND evidence.session_id = ?3
                    AND evidence.status = 'processing' AND evidence.claim_fence = ?4
                    AND EXISTS (
                        SELECT 1 FROM session
                         WHERE session.environment_key = evidence.environment_key
                           AND session.agent = evidence.agent
                           AND session.session_id = evidence.session_id
                           AND session.source_generation = ?5
                    )",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.claim_fence,
                    claim.source_generation,
                    last_error,
                    next_attempt_at_epoch,
                ],
            )?,
            EvidenceFailure::Failed { revisions } => connection.execute(
                "UPDATE session_evidence AS evidence
                    SET status = 'failed', retry_count = retry_count + 1,
                        analyzed_generation = ?5, parser_revision = ?7,
                        analyzer_revision = ?8, evidence_schema_revision = ?9,
                        evidence_json = NULL,
                        last_error = ?6, claimed_at_epoch = NULL,
                        lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL
                  WHERE evidence.environment_key = ?1
                    AND evidence.agent = ?2 AND evidence.session_id = ?3
                    AND evidence.status = 'processing' AND evidence.claim_fence = ?4
                    AND EXISTS (
                        SELECT 1 FROM session
                         WHERE session.environment_key = evidence.environment_key
                           AND session.agent = evidence.agent
                           AND session.session_id = evidence.session_id
                           AND session.source_generation = ?5
                    )",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.claim_fence,
                    claim.source_generation,
                    last_error,
                    revisions.parser_revision,
                    revisions.analyzer_revision,
                    revisions.evidence_schema_revision,
                ],
            )?,
        };
        Ok(updated > 0)
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
    pub fn clear_local_session_data(&self) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM session_relation", [])?;
        tx.execute("DELETE FROM session_analysis", [])?;
        tx.execute("DELETE FROM session_evidence", [])?;
        tx.execute("DELETE FROM turn_content", [])?;
        tx.execute("DELETE FROM session_coverage", [])?;
        tx.execute("DELETE FROM turn", [])?;
        let sessions = tx.execute("DELETE FROM session", [])?;
        tx.execute("DELETE FROM provider_account_seen", [])?;
        tx.execute(
            "DELETE FROM setting
              WHERE key IN (?1, 'internal:liveUsageHistoryV2', 'internal:liveUsageSnapshotV2')",
            params![PROVIDER_ACCOUNT_SECRET_KEY],
        )?;
        tx.execute("DELETE FROM scan_state", [])?;
        tx.execute("UPDATE repository SET session_count = 0", [])?;
        tx.commit()?;
        crate::provider_accounts::clear_cache();
        Ok(sessions)
    }

    /// Delete every antiburn-owned record for one session.
    ///
    /// Local records only. The provider's transcript is never touched — see
    /// [`crate::commands::delete_session_data`].
    pub fn delete_session(&self, key: &SessionKey) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let removed = delete_session_in(&tx, key)?;
        tx.commit()?;
        Ok(removed)
    }

    /* --------------------------------------------------------------------
     * Derived analysis
     * ----------------------------------------------------------------- */

    /// Publish metrics, evidence, and the optional start time as one pass.
    pub fn publish_projections(
        &self,
        record: &AnalysisRecord,
        started_at_epoch: Option<i64>,
        completion: &EvidenceCompletion,
        relations: &[RelationRecord],
    ) -> Result<bool> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO session_analysis (
                 environment_key, agent, session_id, model_breakdown_json,
                 inclusive_models_json, initial_context_json, source_summaries_json,
                 provider_hints_json,
                 source_fingerprint, pricing_generation, analyzed_generation,
                 parser_revision, analyzer_revision, metrics_schema_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 model_breakdown_json = excluded.model_breakdown_json,
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
                    next_attempt_at_epoch = NULL, published_fence = ?12
              WHERE evidence.environment_key = ?1
                AND evidence.agent = ?2 AND evidence.session_id = ?3
                AND evidence.status = 'processing' AND evidence.claim_fence = ?12
                AND EXISTS (
                    SELECT 1 FROM session
                     WHERE session.environment_key = evidence.environment_key
                       AND session.agent = evidence.agent
                       AND session.session_id = evidence.session_id
                       AND session.source_generation = ?5
                )",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                completion.status.as_str(),
                record.analyzed_generation,
                record.source_fingerprint,
                record.parser_revision,
                record.analyzer_revision,
                completion.evidence_schema_revision,
                completion.evidence_json,
                time::OffsetDateTime::now_utc().unix_timestamp(),
                completion.claim_fence,
            ],
        )?;
        if updated == 0 {
            // This pass lost the claim race (a newer claim, or a newer
            // source generation, already moved past it). Dropping the
            // transaction without a commit rolls back the session_analysis
            // upsert above, exactly as before this pass wrote turn rows: a
            // lost race must not publish anything this pass computed. Its
            // rows were never published either, so they are cleaned up here
            // under their own fence, in a separate statement, rather than
            // left to accumulate.
            drop(transaction);
            delete_turn_rows_for_fence(
                &connection,
                &turn_session_key(&record.key),
                completion.claim_fence,
            )?;
            return Ok(false);
        }
        // Every row from an earlier, superseded pass is dropped now that
        // this pass's evidence is what published. Rows already carry this
        // pass's fence — see `analysis::analyze_for_evidence` — so only
        // stale fences are removed.
        delete_turn_rows_except_fence(
            &transaction,
            &turn_session_key(&record.key),
            completion.claim_fence,
        )?;
        replace_relations_in(&transaction, &record.key, RelationKind::Subagent, relations)?;
        transaction.commit()?;
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
        let connection = self.lock();
        mark_evidence_pending_in(&connection, key)
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
    pub fn published_turn_rows(&self, key: &SessionKey) -> Result<Option<Vec<TurnRow>>> {
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
        Ok(Some(query_turn_rows(
            &connection,
            &turn_session_key(key),
            published_fence,
        )?))
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
            published_fence,
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
                 inclusive_models_json, initial_context_json, source_summaries_json,
                 provider_hints_json,
                 source_fingerprint, pricing_generation, analyzed_generation,
                 parser_revision, analyzer_revision, metrics_schema_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 model_breakdown_json = excluded.model_breakdown_json,
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

    /// One session's cached analysis, when it has been computed.
    pub fn analysis(&self, key: &SessionKey) -> Result<Option<AnalysisRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, session_id, model_breakdown_json,
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
                        inclusive_models_json: row.get(4)?,
                        initial_context_json: row.get(5)?,
                        source_summaries_json: row.get(6)?,
                        provider_hints_json: row.get(7)?,
                        source_fingerprint: row.get(8)?,
                        pricing_generation: row.get(9)?,
                        analyzed_generation: row.get(10)?,
                        parser_revision: row.get(11)?,
                        analyzer_revision: row.get(12)?,
                        metrics_schema_revision: row.get(13)?,
                    })
                },
            )
            .optional()?)
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
                    a.provider_hints_json,
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
              ORDER BY COALESCE(s.updated_at_epoch, 0) DESC",
        )?;
        let rows = statement.query_map(params![since_epoch], |row| {
            Ok(UsageEvidenceRecord {
                agent: row.get(0)?,
                updated_at_epoch: row.get(1)?,
                model_breakdown_json: row.get(2)?,
                provider_hints_json: row.get(3)?,
                provider_accounts_json: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Published timestamped turns at or after `since_ms`, grouped by session.
    pub fn session_usage_turns(&self, since_ms: i64) -> Result<Vec<SessionUsageRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "WITH recent_session AS (
                SELECT DISTINCT t.environment_key, t.agent, t.session_id
                  FROM turn t
                  JOIN session_evidence e
                    ON e.environment_key = t.environment_key
                   AND e.agent = t.agent
                   AND e.session_id = t.session_id
                   AND e.published_fence = t.claim_fence
                 WHERE t.ts_ms >= ?1
            )
             SELECT s.environment_key, s.agent, s.session_id, s.wsl_distro,
                    a.provider_hints_json,
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
               FROM recent_session r
               JOIN session s
                 ON s.environment_key = r.environment_key
                AND s.agent = r.agent
                AND s.session_id = r.session_id
               LEFT JOIN session_analysis a
                 ON a.environment_key = s.environment_key
                AND a.agent = s.agent
                AND a.session_id = s.session_id",
        )?;
        let rows = statement.query_map(params![since_ms], |row| {
            Ok(SessionUsageRecord {
                key: SessionKey {
                    environment_key: row.get(0)?,
                    agent: row.get(1)?,
                    session_id: row.get(2)?,
                },
                wsl_distro: row.get(3)?,
                provider_hints_json: row.get(4)?,
                provider_accounts_json: row.get(5)?,
                turns: Vec::new(),
            })
        })?;
        let mut sessions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let indexes: HashMap<_, _> = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| (session.key.clone(), index))
            .collect();
        let mut statement = connection.prepare(
            "SELECT t.environment_key, t.agent, t.session_id,
                    t.ts_ms, t.model, t.input_tokens, t.cache_read_tokens,
                    t.cache_write_tokens, t.output_tokens
               FROM turn t
               JOIN session_evidence e
                 ON e.environment_key = t.environment_key
                AND e.agent = t.agent
                AND e.session_id = t.session_id
                AND e.published_fence = t.claim_fence
              WHERE t.ts_ms >= ?1
              ORDER BY t.ts_ms, t.rowid",
        )?;
        let mut rows = statement.query(params![since_ms])?;
        while let Some(row) = rows.next()? {
            let key = SessionKey {
                environment_key: row.get(0)?,
                agent: row.get(1)?,
                session_id: row.get(2)?,
            };
            let index = indexes
                .get(&key)
                .copied()
                .context("a recent turn has no session metadata")?;
            sessions[index].turns.push(SessionUsageTurnRecord {
                ts_ms: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                cache_read_tokens: row.get(6)?,
                cache_write_tokens: row.get(7)?,
                output_tokens: row.get(8)?,
            });
        }
        Ok(sessions)
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

    /// Refresh the located repositories, preserving each one's include choice.
    ///
    /// A repository the user turned off stays off across rescans: the incoming
    /// records describe what is on disk, never what the user decided.
    pub fn replace_repositories(&self, records: &[RepositoryRecord]) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let seen_at = now_rfc3339();
        for record in records {
            tx.execute(
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
        // Anything the scan no longer sees is gone from this machine.
        tx.execute(
            "DELETE FROM repository WHERE last_seen_at <> ?1",
            params![seen_at],
        )?;
        tx.commit()?;
        Ok(())
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
        let changed = self.lock().execute(
            "UPDATE repository SET enabled = ?2 WHERE key = ?1",
            params![key, enabled],
        )?;
        Ok(changed > 0)
    }
}

/// Read every preference through one already-held connection or transaction.
/// One session's analysis state, as [`Store::hygiene_summary_rows`] reads it.
#[derive(Debug, Clone)]
pub struct HygieneSummaryRow {
    /// True when analysis reached a terminal state for the current source.
    pub settled: bool,
    /// Current ready evidence JSON, when the session has one.
    pub evidence_json: Option<String>,
}

fn read_settings(connection: &Connection) -> Result<AppSettings> {
    let mut statement = connection.prepare("SELECT key, value FROM setting")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut stored: HashMap<String, String> = HashMap::new();
    for row in rows {
        let (key, value) = row?;
        stored.insert(key, value);
    }

    let defaults = AppSettings::default();
    Ok(AppSettings {
        theme: stored
            .get("theme")
            .and_then(|value| ThemePreference::parse(value))
            .unwrap_or(defaults.theme),
        activity_window_days: stored
            .get("activityWindowDays")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.activity_window_days),
        session_data_retention_days: stored
            .get("sessionDataRetentionDays")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.session_data_retention_days),
        onboarding_completed: stored
            .get("onboardingCompleted")
            .map(|value| value == "true")
            .unwrap_or(defaults.onboarding_completed),
        launch_at_login: stored
            .get("launchAtLogin")
            .map(|value| value == "true")
            .unwrap_or(defaults.launch_at_login),
        auto_update: stored
            .get("autoUpdate")
            .map(|value| value == "true")
            .unwrap_or(defaults.auto_update),
        discovery_paused: stored
            .get("discoveryPaused")
            .map(|value| value == "true")
            .unwrap_or(defaults.discovery_paused),
        notifications_enabled: stored
            .get("notificationsEnabled")
            .map(|value| value == "true")
            .unwrap_or(defaults.notifications_enabled),
        notify_update_available: stored
            .get("notifyUpdateAvailable")
            .map(|value| value == "true")
            .unwrap_or(defaults.notify_update_available),
        notify_scan_failure: stored
            .get("notifyScanFailure")
            .map(|value| value == "true")
            .unwrap_or(defaults.notify_scan_failure),
        nudge_placement: stored
            .get("nudgePlacement")
            .and_then(|value| NudgePlacement::parse(value))
            .unwrap_or(defaults.nudge_placement),
        nudge_auto_dismiss_secs: stored
            .get("nudgeAutoDismissSecs")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.nudge_auto_dismiss_secs),
        notification_sound: stored
            .get("notificationSound")
            .map(|value| value == "true")
            .unwrap_or(defaults.notification_sound),
        nudges_respect_dnd: stored
            .get("nudgesRespectDnd")
            .map(|value| value == "true")
            .unwrap_or(defaults.nudges_respect_dnd),
        disk_space_display: stored
            .get("diskSpaceDisplay")
            .and_then(|value| DiskSpaceDisplay::parse(value))
            .unwrap_or(defaults.disk_space_display),
        disk_space_threshold_gb: stored
            .get("diskSpaceThresholdGb")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.disk_space_threshold_gb),
        notify_disk_space_low: stored
            .get("notifyDiskSpaceLow")
            .map(|value| value == "true")
            .unwrap_or(defaults.notify_disk_space_low),
        milestones_5h: stored
            .get("milestonePercentages5h")
            .map(|value| Milestones::parse(value))
            .unwrap_or(defaults.milestones_5h),
        milestones_weekly: stored
            .get("milestonePercentagesWeekly")
            .map(|value| Milestones::parse(value))
            .unwrap_or(defaults.milestones_weekly),
        live_usage_enabled: stored
            .get("liveUsageEnabled")
            .map(|value| value == "true")
            .unwrap_or(defaults.live_usage_enabled),
        live_usage_hidden_providers: stored
            .get("liveUsageHiddenProviders")
            .map(|value| HiddenMeters::parse(value))
            .unwrap_or(defaults.live_usage_hidden_providers.clone()),
        disabled_agents: stored
            .get("disabledAgents")
            .map(|value| DisabledAgents::parse(value))
            .unwrap_or(defaults.disabled_agents.clone()),
        // No stored answer means this database predates the setting. A fresh
        // install takes the default. An install that already finished setup
        // stays off until the reader enables it.
        analytics_enabled: stored
            .get("analyticsEnabled")
            .map(|value| value == "true")
            .unwrap_or_else(|| {
                let finished = stored
                    .get("onboardingCompleted")
                    .map(|value| value == "true")
                    .unwrap_or(false);
                !finished && defaults.analytics_enabled
            }),
        overview_limits_expanded: stored
            .get("overviewLimitsExpanded")
            .map(|value| value == "true")
            .unwrap_or(defaults.overview_limits_expanded),
        session_badge_metric: stored
            .get("sessionBadgeMetric")
            .and_then(|value| SessionBadgeMetric::parse(value))
            .unwrap_or(defaults.session_badge_metric),
    }
    .normalized())
}

/// Write every normalized preference through one transaction.
fn write_settings(connection: &Connection, settings: &AppSettings) -> Result<()> {
    let mut put = connection.prepare(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )?;
    put.execute(params!["theme", settings.theme.as_str()])?;
    put.execute(params![
        "activityWindowDays",
        settings.activity_window_days.to_string()
    ])?;
    put.execute(params![
        "sessionDataRetentionDays",
        settings.session_data_retention_days.to_string()
    ])?;
    put.execute(params![
        "onboardingCompleted",
        bool_text(settings.onboarding_completed)
    ])?;
    put.execute(params![
        "launchAtLogin",
        bool_text(settings.launch_at_login)
    ])?;
    put.execute(params!["autoUpdate", bool_text(settings.auto_update)])?;
    put.execute(params![
        "discoveryPaused",
        bool_text(settings.discovery_paused)
    ])?;
    put.execute(params![
        "notificationsEnabled",
        bool_text(settings.notifications_enabled)
    ])?;
    put.execute(params![
        "notifyUpdateAvailable",
        bool_text(settings.notify_update_available)
    ])?;
    put.execute(params![
        "notifyScanFailure",
        bool_text(settings.notify_scan_failure)
    ])?;
    put.execute(params!["nudgePlacement", settings.nudge_placement.as_str()])?;
    put.execute(params![
        "nudgeAutoDismissSecs",
        settings.nudge_auto_dismiss_secs.to_string()
    ])?;
    put.execute(params![
        "notificationSound",
        bool_text(settings.notification_sound)
    ])?;
    put.execute(params![
        "nudgesRespectDnd",
        bool_text(settings.nudges_respect_dnd)
    ])?;
    put.execute(params![
        "diskSpaceDisplay",
        settings.disk_space_display.as_str()
    ])?;
    put.execute(params![
        "diskSpaceThresholdGb",
        settings.disk_space_threshold_gb.to_string()
    ])?;
    put.execute(params![
        "notifyDiskSpaceLow",
        bool_text(settings.notify_disk_space_low)
    ])?;
    put.execute(params![
        "milestonePercentages5h",
        settings.milestones_5h.as_str()
    ])?;
    put.execute(params![
        "milestonePercentagesWeekly",
        settings.milestones_weekly.as_str()
    ])?;
    put.execute(params![
        "liveUsageEnabled",
        bool_text(settings.live_usage_enabled)
    ])?;
    put.execute(params![
        "liveUsageHiddenProviders",
        settings.live_usage_hidden_providers.as_str()
    ])?;
    put.execute(params!["disabledAgents", settings.disabled_agents.as_str()])?;
    put.execute(params![
        "analyticsEnabled",
        bool_text(settings.analytics_enabled)
    ])?;
    put.execute(params![
        "overviewLimitsExpanded",
        bool_text(settings.overview_limits_expanded)
    ])?;
    put.execute(params![
        "sessionBadgeMetric",
        settings.session_badge_metric.as_str()
    ])?;
    Ok(())
}

/// `true`/`false` as the text the setting table stores.
fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// The current UTC time as a fixed-width RFC 3339 stamp, so plain string
/// comparison of two values stays chronological.
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// An epoch stamp as the RFC 3339 string the views parse.
///
/// Lives beside [`now_rfc3339`] because the two answer the same question in the
/// same spelling; every payload that carries a time uses one of them, so the
/// webview never has to guess a format.
pub fn iso_from_epoch(epoch: Option<i64>) -> String {
    let epoch = epoch.unwrap_or(0);
    time::OffsetDateTime::from_unix_timestamp(epoch)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

fn upsert_session_in(
    connection: &Connection,
    record: &SessionRecord,
) -> Result<(Option<i64>, i64, bool)> {
    let previous_state = connection
        .query_row(
            "SELECT source_generation, activity_cursor FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let previous_generation = previous_state.as_ref().map(|(generation, _)| *generation);
    let activity_cursor_changed = previous_state
        .as_ref()
        .is_some_and(|(_, cursor)| cursor != &record.activity_cursor);
    let now = now_rfc3339();
    connection.execute(
        "INSERT INTO session (
             environment_key, agent, session_id, source_kind, source_label, wsl_distro,
             title, title_source, cwd, surface, updated_at_epoch,
             activity_cursor, activity_source, subagent_count,
             first_seen_at, last_seen_at, source_fingerprint, source_generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                 CASE WHEN ?7 IS NOT NULL THEN ?8 ELSE NULL END,
                 ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15, ?16,
                 CASE WHEN ?16 IS NOT NULL THEN 1 ELSE 0 END)
         ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
             source_kind = excluded.source_kind,
             source_label = excluded.source_label,
             wsl_distro = excluded.wsl_distro,
             title = COALESCE(excluded.title, session.title),
             title_source = CASE
                 WHEN excluded.title IS NOT NULL THEN excluded.title_source
                 WHEN session.title IS NOT NULL THEN session.title_source
                 ELSE NULL
             END,
             cwd = COALESCE(excluded.cwd, session.cwd),
             surface = excluded.surface,
             updated_at_epoch = CASE
                 WHEN session.activity_source = 'event'
                      AND excluded.activity_source <> 'event'
                     THEN session.updated_at_epoch
                 WHEN excluded.activity_source = 'event'
                      AND session.activity_source = 'event'
                     THEN MAX(COALESCE(excluded.updated_at_epoch, 0),
                              COALESCE(session.updated_at_epoch, 0))
                 WHEN excluded.activity_source = 'event'
                     THEN excluded.updated_at_epoch
                 ELSE MAX(COALESCE(excluded.updated_at_epoch, 0),
                          COALESCE(session.updated_at_epoch, 0))
             END,
             activity_cursor = excluded.activity_cursor,
             activity_source = CASE
                 WHEN excluded.activity_source = 'event'
                     THEN 'event'
                 WHEN session.activity_source = 'event'
                     THEN 'event'
                 ELSE excluded.activity_source
             END,
             subagent_count = excluded.subagent_count,
             source_fingerprint = COALESCE(excluded.source_fingerprint, session.source_fingerprint),
             source_generation = CASE
                 WHEN excluded.source_fingerprint IS NULL THEN session.source_generation
                 WHEN session.source_fingerprint = excluded.source_fingerprint
                     THEN session.source_generation
                 ELSE session.source_generation + 1
             END,
             last_seen_at = excluded.last_seen_at",
        params![
            record.key.environment_key,
            record.key.agent,
            record.key.session_id,
            record.source_kind,
            record.source_label,
            record.wsl_distro,
            record.title,
            record.title_source,
            record.cwd,
            record.surface,
            record.updated_at_epoch,
            record.activity_cursor,
            record.activity_source,
            record.subagent_count,
            now,
            record.source_fingerprint,
        ],
    )?;

    // A fork parent is written only when the caller actually observed one.
    //
    // Absence is not evidence. Some adapters resolve lineage only when a
    // session opens. A later scan must not erase that relation.
    if let Some(parent) = &record.fork_parent_session_id {
        connection.execute(
            "INSERT INTO session_relation
                 (environment_key, agent, session_id, kind, related_id, label)
             VALUES (?1, ?2, ?3, 'forkParent', ?4, NULL)
             ON CONFLICT DO NOTHING",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                parent
            ],
        )?;
    }
    let source_generation = connection.query_row(
        "SELECT source_generation FROM session
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![
            record.key.environment_key,
            record.key.agent,
            record.key.session_id
        ],
        |row| row.get(0),
    )?;
    Ok((
        previous_generation,
        source_generation,
        activity_cursor_changed,
    ))
}

fn replace_relations_in(
    connection: &Connection,
    key: &SessionKey,
    kind: RelationKind,
    relations: &[RelationRecord],
) -> Result<()> {
    connection.execute(
        "DELETE FROM session_relation
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND kind = ?4",
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            kind.as_str()
        ],
    )?;
    for relation in relations {
        connection.execute(
            "INSERT INTO session_relation
                 (environment_key, agent, session_id, kind, related_id, label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT DO UPDATE SET label = excluded.label",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                relation.kind.as_str(),
                relation.related_id,
                relation.label,
            ],
        )?;
    }
    Ok(())
}

fn mark_evidence_pending_in(connection: &Connection, key: &SessionKey) -> Result<()> {
    connection.execute(
        "INSERT INTO session_evidence (environment_key, agent, session_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
             status = 'pending', last_error = NULL,
             next_attempt_at_epoch = NULL, retry_count = 0",
        params![key.environment_key, key.agent, key.session_id],
    )?;
    Ok(())
}

fn delete_session_in(connection: &Connection, key: &SessionKey) -> Result<bool> {
    let parameters = params![key.environment_key, key.agent, key.session_id];
    connection.execute(
        "DELETE FROM session_relation
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    connection.execute(
        "DELETE FROM session_analysis
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    // The v11 `ON DELETE CASCADE` also covers this row, but the cascade rides
    // on the per-connection `foreign_keys` pragma; deleting explicitly keeps
    // session deletion pragma-independent.
    connection.execute(
        "DELETE FROM session_evidence
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    // Same reasoning as `session_evidence` above: the v15 and v28 cascades
    // from `session` cover `turn`, `turn_content`, and `session_coverage`,
    // but this stays explicit and pragma-independent. `turn_content` has no
    // direct session key, so it is deleted through `turn`'s rowid.
    delete_turn_rows_in(connection, key)?;
    let removed = connection.execute(
        "DELETE FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    Ok(removed > 0)
}

pub(crate) fn apply_session_retention_in(
    connection: &Connection,
    retention_days: i32,
    now_epoch: i64,
) -> Result<usize> {
    if retention_days == RETAIN_SESSION_DATA_FOREVER {
        return Ok(0);
    }

    let cutoff = now_epoch.saturating_sub(i64::from(retention_days).saturating_mul(86_400));
    let mut statement = connection.prepare(
        "SELECT environment_key, agent, session_id
           FROM session
          WHERE COALESCE(
                    updated_at_epoch,
                    CAST(strftime('%s', last_seen_at) AS INTEGER)
                ) < ?1",
    )?;
    let keys = statement
        .query_map([cutoff], |row| {
            Ok(SessionKey::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let mut removed = 0;
    for key in keys {
        if delete_session_in(connection, &key)? {
            removed += 1;
        }
    }
    Ok(removed)
}

fn turn_session_key(key: &SessionKey) -> TurnSessionKey<'_> {
    TurnSessionKey {
        environment_key: &key.environment_key,
        agent: &key.agent,
        session_id: &key.session_id,
    }
}

fn delete_turn_rows_in(connection: &Connection, key: &SessionKey) -> Result<usize> {
    Ok(delete_turn_rows(connection, &turn_session_key(key))?)
}

fn evidence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRow> {
    let status: EvidenceStatus = row
        .get::<_, String>(3)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(EvidenceRow {
        key: SessionKey::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ),
        status,
        analyzed_generation: row.get(4)?,
        processed_fingerprint: row.get(5)?,
        parser_revision: row.get(6)?,
        analyzer_revision: row.get(7)?,
        evidence_schema_revision: row.get(8)?,
        evidence_json: row.get(9)?,
        retry_count: row.get(10)?,
        claim_fence: row.get(11)?,
        claimed_at_epoch: row.get(12)?,
        lease_expires_at_epoch: row.get(13)?,
        next_attempt_at_epoch: row.get(14)?,
        analyzed_at_epoch: row.get(15)?,
        last_error: row.get(16)?,
        published_fence: row.get(17)?,
    })
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        key: SessionKey::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ),
        source_kind: row.get(3)?,
        source_label: row.get(4)?,
        wsl_distro: row.get(5)?,
        title: row.get(6)?,
        title_source: row.get(7)?,
        cwd: row.get(8)?,
        surface: row.get(9)?,
        updated_at_epoch: row.get(10)?,
        activity_cursor: row.get(11)?,
        activity_source: row.get(12)?,
        subagent_count: row.get(13)?,
        fork_parent_session_id: row.get(14)?,
        source_fingerprint: row.get(15)?,
    })
}

/// Writes and reads turn rows for one claimed evidence pass.
///
/// Holds an owned [`Store`] handle (cheap: it shares the app's one
/// connection, see [`Store`]'s doc comment) plus the session key and claim
/// fence the durable worker already knows for this pass. `Store` cannot
/// implement [`TurnRowStore`] directly — reading and writing rows needs the
/// session key and fence, and `Store` itself does not carry either.
#[derive(Clone)]
pub struct FencedTurnRowStore {
    store: Store,
    key: SessionKey,
    claim_fence: i64,
}

impl FencedTurnRowStore {
    pub fn new(store: Store, key: SessionKey, claim_fence: i64) -> Self {
        Self {
            store,
            key,
            claim_fence,
        }
    }
}

impl TurnRowStore for FencedTurnRowStore {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        // One transaction per batch: one fsync for the batch instead of
        // one for each row, and the lock is held only for the batch.
        let mut connection = self.store.lock();
        let transaction = connection.transaction()?;
        insert_turn_rows(
            &transaction,
            &turn_session_key(&self.key),
            self.claim_fence,
            rows,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        let connection = self.store.lock();
        Ok(query_turn_facts(
            &connection,
            &turn_session_key(&self.key),
            self.claim_fence,
        )?)
    }

    fn query_model_breakdown(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, antiburn_local::pricing::ModelTokens>,
        TurnRowError,
    > {
        let connection = self.store.lock();
        Ok(query_model_breakdown(
            &connection,
            &turn_session_key(&self.key),
            self.claim_fence,
        )?)
    }

    fn query_model_runs(&self) -> Result<Vec<ModelRun>, TurnRowError> {
        let connection = self.store.lock();
        Ok(query_model_runs(
            &connection,
            &turn_session_key(&self.key),
            self.claim_fence,
        )?)
    }

    fn write_coverage_record(&self, record: &SessionCoverageRecord) -> Result<(), TurnRowError> {
        let connection = self.store.lock();
        insert_coverage_record(
            &connection,
            &turn_session_key(&self.key),
            self.claim_fence,
            record,
        )?;
        Ok(())
    }

    fn query_coverage_record(&self) -> Result<Option<SessionCoverageRecord>, TurnRowError> {
        let connection = self.store.lock();
        Ok(query_coverage_record(
            &connection,
            &turn_session_key(&self.key),
            self.claim_fence,
        )?)
    }
}
