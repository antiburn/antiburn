use std::{cell::Cell, path::Path};

use antiburn_local::analysis::{
    SourceFormat, ToolResultChange, ToolResultCompleteness, ToolResultContext,
    ToolResultObservation, ToolResultPosition, ToolResultStatus, UserMessage, UserMessageChange,
    UserMessageCompleteness, UserMessageOrigin, UserMessagePart, UserMessagePosition,
    UserMessageScope,
    opencode_v2_capture::{
        CaptureReconciliation, MAX_CAPTURE_EVENT_BYTES, read_capture_reconciliation_for_scope,
    },
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(directory.path().join("source.sqlite")).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
         CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT, fork_session_id TEXT,
           directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER);
         CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT,
           seq INTEGER, time_created INTEGER, time_updated INTEGER, data TEXT);
         CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id, seq);
         CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY NOT NULL, seq INTEGER NOT NULL);
         INSERT INTO session_v2 VALUES ('session', NULL, NULL, '/synthetic', NULL, 1, 1);
         INSERT INTO session_v2 VALUES ('other', NULL, NULL, '/synthetic', NULL, 1, 1);
         INSERT INTO event_sequence VALUES ('session', 20);",
        )
        .unwrap();
    put(&connection, "message", "assistant", 1, assistant("ok"));
    (directory, connection)
}

fn assistant(output: &str) -> Value {
    json!({"content":[{"type":"tool","id":"call","name":"shell","executed":false,
        "state":{"status":"completed","input":{"command":"true"},
            "content":[{"type":"text","text":output}]}}]})
}

fn put(connection: &Connection, id: &str, kind: &str, seq: i64, data: Value) {
    connection
        .execute(
            "INSERT INTO session_message VALUES (?1, 'session', ?2, ?3, 1, 1, ?4)
         ON CONFLICT(id) DO UPDATE SET type = excluded.type, data = excluded.data",
            params![id, kind, seq, data.to_string()],
        )
        .unwrap();
}

fn target() -> ToolResultObservation {
    ToolResultObservation {
        source_format: SourceFormat::OpenCodeSqliteV2,
        session_id: "session".into(),
        record_id: Some("message".into()),
        call_id: Some("call".into()),
        result_id: Some("terminal".into()),
        position: ToolResultPosition::OpenCodeV2Event {
            sequence: 10,
            item_index: 0,
        },
        revision: 0,
        change: ToolResultChange::Update,
        tool_category: None,
        tool_name: Some("shell".into()),
        arguments: Some(json!({"command":"true"})),
        content: Some("ok".into()),
        status: ToolResultStatus::Success,
        finality: true,
        completeness: ToolResultCompleteness::Incomplete,
        context: Some(ToolResultContext {
            result_position: ToolResultPosition::OpenCodeV2Event {
                sequence: 10,
                item_index: 0,
            },
            main_session: true,
            complete_through_event: false,
            user_messages: Vec::new(),
        }),
        outcome: None,
        provider_executed: Some(false),
    }
}

fn read(path: &Path, targets: &[ToolResultObservation]) -> CaptureReconciliation {
    read_capture_reconciliation_for_scope(
        &path.join("source.sqlite"),
        SourceFormat::OpenCodeSqliteV2,
        &[("session".into(), "/synthetic".into())],
        targets,
        &|| false,
    )
    .unwrap()
}

#[test]
fn exact_binding_checks_do_not_use_bounded_page_membership_or_invent_history() {
    let (directory, connection) = fixture();
    for index in 2..520 {
        put(
            &connection,
            &format!("unrelated-{index}"),
            "assistant",
            index,
            assistant("other"),
        );
    }
    let original = target();
    let snapshot = read(directory.path(), std::slice::from_ref(&original));
    assert_eq!(snapshot.baseline["session"], Some(20));
    assert_eq!(snapshot.bindings.len(), 1);
    assert!(snapshot.bindings[0].retained);
    assert_eq!(snapshot.bindings[0].observation, original);
    assert!(read(directory.path(), &[]).bindings.is_empty());
}

