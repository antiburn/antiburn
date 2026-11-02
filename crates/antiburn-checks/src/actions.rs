//! Pure ordered planning for independently gated status actions.

use serde::Serialize;
use serde_json::{Map, Number, Value};

use crate::compiler::{StatusAction, StatusResult, StatusResultTemplate};
use crate::diagnostics::DiagnosticCode;
use crate::expression_eval::{
    EvaluationBindings, EvaluationBudget, EvaluationError, EvaluationTrace, EvaluationValue,
    evaluate_expression_with_budget, is_truthy,
};
use crate::preparation::{enforce_selected_membership, evaluate_condition, prepare_template};
use crate::results::{EvaluatedAssessment, QuestionResult};

/// A failure that prevents the whole action plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionPlanningError {
    /// The stable code for invalid context or exhausted shared limits.
    pub code: DiagnosticCode,
    /// A short explanation of why planning cannot complete.
    pub message: String,
}

/// The result of one configured status action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActionOutcome {
    /// The zero-based position of this action in the authored list.
    pub index: usize,
    /// The action's applied, skipped, or failed result.
    pub outcome: ActionOutcomeKind,
}

/// The applied, skipped, or failed state for one status action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ActionOutcomeKind {
    /// The condition passed and the action wrote this status result.
    Applied {
        /// The status written by this action.
        result: StatusResult,
        /// The evaluated explanatory text, when configured.
        message: Option<String>,
    },
    /// A condition evaluated to a false-like available value.
    SkippedConditionFalse,
    /// A condition depended on an unavailable context value.
    SkippedConditionUnavailable,
    /// A result or message depended on unavailable evidence.
    SkippedInputUnavailable,
    /// An ordinary expression or input error invalidated only this action.
    Failed(EvaluationError),
}

/// The complete ordered output of planning a status-action list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActionPlan {
    /// The status after applying all valid writes, or the prior status if none wrote.
    pub current_status: Option<StatusResult>,
    /// One outcome for each action, in authored order.
    pub outcomes: Vec<ActionOutcome>,
}

/// Evaluates all actions independently against one frozen context and decision set.
///
/// Conditions read the same cloned context with validated typed results exposed
/// under `decisions`. A skipped or failed action does not block later actions.
/// Valid status writes apply in order, so the last valid write sets the planned
/// status. This function does not persist results or send user-facing messages.
///
/// # Errors
/// Returns an error if the frozen root is not an object, it already contains a
/// `decisions` root, or a shared limit stops evaluation. A shared limit error
/// returns no partial plan. The caller must validate declaration binding first.
pub fn plan_status_actions(
    actions: &[StatusAction],
    frozen_context: &Value,
    assessed: &EvaluatedAssessment,
    prior_status: Option<StatusResult>,
) -> Result<ActionPlan, ActionPlanningError> {
    plan_status_actions_with_budget(
        actions,
        frozen_context,
        assessed,
        prior_status,
        &mut EvaluationBudget::default(),
    )
}

/// Plans all status actions against the assessment's shared evaluation budget.
///
/// # Errors
/// Returns an invalid-context error or a whole-assessment budget error. An error
/// returns no action plan, so earlier tentative writes cannot be accepted.
pub fn plan_status_actions_with_budget(
    actions: &[StatusAction],
    frozen_context: &Value,
    assessed: &EvaluatedAssessment,
    prior_status: Option<StatusResult>,
    budget: &mut EvaluationBudget,
) -> Result<ActionPlan, ActionPlanningError> {
    plan_status_actions_traced(actions, frozen_context, assessed, prior_status, budget)
        .map(|(plan, _)| plan)
}

/// Plans actions and retains every evaluated source dependency.
///
/// # Errors
/// Returns an invalid-context error or a whole-assessment budget error.
pub fn plan_status_actions_traced(
    actions: &[StatusAction],
    frozen_context: &Value,
    assessed: &EvaluatedAssessment,
    prior_status: Option<StatusResult>,
    budget: &mut EvaluationBudget,
) -> Result<(ActionPlan, EvaluationTrace), ActionPlanningError> {
    budget
        .step(1)
        .map_err(|error| planning_error(error.code, &error.message))?;
    let context = context_with_assessment(frozen_context, assessed)?;
    let mut current_status = prior_status;
    let mut outcomes = Vec::new();
    let mut trace = EvaluationTrace::default();
    for (index, action) in actions.iter().enumerate() {
        let result = plan_action(action, &context, budget, &mut trace);
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error)
                if error.code == DiagnosticCode::EvaluationLimitExceeded || budget.exhausted() =>
            {
                return Err(planning_error(
                    DiagnosticCode::EvaluationLimitExceeded,
                    &error.message,
                ));
            }
            Err(error) => ActionOutcomeKind::Failed(error),
        };
        if let ActionOutcomeKind::Applied { result, .. } = &outcome {
            current_status = Some(*result);
        }
        outcomes.push(ActionOutcome { index, outcome });
    }
    Ok((
        ActionPlan {
            current_status,
            outcomes,
        },
        trace,
    ))
}

