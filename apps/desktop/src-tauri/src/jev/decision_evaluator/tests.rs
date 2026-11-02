use super::*;
use crate::jev::config::{
    ContextLimitOverride, SystemOneConnection, SystemOneEndpoint, SystemOneProvider,
    SystemOneResponseMode,
};
use antiburn_checks::domain::{
    ActivationId, ActivationScope, BranchId, FrozenContext, Generation, ObservationPosition,
};
use antiburn_checks::prepared_assessment::decode_prepared_assessment;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "native_tests.rs"]
mod native_tests;

const SOURCE: &str = "name: Selected provider\non:\n  tool_result:\n  window: {size: 3, min: 1}\ndecisions:\n  review:\n    input: {rows: '${{ window.messages }}'}\n    fit: {rows: recent}\n    output:\n      verdict:\n        type: choice\n        question: Does the evidence show a concern?\n        criteria: {concern: Concern., clear: Clear.}\nactions:\n  - type: status\n    if: decisions.review.details.verdict.probabilities.concern > 0.9\n    input: {result: failure}\n";

fn activation() -> EffectiveActivation {
    activation_source(SOURCE)
}
fn activation_source(source: &str) -> EffectiveActivation {
    EffectiveActivation {
        id: ActivationId {
            scope: ActivationScope::default(),
            definition_id: "selected-provider".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: antiburn_checks::discovery::DefinitionScope::Global,
        source_paths: Vec::new(),
        project_root: None,
        compiled: Arc::new(antiburn_checks::compiler::compile(source).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: antiburn_checks::source::PublicationCursor(0),
    }
}
fn mixed_activation() -> EffectiveActivation {
    activation_source(&SOURCE.replace("actions:", "      severity:\n        type: score\n        question: How severe is the concern?\n        criteria: [low, high]\n      applies:\n        type: noul\n        question: Does this apply?\nactions:"))
}
fn candidate(activation: &EffectiveActivation) -> FrozenCandidate {
    FrozenCandidate {
        activation: activation.id.clone(),
        branch: BranchId("main".into()),
        trigger_position: ObservationPosition(3),
        messages: Vec::new(),
        context: FrozenContext {
            value: json!({}),
            dependencies: Vec::new(),
        },
        dependencies: Vec::new(),
        fingerprint: [7; 32],
    }
}
fn connection(endpoint: &str) -> SystemOneConnection {
    SystemOneConnection {
        provider: SystemOneProvider::Custom,
        endpoint: SystemOneEndpoint::ExactUrl(endpoint.into()),
        model: "synthetic-model".into(),
        model_revision: Some("synthetic-digest".into()),
        response_mode: SystemOneResponseMode::Direct,
        credential: None,
        revision: 7,
        context_override: Some(ContextLimitOverride {
            total_input_tokens: Some(64 * 1024),
            state_and_longest_question_tokens: Some(32 * 1024),
            runtime_context_tokens: None,
        }),
    }
}
fn capabilities(connection: &SystemOneConnection) -> ModelCapabilities {
    connection.apply_capability_overrides(ModelCapabilities {
        model: connection.model.clone(),
        model_revision: connection.model_revision.clone(),
        ..ModelCapabilities::jev_default()
    })
}
fn evaluator(connection: SystemOneConnection) -> TypeSafeDecisionEvaluator {
    connection.validate().expect("valid synthetic connection");
    let caps = capabilities(&connection);
    TypeSafeDecisionEvaluator::from_selected_snapshot(
        connection,
        Some("synthetic-secret".into()),
        caps,
    )
    .unwrap()
}
fn prepare(
    evaluator: &TypeSafeDecisionEvaluator,
    activation: &EffectiveActivation,
    input: Value,
) -> Result<PreparedAssessment, PreparationFailure> {
    evaluator.prepare(
        activation,
        activation
            .compiled
            .decisions
            .iter()
            .map(|block| DecisionInput {
                decision: block.name.clone(),
                input: input.clone(),
            })
            .collect(),
        EvaluationBudget::default(),
    )
}
fn validate(
    evaluator: &TypeSafeDecisionEvaluator,
    activation: &EffectiveActivation,
    prepared: &PreparedAssessment,
) -> Result<(), PreparationFailure> {
    evaluator.validate_prepared(activation, prepared, PreparationValidationPurpose::Dispatch)
}
fn attempt(decision: &str, number: u8) -> DecisionAttempt {
    DecisionAttempt::new(decision, number).unwrap()
}
fn valid_response(mixed: bool) -> Value {
    let mut response = json!({"model":"synthetic-model","answers":{"verdict":{"type":"choice","choice":"clear","probabilities":{"clear":0.2,"concern":0.8},"confidence":0.7}},"usage":{"input_tokens":12,"output_tokens":3}});
    if mixed {
        response["answers"]["severity"] = json!({"type":"score","score":0.3,"probabilities":{"0":0.9,"1":0.1},"legend":{"0":"low","1":"high"},"confidence":0.6});
        response["answers"]["applies"] = json!({"type":"noul","noul":0.4});
    }
    response
}
fn accept(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((socket, _)) => {
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                return socket;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("mock accept failed: {error}"),
        }
    }
}
fn read_request(socket: &mut TcpStream) -> Vec<u8> {
    read_request_with_headers(socket).1
}
fn read_request_with_headers(socket: &mut TcpStream) -> (String, Vec<u8>) {
    let mut received = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).unwrap();
        assert!(count > 0);
        received.extend_from_slice(&chunk[..count]);
        assert!(received.len() < MAX_RESIDENT_REQUEST_BYTES);
        if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&received[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if received.len() >= end + 4 + length {
                return (
                    headers.into_owned(),
                    received[end + 4..end + 4 + length].to_vec(),
                );
            }
        }
    }
}
fn reply(socket: &mut TcpStream, status: &str, body: &str) {
    write!(
        socket,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    socket.flush().unwrap();
}

#[test]
fn snapshot_and_whole_set_profile_bind_every_connection_capability_and_declaration() {
    let selected = connection("https://gateway.invalid/private?token=secret-query");
    let evaluator = evaluator(selected.clone());
    let activation = mixed_activation();
    let prepared = prepare(&evaluator, &activation, json!({"rows":["fixed"]})).unwrap();
    let encoded = serde_json::to_string(&prepared).unwrap();
    for secret in ["gateway.invalid", "secret-query", "synthetic-secret"] {
        assert!(!encoded.contains(secret));
    }
    let provenance = prepared_profile_provenance(&prepared.profile).unwrap();
    assert_eq!(provenance.provider, SystemOneProvider::Custom);
    assert_eq!(provenance.model, "synthetic-model");
    assert_eq!(provenance.connection_revision, 7);
    let restored = decode_prepared_assessment(encoded.as_bytes()).unwrap();
    assert_eq!(prepared, restored);
    validate(&evaluator, &activation, &restored).unwrap();
    validate_prepared_assessment(&activation, &restored).unwrap();
    for field in [
        "endpoint", "model", "revision", "mode", "digest", "provider",
    ] {
        let mut changed = selected.clone();
        match field {
            "endpoint" => {
                changed.endpoint = SystemOneEndpoint::ExactUrl("https://changed.invalid".into())
            }
            "model" => changed.model.push_str("-changed"),
            "revision" => changed.revision += 1,
            "mode" => changed.response_mode = SystemOneResponseMode::CloudflareEnvelope,
            "digest" => changed.model_revision = Some("changed".into()),
            "provider" => {
                changed.provider = SystemOneProvider::Ollama;
                changed.endpoint = SystemOneEndpoint::BaseUrl("http://127.0.0.1:9".into());
            }
            _ => unreachable!(),
        }
        assert!(
            validate_prepared_for_connection(&activation, &prepared, &changed).is_err(),
            "{field}"
        );
    }
    for field in ["reserve", "confidence", "probabilities", "limit-source"] {
        let mut caps = capabilities(&selected);
        match field {
            "reserve" => caps.rendering_reserve_tokens += 1,
            "confidence" => caps.output.score.as_mut().unwrap().confidence = false,
            "probabilities" => caps.output.choice.as_mut().unwrap().probabilities = false,
            "limit-source" => {
                caps.request_body_bytes.source =
                    antiburn_local::analysis::jev::capabilities::CapabilitySource::RuntimeMetadata
            }
            _ => unreachable!(),
        }
        let changed =
            TypeSafeDecisionEvaluator::from_selected_snapshot(selected.clone(), None, caps)
                .unwrap();
        assert!(
            validate(&changed, &activation, &prepared).is_err(),
            "{field}"
        );
    }
    for change in ["question", "criterion", "input", "fit", "extra-block"] {
        let mut changed = activation.clone();
        let compiled = Arc::make_mut(&mut changed.compiled);
        match change {
            "question" => {
                let Question::Choice { question, .. } =
                    &mut compiled.decisions[0].outputs[0].question
                else {
                    unreachable!()
                };
                question.push_str(" Changed.");
            }
            "criterion" => {
                let Question::Score { criteria, .. } =
                    &mut compiled.decisions[0].outputs[1].question
                else {
                    unreachable!()
                };
                criteria.swap(0, 1);
            }
            "input" => {
                compiled.decisions[0].input =
                    antiburn_checks::compiler::InputTemplate::Object(Vec::new())
            }
            "fit" => compiled.decisions[0].fit.clear(),
            "extra-block" => {
                let mut block = compiled.decisions[0].clone();
                block.name = "second".into();
                compiled.decisions.push(block);
            }
            _ => unreachable!(),
        }
        assert!(
            validate_prepared_assessment(&changed, &prepared).is_err(),
            "{change}"
        );
    }
}

#[test]
fn saved_requests_and_protected_inputs_cannot_change_under_a_matching_profile() {
    let evaluator = evaluator(connection("https://unused.invalid"));
    let active = mixed_activation();
    let original = prepare(
        &evaluator,
        &active,
        json!({"rows":[1,2],"fixed":"protected"}),
    )
    .unwrap();
    for change in [
        "input",
        "original",
        "question",
        "framing",
        "extra-field",
        "omissions",
        "profile",
        "budget",
    ] {
        let mut changed = original.clone();
        match change {
            "input" => changed.decisions[0].input["fixed"] = json!("changed"),
            "original" => changed.decisions[0].original_input["fixed"] = json!("changed"),
            "question" => {
                changed.decisions[0].request["questions"]["verdict"]["instructions"]["question"] =
                    json!("changed")
            }
            "framing" => changed.decisions[0].request["state"]["framing"] = json!("changed"),
            "extra-field" => changed.decisions[0].request["extra"] = json!(true),
            "omissions" => {
                changed.decisions[0]
                    .report
                    .omissions
                    .push(antiburn_checks::fit::FitOmission {
                        path: "fixed".into(),
                        strategy: antiburn_checks::fit::FitStrategy::Truncate,
                        original_count: 9,
                        retained_count: 8,
                    })
            }
            "profile" => changed.profile = "legacy-profile".into(),
            "budget" => changed.fitting_work_bytes = u64::MAX,
            _ => unreachable!(),
        }
        assert!(
            validate_prepared_assessment(&active, &changed).is_err(),
            "{change}"
        );
    }
}

#[test]
fn fitting_preserves_originals_and_explicit_omissions_without_refitting_saved_sets() {
    let evaluator = evaluator(connection("https://unused.invalid"));
    let active = activation();
    let input = json!({"rows":["a".repeat(20_000),"b".repeat(20_000),"c".repeat(20_000)],"fixed":"required"});
    let prepared = prepare(&evaluator, &active, input.clone()).unwrap();
    let block = &prepared.decisions[0];
    assert_eq!(block.original_input, input);
    assert_eq!(block.input["rows"], json!(["c".repeat(20_000)]));
    assert_eq!(block.input["fixed"], "required");
    assert_eq!(block.report.omissions[0].original_count, 3);
    assert_eq!(block.report.omissions[0].retained_count, 1);
    assert_eq!(
        block.request["state"]["input_omissions"]["targets"],
        json!(block.report.omissions)
    );
    assert!(prepared.fitting_work_bytes > serde_json::to_vec(&block.request).unwrap().len() as u64);
    validate_prepared_assessment(&active, &prepared).unwrap();
    let small = prepare(&evaluator, &active, json!({"rows":["fixed"]})).unwrap();
    assert!(small.decisions[0].report.omissions.is_empty());
    assert!(
        small.decisions[0].request["state"]
            .get("input_omissions")
            .is_none()
    );
}

#[test]
fn all_blocks_are_prepared_before_a_later_failure_and_share_admission() {
    let mut active = activation();
    let compiled = Arc::make_mut(&mut active.compiled);
    let mut second = compiled.decisions[0].clone();
    second.name = "second".into();
    second.fit.clear();
    compiled.decisions.push(second);
    let evaluator = evaluator(connection("http://127.0.0.1:9")).with_usage(Arc::new(|_, _, _| {
        panic!("preparation cannot record usage")
    }));
    let failure = evaluator
        .prepare(
            &active,
            vec![
                DecisionInput {
                    decision: "review".into(),
                    input: json!({"rows":["small"]}),
                },
                DecisionInput {
                    decision: "second".into(),
                    input: json!({"rows":["large".repeat(20_000)]}),
                },
            ],
            EvaluationBudget::default(),
        )
        .unwrap_err();
    assert_eq!(failure.code, PreparationFailureCode::NoFit);
    assert_eq!(failure.context.decision.as_deref(), Some("second"));
    let error = prepare(
        &evaluator,
        &active,
        json!({"rows":["x".repeat(4 * 1024 * 1024)]}),
    )
    .unwrap_err();
    assert_eq!(error.code, PreparationFailureCode::PreparationResourceLimit);
}

#[test]
fn per_type_caps_and_required_details_fail_before_transport() {
    let active = mixed_activation();
    let selected = connection("http://127.0.0.1:9");
    for field in ["choice", "score", "noul", "probabilities"] {
        let mut caps = capabilities(&selected);
        match field {
            "choice" => caps.output.choice = None,
            "score" => caps.output.score = None,
            "noul" => caps.output.noul = false,
            "probabilities" => caps.output.choice.as_mut().unwrap().probabilities = false,
            _ => unreachable!(),
        }
        let evaluator =
            TypeSafeDecisionEvaluator::from_selected_snapshot(selected.clone(), None, caps)
                .unwrap();
        let failure = prepare(&evaluator, &active, json!({"rows":[]})).expect_err(field);
        assert_eq!(
            failure.code,
            PreparationFailureCode::ProviderUnavailable,
            "{field}"
        );
    }
    let mut many = active.clone();
    let Question::Score { criteria, .. } =
        &mut Arc::make_mut(&mut many.compiled).decisions[0].outputs[1].question
    else {
        unreachable!()
    };
    *criteria = vec!["level".into(); 11];
    let failure = prepare(&evaluator(selected.clone()), &many, json!({"rows":[]})).unwrap_err();
    assert_eq!(failure.code, PreparationFailureCode::InvalidRequest);
    assert_eq!(
        failure.context.field.as_deref(),
        Some("decisions.review.output.severity.criteria")
    );
    let budget = failure.context.budget.unwrap();
    assert_eq!(
        budget.kind,
        antiburn_checks::fit::PreparationBudgetKind::Criteria
    );
    assert_eq!((budget.observed, budget.maximum), (11, 10));
    let mut caps = capabilities(&selected);
    caps.output.score.as_mut().unwrap().maximum_criteria = 24;
    let evaluator =
        TypeSafeDecisionEvaluator::from_selected_snapshot(selected, None, caps).unwrap();
    let prepared = prepare(&evaluator, &many, json!({"rows":[]})).unwrap();
    validate(&evaluator, &many, &prepared).unwrap();
}

#[test]
fn raw_typed_results_keep_labels_scores_probabilities_and_independent_confidence() {
    let active = mixed_activation();
    let evaluator = evaluator(connection("https://unused.invalid"));
    let prepared = prepare(&evaluator, &active, json!({"rows":[]})).unwrap();
    let request = serde_json::from_value(prepared.decisions[0].request.clone()).unwrap();
    let result = validate_response(
        &active.compiled.decisions[0],
        &active.compiled.question_requirements(),
        &request,
        serde_json::from_value(valid_response(true)).unwrap(),
        &evaluator.capabilities,
        2,
        SystemOneProvider::Custom,
    )
    .unwrap();
    let values = serde_json::to_value(&result).unwrap();
    assert_eq!(values["answers"][0]["result"]["output"], "clear");
    assert_eq!(
        values["answers"][0]["result"]["probabilities"]["clear"],
        0.2
    );
    assert_eq!(values["answers"][0]["result"]["confidence"], 0.7);
    assert_eq!(values["answers"][1]["result"]["output"], 0.3);
    assert_eq!(
        values["answers"][1]["result"]["probabilities"],
        json!([0.9, 0.1])
    );
    assert_eq!(
        values["answers"][2]["result"],
        json!({"type":"noul","output":0.4})
    );
    assert_eq!(result.attempts(), 2);
}

#[tokio::test]
async fn mixed_blocks_use_one_exact_request_in_selected_direct_envelope_and_ollama_routes() {
    for route in ["direct", "envelope", "ollama", "jev"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            if route == "ollama" {
                let mut socket = accept(&listener);
                let (headers, body) = read_request_with_headers(&mut socket);
                assert!(headers.starts_with("GET /api/tags HTTP/1.1"));
                assert!(body.is_empty());
                reply(
                    &mut socket,
                    "200 OK",
                    &json!({"models": [
                        {"name": "synthetic-model", "digest": "synthetic-digest"}
                    ]})
                    .to_string(),
                );
            }
            let mut socket = accept(&listener);
            let request = read_request(&mut socket);
            let value: Value = serde_json::from_slice(&request).unwrap();
            assert_eq!(value["questions"].as_object().unwrap().len(), 3);
            let mut response = valid_response(true);
            response["model"] = value["model"].clone();
            let body = if route == "envelope" {
                json!({"success":true,"result":response})
            } else {
                response
            };
            reply(&mut socket, "200 OK", &body.to_string());
            (listener, request)
        });
        let mut selected = connection(&endpoint);
        if route == "envelope" {
            selected.response_mode = SystemOneResponseMode::CloudflareEnvelope;
        }
        if route == "ollama" {
            selected.provider = SystemOneProvider::Ollama;
            selected.endpoint = SystemOneEndpoint::BaseUrl(endpoint.clone());
        }
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = records.clone();
        let mut evaluator = if route == "jev" {
            TypeSafeDecisionEvaluator::new(
                crate::jev::client::TypeSafeClient::new("synthetic".into()).unwrap(),
            )
        } else {
            evaluator(selected)
        };
        if route == "jev" {
            evaluator.transport = crate::jev::dispatch::DecisionTransport::Jev {
                client: crate::jev::client::TypeSafeClient::new("synthetic".into()).unwrap(),
                endpoint,
            };
        }
        let evaluator = evaluator.with_usage(Arc::new(move |candidate, attempt, usage| {
            captured
                .lock()
                .unwrap()
                .push((candidate.to_owned(), attempt.clone(), usage));
            Ok(())
        }));
        let active = mixed_activation();
        let prepared = prepare(&evaluator, &active, json!({"rows":["fixed"]})).unwrap();
        let attempt = attempt("review", 1);
        let result = evaluator
            .evaluate_prepared(&active, &candidate(&active), &prepared, &attempt)
            .await
            .unwrap();
        assert_eq!(result.answers().len(), 3);
        let (listener, request) = server.join().unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request).unwrap(),
            prepared.decisions[0].request
        );
        assert!(
            matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        );
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1, attempt);
    }
}

