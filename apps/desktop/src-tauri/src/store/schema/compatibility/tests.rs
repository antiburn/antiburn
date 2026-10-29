use std::collections::BTreeMap;

use rusqlite::{params, types::Value};

use super::*;
use crate::store::{Store, database_path};

const QUOTA: &str = "CREATE INDEX turn_quota_published
    ON turn(environment_key, agent, session_id, claim_fence, ts_ms)
    WHERE ts_ms IS NOT NULL;";

fn migration_steps(sql: &str) -> Vec<&str> {
    let mut steps = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    for line in sql.split_inclusive('\n') {
        if matches!(line, "-- next migration\n" | "-- next migration\r\n") {
            steps.push(&sql[start..offset]);
            start = offset + line.len();
        }
        offset += line.len();
    }
    steps.push(&sql[start..]);
    steps
}

#[test]
fn migration_steps_preserve_sql_with_lf_and_crlf_framing() {
    for newline in ["\n", "\r\n"] {
        for ending in ["", newline] {
            let expected = [
                format!("SELECT '-- next migration';{newline}"),
                format!("-- next migration suffix{newline}SELECT 2;{newline}{newline}"),
                format!("SELECT 3;{ending}"),
            ];
            let fixture = expected.join(&format!("-- next migration{newline}"));
            assert_eq!(migration_steps(&fixture), expected);
        }
    }
}

fn history(connection: &Connection, branch: bool, version: usize) {
    // Both pinned histories share the unchanged first 80 migrations.
    for sql in &super::super::MIGRATIONS[..80] {
        connection.execute_batch(sql).unwrap();
    }
    let branch_steps = migration_steps(include_str!("branch_history.sql"));
    assert_eq!(branch_steps.len(), 7);
    let main_steps = [
        QUOTA,
        branch_steps[0],
        branch_steps[1],
        branch_steps[2],
        include_str!("main_metadata_v85.sql"),
        include_str!("main_registry_v86.sql"),
    ];
    let steps = if branch {
        &branch_steps[..]
    } else {
        &main_steps[..]
    };
    for sql in &steps[..version - 80] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .pragma_update(None, "user_version", version)
        .unwrap();
}

fn historical_connection(branch: bool, version: usize) -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    history(&connection, branch, version);
    connection
}

