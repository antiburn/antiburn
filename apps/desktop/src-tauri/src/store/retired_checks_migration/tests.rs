use std::collections::BTreeMap;

use antiburn_checks::decision::DecisionAttempt;
use rusqlite::types::Value as SqlValue;
use tempfile::TempDir;

use super::*;
use crate::store::{SessionKey, Store, database_path, schema};

const RETIRED_TABLES: [&str; 9] = [
    "burn_check_assessment",
    "burn_check_dispatch_attempt",
    "burn_check_sampled_pair",
    "burn_check_sample_origin",
    "burn_check_instruction_epoch",
    "burn_check_work_answer",
    "burn_check_response_cache",
    "burn_check_request_outcome",
    "burn_check_usage_reservation",
];

fn legacy(version: usize) -> (TempDir, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(database_path(directory.path())).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    for sql in &schema::MIGRATIONS[..version] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .pragma_update(None, "user_version", version)
        .unwrap();
    (directory, connection)
}

fn upgrade(directory: &TempDir, connection: Connection) -> Store {
    Store::from_connection(connection, directory.path().to_path_buf()).unwrap()
}

fn rows(connection: &Connection, table: &str) -> Vec<Vec<SqlValue>> {
    connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap()
        .query_map([], |row| {
            (0..row.as_ref().column_count())
                .map(|index| row.get(index))
                .collect()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn table_exists(connection: &Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .unwrap()
}

fn reservation(id: &str, settled: bool, unknown: bool) -> Value {
    json!({"id": id, "session_key": "session-digest", "provider": "typesafe-systemone",
        "check_id": "ignored_instructions", "model": "jev-1.13.0", "input_tokens": u64::MAX,
        "expires_at_epoch": 1, "settled": settled, "unknown_recorded": unknown})
}

fn save_reservation(
    connection: &Connection,
    id: &str,
    settled: bool,
    unknown: bool,
    dispatched: bool,
) {
    connection
        .execute(
            "INSERT INTO burn_check_usage_reservation VALUES (?1, 1, 'session-digest', ?2)",
            params![id, reservation(id, settled, unknown).to_string()],
        )
        .unwrap();
    if dispatched {
        connection
            .execute(
                "INSERT INTO burn_check_request_outcome VALUES (?1, ?2, 1)",
                params![format!("request-{id}"), id],
            )
            .unwrap();
    }
}

fn saved_ledger(unknown: u64) -> Value {
    json!({"reservations": [], "summary": {
        "aggregates": [{"provider": "typesafe-systemone", "check_id": "ignored_instructions",
            "model": "jev-1.13.0", "price_version": "typesafe-model-catalog-2026-09-25",
            "input_tokens": u64::MAX, "output_tokens": 7, "confirmed_calls": 4,
            "cache_hits": 2, "estimated_cost_nanos": u64::MAX, "last_used_at_epoch": 50}],
        "unknown_outcomes": unknown, "cache_hits": 2, "last_unknown_at_epoch": 60,
        "unpriced_usage": true, "overflow_input_tokens": u64::MAX,
        "overflow_output_tokens": u64::MAX, "overflow_confirmed_calls": u64::MAX,
        "future_field": {"preserved": true}}})
}

#[test]
fn fresh_disk_store_keeps_discovery_enabled_without_a_saved_preference_or_key() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(crate::jev::settings::yaml_discovery_enabled(&store));
    assert!(store.internal_value(FRESH_INITIALIZATION_KEY).is_none());
    assert!(
        store
            .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
            .is_none()
    );
    assert!(
        store
            .internal_value("internal:typesafeKeySavedV1")
            .is_none()
    );
    for table in RETIRED_TABLES {
        assert!(!table_exists(&store.lock(), table), "{table}");
    }
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    assert!(crate::jev::settings::yaml_discovery_enabled(&reopened));
    assert!(reopened.internal_value(FRESH_INITIALIZATION_KEY).is_none());
    assert!(
        reopened
            .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
            .is_none()
    );
    assert!(
        reopened
            .internal_value("internal:typesafeKeySavedV1")
            .is_none()
    );
}

#[test]
fn explicit_yaml_disablement_survives_disk_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "false")
        .unwrap();
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    assert!(!crate::jev::settings::yaml_discovery_enabled(&reopened));
    assert_eq!(
        reopened
            .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
            .as_deref(),
        Some("false")
    );
    assert!(
        reopened
            .internal_value("internal:typesafeKeySavedV1")
            .is_none()
    );
}

#[test]
fn new_database_preparation_does_not_rewrite_explicit_settings() {
    for yaml in ["true", "false", "invalid"] {
        let (_directory, connection) = legacy(85);
        write_setting(
            &connection,
            "internal:yamlBurnChecksDiscoveryEnabledV1",
            yaml,
        )
        .unwrap();
        write_setting(&connection, "internal:typesafeKeySavedV1", "false").unwrap();
        write_setting(&connection, "internal:burnChecksEnabledAtEpochV1", "123").unwrap();
        write_setting(&connection, FRESH_INITIALIZATION_KEY, "true").unwrap();

        prepare(&connection, 100).unwrap();
        assert!(
            read_setting(&connection, FRESH_INITIALIZATION_KEY)
                .unwrap()
                .is_none()
        );

        assert_eq!(
            read_setting(&connection, "internal:yamlBurnChecksDiscoveryEnabledV1")
                .unwrap()
                .as_deref(),
            Some(yaml)
        );
        assert_eq!(
            read_setting(&connection, "internal:typesafeKeySavedV1")
                .unwrap()
                .as_deref(),
            Some("false")
        );
    }
}

