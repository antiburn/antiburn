use super::*;

fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Connection,
    Value,
    OpenCodeJournalCursor,
) {
    fixture_from_json(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_receipt_compatibility/journal-v1.json"
    )))
}

fn eviction_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Connection,
    Value,
    OpenCodeJournalCursor,
) {
    fixture_from_json(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_receipt_compatibility/journal-v1-evicted-context.json"
    )))
}

fn fixture_from_json(
    text: &str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Connection,
    Value,
    OpenCodeJournalCursor,
) {
    let fixture: Value = serde_json::from_str(text).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY,parent_id TEXT); INSERT INTO session VALUES('root',NULL);
        CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,data TEXT);
        CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,data TEXT);
        CREATE TABLE event_sequence(aggregate_id TEXT PRIMARY KEY,seq INTEGER,owner_id TEXT);
        CREATE TABLE event(id TEXT PRIMARY KEY,aggregate_id TEXT,seq INTEGER,type TEXT,data TEXT);
        CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id,seq);").unwrap();
    for event in fixture["events"].as_array().unwrap() {
        connection
            .execute(
                "INSERT INTO event VALUES(?1,'root',?2,?3,?4)",
                params![
                    event["id"].as_str().unwrap(),
                    event["seq"].as_u64().unwrap(),
                    event["type"].as_str().unwrap(),
                    event["data"].to_string()
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO event_sequence VALUES('root',?1,NULL)",
            [fixture["cursor"]["through_sequence"].as_u64().unwrap()],
        )
        .unwrap();
    let cursor = decode(&serde_json::to_vec(&fixture["cursor"]).unwrap()).unwrap();
    (directory, path, connection, fixture, cursor)
}

fn append_events(connection: &Connection, events: &[Value]) {
    for event in events {
        connection
            .execute(
                "INSERT INTO event VALUES(?1,'root',?2,?3,?4)",
                params![
                    event["id"].as_str().unwrap(),
                    event["seq"].as_u64().unwrap(),
                    event["type"].as_str().unwrap(),
                    event["data"].to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE event_sequence SET seq=?1",
                [event["seq"].as_u64().unwrap()],
            )
            .unwrap();
    }
}

fn complete(connection: &Connection, fixture: &Value) {
    connection
        .execute(
            "INSERT INTO event VALUES('append-6','root',6,'message.part.updated.1',?1)",
            [fixture["completion"].to_string()],
        )
        .unwrap();
    connection
        .execute("UPDATE event_sequence SET seq=6", [])
        .unwrap();
}

#[test]
fn main_64_actual_journal_body_keeps_frontier_pending_call_and_terminal_context() {
    let (_directory, path, connection, fixture, cursor) = fixture();
    let idle =
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).unwrap();
    assert_eq!(idle.live_input_events, 0);
    assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
    assert_eq!(idle.cursor.through_sequence, 5);
    assert_eq!(idle.cursor.live_baseline(), cursor.live_baseline());
    assert_eq!(idle.cursor.public_baseline(), Some(5));
    let restored = serde_json::from_slice(&serde_json::to_vec(&idle.cursor).unwrap()).unwrap();
    complete(&connection, &fixture);
    let next = read_opencode_live_tool_result_page(&path, "root", Some(&restored), None, &|| false)
        .unwrap();
    assert_eq!(next.results.len(), 1);
    assert_eq!(next.results[0].call_id.as_deref(), Some("call-1"));
    assert_eq!(next.cursor.public_baseline(), Some(5));
    assert_eq!(
        next.cursor.live.as_ref().unwrap().parts["part-0"]
            .state
            .first_terminal,
        Some(4)
    );
    assert_eq!(
        next.cursor.live.as_ref().unwrap().contexts,
        cursor.live.as_ref().unwrap().contexts
    );
    assert!(
        read_opencode_live_tool_result_page(&path, "root", Some(&next.cursor), None, &|| false)
            .unwrap()
            .results
            .is_empty()
    );
}

#[test]
fn main_64_journal_public_frontier_is_current_and_survives_partial_retry() {
    let (_directory, path, connection, fixture, cursor) = fixture();
    complete(&connection, &fixture);
    let page =
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).unwrap();
    assert_eq!(page.cursor.public_baseline(), Some(6));
    assert_eq!(page.results.len(), 1);
    assert!(page.evidence.events.iter().all(|event| matches!(
        event.fact,
        crate::analysis::NativeEvidenceFact::Message(
            crate::analysis::NativeMessageObservation::ToolResult(_)
        )
    ) || !event.live_eligible));
    let retry: OpenCodeJournalCursor =
        serde_json::from_str(&cursor.main_64_retry_json(&page.cursor).unwrap().unwrap()).unwrap();
    assert_eq!(retry.through_sequence, 5);
    assert_eq!(retry.public_baseline(), Some(6));
    assert_eq!(retry.live_baseline(), cursor.live_baseline());
    assert_eq!(
        retry.live.as_ref().unwrap().parts,
        cursor.live.as_ref().unwrap().parts
    );
}

