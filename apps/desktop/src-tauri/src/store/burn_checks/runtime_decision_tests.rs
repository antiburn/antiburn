use super::*;
use antiburn_checks::decision::Question;
use antiburn_checks::results::{NoulResult, ScoreResult};

const MULTI_CHECK: &str = "name: Multi\non:\n  tool_result:\n  cooldown: 60s\ndecisions:\n  review:\n    input: {activity: '${{ message.tool.result.status }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\n  confirm:\n    input: {activity: '${{ message.tool.result.status }}'}\n    output:\n      severity:\n        type: score\n        question: How severe is it?\n        criteria: [Low., High.]\n      applies:\n        type: noul\n        question: Does it apply?\nactions:\n  - type: status\n    if: ${{ decisions.confirm.outputs.applies > 0.5 }}\n    input: {result: failure, message: Both decisions completed.}\n";

fn result(active: &EffectiveActivation, attempt: &DecisionAttempt) -> DecisionResult {
    let declaration = active
        .compiled
        .decisions
        .iter()
        .find(|block| block.name == attempt.decision)
        .unwrap();
    let answers = declaration
        .outputs
        .iter()
        .map(|named| {
            let result = match &named.question {
                Question::Choice { .. } => QuestionResult::Choice(
                    ChoiceResult::new(
                        &named.question,
                        "yes",
                        Some(BTreeMap::from([("yes".into(), 0.99), ("no".into(), 0.01)])),
                        Some(0.8),
                    )
                    .unwrap(),
                ),
                Question::Score { .. } => QuestionResult::Score(
                    ScoreResult::new(&named.question, 0.75, Some(vec![0.25, 0.75]), Some(0.8))
                        .unwrap(),
                ),
                Question::Noul { .. } => QuestionResult::Noul(NoulResult::new(0.9).unwrap()),
            };
            NamedAnswer::new(&named.name, result).unwrap()
        })
        .collect();
    DecisionResult::new(
        declaration,
        &active.compiled.question_requirements(),
        answers,
        attempt.number,
    )
    .unwrap()
}

fn prepare(adapter: &StoreCheckStore, claimed: &ClaimedCandidate) {
    let prepared = prepare_inputs(
        vec![
            DecisionInput {
                decision: "review".into(),
                input: json!({"activity": "error"}),
            },
            DecisionInput {
                decision: "confirm".into(),
                input: json!({"activity": "error"}),
            },
        ],
        EvaluationBudget::default(),
        "fixture-multi-v1",
    );
    assert_eq!(
        adapter
            .record_preparation(claimed, &PreparationRecord::Ready(prepared))
            .unwrap(),
        ClaimOutcome::Current
    );
}

fn usage(store: &Store, candidate: &FrozenCandidate, attempt: &DecisionAttempt, known: bool) {
    store
        .record_burn_check_decision_usage(
            &hex(&candidate.fingerprint),
            attempt,
            &candidate.activation.definition_id,
            ("custom-systemone", "synthetic-model"),
            known.then_some(antiburn_local::analysis::jev::JevUsage {
                input_tokens: 7,
                output_tokens: 3,
            }),
            100,
        )
        .unwrap();
}

struct MultiEvaluator {
    store: Arc<Store>,
    sends: Mutex<Vec<DecisionAttempt>>,
}

impl DecisionEvaluator for MultiEvaluator {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<DecisionInput>,
        _: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        panic!("a restarted assessment must use its exact saved set")
    }
    fn validate_prepared(
        &self,
        active: &EffectiveActivation,
        prepared: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        assert_eq!(prepared.profile, "fixture-multi-v1");
        validate_fixture_preparation(active, prepared, "fixture-multi-v1")
    }
    fn evaluate_prepared<'a>(
        &'a self,
        active: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        self.sends.lock().unwrap().push(attempt.clone());
        usage(&self.store, candidate, attempt, true);
        Box::pin(async move { Ok(result(active, attempt)) })
    }
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

