use super::*;
use antiburn_checks::engine::EvaluationFailureCode;
use antiburn_checks::expression_eval::EvaluationBudget;
use antiburn_checks::fit::{FitReport, PreparationFailureCode};

struct NoRecoveryDispatch;

impl DecisionEvaluator for NoRecoveryDispatch {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<DecisionInput>,
        _: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        panic!("saved recovery must not prepare another provider request")
    }

    fn validate_prepared(
        &self,
        active: &EffectiveActivation,
        prepared: &PreparedAssessment,
        purpose: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        RecordingEvaluator(Mutex::new(Vec::new())).validate_prepared(active, prepared, purpose)
    }
    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        _: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        panic!("saved recovery must not resend a provider request")
    }
}

fn candidate_pair(
    store: &Store,
    adapter: &Arc<StoreCheckStore>,
    active: &EffectiveActivation,
) -> [FrozenCandidate; 2] {
    let mut candidates = [
        apply(adapter, active, &event(store, 1, "one", "branch-one", 1)),
        apply(adapter, active, &event(store, 2, "two", "branch-two", 1)),
    ];
    candidates.sort_by_key(|candidate| candidate.fingerprint);
    candidates
}

fn restart(store: &Arc<Store>, active: &EffectiveActivation) -> Arc<StoreCheckStore> {
    let adapter = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "restart-owner".into(),
            active.id.scope.clone(),
        )
        .unwrap(),
    );
    adapter.register_activation(active).unwrap();
    adapter
}

fn attempts(store: &Store) -> Vec<String> {
    store
        .lock()
        .prepare(
            "SELECT json_array(attempt_id, attempt_number, state, outcome, response_json)
             FROM burn_check_evaluation_attempt ORDER BY attempt_id",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn record_usage(store: &Store, candidate: &FrozenCandidate) {
    store
        .record_burn_check_decision_usage(
            &hex(&candidate.fingerprint),
            &review_attempt(1),
            &candidate.activation.definition_id,
            ("custom-systemone", "synthetic-model"),
            Some(antiburn_local::analysis::jev::JevUsage {
                input_tokens: 7,
                output_tokens: 3,
            }),
            100,
        )
        .unwrap();
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

fn assert_evaluation_failure(
    store: &Store,
    candidate: &FrozenCandidate,
    code: EvaluationFailureCode,
) {
    assert_eq!(candidate_state(store, candidate), "execution_failed");
    let diagnostic: String = store
        .lock()
        .query_row(
            "SELECT diagnostic_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&candidate.fingerprint)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<EvaluationDiagnostic>(&diagnostic)
            .unwrap()
            .code,
        code
    );
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

fn save_preparation(adapter: &StoreCheckStore, claimed: &ClaimedCandidate) -> serde_json::Value {
    let prepared = PreparationRecord::Ready(prepared_review(
        "synthetic-recovery-v1",
        json!({"original": "input"}),
        json!({"original": "request"}),
        FitReport::default(),
        EvaluationBudget::default(),
    ));
    adapter.record_preparation(claimed, &prepared).unwrap();
    serde_json::to_value(prepared).unwrap()
}

fn corrupt_preparation(store: &Store, candidate: &FrozenCandidate, mut saved: serde_json::Value) {
    saved["evaluation_budget"]["visited_items"] =
        json!(saved["evaluation_budget"]["max_items"].as_u64().unwrap() + 1);
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE candidate_id = ?2",
            params![saved.to_string(), hex(&candidate.fingerprint)],
        )
        .unwrap();
}

#[test]
fn unstarted_model_recovery_settles_legacy_prepared_attempts_without_sending() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let original_preparation = save_preparation(&adapter, &claimed);
    store.lock().execute(
        "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, attempt_number, state, updated_at_epoch)
         VALUES (?1, ?2, 1, 'prepared', 1)",
        params![format!("{}:1", hex(&candidate.fingerprint)), hex(&candidate.fingerprint)],
    ).unwrap();
    let original_progress = adapter.load_source_progress(&active).unwrap();
    let restarted = restart(&store, &active);
    assert_eq!(attempts(&store).len(), 1);
    assert_eq!(candidate_state(&store, &candidate), "execution_failed");
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
                [hex(&candidate.fingerprint)],
                |row| row.get::<_, String>(0)
            )
            .map(|saved| serde_json::from_str::<serde_json::Value>(&saved).unwrap())
            .unwrap(),
        original_preparation
    );
    assert_eq!(
        restarted.load_source_progress(&active).unwrap(),
        original_progress
    );
}