fn seed(connection: &Connection) {
    let profile = read_profile(connection).unwrap();
    connection.execute_batch(r#"
        INSERT INTO setting VALUES ('synthetic-connection', '{ "revision": 7, "credential": "synthetic-reference" }');
        INSERT INTO setting VALUES ('internal:yamlBurnChecksDiscoveryEnabledV1', 'false');
        INSERT INTO setting VALUES ('internal:checkPreferencesV1', '{"revision":7,"checks":{"cache_churn":false,"fixture":true}}');
        INSERT INTO setting VALUES ('internal:burnCheckUsageLedgerV1', '{"summary":{"unknown_outcomes":3,"future_field":{"preserved":true}}}');
        DELETE FROM setting WHERE key = 'internal:jevBurnCheckHistoryBatchEpochV1';
        INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
            first_seen_at, last_seen_at, incarnation, source_generation, source_fingerprint, activity_cursor)
        VALUES ('native', 'opencode', 'v2', 'providerDb', 'opencode:v2:open_code_v2_sqlite', 'now', 'now', 12, 7, 'fingerprint', 'cursor'),
            ('native', 'opencode', 'v1', 'providerDb', 'opencode:v1', 'now', 'now', 13, 8, 'fingerprint-v1', 'cursor-v1'),
            ('native', 'opencode', 'export', 'inline', 'opencode:export', 'now', 'now', 14, 9, 'fingerprint-export', 'cursor-export'),
            ('native', 'claude-code', 'file', 'file', 'fixture.jsonl', 'now', 'now', 15, 10, 'fingerprint-file', 'cursor-file');
        INSERT INTO source_resume VALUES ('native', 'claude-code', 'file', 'source',
            X'010203', 1, 1, 1, 1, 1, 1, 'fingerprint-file', 1);
        INSERT INTO native_file_delivery (environment_key, agent, session_id, incarnation,
            source_label, cwd, parser_revision, cursor_json, active_until_epoch, observed_generation, service_ticket)
        VALUES ('native', 'claude-code', 'file', 15, 'fixture.jsonl', '/synthetic/project', 1, '{ "offset": 7 }', 999, 10, 20);
        INSERT INTO opencode_journal_delivery (environment_key, agent, session_id, incarnation,
            source_label, cwd, parser_revision, cursor_json, main_session, baseline_sequence, service_ticket)
        VALUES ('native', 'opencode', 'v1', 13, 'opencode:v1', '/synthetic/project', 1, '{ "sequence": 8 }', 1, 3, 21);
        INSERT INTO burn_check_activation VALUES ('fixture', '{}', 'revision', 1, 1, 3, 8, 91,
            '{ "version": 1, "cursor": 8 }', '{ "state": "active" }');
        INSERT INTO burn_check_candidate (candidate_id, check_id, activation_scope_key, generation,
            branch_key, session_key_json, trigger_position, selected_publication_head,
            dependencies_json, frozen_input_json, answer_json, input_fingerprint, state,
            claim_owner, claim_fence, created_at_epoch, updated_at_epoch, preparation_json)
        VALUES ('accepted', 'fixture', '{}', 1, 'accepted', '["native","opencode","v2"]', 1, 7,
            '{ "binding": 7 }', '{ "frozen": true }', '{ "choices": [] }', 'input', 'accepted',
            'saved-owner', 4, 90, 91, '{ "status": "ready", "frozen": "saved" }');
        INSERT INTO burn_check_assessment_history VALUES ('assessment', 'accepted', 'fixture', '{}', 1,
            'accepted', 1, 'assessed', '{ "answer": true }', '{ "actions": [] }', 1, 92);
        INSERT INTO burn_check_current_status VALUES ('fixture', '{}', 'accepted', 1, 'failure', 'assessment', 1, 1, 92);
        INSERT INTO burn_check_acceptance_watermark VALUES ('fixture', '{}', 'accepted', 1, 1);
        INSERT INTO burn_check_cooldown VALUES ('fixture', '{}', 1, 'accepted', 150);
        INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('accepted:1', 1);
        UPDATE session_incarnation_seq SET value = 30;
        UPDATE evidence_claim_fence_seq SET value = 40;
        UPDATE work_queue_schedule SET next_ticket = 50, successful_turns = 12;
    "#).unwrap();
    let attempt_columns = if profile.decisions {
        ", decision_name"
    } else {
        ""
    };
    let attempt_value = if profile.decisions { ", 'review'" } else { "" };
    connection.execute_batch(&format!("INSERT INTO burn_check_evaluation_attempt
        (attempt_id, candidate_id, attempt_number, state, outcome, response_json, updated_at_epoch{attempt_columns})
        VALUES ('accepted:1', 'accepted', 1, 'settled', 'answer', '{{ \"answer\": true }}', 91{attempt_value});")).unwrap();
    let gap_column = if profile.source_metadata {
        ", gap_kind"
    } else {
        ""
    };
    let gap_value = if profile.source_metadata {
        ", 'partial'"
    } else {
        ""
    };
    let segment_value = if profile.source_metadata {
        ", 'segment_reset'"
    } else {
        ""
    };
    let selection_column = if profile.selection {
        ", session_selection_json"
    } else {
        ""
    };
    let selection_value = if profile.selection {
        ", '{ \"frozen\": \"selection\" }'"
    } else {
        ""
    };
    connection.execute_batch(&format!(r#"
        INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key, agent,
            session_id, source_format, native_identity, native_position_json, native_revision, payload_json,
            complete, source_generation, claim_fence{gap_column}{selection_column})
        VALUES (7, 'update', 'native', 'opencode', 'v2', '"open_code_v2_sqlite"', 'identity', '{{ "position": 7 }}',
            2, '{{ "record_id": "owner", "text": "exact" }}', 1, 7, 40{gap_value}{selection_value});
        INSERT INTO native_tool_result_current (environment_key, agent, session_id, source_format,
            native_identity, native_position_json, native_revision, payload_json, complete, is_removed,
            publication_sequence, source_generation, claim_fence)
        VALUES ('native', 'opencode', 'v2', '"open_code_v2_sqlite"', 'identity', '{{ "position": 7 }}',
            2, '{{ "record_id": "owner", "text": "exact" }}', 1, 0, 7, 7, 40);
        INSERT INTO native_tool_result_publication (outcome, environment_key, agent, session_id,
            source_format, complete, source_generation, claim_fence, gap_reason)
        VALUES ('coverage_gap', 'native', 'opencode', 'v2', '"open_code_v2_sqlite"', 0, 7, 40, 'v2-history-incomplete');
        INSERT INTO native_tool_result_publication (outcome, environment_key, agent, session_id,
            source_format, complete, source_generation, claim_fence, gap_reason{gap_column})
        VALUES ('coverage_gap', 'native', 'opencode', 'v2', '"open_code_v2_sqlite"', 0, 7, 40, 'fixture-segment'{segment_value});
        INSERT INTO native_tool_result_publication (outcome, environment_key, agent, session_id,
            source_format, complete, source_generation, claim_fence)
        VALUES ('coverage_gap', 'native', 'opencode', 'v1', '"open_code_sqlite_v2"', 0, 8, 40);
        UPDATE sqlite_sequence SET seq = 42 WHERE name = 'native_tool_result_publication';
        INSERT INTO burn_check_observation (check_id, activation_scope_key, observation_id, session_key_json,
            publication_sequence, source_format, native_identity, native_revision, evidence_version, removed, payload_json)
        VALUES ('fixture', '{{}}', 'observation', '["native","opencode","v2"]', 7, '"open_code_v2_sqlite"', 'identity',
            2, 'evidence-v2', 0, '{{ "observed": true }}');
        INSERT INTO burn_check_candidate_dependency (candidate_id, observation_id, session_key_json,
            source_format, native_revision, evidence_version, removed, payload_sha256)
        VALUES ('accepted', 'observation', '["native","opencode","v2"]', '"open_code_v2_sqlite"', 2, 'evidence-v2', 0, 'hash');
        INSERT INTO burn_check_session_attachment (check_id, activation_scope_key, generation, branch_key, incarnation)
        VALUES ('fixture', '{{}}', 1, 'accepted', 12);
    "#)).unwrap();
    if profile.capture {
        connection.execute_batch(r#"
            INSERT INTO opencode_v2_selection VALUES ('native', 'opencode', 'v2');
            INSERT INTO opencode_v2_capture (environment_key, agent, session_id, incarnation, source_generation,
                source_label, cwd, capture_epoch, claim_fence, baseline_sequence, last_sequence, history_gap, lease_expires_at_epoch)
            VALUES ('native', 'opencode', 'v2', 12, 7, 'opencode:v2:open_code_v2_sqlite', '/synthetic/project', 'capture', 40, 3, 8, 1, 500);
            INSERT INTO opencode_v2_source_status VALUES ('native', 'opencode', 'v2', 12, 7,
                'opencode:v2:open_code_v2_sqlite', '/synthetic/project', 'capture-gap', 91);
        "#).unwrap();
    }
    if profile.capture_fields {
        connection.execute_batch("UPDATE opencode_v2_capture SET admitted_source_json = '{ \"receipt\": \"saved\" }', retirement_reason = 'session-moved'").unwrap();
    }
    if profile.decisions {
        for state in ["pending", "dispatching"] {
            connection.execute("INSERT INTO burn_check_candidate (candidate_id, check_id, activation_scope_key,
                generation, branch_key, session_key_json, trigger_position, selected_publication_head,
                dependencies_json, frozen_input_json, input_fingerprint, state, claim_owner, claim_fence,
                created_at_epoch, updated_at_epoch, started_cooldown_until_epoch)
                VALUES (?1, 'fixture', '{}', 1, ?1, '[\"native\",\"opencode\",\"v2\"]', 2, 7,
                '{}', '{ \"frozen\": \"current\" }', 'input', ?1, 'current-owner', 9, 90, 91, 150)", [state]).unwrap();
            connection.execute("INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name,
                attempt_number, state, updated_at_epoch) VALUES (?1, ?1, 'review', 1, 'dispatch_intent', 91)", [state]).unwrap();
        }
    }
    if profile.evidence {
        connection.execute_batch(r#"
            UPDATE burn_check_session_attachment SET source_label = 'opencode:v2:open_code_v2_sqlite',
                source_format = '"open_code_v2_sqlite"', source_receipt = '{ "receipt": 7 }', observer_epoch = 'epoch',
                start_publication = 7, observation_started_at_ms = 12, last_activity_token = 'token',
                last_activity_order_json = '{ "order": 8 }', last_activity_at_ms = 90, activity_revision = 4,
                idle_deadline_ms = 500, idle_emitted_revision = 3, idle_publication = 11, continuity_eligible = 1,
                session_selection_json = '{ "frozen": "attachment" }';
            UPDATE native_file_delivery SET source_selection_json = '{ "frozen": "file" }';
            UPDATE opencode_journal_delivery SET source_selection_json = '{ "frozen": "journal" }';
            UPDATE opencode_v2_capture SET source_selection_json = '{ "frozen": "capture" }';
            UPDATE burn_check_observation SET observer_binding_json = '{ "binding": "observer" }';
            UPDATE burn_check_candidate_dependency SET dependency_binding_json = '{ "binding": "dependency" }';
        "#).unwrap();
        connection.execute_batch(&format!(r#"
            INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key,
                agent, session_id, source_format, payload_json, complete, source_generation, claim_fence,
                event_kind, first_eligible_publication, target_check_id, target_scope_key, target_generation,
                target_branch_key, target_incarnation, target_attachment_epoch, dependency_binding_json{selection_column})
            VALUES (11, 'update', 'native', 'opencode', 'v2', '"open_code_v2_sqlite"', '{{ "frozen": "idle" }}',
                1, 7, 40, 'session_idle', 7, 'fixture', '{{}}', 1, 'accepted', 12, 'epoch',
                '{{ "binding": "frozen-idle" }}'{selection_value});
        "#)).unwrap();
    }
    if profile.source_metadata {
        connection.execute_batch("UPDATE session SET source_format = '\"open_code_v2_sqlite\"' WHERE session_id = 'v2'").unwrap();
    }
}

fn rows(connection: &Connection, table: &str, columns: &[String]) -> Vec<Vec<Value>> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {} FROM {table} ORDER BY {}",
            columns.join(","),
            (1..=columns.len())
                .map(|index| index.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ))
        .unwrap();
    statement
        .query_map([], |row| {
            (0..columns.len()).map(|index| row.get(index)).collect()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn snapshot(connection: &Connection) -> BTreeMap<String, (Vec<String>, Vec<Vec<Value>>)> {
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    tables
        .into_iter()
        .filter(|table| {
            !table.starts_with("burn_check_")
                || ![
                    "burn_check_assessment",
                    "burn_check_dispatch_attempt",
                    "burn_check_sampled_pair",
                    "burn_check_sample_origin",
                    "burn_check_instruction_epoch",
                    "burn_check_work_answer",
                    "burn_check_response_cache",
                    "burn_check_request_outcome",
                    "burn_check_usage_reservation",
                ]
                .contains(&table.as_str())
        })
        .map(|table| {
            let columns = connection
                .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
                .unwrap()
                .query_map([&table], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let values = rows(connection, &table, &columns);
            (table, (columns, values))
        })
        .collect()
}

#[test]
fn pinned_histories_preserve_populated_state_through_disk_open_and_reopen() {
    for (branch, versions) in [(true, 81..=87), (false, 80..=86)] {
        for version in versions {
            let directory = tempfile::tempdir().unwrap();
            let connection = Connection::open(database_path(directory.path())).unwrap();
            connection
                .pragma_update(None, "foreign_keys", true)
                .unwrap();
            history(&connection, branch, version);
            seed(&connection);
            let before = snapshot(&connection);
            drop(connection);
            for _ in 0..2 {
                let store = Store::open(directory.path()).unwrap_or_else(|error| {
                    panic!("branch={branch}, version={version}: {error:#}")
                });
                assert_eq!(store.schema_version().unwrap(), 92);
                store.migrate().unwrap();
                let connection = store.lock();
                assert_eq!(read_profile(&connection).unwrap(), Profile::canonical(92));
                for (table, (columns, expected)) in &before {
                    assert_eq!(
                        rows(&connection, table, columns),
                        *expected,
                        "branch={branch}, version={version}, table={table}"
                    );
                }
                assert_eq!(connection.query_row("SELECT seq FROM sqlite_sequence WHERE name = 'native_tool_result_publication'", [], |row| row.get::<_, i64>(0)).unwrap(), 42);
                assert_eq!(
                    connection
                        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                            row.get::<_, i64>(0)
                        })
                        .unwrap(),
                    0
                );
                assert_eq!(
                    connection
                        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                        .unwrap(),
                    "ok"
                );
                assert!(
                    connection
                        .execute("UPDATE native_tool_result_publication SET complete = 0", [])
                        .is_err()
                );
            }
        }
    }
}

#[test]
fn branch_repair_maps_each_known_shape_and_canonical_profiles_are_no_ops() {
    for version in 81..=87 {
        let mut connection = historical_connection(true, version);
        let expected = version as i64 + if version <= 83 { 1 } else { 3 };
        assert_eq!(
            repair_branch_version(&mut connection, version as i64).unwrap(),
            expected
        );
        assert_eq!(
            read_profile(&connection).unwrap(),
            Profile::canonical(expected)
        );
        assert_eq!(
            repair_branch_version(&mut connection, expected).unwrap(),
            expected
        );
    }
    for version in 81..=92 {
        let mut connection = Connection::open_in_memory().unwrap();
        for sql in &super::super::MIGRATIONS[..version] {
            connection.execute_batch(sql).unwrap();
        }
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        let schema: String = connection
            .query_row(
                "SELECT group_concat(sql, char(10)) FROM sqlite_schema",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            repair_branch_version(&mut connection, version as i64).unwrap(),
            version as i64
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT group_concat(sql, char(10)) FROM sqlite_schema",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            schema
        );
    }
}

#[test]
fn v92_indexes_preserve_v91_rows_and_roll_back_failed_creation() {
    const INDEXES: [&str; 3] = [
        "burn_check_candidate_dependency_native_identity",
        "burn_check_observation_native_identity",
        "native_evidence_publication_correction",
    ];
    for fail_creation in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(database_path(directory.path())).unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        for migration in &super::super::MIGRATIONS[..91] {
            connection.execute_batch(migration).unwrap();
        }
        connection.pragma_update(None, "user_version", 91).unwrap();
        seed(&connection);
        for (id, binding) in [
            ("invalid-json", "{bad"),
            ("empty-binding", "{}"),
            (
                "partial-binding",
                r#"{"NativeCurrent":{"identity":"result"}}"#,
            ),
        ] {
            assert_eq!(
                connection
                    .execute(
                        "INSERT INTO burn_check_candidate_dependency
                     (candidate_id, observation_id, session_key_json, source_format,
                      native_revision, evidence_version, removed, payload_sha256,
                      event_kind, dependency_binding_json)
                     SELECT candidate_id, ?1, session_key_json, 'fixture', 1, 'saved',
                            0, '', 'tool_result', ?2
                       FROM burn_check_candidate WHERE candidate_id = 'accepted'",
                        params![id, binding],
                    )
                    .unwrap(),
                1
            );
        }
        let before = snapshot(&connection);
        let mut store = Store::open_in_memory(directory.path()).unwrap();
        *store.lock() = connection;
        if fail_creation {
            store.lock().execute_batch(
                "CREATE TABLE burn_check_candidate_dependency_native_identity (synthetic INTEGER)",
            ).unwrap();
            let failed_before = snapshot(&store.lock());
            assert!(store.migrate().is_err());
            assert_eq!(store.schema_version().unwrap(), 91);
            assert_eq!(snapshot(&store.lock()), failed_before);
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index'
                       AND name IN (?1, ?2, ?3)",
                        params![INDEXES[0], INDEXES[1], INDEXES[2]],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
            );
            store
                .lock()
                .execute_batch("DROP TABLE burn_check_candidate_dependency_native_identity")
                .unwrap();
        }
        store.migrate().unwrap();
        for _ in 0..2 {
            assert_eq!(store.schema_version().unwrap(), 92);
            {
                let connection = store.lock();
                assert_eq!(read_profile(&connection).unwrap(), Profile::canonical(92));
                for (table, (columns, expected)) in &before {
                    assert_eq!(rows(&connection, table, columns), *expected, "{table}");
                }
                let mut statement = connection
                    .prepare(
                        "SELECT name FROM sqlite_schema WHERE type = 'index'
                       AND name IN (?1, ?2, ?3) ORDER BY name",
                    )
                    .unwrap();
                let indexes = statement
                    .query_map(params![INDEXES[0], INDEXES[1], INDEXES[2]], |row| {
                        row.get::<_, String>(0)
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                assert_eq!(indexes, INDEXES.map(str::to_owned));
                assert_eq!(
                    connection
                        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                            row.get::<_, i64>(0)
                        })
                        .unwrap(),
                    0
                );
            }
            drop(store);
            store = Store::open(directory.path()).unwrap();
        }
    }
}

#[test]
fn pre_registry_branch_versions_keep_populated_state_through_open_and_reopen() {
    for version in 86..=90 {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(database_path(directory.path())).unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        history(&connection, false, 85);
        for sql in &super::super::MIGRATIONS[86..version + 1] {
            connection.execute_batch(sql).unwrap();
        }
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        assert_eq!(
            read_profile(&connection).unwrap(),
            Profile::pre_registry(version as i64)
        );
        seed(&connection);
        let before = snapshot(&connection);
        drop(connection);
        for _ in 0..2 {
            let store = Store::open(directory.path()).unwrap();
            let connection = store.lock();
            assert_eq!(read_profile(&connection).unwrap(), Profile::canonical(92));
            for (table, (columns, expected)) in &before {
                assert_eq!(
                    rows(&connection, table, columns),
                    *expected,
                    "version={version}, table={table}"
                );
            }
            assert!(!antiburn_checks_store::runtime_snapshot_ready(&connection).unwrap());
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0
            );
        }
    }
}

#[test]
fn main_registry_rows_and_initialization_survive_retirement_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(database_path(directory.path())).unwrap();
    history(&connection, false, 86);
    seed(&connection);
    connection
        .execute_batch(
            "INSERT INTO burn_check_runtime_definition VALUES
            ('fixture', '{}', 1, 'revision', 'Saved definition', 'paused');
         UPDATE burn_check_runtime_definition_state SET initialized = 1;",
        )
        .unwrap();
    let before = snapshot(&connection);
    drop(connection);
    for _ in 0..2 {
        let store = Store::open(directory.path()).unwrap();
        let connection = store.lock();
        assert!(antiburn_checks_store::runtime_snapshot_ready(&connection).unwrap());
        for table in [
            "burn_check_runtime_definition",
            "burn_check_runtime_definition_state",
        ] {
            let (columns, expected) = &before[table];
            assert_eq!(rows(&connection, table, columns), *expected);
        }
        assert_eq!(read_profile(&connection).unwrap(), Profile::canonical(92));
    }
}

#[test]
fn malformed_registry_shapes_fail_before_any_migration() {
    for sql in [
        "DROP TABLE burn_check_runtime_definition_state",
        "DELETE FROM burn_check_runtime_definition_state",
        "DROP TABLE burn_check_runtime_definition_state;
         CREATE TABLE burn_check_runtime_definition_state (
            singleton INTEGER PRIMARY KEY, protocol_version INTEGER, initialized INTEGER) STRICT;
         INSERT INTO burn_check_runtime_definition_state VALUES (1, 1, 1);",
        "ALTER TABLE burn_check_runtime_definition ADD COLUMN extra TEXT",
    ] {
        let mut connection = historical_connection(false, 86);
        connection.execute_batch(sql).unwrap();
        assert_profile_rejected(&mut connection, 86, "runtime");
    }
}

#[test]
fn failed_registry_repair_keeps_the_pre_registry_version_and_data() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let mut connection = historical_connection(false, 85);
    connection.execute_batch(super::super::V87).unwrap();
    connection.pragma_update(None, "user_version", 86).unwrap();
    seed(&connection);
    let before = snapshot(&connection);
    connection.authorizer(Some(|context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::Pragma {
                pragma_name: "user_version",
                pragma_value: Some("87"),
            }
        ) {
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }));
    assert!(repair_branch_version(&mut connection, 86).is_err());
    connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(snapshot(&connection), before);
    assert_eq!(
        read_profile(&connection).unwrap(),
        Profile::pre_registry(86)
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        86
    );
    assert_eq!(repair_branch_version(&mut connection, 86).unwrap(), 87);
}