#[test]
fn failed_first_migration_does_not_save_a_fresh_initialization_marker() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(database_path(directory.path())).unwrap();
    connection
        .execute("CREATE TABLE scan_root (injected_failure INTEGER)", [])
        .unwrap();
    assert!(Store::from_connection(connection, directory.path().to_path_buf()).is_err());

    let connection = Connection::open(database_path(directory.path())).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(!table_exists(&connection, "setting"));
    connection.execute("DROP TABLE scan_root", []).unwrap();
    drop(connection);

    let store = Store::open(directory.path()).unwrap();
    assert!(crate::jev::settings::yaml_discovery_enabled(&store));
    assert!(store.internal_value(FRESH_INITIALIZATION_KEY).is_none());
}

#[test]
fn interrupted_fresh_initialization_keeps_its_default_and_explicit_choices() {
    for yaml in [None, Some("false"), Some("invalid")] {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(database_path(directory.path())).unwrap();
        connection
            .execute("CREATE TABLE consent_grant (injected_failure INTEGER)", [])
            .unwrap();
        assert!(Store::from_connection(connection, directory.path().to_path_buf()).is_err());

        let connection = Connection::open(database_path(directory.path())).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            read_setting(&connection, FRESH_INITIALIZATION_KEY)
                .unwrap()
                .as_deref(),
            Some("true")
        );
        connection.execute("DROP TABLE consent_grant", []).unwrap();
        if let Some(value) = yaml {
            write_setting(
                &connection,
                "internal:yamlBurnChecksDiscoveryEnabledV1",
                value,
            )
            .unwrap();
        }
        connection
            .execute_batch(&format!(
                "CREATE TRIGGER injected_marker_failure AFTER DELETE ON setting
                 WHEN OLD.key = '{FRESH_INITIALIZATION_KEY}'
                 BEGIN SELECT RAISE(ABORT, 'marker cleanup failed'); END;"
            ))
            .unwrap();
        assert!(Store::from_connection(connection, directory.path().to_path_buf()).is_err());

        let connection = Connection::open(database_path(directory.path())).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            86
        );
        assert_eq!(
            read_setting(&connection, FRESH_INITIALIZATION_KEY)
                .unwrap()
                .as_deref(),
            Some("true")
        );
        assert_eq!(
            read_setting(&connection, "internal:yamlBurnChecksDiscoveryEnabledV1")
                .unwrap()
                .as_deref(),
            yaml
        );
        assert!(
            read_setting(&connection, "internal:typesafeKeySavedV1")
                .unwrap()
                .is_none()
        );
        connection
            .execute("DROP TRIGGER injected_marker_failure", [])
            .unwrap();
        drop(connection);

        let reopened = Store::open(directory.path()).unwrap();
        assert_eq!(
            crate::jev::settings::yaml_discovery_enabled(&reopened),
            yaml.is_none()
        );
        assert!(reopened.internal_value(FRESH_INITIALIZATION_KEY).is_none());
        assert_eq!(
            reopened
                .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
                .as_deref(),
            yaml
        );
        assert!(
            reopened
                .internal_value("internal:typesafeKeySavedV1")
                .is_none()
        );
    }
}

#[test]
fn effective_yaml_enablement_and_saved_key_marker_survive_upgrade() {
    for yaml in [None, Some("true"), Some("false"), Some("invalid")] {
        for master in [None, Some("0"), Some("not-an-epoch")] {
            let (directory, connection) = legacy(85);
            if let Some(value) = yaml {
                write_setting(
                    &connection,
                    "internal:yamlBurnChecksDiscoveryEnabledV1",
                    value,
                )
                .unwrap();
            }
            if let Some(value) = master {
                write_setting(&connection, "internal:burnChecksEnabledAtEpochV1", value).unwrap();
            }
            write_setting(&connection, "internal:typesafeKeySavedV1", "false").unwrap();
            let expected = match yaml {
                Some("true") => true,
                Some("false") => false,
                _ => master.is_some(),
            };
            let store = upgrade(&directory, connection);
            assert_eq!(
                store
                    .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
                    .as_deref(),
                Some(if expected { "true" } else { "false" }),
                "yaml={yaml:?}, master={master:?}"
            );
            assert_eq!(
                store
                    .internal_value("internal:typesafeKeySavedV1")
                    .as_deref(),
                Some(if master.is_some() { "true" } else { "false" })
            );
            assert!(
                store
                    .internal_value("internal:burnChecksEnabledAtEpochV1")
                    .is_none()
            );
            drop(store);
            let reopened = Store::open(directory.path()).unwrap();
            assert_eq!(
                reopened
                    .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
                    .as_deref(),
                Some(if expected { "true" } else { "false" })
            );
        }
    }
}

