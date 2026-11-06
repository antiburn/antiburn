use super::*;
use rusqlite::params;
use serde_json::json;

fn fixture() -> (Connection, Value) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/cursor_characterization/native_chunked_headers.json"
    ))
    .unwrap();
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY, value TEXT)")
        .unwrap();
    db.execute(
        "INSERT INTO cursorDiskKV VALUES ('composerData:synthetic', ?1)",
        [fixture["composer"].to_string()],
    )
    .unwrap();
    for (index, value) in fixture["chunks"].as_array().unwrap().iter().enumerate() {
        db.execute(
            "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
            params![
                format!("composerConvHeaders:synthetic:{index:06}"),
                value.to_string()
            ],
        )
        .unwrap();
    }
    for bubble in fixture["bubbles"].as_array().unwrap() {
        db.execute(
            "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
            params![
                format!(
                    "bubbleId:synthetic:{}",
                    bubble["bubbleId"].as_str().unwrap()
                ),
                bubble.to_string()
            ],
        )
        .unwrap();
    }
    (db, fixture["composer"]["headerLog"].clone())
}

#[test]
fn chunked_headers_keep_recorded_order_and_short_interior_chunks() {
    let (db, metadata) = fixture();
    assert_eq!(
        read_header_ids(&db, "synthetic", &metadata).unwrap(),
        ["z-user", "a-tool", "m-assistant"]
    );
}

#[test]
fn chunked_headers_reject_missing_duplicate_and_malformed_chunks() {
    for replacement in [
        None,
        Some("{}"),
        Some("not json"),
        Some(r#"[{"bubbleId":"z-user"}]"#),
        Some(r#"[{"bubbleId":""}]"#),
        Some(r#"[{"bubbleId":4}]"#),
        Some(r#"[null]"#),
    ] {
        let (db, metadata) = fixture();
        db.execute(
            "DELETE FROM cursorDiskKV WHERE key='composerConvHeaders:synthetic:000001'",
            [],
        )
        .unwrap();
        if let Some(raw) = replacement {
            db.execute(
                "INSERT INTO cursorDiskKV VALUES ('composerConvHeaders:synthetic:000001', ?1)",
                [raw],
            )
            .unwrap();
        }
        assert!(
            read_header_ids(&db, "synthetic", &metadata).is_none(),
            "{replacement:?}"
        );
    }
}

#[test]
fn chunked_headers_reject_invalid_metadata_and_limits() {
    for (key, value) in [
        ("chunkSize", json!(0)),
        ("chunkSize", json!(128)),
        ("chunkCount", json!(-1)),
        ("chunkCount", json!(0)),
        ("chunkCount", json!(257)),
        ("chunkCount", json!(1.5)),
        ("count", json!(0)),
        ("count", json!(4)),
        ("count", json!(2)),
        ("count", json!(16385)),
        ("generation", Value::Null),
        ("generation", json!("")),
        ("generation", json!("g".repeat(257))),
    ] {
        let (db, mut metadata) = fixture();
        metadata[key] = value;
        assert!(
            read_header_ids(&db, "synthetic", &metadata).is_none(),
            "{key}: {metadata}"
        );
    }
    let (db, mut metadata) = fixture();
    metadata.as_object_mut().unwrap().remove("generation");
    assert!(read_header_ids(&db, "synthetic", &metadata).is_none());
}

#[test]
fn chunked_headers_use_exact_conversation_and_numeric_chunk_keys() {
    let (db, metadata) = fixture();
    db.execute("UPDATE cursorDiskKV SET key='composerConvHeaders:other:000001' WHERE key='composerConvHeaders:synthetic:000001'", []).unwrap();
    assert!(read_header_ids(&db, "synthetic", &metadata).is_none());
    db.execute("UPDATE cursorDiskKV SET key='composerConvHeaders:synthetic:1' WHERE key='composerConvHeaders:other:000001'", []).unwrap();
    assert!(read_header_ids(&db, "synthetic", &metadata).is_none());
}

#[test]
fn chunked_headers_bound_chunk_bytes_and_identifier_bytes() {
    for raw in [
        json!([{"bubbleId":"x".repeat(257)}]).to_string(),
        format!(
            "[{{\"bubbleId\":\"m-assistant\",\"ignored\":\"{}\"}}]",
            "x".repeat(256 * 1024)
        ),
    ] {
        let (db, metadata) = fixture();
        db.execute(
            "UPDATE cursorDiskKV SET value=?1 WHERE key='composerConvHeaders:synthetic:000001'",
            [raw],
        )
        .unwrap();
        assert!(read_header_ids(&db, "synthetic", &metadata).is_none());
    }
}

#[test]
fn chunked_headers_bound_aggregate_bytes_across_valid_chunks() {
    let (db, _) = fixture();
    db.execute(
        "DELETE FROM cursorDiskKV WHERE key LIKE 'composerConvHeaders:%'",
        [],
    )
    .unwrap();
    for index in 0..65 {
        let raw = json!([{"bubbleId": format!("header-{index}"), "ignored": "x".repeat(256 * 1024 - 100)}]).to_string();
        assert!(raw.len() < MAX_CHUNK_BYTES);
        db.execute(
            "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
            params![format!("composerConvHeaders:synthetic:{index:06}"), raw],
        )
        .unwrap();
    }
    let below_limit =
        json!({"chunkSize":256,"chunkCount":64,"count":64,"generation":"synthetic-generation"});
    assert_eq!(
        read_header_ids(&db, "synthetic", &below_limit)
            .unwrap()
            .len(),
        64
    );
    let above_limit =
        json!({"chunkSize":256,"chunkCount":65,"count":65,"generation":"synthetic-generation"});
    assert!(read_header_ids(&db, "synthetic", &above_limit).is_none());
}

fn tool_results(db: &Connection) -> Vec<crate::analysis::ToolResultObservation> {
    use crate::analysis::{
        CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
        SessionMetricsAccumulator, SourceFormat, SourceKind, reader_for,
    };
    let content =
        super::super::build_desktop_cursor_session_content(db, "synthetic", None, None).unwrap();
    let input = SessionInput {
        agent: "cursor".into(),
        session_id: "synthetic".into(),
        source: RawSource::Jsonl(content),
        fork_parent_session_id: None,
        source_format: SourceFormat::CursorIdeComposer,
    };
    let reader = reader_for("cursor");
    let mut sink = CompositeSink::new(
        SessionMetricsAccumulator::new("cursor", "synthetic"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "cursor".into(),
            session_id: "synthetic".into(),
            kind: SourceKind::Jsonl,
            capabilities: reader.capabilities(&input),
        }),
    );
    reader.visit(&input, &mut sink).unwrap();
    sink.tool_result_observations().to_vec()
}

#[test]
fn chunked_source_admits_native_result_with_recorded_user_prefix() {
    let (db, _) = fixture();
    let results = tool_results(&db);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].admission(),
        crate::analysis::ToolResultAdmission::Admitted
    );
    let context = results[0].context.as_ref().unwrap();
    assert!(context.main_session);
    assert!(context.complete_through_event);
    assert_eq!(context.user_messages.len(), 1);
}

#[test]
fn chunked_source_rejects_conflicting_legacy_order_and_bad_log_without_fallback() {
    for change in [
        "conflicting_inline",
        "bad_log_with_good_inline",
        "conversation",
        "conversationMap",
    ] {
        let (db, _) = fixture();
        let raw: String = db
            .query_row(
                "SELECT value FROM cursorDiskKV WHERE key='composerData:synthetic'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut composer: Value = serde_json::from_str(&raw).unwrap();
        match change {
            "conflicting_inline" => {
                composer["fullConversationHeadersOnly"] =
                    json!([{"bubbleId":"a-tool"},{"bubbleId":"z-user"},{"bubbleId":"m-assistant"}])
            }
            "bad_log_with_good_inline" => {
                composer["fullConversationHeadersOnly"] =
                    json!([{"bubbleId":"z-user"},{"bubbleId":"a-tool"},{"bubbleId":"m-assistant"}]);
                composer["headerLog"]["count"] = json!(4);
            }
            "conversation" => composer["conversation"] = json!([{"bubbleId":"z-user"}]),
            _ => composer["conversationMap"] = json!({"z-user":{}}),
        }
        db.execute(
            "UPDATE cursorDiskKV SET value=?1 WHERE key='composerData:synthetic'",
            [composer.to_string()],
        )
        .unwrap();
        let results = tool_results(&db);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].admission(),
            crate::analysis::ToolResultAdmission::UnsafeOrder,
            "{change}"
        );
    }
}

