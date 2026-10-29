use antiburn_checks_store::{
    native_file_receipt_compatible_sql, opencode_journal_receipt_compatible_sql,
    source_ready_at_sql,
};
use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION};
use rusqlite::{Connection, params};
use serde_json::json;

fn file_database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("fixtures/session.sql"))
        .unwrap();
    connection
        .execute(
            "UPDATE session SET agent = 'codex', source_format = '\"codex_rollout_jsonl\"'",
            [],
        )
        .unwrap();
    let cursor = file_cursor(5);
    connection.execute("UPDATE native_file_delivery SET agent = 'codex', parser_revision = ?1, cursor_json = ?2", params![PARSER_REVISION, cursor]).unwrap();
    connection
}

fn file_cursor(revision: u32) -> String {
    json!({"revision": revision, "agent": "codex", "session_id": "s1", "source_format": "codex_rollout_jsonl"}).to_string()
}

fn journal_database() -> Connection {
    let connection = file_database();
    connection.execute("UPDATE session SET agent = 'opencode', source_kind = 'providerDb', source_format = '\"open_code_sqlite_v2\"'", []).unwrap();
    connection.execute("INSERT INTO opencode_journal_delivery VALUES ('native','opencode','s1',7,'source','/repo',?1,?2,0,1,NULL)", params![PARSER_REVISION, journal_cursor(Some(PARSER_REVISION), 5)]).unwrap();
    connection
}

fn journal_cursor(parser: Option<i64>, revision: u32) -> String {
    json!({"session_id": "s1", "parser_revision": parser, "through_sequence": 7,
        "live": {"revision": revision, "baseline": 0, "public_baseline": 3}})
    .to_string()
}

fn compatible(connection: &Connection, table: &str, predicate: String) -> bool {
    connection
        .query_row(
            &format!(
                "SELECT ({predicate}) FROM session s LEFT JOIN {table} d
                  USING(environment_key, agent, session_id)"
            ),
            [PARSER_REVISION],
            |row| row.get(0),
        )
        .unwrap()
}

fn file_compatible(connection: &Connection) -> bool {
    compatible(
        connection,
        "native_file_delivery",
        native_file_receipt_compatible_sql(1),
    )
}

fn journal_compatible(connection: &Connection) -> bool {
    compatible(
        connection,
        "opencode_journal_delivery",
        opencode_journal_receipt_compatible_sql(1),
    )
}