#[test]
fn changed_tool_evidence_retires_only_the_exact_saved_binding() {
    let variants = [
        ("/content/0/name", json!("read")),
        ("/content/0/state/input", json!({"command":"false"})),
        ("/content/0/state/status", json!("error")),
        ("/content/0/state/content/0/text", json!("corrected")),
        ("/content/0/state/metadata", json!({"exit":7})),
        ("/content/0/executed", json!(true)),
    ];
    for (pointer, correction) in variants {
        let (directory, connection) = fixture();
        put(&connection, "unchanged", "assistant", 2, assistant("ok"));
        let mut changed = assistant("ok");
        if pointer == "/content/0/state/metadata" {
            changed["content"][0]["state"]["metadata"] = correction;
        } else {
            *changed.pointer_mut(pointer).unwrap() = correction;
        }
        put(&connection, "message", "assistant", 1, changed);
        let mut other = target();
        other.record_id = Some("unchanged".into());
        let snapshot = read(directory.path(), &[target(), other]);
        assert!(!snapshot.bindings[0].retained, "{pointer}");
        assert!(snapshot.bindings[1].retained, "{pointer}");
    }
}

#[test]
fn deleted_or_moved_messages_and_duplicate_call_ids_do_not_restore_a_binding() {
    for action in ["delete", "move", "duplicate", "malformed", "oversized"] {
        let (directory, connection) = fixture();
        match action {
            "delete" => {
                connection
                    .execute("DELETE FROM session_message", [])
                    .unwrap();
            }
            "move" => {
                connection
                    .execute("UPDATE session_message SET session_id = 'other'", [])
                    .unwrap();
            }
            "duplicate" => {
                let mut message = assistant("ok");
                let tool = message["content"][0].clone();
                message["content"].as_array_mut().unwrap().push(tool);
                put(&connection, "message", "assistant", 1, message);
            }
            "malformed" => {
                connection
                    .execute("UPDATE session_message SET data = '{'", [])
                    .unwrap();
            }
            "oversized" => {
                put(
                    &connection,
                    "message",
                    "assistant",
                    1,
                    assistant(&"x".repeat(MAX_CAPTURE_EVENT_BYTES + 1)),
                );
            }
            _ => unreachable!(),
        }
        let snapshot = read(directory.path(), &[target()]);
        assert!(!snapshot.bindings[0].retained, "{action}");
        assert_eq!(
            snapshot.bindings[0].observation.content.as_deref(),
            Some("ok")
        );
    }
}

#[test]
fn frozen_request_corrections_and_deletions_retire_without_rewriting_text() {
    for (kind, current_text) in [
        ("user", "original"),
        ("user", "corrected"),
        ("synthetic", "original"),
        ("missing", ""),
    ] {
        let (directory, connection) = fixture();
        if kind != "missing" {
            put(&connection, "user", kind, 2, json!({"text":current_text}));
        }
        let mut original = target();
        original
            .context
            .as_mut()
            .unwrap()
            .user_messages
            .push(UserMessage {
                source_format: SourceFormat::OpenCodeSqliteV2,
                session_id: "session".into(),
                message_id: "user".into(),
                position: UserMessagePosition::OpenCodeV2Event { sequence: 2 },
                origin: UserMessageOrigin::OpenCodeUserText,
                scope: UserMessageScope::MainSession,
                change: UserMessageChange::Present,
                completeness: UserMessageCompleteness::Complete,
                parts: vec![UserMessagePart {
                    item_index: 0,
                    native_part_id: None,
                    text: "original".into(),
                }],
                proof_revision: 1,
            });
        let snapshot = read(directory.path(), std::slice::from_ref(&original));
        assert_eq!(
            snapshot.bindings[0].retained,
            kind == "user" && current_text == "original"
        );
        assert_eq!(snapshot.bindings[0].observation, original);
    }
}

#[test]
fn watermark_and_current_bindings_share_one_read_only_wal_snapshot() {
    let (directory, writer) = fixture();
    let calls = Cell::new(0);
    let snapshot = read_capture_reconciliation_for_scope(
        &directory.path().join("source.sqlite"),
        SourceFormat::OpenCodeSqliteV2,
        &[("session".into(), "/synthetic".into())],
        &[target()],
        &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                writer
                    .execute_batch("BEGIN; UPDATE event_sequence SET seq = 30;")
                    .unwrap();
                put(&writer, "message", "assistant", 1, assistant("later"));
                writer.execute_batch("COMMIT").unwrap();
            }
            false
        },
    )
    .unwrap();
    assert_eq!(snapshot.baseline["session"], Some(20));
    assert!(snapshot.bindings[0].retained);
    let db = std::fs::read(directory.path().join("source.sqlite")).unwrap();
    let wal = std::fs::read(directory.path().join("source.sqlite-wal")).unwrap();
    assert!(!read(directory.path(), &[target()]).bindings[0].retained);
    assert_eq!(
        std::fs::read(directory.path().join("source.sqlite")).unwrap(),
        db
    );
    assert_eq!(
        std::fs::read(directory.path().join("source.sqlite-wal")).unwrap(),
        wal
    );
}

