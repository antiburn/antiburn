use antiburn_local::analysis::{
    SourceFormat, ToolResultAdmission, ToolResultChange, ToolResultObservation, ToolResultPosition,
    read_opencode_live_tool_result_page, read_opencode_tool_result_page,
};
use serde_json::json;

use super::*;
use crate::insights_worker::journal::tests::SyntheticJournal;

fn next_live_page(
    store: &Store,
    source: &SyntheticJournal,
    now: i64,
) -> (OpenCodeJournalClaim, OpenCodeToolResultPage) {
    source.index(store, "/synthetic");
    let claim = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
    let page = read_opencode_live_tool_result_page(
        &source.path,
        "native-session",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    (claim, page)
}

fn result_in_message(source: &mut SyntheticJournal, message: &str, part: &str) {
    source.event(
        "message.updated.1",
        json!({"info":{"id":message,"role":"assistant","parentID":"request"}}),
    );
    for status in ["running", "error"] {
        source.event(
            "message.part.updated.1",
            json!({"part":{"id":part,"messageID":message,"type":"tool",
                "callID":format!("call-{part}"),"tool":"bash",
                "state":{"status":status,"input":{"command":"cargo test"},"error":"Synthetic failure"}}}),
        );
    }
}

fn seed_results(store: &Store, key: &SessionKey, results: Vec<ToolResultObservation>) {
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    reconcile_with_policy(
        &tx,
        key,
        1,
        1,
        &ToolResultBatch {
            source_format: results[0].source_format,
            results,
            complete: false,
        },
        PublicationPolicy {
            history_complete: Some(true),
            ..PublicationPolicy::default()
        },
    )
    .unwrap();
    tx.commit().unwrap();
}

#[test]
fn v2_source_label_is_not_claimed_by_the_v1_opencode_journal() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let source = SyntheticJournal::new(16);
    source.index(&store, "/synthetic");
    store
        .lock()
        .execute(
            "UPDATE session SET source_label = 'opencode:native-session:open_code_v2_sqlite'
              WHERE environment_key = 'native' AND agent = 'opencode'
                AND session_id = 'native-session'",
            [],
        )
        .unwrap();

    assert!(
        store
            .claim_next_opencode_journal(100, 60)
            .unwrap()
            .is_none()
    );
}

#[test]
fn v2_selection_retires_the_v1_cursor_without_deleting_immutable_publications() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let (initial, page) = next_live_page(&store, &source, 100);
    assert!(
        store
            .publish_opencode_journal_page(&initial, &page, 100)
            .unwrap()
    );
    let publications_before: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(publications_before > 0);

    let mut v2 = source.record("/synthetic");
    v2.source_label = "opencode:native-session:open_code_v2_sqlite".to_owned();
    store
        .upsert_sessions(&[v2], &crate::agents::evidence_cohort())
        .unwrap();
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM opencode_journal_delivery",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_publication",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        publications_before
    );

    assert!(
        store
            .upsert_sessions(
                &[source.record("/synthetic")],
                &crate::agents::evidence_cohort()
            )
            .is_err()
    );
    assert!(
        store
            .claim_next_opencode_journal(101, 60)
            .unwrap()
            .is_none()
    );

    store.clear_local_session_data().unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    source.index(&store, "/synthetic");
    let fresh = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert!(fresh.cursor().unwrap().is_none());
    assert!(fresh.baseline_sequence.is_none());
    assert!(fresh.requires_full_read);
}

fn publication_head(store: &Store) -> i64 {
    store
        .lock()
        .query_row(
            "SELECT COALESCE(MAX(publication_sequence), 0) FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn user_message_removal_keeps_unrelated_later_results_admitted() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let (claim, page) = next_live_page(&store, &source, 100);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let head = publication_head(&store);

    source.event("message.removed.1", json!({"messageID":"request"}));
    source.start_result(1);
    source.result(1);
    let (claim, page) = next_live_page(&store, &source, 101);
    assert_eq!(page.message_removals.len(), 1);
    assert_eq!(page.message_removals[0].message_id, "request");
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap()
    );
    let events = store
        .native_tool_result_publications_after(head, 10)
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| {
        event.payload.as_ref().unwrap().admission() == ToolResultAdmission::Admitted
            && event.live_eligible == Some(true)
    }));
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 2);
    assert!(current.iter().all(|result| !result.removed));
}

