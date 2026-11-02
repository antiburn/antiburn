use super::*;

fn retain(adapter: &Arc<StoreCheckStore>, active: &EffectiveActivation, page: &SourceBatch) {
    assert_eq!(
        CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(active, page)
            .unwrap()
            .candidates,
        0
    );
}

fn candidate_state(store: &Store, candidate: &FrozenCandidate) -> String {
    store
        .lock()
        .query_row(
            "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&candidate.fingerprint)],
            |row| row.get(0),
        )
        .unwrap()
}

fn evidence_validity(store: &Store) -> Vec<(String, bool, bool)> {
    store.lock().prepare(
        "SELECT s.branch_key, s.evidence_valid, h.evidence_valid FROM burn_check_current_status s
         JOIN burn_check_assessment_history h USING(assessment_id) ORDER BY s.branch_key",
    ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

#[test]
fn membership_correction_durably_invalidates_all_work_states_and_only_its_branch() {
    for work_state in ["pending", "claimed", "dispatching", "answered"] {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(directory.path()).unwrap());
        let source = CHECK.replace(
            "  tool_result:\n",
            "  tool_result:\n    status: [error]\n  window: {size: 2, min: 2}\n",
        );
        let (store, adapter, active) = fixture_with_definition(store, &source);
        let mut excluded = event(&store, 1, "excluded", "branch-a", 1);
        let SourceEvent::Observation { observation, .. } = &mut excluded.events[0] else {
            unreachable!()
        };
        let ObservationKind::ToolResult {
            status, message, ..
        } = &mut observation.kind
        else {
            unreachable!()
        };
        *status = "completed".into();
        message.value["tool"]["result"]["status"] = json!("completed");
        retain(&adapter, &active, &excluded);
        retain(&adapter, &active, &event(&store, 2, "a", "branch-a", 1));
        let accepted_a = apply(&adapter, &active, &event(&store, 3, "b", "branch-a", 1));
        let assessment_a = answer(&adapter, &active, claim(&adapter, &accepted_a), 0.99);
        assert_eq!(
            adapter.accept(&assessment_a).unwrap(),
            AcceptanceOutcome::Accepted
        );
        retain(&adapter, &active, &event(&store, 4, "u", "branch-b", 1));
        let accepted_b = apply(&adapter, &active, &event(&store, 5, "v", "branch-b", 1));
        let assessment_b = answer(&adapter, &active, claim(&adapter, &accepted_b), 0.99);
        assert_eq!(
            adapter.accept(&assessment_b).unwrap(),
            AcceptanceOutcome::Accepted
        );
        let pending = apply(&adapter, &active, &event(&store, 6, "c", "branch-a", 1));
        assert!(
            !accepted_a
                .messages
                .iter()
                .any(|message| message.id == "excluded")
        );
        assert!(
            !pending
                .messages
                .iter()
                .any(|message| message.id == "excluded")
        );
        match work_state {
            "pending" => {}
            "claimed" => {
                claim(&adapter, &pending);
            }
            "dispatching" => {
                adapter
                    .dispatch_review(&claim(&adapter, &pending), 100_000, Duration::ZERO, 1)
                    .unwrap();
            }
            "answered" => {
                answer(&adapter, &active, claim(&adapter, &pending), 0.99);
            }
            _ => unreachable!(),
        }
        assert_eq!(candidate_state(&store, &pending), work_state);
        assert_eq!(
            evidence_validity(&store),
            [
                ("branch-a".into(), true, true),
                ("branch-b".into(), true, true)
            ]
        );
        let before = adapter.load_source_progress(&active).unwrap();
        let correction = event(&store, 7, "excluded", "branch-a", 2);
        let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
        if work_state == "pending" {
            store
                .lock()
                .execute_batch(
                    "CREATE TEMP TRIGGER reject_selector_checkpoint
                BEFORE UPDATE OF checkpoint_json ON burn_check_activation
                BEGIN SELECT RAISE(ABORT, 'reject selector checkpoint'); END;",
                )
                .unwrap();
            assert!(engine.apply_source_page(&active, &correction).is_err());
            assert_eq!(adapter.load_source_progress(&active).unwrap(), before);
            assert_eq!(candidate_state(&store, &pending), "pending");
            assert_eq!(
                evidence_validity(&store),
                [
                    ("branch-a".into(), true, true),
                    ("branch-b".into(), true, true)
                ]
            );
            assert_eq!(store.lock().query_row("SELECT native_revision FROM burn_check_observation WHERE native_identity = 'excluded'", [], |row| row.get::<_, u64>(0)).unwrap(), 1);
            store
                .lock()
                .execute_batch("DROP TRIGGER reject_selector_checkpoint")
                .unwrap();
        }
        let outcome = engine.apply_source_page(&active, &correction).unwrap();
        assert_eq!((outcome.observations, outcome.candidates), (1, 0));
        assert_eq!(candidate_state(&store, &pending), "stale");
        assert_eq!(
            evidence_validity(&store),
            [
                ("branch-a".into(), false, false),
                ("branch-b".into(), true, true)
            ]
        );
        assert_eq!(store.lock().query_row("SELECT native_revision FROM burn_check_observation WHERE native_identity = 'excluded'", [], |row| row.get::<_, u64>(0)).unwrap(), 2);
        let committed = adapter.load_source_progress(&active).unwrap();
        assert_eq!(committed.publication_cursor, PublicationCursor(7));
        assert_eq!(
            committed.observation_position.0,
            before.observation_position.0 + 1
        );
        assert!(
            committed.selection.branches[&BranchId("branch-a".into())]
                .results
                .is_empty()
        );
        assert!(!committed.selection.branches[&BranchId("branch-a".into())].coverage_complete);
        assert_eq!(
            committed.selection.branches[&BranchId("branch-b".into())],
            before.selection.branches[&BranchId("branch-b".into())]
        );
        drop(engine);
        drop(adapter);
        drop(store);

        let store = Arc::new(Store::open(directory.path()).unwrap());
        let restarted =
            StoreCheckStore::new(store.clone(), "restarted".into(), active.id.scope.clone())
                .unwrap();
        restarted.register_activation(&active).unwrap();
        assert_eq!(restarted.load_source_progress(&active).unwrap(), committed);
        assert_eq!(candidate_state(&store, &pending), "stale");
        assert_eq!(
            evidence_validity(&store),
            [
                ("branch-a".into(), false, false),
                ("branch-b".into(), true, true)
            ]
        );
        assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
        assert_eq!(
            restarted.claim_due(&pending, 200_000, true).unwrap(),
            CandidateClaim::Stale
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status WHERE status = 'success'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn ignored_stale_and_duplicate_updates_do_not_replace_durable_observations() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 3));
    let saved = adapter.load_source_progress(&active).unwrap();
    for (sequence, revision) in [(2, 2), (3, 3)] {
        let page = event(&store, sequence, "one", "branch", revision);
        let outcome = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&active, &page)
            .unwrap();
        assert_eq!((outcome.observations, outcome.candidates), (0, 0));
        let progress = adapter.load_source_progress(&active).unwrap();
        assert_eq!(progress.publication_cursor, PublicationCursor(sequence));
        assert_eq!(progress.observation_position, saved.observation_position);
        assert_eq!(progress.selection, saved.selection);
        let observation: (u64, u64, String) = store.lock().query_row(
            "SELECT publication_sequence, native_revision, evidence_version FROM burn_check_observation", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(observation, (1, 3, "one:3".into()));
        assert_eq!(candidate_state(&store, &candidate), "pending");
    }
}
