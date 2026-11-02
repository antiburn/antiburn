use super::*;
use antiburn_checks::engine::RunSettlement;
use antiburn_checks::expression_eval::EvaluationBudget;

const LOCAL_CHECK: &str = "name: Local status\non:\n  tool_result:\nactions:\n  - type: status\n    if: ${{ message.tool.result.status == 'error' }}\n    input: {result: failure, message: Frozen local failure.}\n";

struct NoLocalProvider;

impl DecisionEvaluator for NoLocalProvider {
    fn preparation_recorded(&self, _: &PreparationRecord) {
        panic!("local work must not report provider preparation")
    }

    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<DecisionInput>,
        _: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        panic!("local work must not prepare a provider request")
    }

    fn validate_prepared(
        &self,
        active: &EffectiveActivation,
        prepared: &PreparedAssessment,
        purpose: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        assert!(
            !active.compiled.is_local(),
            "local work must not validate provider preparation"
        );
        RecordingEvaluator(Mutex::new(Vec::new())).validate_prepared(active, prepared, purpose)
    }

    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        _: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        panic!("local work must not send a prepared provider request")
    }
}

fn local_fixture() -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/local-check")).unwrap()),
        LOCAL_CHECK,
    )
}

fn preparation(now_ms: u64) -> LocalPreparation {
    LocalPreparation {
        version: 1,
        started_at_unix_ms: now_ms,
        evaluation_budget: EvaluationBudget::default(),
    }
}

fn count(store: &Store, table: &str) -> i64 {
    store
        .lock()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn assert_no_provider_rows(store: &Store) {
    assert_eq!(count(store, "burn_check_evaluation_attempt"), 0);
    assert_eq!(count(store, "burn_check_decision_usage"), 0);
    assert_eq!(store.burn_check_usage_summary().unwrap().confirmed_calls, 0);
    assert_eq!(
        store.burn_check_usage_summary().unwrap().unknown_outcomes,
        0
    );
}

fn snapshot(store: &Store) -> Vec<String> {
    let connection = store.lock();
    [
        "burn_check_candidate",
        "burn_check_cooldown",
        "burn_check_evaluation_attempt",
        "burn_check_decision_usage",
        "burn_check_current_status",
        "burn_check_assessment_history",
        "burn_check_acceptance_watermark",
    ]
    .into_iter()
    .map(|table| {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                Ok((0..columns)
                    .map(|index| format!("{:?}", row.get_ref(index).unwrap()))
                    .collect::<Vec<_>>())
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        format!("{table}: {rows:?}")
    })
    .collect()
}

fn saved_preparation(store: &Store, candidate: &FrozenCandidate) -> LocalPreparation {
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&candidate.fingerprint)],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Local(preparation) = serde_json::from_str(&saved).unwrap() else {
        panic!("the saved start must remain a typed local preparation");
    };
    preparation
}

fn state(store: &Store, candidate: &FrozenCandidate) -> String {
    store
        .lock()
        .query_row(
            "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&candidate.fingerprint)],
            |row| row.get(0),
        )
        .unwrap()
}