fn plan_action(
    action: &StatusAction,
    context: &Value,
    budget: &mut EvaluationBudget,
    trace: &mut EvaluationTrace,
) -> Result<ActionOutcomeKind, EvaluationError> {
    budget.step(1)?;
    let mut bindings = EvaluationBindings::new();
    if let Some(condition) = &action.condition {
        let evaluated = evaluate_condition(condition, context, &bindings, budget, 1)?;
        trace.merge(&evaluated.trace);
        match evaluated.value {
            EvaluationValue::Unavailable => {
                return Ok(ActionOutcomeKind::SkippedConditionUnavailable);
            }
            EvaluationValue::Available(value) if !is_truthy(&value) => {
                return Ok(ActionOutcomeKind::SkippedConditionFalse);
            }
            EvaluationValue::Available(_) => {}
        }
    }
    let result = match &action.result {
        StatusResultTemplate::Literal(result) => *result,
        StatusResultTemplate::Expression(expression) => {
            let evaluated =
                evaluate_expression_with_budget(expression, context, &bindings, budget, 1)?;
            let evaluated = enforce_selected_membership(evaluated, context);
            trace.merge(&evaluated.trace);
            match evaluated.value {
                EvaluationValue::Unavailable => {
                    return Ok(ActionOutcomeKind::SkippedInputUnavailable);
                }
                EvaluationValue::Available(Value::String(result)) if result == "success" => {
                    StatusResult::Success
                }
                EvaluationValue::Available(Value::String(result)) if result == "failure" => {
                    StatusResult::Failure
                }
                _ => {
                    return Err(invalid_action_input(
                        "Status result must be exactly `success` or `failure`.",
                    ));
                }
            }
        }
    };
    let message = match &action.message {
        None => None,
        Some(template) => {
            let evaluated = prepare_template(template, context, &mut bindings, budget, 1)?;
            trace.merge(&evaluated.trace);
            match evaluated.value {
                EvaluationValue::Unavailable => {
                    return Ok(ActionOutcomeKind::SkippedInputUnavailable);
                }
                EvaluationValue::Available(Value::String(message))
                    if !message.trim().is_empty() =>
                {
                    Some(message)
                }
                _ => {
                    return Err(invalid_action_input(
                        "Status message must be a nonblank string.",
                    ));
                }
            }
        }
    };
    Ok(ActionOutcomeKind::Applied { result, message })
}

fn invalid_action_input(message: &str) -> EvaluationError {
    EvaluationError {
        code: DiagnosticCode::RuntimeInvalidContext,
        message: message.to_owned(),
    }
}

fn context_with_assessment(
    frozen_context: &Value,
    assessed: &EvaluatedAssessment,
) -> Result<Value, ActionPlanningError> {
    let Some(context) = frozen_context.as_object() else {
        return Err(ActionPlanningError {
            code: DiagnosticCode::RuntimeInvalidContext,
            message: "Frozen expression context must be a JSON object.".to_owned(),
        });
    };
    if context.contains_key("decisions") {
        return Err(ActionPlanningError {
            code: DiagnosticCode::RuntimeInvalidContext,
            message: "Frozen context must not contain a `decisions` root.".to_owned(),
        });
    }
    let mut root = context.clone();
    if assessed.decisions().is_empty() {
        return Ok(Value::Object(root));
    }
    let mut decisions = Map::new();
    for decision in assessed.decisions() {
        let mut details = Map::new();
        let mut outputs = Map::new();
        for answer in decision.answers() {
            let mut question_details = Map::new();
            let output = match answer.result() {
                QuestionResult::Choice(choice) => {
                    if let Some(probabilities) = choice.probabilities() {
                        question_details.insert(
                            "probabilities".into(),
                            Value::Object(
                                probabilities
                                    .iter()
                                    .map(|(label, value)| (label.clone(), finite_number(*value)))
                                    .collect(),
                            ),
                        );
                    }
                    if let Some(confidence) = choice.confidence() {
                        question_details.insert("confidence".into(), finite_number(confidence));
                    }
                    Value::String(choice.output().into())
                }
                QuestionResult::Score(score) => {
                    if let Some(probabilities) = score.probabilities() {
                        question_details.insert(
                            "probabilities".into(),
                            Value::Array(
                                probabilities
                                    .iter()
                                    .map(|value| finite_number(*value))
                                    .collect(),
                            ),
                        );
                    }
                    if let Some(confidence) = score.confidence() {
                        question_details.insert("confidence".into(), finite_number(confidence));
                    }
                    finite_number(score.output())
                }
                QuestionResult::Noul(noul) => finite_number(noul.output()),
            };
            details.insert(answer.name().into(), Value::Object(question_details));
            outputs.insert(answer.name().into(), output);
        }
        decisions.insert(
            decision.decision().into(),
            Value::Object(Map::from_iter([
                ("outputs".to_owned(), Value::Object(outputs)),
                ("details".to_owned(), Value::Object(details)),
            ])),
        );
    }
    root.insert("decisions".to_owned(), Value::Object(decisions));
    Ok(Value::Object(root))
}

