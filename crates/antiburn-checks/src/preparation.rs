//! Pure preparation of a frozen candidate for one compiled decision.

use serde_json::{Map, Value};

use crate::compiler::{CompiledCheck, Condition, InputLiteral, InputTemplate};
use crate::domain::FrozenCandidate;
use crate::expression_eval::{EvaluationValue, evaluate_expression, is_truthy};

/// Result of checking one candidate's preconditions and preparing its input.
#[derive(Clone, Debug, PartialEq)]
pub enum PreparationOutcome {
    /// The candidate passed preconditions and has a complete typed input.
    Ready(Value),
    /// A known false condition makes this trigger ineligible.
    Skipped,
    /// A required condition or input reads unavailable evidence.
    Unavailable(String),
    /// Expression evaluation exceeded a runtime limit or failed.
    Failed(String),
}

/// Result of the pre-selection condition on one trigger context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConditionOutcome {
    Matched,
    False,
    Unavailable,
    Failed(String),
}

/// Evaluates the top-level condition before window selection and scheduling.
#[must_use]
pub fn evaluate_trigger_condition(check: &CompiledCheck, context: &Value) -> ConditionOutcome {
    let Some(condition) = &check.condition else {
        return ConditionOutcome::Matched;
    };
    match evaluate_condition(condition, context) {
        Ok(EvaluationValue::Available(value)) if is_truthy(&value) => ConditionOutcome::Matched,
        Ok(EvaluationValue::Available(_)) => ConditionOutcome::False,
        Ok(EvaluationValue::Unavailable) => ConditionOutcome::Unavailable,
        Err(error) => ConditionOutcome::Failed(error.message),
    }
}

/// Checks the top-level condition and recursively prepares the typed input.
///
/// The function reads only the frozen candidate context. It does not call a
/// provider or mutate the candidate. A missing path never becomes JSON null.
#[must_use]
pub fn prepare_decision_input(
    check: &CompiledCheck,
    candidate: &FrozenCandidate,
) -> PreparationOutcome {
    match evaluate_trigger_condition(check, &candidate.context.value) {
        ConditionOutcome::Matched => {}
        ConditionOutcome::False => return PreparationOutcome::Skipped,
        ConditionOutcome::Unavailable => {
            return PreparationOutcome::Unavailable(
                "Top-level condition depends on unavailable evidence.".to_owned(),
            );
        }
        ConditionOutcome::Failed(reason) => return PreparationOutcome::Failed(reason),
    }

    let mut input = Map::new();
    for (key, template) in &check.decision.input {
        match prepare_template(template, &candidate.context.value) {
            Ok(EvaluationValue::Available(value)) => {
                input.insert(key.clone(), value);
            }
            Ok(EvaluationValue::Unavailable) => {
                return PreparationOutcome::Unavailable(format!(
                    "Decision input `{key}` depends on unavailable evidence."
                ));
            }
            Err(error) => return PreparationOutcome::Failed(error),
        }
    }
    PreparationOutcome::Ready(Value::Object(input))
}

fn evaluate_condition(
    condition: &Condition,
    context: &Value,
) -> Result<EvaluationValue, crate::expression_eval::EvaluationError> {
    match condition {
        Condition::Boolean(value) => Ok(EvaluationValue::Available(Value::Bool(*value))),
        Condition::Expression(expression) => evaluate_expression(expression, context),
    }
}

fn prepare_template(template: &InputTemplate, context: &Value) -> Result<EvaluationValue, String> {
    match template {
        InputTemplate::Literal(literal) => Ok(EvaluationValue::Available(literal_value(literal))),
        InputTemplate::Expression(expression) => {
            evaluate_expression(expression, context).map_err(|error| error.message)
        }
        InputTemplate::Sequence(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                match prepare_template(item, context)? {
                    EvaluationValue::Available(value) => values.push(value),
                    EvaluationValue::Unavailable => return Ok(EvaluationValue::Unavailable),
                }
            }
            Ok(EvaluationValue::Available(Value::Array(values)))
        }
        InputTemplate::Object(items) => {
            let mut values = Map::new();
            for (key, item) in items {
                match prepare_template(item, context)? {
                    EvaluationValue::Available(value) => {
                        values.insert(key.clone(), value);
                    }
                    EvaluationValue::Unavailable => return Ok(EvaluationValue::Unavailable),
                }
            }
            Ok(EvaluationValue::Available(Value::Object(values)))
        }
    }
}