fn assert_invalid_recovery(store: &Store, candidate: &FrozenCandidate) {
    assert_eq!(state(store, candidate), "execution_failed");
    let (diagnostic, outcome, actions): (String, String, String) = store
        .lock()
        .query_row(
            "SELECT c.diagnostic_json, h.outcome, h.actions_json
             FROM burn_check_candidate c JOIN burn_check_assessment_history h USING(candidate_id)
             WHERE c.candidate_id = ?1",
            [hex(&candidate.fingerprint)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<EvaluationDiagnostic>(&diagnostic)
            .unwrap()
            .code,
        antiburn_checks::engine::EvaluationFailureCode::InvalidDecision
    );
    assert_eq!(outcome, "execution_failed");
    let actions: serde_json::Value = serde_json::from_str(&actions).unwrap();
    assert!(actions.get("outcomes").is_none());
    assert!(actions.get("current_status").is_none());
    let writes: (i64, i64) = store
        .lock()
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM burn_check_current_status WHERE branch_key = ?1),
                (SELECT COUNT(*) FROM burn_check_acceptance_watermark WHERE branch_key = ?1)",
            [&candidate.branch.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(writes, (0, 0));
}

fn start_answer(
    adapter: &StoreCheckStore,
    active: &EffectiveActivation,
    claimed: ClaimedCandidate,
    now_ms: u64,
    cooldown: Duration,
) -> AssessmentAcceptance {
    assert!(
        adapter
            .load_decision_progress(&claimed)
            .unwrap()
            .attempts
            .is_empty()
    );
    let preparation = preparation(now_ms);
    assert_eq!(
        adapter
            .record_local_start(&claimed, now_ms, cooldown, &preparation)
            .unwrap(),
        ClaimOutcome::Current
    );
    let evaluated = EvaluatedAssessment::local();
    evaluated
        .validate_for_declarations(
            &active.compiled.decisions,
            &active.compiled.question_requirements(),
        )
        .unwrap();
    let actions = plan_status_actions(
        &active.compiled.actions,
        &claimed.candidate.context.value,
        &evaluated,
        claimed.prior_status,
    )
    .unwrap();
    AssessmentAcceptance {
        claimed,
        evaluated,
        actions,
    }
}

fn settle(
    engine: &CheckEngine<StoreCheckStore, NoLocalProvider>,
    active: &EffectiveActivation,
    now_ms: u64,
) -> RunSettlement {
    let StartOutcome::Started(task) = engine.start_one_due(active, now_ms).unwrap() else {
        panic!("the local candidate must start");
    };
    let settled = poll_settlement(task);
    assert_eq!(settled.attempts, 0);
    settled
}

fn restart(store: &Arc<Store>, active: &EffectiveActivation) -> StoreCheckStore {
    let adapter = StoreCheckStore::new(
        store.clone(),
        "local-restart".into(),
        active.id.scope.clone(),
    )
    .unwrap();
    adapter.register_activation(active).unwrap();
    adapter
}

#[test]
fn local_start_is_atomic_idempotent_and_keeps_the_first_cooldown() {
    for stage in ["answer", "cooldown"] {
        let (store, adapter, active) = local_fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        let mut preparation = preparation(1001);
        preparation.evaluation_budget.step(0).unwrap();
        let original = snapshot(&store);
        store.lock().execute_batch(match stage {
            "answer" => "CREATE TRIGGER reject_local BEFORE UPDATE OF answer_json ON burn_check_candidate BEGIN SELECT RAISE(ABORT, 'injected local answer failure'); END",
            "cooldown" => "CREATE TRIGGER reject_local BEFORE INSERT ON burn_check_cooldown BEGIN SELECT RAISE(ABORT, 'injected local cooldown failure'); END",
            _ => unreachable!(),
        }).unwrap();
        assert!(
            adapter
                .record_local_start(&claimed, 1001, Duration::from_millis(2500), &preparation)
                .is_err()
        );
        assert_eq!(snapshot(&store), original, "{stage}");
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_local")
            .unwrap();
        assert_eq!(
            adapter
                .record_local_start(&claimed, 1001, Duration::from_millis(2500), &preparation)
                .unwrap(),
            ClaimOutcome::Current
        );
        assert_eq!(saved_preparation(&store, &candidate), preparation);
        let until: u64 = store
            .lock()
            .query_row(
                "SELECT cooldown_until_epoch FROM burn_check_cooldown",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(until, 4);
        let saved = snapshot(&store);
        store.lock().execute_batch(
            "CREATE TRIGGER reject_candidate_repeat BEFORE UPDATE ON burn_check_candidate BEGIN SELECT RAISE(ABORT, 'local start must not update'); END;
             CREATE TRIGGER reject_cooldown_repeat BEFORE INSERT ON burn_check_cooldown BEGIN SELECT RAISE(ABORT, 'local start must not extend cooldown'); END"
        ).unwrap();
        assert_eq!(
            adapter
                .record_local_start(&claimed, 99_000, Duration::from_secs(120), &preparation)
                .unwrap(),
            ClaimOutcome::Current
        );
        let mut changed = preparation.clone();
        changed.started_at_unix_ms = 99_000;
        assert!(
            adapter
                .record_local_start(&claimed, 99_000, Duration::ZERO, &changed)
                .is_err()
        );
        assert_eq!(snapshot(&store), saved);
        assert_no_provider_rows(&store);
    }
}

#[test]
fn local_start_rejects_provider_operations_and_unexpected_attempts() {
    let (store, adapter, active) = local_fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    assert!(
        adapter
            .load_decision_progress(&claimed)
            .unwrap()
            .attempts
            .is_empty()
    );
    let preparation = preparation(1001);
    let original = snapshot(&store);
    assert!(
        adapter
            .record_preparation(&claimed, &PreparationRecord::Local(preparation.clone()))
            .is_err()
    );
    let provider_preparation = PreparationRecord::Ready(prepared_review(
        "synthetic-provider",
        json!({}),
        json!({}),
        Default::default(),
        EvaluationBudget::default(),
    ));
    assert!(
        adapter
            .record_preparation(&claimed, &provider_preparation)
            .is_err()
    );
    assert!(
        adapter
            .record_dispatch_intent(&claimed, 1001, Duration::ZERO, &review_attempt(1))
            .is_err()
    );
    assert!(
        adapter
            .record_decision_answer(
                &claimed,
                &review_attempt(1),
                &review_result(
                    &EffectiveActivation {
                        compiled: Arc::new(compile(CHECK).unwrap()),
                        ..active.clone()
                    },
                    1,
                    0.99
                )
            )
            .is_err()
    );
    assert!(
        adapter
            .record_attempt_failure(
                &claimed,
                &review_attempt(1),
                AttemptFailure::SafeTransient,
                &transient_diagnostic()
            )
            .is_err()
    );
    assert_eq!(snapshot(&store), original);
    store.lock().execute(
        "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, attempt_number, state, outcome, updated_at_epoch)
         VALUES (?1, ?2, 1, 'settled', 'safe_transient', 1)",
        params![format!("{}:1", hex(&candidate.fingerprint)), hex(&candidate.fingerprint)],
    ).unwrap();
    assert!(
        adapter
            .record_local_start(&claimed, 1001, Duration::ZERO, &preparation)
            .is_err()
    );
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET state = 'pending', claim_owner = NULL",
            [],
        )
        .unwrap();
    assert!(adapter.claim_due(&candidate, 1001, true).is_err());
    assert_eq!(count(&store, "burn_check_cooldown"), 0);

    let (model_store, model_adapter, model_active) = fixture();
    let model = apply(
        &model_adapter,
        &model_active,
        &event(&model_store, 1, "one", "branch", 1),
    );
    let claimed = claim(&model_adapter, &model);
    let original = snapshot(&model_store);
    assert!(
        model_adapter
            .record_local_start(&claimed, 1001, Duration::ZERO, &preparation)
            .is_err()
    );
    assert!(
        model_adapter
            .record_preparation(&claimed, &PreparationRecord::Local(preparation))
            .is_err()
    );
    assert_eq!(snapshot(&model_store), original);
}

#[test]
fn local_start_fences_the_exact_claim_candidate_and_preparation() {
    let (store, adapter, active) = local_fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let old = claim(&adapter, &candidate);
    assert_eq!(
        adapter.release_pre_dispatch_claim(&old).unwrap(),
        ClaimOutcome::Current
    );
    let claimed = claim(&adapter, &candidate);
    let preparation = preparation(1001);
    let original = snapshot(&store);
    assert_eq!(
        adapter
            .record_local_start(&old, 1001, Duration::ZERO, &preparation)
            .unwrap(),
        ClaimOutcome::Stale
    );
    let mut changed = claimed.clone();
    changed.candidate.context.value["changed"] = json!(true);
    assert!(
        adapter
            .record_local_start(&changed, 1001, Duration::ZERO, &preparation)
            .is_err()
    );
    changed = claimed.clone();
    changed.id = CandidateId([42; 32]);
    assert!(
        adapter
            .record_local_start(&changed, 1001, Duration::ZERO, &preparation)
            .is_err()
    );
    let mut invalid = preparation.clone();
    invalid.version = 2;
    assert!(
        adapter
            .record_local_start(&claimed, 1001, Duration::ZERO, &invalid)
            .is_err()
    );
    invalid = preparation.clone();
    assert!(invalid.evaluation_budget.visit_items(1001).is_err());
    assert!(
        adapter
            .record_local_start(&claimed, 1001, Duration::ZERO, &invalid)
            .is_err()
    );
    assert!(
        adapter
            .record_local_start(&claimed, 1002, Duration::ZERO, &preparation)
            .is_err()
    );
    assert_eq!(snapshot(&store), original);
    let assessment = start_answer(&adapter, &active, claimed.clone(), 1001, Duration::ZERO);
    let restarted = restart(&store, &active);
    assert_eq!(
        adapter
            .record_local_start(&claimed, 1001, Duration::ZERO, &preparation)
            .unwrap(),
        ClaimOutcome::Stale
    );
    let RecoveryWork::Answered { claimed, evaluated } =
        restarted.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("a local answer must survive owner replacement");
    };
    assert!(
        restarted
            .load_decision_progress(&claimed)
            .unwrap()
            .attempts
            .is_empty()
    );
    assert_eq!(evaluated.attempts(), 0);
    let recovered = AssessmentAcceptance {
        claimed: claimed.clone(),
        evaluated,
        actions: assessment.actions,
    };
    assert_eq!(
        restarted.accept(&recovered).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_eq!(
        restarted
            .record_local_start(&claimed, 1001, Duration::ZERO, &preparation)
            .unwrap(),
        ClaimOutcome::Stale
    );
    assert_no_provider_rows(&store);
}

#[test]
fn local_zero_cooldown_at_1001ms_does_not_delay_the_next_candidate() {
    let (store, adapter, active) = local_fixture();
    assert!(active.compiled.trigger.cooldown.is_zero());
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider));
    for (sequence, id) in [(1, "one"), (2, "two")] {
        engine
            .apply_source_page(&active, &event(&store, sequence, id, "branch", 1))
            .unwrap();
    }
    assert_eq!(settle(&engine, &active, 1001).outcome, RunOutcome::Accepted);
    assert_eq!(settle(&engine, &active, 1001).outcome, RunOutcome::Accepted);
    assert_eq!(count(&store, "burn_check_cooldown"), 0);
    assert_eq!(count(&store, "burn_check_assessment_history"), 2);
    assert_no_provider_rows(&store);
}