#[test]
fn connection_profiles_pending_credentials_and_exact_unrelated_keys_stay_unchanged() {
    use crate::jev::config::{CredentialReference, SystemOneConnection};
    let (directory, connection) = legacy(85);
    let legacy_connection = SystemOneConnection::jev_default();
    let mut named_connection = legacy_connection.clone();
    named_connection.credential = Some(CredentialReference::Connection(
        "custom-ignored-instructions".into(),
    ));
    legacy_connection.validate().unwrap();
    named_connection.validate().unwrap();
    let profile_json = serde_json::to_string(&json!({"activeId":"custom-ignored-instructions",
        "profiles":{"jev":legacy_connection,"custom-ignored-instructions":named_connection}}))
    .unwrap();
    let preserved = [
        (
            "internal:smartChecksActiveConnectionV1",
            "custom-ignored-instructions",
        ),
        ("internal:smartChecksConnectionsV1", profile_json.as_str()),
        ("internal:smartChecksProviderMigrationV1", "1"),
        (
            "internal:smartChecksConnectionChangePendingV1",
            r#"{"connectionId":"custom-ignored-instructions","revision":12}"#,
        ),
        (
            "internal:smartChecksCredentialRemovalPendingV1",
            "system-one-custom-ignored-instructions",
        ),
        ("internal:typesafeCredentialChangePendingV1", "true"),
        ("internal:typesafeAuthRejectedV1", "true"),
        (
            "internal:installedCheckSnoozesV1",
            r#"[{"detector":"installed:ignored-instructions","scope":"check","until":null}]"#,
        ),
        (
            "internal:burnCheckEnabledAtEpochV1:future_check",
            "preserved",
        ),
        ("internal:smartChecksUnrelatedV1", "preserved"),
    ];
    for (key, value) in preserved {
        write_setting(&connection, key, value).unwrap();
    }
    let obsolete = [
        "internal:jevBurnCheckHistoryDaysV1",
        "internal:jevBurnCheckHistoryBatchEpochV1",
        "internal:burnCheckSchedulerV1",
        "internal:burnCheckResponseCacheV1",
    ];
    for key in obsolete {
        write_setting(&connection, key, "obsolete").unwrap();
    }
    for id in RETIRED_KEYS {
        write_setting(
            &connection,
            &format!("internal:burnCheckEnabledAtEpochV1:{id}"),
            "1",
        )
        .unwrap();
    }
    let store = upgrade(&directory, connection);
    for (key, value) in preserved {
        assert_eq!(store.internal_value(key).as_deref(), Some(value), "{key}");
    }
    let restored: Value = serde_json::from_str(
        &store
            .internal_value("internal:smartChecksConnectionsV1")
            .unwrap(),
    )
    .unwrap();
    let profiles: BTreeMap<String, SystemOneConnection> =
        serde_json::from_value(restored["profiles"].clone()).unwrap();
    assert_eq!(profiles["jev"], legacy_connection);
    assert_eq!(profiles["custom-ignored-instructions"], named_connection);
    for profile in profiles.values() {
        profile.validate().unwrap();
    }
    for key in obsolete {
        assert!(store.internal_value(key).is_none(), "{key}");
    }
    for id in RETIRED_KEYS {
        assert!(
            store
                .internal_value(&format!("internal:burnCheckEnabledAtEpochV1:{id}"))
                .is_none()
        );
    }
    assert!(
        store
            .internal_value("internal:typesafeKeySavedV1")
            .is_none()
    );
}

#[test]
fn preference_and_snooze_filtering_keeps_surviving_choices_and_unknown_members() {
    use antiburn_local::checks::DetectorId;
    let (directory, connection) = legacy(85);
    let mut choices = serde_json::Map::new();
    for detector in DetectorId::ALL {
        choices.insert(
            detector.key().into(),
            Value::Bool(detector.index() % 2 == 0),
        );
    }
    for id in RETIRED_KEYS {
        choices.insert(id.into(), Value::Bool(true));
    }
    choices.insert("future_check".into(), Value::Bool(false));
    let mut preferences = json!({"revision": u64::MAX, "checks": choices, "future": [1,2]});
    write_json_setting(&connection, "internal:checkPreferencesV1", &preferences).unwrap();
    let mut snoozes = vec![
        json!({"detector":"unusedSkills","scope":"check","until":123}),
        json!({"detector":"futureCheck","scope":"future","until":null,"extra":true}),
    ];
    for id in RETIRED_SNOOZES {
        snoozes.push(json!({"detector":id,"scope":"check","until":null}));
    }
    write_json_setting(&connection, "internal:burnCheckSnoozesV1", &snoozes).unwrap();
    let store = upgrade(&directory, connection);
    for id in RETIRED_KEYS {
        preferences["checks"].as_object_mut().unwrap().remove(id);
    }
    assert_eq!(
        serde_json::from_str::<Value>(
            &store.internal_value("internal:checkPreferencesV1").unwrap()
        )
        .unwrap(),
        preferences
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            &store.internal_value("internal:burnCheckSnoozesV1").unwrap()
        )
        .unwrap(),
        json!(&snoozes[..2])
    );
    for detector in DetectorId::ALL {
        assert_eq!(
            store.check_enabled(detector).unwrap(),
            detector.index() % 2 == 0
        );
    }
    assert_eq!(store.check_preferences_revision().unwrap(), u64::MAX);
}

