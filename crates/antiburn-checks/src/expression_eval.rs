//! Pure, bounded evaluation with source traces and frozen-value identity.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::diagnostics::DiagnosticCode;
use crate::expression::{
    AccessSegment, BinaryOperator, Expression, ExpressionLiteral, PathSegment,
};

#[path = "expression_eval/budget.rs"]
mod budget;
#[path = "expression_eval/functions.rs"]
mod functions;
#[path = "expression_eval/json.rs"]
mod json;
#[path = "expression_eval/value.rs"]
mod value;

pub use budget::{
    DEFAULT_MAX_EVALUATION_ITEMS, DEFAULT_MAX_GENERATED_BYTES, DEFAULT_MAX_MATERIALIZED_BYTES,
    EvaluationBudget,
};
pub use value::{
    EvaluatedValue, EvaluationBindings, EvaluationDependency, EvaluationDependencyKind,
    EvaluationTrace,
};
use value::{RuntimeValue, ValueMetadata};

/// The default maximum recursive expression depth for one evaluation.
pub const DEFAULT_MAX_EVALUATION_DEPTH: usize = 32;
/// The default maximum number of expression and path steps for one evaluation.
pub const DEFAULT_MAX_EVALUATION_STEPS: usize = 10_000;

/// A runtime value that keeps missing evidence distinct from JSON null.
#[derive(Clone, Debug, PartialEq)]
pub enum EvaluationValue {
    /// An available JSON-compatible value, including explicit null.
    Available(Value),
    /// A value that the frozen context does not provide.
    Unavailable,
}

/// Resource limits for one pure expression evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationLimits {
    /// Maximum expression nesting depth, including unary and binary nodes.
    pub max_depth: usize,
    /// Maximum evaluated expression nodes and visited path segments.
    pub max_steps: usize,
}

impl Default for EvaluationLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_EVALUATION_DEPTH,
            max_steps: DEFAULT_MAX_EVALUATION_STEPS,
        }
    }
}

/// An evaluation diagnostic for invalid runtime work or exhausted limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvaluationError {
    /// The stable diagnostic code associated with the failure.
    pub code: DiagnosticCode,
    /// A short explanation suitable for an assessment diagnostic.
    pub message: String,
}

/// Evaluates an expression against one immutable JSON context.
///
/// Missing paths produce [`EvaluationValue::Unavailable`]. Explicit JSON null
/// remains available. Evaluation has a depth limit of 32 and a step limit of
/// 10,000 by default.
///
/// # Errors
/// Returns a diagnostic if evaluation exceeds a resource limit.
pub fn evaluate_expression(
    expression: &Expression,
    context: &Value,
) -> Result<EvaluationValue, EvaluationError> {
    evaluate_expression_with_limits(expression, context, EvaluationLimits::default())
}

/// Evaluates an expression with caller-selected depth and step limits.
///
/// A zero limit rejects the first corresponding evaluation step. The function
/// does not mutate the expression or context.
///
/// # Errors
/// Returns a diagnostic if evaluation exceeds either configured limit.
pub fn evaluate_expression_with_limits(
    expression: &Expression,
    context: &Value,
    limits: EvaluationLimits,
) -> Result<EvaluationValue, EvaluationError> {
    let mut budget = EvaluationBudget::new(
        limits,
        DEFAULT_MAX_EVALUATION_ITEMS,
        DEFAULT_MAX_GENERATED_BYTES,
    );
    evaluate_expression_with_budget(
        expression,
        context,
        &EvaluationBindings::new(),
        &mut budget,
        1,
    )
    .map(|value| value.value)
}

