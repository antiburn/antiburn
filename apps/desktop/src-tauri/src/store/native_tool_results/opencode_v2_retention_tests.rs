use super::*;

fn insert_results(
    store: &Store,
    claim: &OpenCodeV2CaptureClaim,
    count: usize,
    content_bytes: usize,
    legacy: bool,
) -> Vec<i64> {
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    let mut sequences = Vec::new();
    for index in 0..count {
        let mut observation = result(u64::try_from(index).unwrap() + 5, false);
        if content_bytes > 0 {
            observation.content = Some("x".repeat(content_bytes));
        }
        let mut value = serde_json::to_value(&observation).unwrap();
        if legacy {
            value.as_object_mut().unwrap().remove("outcome");
            value.as_object_mut().unwrap().remove("provider_executed");
        }
        let identity = result_identity(&observation).unwrap();
        let position = serde_json::to_string(&observation.position).unwrap();
        let payload = value.to_string();
        let format = serde_json::to_string(&SourceFormat::OpenCodeSqliteV2).unwrap();
        tx.execute(
            "INSERT INTO native_tool_result_publication (
              outcome, environment_key, agent, session_id, source_format, native_identity,
              native_position_json, native_revision, payload_json, complete, source_generation,
              claim_fence, live_eligible, project_cwd, session_incarnation)
             VALUES ('update', ?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 0, ?8, ?9, 1, ?10, ?11)",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                format,
                identity,
                position,
                payload,
                claim.source_generation,
                claim.claim_fence,
                claim.cwd,
                claim.incarnation.0
            ],
        )
        .unwrap();
        let publication = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO native_tool_result_current VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 0, 0, ?8, ?9, ?10)",
            params![claim.key.environment_key, claim.key.agent, claim.key.session_id, format,
                identity, position, payload, publication, claim.source_generation, claim.claim_fence],
        ).unwrap();
        sequences.push(publication);
    }
    tx.commit().unwrap();
    sequences
}

fn raw_payload(store: &Store, publication: i64) -> String {
    store.lock().query_row("SELECT payload_json FROM native_tool_result_publication WHERE publication_sequence = ?1",
        [publication], |row| row.get(0)).unwrap()
}

fn active_count(store: &Store) -> usize {
    store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM native_tool_result_current WHERE is_removed = 0",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn prepare(store: &Store, now: i64) -> OpenCodeV2ReconciliationPreparation {
    let records = store.opencode_v2_live_candidates(now, 10).unwrap();
    store
        .prepare_opencode_v2_reconciliation(&records, now)
        .unwrap()
}

