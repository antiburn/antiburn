use std::collections::BTreeMap;

use antiburn_checks::bounded_json::{
    BoundedJsonError, JsonLimits, decode_strict_json, measure_json,
};
use antiburn_checks::compiler::{InputLiteral, InputTemplate};
use antiburn_checks::decision::{
    DecisionAttempt, DecisionBlock, NamedQuestion, Question, QuestionKind, QuestionRequirements,
    attempt_id, validate_declarations, validate_requirements,
};
use antiburn_checks::expression_eval::EvaluationBudget;
use antiburn_checks::fit::{FitOmission, FitReport, FitStrategy};
use antiburn_checks::prepared_assessment::{
    MAX_ASSESSMENT_FITTING_WORK_BYTES, MAX_PREPARED_ASSESSMENT_BYTES,
    MAX_PREPARED_ASSESSMENT_NODES, PREPARATION_LIMITS, PreparedAssessment, PreparedBlock,
    decode_prepared_assessment, measure_prepared_assessment,
};
use antiburn_checks::results::{
    ChoiceResult, DecisionResult, EvaluatedAssessment, MAX_RESULT_BYTES, NamedAnswer, NoulResult,
    QuestionResult, RESULT_LIMITS, ScoreResult, decode_decision_result, decode_results,
    measure_decision_results_with_next, measure_result_capacity, measure_results,
};
use serde::Serialize;
use serde_json::{Value, json};

fn block(name: &str) -> DecisionBlock {
    DecisionBlock {
        name: name.into(),
        input: InputTemplate::Object(vec![(
            "work".into(),
            InputTemplate::Literal(InputLiteral::String("Synthetic work".into())),
        )]),
        fit: vec![],
        outputs: vec![
            NamedQuestion {
                name: "verdict".into(),
                question: Question::Choice {
                    question: "Does the work need review?".into(),
                    criteria: vec![
                        ("review".into(), "Review is needed".into()),
                        ("allow".into(), "Review is not needed".into()),
                    ],
                },
            },
            NamedQuestion {
                name: "severity".into(),
                question: Question::Score {
                    question: "How severe is the problem?".into(),
                    criteria: vec!["Low".into(), "Medium".into(), "High".into()],
                },
            },
            NamedQuestion {
                name: "applies".into(),
                question: Question::Noul {
                    question: "Does the policy apply?".into(),
                },
            },
        ],
    }
}

fn requirements(blocks: &[DecisionBlock], details: bool) -> Vec<QuestionRequirements> {
    blocks
        .iter()
        .flat_map(|block| {
            block.outputs.iter().map(move |question| {
                let categorical = question.question.kind() != QuestionKind::Noul;
                QuestionRequirements::new(
                    &block.name,
                    question,
                    details && categorical,
                    details && categorical,
                )
                .unwrap()
            })
        })
        .collect()
}

fn answers(block: &DecisionBlock) -> Vec<NamedAnswer> {
    vec![
        NamedAnswer::new(
            "verdict",
            QuestionResult::Choice(
                ChoiceResult::new(
                    &block.outputs[0].question,
                    "review",
                    Some(BTreeMap::from([
                        ("review".into(), 0.949_999_999_999_999_8),
                        ("allow".into(), 0.05),
                    ])),
                    Some(0.8),
                )
                .unwrap(),
            ),
        )
        .unwrap(),
        NamedAnswer::new(
            "severity",
            QuestionResult::Score(
                ScoreResult::new(
                    &block.outputs[1].question,
                    1.25,
                    Some(vec![0.1, 0.55, 0.35]),
                    Some(0.7),
                )
                .unwrap(),
            ),
        )
        .unwrap(),
        NamedAnswer::new(
            "applies",
            QuestionResult::Noul(NoulResult::new(0.37).unwrap()),
        )
        .unwrap(),
    ]
}

fn complete(blocks: &[DecisionBlock]) -> EvaluatedAssessment {
    let required = requirements(blocks, true);
    let results = blocks
        .iter()
        .map(|block| DecisionResult::new(block, &required, answers(block), 1).unwrap())
        .collect();
    EvaluatedAssessment::new(blocks, &required, results).unwrap()
}

fn prepared_block(name: &str) -> PreparedBlock {
    PreparedBlock {
        decision: name.into(),
        original_input: json!({"work": ["first", "second"]}),
        input: json!({"work": ["second"]}),
        request: json!({"state": {"work": ["second"]}, "questions": {"applies": {"type": "noul", "question": "Does it apply?"}}}),
        report: FitReport {
            omissions: vec![FitOmission {
                path: "work".into(),
                strategy: FitStrategy::Recent,
                original_count: 2,
                retained_count: 1,
            }],
        },
    }
}

