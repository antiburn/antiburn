use std::collections::HashMap;

use antiburn_local::analysis::opencode_v2_capture::CaptureBindingReconciliation;

use super::*;

#[path = "opencode_v2_retention_tests.rs"]
mod retention;

fn snapshot(
    store: &Store,
    start: &OpenCodeV2CaptureStart,
    retained: bool,
) -> CaptureReconciliation {
    let records = store.opencode_v2_live_candidates(100, 10).unwrap();
    CaptureReconciliation {
        baseline: HashMap::from([(start.key.session_id.clone(), start.baseline_sequence)]),
        bindings: store
            .opencode_v2_reconciliation_targets(&records)
            .unwrap()
            .into_iter()
            .map(|observation| CaptureBindingReconciliation {
                observation,
                retained,
            })
            .collect(),
        invalid_scopes: Default::default(),
    }
}

fn publication_count(store: &Store) -> usize {
    store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn stable_call_identity_deduplicates_new_event_ids_and_keeps_first_terminal_context() {
    let (_directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    let mut first = result(5, false);
    first.result_id = Some("first-event".into());
    assert!(
        store
            .advance_opencode_v2_capture(&claim, 5, Some(&first), 101, 60)
            .unwrap()
    );
    let original = store
        .native_tool_results_current(&start.key)
        .unwrap()
        .remove(0);
    let count = publication_count(&store);
    let mut duplicate = first.clone();
    duplicate.result_id = Some("later-event".into());
    duplicate.position = ToolResultPosition::OpenCodeV2Event {
        sequence: 6,
        item_index: 0,
    };
    duplicate.context.as_mut().unwrap().complete_through_event = true;
    duplicate.context.as_mut().unwrap().result_position = duplicate.position;
    assert!(
        store
            .advance_opencode_v2_capture(&claim, 6, Some(&duplicate), 102, 60)
            .unwrap()
    );
    assert_eq!(publication_count(&store), count);
    assert_eq!(
        store.native_tool_results_current(&start.key).unwrap(),
        vec![original]
    );
    let last: i64 = store
        .lock()
        .query_row("SELECT last_sequence FROM opencode_v2_capture", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(last, 6);
}

#[test]
fn same_revision_conflicting_terminal_does_not_advance_or_publish() {
    let (_directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    let first = result(5, false);
    assert!(
        store
            .advance_opencode_v2_capture(&claim, 5, Some(&first), 101, 60)
            .unwrap()
    );
    let count = publication_count(&store);
    let mut conflict = first.clone();
    conflict.position = ToolResultPosition::OpenCodeV2Event {
        sequence: 6,
        item_index: 0,
    };
    conflict.result_id = Some("conflicting-event".into());
    conflict.content = Some("different output".into());
    assert!(
        !store
            .advance_opencode_v2_capture(&claim, 6, Some(&conflict), 102, 60)
            .unwrap()
    );
    assert_eq!(publication_count(&store), count);
    assert_eq!(
        store.native_tool_results_current(&start.key).unwrap()[0].result,
        first
    );
    let last: i64 = store
        .lock()
        .query_row("SELECT last_sequence FROM opencode_v2_capture", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(last, 5);
}

#[test]
fn reconnect_retires_exact_changed_bindings_and_never_rewrites_frozen_publications() {
    let (_directory, store, _journal, mut start) = setup();
    let claim = started(&store, &start);
    for sequence in [5, 6] {
        assert!(
            store
                .advance_opencode_v2_capture(
                    &claim,
                    sequence,
                    Some(&result(sequence, false)),
                    101,
                    60
                )
                .unwrap()
        );
    }
    let before = store.native_tool_result_publications_after(0, 100).unwrap();
    let old = store.native_tool_results_current(&start.key).unwrap();
    store
        .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
        .unwrap();
    start.baseline_sequence = Some(10);
    let mut snapshot = snapshot(&store, &start, true);
    snapshot.bindings[0].retained = false;
    let retired_id = snapshot.bindings[0].observation.record_id.clone();
    let next = store
        .start_opencode_v2_capture_epoch_reconciled(&[start.clone()], &snapshot, "epoch-b", 103, 60)
        .unwrap()
        .unwrap()
        .remove(0);
    let current = store.native_tool_results_current(&start.key).unwrap();
    assert_eq!(current.len(), 2);
    for row in &current {
        let prior = old
            .iter()
            .find(|old| old.native_identity == row.native_identity)
            .unwrap();
        assert_eq!(row.removed, row.result.record_id == retired_id);
        assert_eq!(row.result.context, prior.result.context);
        assert_eq!(row.native_position, prior.native_position);
        assert_eq!(row.result.content, prior.result.content);
    }
    let after = store.native_tool_result_publications_after(0, 100).unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    let removal = after
        .iter()
        .find(|event| {
            event.outcome
                == crate::store::native_tool_results::ToolResultPublicationOutcome::Removal
        })
        .unwrap();
    assert_eq!(removal.live_eligible, Some(false));
    let mut replay = snapshot.bindings[0].observation.clone();
    replay.position = ToolResultPosition::OpenCodeV2Event {
        sequence: 11,
        item_index: 0,
    };
    replay.result_id = Some("new-event-after-cache-eviction".into());
    replay.content = Some("reappeared old call".into());
    let count = publication_count(&store);
    assert!(
        store
            .advance_opencode_v2_capture(&next, 11, Some(&replay), 104, 60)
            .unwrap()
    );
    assert_eq!(publication_count(&store), count);
    assert_eq!(
        store
            .native_tool_results_current(&start.key)
            .unwrap()
            .iter()
            .filter(|row| row.removed)
            .count(),
        1
    );
}

#[test]
fn omitted_or_changed_snapshot_targets_roll_back_the_whole_new_epoch() {
    for mutation in ["omit", "change", "duplicate"] {
        let (_directory, store, _journal, mut start) = setup();
        let claim = started(&store, &start);
        assert!(
            store
                .advance_opencode_v2_capture(&claim, 5, Some(&result(5, false)), 101, 60)
                .unwrap()
        );
        store
            .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
            .unwrap();
        start.baseline_sequence = Some(10);
        let mut snapshot = snapshot(&store, &start, false);
        match mutation {
            "omit" => snapshot.bindings.clear(),
            "change" => {
                snapshot.bindings[0].observation.content = Some("not the saved payload".into())
            }
            "duplicate" => snapshot.bindings.push(snapshot.bindings[0].clone()),
            _ => unreachable!(),
        }
        let count = publication_count(&store);
        assert!(
            store
                .start_opencode_v2_capture_epoch_reconciled(&[start], &snapshot, "epoch-b", 103, 60)
                .unwrap()
                .is_none(),
            "{mutation}"
        );
        assert_eq!(publication_count(&store), count, "{mutation}");
        let epoch: String = store
            .lock()
            .query_row("SELECT capture_epoch FROM opencode_v2_capture", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(epoch, "epoch-a", "{mutation}");
    }
}

#[test]
fn retirement_publication_failure_rolls_back_new_lease_and_current_state() {
    let (_directory, store, _journal, mut start) = setup();
    let claim = started(&store, &start);
    assert!(
        store
            .advance_opencode_v2_capture(&claim, 5, Some(&result(5, false)), 101, 60)
            .unwrap()
    );
    store
        .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
        .unwrap();
    start.baseline_sequence = Some(10);
    let snapshot = snapshot(&store, &start, false);
    let before = store.native_tool_results_current(&start.key).unwrap();
    let count = publication_count(&store);
    store.lock().execute_batch(
        "CREATE TRIGGER reject_v2_retirement BEFORE INSERT ON native_tool_result_publication
           WHEN NEW.outcome = 'removal' BEGIN SELECT RAISE(ABORT, 'synthetic retirement failure'); END;",
    ).unwrap();
    assert!(
        store
            .start_opencode_v2_capture_epoch_reconciled(
                &[start.clone()],
                &snapshot,
                "epoch-b",
                103,
                60
            )
            .is_err()
    );
    assert_eq!(publication_count(&store), count);
    assert_eq!(
        store.native_tool_results_current(&start.key).unwrap(),
        before
    );
    let lease: (String, Option<i64>) = store
        .lock()
        .query_row(
            "SELECT capture_epoch, lease_expires_at_epoch FROM opencode_v2_capture",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(lease, ("epoch-a".into(), None));
}

#[test]
fn explicit_scope_loss_retires_old_bindings_but_stale_claims_cannot_retire_new_work() {
    for reason in ["session-moved", "session-deleted", "source-scope-changed"] {
        let (_directory, store, _journal, start) = setup();
        let claim = started(&store, &start);
        assert!(
            store
                .advance_opencode_v2_capture(&claim, 5, Some(&result(5, false)), 101, 60)
                .unwrap()
        );
        let mut stale = claim.clone();
        stale.claim_fence -= 1;
        assert_eq!(
            store
                .mark_opencode_v2_capture_gap(&[stale], reason, 102)
                .unwrap(),
            0
        );
        assert!(!store.native_tool_results_current(&start.key).unwrap()[0].removed);
        assert_eq!(
            store
                .mark_opencode_v2_capture_gap(&[claim], reason, 103)
                .unwrap(),
            1
        );
        assert!(store.native_tool_results_current(&start.key).unwrap()[0].removed);
    }
}

#[test]
fn reconciled_start_keeps_scope_incarnation_generation_and_enabled_state_fences() {
    for mutation in ["scope", "incarnation", "generation", "disabled"] {
        let (_directory, store, _journal, mut start) = setup();
        let claim = started(&store, &start);
        assert!(
            store
                .advance_opencode_v2_capture(&claim, 5, Some(&result(5, false)), 101, 60)
                .unwrap()
        );
        store
            .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
            .unwrap();
        start.baseline_sequence = Some(10);
        let snapshot = snapshot(&store, &start, false);
        let sql = match mutation {
            "scope" => "UPDATE session SET cwd = '/moved'",
            "incarnation" => "UPDATE session SET incarnation = incarnation + 1",
            "generation" => "UPDATE session SET source_generation = source_generation + 1",
            "disabled" => "UPDATE burn_check_activation SET enabled = 0",
            _ => unreachable!(),
        };
        store.lock().execute(sql, []).unwrap();
        let count = publication_count(&store);
        assert!(
            store
                .start_opencode_v2_capture_epoch_reconciled(&[start], &snapshot, "epoch-b", 103, 60)
                .unwrap()
                .is_none(),
            "{mutation}"
        );
        assert_eq!(publication_count(&store), count, "{mutation}");
    }
}

#[test]
fn retired_identity_survives_store_restart_and_cannot_create_another_attempt() {
    let (directory, store, _journal, mut start) = setup();
    let claim = started(&store, &start);
    let first = result(5, false);
    assert!(
        store
            .advance_opencode_v2_capture(&claim, 5, Some(&first), 101, 60)
            .unwrap()
    );
    store
        .mark_opencode_v2_capture_gap(&[claim], "session-deleted", 102)
        .unwrap();
    let path = crate::store::database_path(directory.path());
    store
        .lock()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let before = store.native_tool_result_publications_after(0, 100).unwrap();
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    start.baseline_sequence = Some(10);
    let snapshot = snapshot(&store, &start, true);
    assert!(snapshot.bindings.is_empty());
    let next = store
        .start_opencode_v2_capture_epoch_reconciled(&[start.clone()], &snapshot, "epoch-b", 103, 60)
        .unwrap()
        .unwrap()
        .remove(0);
    let mut replay = first;
    replay.position = ToolResultPosition::OpenCodeV2Event {
        sequence: 11,
        item_index: 0,
    };
    replay.result_id = Some("different-event".into());
    let count = publication_count(&store);
    assert!(
        store
            .advance_opencode_v2_capture(&next, 11, Some(&replay), 104, 60)
            .unwrap()
    );
    assert_eq!(publication_count(&store), count);
    assert!(store.native_tool_results_current(&start.key).unwrap()[0].removed);
    assert_eq!(
        &store.native_tool_result_publications_after(0, 100).unwrap()[..before.len()],
        before.as_slice()
    );
}

#[test]
fn scope_retirement_does_not_remove_matching_ids_from_another_session() {
    let (_directory, store, journal, start) = setup();
    let claim = started(&store, &start);
    let mut record = journal.record("/synthetic");
    record.key.session_id = "other-session".into();
    record.source_label = "opencode:other-session:open_code_v2_sqlite".into();
    record.activity_cursor = super::test_admission_cursor(&start.admitted_source, "other-session");
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let mut other = start.clone();
    other.key = record.key;
    other.source_label = record.source_label;
    (other.incarnation, other.source_generation) = store
        .lock()
        .query_row(
            "SELECT incarnation, source_generation FROM session WHERE session_id = 'other-session'",
            [],
            |row| Ok((Incarnation(row.get(0)?), row.get(1)?)),
        )
        .unwrap();
    let other_claim = store
        .start_opencode_v2_capture_epoch(&[other.clone()], "epoch-other", 100, 60)
        .unwrap()
        .unwrap()
        .remove(0);
    let first = result(5, false);
    let mut same_ids = first.clone();
    same_ids.session_id = "other-session".into();
    assert!(
        store
            .advance_opencode_v2_capture(&claim, 5, Some(&first), 101, 60)
            .unwrap()
    );
    assert!(
        store
            .advance_opencode_v2_capture(&other_claim, 5, Some(&same_ids), 101, 60)
            .unwrap()
    );
    assert_eq!(
        store
            .mark_opencode_v2_capture_gap(&[claim], "session-deleted", 102)
            .unwrap(),
        1
    );
    assert!(store.native_tool_results_current(&start.key).unwrap()[0].removed);
    assert!(!store.native_tool_results_current(&other.key).unwrap()[0].removed);
    assert!(
        store
            .renew_opencode_v2_capture_epoch(&[other_claim], 103, 60)
            .unwrap()
    );
}

#[test]
fn source_correction_revert_and_move_retire_exact_persisted_bindings() {
    use antiburn_local::analysis::opencode_v2_capture::read_capture_reconciliation_for_scope;

    for mutation in ["correction", "revert", "move"] {
        let (directory, store, _journal, mut start) = setup();
        let claim = started(&store, &start);
        for sequence in [5, 6] {
            assert!(
                store
                    .advance_opencode_v2_capture(
                        &claim,
                        sequence,
                        Some(&result(sequence, false)),
                        101,
                        60
                    )
                    .unwrap()
            );
        }
        store
            .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
            .unwrap();
        let path = directory.path().join("source.sqlite");
        let source = rusqlite::Connection::open(&path).unwrap();
        source.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT, fork_session_id TEXT,
               directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT,
               seq INTEGER, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE UNIQUE INDEX native_message_seq ON session_message(session_id, seq);
             CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY NOT NULL, seq INTEGER NOT NULL);
             INSERT INTO session_v2 VALUES ('native-session', NULL, NULL, '/synthetic', NULL, 1, 1);
             INSERT INTO event_sequence VALUES ('native-session', 20);",
        ).unwrap();
        for sequence in [5, 6] {
            source.execute(
                "INSERT INTO session_message VALUES (?1, 'native-session', 'assistant', ?2, 1, 1, ?3)",
                params![format!("event-{sequence}"), sequence, json!({"content":[{
                    "type":"tool","id":format!("call-{sequence}"),"name":"bash",
                    "state":{"status":"completed","input":{"command":"true"},
                        "content":[{"type":"text","text":"ok"}]}}]}).to_string()],
            ).unwrap();
        }
        let sql = match mutation {
            "correction" => {
                "UPDATE session_message SET data = '{\"content\":[]}' WHERE id = 'event-6'"
            }
            "revert" => "DELETE FROM session_message WHERE seq >= 6",
            "move" => "UPDATE session_message SET session_id = 'other' WHERE id = 'event-6'",
            _ => unreachable!(),
        };
        source.execute(sql, []).unwrap();
        let records = store.opencode_v2_live_candidates(100, 10).unwrap();
        let targets = store.opencode_v2_reconciliation_targets(&records).unwrap();
        let snapshot = read_capture_reconciliation_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &[(start.key.session_id.clone(), start.cwd.clone())],
            &targets,
            &|| false,
        )
        .unwrap();
        start.baseline_sequence = snapshot.baseline[&start.key.session_id];
        assert!(
            store
                .start_opencode_v2_capture_epoch_reconciled(
                    &[start.clone()],
                    &snapshot,
                    "epoch-b",
                    103,
                    60
                )
                .unwrap()
                .is_some()
        );
        let current = store.native_tool_results_current(&start.key).unwrap();
        assert_eq!(
            current.iter().filter(|row| row.removed).count(),
            1,
            "{mutation}"
        );
        assert!(
            current
                .iter()
                .find(|row| row.result.record_id.as_deref() == Some("event-6"))
                .unwrap()
                .removed,
            "{mutation}"
        );
        assert!(
            !current
                .iter()
                .find(|row| row.result.record_id.as_deref() == Some("event-5"))
                .unwrap()
                .removed,
            "{mutation}"
        );
    }
}
