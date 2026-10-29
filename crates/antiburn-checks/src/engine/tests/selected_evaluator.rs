use super::*;

#[derive(Default)]
struct SelectedEvaluator {
    stages: Mutex<Vec<&'static str>>,
    retry_once: bool,
}

impl DecisionEvaluator for SelectedEvaluator {
    fn prepare(
        &self,
        activation: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        self.stages.lock().unwrap().push("prepare");
        let mut prepared = test_prepare(activation, inputs, budget)?;
        prepared.profile = "selected-snapshot".into();
        Ok(prepared)
    }

    fn preparation_recorded(&self, preparation: &PreparationRecord) {
        assert!(
            matches!(preparation, PreparationRecord::Ready(prepared) if prepared.profile == "selected-snapshot")
        );
        self.stages.lock().unwrap().push("preparation_recorded");
    }

    fn validate_prepared(
        &self,
        _: &EffectiveActivation,
        prepared: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        assert_eq!(prepared.profile, "selected-snapshot");
        self.stages.lock().unwrap().push("validate_prepared");
        Ok(())
    }

    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        assert_eq!(prepared.profile, "selected-snapshot");
        assert_eq!(
            prepared.decisions[0].request,
            json!({"input": {"activity": "error"}})
        );
        self.stages.lock().unwrap().push("evaluate_prepared");
        assert_eq!(prepared.decisions[0].input, json!({"activity": "error"}));
        Box::pin(async move {
            if self.retry_once && attempt.number == 1 {
                return Err(EvaluationFailure {
                    code: EvaluationFailureCode::ProviderOverloaded,
                    kind: AttemptFailure::SafeTransient,
                    detail: "Synthetic safe rejection.".into(),
                    attempts: 1,
                });
            }
            Ok(test_answer(activation, attempt, Some("yes")))
        })
    }
}

#[test]
fn supplied_evaluator_owns_preflight_and_retries_after_engine_drop() {
    let store = Arc::new(FakeStore::default());
    let default = Arc::new(SelectedEvaluator::default());
    let selected = Arc::new(SelectedEvaluator {
        retry_once: true,
        ..Default::default()
    });
    let engine = CheckEngine::new(store.clone(), default.clone());
    let activation = activation();
    let branches = BTreeSet::from([BranchId("branch-a".into())]);
    engine
        .enqueue(&activation, candidate(&activation, 1))
        .unwrap();
    let evaluator = selected.clone();
    let started: StartOutcome<'static> = std::thread::spawn(move || {
        engine.start_one_due_for_branches_with_evaluator(&activation, 0, &branches, evaluator)
    })
    .join()
    .unwrap()
    .unwrap();
    assert_eq!(
        *selected.stages.lock().unwrap(),
        ["prepare", "preparation_recorded", "validate_prepared"]
    );
    assert!(default.stages.lock().unwrap().is_empty());
    let StartOutcome::Started(task) = started else {
        panic!("selected evaluator must start the candidate");
    };
    let settled = run_task(task);
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!(settled.attempts, 2);
    assert_eq!(
        *selected.stages.lock().unwrap(),
        [
            "prepare",
            "preparation_recorded",
            "validate_prepared",
            "validate_prepared",
            "evaluate_prepared",
            "validate_prepared",
            "evaluate_prepared",
            "validate_prepared",
        ]
    );
    assert!(default.stages.lock().unwrap().is_empty());
    assert_eq!(*store.dispatches.lock().unwrap(), 2);
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
}

#[test]
fn existing_branch_method_keeps_the_default_evaluator() {
    let evaluator = Arc::new(SelectedEvaluator::default());
    let engine = CheckEngine::new(Arc::new(FakeStore::default()), evaluator.clone());
    let activation = activation();
    engine
        .enqueue(&activation, candidate(&activation, 1))
        .unwrap();
    let StartOutcome::Started(task) = engine
        .start_one_due_for_branches(
            &activation,
            0,
            &BTreeSet::from([BranchId("branch-a".into())]),
        )
        .unwrap()
    else {
        panic!("the default evaluator must start the candidate");
    };
    assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
    assert_eq!(
        *evaluator.stages.lock().unwrap(),
        [
            "prepare",
            "preparation_recorded",
            "validate_prepared",
            "validate_prepared",
            "evaluate_prepared",
            "validate_prepared",
        ]
    );
}
