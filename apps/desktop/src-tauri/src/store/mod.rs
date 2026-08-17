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
//! # What is *not* here
//!
//! Transcript bodies. No message text, no tool arguments, no file contents. The
//! database stores identities, locations, derived numbers, and two short
//! derived excerpts — a session's title and each skill's one-line description,
//! both length-capped before they are written. A session's messages stay in the
//! file its vendor wrote and are re-read on demand. [`schema`] states that as a
//! schema-level contract, and [`crate::export`] honors the same rule.
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

pub use model::{
    AnalysisRecord, AppSettings, DiskSpaceDisplay, MAX_ACTIVITY_DAYS, MIN_ACTIVITY_DAYS,
    Milestones, NudgePlacement, RelationKind, RelationRecord, RepositoryRecord, SessionKey,
    SessionRecord, ThemePreference, UsageEvidenceRecord,
};

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
            tx.execute_batch(sql)
                .with_context(|| format!("migration {version} failed"))?;
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
        let settings = AppSettings {
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
            notify_usage_anomalies: stored
                .get("notifyUsageAnomalies")
                .map(|value| value == "true")
                .unwrap_or(defaults.notify_usage_anomalies),
            milestones_5h: stored
                .get("milestones5h")
                .map(|value| Milestones::parse(value))
                .unwrap_or(defaults.milestones_5h),
            milestones_weekly: stored
                .get("milestonesWeekly")
                .map(|value| Milestones::parse(value))
                .unwrap_or(defaults.milestones_weekly),
            live_usage_enabled: stored
                .get("liveUsageEnabled")
                .map(|value| value == "true")
                .unwrap_or(defaults.live_usage_enabled),
        };
        Ok(settings.normalized())
    }

    /// Replace every preference, returning what was actually stored (clamped).
    pub fn save_settings(&self, settings: &AppSettings) -> Result<AppSettings> {
        let settings = settings.clone().normalized();
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        {
            let mut put = tx.prepare(
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
                "notifyUsageAnomalies",
                bool_text(settings.notify_usage_anomalies)
            ])?;
            put.execute(params!["milestones5h", settings.milestones_5h.as_str()])?;
            put.execute(params![
                "milestonesWeekly",
                settings.milestones_weekly.as_str()
            ])?;
            put.execute(params![
                "liveUsageEnabled",
                bool_text(settings.live_usage_enabled)
            ])?;
        }
        tx.commit()?;
        Ok(settings)
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
     * Sessions
     * ----------------------------------------------------------------- */

    /// Insert or refresh a batch of sessions in one transaction.
    ///
    /// `first_seen_at` survives a rescan; everything else is replaced with what
    /// the scan just observed, so a renamed session picks up its new title
    /// without producing a second row.
    pub fn upsert_sessions(&self, records: &[SessionRecord]) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        for record in records {
            upsert_session_in(&tx, record)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sessions whose heartbeat falls at or after `since_epoch`, newest first.
    pub fn recent_sessions(&self, since_epoch: i64, limit: usize) -> Result<Vec<SessionRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, session_id, source_kind, source_label, wsl_distro,
                    title, title_source, cwd, surface, updated_at_epoch, subagent_count,
                    (SELECT related_id FROM session_relation r
                       WHERE r.environment_key = s.environment_key
                         AND r.agent = s.agent
                         AND r.session_id = s.session_id
                         AND r.kind = 'forkParent'
                       LIMIT 1)
               FROM session s
              WHERE COALESCE(updated_at_epoch, 0) >= ?1
              ORDER BY COALESCE(updated_at_epoch, 0) DESC, session_id DESC
              LIMIT ?2",
        )?;
        let rows = statement.query_map(params![since_epoch, limit as i64], session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One session's cached metadata, when it has been seen.
    pub fn session(&self, key: &SessionKey) -> Result<Option<SessionRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, session_id, source_kind, source_label, wsl_distro,
                    title, title_source, cwd, surface, updated_at_epoch, subagent_count,
                    (SELECT related_id FROM session_relation r
                       WHERE r.environment_key = s.environment_key
                         AND r.agent = s.agent
                         AND r.session_id = s.session_id
                         AND r.kind = 'forkParent'
                       LIMIT 1)
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

    /// Drop sessions whose heartbeat predates `before_epoch` and everything
    /// derived from them.
    ///
    /// Retention, not deletion-on-behalf: these rows describe transcripts that
    /// have aged out of every window the app can show. The provider's own files
    /// are untouched.
    pub fn prune_sessions_before(&self, before_epoch: i64) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let stale: Vec<(String, String, String)> = {
            let mut statement = tx.prepare(
                "SELECT environment_key, agent, session_id FROM session
                  WHERE COALESCE(updated_at_epoch, 0) < ?1",
            )?;
            let rows = statement.query_map(params![before_epoch], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (environment_key, agent, session_id) in &stale {
            delete_session_in(&tx, &SessionKey::new(environment_key, agent, session_id))?;
        }
        tx.commit()?;
        Ok(stale.len())
    }

    /// Forget the entire derived index: every session, its analysis, its
    /// relations, and the per-agent scan bookkeeping. Returns how many sessions
    /// were dropped.
    ///
    /// **antiburn's own tables only.** Not one provider file is opened, let
    /// alone written — the transcripts this index was derived *from* are the
    /// agents' files and stay exactly where they are, which is why a later scan
    /// finds all of it again.
    ///
    /// Deliberately spared, because none of it is derived data:
    ///
    /// - `setting` — the reader's preferences, including whether onboarding is
    ///   done. Clearing the index is not a factory reset.
    /// - `scan_root` — the folders the reader pointed the scanner at.
    /// - `repository` — the include/ignore choices the reader made. Their
    ///   session counts *are* derived, so those are zeroed here and refilled by
    ///   the next pass.
    pub fn clear_derived_index(&self) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM session_relation", [])?;
        tx.execute("DELETE FROM session_analysis", [])?;
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

    /// Cache one session's derived analysis.
    pub fn save_analysis(&self, record: &AnalysisRecord) -> Result<()> {
        self.lock().execute(
            "INSERT INTO session_analysis (
                 environment_key, agent, session_id, metrics_json, cost_json,
                 model_breakdown_json, active_secs, duration_secs, pattern_score,
                 source_fingerprint, pricing_generation, analyzed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 metrics_json = excluded.metrics_json,
                 cost_json = excluded.cost_json,
                 model_breakdown_json = excluded.model_breakdown_json,
                 active_secs = excluded.active_secs,
                 duration_secs = excluded.duration_secs,
                 pattern_score = excluded.pattern_score,
                 source_fingerprint = excluded.source_fingerprint,
                 pricing_generation = excluded.pricing_generation,
                 analyzed_at = excluded.analyzed_at",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                record.metrics_json,
                record.cost_json,
                record.model_breakdown_json,
                record.active_secs,
                record.duration_secs,
                record.pattern_score,
                record.source_fingerprint,
                record.pricing_generation,
                now_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// One session's cached analysis, when it has been computed.
    pub fn analysis(&self, key: &SessionKey) -> Result<Option<AnalysisRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, session_id, metrics_json, cost_json,
                    model_breakdown_json, active_secs, duration_secs, pattern_score,
                    source_fingerprint, pricing_generation
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
                        metrics_json: row.get(3)?,
                        cost_json: row.get(4)?,
                        model_breakdown_json: row.get(5)?,
                        active_secs: row.get(6)?,
                        duration_secs: row.get(7)?,
                        pattern_score: row.get(8)?,
                        source_fingerprint: row.get(9)?,
                        pricing_generation: row.get(10)?,
                    })
                },
            )
            .optional()?)
    }

    /// Agent, heartbeat, and token breakdown for every session at or after
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
        tx.execute(
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
            tx.execute(
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

fn upsert_session_in(connection: &Connection, record: &SessionRecord) -> Result<()> {
    let now = now_rfc3339();
    connection.execute(
        "INSERT INTO session (
             environment_key, agent, session_id, source_kind, source_label, wsl_distro,
             title, title_source, cwd, surface, updated_at_epoch, subagent_count,
             first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
         ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
             source_kind = excluded.source_kind,
             source_label = excluded.source_label,
             wsl_distro = excluded.wsl_distro,
             title = COALESCE(excluded.title, session.title),
             title_source = COALESCE(excluded.title_source, session.title_source),
             cwd = COALESCE(excluded.cwd, session.cwd),
             surface = excluded.surface,
             updated_at_epoch = MAX(COALESCE(excluded.updated_at_epoch, 0),
                                    COALESCE(session.updated_at_epoch, 0)),
             subagent_count = excluded.subagent_count,
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
            record.subagent_count,
            now,
        ],
    )?;

    // A fork parent is written only when the caller actually observed one.
    //
    // Absence is *not* evidence: the scan never reads transcripts deep enough
    // to see a lineage header, so it always passes `None`, and treating that as
    // "no parent" would erase a relation resolved when the session was opened
    // on every subsequent pass. Lineage is a property of a transcript, and a
    // transcript's lineage does not change.
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
        subagent_count: row.get(11)?,
        fork_parent_session_id: row.get(12)?,
    })
}
