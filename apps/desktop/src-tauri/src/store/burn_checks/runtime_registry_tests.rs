use super::*;

const LOCAL_CHECK: &str = "name: Local registry\non:\n  tool_result:\nactions:\n  - type: status\n    input: {result: failure}\n";

struct RegistrationPause {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl Drop for RegistrationPause {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

fn pause_unchanged_registration(
    adapter: &Arc<StoreCheckStore>,
    active: &EffectiveActivation,
) -> (
    RegistrationPause,
    std::thread::JoinHandle<Result<(), StoreError>>,
) {
    let (entered, reached) = std::sync::mpsc::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let resume = Mutex::new(resume);
    let paused = AtomicBool::new(false);
    adapter
        .store
        .set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == crate::store::burn_checks::test_hooks::Stage::BeforeActivationRegistration
                && !paused.swap(true, Ordering::SeqCst)
            {
                entered.send(()).unwrap();
                resume.lock().unwrap().recv().unwrap();
            }
        })));
    let pause = RegistrationPause {
        entered: reached,
        release,
    };
    let adapter = adapter.clone();
    let active = active.clone();
    let task = std::thread::spawn(move || {
        adapter.activate(
            &active.id.definition_id,
            &LifecycleTransition::Unchanged(active.clone()),
            Some(&active),
        )
    });
    pause.entered.recv_timeout(Duration::from_secs(10)).unwrap();
    (pause, task)
}

