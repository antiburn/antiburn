use super::*;

struct LabelOnlyEvaluator(&'static str);

fn label_answer(
    activation: &EffectiveActivation,
    attempt: &DecisionAttempt,
    output: &str,
) -> DecisionResult {
    let block = activation
        .compiled
        .decisions
        .iter()
        .find(|block| block.name == attempt.decision)
        .unwrap();
    let question = &block.outputs[0];
    let result = crate::results::ChoiceResult::new(&question.question, output, None, None).unwrap();
    let mut requirements = activation.compiled.question_requirements();
    for requirement in &mut requirements {
        requirement.probabilities = false;
        requirement.confidence = false;
    }
    DecisionResult::new(
        block,
        &requirements,
        vec![
            crate::results::NamedAnswer::new(
                &question.name,
                crate::results::QuestionResult::Choice(result),
            )
            .unwrap(),
        ],
        attempt.number,
    )
    .unwrap()
}

impl DecisionEvaluator for LabelOnlyEvaluator {
    fn prepare(
        &self,
        activation: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        test_prepare(activation, inputs, budget)
    }
    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &PreparedAssessment,
        purpose: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        test_validate(activation, prepared, purpose)
    }
    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        _: &'a crate::domain::FrozenCandidate,
        _: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        Box::pin(async move { Ok(label_answer(activation, attempt, self.0)) })
    }
}

#[test]
fn label_only_replay_persists_answers_and_only_explicit_label_status() {
    let mut activation = activation();
    let check = Arc::make_mut(&mut activation.compiled);
    check.actions = vec![crate::compiler::StatusAction {
        condition: Some(crate::compiler::Condition::Expression(
            crate::expression::parse_expression("decisions.review.outputs.verdict == 'stuck'")
                .unwrap(),
        )),
        result: crate::compiler::StatusResult::Failure.into(),
        message: None,
    }];
    for (output, expected) in [
        ("stuck", Some(crate::compiler::StatusResult::Failure)),
        ("clear", None),
        ("uncertain", None),
    ] {
        let store = Arc::new(MemoryStore::default());
        let report = block_on(replay(
            ReplayRunId("label-only".into()),
            activation.clone(),
            store.clone(),
            Arc::new(LabelOnlyEvaluator(output)),
            &mut Events((1..=3).map(event).collect::<Vec<_>>().into_iter()),
            small_limits(),
            || false,
        ))
        .unwrap();
        assert_eq!(report.end, ReplayEnd::Complete);
        assert_eq!((report.evaluations, report.attempts), (1, 1));
        let answers = store.answers.lock().unwrap();
        assert_eq!(answers.len(), 1);
        let saved = serde_json::to_value(&answers[0]).unwrap();
        assert!(
            !saved["decisions"][0]["answers"][0]["result"]
                .as_object()
                .unwrap()
                .contains_key("probabilities")
        );
        let restored = crate::results::decode_results(&serde_json::to_vec(&saved).unwrap())
            .unwrap()
            .validate_for_declarations(
                &activation.compiled.decisions,
                &activation.compiled.question_requirements(),
            )
            .unwrap();
        assert_eq!(restored, answers[0]);
        let accepted = store.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].status_write(), expected);
        assert_eq!(accepted[0].actions.current_status, expected);
    }
}

#[test]
fn probability_checks_reject_label_only_replay_before_answer_or_success() {
    for output in ["stuck", "clear", "uncertain"] {
        let store = Arc::new(MemoryStore::default());
        let report = block_on(replay(
            ReplayRunId("missing-probabilities".into()),
            activation(),
            store.clone(),
            Arc::new(LabelOnlyEvaluator(output)),
            &mut Events((1..=3).map(event).collect::<Vec<_>>().into_iter()),
            small_limits(),
            || false,
        ))
        .unwrap();
        assert!(matches!(report.end, ReplayEnd::ExecutionError(_)));
        assert_eq!((report.evaluations, report.attempts), (1, 1));
        assert!(matches!(
            &report.steps.last().unwrap().outcome,
            Some(RunOutcome::Failed(detail)) if detail.contains("probability")
        ));
        assert_eq!(
            store.evaluation_failures.lock().unwrap()[0].code,
            crate::engine::EvaluationFailureCode::InvalidDecision
        );
        assert_eq!(*store.failures.lock().unwrap(), [AttemptFailure::Permanent]);
        assert!(store.answers.lock().unwrap().is_empty());
        assert!(store.accepted.lock().unwrap().is_empty());
    }
}