#[test]
fn local_positive_cooldown_and_answer_survive_file_store_restart() {
    let directory = tempfile::tempdir().unwrap();
    let source = LOCAL_CHECK.replace("  tool_result:\n", "  tool_result:\n  cooldown: 2s\n");
    let (store, adapter, active) =
        fixture_with_definition(Arc::new(Store::open(directory.path()).unwrap()), &source);
    let allowed = Arc::new(AtomicBool::new(false));
    let gate = allowed.clone();
    let adapter = Arc::new(
        Arc::try_unwrap(adapter)
            .ok()
            .unwrap()
            .with_publication_gate(Arc::new(move |_| gate.load(Ordering::SeqCst))),
    );
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let second = apply(&adapter, &active, &event(&store, 2, "two", "branch", 1));
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider));
    engine.recover_pending().unwrap();
    assert_eq!(settle(&engine, &active, 1001).outcome, RunOutcome::Deferred);
    let original = saved_preparation(&store, &first);
    assert_eq!(original.started_at_unix_ms, 1001);
    assert_eq!(state(&store, &first), "answered");
    assert_no_provider_rows(&store);
    drop(engine);
    drop(adapter);
    drop(store);

    let store = Arc::new(Store::open(directory.path()).unwrap());
    let gate = allowed.clone();
    let adapter = Arc::new(
        restart(&store, &active)
            .with_publication_gate(Arc::new(move |_| gate.load(Ordering::SeqCst))),
    );
    store.lock().execute_batch("CREATE TRIGGER reject_second_start BEFORE UPDATE OF preparation_json ON burn_check_candidate BEGIN SELECT RAISE(ABORT, 'recovery must not start again'); END").unwrap();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider));
    engine.recover_pending().unwrap();
    assert_eq!(saved_preparation(&store, &first), original);
    assert_eq!(state(&store, &first), "answered");
    assert_eq!(count(&store, "burn_check_assessment_history"), 0);
    allowed.store(true, Ordering::SeqCst);
    engine.recover_pending().unwrap();
    assert_eq!(state(&store, &first), "accepted");
    assert_eq!(saved_preparation(&store, &first), original);
    assert_eq!(
        adapter.claim_due(&second, 3999, false).unwrap(),
        CandidateClaim::NotDue {
            retry_at_unix_ms: 4000
        }
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_second_start")
        .unwrap();
    assert_eq!(settle(&engine, &active, 4000).outcome, RunOutcome::Accepted);
    assert_eq!(count(&store, "burn_check_assessment_history"), 2);
    assert_no_provider_rows(&store);
}

