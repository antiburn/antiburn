use super::*;
use capabilities::{ModelCapabilities, ModelLimitOverride};
use serde_json::json;

fn native_request() -> JevRequest {
    JevRequest {
        model: NATIVE_MODEL_ALIAS.into(),
        state: json!("0"),
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Noul {
                instructions: json!("Assess the input"),
                criteria: None,
            },
        )]),
    }
}

fn native_response() -> JevResponse {
    JevResponse {
        model: "jev-1.14.0".into(),
        answers: BTreeMap::from([("q".into(), JevAnswer::Noul { noul: 0.5 })]),
        usage: JevUsage {
            input_tokens: 12,
            output_tokens: 1,
        },
    }
}

fn state_and_question_tokens(request: &JevRequest) -> u64 {
    json_count(&request.state, usize::MAX)
        .unwrap()
        .estimate
        .tokens()
        + request
            .questions
            .values()
            .map(|question| json_count(question, usize::MAX).unwrap().estimate.tokens())
            .max()
            .unwrap()
}

#[test]
fn large_native_input_passes_without_widening_the_standard_route() {
    let mut request = native_request();
    request.state = json!({"evidence": "a".repeat(320_000)});
    let bytes =
        validate_jev_request_with_capabilities(&request, &ModelCapabilities::jev_native()).unwrap();
    assert!(bytes > 256 * 1024);
    assert_eq!(bytes, serde_json::to_vec(&request).unwrap().len());
    assert!(estimate_jev_request_tokens(&request).unwrap() > 60 * 1024);

    assert_eq!(
        validate_jev_request(&request),
        Err(JevError::UnsupportedModel)
    );
    request.model = PINNED_MODEL.into();
    assert!(matches!(
        validate_jev_request(&request),
        Err(JevError::RequestTooLarge {
            maximum: 262_144,
            ..
        })
    ));
}

#[test]
fn native_total_estimate_accepts_the_exact_ceiling_and_rejects_one_more_token() {
    let limits = ModelCapabilities::jev_native();
    let maximum = limits.usable_input_tokens().unwrap();
    let mut request = native_request();
    let overhead = estimate_jev_request_tokens(&request).unwrap() - 1;
    request.state = json!("0".repeat((maximum - overhead) as usize));
    assert_eq!(estimate_jev_request_tokens(&request), Ok(maximum));
    assert!(state_and_question_tokens(&request) < maximum);
    assert!(validate_jev_request_with_capabilities(&request, &limits).is_ok());

    request.state = json!(format!("{}0", request.state.as_str().unwrap()));
    assert_eq!(estimate_jev_request_tokens(&request), Ok(maximum + 1));
    assert!(state_and_question_tokens(&request) <= maximum);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::RequestTokenLimitExceeded {
            tokens: maximum + 1,
            maximum,
        })
    );
}

#[test]
fn native_state_estimate_keeps_its_independent_exact_ceiling() {
    let limits = ModelCapabilities::jev_native().with_override(&ModelLimitOverride {
        state_and_longest_question_tokens: Some(250_000),
        ..Default::default()
    });
    let maximum = limits.usable_state_tokens().unwrap();
    let mut request = native_request();
    let overhead = state_and_question_tokens(&request) - 1;
    request.state = json!("0".repeat((maximum - overhead) as usize));
    assert_eq!(state_and_question_tokens(&request), maximum);
    assert!(estimate_jev_request_tokens(&request).unwrap() < limits.usable_input_tokens().unwrap());
    assert!(validate_jev_request_with_capabilities(&request, &limits).is_ok());

    request.state = json!(format!("{}0", request.state.as_str().unwrap()));
    assert_eq!(state_and_question_tokens(&request), maximum + 1);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::RequestTokenLimitExceeded {
            tokens: maximum + 1,
            maximum,
        })
    );
}

#[test]
fn native_body_guard_accepts_its_exact_byte_boundary() {
    let mut limits = ModelCapabilities::jev_native();
    // Isolate the byte guard from the stricter token guards.
    limits.total_input_tokens.value = Some(1_000_000);
    limits.state_and_longest_question_tokens.value = Some(1_000_000);
    let maximum = limits.request_body_bytes.value.unwrap() as usize;
    let mut request = native_request();
    let overhead = serde_json::to_vec(&request).unwrap().len() - 1;
    request.state = json!("0".repeat(maximum - overhead));
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Ok(maximum)
    );
    request.state = json!(format!("{}0", request.state.as_str().unwrap()));
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::RequestTooLarge {
            bytes: maximum + 1,
            maximum,
        })
    );
}