#[tokio::test]
async fn safe_retry_reuses_saved_requests_and_names_usage_for_each_block() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for status in ["200 OK", "429 Too Many Requests", "200 OK"] {
            let mut socket = accept(&listener);
            requests.push(read_request(&mut socket));
            reply(&mut socket, status, &valid_response(false).to_string());
        }
        assert_eq!(requests[1], requests[2]);
        requests
    });
    let records = Arc::new(Mutex::new(Vec::new()));
    let captured = records.clone();
    let evaluator =
        evaluator(connection(&endpoint)).with_usage(Arc::new(move |_, attempt, usage| {
            captured.lock().unwrap().push((attempt.clone(), usage));
            Ok(())
        }));
    let mut active = activation();
    let compiled = Arc::make_mut(&mut active.compiled);
    let mut second = compiled.decisions[0].clone();
    second.name = "second".into();
    compiled.decisions.push(second);
    let candidate = candidate(&active);
    let prepared = prepare(&evaluator, &active, json!({"rows":["fixed"]})).unwrap();
    evaluator
        .evaluate_prepared(&active, &candidate, &prepared, &attempt("review", 1))
        .await
        .unwrap();
    let error = evaluator
        .evaluate_prepared(&active, &candidate, &prepared, &attempt("second", 1))
        .await
        .unwrap_err();
    assert_eq!(error.kind, AttemptFailure::SafeTransient);
    let resumed = decode_prepared_assessment(&serde_json::to_vec(&prepared).unwrap()).unwrap();
    let result = evaluator
        .evaluate_prepared(&active, &candidate, &resumed, &attempt("second", 2))
        .await
        .unwrap();
    assert_eq!(result.attempts(), 2);
    server.join().unwrap();
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].0, attempt("review", 1));
    assert_eq!(records[1].0, attempt("second", 2));
}

