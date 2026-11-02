use serde::ser::{SerializeMap, SerializeSeq, SerializeStruct};

use super::*;

// A finite binary64 number needs at most 17 significant decimal digits, a sign,
// a point, and an exponent. A quoted 32-byte placeholder reserves more space.
const NUMBER_RESERVATION: &str = "00000000000000000000000000000000";

/// Reserves the complete typed result shape before dispatch. This includes all
/// optional details, even when actions do not require them, because an adapter
/// can return those details. It reserves the longest declared Choice label and
/// more than the maximum finite f64 JSON width for each number. It never uses
/// average label lengths or expected probability values.
pub fn measure_result_capacity(
    declarations: &[DecisionBlock],
    requirements: &[QuestionRequirements],
) -> Result<JsonMeasurement, ResultError> {
    validate_requirements(declarations, requirements)?;
    measure_json(&AssessmentCapacity(declarations), RESULT_LIMITS).map_err(Into::into)
}

struct AssessmentCapacity<'a>(&'a [DecisionBlock]);
impl Serialize for AssessmentCapacity<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("EvaluatedAssessment", 3)?;
        record.serialize_field("version", &RESULT_VERSION)?;
        record.serialize_field("decisions", &DecisionsCapacity(self.0))?;
        record.serialize_field("attempts", &u64::MAX)?;
        record.end()
    }
}

struct DecisionsCapacity<'a>(&'a [DecisionBlock]);
impl Serialize for DecisionsCapacity<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for decision in self.0 {
            sequence.serialize_element(&DecisionCapacity(decision))?;
        }
        sequence.end()
    }
}

struct DecisionCapacity<'a>(&'a DecisionBlock);
impl Serialize for DecisionCapacity<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("DecisionResult", 3)?;
        record.serialize_field("decision", &self.0.name)?;
        record.serialize_field("answers", &AnswersCapacity(&self.0.outputs))?;
        record.serialize_field("attempts", &2u8)?;
        record.end()
    }
}

struct AnswersCapacity<'a>(&'a [crate::decision::NamedQuestion]);
impl Serialize for AnswersCapacity<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for question in self.0 {
            sequence.serialize_element(&AnswerCapacity(question))?;
        }
        sequence.end()
    }
}

struct AnswerCapacity<'a>(&'a crate::decision::NamedQuestion);
impl Serialize for AnswerCapacity<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("NamedAnswer", 2)?;
        record.serialize_field("name", &self.0.name)?;
        record.serialize_field("result", &QuestionCapacity(&self.0.question))?;
        record.end()
    }
}

struct QuestionCapacity<'a>(&'a Question);
impl Serialize for QuestionCapacity<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct(
            "QuestionResult",
            if matches!(self.0, Question::Noul { .. }) {
                2
            } else {
                4
            },
        )?;
        match self.0 {
            Question::Choice { criteria, .. } => {
                record.serialize_field("type", "choice")?;
                let longest = criteria
                    .iter()
                    .map(|(label, _)| label)
                    .max_by_key(|label| label.len())
                    .ok_or_else(|| serde::ser::Error::custom("Choice criteria are missing."))?;
                record.serialize_field("output", longest)?;
                record.serialize_field("probabilities", &ChoiceCapacity(criteria))?;
                record.serialize_field("confidence", NUMBER_RESERVATION)?;
            }
            Question::Score { criteria, .. } => {
                record.serialize_field("type", "score")?;
                record.serialize_field("output", NUMBER_RESERVATION)?;
                record.serialize_field("probabilities", &ScoreCapacity(criteria.len()))?;
                record.serialize_field("confidence", NUMBER_RESERVATION)?;
            }
            Question::Noul { .. } => {
                record.serialize_field("type", "noul")?;
                record.serialize_field("output", NUMBER_RESERVATION)?;
            }
        }
        record.end()
    }
}

struct ChoiceCapacity<'a>(&'a [(String, String)]);
impl Serialize for ChoiceCapacity<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut mapping = serializer.serialize_map(Some(self.0.len()))?;
        for (label, _) in self.0 {
            mapping.serialize_entry(label, NUMBER_RESERVATION)?;
        }
        mapping.end()
    }
}

struct ScoreCapacity(usize);
impl Serialize for ScoreCapacity {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0))?;
        for _ in 0..self.0 {
            sequence.serialize_element(NUMBER_RESERVATION)?;
        }
        sequence.end()
    }
}