#[test]
fn invalid_saved_answers_settle_only_the_bad_row_and_recover_later_answers() {
    for corruption in ["empty", "declaration", "probabilities", "shape", "missing"] {
        let (store, adapter, active) = fixture();
        let [bad, good] = candidate_pair(&store, &adapter, &active);
        let assessment = answer(&adapter, &active, claim(&adapter, &bad), 0.99);
        answer(&adapter, &active, claim(&adapter, &good), 0.99);
        let mut saved = serde_json::to_value(assessment.evaluated).unwrap();
        match corruption {
            "empty" => saved["decisions"] = json!([]),
            "declaration" => saved["decisions"][0]["answers"][0]["name"] = json!("other"),
            "probabilities" => {
                saved["decisions"][0]["answers"][0]["result"]["probabilities"] =
                    json!({"yes": 1.5, "no": -0.5});
            }
            "shape" => saved = json!({"choices": "invalid", "attempts": 1}),
            "missing" => {}
            _ => unreachable!(),
        }
        let saved = (corruption != "missing").then(|| saved.to_string());
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET answer_json = ?1 WHERE candidate_id = ?2",
                params![saved, hex(&bad.fingerprint)],
            )
            .unwrap();
        record_usage(&store, &bad);
        let original_attempts = attempts(&store);
        let original_usage = store.burn_check_usage_summary().unwrap();
        let original_progress = adapter.load_source_progress(&active).unwrap();
        let restarted = restart(&store, &active);
        let engine = CheckEngine::new(restarted.clone(), Arc::new(NoRecoveryDispatch));
        engine.recover_pending().unwrap();
        assert_evaluation_failure(&store, &bad, EvaluationFailureCode::InvalidDecision);
        assert_eq!(candidate_state(&store, &good), "accepted", "{corruption}");
        assert_eq!(attempts(&store), original_attempts);
        assert_eq!(store.burn_check_usage_summary().unwrap(), original_usage);
        assert_eq!(
            restarted.load_source_progress(&active).unwrap(),
            original_progress
        );
        assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    }
}

#[test]
fn an_invalid_answer_only_page_does_not_hide_later_recovery_work() {
    let (store, adapter, active) = fixture();
    let [bad, good] = candidate_pair(&store, &adapter, &active);
    answer(&adapter, &active, claim(&adapter, &bad), 0.99);
    answer(&adapter, &active, claim(&adapter, &good), 0.99);
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET answer_json = '{}' WHERE candidate_id = ?1",
            [hex(&bad.fingerprint)],
        )
        .unwrap();
    let restarted = restart(&store, &active);
    let records = restarted.load_recovery(None, 1).unwrap();
    assert_eq!(records.len(), 1);
    let RecoveryWork::Answered { claimed, .. } = &records[0].work else {
        panic!("the valid saved answer must remain recoverable");
    };
    assert_eq!(claimed.id, CandidateId::from(&good));
    assert_evaluation_failure(&store, &bad, EvaluationFailureCode::InvalidDecision);
    assert_eq!(candidate_state(&store, &good), "answered");
}

#[test]
fn an_invalid_saved_budget_does_not_block_later_answered_recovery() {
    let (store, adapter, active) = fixture();
    let [bad, good] = candidate_pair(&store, &adapter, &active);
    let claimed = claim(&adapter, &bad);
    let saved = save_preparation(&adapter, &claimed);
    answer(&adapter, &active, claimed, 0.99);
    answer(&adapter, &active, claim(&adapter, &good), 0.99);
    corrupt_preparation(&store, &bad, saved);
    record_usage(&store, &bad);
    let original_attempts = attempts(&store);
    let original_usage = store.burn_check_usage_summary().unwrap();
    let restarted = restart(&store, &active);
    let RecoveryWork::Answered { claimed, .. } =
        restarted.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("the valid later answer must remain recoverable");
    };
    assert_eq!(claimed.id, CandidateId::from(&good));
    let engine = CheckEngine::new(restarted, Arc::new(NoRecoveryDispatch));
    engine.recover_pending().unwrap();
    assert_evaluation_failure(&store, &bad, EvaluationFailureCode::InvalidDecision);
    assert_eq!(candidate_state(&store, &good), "accepted");
    assert_eq!(attempts(&store), original_attempts);
    assert_eq!(store.burn_check_usage_summary().unwrap(), original_usage);
}