#[tokio::test]
async fn invalid_answers_keep_usage_and_callback_errors_win_without_resending() {
    for (mode, storage_failure) in [
        (SystemOneResponseMode::Direct, false),
        (SystemOneResponseMode::CloudflareEnvelope, false),
        (SystemOneResponseMode::Direct, true),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut socket = accept(&listener);
            read_request(&mut socket);
            let mut response = valid_response(false);
            response["answers"]["verdict"]["choice"] = json!("undeclared");
            let body = if mode == SystemOneResponseMode::CloudflareEnvelope {
                json!({"success":true,"result":response})
            } else {
                response
            };
            reply(&mut socket, "200 OK", &body.to_string());
        });
        let mut selected = connection(&endpoint);
        selected.response_mode = mode;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captured = calls.clone();
        let evaluator = evaluator(selected)
            .with_cancellation(move || cancel.load(Ordering::Acquire))
            .with_usage(Arc::new(move |_, _, usage| {
                captured.lock().unwrap().push(usage);
                cancelled.store(true, Ordering::Release);
                if storage_failure {
                    Err("synthetic storage failure".into())
                } else {
                    Ok(())
                }
            }));
        let active = activation();
        let prepared = prepare(&evaluator, &active, json!({"rows":[]})).unwrap();
        let error = evaluator
            .evaluate_prepared(
                &active,
                &candidate(&active),
                &prepared,
                &attempt("review", 1),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            if storage_failure {
                EvaluationFailureCode::StorageFailure
            } else {
                EvaluationFailureCode::InvalidResponse
            }
        );
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .map(|observation| observation.as_ref().map(|observation| observation.usage))
                .collect::<Vec<_>>(),
            [Some(JevUsage {
                input_tokens: 12,
                output_tokens: 3
            })]
        );
        server.join().unwrap();
    }
}

