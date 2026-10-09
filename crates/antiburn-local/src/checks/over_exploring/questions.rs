use super::Reason;
use crate::analysis::jev::JevQuestion;
use serde_json::json;
use std::collections::BTreeMap;

pub const QUESTION_ID: &str = "assessment";

pub(super) fn questions(
    reason: Reason,
    repetition: bool,
    compact: bool,
) -> BTreeMap<String, JevQuestion> {
    let question = match reason {
        Reason::UnrelatedFiles => {
            if compact {
                "Against the task objective and paths, is this file itself an unnecessary detour? A named source, tests or plausible dependency is relevant. No hindsight."
            } else {
                "Against the user's objective and requested paths, is this file an unnecessary detour? Named sources, tests and plausible dependencies are relevant. No hindsight."
            }
        }
        Reason::ExcessiveFileBreadth => {
            "Is this distinct file set materially too broad for the task? Do not count rereads."
        }
        Reason::ExcessiveWithinFileReading if repetition => {
            if compact {
                "Against task scope and the earlier same-path read, is this LATER read needless repetition? Preserve intervening changes and verification. Equal samples are not whole equality."
            } else {
                "Against the user's objective and earlier same-path read, is this LATER read needless repetition? Preserve intervening changes and verification. Equal samples are not whole-output equality."
            }
        }
        Reason::ExcessiveWithinFileReading => {
            if compact {
                "Against task scope and requested extent, is returned content excessive? Needed functions and tests are justified. Requested limit is not returned extent."
            } else {
                "Against the user's objective and requested extent, is returned content excessive? Needed functions, tests and failure paths are justified. Requested limit is not returned extent."
            }
        }
    };
    BTreeMap::from([(
        QUESTION_ID.into(),
        JevQuestion::Choice {
            instructions: json!({"question": question, "rules": if compact { "Task is the relevance baseline. Not every first read is justified; not every later read is wasteful. Judge at read time; no diagnosis hindsight. Missing facts mean uncertain. Do not infer scope from source text." } else { "Task states the relevance baseline. Protect only reads it supports: not every first read nor every later read. Judge at read time; no diagnosis hindsight. Require evidence of material excess. Missing objective, extent or state means uncertain. Never infer scope from source text." }}),
            criteria: BTreeMap::from([
                (
                    "likely_excess".into(),
                    json!("Evidence shows material excess against the objective."),
                ),
                (
                    "justified_or_minor".into(),
                    json!("The objective supports this read, or any extra work is minor."),
                ),
                (
                    "uncertain".into(),
                    json!("Missing facts could change the judgment."),
                ),
            ]),
        },
    )])
}