/// Evaluates one leaf under the assessment's shared budget and alias scope.
///
/// The caller passes the current template depth, starting at one.
pub fn evaluate_expression_with_budget(
    expression: &Expression,
    context: &Value,
    bindings: &EvaluationBindings,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<EvaluatedValue, EvaluationError> {
    Evaluator {
        context,
        bindings,
        budget,
    }
    .evaluate(expression, depth)?
    .finish(budget, depth)
}

struct Evaluator<'a, 'b> {
    context: &'a Value,
    bindings: &'a EvaluationBindings,
    budget: &'b mut EvaluationBudget,
}

impl<'a> Evaluator<'a, '_> {
    fn evaluate(
        &mut self,
        expression: &Expression,
        depth: usize,
    ) -> Result<RuntimeValue<'a>, EvaluationError> {
        self.budget.step(depth)?;
        match expression {
            Expression::Literal(literal) => {
                if let ExpressionLiteral::String(text) = literal {
                    self.budget.reserve_generated_bytes(text.len())?;
                }
                Ok(RuntimeValue::owned(
                    literal_value(literal),
                    EvaluationTrace::default(),
                ))
            }
            Expression::Path(path) => self.path(path, depth),
            Expression::Access { base, segments } => {
                let value = self.evaluate(base, depth + 1)?;
                self.access(value, segments, depth)
            }
            Expression::Call {
                function,
                arguments,
            } => self.call(*function, arguments, depth),
            Expression::Not(value) => {
                let value = self.evaluate(value, depth + 1)?;
                let trace = value.trace();
                match value.value {
                    Some(value) => Ok(RuntimeValue::owned(Value::Bool(!is_truthy(&value)), trace)),
                    None => Ok(RuntimeValue::unavailable(ValueMetadata::derived(trace))),
                }
            }
            Expression::Binary {
                operator,
                left,
                right,
            } => {
                let left_value = self.evaluate(left, depth + 1)?;
                let Some(left_json) = left_value.value.as_ref() else {
                    return Ok(left_value);
                };
                if (*operator == BinaryOperator::And && !is_truthy(left_json))
                    || (*operator == BinaryOperator::Or && is_truthy(left_json))
                {
                    return Ok(left_value);
                }
                let mut right_value = self.evaluate(right, depth + 1)?;
                if matches!(operator, BinaryOperator::And | BinaryOperator::Or) {
                    right_value.metadata.dependencies.merge(&left_value.trace());
                    return Ok(right_value);
                }
                compare(*operator, left_value, right_value, self.budget)
            }
        }
    }

    fn path(
        &mut self,
        path: &[PathSegment],
        depth: usize,
    ) -> Result<RuntimeValue<'a>, EvaluationError> {
        let Some((PathSegment::Property(root), remaining)) = path.split_first() else {
            return Ok(RuntimeValue::unavailable(ValueMetadata::default()));
        };
        let mut value = if let Some(binding) = self.bindings.get(root) {
            RuntimeValue {
                value: match &binding.value {
                    EvaluationValue::Available(value) => Some(Cow::Borrowed(value)),
                    EvaluationValue::Unavailable => None,
                },
                metadata: binding.metadata.clone(),
            }
        } else {
            RuntimeValue {
                value: self
                    .context
                    .as_object()
                    .and_then(|object| object.get(root))
                    .map(Cow::Borrowed),
                metadata: ValueMetadata::frozen(vec![PathSegment::Property(root.clone())]),
            }
        };
        for (position, segment) in remaining.iter().enumerate() {
            self.budget.step(depth.saturating_add(position))?;
            value = self.select(value, segment);
        }
        Ok(value)
    }

    fn select(&mut self, value: RuntimeValue<'a>, segment: &PathSegment) -> RuntimeValue<'a> {
        let Some(json) = value.value else {
            return RuntimeValue::unavailable(value.metadata);
        };
        if let (PathSegment::Property(property), Value::Array(items)) = (segment, json.as_ref())
            && property == "length"
        {
            return RuntimeValue::owned(
                Value::Number(Number::from(items.len())),
                value.metadata.membership_trace(),
            );
        }
        let membership =
            matches!(json.as_ref(), Value::Array(_)).then(|| value.metadata.membership_trace());
        let mut metadata = value.metadata.child(segment);
        if let Some(membership) = membership {
            metadata.dependencies.merge(&membership);
        }
        let selected = match json {
            Cow::Borrowed(value) => match (segment, value) {
                (PathSegment::Property(property), Value::Object(object)) => {
                    object.get(property).map(Cow::Borrowed)
                }
                (PathSegment::Index(index), Value::Array(items)) => {
                    items.get(*index).map(Cow::Borrowed)
                }
                _ => None,
            },
            Cow::Owned(value) => match (segment, value) {
                (PathSegment::Property(property), Value::Object(mut object)) => {
                    object.remove(property).map(Cow::Owned)
                }
                (PathSegment::Index(index), Value::Array(items)) => {
                    items.into_iter().nth(*index).map(Cow::Owned)
                }
                _ => None,
            },
        };
        RuntimeValue {
            value: selected,
            metadata,
        }
    }

    fn access(
        &mut self,
        mut value: RuntimeValue<'a>,
        segments: &[AccessSegment],
        depth: usize,
    ) -> Result<RuntimeValue<'a>, EvaluationError> {
        for (position, segment) in segments.iter().enumerate() {
            self.budget.step(depth.saturating_add(position))?;
            if value.value.is_none() {
                return Ok(value);
            }
            match segment {
                AccessSegment::Property(property) => {
                    value = self.select(value, &PathSegment::Property(property.clone()))
                }
                AccessSegment::Index(index) => {
                    value = self.select(value, &PathSegment::Index(*index))
                }
                AccessSegment::Dynamic(expression) => {
                    let selector = self.evaluate(expression, depth + 1)?;
                    let trace = selector.trace();
                    value.metadata.dependencies.merge(&trace);
                    let Some(selector) = selector.value else {
                        return Ok(RuntimeValue::unavailable(ValueMetadata::derived(
                            value.trace(),
                        )));
                    };
                    let segment = match selector.as_ref() {
                        Value::String(key) => {
                            self.budget.reserve_generated_bytes(key.len())?;
                            Some(PathSegment::Property(key.clone()))
                        }
                        Value::Number(number) => integral_index(number).map(PathSegment::Index),
                        _ => None,
                    };
                    let Some(segment) = segment else {
                        return Ok(RuntimeValue::unavailable(ValueMetadata::derived(
                            value.trace(),
                        )));
                    };
                    value = self.select(value, &segment);
                }
                AccessSegment::Projection => {
                    return self.project(value, &segments[position + 1..], depth + 1);
                }
            }
        }
        Ok(value)
    }

    fn project(
        &mut self,
        value: RuntimeValue<'a>,
        remaining: &[AccessSegment],
        depth: usize,
    ) -> Result<RuntimeValue<'a>, EvaluationError> {
        self.budget.step(depth)?;
        let mut dependencies = value.metadata.membership_trace();
        let Some(json) = value.value else {
            return Ok(RuntimeValue::unavailable(ValueMetadata::derived(
                dependencies,
            )));
        };
        let count = match json.as_ref() {
            Value::Array(items) => items.len(),
            Value::Object(object) => object.len(),
            _ => {
                return Ok(RuntimeValue::unavailable(ValueMetadata::derived(
                    value.metadata.trace(),
                )));
            }
        };
        self.budget.visit_items(count)?;
        self.budget
            .reserve_generated_bytes(count.saturating_add(2))?;
        let mut output = Vec::with_capacity(count);
        let mut item_metadata = Vec::with_capacity(count);
        let entries: Vec<(PathSegment, Cow<'a, Value>)> = match json {
            Cow::Borrowed(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(index, value)| (PathSegment::Index(index), Cow::Borrowed(value)))
                .collect(),
            Cow::Borrowed(Value::Object(object)) => {
                for key in object.keys() {
                    self.budget.reserve_generated_bytes(key.len())?;
                }
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_unstable_by_key(|(key, _)| *key);
                entries
                    .into_iter()
                    .map(|(key, value)| (PathSegment::Property(key.clone()), Cow::Borrowed(value)))
                    .collect()
            }
            Cow::Owned(Value::Array(items)) => items
                .into_iter()
                .enumerate()
                .map(|(index, value)| (PathSegment::Index(index), Cow::Owned(value)))
                .collect(),
            Cow::Owned(Value::Object(object)) => {
                let mut entries: Vec<_> = object.into_iter().collect();
                entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
                entries
                    .into_iter()
                    .map(|(key, value)| (PathSegment::Property(key), Cow::Owned(value)))
                    .collect()
            }
            _ => unreachable!("projection type is checked"),
        };
        for (segment, item) in entries {
            let item = RuntimeValue {
                value: Some(item),
                metadata: value.metadata.child(&segment),
            };
            let selected = self.access(item, remaining, depth)?;
            if selected.value.is_none() {
                dependencies.merge(&selected.trace());
                for metadata in &item_metadata {
                    dependencies.merge(&ValueMetadata::trace(metadata));
                }
                return Ok(RuntimeValue::unavailable(ValueMetadata::derived(
                    dependencies,
                )));
            }
            let selected = selected.finish(self.budget, depth)?;
            item_metadata.push(selected.metadata);
            let EvaluationValue::Available(json) = selected.value else {
                unreachable!("selected value is available")
            };
            output.push(json);
        }
        let mut result = RuntimeValue::owned(Value::Array(output), dependencies);
        result.metadata.items = item_metadata;
        Ok(result)
    }
}