#[test]
fn abandoned_local_claim_requeues_without_a_start_or_cooldown() {
    let (store, adapter, active) = local_fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let old = claim(&adapter, &candidate);
    let restarted = Arc::new(restart(&store, &active));
    assert_eq!(state(&store, &candidate), "pending");
    assert_eq!(count(&store, "burn_check_cooldown"), 0);
    let claimed = claim(&restarted, &candidate);
    assert!(
        restarted
            .load_decision_progress(&claimed)
            .unwrap()
            .attempts
            .is_empty()
    );
    assert!(restarted.load_preparation(&claimed).unwrap().is_none());
    assert_eq!(
        adapter
            .record_local_start(&old, 1001, Duration::ZERO, &preparation(1001))
            .unwrap(),
        ClaimOutcome::Stale
    );
    let assessment = start_answer(&restarted, &active, claimed, 1001, Duration::ZERO);
    assert_eq!(
        restarted.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_no_provider_rows(&store);
}

#[test]
fn publication_gate_runs_after_the_hook_without_store_or_registry_locks() {
    for local in [true, false] {
        let (store, adapter, active) = if local { local_fixture() } else { fixture() };
        let allowed = Arc::new(AtomicBool::new(true));
        let hook_seen = Arc::new(AtomicBool::new(false));
        let weak = Arc::new(Mutex::new(None::<std::sync::Weak<StoreCheckStore>>));
        let gate_weak = weak.clone();
        let gate_allowed = allowed.clone();
        let gate_seen = hook_seen.clone();
        let gate_store = store.clone();
        let adapter = Arc::new(
            Arc::try_unwrap(adapter)
                .ok()
                .unwrap()
                .with_publication_gate(Arc::new(move |active| {
                    assert_eq!(active.compiled.is_local(), local);
                    assert!(gate_seen.load(Ordering::SeqCst));
                    assert!(gate_store.connection.try_lock().is_ok());
                    let adapter = gate_weak
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .upgrade()
                        .unwrap();
                    assert!(adapter.activations.try_lock().is_ok());
                    gate_allowed.load(Ordering::SeqCst)
                })),
        );
        *weak.lock().unwrap() = Some(Arc::downgrade(&adapter));
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        let assessment = if local {
            start_answer(&adapter, &active, claimed, 1001, Duration::ZERO)
        } else {
            answer(&adapter, &active, claimed, 0.99)
        };
        let original = snapshot(&store);
        let hook_allowed = allowed.clone();
        let hook_seen = hook_seen.clone();
        store.set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == super::super::super::test_hooks::Stage::BeforeAcceptance {
                hook_seen.store(true, Ordering::SeqCst);
                hook_allowed.store(false, Ordering::SeqCst);
            }
        })));
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Deferred
        );
        assert_eq!(snapshot(&store), original);
        store.set_burn_check_test_hook(None);
        allowed.store(true, Ordering::SeqCst);
        CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider))
            .recover_pending()
            .unwrap();
        assert_eq!(state(&store, &candidate), "accepted");
        assert_eq!(count(&store, "burn_check_assessment_history"), 1);
        if local {
            assert_no_provider_rows(&store);
        }
    }
}