#[test]
fn file_reopen_reuses_a_and_sends_only_b2_then_accepts_once() {
    let directory = tempfile::tempdir().unwrap();
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open(directory.path()).unwrap()),
        MULTI_CHECK,
    );
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    prepare(&adapter, &claimed);
    let first = review_attempt(1);
    let second = DecisionAttempt::new("confirm", 1).unwrap();
    assert!(
        adapter
            .record_dispatch_intent(&claimed, 1001, Duration::from_secs(60), &second)
            .is_err()
    );
    adapter
        .record_dispatch_intent(&claimed, 1001, Duration::from_secs(60), &first)
        .unwrap();
    usage(&store, &candidate, &first, true);
    assert_eq!(
        adapter
            .record_decision_answer(&claimed, &first, &result(&active, &first))
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(state(&store, &candidate), "claimed");
    adapter
        .record_dispatch_intent(&claimed, 2001, Duration::from_secs(120), &second)
        .unwrap();
    usage(&store, &candidate, &second, true);
    adapter
        .record_attempt_failure(
            &claimed,
            &second,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT cooldown_until_epoch FROM burn_check_cooldown",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        62
    );
    drop(adapter);
    drop(store);

    let store = Arc::new(Store::open(directory.path()).unwrap());
    let adapter = Arc::new(
        StoreCheckStore::new(store.clone(), "restart".into(), active.id.scope.clone()).unwrap(),
    );
    adapter.register_activation(&active).unwrap();
    let pending = adapter.load_recovery(None, 10).unwrap();
    assert!(matches!(
        &pending[0].work,
        RecoveryWork::Pending { resumed: true, .. }
    ));
    let evaluator = Arc::new(MultiEvaluator {
        store: store.clone(),
        sends: Mutex::new(Vec::new()),
    });
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    assert_eq!(engine.recover_pending().unwrap().candidates, 1);
    let StartOutcome::Started(task) = engine.start_one_due(&active, 3001).unwrap() else {
        panic!("partial work must resume within its own cooldown")
    };
    let settled = poll_settlement(task);
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!((settled.attempts, settled.new_intents), (3, 1));
    assert_eq!(
        *evaluator.sends.lock().unwrap(),
        [DecisionAttempt::new("confirm", 2).unwrap()]
    );
    assert_eq!(state(&store, &candidate), "accepted");
    assert_eq!(store.burn_check_usage_summary().unwrap().confirmed_calls, 3);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_assessment_history WHERE outcome = 'assessed'",
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
                "SELECT COUNT(*) FROM burn_check_current_status",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let mut statement_connection = store.lock();
    let tx = statement_connection.transaction().unwrap();
    let rows = adapter
        .read_decision_progress(&tx, &adapter.claim_record(&claimed).unwrap(), &active)
        .unwrap_err();
    assert!(rows.to_string().contains("stale"));
    drop(tx);
    drop(statement_connection);
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
}