fn integral_index(number: &Number) -> Option<usize> {
    let number = decimal_value(number)?;
    if number.negative || number.scale > 0 {
        return None;
    }
    let zeros = usize::try_from(number.scale.checked_neg()?).ok()?;
    if number.digits.len().checked_add(zeros)? > usize::MAX.to_string().len() {
        return None;
    }
    let mut index = 0usize;
    for digit in number
        .digits
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0, zeros))
    {
        index = index.checked_mul(10)?.checked_add(usize::from(digit))?;
    }
    Some(index)
}

fn literal_value(literal: &ExpressionLiteral) -> Value {
    match literal {
        ExpressionLiteral::String(value) => Value::String(value.clone()),
        ExpressionLiteral::Number(value) => Value::Number(value.clone()),
        ExpressionLiteral::Boolean(value) => Value::Bool(*value),
        ExpressionLiteral::Null => Value::Null,
    }
}

/// Returns whether an available JSON value is true-like in an expression.
#[must_use]
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => {
            DecimalNumber::parse(&value.to_string()).is_some_and(|number| !number.is_zero())
        }
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn compare<'a>(
    operator: BinaryOperator,
    left: RuntimeValue<'a>,
    right: RuntimeValue<'a>,
    budget: &mut EvaluationBudget,
) -> Result<RuntimeValue<'a>, EvaluationError> {
    let mut trace = left.trace();
    trace.merge(&right.trace());
    let (Some(left_value), Some(right_value)) = (left.value, right.value) else {
        return Ok(RuntimeValue::unavailable(ValueMetadata::derived(trace)));
    };
    let same_identity = match (&left.metadata.identity, &right.metadata.identity) {
        (Some(left), Some(right)) => left.same(right),
        _ => false,
    };
    let result = match operator {
        BinaryOperator::Equal => equal(&left_value, &right_value, same_identity, budget)?,
        BinaryOperator::NotEqual => !equal(&left_value, &right_value, same_identity, budget)?,
        BinaryOperator::Less => {
            ordered(&left_value, &right_value, budget)?.is_some_and(|order| order.is_lt())
        }
        BinaryOperator::LessOrEqual => {
            ordered(&left_value, &right_value, budget)?.is_some_and(|order| !order.is_gt())
        }
        BinaryOperator::Greater => {
            ordered(&left_value, &right_value, budget)?.is_some_and(|order| order.is_gt())
        }
        BinaryOperator::GreaterOrEqual => {
            ordered(&left_value, &right_value, budget)?.is_some_and(|order| !order.is_lt())
        }
        BinaryOperator::And | BinaryOperator::Or => unreachable!("logical operators short-circuit"),
    };
    Ok(RuntimeValue::owned(Value::Bool(result), trace))
}

