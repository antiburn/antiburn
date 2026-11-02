use super::*;
use capabilities::{CapabilityLimit, CapabilitySource, ModelCapabilities};
use requests::{decision_content, generic_question_prompt, tev1_question_prompt};
use serde_json::{Value, json};

#[test]
fn tev1_prompt_matches_the_pinned_ollama_publisher_fixture() {
    // https://github.com/ollama/ollama/blob/v0.40.1/decision/prompts_test.go
    let question = JevQuestion::Noul {
        instructions: json!("Refund requested?"),
        criteria: None,
    };
    assert_eq!(
        tev1_question_prompt(
            "Paid €20 & <tag>. Literal \\u2028, quote \" and colon: comma,",
            &question
        )
        .unwrap(),
        r#"{"state": "Paid €20 & <tag>. Literal \\u2028, quote \" and colon: comma,", "question": "Refund requested?", "options": [{"label": "A", "key": "false", "description": "No"}, {"label": "B", "key": "true", "description": "Yes"}]}"#,
    );
    let state = json!({"frames": ["first", "quote \"\\\n漢字\u{2028}\u{2029}"]});
    let prompt = tev1_question_prompt(&decision_content(&state).unwrap(), &question).unwrap();
    let decoded: Value = serde_json::from_str(&prompt).unwrap();
    assert_eq!(decoded["state"].as_str().unwrap(), state.to_string());
    assert!(prompt.contains(r#"\"frames\""#));
    assert!(prompt.contains('\u{2028}'));
    assert!(!prompt.contains("\\u2028"));
}

#[test]
fn generic_prompt_matches_the_pinned_ollama_publisher_fixture() {
    let questions = BTreeMap::from([(
        "refund".into(),
        JevQuestion::Noul {
            instructions: json!("Refund requested?"),
            criteria: None,
        },
    )]);
    assert_eq!(
        generic_question_prompt(
            "Paid €20 & <tag>. Literal \\u2028, quote \" and colon: comma,",
            &questions,
            "refund"
        )
        .unwrap(),
        concat!(
            r#"{"context": "Paid €20 & \u003ctag\u003e. Literal \\u2028, quote \" and colon: comma,", "schema": [{"name": "refund", "description": "Refund requested?", "choices": [{"code": "A", "value": false, "description": "No"}, {"code": "B", "value": true, "description": "Yes"}]}]}"#,
            "\n\nRequested field: \"refund\""
        )
    );
    let questions = BTreeMap::from([
        (
            "choice".into(),
            JevQuestion::Choice {
                instructions: json!({"q": "<Which?>"}),
                criteria: BTreeMap::from([("a".into(), Value::Null), ("b".into(), json!(""))]),
            },
        ),
        (
            "score".into(),
            JevQuestion::Score {
                instructions: json!("Score it."),
                criteria: vec![json!("low"), json!("high")],
            },
        ),
    ]);
    let prompt = generic_question_prompt("{\"text\":\"漢字🚀\\n\"}", &questions, "choice").unwrap();
    let payload: Value =
        serde_json::from_str(prompt.split("\n\nRequested field:").next().unwrap()).unwrap();
    assert_eq!(payload["schema"][0]["choices"][0]["value"], "a");
    assert_eq!(payload["schema"][0]["choices"][1]["description"], "");
    assert_eq!(payload["schema"][1]["choices"][0]["value"], "0");
    assert!(prompt.contains("\\u003c"));
}

fn choice() -> JevQuestion {
    JevQuestion::Choice {
        instructions: json!("Assess this input"),
        criteria: BTreeMap::from([("yes".into(), json!("Yes")), ("no".into(), json!("No"))]),
    }
}

#[test]
fn response_validation_checks_distribution_and_model() {
    let request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!("sample"),
        questions: BTreeMap::from([("q".into(), choice())]),
    };
    let response = JevResponse {
        model: request.model.clone(),
        answers: BTreeMap::from([(
            "q".into(),
            JevAnswer::Choice {
                choice: "yes".into(),
                probabilities: BTreeMap::from([("yes".into(), 0.1), ("no".into(), 0.9)]),
                confidence: 0.8,
            },
        )]),
        usage: JevUsage {
            input_tokens: 10,
            output_tokens: 2,
        },
    };
    assert_eq!(validate_jev_response(&response, &request), Ok(()));
    let JevAnswer::Choice {
        choice,
        probabilities,
        ..
    } = &response.answers["q"]
    else {
        panic!("test response uses Choice")
    };
    assert_eq!(
        highest_probability_choice(choice, probabilities),
        Some("no")
    );
    assert_eq!(
        highest_probability_choice(
            "yes",
            &BTreeMap::from([("yes".into(), 0.5), ("no".into(), 0.5)])
        ),
        Some("yes")
    );
    let mut invalid = response.clone();
    invalid.model = "jev-latest".into();
    assert_eq!(
        validate_jev_response(&invalid, &request),
        Err(JevError::ResponseModelMismatch)
    );
    let mut invalid = response;
    if let JevAnswer::Choice { probabilities, .. } = invalid.answers.get_mut("q").unwrap() {
        probabilities.insert("no".into(), 1.2);
    }
    assert_eq!(
        validate_jev_response(&invalid, &request),
        Err(JevError::InvalidChoiceDistribution)
    );
}

#[test]
fn request_token_and_body_limits_remain_independent() {
    let request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!({"text": "x".repeat(2_000)}),
        questions: BTreeMap::from([("q".into(), choice())]),
    };
    let mut limits = ModelCapabilities::jev_default();
    limits.total_input_tokens = CapabilityLimit::known(1_000, CapabilitySource::Manual);
    limits.runtime_context_tokens = CapabilityLimit::unknown();
    limits.state_and_longest_question_tokens =
        CapabilityLimit::known(10_000, CapabilitySource::Manual);
    limits.state_and_longest_question_bytes =
        CapabilityLimit::known(10_000, CapabilitySource::Manual);
    limits.request_body_bytes = CapabilityLimit::known(10_000, CapabilitySource::Manual);
    assert!(matches!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::RequestTokenLimitExceeded { .. })
    ));
    limits.total_input_tokens.value = Some(10_000);
    let bytes = validate_jev_request_with_capabilities(&request, &limits).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&request).unwrap().len());
    limits.request_body_bytes.value = Some(bytes as u64 - 1);
    assert!(matches!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::RequestTooLarge { .. })
    ));
}