#[test]
fn chunked_source_preserves_legacy_inline_order_support() {
    let (db, _) = fixture();
    db.execute("UPDATE cursorDiskKV SET value=?1 WHERE key='composerData:synthetic'", [json!({"fullConversationHeadersOnly":[{"bubbleId":"z-user"},{"bubbleId":"a-tool"},{"bubbleId":"m-assistant"}]}).to_string()]).unwrap();
    assert_eq!(
        tool_results(&db)[0].admission(),
        crate::analysis::ToolResultAdmission::Admitted
    );
}

#[test]
fn chunked_headers_snapshot_does_not_mix_committed_generations() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("state.vscdb");
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY, value TEXT)",
        )
        .unwrap();
    let old = json!({"chunkSize":256,"chunkCount":1,"count":1,"generation":"old"});
    reader
        .execute(
            "INSERT INTO cursorDiskKV VALUES ('composerData:synthetic', ?1)",
            [old.to_string()],
        )
        .unwrap();
    reader
        .execute(
            "INSERT INTO cursorDiskKV VALUES ('composerConvHeaders:synthetic:000000', ?1)",
            [json!([{"bubbleId":"old-user"}]).to_string()],
        )
        .unwrap();
    let snapshot = reader.unchecked_transaction().unwrap();
    let raw: String = snapshot
        .query_row(
            "SELECT value FROM cursorDiskKV WHERE key='composerData:synthetic'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let metadata: Value = serde_json::from_str(&raw).unwrap();
    let writer = Connection::open(&path).unwrap();
    let update = writer.unchecked_transaction().unwrap();
    let mut new = old.clone();
    new["generation"] = json!("new");
    update
        .execute(
            "UPDATE cursorDiskKV SET value=?1 WHERE key='composerData:synthetic'",
            [new.to_string()],
        )
        .unwrap();
    update
        .execute(
            "UPDATE cursorDiskKV SET value=?1 WHERE key='composerConvHeaders:synthetic:000000'",
            [json!([{"bubbleId":"new-user"}]).to_string()],
        )
        .unwrap();
    update.commit().unwrap();
    assert_eq!(
        read_header_ids(&snapshot, "synthetic", &metadata).unwrap(),
        ["old-user"]
    );
    snapshot.commit().unwrap();
    assert_eq!(
        read_header_ids(&reader, "synthetic", &new).unwrap(),
        ["new-user"]
    );
}
