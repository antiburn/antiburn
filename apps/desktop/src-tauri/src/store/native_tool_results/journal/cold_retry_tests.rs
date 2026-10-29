use std::collections::BTreeSet;

use antiburn_local::analysis::{
    OpenCodeJournalReadError, opencode_journal_has_new_events, read_opencode_live_tool_result_page,
    read_opencode_tool_result_page,
};
use serde_json::json;

use super::tests::{next_live_page, publication_head};
use super::*;
use crate::insights_worker::journal::tests::SyntheticJournal;

fn assert_publication_budget(store: &Store, after: i64) {
    let (count, bytes): (usize, usize) = store
        .lock()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(COALESCE(length(CAST(payload_json AS BLOB)), 0)
         + COALESCE(length(CAST(dependency_binding_json AS BLOB)), 0)
         + COALESCE(length(CAST(session_selection_json AS BLOB)), 0)), 0)
         FROM native_tool_result_publication WHERE publication_sequence > ?1",
            [after],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(
        count <= 40 && bytes <= 4 * 1024 * 1024,
        "{count} outputs, {bytes} bytes"
    );
}

fn source_ready(store: &Store) -> bool {
    let ready = antiburn_checks_store::source_ready_at_sql(1, 2, 3, "100");
    store
        .lock()
        .query_row(
            &format!(
                "SELECT COALESCE(({ready}), 0) FROM session s
            LEFT JOIN session_evidence e USING(environment_key, agent, session_id)"
            ),
            params![
                PARSER_REVISION,
                antiburn_local::analysis::ANALYZER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION
            ],
            |row| row.get(0),
        )
        .unwrap()
}

