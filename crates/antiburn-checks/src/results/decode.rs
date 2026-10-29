use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::*;
use crate::bounded_json::decode_strict_json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedAssessment {
    version: u8,
    decisions: Vec<SavedDecision>,
    attempts: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedDecision {
    decision: String,
    answers: Vec<SavedAnswer>,
    attempts: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedAnswer {
    name: String,
    result: SavedQuestionResult,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum SavedQuestionResult {
    Choice {
        output: String,
        #[serde(default, deserialize_with = "present")]
        probabilities: Option<BTreeMap<String, f64>>,
        #[serde(default, deserialize_with = "present")]
        confidence: Option<f64>,
    },
    Score {
        output: f64,
        #[serde(default, deserialize_with = "present")]
        probabilities: Option<Vec<f64>>,
        #[serde(default, deserialize_with = "present")]
        confidence: Option<f64>,
    },
    Noul {
        output: f64,
    },
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl SavedQuestionResult {
    fn validate_shape(&self) -> Result<(), ResultError> {
        match self {
            Self::Choice {
                output,
                probabilities,
                confidence,
            } => {
                if !is_identifier(output) {
                    return Err(result_error(
                        "Choice output must be a declared label identifier.",
                    ));
                }
                if let Some(probabilities) = probabilities {
                    if probabilities.len() < 2
                        || !probabilities.contains_key(output)
                        || probabilities.keys().any(|label| !is_identifier(label))
                    {
                        return Err(result_error(
                            "Saved Choice probabilities have invalid labels.",
                        ));
                    }
                    validate_distribution(probabilities.values().copied())?;
                }
                validate_confidence(*confidence)
            }
            Self::Score {
                output,
                probabilities,
                confidence,
            } => {
                if !output.is_finite() || *output < 0.0 {
                    return Err(result_error(
                        "Saved Score output must be finite and nonnegative.",
                    ));
                }
                if let Some(probabilities) = probabilities {
                    if probabilities.len() < 2 || *output > (probabilities.len() - 1) as f64 {
                        return Err(result_error(
                            "Saved Score output or distribution length is invalid.",
                        ));
                    }
                    validate_distribution(probabilities.iter().copied())?;
                }
                validate_confidence(*confidence)
            }
            Self::Noul { output } => validate_probability(*output),
        }
    }

    fn bind(self, question: &Question) -> Result<QuestionResult, ResultError> {
        match self {
            Self::Choice {
                output,
                probabilities,
                confidence,
            } => ChoiceResult::new(question, output, probabilities, confidence)
                .map(QuestionResult::Choice),
            Self::Score {
                output,
                probabilities,
                confidence,
            } => ScoreResult::new(question, output, probabilities, confidence)
                .map(QuestionResult::Score),
            Self::Noul { output } if matches!(question, Question::Noul { .. }) => {
                NoulResult::new(output).map(QuestionResult::Noul)
            }
            Self::Noul { .. } => Err(result_error("Noul result requires a Noul declaration.")),
        }
    }
}

impl SavedDecision {
    fn validate_shape(&self) -> Result<(), ResultError> {
        if !is_identifier(&self.decision)
            || !(1..=2).contains(&self.attempts)
            || self.answers.is_empty()
            || self.answers.len() > crate::decision::MAX_QUESTIONS_PER_DECISION
        {
            return Err(result_error(
                "Saved decision name, attempts, or answer count is invalid.",
            ));
        }
        let mut names = BTreeSet::new();
        for answer in &self.answers {
            if !is_identifier(&answer.name) || !names.insert(answer.name.as_str()) {
                return Err(result_error("Saved answer names must be valid and unique."));
            }
            answer.result.validate_shape()?;
        }
        Ok(())
    }

    fn bind(
        self,
        declaration: &DecisionBlock,
        requirements: &[QuestionRequirements],
    ) -> Result<DecisionResult, ResultError> {
        if self.decision != declaration.name {
            return Err(result_error(
                "Saved result does not match the declared decision.",
            ));
        }
        if self.answers.len() != declaration.outputs.len() {
            return Err(result_error(
                "Saved answers must match the exact declared question set.",
            ));
        }
        let answers = self
            .answers
            .into_iter()
            .map(|answer| {
                let question = declaration
                    .outputs
                    .iter()
                    .find(|question| question.name == answer.name)
                    .ok_or_else(|| result_error("Saved answer contains an undeclared question."))?;
                NamedAnswer::new(answer.name, answer.result.bind(&question.question)?)
            })
            .collect::<Result<Vec<_>, ResultError>>()?;
        DecisionResult::new(declaration, requirements, answers, self.attempts)
    }
}

/// Strictly decoded data, not permission to plan actions or send a request.
/// Only declaration binding can produce an `EvaluatedAssessment`.
pub struct DecodedAssessment {
    saved: SavedAssessment,
}

impl DecodedAssessment {
    pub fn validate_for_declarations(
        self,
        declarations: &[DecisionBlock],
        requirements: &[QuestionRequirements],
    ) -> Result<EvaluatedAssessment, ResultError> {
        validate_requirements(declarations, requirements)?;
        if self.saved.decisions.len() != declarations.len() {
            return Err(result_error(
                "Saved results must match the exact declared decision set.",
            ));
        }
        let declared = declarations
            .iter()
            .map(|declaration| (declaration.name.as_str(), declaration))
            .collect::<BTreeMap<_, _>>();
        let decisions = self
            .saved
            .decisions
            .into_iter()
            .map(|saved| {
                let declaration = declared
                    .get(saved.decision.as_str())
                    .ok_or_else(|| result_error("Saved results contain an undeclared decision."))?;
                saved.bind(declaration, requirements)
            })
            .collect::<Result<Vec<_>, ResultError>>()?;
        let result = EvaluatedAssessment::new(declarations, requirements, decisions)?;
        if result.attempts != self.saved.attempts {
            return Err(result_error(
                "Saved assessment attempt total does not match its decision results.",
            ));
        }
        Ok(result)
    }
}

/// One decoded durable response. Bind it before deriving progress or actions.
pub struct DecodedDecisionResult {
    saved: SavedDecision,
}

impl DecodedDecisionResult {
    pub fn validate_for_declaration(
        self,
        declaration: &DecisionBlock,
        requirements: &[QuestionRequirements],
    ) -> Result<DecisionResult, ResultError> {
        self.saved.bind(declaration, requirements)
    }
}

pub fn decode_results(bytes: &[u8]) -> Result<DecodedAssessment, ResultError> {
    let value = decode_strict_json(bytes, RESULT_LIMITS)?;
    let saved: SavedAssessment =
        serde_json::from_value(value).map_err(|error| result_error(error.to_string()))?;
    if saved.version != RESULT_VERSION {
        return Err(result_error("Saved result version is unsupported."));
    }
    let mut names = BTreeSet::new();
    let mut attempts = 0u64;
    for decision in &saved.decisions {
        if !names.insert(decision.decision.as_str()) {
            return Err(result_error("Saved decision results must be unique."));
        }
        decision.validate_shape()?;
        attempts = attempts
            .checked_add(u64::from(decision.attempts))
            .ok_or_else(|| result_error("Saved attempt total overflowed."))?;
    }
    if attempts != saved.attempts {
        return Err(result_error(
            "Saved assessment attempt total does not match its decision results.",
        ));
    }
    Ok(DecodedAssessment { saved })
}

pub fn decode_decision_result(bytes: &[u8]) -> Result<DecodedDecisionResult, ResultError> {
    let value = decode_strict_json(bytes, RESULT_LIMITS)?;
    let saved: SavedDecision =
        serde_json::from_value(value).map_err(|error| result_error(error.to_string()))?;
    saved.validate_shape()?;
    Ok(DecodedDecisionResult { saved })
}
