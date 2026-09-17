use std::collections::BTreeSet;

use rusqlite::Connection;
use serde_json::Value;

const MANIFEST: &str = include_str!("fixtures/source_contracts/cases.json");
const INVALID_CASES: &str = include_str!("fixtures/source_contracts/invalid_cases.jsonl");
const BOUNDARY_CASES: &str = include_str!("fixtures/source_contracts/boundary_cases.jsonl");

#[test]
fn manifest_pins_each_source_contract() {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    for family in [
        "copilot",
        "cline",
        "amp",
        "devin_local",
        "antigravity",
        "kiro",
        "cursor",
    ] {
        let entry = &manifest[family];
        assert!(entry.is_object(), "missing {family} contract");
        assert!(entry["required_cases"].as_array().unwrap().len() >= 5);
        assert!(entry.get("required_fields").is_some() || entry.get("required_tables").is_some());
    }
}

#[test]
fn invalid_case_manifest_covers_each_source_family() {
    let families: BTreeSet<_> = INVALID_CASES
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["family"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let expected: BTreeSet<String> = [
        "amp",
        "antigravity",
        "cline",
        "copilot",
        "cursor",
        "devin_local",
        "kiro",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(families, expected);
}

#[test]
fn boundary_cases_match_the_declared_contract() {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let mut declared = BTreeSet::new();
    for family in [
        "copilot",
        "cline",
        "amp",
        "devin_local",
        "antigravity",
        "kiro",
        "cursor",
    ] {
        for case in manifest[family]["required_cases"].as_array().unwrap() {
            declared.insert((family.to_owned(), case.as_str().unwrap().to_owned()));
        }
    }
    let actual: BTreeSet<_> = BOUNDARY_CASES
        .lines()
        .map(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            (
                value["family"].as_str().unwrap().to_owned(),
                value["case"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(actual, declared);
}

#[test]
fn devin_local_migration_17_source_has_required_tables() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 17;
         CREATE TABLE sessions (session_id TEXT PRIMARY KEY, model TEXT, head_node_id TEXT);
         CREATE TABLE message_nodes (session_id TEXT, node_id TEXT, parent_node_id TEXT);
         CREATE TABLE subagent_heads (session_id TEXT, tool_call_id TEXT, child_session_id TEXT);
         CREATE TABLE tool_call_state (session_id TEXT, tool_call_id TEXT, kind TEXT);",
        )
        .unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        17
    );
    for table in [
        "sessions",
        "message_nodes",
        "subagent_heads",
        "tool_call_state",
    ] {
        assert!(
            connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(())
                )
                .is_ok(),
            "missing {table}"
        );
    }
}

#[test]
fn copilot_schema_7_source_has_required_usage_columns() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 7;
         CREATE TABLE sessions (session_id TEXT PRIMARY KEY, shutdown_model TEXT);
         CREATE TABLE request_usage (
             session_id TEXT, request_id TEXT, agent_id TEXT,
             input_tokens INTEGER, output_tokens INTEGER,
             cache_read_tokens INTEGER, cache_write_tokens INTEGER
         );",
        )
        .unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let columns: BTreeSet<String> = connection
        .prepare("PRAGMA table_info(request_usage)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .map(|column| column.unwrap())
        .collect();
    assert!(
        columns.is_superset(
            &[
                "session_id",
                "request_id",
                "agent_id",
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        )
    );
}

#[test]
fn cursor_interactive_source_has_task_without_usage_or_child_model() {
    let content = include_str!("fixtures/source_contracts/cursor_interactive_negative.jsonl");
    assert!(content.contains("\"name\":\"Task\""));
    assert!(!content.contains("inputTokens"));
    assert!(!content.contains("childModel"));
}