#[tokio::test]
async fn cancellation_and_incompatible_profiles_stop_before_admission() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let original = evaluator(connection(&endpoint));
    let active = activation();
    let prepared = prepare(&original, &active, json!({"rows":[]})).unwrap();
    let cancelled = original
        .with_cancellation(|| true)
        .with_usage(Arc::new(|_, _, _| {
            panic!("unsent request cannot record usage")
        }));
    assert!(
        cancelled
            .evaluate_prepared(
                &active,
                &candidate(&active),
                &prepared,
                &attempt("review", 1)
            )
            .await
            .is_err()
    );
    let changed = evaluator(connection("http://127.0.0.1:9"));
    assert!(
        changed
            .evaluate_prepared(
                &active,
                &candidate(&active),
                &prepared,
                &attempt("review", 2)
            )
            .await
            .is_err()
    );
    assert!(matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
}

#[test]
fn local_declarations_reject_model_hooks_and_historical_profiles_are_read_only() {
    let evaluator = evaluator(connection("https://unused.invalid"));
    let active = activation();
    let mut prepared = prepare(&evaluator, &active, json!({"rows":[]})).unwrap();
    let local = activation_source(
        "name: Local\non:\n  tool_result:\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    assert!(prepare(&evaluator, &local, json!({})).is_err());
    assert!(validate(&evaluator, &local, &prepared).is_err());
    let mut profile: Value = serde_json::from_str(&prepared.profile).unwrap();
    profile["construction_version"] = json!("system-one-choice-input-v4");
    prepared.profile = profile.to_string();
    assert!(prepared_profile_provenance(&prepared.profile).is_some());
    assert!(validate_prepared_assessment(&active, &prepared).is_err());
}

#[test]
fn optional_probability_and_confidence_details_follow_independent_verified_flags() {
    let active = activation_source(&SOURCE.replace(
        "decisions.review.details.verdict.probabilities.concern > 0.9",
        "true",
    ));
    let selected = connection("https://unused.invalid");
    for (probabilities, confidence) in [(true, false), (false, true), (false, false)] {
        let mut caps = capabilities(&selected);
        let output = caps.output.choice.as_mut().unwrap();
        output.probabilities = probabilities;
        output.confidence = confidence;
        let evaluator =
            TypeSafeDecisionEvaluator::from_selected_snapshot(selected.clone(), None, caps)
                .unwrap();
        let prepared = prepare(&evaluator, &active, json!({"rows":[]})).unwrap();
        let request = serde_json::from_value(prepared.decisions[0].request.clone()).unwrap();
        let result = validate_response(
            &active.compiled.decisions[0],
            &active.compiled.question_requirements(),
            &request,
            serde_json::from_value(valid_response(false)).unwrap(),
            &evaluator.capabilities,
            1,
            SystemOneProvider::Custom,
        )
        .unwrap();
        let QuestionResult::Choice(answer) = result.answers()[0].result() else {
            unreachable!()
        };
        assert_eq!(answer.output(), "clear");
        assert_eq!(answer.probabilities().is_some(), probabilities);
        assert_eq!(answer.confidence().is_some(), confidence);
    }
}

#[tokio::test]
async fn post_send_disconnect_records_unknown_once_without_an_automatic_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut socket = accept(&listener);
        read_request(&mut socket);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n")
            .unwrap();
        drop(socket);
        listener
    });
    let usage = Arc::new(Mutex::new(Vec::new()));
    let recorded = usage.clone();
    let evaluator = evaluator(connection(&endpoint)).with_usage(Arc::new(move |_, _, usage| {
        recorded.lock().unwrap().push(usage);
        Ok(())
    }));
    let active = activation();
    let prepared = prepare(&evaluator, &active, json!({"rows":[]})).unwrap();
    let error = evaluator
        .evaluate_prepared(
            &active,
            &candidate(&active),
            &prepared,
            &attempt("review", 1),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, AttemptFailure::Unknown);
    assert_eq!(error.attempts, 1);
    assert_eq!(*usage.lock().unwrap(), [None]);
    let listener = server.join().unwrap();
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
fn authentication_and_failure_classification_remain_typed() {
    for (error, code, kind) in [
        (
            JevError::AuthenticationRejected,
            EvaluationFailureCode::AuthenticationRejected,
            AttemptFailure::Permanent,
        ),
        (
            JevError::ResponseDecode,
            EvaluationFailureCode::InvalidResponse,
            AttemptFailure::Permanent,
        ),
        (
            JevError::ProgressStorageFailure,
            EvaluationFailureCode::StorageFailure,
            AttemptFailure::Permanent,
        ),
        (
            JevError::RequestOutcomeUnknown,
            EvaluationFailureCode::DeliveryUnknown,
            AttemptFailure::Unknown,
        ),
        (
            JevError::RateLimited { retry_after: None },
            EvaluationFailureCode::RateLimited,
            AttemptFailure::SafeTransient,
        ),
    ] {
        let active = activation();
        let evaluator = evaluator(connection("https://unused.invalid"));
        let prepared = prepare(&evaluator, &active, json!({"rows":[]})).unwrap();
        let request = serde_json::from_value(prepared.decisions[0].request.clone()).unwrap();
        let outcome = classify_outcome(
            &active.compiled.decisions[0],
            &active.compiled.question_requirements(),
            &request,
            Err(error.clone()),
            &evaluator.capabilities,
            2,
            SystemOneProvider::Custom,
        );
        let called = Arc::new(AtomicBool::new(false));
        let captured = called.clone();
        let callback: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(move || {
            captured.store(true, Ordering::Release);
            Ok(())
        });
        settle_authentication_rejection(&outcome, Some(&callback), 2).unwrap();
        assert_eq!(
            called.load(Ordering::Acquire),
            error == JevError::AuthenticationRejected
        );
        let error = evaluation_result(outcome, 2, SystemOneProvider::Custom, "synthetic-model")
            .unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.kind, kind);
        assert_eq!(error.attempts, 2);
    }
}