#[test]
fn request_estimate_counts_all_serialized_escaping_utf8_and_metadata_without_reserve() {
    let mut request = native_request();
    request.state = json!({
        "evidence": "letters \" \\ \n \t \u{0} 漢字 🚀",
        "input_omissions": {
            "framing": "The selected text is incomplete.",
            "targets": [{"path": "activity[0].content", "strategy": "truncate", "original_count": 4096, "retained_count": 1024}]
        }
    });
    request.questions.insert(
        "choice-漢字".into(),
        JevQuestion::Choice {
            instructions: json!("Choose \" \\ \n 漢字 🚀"),
            criteria: BTreeMap::from([
                ("yes".into(), json!({"description": "Approve \t 🚀"})),
                ("no".into(), json!("Reject \u{0}")),
            ]),
        },
    );
    let serialized = serde_json::to_vec(&request).unwrap();
    let units: u64 = serialized
        .iter()
        .map(|byte| if byte.is_ascii_alphabetic() { 3 } else { 4 })
        .sum();
    let expected = units.div_ceil(4);
    assert_eq!(estimate_jev_request_tokens(&request), Ok(expected));
    let mut limits = ModelCapabilities::jev_native();
    limits.total_input_tokens.value = Some(expected + limits.rendering_reserve_tokens);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Ok(serialized.len())
    );
    limits.total_input_tokens.value = Some(expected + limits.rendering_reserve_tokens - 1);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::RequestTokenLimitExceeded {
            tokens: expected,
            maximum: expected - 1,
        })
    );
    request.questions.remove("choice-漢字");
    let without_choice = estimate_jev_request_tokens(&request).unwrap();
    assert!(without_choice < expected);
    request
        .state
        .as_object_mut()
        .unwrap()
        .remove("input_omissions");
    assert!(estimate_jev_request_tokens(&request).unwrap() < without_choice);
}

#[test]
fn only_explicit_native_validation_accepts_concrete_or_future_alias_responses() {
    let request = native_request();
    let mut response = native_response();
    let limits = ModelCapabilities::jev_native();
    for model in ["jev-1.14.0", "future/jev:release_2040", NATIVE_MODEL_ALIAS] {
        response.model = model.into();
        assert_eq!(
            validate_native_jev_response_with_capabilities(&response, &request, &limits),
            Ok(())
        );
        let strict = if model == NATIVE_MODEL_ALIAS {
            Ok(())
        } else {
            Err(JevError::ResponseModelMismatch)
        };
        assert_eq!(validate_jev_response(&response, &request), strict);
        assert_eq!(
            validate_jev_response_with_capabilities(&response, &request, &limits),
            strict
        );
    }

    let mut standard_request = request;
    standard_request.model = PINNED_MODEL.into();
    let standard = ModelCapabilities::jev_default();
    response.model = "jev-1.14.0".into();
    assert_eq!(
        validate_native_jev_response_with_capabilities(&response, &standard_request, &standard),
        Err(JevError::ResponseModelMismatch)
    );
    response.model = PINNED_MODEL.into();
    assert_eq!(
        validate_native_jev_response_with_capabilities(&response, &standard_request, &standard),
        Ok(())
    );
}

#[test]
fn native_response_identifiers_use_the_bounded_local_character_rules() {
    let request = native_request();
    let mut response = native_response();
    let limits = ModelCapabilities::jev_native();
    response.model = "a".repeat(128);
    assert_eq!(
        validate_native_jev_response_with_capabilities(&response, &request, &limits),
        Ok(())
    );
    for model in [
        "",
        " ",
        "a b",
        "a\nb",
        "漢字",
        "model?",
        "model%",
        &"a".repeat(129),
    ] {
        response.model = model.into();
        assert_eq!(
            validate_native_jev_response_with_capabilities(&response, &request, &limits),
            Err(JevError::ResponseModelMismatch),
            "{model:?}"
        );
    }
}