#[test]
fn assistant_removal_uses_exact_durable_bindings_after_parser_eviction_in_native_order() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let (claim, page) = next_live_page(&store, &source, 100);
    let original = page.results[0].clone();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );

    let mut other_format = original.clone();
    other_format.source_format = SourceFormat::ClaudeJsonl;
    seed_results(&store, &claim.key, vec![other_format]);
    let other_key = SessionKey::new("native", "opencode", "other-session");
    let mut other_record = source.record("/other");
    other_record.key = other_key.clone();
    other_record.source_label = "opencode:other-session".into();
    store
        .upsert_sessions(&[other_record], &crate::agents::evidence_cohort())
        .unwrap();
    let mut other_session = original.clone();
    other_session.session_id.clone_from(&other_key.session_id);
    seed_results(&store, &other_key, vec![other_session]);
    for index in 1..140 {
        source.start_result(index);
        source.result(index);
    }
    result_in_message(&mut source, "assistant-child", "part-child");
    let (claim, page) = next_live_page(&store, &source, 101);
    let cursor = serde_json::to_value(&page.cursor).unwrap();
    assert!(cursor["live"]["parts"].get("part-0").is_none());
    assert!(cursor["live"]["parts"].get("part-139").is_some());
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap()
    );
    let head = publication_head(&store);

    source.result(139);
    let update_sequence = source.sequence as u64;
    source.event("message.removed.1", json!({"messageID":"assistant"}));
    let removal_sequence = source.sequence as u64;
    result_in_message(&mut source, "after-removal", "part-after");
    let (claim, page) = next_live_page(&store, &source, 102);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 102)
            .unwrap()
    );
    let events = store
        .native_tool_result_publications_after(head, 1000)
        .unwrap();
    assert_eq!(events.len(), 143);
    assert_eq!(
        events[0].native_position,
        Some(ToolResultPosition::OpenCodeEvent {
            sequence: update_sequence,
            item_index: 0,
        })
    );
    assert_eq!(
        events[0].payload.as_ref().unwrap().change,
        ToolResultChange::Update
    );
    for event in &events[1..141] {
        let result = event.payload.as_ref().unwrap();
        assert_eq!(result.record_id.as_deref(), Some("assistant"));
        assert_eq!(result.change, ToolResultChange::Removal);
        assert_eq!(result.revision, removal_sequence + 1);
        assert_eq!(
            result.position,
            ToolResultPosition::OpenCodeEvent {
                sequence: removal_sequence,
                item_index: 0,
            }
        );
    }
    assert!(events[141..].iter().all(|event| {
        event.payload.as_ref().unwrap().admission() == ToolResultAdmission::Admitted
    }));
    let current = store.native_tool_results_current(&claim.key).unwrap();
    let removed: Vec<_> = current.iter().filter(|result| result.removed).collect();
    assert_eq!(removed.len(), 140);
    let evicted = removed
        .iter()
        .find(|result| result.result.result_id.as_deref() == Some("part-0"))
        .unwrap();
    assert_eq!(evicted.result.context, original.context);
    assert_eq!(current.iter().filter(|result| !result.removed).count(), 3);
    assert!(!store.native_tool_results_current(&other_key).unwrap()[0].removed);
    let head = publication_head(&store);

    source.result(0);
    source.result(139);
    let (claim, mut page) = next_live_page(&store, &source, 103);
    assert!(
        page.results
            .iter()
            .any(|result| result.admission() == ToolResultAdmission::UnsafeIdentity)
    );
    page.results.insert(0, original);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 103)
            .unwrap()
    );
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap(),
        current
    );
    assert!(
        store
            .native_tool_result_publications_after(head, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn message_removal_rolls_back_current_outbox_and_cursor_on_publication_failure() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let (claim, page) = next_live_page(&store, &source, 100);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let cursor = serde_json::to_string(&page.cursor).unwrap();
    let current = store.native_tool_results_current(&claim.key).unwrap();
    let head = publication_head(&store);
    source.event("message.removed.1", json!({"messageID":"assistant"}));
    result_in_message(&mut source, "after-removal", "part-after");
    let (claim, page) = next_live_page(&store, &source, 101);
    store.lock().execute_batch("CREATE TEMP TRIGGER reject_removal_cursor BEFORE UPDATE OF cursor_json ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic removal rollback'); END;").unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .is_err()
    );
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap(),
        current
    );
    assert_eq!(publication_head(&store), head);
    let saved: String = store
        .lock()
        .query_row(
            "SELECT cursor_json FROM opencode_journal_delivery",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, cursor);
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_removal_cursor")
        .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap()
    );
    assert!(
        store
            .native_tool_results_current(&claim.key)
            .unwrap()
            .iter()
            .any(|result| result.removed)
    );
}

