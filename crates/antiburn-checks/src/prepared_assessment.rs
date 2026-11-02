//! Bounded immutable requests for one complete model assessment.

use std::collections::BTreeSet;
use std::fmt;

use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::bounded_json::{
    BoundedJsonError, JsonLimits, JsonMeasurement, decode_strict_json, measure_json,
};
use crate::decision::{DecisionBlock, is_identifier, validate_declarations};
use crate::expression_eval::EvaluationBudget;
use crate::fit::{FitOmission, FitReport, FitStrategy, MAX_FIT_INPUT_DEPTH};

pub const PREPARED_ASSESSMENT_VERSION: u8 = 1;
pub const MAX_PREPARED_ASSESSMENT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_PREPARED_ASSESSMENT_NODES: usize = 65_536;
pub const MAX_ASSESSMENT_FITTING_WORK_BYTES: u64 = 64 * 1024 * 1024;
pub const PREPARATION_LIMITS: JsonLimits = JsonLimits {
    max_bytes: MAX_PREPARED_ASSESSMENT_BYTES,
    max_nodes: MAX_PREPARED_ASSESSMENT_NODES,
    max_depth: MAX_FIT_INPUT_DEPTH,
};

// The internally tagged Ready record adds one value and these serialized bytes.
const READY_TAG_BYTES: u64 = 17;
const INNER_LIMITS: JsonLimits = JsonLimits {
    max_bytes: MAX_PREPARED_ASSESSMENT_BYTES - READY_TAG_BYTES,
    max_nodes: MAX_PREPARED_ASSESSMENT_NODES - 1,
    max_depth: MAX_FIT_INPUT_DEPTH,
};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PreparedAssessment {
    pub version: u8,
    /// The adapter owns this sanitized descriptor or digest. Core treats it as opaque.
    pub profile: String,
    pub evaluation_budget: EvaluationBudget,
    /// Consumed preparation work, not an allowance for restart validation.
    pub fitting_work_bytes: u64,
    pub decisions: Vec<PreparedBlock>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PreparedBlock {
    pub decision: String,
    pub original_input: Value,
    pub input: Value,
    pub request: Value,
    pub report: FitReport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedAssessmentError {
    pub message: String,
}

impl fmt::Display for PreparedAssessmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for PreparedAssessmentError {}
impl From<BoundedJsonError> for PreparedAssessmentError {
    fn from(error: BoundedJsonError) -> Self {
        invalid(error.to_string())
    }
}
fn invalid(message: impl Into<String>) -> PreparedAssessmentError {
    PreparedAssessmentError {
        message: message.into(),
    }
}

impl PreparedAssessment {
    pub fn new(
        profile: String,
        evaluation_budget: EvaluationBudget,
        fitting_work_bytes: u64,
        decisions: Vec<PreparedBlock>,
    ) -> Result<Self, PreparedAssessmentError> {
        let prepared = Self {
            version: PREPARED_ASSESSMENT_VERSION,
            profile,
            evaluation_budget,
            fitting_work_bytes,
            decisions,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    fn validate_header(&self) -> Result<(), PreparedAssessmentError> {
        if self.version != PREPARED_ASSESSMENT_VERSION {
            return Err(invalid("Saved preparation version is unsupported."));
        }
        if self.profile.trim().is_empty() {
            return Err(invalid("Saved preparation profile is missing."));
        }
        if self.fitting_work_bytes > MAX_ASSESSMENT_FITTING_WORK_BYTES {
            return Err(invalid("Saved fitting work exceeds the assessment limit."));
        }
        self.evaluation_budget
            .validate_for_resume()
            .map_err(|error| invalid(error.message))
    }

    /// Checks the complete Ready record, including its status tag. Request
    /// construction and profile compatibility remain the adapter's responsibility.
    pub fn validate(&self) -> Result<(), PreparedAssessmentError> {
        self.validate_header()?;
        if self.decisions.is_empty() {
            return Err(invalid("Model preparation requires at least one decision."));
        }
        validate_names(&self.decisions, None)?;
        measure_prepared_assessment(self)?;
        Ok(())
    }

    pub fn validate_for_declarations(
        &self,
        declarations: &[DecisionBlock],
    ) -> Result<(), PreparedAssessmentError> {
        self.validate()?;
        validate_declarations(declarations).map_err(|error| invalid(error.message))?;
        if self.decisions.len() != declarations.len()
            || self
                .decisions
                .iter()
                .zip(declarations)
                .any(|(block, declaration)| block.decision != declaration.name)
        {
            return Err(invalid(
                "Prepared decisions must match the exact declaration order.",
            ));
        }
        Ok(())
    }

    /// Checks borrowed data before moving a block into the retained set. An empty
    /// builder is allowed here, but it is not a valid complete model preparation.
    pub fn check_append(
        &self,
        next: &PreparedBlock,
    ) -> Result<JsonMeasurement, PreparedAssessmentError> {
        self.validate_header()?;
        validate_names(&self.decisions, Some(next))?;
        measure_prepared_assessment_with_block(self, next)
    }

    pub fn try_push(&mut self, next: PreparedBlock) -> Result<(), PreparedAssessmentError> {
        self.check_append(&next)?;
        self.decisions.push(next);
        Ok(())
    }
}

fn validate_names(
    blocks: &[PreparedBlock],
    next: Option<&PreparedBlock>,
) -> Result<(), PreparedAssessmentError> {
    let mut names = BTreeSet::new();
    for block in blocks.iter().chain(next) {
        if !is_identifier(&block.decision) || !names.insert(block.decision.as_str()) {
            return Err(invalid("Prepared decision names must be valid and unique."));
        }
    }
    Ok(())
}

struct BlocksView<'a> {
    blocks: &'a [PreparedBlock],
    next: Option<&'a PreparedBlock>,
}
impl Serialize for BlocksView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence =
            serializer.serialize_seq(Some(self.blocks.len() + usize::from(self.next.is_some())))?;
        for block in self.blocks.iter().chain(self.next) {
            sequence.serialize_element(block)?;
        }
        sequence.end()
    }
}

struct ReadyView<'a> {
    prepared: &'a PreparedAssessment,
    next: Option<&'a PreparedBlock>,
}
impl Serialize for ReadyView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("PreparationRecord", 6)?;
        record.serialize_field("status", "ready")?;
        record.serialize_field("version", &self.prepared.version)?;
        record.serialize_field("profile", &self.prepared.profile)?;
        record.serialize_field("evaluation_budget", &self.prepared.evaluation_budget)?;
        record.serialize_field("fitting_work_bytes", &self.prepared.fitting_work_bytes)?;
        record.serialize_field(
            "decisions",
            &BlocksView {
                blocks: &self.prepared.decisions,
                next: self.next,
            },
        )?;
        record.end()
    }
}

