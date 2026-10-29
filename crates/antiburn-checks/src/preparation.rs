//! Pure preparation of a frozen candidate for one compiled decision.

use serde_json::{Map, Value};

use crate::compiler::{CompiledCheck, Condition, InputLiteral, InputTemplate, TemplatePart};
use crate::decision::DecisionBlock;
use crate::diagnostics::DiagnosticCode;
use crate::domain::FrozenCandidate;
use crate::expression::PathSegment;
use crate::expression_eval::{
    EvaluatedValue, EvaluationBindings, EvaluationBudget, EvaluationDependencyKind,
    EvaluationError, EvaluationTrace, EvaluationValue, evaluate_expression_with_budget, is_truthy,
    scalar_text,
};

/// Result of checking one candidate's preconditions and preparing its input.
#[derive(Clone, Debug, PartialEq)]
pub enum PreparationOutcome {
    /// The candidate passed preconditions and has a complete typed input.
    Ready(Value),
    /// A required condition or input reads unavailable evidence.
    Unavailable(String),
    /// An ordinary expression or input error stopped preparation.
    Failed(String),
    /// The assessment's shared evaluation budget is exhausted.
    ResourceLimit(String),
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
    match evaluate_condition(
        condition,
        context,
        &EvaluationBindings::new(),
        &mut EvaluationBudget::default(),
        1,
    ) {
        Ok(value) => match value.value {
            EvaluationValue::Available(value) if is_truthy(&value) => ConditionOutcome::Matched,
            EvaluationValue::Available(_) => ConditionOutcome::False,
            EvaluationValue::Unavailable => ConditionOutcome::Unavailable,
        },
        Err(error) => ConditionOutcome::Failed(error.message),
    }
}

/// Prepares one block input from the frozen context.
///
/// The function reads only the frozen candidate context. It does not call a
/// provider or mutate the candidate. A missing path never becomes JSON null.
#[must_use]
pub fn prepare_decision_input(
    decision: &DecisionBlock,
    candidate: &FrozenCandidate,
) -> PreparationOutcome {
    prepare_decision_input_with_budget(decision, candidate, &mut EvaluationBudget::default())
}

/// Prepares one decision using the assessment's shared budget.
///
/// The top-level condition uses a separate pre-selection budget. Input templates
/// use the supplied budget, which the caller must retain for action planning.
#[must_use]
pub fn prepare_decision_input_with_budget(
    decision: &DecisionBlock,
    candidate: &FrozenCandidate,
    budget: &mut EvaluationBudget,
) -> PreparationOutcome {
    match evaluate_decision_input_with_budget(decision, &candidate.context.value, budget) {
        Ok(EvaluatedValue {
            value: EvaluationValue::Available(value),
            ..
        }) => PreparationOutcome::Ready(value),
        Ok(_) => PreparationOutcome::Unavailable(
            "Decision input depends on unavailable evidence.".to_owned(),
        ),
        Err(error)
            if error.code == DiagnosticCode::EvaluationLimitExceeded || budget.exhausted() =>
        {
            PreparationOutcome::ResourceLimit(error.message)
        }
        Err(error) => PreparationOutcome::Failed(error.message),
    }
}

/// Evaluates input values and retains the selected source dependencies.
///
/// This does not evaluate the pre-selection condition. The selector must check
/// that condition before it builds a window.
///
/// # Errors
/// Returns an evaluation error for invalid input work or an exhausted budget.
pub fn evaluate_decision_input_with_budget(
    decision: &DecisionBlock,
    context: &Value,
    budget: &mut EvaluationBudget,
) -> Result<EvaluatedValue, EvaluationError> {
    prepare_template(
        &decision.input,
        context,
        &mut EvaluationBindings::new(),
        budget,
        1,
    )
    .map(|value| enforce_selected_membership(value, context))
}

/// Decodes the full saved record before allocating typed executable state.
pub fn decode_preparation_record(
    bytes: &[u8],
) -> Result<crate::fit::PreparationRecord, crate::fit::PreparationFailure> {
    use crate::fit::{PreparationFailure, PreparationFailureCode, PreparationRecord};
    let invalid = || PreparationFailure {
        code: PreparationFailureCode::IncompatibleProfile,
        detail: "The saved preparation is invalid or unsupported.".into(),
        report: None,
        context: Default::default(),
    };
    let value = crate::bounded_json::decode_strict_json(
        bytes,
        crate::prepared_assessment::PREPARATION_LIMITS,
    )
    .map_err(|_| invalid())?;
    let record: PreparationRecord = serde_json::from_value(value).map_err(|_| invalid())?;
    match &record {
        PreparationRecord::Ready(prepared) => prepared.validate().map_err(|_| invalid())?,
        PreparationRecord::Local(prepared) => prepared.validate_for_resume()?,
        PreparationRecord::Rejected(_) => {}
    }
    Ok(record)
}