#[test]
fn local_recovery_isolates_invalid_saved_kinds_versions_results_and_budgets() {
    for corruption in [
        "missing",
        "model",
        "rejected",
        "version",
        "budget_counter",
        "budget_exhausted",
        "budget_limits",
        "budget_missing",
        "unknown_field",
        "answer",
        "attempt",
    ] {
        let (store, adapter, active) = local_fixture();
        let mut candidates = [
            apply(&adapter, &active, &event(&store, 1, "one", "branch-one", 1)),
            apply(&adapter, &active, &event(&store, 2, "two", "branch-two", 1)),
        ];
        candidates.sort_by_key(|candidate| candidate.fingerprint);
        let [bad, good] = candidates;
        start_answer(
            &adapter,
            &active,
            claim(&adapter, &bad),
            1001,
            Duration::ZERO,
        );
        start_answer(
            &adapter,
            &active,
            claim(&adapter, &good),
            1001,
            Duration::ZERO,
        );
        let mut saved = serde_json::to_value(PreparationRecord::Local(preparation(1001))).unwrap();
        match corruption {
            "missing" => saved = serde_json::Value::Null,
            "model" => {
                saved = serde_json::to_value(PreparationRecord::Ready(prepared_review(
                    "synthetic-model",
                    json!({}),
                    json!({}),
                    Default::default(),
                    EvaluationBudget::default(),
                )))
                .unwrap()
            }
            "rejected" => {
                saved = serde_json::to_value(PreparationRecord::Rejected(PreparationFailure {
                    code: antiburn_checks::fit::PreparationFailureCode::IncompatibleProfile,
                    detail: "Synthetic rejection.".into(),
                    report: None,
                    context: Default::default(),
                }))
                .unwrap()
            }
            "version" => saved["version"] = json!(2),
            "budget_counter" => saved["evaluation_budget"]["visited_items"] = json!(1001),
            "budget_exhausted" => saved["evaluation_budget"]["exhausted"] = json!(true),
            "budget_limits" => saved["evaluation_budget"]["max_items"] = json!(999),
            "budget_missing" => {
                saved.as_object_mut().unwrap().remove("evaluation_budget");
            }
            "unknown_field" => saved["unexpected"] = json!(true),
            "answer" => {
                store.lock().execute("UPDATE burn_check_candidate SET answer_json = '{\"version\":1,\"decisions\":[],\"attempts\":1}' WHERE candidate_id = ?1", [hex(&bad.fingerprint)]).unwrap();
            }
            "attempt" => {
                store.lock().execute(
                    "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, attempt_number, state, outcome, updated_at_epoch)
                     VALUES (?1, ?2, 1, 'settled', 'safe_transient', 1)",
                    params![format!("{}:1", hex(&bad.fingerprint)), hex(&bad.fingerprint)],
                ).unwrap();
            }
            _ => unreachable!(),
        }
        let saved = (!saved.is_null()).then(|| saved.to_string());
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE candidate_id = ?2",
                params![saved, hex(&bad.fingerprint)],
            )
            .unwrap();
        let original_progress = adapter.load_source_progress(&active).unwrap();
        let original_attempts = count(&store, "burn_check_evaluation_attempt");
        let restarted = Arc::new(restart(&store, &active));
        CheckEngine::new(restarted.clone(), Arc::new(NoLocalProvider))
            .recover_pending()
            .unwrap();
        assert_eq!(state(&store, &bad), "execution_failed", "{corruption}");
        assert_eq!(state(&store, &good), "accepted", "{corruption}");
        let diagnostic: String = store
            .lock()
            .query_row(
                "SELECT diagnostic_json FROM burn_check_candidate WHERE candidate_id = ?1",
                [hex(&bad.fingerprint)],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str::<EvaluationDiagnostic>(&diagnostic).unwrap();
        assert_eq!(
            count(&store, "burn_check_evaluation_attempt"),
            original_attempts
        );
        assert_eq!(count(&store, "burn_check_decision_usage"), 0);
        assert_eq!(count(&store, "burn_check_current_status"), 1);
        assert_eq!(
            restarted.load_source_progress(&active).unwrap(),
            original_progress
        );
        assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    }
}

#[test]
fn local_recovery_rejects_unknown_answer_fields_and_accepts_later_work() {
    let (store, adapter, active) = local_fixture();
    let mut candidates = [
        apply(&adapter, &active, &event(&store, 1, "one", "branch-one", 1)),
        apply(&adapter, &active, &event(&store, 2, "two", "branch-two", 1)),
    ];
    candidates.sort_by_key(|candidate| candidate.fingerprint);
    let [bad, good] = candidates;
    let assessment = start_answer(
        &adapter,
        &active,
        claim(&adapter, &bad),
        1001,
        Duration::ZERO,
    );
    start_answer(
        &adapter,
        &active,
        claim(&adapter, &good),
        1001,
        Duration::ZERO,
    );
    let saved_answer =
        json!({"version": 1, "decisions": [], "attempts": 0, "unexpected": true}).to_string();
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET answer_json = ?1 WHERE candidate_id = ?2",
            params![saved_answer, hex(&bad.fingerprint)],
        )
        .unwrap();
    let error = adapter.accept(&assessment).unwrap_err();
    assert!(error.message.contains("unknown field"));
    assert_eq!(count(&store, "burn_check_assessment_history"), 0);
    let original_progress = adapter.load_source_progress(&active).unwrap();
    let original_preparation = saved_preparation(&store, &bad);
    let restarted = Arc::new(restart(&store, &active));
    CheckEngine::new(restarted.clone(), Arc::new(NoLocalProvider))
        .recover_pending()
        .unwrap();
    assert_invalid_recovery(&store, &bad);
    assert_eq!(state(&store, &good), "accepted");
    assert_eq!(saved_preparation(&store, &bad), original_preparation);
    let retained_answer: String = store
        .lock()
        .query_row(
            "SELECT answer_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&bad.fingerprint)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_answer, saved_answer);
    assert_eq!(
        restarted.load_source_progress(&active).unwrap(),
        original_progress
    );
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    assert_no_provider_rows(&store);
}

