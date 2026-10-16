//! Pure runtime evaluation for the compiled expression subset.

use serde::Serialize;
use serde_json::{Number, Value};

use crate::diagnostics::DiagnosticCode;
use crate::expression::{BinaryOperator, Expression, ExpressionLiteral, PathSegment};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    let mut evaluator = Evaluator {
        context,
        limits,
        steps: 0,
    };
    evaluator.evaluate(expression, 1).map(|value| value.value)
}

struct EvaluatedValue {
    value: EvaluationValue,
    origin: Option<Vec<PathSegment>>,
}

struct Evaluator<'a> {
    context: &'a Value,
    limits: EvaluationLimits,
    steps: usize,
}

impl Evaluator<'_> {
    fn evaluate(
        &mut self,
        expression: &Expression,
        depth: usize,
    ) -> Result<EvaluatedValue, EvaluationError> {
        self.step()?;
        if depth > self.limits.max_depth {
            return Err(limit_error(
                "Expression evaluation exceeded the depth limit.",
            ));
        }
        match expression {
            Expression::Literal(literal) => Ok(EvaluatedValue {
                value: EvaluationValue::Available(literal_value(literal)),
                origin: None,
            }),
            Expression::Path(path) => self.path(path),
            Expression::Not(value) => match self.evaluate(value, depth + 1)?.value {
                EvaluationValue::Available(value) => Ok(EvaluatedValue {
                    value: EvaluationValue::Available(Value::Bool(!is_truthy(&value))),
                    origin: None,
                }),
                EvaluationValue::Unavailable => Ok(EvaluatedValue {
                    value: EvaluationValue::Unavailable,
                    origin: None,
                }),
            },
            Expression::Binary {
                operator,
                left,
                right,
            } => {
                let left_value = self.evaluate(left, depth + 1)?;
                match (operator, left_value) {
                    (BinaryOperator::And, value) if matches!(&value.value, EvaluationValue::Available(item) if !is_truthy(item)) => {
                        Ok(value)
                    }
                    (BinaryOperator::Or, value) if matches!(&value.value, EvaluationValue::Available(item) if is_truthy(item)) => {
                        Ok(value)
                    }
                    (BinaryOperator::And | BinaryOperator::Or, value)
                        if matches!(&value.value, EvaluationValue::Unavailable) =>
                    {
                        Ok(value)
                    }
                    (BinaryOperator::And | BinaryOperator::Or, _) => {
                        self.evaluate(right, depth + 1)
                    }
                    (operator, left_value) => {
                        let right_value = self.evaluate(right, depth + 1)?;
                        Ok(EvaluatedValue {
                            value: compare(operator, left_value, right_value),
                            origin: None,
                        })
                    }
                }
            }
        }
    }

    fn path(&mut self, path: &[PathSegment]) -> Result<EvaluatedValue, EvaluationError> {
        let Some((PathSegment::Property(root), remaining)) = path.split_first() else {
            return Ok(unavailable());
        };
        let mut value = match self.context.get(root) {
            Some(value) => value,
            None => return Ok(unavailable()),
        };
        for (index, segment) in remaining.iter().enumerate() {
            self.step()?;
            value = match (segment, value) {
                (PathSegment::Property(property), Value::Object(object)) => {
                    match object.get(property) {
                        Some(value) => value,
                        None if property == "length" => {
                            return Ok(unavailable());
                        }
                        None => return Ok(unavailable()),
                    }
                }
                (PathSegment::Property(property), Value::Array(items)) if property == "length" => {
                    if index + 1 != remaining.len() {
                        return Ok(unavailable());
                    }
                    return Ok(EvaluatedValue {
                        value: EvaluationValue::Available(Value::Number(Number::from(items.len()))),
                        origin: None,
                    });
                }
                (PathSegment::Index(index), Value::Array(items)) => match items.get(*index) {
                    Some(value) => value,
                    None => return Ok(unavailable()),
                },
                _ => return Ok(unavailable()),
            };
        }
        Ok(EvaluatedValue {
            value: EvaluationValue::Available(value.clone()),
            origin: Some(path.to_vec()),
        })
    }

    fn step(&mut self) -> Result<(), EvaluationError> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > self.limits.max_steps {
            return Err(limit_error(
                "Expression evaluation exceeded the step limit.",
            ));
        }
        Ok(())
    }
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

fn compare(
    operator: &BinaryOperator,
    left: EvaluatedValue,
    right: EvaluatedValue,
) -> EvaluationValue {
    let (EvaluationValue::Available(left_value), EvaluationValue::Available(right_value)) =
        (left.value, right.value)
    else {
        return EvaluationValue::Unavailable;
    };
    let result = match operator {
        BinaryOperator::Equal => equal(
            &left_value,
            &right_value,
            left.origin.is_some() && left.origin == right.origin,
        ),
        BinaryOperator::NotEqual => !equal(
            &left_value,
            &right_value,
            left.origin.is_some() && left.origin == right.origin,
        ),
        BinaryOperator::Less => {
            ordered(&left_value, &right_value).is_some_and(|order| order.is_lt())
        }
        BinaryOperator::LessOrEqual => {
            ordered(&left_value, &right_value).is_some_and(|order| !order.is_gt())
        }
        BinaryOperator::Greater => {
            ordered(&left_value, &right_value).is_some_and(|order| order.is_gt())
        }
        BinaryOperator::GreaterOrEqual => {
            ordered(&left_value, &right_value).is_some_and(|order| !order.is_lt())
        }
        BinaryOperator::And | BinaryOperator::Or => unreachable!("logical operators short-circuit"),
    };
    EvaluationValue::Available(Value::Bool(result))
}

fn equal(left: &Value, right: &Value, same_origin: bool) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => decimal_value(left) == decimal_value(right),
        (Value::String(left), Value::String(right)) => left.to_lowercase() == right.to_lowercase(),
        (Value::Array(_) | Value::Object(_), Value::Array(_) | Value::Object(_)) => same_origin,
        (left, right) => match (coerce_number(left), coerce_number(right)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        },
    }
}

fn ordered(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::String(left), Value::String(right)) => {
            Some(left.to_lowercase().cmp(&right.to_lowercase()))
        }
        (Value::Number(left), Value::Number(right)) => {
            decimal_value(left)?.partial_cmp(&decimal_value(right)?)
        }
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        _ => None,
    }
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

fn unavailable() -> EvaluatedValue {
    EvaluatedValue {
        value: EvaluationValue::Unavailable,
        origin: None,
    }
}

fn limit_error(message: &str) -> EvaluationError {
    EvaluationError {
        code: DiagnosticCode::EvaluationLimitExceeded,
        message: message.to_owned(),
    }
}