#[test]
fn changed_scope_is_reported_per_session_and_cancellation_fails_closed() {
    let (directory, connection) = fixture();
    let path = directory.path().join("source.sqlite");
    let scopes = [("session".into(), "/synthetic".into())];
    assert!(
        read_capture_reconciliation_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &scopes,
            &[target()],
            &|| true
        )
        .is_err()
    );
    for sql in [
        "UPDATE session_v2 SET directory = '/moved'",
        "UPDATE session_v2 SET parent_id = 'parent'",
        "DELETE FROM session_v2",
    ] {
        connection.execute(sql, []).unwrap();
        let snapshot = read_capture_reconciliation_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &scopes,
            &[target()],
            &|| false,
        )
        .unwrap();
        assert_eq!(
            snapshot.invalid_scopes,
            std::collections::HashSet::from(["session".into()])
        );
        assert_eq!(snapshot.baseline["session"], None);
        assert!(!snapshot.bindings[0].retained);
    }
}

#[test]
fn regressed_native_watermark_cannot_retain_an_old_terminal_binding() {
    let (directory, connection) = fixture();
    connection
        .execute("UPDATE event_sequence SET seq = 9", [])
        .unwrap();
    assert!(!read(directory.path(), &[target()]).bindings[0].retained);
    connection
        .execute("DELETE FROM event_sequence", [])
        .unwrap();
    assert!(!read(directory.path(), &[target()]).bindings[0].retained);
}

#[test]
fn missing_lineage_column_is_an_unsupported_schema() {
    let (directory, connection) = fixture();
    connection
        .execute("ALTER TABLE session_v2 DROP COLUMN fork_session_id", [])
        .unwrap();
    let error = read_capture_reconciliation_for_scope(
        &directory.path().join("source.sqlite"),
        SourceFormat::OpenCodeSqliteV2,
        &[("session".into(), "/synthetic".into())],
        &[target()],
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<antiburn_local::analysis::OpenCodeV2SnapshotError>(),
        Some(&antiburn_local::analysis::OpenCodeV2SnapshotError::UnsupportedSchema)
    );
}

#[test]
fn reconciliation_bounds_saved_payloads_and_caches_repeated_source_reads() {
    use antiburn_local::analysis::{
        OpenCodeV2SnapshotError, opencode_v2_capture::MAX_RECONCILIATION_BYTES,
    };

    let (directory, connection) = fixture();
    let path = directory.path().join("source.sqlite");
    let scopes = [("session".into(), "/synthetic".into())];
    let mut oversized = target();
    oversized.content = Some("x".repeat(MAX_RECONCILIATION_BYTES + 1));
    let error = read_capture_reconciliation_for_scope(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        &scopes,
        &[oversized],
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<OpenCodeV2SnapshotError>(),
        Some(&OpenCodeV2SnapshotError::SourceLimit)
    );
    let mut large = assistant("ok");
    large["content"][0]["state"]["metadata"] = json!({"unused":"x".repeat(16 * 1024)});
    put(&connection, "message", "assistant", 1, large);
    let snapshot = read_capture_reconciliation_for_scope(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        &scopes,
        &vec![target(); 1200],
        &|| false,
    )
    .unwrap();
    assert_eq!(snapshot.bindings.len(), 1200);
    assert!(snapshot.bindings.iter().all(|binding| binding.retained));
}

#[test]
fn unique_native_read_limit_retires_unavailable_bindings_without_blocking_recovery() {
    let (directory, connection) = fixture();
    let mut large = assistant("ok");
    large["content"][0]["state"]["metadata"] = json!({"unused":"x".repeat(200 * 1024)});
    let mut targets = Vec::new();
    connection.execute_batch("BEGIN").unwrap();
    for index in 2..102 {
        let id = format!("message-{index}");
        put(&connection, &id, "assistant", index, large.clone());
        let mut observation = target();
        observation.record_id = Some(id);
        targets.push(observation);
    }
    connection.execute_batch("COMMIT").unwrap();
    let snapshot = read(directory.path(), &targets);
    assert_eq!(snapshot.bindings.len(), targets.len());
    let retained = snapshot
        .bindings
        .iter()
        .filter(|binding| binding.retained)
        .count();
    assert!(retained > 0 && retained < targets.len());
    for (binding, original) in snapshot.bindings.iter().zip(targets) {
        assert_eq!(binding.observation, original);
    }
}
