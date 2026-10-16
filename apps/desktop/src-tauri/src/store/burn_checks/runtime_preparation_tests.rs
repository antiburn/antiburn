use super::*;
use antiburn_checks::engine::{EvaluationFailureCode, RunSettlement};
use antiburn_checks::fit::{FitReport, PreparationFailureCode, PreparedDecision};

pub(super) struct PreparationEvaluator {
    prepare_count: std::sync::atomic::AtomicUsize,
    recorded: Mutex<Vec<PreparationRecord>>,
    sent: Mutex<Vec<PreparedDecision>>,
    reject: bool,
    incompatible: bool,
}

impl PreparationEvaluator {
    pub(super) fn new(reject: bool, incompatible: bool) -> Self {
        Self {
            prepare_count: std::sync::atomic::AtomicUsize::new(0),
            recorded: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            reject,
            incompatible,
        }
    }
}

impl DecisionEvaluator for PreparationEvaluator {
    fn preparation_recorded(&self, preparation: &PreparationRecord) {
        self.recorded.lock().unwrap().push(preparation.clone());
    }

    fn prepare(
        &self,
        _: &EffectiveActivation,
        input: serde_json::Value,
    ) -> Result<PreparedDecision, PreparationFailure> {
        self.prepare_count.fetch_add(1, Ordering::SeqCst);
        if self.reject {
            Err(PreparationFailure { code: PreparationFailureCode::NoFit, detail: "The complete request exceeds the local model budget after permitted reductions.".into(), report: Some(FitReport::default()), context: Default::default() })
        } else {
            Ok(PreparedDecision {
                profile: "fixture-v1".into(),
                input,
                request: Some(json!({"exact": "saved"})),
                report: FitReport::default(),
            })
        }
    }

    fn validate_prepared(
        &self,
        _: &EffectiveActivation,
        _: &PreparedDecision,
    ) -> Result<(), PreparationFailure> {
        if self.incompatible {
            Err(PreparationFailure {
                code: PreparationFailureCode::IncompatibleProfile,
                detail: "The saved request uses an incompatible preparation profile.".into(),
                report: None,
                context: Default::default(),
            })
        } else {
            Ok(())
        }
    }

    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        prepared: &'a PreparedDecision,
        _: u8,
    ) -> DecisionFuture<'a> {
        self.sent.lock().unwrap().push(prepared.clone());
        Box::pin(async {
            Err(EvaluationFailure {
                code: EvaluationFailureCode::AuthenticationRejected,
                kind: AttemptFailure::Permanent,
                detail: "Synthetic authentication failure.".into(),
                attempts: 1,
            })
        })
    }

    fn evaluate<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a serde_json::Value,
        _: u8,
    ) -> DecisionFuture<'a> {
        panic!("the engine must use the exact saved preparation")
    }
}

fn run(
    engine: &CheckEngine<StoreCheckStore, PreparationEvaluator>,
    active: &EffectiveActivation,
) -> RunSettlement {
    let StartOutcome::Started(task) = engine.start_one_due(active, 100_000).unwrap() else {
        panic!("candidate must start");
    };
    poll_settlement(task)
}

#[test]
fn no_fit_has_zero_attempts_no_cooldown_no_status_and_does_not_block_later_candidates() {
    let (store, adapter, active) = fixture();
    let evaluator = Arc::new(PreparationEvaluator::new(true, false));
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    engine
        .apply_source_page(&active, &event(&store, 2, "two", "branch", 1))
        .unwrap();
    for _ in 0..2 {
        let settled = run(&engine, &active);
        assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
        assert_eq!(settled.attempts, 0);
    }
    assert!(matches!(
        engine.start_one_due(&active, 100_000).unwrap(),
        StartOutcome::NotDue
    ));
    assert!(evaluator.sent.lock().unwrap().is_empty());
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    let records = store
        .lock()
        .prepare("SELECT preparation_json FROM burn_check_candidate ORDER BY trigger_position")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(evaluator.recorded.lock().unwrap().len(), 2);
    for saved in records {
        let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
            panic!("local failure must be typed");
        };
        assert_eq!(failure.code, PreparationFailureCode::NoFit);
    }
    let restarted = StoreCheckStore::new(store, "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
}