#[test]
fn cloudflare_preflight_keeps_its_identifier_boundary_and_direct_modes_do_not_inherit_it() {
    for length in [100, 101] {
        let name = "a".repeat(length);
        let active = activation_source(&SOURCE.replace("verdict", &name));
        let direct = evaluator(connection("https://unused.invalid"));
        assert!(prepare(&direct, &active, json!({"rows":[]})).is_ok());
        for provider in [SystemOneProvider::Cloudflare, SystemOneProvider::Custom] {
            let mut selected = connection("https://unused.invalid");
            selected.provider = provider;
            selected.response_mode = SystemOneResponseMode::CloudflareEnvelope;
            let caps = if provider == SystemOneProvider::Cloudflare {
                selected.endpoint =
                    SystemOneEndpoint::CloudflareAccount("synthetic-account".into());
                selected.model = "clef".into();
                selected.model_revision = None;
                crate::jev_cloudflare::cloudflare_capabilities("clef")
            } else {
                capabilities(&selected)
            };
            let evaluator = TypeSafeDecisionEvaluator::from_selected_snapshot(
                selected,
                Some("synthetic".into()),
                caps,
            )
            .unwrap();
            let result = prepare(&evaluator, &active, json!({"rows":[]}));
            if length == 100 {
                validate(&evaluator, &active, &result.unwrap()).unwrap();
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.code, PreparationFailureCode::InvalidRequest);
                assert_eq!(
                    error.context.field,
                    Some(format!("decisions.review.output.{name}"))
                );
            }
        }
    }
}

#[test]
fn fitting_work_is_one_shared_allowance_across_all_reductions_and_blocks() {
    let mut active =
        activation_source(&SOURCE.replace("rows", "text").replace("recent", "truncate"));
    let declaration = active.compiled.decisions[0].clone();
    Arc::make_mut(&mut active.compiled).decisions = (0..20)
        .map(|index| {
            let mut block = declaration.clone();
            block.name = format!("block_{index}");
            block
        })
        .collect();
    let evaluator = evaluator(connection("http://127.0.0.1:9"))
        .with_usage(Arc::new(|_, _, _| panic!("fitting cannot dispatch")));
    let failure = prepare(&evaluator, &active, json!({"text":"x".repeat(100_000)})).unwrap_err();
    assert_eq!(
        failure.code,
        PreparationFailureCode::PreparationResourceLimit
    );
    let budget = failure.context.budget.unwrap();
    assert_eq!(
        budget.kind,
        antiburn_checks::fit::PreparationBudgetKind::WorkBytes
    );
    assert_eq!(budget.maximum, 64 * 1024 * 1024);
    assert!(budget.observed > budget.maximum);
    assert_ne!(failure.context.decision.as_deref(), Some("block_0"));
}