#[test]
fn unknown_b_after_file_reopen_never_resends_either_decision() {
    let directory = tempfile::tempdir().unwrap();
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open(directory.path()).unwrap()),
        MULTI_CHECK,
    );
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    prepare(&adapter, &claimed);
    let first = review_attempt(1);
    adapter
        .record_dispatch_intent(&claimed, 1001, Duration::ZERO, &first)
        .unwrap();
    adapter
        .record_decision_answer(&claimed, &first, &result(&active, &first))
        .unwrap();
    usage(&store, &candidate, &first, true);
    let second = DecisionAttempt::new("confirm", 1).unwrap();
    adapter
        .record_dispatch_intent(&claimed, 1001, Duration::ZERO, &second)
        .unwrap();
    usage(&store, &candidate, &second, false);
    drop(adapter);
    drop(store);
    let store = Arc::new(Store::open(directory.path()).unwrap());
    let adapter = Arc::new(
        StoreCheckStore::new(store.clone(), "restart".into(), active.id.scope.clone()).unwrap(),
    );
    adapter.register_activation(&active).unwrap();
    let evaluator = Arc::new(MultiEvaluator {
        store: store.clone(),
        sends: Mutex::new(Vec::new()),
    });
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    assert_eq!(engine.recover_pending().unwrap().candidates, 0);
    assert!(matches!(
        engine.start_one_due(&active, 999_000).unwrap(),
        StartOutcome::NotDue
    ));
    assert!(evaluator.sends.lock().unwrap().is_empty());
    assert_eq!(state(&store, &candidate), "execution_failed");
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT outcome FROM burn_check_evaluation_attempt WHERE decision_name = 'confirm'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "unknown"
    );
    assert_eq!(
        store.burn_check_usage_summary().unwrap().unknown_outcomes,
        1
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_current_status",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn partial_release_keeps_rows_preparation_and_fences_old_claims() {
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/multi-release")).unwrap()),
        MULTI_CHECK,
    );
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    prepare(&adapter, &claimed);
    let first = review_attempt(1);
    adapter
        .record_dispatch_intent(&claimed, 1001, Duration::from_secs(60), &first)
        .unwrap();
    adapter
        .record_decision_answer(&claimed, &first, &result(&active, &first))
        .unwrap();
    let saved = adapter.load_preparation(&claimed).unwrap();
    assert_eq!(
        adapter.release_pre_dispatch_claim(&claimed).unwrap(),
        ClaimOutcome::Current
    );
    assert!(adapter.load_decision_progress(&claimed).is_err());
    store
        .lock()
        .execute(
            "UPDATE burn_check_cooldown SET cooldown_until_epoch = 120",
            [],
        )
        .unwrap();
    assert!(matches!(
        adapter.claim_due(&candidate, 1002, false).unwrap(),
        CandidateClaim::NotDue {
            retry_at_unix_ms: 120_000
        }
    ));
    store
        .lock()
        .execute(
            "UPDATE burn_check_cooldown SET cooldown_until_epoch = 62",
            [],
        )
        .unwrap();
    let CandidateClaim::Claimed(current) = adapter.claim_due(&candidate, 1002, false).unwrap()
    else {
        panic!("partial progress must bypass only its own cooldown")
    };
    assert!(current.fence > claimed.fence);
    assert_eq!(adapter.load_preparation(&current).unwrap(), saved);
    let next = DecisionAttempt::new("confirm", 1).unwrap();
    assert_eq!(
        adapter
            .record_dispatch_intent(&claimed, 1002, Duration::ZERO, &next)
            .unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(
        adapter
            .record_dispatch_intent(&current, 1002, Duration::ZERO, &next)
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        adapter.release_pre_dispatch_claim(&current).unwrap(),
        ClaimOutcome::Stale
    );
}

#[test]
fn a_later_preparation_failure_and_oversized_rejection_write_no_intents_or_cooldown() {
    struct RejectLater;
    impl DecisionEvaluator for RejectLater {
        fn prepare(
            &self,
            _: &EffectiveActivation,
            inputs: Vec<DecisionInput>,
            _: EvaluationBudget,
        ) -> Result<PreparedAssessment, PreparationFailure> {
            assert_eq!(
                inputs
                    .iter()
                    .map(|input| input.decision.as_str())
                    .collect::<Vec<_>>(),
                ["review", "confirm"]
            );
            Err(PreparationFailure {
                code: antiburn_checks::fit::PreparationFailureCode::NoFit,
                detail: "The second decision does not fit.".into(),
                report: None,
                context: Default::default(),
            })
        }
        fn validate_prepared(
            &self,
            _: &EffectiveActivation,
            _: &PreparedAssessment,
            _: PreparationValidationPurpose,
        ) -> Result<(), PreparationFailure> {
            panic!("a rejected set must not validate")
        }
        fn evaluate_prepared<'a>(
            &'a self,
            _: &'a EffectiveActivation,
            _: &'a FrozenCandidate,
            _: &'a PreparedAssessment,
            _: &'a DecisionAttempt,
        ) -> DecisionFuture<'a> {
            panic!("a rejected set must not send")
        }
    }
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/multi-preflight")).unwrap()),
        MULTI_CHECK,
    );
    let engine = CheckEngine::new(adapter.clone(), Arc::new(RejectLater));
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1001).unwrap() else {
        panic!("the candidate must settle locally")
    };
    let settled = poll_settlement(task);
    assert_eq!((settled.attempts, settled.new_intents), (0, 0));
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    let candidate = apply(&adapter, &active, &event(&store, 2, "two", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let failure = PreparationFailure {
        code: antiburn_checks::fit::PreparationFailureCode::NoFit,
        detail: "The omission report exceeds the saved-state limit.".into(),
        report: Some(antiburn_checks::fit::FitReport {
            omissions: vec![antiburn_checks::fit::FitOmission {
                path: "x".repeat(8 * 1024 * 1024),
                strategy: antiburn_checks::fit::FitStrategy::Truncate,
                original_count: 2,
                retained_count: 1,
            }],
        }),
        context: Default::default(),
    };
    assert!(
        adapter
            .record_preparation_rejection(&claimed, &failure)
            .is_err()
    );
    assert!(
        adapter
            .record_preparation(&claimed, &PreparationRecord::Rejected(failure))
            .is_err()
    );
    assert_eq!(state(&store, &candidate), "claimed");
    assert!(adapter.load_preparation(&claimed).unwrap().is_none());
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
            0
        );
    }
}