fn prepared() -> PreparedAssessment {
    PreparedAssessment::new(
        "sanitized-profile-digest".into(),
        EvaluationBudget::default(),
        1234,
        vec![prepared_block("review")],
    )
    .unwrap()
}

fn count_nodes(value: &Value, depth: usize) -> (usize, usize) {
    let children: Vec<&Value> = match value {
        Value::Array(values) => values.iter().collect(),
        Value::Object(values) => values.values().collect(),
        _ => vec![],
    };
    children
        .into_iter()
        .fold((1, depth), |(nodes, maximum), child| {
            let (child_nodes, child_depth) = count_nodes(child, depth + 1);
            (nodes + child_nodes, maximum.max(child_depth))
        })
}

#[test]
fn mixed_result_shape_preserves_raw_choice_and_score_array() {
    let blocks = vec![block("review")];
    let result = complete(&blocks);
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(
        value,
        json!({"version": 1, "attempts": 1, "decisions": [{
            "decision": "review", "attempts": 1, "answers": [
                {"name": "verdict", "result": {"type": "choice", "output": "review", "probabilities": {"review": 0.949_999_999_999_999_8, "allow": 0.05}, "confidence": 0.8}},
                {"name": "severity", "result": {"type": "score", "output": 1.25, "probabilities": [0.1, 0.55, 0.35], "confidence": 0.7}},
                {"name": "applies", "result": {"type": "noul", "output": 0.37}}
            ]
        }]})
    );
    let QuestionResult::Choice(choice) = result.decisions()[0].answers()[0].result() else {
        panic!("expected Choice")
    };
    assert_eq!(choice.output(), "review");
    assert!(choice.probabilities().unwrap()["review"] < 0.95);
    assert_eq!(choice.confidence(), Some(0.8));
    let QuestionResult::Score(score) = result.decisions()[0].answers()[1].result() else {
        panic!("expected Score")
    };
    assert_eq!(score.output(), 1.25);
    assert_eq!(score.probabilities(), Some([0.1, 0.55, 0.35].as_slice()));
    assert_eq!(score.confidence(), Some(0.7));
    let decoded = decode_results(&serde_json::to_vec(&result).unwrap())
        .unwrap()
        .validate_for_declarations(&blocks, &requirements(&blocks, true))
        .unwrap();
    assert_eq!(decoded, result);
}

#[test]
fn constructors_restore_authored_block_and_question_order() {
    let blocks = vec![block("second_name"), block("first_name")];
    let required = requirements(&blocks, true);
    let decisions = blocks
        .iter()
        .rev()
        .map(|block| {
            let mut reversed = answers(block);
            reversed.reverse();
            DecisionResult::new(block, &required, reversed, 2).unwrap()
        })
        .collect();
    let result = EvaluatedAssessment::new(&blocks, &required, decisions).unwrap();
    assert_eq!(
        result
            .decisions()
            .iter()
            .map(DecisionResult::decision)
            .collect::<Vec<_>>(),
        ["second_name", "first_name"]
    );
    assert_eq!(
        result.decisions()[0]
            .answers()
            .iter()
            .map(NamedAnswer::name)
            .collect::<Vec<_>>(),
        ["verdict", "severity", "applies"]
    );
    assert_eq!(result.attempts(), 4);
    result
        .validate_for_declarations(&blocks, &required)
        .unwrap();
}

#[test]
fn optional_details_are_omitted_but_required_details_cannot_disappear() {
    let blocks = vec![block("review")];
    let mut partial = answers(&blocks[0]);
    partial[0] = NamedAnswer::new(
        "verdict",
        QuestionResult::Choice(
            ChoiceResult::new(&blocks[0].outputs[0].question, "allow", None, None).unwrap(),
        ),
    )
    .unwrap();
    partial[1] = NamedAnswer::new(
        "severity",
        QuestionResult::Score(
            ScoreResult::new(&blocks[0].outputs[1].question, 0.1, None, None).unwrap(),
        ),
    )
    .unwrap();
    let result = DecisionResult::new(
        &blocks[0],
        &requirements(&blocks, false),
        partial.clone(),
        1,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&result).unwrap()["answers"][0]["result"],
        json!({"type": "choice", "output": "allow"})
    );
    assert!(DecisionResult::new(&blocks[0], &requirements(&blocks, true), partial, 1).is_err());
    let mut only_confidence = requirements(&blocks, false);
    only_confidence[0].confidence = true;
    assert!(
        result
            .validate_for_declaration(&blocks[0], &only_confidence)
            .is_err()
    );
}

