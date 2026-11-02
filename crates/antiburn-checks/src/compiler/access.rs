use crate::decision::QuestionKind;
use crate::diagnostics::DiagnosticCode;
use crate::expression::{
    AccessSegment, BinaryOperator, Expression, ExpressionFunction, ExpressionLiteral, PathSegment,
};

use super::{ExpressionContext, TriggerFamily, Validator, reference_error, unsupported_reference};

type AccessResult = Result<ValueShape, (DiagnosticCode, String)>;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ValueShape {
    Scalar,
    Open,
    Array(Box<Self>),
    FixedArray(Box<Self>, usize),
    Record(Record),
    Alternatives(Vec<Self>),
}

impl ValueShape {
    pub(super) fn map_item(self) -> Option<Self> {
        match self {
            Self::Array(item) | Self::FixedArray(item, _) => Some(*item),
            Self::Open => Some(Self::Open),
            Self::Alternatives(items) => {
                let mut items: Vec<_> = items.into_iter().filter_map(Self::map_item).collect();
                match items.len() {
                    0 => None,
                    1 => items.pop(),
                    _ => Some(Self::Alternatives(items)),
                }
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Record {
    Session,
    Turn,
    Request,
    Window,
    Message,
    Quality,
    Tool,
    Result,
    Outcome,
    Agent,
    Global,
    Repo,
    Instruction,
    InstructionScope,
    Skill,
    Decisions,
    Decision(usize),
    Outputs(usize),
    Details(usize),
    QuestionDetails(usize, usize),
    Probabilities(usize, usize),
}

impl Validator {
    pub(super) fn resolve_expression(
        &self,
        expression: &Expression,
        context: ExpressionContext,
    ) -> AccessResult {
        match expression {
            Expression::Literal(_) => Ok(ValueShape::Scalar),
            Expression::Path(path) => {
                let Some((PathSegment::Property(root), rest)) = path.split_first() else {
                    return Err(reference_error(
                        "Expression paths must start with a context root.",
                    ));
                };
                let mut shape = self.root_shape(root, context)?;
                for segment in rest {
                    shape = match segment {
                        PathSegment::Property(key) => self.property_shape(shape, key)?,
                        PathSegment::Index(index) => self.index_shape(shape, Some(*index))?,
                    };
                }
                Ok(shape)
            }
            Expression::Access { base, segments } => {
                let shape = self.resolve_expression(base, context)?;
                self.access_shape(shape, segments, context)
            }
            Expression::Call {
                function,
                arguments,
            } => {
                for argument in arguments {
                    self.resolve_expression(argument, context)?;
                }
                Ok(match function {
                    ExpressionFunction::FromJSON => ValueShape::Open,
                    _ => ValueShape::Scalar,
                })
            }
            Expression::Not(value) => {
                self.resolve_expression(value, context)?;
                Ok(ValueShape::Scalar)
            }
            Expression::Binary {
                operator,
                left,
                right,
            } => {
                let known_truth = match left.as_ref() {
                    Expression::Literal(ExpressionLiteral::Null) => Some(false),
                    Expression::Literal(ExpressionLiteral::Boolean(value)) => Some(*value),
                    Expression::Literal(ExpressionLiteral::String(value)) => {
                        Some(!value.is_empty())
                    }
                    Expression::Literal(ExpressionLiteral::Number(value)) => {
                        value.as_f64().map(|value| value != 0.0)
                    }
                    _ => None,
                };
                let left = self.resolve_expression(left, context)?;
                let right = self.resolve_expression(right, context)?;
                if let Some(truth) = known_truth
                    && matches!(operator, BinaryOperator::And | BinaryOperator::Or)
                {
                    return Ok(if (*operator == BinaryOperator::Or) == truth {
                        left
                    } else {
                        right
                    });
                }
                if matches!(operator, BinaryOperator::And | BinaryOperator::Or)
                    && matches!(
                        left,
                        ValueShape::Array(_) | ValueShape::FixedArray(_, _) | ValueShape::Record(_)
                    )
                {
                    return Ok(if *operator == BinaryOperator::Or {
                        left
                    } else {
                        right
                    });
                }
                Ok(match operator {
                    BinaryOperator::And | BinaryOperator::Or if left != right => {
                        ValueShape::Alternatives(vec![left, right])
                    }
                    BinaryOperator::And | BinaryOperator::Or => left,
                    _ => ValueShape::Scalar,
                })
            }
        }
    }

    fn root_shape(&self, root: &str, context: ExpressionContext) -> AccessResult {
        if context == ExpressionContext::DecisionInput
            && let Some((_, shape)) = self.aliases.iter().rev().find(|(name, _)| name == root)
        {
            return Ok(shape.clone());
        }
        let record = match root {
            "session" => Record::Session,
            "message" if !matches!(self.trigger_family, Some(TriggerFamily::Messages { .. })) => {
                return Err(unsupported_reference(
                    "The `message` root requires a message-family trigger.",
                ));
            }
            "message" => Record::Message,
            "agent" => Record::Agent,
            "global" => Record::Global,
            "repo" => Record::Repo,
            "window" if context == ExpressionContext::TopLevel => {
                return Err(unsupported_reference(
                    "The `window` root is unavailable to top-level conditions.",
                ));
            }
            "window" if !self.has_window => {
                return Err(reference_error("The `window` root requires `on.window`."));
            }
            "window" => Record::Window,
            "decisions" if context != ExpressionContext::ActionCondition => {
                return Err(unsupported_reference(
                    "The `decisions` root is available only to actions.",
                ));
            }
            "decisions" if self.decisions.is_empty() => {
                return Err(reference_error(
                    "Decision results require a compiled decision.",
                ));
            }
            "decisions" => Record::Decisions,
            "turn" if matches!(self.trigger_family, Some(TriggerFamily::ResponseCycle(_))) => {
                Record::Turn
            }
            "turn" => {
                return Err(unsupported_reference(
                    "The `turn` root requires `on.turn_end`.",
                ));
            }
            _ => return Err(reference_error(&format!("Unknown context root `{root}`."))),
        };
        Ok(ValueShape::Record(record))
    }

    fn access_shape(
        &self,
        mut shape: ValueShape,
        segments: &[AccessSegment],
        context: ExpressionContext,
    ) -> AccessResult {
        for (index, segment) in segments.iter().enumerate() {
            shape = match segment {
                AccessSegment::Property(key) => self.property_shape(shape, key)?,
                AccessSegment::Index(index) => self.index_shape(shape, Some(*index))?,
                AccessSegment::Dynamic(selector) => {
                    self.resolve_expression(selector, context)?;
                    match selector.as_ref() {
                        Expression::Literal(ExpressionLiteral::String(key)) => {
                            self.property_shape(shape, key)?
                        }
                        Expression::Literal(ExpressionLiteral::Number(number)) => {
                            self.numeric_index_shape(shape, number)?
                        }
                        _ => self.dynamic_shape(shape)?,
                    }
                }
                AccessSegment::Projection => {
                    let item = self.projection_shape(shape)?;
                    let selected = self.access_shape(item, &segments[index + 1..], context)?;
                    return Ok(ValueShape::Array(Box::new(selected)));
                }
            };
        }
        Ok(shape)
    }

    fn property_shape(&self, shape: ValueShape, key: &str) -> AccessResult {
        match shape {
            ValueShape::Open => Ok(ValueShape::Open),
            ValueShape::Array(_) | ValueShape::FixedArray(_, _) if key == "length" => {
                Ok(ValueShape::Scalar)
            }
            ValueShape::Alternatives(items) => {
                if items.is_empty() {
                    return Err(reference_error(
                        "The selected record has no declared child fields.",
                    ));
                }
                let shapes = items
                    .into_iter()
                    .map(|item| self.property_shape(item, key))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ValueShape::Alternatives(shapes))
            }
            ValueShape::Record(record) => self.record_field(record, key),
            _ => Err(reference_error(
                "The selected value has no such child field.",
            )),
        }
    }

    fn index_shape(&self, shape: ValueShape, index: Option<usize>) -> AccessResult {
        match shape {
            ValueShape::Open => Ok(ValueShape::Open),
            ValueShape::Array(item) => Ok(*item),
            ValueShape::FixedArray(item, count) if index.is_none_or(|index| index < count) => {
                Ok(*item)
            }
            ValueShape::FixedArray(_, _) => Err(reference_error(
                "Score probability index is outside the declared criteria.",
            )),
            ValueShape::Alternatives(items) => Ok(ValueShape::Alternatives(
                items
                    .into_iter()
                    .map(|item| self.index_shape(item, index))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            _ => Err(reference_error(
                "A normalized object or scalar cannot use an array index.",
            )),
        }
    }

    fn dynamic_shape(&self, shape: ValueShape) -> AccessResult {
        match shape {
            ValueShape::Open => Ok(ValueShape::Open),
            ValueShape::Array(item) | ValueShape::FixedArray(item, _) => Ok(*item),
            ValueShape::Record(Record::Probabilities(_, _)) => Ok(ValueShape::Scalar),
            ValueShape::Alternatives(items) => Ok(ValueShape::Alternatives(
                items
                    .into_iter()
                    .map(|item| self.dynamic_shape(item))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            _ => Err(reference_error(
                "Dynamic keys cannot select unknown fields on closed normalized records.",
            )),
        }
    }

    fn numeric_index_shape(&self, shape: ValueShape, number: &serde_json::Number) -> AccessResult {
        match shape {
            ValueShape::FixedArray(item, count) => {
                let valid = number.as_f64().is_some_and(|index| {
                    index >= 0.0 && index.fract() == 0.0 && index < count as f64
                });
                if valid {
                    Ok(*item)
                } else {
                    Err(reference_error(
                        "Score probability index is outside the declared criteria.",
                    ))
                }
            }
            ValueShape::Alternatives(items) => Ok(ValueShape::Alternatives(
                items
                    .into_iter()
                    .map(|item| self.numeric_index_shape(item, number))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            _ => self.index_shape(shape, None),
        }
    }

    fn projection_shape(&self, shape: ValueShape) -> AccessResult {
        match shape {
            ValueShape::Open => Ok(ValueShape::Open),
            ValueShape::Array(item) | ValueShape::FixedArray(item, _) => Ok(*item),
            ValueShape::Record(record) => {
                let fields = self.record_fields(record);
                let shapes = fields
                    .iter()
                    .map(|key| self.record_field(record, key))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ValueShape::Alternatives(shapes))
            }
            ValueShape::Alternatives(items) => Ok(ValueShape::Alternatives(
                items
                    .into_iter()
                    .map(|item| self.projection_shape(item))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            ValueShape::Scalar => Err(reference_error("Projection requires an array or object.")),
        }
    }

    fn record_field(&self, record: Record, key: &str) -> AccessResult {
        use Record::*;
        use ValueShape::{Array, Open, Record as Object, Scalar};
        let result = match (record, key) {
            (Session, "id" | "requests_complete" | "source_coverage") => Scalar,
            (Session, "model" | "fast_mode") => Scalar,
            (Session, "start_reason")
                if matches!(
                    self.trigger_family,
                    Some(TriggerFamily::Session {
                        session_start: Some(_),
                        ..
                    })
                ) =>
            {
                Scalar
            }
            (Session, "idle_seconds" | "last_activity_at")
                if matches!(
                    self.trigger_family,
                    Some(TriggerFamily::Session {
                        session_idle: Some(_),
                        ..
                    })
                ) =>
            {
                Scalar
            }
            (Session, "requests") => Array(Box::new(Object(Request))),
            (
                Turn,
                "id" | "source" | "status" | "model" | "fast_mode" | "started_at" | "ended_at",
            ) => Scalar,
            (Turn, "messages") => Array(Box::new(Object(Message))),
            (Window, "messages") => Array(Box::new(Object(Message))),
            (Request, "id" | "source" | "kind" | "text" | "in_reply_to") => Scalar,
            (
                Message,
                "id" | "source" | "occurred_at" | "kind" | "role" | "model" | "fast_mode"
                | "content",
            ) => Scalar,
            (Message, "tool") => Object(Tool),
            (Message, "quality") => Object(Quality),
            (
                Quality,
                "source_complete"
                | "tool_name_available"
                | "category_available"
                | "arguments_available"
                | "result_content_available"
                | "request_history_complete"
                | "main_session"
                | "paths_available"
                | "paths_complete"
                | "model_limit"
                | "fast_mode_limit"
                | "fast_mode_evidence"
                | "category_limit",
            ) => Scalar,
            (Quality, "paths_limits") => Array(Box::new(Scalar)),
            (Tool, "name" | "category" | "call_id" | "provider_executed") => Scalar,
            (Tool, "paths") => Array(Box::new(Scalar)),
            (Tool, "arguments") => Open,
            (Tool, "result") => Object(Result),
            (Result, "status" | "output_bytes") => Scalar,
            (Result, "content") => Open,
            (Result, "outcome") => Object(Outcome),
            (
                Outcome,
                "kind" | "state" | "exit" | "signal" | "timed_out" | "shell_id" | "error"
                | "session_id",
            ) => Scalar,
            (Agent, "name") | (Repo, "root") => Scalar,
            (Global | Repo, "instructions") => Array(Box::new(Object(Instruction))),
            (Global | Repo, "skills") => Array(Box::new(Object(Skill))),
            (Global | Repo, "instructions_complete" | "skills_complete" | "snapshot_revision") => {
                Scalar
            }
            (Global | Repo, "instructions_limits" | "skills_limits") => Array(Box::new(Scalar)),
            (Instruction, "path" | "content" | "observed_at") => Scalar,
            (Instruction, "scope") => Object(InstructionScope),
            (InstructionScope, "root") => Scalar,
            (InstructionScope, "globs") => Array(Box::new(Scalar)),
            (Skill, "name" | "description" | "path") => Scalar,
            (Decisions, name) => self
                .decisions
                .iter()
                .position(|symbols| symbols.name == name)
                .map(|index| Object(Decision(index)))
                .ok_or_else(|| reference_error("Unknown decision name."))?,
            (Decision(index), "outputs") => Object(Outputs(index)),
            (Decision(index), "details") => Object(Details(index)),
            (Outputs(index), name)
                if self.decisions[index]
                    .outputs
                    .iter()
                    .any(|output| output.name == name) =>
            {
                Scalar
            }
            (Details(index), name) => self.decisions[index]
                .outputs
                .iter()
                .position(|output| output.name == name)
                .map(|question| Object(QuestionDetails(index, question)))
                .ok_or_else(|| reference_error("Unknown question name."))?,
            (QuestionDetails(index, question), "probabilities") => {
                match self.decisions[index].outputs[question].kind {
                    QuestionKind::Choice => Object(Probabilities(index, question)),
                    QuestionKind::Score => ValueShape::FixedArray(
                        Box::new(Scalar),
                        self.decisions[index].outputs[question].criteria_count,
                    ),
                    QuestionKind::Noul => {
                        return Err(reference_error(
                            "Noul has no probability distribution detail.",
                        ));
                    }
                }
            }
            (QuestionDetails(index, question), "confidence")
                if self.decisions[index].outputs[question].kind != QuestionKind::Noul =>
            {
                Scalar
            }
            (Probabilities(index, question), label)
                if self.decisions[index].outputs[question]
                    .labels
                    .contains(label) =>
            {
                Scalar
            }
            (Session, "start_reason" | "idle_seconds" | "last_activity_at")
            | (Agent, "skills")
            | (Repo, "branch") => {
                return Err(unsupported_reference(
                    "The selected context field or provider detail is not supported yet.",
                ));
            }
            _ => {
                return Err(reference_error(&format!(
                    "Unknown normalized field `{key}`."
                )));
            }
        };
        Ok(result)
    }

    fn record_fields(&self, record: Record) -> Vec<&str> {
        use Record::*;
        match record {
            Session => {
                let mut fields = vec![
                    "id",
                    "requests_complete",
                    "source_coverage",
                    "requests",
                    "model",
                    "fast_mode",
                ];
                if let Some(TriggerFamily::Session {
                    session_start,
                    session_idle,
                }) = &self.trigger_family
                {
                    if session_start.is_some() {
                        fields.push("start_reason");
                    }
                    if session_idle.is_some() {
                        fields.extend(["idle_seconds", "last_activity_at"]);
                    }
                }
                fields
            }
            Turn => vec![
                "id",
                "source",
                "status",
                "messages",
                "model",
                "fast_mode",
                "started_at",
                "ended_at",
            ],
            Request => vec!["id", "source", "kind", "text", "in_reply_to"],
            Window => vec!["messages"],
            Message => vec![
                "id",
                "source",
                "occurred_at",
                "kind",
                "role",
                "model",
                "fast_mode",
                "content",
                "tool",
                "quality",
            ],
            Quality => vec![
                "source_complete",
                "tool_name_available",
                "category_available",
                "arguments_available",
                "result_content_available",
                "request_history_complete",
                "main_session",
                "paths_available",
                "paths_complete",
                "paths_limits",
                "model_limit",
                "fast_mode_limit",
                "fast_mode_evidence",
                "category_limit",
            ],
            Tool => vec![
                "name",
                "category",
                "call_id",
                "provider_executed",
                "paths",
                "arguments",
                "result",
            ],
            Result => vec!["status", "output_bytes", "content", "outcome"],
            Outcome => vec![
                "kind",
                "state",
                "exit",
                "signal",
                "timed_out",
                "shell_id",
                "error",
                "session_id",
            ],
            Agent => vec!["name"],
            Global | Repo => {
                let mut fields = vec![
                    "instructions",
                    "instructions_complete",
                    "instructions_limits",
                    "skills",
                    "skills_complete",
                    "skills_limits",
                    "snapshot_revision",
                ];
                if record == Repo {
                    fields.push("root");
                }
                fields
            }
            Instruction => vec!["path", "content", "observed_at", "scope"],
            InstructionScope => vec!["root", "globs"],
            Skill => vec!["name", "description", "path"],
            Decisions => self
                .decisions
                .iter()
                .map(|symbols| symbols.name.as_str())
                .collect(),
            Decision(_) => vec!["outputs", "details"],
            Outputs(index) | Details(index) => self.decisions[index]
                .outputs
                .iter()
                .map(|question| question.name.as_str())
                .collect(),
            QuestionDetails(index, question)
                if self.decisions[index].outputs[question].kind != QuestionKind::Noul =>
            {
                vec!["probabilities", "confidence"]
            }
            QuestionDetails(_, _) => Vec::new(),
            Probabilities(index, question) => self.decisions[index].outputs[question]
                .labels
                .iter()
                .map(String::as_str)
                .collect(),
        }
    }
}