#[test]
fn message_removal_fanout_limits_reject_the_whole_page_before_cursor_commit() {
    use antiburn_local::analysis::MAX_TOOL_RESULT_OBSERVATIONS;

    for limit in ["observation", "byte"] {
        let state = tempfile::tempdir().unwrap();
        let store = Store::open(state.path()).unwrap();
        SyntheticJournal::enable_live(&store, "/synthetic");
        let mut source = SyntheticJournal::new(16);
        source.result(0);
        let (claim, page) = next_live_page(&store, &source, 100);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, 100)
                .unwrap()
        );
        let cursor = serde_json::to_string(&page.cursor).unwrap();
        let mut template = page.results[0].clone();
        template.context = None;
        template.arguments = None;
        template.content = (limit == "byte").then(|| "x".repeat(64 * 1024));
        let count = if limit == "observation" {
            MAX_TOOL_RESULT_OBSERVATIONS
        } else {
            64
        };
        let results = (1..=count)
            .map(|index| {
                let mut result = template.clone();
                result.call_id = Some(format!("seed-call-{index}"));
                result.result_id = Some(format!("seed-part-{index}"));
                result
            })
            .collect();
        seed_results(&store, &claim.key, results);
        let head = publication_head(&store);
        source.event("message.removed.1", json!({"messageID":"assistant"}));
        let (claim, page) = next_live_page(&store, &source, 101);
        // One deletion cannot commit only part of its stored bindings.
        let error = store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("exceeds its {limit} limit")),
            "{error}"
        );
        assert_eq!(publication_head(&store), head);
        let saved: (String, i64) = store.lock().query_row(
            "SELECT cursor_json, (SELECT COUNT(*) FROM native_tool_result_current WHERE is_removed = 1)
               FROM opencode_journal_delivery", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(saved, (cursor, 0));
    }
}

#[test]
fn old_parser_receipt_rebuilds_poisoned_current_state_without_a_new_frontier() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.event("message.removed.1", json!({"messageID":"request"}));
    source.start_result(1);
    source.result(1);
    let (claim, page) = next_live_page(&store, &source, 100);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let frontier = page.snapshot_end;
    let generation = claim.source_generation;
    let mut poisoned = serde_json::to_value(&page.cursor).unwrap();
    poisoned["parser_revision"] = json!(58);
    poisoned["live"]["revision"] = json!(1);
    poisoned["live"]["history_safe"] = json!(false);
    store
        .lock()
        .execute_batch(
            "UPDATE native_tool_result_current
        SET payload_json = json_set(payload_json, '$.completeness', 'UnsafeOrder'), complete = 0;",
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = 58,
        cursor_json = ?1, pending = 0, blocked = 0, history_gap = 1, retry_count = 5,
        next_attempt_at_epoch = NULL, last_error = 'journal-history-incomplete'",
            [poisoned.to_string()],
        )
        .unwrap();
    drop(store);

    let store = Store::open(state.path()).unwrap();
    let (claim, page) = next_live_page(&store, &source, 101);
    assert!(claim.cursor().unwrap().is_none());
    assert!(claim.baseline_sequence.is_none());
    assert!(claim.requires_full_read);
    assert_eq!(claim.retry_count, 0);
    assert_eq!(claim.source_generation, generation);
    assert_eq!(page.snapshot_end, frontier);
    assert!(!page.has_more);
    assert!(page.history_complete);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap()
    );
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 2);
    assert!(
        current
            .iter()
            .all(|result| result.result.admission() == ToolResultAdmission::Admitted)
    );
    let cursor = serde_json::to_value(&page.cursor).unwrap();
    assert_eq!(cursor["parser_revision"], json!(PARSER_REVISION));
    assert_eq!(cursor["live"]["revision"], json!(2));
}