/// Replaces an oversized record with a small explicit resource rejection.
#[must_use]
pub fn bounded_preparation_record(
    record: crate::fit::PreparationRecord,
) -> crate::fit::PreparationRecord {
    use crate::fit::{PreparationFailure, PreparationFailureCode, PreparationRecord};
    if crate::bounded_json::measure_json(&record, crate::prepared_assessment::PREPARATION_LIMITS)
        .is_ok()
    {
        return record;
    }
    PreparationRecord::Rejected(PreparationFailure {
        code: PreparationFailureCode::PreparationResourceLimit,
        detail:
            "The saved preparation exceeds the local record limit. The model request was not sent."
                .into(),
        report: None,
        context: Default::default(),
    })
}

/// Checks selected collection operations against the frozen request-history facts.
///
/// Whole value copies remain available with partial history. Only operations
/// that depend on `session.requests` membership require proven complete history.
/// Bounded window selections and provided JSON arrays need no history flag.
#[must_use]
pub fn selected_membership_is_available(trace: &EvaluationTrace, context: &Value) -> bool {
    let reads_requests_membership = trace.dependencies.iter().any(|dependency| {
        dependency.kind == EvaluationDependencyKind::Membership
            && matches!(dependency.path.as_slice(), [PathSegment::Property(session), PathSegment::Property(requests)] if session == "session" && requests == "requests")
    });
    !reads_requests_membership
        || context
            .get("session")
            .and_then(|session| session.get("requests_complete"))
            == Some(&Value::Bool(true))
}

pub(crate) fn enforce_selected_membership(
    mut value: EvaluatedValue,
    context: &Value,
) -> EvaluatedValue {
    if !selected_membership_is_available(&value.trace, context) {
        value.value = EvaluationValue::Unavailable;
    }
    value
}