fn assert_initial_partial(
    store: &Store,
    key: &SessionKey,
    page: &OpenCodeToolResultPage,
    after: i64,
) -> String {
    assert_eq!(page.results.len(), 40);
    assert!(!page.evidence.history_complete);
    assert_publication_budget(store, after);
    let outputs: (usize, usize, usize) = store
        .lock()
        .query_row(
            "SELECT COUNT(*), SUM(outcome = 'update' AND event_kind = 'tool_result'),
            SUM(outcome = 'coverage_gap') FROM native_tool_result_publication
         WHERE publication_sequence > ?1",
            [after],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(outputs, (40, 39, 1));
    assert_eq!(store.native_tool_results_current(key).unwrap().len(), 39);
    let (saved, pending, baseline): (String, bool, Option<u64>) = store
        .lock()
        .query_row(
            "SELECT cursor_json, pending, baseline_sequence FROM opencode_journal_delivery",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(pending);
    assert_eq!(baseline, page.cursor.live_baseline());
    assert!(saved.len() <= 1024 * 1024);
    let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert!(value["cold_replay"].is_object());
    let cursor: OpenCodeJournalCursor = serde_json::from_str(&saved).unwrap();
    assert_eq!(cursor.live_baseline(), page.cursor.live_baseline());
    assert_eq!(cursor.public_baseline(), page.cursor.public_baseline());
    assert!(!source_ready(store));
    saved
}

fn results_after_old_assistant_header(source: &mut SyntheticJournal) {
    source.connection.execute_batch("BEGIN").unwrap();
    for _ in 0..4100 {
        source.event("session.updated.1", json!({"info":{"id":"native-session"}}));
    }
    for index in 0..300 {
        source.result(index);
    }
    source.connection.execute_batch("COMMIT").unwrap();
}

#[test]
fn cold_journal_retry_keeps_original_ownership_after_projection_delete_and_restart() {
    cold_journal_retry_keeps_original_ownership_at_parser_revision(PARSER_REVISION);
}

#[test]
fn parser_66_cold_journal_retry_keeps_partial_publication_after_restart() {
    cold_journal_retry_keeps_original_ownership_at_parser_revision(66);
}

fn cold_journal_retry_keeps_original_ownership_at_parser_revision(parser_revision: i64) {
    use antiburn_local::analysis::ToolResultChange;
    use sha2::{Digest, Sha256};

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    results_after_old_assistant_header(&mut source);
    let (claim, initial) = next_live_page(&store, &source, 100);
    assert_eq!(initial.live_input_events, 4096);
    assert_eq!(initial.results.len(), 40);
    assert!(initial.results.iter().all(|result| {
        result
            .context
            .as_ref()
            .is_some_and(|context| context.main_session)
    }));
    let head = publication_head(&store);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &initial, 100)
            .unwrap()
    );
    assert_initial_partial(&store, &claim.key, &initial, head);
    assert_publication_budget(&store, head);
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap().len(),
        39
    );
    let saved: String = store
        .lock()
        .query_row(
            "SELECT cursor_json FROM opencode_journal_delivery",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(saved.len() <= 1024 * 1024);
    let mut value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert!(value["cold_replay"]["assistant_projections"]["assistant"].is_object());
    let mut old = serde_json::to_value(&initial.cursor).unwrap();
    old["parser_revision"] = json!(parser_revision);
    let cursor: OpenCodeJournalCursor = serde_json::from_value(old).unwrap();
    let digest: [u8; 32] = Sha256::digest(
        serde_json::to_vec(&(
            &cursor,
            &initial.evidence,
            initial.history_complete,
            &value["cold_replay"]["assistant_projections"],
        ))
        .unwrap(),
    )
    .into();
    value["parser_revision"] = json!(parser_revision);
    value["cold_replay"]["output_sha256"] = json!(digest);
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = ?1, cursor_json = ?2",
            params![parser_revision, value.to_string()],
        )
        .unwrap();
    drop(store);

    source.event("message.removed.1", json!({"messageID":"assistant"}));
    source
        .connection
        .execute("DELETE FROM message WHERE id = 'assistant'", [])
        .unwrap();
    let store = Store::open(directory.path()).unwrap();
    let (retry, replay) = next_live_page(&store, &source, 101);
    assert_eq!(replay.results, initial.results);
    assert_eq!(replay.evidence, initial.evidence);
    assert_eq!(replay.cursor, initial.cursor);
    assert!(replay.has_more);
    let head = publication_head(&store);
    assert!(
        store
            .publish_opencode_journal_page(&retry, &replay, 101)
            .unwrap()
    );
    assert_publication_budget(&store, head);
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 40);
    assert!(current.iter().all(|result| !result.removed));
    for expected in &initial.results {
        assert!(current.iter().any(|result| &result.result == expected));
    }
    assert!(source_ready(&store));
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    let mut finished = false;
    for now in 102..108 {
        let pending: bool = store
            .lock()
            .query_row("SELECT pending FROM opencode_journal_delivery", [], |row| {
                row.get(0)
            })
            .unwrap();
        if !pending {
            finished = true;
            break;
        }
        let (claim, page) = next_live_page(&store, &source, now);
        let head = publication_head(&store);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .unwrap()
        );
        assert_publication_budget(&store, head);
    }
    assert!(finished);
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 40);
    assert!(
        current
            .iter()
            .all(|result| result.removed && result.result.change == ToolResultChange::Removal)
    );
    let healthy: bool = store
        .lock()
        .query_row(
            "SELECT blocked = 0 AND pending = 0
         AND COALESCE(last_error, '') <> 'journal-prefix-changed'
         AND json_type(cursor_json, '$.cold_replay') IS NULL
         FROM opencode_journal_delivery",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(healthy);
}

#[test]
fn cold_journal_retry_keeps_absent_ownership_after_projection_insert() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    results_after_old_assistant_header(&mut source);
    source
        .connection
        .execute("DELETE FROM message WHERE id = 'assistant'", [])
        .unwrap();
    let (claim, initial) = next_live_page(&store, &source, 100);
    assert_eq!(initial.results.len(), 40);
    assert!(initial.results.iter().all(|result| {
        result
            .context
            .as_ref()
            .is_some_and(|context| !context.main_session)
    }));
    let head = publication_head(&store);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &initial, 100)
            .unwrap()
    );
    let encoded = assert_initial_partial(&store, &claim.key, &initial, head);
    assert!(encoded.len() <= 1024 * 1024);
    let mut value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    let projections = value["cold_replay"]["assistant_projections"]
        .as_object()
        .unwrap();
    assert!(projections.contains_key("assistant"));
    assert!(projections["assistant"].is_null());
    let cursor: OpenCodeJournalCursor = serde_json::from_value(value.clone()).unwrap();
    drop(store);
    source
        .connection
        .execute(
            "INSERT INTO message VALUES ('assistant', 'native-session', 1000, 1000,
         '{\"role\":\"assistant\",\"parentID\":\"new-parent\"}')",
            [],
        )
        .unwrap();
    let store = Store::open(directory.path()).unwrap();
    let (retry, replay) = next_live_page(&store, &source, 101);
    assert_eq!(retry.cursor().unwrap().unwrap(), cursor);
    assert_eq!(replay.results, initial.results);
    assert_eq!(replay.cursor, initial.cursor);
    let head = publication_head(&store);
    assert!(
        store
            .publish_opencode_journal_page(&retry, &replay, 101)
            .unwrap()
    );
    assert_publication_budget(&store, head);
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap().len(),
        40
    );
    assert!(source_ready(&store));

    value["cold_replay"]["assistant_projections"] = json!({});
    let missing: OpenCodeJournalCursor = serde_json::from_value(value).unwrap();
    let error = read_opencode_live_tool_result_page(
        &source.path,
        "native-session",
        Some(&missing),
        missing.live_baseline(),
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<OpenCodeJournalReadError>(),
        Some(&OpenCodeJournalReadError::SourceChanged)
    );
}

