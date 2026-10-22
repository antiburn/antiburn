use super::*;

fn gap(store: &Store, sequence: u64, reset: bool) -> SourceBatch {
    store
        .lock()
        .execute(
            "INSERT INTO native_tool_result_publication
          (publication_sequence, outcome, environment_key, agent, session_id,
           source_format, complete, source_generation, claim_fence)
         VALUES (?1, 'coverage_gap', 'native', 'fixture', 'branch-a', 'fixture', 0, 1, 1)",
            [sequence],
        )
        .unwrap();
    let cursor = PublicationCursor(sequence);
    let branch = BranchId("branch-a".into());
    let reason = "synthetic-capture-gap".into();
    SourceBatch {
        events: vec![if reset {
            SourceEvent::SegmentReset {
                cursor,
                branch,
                reason,
            }
        } else {
            SourceEvent::Gap {
                cursor,
                branch,
                reason,
            }
        }],
        next: cursor,
        has_more: false,
    }
}

fn saved_results(store: &Store) -> (String, String, String, String) {
    let connection = store.lock();
    let rows = |table: &str| {
        let filter = if table == "burn_check_assessment_history" {
            " WHERE outcome = 'assessed'"
        } else {
            ""
        };
        connection.query_row(
            &format!("SELECT json_group_array(json(row)) FROM (
              SELECT json_array({}) AS row FROM {table}{filter} ORDER BY row)", match table {
                "burn_check_current_status" => "branch_key, status, assessment_id, position, evidence_valid, updated_at_epoch",
                "burn_check_assessment_history" => "assessment_id, outcome, answer_json, actions_json, evidence_valid, accepted_at_epoch",
                "burn_check_acceptance_watermark" => "branch_key, generation, position",
                "burn_check_cooldown" => "branch_key, generation, cooldown_until_epoch",
                _ => unreachable!(),
            }),
            [],
            |row| row.get::<_, String>(0),
        ).unwrap()
    };
    (
        rows("burn_check_current_status"),
        rows("burn_check_assessment_history"),
        rows("burn_check_acceptance_watermark"),
        rows("burn_check_cooldown"),
    )
}

#[test]
fn source_gaps_preserve_accepted_status_and_only_segment_resets_cancel_pending_work() {
    for reset in [false, true] {
        for status in ["success", "failure"] {
            for state in ["pending", "claimed", "dispatching", "answered"] {
                let directory = tempfile::tempdir().unwrap();
                let source = CHECK
                    .replace(
                        "  tool_result:\n",
                        "  tool_result:\n  cooldown: 0s\n  window: {size: 2, min: 2}\n",
                    )
                    .replace("result: failure", &format!("result: {status}"));
                let (store, adapter, active) = fixture_with_definition(
                    Arc::new(Store::open(directory.path()).unwrap()),
                    &source,
                );
                for (sequence, id, branch) in [(1, "a", "branch-a"), (3, "u", "branch-b")] {
                    CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
                        .apply_source_page(&active, &event(&store, sequence, id, branch, 1))
                        .unwrap();
                    let candidate = apply(
                        &adapter,
                        &active,
                        &event(&store, sequence + 1, &format!("{id}-next"), branch, 1),
                    );
                    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
                    assert_eq!(
                        adapter.accept(&assessment).unwrap(),
                        AcceptanceOutcome::Accepted
                    );
                }
                let pending = apply(
                    &adapter,
                    &active,
                    &event(&store, 5, "pending", "branch-a", 1),
                );
                match state {
                    "pending" => {}
                    "claimed" => {
                        claim(&adapter, &pending);
                    }
                    "dispatching" => {
                        adapter
                            .record_dispatch_intent(
                                &claim(&adapter, &pending),
                                100_000,
                                Duration::ZERO,
                                1,
                            )
                            .unwrap();
                    }
                    "answered" => {
                        answer(&adapter, &active, claim(&adapter, &pending), 0.99);
                    }
                    _ => unreachable!(),
                }
                let saved = saved_results(&store);
                let before = adapter.load_source_progress(&active).unwrap();
                let attempts: u64 = store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
                engine
                    .apply_source_page(&active, &gap(&store, 6, reset))
                    .unwrap();
                assert_eq!(
                    saved_results(&store),
                    saved,
                    "reset={reset}, status={status}, state={state}"
                );
                let progress = adapter.load_source_progress(&active).unwrap();
                let affected = &progress.selection.branches[&BranchId("branch-a".into())];
                assert!(!affected.coverage_complete);
                assert_eq!(affected.results.is_empty(), reset);
                assert_eq!(
                    progress.selection.branches[&BranchId("branch-b".into())],
                    before.selection.branches[&BranchId("branch-b".into())]
                );
                assert_eq!(progress.publication_cursor, PublicationCursor(6));
                assert_eq!(
                    store
                        .lock()
                        .query_row(
                            "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                            [],
                            |row| row.get::<_, u64>(0)
                        )
                        .unwrap(),
                    attempts
                );
                let expected = if reset { "stale" } else { state };
                let pending_state: String = store
                    .lock()
                    .query_row(
                        "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
                        [hex(&pending.fingerprint)],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(pending_state, expected);
                drop(engine);
                drop(adapter);
                drop(store);

                let restarted = Arc::new(Store::open(directory.path()).unwrap());
                let adapter = StoreCheckStore::new(
                    restarted.clone(),
                    "after-gap".into(),
                    active.id.scope.clone(),
                )
                .unwrap();
                adapter.register_activation(&active).unwrap();
                assert_eq!(saved_results(&restarted), saved);
                assert_eq!(adapter.load_source_progress(&active).unwrap(), progress);
                if reset {
                    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
                    assert_eq!(
                        adapter.claim_due(&pending, 200_000, true).unwrap(),
                        CandidateClaim::Stale
                    );
                }
            }
        }
    }
}

#[test]
fn a_gap_without_an_accepted_assessment_does_not_create_a_status() {
    for reset in [false, true] {
        let (store, adapter, active) = fixture();
        let pending = apply(
            &adapter,
            &active,
            &event(&store, 1, "pending", "branch-a", 1),
        );
        CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&active, &gap(&store, 2, reset))
            .unwrap();
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_assessment_history",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        if reset {
            assert_eq!(
                adapter.claim_due(&pending, 200_000, true).unwrap(),
                CandidateClaim::Stale
            );
        }
    }
}

#[test]
fn same_page_quality_gap_keeps_saved_status_and_previously_admitted_work() {
    for gap_first in [false, true] {
        let (store, adapter, active) = fixture();
        let accepted = apply(&adapter, &active, &event(&store, 1, "saved", "branch-a", 1));
        let assessment = answer(&adapter, &active, claim(&adapter, &accepted), 0.99);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
        let pending = apply(
            &adapter,
            &active,
            &event(&store, 2, "pending", "branch-a", 1),
        );
        let saved = saved_results(&store);
        let mut page = if gap_first {
            gap(&store, 3, false)
        } else {
            event(&store, 3, "new", "branch-a", 1)
        };
        let last = if gap_first {
            event(&store, 4, "new", "branch-a", 1)
        } else {
            gap(&store, 4, false)
        };
        page.events.extend(last.events);
        page.next = last.next;
        let outcome = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&active, &page)
            .unwrap();
        assert_eq!(outcome.candidates, 1);
        assert_eq!(saved_results(&store), saved);
        let assessment = answer(&adapter, &active, claim(&adapter, &pending), 0.99);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
    }
}
