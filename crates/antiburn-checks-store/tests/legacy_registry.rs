use antiburn_checks_store::{Eligibility, read_session_checks};
use rusqlite::Connection;

fn database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("fixtures/registry_v82.sql"))
        .unwrap();
    connection
}

#[test]
fn legacy_registry_withholds_yaml_projection_without_writes() {
    for version in 81..=83 {
        let connection = database();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        connection.pragma_update(None, "query_only", true).unwrap();
        let before = connection.total_changes();
        assert!(
            read_session_checks(&connection, "native", "codex", "s1")
                .unwrap()
                .is_empty()
        );
        assert_eq!(connection.total_changes(), before);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            version
        );
        assert!(connection.execute("DELETE FROM session", []).is_err());
    }
}

#[test]
fn legacy_registry_withholding_still_requires_status_history_columns() {
    for sql in [
        "ALTER TABLE burn_check_assessment_history DROP COLUMN check_id",
        "ALTER TABLE burn_check_assessment_history DROP COLUMN activation_scope_key",
        "ALTER TABLE burn_check_assessment_history DROP COLUMN branch_key",
        "ALTER TABLE burn_check_assessment_history DROP COLUMN trigger_position",
        "ALTER TABLE burn_check_current_status DROP COLUMN position",
    ] {
        let connection = database();
        connection.execute_batch(sql).unwrap();
        connection.pragma_update(None, "query_only", true).unwrap();
        assert!(read_session_checks(&connection, "native", "codex", "s1").is_err());
    }
}

#[test]
fn legacy_registry_withholding_still_requires_source_columns() {
    for (table, column) in [
        ("session", "environment_key"),
        ("session", "agent"),
        ("session", "session_id"),
        ("session", "cwd"),
        ("session", "source_kind"),
        ("session", "source_generation"),
        ("session", "incarnation"),
        ("session", "source_label"),
        ("session", "wsl_distro"),
        ("session_relation", "environment_key"),
        ("session_relation", "agent"),
        ("session_relation", "session_id"),
        ("session_relation", "related_id"),
        ("session_relation", "kind"),
        ("session_evidence", "environment_key"),
        ("session_evidence", "agent"),
        ("session_evidence", "session_id"),
        ("session_evidence", "status"),
        ("session_evidence", "analyzed_generation"),
        ("session_evidence", "parser_revision"),
        ("session_evidence", "analyzer_revision"),
        ("session_evidence", "evidence_schema_revision"),
        ("native_file_delivery", "cursor_json"),
        ("opencode_journal_delivery", "cursor_json"),
        ("opencode_journal_delivery", "main_session"),
        ("native_tool_result_publication", "source_format"),
        ("native_tool_result_publication", "session_incarnation"),
        ("native_tool_result_publication", "project_cwd"),
        ("native_tool_result_publication", "gap_reason"),
    ] {
        let connection = database();
        connection
            .execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column}"))
            .unwrap();
        connection.pragma_update(None, "query_only", true).unwrap();
        let before = connection.total_changes();
        let error = read_session_checks(&connection, "native", "codex", "s1").unwrap_err();
        assert!(
            format!("{error:#}").contains(column),
            "{table}.{column}: {error:#}"
        );
        assert_eq!(connection.total_changes(), before);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            82
        );
    }
}

#[test]
fn incomplete_or_current_source_schema_is_not_a_legacy_fallback() {
    for sql in [
        "PRAGMA user_version = 92",
        "PRAGMA user_version = 84",
        "ALTER TABLE session ADD COLUMN source_format TEXT",
        "ALTER TABLE native_tool_result_publication ADD COLUMN gap_kind TEXT",
        "CREATE TABLE opencode_v2_capture (session_id TEXT)",
        "CREATE TABLE opencode_v2_selection (session_id TEXT)",
        "CREATE TABLE opencode_v2_source_status (session_id TEXT)",
        "CREATE INDEX session_relation_related ON session_relation(related_id)",
        "DROP TABLE native_tool_result_publication",
        "DROP TABLE native_file_delivery",
        "DROP TABLE opencode_journal_delivery",
    ] {
        let connection = database();
        connection.execute_batch(sql).unwrap();
        connection.pragma_update(None, "query_only", true).unwrap();
        assert!(
            read_session_checks(&connection, "native", "codex", "s1").is_err(),
            "{sql}"
        );
    }
}

#[test]
fn main_v86_receipt_does_not_restore_saved_yaml_status() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("fixtures/session.sql"))
        .unwrap();
    connection.execute_batch(r#"
        PRAGMA user_version = 86;
        UPDATE session SET agent = 'codex', source_format = '"codex_rollout_jsonl"';
        UPDATE native_file_delivery SET agent = 'codex', parser_revision = 64,
            cursor_json = '{"revision":1,"agent":"codex","session_id":"s1","source_format":"codex_rollout_jsonl"}';
        INSERT INTO burn_check_activation VALUES
            ('retry-loop', '{"environment":"native","session_cwd":"/repo","run":"Live"}', 2, 1, 'revision');
        INSERT INTO burn_check_runtime_definition
            SELECT check_id, activation_scope_key, generation, definition_revision,
                'Saved check', 'available' FROM burn_check_activation;
        INSERT INTO burn_check_assessment_history VALUES
            ('a1', 2, 'assessed', 1,
             '{"current_status":"Failure","outcomes":[{"index":0,"outcome":{"Applied":{"result":"Failure","message":"Saved YAML finding."}}}]}',
             123, 'retry-loop', '{"environment":"native","session_cwd":"/repo","run":"Live"}',
             '["native","codex","s1"]', 4);
        INSERT INTO burn_check_current_status VALUES
            ('retry-loop', '{"environment":"native","session_cwd":"/repo","run":"Live"}',
             2, '["native","codex","s1"]', 'failure', 'a1', 1, 4);
    "#).unwrap();
    connection.pragma_update(None, "query_only", true).unwrap();
    let before = connection.total_changes();
    let checks = read_session_checks(&connection, "native", "codex", "s1").unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, None);
    assert_eq!(checks[0].message, None);
    assert_eq!(checks[0].assessed_at_epoch, None);
    assert_ne!(checks[0].eligibility, Eligibility::Ready);
    assert_eq!(connection.total_changes(), before);
}
