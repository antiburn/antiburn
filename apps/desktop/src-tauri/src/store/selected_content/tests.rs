use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use antiburn_local::analysis::{JevInputField, JevInputSelection, SelectedContentCursor};

use super::*;

fn fixture() -> (Store, SessionKey) {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-keyset-tests")).unwrap();
    let key = SessionKey::new("native", "claude-code", "keyset");
    {
        let conn = store.lock();
        conn.execute_batch(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
                first_seen_at, last_seen_at, source_generation, source_fingerprint)
             VALUES ('native', 'claude-code', 'keyset', 'file', 'synthetic', 'now', 'now', 7, 'source');",
        ).unwrap();
        conn.execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id, status,
                analyzed_generation, processed_fingerprint, parser_revision, analyzer_revision,
                evidence_schema_revision, published_fence, claim_fence)
             VALUES ('native', 'claude-code', 'keyset', 'ready', 7, 'source', ?1, ?2, ?3, 11, 11)",
            params![
                PARSER_REVISION,
                antiburn_local::analysis::ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION
            ],
        )
        .unwrap();
        for source in ["a", "b"] {
            for ordinal in 0..90 {
                for tie in 0..2 {
                    conn.execute(
                        "INSERT INTO turn (environment_key, agent, session_id, claim_fence,
                            source_key, thread_id, turn_index, scope, role, ts_ms, uuid,
                            input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                            is_compaction_boundary)
                         VALUES ('native', 'claude-code', 'keyset', 11, ?1, ?1, ?2, 'main',
                            'assistant', 2000, ?3, 0, 0, 0, 0, 0)",
                        params![source, ordinal, format!("{source}-{ordinal}-{tie}")],
                    )
                    .unwrap();
                    let rowid = conn.last_insert_rowid();
                    for part in 0..3 {
                        conn.execute(
                            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, content,
                                truncated) VALUES (?1, ?2, 'assistant', 'assistant', ?3, 0)",
                            params![rowid, part, format!("{source}-{ordinal}-{tie}-{part}").as_bytes()],
                        ).unwrap();
                    }
                }
            }
        }
    }
    (store, key)
}

fn read(
    store: &Store,
    key: &SessionKey,
    cursor: Option<&SelectedContentCursor>,
    after_ms: Option<i64>,
) -> SelectedContentPage {
    store
        .published_turn_content_keyset_selected(
            key,
            SelectedContentRequest {
                source_generation: 7,
                after_ms,
                source_positions: &BTreeMap::new(),
                selection: JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                cursor,
            },
        )
        .unwrap()
        .unwrap()
}

#[test]
fn selected_pages_resume_json_with_source_turn_and_part_ties() {
    for after_ms in [None, Some(1000)] {
        let (store, key) = fixture();
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        loop {
            let page = read(&store, &key, cursor.as_ref(), after_ms);
            for part in page.content.parts.iter().filter(|part| !part.context_only) {
                assert!(
                    seen.insert(part.part.text.clone()),
                    "duplicate selected part"
                );
            }
            let Some(next) = page.next_cursor else { break };
            let saved = SelectedContentProgress {
                revision: SELECTED_CONTENT_PROGRESS_REVISION,
                cursor: Some(next),
            };
            store.set_internal_value(
                "internal:keysetTest",
                &serde_json::to_string(&saved).unwrap(),
            );
            let restored: SelectedContentProgress =
                serde_json::from_str(&store.internal_value("internal:keysetTest").unwrap())
                    .unwrap();
            assert_eq!(restored.revision, SELECTED_CONTENT_PROGRESS_REVISION);
            cursor = restored.cursor;
        }
        assert_eq!(seen.len(), 1080);
    }
}

#[test]
fn selected_resume_survives_earlier_deletion_and_excludes_unpublished_rows() {
    let (store, key) = fixture();
    let first = read(&store, &key, None, None);
    let expected = read(&store, &key, first.next_cursor.as_ref(), None);
    {
        let conn = store.lock();
        conn.execute(
            "DELETE FROM turn WHERE rowid = (SELECT MIN(rowid) FROM turn)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn (environment_key, agent, session_id, claim_fence, source_key,
                thread_id, turn_index, scope, role, input_tokens, cache_read_tokens,
                cache_write_tokens, output_tokens, is_compaction_boundary)
             VALUES ('native', 'claude-code', 'keyset', 12, 'a', 'a', 45, 'main', 'assistant', 0, 0, 0, 0, 0);
             INSERT INTO turn_content (turn_rowid, part_index, kind, authority, content, truncated)
             VALUES (last_insert_rowid(), 0, 'assistant', 'assistant', CAST('unpublished' AS BLOB), 0);",
        ).unwrap();
    }
    assert_eq!(
        read(&store, &key, first.next_cursor.as_ref(), None),
        expected
    );
}

