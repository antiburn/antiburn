use super::*;
use antiburn_checks::engine::{EvaluationFailureCode, ExecutionPolicy, RunSettlement};

fn settle(
    engine: &CheckEngine<StoreCheckStore, RecordingEvaluator>,
    active: &EffectiveActivation,
) -> RunSettlement {
    let StartOutcome::Started(task) = engine.start_one_due(active, 100_000).unwrap() else {
        panic!("candidate must start");
    };
    poll_settlement(task)
}

#[test]
fn pre_dispatch_store_errors_preserve_work_and_allow_safe_same_branch_progress() {
    for stage in ["load", "save", "rejection", "intent"] {
        let store =
            Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-pre-dispatch-test")).unwrap());
        let source = if stage == "rejection" {
            CHECK.replace("message.tool.result.status", "message.tool.result.content")
        } else {
            CHECK.to_owned()
        };
        let (store, adapter, active) = fixture_with_definition(store, &source);
        let evaluator = Arc::new(RecordingEvaluator(Mutex::new(Vec::new())));
        let mut engine = CheckEngine::with_policy(
            adapter.clone(),
            evaluator.clone(),
            ExecutionPolicy::SequentialReplay,
        );
        engine
            .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
            .unwrap();
        let fail_sql = match stage {
            "load" => {
                "ALTER TABLE burn_check_candidate RENAME COLUMN preparation_json TO unavailable_preparation_json"
            }
            "save" => {
                "CREATE TRIGGER reject_preflight BEFORE UPDATE OF preparation_json ON burn_check_candidate WHEN NEW.preparation_json IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected preparation failure'); END"
            }
            "rejection" => {
                "CREATE TRIGGER reject_preflight BEFORE INSERT ON burn_check_assessment_history BEGIN SELECT RAISE(ABORT, 'injected terminal failure'); END"
            }
            "intent" => {
                "CREATE TRIGGER reject_preflight BEFORE INSERT ON burn_check_evaluation_attempt BEGIN SELECT RAISE(ABORT, 'injected dispatch failure'); END"
            }
            _ => unreachable!(),
        };
        store.lock().execute_batch(fail_sql).unwrap();
        assert_start_error(&engine, &active);
        assert!(evaluator.0.lock().unwrap().is_empty(), "{stage}");
        let row: (String, Option<String>) = store
            .lock()
            .query_row(
                "SELECT state, claim_owner FROM burn_check_candidate",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        if stage == "intent" {
            assert_eq!(row, ("claimed".into(), Some("owner-1".into())), "{stage}");
        } else {
            assert_eq!(row, ("pending".into(), None), "{stage}");
        }
        for table in [
            "burn_check_evaluation_attempt",
            "burn_check_cooldown",
            "burn_check_current_status",
            "burn_check_assessment_history",
        ] {
            assert_eq!(
                store
                    .lock()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0,
                "{stage}: {table}"
            );
        }
        store
            .lock()
            .execute_batch(if stage == "load" {
                "ALTER TABLE burn_check_candidate RENAME COLUMN unavailable_preparation_json TO preparation_json"
            } else {
                "DROP TRIGGER reject_preflight"
            })
            .unwrap();
        if stage == "intent" {
            let restarted = Arc::new(
                StoreCheckStore::new(
                    store.clone(),
                    "storage-restart".into(),
                    active.id.scope.clone(),
                )
                .unwrap(),
            );
            restarted.register_activation(&active).unwrap();
            engine = CheckEngine::with_policy(
                restarted,
                evaluator.clone(),
                ExecutionPolicy::SequentialReplay,
            );
            engine.recover_pending().unwrap();
        }
        let first = settle(&engine, &active);
        if stage == "rejection" {
            assert!(matches!(first.outcome, RunOutcome::NotAssessed(_)));
        } else {
            assert_eq!(first.outcome, RunOutcome::Accepted, "{stage}");
        }
        let mut next = event(&store, 2, "two", "branch", 1);
        if let SourceEvent::Observation { observation, .. } = &mut next.events[0]
            && let ObservationKind::ToolResult { message, .. } = &mut observation.kind
        {
            message.value["tool"]["result"]["content"] = json!("available result");
        }
        engine.apply_source_page(&active, &next).unwrap();
        assert_eq!(
            settle(&engine, &active).outcome,
            RunOutcome::Accepted,
            "{stage}"
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT MAX(trigger_position) FROM burn_check_assessment_history",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            52,
            "{stage}"
        );
    }
}

#[test]
fn failed_release_survives_window_coalescing_and_is_retried_without_restart() {
    let store = Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-release-test")).unwrap());
    let source = CHECK.replace(
        "  tool_result:\n",
        "  tool_result:\n  window: {size: 1, min: 1}\n",
    );
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = Arc::new(RecordingEvaluator(Mutex::new(Vec::new())));
    let engine = CheckEngine::with_policy(
        adapter.clone(),
        evaluator.clone(),
        ExecutionPolicy::SequentialReplay,
    );
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    store.lock().execute_batch("CREATE TRIGGER reject_preparation BEFORE UPDATE OF preparation_json ON burn_check_candidate BEGIN SELECT RAISE(ABORT, 'injected preparation failure'); END;
        CREATE TRIGGER reject_release BEFORE UPDATE OF state ON burn_check_candidate WHEN OLD.state = 'claimed' AND NEW.state = 'pending' BEGIN SELECT RAISE(ABORT, 'injected release failure'); END;").unwrap();
    for _ in 0..2 {
        assert_start_error(&engine, &active);
    }
    assert!(evaluator.0.lock().unwrap().is_empty());
    engine
        .apply_source_page(&active, &event(&store, 2, "two", "branch", 1))
        .unwrap();
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_preparation; DROP TRIGGER reject_release;")
        .unwrap();
    assert_eq!(settle(&engine, &active).outcome, RunOutcome::Accepted);
    let states = store
        .lock()
        .prepare(
            "SELECT trigger_position, state FROM burn_check_candidate ORDER BY trigger_position",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(states, [(51, "coalesced".into()), (52, "accepted".into())]);
    assert_eq!(*evaluator.0.lock().unwrap(), [1]);
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
}

#[test]
fn pre_dispatch_release_is_fenced_and_cannot_release_a_dispatch_intent() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let first = claim(&adapter, &candidate);
    assert_eq!(
        adapter.release_pre_dispatch_claim(&first).unwrap(),
        ClaimOutcome::Current
    );
    let second = claim(&adapter, &candidate);
    assert!(second.fence > first.fence);
    assert_eq!(
        adapter.release_pre_dispatch_claim(&first).unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(
        adapter
            .dispatch_review(&first, 100_000, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(
        adapter
            .dispatch_review(&second, 100_000, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        adapter.release_pre_dispatch_claim(&second).unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "dispatching"
    );
    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    let diagnostic: String = store
        .lock()
        .query_row(
            "SELECT diagnostic_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<EvaluationDiagnostic>(&diagnostic)
            .unwrap()
            .code,
        EvaluationFailureCode::DeliveryUnknown
    );
}

#[test]
fn releasing_a_recovered_retry_preserves_preparation_and_attempt_number() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let first = claim(&adapter, &candidate);
    let prepared = PreparationRecord::Ready(prepared_review(
        "fixture-v1",
        json!({"activity": "frozen"}),
        json!({"exact": "request"}),
        Default::default(),
        EvaluationBudget::default(),
    ));
    adapter.record_preparation(&first, &prepared).unwrap();
    adapter
        .dispatch_review(&first, 1_000, Duration::ZERO, 1)
        .unwrap();
    adapter
        .fail_review(
            &first,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    let retry = claim(&restarted, &candidate);
    assert_eq!(next_review_attempt(&restarted, &retry), 2);
    assert_eq!(
        restarted.release_pre_dispatch_claim(&retry).unwrap(),
        ClaimOutcome::Current
    );
    let fresh = claim(&restarted, &candidate);
    assert_eq!(next_review_attempt(&restarted, &fresh), 2);
    assert_eq!(restarted.load_preparation(&fresh).unwrap(), Some(prepared));
    assert_eq!(
        restarted
            .dispatch_review(&retry, 100_000, Duration::ZERO, 2)
            .unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(
        restarted
            .dispatch_review(&fresh, 100_000, Duration::ZERO, 2)
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn failed_attempt_diagnostic_and_settlement_are_atomic_and_survive_restart() {
    for (code, kind, number) in [
        (
            EvaluationFailureCode::AuthenticationRejected,
            AttemptFailure::Permanent,
            1,
        ),
        (
            EvaluationFailureCode::InvalidResponse,
            AttemptFailure::Permanent,
            1,
        ),
        (
            EvaluationFailureCode::RateLimited,
            AttemptFailure::SafeTransient,
            1,
        ),
        (
            EvaluationFailureCode::RateLimited,
            AttemptFailure::SafeTransient,
            2,
        ),
    ] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        adapter
            .dispatch_review(&claimed, 100_000, Duration::ZERO, 1)
            .unwrap();
        if number == 2 {
            adapter
                .fail_review(
                    &claimed,
                    1,
                    AttemptFailure::SafeTransient,
                    &transient_diagnostic(),
                )
                .unwrap();
            adapter
                .dispatch_review(&claimed, 100_000, Duration::ZERO, 2)
                .unwrap();
            assert!(
                store
                    .lock()
                    .query_row(
                        "SELECT diagnostic_json IS NULL FROM burn_check_candidate",
                        [],
                        |row| row.get::<_, bool>(0)
                    )
                    .unwrap()
            );
        }
        let diagnostic = EvaluationDiagnostic {
            code,
            detail: "Synthetic classified failure.".into(),
        };
        store.lock().execute_batch("CREATE TRIGGER reject_attempt_diagnostic BEFORE UPDATE OF diagnostic_json ON burn_check_candidate WHEN NEW.diagnostic_json IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected diagnostic failure'); END;").unwrap();
        assert!(
            adapter
                .fail_review(&claimed, number, kind, &diagnostic)
                .is_err()
        );
        let state: String = store
            .lock()
            .query_row(
                "SELECT state FROM burn_check_evaluation_attempt WHERE attempt_number = ?1",
                [number],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "dispatch_intent");
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_attempt_diagnostic")
            .unwrap();
        adapter
            .fail_review(&claimed, number, kind, &diagnostic)
            .unwrap();
        let restarted =
            StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
        restarted.register_activation(&active).unwrap();
        let saved: String = store
            .lock()
            .query_row(
                "SELECT diagnostic_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<EvaluationDiagnostic>(&saved).unwrap(),
            diagnostic
        );
        assert_eq!(
            restarted.load_recovery(None, 10).unwrap().len(),
            usize::from(kind == AttemptFailure::SafeTransient && number == 1)
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                    [],
                    |row| row.get::<_, u8>(0)
                )
                .unwrap(),
            number
        );
    }
}

#[test]
fn terminal_write_failure_after_engine_settlement_preserves_authentication_diagnostic() {
    let (store, adapter, active) = fixture();
    let evaluator = Arc::new(super::preparation::PreparationEvaluator::new(false, false));
    let engine = CheckEngine::new(adapter, evaluator);
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    store.lock().execute_batch("CREATE TRIGGER reject_terminal_history BEFORE INSERT ON burn_check_assessment_history BEGIN SELECT RAISE(ABORT, 'injected terminal history failure'); END;").unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("candidate must start");
    };
    assert!(poll_task_result(task).is_err());
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_terminal_history")
        .unwrap();
    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    let saved: String = store
        .lock()
        .query_row(
            "SELECT diagnostic_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<EvaluationDiagnostic>(&saved)
            .unwrap()
            .code,
        EvaluationFailureCode::AuthenticationRejected
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT outcome FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "execution_failed"
    );
}