#[test]
fn preparation_reads_reject_raw_duplicates_unknown_fields_versions_and_byte_overflow() {
    for corrupt in [
        "duplicate",
        "unknown",
        "version",
        "budget",
        "nodes",
        "depth",
        "bytes",
        "local_bytes",
        "rejected_bytes",
    ] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        let prepared = PreparationRecord::Ready(prepared_review(
            "fixture-v1",
            json!({}),
            json!({}),
            Default::default(),
            EvaluationBudget::default(),
        ));
        let raw = serde_json::to_string(&prepared).unwrap();
        let raw = match corrupt {
            "duplicate" => raw.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
            "unknown" => raw.replacen("{", "{\"unknown\":true,", 1),
            "version" => raw.replacen("\"version\":1", "\"version\":99", 1),
            "budget" => {
                let mut value = serde_json::to_value(&prepared).unwrap();
                value.as_object_mut().unwrap().remove("evaluation_budget");
                value.to_string()
            }
            "nodes" => {
                let mut value = serde_json::to_value(&prepared).unwrap();
                value["decisions"][0]["request"] =
                    serde_json::Value::Array(vec![serde_json::Value::Null; 65_536]);
                value.to_string()
            }
            "depth" => {
                let mut value = serde_json::Value::Null;
                for _ in 0..64 {
                    value = json!([value]);
                }
                let mut record = serde_json::to_value(&prepared).unwrap();
                record["decisions"][0]["request"] = value;
                record.to_string()
            }
            "bytes" => format!("{}{}", raw, " ".repeat(8 * 1024 * 1024)),
            "local_bytes" => format!(
                "{}{}",
                serde_json::to_string(&PreparationRecord::Local(LocalPreparation::new(1001)))
                    .unwrap(),
                " ".repeat(8 * 1024 * 1024)
            ),
            "rejected_bytes" => format!(
                "{}{}",
                serde_json::to_string(&PreparationRecord::Rejected(PreparationFailure {
                    code: antiburn_checks::fit::PreparationFailureCode::NoFit,
                    detail: "No fit.".into(),
                    report: None,
                    context: Default::default(),
                }))
                .unwrap(),
                " ".repeat(8 * 1024 * 1024)
            ),
            _ => unreachable!(),
        };
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE candidate_id = ?2",
                params![raw, hex(&candidate.fingerprint)],
            )
            .unwrap();
        assert!(
            matches!(
                adapter.load_preparation(&claimed).unwrap(),
                Some(PreparationRecord::Rejected(_))
            ),
            "{corrupt}"
        );
    }
}