fn assessment(active: &EffectiveActivation, claimed: ClaimedCandidate) -> AssessmentAcceptance {
    let evaluated = if active.compiled.is_local() {
        EvaluatedAssessment::local()
    } else {
        EvaluatedAssessment::new(
            &active.compiled.decisions,
            &active.compiled.question_requirements(),
            vec![review_result(active, 1, 0.99)],
        )
        .unwrap()
    };
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

#[test]
fn unchanged_reconciliation_keeps_model_dispatch_and_in_flight_answers_current() {
    let (store, adapter, active) = fixture();
    let first = apply(&adapter, &active, &event(&store, 1, "one", "answer", 1));
    let first_claim = claim(&adapter, &first);
    adapter
        .dispatch_review(&first_claim, 100_000, Duration::ZERO, 1)
        .unwrap();
    let second = apply(&adapter, &active, &event(&store, 2, "two", "dispatch", 1));
    let second_claim = claim(&adapter, &second);
    let (pause, task) = pause_unchanged_registration(&adapter, &active);
    assert!(adapter.activations.try_lock().is_ok());
    assert!(store.connection.try_lock().is_ok());
    assert_eq!(
        adapter.registered_activation(&active.id),
        Some(active.clone())
    );
    assert_eq!(
        adapter
            .dispatch_review(&second_claim, 100_000, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Current
    );
    let first_answer = assessment(&active, first_claim);
    assert_eq!(
        adapter
            .record_decision_answer(
                &first_answer.claimed,
                &review_attempt(1),
                &review_result(&active, 1, 0.99),
            )
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        adapter.accept(&first_answer).unwrap(),
        AcceptanceOutcome::Accepted
    );
    drop(pause);
    task.join().unwrap().unwrap();
    store.set_burn_check_test_hook(None);
    let second_answer = assessment(&active, second_claim);
    assert_eq!(
        adapter
            .record_decision_answer(
                &second_answer.claimed,
                &review_attempt(1),
                &review_result(&active, 1, 0.99),
            )
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        adapter.accept(&second_answer).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_eq!(adapter.registered_activation(&active.id), Some(active));
}

#[test]
fn unchanged_reconciliation_keeps_local_start_and_later_branch_work_current() {
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/local-registry")).unwrap()),
        LOCAL_CHECK,
    );
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let first_claim = claim(&adapter, &first);
    let (pause, task) = pause_unchanged_registration(&adapter, &active);
    assert_eq!(
        adapter
            .record_local_start(
                &first_claim,
                100_000,
                Duration::ZERO,
                &LocalPreparation::new(100_000),
            )
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        adapter.accept(&assessment(&active, first_claim)).unwrap(),
        AcceptanceOutcome::Accepted
    );
    drop(pause);
    task.join().unwrap().unwrap();
    store.set_burn_check_test_hook(None);
    let second = apply(&adapter, &active, &event(&store, 2, "two", "branch", 1));
    assert!(matches!(
        adapter.claim_due(&second, 100_001, false).unwrap(),
        CandidateClaim::Claimed(_)
    ));
    let attempts: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 0);
}

#[test]
fn activation_replacement_and_deactivation_reject_old_claims_and_answers() {
    for local in [false, true] {
        for replacement in [false, true] {
            let (store, adapter, active) = fixture_with_definition(
                Arc::new(Store::open_in_memory(Path::new("/synthetic/revoked-registry")).unwrap()),
                if local { LOCAL_CHECK } else { CHECK },
            );
            let first = apply(&adapter, &active, &event(&store, 1, "one", "started", 1));
            let claimed = claim(&adapter, &first);
            let pending = apply(&adapter, &active, &event(&store, 2, "two", "pending", 1));
            if local {
                adapter
                    .record_local_start(
                        &claimed,
                        100_000,
                        Duration::ZERO,
                        &LocalPreparation::new(100_000),
                    )
                    .unwrap();
            } else {
                adapter
                    .dispatch_review(&claimed, 100_000, Duration::ZERO, 1)
                    .unwrap();
            }
            let answer = assessment(&active, claimed);
            let mut next = active.clone();
            next.id.generation = Generation(2);
            next.revision = [2; 32];
            let transition = if replacement {
                LifecycleTransition::Activated(next.clone())
            } else {
                LifecycleTransition::Deactivated {
                    revision: active.revision,
                    project_root: active.project_root.clone(),
                    scope: active.definition_scope,
                    source_paths: active.source_paths.clone(),
                    inactive_kind: InactiveKind::Paused,
                    next_generation: Generation(2),
                }
            };
            adapter
                .activate("retry", &transition, replacement.then_some(&next))
                .unwrap();
            assert!(adapter.registered_activation(&active.id).is_none());
            assert_eq!(
                adapter.registered_activation(&next.id),
                replacement.then_some(next.clone())
            );
            assert!(adapter.register_activation(&active).is_err());
            assert!(adapter.registered_activation(&active.id).is_none());
            assert!(matches!(
                adapter.claim_due(&pending, 100_001, true).unwrap(),
                CandidateClaim::Stale
            ));
            if local {
                assert_eq!(
                    adapter
                        .record_local_start(
                            &answer.claimed,
                            100_000,
                            Duration::ZERO,
                            &LocalPreparation::new(100_000),
                        )
                        .unwrap(),
                    ClaimOutcome::Stale
                );
            } else {
                assert_eq!(
                    adapter
                        .record_dispatch_intent(
                            &answer.claimed,
                            100_001,
                            Duration::ZERO,
                            &review_attempt(1),
                        )
                        .unwrap(),
                    ClaimOutcome::Stale
                );
                assert_eq!(
                    adapter
                        .record_decision_answer(
                            &answer.claimed,
                            &review_attempt(1),
                            &review_result(&active, 1, 0.99),
                        )
                        .unwrap(),
                    ClaimOutcome::Stale
                );
            }
            assert_eq!(adapter.accept(&answer).unwrap(), AcceptanceOutcome::Stale);
            let writes: i64 = store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(writes, 0);
        }
    }
}

#[test]
fn failed_activation_validation_keeps_registry_but_rejects_durably_revoked_work() {
    let (store, adapter, active) = fixture();
    let first = apply(&adapter, &active, &event(&store, 1, "one", "started", 1));
    let claimed = claim(&adapter, &first);
    adapter
        .dispatch_review(&claimed, 100_000, Duration::ZERO, 1)
        .unwrap();
    let pending = apply(&adapter, &active, &event(&store, 2, "two", "pending", 1));
    let dispatch = apply(&adapter, &active, &event(&store, 3, "three", "dispatch", 1));
    let dispatch_claim = claim(&adapter, &dispatch);
    adapter
        .record_preparation(
            &dispatch_claim,
            &PreparationRecord::Ready(prepared_review(
                "fixture-v1",
                json!({}),
                json!({"exact": "saved"}),
                Default::default(),
                EvaluationBudget::default(),
            )),
        )
        .unwrap();
    let acceptance_candidate = apply(
        &adapter,
        &active,
        &event(&store, 4, "four", "acceptance", 1),
    );
    let saved_answer = answer(
        &adapter,
        &active,
        claim(&adapter, &acceptance_candidate),
        0.99,
    );
    let in_flight_answer = assessment(&active, claimed);
    let (pause, task) = pause_unchanged_registration(&adapter, &active);
    store
        .lock()
        .execute("UPDATE burn_check_activation SET enabled = 0", [])
        .unwrap();
    drop(pause);
    let error = task.join().unwrap().unwrap_err();
    store.set_burn_check_test_hook(None);
    assert!(error.message.contains("durable generation and revision"));
    assert_eq!(
        adapter.registered_activation(&active.id),
        Some(active.clone())
    );
    let state = |candidate: &FrozenCandidate| -> String {
        store
            .lock()
            .query_row(
                "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
                [hex(&candidate.fingerprint)],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(state(&pending), "pending");
    assert!(matches!(
        adapter.claim_due(&pending, 100_001, true).unwrap(),
        CandidateClaim::Stale
    ));
    assert_eq!(state(&dispatch), "claimed");
    assert_eq!(
        adapter
            .record_dispatch_intent(&dispatch_claim, 100_001, Duration::ZERO, &review_attempt(1),)
            .unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(state(&first), "dispatching");
    assert_eq!(
        adapter
            .record_decision_answer(
                &in_flight_answer.claimed,
                &review_attempt(1),
                &review_result(&active, 1, 0.99),
            )
            .unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(state(&acceptance_candidate), "answered");
    assert_eq!(
        adapter.accept(&saved_answer).unwrap(),
        AcceptanceOutcome::Stale
    );
    let writes: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_current_status",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(writes, 0);
}