fn ready(connection: &Connection) -> bool {
    let predicate = source_ready_at_sql(1, 2, 3, "200");
    connection
        .query_row(
            &format!(
                "SELECT COALESCE(({predicate}), 0) FROM session s
                  LEFT JOIN session_evidence e USING(environment_key, agent, session_id)"
            ),
            params![PARSER_REVISION, ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn file_conversion_and_current_layouts_are_distinct_from_readiness() {
    for parser in [
        None,
        Some(62),
        Some(63),
        Some(64),
        Some(65),
        Some(66),
        Some(PARSER_REVISION),
        Some(999),
    ] {
        for revision in [1, 2, 4, 5, 6] {
            let connection = file_database();
            connection
                .execute(
                    "UPDATE native_file_delivery SET parser_revision = ?1, cursor_json = ?2",
                    params![parser, file_cursor(revision)],
                )
                .unwrap();
            let current_layout = parser
                .is_some_and(|value| [63, 64, 65, 66, PARSER_REVISION].contains(&value))
                && revision == 5;
            assert_eq!(
                file_compatible(&connection),
                current_layout
                    || (parser == Some(62) && revision == 4)
                    || (parser == Some(64) && revision == 1),
                "parser {parser:?}, cursor {revision}"
            );
            assert_eq!(ready(&connection), current_layout);
        }
    }
}

#[test]
fn journal_conversion_and_current_layouts_are_distinct_from_readiness() {
    for parser in [
        None,
        Some(62),
        Some(63),
        Some(64),
        Some(65),
        Some(66),
        Some(PARSER_REVISION),
        Some(999),
    ] {
        for revision in [1, 2, 3, 4, 5, 6] {
            let connection = journal_database();
            connection
                .execute(
                    "UPDATE opencode_journal_delivery SET parser_revision = ?1, cursor_json = ?2",
                    params![parser, journal_cursor(parser, revision)],
                )
                .unwrap();
            let current_layout = parser
                .is_some_and(|value| [63, 64, 65, 66, PARSER_REVISION].contains(&value))
                && revision == 5;
            assert_eq!(
                journal_compatible(&connection),
                current_layout
                    || (parser == Some(62) && revision == 4)
                    || (parser == Some(64) && revision == 2),
                "parser {parser:?}, live state {revision}"
            );
            assert_eq!(ready(&connection), current_layout);
        }
    }
}

#[test]
fn empty_receipts_allow_initialization_but_never_make_a_source_ready() {
    for parser in [
        None,
        Some(62),
        Some(63),
        Some(64),
        Some(65),
        Some(66),
        Some(PARSER_REVISION),
        Some(999),
    ] {
        let file = file_database();
        file.execute(
            "UPDATE native_file_delivery SET parser_revision = ?1, cursor_json = NULL",
            [parser],
        )
        .unwrap();
        assert_eq!(file_compatible(&file), parser != Some(999));
        assert!(!ready(&file));

        let journal = journal_database();
        journal
            .execute(
                "UPDATE opencode_journal_delivery SET parser_revision = ?1, cursor_json = NULL",
                [parser],
            )
            .unwrap();
        assert_eq!(journal_compatible(&journal), parser != Some(999));
        assert!(!ready(&journal));
    }
    let file = file_database();
    file.execute("DELETE FROM native_file_delivery", [])
        .unwrap();
    assert!(file_compatible(&file));
    assert!(!ready(&file));

    let journal = journal_database();
    journal
        .execute("DELETE FROM opencode_journal_delivery", [])
        .unwrap();
    assert!(journal_compatible(&journal));
    assert!(!ready(&journal));
}

#[test]
fn compatibility_does_not_grant_health_or_replace_source_binding_policy() {
    let file = file_database();
    file.execute(
        "UPDATE native_file_delivery SET incarnation = 8, source_label = 'old', blocked = 1",
        [],
    )
    .unwrap();
    assert!(file_compatible(&file));
    assert!(!ready(&file));

    let journal = journal_database();
    journal.execute("UPDATE opencode_journal_delivery SET incarnation = 8, source_label = 'old', blocked = 1", []).unwrap();
    assert!(journal_compatible(&journal));
    assert!(!ready(&journal));
}

#[test]
fn conversion_receipts_still_need_matching_public_identity_fields() {
    let file = file_database();
    file.execute(
        "UPDATE native_file_delivery SET parser_revision = 62, cursor_json = ?1",
        [file_cursor(4)],
    )
    .unwrap();
    assert!(file_compatible(&file));
    file.execute("UPDATE native_file_delivery SET cursor_json = json_set(cursor_json, '$.session_id', 'other')", []).unwrap();
    assert!(!file_compatible(&file));

    let journal = journal_database();
    journal
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = 62, cursor_json = ?1",
            [journal_cursor(Some(62), 4)],
        )
        .unwrap();
    assert!(journal_compatible(&journal));
    journal.execute("UPDATE opencode_journal_delivery SET cursor_json = json_remove(cursor_json, '$.live.public_baseline')", []).unwrap();
    assert!(journal_compatible(&journal));
    assert!(!ready(&journal));
    journal.execute("UPDATE opencode_journal_delivery SET cursor_json = json_set(cursor_json, '$.parser_revision', 64)", []).unwrap();
    assert!(!journal_compatible(&journal));
}

#[test]
fn pinned_main_pairs_keep_public_identity_and_baseline_gates() {
    let file = file_database();
    file.execute(
        "UPDATE native_file_delivery SET parser_revision = 64, cursor_json = ?1",
        [file_cursor(1)],
    )
    .unwrap();
    assert!(file_compatible(&file));
    assert!(!ready(&file));
    for (field, value) in [
        ("session_id", "other"),
        ("agent", "pi"),
        ("source_format", "pi_v3_jsonl"),
    ] {
        let mut cursor: serde_json::Value = serde_json::from_str(&file_cursor(1)).unwrap();
        cursor[field] = json!(value);
        file.execute(
            "UPDATE native_file_delivery SET cursor_json = ?1",
            [cursor.to_string()],
        )
        .unwrap();
        assert!(!file_compatible(&file), "{field}");
        assert!(!ready(&file));
    }

    let journal = journal_database();
    journal
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = 64, cursor_json = ?1",
            [journal_cursor(Some(64), 2)],
        )
        .unwrap();
    assert!(journal_compatible(&journal));
    assert!(!ready(&journal));
    for encoded in [
        None,
        Some(62),
        Some(63),
        Some(65),
        Some(PARSER_REVISION),
        Some(999),
    ] {
        journal
            .execute(
                "UPDATE opencode_journal_delivery SET cursor_json = ?1",
                [journal_cursor(encoded, 2)],
            )
            .unwrap();
        assert!(!journal_compatible(&journal));
    }
    let mut wrong_identity: serde_json::Value =
        serde_json::from_str(&journal_cursor(Some(64), 2)).unwrap();
    wrong_identity["session_id"] = json!("other");
    journal
        .execute(
            "UPDATE opencode_journal_delivery SET cursor_json = ?1",
            [wrong_identity.to_string()],
        )
        .unwrap();
    assert!(!journal_compatible(&journal));
    for baseline in [json!(null), json!(-1), json!(1.5), json!("0"), json!(true)] {
        let mut cursor: serde_json::Value =
            serde_json::from_str(&journal_cursor(Some(64), 2)).unwrap();
        cursor["live"]["baseline"] = baseline;
        journal
            .execute(
                "UPDATE opencode_journal_delivery SET cursor_json = ?1",
                [cursor.to_string()],
            )
            .unwrap();
        assert!(!journal_compatible(&journal));
        assert!(!ready(&journal));
    }
}

#[test]
fn current_journals_reject_malformed_public_baselines() {
    let journal = journal_database();
    for baseline in [json!(null), json!(-1), json!(1.5), json!("0"), json!(true)] {
        let mut cursor: serde_json::Value =
            serde_json::from_str(&journal_cursor(Some(PARSER_REVISION), 5)).unwrap();
        cursor["live"]["public_baseline"] = baseline;
        journal
            .execute(
                "UPDATE opencode_journal_delivery SET cursor_json = ?1",
                [cursor.to_string()],
            )
            .unwrap();
        assert!(!journal_compatible(&journal));
        assert!(!ready(&journal));
    }
}
