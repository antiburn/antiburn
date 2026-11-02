//! Shared declarations and capability-independent question requirements.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::compiler::InputTemplate;
use crate::fit::FitRule;

pub const MAX_QUESTIONS_PER_DECISION: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionBlock {
    pub name: String,
    pub input: InputTemplate,
    pub fit: Vec<FitRule>,
    pub outputs: Vec<NamedQuestion>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionInput {
    pub decision: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NamedQuestion {
    pub name: String,
    pub question: Question,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    Choice {
        question: String,
        criteria: Vec<(String, String)>,
    },
    Score {
        question: String,
        criteria: Vec<String>,
    },
    Noul {
        question: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Choice,
    Score,
    Noul,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRequirements {
    pub decision: String,
    pub question: String,
    pub kind: QuestionKind,
    pub criteria_count: usize,
    pub probabilities: bool,
    pub confidence: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionContractError {
    pub message: String,
}

impl fmt::Display for DecisionContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DecisionContractError {}

pub(crate) fn contract_error(message: impl Into<String>) -> DecisionContractError {
    DecisionContractError {
        message: message.into(),
    }
}

pub(crate) fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_fixed_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains("${{")
}

impl Question {
    #[must_use]
    pub const fn kind(&self) -> QuestionKind {
        match self {
            Self::Choice { .. } => QuestionKind::Choice,
            Self::Score { .. } => QuestionKind::Score,
            Self::Noul { .. } => QuestionKind::Noul,
        }
    }

    #[must_use]
    pub fn criteria_count(&self) -> usize {
        match self {
            Self::Choice { criteria, .. } => criteria.len(),
            Self::Score { criteria, .. } => criteria.len(),
            Self::Noul { .. } => 0,
        }
    }

    /// Checks language rules without applying a provider's limits.
    pub fn validate(&self) -> Result<(), DecisionContractError> {
        let question = match self {
            Self::Choice { question, criteria } => {
                let mut labels = BTreeSet::new();
                if criteria.len() < 2
                    || criteria.iter().any(|(label, description)| {
                        !is_identifier(label)
                            || !is_fixed_text(description)
                            || !labels.insert(label.as_str())
                    })
                {
                    return Err(contract_error(
                        "Choice requires at least two unique labels with fixed descriptions.",
                    ));
                }
                question
            }
            Self::Score { question, criteria } => {
                if criteria.len() < 2 || criteria.iter().any(|text| !is_fixed_text(text)) {
                    return Err(contract_error(
                        "Score requires at least two fixed criterion descriptions.",
                    ));
                }
                question
            }
            Self::Noul { question } => question,
        };
        if !is_fixed_text(question) {
            return Err(contract_error("Question text must be nonblank and fixed."));
        }
        Ok(())
    }
}

impl DecisionBlock {
    pub fn validate(&self) -> Result<(), DecisionContractError> {
        if !is_identifier(&self.name) {
            return Err(contract_error("Decision name must be a valid identifier."));
        }
        if self.outputs.is_empty() || self.outputs.len() > MAX_QUESTIONS_PER_DECISION {
            return Err(contract_error(
                "Each decision requires one to eight questions.",
            ));
        }
        let mut names = BTreeSet::new();
        for output in &self.outputs {
            if !is_identifier(&output.name) || !names.insert(output.name.as_str()) {
                return Err(contract_error("Question names must be valid and unique."));
            }
            output.question.validate()?;
        }
        Ok(())
    }
}

impl QuestionRequirements {
    pub fn new(
        decision: impl Into<String>,
        question: &NamedQuestion,
        probabilities: bool,
        confidence: bool,
    ) -> Result<Self, DecisionContractError> {
        let requirements = Self {
            decision: decision.into(),
            question: question.name.clone(),
            kind: question.question.kind(),
            criteria_count: question.question.criteria_count(),
            probabilities,
            confidence,
        };
        requirements.validate_for_question(&requirements.decision, question)?;
        Ok(requirements)
    }

    pub fn validate_for_question(
        &self,
        decision: &str,
        question: &NamedQuestion,
    ) -> Result<(), DecisionContractError> {
        question.question.validate()?;
        if !is_identifier(decision)
            || !is_identifier(&question.name)
            || self.decision != decision
            || self.question != question.name
            || self.kind != question.question.kind()
            || self.criteria_count != question.question.criteria_count()
        {
            return Err(contract_error(
                "Question requirements do not match the declaration.",
            ));
        }
        if self.kind == QuestionKind::Noul && (self.probabilities || self.confidence) {
            return Err(contract_error(
                "Noul has no probability distribution or confidence detail.",
            ));
        }
        Ok(())
    }
}

pub fn validate_declarations(declarations: &[DecisionBlock]) -> Result<(), DecisionContractError> {
    let mut names = BTreeSet::new();
    for declaration in declarations {
        declaration.validate()?;
        if !names.insert(declaration.name.as_str()) {
            return Err(contract_error("Decision names must be unique."));
        }
    }
    Ok(())
}

/// Requires one requirements record for every declared question, including those
/// that do not read optional details.
pub fn validate_requirements(
    declarations: &[DecisionBlock],
    requirements: &[QuestionRequirements],
) -> Result<(), DecisionContractError> {
    validate_declarations(declarations)?;
    let expected = declarations
        .iter()
        .flat_map(|block| {
            block
                .outputs
                .iter()
                .map(move |question| ((block.name.as_str(), question.name.as_str()), question))
        })
        .collect::<BTreeMap<_, _>>();
    if expected.len() != requirements.len() {
        return Err(contract_error(
            "Requirements must match the exact declared question set.",
        ));
    }
    let mut names = BTreeSet::new();
    for required in requirements {
        if !names.insert((&required.decision, &required.question)) {
            return Err(contract_error("Question requirements must be unique."));
        }
        let Some(question) =
            expected.get(&(required.decision.as_str(), required.question.as_str()))
        else {
            return Err(contract_error(
                "Requirements contain an undeclared question.",
            ));
        };
        required.validate_for_question(&required.decision, question)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecisionAttempt {
    pub decision: String,
    pub number: u8,
}

impl DecisionAttempt {
    pub fn new(decision: impl Into<String>, number: u8) -> Result<Self, DecisionContractError> {
        let attempt = Self {
            decision: decision.into(),
            number,
        };
        attempt.validate()?;
        Ok(attempt)
    }

    pub fn validate(&self) -> Result<(), DecisionContractError> {
        if !is_identifier(&self.decision) || !(1..=2).contains(&self.number) {
            return Err(contract_error(
                "A decision attempt requires a valid name and number one or two.",
            ));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for DecisionAttempt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct SavedAttempt {
            decision: String,
            number: u8,
        }
        let saved = SavedAttempt::deserialize(deserializer)?;
        Self::new(saved.decision, saved.number).map_err(serde::de::Error::custom)
    }
}

/// Encodes candidate, UTF-8 name length, name, and attempt without provider data.
#[must_use]
pub fn attempt_id(candidate: &[u8; 32], attempt: &DecisionAttempt) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(96 + attempt.decision.len());
    encoded.push_str("burn-check-decision-v1:32:");
    for byte in candidate {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    write!(
        &mut encoded,
        ":{}:{}:1:{}",
        attempt.decision.len(),
        attempt.decision,
        attempt.number
    )
    .expect("writing to a String cannot fail");
    encoded
}