#[test]
fn result_sets_reject_missing_extra_duplicate_and_wrong_types() {
    let blocks = vec![block("review")];
    let required = requirements(&blocks, true);
    let valid = answers(&blocks[0]);
    assert!(DecisionResult::new(&blocks[0], &required, valid[..2].to_vec(), 1).is_err());
    let mut extra = valid.clone();
    extra.push(valid[0].clone());
    assert!(DecisionResult::new(&blocks[0], &required, extra, 1).is_err());
    let mut duplicate = valid.clone();
    duplicate[2] = valid[0].clone();
    assert!(DecisionResult::new(&blocks[0], &required, duplicate, 1).is_err());
    let mut unknown = valid.clone();
    unknown[2] = NamedAnswer::new(
        "unknown",
        QuestionResult::Noul(NoulResult::new(0.4).unwrap()),
    )
    .unwrap();
    assert!(DecisionResult::new(&blocks[0], &required, unknown, 1).is_err());
    let mut wrong_type = valid.clone();
    wrong_type[0] = NamedAnswer::new(
        "verdict",
        QuestionResult::Noul(NoulResult::new(0.4).unwrap()),
    )
    .unwrap();
    assert!(DecisionResult::new(&blocks[0], &required, wrong_type, 1).is_err());
    for attempts in [0, 3, u8::MAX] {
        assert!(DecisionResult::new(&blocks[0], &required, valid.clone(), attempts).is_err());
    }
    let decision = DecisionResult::new(&blocks[0], &required, valid, 1).unwrap();
    assert!(EvaluatedAssessment::new(&blocks, &required, vec![]).is_err());
    assert!(
        EvaluatedAssessment::new(&blocks, &required, vec![decision.clone(), decision.clone()])
            .is_err()
    );
    let other = vec![block("other")];
    assert!(
        EvaluatedAssessment::new(&other, &requirements(&other, true), vec![decision.clone()])
            .is_err()
    );
    let two = vec![block("review"), block("other")];
    assert!(
        EvaluatedAssessment::new(
            &two,
            &requirements(&two, true),
            vec![decision.clone(), decision]
        )
        .is_err()
    );
}

#[test]
fn finite_values_ranges_exact_probability_labels_and_lengths_are_required() {
    let declaration = block("review");
    let choice = &declaration.outputs[0].question;
    let score = &declaration.outputs[1].question;
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 1.01] {
        assert!(NoulResult::new(invalid).is_err());
        assert!(ChoiceResult::new(choice, "allow", None, Some(invalid)).is_err());
        assert!(ScoreResult::new(score, 0.5, None, Some(invalid)).is_err());
        assert!(
            ChoiceResult::new(
                choice,
                "allow",
                Some(BTreeMap::from([
                    ("allow".into(), invalid),
                    ("review".into(), 0.5)
                ])),
                None
            )
            .is_err()
        );
        assert!(ScoreResult::new(score, 0.5, Some(vec![invalid, 0.5, 0.0]), None).is_err());
    }
    for invalid in [f64::NAN, f64::INFINITY, -0.01, 2.01] {
        assert!(ScoreResult::new(score, invalid, None, None).is_err());
    }
    for valid in [0.0, 1.25, 2.0] {
        assert!(ScoreResult::new(score, valid, None, None).is_ok());
    }
    for valid in [0.0, 1.0] {
        assert!(NoulResult::new(valid).is_ok());
    }
    assert!(ChoiceResult::new(choice, "unknown", None, None).is_err());
    assert!(
        ChoiceResult::new(
            choice,
            "allow",
            Some(BTreeMap::from([
                ("allow".into(), 0.5),
                ("unknown".into(), 0.5)
            ])),
            None
        )
        .is_err()
    );
    assert!(ScoreResult::new(score, 0.5, Some(vec![0.5, 0.5]), None).is_err());
    assert!(ChoiceResult::new(score, "allow", None, None).is_err());
    assert!(ScoreResult::new(choice, 0.5, None, None).is_err());
}