#[test]
fn request_question_and_criteria_limits_remain_enforced() {
    let mut request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!("sample"),
        questions: BTreeMap::from([("q".into(), choice())]),
    };
    let mut limits = ModelCapabilities::jev_default();
    limits.questions_per_request.value = Some(1);
    limits.criteria_per_question.value = Some(2);
    assert!(validate_jev_request_with_capabilities(&request, &limits).is_ok());
    request.questions.insert("q2".into(), choice());
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::QuestionLimitExceeded)
    );
    request.questions.remove("q2");
    limits.criteria_per_question.value = Some(1);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::QuestionLimitExceeded)
    );
}

#[test]
fn per_type_limits_do_not_use_the_choice_limit_for_score_questions() {
    let mut request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!("sample"),
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Score {
                instructions: json!("Rate the input"),
                criteria: vec![json!("level"); 11],
            },
        )]),
    };
    let limits = ModelCapabilities::jev_default();
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::QuestionLimitExceeded)
    );
    request.questions.insert(
        "q".into(),
        JevQuestion::Choice {
            instructions: json!("Pick one"),
            criteria: (0..11)
                .map(|index| (format!("label{index}"), json!("description")))
                .collect(),
        },
    );
    assert!(validate_jev_request_with_capabilities(&request, &limits).is_ok());
}

#[test]
fn request_validation_checks_every_question_and_criteria_shape() {
    let mut request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!("sample"),
        questions: BTreeMap::from([("q".into(), choice())]),
    };
    for question in [
        JevQuestion::Choice {
            instructions: json!("Pick one"),
            criteria: BTreeMap::from([("label".into(), json!(42))]),
        },
        JevQuestion::Noul {
            instructions: json!("Assess"),
            criteria: Some(json!({"other": "not a valid key"})),
        },
        JevQuestion::Score {
            instructions: json!("Rate"),
            criteria: vec![json!(null)],
        },
        JevQuestion::Noul {
            instructions: json!(false),
            criteria: None,
        },
    ] {
        request.questions.insert("second".into(), question);
        assert_eq!(
            validate_jev_request(&request),
            Err(JevError::InvalidRequestSchema)
        );
    }
}