#[test]
fn preference_revision_advances_only_when_a_retired_choice_is_removed() {
    for retired in [false, true] {
        let (directory, connection) = legacy(85);
        let raw = if retired {
            r#"{"revision":7,"checks":{"unused_skills":false,"ignored_instructions":false}}"#
        } else {
            r#"{"revision":7,"checks":{"unused_skills":false}}"#
        };
        write_setting(&connection, "internal:checkPreferencesV1", raw).unwrap();
        let store = upgrade(&directory, connection);
        assert_eq!(
            store.check_preferences_revision().unwrap(),
            if retired { 8 } else { 7 }
        );
        assert!(
            !store
                .check_enabled(antiburn_local::checks::DetectorId::UnusedSkills)
                .unwrap()
        );
    }
}

#[test]
fn dispatched_unknown_usage_settles_once_without_changing_known_or_overflow_totals() {
    let (directory, connection) = legacy(85);
    let original = saved_ledger(3);
    write_json_setting(&connection, USAGE_LEDGER_KEY, &original).unwrap();
    save_reservation(&connection, "known", true, false, true);
    save_reservation(&connection, "already-unknown", false, true, true);
    save_reservation(&connection, "expired-dispatched", false, false, true);
    save_reservation(&connection, "unsent", false, false, false);
    connection
        .execute_batch(
            "INSERT INTO burn_check_request_outcome VALUES
        ('second-identity', 'expired-dispatched', 1), ('orphan-known', 'pruned-known', 1);",
        )
        .unwrap();
    connection.execute("INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('yaml-known:1', 1), ('yaml-unknown:1', 0)", []).unwrap();
    let mut markers = rows(&connection, "burn_check_decision_usage");
    for marker in &mut markers {
        marker.push(SqlValue::Null);
    }
    let before = time::OffsetDateTime::now_utc().unix_timestamp();
    let store = upgrade(&directory, connection);
    let mut expected = original;
    expected.as_object_mut().unwrap().remove("reservations");
    expected["summary"]["unknown_outcomes"] = json!(4);
    let actual: Value =
        serde_json::from_str(&store.internal_value(USAGE_LEDGER_KEY).unwrap()).unwrap();
    assert!(actual["summary"]["last_unknown_at_epoch"].as_i64().unwrap() >= before);
    expected["summary"]["last_unknown_at_epoch"] =
        actual["summary"]["last_unknown_at_epoch"].clone();
    assert_eq!(actual, expected);
    assert_eq!(rows(&store.lock(), "burn_check_decision_usage"), markers);
    for table in RETIRED_TABLES {
        assert!(!table_exists(&store.lock(), table), "{table}");
    }
    let summary = store.burn_check_usage_summary().unwrap();
    assert_eq!(summary.input_tokens, u64::MAX);
    assert_eq!(summary.output_tokens, u64::MAX);
    assert_eq!(summary.confirmed_calls, u64::MAX);
    assert_eq!(summary.unknown_outcomes, 4);
    assert_eq!(summary.estimated_usd, None);
    let saved = store.internal_value(USAGE_LEDGER_KEY).unwrap();
    drop(store);
    for _ in 0..2 {
        let reopened = Store::open(directory.path()).unwrap();
        assert_eq!(
            reopened.internal_value(USAGE_LEDGER_KEY).as_deref(),
            Some(saved.as_str())
        );
        assert_eq!(reopened.burn_check_usage_summary().unwrap(), summary);
    }
}

#[test]
fn unknown_counts_above_signed_range_saturate_without_float_conversion() {
    for unknown in [i64::MAX as u64 + 17, u64::MAX] {
        let (directory, connection) = legacy(85);
        write_json_setting(&connection, USAGE_LEDGER_KEY, &saved_ledger(unknown)).unwrap();
        save_reservation(&connection, "dispatched", false, false, true);
        let store = upgrade(&directory, connection);
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            unknown.saturating_add(1)
        );
    }
}

#[test]
fn missing_ledger_records_only_backed_dispatched_usage_and_keeps_yaml_dedup_usable() {
    let (directory, connection) = legacy(85);
    save_reservation(&connection, "dispatched", false, false, true);
    save_reservation(&connection, "unsent", false, false, false);
    let store = upgrade(&directory, connection);
    assert_eq!(
        store.burn_check_usage_summary().unwrap().unknown_outcomes,
        1
    );
    drop(store);

    let (directory, connection) = legacy(85);
    let candidate = "a".repeat(64);
    write_json_setting(
        &connection,
        USAGE_LEDGER_KEY,
        &json!({"summary":{"unknown_outcomes":1}}),
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES (?1, 0)",
            [format!("{candidate}:1")],
        )
        .unwrap();
    save_reservation(&connection, "dispatched", false, false, true);
    let store = upgrade(&directory, connection);
    assert_eq!(
        store.burn_check_usage_summary().unwrap().unknown_outcomes,
        2
    );
    let attempt = DecisionAttempt::new("review", 1).unwrap();
    store
        .record_burn_check_decision_usage(
            &candidate,
            &attempt,
            "yaml-check",
            (
                "typesafe-systemone",
                antiburn_local::analysis::jev::PINNED_MODEL,
            ),
            None,
            100,
        )
        .unwrap();
    assert_eq!(
        store.burn_check_usage_summary().unwrap().unknown_outcomes,
        3
    );
    for _ in 0..2 {
        store
            .record_burn_check_decision_usage(
                &candidate,
                &attempt,
                "yaml-check",
                (
                    "typesafe-systemone",
                    antiburn_local::analysis::jev::PINNED_MODEL,
                ),
                Some(antiburn_local::analysis::jev::JevUsage {
                    input_tokens: 123,
                    output_tokens: 4,
                }),
                101,
            )
            .unwrap();
    }
    let summary = store.burn_check_usage_summary().unwrap();
    assert_eq!(summary.unknown_outcomes, 2);
    assert_eq!(summary.confirmed_calls, 1);
    assert_eq!(summary.input_tokens, 123);
    assert_eq!(summary.output_tokens, 4);
}

