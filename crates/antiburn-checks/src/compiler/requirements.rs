use crate::decision::{DecisionBlock, NamedQuestion, Question, QuestionKind, QuestionRequirements};
use crate::expression::{AccessSegment, BinaryOperator, Expression, PathSegment};

#[derive(Clone, Copy, Default)]
struct DetailsRead {
    probabilities: bool,
    confidence: bool,
}

impl DetailsRead {
    fn merge(&mut self, other: Self) {
        self.probabilities |= other.probabilities;
        self.confidence |= other.confidence;
    }
}

enum SelectedValue {
    Other,
    Scalar(DetailsRead),
    Record(DecisionRecord),
    Array(Box<Self>),
    Alternatives(Vec<Self>),
}

#[derive(Clone, Copy)]
enum DecisionRecord {
    Decisions,
    Decision,
    Outputs,
    Details,
    QuestionDetails,
    Probabilities,
}

struct Analysis {
    value: SelectedValue,
    reads: DetailsRead,
}

pub(super) fn inspect(
    expression: &Expression,
    decision: &DecisionBlock,
    question: &NamedQuestion,
    requirements: &mut QuestionRequirements,
) {
    let target = Target { decision, question };
    let analysis = analyze(expression, &target);
    let mut reads = analysis.reads;
    reads.merge(analysis.value.details_read(&target));
    requirements.probabilities |= reads.probabilities;
    requirements.confidence |= reads.confidence;
}

struct Target<'a> {
    decision: &'a DecisionBlock,
    question: &'a NamedQuestion,
}

impl Target<'_> {
    fn details(&self) -> DetailsRead {
        let categorical = self.question.question.kind() != QuestionKind::Noul;
        DetailsRead {
            probabilities: categorical,
            confidence: categorical,
        }
    }
}

fn analyze(expression: &Expression, target: &Target<'_>) -> Analysis {
    let mut reads = DetailsRead::default();
    let value = match expression {
        Expression::Literal(_) => SelectedValue::Other,
        Expression::Path(path) => {
            let mut value = if matches!(path.first(), Some(PathSegment::Property(root)) if root == "decisions")
            {
                SelectedValue::Record(DecisionRecord::Decisions)
            } else {
                SelectedValue::Other
            };
            for segment in path.iter().skip(1) {
                value = match segment {
                    PathSegment::Property(key) => value.property(key, target),
                    PathSegment::Index(_) => value.index(target),
                };
            }
            value
        }
        Expression::Access { base, segments } => {
            let base = analyze(base, target);
            reads.merge(base.reads);
            select(base.value, segments, target, &mut reads)
        }
        Expression::Call { arguments, .. } => {
            for argument in arguments {
                let argument = analyze(argument, target);
                reads.merge(argument.reads);
                reads.merge(argument.value.details_read(target));
            }
            SelectedValue::Other
        }
        Expression::Not(value) => {
            let value = analyze(value, target);
            reads.merge(value.reads);
            reads.merge(value.value.details_read(target));
            SelectedValue::Other
        }
        Expression::Binary {
            operator,
            left,
            right,
        } => {
            let left = analyze(left, target);
            let right = analyze(right, target);
            reads.merge(left.reads);
            reads.merge(right.reads);
            reads.merge(left.value.details_read(target));
            match operator {
                BinaryOperator::And | BinaryOperator::Or => {
                    SelectedValue::Alternatives(vec![left.value, right.value])
                }
                _ => {
                    reads.merge(right.value.details_read(target));
                    SelectedValue::Other
                }
            }
        }
    };
    Analysis { value, reads }
}

fn select(
    mut value: SelectedValue,
    segments: &[AccessSegment],
    target: &Target<'_>,
    reads: &mut DetailsRead,
) -> SelectedValue {
    for (position, segment) in segments.iter().enumerate() {
        value = match segment {
            AccessSegment::Property(key) => value.property(key, target),
            AccessSegment::Index(_) => value.index(target),
            AccessSegment::Dynamic(selector) => {
                let analyzed = analyze(selector, target);
                reads.merge(analyzed.reads);
                reads.merge(analyzed.value.details_read(target));
                match selector.as_ref() {
                    Expression::Literal(crate::expression::ExpressionLiteral::String(key)) => {
                        value.property(key, target)
                    }
                    _ => value.dynamic(target),
                }
            }
            AccessSegment::Projection => {
                let item = value.projection(target);
                let selected = select(item, &segments[position + 1..], target, reads);
                return SelectedValue::Array(Box::new(selected));
            }
        };
    }
    value
}