#[test]
fn old_parser_recovery_removes_a_stored_later_correction_and_keeps_its_tombstone() {
    use antiburn_local::analysis::ToolResultCompleteness;

    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let (claim, page) = next_live_page(&store, &source, 100);
    let original = page.results[0].clone();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );

    source.event("message.removed.1", json!({"messageID":"assistant"}));
    let removal_sequence = source.sequence as u64;
    source.result(0);
    let correction_sequence = source.sequence as u64;
    let (claim, page) = next_live_page(&store, &source, 101);
    let frontier = page.snapshot_end;
    let generation = claim.source_generation;
    let mut correction = original.clone();
    correction.position = ToolResultPosition::OpenCodeEvent {
        sequence: correction_sequence,
        item_index: 0,
    };
    correction.revision = correction_sequence + 1;
    correction.completeness = ToolResultCompleteness::UnsafeOrder;
    seed_results(&store, &claim.key, vec![correction.clone()]);
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 1);
    assert!(!current[0].removed);
    assert_eq!(current[0].native_revision, correction.revision as i64);
    assert!(current[0].result.revision > removal_sequence + 1);
    assert_eq!(
        current[0].result.admission(),
        ToolResultAdmission::UnsafeOrder
    );

    let mut poisoned = serde_json::to_value(&page.cursor).unwrap();
    poisoned["parser_revision"] = json!(58);
    poisoned["live"]["revision"] = json!(1);
    poisoned["live"]["history_safe"] = json!(false);
    poisoned["live"]["parts"]["part-0"]["unsafe_ownership"] = json!(false);
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = 58,
            cursor_json = ?1, observed_generation = ?2, pending = 0, blocked = 0,
            history_gap = 1, lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL,
            last_error = 'journal-history-incomplete'",
            params![poisoned.to_string(), generation],
        )
        .unwrap();
    let head = publication_head(&store);
    drop(store);

    let store = Store::open(state.path()).unwrap();
    let (claim, page) = next_live_page(&store, &source, 102);
    assert!(claim.cursor().unwrap().is_none());
    assert!(claim.requires_full_read);
    assert_eq!(claim.source_generation, generation);
    assert_eq!(page.snapshot_end, frontier);
    assert_eq!(page.message_removals.len(), 1);
    assert_eq!(page.message_removals[0].sequence, removal_sequence);
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].revision, correction.revision);
    assert_eq!(
        page.results[0].admission(),
        ToolResultAdmission::UnsafeOwnership
    );
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 102)
            .unwrap()
    );

    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 1);
    assert!(current[0].removed);
    assert_eq!(current[0].result.revision, correction.revision + 1);
    let position = ToolResultPosition::OpenCodeEvent {
        sequence: removal_sequence,
        item_index: 0,
    };
    assert_eq!(current[0].native_position, position);
    assert_eq!(current[0].result.context, original.context);
    let events = store
        .native_tool_result_publications_after(head, 10)
        .unwrap();
    let results: Vec<_> = events
        .iter()
        .filter(|event| event.payload.is_some())
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].native_position, Some(position));
    assert_eq!(
        results[0].native_revision,
        Some((correction.revision + 1) as i64)
    );
    assert_eq!(
        results[0].payload.as_ref().unwrap().change,
        ToolResultChange::Removal
    );
    assert_eq!(
        results[0].payload.as_ref().unwrap().context,
        original.context
    );
    let head = publication_head(&store);

    source.result(0);
    source.result(0);
    let (claim, page) = next_live_page(&store, &source, 103);
    assert!(page.results.last().unwrap().revision > current[0].result.revision);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 103)
            .unwrap()
    );
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap(),
        current
    );
    assert!(
        store
            .native_tool_result_publications_after(head, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn live_window_and_incremental_state_commit_atomically_and_resume_after_restart() {
    use antiburn_local::analysis::{ToolResultAdmission, read_opencode_live_tool_result_page};
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.results(0..300);
    source.start_result(300);
    source.index(&store, "/synthetic");
    let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_live_tool_result_page(&source.path, "native-session", None, None, &|| false)
            .unwrap();
    assert_eq!(page.results.len(), 40);
    assert!(!page.has_more);
    store.lock().execute_batch("CREATE TEMP TRIGGER reject_live_cursor BEFORE UPDATE OF cursor_json ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic live rollback'); END;").unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .is_err()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT cursor_json FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap(),
        None
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_live_cursor")
        .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let published = store.native_tool_result_publications_after(0, 100).unwrap();
    assert_eq!(published.len(), 40);
    assert!(
        published
            .iter()
            .all(|event| event.live_eligible == Some(true))
    );
    let frozen = published[30].payload.as_ref().unwrap().context.clone();
    let head = published.last().unwrap().publication_sequence;
    drop(store);
    let store = Store::open(state.path()).unwrap();
    source.result(300);
    source.result(290);
    source.result(0);
    source.index(&store, "/synthetic");
    let next = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(next.cursor().unwrap(), Some(page.cursor.clone()));
    assert_eq!(next.baseline_sequence, page.cursor.live_baseline());
    let page = read_opencode_live_tool_result_page(
        &source.path,
        "native-session",
        next.cursor().unwrap().as_ref(),
        next.baseline_sequence,
        &|| false,
    )
    .unwrap();
    assert_eq!(page.live_input_events, 3);
    assert!(
        store
            .publish_opencode_journal_page(&next, &page, 101)
            .unwrap()
    );
    assert!(
        !store
            .publish_opencode_journal_page(&next, &page, 101)
            .unwrap()
    );
    let events = store
        .native_tool_result_publications_after(head, 100)
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].payload.as_ref().unwrap().admission(),
        ToolResultAdmission::Admitted
    );
    assert_eq!(events[1].payload.as_ref().unwrap().context, frozen);
    assert_eq!(
        events[2].payload.as_ref().unwrap().admission(),
        ToolResultAdmission::UnsafeIdentity
    );
}