#[test]
fn probability_sum_uses_the_exact_f64_tolerance_without_normalization() {
    let declaration = block("review");
    for sum in [
        0.979_999_999_999_999_9,
        0.98,
        0.980_000_000_000_000_1,
        1.019_999_999_999_999_8,
        1.02,
        1.020_000_000_000_000_2,
    ] {
        let probabilities = BTreeMap::from([("allow".into(), 0.5), ("review".into(), sum - 0.5)]);
        let actual_sum = probabilities.values().sum::<f64>();
        let valid = (actual_sum - 1.0).abs() <= 0.02;
        let result = ChoiceResult::new(
            &declaration.outputs[0].question,
            "review",
            Some(probabilities.clone()),
            None,
        );
        assert_eq!(result.is_ok(), valid, "sum={actual_sum:?}");
        if let Ok(result) = result {
            assert_eq!(result.probabilities(), Some(&probabilities));
        }
        let distribution = vec![0.5, sum - 0.5, 0.0];
        let actual_sum = distribution.iter().sum::<f64>();
        let result = ScoreResult::new(
            &declaration.outputs[1].question,
            1.0,
            Some(distribution.clone()),
            None,
        );
        assert_eq!(result.is_ok(), (actual_sum - 1.0).abs() <= 0.02);
        if let Ok(result) = result {
            assert_eq!(result.probabilities(), Some(distribution.as_slice()));
        }
    }
}

#[test]
fn requirements_match_the_exact_declarations_without_provider_caps() {
    let blocks = vec![block("review")];
    let required = requirements(&blocks, true);
    validate_requirements(&blocks, &required).unwrap();
    assert!(validate_requirements(&blocks, &required[..2]).is_err());
    for index in 0..3 {
        let mut invalid = required.clone();
        invalid[index].criteria_count += 1;
        assert!(validate_requirements(&blocks, &invalid).is_err());
    }
    let mut invalid = required.clone();
    invalid[1] = invalid[0].clone();
    assert!(validate_requirements(&blocks, &invalid).is_err());
    let mut invalid = required.clone();
    invalid[1].kind = QuestionKind::Choice;
    assert!(validate_requirements(&blocks, &invalid).is_err());
    let mut invalid = required.clone();
    invalid[1].decision = "other".into();
    assert!(validate_requirements(&blocks, &invalid).is_err());
    for (probabilities, confidence) in [(true, false), (false, true)] {
        assert!(
            QuestionRequirements::new("review", &blocks[0].outputs[2], probabilities, confidence)
                .is_err()
        );
    }
    let mut many = blocks[0].clone();
    many.outputs[1].question = Question::Score {
        question: "Which level?".into(),
        criteria: vec!["Level".into(); 27],
    };
    many.validate().unwrap();
}

#[test]
fn declaration_validation_limits_questions_not_independent_blocks() {
    let mut declaration = block("review");
    declaration.outputs = (0..8)
        .map(|index| NamedQuestion {
            name: format!("q{index}"),
            question: Question::Noul {
                question: "Does it apply?".into(),
            },
        })
        .collect();
    declaration.validate().unwrap();
    declaration.outputs.push(NamedQuestion {
        name: "q8".into(),
        question: Question::Noul {
            question: "Does it apply?".into(),
        },
    });
    assert!(declaration.validate().is_err());
    let many = (0..100)
        .map(|index| block(&format!("d{index}")))
        .collect::<Vec<_>>();
    validate_declarations(&many).unwrap();
    assert!(validate_declarations(&[block("review"), block("review")]).is_err());
    for text in ["", "  ", "Question ${{ true }}"] {
        assert!(
            Question::Noul {
                question: text.into()
            }
            .validate()
            .is_err()
        );
    }
    let mut duplicate = block("review");
    duplicate.outputs[1].name = duplicate.outputs[0].name.clone();
    assert!(duplicate.validate().is_err());
}