#[test]
fn progress_rejects_holes_unknown_names_contradictions_and_oversized_responses() {
    for corruption in [
        "hole",
        "unknown",
        "contradiction",
        "count",
        "bytes",
        "duplicate",
        "duplicate_answer",
    ] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        adapter
            .dispatch_review(&claimed, 1001, Duration::ZERO, 1)
            .unwrap();
        adapter
            .record_decision_answer(
                &claimed,
                &review_attempt(1),
                &review_result(&active, 1, 0.99),
            )
            .unwrap();
        let id = hex(&candidate.fingerprint);
        match corruption {
            "hole" => {
                store.lock().execute("UPDATE burn_check_evaluation_attempt SET attempt_number = 2, attempt_id = ?2 WHERE candidate_id = ?1", params![id, attempt_id(&candidate.fingerprint, &review_attempt(2))]).unwrap();
            }
            "unknown" => {
                let unknown = DecisionAttempt::new("unknown", 1).unwrap();
                store.lock().execute("UPDATE burn_check_evaluation_attempt SET decision_name = 'unknown', attempt_id = ?2 WHERE candidate_id = ?1", params![id, attempt_id(&candidate.fingerprint, &unknown)]).unwrap();
            }
            "contradiction" => {
                store.lock().execute("UPDATE burn_check_evaluation_attempt SET outcome = 'safe_transient' WHERE candidate_id = ?1", [&id]).unwrap();
            }
            "count" => {
                let mut answer = serde_json::to_value(review_result(&active, 1, 0.99)).unwrap();
                answer["attempts"] = json!(2);
                store.lock().execute("UPDATE burn_check_evaluation_attempt SET response_json = ?2 WHERE candidate_id = ?1", params![id, answer.to_string()]).unwrap();
            }
            "bytes" => {
                let raw = format!(
                    "{}{}",
                    serde_json::to_string(&review_result(&active, 1, 0.99)).unwrap(),
                    " ".repeat(512 * 1024)
                );
                store.lock().execute("UPDATE burn_check_evaluation_attempt SET response_json = ?2 WHERE candidate_id = ?1", params![id, raw]).unwrap();
            }
            "duplicate" => {
                let raw = serde_json::to_string(&review_result(&active, 1, 0.99))
                    .unwrap()
                    .replacen("\"attempts\":1", "\"attempts\":1,\"attempts\":1", 1);
                store.lock().execute("UPDATE burn_check_evaluation_attempt SET response_json = ?2 WHERE candidate_id = ?1", params![id, raw]).unwrap();
            }
            "duplicate_answer" => {
                store.lock().execute("INSERT INTO burn_check_evaluation_attempt
                    (attempt_id, candidate_id, decision_name, attempt_number, state, outcome, response_json, updated_at_epoch)
                    VALUES (?1, ?2, 'review', 2, 'settled', 'answer', ?3, 100)",
                    params![attempt_id(&candidate.fingerprint, &review_attempt(2)), id,
                        serde_json::to_string(&review_result(&active, 2, 0.99)).unwrap()]).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            adapter.load_decision_progress(&claimed).is_err(),
            "{corruption}"
        );
    }
}

