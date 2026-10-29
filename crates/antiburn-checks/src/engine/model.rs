use super::*;
use crate::check_store::{DecisionAttemptOutcome, SavedDecisionAttempt};

impl<S: CheckStore + 'static, E: DecisionEvaluator> CheckEngine<S, E> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_model<'task>(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        maximum_new_intents: u64,
        mut pending: PendingCandidate,
        claimed: ClaimedCandidate,
        mut guard: InFlightGuard,
        evaluator: Arc<E>,
    ) -> Result<StartOutcome<'task>, StoreError>
    where
        S: 'task,
        E: 'task,
    {
        let progress = match self.store.load_decision_progress(&claimed) {
            Ok(progress) => progress,
            Err(error) => {
                pending.resumed = true;
                return Err(self.restore_pre_dispatch(
                    guard.key.clone(),
                    pending,
                    claimed,
                    guard,
                    error,
                ));
            }
        };
        let validated = match validate_decision_progress(
            &pending.compiled.decisions,
            &pending.compiled.question_requirements(),
            &progress,
        ) {
            Ok(validated) => validated,
            Err(error) => {
                let result = self.store.record_evaluation_failure(
                    &claimed,
                    &EvaluationDiagnostic {
                        code: EvaluationFailureCode::InvalidDecision,
                        detail: error.message.clone(),
                    },
                );
                guard.finish();
                let attempts = u64::try_from(progress.attempts.len()).unwrap_or(u64::MAX);
                return Ok(StartOutcome::Started(Box::pin(async move {
                    let recorded = result.map_err(|source| EvaluationTaskError {
                        source,
                        attempts,
                        new_intents: 0,
                        intent_commit_uncertain: false,
                    })?;
                    Ok(RunSettlement {
                        outcome: if recorded == ClaimOutcome::Stale {
                            RunOutcome::Stale
                        } else {
                            RunOutcome::Failed(error.message)
                        },
                        attempts,
                        new_intents: 0,
                    })
                })));
            }
        };
        let attempts = validated.attempts;
        pending.resumed |= attempts > 0;
        let purpose = if matches!(validated.state, DecisionProgressState::Complete(_)) {
            PreparationValidationPurpose::Publication
        } else {
            PreparationValidationPurpose::Dispatch
        };
        let prepared = match self.prepare_claimed(
            activation,
            &pending,
            &claimed,
            evaluator.as_ref(),
            attempts,
            purpose,
        ) {
            Ok(PreflightOutcome::Ready(prepared)) => prepared,
            Ok(PreflightOutcome::Settled(settlement)) => {
                self.clear_ticket_if_idle(&guard.key);
                guard.finish();
                return Ok(StartOutcome::Started(Box::pin(
                    async move { Ok(settlement) },
                )));
            }
            Ok(PreflightOutcome::Stale) => {
                guard.finish();
                return Ok(StartOutcome::Stale);
            }
            Err(error) => {
                let source =
                    self.restore_pre_dispatch(guard.key.clone(), pending, claimed, guard, error);
                return Ok(StartOutcome::Started(Box::pin(async move {
                    Err(EvaluationTaskError {
                        source,
                        attempts,
                        new_intents: 0,
                        intent_commit_uncertain: false,
                    })
                })));
            }
        };
        let cooldown = if self.policy == ExecutionPolicy::SequentialReplay {
            std::time::Duration::ZERO
        } else {
            pending.compiled.trigger.cooldown
        };
        let release_store = Arc::clone(&self.store);
        let release_state = Arc::clone(&self.state);
        let release_claimed = claimed.clone();
        let mut release_pending = pending.clone();
        release_pending.resumed = true;
        let release_key = guard.key.clone();
        guard.release_branch_on_drop = true;
        guard.safe_release = Some(Box::new(move || {
            let released = release_store.release_pre_dispatch_claim(&release_claimed);
            let mut state = release_state
                .lock()
                .expect("scheduler mutex is not poisoned");
            match released {
                Ok(ClaimOutcome::Stale) => {}
                Ok(ClaimOutcome::Current) => {
                    restore_pending(&mut state, release_key, release_pending)
                }
                Err(_) => {
                    state
                        .pending_releases
                        .insert(release_key.clone(), release_claimed);
                    restore_pending(&mut state, release_key, release_pending);
                }
            }
        }));
        let store = Arc::clone(&self.store);
        let activation = activation.clone();
        let policy = self.policy;
        Ok(StartOutcome::Started(Box::pin(async move {
            let mut counts = TaskCounts {
                attempts,
                new_intents: 0,
            };
            let result = run_model(
                store.as_ref(),
                evaluator.as_ref(),
                &activation,
                &pending.candidate,
                &claimed,
                &prepared,
                progress,
                maximum_new_intents,
                now_unix_ms,
                cooldown,
                policy,
                &mut guard,
                &mut counts,
            )
            .await;
            match result {
                Ok(outcome) => {
                    guard.finish();
                    Ok(RunSettlement {
                        outcome,
                        attempts: counts.attempts,
                        new_intents: counts.new_intents,
                    })
                }
                Err(error) => Err(error),
            }
        })))
    }
}

