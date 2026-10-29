use antiburn_local::analysis::{
    SourceFormat, ToolResultAdmission, ToolResultChange, ToolResultObservation, ToolResultPosition,
    read_opencode_live_tool_result_page, read_opencode_tool_result_page,
};
use serde_json::json;

use super::*;
use crate::insights_worker::journal::tests::SyntheticJournal;
use crate::store::native_tool_results::{
    ToolResultPublicationOutcome, main_64_preservation_tests as preservation,
};

pub(super) fn next_live_page(
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

pub(super) fn publication_head(store: &Store) -> i64 {
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
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.gap_reason.as_deref() == Some("native-message-removal-unbound"))
            .count(),
        1
    );
    let results = events
        .iter()
        .filter(|event| event.payload.is_some())
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|event| {
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
    let mut now = 101;
    loop {
        let (claim, page) = next_live_page(&store, &source, now);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .unwrap()
        );
        if !page.has_more {
            let cursor = serde_json::to_value(&page.cursor).unwrap();
            assert!(cursor["live"]["parts"].get("part-0").is_none());
            assert!(cursor["live"]["parts"].get("part-139").is_some());
            break;
        }
        now += 1;
    }
    let head = publication_head(&store);

    source.result(139);
    let update_sequence = source.sequence as u64;
    source.event("message.removed.1", json!({"messageID":"assistant"}));
    let removal_sequence = source.sequence as u64;
    result_in_message(&mut source, "after-removal", "part-after");
    loop {
        now += 1;
        let (claim, page) = next_live_page(&store, &source, now);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .unwrap()
        );
        let pending: bool = store
            .lock()
            .query_row("SELECT pending FROM opencode_journal_delivery", [], |r| {
                r.get(0)
            })
            .unwrap();
        if !pending {
            break;
        }
    }
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
    now += 1;
    let (claim, mut page) = next_live_page(&store, &source, now);
    assert!(
        page.results
            .iter()
            .any(|result| result.admission() == ToolResultAdmission::UnsafeIdentity)
    );
    page.results.insert(0, original.clone());
    page.evidence.events.insert(
        0,
        antiburn_local::analysis::NativeEvidenceEvent {
            live_eligible: true,
            fact: antiburn_local::analysis::NativeEvidenceFact::Message(
                antiburn_local::analysis::NativeMessageObservation::ToolResult(original),
            ),
        },
    );
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, now)
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
fn message_removal_fanout_commits_bounded_progress_before_its_source_cursor() {
    use antiburn_local::analysis::MAX_TOOL_RESULT_OBSERVATIONS;

    for limit in ["observation", "byte", "exact-count"] {
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
        template.content = (limit == "byte").then(|| "界".repeat(64 * 1024));
        let count = if limit == "observation" {
            MAX_TOOL_RESULT_OBSERVATIONS
        } else if limit == "byte" {
            64
        } else {
            39
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
        source.event(
            "message.updated.1",
            json!({"info":{"id":"later-user","role":"user"}}),
        );
        source.event("message.part.updated.1", json!({"part":{"id":"later-text","messageID":"later-user","type":"text","text":"Work after the removal"}}));
        let (claim, page) = next_live_page(&store, &source, 101);
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, 101)
                .unwrap()
        );
        assert!(publication_head(&store) > head);
        let saved: (String, i64) = store.lock().query_row(
            "SELECT cursor_json, (SELECT COUNT(*) FROM native_tool_result_current WHERE is_removed = 1)
               FROM opencode_journal_delivery", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(saved.0, cursor);
        assert!(saved.1 > 0 && saved.1 <= 40);
        let outputs: (usize, usize) = store
            .lock()
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(COALESCE(length(CAST(payload_json AS BLOB)), 0)
                + COALESCE(length(CAST(dependency_binding_json AS BLOB)), 0)), 0)
             FROM native_tool_result_publication WHERE publication_sequence > ?1",
                [head],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(
            outputs.0 <= 40 && outputs.1 <= 4 * 1024 * 1024,
            "{limit}: {outputs:?}"
        );
        assert!(
            !store
                .native_evidence_publications_after(head, 100)
                .unwrap()
                .iter()
                .any(|publication| publication.event_kind
                    == crate::store::native_tool_results::evidence::EvidenceEventKind::UserMessage)
        );
        let mut now = 101;
        for _ in 0..count / 16 + 3 {
            let pending: bool = store
                .lock()
                .query_row("SELECT pending FROM opencode_journal_delivery", [], |r| {
                    r.get(0)
                })
                .unwrap();
            if !pending {
                break;
            }
            now += 1;
            let (claim, page) = next_live_page(&store, &source, now);
            assert!(
                store
                    .publish_opencode_journal_page(&claim, &page, now)
                    .unwrap()
            );
        }
        assert_eq!(
            store
                .native_tool_results_current(&claim.key)
                .unwrap()
                .iter()
                .filter(|r| r.removed)
                .count(),
            count + 1
        );
        assert!(
            store
                .lock()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM native_tool_result_publication
            WHERE publication_sequence > ?1 AND event_kind = 'user_message')",
                    [head],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap(),
            "{limit}"
        );
    }
}