#[test]
fn response_validation_keeps_answer_identity_and_type_limits() {
    let request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!("sample"),
        questions: BTreeMap::from([("q".into(), choice())]),
    };
    let mut response = JevResponse {
        model: request.model.clone(),
        answers: BTreeMap::new(),
        usage: JevUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
        },
    };
    assert_eq!(
        validate_jev_response(&response, &request),
        Err(JevError::ResponseAnswerCountMismatch)
    );
    response
        .answers
        .insert("other".into(), JevAnswer::Noul { noul: 0.5 });
    assert_eq!(
        validate_jev_response(&response, &request),
        Err(JevError::ResponseAnswerMissing)
    );
    response.answers = BTreeMap::from([("q".into(), JevAnswer::Noul { noul: 0.5 })]);
    assert_eq!(
        validate_jev_response(&response, &request),
        Err(JevError::ResponseAnswerTypeMismatch)
    );
    response.answers.insert(
        "q".into(),
        JevAnswer::Choice {
            choice: "yes".into(),
            probabilities: BTreeMap::from([("yes".into(), 0.9), ("no".into(), 0.1)]),
            confidence: 0.8,
        },
    );
    let mut limits = ModelCapabilities::jev_default();
    limits.runtime_context_tokens.value = Some(100);
    assert_eq!(
        validate_jev_response_with_capabilities(&response, &request, &limits),
        Ok(())
    );
    limits.criteria_per_question.value = Some(1);
    assert_eq!(
        validate_jev_response_with_capabilities(&response, &request, &limits),
        Err(JevError::QuestionLimitExceeded)
    );
    if let JevAnswer::Choice { probabilities, .. } = response.answers.get_mut("q").unwrap() {
        probabilities.insert("yes".into(), 0.2);
    }
    assert_eq!(
        validate_jev_response(&response, &request),
        Err(JevError::InvalidProbabilitySum)
    );
}

#[test]
fn noul_and_score_response_probabilities_stay_bounded() {
    let mut request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!("sample"),
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Noul {
                instructions: json!("Is it valid?"),
                criteria: None,
            },
        )]),
    };
    let mut response = JevResponse {
        model: request.model.clone(),
        answers: BTreeMap::from([("q".into(), JevAnswer::Noul { noul: 0.5 })]),
        usage: JevUsage {
            input_tokens: 1,
            output_tokens: 1,
        },
    };
    assert_eq!(validate_jev_response(&response, &request), Ok(()));
    for invalid in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        response
            .answers
            .insert("q".into(), JevAnswer::Noul { noul: invalid });
        assert_eq!(
            validate_jev_response(&response, &request),
            Err(JevError::InvalidNoulProbability)
        );
    }
    request.questions.insert(
        "q".into(),
        JevQuestion::Score {
            instructions: json!("Rate this input"),
            criteria: vec![json!("low"), json!("high")],
        },
    );
    response.answers.insert(
        "q".into(),
        JevAnswer::Score {
            score: 0.5,
            legend: BTreeMap::from([("0".into(), "low".into()), ("1".into(), "high".into())]),
            probabilities: BTreeMap::from([("0".into(), 0.5), ("1".into(), 0.5)]),
            confidence: 0.8,
        },
    );
    assert_eq!(validate_jev_response(&response, &request), Ok(()));
    if let JevAnswer::Score { confidence, .. } = response.answers.get_mut("q").unwrap() {
        *confidence = f64::NAN;
    }
    assert_eq!(
        validate_jev_response(&response, &request),
        Err(JevError::InvalidScoreDistribution)
    );
}

fn score_request_and_response() -> (JevRequest, JevResponse) {
    let request = JevRequest {
        model: PINNED_MODEL.into(),
        state: json!("synthetic"),
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Score {
                instructions: json!("Rate the input"),
                criteria: vec![json!("low"), json!("high")],
            },
        )]),
    };
    let response = JevResponse {
        model: request.model.clone(),
        answers: BTreeMap::from([(
            "q".into(),
            JevAnswer::Score {
                score: 0.3,
                legend: BTreeMap::from([("0".into(), "low".into()), ("1".into(), "high".into())]),
                probabilities: BTreeMap::from([("0".into(), 0.9), ("1".into(), 0.1)]),
                confidence: 0.8,
            },
        )]),
        usage: JevUsage {
            input_tokens: 12,
            output_tokens: 1,
        },
    };
    (request, response)
}