fn literal_value(literal: &InputLiteral) -> Value {
    match literal {
        InputLiteral::Null => Value::Null,
        InputLiteral::Boolean(value) => Value::Bool(*value),
        InputLiteral::Number(value) => Value::Number(value.clone()),
        InputLiteral::String(value) => Value::String(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::compiler::compile;
    use crate::domain::{
        ActivationId, BranchId, FrozenCandidate, FrozenContext, Generation, ObservationPosition,
    };

    fn candidate(context: Value) -> FrozenCandidate {
        FrozenCandidate {
            activation: ActivationId {
                scope: crate::domain::ActivationScope::default(),
                definition_id: "failed-retry-loop".to_owned(),
                generation: Generation(1),
            },
            branch: BranchId("main-session".to_owned()),
            trigger_position: ObservationPosition(3),
            messages: Vec::new(),
            context: FrozenContext {
                value: context,
                dependencies: Vec::new(),
            },
            dependencies: Vec::new(),
            fingerprint: [0; 32],
        }
    }

    #[test]
    fn actual_example_prepares_requests_and_window_from_frozen_context() {
        let source = include_str!("../../../docs/burn-check/examples/failed-retry-loop.yml");
        let check = compile(source).expect("the maintained example compiles");
        let prepared = prepare_decision_input(
            &check,
            &candidate(json!({
                "session": {"requests": [{"id": "r1", "text": "Retry this"}], "requests_complete": true, "source_coverage": "complete"},
                "message": {"tool": {"result": {"status": "error"}}},
                "window": {"messages": [{"tool": {"result": {"status": "error"}}}]},
            })),
        );
        let PreparationOutcome::Ready(value) = prepared else {
            panic!("complete example inputs must prepare: {prepared:?}");
        };
        assert_eq!(value["requests"][0]["text"], "Retry this");
        assert_eq!(value["activity"][0]["tool"]["result"]["status"], "error");
    }

    #[test]
    fn false_condition_and_missing_input_do_not_become_provider_values() {
        let check = compile(
            "name: Test\non:\n  tool_result:\nif: false\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input:\n      value: \"${{ message.tool.result.status }}\"\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n",
        )
        .unwrap();
        assert_eq!(
            prepare_decision_input(&check, &candidate(json!({}))),
            PreparationOutcome::Skipped
        );

        let check = compile(
            "name: Test\non:\n  tool_result:\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input:\n      value: \"${{ message.tool.result.status }}\"\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n",
        )
        .unwrap();
        assert!(matches!(
            prepare_decision_input(&check, &candidate(json!({}))),
            PreparationOutcome::Unavailable(_)
        ));
    }

    #[test]
    fn recursive_inputs_preserve_null_types_and_expression_text_as_data() {
        let check = compile(
            "name: Typed input\non:\n  tool_result:\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input:\n      nested:\n        - null\n        - arguments: \"${{ message.tool.arguments }}\"\n          requests: \"${{ session.requests }}\"\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n",
        ).unwrap();
        let context = json!({
            "session": {"requests": [{"value": "${{ unavailable.path }}"}]},
            "message": {"tool": {"arguments": {"count": 3, "enabled": true, "optional": null}}},
        });
        let PreparationOutcome::Ready(input) = prepare_decision_input(&check, &candidate(context))
        else {
            panic!("complete typed input must prepare");
        };
        assert_eq!(
            input,
            json!({"nested": [null, {
                "arguments": {"count": 3, "enabled": true, "optional": null},
                "requests": [{"value": "${{ unavailable.path }}"}],
            }]})
        );
        assert!(matches!(
            prepare_decision_input(
                &check,
                &candidate(json!({"session": {"requests": [{"value": "task"}]}}))
            ),
            PreparationOutcome::Unavailable(_)
        ));
    }
}