pub(crate) fn evaluate_condition(
    condition: &Condition,
    context: &Value,
    bindings: &EvaluationBindings,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<EvaluatedValue, EvaluationError> {
    match condition {
        Condition::Boolean(value) => {
            budget.step(depth)?;
            Ok(EvaluatedValue::available(Value::Bool(*value)))
        }
        Condition::Expression(expression) => {
            evaluate_expression_with_budget(expression, context, bindings, budget, depth)
                .map(|value| enforce_selected_membership(value, context))
        }
    }
}

pub(crate) fn prepare_template(
    template: &InputTemplate,
    context: &Value,
    bindings: &mut EvaluationBindings,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<EvaluatedValue, EvaluationError> {
    budget.step(depth)?;
    match template {
        InputTemplate::Literal(literal) => {
            budget.reserve_generated_bytes(match literal {
                InputLiteral::String(text) => text.len(),
                InputLiteral::Null => 4,
                InputLiteral::Boolean(_) => 5,
                InputLiteral::Number(_) => 32,
            })?;
            Ok(EvaluatedValue::available(literal_value(literal)))
        }
        InputTemplate::Expression(expression) => {
            evaluate_expression_with_budget(expression, context, bindings, budget, depth)
                .map(|value| enforce_selected_membership(value, context))
        }
        InputTemplate::Text(parts) => prepare_text(parts, context, bindings, budget, depth),
        InputTemplate::Sequence(items) => {
            budget.visit_items(items.len())?;
            budget.reserve_generated_bytes(items.len().saturating_add(2))?;
            let mut values = Vec::new();
            let mut trace = EvaluationTrace::default();
            for item in items {
                let evaluated = prepare_template(item, context, bindings, budget, depth + 1)?;
                trace.merge(&evaluated.trace);
                match evaluated.value {
                    EvaluationValue::Available(value) => values.push(value),
                    EvaluationValue::Unavailable => {
                        return Ok(with_trace(EvaluationValue::Unavailable, trace));
                    }
                }
            }
            Ok(with_trace(
                EvaluationValue::Available(Value::Array(values)),
                trace,
            ))
        }
        InputTemplate::Object(items) => prepare_object(items, context, bindings, budget, depth),
        InputTemplate::Map {
            collection,
            alias,
            condition,
            value,
        } => {
            let collection =
                evaluate_expression_with_budget(collection, context, bindings, budget, depth + 1)?;
            let count = match &collection.value {
                EvaluationValue::Unavailable => return Ok(collection),
                EvaluationValue::Available(Value::Array(items)) => items.len(),
                _ => return Err(invalid_template("Map collection must be an array.")),
            };
            budget.reserve_generated_bytes(2)?;
            let mut trace = collection.membership_trace();
            if !selected_membership_is_available(&trace, context) {
                return Ok(with_trace(EvaluationValue::Unavailable, trace));
            }
            let mut values = Vec::new();
            for index in 0..count {
                let item = collection.array_item(index, budget)?;
                budget.reserve_generated_bytes(alias.len())?;
                bindings.insert(alias.clone(), item);
                let item_result = prepare_map_item(
                    condition.as_ref(),
                    value,
                    context,
                    bindings,
                    budget,
                    depth + 1,
                    &mut trace,
                );
                bindings.remove(alias);
                match item_result? {
                    MapItem::Excluded => {}
                    MapItem::Value(value) => {
                        budget.reserve_generated_bytes(1)?;
                        values.push(value);
                    }
                    MapItem::Unavailable => {
                        return Ok(with_trace(EvaluationValue::Unavailable, trace));
                    }
                }
            }
            Ok(with_trace(
                EvaluationValue::Available(Value::Array(values)),
                trace,
            ))
        }
    }
}

fn prepare_object(
    items: &[(String, InputTemplate)],
    context: &Value,
    bindings: &mut EvaluationBindings,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<EvaluatedValue, EvaluationError> {
    budget.step(depth)?;
    budget.visit_items(items.len())?;
    budget.reserve_generated_bytes(2)?;
    let mut values = Map::new();
    let mut trace = EvaluationTrace::default();
    for (key, item) in items {
        budget.reserve_generated_bytes(key.len().saturating_add(4))?;
        let evaluated = prepare_template(item, context, bindings, budget, depth + 1)?;
        trace.merge(&evaluated.trace);
        match evaluated.value {
            EvaluationValue::Available(value) => {
                values.insert(key.clone(), value);
            }
            EvaluationValue::Unavailable => {
                return Ok(with_trace(EvaluationValue::Unavailable, trace));
            }
        }
    }
    Ok(with_trace(
        EvaluationValue::Available(Value::Object(values)),
        trace,
    ))
}

fn prepare_text(
    parts: &[TemplatePart],
    context: &Value,
    bindings: &EvaluationBindings,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<EvaluatedValue, EvaluationError> {
    let mut text = String::new();
    let mut prepared = EvaluatedValue::available(Value::Null);
    for part in parts {
        budget.step(depth + 1)?;
        match part {
            TemplatePart::Literal(literal) => {
                budget.reserve_generated_bytes(literal.len())?;
                text.push_str(literal);
            }
            TemplatePart::Expression(expression) => {
                let evaluated = evaluate_expression_with_budget(
                    expression,
                    context,
                    bindings,
                    budget,
                    depth + 1,
                )?;
                let evaluated = enforce_selected_membership(evaluated, context);
                prepared.merge_text_provenance(&evaluated);
                match evaluated.value {
                    EvaluationValue::Available(value) => {
                        text.push_str(&scalar_text(&value, budget)?)
                    }
                    EvaluationValue::Unavailable => {
                        prepared.value = EvaluationValue::Unavailable;
                        return Ok(prepared);
                    }
                }
            }
        }
    }
    prepared.value = EvaluationValue::Available(Value::String(text));
    Ok(prepared)
}

enum MapItem {
    Excluded,
    Value(Value),
    Unavailable,
}

fn prepare_map_item(
    condition: Option<&Condition>,
    template: &InputTemplate,
    context: &Value,
    bindings: &mut EvaluationBindings,
    budget: &mut EvaluationBudget,
    depth: usize,
    trace: &mut EvaluationTrace,
) -> Result<MapItem, EvaluationError> {
    if let Some(condition) = condition {
        let evaluated = evaluate_condition(condition, context, bindings, budget, depth)?;
        trace.merge(&evaluated.trace);
        match evaluated.value {
            EvaluationValue::Unavailable => return Ok(MapItem::Unavailable),
            EvaluationValue::Available(value) if !is_truthy(&value) => {
                return Ok(MapItem::Excluded);
            }
            EvaluationValue::Available(_) => {}
        }
    }
    let evaluated = prepare_template(template, context, bindings, budget, depth)?;
    trace.merge(&evaluated.trace);
    Ok(match evaluated.value {
        EvaluationValue::Available(value) => MapItem::Value(value),
        EvaluationValue::Unavailable => MapItem::Unavailable,
    })
}

fn with_trace(value: EvaluationValue, trace: EvaluationTrace) -> EvaluatedValue {
    let mut evaluated = match value {
        EvaluationValue::Available(value) => EvaluatedValue::available(value),
        EvaluationValue::Unavailable => {
            let mut evaluated = EvaluatedValue::available(Value::Null);
            evaluated.value = EvaluationValue::Unavailable;
            evaluated
        }
    };
    evaluated.trace = trace;
    evaluated
}

#[cfg(test)]
mod text_provenance_tests {
    use super::*;
    use crate::expression::parse_expression;
    use serde_json::json;

    #[test]
    fn embedded_text_keeps_deferred_membership_after_parsing() {
        let context = json!({"session": {"requests": [], "requests_complete": false}});
        let mut budget = EvaluationBudget::default();
        let text = prepare_text(
            &[
                TemplatePart::Literal("{\"requests\":".into()),
                TemplatePart::Expression(parse_expression("toJSON(session.requests)").unwrap()),
                TemplatePart::Literal("}".into()),
            ],
            &context,
            &EvaluationBindings::new(),
            &mut budget,
            1,
        )
        .unwrap();
        assert_eq!(
            text.value,
            EvaluationValue::Available(json!("{\"requests\":[]}"))
        );
        let mut bindings = EvaluationBindings::new();
        bindings.insert("prepared".into(), text);
        let length = evaluate_expression_with_budget(
            &parse_expression("fromJSON(prepared).requests.length").unwrap(),
            &context,
            &bindings,
            &mut budget,
            1,
        )
        .unwrap();
        assert_eq!(
            enforce_selected_membership(length, &context).value,
            EvaluationValue::Unavailable
        );
    }
}

fn invalid_template(message: &str) -> EvaluationError {
    EvaluationError {
        code: DiagnosticCode::RuntimeInvalidContext,
        message: message.to_owned(),
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
                definition_id: "over-exploring".to_owned(),
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
        let source = include_str!("../../../.antiburn/checks/over-exploring.yml");
        let check = compile(source).expect("the maintained example compiles");
        let prepared = prepare_decision_input(
            &check.decisions[0],
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
            "name: Test\non:\n  tool_result:\nif: false\ndecisions:\n  review:\n    input:\n      value: \"${{ message.tool.result.status }}\"\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n",
        )
        .unwrap();
        assert_eq!(
            evaluate_trigger_condition(&check, &json!({})),
            ConditionOutcome::False
        );

        let check = compile(
            "name: Test\non:\n  tool_result:\ndecisions:\n  review:\n    input:\n      value: \"${{ message.tool.result.status }}\"\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n",
        )
        .unwrap();
        assert!(matches!(
            prepare_decision_input(&check.decisions[0], &candidate(json!({}))),
            PreparationOutcome::Unavailable(_)
        ));
    }

    #[test]
    fn recursive_inputs_preserve_null_types_and_expression_text_as_data() {
        let check = compile(
            "name: Typed input\non:\n  tool_result:\ndecisions:\n  review:\n    input:\n      nested:\n        - null\n        - arguments: \"${{ message.tool.arguments }}\"\n          requests: \"${{ session.requests }}\"\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n",
        ).unwrap();
        let context = json!({
            "session": {"requests": [{"value": "${{ unavailable.path }}"}]},
            "message": {"tool": {"arguments": {"count": 3, "enabled": true, "optional": null}}},
        });
        let PreparationOutcome::Ready(input) =
            prepare_decision_input(&check.decisions[0], &candidate(context))
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
                &check.decisions[0],
                &candidate(json!({"session": {"requests": [{"value": "task"}]}}))
            ),
            PreparationOutcome::Unavailable(_)
        ));
    }
}
