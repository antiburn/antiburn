use super::*;
use crate::decision::Question;
use crate::prepared_assessment::PreparedBlock;
use crate::results::{ChoiceResult, NamedAnswer, NoulResult, QuestionResult, ScoreResult};

pub(crate) fn test_prepare(
    activation: &EffectiveActivation,
    inputs: Vec<DecisionInput>,
    budget: EvaluationBudget,
) -> Result<PreparedAssessment, PreparationFailure> {
    let prepared = PreparedAssessment::new(
        "synthetic-v1".into(),
        budget,
        0,
        inputs
            .into_iter()
            .map(|input| PreparedBlock {
                decision: input.decision,
                original_input: input.input.clone(),
                request: serde_json::json!({"input": input.input.clone()}),
                input: input.input,
                report: Default::default(),
            })
            .collect(),
    )
    .map_err(|error| incompatible_preparation(&error.to_string()))?;
    prepared
        .validate_for_declarations(&activation.compiled.decisions)
        .map_err(|error| incompatible_preparation(&error.to_string()))?;
    Ok(prepared)
}

pub(crate) fn test_validate(
    activation: &EffectiveActivation,
    prepared: &PreparedAssessment,
    _: PreparationValidationPurpose,
) -> Result<(), PreparationFailure> {
    prepared
        .validate_for_declarations(&activation.compiled.decisions)
        .map_err(|error| incompatible_preparation(&error.to_string()))?;
    if prepared.profile != "synthetic-v1"
        || prepared
            .decisions
            .iter()
            .any(|block| block.request != serde_json::json!({"input": block.input}))
    {
        return Err(incompatible_preparation(
            "The synthetic request profile does not match.",
        ));
    }
    Ok(())
}

pub(crate) fn test_answer(
    activation: &EffectiveActivation,
    attempt: &DecisionAttempt,
    output: Option<&str>,
) -> DecisionResult {
    let block = activation
        .compiled
        .decisions
        .iter()
        .find(|block| block.name == attempt.decision)
        .unwrap();
    let answers = block
        .outputs
        .iter()
        .map(|named| {
            let result = match &named.question {
                Question::Choice { criteria, .. } => {
                    let output = output.unwrap_or(&criteria[0].0);
                    let probabilities = criteria
                        .iter()
                        .map(|(label, _)| {
                            (
                                label.clone(),
                                if label == output {
                                    0.98
                                } else {
                                    0.02 / (criteria.len() - 1) as f64
                                },
                            )
                        })
                        .collect();
                    QuestionResult::Choice(
                        ChoiceResult::new(&named.question, output, Some(probabilities), Some(0.9))
                            .unwrap(),
                    )
                }
                Question::Score { criteria, .. } => QuestionResult::Score(
                    ScoreResult::new(
                        &named.question,
                        (criteria.len() - 1) as f64 / 2.0,
                        Some(vec![1.0 / criteria.len() as f64; criteria.len()]),
                        Some(0.9),
                    )
                    .unwrap(),
                ),
                Question::Noul { .. } => QuestionResult::Noul(NoulResult::new(0.7).unwrap()),
            };
            NamedAnswer::new(named.name.clone(), result).unwrap()
        })
        .collect();
    DecisionResult::new(
        block,
        &activation.compiled.question_requirements(),
        answers,
        attempt.number,
    )
    .unwrap()
}