struct TaskCounts {
    attempts: u64,
    new_intents: u64,
}

impl TaskCounts {
    fn error(&self, source: StoreError) -> EvaluationTaskError {
        EvaluationTaskError {
            source,
            attempts: self.attempts,
            new_intents: self.new_intents,
            intent_commit_uncertain: false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_model<S: CheckStore, E: DecisionEvaluator>(
    store: &S,
    evaluator: &E,
    activation: &EffectiveActivation,
    candidate: &FrozenCandidate,
    claimed: &ClaimedCandidate,
    prepared: &PreparedAssessment,
    mut progress: DecisionProgress,
    maximum_new_intents: u64,
    now_unix_ms: u64,
    cooldown: std::time::Duration,
    policy: ExecutionPolicy,
    guard: &mut InFlightGuard,
    counts: &mut TaskCounts,
) -> Result<RunOutcome, EvaluationTaskError> {
    let requirements = activation.compiled.question_requirements();
    let mut first_intent = true;
    let mut last_failure = None;
    loop {
        let validated =
            validate_decision_progress(&activation.compiled.decisions, &requirements, &progress)
                .map_err(|source| counts.error(source))?;
        if validated.attempts != counts.attempts {
            return Err(counts.error(StoreError {
                message: "saved attempt count changed during evaluation".into(),
            }));
        }
        let (attempt, unfinished_decisions) = match validated.state {
            DecisionProgressState::Complete(evaluated) => {
                guard.release_branch_on_drop = true;
                guard.safe_release = None;
                return publish_model(store, evaluator, activation, claimed, prepared, evaluated)
                    .map_err(|source| counts.error(source));
            }
            DecisionProgressState::Terminal(terminal) => {
                guard.release_branch_on_drop = false;
                let diagnostic = last_failure
                    .take()
                    .unwrap_or_else(|| terminal_diagnostic(terminal));
                let detail = diagnostic.detail.clone();
                let recorded = store
                    .record_evaluation_failure(claimed, &diagnostic)
                    .map_err(|source| counts.error(source))?;
                return Ok(if recorded == ClaimOutcome::Stale {
                    RunOutcome::Stale
                } else {
                    RunOutcome::Failed(detail)
                });
            }
            DecisionProgressState::Next {
                attempt,
                unfinished_decisions,
            } => (attempt, unfinished_decisions),
        };
        let remaining = maximum_new_intents.saturating_sub(counts.new_intents);
        if remaining == 0 || (first_intent && remaining < unfinished_decisions) {
            let recorded = store
                .record_not_assessed(claimed, "Replay provider attempt limit reached.")
                .map_err(|source| counts.error(source))?;
            return Ok(if recorded == ClaimOutcome::Stale {
                RunOutcome::Stale
            } else {
                RunOutcome::LimitReached
            });
        }
        if let Err(failure) = evaluator.validate_prepared(
            activation,
            prepared,
            PreparationValidationPurpose::Dispatch,
        ) {
            let failure = bounded_failure(failure);
            let recorded = store
                .record_preparation_rejection(claimed, &failure)
                .map_err(|source| counts.error(source))?;
            return Ok(if recorded == ClaimOutcome::Stale {
                RunOutcome::Stale
            } else {
                RunOutcome::NotAssessed(failure.detail)
            });
        }
        // A failed intent write can have committed. Keep the branch reserved.
        guard.release_branch_on_drop = false;
        match store.record_dispatch_intent(claimed, now_unix_ms, cooldown, &attempt) {
            Ok(ClaimOutcome::Current) => {}
            Ok(ClaimOutcome::Stale) => return Ok(RunOutcome::Stale),
            Err(source) => {
                let mut error = counts.error(source);
                error.intent_commit_uncertain = true;
                return Err(error);
            }
        }
        counts.attempts += 1;
        counts.new_intents += 1;
        if first_intent && counts.attempts == 1 {
            mark_scheduler_started(&guard.state, &guard.key, now_unix_ms, cooldown, policy);
        }
        first_intent = false;
        let evaluated = evaluator
            .evaluate_prepared(activation, candidate, prepared, &attempt)
            .await;
        let (recorded, outcome, diagnostic) =
            settle_decision_attempt(store, activation, claimed, &attempt, evaluated, &progress)
                .map_err(|source| counts.error(source))?;
        if recorded == ClaimOutcome::Stale {
            return Ok(RunOutcome::Stale);
        }
        last_failure = diagnostic;
        progress
            .attempts
            .push(SavedDecisionAttempt { attempt, outcome });
        let settled =
            validate_decision_progress(&activation.compiled.decisions, &requirements, &progress)
                .map_err(|source| counts.error(source))?;
        match settled.state {
            DecisionProgressState::Next { .. } => guard.release_branch_on_drop = true,
            DecisionProgressState::Complete(_) => {
                guard.release_branch_on_drop = true;
                guard.safe_release = None;
            }
            DecisionProgressState::Terminal(_) => continue,
        }
        let mut yielded = false;
        std::future::poll_fn(|context| {
            if yielded {
                std::task::Poll::Ready(())
            } else {
                yielded = true;
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
        .await;
    }
}

fn settle_decision_attempt<S: CheckStore>(
    store: &S,
    activation: &EffectiveActivation,
    claimed: &ClaimedCandidate,
    attempt: &DecisionAttempt,
    evaluated: Result<DecisionResult, EvaluationFailure>,
    progress: &DecisionProgress,
) -> Result<
    (
        ClaimOutcome,
        DecisionAttemptOutcome,
        Option<EvaluationDiagnostic>,
    ),
    StoreError,
> {
    let evaluated =
        evaluated.and_then(|answer| validate_answer(activation, attempt, answer, progress));
    match evaluated {
        Ok(answer) => {
            let recorded = store.record_decision_answer(claimed, attempt, &answer)?;
            Ok((recorded, DecisionAttemptOutcome::Answer(answer), None))
        }
        Err(failure) => {
            let diagnostic = EvaluationDiagnostic {
                code: failure.code,
                detail: failure.detail,
            };
            let recorded =
                store.record_attempt_failure(claimed, attempt, failure.kind, &diagnostic)?;
            let outcome = match failure.kind {
                AttemptFailure::SafeTransient => DecisionAttemptOutcome::SafeTransient,
                AttemptFailure::Unknown => DecisionAttemptOutcome::Unknown,
                AttemptFailure::Permanent => DecisionAttemptOutcome::Permanent,
            };
            Ok((recorded, outcome, Some(diagnostic)))
        }
    }
}

fn validate_answer(
    activation: &EffectiveActivation,
    attempt: &DecisionAttempt,
    answer: DecisionResult,
    progress: &DecisionProgress,
) -> Result<DecisionResult, EvaluationFailure> {
    let invalid = |detail| EvaluationFailure {
        code: EvaluationFailureCode::InvalidDecision,
        kind: AttemptFailure::Permanent,
        detail,
        attempts: attempt.number,
    };
    let block = activation
        .compiled
        .decisions
        .iter()
        .find(|block| block.name == attempt.decision)
        .expect("validated progress names a declared decision");
    answer
        .validate_for_declaration(block, &activation.compiled.question_requirements())
        .map_err(|error| invalid(error.to_string()))?;
    if answer.attempts() != attempt.number {
        return Err(invalid(
            "The answer attempt count does not match its durable intent.".into(),
        ));
    }
    let previous = progress
        .attempts
        .iter()
        .filter_map(|row| match &row.outcome {
            DecisionAttemptOutcome::Answer(answer) => Some(answer.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    crate::results::measure_decision_results_with_next(&previous, &answer)
        .map_err(|error| invalid(error.to_string()))?;
    Ok(answer)
}

pub(super) fn publish_model<S: CheckStore, E: DecisionEvaluator>(
    store: &S,
    evaluator: &E,
    activation: &EffectiveActivation,
    claimed: &ClaimedCandidate,
    prepared: &PreparedAssessment,
    evaluated: EvaluatedAssessment,
) -> Result<RunOutcome, StoreError> {
    if let Err(failure) = evaluator.validate_prepared(
        activation,
        prepared,
        PreparationValidationPurpose::Publication,
    ) {
        let failure = bounded_failure(failure);
        let recorded = store.record_preparation_rejection(claimed, &failure)?;
        return Ok(if recorded == ClaimOutcome::Stale {
            RunOutcome::Stale
        } else {
            RunOutcome::NotAssessed(failure.detail)
        });
    }
    let mut budget = prepared.evaluation_budget.clone();
    let actions = match plan_status_actions_with_budget(
        &activation.compiled.actions,
        &claimed.candidate.context.value,
        &evaluated,
        claimed.prior_status,
        &mut budget,
    ) {
        Ok(actions) => actions,
        Err(error) => {
            let recorded = store.record_evaluation_failure(
                claimed,
                &EvaluationDiagnostic {
                    code: EvaluationFailureCode::ActionFailed,
                    detail: error.message.clone(),
                },
            )?;
            return Ok(if recorded == ClaimOutcome::Stale {
                RunOutcome::Stale
            } else {
                RunOutcome::Failed(error.message)
            });
        }
    };
    Ok(
        match store.accept(&AssessmentAcceptance {
            claimed: claimed.clone(),
            evaluated,
            actions,
        })? {
            AcceptanceOutcome::Accepted | AcceptanceOutcome::AlreadyAccepted => {
                RunOutcome::Accepted
            }
            AcceptanceOutcome::Deferred => RunOutcome::Deferred,
            AcceptanceOutcome::Stale => RunOutcome::Stale,
        },
    )
}

pub(super) fn incompatible_preparation(detail: &str) -> PreparationFailure {
    PreparationFailure {
        code: PreparationFailureCode::IncompatibleProfile,
        detail: detail.into(),
        report: None,
        context: Default::default(),
    }
}

pub(super) fn bounded_failure(failure: PreparationFailure) -> PreparationFailure {
    match crate::preparation::bounded_preparation_record(PreparationRecord::Rejected(failure)) {
        PreparationRecord::Rejected(failure) => failure,
        _ => unreachable!("a rejected record remains rejected"),
    }
}

pub(super) fn prepare_all_inputs(
    compiled: &crate::compiler::CompiledCheck,
    candidate: &FrozenCandidate,
) -> Result<(Vec<DecisionInput>, EvaluationBudget), PreparationFailure> {
    if let Some(failure) = local::local_condition_failure(compiled, candidate) {
        return Err(failure);
    }
    crate::results::measure_result_capacity(&compiled.decisions, &compiled.question_requirements())
        .map_err(|error| incompatible_preparation(&error.to_string()))?;
    let mut budget = EvaluationBudget::default();
    let mut inputs = Vec::new();
    for block in &compiled.decisions {
        let (code, detail) = match prepare_decision_input_with_budget(block, candidate, &mut budget)
        {
            PreparationOutcome::Ready(input) => {
                inputs.push(DecisionInput {
                    decision: block.name.clone(),
                    input,
                });
                continue;
            }
            PreparationOutcome::Unavailable(detail) => {
                (PreparationFailureCode::InputUnavailable, detail)
            }
            PreparationOutcome::Failed(detail) => (PreparationFailureCode::InvalidInput, detail),
            PreparationOutcome::ResourceLimit(detail) => {
                (PreparationFailureCode::PreparationResourceLimit, detail)
            }
        };
        return Err(PreparationFailure {
            code,
            detail,
            report: None,
            context: PreparationFailureContext {
                decision: Some(block.name.clone()),
                field: Some(format!("decisions.{}.input", block.name)),
                ..Default::default()
            }
            .into(),
        });
    }
    Ok((inputs, budget))
}

fn terminal_diagnostic(
    terminal: crate::check_store::DecisionProgressTerminal,
) -> EvaluationDiagnostic {
    use crate::check_store::DecisionProgressTerminal;
    let (code, detail) = match terminal {
        DecisionProgressTerminal::UnresolvedIntent => (
            EvaluationFailureCode::DeliveryUnknown,
            "An earlier decision dispatch has no settled delivery outcome. Automatic retry is blocked.",
        ),
        DecisionProgressTerminal::Unknown => (
            EvaluationFailureCode::DeliveryUnknown,
            "Decision delivery is unknown. Automatic retry is blocked.",
        ),
        DecisionProgressTerminal::Permanent => (
            EvaluationFailureCode::InvalidDecision,
            "Decision evaluation failed permanently.",
        ),
        DecisionProgressTerminal::AttemptsExhausted => (
            EvaluationFailureCode::ProviderUnavailable,
            "Decision retries are exhausted.",
        ),
    };
    EvaluationDiagnostic {
        code,
        detail: detail.into(),
    }
}