#[test]
fn jev_budget_rejection_occurs_before_the_engine_writes_dispatch_intent() {
    let source = CHECK.replace(
        "question: Is this useful?",
        &format!("question: {}", "q".repeat(50_000)),
    );
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-preparation-test")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = crate::jev::decision_evaluator::TypeSafeDecisionEvaluator::new(
        crate::jev::client::TypeSafeClient::new("synthetic-key".into()).unwrap(),
    )
    .with_usage(Arc::new(|_, _, _| {
        panic!("local no-fit must not reach provider usage settlement")
    }));
    let engine = CheckEngine::new(adapter, Arc::new(evaluator));
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("local rejection must settle");
    };
    let settled = poll_settlement(task);
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!(settled.attempts, 0);
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
        panic!("Jev preflight rejection must be typed");
    };
    assert_eq!(failure.code, PreparationFailureCode::NoFit);
}

#[test]
fn fitting_resource_limit_is_persisted_before_any_dispatch_or_cooldown() {
    let values =
        serde_json::to_string(&vec!["x"; antiburn_checks::fit::MAX_FIT_TARGETS + 1]).unwrap();
    let source = CHECK.replace(
        "input: {activity: \"${{ message.tool.result.status }}\"}",
        &format!("input: {{activity: {values}}}\n    fit:\n      'activity[*]': truncate"),
    );
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-resource-limit-test")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = crate::jev::decision_evaluator::TypeSafeDecisionEvaluator::new(
        crate::jev::client::TypeSafeClient::new("synthetic-key".into()).unwrap(),
    )
    .with_usage(Arc::new(|_, _, _| {
        panic!("resource rejection must not reach a provider attempt")
    }));
    let engine = CheckEngine::new(adapter, Arc::new(evaluator));
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("resource rejection must settle locally");
    };
    let settled = poll_settlement(task);
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!(settled.attempts, 0);
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
        panic!("resource rejection must be typed");
    };
    assert_eq!(
        failure.code,
        PreparationFailureCode::PreparationResourceLimit
    );
    assert_eq!(
        failure.context.field.as_deref(),
        Some("decisions.review.fit")
    );
    assert_eq!(failure.context.input_path.as_deref(), Some("activity[*]"));
}