#[test]
fn main_64_journal_rejects_other_shapes_and_changed_source_bindings() {
    let (_directory, path, connection, fixture, cursor) = fixture();
    for mutation in ["parser", "revision", "public", "part", "requests"] {
        let mut value = fixture["cursor"].clone();
        match mutation {
            "parser" => value["parser_revision"] = serde_json::json!(63),
            "revision" => value["live"]["revision"] = serde_json::json!(4),
            "public" => value["live"]["public_baseline"] = serde_json::json!(5),
            "part" => value["live"]["parts"]["part-0"]["state"]["facts"] = serde_json::json!(null),
            "requests" => value["live"]["requests"]["removed_parts"] = serde_json::json!({}),
            _ => unreachable!(),
        }
        assert!(
            decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{mutation}"
        );
    }
    assert!(
        read_opencode_live_tool_result_page(&path, "other", Some(&cursor), None, &|| false)
            .is_err()
    );
    connection
        .execute("UPDATE event_sequence SET owner_id='other'", [])
        .unwrap();
    assert!(
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).is_err()
    );
    connection
        .execute("UPDATE event_sequence SET owner_id=NULL", [])
        .unwrap();
    connection
        .execute("UPDATE session SET parent_id='parent'", [])
        .unwrap();
    assert!(
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).is_err()
    );
    connection
        .execute("UPDATE session SET parent_id=NULL", [])
        .unwrap();
    connection
        .execute("UPDATE event SET data='{}' WHERE seq=5", [])
        .unwrap();
    assert!(
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).is_err()
    );
}

#[test]
fn main_64_evicted_context_idle_conversion_keeps_results_and_allocation_state() {
    let (_directory, path, _connection, fixture, cursor) = eviction_fixture();
    let old = cursor.live.as_ref().unwrap();
    assert_eq!(old.parts.len(), 18);
    assert_eq!(old.contexts.len(), 16);
    assert_eq!(old.next_context, 17);
    assert_eq!(old.contexts.front().unwrap().0, 1);
    assert_eq!(old.parts["part-0"].context, Some(0));
    assert_eq!(fixture["completed"].as_array().unwrap().len(), 17);
    let idle =
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).unwrap();
    let converted = idle.cursor.live.as_ref().unwrap();
    assert_eq!(idle.live_input_events, 0);
    assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
    assert_eq!(idle.cursor.through_sequence, cursor.through_sequence);
    assert_eq!(idle.cursor.live_baseline(), cursor.live_baseline());
    assert_eq!(idle.cursor.public_baseline(), Some(38));
    assert_eq!(converted.parts, old.parts);
    assert_eq!(converted.contexts, old.contexts);
    assert_eq!(converted.next_context, old.next_context);
    assert_eq!(converted.requests, old.requests);
    assert_eq!(converted.history_safe, old.history_safe);
    assert_eq!(converted.evidence_gap, old.evidence_gap);
    assert_eq!(fixture["cursor"]["live"]["requests"]["incomplete"], true);
    let restored = serde_json::from_slice(&serde_json::to_vec(&idle.cursor).unwrap()).unwrap();
    let restarted =
        read_opencode_live_tool_result_page(&path, "root", Some(&restored), None, &|| false)
            .unwrap();
    assert!(restarted.results.is_empty() && restarted.evidence.events.is_empty());
    assert_eq!(restarted.cursor, idle.cursor);
}