#[test]
fn selected_resume_rejects_changed_selection_positions_and_fence() {
    let (store, key) = fixture();
    let first = read(&store, &key, None, Some(1000));
    let positions = BTreeMap::from([("a".to_owned(), 2)]);
    for (selection, source_positions, after_ms) in [
        (JevInputSelection::ALL, &BTreeMap::new(), Some(1000)),
        (
            JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
            &positions,
            Some(1000),
        ),
        (
            JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
            &BTreeMap::new(),
            Some(1001),
        ),
    ] {
        assert!(matches!(
            store.published_turn_content_keyset_selected(
                &key,
                SelectedContentRequest {
                    source_generation: 7,
                    after_ms,
                    source_positions,
                    selection,
                    cursor: first.next_cursor.as_ref(),
                }
            ),
            Err(SelectedContentQueryError::StaleCursor)
        ));
    }
    store
        .lock()
        .execute("UPDATE session_evidence SET published_fence = 12", [])
        .unwrap();
    assert!(matches!(
        store.published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: 7,
                after_ms: Some(1000),
                source_positions: &BTreeMap::new(),
                selection: JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                cursor: first.next_cursor.as_ref(),
            }
        ),
        Err(SelectedContentQueryError::StaleCursor)
    ));
}

#[test]
fn selected_adapter_rejects_stale_publications_before_reading_content() {
    for change in [
        "UPDATE session_evidence SET status = 'processing'",
        "UPDATE session SET source_generation = 8",
        "UPDATE session SET source_fingerprint = 'replacement'",
        "UPDATE session_evidence SET parser_revision = 0",
        "UPDATE session_evidence SET evidence_schema_revision = 0",
    ] {
        let (store, key) = fixture();
        let first = read(&store, &key, None, None);
        store.lock().execute(change, []).unwrap();
        assert!(
            store
                .published_turn_content_keyset_selected(
                    &key,
                    SelectedContentRequest {
                        source_generation: 7,
                        after_ms: None,
                        source_positions: &BTreeMap::new(),
                        selection: JevInputSelection::ALL,
                        cursor: first.next_cursor.as_ref(),
                    }
                )
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn large_excluded_bodies_do_not_use_selected_page_budget() {
    let (store, key) = fixture();
    let expected = read(&store, &key, None, Some(1000));
    {
        let conn = store.lock();
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, tool_name,
                content, truncated) VALUES ((SELECT MAX(rowid) FROM turn), 3,
                'tool_result', 'tool', 'bash', ?1, 0)",
            [vec![b'x'; 2 * 1024 * 1024]],
        )
        .unwrap();
    }
    assert_eq!(read(&store, &key, None, Some(1000)), expected);
}

#[test]
fn shared_adapter_keeps_selected_edit_paths_and_other_checks_outputs() {
    let (store, key) = fixture();
    {
        let conn = store.lock();
        let fields = serde_json::json!({
            "category": "file_edit", "malformed": false,
            "values": {"file_edit_path": "src/app.rs", "file_edit_content": "x".repeat(2 * 1024 * 1024)}
        });
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, tool_name,
                content, truncated, normalized_fields_json)
             VALUES ((SELECT MAX(rowid) FROM turn), 3, 'tool_input', 'assistant', 'edit',
                CAST('' AS BLOB), 0, ?1)",
            [fields.to_string()],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, tool_name,
                content, truncated) VALUES ((SELECT MAX(rowid) FROM turn), 4,
                'tool_result', 'tool', 'bash', CAST('selected output' AS BLOB), 0);",
        )
        .unwrap();
    }
    for (field, expected) in [
        (JevInputField::FileEditPath, "src/app.rs"),
        (JevInputField::BashCommandOutput, "selected output"),
    ] {
        let page = store
            .published_turn_content_keyset_selected(
                &key,
                SelectedContentRequest {
                    source_generation: 7,
                    after_ms: None,
                    source_positions: &BTreeMap::new(),
                    selection: JevInputSelection::from_fields(&[field]),
                    cursor: None,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(page.content.parts.len(), 1);
        assert!(page.next_cursor.is_none());
        assert_eq!(page.content.coverage.oversized_parts, 0);
        let part = &page.content.parts[0].part;
        if let Some(normalized) = &part.normalized_fields {
            assert_eq!(normalized.values.len(), 1);
            assert_eq!(normalized.values.get(&field).unwrap(), expected);
            assert!(part.text.is_empty());
        } else {
            assert_eq!(part.text, expected);
        }
    }
}
