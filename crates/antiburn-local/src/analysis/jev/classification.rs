//! Bounded reference classification with exact, evaluator-bound identities.

use super::{
    JevAnswer, JevEvidenceReference, JevInputWindow, JevQuestion, JevWorkItem, JevWorkItemResult,
    highest_probability_choice,
};
use crate::checks::ignored_instructions::sha256_hex;
use serde_json::json;
use std::collections::BTreeMap;

pub trait ReferenceClassifier {
    type Properties;

    fn questions(&self) -> BTreeMap<String, JevQuestion>;
    fn revision(&self) -> String;
    fn properties(&self, result: Option<&JevWorkItemResult>) -> Self::Properties;

    fn prepare(
        &self,
        reference: serde_json::Value,
        evidence: Vec<JevEvidenceReference>,
    ) -> Result<JevWorkItem, super::JevError> {
        reference_classification(reference, &self.revision(), self.questions(), evidence)
    }
}

pub fn reference_classification(
    reference_context: serde_json::Value,
    evaluator_revision: &str,
    questions: BTreeMap<String, JevQuestion>,
    evidence: Vec<JevEvidenceReference>,
) -> Result<JevWorkItem, super::JevError> {
    let bytes = serde_json::to_vec(&(
        super::PINNED_MODEL,
        crate::analysis::PARSER_REVISION,
        evaluator_revision,
        &reference_context,
        &questions,
        &evidence,
    ))
    .map_err(|_| super::JevError::InvalidCheckContext)?;
    Ok(JevWorkItem {
        id: format!("classification-{}", sha256_hex(&bytes)),
        window: JevInputWindow {
            fields: json!({"reference": reference_context}),
            evidence,
        },
        questions,
    })
}

pub fn confident_choice<'a>(
    result: &'a JevWorkItemResult,
    question: &str,
    threshold: f64,
) -> Option<&'a str> {
    let JevAnswer::Choice {
        choice,
        probabilities,
        confidence,
        ..
    } = result.answers.get(question)?
    else {
        return None;
    };
    if !threshold.is_finite()
        || !(0.0..=1.0).contains(&threshold)
        || !super::valid_probability(*confidence)
        || !probabilities.contains_key(choice)
        || probabilities
            .values()
            .any(|value| !super::valid_probability(*value))
        || super::validate_probability_sum(probabilities.values().copied()).is_err()
    {
        return None;
    }
    let selected = highest_probability_choice(choice, probabilities)?;
    (probabilities.get(selected).copied()? >= threshold).then_some(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_missing_classification_cannot_settle_a_property() {
        let mut result = JevWorkItemResult {
            request_id: "request".to_owned(),
            work_item_id: "reference".to_owned(),
            answers: BTreeMap::new(),
            evidence: Vec::new(),
            model: super::super::PINNED_MODEL.to_owned(),
            usage: super::super::JevUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        };
        assert!(confident_choice(&result, "property", 0.9).is_none());
        for (choice, first, second, confidence) in [
            ("yes", f64::NAN, 0.0, 1.0),
            ("yes", 1.1, -0.1, 1.0),
            ("yes", 0.99, 0.99, 1.0),
            ("missing", 0.99, 0.01, 1.0),
            ("yes", 0.99, 0.01, f64::INFINITY),
        ] {
            result.answers.insert(
                "property".to_owned(),
                JevAnswer::Choice {
                    choice: choice.to_owned(),
                    probabilities: BTreeMap::from([
                        ("yes".to_owned(), first),
                        ("no".to_owned(), second),
                    ]),
                    confidence,
                },
            );
            assert!(confident_choice(&result, "property", 0.9).is_none());
        }
    }

    struct LanguageClassifier;

    impl ReferenceClassifier for LanguageClassifier {
        type Properties = Option<String>;

        fn revision(&self) -> String {
            "language-classification:1".to_owned()
        }

        fn questions(&self) -> BTreeMap<String, JevQuestion> {
            BTreeMap::from([(
                "language".to_owned(),
                JevQuestion::Choice {
                    instructions: json!("Which language does reference.text use?"),
                    criteria: BTreeMap::from([
                        ("english".to_owned(), json!("English")),
                        ("other".to_owned(), json!("Another language")),
                    ]),
                },
            )])
        }

        fn properties(&self, result: Option<&JevWorkItemResult>) -> Self::Properties {
            result
                .and_then(|result| confident_choice(result, "language", 0.9))
                .map(str::to_owned)
        }
    }

    #[test]
    fn a_second_classifier_uses_the_same_contract_and_exact_context_identity() {
        let first = LanguageClassifier
            .prepare(json!({"text":"A reference."}), Vec::new())
            .unwrap();
        let same = LanguageClassifier
            .prepare(json!({"text":"A reference."}), Vec::new())
            .unwrap();
        let changed = LanguageClassifier
            .prepare(json!({"text":"A changed reference."}), Vec::new())
            .unwrap();
        assert_eq!(first, same);
        assert_ne!(first.id, changed.id);
        let revision = reference_classification(
            json!({"text":"A reference."}),
            "language-classification:2",
            LanguageClassifier.questions(),
            Vec::new(),
        )
        .unwrap();
        assert_ne!(first.id, revision.id);
        assert!(LanguageClassifier.properties(None).is_none());
        assert!(!first.questions.contains_key("obligation"));
    }
}