#[test]
fn cursor_and_baseline_commit_atomically_with_outbox_and_survive_restart() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(40 * 1024);
    source.results(0..300);
    source.index(&store, "/synthetic");
    let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(page.has_more && page.history_complete && !page.complete);
    store.lock().execute_batch("CREATE TEMP TRIGGER reject_journal_cursor BEFORE UPDATE OF cursor_json ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic rollback'); END;").unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .is_err()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT cursor_json FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap(),
        None
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_journal_cursor")
        .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    assert!(
        !store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let count: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(store);
    let store = Store::open(state.path()).unwrap();
    let next = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(next.cursor().unwrap(), Some(page.cursor));
    assert_eq!(next.baseline_sequence, Some(source.sequence as u64));
    let page = read_opencode_tool_result_page(
        &source.path,
        "native-session",
        next.cursor().unwrap().as_ref(),
        &|| false,
    )
    .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&next, &page, 101)
            .unwrap()
    );
    assert!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_publication",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap()
            > count
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_current WHERE is_removed = 1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    use antiburn_checks::source::{PublicationCursor, PublicationRead, PublishedEvidenceReader};
    let reader = crate::published_evidence_reader::StorePublishedEvidenceReader::new(&store);
    let published = reader
        .read_batch(PublicationRead {
            after: PublicationCursor(0),
            limit: 1000,
        })
        .unwrap();
    assert!(published.has_more);
    assert!(published.events.len() < count as usize + page.results.len());
}

