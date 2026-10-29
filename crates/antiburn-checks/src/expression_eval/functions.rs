use std::borrow::Cow;

use serde_json::Value;

use crate::expression::{Expression, ExpressionFunction, PathSegment};

use super::value::{RuntimeValue, ValueMetadata};
use super::{
    EvaluationError, EvaluationTrace, Evaluator, decimal_value, equal, invalid_error, lowercase,
    scalar_text,
};

impl<'a> Evaluator<'a, '_> {
    pub(super) fn call(
        &mut self,
        function: ExpressionFunction,
        arguments: &[Expression],
        depth: usize,
    ) -> Result<RuntimeValue<'a>, EvaluationError> {
        if !function.accepts_arity(arguments.len()) {
            return Err(invalid_error(
                "Wrong number of expression function arguments.",
            ));
        }
        let mut values = Vec::new();
        let mut trace = EvaluationTrace::default();
        for argument in arguments {
            let value = self.evaluate(argument, depth + 1)?;
            trace.merge(&value.trace());
            if value.value.is_none() {
                return Ok(RuntimeValue::unavailable(ValueMetadata::derived(trace)));
            }
            values.push(value);
        }
        match function {
            ExpressionFunction::StrictEquals => {
                let left = available(&values[0]);
                let right = available(&values[1]);
                if matches!(left, Value::Array(_) | Value::Object(_))
                    || matches!(right, Value::Array(_) | Value::Object(_))
                {
                    return Err(invalid_error("strictEquals requires primitive arguments."));
                }
                let result = match (left, right) {
                    (Value::Number(left), Value::Number(right)) => {
                        decimal_value(left) == decimal_value(right)
                    }
                    _ => left == right,
                };
                Ok(RuntimeValue::owned(Value::Bool(result), trace))
            }
            ExpressionFunction::Contains => {
                if let Value::Array(items) = available(&values[0]) {
                    let mut trace = values[0].metadata.membership_trace();
                    trace.merge(&values[1].trace());
                    let right = available(&values[1]);
                    let mut result = false;
                    for (index, item) in items.iter().enumerate() {
                        self.budget.visit_items(1)?;
                        self.budget.step(depth + 1)?;
                        let metadata = values[0].metadata.child(&PathSegment::Index(index));
                        trace.merge(&metadata.trace());
                        let same_identity = match (&metadata.identity, &values[1].metadata.identity)
                        {
                            (Some(left), Some(right)) => left.same(right),
                            _ => false,
                        };
                        if equal(item, right, same_identity, self.budget)? {
                            result = true;
                            break;
                        }
                    }
                    Ok(RuntimeValue::owned(Value::Bool(result), trace))
                } else {
                    let text = scalar_text(available(&values[0]), self.budget)?;
                    let needle = scalar_text(available(&values[1]), self.budget)?;
                    let text = lowercase(&text, self.budget)?;
                    let needle = lowercase(&needle, self.budget)?;
                    Ok(RuntimeValue::owned(
                        Value::Bool(text.contains(&needle)),
                        trace,
                    ))
                }
            }
            ExpressionFunction::StartsWith | ExpressionFunction::EndsWith => {
                let text = scalar_text(available(&values[0]), self.budget)?;
                let part = scalar_text(available(&values[1]), self.budget)?;
                let text = lowercase(&text, self.budget)?;
                let part = lowercase(&part, self.budget)?;
                let result = if function == ExpressionFunction::StartsWith {
                    text.starts_with(&part)
                } else {
                    text.ends_with(&part)
                };
                Ok(RuntimeValue::owned(Value::Bool(result), trace))
            }
            ExpressionFunction::Join => {
                let separator = if let Some(value) = values.get(1) {
                    Cow::Owned(scalar_text(available(value), self.budget)?)
                } else {
                    Cow::Borrowed(",")
                };
                if matches!(available(&values[0]), Value::String(_)) {
                    let mut value = values.remove(0);
                    value.metadata.dependencies.merge(&trace);
                    return Ok(value);
                }
                let Value::Array(items) = available(&values[0]) else {
                    return Err(invalid_error("join requires an array or a string."));
                };
                trace.merge(&values[0].metadata.membership_trace());
                self.budget.visit_items(items.len())?;
                let mut text = String::new();
                let mut sources = values[0].metadata.text_source_trace();
                for (index, item) in items.iter().enumerate() {
                    self.budget.step(depth + 1)?;
                    sources.merge(
                        &values[0]
                            .metadata
                            .child(&PathSegment::Index(index))
                            .text_source_trace(),
                    );
                    if index > 0 {
                        append(&mut text, &separator, self.budget)?;
                        if let Some(separator) = values.get(1) {
                            sources.merge(&separator.metadata.text_source_trace());
                        }
                    }
                    let item = scalar_text(item, self.budget)?;
                    append(&mut text, &item, self.budget)?;
                }
                let mut result = RuntimeValue::owned(Value::String(text), trace);
                result.metadata.text_sources = sources;
                Ok(result)
            }
            ExpressionFunction::Format => {
                let template = scalar_text(available(&values[0]), self.budget)?;
                let mut replacements = Vec::new();
                for value in &values[1..] {
                    replacements.push(scalar_text(available(value), self.budget)?);
                }
                let mut sources = values[0].metadata.text_source_trace();
                let text = render_format(
                    &template,
                    &replacements,
                    &values[1..],
                    &mut sources,
                    self.budget,
                    depth,
                )?;
                let mut result = RuntimeValue::owned(Value::String(text), trace);
                result.metadata.text_sources = sources;
                Ok(result)
            }
            ExpressionFunction::ToJSON => {
                let provenance = values[0].metadata.source_provenance();
                let (text, sources) = super::json::serialize(
                    available(&values[0]),
                    &provenance,
                    self.budget,
                    depth + 1,
                )?;
                let mut result = RuntimeValue::owned(Value::String(text), trace);
                result.metadata.encoded_provenance = Some(Box::new(provenance));
                result.metadata.text_sources = sources;
                Ok(result)
            }
            ExpressionFunction::FromJSON => {
                let Value::String(text) = available(&values[0]) else {
                    return Err(invalid_error("fromJSON requires a string."));
                };
                let value = super::json::parse(text, self.budget, depth + 1)?;
                let mut result = RuntimeValue::owned(value, trace);
                if let Some(provenance) = &values[0].metadata.encoded_provenance {
                    result.metadata.restore_provenance((**provenance).clone());
                } else {
                    result.metadata.membership_sources = values[0].metadata.text_source_trace();
                }
                Ok(result)
            }
        }
    }
}

