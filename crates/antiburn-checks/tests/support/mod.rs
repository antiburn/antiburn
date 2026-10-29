use std::collections::BTreeMap;

use antiburn_checks::compiler::{CompiledCheck, InputLiteral, InputTemplate};
use antiburn_checks::decision::{DecisionBlock, NamedQuestion, Question, QuestionRequirements};
use antiburn_checks::results::{
    ChoiceResult, DecisionResult, EvaluatedAssessment, NamedAnswer, QuestionResult, ResultError,
};

pub fn choice_labels(check: &CompiledCheck) -> Vec<String> {
    let Question::Choice { criteria, .. } = &check.decisions[0].outputs[0].question else {
        panic!("The fixture must declare one Choice question.");
    };
    criteria.iter().map(|(label, _)| label.clone()).collect()
}

pub fn choice_block(decision: &str, question: &str, labels: &[String]) -> DecisionBlock {
    DecisionBlock {
        name: decision.into(),
        input: InputTemplate::Object(vec![(
            "fixture".into(),
            InputTemplate::Literal(InputLiteral::Boolean(true)),
        )]),
        fit: vec![],
        outputs: vec![NamedQuestion {
            name: question.into(),
            question: Question::Choice {
                question: "Which label applies?".into(),
                criteria: labels
                    .iter()
                    .map(|label| (label.clone(), "Synthetic description.".into()))
                    .collect(),
            },
        }],
    }
}

pub fn choice_for_check(
    check: &CompiledCheck,
    output: &str,
    probabilities: Option<BTreeMap<String, f64>>,
) -> Result<EvaluatedAssessment, ResultError> {
    let declaration = &check.decisions[0];
    let requirements = check.question_requirements();
    let question = &declaration.outputs[0];
    let answer = NamedAnswer::new(
        &question.name,
        QuestionResult::Choice(ChoiceResult::new(
            &question.question,
            output,
            probabilities,
            None,
        )?),
    )?;
    let result = DecisionResult::new(declaration, &requirements, vec![answer], 1)?;
    EvaluatedAssessment::new(&check.decisions, &requirements, vec![result])
}

pub fn assessed_choice(
    decision: &str,
    question: &str,
    labels: &[String],
    output: &str,
    probabilities: Option<BTreeMap<String, f64>>,
) -> Result<EvaluatedAssessment, ResultError> {
    let declaration = choice_block(decision, question, labels);
    let required = QuestionRequirements::new(decision, &declaration.outputs[0], false, false)?;
    let answer = NamedAnswer::new(
        question,
        QuestionResult::Choice(ChoiceResult::new(
            &declaration.outputs[0].question,
            output,
            probabilities,
            None,
        )?),
    )?;
    let result = DecisionResult::new(
        &declaration,
        std::slice::from_ref(&required),
        vec![answer],
        1,
    )?;
    EvaluatedAssessment::new(&[declaration], &[required], vec![result])
}