#[test]
fn an_invalid_saved_budget_settles_a_safe_retry_without_resend() {
    let (store, adapter, active) = fixture();
    let [bad, good] = candidate_pair(&store, &adapter, &active);
    let claimed = claim(&adapter, &bad);
    let saved = save_preparation(&adapter, &claimed);
    adapter
        .dispatch_review(&claimed, 1_000, Duration::ZERO, 1)
        .unwrap();
    adapter
        .fail_review(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    answer(&adapter, &active, claim(&adapter, &good), 0.99);
    corrupt_preparation(&store, &bad, saved);
    record_usage(&store, &bad);
    let original_attempts = attempts(&store);
    let original_usage = store.burn_check_usage_summary().unwrap();
    let restarted = restart(&store, &active);
    let engine = CheckEngine::new(restarted.clone(), Arc::new(NoRecoveryDispatch));
    assert_eq!(engine.recover_pending().unwrap().candidates, 0);
    assert_eq!(candidate_state(&store, &good), "accepted");
    assert!(matches!(
        engine.start_one_due(&active, 100_000).unwrap(),
        StartOutcome::NotDue
    ));
    assert_eq!(candidate_state(&store, &bad), "not_assessed");
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&bad.fingerprint)],
            |row| row.get(0),
        )
        .unwrap();
    assert!(antiburn_checks::preparation::decode_preparation_record(saved.as_bytes()).is_err());
    assert_eq!(attempts(&store), original_attempts);
    assert_eq!(store.burn_check_usage_summary().unwrap(), original_usage);
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
}

#[test]
fn preparation_json_decode_failures_do_not_hide_database_read_errors() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = '{}' WHERE candidate_id = ?1",
            [hex(&candidate.fingerprint)],
        )
        .unwrap();
    let Some(PreparationRecord::Rejected(failure)) = adapter.load_preparation(&claimed).unwrap()
    else {
        panic!("invalid preparation JSON must produce a typed rejection");
    };
    assert_eq!(failure.code, PreparationFailureCode::IncompatibleProfile);
    store
        .lock()
        .execute_batch(
            "ALTER TABLE burn_check_candidate RENAME COLUMN preparation_json TO unreadable_preparation_json",
        )
        .unwrap();
    let error = adapter.load_preparation(&claimed).unwrap_err();
    assert!(error.message.contains("no such column: preparation_json"));
    assert_eq!(candidate_state(&store, &candidate), "claimed");
}

#[test]
fn database_failure_when_rejecting_a_saved_answer_still_aborts_recovery() {
    let (store, adapter, active) = fixture();
    let [bad, good] = candidate_pair(&store, &adapter, &active);
    answer(&adapter, &active, claim(&adapter, &bad), 0.99);
    answer(&adapter, &active, claim(&adapter, &good), 0.99);
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET answer_json = '{}' WHERE candidate_id = ?1",
            [hex(&bad.fingerprint)],
        )
        .unwrap();
    let restarted = restart(&store, &active);
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_recovery BEFORE INSERT ON burn_check_assessment_history
             BEGIN SELECT RAISE(ABORT, 'injected recovery write failure'); END",
        )
        .unwrap();
    let engine = CheckEngine::new(restarted, Arc::new(NoRecoveryDispatch));
    let error = engine.recover_pending().unwrap_err();
    assert!(error.message.contains("injected recovery write failure"));
    assert_eq!(candidate_state(&store, &bad), "answered");
    assert_eq!(candidate_state(&store, &good), "answered");
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_recovery")
        .unwrap();
    engine.recover_pending().unwrap();
    assert_evaluation_failure(&store, &bad, EvaluationFailureCode::InvalidDecision);
    assert_eq!(candidate_state(&store, &good), "accepted");
}