#[test]
fn local_file_store_restart_preserves_prepared_attempts_and_rejects_only_the_bad_row() {
    let directory = tempfile::tempdir().unwrap();
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open(directory.path()).unwrap()),
        LOCAL_CHECK,
    );
    let mut candidates = [
        apply(&adapter, &active, &event(&store, 1, "one", "branch-one", 1)),
        apply(&adapter, &active, &event(&store, 2, "two", "branch-two", 1)),
    ];
    candidates.sort_by_key(|candidate| candidate.fingerprint);
    let [bad, good] = candidates;
    start_answer(
        &adapter,
        &active,
        claim(&adapter, &bad),
        1001,
        Duration::ZERO,
    );
    start_answer(
        &adapter,
        &active,
        claim(&adapter, &good),
        1001,
        Duration::ZERO,
    );
    store.lock().execute(
        "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, attempt_number, state, updated_at_epoch)
         VALUES (?1, ?2, 1, 'prepared', 1)",
        params![format!("{}:1", hex(&bad.fingerprint)), hex(&bad.fingerprint)],
    ).unwrap();
    let original_progress = adapter.load_source_progress(&active).unwrap();
    let original_preparation = saved_preparation(&store, &bad);
    let original_usage = store.burn_check_usage_summary().unwrap();
    drop(adapter);
    drop(store);

    let store = Arc::new(Store::open(directory.path()).unwrap());
    let restarted = Arc::new(restart(&store, &active));
    let read_attempt = || -> (String, u8, String, Option<String>, i64) {
        store.lock().query_row(
            "SELECT candidate_id, attempt_number, state, outcome, updated_at_epoch FROM burn_check_evaluation_attempt",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).unwrap()
    };
    let original_attempt = (hex(&bad.fingerprint), 1, "prepared".into(), None, 1);
    assert_eq!(read_attempt(), original_attempt);
    assert_eq!(state(&store, &bad), "answered");
    CheckEngine::new(restarted.clone(), Arc::new(NoLocalProvider))
        .recover_pending()
        .unwrap();
    assert_invalid_recovery(&store, &bad);
    assert_eq!(state(&store, &good), "accepted");
    assert_eq!(read_attempt(), original_attempt);
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 1);
    assert_eq!(count(&store, "burn_check_decision_usage"), 0);
    assert_eq!(store.burn_check_usage_summary().unwrap(), original_usage);
    assert_eq!(saved_preparation(&store, &bad), original_preparation);
    assert_eq!(
        restarted.load_source_progress(&active).unwrap(),
        original_progress
    );
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
}

#[test]
fn local_acceptance_checks_saved_preparation_inside_the_atomic_transaction() {
    for corruption in ["kind", "version", "budget"] {
        let (store, adapter, active) = local_fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = start_answer(
            &adapter,
            &active,
            claim(&adapter, &candidate),
            1001,
            Duration::ZERO,
        );
        let hook_store = store.clone();
        let id = hex(&candidate.fingerprint);
        store.set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == super::super::super::test_hooks::Stage::BeforeAcceptance {
                let mut saved = serde_json::to_value(PreparationRecord::Local(preparation(1001))).unwrap();
                match corruption {
                    "kind" => saved["status"] = json!("ready"),
                    "version" => saved["version"] = json!(2),
                    "budget" => saved["evaluation_budget"]["exhausted"] = json!(true),
                    _ => unreachable!(),
                }
                hook_store.lock().execute("UPDATE burn_check_candidate SET preparation_json = ?1 WHERE candidate_id = ?2", params![saved.to_string(), id]).unwrap();
            }
        })));
        assert!(adapter.accept(&assessment).is_err(), "{corruption}");
        store.set_burn_check_test_hook(None);
        assert_eq!(state(&store, &candidate), "answered");
        assert_eq!(count(&store, "burn_check_current_status"), 0);
        assert_eq!(count(&store, "burn_check_assessment_history"), 0);
        assert_eq!(count(&store, "burn_check_acceptance_watermark"), 0);
        assert_no_provider_rows(&store);
    }

    let (store, adapter, active) = local_fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = start_answer(
        &adapter,
        &active,
        claim(&adapter, &candidate),
        1001,
        Duration::ZERO,
    );
    let original = snapshot(&store);
    store.lock().execute_batch("CREATE TRIGGER reject_local_status BEFORE INSERT ON burn_check_current_status BEGIN SELECT RAISE(ABORT, 'injected local acceptance failure'); END").unwrap();
    let error = adapter.accept(&assessment).unwrap_err();
    assert!(error.message.contains("injected local acceptance failure"));
    assert_eq!(snapshot(&store), original);
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_local_status")
        .unwrap();
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
}

