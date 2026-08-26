// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
//! app-controlled and on-device, and clear/delete paths apply to it. Provider
//! source transcripts are never modified or deleted. [`schema`] carries the
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
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::dto::DeferredPermissionDir;

pub use model::{
    AnalysisRecord, AppSettings, DiskSpaceDisplay, EvidenceClaim, EvidenceCompletion,
    EvidenceFailure, EvidenceRow, EvidenceStatus, MAX_ACTIVITY_DAYS, MILESTONE_OPTIONS,
    MIN_ACTIVITY_DAYS, Milestones, NudgePlacement, ProjectionRevisions, PublishedEvidence,
    RelationKind, RelationRecord, RepositoryRecord, SessionActivityKey, SessionActivityState,
    SessionKey, SessionRecord, SourceVersionState, ThemePreference, UsageEvidenceRecord,
};

/// Internal-scalar key holding the protected directories the last pass declined
/// to read.
pub const DEFERRED_PERMISSION_DIRS_KEY: &str = "internal:deferredPermissionDirs";

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

/// The app's local database.
pub struct Store {
    connection: Mutex<Connection>,
    /// The directory the engine's own state files (scan roots, ignored paths)
    /// live in. The engine never chooses this; the shell does.
    state_dir: PathBuf,
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

    fn from_connection(connection: Connection, state_dir: PathBuf) -> Result<Store> {
        // WAL keeps a read during a scan write from blocking, and `NORMAL` is
        // the documented companion: a crash can lose the last commit, which for
        // a rebuildable cache is cheaper than an fsync per transaction.
        connection.pragma_update(None, "journal_mode", "WAL").ok();
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let store = Store {
            connection: Mutex::new(connection),
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
        self.update_settings(|current| *current = settings.clone())
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

    /* -----------------------------------------------------------------
     * Anonymised application events (D-027, deviations D-28)
     * ----------------------------------------------------------------- */

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
            let (previous_generation, source_generation) = upsert_session_in(&tx, record)?;
            let generation_increased =
                previous_generation.is_none_or(|previous| source_generation > previous);
            if evidence_agents.contains(&record.key.agent.as_str())
                && (generation_increased || source_returned)
            {
                mark_evidence_pending_in(&tx, &record.key)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Sessions whose activity falls at or after `since_epoch`, newest first.
    pub fn recent_sessions(&self, since_epoch: i64, limit: usize) -> Result<Vec<SessionRecord>> {
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
              WHERE COALESCE(updated_at_epoch, 0) >= ?1
              ORDER BY COALESCE(updated_at_epoch, 0) DESC, session_id DESC
              LIMIT ?2",
        )?;
        let rows = statement.query_map(params![since_epoch, limit as i64], session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return the persisted activity cursor keyed by environment,
    /// agent, and source label. A single map keeps the scan's cheap size gate
    /// outside the SQLite lock without allowing native/WSL rows to collide.
    pub fn session_activity_states(
        &self,
    ) -> Result<HashMap<SessionActivityKey, SessionActivityState>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, source_label, activity_cursor,
                    updated_at_epoch, activity_source
               FROM session",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                SessionActivityKey::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ),
                SessionActivityState {
                    activity_cursor: row.get(3)?,
                    updated_at_epoch: row.get(4)?,
                    activity_source: row.get(5)?,
                },
            ))
        })?;
        let mut states = HashMap::new();
        for row in rows {
            let (key, state) = row?;
            states.insert(key, state);
        }
        Ok(states)
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
                "SELECT environment_key, agent, session_id, status,
                        analyzed_generation, processed_fingerprint,
                        parser_revision, analyzer_revision, evidence_schema_revision,
                        evidence_json, diagnostics_json, retry_count, claim_fence,
                        claimed_at_epoch, lease_expires_at_epoch,
                        next_attempt_at_epoch, analyzed_at_epoch, last_error
                   FROM session_evidence
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![key.environment_key, key.agent, key.session_id],
                evidence_from_row,
            )
            .optional()?)
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
                        evidence_json = NULL, diagnostics_json = NULL,
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
    ///   done. Clearing the index is not a factory reset.
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
        let sessions = tx.execute("DELETE FROM session", [])?;
        tx.execute("DELETE FROM scan_state", [])?;
        tx.execute("UPDATE repository SET session_count = 0", [])?;
        tx.commit()?;
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
                 inclusive_models_json, source_fingerprint, pricing_generation,
                 analyzed_generation, parser_revision, analyzer_revision,
                 metrics_schema_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 model_breakdown_json = excluded.model_breakdown_json,
                 inclusive_models_json = excluded.inclusive_models_json,
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
                    evidence_json = ?10, diagnostics_json = ?11,
                    analyzed_at_epoch = ?12, retry_count = 0, last_error = NULL,
                    claimed_at_epoch = NULL, lease_expires_at_epoch = NULL,
                    next_attempt_at_epoch = NULL
              WHERE evidence.environment_key = ?1
                AND evidence.agent = ?2 AND evidence.session_id = ?3
                AND evidence.status = 'processing' AND evidence.claim_fence = ?13
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
                completion.diagnostics_json,
                time::OffsetDateTime::now_utc().unix_timestamp(),
                completion.claim_fence,
            ],
        )?;
        if updated == 0 {
            return Ok(false);
        }
        replace_relations_in(&transaction, &record.key, RelationKind::Subagent, relations)?;
        transaction.commit()?;
        Ok(true)
    }

    /// Cache one session's derived analysis.
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
                 inclusive_models_json, source_fingerprint, pricing_generation,
                 analyzed_generation, parser_revision, analyzer_revision,
                 metrics_schema_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 model_breakdown_json = excluded.model_breakdown_json,
                 inclusive_models_json = excluded.inclusive_models_json,
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
                    inclusive_models_json, source_fingerprint, pricing_generation,
                    analyzed_generation, parser_revision, analyzer_revision,
                    metrics_schema_revision
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
                        source_fingerprint: row.get(5)?,
                        pricing_generation: row.get(6)?,
                        analyzed_generation: row.get(7)?,
                        parser_revision: row.get(8)?,
                        analyzer_revision: row.get(9)?,
                        metrics_schema_revision: row.get(10)?,
                    })
                },
            )
            .optional()?)
    }

    /// Agent, activity timestamp, and token breakdown for every session at or after
    /// `since_epoch`.
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
            "SELECT s.agent, COALESCE(s.updated_at_epoch, 0), a.model_breakdown_json
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
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
        // No stored answer means this database predates the setting. A fresh
        // install takes the default and meets the control on the Ready screen;
        // one that already finished onboarding was told analytics did not
        // exist, so it stays off until the reader says otherwise. Upgrading
        // must never start sending on somebody's behalf.
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
        "analyticsEnabled",
        bool_text(settings.analytics_enabled)
    ])?;
    put.execute(params![
        "overviewLimitsExpanded",
        bool_text(settings.overview_limits_expanded)
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
) -> Result<(Option<i64>, i64)> {
    let previous_generation = connection
        .query_row(
            "SELECT source_generation FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
            |row| row.get(0),
        )
        .optional()?;
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
    Ok((previous_generation, source_generation))
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
    let removed = connection.execute(
        "DELETE FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    Ok(removed > 0)
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
        diagnostics_json: row.get(10)?,
        retry_count: row.get(11)?,
        claim_fence: row.get(12)?,
        claimed_at_epoch: row.get(13)?,
        lease_expires_at_epoch: row.get(14)?,
        next_attempt_at_epoch: row.get(15)?,
        analyzed_at_epoch: row.get(16)?,
        last_error: row.get(17)?,
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