#[test]
fn stored_label_only_recovery_keeps_prior_failure_and_never_resends() {
    for reads_probabilities in [false, true] {
        let mut activation = activation();
        if !reads_probabilities {
            Arc::make_mut(&mut activation.compiled).actions = vec![crate::compiler::StatusAction {
                condition: Some(crate::compiler::Condition::Expression(
                    crate::expression::parse_expression(
                        "decisions.review.outputs.verdict == 'stuck'",
                    )
                    .unwrap(),
                )),
                result: crate::compiler::StatusResult::Failure.into(),
                message: None,
            }];
        }
        let attempt = DecisionAttempt::new("review", 1).unwrap();
        let answer = label_answer(&activation, &attempt, "clear");
        let mut relaxed = activation.compiled.question_requirements();
        for requirement in &mut relaxed {
            requirement.probabilities = false;
            requirement.confidence = false;
        }
        let evaluated = EvaluatedAssessment::new(
            &activation.compiled.decisions,
            &relaxed,
            vec![answer.clone()],
        )
        .unwrap();
        let candidate = crate::domain::FrozenCandidate {
            activation: activation.id.clone(),
            branch: BranchId("main".into()),
            trigger_position: ObservationPosition(3),
            messages: Vec::new(),
            context: FrozenContext {
                value: json!({
                    "session": {"requests": [], "requests_complete": true, "source_coverage": "complete"},
                    "window": {"messages": [
                        {"tool": {"result": {"status": "error"}}},
                        {"tool": {"result": {"status": "error"}}},
                        {"tool": {"result": {"status": "error"}}}
                    ]},
                }),
                dependencies: Vec::new(),
            },
            dependencies: Vec::new(),
            fingerprint: [3; 32],
        };
        let store = Arc::new(MemoryStore::default());
        let id = CandidateId::from(&candidate);
        let mut budget = EvaluationBudget::default();
        let crate::preparation::PreparationOutcome::Ready(input) =
            crate::preparation::prepare_decision_input_with_budget(
                &activation.compiled.decisions[0],
                &candidate,
                &mut budget,
            )
        else {
            panic!("complete frozen input");
        };
        let inputs = vec![DecisionInput {
            decision: "review".into(),
            input,
        }];
        store.preparations.lock().unwrap().insert(
            id.clone(),
            crate::fit::PreparationRecord::Ready(
                test_prepare(&activation, inputs, budget).unwrap(),
            ),
        );
        store.attempt_rows.lock().unwrap().insert(
            id,
            DecisionProgress {
                attempts: vec![SavedDecisionAttempt {
                    attempt,
                    outcome: DecisionAttemptOutcome::Answer(answer),
                }],
            },
        );
        store.recovery.lock().unwrap().push(RecoveryRecord {
            activation,
            work: crate::check_store::RecoveryWork::Answered {
                claimed: ClaimedCandidate {
                    id: CandidateId::from(&candidate),
                    candidate,
                    prior_status: Some(crate::compiler::StatusResult::Failure),
                    fence: 1,
                },
                evaluated: evaluated.clone(),
            },
        });
        let evaluator = Arc::new(ScriptedEvaluator {
            inputs: Mutex::new(Vec::new()),
        });
        let engine = CheckEngine::new(store.clone(), evaluator.clone());
        let recovered = engine.recover_pending();
        if reads_probabilities {
            assert_eq!(recovered.unwrap().candidates, 0);
            let failures = store.evaluation_failures.lock().unwrap();
            assert_eq!(failures.len(), 1);
            assert_eq!(
                failures[0].code,
                crate::engine::EvaluationFailureCode::InvalidDecision
            );
            assert!(failures[0].detail.contains("probability"));
            assert!(store.accepted.lock().unwrap().is_empty());
        } else {
            assert_eq!(recovered.unwrap().candidates, 0);
            let accepted = store.accepted.lock().unwrap();
            assert_eq!(accepted.len(), 1);
            assert_eq!(accepted[0].evaluated, evaluated);
            assert_eq!(accepted[0].status_write(), None);
            assert_eq!(
                accepted[0].actions.current_status,
                Some(crate::compiler::StatusResult::Failure),
            );
        }
        assert!(evaluator.inputs.lock().unwrap().is_empty());
        assert!(store.dispatches.lock().unwrap().is_empty());
        assert!(store.answers.lock().unwrap().is_empty());
    }
}