#[test]
fn unknown_parser_receipt_fails_closed_without_replaying_a_poisoned_prefix() {
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
    assert!(
        store
            .claim_next_opencode_journal(101, 60)
            .unwrap()
            .is_none()
    );
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current.len(), 2);
    assert!(
        current
            .iter()
            .all(|result| result.result.admission() == ToolResultAdmission::UnsafeOrder)
    );
    let saved: String = store
        .lock()
        .query_row(
            "SELECT cursor_json FROM opencode_journal_delivery",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(saved, poisoned.to_string());
}

fn main_64_journal_fixture() -> (
    tempfile::TempDir,
    std::sync::Arc<Store>,
    std::path::PathBuf,
    rusqlite::Connection,
    serde_json::Value,
    SessionKey,
) {
    use crate::store::SessionRecord;
    let saved: serde_json::Value = serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/native_receipt_compatibility/journal-v1.json"))).unwrap();
    assert_eq!(saved["cursor"]["parser_revision"], 64);
    assert_eq!(saved["cursor"]["live"]["revision"], 2);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.db");
    let source = rusqlite::Connection::open(&path).unwrap();
    source.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY,parent_id TEXT); INSERT INTO session VALUES('root',NULL);
        CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,data TEXT);
        CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,data TEXT);
        CREATE TABLE event_sequence(aggregate_id TEXT PRIMARY KEY,seq INTEGER,owner_id TEXT);
        CREATE TABLE event(id TEXT PRIMARY KEY,aggregate_id TEXT,seq INTEGER,type TEXT,data TEXT);
        CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id,seq);").unwrap();
    for event in saved["events"].as_array().unwrap() {
        source
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
    source
        .execute("INSERT INTO event_sequence VALUES('root',5,NULL)", [])
        .unwrap();
    let store = std::sync::Arc::new(Store::open(&directory.path().join("app")).unwrap());
    SyntheticJournal::enable_live(&store, "/synthetic");
    let key = SessionKey::new("native", "opencode", "root");
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "providerDb".into(),
                source_label: "opencode:root".into(),
                cwd: Some("/synthetic".into()),
                wsl_distro: None,
                title: None,
                title_source: None,
                surface: "cli".into(),
                updated_at_epoch: Some(100),
                activity_cursor: String::new(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some("synthetic".into()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let completed: Vec<ToolResultObservation> =
        serde_json::from_value(saved["completed"].clone()).unwrap();
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        reconcile_with_policy(
            &tx,
            &key,
            claim.source_generation,
            claim.claim_fence,
            &ToolResultBatch {
                source_format: completed[0].source_format,
                results: completed,
                complete: false,
            },
            PublicationPolicy {
                live_eligible: Some(true),
                gap_kind: PublicationGapKind::Partial,
                ..Default::default()
            },
        )
        .unwrap();
        tx.commit().unwrap();
    }
    store.lock().execute("UPDATE opencode_journal_delivery SET parser_revision=64,cursor_json=?1,
        baseline_sequence=0,observed_generation=?2,pending=1,lease_expires_at_epoch=NULL,next_attempt_at_epoch=101",
        params![saved["cursor"].to_string(), claim.source_generation]).unwrap();
    (directory, store, path, source, saved, key)
}

#[test]
fn main_64_actual_journal_receipt_converts_and_continues_without_reseed() {
    let (directory, store, path, source, saved, key) = main_64_journal_fixture();
    let paid = preservation::seed(&store);
    let before = store.native_tool_results_current(&key).unwrap();
    drop(store);
    let store = std::sync::Arc::new(Store::open(&directory.path().join("app")).unwrap());
    paid.assert_preserved(&store);
    let claim = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let page = read_opencode_live_tool_result_page(
        &path,
        "root",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    assert_eq!(page.live_input_events, 0);
    assert!(page.results.is_empty() && page.evidence.events.is_empty());
    store
        .publish_opencode_journal_page(&claim, &page, 101)
        .unwrap();
    assert_eq!(store.native_tool_results_current(&key).unwrap(), before);
    paid.assert_preserved(&store);
    source
        .execute(
            "INSERT INTO event VALUES('append-6','root',6,'message.part.updated.1',?1)",
            [saved["completion"].to_string()],
        )
        .unwrap();
    source
        .execute("UPDATE event_sequence SET seq=6", [])
        .unwrap();
    let claim = store.claim_next_opencode_journal(102, 60).unwrap().unwrap();
    let page = read_opencode_live_tool_result_page(
        &path,
        "root",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].call_id.as_deref(), Some("call-1"));
    store
        .publish_opencode_journal_page(&claim, &page, 102)
        .unwrap();
    assert_eq!(
        store.native_tool_results_current(&key).unwrap().len(),
        before.len() + 1
    );
    paid.assert_preserved(&store);
    drop(store);
    let store = std::sync::Arc::new(Store::open(&directory.path().join("app")).unwrap());
    let head = publication_head(&store);
    let claim = store.claim_next_opencode_journal(103, 60).unwrap().unwrap();
    let idle = read_opencode_live_tool_result_page(
        &path,
        "root",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
    store
        .publish_opencode_journal_page(&claim, &idle, 103)
        .unwrap();
    assert_eq!(publication_head(&store), head);
    paid.assert_paid_fences(&store, false);
}

fn assert_retained_journal_parser_mismatch_rejected(
    store_directory: &std::path::Path,
    store: std::sync::Arc<Store>,
    key: &SessionKey,
    stored_parser: i64,
    cursor: serde_json::Value,
) {
    let encoded_parser = cursor["parser_revision"].as_i64().unwrap();
    assert_ne!(stored_parser, encoded_parser);
    let encoded = cursor.to_string();
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision=?1,cursor_json=?2,
        pending=1,blocked=0,lease_expires_at_epoch=NULL,next_attempt_at_epoch=101",
            params![stored_parser, encoded],
        )
        .unwrap();
    let receipt = |store: &Store| {
        store.lock().query_row("SELECT parser_revision,cursor_json,baseline_sequence FROM opencode_journal_delivery", [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<u64>>(2)?))).unwrap()
    };
    let before_receipt = receipt(&store);
    let before_results = store.native_tool_results_current(key).unwrap();
    assert!(!before_results.is_empty());
    let before_publications = store.native_evidence_publications_after(0, 100).unwrap();
    let claim = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(&claim.key, key);
    assert_eq!(claim.parser_revision, Some(stored_parser));
    assert_eq!(claim.cursor_json.as_deref(), Some(encoded.as_str()));
    assert!(
        claim.cursor().is_err(),
        "stored parser {stored_parser}, encoded parser {encoded_parser}, live layout {}",
        cursor["live"]["revision"]
    );
    assert_eq!(receipt(&store), before_receipt);
    assert_eq!(
        store.native_tool_results_current(key).unwrap(),
        before_results
    );
    assert_eq!(
        store.native_evidence_publications_after(0, 100).unwrap(),
        before_publications
    );
    assert!(
        store
            .fail_opencode_journal(&claim, JournalFailure::InvalidCursor, 101, 101)
            .unwrap()
    );
    assert_eq!(receipt(&store), before_receipt);
    assert_eq!(
        store.native_tool_results_current(key).unwrap(),
        before_results
    );
    assert!(store.lock().query_row("SELECT blocked=1 AND pending=0 AND last_error='journal-cursor-invalid' FROM opencode_journal_delivery", [], |row| row.get::<_, bool>(0)).unwrap());
    let publications = store.native_evidence_publications_after(0, 100).unwrap();
    assert!(publications.starts_with(&before_publications));
    assert_eq!(publications.len(), before_publications.len() + 1);
    assert_eq!(
        publications.last().unwrap().outcome,
        crate::store::native_tool_results::ToolResultPublicationOutcome::CoverageGap
    );
    assert_eq!(
        publications.last().unwrap().gap_reason.as_deref(),
        Some("journal-cursor-invalid")
    );
    drop(store);
    let store = Store::open(store_directory).unwrap();
    assert!(
        store
            .claim_next_opencode_journal(200, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(receipt(&store), before_receipt);
    assert_eq!(
        store.native_tool_results_current(key).unwrap(),
        before_results
    );
    assert_eq!(
        store.native_evidence_publications_after(0, 100).unwrap(),
        publications
    );
}

#[test]
fn main_64_journal_loader_rejects_mismatched_row_and_encoded_legacy_parsers() {
    for (stored, encoded, live) in [
        (64, 62, 4),
        (62, 64, 2),
        (63, 64, 2),
        (65, 64, 2),
        (66, 64, 2),
        (PARSER_REVISION, 64, 2),
    ] {
        let (directory, store, _path, source, saved, key) = main_64_journal_fixture();
        let mut cursor = saved["cursor"].clone();
        cursor["parser_revision"] = json!(encoded);
        cursor["live"]["revision"] = json!(live);
        append_main_64_journal_event(&source, "message.part.updated.1", &saved["completion"]);
        assert_retained_journal_parser_mismatch_rejected(
            &directory.path().join("app"),
            store,
            &key,
            stored,
            cursor,
        );
    }
}

#[test]
fn journal_cursor_loader_rejects_mismatched_known_current_layout_parsers() {
    for stored in [62, 63, 64, 65, 66, PARSER_REVISION] {
        for encoded in [63, 64, 65, 66, PARSER_REVISION] {
            if stored == encoded {
                continue;
            }
            let directory = tempfile::tempdir().unwrap();
            let store = std::sync::Arc::new(Store::open(directory.path()).unwrap());
            SyntheticJournal::enable_live(&store, "/synthetic");
            let mut source = SyntheticJournal::new(16);
            source.result(0);
            let (claim, page) = next_live_page(&store, &source, 100);
            store
                .publish_opencode_journal_page(&claim, &page, 100)
                .unwrap();
            let mut cursor = serde_json::to_value(&page.cursor).unwrap();
            assert_eq!(cursor["live"]["revision"], 5);
            cursor["parser_revision"] = json!(encoded);
            source.start_result(1);
            source.result(1);
            assert_retained_journal_parser_mismatch_rejected(
                directory.path(),
                store,
                &claim.key,
                stored,
                cursor,
            );
        }
    }
}

fn append_main_64_journal_event(
    source: &rusqlite::Connection,
    kind: &str,
    data: &serde_json::Value,
) -> u64 {
    let mut data = data.clone();
    data["sessionID"] = json!("root");
    let sequence: u64 = source
        .query_row(
            "UPDATE event_sequence SET seq=seq+1 RETURNING seq",
            [],
            |row| row.get(0),
        )
        .unwrap();
    source
        .execute(
            "INSERT INTO event VALUES(?1,'root',?2,?3,?4)",
            params![
                format!("append-{sequence}"),
                sequence,
                kind,
                data.to_string()
            ],
        )
        .unwrap();
    sequence
}

fn main_64_journal_attachment_targets(
    store: &Store,
    key: &SessionKey,
) -> std::collections::BTreeSet<String> {
    let connection = store.lock();
    let rows = connection
        .prepare(
            "SELECT json_array(t.check_id,t.activation_scope_key,t.generation,
             t.branch_key,t.incarnation,t.observer_epoch)
         FROM burn_check_session_attachment t
         WHERE t.branch_key=?1 ORDER BY t.check_id",
        )
        .unwrap()
        .query_map([key.to_identity_json()], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.into_iter().collect()
}

fn assert_main_64_journal_updates_unique(
    store: &Store,
    key: &SessionKey,
    expected_attachments: &std::collections::BTreeSet<String>,
) {
    let format = serde_json::to_string(&SourceFormat::OpenCodeSqliteV1).unwrap();
    assert_eq!(
        main_64_journal_attachment_targets(store, key),
        *expected_attachments
    );
    let connection = store.lock();
    for (kind, expected, identities) in [
        ("tool_result", 82, 42),
        ("tool_call", 40, 40),
        ("user_message", 1, 1),
        ("assistant_message", 0, 0),
        ("response_cycle", 0, 0),
    ] {
        let counts: (usize, usize, usize, usize) = connection
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT json_array(
                 p.environment_key,p.agent,p.session_id,p.source_format,
                  p.project_cwd,p.session_incarnation,p.event_kind,p.native_identity)),
                COUNT(DISTINCT json_array(p.native_identity,p.native_revision)),
               COALESCE(SUM(p.native_identity IS NOT NULL AND p.native_revision IS NOT NULL
                 AND p.native_position_json IS NOT NULL AND p.target_check_id IS NULL
                 AND p.source_format=?5 AND p.project_cwd IS s.cwd
                 AND p.session_incarnation=s.incarnation),0)
             FROM native_tool_result_publication p JOIN session s
               USING(environment_key,agent,session_id)
             WHERE p.environment_key=?1 AND p.agent=?2 AND p.session_id=?3
               AND p.outcome='update' AND p.event_kind=?4",
                params![key.environment_key, key.agent, key.session_id, kind, format],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(counts, (expected, identities, expected, expected), "{kind}");
    }
    let transitions = connection
        .prepare(
            "SELECT json_extract(payload_json,'$.call_id'),COUNT(*),
             SUM(json_extract(payload_json,'$.finality')=1),
             SUM(json_extract(payload_json,'$.finality')=0)
         FROM native_tool_result_publication
         WHERE environment_key=?1 AND agent=?2 AND session_id=?3
           AND outcome='update' AND event_kind='tool_result'
         GROUP BY native_identity ORDER BY json_extract(payload_json,'$.call_id')",
        )
        .unwrap()
        .query_map(
            params![key.environment_key, key.agent, key.session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (
                        row.get::<_, usize>(1)?,
                        row.get::<_, usize>(2)?,
                        row.get::<_, usize>(3)?,
                    ),
                ))
            },
        )
        .unwrap()
        .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()
        .unwrap();
    assert_eq!(
        transitions,
        (0..42)
            .map(|index| (
                format!("call-{index}"),
                if index < 2 { (1, 1, 0) } else { (2, 1, 1) }
            ))
            .collect()
    );
    let hosts: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM native_tool_result_publication
         WHERE environment_key=?1 AND agent=?2 AND session_id=?3
           AND outcome='update' AND event_kind IN ('session_start','session_idle')",
            params![key.environment_key, key.agent, key.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hosts, 0);
    let total: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM native_tool_result_publication
         WHERE environment_key=?1 AND agent=?2 AND session_id=?3 AND outcome='update'",
            params![key.environment_key, key.agent, key.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total, 82 + 40 + 1);
}