fn available<'a>(value: &'a RuntimeValue<'_>) -> &'a Value {
    value
        .value
        .as_ref()
        .map(Cow::as_ref)
        .expect("call arguments are available")
}

pub(super) fn append(
    output: &mut String,
    text: &str,
    budget: &mut super::EvaluationBudget,
) -> Result<(), EvaluationError> {
    budget.reserve_generated_bytes(text.len())?;
    output.push_str(text);
    Ok(())
}

fn render_format(
    template: &str,
    values: &[String],
    evaluated: &[RuntimeValue<'_>],
    sources: &mut EvaluationTrace,
    budget: &mut super::EvaluationBudget,
    depth: usize,
) -> Result<String, EvaluationError> {
    let mut output = String::new();
    let mut cursor = 0;
    while cursor < template.len() {
        budget.step(depth)?;
        let rest = &template[cursor..];
        if rest.starts_with("{{") || rest.starts_with("}}") {
            append(&mut output, &rest[..1], budget)?;
            cursor += 2;
        } else if rest.starts_with('{') {
            let Some(end) = rest.find('}') else {
                return Err(invalid_error("format has an unmatched opening brace."));
            };
            let index = &rest[1..end];
            if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid_error(
                    "format placeholders require a nonnegative integer.",
                ));
            }
            let index = index
                .parse::<usize>()
                .ok()
                .filter(|index| *index < values.len())
                .ok_or_else(|| invalid_error("format references an unavailable argument."))?;
            let value = &values[index];
            sources.merge(&evaluated[index].metadata.text_source_trace());
            append(&mut output, value, budget)?;
            cursor += end + 1;
        } else if rest.starts_with('}') {
            return Err(invalid_error("format has an unmatched closing brace."));
        } else {
            let end = rest.find(['{', '}']).unwrap_or(rest.len());
            append(&mut output, &rest[..end], budget)?;
            cursor += end;
        }
    }
    Ok(output)
}