#[test]
fn pre_registry_served_model_column_requires_its_known_usage_constraints() {
    let mut connection = historical_connection(false, 85);
    for sql in &super::super::MIGRATIONS[86..90] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute_batch("ALTER TABLE burn_check_decision_usage ADD COLUMN served_model TEXT")
        .unwrap();
    connection.pragma_update(None, "user_version", 90).unwrap();
    assert_profile_rejected(&mut connection, 90, "usage observation constraints");
}

#[test]
fn malformed_profiles_fail_without_schema_or_data_changes() {
    for (version, sql) in [
        (81, "DROP TABLE opencode_v2_source_status"),
        (81, "ALTER TABLE opencode_v2_capture DROP COLUMN cwd"),
        (
            82,
            "ALTER TABLE opencode_v2_capture DROP COLUMN retirement_reason",
        ),
        (83, "CREATE INDEX turn_quota_published ON turn(ts_ms)"),
        (84, "ALTER TABLE session ADD COLUMN source_format TEXT"),
        (
            85,
            "ALTER TABLE burn_check_candidate DROP COLUMN started_cooldown_until_epoch",
        ),
        (
            86,
            "ALTER TABLE opencode_v2_capture DROP COLUMN source_selection_json",
        ),
        (87, "DROP INDEX native_evidence_current_owner"),
    ] {
        let mut connection = historical_connection(true, version);
        connection.execute_batch(sql).unwrap();
        let before = snapshot(&connection);
        let schema: String = connection
            .query_row(
                "SELECT group_concat(sql, char(10)) FROM sqlite_schema",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let error = repair_branch_version(&mut connection, version as i64).unwrap_err();
        assert!(
            format!("{error:#}").contains(&format!("version {version}")),
            "{error:#}"
        );
        assert_eq!(snapshot(&connection), before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT group_concat(sql, char(10)) FROM sqlite_schema",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            schema
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            version as i64
        );
    }
}

#[test]
fn failed_backfills_roll_back_index_columns_trigger_rows_and_version() {
    for table in ["session", "native_tool_result_publication"] {
        let mut connection = historical_connection(true, 87);
        seed(&connection);
        connection.execute_batch(&format!("CREATE TRIGGER fail_backfill BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT, 'synthetic backfill failure'); END")).unwrap();
        let before = snapshot(&connection);
        let schema: String = connection
            .query_row(
                "SELECT group_concat(sql, char(10)) FROM sqlite_schema",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(repair_branch_version(&mut connection, 87).is_err());
        assert_eq!(snapshot(&connection), before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT group_concat(sql, char(10)) FROM sqlite_schema",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            schema
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            87
        );
        assert!(
            connection
                .execute("UPDATE native_tool_result_publication SET complete = 0", [])
                .is_err()
        );
        connection
            .execute_batch("DROP TRIGGER fail_backfill")
            .unwrap();
        assert_eq!(repair_branch_version(&mut connection, 87).unwrap(), 90);
    }
}

fn assert_profile_rejected(connection: &mut Connection, version: i64, detail: &str) {
    let before = snapshot(connection);
    let schema_columns = ["type", "name", "tbl_name", "rootpage", "sql"].map(String::from);
    let sequence_columns = ["name", "seq"].map(String::from);
    let schema = rows(connection, "sqlite_schema", &schema_columns);
    let sequences = rows(connection, "sqlite_sequence", &sequence_columns);
    let error = repair_branch_version(connection, version).unwrap_err();
    let error = format!("{error:#}");
    assert!(error.contains(&format!("version {version}")), "{error}");
    assert!(error.contains(detail), "{error}");
    assert_eq!(snapshot(connection), before);
    assert_eq!(rows(connection, "sqlite_schema", &schema_columns), schema);
    assert_eq!(
        rows(connection, "sqlite_sequence", &sequence_columns),
        sequences
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        version
    );
}

#[test]
fn named_decision_profiles_reject_extra_legacy_unique_keys_under_any_name() {
    for (name, columns) in [
        (
            "retained_legacy_attempt_key",
            "candidate_id, attempt_number",
        ),
        ("unexpected_key", "attempt_number, candidate_id"),
    ] {
        let mut connection = historical_connection(true, 87);
        seed(&connection);
        connection
            .execute_batch(&format!(
                "CREATE UNIQUE INDEX {name}
            ON burn_check_evaluation_attempt({columns})"
            ))
            .unwrap();
        assert_profile_rejected(
            &mut connection,
            87,
            "invalid named-decision unique-key layout",
        );
    }
}

fn insert_host_publication(connection: &Connection, event_kind: &str) -> rusqlite::Result<usize> {
    connection.execute("INSERT INTO native_tool_result_publication (outcome, environment_key,
        agent, session_id, source_format, payload_json, complete, source_generation, claim_fence,
        event_kind, target_check_id, target_scope_key, target_generation, target_branch_key,
        target_incarnation, target_attachment_epoch)
        VALUES ('update', 'native', 'opencode', 'v2', '\"open_code_v2_sqlite\"', '{ \"host\": true }',
            1, 7, 40, ?1, 'fixture', '{}', 1, 'accepted', 12, 'epoch')", [event_kind])
}

#[test]
fn malformed_publication_nullability_and_host_constraints_are_rejected_without_mutation() {
    for (from, to) in [
        ("native_identity TEXT,", "native_identity TEXT NOT NULL,"),
        (
            "native_position_json TEXT CHECK",
            "native_position_json TEXT NOT NULL CHECK",
        ),
        (
            "native_revision INTEGER,",
            "native_revision INTEGER NOT NULL,",
        ),
        ("'session_start', 'session_idle'", "'session_idle'"),
        (
            "event_kind IN ('session_start', 'session_idle') AND outcome = 'update'",
            "event_kind IN ('session_start', 'session_idle') AND outcome = 'removal'",
        ),
        (
            ") STRICT;",
            ", CHECK (native_identity IS NOT NULL)) STRICT;",
        ),
    ] {
        let mut connection = historical_connection(true, 85);
        let steps = migration_steps(include_str!("branch_history.sql"));
        let sql = steps[5].replacen(from, to, 1);
        assert_ne!(sql, steps[5]);
        connection.execute_batch(&sql).unwrap();
        connection.execute_batch(steps[6]).unwrap();
        connection.pragma_update(None, "user_version", 87).unwrap();
        connection.execute_batch(r#"
            INSERT INTO setting VALUES ('synthetic-probe', '{ "saved": true }');
            INSERT INTO burn_check_activation VALUES ('fixture', '{}', 'revision', 1, 1, 3, 7, 91,
                '{ "cursor": 7 }', '{ "state": "active" }');
            INSERT INTO burn_check_candidate (candidate_id, check_id, activation_scope_key,
                generation, branch_key, session_key_json, trigger_position, selected_publication_head,
                dependencies_json, frozen_input_json, answer_json, input_fingerprint, state,
                created_at_epoch, updated_at_epoch)
            VALUES ('accepted', 'fixture', '{}', 1, 'accepted', '["native","opencode","v2"]', 1, 7,
                '{}', '{ "frozen": true }', '{ "answer": true }', 'fingerprint', 'accepted', 90, 91);
            INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name,
                attempt_number, state, outcome, response_json, updated_at_epoch)
            VALUES ('saved', 'accepted', 'review', 1, 'settled', 'answer', '{ "answer": true }', 91);
            INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key,
                agent, session_id, source_format, native_identity, native_position_json, native_revision,
                payload_json, complete, source_generation, claim_fence)
            VALUES (7, 'update', 'native', 'opencode', 'v2', '"open_code_v2_sqlite"', 'identity', '{}', 2,
                '{ "frozen": "exact bytes" }', 1, 7, 40);
            UPDATE sqlite_sequence SET seq = 42 WHERE name = 'native_tool_result_publication';
        "#).unwrap();
        assert!(insert_host_publication(&connection, "session_start").is_err());
        assert_profile_rejected(
            &mut connection,
            87,
            "invalid public evidence publication constraints",
        );
    }
}

#[test]
fn evidence_index_and_publication_guard_definitions_are_checked() {
    for (kind, name, sql) in [
        (
            "INDEX",
            "native_tool_result_publication_session",
            "CREATE INDEX native_tool_result_publication_session ON native_tool_result_publication(session_id, publication_sequence)",
        ),
        (
            "INDEX",
            "native_tool_result_publication_identity",
            "CREATE INDEX native_tool_result_publication_identity ON native_tool_result_publication(native_identity)",
        ),
        (
            "INDEX",
            "native_evidence_publication_target",
            "CREATE INDEX native_evidence_publication_target ON native_tool_result_publication(target_scope_key, publication_sequence)",
        ),
        (
            "INDEX",
            "native_evidence_publication_unsafe",
            "CREATE INDEX native_evidence_publication_unsafe ON native_tool_result_publication(environment_key, agent, session_id, source_format, session_incarnation, project_cwd, publication_sequence) WHERE outcome = 'coverage_gap' AND gap_reason IN ('NATIVE-UNSAFE-IDENTITY', 'native-unsafe-order', 'native-unsafe-ownership')",
        ),
        (
            "INDEX",
            "native_tool_result_current_session",
            "CREATE INDEX native_tool_result_current_session ON native_tool_result_current(session_id)",
        ),
        (
            "INDEX",
            "native_evidence_current_owner",
            "CREATE INDEX native_evidence_current_owner ON native_tool_result_current(owner_message_id)",
        ),
        (
            "INDEX",
            "burn_check_attachment_idle_due",
            "CREATE INDEX burn_check_attachment_idle_due ON burn_check_session_attachment(idle_deadline_ms) WHERE observer_epoch IS NOT NULL AND continuity_eligible = 0 AND idle_emitted_revision IS NULL",
        ),
        (
            "TRIGGER",
            "native_tool_result_publication_no_update",
            "CREATE TRIGGER native_tool_result_publication_no_update BEFORE UPDATE ON native_tool_result_publication BEGIN SELECT 1; END",
        ),
        (
            "TRIGGER",
            "native_tool_result_publication_no_delete",
            "CREATE TRIGGER native_tool_result_publication_no_delete BEFORE DELETE ON native_tool_result_publication WHEN 0 BEGIN SELECT RAISE(ABORT, 'native tool-result publications are append-only'); END",
        ),
    ] {
        let mut connection = historical_connection(true, 87);
        seed(&connection);
        connection
            .execute_batch(&format!("DROP {kind} {name}; {sql}"))
            .unwrap();
        assert_profile_rejected(
            &mut connection,
            87,
            if kind == "INDEX" {
                "invalid migration index"
            } else {
                "invalid publication guard"
            },
        );
    }
}

#[test]
fn valid_evidence_histories_accept_host_events_and_a_second_decision_block() {
    for (branch, version) in [(true, 86), (true, 87), (false, 85)] {
        let connection = historical_connection(branch, version);
        seed(&connection);
        let directory = tempfile::tempdir().unwrap();
        let store = Store::from_connection(connection, directory.path().to_path_buf()).unwrap();
        for event_kind in ["session_start", "session_idle"] {
            assert_eq!(
                insert_host_publication(&store.lock(), event_kind).unwrap(),
                1
            );
        }
        store
            .lock()
            .execute(
                "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id,
            decision_name, attempt_number, state, updated_at_epoch)
            VALUES ('second-block', 'accepted', 'block-two', 1, 'prepared', 92)",
                [],
            )
            .unwrap();
    }
}

