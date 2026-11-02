//! Typed results. Constructors bind results to declarations before use.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;

use crate::bounded_json::{BoundedJsonError, JsonLimits, JsonMeasurement, measure_json};
use crate::decision::{
    DecisionBlock, DecisionContractError, Question, QuestionRequirements, is_identifier,
    validate_requirements,
};

mod capacity;
mod decode;
pub use capacity::measure_result_capacity;
pub use decode::{
    DecodedAssessment, DecodedDecisionResult, decode_decision_result, decode_results,
};

pub const RESULT_VERSION: u8 = 1;
pub const MAX_RESULT_BYTES: u64 = 512 * 1024;
pub const RESULT_LIMITS: JsonLimits = JsonLimits {
    max_bytes: MAX_RESULT_BYTES,
    max_nodes: 65_536,
    max_depth: crate::fit::MAX_FIT_INPUT_DEPTH,
};
pub const PROBABILITY_SUM_TOLERANCE: f64 = 0.02;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultError {
    pub message: String,
}

impl fmt::Display for ResultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for ResultError {}
impl From<DecisionContractError> for ResultError {
    fn from(error: DecisionContractError) -> Self {
        result_error(error.message)
    }
}
impl From<BoundedJsonError> for ResultError {
    fn from(error: BoundedJsonError) -> Self {
        result_error(error.to_string())
    }
}
fn result_error(message: impl Into<String>) -> ResultError {
    ResultError {
        message: message.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChoiceResult {
    output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    probabilities: Option<BTreeMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<f64>,
}

impl ChoiceResult {
    pub fn new(
        question: &Question,
        output: impl Into<String>,
        probabilities: Option<BTreeMap<String, f64>>,
        confidence: Option<f64>,
    ) -> Result<Self, ResultError> {
        let result = Self {
            output: output.into(),
            probabilities,
            confidence,
        };
        result.validate(question)?;
        Ok(result)
    }

    fn validate(&self, question: &Question) -> Result<(), ResultError> {
        question.validate()?;
        let Question::Choice { criteria, .. } = question else {
            return Err(result_error("Choice result requires a Choice declaration."));
        };
        if !criteria.iter().any(|(label, _)| label == &self.output) {
            return Err(result_error("Choice output is not a declared label."));
        }
        if let Some(probabilities) = &self.probabilities {
            if probabilities.len() != criteria.len()
                || criteria
                    .iter()
                    .any(|(label, _)| !probabilities.contains_key(label))
            {
                return Err(result_error(
                    "Choice probabilities must match the exact declared labels.",
                ));
            }
            validate_distribution(probabilities.values().copied())?;
        }
        validate_confidence(self.confidence)
    }

    #[must_use]
    pub fn output(&self) -> &str {
        &self.output
    }
    #[must_use]
    pub fn probabilities(&self) -> Option<&BTreeMap<String, f64>> {
        self.probabilities.as_ref()
    }
    #[must_use]
    pub const fn confidence(&self) -> Option<f64> {
        self.confidence
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScoreResult {
    output: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    probabilities: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<f64>,
}

impl ScoreResult {
    pub fn new(
        question: &Question,
        output: f64,
        probabilities: Option<Vec<f64>>,
        confidence: Option<f64>,
    ) -> Result<Self, ResultError> {
        let result = Self {
            output,
            probabilities,
            confidence,
        };
        result.validate(question)?;
        Ok(result)
    }

    fn validate(&self, question: &Question) -> Result<(), ResultError> {
        question.validate()?;
        let Question::Score { criteria, .. } = question else {
            return Err(result_error("Score result requires a Score declaration."));
        };
        let maximum = (criteria.len() - 1) as f64;
        if !self.output.is_finite() || !(0.0..=maximum).contains(&self.output) {
            return Err(result_error(
                "Score output must be finite and within the declared criterion range.",
            ));
        }
        if let Some(probabilities) = &self.probabilities {
            if probabilities.len() != criteria.len() {
                return Err(result_error(
                    "Score probabilities must match the declared criterion count.",
                ));
            }
            validate_distribution(probabilities.iter().copied())?;
        }
        validate_confidence(self.confidence)
    }

    #[must_use]
    pub const fn output(&self) -> f64 {
        self.output
    }
    #[must_use]
    pub fn probabilities(&self) -> Option<&[f64]> {
        self.probabilities.as_deref()
    }
    #[must_use]
    pub const fn confidence(&self) -> Option<f64> {
        self.confidence
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NoulResult {
    output: f64,
}

impl NoulResult {
    pub fn new(output: f64) -> Result<Self, ResultError> {
        validate_probability(output)?;
        Ok(Self { output })
    }
    #[must_use]
    pub const fn output(&self) -> f64 {
        self.output
    }
}

fn validate_probability(value: f64) -> Result<(), ResultError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(result_error(
            "Probabilities and confidence must be finite values from zero to one.",
        ));
    }
    Ok(())
}

fn validate_confidence(confidence: Option<f64>) -> Result<(), ResultError> {
    confidence.map_or(Ok(()), validate_probability)
}

fn validate_distribution(probabilities: impl Iterator<Item = f64>) -> Result<(), ResultError> {
    let mut sum = 0.0;
    for probability in probabilities {
        validate_probability(probability)?;
        sum += probability;
    }
    if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(result_error(
            "Probability sum must differ from one by no more than 0.02 under f64 arithmetic.",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestionResult {
    Choice(ChoiceResult),
    Score(ScoreResult),
    Noul(NoulResult),
}

impl QuestionResult {
    fn validate(
        &self,
        question: &Question,
        required: &QuestionRequirements,
    ) -> Result<(), ResultError> {
        let (probabilities, confidence) = match (self, question) {
            (Self::Choice(result), Question::Choice { .. }) => {
                result.validate(question)?;
                (result.probabilities.is_some(), result.confidence.is_some())
            }
            (Self::Score(result), Question::Score { .. }) => {
                result.validate(question)?;
                (result.probabilities.is_some(), result.confidence.is_some())
            }
            (Self::Noul(result), Question::Noul { .. }) => {
                validate_probability(result.output)?;
                (false, false)
            }
            _ => {
                return Err(result_error(
                    "Result type does not match the declared question.",
                ));
            }
        };
        if (required.probabilities && !probabilities) || (required.confidence && !confidence) {
            return Err(result_error(
                "Result is missing required probability or confidence details.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NamedAnswer {
    name: String,
    result: QuestionResult,
}

impl NamedAnswer {
    pub fn new(name: impl Into<String>, result: QuestionResult) -> Result<Self, ResultError> {
        let name = name.into();
        if !is_identifier(&name) {
            return Err(result_error("Answer name must be a valid identifier."));
        }
        Ok(Self { name, result })
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub const fn result(&self) -> &QuestionResult {
        &self.result
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionResult {
    decision: String,
    answers: Vec<NamedAnswer>,
    attempts: u8,
}

impl DecisionResult {
    /// Requirements can contain other blocks. This constructor checks the exact
    /// subset for this block. Complete assessment validation checks the full set.
    pub fn new(
        declaration: &DecisionBlock,
        requirements: &[QuestionRequirements],
        mut answers: Vec<NamedAnswer>,
        attempts: u8,
    ) -> Result<Self, ResultError> {
        validate_answers(declaration, requirements, &answers, attempts)?;
        answers.sort_by_key(|answer| {
            declaration
                .outputs
                .iter()
                .position(|output| output.name == answer.name)
        });
        let result = Self {
            decision: declaration.name.clone(),
            answers,
            attempts,
        };
        measure_decision_results_with_next(&[], &result)?;
        Ok(result)
    }

    pub fn validate_for_declaration(
        &self,
        declaration: &DecisionBlock,
        requirements: &[QuestionRequirements],
    ) -> Result<(), ResultError> {
        if self.decision != declaration.name {
            return Err(result_error(
                "Decision result does not match the declared block.",
            ));
        }
        validate_answers(declaration, requirements, &self.answers, self.attempts)?;
        if self
            .answers
            .iter()
            .zip(&declaration.outputs)
            .any(|(answer, output)| answer.name != output.name)
        {
            return Err(result_error("Answers must follow declared question order."));
        }
        measure_decision_results_with_next(&[], self)?;
        Ok(())
    }
    #[must_use]
    pub fn decision(&self) -> &str {
        &self.decision
    }
    #[must_use]
    pub fn answers(&self) -> &[NamedAnswer] {
        &self.answers
    }
    #[must_use]
    pub const fn attempts(&self) -> u8 {
        self.attempts
    }
}

fn validate_answers(
    declaration: &DecisionBlock,
    requirements: &[QuestionRequirements],
    answers: &[NamedAnswer],
    attempts: u8,
) -> Result<(), ResultError> {
    declaration.validate()?;
    if !(1..=2).contains(&attempts) {
        return Err(result_error("Decision attempts must be one or two."));
    }
    if answers.len() != declaration.outputs.len() {
        return Err(result_error(
            "Answers must match the exact declared question set.",
        ));
    }
    let required = requirements
        .iter()
        .filter(|required| required.decision == declaration.name)
        .collect::<Vec<_>>();
    if required.len() != declaration.outputs.len() {
        return Err(result_error(
            "Requirements must match the exact declared question set.",
        ));
    }
    let mut names = BTreeSet::new();
    let mut required_names = BTreeSet::new();
    for required in &required {
        if !required_names.insert(required.question.as_str()) {
            return Err(result_error("Question requirements must be unique."));
        }
        let Some(question) = declaration
            .outputs
            .iter()
            .find(|question| question.name == required.question)
        else {
            return Err(result_error("Requirements contain an undeclared question."));
        };
        required.validate_for_question(&declaration.name, question)?;
    }
    for answer in answers {
        if !names.insert(answer.name.as_str()) {
            return Err(result_error("Answer names must be unique."));
        }
        let Some(question) = declaration
            .outputs
            .iter()
            .find(|question| question.name == answer.name)
        else {
            return Err(result_error("Answer contains an undeclared question."));
        };
        let required = required
            .iter()
            .find(|required| required.question == answer.name)
            .ok_or_else(|| result_error("Answer requirements are missing."))?;
        answer.result.validate(&question.question, required)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EvaluatedAssessment {
    version: u8,
    decisions: Vec<DecisionResult>,
    attempts: u64,
}

impl EvaluatedAssessment {
    pub fn new(
        declarations: &[DecisionBlock],
        requirements: &[QuestionRequirements],
        mut decisions: Vec<DecisionResult>,
    ) -> Result<Self, ResultError> {
        validate_requirements(declarations, requirements)?;
        if decisions.len() != declarations.len() {
            return Err(result_error(
                "Results must match the exact declared decision set.",
            ));
        }
        let indices = declarations
            .iter()
            .enumerate()
            .map(|(index, declaration)| (declaration.name.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let mut names = BTreeSet::new();
        for decision in &decisions {
            if !names.insert(decision.decision.as_str()) {
                return Err(result_error("Decision results must be unique."));
            }
            let Some(index) = indices.get(decision.decision.as_str()) else {
                return Err(result_error("Results contain an undeclared decision."));
            };
            decision.validate_for_declaration(&declarations[*index], requirements)?;
        }
        let attempts = total_attempts(&decisions)?;
        decisions.sort_by_key(|decision| indices.get(decision.decision.as_str()).copied());
        let result = Self {
            version: RESULT_VERSION,
            decisions,
            attempts,
        };
        measure_results(&result)?;
        Ok(result)
    }

    #[must_use]
    pub const fn local() -> Self {
        Self {
            version: RESULT_VERSION,
            decisions: Vec::new(),
            attempts: 0,
        }
    }

    pub fn validate_for_declarations(
        &self,
        declarations: &[DecisionBlock],
        requirements: &[QuestionRequirements],
    ) -> Result<(), ResultError> {
        validate_requirements(declarations, requirements)?;
        if self.version != RESULT_VERSION {
            return Err(result_error("Saved result version is unsupported."));
        }
        if self.decisions.len() != declarations.len() {
            return Err(result_error(
                "Results must match the exact declared decision set.",
            ));
        }
        for (result, declaration) in self.decisions.iter().zip(declarations) {
            result.validate_for_declaration(declaration, requirements)?;
        }
        if self.attempts != total_attempts(&self.decisions)? {
            return Err(result_error(
                "Assessment attempt total does not match its decision results.",
            ));
        }
        measure_results(self)?;
        Ok(())
    }
    #[must_use]
    pub const fn version(&self) -> u8 {
        self.version
    }
    #[must_use]
    pub fn decisions(&self) -> &[DecisionResult] {
        &self.decisions
    }
    #[must_use]
    pub const fn attempts(&self) -> u64 {
        self.attempts
    }
}

fn total_attempts(decisions: &[DecisionResult]) -> Result<u64, ResultError> {
    decisions.iter().try_fold(0u64, |total, decision| {
        total
            .checked_add(u64::from(decision.attempts))
            .ok_or_else(|| result_error("Assessment attempt total overflowed."))
    })
}

pub fn measure_results(results: &EvaluatedAssessment) -> Result<JsonMeasurement, ResultError> {
    measure_json(results, RESULT_LIMITS).map_err(Into::into)
}

/// Checks the complete result wrapper before a caller appends a partial result.
/// This does not make an incomplete set a valid assessment.
pub fn measure_decision_results_with_next(
    results: &[DecisionResult],
    next: &DecisionResult,
) -> Result<JsonMeasurement, ResultError> {
    use serde::ser::{SerializeSeq, SerializeStruct};
    struct Sequence<'a>(&'a [DecisionResult], &'a DecisionResult);
    impl Serialize for Sequence<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut sequence = serializer.serialize_seq(Some(self.0.len() + 1))?;
            for result in self.0 {
                sequence.serialize_element(result)?;
            }
            sequence.serialize_element(self.1)?;
            sequence.end()
        }
    }
    struct Wrapper<'a> {
        sequence: Sequence<'a>,
        attempts: u64,
    }
    impl Serialize for Wrapper<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut record = serializer.serialize_struct("EvaluatedAssessment", 3)?;
            record.serialize_field("version", &RESULT_VERSION)?;
            record.serialize_field("decisions", &self.sequence)?;
            record.serialize_field("attempts", &self.attempts)?;
            record.end()
        }
    }
    if results
        .iter()
        .any(|result| result.decision == next.decision)
    {
        return Err(result_error("Decision results must be unique."));
    }
    let attempts = total_attempts(results)?
        .checked_add(u64::from(next.attempts))
        .ok_or_else(|| result_error("Assessment attempt total overflowed."))?;
    measure_json(
        &Wrapper {
            sequence: Sequence(results, next),
            attempts,
        },
        RESULT_LIMITS,
    )
    .map_err(Into::into)
}
