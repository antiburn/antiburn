use std::collections::BTreeMap;

use super::assessment::CandidateComparison;
use crate::analysis::jev::JevQuestion;
use serde_json::json;

pub(super) const QUESTION_DECISION: &str = "decision";

pub(super) fn target_question_key(comparison_id: &str, question: &str) -> String {
    format!("target-{comparison_id}::{question}")
}

pub(super) fn window_questions(
    comparisons: &[&CandidateComparison],
) -> BTreeMap<String, JevQuestion> {
    comparisons
        .iter()
        .enumerate()
        .flat_map(|(index, comparison)| {
            comparison_questions(index)
                .into_iter()
                .map(|(name, question)| (target_question_key(&comparison.id, &name), question))
        })
        .collect()
}

pub(super) fn comparison_questions(target_index: usize) -> BTreeMap<String, JevQuestion> {
    BTreeMap::from([(
        QUESTION_DECISION.to_owned(),
        choice_question(
            &format!(
                "Does candidate_action directly conflict with instruction_targets[{target_index}], including its conditions and supplied exception? Current-file evidence shows the current rule, not which rule applied then. A command request does not prove success. Variable names do not prove secret values. Do not infer action from quoted text, omitted work, or missed deadlines."
            ),
            [
                (
                    "conflict",
                    "The recorded action directly conflicts with the rule.",
                ),
                ("no_conflict", "The action does not directly conflict."),
                ("uncertain", "Evidence unknown."),
            ],
        ),
    )])
}

pub(super) fn choice_question<const N: usize>(
    instructions: &str,
    criteria: [(&str, &str); N],
) -> JevQuestion {
    JevQuestion::Choice {
        instructions: json!(instructions),
        criteria: criteria
            .into_iter()
            .map(|(key, value)| (key.to_owned(), json!(value)))
            .collect(),
    }
}