#[test]
fn embedded_reservations_upgrade_from_v65_and_indexed_flags_win_on_duplicate_ids() {
    for version in [65, 85] {
        let (directory, connection) = legacy(version);
        let ledger =
            json!({"reservations": [reservation("embedded", false, false)], "summary": {}});
        write_json_setting(&connection, USAGE_LEDGER_KEY, &ledger).unwrap();
        connection
            .execute(
                "INSERT INTO burn_check_request_outcome VALUES ('request', 'embedded', 1)",
                [],
            )
            .unwrap();
        if version == 85 {
            save_reservation(&connection, "embedded", true, false, false);
        }
        let store = upgrade(&directory, connection);
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            if version == 65 { 1 } else { 0 }
        );
        assert!(
            !serde_json::from_str::<Value>(&store.internal_value(USAGE_LEDGER_KEY).unwrap())
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("reservations")
        );
    }
}

#[test]
fn v85_embedded_reservations_without_indexed_rows_settle_only_new_dispatched_usage() {
    let (directory, connection) = legacy(85);
    let mut original = saved_ledger(3);
    original["reservations"] = json!([
        reservation("dispatched", false, false),
        reservation("known", true, false),
        reservation("already-unknown", false, true),
        reservation("unsent", false, false),
    ]);
    write_json_setting(&connection, USAGE_LEDGER_KEY, &original).unwrap();
    connection
        .execute_batch(
            "INSERT INTO burn_check_request_outcome VALUES
             ('first', 'dispatched', 1), ('duplicate', 'dispatched', 1),
             ('known', 'known', 1), ('unknown', 'already-unknown', 1),
             ('orphan', 'pruned-known', 1);",
        )
        .unwrap();
    assert!(rows(&connection, "burn_check_usage_reservation").is_empty());

    let store = upgrade(&directory, connection);
    let saved = store.internal_value(USAGE_LEDGER_KEY).unwrap();
    let actual: Value = serde_json::from_str(&saved).unwrap();
    let mut expected = original;
    expected.as_object_mut().unwrap().remove("reservations");
    expected["summary"]["unknown_outcomes"] = json!(4);
    expected["summary"]["last_unknown_at_epoch"] =
        actual["summary"]["last_unknown_at_epoch"].clone();
    assert_eq!(actual, expected);
    assert_eq!(
        store.burn_check_usage_summary().unwrap().unknown_outcomes,
        4
    );
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    assert_eq!(reopened.internal_value(USAGE_LEDGER_KEY), Some(saved));
}

#[test]
fn malformed_dispatched_reservations_roll_back_all_state_and_allow_a_safe_retry() {
    for data in [
        "not json",
        r#"{"id":"dispatched","settled":"invalid"}"#,
        r#"{"id":"different","settled":false}"#,
    ] {
        let (directory, connection) = legacy(85);
        write_setting(&connection, "internal:burnChecksEnabledAtEpochV1", "1").unwrap();
        write_json_setting(&connection, USAGE_LEDGER_KEY, &saved_ledger(3)).unwrap();
        save_reservation(&connection, "dispatched", false, false, true);
        connection
            .execute(
                "UPDATE burn_check_usage_reservation SET data = ?1 WHERE id = 'dispatched'",
                [data],
            )
            .unwrap();
        seed_remediation(&connection, "retired", "ignored_instructions");
        seed_contribution(&connection, "retired", "retired", "ignored_instructions");
        let snapshots = RETIRED_TABLES
            .into_iter()
            .chain(["setting", "remediation", "remediation_contribution"])
            .map(|table| (table, rows(&connection, table)))
            .collect::<BTreeMap<_, _>>();
        assert!(Store::from_connection(connection, directory.path().to_path_buf()).is_err());

        let connection = Connection::open(database_path(directory.path())).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            86
        );
        for (table, before) in snapshots {
            assert!(table_exists(&connection, table), "{table}");
            assert_eq!(rows(&connection, table), before, "{data}: {table}");
        }
        connection
            .execute(
                "UPDATE burn_check_usage_reservation SET data = ?1 WHERE id = 'dispatched'",
                [reservation("dispatched", false, false).to_string()],
            )
            .unwrap();
        let store = upgrade(&directory, connection);
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            4
        );
        store.migrate().unwrap();
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            4
        );
    }
}