#[test]
fn native_alias_policy_keeps_capability_and_answer_checks() {
    let request = native_request();
    let response = native_response();
    let mut limits = ModelCapabilities::jev_native();
    limits.model = PINNED_MODEL.into();
    assert_eq!(
        validate_native_jev_response_with_capabilities(&response, &request, &limits),
        Err(JevError::QuestionLimitExceeded)
    );
    limits = ModelCapabilities::jev_native();
    limits.questions_per_request.value = Some(0);
    assert_eq!(
        validate_native_jev_response_with_capabilities(&response, &request, &limits),
        Err(JevError::QuestionLimitExceeded)
    );

    limits = ModelCapabilities::jev_native();
    let mut invalid = response.clone();
    invalid.answers.clear();
    assert_eq!(
        validate_native_jev_response_with_capabilities(&invalid, &request, &limits),
        Err(JevError::ResponseAnswerCountMismatch)
    );
    invalid
        .answers
        .insert("other".into(), JevAnswer::Noul { noul: 0.5 });
    assert_eq!(
        validate_native_jev_response_with_capabilities(&invalid, &request, &limits),
        Err(JevError::ResponseAnswerMissing)
    );
    for noul in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        invalid.answers = BTreeMap::from([("q".into(), JevAnswer::Noul { noul })]);
        assert_eq!(
            validate_native_jev_response_with_capabilities(&invalid, &request, &limits),
            Err(JevError::InvalidNoulProbability)
        );
    }
}

#[test]
fn native_categorical_answers_keep_strict_criteria_distributions_and_types() {
    let mut request = native_request();
    let mut response = native_response();
    for (question, answer, invalid_fields) in [
        (
            JevQuestion::Choice {
                instructions: json!("Choose one"),
                criteria: BTreeMap::from([
                    ("yes".into(), json!("Yes")),
                    ("no".into(), json!("No")),
                ]),
            },
            json!({"type":"choice","choice":"yes","probabilities":{"yes":0.9,"no":0.1},"confidence":0.8}),
            vec![
                (
                    "choice",
                    json!("other"),
                    JevError::InvalidChoiceDistribution,
                ),
                (
                    "confidence",
                    json!(1.1),
                    JevError::InvalidChoiceDistribution,
                ),
                (
                    "probabilities",
                    json!({"yes":0.9}),
                    JevError::InvalidChoiceDistribution,
                ),
                (
                    "probabilities",
                    json!({"yes":0.9,"no":0.1,"other":0.0}),
                    JevError::InvalidChoiceDistribution,
                ),
                (
                    "probabilities",
                    json!({"yes":0.2,"no":0.1}),
                    JevError::InvalidProbabilitySum,
                ),
            ],
        ),
        (
            JevQuestion::Score {
                instructions: json!("Rate the input"),
                criteria: vec![json!("low"), json!("high")],
            },
            json!({"type":"score","score":0.3,"legend":{"0":"low","1":"high"},"probabilities":{"0":0.9,"1":0.1},"confidence":0.8}),
            vec![
                ("score", json!(1.1), JevError::InvalidScoreDistribution),
                (
                    "confidence",
                    json!(-0.1),
                    JevError::InvalidScoreDistribution,
                ),
                (
                    "legend",
                    json!({"0":"high","1":"low"}),
                    JevError::InvalidScoreDistribution,
                ),
                (
                    "probabilities",
                    json!({"0":0.9,"01":0.1}),
                    JevError::InvalidScoreDistribution,
                ),
                (
                    "probabilities",
                    json!({"0":0.2,"1":0.1}),
                    JevError::InvalidProbabilitySum,
                ),
            ],
        ),
    ] {
        request.questions.insert("q".into(), question);
        response
            .answers
            .insert("q".into(), serde_json::from_value(answer.clone()).unwrap());
        let mut limits = ModelCapabilities::jev_native();
        assert_eq!(
            validate_native_jev_response_with_capabilities(&response, &request, &limits),
            Ok(())
        );
        limits.criteria_per_question.value = Some(1);
        assert_eq!(
            validate_native_jev_response_with_capabilities(&response, &request, &limits),
            Err(JevError::QuestionLimitExceeded)
        );
        limits = ModelCapabilities::jev_native();
        for (field, value, expected) in invalid_fields {
            let mut invalid = answer.clone();
            invalid.as_object_mut().unwrap().insert(field.into(), value);
            response
                .answers
                .insert("q".into(), serde_json::from_value(invalid).unwrap());
            assert_eq!(
                validate_native_jev_response_with_capabilities(&response, &request, &limits),
                Err(expected)
            );
        }
        response
            .answers
            .insert("q".into(), JevAnswer::Noul { noul: 0.5 });
        assert_eq!(
            validate_native_jev_response_with_capabilities(&response, &request, &limits),
            Err(JevError::ResponseAnswerTypeMismatch)
        );
    }
}