#[test]
fn local_acceptance_preserves_durable_master_authentication_and_transition_gates() {
    for (key, blocked, restored) in [
        ("internal:yamlBurnChecksDiscoveryEnabledV1", "false", "true"),
        (
            "internal:typesafeCredentialChangePendingV1",
            "true",
            "false",
        ),
        (
            "internal:smartChecksConnectionChangePendingV1",
            "synthetic-change",
            "",
        ),
        (
            "internal:smartChecksCredentialRemovalPendingV1",
            "synthetic-removal",
            "",
        ),
        ("internal:typesafeAuthRejectedV1", "true", "false"),
    ] {
        let (store, adapter, active) = local_fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = start_answer(
            &adapter,
            &active,
            claim(&adapter, &candidate),
            1001,
            Duration::from_secs(2),
        );
        let original = snapshot(&store);
        store.set_internal_value_checked(key, blocked).unwrap();
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Deferred,
            "{key}"
        );
        assert_eq!(snapshot(&store), original);
        store.set_internal_value_checked(key, restored).unwrap();
        CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider))
            .recover_pending()
            .unwrap();
        assert_eq!(state(&store, &candidate), "accepted");
        assert_no_provider_rows(&store);
    }
}

#[test]
fn local_source_definition_and_connection_changes_revoke_saved_answers() {
    for change in ["correction", "removal", "definition", "connection"] {
        let (store, adapter, active) = local_fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = start_answer(
            &adapter,
            &active,
            claim(&adapter, &candidate),
            1001,
            Duration::ZERO,
        );
        match change {
            "correction" => {
                let mut page = event(&store, 2, "one", "branch", 1);
                let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
                    unreachable!()
                };
                observation.version = EvidenceVersion("corrected-local-result".into());
                CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider))
                    .apply_source_page(&active, &page)
                    .unwrap();
            }
            "removal" => {
                store.lock().execute("INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence)
                    SELECT 2, 'removal', environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, '{}', 1, source_generation, claim_fence
                    FROM native_tool_result_publication WHERE publication_sequence = 1", []).unwrap();
                let page = SourceBatch {
                    events: vec![SourceEvent::Removal {
                        cursor: PublicationCursor(2),
                        identity: "one".into(),
                        revision: 1,
                        branch: BranchId("branch".into()),
                    }],
                    next: PublicationCursor(2),
                    has_more: false,
                };
                CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider))
                    .apply_source_page(&active, &page)
                    .unwrap();
            }
            "definition" => {
                let mut next = active.clone();
                next.id.generation = Generation(2);
                next.revision = [2; 32];
                adapter
                    .activate(
                        "retry",
                        &LifecycleTransition::Activated(next.clone()),
                        Some(&next),
                    )
                    .unwrap();
            }
            "connection" => {
                store
                    .cancel_unfinished_burn_check_assessments_for_connection_change()
                    .unwrap();
                store
                    .cancel_unfinished_burn_check_assessments_for_connection_change()
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Stale,
            "{change}"
        );
        assert_eq!(
            adapter
                .record_local_start(
                    &assessment.claimed,
                    1001,
                    Duration::ZERO,
                    &preparation(1001)
                )
                .unwrap(),
            ClaimOutcome::Stale
        );
        assert_eq!(count(&store, "burn_check_current_status"), 0);
        assert_no_provider_rows(&store);
    }
}

#[test]
fn local_no_write_preserves_status_evidence_and_time_through_pruning() {
    let (store, adapter, active) = local_fixture();
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = start_answer(
        &adapter,
        &active,
        claim(&adapter, &first),
        1001,
        Duration::ZERO,
    );
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    let status = || -> String {
        store.lock().query_row(
        "SELECT json_array(s.status, s.assessment_id, s.position, s.updated_at_epoch, s.evidence_valid, h.actions_json)
         FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
        [], |row| row.get(0),
    ).unwrap()
    };
    let original = status();
    let mut page = event(&store, 2, "two", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        unreachable!()
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        unreachable!()
    };
    message.value["tool"]["result"]["status"] = json!("ok");
    let second = apply(&adapter, &active, &page);
    let assessment = start_answer(
        &adapter,
        &active,
        claim(&adapter, &second),
        1001,
        Duration::ZERO,
    );
    assert_eq!(assessment.status_write(), None);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_eq!(status(), original);
    let watermark: u64 = store
        .lock()
        .query_row(
            "SELECT position FROM burn_check_acceptance_watermark",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(watermark, second.trigger_position.0);
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), now_epoch() + 1).unwrap();
    assert_eq!(status(), original);
    for (candidate, retained) in [(&first, true), (&second, false)] {
        let frozen: bool = store.lock().query_row("SELECT frozen_input_json <> '{}' FROM burn_check_candidate WHERE candidate_id = ?1", [hex(&candidate.fingerprint)], |row| row.get(0)).unwrap();
        assert_eq!(frozen, retained);
        assert_eq!(saved_preparation(&store, candidate), preparation(1001));
    }
    let mut correction = event(&store, 3, "one", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut correction.events[0] else {
        unreachable!()
    };
    observation.version = EvidenceVersion("invalidated-local-evidence".into());
    CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider))
        .apply_source_page(&active, &correction)
        .unwrap();
    let valid: (bool, bool) = store.lock().query_row("SELECT s.evidence_valid, h.evidence_valid FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(valid, (false, false));
    assert_no_provider_rows(&store);
}

