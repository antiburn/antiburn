use super::*;
use crate::domain::FrozenCandidate;

struct NoModel;

impl DecisionEvaluator for NoModel {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<DecisionInput>,
        _: EvaluationBudget,
    ) -> Result<PreparedAssessment, crate::fit::PreparationFailure> {
        panic!("local replay must not prepare a provider");
    }

    fn preparation_recorded(&self, _: &crate::fit::PreparationRecord) {
        panic!("local replay must not report provider preparation");
    }

    fn validate_prepared(
        &self,
        _: &EffectiveActivation,
        _: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        panic!("local replay must not validate a provider");
    }

    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        _: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        panic!("local replay must not send a provider request");
    }
}

#[test]
fn local_replay_with_zero_attempt_budget_counts_evaluations_and_bypasses_cooldown() {
    let store = Arc::new(MemoryStore::default());
    let active = activation_for(
        "name: Local replay\non:\n  tool_result:\n  cooldown: 1h\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    let report = block_on(replay(
        ReplayRunId("local-replay".into()),
        active,
        store.clone(),
        Arc::new(NoModel),
        &mut Events((1..=4).map(event).collect::<Vec<_>>().into_iter()),
        ReplayLimits {
            events: 4,
            evaluations: 4,
            attempts: 0,
        },
        || false,
    ))
    .unwrap();
    assert_eq!(report.end, ReplayEnd::Complete);
    assert_eq!(
        (report.events, report.evaluations, report.attempts),
        (4, 4, 0)
    );
    assert_eq!(store.answers.lock().unwrap().len(), 4);
    assert_eq!(store.accepted.lock().unwrap().len(), 4);
    assert!(store.dispatches.lock().unwrap().is_empty());
    assert!(store.failures.lock().unwrap().is_empty());
    assert!(
        store
            .local_starts
            .lock()
            .unwrap()
            .iter()
            .all(|(_, cooldown)| cooldown.is_zero())
    );
    assert!(
        store
            .accepted
            .lock()
            .unwrap()
            .iter()
            .all(|accepted| accepted.evaluated.decisions().is_empty()
                && accepted.evaluated.attempts() == 0)
    );
}

#[test]
fn local_replay_still_honors_event_evaluation_and_cancellation_limits() {
    for (events, evaluations, cancelled, expected) in [
        (2, 4, false, (2, 2, ReplayEnd::LimitReached)),
        (4, 1, false, (2, 1, ReplayEnd::LimitReached)),
        (4, 4, true, (0, 0, ReplayEnd::Cancelled)),
    ] {
        let store = Arc::new(MemoryStore::default());
        let active = activation_for(
            "name: Local replay\non:\n  tool_result:\nactions:\n  - type: status\n    input: {result: failure}\n",
        );
        let report = block_on(replay(
            ReplayRunId("local-limit".into()),
            active,
            store.clone(),
            Arc::new(NoModel),
            &mut Events((1..=4).map(event).collect::<Vec<_>>().into_iter()),
            ReplayLimits {
                events,
                evaluations,
                attempts: 0,
            },
            || cancelled,
        ))
        .unwrap();
        assert_eq!((report.events, report.evaluations, report.end), expected);
        assert_eq!(report.attempts, 0);
        assert!(store.dispatches.lock().unwrap().is_empty());
    }
}