#[test]
fn failed_evidence_migration_rolls_back_temporary_work_and_keeps_the_version() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let connection = historical_connection(false, 85);
    for sql in &super::super::MIGRATIONS[85..88] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 88).unwrap();
    seed(&connection);
    connection
        .execute_batch("CREATE TEMP TABLE current_evidence_copy (synthetic INTEGER)")
        .unwrap();
    let before = snapshot(&connection);
    *store.lock() = connection;
    assert!(store.migrate().is_err());
    assert_eq!(store.schema_version().unwrap(), 88);
    assert_eq!(snapshot(&store.lock()), before);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_temp_schema WHERE name = 'publication_high_water'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    store
        .lock()
        .execute_batch("DROP TABLE current_evidence_copy")
        .unwrap();
    store.migrate().unwrap();
    assert_eq!(store.schema_version().unwrap(), 92);
}

#[test]
fn incoming_source_and_gap_backfills_survive_the_evidence_rebuild() {
    let connection = historical_connection(false, 84);
    seed(&connection);
    let directory = tempfile::tempdir().unwrap();
    let store = Store::from_connection(connection, directory.path().to_path_buf()).unwrap();
    let connection = store.lock();
    for (session, format) in [
        ("v1", "\"open_code_sqlite_v2\""),
        ("v2", "\"open_code_v2_sqlite\""),
        ("export", "\"open_code_jsonl\""),
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT source_format FROM session WHERE session_id = ?1",
                    [session],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            format
        );
    }
    for (session, reason, expected) in [
        ("v2", Some("v2-history-incomplete"), "partial"),
        ("v2", Some("fixture-segment"), "segment_reset"),
        ("v1", None, "invalidated"),
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT gap_kind FROM native_tool_result_publication
            WHERE outcome = 'coverage_gap' AND session_id = ?1 AND gap_reason IS ?2",
                    rusqlite::params![session, reason],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            expected
        );
    }
}