#[test]
fn source_and_connection_revocation_block_next_intents_and_keep_late_settlement() {
    for connection_change in [false, true] {
        for late in [false, true] {
            let (store, adapter, active) = fixture_with_definition(
                Arc::new(Store::open_in_memory(Path::new("/synthetic/multi-revocation")).unwrap()),
                MULTI_CHECK,
            );
            let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
            let claimed = claim(&adapter, &candidate);
            prepare(&adapter, &claimed);
            let first = review_attempt(1);
            adapter
                .record_dispatch_intent(&claimed, 1001, Duration::ZERO, &first)
                .unwrap();
            adapter
                .record_decision_answer(&claimed, &first, &result(&active, &first))
                .unwrap();
            usage(&store, &candidate, &first, true);
            let second = DecisionAttempt::new("confirm", 1).unwrap();
            if late {
                adapter
                    .record_dispatch_intent(&claimed, 1001, Duration::ZERO, &second)
                    .unwrap();
                usage(&store, &candidate, &second, false);
            }
            if connection_change {
                store
                    .cancel_unfinished_burn_check_assessments_for_connection_change()
                    .unwrap();
                store
                    .cancel_unfinished_burn_check_assessments_for_connection_change()
                    .unwrap();
            } else {
                // This direct change tests the dependency check independently of source-page revocation.
                store
                    .lock()
                    .execute(
                        "UPDATE burn_check_observation SET payload_json = '{\"changed\":true}'",
                        [],
                    )
                    .unwrap();
            }
            if late {
                assert_eq!(
                    adapter
                        .record_decision_answer(&claimed, &second, &result(&active, &second))
                        .unwrap(),
                    ClaimOutcome::Stale
                );
                usage(&store, &candidate, &second, true);
                assert_eq!(store.burn_check_usage_summary().unwrap().confirmed_calls, 2);
                assert_eq!(
                    store.burn_check_usage_summary().unwrap().unknown_outcomes,
                    0
                );
                assert_eq!(store.lock().query_row("SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE outcome = 'answer' AND response_json IS NOT NULL", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
                let actions: String = store
                    .lock()
                    .query_row(
                        "SELECT actions_json FROM burn_check_assessment_history",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(actions, "{}");
            }
            assert_eq!(
                adapter
                    .record_dispatch_intent(&claimed, 1001, Duration::ZERO, &second)
                    .unwrap(),
                ClaimOutcome::Stale
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
                if late { 2 } else { 1 }
            );
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM burn_check_current_status",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
        }
    }
}

fn complete(
    adapter: &StoreCheckStore,
    active: &EffectiveActivation,
    candidate: &FrozenCandidate,
) -> AssessmentAcceptance {
    let claimed = claim(adapter, candidate);
    prepare(adapter, &claimed);
    let mut results = Vec::new();
    for name in ["review", "confirm"] {
        let attempt = DecisionAttempt::new(name, 1).unwrap();
        adapter
            .record_dispatch_intent(&claimed, 1001, Duration::ZERO, &attempt)
            .unwrap();
        let answer = result(active, &attempt);
        adapter
            .record_decision_answer(&claimed, &attempt, &answer)
            .unwrap();
        results.push(answer);
    }
    let evaluated = EvaluatedAssessment::new(
        &active.compiled.decisions,
        &active.compiled.question_requirements(),
        results,
    )
    .unwrap();
    let actions = plan_status_actions(
        &active.compiled.actions,
        &candidate.context.value,
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
fn multi_no_write_keeps_exact_status_evidence_and_retention_clears_all_unused_payloads() {
    let source = MULTI_CHECK.replace(
        "decisions.confirm.outputs.applies > 0.5",
        "decisions.confirm.outputs.applies > 0.5 && message.tool.result.status == 'error'",
    );
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/multi-retention")).unwrap()),
        &source,
    );
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let initial = complete(&adapter, &active, &first);
    assert_eq!(
        adapter.accept(&initial).unwrap(),
        AcceptanceOutcome::Accepted
    );
    let original = store.lock().query_row("SELECT json_array(assessment_id, updated_at_epoch, status) FROM burn_check_current_status", [], |row| row.get::<_, String>(0)).unwrap();
    let mut page = event(&store, 2, "two", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        unreachable!()
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        unreachable!()
    };
    message.value["tool"]["result"]["status"] = json!("okay");
    let second = apply(&adapter, &active, &page);
    let no_write = complete(&adapter, &active, &second);
    assert!(no_write.status_write().is_none());
    assert_eq!(
        adapter.accept(&no_write).unwrap(),
        AcceptanceOutcome::Accepted
    );
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), now_epoch() + 1).unwrap();
    assert_eq!(store.lock().query_row("SELECT json_array(assessment_id, updated_at_epoch, status) FROM burn_check_current_status", [], |row| row.get::<_, String>(0)).unwrap(), original);
    let prepared: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&first.fingerprint)],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Ready(prepared) =
        antiburn_checks::preparation::decode_preparation_record(prepared.as_bytes()).unwrap()
    else {
        panic!("the status-writing set must remain complete")
    };
    assert_eq!(prepared.decisions.len(), 2);
    let pruned: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&second.fingerprint)],
            |row| row.get(0),
        )
        .unwrap();
    let pruned: serde_json::Value = serde_json::from_str(&pruned).unwrap();
    for decision in pruned["decisions"].as_array().unwrap() {
        for payload in ["original_input", "input", "request"] {
            assert!(decision.get(payload).is_none());
        }
    }
    let responses: (i64, i64) = store.lock().query_row("SELECT
        (SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE candidate_id = ?1 AND response_json IS NOT NULL),
        (SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE candidate_id = ?2 AND response_json IS NOT NULL)",
        params![hex(&first.fingerprint), hex(&second.fingerprint)], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(responses, (2, 0));
    CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
        .apply_source_page(&active, &event(&store, 3, "one", "branch", 2))
        .unwrap();
    assert!(
        !store
            .lock()
            .query_row(
                "SELECT evidence_valid FROM burn_check_current_status",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[test]
fn corrupt_preparation_or_attempt_response_does_not_hide_a_later_answered_row() {
    for corrupt_preparation in [false, true] {
        let (store, adapter, active) = fixture();
        let mut candidates = [
            apply(&adapter, &active, &event(&store, 1, "one", "branch-one", 1)),
            apply(&adapter, &active, &event(&store, 2, "two", "branch-two", 1)),
        ];
        candidates.sort_by_key(|candidate| candidate.fingerprint);
        let [bad, good] = candidates;
        answer(&adapter, &active, claim(&adapter, &bad), 0.99);
        answer(&adapter, &active, claim(&adapter, &good), 0.99);
        if corrupt_preparation {
            store.lock().execute("UPDATE burn_check_candidate SET preparation_json = '{\"status\":\"ready\",\"version\":999}' WHERE candidate_id = ?1", [hex(&bad.fingerprint)]).unwrap();
        } else {
            store.lock().execute("UPDATE burn_check_evaluation_attempt SET response_json = '{}' WHERE candidate_id = ?1", [hex(&bad.fingerprint)]).unwrap();
        }
        let adapter = Arc::new(
            StoreCheckStore::new(store.clone(), "row-restart".into(), active.id.scope.clone())
                .unwrap(),
        );
        adapter.register_activation(&active).unwrap();
        let evaluator = Arc::new(RecordingEvaluator(Mutex::new(Vec::new())));
        let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
        engine.recover_pending().unwrap();
        assert_eq!(state(&store, &bad), "execution_failed");
        assert_eq!(state(&store, &good), "accepted");
        assert!(evaluator.0.lock().unwrap().is_empty());
        assert!(adapter.load_recovery(None, 1).unwrap().is_empty());
    }
}

#[test]
fn rolling_updates_never_coalesce_started_partial_work() {
    let source = MULTI_CHECK.replace(
        "  cooldown: 60s",
        "  cooldown: 60s\n  window: {size: 1, min: 1}",
    );
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/multi-coalescing")).unwrap()),
        &source,
    );
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &first);
    prepare(&adapter, &claimed);
    let attempt = review_attempt(1);
    adapter
        .record_dispatch_intent(&claimed, 1001, Duration::from_secs(60), &attempt)
        .unwrap();
    adapter
        .record_decision_answer(&claimed, &attempt, &result(&active, &attempt))
        .unwrap();
    adapter.release_pre_dispatch_claim(&claimed).unwrap();
    let second = apply(&adapter, &active, &event(&store, 2, "two", "branch", 1));
    assert_eq!(state(&store, &first), "pending");
    assert_eq!(state(&store, &second), "pending");
    assert!(matches!(
        adapter.claim_due(&second, 1002, false).unwrap(),
        CandidateClaim::NotDue { .. }
    ));
    assert!(matches!(
        adapter.claim_due(&first, 1002, false).unwrap(),
        CandidateClaim::Claimed(_)
    ));
}

#[test]
fn aggregate_progress_bytes_are_bounded_before_a_second_response_is_decoded() {
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/multi-progress-bytes")).unwrap()),
        MULTI_CHECK,
    );
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = complete(&adapter, &active, &candidate);
    store.lock().execute("UPDATE burn_check_evaluation_attempt SET response_json = response_json || ?1 WHERE candidate_id = ?2",
        params![" ".repeat(300 * 1024), hex(&candidate.fingerprint)]).unwrap();
    let error = adapter
        .load_decision_progress(&assessment.claimed)
        .unwrap_err();
    assert!(error.message.contains("retained byte limit"));
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_current_status",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn retention_clears_malformed_blocks_and_healthy_payloads_without_pruning_retry_or_usage() {
    let (store, adapter, active) = fixture();
    let mut terminal = Vec::new();
    for (sequence, branch) in [(1, "healthy"), (2, "corrupt")] {
        let candidate = apply(
            &adapter,
            &active,
            &event(&store, sequence, "one", branch, 1),
        );
        let claimed = claim(&adapter, &candidate);
        adapter
            .record_preparation(
                &claimed,
                &PreparationRecord::Ready(prepared_review(
                    "fixture-v1",
                    json!({"text": "private assessed input"}),
                    json!({"exact": "saved"}),
                    Default::default(),
                    EvaluationBudget::default(),
                )),
            )
            .unwrap();
        let assessment = answer(&adapter, &active, claimed, 0.4);
        assert!(assessment.status_write().is_none());
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
        usage(&store, &candidate, &review_attempt(1), true);
        terminal.push(candidate);
    }
    let retry = apply(&adapter, &active, &event(&store, 3, "one", "retry", 1));
    let claimed = claim(&adapter, &retry);
    adapter
        .dispatch_review(&claimed, 1001, Duration::from_secs(60), 1)
        .unwrap();
    adapter
        .fail_review(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    usage(&store, &retry, &review_attempt(1), true);
    assert_eq!(
        adapter.release_pre_dispatch_claim(&claimed).unwrap(),
        ClaimOutcome::Current
    );
    let retry_snapshot = || {
        store
            .lock()
            .query_row(
                "SELECT json_array(state, frozen_input_json,
        dependencies_json, preparation_json, answer_json, claim_fence, started_cooldown_until_epoch)
        FROM burn_check_candidate WHERE candidate_id = ?1",
                [hex(&retry.fingerprint)],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    let retry_attempt = || {
        store
            .lock()
            .query_row(
                "SELECT json_array(attempt_id, decision_name,
        attempt_number, state, outcome, response_json, updated_at_epoch)
        FROM burn_check_evaluation_attempt WHERE candidate_id = ?1",
                [hex(&retry.fingerprint)],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    let usage_snapshot = || {
        store
            .lock()
            .query_row(
                "SELECT json_group_array(json_array(attempt_id, usage_known))
        FROM (SELECT attempt_id, usage_known FROM burn_check_decision_usage ORDER BY attempt_id)",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    let original_retry = retry_snapshot();
    let original_attempt = retry_attempt();
    let original_usage = usage_snapshot();
    let original_summary = store.burn_check_usage_summary().unwrap();
    let malformed = json!({"status": "ready", "decisions": [
        "not-json", 7, true, null, ["private nested payload"], {
            "decision": "review", "original_input": {"text": "private original"},
            "input": {"text": "private fitted"}, "request": {"text": "private request"},
            "report": {"omissions": []}
        }
    ]});
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE candidate_id = ?2",
            params![malformed.to_string(), hex(&terminal[1].fingerprint)],
        )
        .unwrap();
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), now_epoch() + 1).unwrap();
    for candidate in &terminal {
        let (frozen, dependencies, answer, preparation): (String, String, Option<String>, String) =
            store
                .lock()
                .query_row(
                    "SELECT frozen_input_json, dependencies_json, answer_json, preparation_json
             FROM burn_check_candidate WHERE candidate_id = ?1",
                    [hex(&candidate.fingerprint)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
        assert_eq!(
            (frozen.as_str(), dependencies.as_str(), answer),
            ("{}", "{}", None)
        );
        let preparation: serde_json::Value = serde_json::from_str(&preparation).unwrap();
        for block in preparation["decisions"].as_array().unwrap() {
            assert!(block.is_object());
            for payload in ["original_input", "input", "request"] {
                assert!(block.get(payload).is_none());
            }
        }
        if candidate == &terminal[1] {
            for block in &preparation["decisions"].as_array().unwrap()[..5] {
                assert_eq!(block, &json!({}));
            }
        }
        assert!(!preparation.to_string().contains("private"));
        assert!(!preparation.to_string().contains("not-json"));
        let retained: (bool, bool) = store.lock().query_row("SELECT
            EXISTS(SELECT 1 FROM burn_check_evaluation_attempt WHERE candidate_id = ?1 AND response_json IS NOT NULL),
            EXISTS(SELECT 1 FROM burn_check_assessment_history WHERE candidate_id = ?1 AND answer_json IS NOT NULL)",
            [hex(&candidate.fingerprint)], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(retained, (false, false));
    }
    assert_eq!(retry_snapshot(), original_retry);
    assert_eq!(retry_attempt(), original_attempt);
    assert_eq!(usage_snapshot(), original_usage);
    assert_eq!(store.burn_check_usage_summary().unwrap(), original_summary);
}