impl SelectedValue {
    fn details_read(&self, target: &Target<'_>) -> DetailsRead {
        match self {
            Self::Other | Self::Record(DecisionRecord::Outputs) => DetailsRead::default(),
            Self::Scalar(reads) => *reads,
            Self::Record(DecisionRecord::Probabilities) => DetailsRead {
                probabilities: true,
                confidence: false,
            },
            Self::Record(_) => target.details(),
            Self::Array(item) => item.details_read(target),
            Self::Alternatives(items) => {
                let mut reads = DetailsRead::default();
                for item in items {
                    reads.merge(item.details_read(target));
                }
                reads
            }
        }
    }

    fn property(self, key: &str, target: &Target<'_>) -> Self {
        use DecisionRecord::*;
        match self {
            Self::Record(record) => match (record, key) {
                (Decisions, name) if name == target.decision.name => Self::Record(Decision),
                (Decision, "outputs") => Self::Record(Outputs),
                (Decision, "details") => Self::Record(Details),
                (Outputs, name) if name == target.question.name => Self::Other,
                (Details, name) if name == target.question.name => Self::Record(QuestionDetails),
                (QuestionDetails, "probabilities")
                    if target.question.question.kind() != QuestionKind::Noul =>
                {
                    Self::Record(Probabilities)
                }
                (QuestionDetails, "confidence")
                    if target.question.question.kind() != QuestionKind::Noul =>
                {
                    Self::Scalar(DetailsRead {
                        probabilities: false,
                        confidence: true,
                    })
                }
                (Probabilities, label)
                    if matches!(&target.question.question, Question::Choice { criteria, .. } if criteria.iter().any(|(name, _)| name == label))
                        || (target.question.question.kind() == QuestionKind::Score
                            && label == "length") =>
                {
                    Self::Scalar(DetailsRead {
                        probabilities: true,
                        confidence: false,
                    })
                }
                _ => Self::Other,
            },
            Self::Alternatives(items) => Self::Alternatives(
                items
                    .into_iter()
                    .map(|item| item.property(key, target))
                    .collect(),
            ),
            Self::Array(item) if key == "length" => Self::Scalar(item.details_read(target)),
            _ => Self::Other,
        }
    }

    fn index(self, target: &Target<'_>) -> Self {
        match self {
            Self::Array(item) => *item,
            Self::Alternatives(items) => {
                Self::Alternatives(items.into_iter().map(|item| item.index(target)).collect())
            }
            Self::Record(DecisionRecord::Probabilities)
                if target.question.question.kind() == QuestionKind::Score =>
            {
                Self::Scalar(self.details_read(target))
            }
            _ => Self::Other,
        }
    }

    fn dynamic(self, target: &Target<'_>) -> Self {
        match self {
            Self::Record(DecisionRecord::Probabilities) => Self::Scalar(self.details_read(target)),
            Self::Alternatives(items) => {
                Self::Alternatives(items.into_iter().map(|item| item.dynamic(target)).collect())
            }
            _ => self.index(target),
        }
    }

    fn projection(self, target: &Target<'_>) -> Self {
        use DecisionRecord::*;
        match self {
            Self::Array(item) => *item,
            Self::Record(record) => match record {
                Decisions => Self::Record(Decision),
                Decision => Self::Alternatives(vec![Self::Record(Outputs), Self::Record(Details)]),
                Outputs => Self::Other,
                Details => Self::Record(QuestionDetails),
                QuestionDetails if target.question.question.kind() != QuestionKind::Noul => {
                    Self::Alternatives(vec![
                        Self::Record(Probabilities),
                        Self::Scalar(DetailsRead {
                            probabilities: false,
                            confidence: true,
                        }),
                    ])
                }
                QuestionDetails => Self::Other,
                Probabilities => Self::Scalar(self.details_read(target)),
            },
            Self::Alternatives(items) => Self::Alternatives(
                items
                    .into_iter()
                    .map(|item| item.projection(target))
                    .collect(),
            ),
            _ => Self::Other,
        }
    }
}