fn insert_gap(
    connection: &Connection,
    agent: &str,
    format: &str,
    reason: &str,
    kind: Option<&str>,
) -> i64 {
    let session = match (agent, format) {
        ("claude-code", _) => "file",
        ("opencode", "\"open_code_sqlite_v2\"") => "v1",
        ("opencode", _) => "v2",
        _ => "gap-fixture",
    };
    connection
        .execute(
            "INSERT OR IGNORE INTO session (environment_key, agent, session_id,
        source_kind, source_label, first_seen_at, last_seen_at)
        VALUES ('native', ?1, ?2, 'file', 'synthetic-gap-fixture', 'now', 'now')",
            [agent, session],
        )
        .unwrap();
    let sql = "INSERT INTO native_tool_result_publication (outcome, environment_key, agent,
        session_id, source_format, complete, source_generation, claim_fence, gap_reason";
    if let Some(kind) = kind {
        connection
            .execute(
                &format!(
                    "{sql}, gap_kind) VALUES ('coverage_gap', 'native', ?1,
            ?2, ?3, 0, 7, 40, ?4, ?5)"
                ),
                rusqlite::params![agent, session, format, reason, kind],
            )
            .unwrap();
    } else {
        connection
            .execute(
                &format!(
                    "{sql}) VALUES ('coverage_gap', 'native', ?1,
            ?2, ?3, 0, 7, 40, ?4)"
                ),
                rusqlite::params![agent, session, format, reason],
            )
            .unwrap();
    }
    connection.last_insert_rowid()
}