#[test]
fn main_64_evicted_context_corrections_and_pending_completion_keep_old_encoder_contexts() {
    let (_directory, path, connection, fixture, cursor) = eviction_fixture();
    let idle =
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).unwrap();
    let restored = serde_json::from_slice(&serde_json::to_vec(&idle.cursor).unwrap()).unwrap();
    append_events(
        &connection,
        fixture["continuation_events"].as_array().unwrap(),
    );
    let next = read_opencode_live_tool_result_page(&path, "root", Some(&restored), None, &|| false)
        .unwrap();
    let expected: Vec<ToolResultObservation> =
        serde_json::from_value(fixture["continued"].clone()).unwrap();
    assert_eq!(next.results.len(), 3);
    for (result, expected) in next.results.iter().zip(&expected) {
        assert_eq!(result.result_id, expected.result_id);
        assert_eq!(result.position, expected.position);
        assert_eq!(result.revision, expected.revision);
        assert_eq!(result.content, expected.content);
        assert_eq!(result.context, expected.context);
        assert_eq!(result.completeness, expected.completeness);
        assert_eq!(result.finality, expected.finality);
    }
    let evicted = next.results[0].context.as_ref().unwrap();
    assert_eq!(
        evicted.result_position,
        ToolResultPosition::OpenCodeEvent {
            sequence: 5,
            item_index: 0
        }
    );
    assert!(evicted.user_messages.is_empty());
    assert!(!evicted.main_session && !evicted.complete_through_event);
    let retained = next.results[1].context.as_ref().unwrap();
    assert_eq!(
        retained.result_position,
        ToolResultPosition::OpenCodeEvent {
            sequence: 37,
            item_index: 0
        }
    );
    assert_eq!(
        retained.user_messages[0].parts[0].text,
        "synthetic request 16"
    );
    let pending = next.results[2].context.as_ref().unwrap();
    assert_eq!(
        pending.user_messages[0].parts[0].text,
        "synthetic request 17"
    );
    assert!(!pending.complete_through_event);
    let live = next.cursor.live.as_ref().unwrap();
    assert_eq!(live.parts["part-0"].context, Some(0));
    assert_eq!(live.parts["part-0"].state.first_terminal, Some(5));
    assert_eq!(live.parts["part-16"].state.first_terminal, Some(37));
    assert_eq!(live.parts["part-17"].state.first_terminal, Some(42));
    assert_eq!(live.parts["part-17"].context, Some(17));
    assert_eq!(live.next_context, 18);
    assert_eq!(next.cursor.public_baseline(), Some(38));
    let restarted = serde_json::from_slice(&serde_json::to_vec(&next.cursor).unwrap()).unwrap();
    let idle =
        read_opencode_live_tool_result_page(&path, "root", Some(&restarted), None, &|| false)
            .unwrap();
    assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
}

#[test]
fn main_64_evicted_context_retry_keeps_old_sequence_and_does_not_rebind_requests() {
    let (_directory, path, connection, fixture, cursor) = eviction_fixture();
    append_events(
        &connection,
        fixture["continuation_events"].as_array().unwrap(),
    );
    let page =
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).unwrap();
    let retry: OpenCodeJournalCursor =
        serde_json::from_str(&cursor.main_64_retry_json(&page.cursor).unwrap().unwrap()).unwrap();
    assert_eq!(retry.through_sequence, 38);
    assert_eq!(retry.public_baseline(), Some(42));
    let old = cursor.live.as_ref().unwrap();
    let saved = retry.live.as_ref().unwrap();
    assert_eq!(saved.parts, old.parts);
    assert_eq!(saved.contexts, old.contexts);
    assert_eq!(saved.next_context, old.next_context);
    let retried =
        read_opencode_live_tool_result_page(&path, "root", Some(&retry), None, &|| false).unwrap();
    assert_eq!(retried.results, page.results);
    assert_eq!(retried.cursor, page.cursor);
    assert!(retried.evidence.events.iter().all(|event| matches!(
        event.fact,
        crate::analysis::NativeEvidenceFact::Message(
            crate::analysis::NativeMessageObservation::ToolResult(_)
        )
    ) || !event.live_eligible));
}

#[test]
fn main_64_journal_preserves_unsafe_flags_through_eviction_conversion() {
    let (_directory, path, _connection, fixture, _cursor) = eviction_fixture();
    let mut value = fixture["cursor"].clone();
    value["live"]["history_safe"] = serde_json::json!(false);
    value["live"]["evidence_gap"] = serde_json::json!(true);
    value["live"]["parts"]["part-0"]["unsafe_ownership"] = serde_json::json!(true);
    value["live"]["parts"]["part-0"]["state"]["invalid"] = serde_json::json!(true);
    let cursor = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
    let idle =
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).unwrap();
    let live = idle.cursor.live.as_ref().unwrap();
    assert!(!live.history_safe && live.evidence_gap);
    assert_eq!(live.parts, cursor.live.as_ref().unwrap().parts);
    assert!(!idle.history_complete);
}

