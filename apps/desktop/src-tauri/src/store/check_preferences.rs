use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::checks::DetectorId;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::Store;

const PREFERENCES_KEY: &str = "internal:checkPreferencesV1";

/// Checks shipped before individual choices existed keep their old behavior.
/// Adding a detector to `DetectorId` must not add it here: new checks are opt in.
const LEGACY_LOCAL_CHECKS: [DetectorId; 9] = [
    DetectorId::SessionsOverDepth,
    DetectorId::ModelOverthinking,
    DetectorId::OverpoweredSubagents,
    DetectorId::UnusedMcpServers,
    DetectorId::UnusedBuiltInTools,
    DetectorId::UnusedSkills,
    DetectorId::OldModelUsage,
    DetectorId::OveruseOfFastMode,
    DetectorId::CacheChurn,
];

#[derive(Debug, Default, Deserialize, Serialize)]
struct CheckPreferences {
    revision: u64,
    checks: BTreeMap<String, bool>,
}

impl Store {
    /// Return enabled detector IDs. Unknown stored IDs remain intact for newer apps.
    pub fn enabled_checks(&self) -> Result<BTreeSet<DetectorId>> {
        let connection = self.lock();
        enabled_checks_in(&connection)
    }

    pub fn check_preferences_snapshot(&self) -> Result<(BTreeSet<DetectorId>, u64)> {
        let connection = self.lock();
        check_preferences_snapshot_in(&connection)
    }

    pub fn check_enabled(&self, detector: DetectorId) -> Result<bool> {
        let connection = self.lock();
        let preferences = read_or_migrate(&connection)?;
        Ok(preferences
            .checks
            .get(detector.key())
            .copied()
            .unwrap_or(false))
    }

    pub fn check_preferences_revision(&self) -> Result<u64> {
        let connection = self.lock();
        Ok(read_or_migrate(&connection)?.revision)
    }

    /// Change one choice against the latest stored document.
    ///
    /// The transaction preserves unrelated and unknown detector IDs. The bool
    /// reports an actual saved transition so callers can avoid duplicate events.
    pub fn set_check_enabled(&self, detector: DetectorId, enabled: bool) -> Result<bool> {
        self.set_check_enabled_with_smart_transition(detector, enabled, false, 0)
    }

