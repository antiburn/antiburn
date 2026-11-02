//! Preserve live settings and incurred usage before the V87 table removal.

use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::usage_accounting::{USAGE_LEDGER_KEY, UsageLedger, write_json_setting};

pub(super) const VERSION: i64 = 87;
pub(super) const FRESH_INITIALIZATION_KEY: &str = "internal:freshDatabaseInitializationV1";
const RETIRED_KEYS: [&str; 4] = [
    "ignored_instructions",
    "skill_opportunities",
    "over_exploring",
    "scope_creep",
];
const RETIRED_SNOOZES: [&str; 4] = [
    "ignoredInstructions",
    "skillOpportunities",
    "overExploring",
    "scopeCreep",
];

#[derive(Deserialize)]
struct LegacyReservation {
    id: String,
    #[serde(default)]
    settled: bool,
    #[serde(default)]
    unknown_recorded: bool,
}

pub(super) fn prepare(connection: &Connection, now_epoch: i64) -> Result<()> {
    if read_setting(connection, FRESH_INITIALIZATION_KEY)?.as_deref() != Some("true") {
        preserve_enablement(connection)?;
    }
    filter_preferences(connection)?;
    filter_snoozes(connection)?;
    settle_dispatched_usage(connection, now_epoch)?;
    connection.execute(
        "DELETE FROM setting WHERE key = ?1",
        [FRESH_INITIALIZATION_KEY],
    )?;
    Ok(())
}

fn read_setting(connection: &Connection, key: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row("SELECT value FROM setting WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

fn write_setting(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn preserve_enablement(connection: &Connection) -> Result<()> {
    let legacy_enabled = read_setting(connection, "internal:burnChecksEnabledAtEpochV1")?.is_some();
    let yaml_key = "internal:yamlBurnChecksDiscoveryEnabledV1";
    let enabled = match read_setting(connection, yaml_key)?.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => legacy_enabled,
    };
    write_setting(connection, yaml_key, if enabled { "true" } else { "false" })?;
    if legacy_enabled {
        write_setting(connection, "internal:typesafeKeySavedV1", "true")?;
    }
    Ok(())
}

fn filter_preferences(connection: &Connection) -> Result<()> {
    let key = "internal:checkPreferencesV1";
    let Some(raw) = read_setting(connection, key)? else {
        return Ok(());
    };
    let mut value: Value = serde_json::from_str(&raw).context("invalid check preferences")?;
    let revision = value
        .get("revision")
        .and_then(Value::as_u64)
        .context("invalid check preference revision")?;
    let checks = value
        .get_mut("checks")
        .and_then(Value::as_object_mut)
        .context("invalid check preference choices")?;
    ensure!(
        checks.values().all(Value::is_boolean),
        "invalid check preference choice"
    );
    let mut changed = false;
    for retired in RETIRED_KEYS {
        changed |= checks.remove(retired).is_some();
    }
    if changed {
        value["revision"] = Value::from(revision.saturating_add(1));
        write_json_setting(connection, key, &value)?;
    }
    Ok(())
}

fn filter_snoozes(connection: &Connection) -> Result<()> {
    let key = "internal:burnCheckSnoozesV1";
    let Some(raw) = read_setting(connection, key)? else {
        return Ok(());
    };
    let mut records: Vec<Value> =
        serde_json::from_str(&raw).context("invalid native check snoozes")?;
    let previous_len = records.len();
    records.retain(|record| {
        !record
            .get("detector")
            .and_then(Value::as_str)
            .is_some_and(|detector| RETIRED_SNOOZES.contains(&detector))
    });
    if records.len() != previous_len {
        write_json_setting(connection, key, &records)?;
    }
    Ok(())
}

fn settle_dispatched_usage(connection: &Connection, now_epoch: i64) -> Result<()> {
    let raw = read_setting(connection, USAGE_LEDGER_KEY)?;
    let mut value: Value = raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("invalid check usage ledger")?
        .unwrap_or_else(|| json!({"summary": {}}));
    let ledger: UsageLedger =
        serde_json::from_value(value.clone()).context("invalid check usage totals")?;
    let mut statement =
        connection.prepare("SELECT DISTINCT reservation_id FROM burn_check_request_outcome")?;
    let mut unresolved = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let mut statement = connection.prepare(
        "SELECT r.id, r.data FROM burn_check_usage_reservation AS r
         WHERE EXISTS (SELECT 1 FROM burn_check_request_outcome AS o WHERE o.reservation_id = r.id)",
    )?;
    let mut new_unknown = 0_u64;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, data) = row?;
        let reservation: LegacyReservation =
            serde_json::from_str(&data).context("invalid dispatched check usage reservation")?;
        ensure!(
            reservation.id == id,
            "check usage reservation identity mismatch"
        );
        unresolved.remove(&id);
        if !reservation.settled && !reservation.unknown_recorded {
            new_unknown = new_unknown.saturating_add(1);
        }
    }
    let embedded = match value.get("reservations") {
        None => Vec::new(),
        Some(reservations) => {
            serde_json::from_value::<Vec<LegacyReservation>>(reservations.clone())
                .context("invalid embedded check usage reservations")?
        }
    };
    for reservation in embedded {
        // The indexed row owns any newer settlement flags for the same ID.
        if unresolved.remove(&reservation.id)
            && !reservation.settled
            && !reservation.unknown_recorded
        {
            new_unknown = new_unknown.saturating_add(1);
        }
    }
    let object = value
        .as_object_mut()
        .context("invalid check usage ledger object")?;
    let removed_reservations = object.remove("reservations").is_some();
    if new_unknown > 0 {
        let summary = object
            .entry("summary")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("invalid check usage summary")?;
        summary.insert(
            "unknown_outcomes".into(),
            Value::from(ledger.summary.unknown_outcomes.saturating_add(new_unknown)),
        );
        summary.insert("last_unknown_at_epoch".into(), Value::from(now_epoch));
    }
    if new_unknown > 0 || (raw.is_some() && removed_reservations) {
        write_json_setting(connection, USAGE_LEDGER_KEY, &value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