fn equal(
    left: &Value,
    right: &Value,
    same_origin: bool,
    budget: &mut EvaluationBudget,
) -> Result<bool, EvaluationError> {
    Ok(match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => decimal_value(left) == decimal_value(right),
        (Value::String(left), Value::String(right)) => {
            lowercase(left, budget)? == lowercase(right, budget)?
        }
        (Value::Array(_) | Value::Object(_), Value::Array(_) | Value::Object(_)) => same_origin,
        (left, right) => match (coerce_number(left), coerce_number(right)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        },
    })
}

fn ordered(
    left: &Value,
    right: &Value,
    budget: &mut EvaluationBudget,
) -> Result<Option<std::cmp::Ordering>, EvaluationError> {
    Ok(match (left, right) {
        (Value::String(left), Value::String(right)) => {
            Some(lowercase(left, budget)?.cmp(&lowercase(right, budget)?))
        }
        (Value::Number(left), Value::Number(right)) => decimal_value(left)
            .zip(decimal_value(right))
            .map(|(left, right)| left.cmp(&right)),
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        _ if std::mem::discriminant(left) != std::mem::discriminant(right) => coerce_number(left)
            .zip(coerce_number(right))
            .map(|(left, right)| left.cmp(&right)),
        _ => None,
    })
}

