use super::*;
use crate::fit::LocalPreparation;
use crate::preparation::{ConditionOutcome, evaluate_trigger_condition};

impl<S: CheckStore + 'static, E: DecisionEvaluator> CheckEngine<S, E> {
    pub(super) fn start_local<'task>(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        pending: PendingCandidate,
        claimed: ClaimedCandidate,
        mut guard: InFlightGuard,
        evaluator: Arc<E>,
    ) -> Result<StartOutcome<'task>, StoreError>
    where
        S: 'task,
        E: 'task,
    {
        let key = guard.key.clone();
        if !evaluator.local_execution_is_current(activation) {
            match self.store.release_pre_dispatch_claim(&claimed) {
                Ok(ClaimOutcome::Current) => self.restore_candidate(key, pending),
                Ok(ClaimOutcome::Stale) => {
                    self.clear_ticket_if_idle(&key);
                    guard.finish();
                    return Ok(StartOutcome::Stale);
                }
                Err(error) => {
                    return Err(self.restore_pre_dispatch(key, pending, claimed, guard, error));
                }
            }
            guard.finish();
            return Ok(StartOutcome::NotDue);
        }
        if let Some(failure) = local_condition_failure(&pending.compiled, &pending.candidate) {
            let recorded = match self.store.record_preparation_rejection(&claimed, &failure) {
                Ok(recorded) => recorded,
                Err(error) => {
                    return Err(self.restore_pre_dispatch(key, pending, claimed, guard, error));
                }
            };
            self.clear_ticket_if_idle(&key);
            guard.finish();
            let outcome = if recorded == ClaimOutcome::Stale {
                RunOutcome::Stale
            } else if failure.code == PreparationFailureCode::ConditionFalse {
                RunOutcome::Skipped
            } else {
                RunOutcome::NotAssessed(failure.detail)
            };
            return Ok(StartOutcome::Started(Box::pin(async move {
                Ok(RunSettlement {
                    outcome,
                    attempts: 0,
                    new_intents: 0,
                })
            })));
        }
        let prepared = LocalPreparation::new(now_unix_ms);
        let cooldown = if self.policy == ExecutionPolicy::SequentialReplay {
            std::time::Duration::ZERO
        } else {
            pending.compiled.trigger.cooldown
        };
        match self
            .store
            .record_local_start(&claimed, now_unix_ms, cooldown, &prepared)
        {
            Ok(ClaimOutcome::Current) => {}
            Ok(ClaimOutcome::Stale) => {
                self.clear_ticket_if_idle(&key);
                guard.finish();
                return Ok(StartOutcome::Stale);
            }
            Err(error) => {
                return Err(self.restore_pre_dispatch(key, pending, claimed, guard, error));
            }
        }
        self.mark_started(&key, now_unix_ms, cooldown);
        let store = Arc::clone(&self.store);
        let activation = activation.clone();
        Ok(StartOutcome::Started(Box::pin(async move {
            let evaluated = EvaluatedAssessment::local();
            let mut budget = prepared.evaluation_budget;
            let outcome = if !evaluator.local_execution_is_current(&activation) {
                RunOutcome::Deferred
            } else {
                match plan_status_actions_with_budget(
                    &pending.compiled.actions,
                    &pending.candidate.context.value,
                    &evaluated,
                    claimed.prior_status,
                    &mut budget,
                ) {
                    Ok(actions) => {
                        if !evaluator.local_execution_is_current(&activation) {
                            RunOutcome::Deferred
                        } else {
                            match store
                                .accept(&AssessmentAcceptance {
                                    claimed,
                                    evaluated,
                                    actions,
                                })
                                .map_err(local_task_error)?
                            {
                                AcceptanceOutcome::Accepted
                                | AcceptanceOutcome::AlreadyAccepted => RunOutcome::Accepted,
                                AcceptanceOutcome::Deferred => RunOutcome::Deferred,
                                AcceptanceOutcome::Stale => RunOutcome::Stale,
                            }
                        }
                    }
                    Err(error) => {
                        let recorded = store
                            .record_evaluation_failure(
                                &claimed,
                                &EvaluationDiagnostic {
                                    code: EvaluationFailureCode::ActionFailed,
                                    detail: error.message.clone(),
                                },
                            )
                            .map_err(local_task_error)?;
                        if recorded == ClaimOutcome::Stale {
                            RunOutcome::Stale
                        } else {
                            RunOutcome::Failed(error.message)
                        }
                    }
                }
            };
            guard.finish();
            Ok(RunSettlement {
                outcome,
                attempts: 0,
                new_intents: 0,
            })
        })))
    }
}

fn local_task_error(source: StoreError) -> EvaluationTaskError {
    EvaluationTaskError {
        source,
        attempts: 0,
        new_intents: 0,
        intent_commit_uncertain: false,
    }
}

pub(super) fn local_condition_failure(
    compiled: &crate::compiler::CompiledCheck,
    candidate: &FrozenCandidate,
) -> Option<PreparationFailure> {
    let (code, detail) = match evaluate_trigger_condition(compiled, &candidate.context.value) {
        ConditionOutcome::Matched => return None,
        ConditionOutcome::False => (
            PreparationFailureCode::ConditionFalse,
            "condition was false".into(),
        ),
        ConditionOutcome::Unavailable => (
            PreparationFailureCode::InputUnavailable,
            "Top-level condition depends on unavailable evidence.".into(),
        ),
        ConditionOutcome::Failed(detail) => (PreparationFailureCode::InvalidInput, detail),
    };
    Some(PreparationFailure {
        code,
        detail,
        report: None,
        context: Default::default(),
    })
}