/// Includes the future `PreparationRecord::Ready` tag, not only its inner set.
pub fn measure_prepared_assessment(
    prepared: &PreparedAssessment,
) -> Result<JsonMeasurement, PreparedAssessmentError> {
    measure_json(
        &ReadyView {
            prepared,
            next: None,
        },
        PREPARATION_LIMITS,
    )
    .map_err(Into::into)
}

pub fn measure_prepared_assessment_with_block(
    prepared: &PreparedAssessment,
    next: &PreparedBlock,
) -> Result<JsonMeasurement, PreparedAssessmentError> {
    measure_json(
        &ReadyView {
            prepared,
            next: Some(next),
        },
        PREPARATION_LIMITS,
    )
    .map_err(Into::into)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedAssessment {
    version: u8,
    profile: String,
    evaluation_budget: EvaluationBudget,
    fitting_work_bytes: u64,
    decisions: Vec<SavedBlock>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedBlock {
    decision: String,
    original_input: Value,
    input: Value,
    request: Value,
    report: SavedReport,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedReport {
    omissions: Vec<SavedOmission>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedOmission {
    path: String,
    strategy: FitStrategy,
    original_count: usize,
    retained_count: usize,
}

impl<'de> Deserialize<'de> for PreparedAssessment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = crate::bounded_json::deserialize_strict_value(deserializer, INNER_LIMITS)?;
        let saved: SavedAssessment =
            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        let prepared = Self {
            version: saved.version,
            profile: saved.profile,
            evaluation_budget: saved.evaluation_budget,
            fitting_work_bytes: saved.fitting_work_bytes,
            decisions: saved
                .decisions
                .into_iter()
                .map(|block| PreparedBlock {
                    decision: block.decision,
                    original_input: block.original_input,
                    input: block.input,
                    request: block.request,
                    report: FitReport {
                        omissions: block
                            .report
                            .omissions
                            .into_iter()
                            .map(|omission| FitOmission {
                                path: omission.path,
                                strategy: omission.strategy,
                                original_count: omission.original_count,
                                retained_count: omission.retained_count,
                            })
                            .collect(),
                    },
                })
                .collect(),
        };
        prepared.validate().map_err(serde::de::Error::custom)?;
        Ok(prepared)
    }
}

/// Decodes the inner assessment. Bounds reserve the full Ready tag. Record-level
/// callers must first decode the whole record with `PREPARATION_LIMITS`; Serde
/// alone cannot bound raw bytes, including whitespace, before decoding.
pub fn decode_prepared_assessment(
    bytes: &[u8],
) -> Result<PreparedAssessment, PreparedAssessmentError> {
    let value = decode_strict_json(bytes, INNER_LIMITS)?;
    serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
}