fn gap_kind(connection: &Connection, sequence: i64) -> String {
    connection
        .query_row(
            "SELECT gap_kind FROM native_tool_result_publication
        WHERE publication_sequence = ?1",
            [sequence],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn untyped_soft_history_gaps_stay_partial_without_changing_accepted_data_or_progress() {
    for (branch, versions) in [(true, 81..=87), (false, 80..=84)] {
        for version in versions {
            let connection = historical_connection(branch, version);
            seed(&connection);
            let cases = [
                (
                    "claude-code",
                    "\"claude_jsonl\"",
                    "file-history-incomplete",
                    "partial",
                ),
                (
                    "codex",
                    "\"codex_rollout_jsonl\"",
                    "file-history-incomplete",
                    "partial",
                ),
                (
                    "pi",
                    "\"pi_v3_jsonl\"",
                    "file-history-incomplete",
                    "partial",
                ),
                (
                    "opencode",
                    "\"open_code_sqlite_v2\"",
                    "journal-history-incomplete",
                    "partial",
                ),
                (
                    "claude-code",
                    "\"claude_jsonl\"",
                    "native-unsafe-identity",
                    "invalidated",
                ),
                (
                    "opencode",
                    "\"open_code_sqlite_v2\"",
                    "journal-prefix-changed",
                    "invalidated",
                ),
                (
                    "claude-code",
                    "\"claude_jsonl\"",
                    "file-history-incomplete-extra",
                    "invalidated",
                ),
                (
                    "codex",
                    "\"claude_jsonl\"",
                    "file-history-incomplete",
                    "invalidated",
                ),
                (
                    "opencode",
                    "\"open_code_v2_sqlite\"",
                    "capture-lost",
                    "segment_reset",
                ),
                (
                    "opencode",
                    "\"open_code_v2_sqlite\"",
                    "journal-history-incomplete",
                    "segment_reset",
                ),
            ];
            let expected: Vec<_> = cases
                .into_iter()
                .map(|(agent, format, reason, expected)| {
                    (
                        insert_gap(&connection, agent, format, reason, None),
                        expected,
                    )
                })
                .collect();
            let before = snapshot(&connection);
            let head = connection.last_insert_rowid();
            let directory = tempfile::tempdir().unwrap();
            let store = Store::from_connection(connection, directory.path().to_path_buf()).unwrap();
            for (sequence, expected) in expected {
                assert_eq!(
                    gap_kind(&store.lock(), sequence),
                    expected,
                    "branch={branch}, version={version}, sequence={sequence}"
                );
            }
            for (table, (columns, expected)) in &before {
                assert_eq!(
                    rows(&store.lock(), table, columns),
                    *expected,
                    "branch={branch}, version={version}, table={table}"
                );
            }
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT seq FROM sqlite_sequence
                WHERE name = 'native_tool_result_publication'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                head
            );
            assert!(store.internal_value(UNTYPED_GAP_HEAD_KEY).is_none());
        }
    }
}

#[test]
fn existing_typed_history_gap_values_are_not_reclassified() {
    let connection = historical_connection(false, 85);
    seed(&connection);
    let mut expected = Vec::new();
    for kind in ["invalidated", "partial", "segment_reset"] {
        for (agent, format, reason) in [
            ("claude-code", "\"claude_jsonl\"", "file-history-incomplete"),
            (
                "opencode",
                "\"open_code_sqlite_v2\"",
                "journal-history-incomplete",
            ),
        ] {
            expected.push((
                insert_gap(&connection, agent, format, reason, Some(kind)),
                kind,
            ));
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let store = Store::from_connection(connection, directory.path().to_path_buf()).unwrap();
    for (sequence, expected) in expected {
        assert_eq!(gap_kind(&store.lock(), sequence), expected);
    }
    assert!(store.internal_value(UNTYPED_GAP_HEAD_KEY).is_none());
}

#[test]
fn interrupted_untyped_upgrade_keeps_its_boundary_and_preserves_newer_typed_gaps() {
    for branch in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = Connection::open(database_path(directory.path())).unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        history(&connection, branch, if branch { 86 } else { 84 });
        seed(&connection);
        let old_gap = insert_gap(
            &connection,
            "claude-code",
            "\"claude_jsonl\"",
            "file-history-incomplete",
            None,
        );
        if branch {
            assert_eq!(repair_branch_version(&mut connection, 86).unwrap(), 89);
        } else {
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_retirement BEFORE UPDATE ON setting
            WHEN OLD.key = 'internal:yamlBurnChecksDiscoveryEnabledV1'
            BEGIN SELECT RAISE(ABORT, 'synthetic retirement failure'); END",
                )
                .unwrap();
            assert!(Store::from_connection(connection, directory.path().to_path_buf()).is_err());
            connection = Connection::open(database_path(directory.path())).unwrap();
            assert_eq!(
                connection
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                86
            );
            connection
                .execute_batch("DROP TRIGGER fail_retirement")
                .unwrap();
        }
        assert_eq!(gap_kind(&connection, old_gap), "invalidated");
        let new_gap = insert_gap(
            &connection,
            "claude-code",
            "\"claude_jsonl\"",
            "file-history-incomplete",
            Some("invalidated"),
        );
        assert!(new_gap > old_gap);
        drop(connection);
        for _ in 0..2 {
            let store = Store::open(directory.path()).unwrap();
            assert_eq!(store.schema_version().unwrap(), 92);
            assert_eq!(gap_kind(&store.lock(), old_gap), "partial");
            assert_eq!(gap_kind(&store.lock(), new_gap), "invalidated");
            assert!(store.internal_value(UNTYPED_GAP_HEAD_KEY).is_none());
        }
    }
}