#[test]
fn mismatched_frozen_activation_settles_only_the_bad_first_recovery_row() {
    for corruption in ["generation", "definition", "registered_definition", "scope"] {
        let (store, adapter, active) = fixture();
        let other = if corruption == "registered_definition" {
            let mut other = active.clone();
            other.id.definition_id = "other-check".into();
            adapter
                .activate(
                    "other-check",
                    &LifecycleTransition::Activated(other.clone()),
                    Some(&other),
                )
                .unwrap();
            Some(other)
        } else {
            None
        };
        let [bad, good] = candidate_pair(&store, &adapter, &active);
        answer(&adapter, &active, claim(&adapter, &bad), 0.99);
        answer(&adapter, &active, claim(&adapter, &good), 0.99);
        let original_attempts = attempts(&store);
        let original_progress = adapter.load_source_progress(&active).unwrap();
        let mut frozen = serde_json::to_value(&bad).unwrap();
        match corruption {
            "generation" => frozen["activation"]["generation"] = json!(999),
            "definition" | "registered_definition" => {
                frozen["activation"]["definition_id"] = json!("other-check")
            }
            "scope" => frozen["activation"]["scope"]["session_cwd"] = json!("/other-project"),
            _ => unreachable!(),
        }
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET frozen_input_json = ?1 WHERE candidate_id = ?2",
                params![frozen.to_string(), hex(&bad.fingerprint)],
            )
            .unwrap();
        let restarted = restart(&store, &active);
        if let Some(other) = &other {
            restarted.register_activation(other).unwrap();
        }
        let records = restarted.load_recovery(None, 1).unwrap();
        assert_eq!(records.len(), 1, "{corruption}");
        let RecoveryWork::Answered { claimed, .. } = &records[0].work else {
            panic!("the later valid row must remain recoverable");
        };
        assert_eq!(claimed.id, CandidateId::from(&good));
        assert_evaluation_failure(&store, &bad, EvaluationFailureCode::InvalidDecision);
        CheckEngine::new(restarted.clone(), Arc::new(NoRecoveryDispatch))
            .recover_pending()
            .unwrap();
        assert_eq!(candidate_state(&store, &good), "accepted", "{corruption}");
        assert_eq!(attempts(&store), original_attempts);
        assert_eq!(
            restarted.load_source_progress(&active).unwrap(),
            original_progress
        );
        assert!(restarted.load_recovery(None, 1).unwrap().is_empty());
    }
}

#[test]
fn unregistered_durable_activation_remains_a_recovery_setup_error() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    let restarted = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "unregistered-owner".into(),
            active.id.scope.clone(),
        )
        .unwrap(),
    );
    let error = restarted.load_recovery(None, 1).unwrap_err();
    assert!(error.message.contains("register all durable activations"));
    assert_eq!(candidate_state(&store, &candidate), "answered");
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    restarted.register_activation(&active).unwrap();
    CheckEngine::new(restarted, Arc::new(NoRecoveryDispatch))
        .recover_pending()
        .unwrap();
    assert_eq!(candidate_state(&store, &candidate), "accepted");
}

struct RecoveryPause {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl RecoveryPause {
    fn wait(&self) {
        self.entered.recv_timeout(Duration::from_secs(10)).unwrap();
    }
}

impl Drop for RecoveryPause {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

fn pause_recovery(
    store: &Store,
    expected: crate::store::burn_checks::test_hooks::Stage,
) -> RecoveryPause {
    let (entered, reached) = std::sync::mpsc::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let resume = Mutex::new(resume);
    let paused = AtomicBool::new(false);
    store.set_burn_check_test_hook(Some(Arc::new(move |stage| {
        if stage == expected && !paused.swap(true, Ordering::SeqCst) {
            entered.send(()).unwrap();
            resume.lock().unwrap().recv().unwrap();
        }
    })));
    RecoveryPause {
        entered: reached,
        release,
    }
}

fn saved_claim(
    store: &Store,
    candidate: &FrozenCandidate,
) -> (String, Option<String>, u64, Option<String>) {
    store.lock().query_row(
        "SELECT state, claim_owner, claim_fence, answer_json FROM burn_check_candidate WHERE candidate_id = ?1",
        [hex(&candidate.fingerprint)], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap()
}

#[test]
fn recovery_page_cannot_reject_a_pending_candidate_that_answers_after_paging() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let pause = pause_recovery(
        &store,
        crate::store::burn_checks::test_hooks::Stage::AfterRecoveryPage,
    );
    let recovery = adapter.clone();
    let task = std::thread::spawn(move || recovery.load_recovery(None, 10));
    pause.wait();
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    record_usage(&store, &candidate);
    let saved = saved_claim(&store, &candidate);
    let original_attempts = attempts(&store);
    let original_usage = store.burn_check_usage_summary().unwrap();
    drop(pause);
    task.join().unwrap().unwrap();
    store.set_burn_check_test_hook(None);
    assert_eq!(saved_claim(&store, &candidate), saved);
    assert_eq!(attempts(&store), original_attempts);
    assert_eq!(store.burn_check_usage_summary().unwrap(), original_usage);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
}

#[test]
fn recovery_page_cannot_reject_a_newer_fence_at_the_same_answered_state() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET answer_json = '{}' WHERE candidate_id = ?1",
            [hex(&candidate.fingerprint)],
        )
        .unwrap();
    let pause = pause_recovery(
        &store,
        crate::store::burn_checks::test_hooks::Stage::AfterRecoveryPage,
    );
    let recovery = adapter.clone();
    let task = std::thread::spawn(move || recovery.load_recovery(None, 10));
    pause.wait();
    let newer = restart(&store, &active);
    let saved = saved_claim(&store, &candidate);
    let original_attempts = attempts(&store);
    drop(pause);
    assert!(task.join().unwrap().unwrap().is_empty());
    store.set_burn_check_test_hook(None);
    assert_eq!(saved_claim(&store, &candidate), saved);
    assert_eq!(attempts(&store), original_attempts);
    let history: u64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_assessment_history",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history, 0);
    assert!(newer.load_recovery(None, 10).unwrap().is_empty());
    assert_evaluation_failure(&store, &candidate, EvaluationFailureCode::InvalidDecision);
}