#[test]
fn score_requires_canonical_keys_and_the_exact_ordered_legend() {
    let (request, response) = score_request_and_response();
    assert_eq!(validate_jev_response(&response, &request), Ok(()));
    for key in ["01", "-1", "2", "+1", "1.0", "wrong"] {
        for replace_legend in [true, false] {
            let mut invalid = response.clone();
            let JevAnswer::Score {
                legend,
                probabilities,
                ..
            } = invalid.answers.get_mut("q").unwrap()
            else {
                unreachable!()
            };
            if replace_legend {
                let text = legend.remove("1").unwrap();
                legend.insert(key.into(), text);
            } else {
                let probability = probabilities.remove("1").unwrap();
                probabilities.insert(key.into(), probability);
            }
            assert_eq!(
                validate_jev_response(&invalid, &request),
                Err(JevError::InvalidScoreDistribution),
                "{key}"
            );
        }
    }
    for legend in [
        BTreeMap::from([("0".into(), "high".into()), ("1".into(), "low".into())]),
        BTreeMap::from([("0".into(), "Low".into()), ("1".into(), "high".into())]),
        BTreeMap::from([("0".into(), "low ".into()), ("1".into(), "high".into())]),
    ] {
        let mut invalid = response.clone();
        let JevAnswer::Score { legend: actual, .. } = invalid.answers.get_mut("q").unwrap() else {
            unreachable!()
        };
        *actual = legend;
        assert_eq!(
            validate_jev_response(&invalid, &request),
            Err(JevError::InvalidScoreDistribution)
        );
    }
}

#[test]
fn score_preserves_fractional_output_without_a_weighted_mean_fallback() {
    let (request, response) = score_request_and_response();
    for score in [0.0, 0.3, 1.0] {
        let mut valid = response.clone();
        let JevAnswer::Score { score: actual, .. } = valid.answers.get_mut("q").unwrap() else {
            unreachable!()
        };
        *actual = score;
        assert_eq!(validate_jev_response(&valid, &request), Ok(()));
        let JevAnswer::Score { score: actual, .. } = &valid.answers["q"] else {
            unreachable!()
        };
        assert_eq!(*actual, score);
    }
    for score in [
        -f64::EPSILON,
        1.0 + f64::EPSILON,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let mut invalid = response.clone();
        let JevAnswer::Score { score: actual, .. } = invalid.answers.get_mut("q").unwrap() else {
            unreachable!()
        };
        *actual = score;
        assert_eq!(
            validate_jev_response(&invalid, &request),
            Err(JevError::InvalidScoreDistribution)
        );
    }
}

#[test]
fn score_sum_uses_criterion_order_for_double_digit_indexes() {
    let weights = [
        0.00461143821674271,
        0.06235434193694818,
        0.02791763000540763,
        0.053222560110663585,
        0.04691279714001585,
        0.03413066297939142,
        0.09928090350243059,
        0.019469692702212067,
        0.04109444961733118,
        0.02017622393103782,
        0.06298293549906385,
        0.02750664269275705,
        0.03542359085937354,
        0.074359485750485,
        0.0319231653896175,
        0.055602563694619585,
        0.09002619292962315,
        0.010052684530593218,
        0.006133410275866783,
        0.02278436393216792,
        0.07617327571275428,
        0.06126736868248141,
        0.023635305768224292,
        0.03295831414019133,
    ];
    let probabilities: BTreeMap<_, _> = weights
        .iter()
        .enumerate()
        .map(|(index, value)| (index.to_string(), *value))
        .collect();
    assert!((weights.iter().sum::<f64>() - 1.0).abs() <= 0.02);
    assert!((probabilities.values().sum::<f64>() - 1.0).abs() > 0.02);
    let criteria: Vec<_> = (0..weights.len())
        .map(|index| json!(format!("Level {index}")))
        .collect();
    let request = JevRequest {
        model: "synthetic-score-profile".into(),
        state: json!("synthetic"),
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Score {
                instructions: json!("Rate the input"),
                criteria,
            },
        )]),
    };
    let response = JevResponse {
        model: request.model.clone(),
        answers: BTreeMap::from([(
            "q".into(),
            JevAnswer::Score {
                score: 5.0,
                legend: (0..weights.len())
                    .map(|index| (index.to_string(), format!("Level {index}")))
                    .collect(),
                probabilities,
                confidence: 0.8,
            },
        )]),
        usage: JevUsage {
            input_tokens: 12,
            output_tokens: 1,
        },
    };
    assert_eq!(validate_jev_response(&response, &request), Ok(()));
}