fn coerce_number(value: &Value) -> Option<DecimalNumber> {
    match value {
        Value::Null | Value::Bool(false) => DecimalNumber::parse("0"),
        Value::Bool(true) => DecimalNumber::parse("1"),
        Value::Number(value) => decimal_value(value),
        Value::String(value) if value.is_empty() => DecimalNumber::parse("0"),
        Value::String(value) => serde_json::from_str::<Number>(value)
            .ok()
            .and_then(|number| decimal_value(&number)),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn decimal_value(value: &Number) -> Option<DecimalNumber> {
    DecimalNumber::parse(&value.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DecimalNumber {
    negative: bool,
    digits: Vec<u8>,
    scale: i64,
}

impl DecimalNumber {
    fn parse(source: &str) -> Option<Self> {
        let (mantissa, exponent) = match source.find(['e', 'E']) {
            Some(index) => (&source[..index], parse_exponent(&source[index + 1..])?),
            None => (source, 0),
        };
        let negative = mantissa.starts_with('-');
        let unsigned = mantissa.strip_prefix(['-', '+']).unwrap_or(mantissa);
        let (integer, fraction) = match unsigned.split_once('.') {
            Some((integer, fraction)) => (integer, fraction),
            None => (unsigned, ""),
        };
        if integer.is_empty()
            || !integer.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let mut digits = integer
            .bytes()
            .chain(fraction.bytes())
            .map(|byte| byte - b'0')
            .collect::<Vec<_>>();
        let first_nonzero = digits.iter().position(|digit| *digit != 0);
        let Some(first_nonzero) = first_nonzero else {
            return Some(Self {
                negative: false,
                digits: vec![0],
                scale: 0,
            });
        };
        digits.drain(..first_nonzero);
        let mut scale = i64::try_from(fraction.len())
            .unwrap_or(i64::MAX)
            .saturating_sub(exponent);
        while digits.len() > 1 && digits.last() == Some(&0) {
            digits.pop();
            scale = scale.saturating_sub(1);
        }
        Some(Self {
            negative,
            digits,
            scale,
        })
    }

    fn is_zero(&self) -> bool {
        self.digits == [0]
    }

    fn order(&self) -> i64 {
        i64::try_from(self.digits.len())
            .unwrap_or(i64::MAX)
            .saturating_sub(self.scale)
    }
}

impl PartialOrd for DecimalNumber {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DecimalNumber {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        match (self.is_zero(), other.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => {
                return if other.negative {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (false, true) => {
                return if self.negative {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, false) => {}
        }
        if self.negative != other.negative {
            return if self.negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let magnitude = self.order().cmp(&other.order()).then_with(|| {
            let length = self.digits.len().max(other.digits.len());
            (0..length)
                .map(|index| self.digits.get(index).copied().unwrap_or(0))
                .cmp((0..length).map(|index| other.digits.get(index).copied().unwrap_or(0)))
        });
        if self.negative {
            magnitude.reverse()
        } else {
            magnitude
        }
    }
}

fn parse_exponent(source: &str) -> Option<i64> {
    if source.is_empty() {
        return None;
    }
    source.parse::<i64>().ok().or_else(|| {
        let digits = source.strip_prefix(['-', '+']).unwrap_or(source);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            None
        } else if source.starts_with('-') {
            Some(i64::MIN)
        } else {
            Some(i64::MAX)
        }
    })
}

fn lowercase(value: &str, budget: &mut EvaluationBudget) -> Result<String, EvaluationError> {
    // Contextual final sigma changes the character, not its UTF-8 byte count.
    let bytes = value
        .chars()
        .flat_map(char::to_lowercase)
        .map(char::len_utf8)
        .sum();
    budget.reserve_generated_bytes(bytes)?;
    Ok(value.to_lowercase())
}

fn limit_error(message: &str) -> EvaluationError {
    EvaluationError {
        code: DiagnosticCode::EvaluationLimitExceeded,
        message: message.to_owned(),
    }
}

fn invalid_error(message: &str) -> EvaluationError {
    EvaluationError {
        code: DiagnosticCode::RuntimeInvalidContext,
        message: message.to_owned(),
    }
}

/// Converts a scalar to template text and reserves its bytes before allocation.
pub fn scalar_text(
    value: &Value,
    budget: &mut EvaluationBudget,
) -> Result<String, EvaluationError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(value) => {
            let text = if *value { "true" } else { "false" };
            budget.reserve_generated_bytes(text.len())?;
            Ok(text.to_owned())
        }
        Value::Number(value) => {
            use std::fmt::Write;
            struct ByteCount(usize);
            impl std::fmt::Write for ByteCount {
                fn write_str(&mut self, text: &str) -> std::fmt::Result {
                    self.0 += text.len();
                    Ok(())
                }
            }
            let mut count = ByteCount(0);
            write!(&mut count, "{value}").expect("counting bytes cannot fail");
            budget.reserve_generated_bytes(count.0)?;
            Ok(value.to_string())
        }
        Value::String(value) => {
            budget.reserve_generated_bytes(value.len())?;
            Ok(value.clone())
        }
        Value::Array(_) | Value::Object(_) => Err(invalid_error(
            "Arrays and objects require explicit toJSON conversion.",
        )),
    }
}

fn copy_value(
    value: &Value,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    check_copy(value, budget, depth)?;
    Ok(value.clone())
}

fn check_copy(
    value: &Value,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<(), EvaluationError> {
    budget.step(depth)?;
    match value {
        Value::Array(items) => {
            budget.visit_items(items.len())?;
            budget.reserve_materialized_bytes(items.len().saturating_add(2))?;
            for item in items {
                check_copy(item, budget, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            budget.visit_items(object.len())?;
            budget.reserve_materialized_bytes(object.len().saturating_add(2))?;
            for (key, value) in object {
                budget.reserve_materialized_bytes(key.len().saturating_add(3))?;
                check_copy(value, budget, depth + 1)?;
            }
            Ok(())
        }
        Value::String(value) => budget.reserve_materialized_bytes(value.len()),
        _ => budget.reserve_materialized_bytes(32),
    }
}

#[cfg(test)]
#[path = "expression_eval/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "expression_eval/provenance_tests.rs"]
mod provenance_tests;