#[test]
fn local_shared_action_budget_failure_keeps_cooldown_without_publishing_tentative_status() {
    let source = "name: Local budget\non:\n  tool_result:\n  cooldown: 2s\nactions:\n  - type: status\n    input: {result: failure, message: Tentative local failure.}\n  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {result: success}\n  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {result: success}\n";
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/local-budget")).unwrap()),
        source,
    );
    let mut page = event(&store, 1, "one", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        unreachable!()
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        unreachable!()
    };
    message.value["tool"]["arguments"] =
        json!({"items": (0..600).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()});
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider));
    engine.apply_source_page(&active, &page).unwrap();
    let settled = settle(&engine, &active, 1001);
    assert!(matches!(settled.outcome, RunOutcome::Failed(_)));
    assert_eq!(count(&store, "burn_check_current_status"), 0);
    assert_eq!(count(&store, "burn_check_acceptance_watermark"), 0);
    let (diagnostic, actions): (String, String) = store.lock().query_row(
        "SELECT c.diagnostic_json, h.actions_json FROM burn_check_candidate c JOIN burn_check_assessment_history h USING(candidate_id)",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    let diagnostic: EvaluationDiagnostic = serde_json::from_str(&diagnostic).unwrap();
    assert_eq!(
        diagnostic.code,
        antiburn_checks::engine::EvaluationFailureCode::ActionFailed
    );
    assert!(diagnostic.detail.contains("visited-item limit"));
    let actions: serde_json::Value = serde_json::from_str(&actions).unwrap();
    assert!(actions.get("outcomes").is_none());
    let until: u64 = store
        .lock()
        .query_row(
            "SELECT cooldown_until_epoch FROM burn_check_cooldown",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(until, 4);
    assert_no_provider_rows(&store);
}

#[test]
fn local_partial_membership_and_invalid_actions_leave_later_actions_independent() {
    let source = "name: Local independent actions\non:\n  tool_result:\nactions:\n  - type: status\n    if: session.requests[0].text == 'task'\n    input: {result: success}\n  - type: status\n    input: {result: '${{ message.tool.arguments.invalid }}'}\n  - type: status\n    input: {result: failure}\n";
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/local-independent-actions")).unwrap()),
        source,
    );
    let mut page = event(&store, 1, "one", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        unreachable!()
    };
    observation.context.value["session"] =
        json!({"requests": [{"text": "task"}], "requests_complete": false});
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        unreachable!()
    };
    message.value["tool"]["arguments"] = json!({"invalid": "other"});
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoLocalProvider));
    engine.apply_source_page(&active, &page).unwrap();
    assert_eq!(settle(&engine, &active, 1001).outcome, RunOutcome::Accepted);
    let (status, actions): (String, String) = store.lock().query_row(
        "SELECT s.status, h.actions_json FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(status, "failure");
    let actions: serde_json::Value = serde_json::from_str(&actions).unwrap();
    assert_eq!(
        actions["outcomes"][0]["outcome"],
        "SkippedConditionUnavailable"
    );
    assert!(actions["outcomes"][1]["outcome"].get("Failed").is_some());
    assert_no_provider_rows(&store);
}

#[test]
fn replaced_model_activations_still_record_incurred_answers_and_failures_without_publication() {
    for answered in [false, true] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        adapter
            .dispatch_review(&claimed, 1001, Duration::ZERO, 1)
            .unwrap();
        let mut next = active.clone();
        next.id.generation = Generation(2);
        next.revision = [2; 32];
        adapter
            .activate(
                "retry",
                &LifecycleTransition::Activated(next.clone()),
                Some(&next),
            )
            .unwrap();
        if answered {
            let result = review_result(&active, 1, 0.99);
            assert_eq!(
                adapter
                    .record_decision_answer(&claimed, &review_attempt(1), &result)
                    .unwrap(),
                ClaimOutcome::Stale
            );
            let evaluated = EvaluatedAssessment::new(
                &active.compiled.decisions,
                &active.compiled.question_requirements(),
                vec![result],
            )
            .unwrap();
            let actions = plan_status_actions(
                &active.compiled.actions,
                &candidate.context.value,
                &evaluated,
                None,
            )
            .unwrap();
            assert_eq!(
                adapter
                    .accept(&AssessmentAcceptance {
                        claimed,
                        evaluated,
                        actions
                    })
                    .unwrap(),
                AcceptanceOutcome::Stale
            );
        } else {
            assert_eq!(
                adapter
                    .fail_review(
                        &claimed,
                        1,
                        AttemptFailure::Permanent,
                        &transient_diagnostic()
                    )
                    .unwrap(),
                ClaimOutcome::Stale
            );
        }
        let (attempt_state, outcome): (String, String) = store
            .lock()
            .query_row(
                "SELECT state, outcome FROM burn_check_evaluation_attempt",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempt_state, "settled");
        assert_eq!(outcome, if answered { "answer" } else { "permanent" });
        assert_eq!(state(&store, &candidate), "cancelled");
        assert_eq!(count(&store, "burn_check_current_status"), 0);
    }
}