#[test]
fn legacy_missing_optional_fields_reconcile_and_retire_without_rewriting_history() {
    for retained in [true, false] {
        let (_directory, store, _journal, mut start) = setup();
        let claim = started(&store, &start);
        let publication = insert_results(&store, &claim, 1, 0, true)[0];
        let raw = raw_payload(&store, publication);
        assert!(!raw.contains("outcome"));
        assert!(!raw.contains("provider_executed"));
        store
            .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
            .unwrap();
        assert_eq!(
            prepare(&store, 103),
            OpenCodeV2ReconciliationPreparation::Ready
        );
        start.baseline_sequence = Some(10);
        let snapshot = snapshot(&store, &start, retained);
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
        let current = store
            .native_tool_results_current(&start.key)
            .unwrap()
            .remove(0);
        assert_eq!(current.removed, !retained);
        assert_eq!(current.result.context, result(5, false).context);
        assert_eq!(current.native_position, result(5, false).position);
        assert_eq!(raw_payload(&store, publication), raw);
        if retained {
            let current_raw: String = store
                .lock()
                .query_row(
                    "SELECT payload_json FROM native_tool_result_current",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(current_raw, raw);
        }
    }
}

#[test]
fn count_retention_makes_progress_and_preserves_tombstones_and_first_terminal_evidence() {
    let (_directory, store, _journal, mut start) = setup();
    let claim = started(&store, &start);
    let publications = insert_results(&store, &claim, MAX_RECONCILIATION_BINDINGS + 1, 0, false);
    let original = raw_payload(&store, publications[0]);
    store
        .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
        .unwrap();
    assert_eq!(
        prepare(&store, 103),
        OpenCodeV2ReconciliationPreparation::Progress
    );
    assert_eq!(
        active_count(&store),
        MAX_RECONCILIATION_BINDINGS + 1 - MAX_RETIREMENT_RESULTS
    );
    assert_eq!(
        prepare(&store, 104),
        OpenCodeV2ReconciliationPreparation::Ready
    );
    let first = store
        .native_tool_results_current(&start.key)
        .unwrap()
        .into_iter()
        .find(|current| current.result.record_id.as_deref() == Some("event-5"))
        .unwrap();
    assert!(first.removed);
    assert_eq!(first.result, result(5, false));
    assert_eq!(raw_payload(&store, publications[0]), original);
    start.baseline_sequence = Some(10_000);
    let snapshot = snapshot(&store, &start, true);
    let next = store
        .start_opencode_v2_capture_epoch_reconciled(&[start.clone()], &snapshot, "epoch-b", 105, 60)
        .unwrap()
        .unwrap()
        .remove(0);
    let mut duplicate = result(5, false);
    duplicate.position = ToolResultPosition::OpenCodeV2Event {
        sequence: 10_001,
        item_index: 0,
    };
    duplicate.result_id = Some("replayed-evicted-result".into());
    let count = publication_count(&store);
    assert!(
        store
            .advance_opencode_v2_capture(&next, 10_001, Some(&duplicate), 106, 60)
            .unwrap()
    );
    assert_eq!(publication_count(&store), count);
    assert!(
        store
            .advance_opencode_v2_capture(&next, 10_002, Some(&result(10_002, false)), 107, 60)
            .unwrap()
    );
}

#[test]
fn byte_retention_and_scope_retirement_do_not_load_large_saved_payloads() {
    for scope_loss in [false, true] {
        let (_directory, store, _journal, start) = setup();
        let claim = started(&store, &start);
        let publications = insert_results(&store, &claim, 40, 500_000, true);
        let first = raw_payload(&store, publications[0]);
        store
            .mark_opencode_v2_capture_gap(
                std::slice::from_ref(&claim),
                if scope_loss {
                    "session-deleted"
                } else {
                    "capture-lost"
                },
                102,
            )
            .unwrap();
        if scope_loss {
            assert_eq!(
                store
                    .continue_opencode_v2_scope_retirement(&[claim], "session-deleted", 103)
                    .unwrap(),
                OpenCodeV2ReconciliationPreparation::Ready
            );
            assert_eq!(active_count(&store), 0);
        } else {
            assert_eq!(
                prepare(&store, 103),
                OpenCodeV2ReconciliationPreparation::Progress
            );
            assert_eq!(
                prepare(&store, 104),
                OpenCodeV2ReconciliationPreparation::Ready
            );
            let records = store.opencode_v2_live_candidates(104, 10).unwrap();
            assert!(
                store
                    .opencode_v2_reconciliation_targets(&records)
                    .unwrap()
                    .len()
                    < 40
            );
        }
        assert_eq!(raw_payload(&store, publications[0]), first);
        let tombstone_raw: String = store.lock().query_row("SELECT payload_json FROM native_tool_result_current WHERE is_removed = 1 ORDER BY rowid LIMIT 1", [], |row| row.get(0)).unwrap();
        assert_eq!(tombstone_raw, first);
        let quality: usize = store.lock().query_row("SELECT COUNT(*) FROM native_tool_result_publication WHERE outcome = 'removal' AND complete = 0 AND live_eligible = 0 AND payload_json IS NOT NULL", [], |row| row.get(0)).unwrap();
        assert!(quality > 0);
    }
}

#[test]
fn interrupted_scope_retirement_resumes_from_the_receipt_after_restart() {
    let (directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    insert_results(&store, &claim, MAX_RETIREMENT_RESULTS * 2 + 7, 0, true);
    store
        .mark_opencode_v2_capture_gap(&[claim], "session-moved", 102)
        .unwrap();
    assert_eq!(active_count(&store), MAX_RETIREMENT_RESULTS + 7);
    let path = crate::store::database_path(directory.path());
    store
        .lock()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.recover_opencode_v2_capture_claims(103).unwrap(), 0);
    assert_eq!(
        prepare(&store, 104),
        OpenCodeV2ReconciliationPreparation::Progress
    );
    assert_eq!(active_count(&store), 7);
    assert_eq!(
        prepare(&store, 105),
        OpenCodeV2ReconciliationPreparation::Progress
    );
    assert_eq!(active_count(&store), 0);
    assert_eq!(
        prepare(&store, 106),
        OpenCodeV2ReconciliationPreparation::Ready
    );
    let reason: Option<String> = store
        .lock()
        .query_row(
            "SELECT retirement_reason FROM opencode_v2_capture",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, None);
    assert!(
        store
            .native_tool_results_current(&start.key)
            .unwrap()
            .iter()
            .all(|row| row.removed)
    );
}

#[test]
fn retention_and_continuation_keep_active_lease_and_epoch_fences() {
    let (_directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    insert_results(&store, &claim, MAX_RECONCILIATION_BINDINGS + 1, 0, false);
    let count = publication_count(&store);
    assert_eq!(
        prepare(&store, 101),
        OpenCodeV2ReconciliationPreparation::Fenced
    );
    assert_eq!(publication_count(&store), count);
    store
        .mark_opencode_v2_capture_gap(std::slice::from_ref(&claim), "session-deleted", 102)
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE opencode_v2_capture SET claim_fence = claim_fence + 1",
            [],
        )
        .unwrap();
    let count = publication_count(&store);
    assert_eq!(
        store
            .continue_opencode_v2_scope_retirement(&[claim], "session-deleted", 103)
            .unwrap(),
        OpenCodeV2ReconciliationPreparation::Fenced
    );
    assert_eq!(publication_count(&store), count);
}

#[test]
fn retention_failure_rolls_back_tombstones_publications_and_scope_marker() {
    let (_directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    insert_results(&store, &claim, MAX_RECONCILIATION_BINDINGS + 1, 0, false);
    store
        .mark_opencode_v2_capture_gap(&[claim], "capture-lost", 102)
        .unwrap();
    let count = publication_count(&store);
    store.lock().execute_batch("CREATE TRIGGER reject_retention BEFORE INSERT ON native_tool_result_publication
        WHEN NEW.gap_reason = 'v2-current-retention' BEGIN SELECT RAISE(ABORT, 'synthetic retention failure'); END;").unwrap();
    let records = store.opencode_v2_live_candidates(103, 10).unwrap();
    assert!(
        store
            .prepare_opencode_v2_reconciliation(&records, 103)
            .is_err()
    );
    assert_eq!(active_count(&store), MAX_RECONCILIATION_BINDINGS + 1);
    assert_eq!(publication_count(&store), count);
}