fn seed_remediation(connection: &Connection, id: &str, detector: &str) {
    connection.execute("INSERT INTO remediation
        (remediation_id, target_key, environment_key, agent, scope_kind, scope_key, state,
         definition_json, result_json, created_at_epoch, updated_at_epoch)
        VALUES (?1, ?1, 'native', 'claude-code', 'session', 'scope', 'waitingForPromptUse', ?2, '{\"version\":1}', 1, 1)",
        params![id, json!({"version":1,"detector":detector}).to_string()]).unwrap();
}

fn seed_contribution(connection: &Connection, id: &str, remediation: &str, detector: &str) {
    connection
        .execute(
            "INSERT INTO remediation_contribution VALUES
        (?1, ?2, ?3, 'passive', '{\"version\":1}', '{\"version\":1}', 1, 2, 2)",
            params![id, remediation, detector],
        )
        .unwrap();
}

#[test]
fn remediation_removal_uses_exact_detector_and_linked_contribution_identities() {
    let (directory, connection) = legacy(85);
    seed_remediation(&connection, "keep", "unused_skills");
    seed_contribution(&connection, "keep", "keep", "unused_skills");
    for id in RETIRED_KEYS {
        seed_remediation(&connection, id, id);
        seed_contribution(&connection, id, id, id);
        seed_contribution(&connection, &format!("linked-{id}"), id, "future_check");
        seed_contribution(&connection, &format!("orphan-{id}"), "deleted-watch", id);
    }
    seed_remediation(&connection, "similar-name", "ignored_instructions_extra");
    let expected = rows(&connection, "remediation")
        .into_iter()
        .filter(|row| {
            row[0] == SqlValue::Text("keep".into())
                || row[0] == SqlValue::Text("similar-name".into())
        })
        .collect::<Vec<_>>();
    let store = upgrade(&directory, connection);
    assert_eq!(rows(&store.lock(), "remediation"), expected);
    assert_eq!(rows(&store.lock(), "remediation_contribution").len(), 1);
}

#[test]
fn unreadable_ledger_preferences_or_snoozes_fail_without_partial_migration() {
    for (key, value) in [
        (USAGE_LEDGER_KEY, "not json"),
        (USAGE_LEDGER_KEY, r#"{"summary":{"unknown_outcomes":-1}}"#),
        (USAGE_LEDGER_KEY, r#"{"reservations":false,"summary":{}}"#),
        ("internal:checkPreferencesV1", "not json"),
        ("internal:burnCheckSnoozesV1", "not json"),
    ] {
        let (directory, connection) = legacy(85);
        write_setting(&connection, "internal:burnChecksEnabledAtEpochV1", "1").unwrap();
        write_setting(&connection, key, value).unwrap();
        let settings = rows(&connection, "setting");
        assert!(
            Store::from_connection(connection, directory.path().to_path_buf()).is_err(),
            "{key}={value}"
        );
        let connection = Connection::open(database_path(directory.path())).unwrap();
        assert_eq!(rows(&connection, "setting"), settings);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            86
        );
        for table in RETIRED_TABLES {
            assert!(table_exists(&connection, table));
        }
    }
}

#[test]
fn failure_before_ledger_write_before_drop_or_at_commit_rolls_back_and_retries_once() {
    for fault in ["ledger", "drop", "commit"] {
        let (directory, connection) = legacy(85);
        write_setting(&connection, "internal:burnChecksEnabledAtEpochV1", "1").unwrap();
        write_json_setting(&connection, USAGE_LEDGER_KEY, &saved_ledger(2)).unwrap();
        save_reservation(&connection, "dispatched", false, false, true);
        seed_remediation(&connection, "retired", "ignored_instructions");
        seed_contribution(&connection, "retired", "retired", "ignored_instructions");
        match fault {
            "ledger" => connection.execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON setting
                WHEN NEW.key = 'internal:burnCheckUsageLedgerV1' BEGIN SELECT RAISE(ABORT, 'ledger write failed'); END;").unwrap(),
            "drop" => connection.execute_batch("CREATE TRIGGER injected_failure BEFORE DELETE ON remediation
                BEGIN SELECT RAISE(ABORT, 'table drop preparation failed'); END;").unwrap(),
            "commit" => connection.execute_batch("CREATE TABLE guard_parent(id INTEGER PRIMARY KEY);
                CREATE TABLE guard_child(id INTEGER REFERENCES guard_parent(id) DEFERRABLE INITIALLY DEFERRED);
                CREATE TRIGGER injected_failure AFTER DELETE ON remediation BEGIN INSERT INTO guard_child VALUES (1); END;").unwrap(),
            _ => unreachable!(),
        }
        let snapshots = RETIRED_TABLES
            .into_iter()
            .chain(["setting", "remediation", "remediation_contribution"])
            .map(|table| (table, rows(&connection, table)))
            .collect::<BTreeMap<_, _>>();
        assert!(
            Store::from_connection(connection, directory.path().to_path_buf()).is_err(),
            "{fault}"
        );
        let connection = Connection::open(database_path(directory.path())).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            86,
            "{fault}"
        );
        for (table, before) in snapshots {
            assert_eq!(rows(&connection, table), before, "{fault}: {table}");
        }
        connection
            .execute_batch("DROP TRIGGER injected_failure")
            .unwrap();
        let store = upgrade(&directory, connection);
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            3
        );
        store.migrate().unwrap();
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            3
        );
    }
}