fn replace_answer_in(
    connection: &mut rusqlite::Connection,
    candidate: &FrozenCandidate,
    active: &EffectiveActivation,
) {
    let answer = review_result(active, 1, 0.01);
    let evaluated = EvaluatedAssessment::new(
        &active.compiled.decisions,
        &active.compiled.question_requirements(),
        vec![answer.clone()],
    )
    .unwrap();
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "UPDATE burn_check_evaluation_attempt SET response_json = ?2 WHERE candidate_id = ?1",
        params![
            hex(&candidate.fingerprint),
            serde_json::to_string(&answer).unwrap()
        ],
    )
    .unwrap();
    tx.execute(
        "UPDATE burn_check_candidate SET answer_json = ?2, claim_fence = claim_fence + 1 WHERE candidate_id = ?1",
        params![hex(&candidate.fingerprint), serde_json::to_string(&evaluated).unwrap()],
    ).unwrap();
    tx.commit().unwrap();
}

#[test]
fn recovery_cannot_mix_old_decision_progress_with_a_new_answer() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    record_usage(&store, &candidate);
    let original_usage = store.burn_check_usage_summary().unwrap();
    let pause = pause_recovery(
        &store,
        crate::store::burn_checks::test_hooks::Stage::AfterRecoveryProgress,
    );
    let recovery = adapter.clone();
    let task = std::thread::spawn(move || recovery.load_recovery(None, 10));
    pause.wait();
    let writer = match store.connection.try_lock() {
        Ok(mut connection) => {
            // An unlocked reader permits this write before its next answer read.
            replace_answer_in(&mut connection, &candidate, &active);
            None
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            let store = store.clone();
            let candidate = candidate.clone();
            let active = active.clone();
            Some(std::thread::spawn(move || {
                replace_answer_in(&mut store.lock(), &candidate, &active)
            }))
        }
        Err(error) => panic!("Store lock failed: {error}"),
    };
    drop(pause);
    let records = task.join().unwrap().unwrap();
    if let Some(writer) = writer {
        writer.join().unwrap();
    }
    store.set_burn_check_test_hook(None);
    assert_eq!(candidate_state(&store, &candidate), "answered");
    assert_eq!(records.len(), 1);
    let record = records.into_iter().next().unwrap();
    let RecoveryWork::Answered { claimed, evaluated } = record.work else {
        panic!("the consistent saved answer must remain recoverable");
    };
    let actions = plan_status_actions(
        &active.compiled.actions,
        &candidate.context.value,
        &evaluated,
        claimed.prior_status,
    )
    .unwrap();
    let saved = saved_claim(&store, &candidate);
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
    assert_eq!(saved_claim(&store, &candidate), saved);
    CheckEngine::new(adapter, Arc::new(NoRecoveryDispatch))
        .recover_pending()
        .unwrap();
    assert_eq!(candidate_state(&store, &candidate), "accepted");
    assert_eq!(store.burn_check_usage_summary().unwrap(), original_usage);
}