#[test]
fn cold_journal_retry_preserves_the_window_and_new_user_across_append_and_restart() {
    use crate::store::native_tool_results::evidence::EvidenceEventKind;

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.results(0..300);
    let (claim, initial) = next_live_page(&store, &source, 100);
    assert_eq!(initial.results.len(), 40);
    let baseline = initial.cursor.live_baseline();
    let public_baseline = initial.cursor.public_baseline();
    let head = publication_head(&store);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &initial, 100)
            .unwrap()
    );
    assert_initial_partial(&store, &claim.key, &initial, head);
    assert_publication_budget(&store, head);
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap().len(),
        39
    );
    let saved: (String, Option<u64>, bool) = store
        .lock()
        .query_row(
            "SELECT cursor_json, baseline_sequence, pending FROM opencode_journal_delivery",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(saved.0.len() <= 1024 * 1024 && saved.2);
    assert_eq!(saved.1, baseline);
    let cursor: OpenCodeJournalCursor = serde_json::from_str(&saved.0).unwrap();
    assert_eq!(cursor.live_baseline(), baseline);
    assert_eq!(cursor.public_baseline(), public_baseline);
    assert!(opencode_journal_has_new_events(&source.path, "native-session", &cursor).unwrap());
    let partial_head = publication_head(&store);
    assert!(
        !store
            .publish_opencode_journal_page(&claim, &initial, 100)
            .unwrap()
    );
    assert_eq!(publication_head(&store), partial_head);
    drop(store);

    source.event(
        "message.updated.1",
        json!({"info":{"id":"later-user","role":"user"}}),
    );
    source.event(
        "message.part.updated.1",
        json!({"part":{
            "id":"later-text","messageID":"later-user","type":"text","text":"Review the new change"
        }}),
    );
    source.event("message.part.updated.1", json!({"part":{
        "id":"part-299","messageID":"assistant","sessionID":"native-session",
        "type":"tool","callID":"call-299","tool":"bash",
        "state":{"status":"error","input":{"command":"cargo test parser"},"error":"Corrected failure 299"}
    }}));
    source.event(
        "message.updated.1",
        json!({"info":{"id":"assistant","role":"assistant","parentID":"request"}}),
    );
    for index in 300..340 {
        source.start_result(index);
        source.result(index);
    }

    let store = Store::open(directory.path()).unwrap();
    let (retry, replay) = next_live_page(&store, &source, 101);
    assert_eq!(replay.cursor, initial.cursor);
    assert_eq!(replay.results, initial.results);
    assert_eq!(replay.evidence, initial.evidence);
    assert!(replay.has_more);
    assert_eq!(retry.baseline_sequence, baseline);
    let head = publication_head(&store);
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_cold_retry_cursor
        BEFORE UPDATE OF cursor_json ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic cold retry rollback'); END;",
        )
        .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&retry, &replay, 101)
            .is_err()
    );
    assert_eq!(publication_head(&store), head);
    let unchanged: String = store
        .lock()
        .query_row(
            "SELECT cursor_json FROM opencode_journal_delivery",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, saved.0);
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_cold_retry_cursor")
        .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&retry, &replay, 101)
            .unwrap()
    );
    assert_publication_budget(&store, head);
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap().len(),
        40
    );
    assert!(source_ready(&store));
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    let mut finished = false;
    for now in 102..122 {
        let pending: bool = store
            .lock()
            .query_row("SELECT pending FROM opencode_journal_delivery", [], |row| {
                row.get(0)
            })
            .unwrap();
        if !pending {
            finished = true;
            break;
        }
        let head = publication_head(&store);
        let (claim, page) = next_live_page(&store, &source, now);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .unwrap()
        );
        assert_publication_budget(&store, head);
    }
    assert!(
        finished,
        "the original window and appended suffix must finish"
    );
    let publications = store.native_evidence_publications_after(0, 1000).unwrap();
    let users: Vec<_> = publications
        .iter()
        .filter(|publication| publication.event_kind == EvidenceEventKind::UserMessage)
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].live_eligible, Some(true));
    let later_user = serde_json::to_string(&("user_text", "native-session", "later-user")).unwrap();
    assert_eq!(
        users[0].native_identity.as_deref(),
        Some(later_user.as_str())
    );
    let results = store
        .native_tool_result_publications_after(0, 1000)
        .unwrap();
    for expected in &initial.results {
        let initial_publications: Vec<_> = results
            .iter()
            .filter(|publication| {
                publication.payload.as_ref().is_some_and(|result| {
                    result.result_id == expected.result_id && result.revision == expected.revision
                })
            })
            .collect();
        assert_eq!(initial_publications.len(), 1, "{:?}", expected.result_id);
        assert_eq!(initial_publications[0].payload.as_ref(), Some(expected));
    }
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 80);
    assert!(current.iter().all(|result| !result.removed));
    assert_eq!(
        current
            .iter()
            .filter_map(|result| result.result.result_id.clone())
            .collect::<BTreeSet<_>>(),
        (260..340)
            .map(|index| format!("part-{index}"))
            .collect::<BTreeSet<_>>()
    );
    assert!(current.iter().any(|result| {
        result.result.result_id.as_deref() == Some("part-299")
            && result.result.content.as_deref() == Some("Corrected failure 299")
    }));
}