fn seed_live_runtime(connection: &Connection) {
    connection.execute_batch(r#"
        INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
            first_seen_at, last_seen_at, activity_cursor, source_generation, source_fingerprint)
        VALUES ('native', 'claude-code', 'session', 'file', 'fixture.jsonl', 'now', 'now', 'cursor-7', 7, 'fingerprint-7');
        INSERT INTO source_resume VALUES ('native', 'claude-code', 'session', 'source',
            X'010203', 1, 1, 1, 1, 1, 1, 'fingerprint-7', 1);
        INSERT INTO native_file_delivery (environment_key, agent, session_id, incarnation,
            source_label, cwd, parser_revision, cursor_json, active_until_epoch, observed_generation)
        VALUES ('native', 'claude-code', 'session', 1, 'fixture.jsonl', '/synthetic/project', 1, '{"offset":7}', 999, 7);
        INSERT INTO native_tool_result_publication (outcome, environment_key, agent, session_id,
            source_format, complete, source_generation, claim_fence)
        VALUES ('coverage_complete', 'native', 'claude-code', 'session', 'claude_jsonl', 1, 7, 8);
        INSERT INTO burn_check_activation VALUES ('scope-creep', 'repo', 'revision', 1, 1, 0, 1, 1,
            '{"cursor":1,"branches":{"session":{"position":7}}}', '{"state":"active"}');
        INSERT INTO burn_check_observation VALUES ('scope-creep', 'repo', 'observation',
            '["native","claude-code","session"]', 1, 'claude_jsonl', 'native-identity', 1,
            'version-1', 0, '{"branch":"session","content":"observed"}');
        INSERT INTO burn_check_candidate (candidate_id, check_id, activation_scope_key, generation,
            branch_key, session_key_json, trigger_position, selected_publication_head, dependencies_json,
            frozen_input_json, input_fingerprint, state, created_at_epoch, updated_at_epoch, preparation_json)
        VALUES ('accepted', 'scope-creep', 'repo', 1, 'session', '["native","claude-code","session"]',
            1, 1, '{}', '{"selected":"evidence"}', 'fingerprint', 'accepted', 1, 1, '{"status":"ready"}'),
            ('unknown', 'scope-creep', 'repo', 1, 'session', '["native","claude-code","session"]',
            2, 1, '{}', '{"selected":"pending"}', 'pending-fingerprint', 'dispatching', 2, 2, '{"status":"ready"}');
        INSERT INTO burn_check_candidate_dependency VALUES ('unknown', 'observation',
            '["native","claude-code","session"]', 'claude_jsonl', 1, 'version-1', 0, 'payload-digest');
        INSERT INTO burn_check_cooldown VALUES ('scope-creep', 'repo', 1, 'session', 999);
        INSERT INTO burn_check_evaluation_attempt VALUES ('unknown:1', 'unknown', 1, 'dispatch_intent', NULL, NULL, 2);
        INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('unknown:1', 0), ('accepted:1', 1);
        INSERT INTO burn_check_assessment_history VALUES ('assessment', 'accepted', 'scope-creep',
            'repo', 1, 'session', 1, 'assessed', '{"label":"bad"}', '[]', 1, 1);
        INSERT INTO burn_check_current_status VALUES ('scope-creep', 'repo', 'session', 1,
            'failure', 'assessment', 1, 1, 1);
        INSERT INTO burn_check_acceptance_watermark VALUES ('scope-creep', 'repo', 'session', 1, 1);
        INSERT INTO burn_check_session_attachment VALUES ('scope-creep', 'repo', 1, 'session', 1);
    "#).unwrap();
    let mut progress = antiburn_checks::check_store::SourceProgress::default();
    progress.publication_cursor.0 = 1;
    progress.observation_position.0 = 7;
    let branch = SessionKey::new("native", "claude-code", "session").to_identity_json();
    progress.selection.branches.insert(
        antiburn_checks::domain::BranchId(branch.clone()),
        Default::default(),
    );
    for table in [
        "burn_check_candidate",
        "burn_check_cooldown",
        "burn_check_assessment_history",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_session_attachment",
    ] {
        connection
            .execute(&format!("UPDATE {table} SET branch_key = ?1"), [&branch])
            .unwrap();
    }
    connection.execute("UPDATE burn_check_observation SET payload_json = json_set(payload_json, '$.branch', ?1)", [&branch]).unwrap();
    connection
        .execute(
            "UPDATE burn_check_activation SET checkpoint_json = ?1",
            [progress.encode().unwrap()],
        )
        .unwrap();
}