    pub fn set_check_enabled_with_smart_transition(
        &self,
        detector: DetectorId,
        enabled: bool,
        smart: bool,
        now_epoch: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut preferences = read_or_migrate(&transaction)?;
        if preferences
            .checks
            .get(detector.key())
            .copied()
            .unwrap_or(false)
            == enabled
        {
            transaction.commit()?;
            return Ok(false);
        }
        if smart {
            if enabled {
                super::burn_check::capture_burn_check_boundaries_in(
                    &transaction,
                    &[detector.key()],
                    now_epoch,
                    false,
                )?;
            } else {
                super::burn_check::cancel_burn_check_in(&transaction, detector.key())?;
            }
        }
        preferences
            .checks
            .insert(detector.key().to_owned(), enabled);
        preferences.revision = preferences.revision.saturating_add(1);
        write_preferences(&transaction, &preferences)?;
        if enabled {
            transaction.execute(
                "UPDATE remediation
                    SET dirty_revision = dirty_revision + 1,
                        updated_at_epoch = MAX(updated_at_epoch, ?2)
                  WHERE state IN ('watching', 'fixed', 'recurred')
                    AND json_extract(definition_json, '$.detector') = ?1",
                params![detector.key(), now_epoch.max(0)],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }
}

pub(crate) fn enabled_checks_in(connection: &Connection) -> Result<BTreeSet<DetectorId>> {
    Ok(check_preferences_snapshot_in(connection)?.0)
}

pub(crate) fn check_preferences_snapshot_in(
    connection: &Connection,
) -> Result<(BTreeSet<DetectorId>, u64)> {
    let preferences = read_or_migrate(connection)?;
    let enabled = DetectorId::ALL
        .into_iter()
        .filter(|detector| {
            preferences
                .checks
                .get(detector.key())
                .copied()
                .unwrap_or(false)
        })
        .collect();
    Ok((enabled, preferences.revision))
}

fn read_or_migrate(connection: &Connection) -> Result<CheckPreferences> {
    let stored = connection
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            [PREFERENCES_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(stored) = stored {
        return serde_json::from_str(&stored).context("invalid check preferences");
    }

    let mut checks = LEGACY_LOCAL_CHECKS
        .into_iter()
        .map(|detector| (detector.key().to_owned(), true))
        .collect::<BTreeMap<_, _>>();
    checks.insert(DetectorId::IgnoredInstructions.key().to_owned(), true);
    let preferences = CheckPreferences {
        revision: 0,
        checks,
    };
    write_preferences(connection, &preferences)?;
    Ok(preferences)
}

fn write_preferences(connection: &Connection, preferences: &CheckPreferences) -> Result<()> {
    connection.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![PREFERENCES_KEY, serde_json::to_string(preferences)?],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn store() -> Store {
        Store::open_in_memory(Path::new("/tmp/antiburn-check-preferences-test"))
            .expect("in-memory store")
    }

    #[test]
    fn migration_keeps_every_existing_check_on() {
        let store = store();
        let enabled = store.enabled_checks().unwrap();
        let expected = LEGACY_LOCAL_CHECKS
            .into_iter()
            .chain([DetectorId::IgnoredInstructions])
            .collect::<BTreeSet<_>>();
        assert_eq!(enabled, expected);
        assert_eq!(store.check_preferences_revision().unwrap(), 0);
    }

    #[test]
    fn individual_updates_preserve_unrelated_and_unknown_choices() {
        let store = store();
        store.set_internal_value(
            PREFERENCES_KEY,
            r#"{"revision":7,"checks":{"future_check":true,"cache_churn":true}}"#,
        );

        assert!(
            store
                .set_check_enabled(DetectorId::CacheChurn, false)
                .unwrap()
        );
        assert!(
            !store
                .set_check_enabled(DetectorId::CacheChurn, false)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 8);
        let stored: CheckPreferences =
            serde_json::from_str(&store.internal_value(PREFERENCES_KEY).expect("preferences"))
                .unwrap();
        assert_eq!(stored.checks.get("future_check"), Some(&true));
        assert_eq!(stored.checks.get("cache_churn"), Some(&false));
    }

    #[test]
    fn repeated_choice_is_a_no_op_without_a_revision_change() {
        let store = store();

        assert_eq!(store.check_preferences_revision().unwrap(), 0);
        assert!(
            !store
                .set_check_enabled(DetectorId::IgnoredInstructions, true)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 0);

        assert!(
            store
                .set_check_enabled(DetectorId::IgnoredInstructions, false)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 1);
        assert!(
            !store
                .set_check_enabled(DetectorId::IgnoredInstructions, false)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 1);
    }

    #[test]
    fn unknown_check_ids_fail_closed_at_execution_boundaries() {
        let store = store();
        assert!(
            store
                .burn_check_candidates("future_unregistered_check", 10_000, 180, 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .capture_burn_check_boundaries(&["future_unregistered_check"], 10_000)
                .is_err()
        );
    }

    #[test]
    fn smart_disable_cancels_only_the_selected_checks_active_assessments() {
        let store = store();
        let connection = store.lock();
        for (check_id, status) in [
            ("ignored_instructions", "queued"),
            ("ignored_instructions", "running"),
            ("cache_churn", "queued"),
            ("cache_churn", "running"),
        ] {
            let session_id = format!("session-{check_id}-{status}");
            connection
                .execute(
                    "INSERT INTO session (
                         environment_key, agent, session_id, source_kind, source_label,
                         first_seen_at, last_seen_at)
                     VALUES ('native', 'claude-code', ?1, 'file', ?2,
                             '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
                    rusqlite::params![session_id, format!("/tmp/{check_id}-{status}.jsonl")],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO burn_check_assessment (
                         environment_key, agent, session_id, check_id, incarnation,
                         status, created_at_epoch, updated_at_epoch)
                     VALUES ('native', 'claude-code', ?1, ?2, 0, ?3, 1, 1)",
                    rusqlite::params![session_id, check_id, status],
                )
                .unwrap();
        }
        drop(connection);

        assert!(
            store
                .set_check_enabled_with_smart_transition(
                    DetectorId::IgnoredInstructions,
                    false,
                    true,
                    10,
                )
                .unwrap()
        );

        let connection = store.lock();
        let statuses = connection
            .prepare(
                "SELECT check_id, status FROM burn_check_assessment
                 ORDER BY check_id, status",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            statuses,
            vec![
                ("cache_churn".to_owned(), "queued".to_owned()),
                ("cache_churn".to_owned(), "running".to_owned()),
                ("ignored_instructions".to_owned(), "superseded".to_owned()),
                ("ignored_instructions".to_owned(), "superseded".to_owned()),
            ]
        );
        drop(connection);
        assert!(
            !store
                .check_enabled(DetectorId::IgnoredInstructions)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 1);
    }

    #[test]
    fn enabling_a_smart_check_while_paused_refreshes_its_future_boundary() {
        let store = store();
        store
            .lock()
            .execute(
                "INSERT INTO session (
                     environment_key, agent, session_id, source_kind, source_label,
                     first_seen_at, last_seen_at, source_generation, activity_cursor)
                 VALUES ('native', 'claude-code', 'paused-enable', 'file', '/tmp/session.jsonl',
                         '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 1, 'before')",
                [],
            )
            .unwrap();
        store
            .capture_burn_check_boundaries(&[DetectorId::IgnoredInstructions.key()], 10)
            .unwrap();
        store
            .lock()
            .execute(
                "UPDATE burn_check_assessment
                    SET status = 'running', input_revision = 'old-input'
                  WHERE session_id = 'paused-enable'",
                [],
            )
            .unwrap();
        assert!(
            store
                .set_check_enabled_with_smart_transition(
                    DetectorId::IgnoredInstructions,
                    false,
                    true,
                    20,
                )
                .unwrap()
        );
        store.disable_burn_checks().unwrap();
        store
            .lock()
            .execute(
                "UPDATE session SET source_generation = 2, activity_cursor = 'while-paused'
                  WHERE session_id = 'paused-enable'",
                [],
            )
            .unwrap();

        assert!(
            store
                .set_check_enabled_with_smart_transition(
                    DetectorId::IgnoredInstructions,
                    true,
                    true,
                    30,
                )
                .unwrap()
        );
        assert!(
            store
                .internal_value("internal:burnChecksEnabledAtEpochV1")
                .is_none()
        );
        let paused_boundary: (i64, String, String, Option<String>) = store
            .lock()
            .query_row(
                "SELECT boundary_generation, boundary_activity_cursor, status, input_revision
                   FROM burn_check_assessment WHERE session_id = 'paused-enable'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            paused_boundary,
            (2, "while-paused".to_owned(), "idle".to_owned(), None)
        );

        store
            .capture_burn_check_boundaries(&[DetectorId::IgnoredInstructions.key()], 40)
            .unwrap();
        assert!(
            store
                .internal_value("internal:burnChecksEnabledAtEpochV1")
                .is_some()
        );
        let resumed_boundary: i64 = store
            .lock()
            .query_row(
                "SELECT boundary_at_epoch FROM burn_check_assessment
                  WHERE session_id = 'paused-enable'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(resumed_boundary, 40);
    }
}