#[test]
fn safe_retry_after_restart_reuses_exact_preparation_and_persists_typed_failure() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let prepared = PreparedDecision {
        profile: "fixture-v1".into(),
        input: json!({"original": [3,4]}),
        request: Some(json!({"exact": "frozen request"})),
        report: FitReport::default(),
    };
    adapter
        .record_preparation(&claimed, &PreparationRecord::Ready(prepared.clone()))
        .unwrap();
    let mut changed = prepared.clone();
    changed.input = json!({"changed": true});
    assert!(
        adapter
            .record_preparation(&claimed, &PreparationRecord::Ready(changed))
            .is_err()
    );
    adapter
        .record_dispatch_intent(&claimed, 1_000, Duration::ZERO, 1)
        .unwrap();
    adapter
        .record_attempt_failure(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let restarted = Arc::new(
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let evaluator = Arc::new(PreparationEvaluator::new(false, false));
    let engine = CheckEngine::new(restarted, evaluator.clone());
    assert_eq!(engine.recover_pending().unwrap().candidates, 1);
    let settled = run(&engine, &active);
    assert_eq!(settled.attempts, 2);
    assert!(matches!(settled.outcome, RunOutcome::Failed(_)));
    assert_eq!(evaluator.prepare_count.load(Ordering::SeqCst), 0);
    assert!(evaluator.recorded.lock().unwrap().is_empty());
    assert_eq!(*evaluator.sent.lock().unwrap(), [prepared]);
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
}

#[test]
fn recovery_rejects_incompatible_or_missing_preparation_without_a_new_attempt() {
    for saved in [false, true] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        if saved {
            let prepared = PreparedDecision {
                profile: "old-profile".into(),
                input: json!({}),
                request: Some(json!({})),
                report: FitReport::default(),
            };
            adapter
                .record_preparation(&claimed, &PreparationRecord::Ready(prepared))
                .unwrap();
        }
        adapter
            .record_dispatch_intent(&claimed, 1_000, Duration::ZERO, 1)
            .unwrap();
        adapter
            .record_attempt_failure(
                &claimed,
                1,
                AttemptFailure::SafeTransient,
                &transient_diagnostic(),
            )
            .unwrap();
        let restarted = Arc::new(
            StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
        );
        restarted.register_activation(&active).unwrap();
        let evaluator = Arc::new(PreparationEvaluator::new(false, true));
        let engine = CheckEngine::new(restarted, evaluator.clone());
        engine.recover_pending().unwrap();
        let settled = run(&engine, &active);
        assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
        assert_eq!(settled.attempts, 1);
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
        assert_eq!(evaluator.prepare_count.load(Ordering::SeqCst), 0);
        assert!(evaluator.recorded.lock().unwrap().is_empty());
        assert!(evaluator.sent.lock().unwrap().is_empty());
        let saved: String = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
            panic!("recovery rejection must be typed");
        };
        assert_eq!(failure.code, PreparationFailureCode::IncompatibleProfile);
    }
}

#[test]
fn preparation_observation_follows_commit_and_never_repeats_after_later_storage_failure() {
    for stage in ["save", "rejection", "intent"] {
        let (store, adapter, active) = fixture();
        let evaluator = Arc::new(PreparationEvaluator::new(stage == "rejection", false));
        let engine = CheckEngine::new(adapter, evaluator.clone());
        engine
            .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
            .unwrap();
        store.lock().execute_batch(match stage {
            "save" => "CREATE TRIGGER reject_preflight BEFORE UPDATE OF preparation_json ON burn_check_candidate WHEN NEW.preparation_json IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected preparation failure'); END",
            "rejection" => "CREATE TRIGGER reject_preflight BEFORE INSERT ON burn_check_assessment_history BEGIN SELECT RAISE(ABORT, 'injected rejection failure'); END",
            "intent" => "CREATE TRIGGER reject_preflight BEFORE INSERT ON burn_check_evaluation_attempt BEGIN SELECT RAISE(ABORT, 'injected intent failure'); END",
            _ => unreachable!(),
        }).unwrap();
        assert!(engine.start_one_due(&active, 100_000).is_err());
        assert_eq!(
            evaluator.recorded.lock().unwrap().len(),
            usize::from(stage != "save"),
            "{stage}"
        );
        assert!(evaluator.sent.lock().unwrap().is_empty());
        let saved: Option<String> = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(saved.is_some(), stage != "save");
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_preflight")
            .unwrap();
        run(&engine, &active);
        let records = evaluator.recorded.lock().unwrap();
        assert_eq!(records.len(), 1, "{stage}");
        let saved: String = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            records[0],
            serde_json::from_str::<PreparationRecord>(&saved).unwrap()
        );
    }
}

#[test]
fn unavailable_input_is_observed_without_calling_the_preparer_or_provider() {
    let store = Arc::new(
        Store::open_in_memory(Path::new("/tmp/antiburn-preparation-observer-test")).unwrap(),
    );
    let source = CHECK.replace("message.tool.result.status", "message.tool.result.content");
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = Arc::new(PreparationEvaluator::new(false, false));
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    assert!(matches!(
        run(&engine, &active).outcome,
        RunOutcome::NotAssessed(_)
    ));
    assert_eq!(evaluator.prepare_count.load(Ordering::SeqCst), 0);
    assert!(evaluator.sent.lock().unwrap().is_empty());
    let records = evaluator.recorded.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert!(
        matches!(&records[0], PreparationRecord::Rejected(error) if error.code == PreparationFailureCode::InputUnavailable)
    );
}