#[test]
fn live_yaml_state_is_preserved_and_legacy_unknown_work_cannot_resume() {
    let (directory, connection) = legacy(85);
    seed_live_runtime(&connection);
    write_json_setting(&connection, USAGE_LEDGER_KEY, &saved_ledger(1)).unwrap();
    let tables = [
        "session",
        "source_resume",
        "native_file_delivery",
        "native_tool_result_publication",
        "burn_check_activation",
        "burn_check_observation",
        "burn_check_candidate",
        "burn_check_candidate_dependency",
        "burn_check_cooldown",
        "burn_check_evaluation_attempt",
        "burn_check_decision_usage",
        "burn_check_assessment_history",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_session_attachment",
        "session_incarnation_seq",
        "evidence_claim_fence_seq",
        "work_queue_schedule",
    ];
    let snapshots = tables
        .into_iter()
        .map(|table| (table, rows(&connection, table)))
        .collect::<BTreeMap<_, _>>();
    let candidate_columns = connection
        .prepare("SELECT name FROM pragma_table_info('burn_check_candidate') ORDER BY cid")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let store = upgrade(&directory, connection);
    for (table, mut before) in snapshots {
        let mut actual = rows(&store.lock(), table);
        match table {
            "burn_check_candidate_dependency" | "burn_check_observation" => {
                for row in &mut before {
                    row.extend([SqlValue::Text("tool_result".into()), SqlValue::Null]);
                }
            }
            "native_file_delivery" | "burn_check_decision_usage" => {
                for row in &mut before {
                    row.push(SqlValue::Null);
                }
            }
            "native_tool_result_publication" => {
                for row in &mut before {
                    let gap_kind = row.pop().unwrap();
                    row.push(SqlValue::Text("tool_result".into()));
                    row.extend(std::iter::repeat_n(SqlValue::Null, 9));
                    row.extend([gap_kind, SqlValue::Null]);
                }
            }
            "burn_check_session_attachment" => {
                // V89 leaves existing attachment observers uninitialized.
                for row in &mut before {
                    row.extend(std::iter::repeat_n(SqlValue::Null, 9));
                    row.push(SqlValue::Integer(0));
                    row.extend(std::iter::repeat_n(SqlValue::Null, 3));
                    row.extend([SqlValue::Integer(0), SqlValue::Null]);
                }
            }
            "burn_check_candidate" => {
                for row in &mut before {
                    if row[0] == SqlValue::Text("unknown".into()) {
                        for (name, value) in [
                            ("state", SqlValue::Text("not_assessed".into())),
                            ("claim_owner", SqlValue::Null),
                            ("diagnostic_json", SqlValue::Text(
                                r#"{"code":"invalid_decision","detail":"The saved assessment uses an incompatible decision format."}"#.into(),
                            )),
                        ] {
                            let index = candidate_columns.iter().position(|column| column == name).unwrap();
                            row[index] = value;
                        }
                        let fence = candidate_columns
                            .iter()
                            .position(|column| column == "claim_fence")
                            .unwrap();
                        let SqlValue::Integer(value) = row[fence] else {
                            panic!("the legacy claim fence must be an integer");
                        };
                        row[fence] = SqlValue::Integer(value + 1);
                    }
                    row.push(SqlValue::Null);
                }
            }
            "burn_check_evaluation_attempt" => {
                assert_eq!(before.len(), 1);
                before[0].insert(2, SqlValue::Null);
                before[0][4] = SqlValue::Text("settled".into());
                before[0][5] = SqlValue::Text("unknown".into());
            }
            "burn_check_assessment_history" => {
                assert_eq!(actual.len(), before.len() + 1);
                let terminal = actual.pop().unwrap();
                assert_eq!(terminal[0], SqlValue::Text("assessment:unknown".into()));
                assert_eq!(terminal[1], SqlValue::Text("unknown".into()));
                assert_eq!(terminal[7], SqlValue::Text("not_assessed".into()));
                assert_eq!(terminal[10], SqlValue::Integer(0));
            }
            _ => {}
        }
        assert_eq!(actual, before, "{table}");
    }
    assert_eq!(
        store.burn_check_usage_summary().unwrap().unknown_outcomes,
        1
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn session_deletion_retention_and_clear_work_after_table_removal() {
    for retain in [false, true] {
        let (directory, connection) = legacy(85);
        seed_live_runtime(&connection);
        connection
            .execute("UPDATE session SET updated_at_epoch = 1", [])
            .unwrap();
        write_json_setting(&connection, USAGE_LEDGER_KEY, &saved_ledger(1)).unwrap();
        let store = upgrade(&directory, connection);
        let before = store.burn_check_usage_summary().unwrap();
        let markers = rows(&store.lock(), "burn_check_decision_usage");
        if retain {
            assert_eq!(
                crate::store::apply_session_retention_in(&store.lock(), 30, 10_000_000).unwrap(),
                1
            );
        } else {
            assert!(
                store
                    .delete_session(&SessionKey::new("native", "claude-code", "session"))
                    .unwrap()
                    .is_some()
            );
        }
        assert_eq!(store.burn_check_usage_summary().unwrap(), before);
        assert_eq!(rows(&store.lock(), "burn_check_decision_usage"), markers);
        assert!(rows(&store.lock(), "source_resume").is_empty());
        assert!(rows(&store.lock(), "burn_check_candidate").is_empty());
        assert!(rows(&store.lock(), "burn_check_session_attachment").is_empty());
        let checkpoint: String = store
            .lock()
            .query_row(
                "SELECT checkpoint_json FROM burn_check_activation",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            antiburn_checks::check_store::SourceProgress::decode(&checkpoint)
                .unwrap()
                .selection
                .branches
                .is_empty()
        );
        store.clear_local_session_data().unwrap();
        assert_eq!(
            store.burn_check_usage_summary().unwrap(),
            crate::store::BurnCheckUsageSummary::default()
        );
        assert!(rows(&store.lock(), "burn_check_decision_usage").is_empty());
    }
}