#[test]
fn main_64_journal_partial_publication_rolls_back_and_restarts_with_a_frozen_public_frontier() {
    let (directory, store, path, source, saved, key) = main_64_journal_fixture();
    let paid = preservation::seed(&store);
    let expected_attachments = main_64_journal_attachment_targets(&store, &key);
    assert!(expected_attachments.is_empty());
    let accepted_results = store.native_tool_results_current(&key).unwrap();
    let appended_assistant = "appended-assistant";
    append_main_64_journal_event(
        &source,
        "message.updated.1",
        &json!({"info":{"id":appended_assistant,"sessionID":"root","role":"assistant","parentID":"request"}}),
    );
    append_main_64_journal_event(&source, "message.part.updated.1", &saved["completion"]);
    for index in 2..42 {
        let mut part = saved["completion"].clone();
        part["part"]["id"] = json!(format!("part-{index}"));
        part["part"]["messageID"] = json!(appended_assistant);
        part["part"]["callID"] = json!(format!("call-{index}"));
        part["part"]["state"]["input"]["command"] = json!(format!("run-call-{index}"));
        part["part"]["state"]["status"] = json!("running");
        part["part"]["state"]
            .as_object_mut()
            .unwrap()
            .remove("error");
        append_main_64_journal_event(&source, "message.part.updated.1", &part);
        part["part"]["state"]["status"] = json!("error");
        part["part"]["state"]["error"] = json!("Synthetic failure.");
        append_main_64_journal_event(&source, "message.part.updated.1", &part);
    }
    store.lock().execute_batch("CREATE TRIGGER expand_main_64_journal_output AFTER INSERT ON native_tool_result_publication
        WHEN NEW.outcome='update' BEGIN
          INSERT INTO native_tool_result_publication(outcome,environment_key,agent,session_id,
            source_format,complete,source_generation,claim_fence,project_cwd,session_incarnation,gap_reason,gap_kind)
          VALUES ('coverage_gap',NEW.environment_key,NEW.agent,NEW.session_id,NEW.source_format,
            0,NEW.source_generation,NEW.claim_fence,NEW.project_cwd,NEW.session_incarnation,'test-page-budget','partial');
        END;").unwrap();
    let before_conversion = publication_head(&store);
    let claim = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let page = read_opencode_live_tool_result_page(
        &path,
        "root",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    let frontier = page.snapshot_end;
    let result_baseline = saved["cursor"]["live"]["baseline"].as_u64().unwrap();
    assert_eq!(page.cursor.live_baseline(), Some(result_baseline));
    let assert_frontier = |store: &Store| {
        preservation::assert_publication_frontier(
            store,
            before_conversion,
            ToolResultPosition::OpenCodeEvent {
                sequence: frontier,
                item_index: 0,
            },
            result_baseline,
        )
    };
    let consumed = saved["cursor"]["through_sequence"].as_u64().unwrap();
    assert_eq!(page.cursor.public_baseline(), Some(frontier));
    assert!(
        serde_json::to_value(&page.cursor).unwrap()["through_sequence"]
            .as_u64()
            .unwrap()
            > consumed
    );
    assert!(
        page.results
            .iter()
            .all(|result| result.call_id.as_deref() != Some("call-0"))
    );
    assert!(page.evidence.events.iter().any(|event| matches!(
        &event.fact,
        antiburn_local::analysis::NativeEvidenceFact::Message(
            antiburn_local::analysis::NativeMessageObservation::Public(call)
        ) if call.call_id.as_deref() == Some("call-2")
            && call.record_id == appended_assistant
            && call.eligible
            && !event.live_eligible
    )));
    let receipt = || {
        store.lock().query_row("SELECT json_array(parser_revision,cursor_json,baseline_sequence,observed_generation,pending,claim_fence,lease_expires_at_epoch) FROM opencode_journal_delivery", [], |row| row.get::<_, String>(0)).unwrap()
    };
    let before_receipt = receipt();
    let before_publications = store.native_evidence_publications_after(0, 1000).unwrap();
    store.lock().execute_batch("CREATE TEMP TRIGGER reject_main_64_journal_cursor BEFORE UPDATE OF cursor_json ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic conversion rollback'); END;").unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .is_err()
    );
    assert_eq!(receipt(), before_receipt);
    assert_eq!(
        store.native_evidence_publications_after(0, 1000).unwrap(),
        before_publications
    );
    assert_eq!(
        store.native_tool_results_current(&key).unwrap(),
        accepted_results
    );
    paid.assert_preserved(&store);
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_main_64_journal_cursor")
        .unwrap();
    store
        .publish_opencode_journal_page(&claim, &page, 101)
        .unwrap();
    let (parser, encoded, pending, baseline): (i64, String, bool, u64) = store.lock().query_row(
        "SELECT parser_revision,cursor_json,pending,baseline_sequence FROM opencode_journal_delivery", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    assert_eq!(parser, PARSER_REVISION);
    assert!(
        pending,
        "the first converted page must exceed the publication budget"
    );
    let encoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(encoded["parser_revision"], PARSER_REVISION);
    assert_eq!(encoded["live"]["revision"], 5);
    assert_eq!(encoded["through_sequence"], consumed);
    assert_eq!(encoded["live"]["public_baseline"], frontier);
    assert_eq!(
        encoded["live"]["baseline"],
        saved["cursor"]["live"]["baseline"]
    );
    assert_eq!(
        encoded["live"]["contexts"],
        saved["cursor"]["live"]["contexts"]
    );
    assert!(encoded["live"]["parts"]["part-1"]["state"]["first_terminal"].is_null());
    assert_eq!(encoded["live"]["parts"]["part-1"]["last_sequence"], 5);
    assert_eq!(
        encoded["live"]["parts"]["part-0"]["state"]["first_terminal"],
        saved["cursor"]["live"]["parts"]["part-0"]["state"]["first_terminal"]
    );
    assert_eq!(baseline, 0);
    let partial = store.native_tool_results_current(&key).unwrap();
    assert!(partial.len() > 1 && partial.len() < 42);
    assert!(partial.contains(&accepted_results[0]));
    paid.assert_preserved(&store);
    assert_frontier(&store);
    append_main_64_journal_event(
        &source,
        "message.updated.1",
        &json!({"info":{"id":"late-user","sessionID":"root","role":"user"}}),
    );
    let final_sequence = append_main_64_journal_event(
        &source,
        "message.part.updated.1",
        &json!({"part":{"id":"late-user-text","messageID":"late-user","sessionID":"root","type":"text","text":"Public input after the frozen frontier."}}),
    );
    drop(store);
    let store = std::sync::Arc::new(Store::open(&directory.path().join("app")).unwrap());
    paid.assert_preserved(&store);
    assert_frontier(&store);
    let mut finished = false;
    for now in 102..122 {
        let claim = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
        let cursor = claim.cursor().unwrap().unwrap();
        let loaded_sequence = serde_json::to_value(&cursor).unwrap()["through_sequence"].clone();
        assert_eq!(cursor.public_baseline(), Some(frontier));
        assert_eq!(cursor.live_baseline(), Some(0));
        let page = read_opencode_live_tool_result_page(
            &path,
            "root",
            Some(&cursor),
            claim.baseline_sequence,
            &|| false,
        )
        .unwrap();
        assert_eq!(page.cursor.public_baseline(), Some(frontier));
        store
            .publish_opencode_journal_page(&claim, &page, now)
            .unwrap();
        paid.assert_preserved(&store);
        assert_frontier(&store);
        let current = store.native_tool_results_current(&key).unwrap();
        if page.results.iter().any(|result| {
            !current
                .iter()
                .any(|saved| saved.result.call_id == result.call_id)
        }) {
            let encoded: String = store
                .lock()
                .query_row(
                    "SELECT cursor_json FROM opencode_journal_delivery",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&encoded).unwrap()["through_sequence"],
                loaded_sequence
            );
        }
        if !store
            .lock()
            .query_row("SELECT pending FROM opencode_journal_delivery", [], |row| {
                row.get::<_, bool>(0)
            })
            .unwrap()
        {
            assert_eq!(
                serde_json::to_value(&page.cursor).unwrap()["through_sequence"],
                final_sequence
            );
            finished = true;
            break;
        }
    }
    assert!(finished, "bounded conversion retries must finish");
    let current = store.native_tool_results_current(&key).unwrap();
    assert_eq!(current.len(), 42);
    assert_eq!(
        current
            .iter()
            .map(|row| row.result.call_id.clone().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        (0..42)
            .map(|index| format!("call-{index}"))
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert!(current.contains(&accepted_results[0]));
    assert_main_64_journal_updates_unique(&store, &key, &expected_attachments);
    assert!(store.lock().query_row("SELECT COUNT(*)=1 AND MIN(live_eligible)=1 FROM native_tool_result_publication
        WHERE event_kind='user_message' AND json_extract(payload_json, '$.message.message_id')='late-user'", [], |row| row.get::<_, bool>(0)).unwrap());
    let head = publication_head(&store);
    let claim = store.claim_next_opencode_journal(122, 60).unwrap().unwrap();
    let idle = read_opencode_live_tool_result_page(
        &path,
        "root",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
    store
        .publish_opencode_journal_page(&claim, &idle, 122)
        .unwrap();
    let metadata = store
        .native_evidence_publications_after(head, 1000)
        .unwrap();
    // The budget trigger leaves test-only coverage metadata after each update.
    assert_eq!(metadata.len(), 1);
    let metadata = &metadata[0];
    let complete = idle.history_complete
        && idle.evidence.history_complete
        && !claim.history_gap
        && claim.main_session;
    assert_eq!(metadata.session, key);
    assert_eq!(
        metadata.event_kind,
        crate::store::native_tool_results::evidence::EvidenceEventKind::ToolResult
    );
    assert_eq!(metadata.complete, complete);
    assert_eq!(
        metadata.outcome,
        if complete {
            ToolResultPublicationOutcome::CoverageRestored
        } else {
            ToolResultPublicationOutcome::CoverageGap
        }
    );
    assert_eq!(
        metadata.gap_reason.as_deref(),
        (!complete).then_some("journal-history-incomplete")
    );
    assert_eq!(metadata.gap_kind, PublicationGapKind::Partial);
    assert!(
        metadata.payload.is_none()
            && metadata.native_identity.is_none()
            && metadata.native_position.is_none()
            && metadata.native_revision.is_none()
            && metadata.live_eligible.is_none()
            && metadata.target.is_none()
    );
    assert_eq!(store.native_tool_results_current(&key).unwrap(), current);
    assert_main_64_journal_updates_unique(&store, &key, &expected_attachments);
    let head = publication_head(&store);
    let claim = store.claim_next_opencode_journal(123, 60).unwrap().unwrap();
    let idle = read_opencode_live_tool_result_page(
        &path,
        "root",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
    store
        .publish_opencode_journal_page(&claim, &idle, 123)
        .unwrap();
    assert_eq!(publication_head(&store), head);
    assert_frontier(&store);
    paid.assert_paid_fences(&store, false);
}

#[test]
fn malformed_main_parser_64_journal_2_blocks_without_replaying_saved_results() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let (claim, first) = next_live_page(&store, &source, 100);
    store
        .publish_opencode_journal_page(&claim, &first, 100)
        .unwrap();
    let current = store.native_tool_results_current(&claim.key).unwrap();
    let head = publication_head(&store);
    let baseline = first.cursor.live_baseline();
    let mut cursor = serde_json::to_value(&first.cursor).unwrap();
    cursor["parser_revision"] = json!(64);
    cursor["live"]["revision"] = json!(2);
    cursor["live"].as_object_mut().unwrap().remove("public");
    cursor["live"]
        .as_object_mut()
        .unwrap()
        .remove("public_baseline");
    let saved = cursor.to_string();
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = 64, cursor_json = ?1,
             pending = 1, next_attempt_at_epoch = 101",
            [&saved],
        )
        .unwrap();
    drop(store);

    let store = Store::open(state.path()).unwrap();
    let claim = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(claim.cursor_json.as_deref(), Some(saved.as_str()));
    assert_eq!(claim.baseline_sequence, baseline);
    assert!(claim.requires_full_read);
    assert!(claim.cursor().is_err());
    assert!(
        store
            .fail_opencode_journal(&claim, JournalFailure::InvalidCursor, 101, 101)
            .unwrap()
    );
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap(),
        current
    );
    let publications = store
        .native_tool_result_publications_after(head, 100)
        .unwrap();
    assert_eq!(publications.len(), 1);
    assert_eq!(
        publications[0].outcome,
        crate::store::native_tool_results::ToolResultPublicationOutcome::CoverageGap
    );
    assert_eq!(
        publications[0].gap_reason.as_deref(),
        Some("journal-cursor-invalid")
    );
    let receipt: (i64, String, bool, bool, String, Option<u64>) = store
        .lock()
        .query_row(
            "SELECT parser_revision, cursor_json, blocked, pending, last_error, baseline_sequence
           FROM opencode_journal_delivery",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        receipt,
        (
            64,
            saved.clone(),
            true,
            false,
            "journal-cursor-invalid".into(),
            baseline
        )
    );
    source.result(1);
    source.index(&store, "/synthetic");
    assert!(
        store
            .claim_next_opencode_journal(102, 60)
            .unwrap()
            .is_none()
    );
    drop(store);

    let store = Store::open(state.path()).unwrap();
    assert!(
        store
            .claim_next_opencode_journal(200, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap(),
        current
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT cursor_json FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        saved
    );
}

#[test]
fn compatible_parser_receipts_keep_journal_5_progress_baselines_and_pending_calls() {
    for parser_revision in [63, 64, 65, 66] {
        let state = tempfile::tempdir().unwrap();
        let store = Store::open(state.path()).unwrap();
        SyntheticJournal::enable_live(&store, "/synthetic");
        let mut source = SyntheticJournal::new(16);
        source.result(0);
        source.start_result(1);
        let (claim, first) = next_live_page(&store, &source, 100);
        assert_eq!(first.results.len(), 1);
        store
            .publish_opencode_journal_page(&claim, &first, 100)
            .unwrap();
        let current = store.native_tool_results_current(&claim.key).unwrap();
        let head = publication_head(&store);
        let mut old = serde_json::to_value(&first.cursor).unwrap();
        old["parser_revision"] = json!(parser_revision);
        store
            .lock()
            .execute(
                "UPDATE opencode_journal_delivery SET parser_revision = ?1, cursor_json = ?2,
                 pending = 1, next_attempt_at_epoch = 101",
                rusqlite::params![parser_revision, old.to_string()],
            )
            .unwrap();
        drop(store);

        let store = Store::open(state.path()).unwrap();
        let (claim, idle) = next_live_page(&store, &source, 101);
        let restored = serde_json::to_value(claim.cursor().unwrap().unwrap()).unwrap();
        assert_eq!(restored, old);
        assert_eq!(claim.baseline_sequence, first.cursor.live_baseline());
        assert!(claim.requires_full_read);
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT parser_revision FROM opencode_journal_delivery",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            parser_revision
        );
        assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
        assert_eq!(idle.live_input_events, 0);
        let saved = serde_json::to_value(&idle.cursor).unwrap();
        assert_eq!(saved["through_sequence"], old["through_sequence"]);
        assert_eq!(saved["live"], old["live"]);
        assert_eq!(saved["parser_revision"], PARSER_REVISION);
        store
            .publish_opencode_journal_page(&claim, &idle, 101)
            .unwrap();
        assert_eq!(publication_head(&store), head);
        assert_eq!(
            store.native_tool_results_current(&claim.key).unwrap(),
            current
        );

        source.result(1);
        let (claim, completed) = next_live_page(&store, &source, 102);
        assert_eq!(completed.results.len(), 1);
        assert_eq!(completed.results[0].call_id.as_deref(), Some("call-1"));
        assert_eq!(
            completed.cursor.live_baseline(),
            first.cursor.live_baseline()
        );
        assert_eq!(
            completed.cursor.public_baseline(),
            first.cursor.public_baseline()
        );
        store
            .publish_opencode_journal_page(&claim, &completed, 102)
            .unwrap();
        assert_eq!(
            store.native_tool_results_current(&claim.key).unwrap().len(),
            2
        );
        let (_, idle) = next_live_page(&store, &source, 103);
        assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
    }
}

#[test]
fn named_parser_62_and_journal_4_predecessor_keeps_progress_until_page_commit() {
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
    let baseline = page.cursor.live_baseline();
    let mut old = serde_json::to_value(&page.cursor).unwrap();
    old["parser_revision"] = json!(62);
    old["live"]["revision"] = json!(4);
    store
        .lock()
        .execute(
            "UPDATE opencode_journal_delivery SET parser_revision = 62, cursor_json = ?1,
        pending = 1, next_attempt_at_epoch = 101",
            [old.to_string()],
        )
        .unwrap();
    let (claim, page) = next_live_page(&store, &source, 101);
    assert!(claim.cursor().unwrap().is_some());
    assert_eq!(claim.baseline_sequence, baseline);
    assert!(claim.requires_full_read);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT parser_revision FROM opencode_journal_delivery",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        62
    );
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap()
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT parser_revision FROM opencode_journal_delivery",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        PARSER_REVISION
    );
    assert_eq!(publication_head(&store), head);
    assert_eq!(page.cursor.live_baseline(), baseline);
    assert_eq!(page.cursor.public_baseline(), Some(page.snapshot_end));
}

#[test]
fn unknown_parser_cannot_replay_a_removal_over_a_saved_later_correction() {
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
    assert!(
        store
            .claim_next_opencode_journal(102, 60)
            .unwrap()
            .is_none()
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
    use crate::store::native_tool_results::ToolResultPublicationOutcome;
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
    let expected_cursor = serde_json::to_string(&page.cursor).unwrap();
    let mut now = 100;
    let mut finished = false;
    for _ in 0..3 {
        let (cursor, pending): (Option<String>, bool) = store
            .lock()
            .query_row(
                "SELECT cursor_json,pending FROM opencode_journal_delivery",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        if cursor.as_deref() == Some(expected_cursor.as_str()) {
            assert!(!pending);
            finished = true;
            break;
        }
        assert!(cursor.is_some() && pending);
        let partial = store.native_tool_results_current(&claim.key).unwrap();
        assert!(!partial.is_empty() && partial.len() < page.results.len());
        now += 1;
        let (retry, retry_page) = next_live_page(&store, &source, now);
        let saved = retry.cursor().unwrap().unwrap();
        assert_eq!(saved.live_baseline(), page.cursor.live_baseline());
        assert_eq!(saved.public_baseline(), page.cursor.public_baseline());
        assert_eq!(retry_page.cursor, page.cursor);
        assert_eq!(retry_page.results, page.results);
        assert!(
            store
                .publish_opencode_journal_page(&retry, &retry_page, now)
                .unwrap()
        );
    }
    assert!(
        finished,
        "the cold result window must finish bounded commits"
    );
    let publications = store.native_tool_result_publications_after(0, 100).unwrap();
    let published: Vec<_> = publications
        .iter()
        .filter(|event| event.payload.is_some())
        .collect();
    assert_eq!(published.len(), 40);
    assert!(published.iter().all(
        |event| event.outcome == ToolResultPublicationOutcome::Update
            && event.live_eligible == Some(true)
    ));
    assert_eq!(
        store.native_tool_results_current(&claim.key).unwrap().len(),
        40
    );
    assert_eq!(
        published
            .iter()
            .map(|event| event.payload.as_ref().unwrap().result_id.clone().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        (260..300)
            .map(|index| format!("part-{index}"))
            .collect::<std::collections::BTreeSet<_>>()
    );
    let coverage: Vec<_> = publications
        .iter()
        .filter(|event| event.payload.is_none())
        .collect();
    assert_eq!(coverage.len(), 1);
    assert_eq!(
        coverage[0].outcome,
        ToolResultPublicationOutcome::CoverageGap
    );
    assert_eq!(coverage[0].gap_kind, PublicationGapKind::Partial);
    assert_eq!(
        coverage[0].gap_reason.as_deref(),
        Some("journal-history-incomplete")
    );
    assert_eq!(coverage[0].live_eligible, None);
    let frozen = published[30].payload.as_ref().unwrap().context.clone();
    let head = publication_head(&store);
    drop(store);
    let store = Store::open(state.path()).unwrap();
    source.result(300);
    source.result(290);
    source.result(0);
    source.index(&store, "/synthetic");
    now += 1;
    let next = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
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
            .publish_opencode_journal_page(&next, &page, now)
            .unwrap()
    );
    assert!(
        !store
            .publish_opencode_journal_page(&next, &page, now)
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
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_journal_cursor BEFORE UPDATE ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic rollback'); END;",
        )
        .unwrap();
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
    let mut now = 100;
    let expected_cursor = serde_json::to_string(&page.cursor).unwrap();
    let mut finished = false;
    for _ in 0..32 {
        let cursor: Option<String> = store
            .lock()
            .query_row(
                "SELECT cursor_json FROM opencode_journal_delivery",
                [],
                |row| row.get(0),
            )
            .unwrap();
        if cursor.as_deref() == Some(expected_cursor.as_str()) {
            finished = true;
            break;
        }
        assert!(
            cursor.is_none(),
            "partial output must not advance the receipt"
        );
        now += 1;
        let continuation = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
        assert!(
            store
                .publish_opencode_journal_page(&continuation, &page, now)
                .unwrap()
        );
    }
    assert!(finished, "bounded native output must finish the page");
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
    now += 1;
    let next = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
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
            .publish_opencode_journal_page(&next, &page, now)
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
use crate::store::native_tool_results::publications::reconcile_with_policy;
