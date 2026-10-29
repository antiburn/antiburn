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
    /// The result identifies a saved transition, not a repeated request.
    pub fn set_check_enabled(&self, detector: DetectorId, enabled: bool) -> Result<bool> {
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
        preferences
            .checks
            .insert(detector.key().to_owned(), enabled);
        preferences.revision = preferences.revision.saturating_add(1);
        write_preferences(&transaction, &preferences)?;
        if enabled {
            transaction.execute(
                "UPDATE remediation SET dirty_revision = dirty_revision + 1,
                     updated_at_epoch = MAX(updated_at_epoch, ?2)
                 WHERE state IN ('watching', 'fixed', 'recurred')
                   AND json_extract(definition_json, '$.detector') = ?1",
                params![
                    detector.key(),
                    time::OffsetDateTime::now_utc().unix_timestamp().max(0)
                ],
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
    let preferences = CheckPreferences {
        revision: 0,
        checks: LEGACY_LOCAL_CHECKS
            .into_iter()
            .map(|detector| (detector.key().to_owned(), true))
            .collect(),
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
    use super::*;

    fn store() -> Store {
        Store::open_in_memory(std::path::Path::new("check-preferences-test")).unwrap()
    }

    #[test]
    fn migration_keeps_every_deterministic_check_on() {
        let store = store();
        assert_eq!(
            store.enabled_checks().unwrap(),
            LEGACY_LOCAL_CHECKS.into_iter().collect()
        );
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
            serde_json::from_str(&store.internal_value(PREFERENCES_KEY).unwrap()).unwrap();
        assert_eq!(stored.checks.get("future_check"), Some(&true));
        assert_eq!(stored.checks.get("cache_churn"), Some(&false));
    }

    #[test]
    fn repeated_choice_is_a_no_op_without_a_revision_change() {
        let store = store();
        assert!(
            !store
                .set_check_enabled(DetectorId::UnusedSkills, true)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 0);
        assert!(
            store
                .set_check_enabled(DetectorId::UnusedSkills, false)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 1);
        assert!(
            !store
                .set_check_enabled(DetectorId::UnusedSkills, false)
                .unwrap()
        );
        assert_eq!(store.check_preferences_revision().unwrap(), 1);
    }
}