#[test]
fn a_stale_preparation_claim_emits_no_observation() {
    struct RevokingEvaluator {
        store: Arc<Store>,
        observations: std::sync::atomic::AtomicUsize,
    }
    impl DecisionEvaluator for RevokingEvaluator {
        fn prepare(
            &self,
            _: &EffectiveActivation,
            input: serde_json::Value,
        ) -> Result<PreparedDecision, PreparationFailure> {
            self.store
                .lock()
                .execute(
                    "UPDATE burn_check_candidate SET state = 'stale' WHERE state = 'claimed'",
                    [],
                )
                .unwrap();
            Ok(PreparedDecision {
                profile: "fixture-v1".into(),
                input,
                request: None,
                report: FitReport::default(),
            })
        }

        fn preparation_recorded(&self, _: &PreparationRecord) {
            self.observations.fetch_add(1, Ordering::SeqCst);
        }

        fn evaluate<'a>(
            &'a self,
            _: &'a EffectiveActivation,
            _: &'a FrozenCandidate,
            _: &'a serde_json::Value,
            _: u8,
        ) -> DecisionFuture<'a> {
            panic!("A stale preparation must not dispatch")
        }
    }
    let (store, adapter, active) = fixture();
    let evaluator = Arc::new(RevokingEvaluator {
        store: store.clone(),
        observations: std::sync::atomic::AtomicUsize::new(0),
    });
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    assert!(matches!(
        engine.start_one_due(&active, 100_000).unwrap(),
        StartOutcome::Stale
    ));
    assert_eq!(evaluator.observations.load(Ordering::SeqCst), 0);
}

#[test]
fn recovery_fences_changed_compiled_policy_even_with_the_same_activation_revision() {
    let (store, adapter, mut active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let evaluator = crate::jev::decision_evaluator::TypeSafeDecisionEvaluator::new(
        crate::jev::client::TypeSafeClient::new("synthetic-key".into()).unwrap(),
    )
    .with_usage(Arc::new(|_, _, _| {
        panic!("changed policy must not reach a provider attempt")
    }));
    let prepared = evaluator
        .prepare(&active, json!({"activity": "error"}))
        .unwrap();
    adapter
        .record_preparation(&claimed, &PreparationRecord::Ready(prepared))
        .unwrap();
    adapter
        .record_dispatch_intent(&claimed, 1_000, Duration::ZERO, 1)
        .unwrap();
    adapter
        .record_attempt_failure(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    Arc::make_mut(&mut active.compiled)
        .decision
        .question
        .push_str(" Changed policy.");
    let restarted = Arc::new(
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let engine = CheckEngine::new(restarted, Arc::new(evaluator));
    engine.recover_pending().unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("changed preparation must settle locally");
    };
    let settled = poll_settlement(task);
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!(settled.attempts, 1);
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
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
        panic!("policy mismatch must be typed");
    };
    assert_eq!(failure.code, PreparationFailureCode::IncompatibleProfile);
    assert_eq!(failure.context.decision.as_deref(), Some("review"));
}

#[test]
fn migration_adds_nullable_preparation_to_existing_candidates() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for migration in &crate::store::schema::MIGRATIONS[..76] {
        connection.execute_batch(migration).unwrap();
    }
    connection.pragma_update(None, "user_version", 76).unwrap();
    connection
        .execute_batch(crate::store::schema::MIGRATIONS[76])
        .unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(burn_check_candidate)")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, bool>(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(columns.contains(&("preparation_json".into(), false)));
    assert!(columns.contains(&("diagnostic_json".into(), false)));
}
