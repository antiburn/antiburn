//! Pure ordered planning for independently gated status actions.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::compiler::{Condition, StatusAction, StatusResult};
use crate::diagnostics::DiagnosticCode;
use crate::expression_eval::{EvaluationError, EvaluationValue, evaluate_expression, is_truthy};

const PROBABILITY_SUM_TOLERANCE: f64 = 0.02;

/// A validated result for one declared Choice question.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChoiceResult {
    decision: String,
    question: String,
    output: String,
    probabilities: BTreeMap<String, f64>,
}

impl<'de> Deserialize<'de> for ChoiceResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct SavedChoice {
            decision: String,
            question: String,
            output: String,
            probabilities: BTreeMap<String, f64>,
        }
        let saved = SavedChoice::deserialize(deserializer)?;
        let labels = saved.probabilities.keys().cloned().collect::<Vec<_>>();
        Self::validate(
            saved.decision,
            saved.question,
            &labels,
            saved.output,
            saved.probabilities,
        )
        .map_err(|error| serde::de::Error::custom(error.message))
    }
}

impl ChoiceResult {
    /// Validates a selected label and its complete probability map.
    ///
    /// Every declared label must have one finite probability in `[0, 1]`, and
    /// the absolute error of their sum from one must not exceed `0.02` under
    /// `f64` arithmetic. This matches the supported TypeSafe wire contract.
    /// Keep each probability unchanged; action thresholds read the raw values.
    /// The selected output must match a declared label exactly. Labels have no
    /// implicit behavior.
    ///
    /// # Errors
    /// Returns an error if identifiers, labels, probabilities, or their sum
    /// violate the Choice contract.
    pub fn validate(
        decision: impl Into<String>,
        question: impl Into<String>,
        labels: &[String],
        output: impl Into<String>,
        probabilities: BTreeMap<String, f64>,
    ) -> Result<Self, ChoiceResultError> {
        let decision = decision.into();
        let question = question.into();
        let output = output.into();
        let expected = labels.iter().collect::<BTreeSet<_>>();
        if decision.is_empty() || question.is_empty() {
            return Err(choice_error(
                "Choice decision and question identifiers must be nonempty.",
            ));
        }
        if labels.len() < 2 || expected.len() != labels.len() {
            return Err(choice_error("Choice requires at least two unique labels."));
        }
        if !expected.contains(&output) {
            return Err(choice_error("Choice output is not a declared label."));
        }
        if probabilities.len() != expected.len()
            || probabilities.keys().any(|label| !expected.contains(label))
        {
            return Err(choice_error(
                "Choice probabilities must contain exactly the declared labels.",
            ));
        }
        let mut sum = 0.0;
        for probability in probabilities.values() {
            if !probability.is_finite() || !(0.0..=1.0).contains(probability) {
                return Err(choice_error(
                    "Choice probabilities must be finite values from zero to one.",
                ));
            }
            sum += probability;
        }
        if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
            return Err(choice_error(
                "Choice probability sum must differ from one by no more than 0.02.",
            ));
        }
        Ok(Self {
            decision,
            question,
            output,
            probabilities,
        })
    }

    /// Checks recovered results against the current compiled declaration.
    pub fn validate_declaration(
        &self,
        decision: &crate::compiler::ChoiceDecision,
    ) -> Result<(), ChoiceResultError> {
        if self.decision != decision.name || self.question != decision.question_id {
            return Err(choice_error(
                "Choice result does not match the declared question.",
            ));
        }
        Self::validate(
            self.decision.clone(),
            self.question.clone(),
            &decision
                .criteria
                .iter()
                .map(|(label, _)| label.clone())
                .collect::<Vec<_>>(),
            self.output.clone(),
            self.probabilities.clone(),
        )
        .map(|_| ())
    }
}

/// A validation failure for a provider Choice result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceResultError {
    /// The stable diagnostic code for invalid decision evidence.
    pub code: DiagnosticCode,
    /// A short explanation of the invalid result.
    pub message: String,
}