#[test]
fn score_probabilities_and_confidence_require_finite_unit_values() {
    let (request, response) = score_request_and_response();
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
        for replace_confidence in [true, false] {
            let mut invalid = response.clone();
            let JevAnswer::Score {
                probabilities,
                confidence,
                ..
            } = invalid.answers.get_mut("q").unwrap()
            else {
                unreachable!()
            };
            if replace_confidence {
                *confidence = value;
            } else {
                probabilities.insert("0".into(), value);
            }
            assert_eq!(
                validate_jev_response(&invalid, &request),
                Err(JevError::InvalidScoreDistribution)
            );
        }
    }
}

#[test]
fn choice_and_score_share_the_f64_sum_boundary_without_renormalization() {
    let (score_request, score_response) = score_request_and_response();
    for sum in [
        (1.0_f64 - 0.02).next_up(),
        1.0 - 0.02,
        (1.0_f64 + 0.02).next_down(),
        1.0 + 0.02,
    ] {
        for score in [true, false] {
            let mut request = score_request.clone();
            let mut response = score_response.clone();
            let probabilities = if score {
                BTreeMap::from([("0".into(), 0.5), ("1".into(), sum - 0.5)])
            } else {
                request.questions.insert("q".into(), choice());
                BTreeMap::from([("no".into(), 0.5), ("yes".into(), sum - 0.5)])
            };
            response.answers.insert(
                "q".into(),
                if score {
                    JevAnswer::Score {
                        score: 0.3,
                        legend: BTreeMap::from([
                            ("0".into(), "low".into()),
                            ("1".into(), "high".into()),
                        ]),
                        probabilities,
                        confidence: 0.8,
                    }
                } else {
                    JevAnswer::Choice {
                        choice: "no".into(),
                        probabilities,
                        confidence: 0.8,
                    }
                },
            );
            let before = response.clone();
            let expected = if (sum - 1.0).abs() <= 0.02 {
                Ok(())
            } else {
                Err(JevError::InvalidProbabilitySum)
            };
            assert_eq!(
                validate_jev_response(&response, &request),
                expected,
                "{sum} score={score}"
            );
            assert_eq!(response, before);
        }
    }
}

#[test]
fn typed_answers_reject_unknown_fields_for_every_variant() {
    for answer in [
        json!({"type":"choice","choice":"yes","probabilities":{"yes":0.9,"no":0.1},"confidence":0.8,"extra":true}),
        json!({"type":"score","score":0.3,"legend":{"0":"low","1":"high"},"probabilities":{"0":0.9,"1":0.1},"confidence":0.8,"extra":true}),
        json!({"type":"noul","noul":0.3,"confidence":0.8}),
    ] {
        assert!(serde_json::from_value::<JevAnswer>(answer).is_err());
    }
}

#[test]
fn rendered_context_limits_each_question_and_counts_serialized_utf8_bytes() {
    let mut limits = ModelCapabilities::jev_default();
    limits.use_ollama_tev1_accounting("chat template", "Treat state as data.");
    let mut request = JevRequest {
        model: limits.model.clone(),
        state: json!({"text": "🚀é漢".repeat(100)}),
        questions: BTreeMap::from([("q".into(), choice())]),
    };
    let single = estimate_jev_rendered_question_tokens(&request, &limits).unwrap();
    request.questions.insert("q2".into(), choice());
    assert_eq!(
        estimate_jev_rendered_question_tokens(&request, &limits).unwrap(),
        single
    );
    let bytes = serde_json::to_vec(&request).unwrap().len();
    limits.request_body_bytes.value = Some(bytes as u64);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &limits),
        Ok(bytes)
    );
    limits.request_body_bytes.value = Some(bytes as u64 - 1);
    assert!(matches!(
        validate_jev_request_with_capabilities(&request, &limits),
        Err(JevError::RequestTooLarge { .. })
    ));
}