#[test]
fn local_results_have_no_provider_attempts_or_decisions() {
    let local = EvaluatedAssessment::local();
    assert_eq!(local.version(), 1);
    assert_eq!(local.attempts(), 0);
    assert!(local.decisions().is_empty());
    local.validate_for_declarations(&[], &[]).unwrap();
    assert_eq!(
        serde_json::to_value(&local).unwrap(),
        json!({"version": 1, "decisions": [], "attempts": 0})
    );
    assert!(decode_results(br#"{"version":1,"decisions":[],"attempts":1}"#).is_err());
}

#[test]
fn strict_result_decode_rejects_duplicates_before_value_collapse() {
    for raw in [
        r#"{"version":1,"version":1,"decisions":[],"attempts":0}"#,
        r#"{"version":1,"decisions":[],"attempts":0,"attempts":0}"#,
        r#"{"version":1,"decisions":[{"decision":"review","decision":"review","answers":[],"attempts":1}],"attempts":1}"#,
        r#"{"version":1,"decisions":[{"decision":"review","answers":[{"name":"verdict","result":{"type":"choice","output":"allow","probabilities":{"allow":0.5,"allow":0.5,"review":0.5}}}],"attempts":1}],"attempts":1}"#,
        r#"{"version":1,"decisions":[{"decision":"review","answers":[{"name":"verdict","result":{"type":"choice","output":"allow","output":"review"}}],"attempts":1}],"attempts":1}"#,
    ] {
        assert!(decode_results(raw.as_bytes()).is_err(), "{raw}");
    }
    let blocks = vec![block("review")];
    let mut saved = serde_json::to_value(complete(&blocks)).unwrap();
    let duplicate = saved["decisions"][0].clone();
    saved["decisions"].as_array_mut().unwrap().push(duplicate);
    saved["attempts"] = json!(2);
    assert!(decode_results(&serde_json::to_vec(&saved).unwrap()).is_err());
    let mut saved = serde_json::to_value(complete(&blocks)).unwrap();
    saved["decisions"][0]["answers"][1] = saved["decisions"][0]["answers"][0].clone();
    assert!(decode_results(&serde_json::to_vec(&saved).unwrap()).is_err());
}

#[test]
fn strict_result_decode_rejects_unknown_fields_nulls_and_wrong_detail_shapes() {
    let blocks = vec![block("review")];
    let valid = serde_json::to_value(complete(&blocks)).unwrap();
    for path in ["root", "decision", "answer", "choice", "score", "noul"] {
        let mut saved = valid.clone();
        let target = match path {
            "root" => &mut saved,
            "decision" => &mut saved["decisions"][0],
            "answer" => &mut saved["decisions"][0]["answers"][0],
            "choice" => &mut saved["decisions"][0]["answers"][0]["result"],
            "score" => &mut saved["decisions"][0]["answers"][1]["result"],
            _ => &mut saved["decisions"][0]["answers"][2]["result"],
        };
        target["unexpected"] = json!(1);
        assert!(
            decode_results(&serde_json::to_vec(&saved).unwrap()).is_err(),
            "{path}"
        );
    }
    for question in [0, 1] {
        for detail in ["probabilities", "confidence"] {
            let mut saved = valid.clone();
            saved["decisions"][0]["answers"][question]["result"][detail] = Value::Null;
            assert!(decode_results(&serde_json::to_vec(&saved).unwrap()).is_err());
        }
    }
    let mut saved = valid.clone();
    saved["decisions"][0]["answers"][1]["result"]["probabilities"] =
        json!({"0": 0.1, "1": 0.55, "2": 0.35});
    assert!(decode_results(&serde_json::to_vec(&saved).unwrap()).is_err());
    let mut saved = valid;
    saved["decisions"][0]["answers"][2]["result"]["confidence"] = json!(0.7);
    assert!(decode_results(&serde_json::to_vec(&saved).unwrap()).is_err());
}

#[test]
fn decoded_results_need_declaration_binding_and_totals_versions_are_exact() {
    let blocks = vec![block("review")];
    let valid = serde_json::to_value(complete(&blocks)).unwrap();
    for version in [json!(0), json!(2), json!("1"), json!(1.0)] {
        let mut saved = valid.clone();
        saved["version"] = version;
        assert!(decode_results(&serde_json::to_vec(&saved).unwrap()).is_err());
    }
    let mut saved = valid.clone();
    saved["attempts"] = json!(2);
    assert!(decode_results(&serde_json::to_vec(&saved).unwrap()).is_err());
    let mut saved = valid;
    saved["decisions"][0]["decision"] = json!("other");
    let decoded = decode_results(&serde_json::to_vec(&saved).unwrap()).unwrap();
    assert!(
        decoded
            .validate_for_declarations(&blocks, &requirements(&blocks, true))
            .is_err()
    );
    let result = complete(&blocks).decisions()[0].clone();
    assert_eq!(
        decode_decision_result(&serde_json::to_vec(&result).unwrap())
            .unwrap()
            .validate_for_declaration(&blocks[0], &requirements(&blocks, true))
            .unwrap(),
        result
    );
}

#[test]
fn result_raw_byte_limit_applies_before_parsing() {
    let mut raw = br#"{"version":1,"decisions":[],"attempts":0}"#.to_vec();
    raw.resize(MAX_RESULT_BYTES as usize, b' ');
    decode_results(&raw)
        .unwrap()
        .validate_for_declarations(&[], &[])
        .unwrap();
    raw.push(b' ');
    assert!(decode_results(&raw).is_err());
    let bytes = vec![b'!'; MAX_RESULT_BYTES as usize + 1];
    assert!(matches!(
        decode_strict_json(&bytes, RESULT_LIMITS),
        Err(BoundedJsonError::Bytes { .. })
    ));
}

#[test]
fn strict_visitor_rejects_crossing_nodes_depth_and_duplicates_before_values() {
    let limits = JsonLimits {
        max_bytes: 100,
        max_nodes: 2,
        max_depth: 2,
    };
    assert_eq!(
        decode_strict_json(b"[null]", limits).unwrap(),
        json!([null])
    );
    assert!(matches!(
        decode_strict_json(b"[null, invalid]", limits),
        Err(BoundedJsonError::Nodes {
            observed: 3,
            maximum: 2
        })
    ));
    assert!(matches!(
        decode_strict_json(
            b"[[invalid]]",
            JsonLimits {
                max_nodes: 10,
                ..limits
            }
        ),
        Err(BoundedJsonError::Depth {
            observed: 3,
            maximum: 2
        })
    ));
    assert!(
        matches!(decode_strict_json(br#"{"a":null,"a":invalid}"#, limits), Err(BoundedJsonError::DuplicateKey(key)) if key == "a")
    );
    assert!(decode_strict_json(b"null true", limits).is_err());
    assert!(decode_strict_json(b"1e9999", limits).is_err());
}

#[test]
fn collect_str_checks_the_node_limit_without_formatting() {
    struct PanicDisplay;

    impl std::fmt::Display for PanicDisplay {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("The node pass must not format Display output.");
        }
    }

    impl Serialize for PanicDisplay {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }

    let limits = JsonLimits {
        max_bytes: 64,
        max_nodes: 0,
        max_depth: 1,
    };
    assert_eq!(
        measure_json(&PanicDisplay, limits),
        Err(BoundedJsonError::Nodes {
            observed: 1,
            maximum: 0,
        })
    );
}

#[test]
fn collect_str_streams_only_until_the_byte_limit() {
    struct StreamingDisplay {
        writes: std::cell::Cell<usize>,
    }

    impl std::fmt::Display for StreamingDisplay {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for _ in 0..1_000_000 {
                self.writes.set(self.writes.get() + 1);
                formatter.write_str("0123456789abcdef")?;
            }
            Ok(())
        }
    }

    impl Serialize for StreamingDisplay {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }

    let value = StreamingDisplay {
        writes: std::cell::Cell::new(0),
    };
    let limits = JsonLimits {
        max_bytes: 64,
        max_nodes: 1,
        max_depth: 1,
    };
    assert!(matches!(
        measure_json(&value, limits),
        Err(BoundedJsonError::Bytes { maximum: 64, .. })
    ));
    assert!((1..=5).contains(&value.writes.get()));
}

#[test]
fn measurements_match_real_serialized_bytes_nodes_and_depth() {
    #[derive(Serialize)]
    enum Shapes {
        Unit,
        Newtype(Value),
        Tuple(bool, Value),
        Record { text: String },
    }
    #[derive(Serialize)]
    struct Wrapper {
        shapes: Vec<Shapes>,
        optional: Option<u8>,
        bytes: Vec<u8>,
    }
    let value = Wrapper {
        shapes: vec![
            Shapes::Unit,
            Shapes::Newtype(json!([1, {"text": "escaped\n\""}])),
            Shapes::Tuple(true, Value::Null),
            Shapes::Record {
                text: "Unicode 文".into(),
            },
        ],
        optional: None,
        bytes: vec![1, 2],
    };
    let actual = serde_json::to_value(&value).unwrap();
    let (nodes, depth) = count_nodes(&actual, 1);
    let measured = measure_json(&value, PREPARATION_LIMITS).unwrap();
    assert_eq!(
        measured.bytes,
        serde_json::to_vec(&value).unwrap().len() as u64
    );
    assert_eq!((measured.nodes, measured.depth), (nodes, depth));
    assert!(matches!(
        measure_json(
            &value,
            JsonLimits {
                max_nodes: nodes - 1,
                ..PREPARATION_LIMITS
            }
        ),
        Err(BoundedJsonError::Nodes { .. })
    ));
    assert!(matches!(
        measure_json(
            &value,
            JsonLimits {
                max_depth: depth - 1,
                ..PREPARATION_LIMITS
            }
        ),
        Err(BoundedJsonError::Depth { .. })
    ));
    assert!(matches!(
        measure_json(
            &value,
            JsonLimits {
                max_bytes: measured.bytes - 1,
                ..PREPARATION_LIMITS
            }
        ),
        Err(BoundedJsonError::Bytes { .. })
    ));
    assert!(measure_json(&f64::NAN, PREPARATION_LIMITS).is_err());
}

#[test]
fn result_capacity_reserves_all_labels_names_optional_details_and_wrapper() {
    let blocks = vec![block("review"), block("another_review")];
    let required = requirements(&blocks, false);
    let reservation = measure_result_capacity(&blocks, &required).unwrap();
    let actual = measure_results(&complete(&blocks)).unwrap();
    assert!(reservation.bytes > actual.bytes);
    assert_eq!(reservation.nodes, actual.nodes);
    assert_eq!(reservation.depth, actual.depth);
    let mut long = blocks.clone();
    let Question::Choice { criteria, .. } = &mut long[0].outputs[0].question else {
        unreachable!()
    };
    criteria[1].0 = format!("label{}", "x".repeat(MAX_RESULT_BYTES as usize / 2));
    assert!(measure_result_capacity(&long, &requirements(&long, false)).is_err());
    let one = complete(&blocks[..1]);
    let second = complete(&blocks[1..]);
    assert_eq!(
        measure_decision_results_with_next(one.decisions(), &second.decisions()[0]).unwrap(),
        actual
    );
    assert!(measure_decision_results_with_next(one.decisions(), &one.decisions()[0]).is_err());
}

#[test]
fn prepared_measurement_counts_the_full_ready_wrapper_and_all_payloads() {
    #[derive(Serialize)]
    #[serde(tag = "status", rename_all = "snake_case")]
    enum Record<'a> {
        Ready(&'a PreparedAssessment),
    }
    let prepared = prepared();
    let record = Record::Ready(&prepared);
    let value = serde_json::to_value(&record).unwrap();
    let (nodes, depth) = count_nodes(&value, 1);
    let measurement = measure_prepared_assessment(&prepared).unwrap();
    assert_eq!(
        measurement.bytes,
        serde_json::to_vec(&record).unwrap().len() as u64
    );
    assert_eq!((measurement.nodes, measurement.depth), (nodes, depth));
    assert_eq!(
        measurement.bytes,
        serde_json::to_vec(&prepared).unwrap().len() as u64 + 17
    );
    let original = serde_json::to_vec(&prepared).unwrap();
    assert_eq!(decode_prepared_assessment(&original).unwrap(), prepared);
    assert_eq!(prepared.fitting_work_bytes, 1234);
}

#[test]
fn prepared_append_checks_before_retaining_and_preserves_the_original() {
    let mut prepared = prepared();
    let mut second = prepared_block("other");
    second.original_input = json!({"large": "x".repeat(MAX_PREPARED_ASSESSMENT_BYTES as usize)});
    assert!(prepared.try_push(second).is_err());
    assert_eq!(prepared.decisions.len(), 1);
    assert_eq!(
        prepared.decisions[0].original_input,
        json!({"work": ["first", "second"]})
    );
    assert!(prepared.try_push(prepared_block("review")).is_err());
    let second = prepared_block("other");
    let expected = prepared.check_append(&second).unwrap();
    prepared.try_push(second).unwrap();
    assert_eq!(measure_prepared_assessment(&prepared).unwrap(), expected);
    prepared
        .validate_for_declarations(&[block("review"), block("other")])
        .unwrap();
    assert!(
        prepared
            .validate_for_declarations(&[block("other"), block("review")])
            .is_err()
    );
}

#[test]
fn prepared_nodes_include_header_original_input_request_and_report() {
    let mut prepared = prepared();
    prepared.decisions[0].request = json!([]);
    let base = measure_prepared_assessment(&prepared).unwrap();
    prepared.decisions[0].request = json!(vec![
        Value::Null;
        MAX_PREPARED_ASSESSMENT_NODES - base.nodes
    ]);
    assert_eq!(
        measure_prepared_assessment(&prepared).unwrap().nodes,
        MAX_PREPARED_ASSESSMENT_NODES
    );
    decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).unwrap();
    prepared.decisions[0]
        .request
        .as_array_mut()
        .unwrap()
        .push(Value::Null);
    assert!(measure_prepared_assessment(&prepared).is_err());
    assert!(decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).is_err());
}