#[test]
fn unassigned_sessions_wait_for_an_enabled_activation_in_the_assigned_project() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let mut record = source.record("/synthetic");
    record.cwd = None;
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(
        store
            .claim_next_opencode_journal(100, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.next_opencode_journal_attempt(100).unwrap(), None);
    record.cwd = Some("/new-project".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(
        store
            .claim_next_opencode_journal(101, 60)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    SyntheticJournal::enable_live(&store, "/new-project");
    let claim = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap()
    );
    assert_eq!(
        store
            .native_tool_result_publications_for_project_after("native", "/new-project", 0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn append_during_a_page_keeps_lease_and_prefix_cas_but_not_project_or_incarnation_changes() {
    for change in [
        "append",
        "project",
        "incarnation",
        "lease",
        "fork",
        "subagent",
        "wsl",
    ] {
        let state = tempfile::tempdir().unwrap();
        let store = Store::open(state.path()).unwrap();
        SyntheticJournal::enable_live(&store, "/synthetic");
        SyntheticJournal::enable_live(&store, "/other");
        let mut source = SyntheticJournal::new(16);
        source.result(0);
        source.index(&store, "/synthetic");
        let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
        let page = read_opencode_tool_result_page(&source.path, "native-session", None, &|| false)
            .unwrap();
        source.result(1);
        if change == "incarnation" {
            store.delete_session(&claim.key).unwrap();
        }
        let mut record = source.record(if change == "project" {
            "/other"
        } else {
            "/synthetic"
        });
        if change == "fork" {
            record.fork_parent_session_id = Some("synthetic-parent".into());
        }
        if change == "subagent" {
            store.lock().execute("INSERT INTO session_relation VALUES ('native', 'opencode', 'parent', 'subagent', 'native-session', NULL)", []).unwrap();
        }
        if change == "wsl" {
            record.wsl_distro = Some("Synthetic".into());
        }
        store
            .upsert_sessions(&[record], &crate::agents::evidence_cohort())
            .unwrap();
        let now = if change == "lease" { 161 } else { 101 };
        assert_eq!(
            store.renew_opencode_journal_lease(&claim, now, 60).unwrap(),
            change == "append"
        );
        assert_eq!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .unwrap(),
            change == "append",
            "{change}"
        );
        let events = store.native_tool_result_publications_after(0, 10).unwrap();
        if change == "append" {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].live_eligible, Some(false));
            let selected = {
                let mut connection = store.lock();
                let transaction = connection.transaction().unwrap();
                let fair_turn = transaction
                    .query_row(
                        "SELECT successful_turns % 4 = 3 FROM work_queue_schedule
                          WHERE domain = 'native_delivery'",
                        [],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap();
                let selected =
                    super::super::next_live_source_in(&transaction, 102, fair_turn).unwrap();
                transaction.rollback().unwrap();
                selected
            };
            assert_eq!(
                selected.as_ref().map(|item| item.0.as_str()),
                Some("journal")
            );
            let next = store.claim_next_opencode_journal(102, 60).unwrap().unwrap();
            let page = read_opencode_tool_result_page(
                &source.path,
                "native-session",
                next.cursor().unwrap().as_ref(),
                &|| false,
            )
            .unwrap();
            assert!(
                store
                    .publish_opencode_journal_page(&next, &page, 102)
                    .unwrap()
            );
            assert_eq!(
                store
                    .native_tool_result_publications_after(events[0].publication_sequence, 10)
                    .unwrap()[0]
                    .live_eligible,
                Some(true)
            );
        } else {
            assert!(events.is_empty());
            if matches!(change, "fork" | "subagent" | "wsl") {
                assert!(
                    store
                        .claim_next_opencode_journal(now, 60)
                        .unwrap()
                        .is_none()
                );
                continue;
            }
            let replacement = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
            assert_ne!(replacement.claim_fence, claim.claim_fence);
        }
    }
}

#[test]
fn native_delivery_fair_turn_survives_restart_and_is_shared_by_both_claimers() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    SyntheticJournal::enable_live(&store, "/other");
    let mut journal = SyntheticJournal::new(16);
    journal.result(0);
    let journal_record = journal.record("/synthetic");
    journal.index(&store, "/synthetic");

    let mut file_record = journal_record.clone();
    file_record.key = SessionKey::new("native", "codex", "file-session");
    file_record.source_kind = "file".into();
    file_record.source_label = "/synthetic/session.jsonl".into();
    file_record.updated_at_epoch = Some(2_000);
    store
        .upsert_sessions(&[file_record], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO native_file_delivery (
                 environment_key, agent, session_id, incarnation, source_label, cwd,
                 parser_revision, active_until_epoch, service_ticket)
             SELECT environment_key, agent, session_id, incarnation, source_label, cwd,
                    ?1, 0, 10
               FROM session WHERE session_id = 'file-session'",
            [antiburn_local::analysis::PARSER_REVISION],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO opencode_journal_delivery (
                 environment_key, agent, session_id, incarnation, source_label, cwd,
                 parser_revision, main_session, service_ticket)
             SELECT environment_key, agent, session_id, incarnation, source_label, cwd,
                    ?1, 1, 9
               FROM session WHERE session_id = 'native-session'",
            [antiburn_local::analysis::PARSER_REVISION],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE work_queue_schedule SET next_ticket = 11, successful_turns = 3
              WHERE domain = 'native_delivery'",
            [],
        )
        .unwrap();
    drop(store);

    let store = Store::open(state.path()).unwrap();
    store.recover_native_file_claims().unwrap();
    store.recover_opencode_journal_claims().unwrap();

    // Recent-first favors the file session, but this is the fourth shared turn.
    assert!(store.claim_next_native_file(2_100, 60).unwrap().is_none());
    let claim = store
        .claim_next_opencode_journal(2_100, 60)
        .unwrap()
        .expect("oldest ticket across source families");
    assert_eq!(claim.key.session_id, "native-session");
    let schedule: (i64, i64) = store
        .lock()
        .query_row(
            "SELECT next_ticket, successful_turns FROM work_queue_schedule
              WHERE domain = 'native_delivery'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(schedule, (12, 4));
}

#[test]
fn known_fork_ownership_fences_admitted_work_and_prevents_new_reads() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    let original_context = page.results[0].context.clone();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let head = store.native_tool_result_publications_after(0, 10).unwrap()[0].publication_sequence;
    let admitted = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let mut record = source.record("/synthetic");
    record.fork_parent_session_id = Some("synthetic-parent".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(
        !store
            .publish_opencode_journal_page(&admitted, &page, 101)
            .unwrap()
    );
    assert!(
        !store
            .renew_opencode_journal_lease(&admitted, 101, 60)
            .unwrap()
    );
    assert!(
        store
            .claim_next_opencode_journal(101, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.next_opencode_journal_attempt(101).unwrap(), None);
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current[0].result.context, original_context);
    let events = store
        .native_tool_result_publications_after(head, 10)
        .unwrap();
    assert!(events.is_empty());
}

#[test]
fn changed_prefix_keeps_cursor_and_persists_a_blocked_gap_without_repeated_reads() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let first = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&first, &page, 100)
            .unwrap()
    );
    source.result(1);
    source.index(&store, "/synthetic");
    let next = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    source.connection.execute("UPDATE event SET data = replace(data, 'Parser failure 0', 'Changed failure') WHERE seq = 4", []).unwrap();
    assert!(
        read_opencode_tool_result_page(
            &source.path,
            "native-session",
            next.cursor().unwrap().as_ref(),
            &|| false
        )
        .is_err()
    );
    assert!(
        store
            .fail_opencode_journal(&next, JournalFailure::SourceChanged, 101, 131)
            .unwrap()
    );
    source.result(2);
    source.index(&store, "/synthetic");
    assert!(
        store
            .claim_next_opencode_journal(1_000, 60)
            .unwrap()
            .is_none()
    );
    let saved: (String, String) = store
        .lock()
        .query_row(
            "SELECT cursor_json, last_error FROM opencode_journal_delivery",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved.0, serde_json::to_string(&page.cursor).unwrap());
    assert_eq!(saved.1, "journal-prefix-changed");
    let events = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(
        events.last().unwrap().gap_reason.as_deref(),
        Some("journal-prefix-changed")
    );
    drop(store);
    let store = Store::open(state.path()).unwrap();
    assert!(
        store
            .claim_next_opencode_journal(2_000, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .last()
            .unwrap()
            .gap_reason
            .as_deref(),
        Some("journal-prefix-changed")
    );
}