fn finite_number(value: f64) -> Value {
    Value::Number(Number::from_f64(value).expect("validated results contain only finite numbers"))
}

fn planning_error(code: DiagnosticCode, message: &str) -> ActionPlanningError {
    ActionPlanningError {
        code,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod probability_tests {
    use super::*;
    use crate::compiler::CompiledCheck;
    use crate::expression_eval::evaluate_expression;
    use crate::results::{ChoiceResult, DecisionResult, NamedAnswer, ResultError};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn check() -> CompiledCheck {
        crate::compiler::compile(include_str!("../tests/fixtures/retry-review.yml")).unwrap()
    }

    fn choice(probabilities: BTreeMap<String, f64>) -> Result<ChoiceResult, ResultError> {
        ChoiceResult::new(
            &check().decisions[0].outputs[0].question,
            "stuck",
            Some(probabilities),
            None,
        )
    }

    fn assessment(check: &CompiledCheck, choice: ChoiceResult) -> EvaluatedAssessment {
        let required = check.question_requirements();
        let decision = &check.decisions[0];
        let answer =
            NamedAnswer::new(&decision.outputs[0].name, QuestionResult::Choice(choice)).unwrap();
        let result = DecisionResult::new(decision, &required, vec![answer], 1).unwrap();
        EvaluatedAssessment::new(&check.decisions, &required, vec![result]).unwrap()
    }

    #[test]
    fn label_only_context_keeps_probabilities_and_confidence_unavailable() {
        let mut check = check();
        check.actions.clear();
        let choice =
            ChoiceResult::new(&check.decisions[0].outputs[0].question, "stuck", None, None)
                .unwrap();
        let context = context_with_assessment(&json!({}), &assessment(&check, choice)).unwrap();
        assert_eq!(
            context["decisions"]["review"]["outputs"]["verdict"],
            "stuck"
        );
        let details = context["decisions"]["review"]["details"]["verdict"]
            .as_object()
            .unwrap();
        assert!(!details.contains_key("probabilities"));
        assert!(!details.contains_key("confidence"));
        for path in [
            "decisions.review.details.verdict.probabilities",
            "decisions.review.details.verdict.probabilities.stuck",
            "decisions.review.details.verdict.confidence",
        ] {
            assert_eq!(
                evaluate_expression(
                    &crate::expression::parse_expression(path).unwrap(),
                    &context
                )
                .unwrap(),
                EvaluationValue::Unavailable,
            );
        }
    }

    #[test]
    fn rounded_probability_sums_are_accepted_without_changing_action_thresholds() {
        let check = check();
        for (probability, expected_status) in [
            (0.949, Some(StatusResult::Success)),
            (0.95, Some(StatusResult::Failure)),
        ] {
            let probabilities = BTreeMap::from([
                ("stuck".into(), probability),
                ("clear".into(), 0.03),
                (
                    "uncertain".into(),
                    if probability == 0.95 { 0.01 } else { 0.011 },
                ),
            ]);
            let answer =
                choice(probabilities.clone()).expect("a 0.99 sum meets the provider contract");
            assert_eq!(answer.probabilities(), Some(&probabilities));
            let plan = plan_status_actions(
                &check.actions,
                &json!({}),
                &assessment(&check, answer),
                None,
            )
            .unwrap();
            assert_eq!(plan.current_status, expected_status);
        }
        let above_one = BTreeMap::from([
            ("stuck".into(), 0.96),
            ("clear".into(), 0.03),
            ("uncertain".into(), 0.02),
        ]);
        assert_eq!(
            choice(above_one.clone()).unwrap().probabilities(),
            Some(&above_one)
        );
    }

    #[test]
    fn sums_outside_provider_tolerance_and_invalid_individual_values_are_rejected() {
        for probabilities in [
            BTreeMap::from([
                ("stuck".into(), 0.90),
                ("clear".into(), 0.04),
                ("uncertain".into(), 0.01),
            ]),
            BTreeMap::from([
                ("stuck".into(), 0.99),
                ("clear".into(), 0.03),
                ("uncertain".into(), 0.01),
            ]),
            BTreeMap::from([
                ("stuck".into(), f64::NAN),
                ("clear".into(), 0.0),
                ("uncertain".into(), 0.0),
            ]),
            BTreeMap::from([
                ("stuck".into(), 1.01),
                ("clear".into(), 0.0),
                ("uncertain".into(), 0.0),
            ]),
            BTreeMap::from([
                ("stuck".into(), 1.0),
                ("clear".into(), -0.01),
                ("uncertain".into(), 0.01),
            ]),
        ] {
            assert!(choice(probabilities).is_err());
        }
    }
}