#[test]
fn prepared_full_wrapper_depth_and_bytes_have_exact_boundaries() {
    let mut prepared = prepared();
    let mut nested = Value::Null;
    // The request starts at depth four in the complete Ready record.
    for _ in 0..60 {
        nested = json!([nested]);
    }
    prepared.decisions[0].request = nested;
    assert_eq!(measure_prepared_assessment(&prepared).unwrap().depth, 64);
    decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).unwrap();
    prepared.decisions[0].request = json!([prepared.decisions[0].request.take()]);
    assert!(decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).is_err());
    let mut prepared = self::prepared();
    prepared.decisions[0].request = json!("");
    let base = measure_prepared_assessment(&prepared).unwrap().bytes;
    prepared.decisions[0].request =
        json!("x".repeat((MAX_PREPARED_ASSESSMENT_BYTES - base) as usize));
    assert_eq!(
        measure_prepared_assessment(&prepared).unwrap().bytes,
        MAX_PREPARED_ASSESSMENT_BYTES
    );
    decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).unwrap();
    prepared.profile.push('x');
    assert!(decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).is_err());
}

#[test]
fn prepared_decode_rejects_unknown_duplicate_missing_version_and_budget_state() {
    let valid = serde_json::to_value(prepared()).unwrap();
    for path in ["root", "block", "report", "omission", "budget"] {
        let mut saved = valid.clone();
        let target = match path {
            "root" => &mut saved,
            "block" => &mut saved["decisions"][0],
            "report" => &mut saved["decisions"][0]["report"],
            "omission" => &mut saved["decisions"][0]["report"]["omissions"][0],
            _ => &mut saved["evaluation_budget"],
        };
        target["unknown"] = json!(true);
        assert!(
            decode_prepared_assessment(&serde_json::to_vec(&saved).unwrap()).is_err(),
            "{path}"
        );
    }
    for field in [
        "version",
        "profile",
        "evaluation_budget",
        "fitting_work_bytes",
        "decisions",
    ] {
        let mut saved = valid.clone();
        saved.as_object_mut().unwrap().remove(field);
        assert!(decode_prepared_assessment(&serde_json::to_vec(&saved).unwrap()).is_err());
    }
    for version in [json!(0), json!(2), json!(1.0), Value::Null] {
        let mut saved = valid.clone();
        saved["version"] = version;
        assert!(decode_prepared_assessment(&serde_json::to_vec(&saved).unwrap()).is_err());
    }
    let mut saved = valid.clone();
    saved["evaluation_budget"]["exhausted"] = json!(true);
    assert!(decode_prepared_assessment(&serde_json::to_vec(&saved).unwrap()).is_err());
    let mut saved = valid;
    saved["decisions"][0]["request"] = json!({"duplicate": 1});
    let raw = serde_json::to_string(&saved)
        .unwrap()
        .replace("\"duplicate\":1", "\"duplicate\":1,\"duplicate\":2");
    assert!(decode_prepared_assessment(raw.as_bytes()).is_err());
    assert!(serde_json::from_str::<PreparedAssessment>(&raw).is_err());
}