#[test]
fn cold_journal_retry_preserves_pending_window_after_legacy_strict_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.results(0..300);
    source.index(&store, "/synthetic");
    store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let strict =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    let mut legacy = serde_json::to_value(&strict.cursor).unwrap();
    legacy["parser_revision"] = json!(62);
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET cursor_json = ?1, parser_revision = 62,
         baseline_sequence = ?2, lease_expires_at_epoch = NULL",
            params![legacy.to_string(), source.sequence],
        )
        .unwrap();

    let (claim, initial) = next_live_page(&store, &source, 101);
    assert!(claim.cursor().unwrap().unwrap().live_baseline().is_none());
    assert_eq!(initial.results.len(), 40);
    let head = publication_head(&store);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &initial, 101)
            .unwrap()
    );
    assert_initial_partial(&store, &claim.key, &initial, head);
    let (retry, replay) = next_live_page(&store, &source, 102);
    assert_eq!(retry.baseline_sequence, initial.cursor.live_baseline());
    assert_eq!(
        retry.cursor().unwrap().unwrap().public_baseline(),
        initial.cursor.public_baseline()
    );
    assert_eq!(replay.results, initial.results);
    assert_eq!(replay.cursor, initial.cursor);
    assert!(
        store
            .publish_opencode_journal_page(&retry, &replay, 102)
            .unwrap()
    );
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap().len(),
        40
    );
    let complete: bool = store
        .lock()
        .query_row(
            "SELECT pending = 0 AND json_type(cursor_json, '$.cold_replay') IS NULL
         FROM opencode_journal_delivery",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(complete);
}