#[test]
fn cancelled_delivery_retains_its_cursor_and_does_not_consume_a_retry_or_publish_a_gap() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let first = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    assert!(
        store
            .fail_opencode_journal(&first, JournalFailure::Cancelled, 100, 130)
            .unwrap()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .claim_next_opencode_journal(100, 60)
            .unwrap()
            .is_none()
    );
    let retry = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(retry.retry_count, 0);
    assert!(retry.cursor().unwrap().is_none());
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        !store
            .publish_opencode_journal_page(&first, &page, 101)
            .unwrap()
    );
    assert!(
        store
            .publish_opencode_journal_page(&retry, &page, 101)
            .unwrap()
    );
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn clear_local_data_removes_journal_state_and_fences_old_incarnation_publication() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let first = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&first, &page, 100)
            .unwrap()
    );
    source.result(1);
    source.index(&store, "/synthetic");
    let old = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let page = read_opencode_tool_result_page(
        &source.path,
        "native-session",
        old.cursor().unwrap().as_ref(),
        &|| false,
    )
    .unwrap();
    store.clear_local_session_data().unwrap();
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    source.index(&store, "/synthetic");
    SyntheticJournal::enable_live(&store, "/synthetic");
    let replacement = store.claim_next_opencode_journal(102, 60).unwrap().unwrap();
    assert_ne!(old.incarnation, replacement.incarnation);
    assert_ne!(old.claim_fence, replacement.claim_fence);
    assert!(
        !store
            .publish_opencode_journal_page(&old, &page, 102)
            .unwrap()
    );
    assert!(replacement.cursor().unwrap().is_none());
}