#[test]
fn fitting_work_is_an_immutable_bounded_fact_not_a_new_restart_allowance() {
    let mut prepared = prepared();
    prepared.fitting_work_bytes = MAX_ASSESSMENT_FITTING_WORK_BYTES;
    prepared.validate().unwrap();
    let restored = decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).unwrap();
    assert_eq!(
        restored.fitting_work_bytes,
        MAX_ASSESSMENT_FITTING_WORK_BYTES
    );
    restored.validate().unwrap();
    prepared.fitting_work_bytes += 1;
    assert!(prepared.validate().is_err());
    assert!(decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).is_err());
}

#[test]
fn attempt_ids_bind_candidate_decision_and_number_without_provider_data() {
    let first = DecisionAttempt::new("review", 1).unwrap();
    let second = DecisionAttempt::new("review", 2).unwrap();
    assert_eq!(
        attempt_id(&[0; 32], &first),
        format!("burn-check-decision-v1:32:{}:6:review:1:1", "00".repeat(32))
    );
    assert_ne!(attempt_id(&[0; 32], &first), attempt_id(&[0; 32], &second));
    assert_ne!(attempt_id(&[0; 32], &first), attempt_id(&[1; 32], &first));
    assert_ne!(
        attempt_id(&[0; 32], &first),
        attempt_id(&[0; 32], &DecisionAttempt::new("review1", 1).unwrap())
    );
    for number in [0, 3, u8::MAX] {
        assert!(DecisionAttempt::new("review", number).is_err());
    }
    assert!(DecisionAttempt::new("bad:name", 1).is_err());
    assert!(
        serde_json::from_str::<DecisionAttempt>(r#"{"decision":"review","number":3}"#).is_err()
    );
}