#[test]
fn cold_journal_retry_rejects_changed_source_before_publishing_pending_results() {
    for change in [
        "owner",
        "origin",
        "boundary",
        "interior",
        "missing-event",
        "fork",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        SyntheticJournal::enable_live(&store, "/synthetic");
        let mut source = SyntheticJournal::new(16);
        source.results(0..300);
        let (claim, page) = next_live_page(&store, &source, 100);
        let head = publication_head(&store);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, 100)
                .unwrap()
        );
        assert_initial_partial(&store, &claim.key, &page, head);
        let current = store.native_tool_results_current(&claim.key).unwrap();
        let before = publication_head(&store);
        match change {
            "owner" => {
                source
                    .connection
                    .execute("UPDATE event_sequence SET owner_id = 'other-owner'", [])
                    .unwrap();
            }
            "origin" => {
                source
                    .connection
                    .execute("UPDATE event SET id = 'changed-origin' WHERE seq = 0", [])
                    .unwrap();
            }
            "boundary" => {
                source
                    .connection
                    .execute(
                        "UPDATE event SET id = 'changed-boundary' WHERE seq = ?1",
                        [source.sequence],
                    )
                    .unwrap();
            }
            "interior" => {
                source.connection.execute("UPDATE event SET data = json_set(data, '$.part.state.error', 'Changed pending input') WHERE json_extract(data, '$.part.id') = ?1", [page.results[20].result_id.as_deref().unwrap()]).unwrap();
            }
            "missing-event" => {
                source
                    .connection
                    .execute("DELETE FROM event WHERE seq = 20", [])
                    .unwrap();
            }
            "fork" => {
                source
                    .connection
                    .execute("UPDATE session SET parent_id = 'parent'", [])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let retry = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
        let cursor = retry.cursor().unwrap().unwrap();
        let error = read_opencode_live_tool_result_page(
            &source.path,
            "native-session",
            Some(&cursor),
            retry.baseline_sequence,
            &|| false,
        )
        .unwrap_err();
        let expected = if change == "missing-event" {
            OpenCodeJournalReadError::IncompleteHistory
        } else {
            OpenCodeJournalReadError::SourceChanged
        };
        assert_eq!(
            error.downcast_ref::<OpenCodeJournalReadError>(),
            Some(&expected),
            "{change}: {error:#}"
        );
        assert_eq!(publication_head(&store), before, "{change}");
        let failure = if change == "missing-event" {
            JournalFailure::IncompleteHistory
        } else {
            JournalFailure::SourceChanged
        };
        assert!(
            store
                .fail_opencode_journal(&retry, failure, 101, 102)
                .unwrap()
        );
        assert_eq!(
            store.native_tool_results_current(&claim.key).unwrap(),
            current,
            "{change}"
        );
        let blocked: bool = store
            .lock()
            .query_row("SELECT blocked FROM opencode_journal_delivery", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(blocked, change != "missing-event", "{change}");
        assert_eq!(
            store
                .claim_next_opencode_journal(102, 60)
                .unwrap()
                .is_none(),
            change != "missing-event",
            "{change}"
        );
    }
}

#[test]
fn cold_journal_retry_rejects_malformed_state_and_output_digest() {
    for change in [
        "digest",
        "baseline",
        "missing-live",
        "legacy",
        "unknown-field",
        "invalid-digest",
        "missing-projections",
        "oversized-projection",
        "too-many-projections",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        SyntheticJournal::enable_live(&store, "/synthetic");
        let mut source = SyntheticJournal::new(16);
        source.results(0..300);
        let (claim, page) = next_live_page(&store, &source, 100);
        let head = publication_head(&store);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, 100)
                .unwrap()
        );
        assert_initial_partial(&store, &claim.key, &page, head);
        let before = publication_head(&store);
        let saved: String = store
            .lock()
            .query_row(
                "SELECT cursor_json FROM opencode_journal_delivery",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        match change {
            "digest" => value["cold_replay"]["output_sha256"] = json!(vec![0; 32]),
            "baseline" => value["live"]["public_baseline"] = json!(0),
            "missing-live" => value["live"] = serde_json::Value::Null,
            "legacy" => {
                value["parser_revision"] = json!(62);
                value["live"]["revision"] = json!(4);
            }
            "unknown-field" => value["cold_replay"]["unexpected"] = json!(true),
            "invalid-digest" => value["cold_replay"]["output_sha256"] = json!([1, 2, 3]),
            "missing-projections" => {
                value["cold_replay"]
                    .as_object_mut()
                    .unwrap()
                    .remove("assistant_projections");
            }
            "oversized-projection" => {
                value["cold_replay"]["assistant_projections"]["assistant"] =
                    json!({"parent_id":"x".repeat(16 * 1024 + 1)})
            }
            "too-many-projections" => {
                value["cold_replay"]["assistant_projections"] = serde_json::Value::Object(
                    (0..41)
                        .map(|index| (format!("assistant-{index}"), serde_json::Value::Null))
                        .collect(),
                )
            }
            _ => unreachable!(),
        }
        store
            .lock()
            .execute(
                "UPDATE opencode_journal_delivery SET cursor_json = ?1, parser_revision = ?2",
                params![
                    value.to_string(),
                    value["parser_revision"].as_i64().unwrap()
                ],
            )
            .unwrap();
        let retry = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
        let result = retry.cursor().and_then(|cursor| {
            read_opencode_live_tool_result_page(
                &source.path,
                "native-session",
                cursor.as_ref(),
                retry.baseline_sequence,
                &|| false,
            )
        });
        assert!(result.is_err(), "{change}");
        assert_eq!(publication_head(&store), before, "{change}");
        assert_eq!(
            store.native_tool_results_current(&claim.key).unwrap().len(),
            39,
            "{change}"
        );
    }
}