/// A failure while building a frozen action-planning context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionPlanningError {
    /// The stable diagnostic code associated with the invalid context.
    pub code: DiagnosticCode,
    /// A short explanation of why planning could not start.
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
        /// The fixed explanatory text, when configured.
        message: Option<String>,
    },
    /// A condition evaluated to a false-like available value.
    SkippedConditionFalse,
    /// A condition depended on an unavailable context value.
    SkippedConditionUnavailable,
    /// Expression evaluation failed, for example because a resource limit ran out.
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
/// Conditions read the same cloned context with validated Choice results exposed
/// under `decisions`. A skipped or failed action does not block later actions.
/// Valid status writes apply in order, so the last valid write sets the planned
/// status. This function does not persist results or send user-facing messages.
///
/// # Errors
/// Returns an error if the frozen root is not an object, it already contains a
/// `decisions` root, or Choice results repeat a decision name.
pub fn plan_status_actions(
    actions: &[StatusAction],
    frozen_context: &Value,
    choices: &[ChoiceResult],
    prior_status: Option<StatusResult>,
) -> Result<ActionPlan, ActionPlanningError> {
    let context = context_with_choices(frozen_context, choices)?;
    let mut current_status = prior_status;
    let mut outcomes = Vec::with_capacity(actions.len());
    for (index, action) in actions.iter().enumerate() {
        let outcome = match action.condition.as_ref() {
            None => apply_action(action, &mut current_status),
            Some(Condition::Boolean(false)) => ActionOutcomeKind::SkippedConditionFalse,
            Some(Condition::Boolean(true)) => apply_action(action, &mut current_status),
            Some(Condition::Expression(expression)) => {
                match evaluate_expression(expression, &context) {
                    Ok(EvaluationValue::Unavailable) => {
                        ActionOutcomeKind::SkippedConditionUnavailable
                    }
                    Ok(EvaluationValue::Available(value)) if is_truthy(&value) => {
                        apply_action(action, &mut current_status)
                    }
                    Ok(EvaluationValue::Available(_)) => ActionOutcomeKind::SkippedConditionFalse,
                    Err(error) => ActionOutcomeKind::Failed(error),
                }
            }
        };
        outcomes.push(ActionOutcome { index, outcome });
    }
    Ok(ActionPlan {
        current_status,
        outcomes,
    })
}

fn apply_action(
    action: &StatusAction,
    current_status: &mut Option<StatusResult>,
) -> ActionOutcomeKind {
    *current_status = Some(action.result);
    ActionOutcomeKind::Applied {
        result: action.result,
        message: action.message.clone(),
    }
}

fn context_with_choices(
    frozen_context: &Value,
    choices: &[ChoiceResult],
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
    if choices.is_empty() {
        return Ok(Value::Object(root));
    }
    let mut decisions = Map::new();
    for choice in choices {
        if decisions.contains_key(&choice.decision) {
            return Err(planning_error(
                DiagnosticCode::RuntimeInvalidDecision,
                "Choice results must have unique decision names.",
            ));
        }
        let probabilities = choice
            .probabilities
            .iter()
            .map(|(label, probability)| {
                let number = Number::from_f64(*probability)
                    .expect("validated Choice probabilities are finite");
                (label.clone(), Value::Number(number))
            })
            .collect::<Map<_, _>>();
        let mut details = Map::new();
        details.insert(
            choice.question.clone(),
            Value::Object(Map::from_iter([(
                "probabilities".to_owned(),
                Value::Object(probabilities),
            )])),
        );
        let mut outputs = Map::new();
        outputs.insert(
            choice.question.clone(),
            Value::String(choice.output.clone()),
        );
        decisions.insert(
            choice.decision.clone(),
            Value::Object(Map::from_iter([
                ("outputs".to_owned(), Value::Object(outputs)),
                ("details".to_owned(), Value::Object(details)),
            ])),
        );
    }
    root.insert("decisions".to_owned(), Value::Object(decisions));
    Ok(Value::Object(root))
}

fn choice_error(message: &str) -> ChoiceResultError {
    ChoiceResultError {
        code: DiagnosticCode::RuntimeInvalidDecision,
        message: message.to_owned(),
    }
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
    use serde_json::json;

    fn choice(probabilities: BTreeMap<String, f64>) -> Result<ChoiceResult, ChoiceResultError> {
        ChoiceResult::validate(
            "review",
            "verdict",
            &["stuck".into(), "clear".into(), "uncertain".into()],
            "stuck",
            probabilities,
        )
    }

    #[test]
    fn rounded_probability_sums_are_accepted_without_changing_action_thresholds() {
        let check = crate::compiler::compile(include_str!(
            "../../../docs/burn-check/examples/failed-retry-loop.yml"
        ))
        .unwrap();
        for (probability, expected_status) in [(0.949, None), (0.95, Some(StatusResult::Failure))] {
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
            assert_eq!(answer.probabilities, probabilities);
            let plan = plan_status_actions(&check.actions, &json!({}), &[answer], None).unwrap();
            assert_eq!(plan.current_status, expected_status);
        }
        let above_one = BTreeMap::from([
            ("stuck".into(), 0.96),
            ("clear".into(), 0.03),
            ("uncertain".into(), 0.02),
        ]);
        assert_eq!(choice(above_one.clone()).unwrap().probabilities, above_one);
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