#[test]
fn main_64_journal_rejects_reused_future_unordered_and_overflowing_counters() {
    let (_directory, _path, _connection, saved_fixture, _cursor) = eviction_fixture();
    for mutation in [
        "next-reuse",
        "next-future",
        "next-overflow",
        "duplicate",
        "order",
        "gap",
        "future-key",
        "old-cache-tail",
        "empty-cache",
        "future-reference",
        "future-before-terminal",
        "missing-terminal",
        "revision-bound",
        "revision-overflow",
        "zero-terminal-revision",
        "sequence-overflow",
        "part-sequence-overflow",
    ] {
        let mut value = saved_fixture["cursor"].clone();
        match mutation {
            "next-reuse" => value["live"]["next_context"] = serde_json::json!(16),
            "next-future" => value["live"]["next_context"] = serde_json::json!(40),
            "next-overflow" => value["live"]["next_context"] = serde_json::json!(u64::MAX),
            "duplicate" => {
                value["live"]["contexts"][1][0] = value["live"]["contexts"][0][0].clone()
            }
            "order" => value["live"]["contexts"].as_array_mut().unwrap().swap(0, 1),
            "gap" => value["live"]["contexts"][0][0] = serde_json::json!(0),
            "future-key" => value["live"]["contexts"][15][0] = serde_json::json!(17),
            "old-cache-tail" => {
                value["live"]["contexts"]
                    .as_array_mut()
                    .unwrap()
                    .pop()
                    .unwrap();
            }
            "empty-cache" => value["live"]["contexts"] = serde_json::json!([]),
            "future-reference" => {
                value["live"]["parts"]["part-0"]["context"] = serde_json::json!(17)
            }
            "future-before-terminal" => {
                value["live"]["parts"]["part-0"]["context"] = serde_json::json!(16)
            }
            "missing-terminal" => {
                value["live"]["parts"]["part-0"]["state"]["first_terminal"] = Value::Null
            }
            "revision-bound" => {
                value["live"]["parts"]["part-0"]["state"]["revision"] = serde_json::json!(7)
            }
            "revision-overflow" => {
                value["live"]["parts"]["part-0"]["state"]["revision"] = serde_json::json!(u64::MAX)
            }
            "zero-terminal-revision" => {
                value["live"]["parts"]["part-0"]["state"]["revision"] = serde_json::json!(0)
            }
            "sequence-overflow" => value["through_sequence"] = serde_json::json!(u64::MAX),
            "part-sequence-overflow" => {
                value["live"]["parts"]["part-0"]["last_sequence"] = serde_json::json!(u64::MAX)
            }
            _ => unreachable!(),
        }
        assert!(
            decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{mutation}"
        );
    }
    let (_directory, _path, _connection, fixture, _cursor) = fixture();
    let mut reused = fixture["cursor"].clone();
    reused["live"]["next_context"] = serde_json::json!(0);
    assert!(decode(&serde_json::to_vec(&reused).unwrap()).is_err());
}

#[test]
fn main_64_named_journal_decoder_rejects_other_parser_layout_pairs() {
    let (_directory, path, _connection, fixture, cursor) = fixture();
    for parser in [61, 62, 63, 64, 65, 66, PARSER_REVISION, PARSER_REVISION + 1] {
        for layout in [1, 2, 3, 4, 5, 6] {
            let mut value = fixture["cursor"].clone();
            value["parser_revision"] = serde_json::json!(parser);
            value["live"]["revision"] = serde_json::json!(layout);
            assert_eq!(
                decode(&serde_json::to_vec(&value).unwrap()).is_ok(),
                parser == 64 && layout == 2,
                "parser={parser}, layout={layout}"
            );
        }
    }
    let generic: OpenCodeJournalCursor = serde_json::from_value(fixture["cursor"].clone()).unwrap();
    assert!(!generic.main_64);
    assert!(
        read_opencode_live_tool_result_page(&path, "root", Some(&generic), None, &|| false)
            .is_err()
    );
    assert!(
        read_opencode_live_tool_result_page(&path, "root", Some(&cursor), None, &|| false).is_ok()
    );
}
