use super::*;
use crate::store::Store;
use antiburn_checks::expression_eval::EvaluationValue;
use antiburn_checks::fit::{NoFitReason, PreparationBudgetKind};
use antiburn_local::analysis::jev::{
    NATIVE_MODEL_ALIAS, PINNED_MODEL, estimate_jev_request_tokens,
};

fn read_native_request(socket: &mut TcpStream) -> Vec<u8> {
    let (headers, body) = read_request_with_headers(socket);
    let long_context = headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("TypeSafe-Danger-Zone-Long-Context")
                .then(|| value.trim())
        })
        .collect::<Vec<_>>();
    assert_eq!(long_context, ["true"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["model"],
        NATIVE_MODEL_ALIAS
    );
    body
}

fn persisted_usage_aggregate(store: &Store, model: &str) -> Value {
    let ledger: Value = serde_json::from_str(
        &store
            .internal_value("internal:burnCheckUsageLedgerV1")
            .unwrap(),
    )
    .unwrap();
    let aggregates = ledger["summary"]["aggregates"].as_array().unwrap();
    let matches = aggregates
        .iter()
        .filter(|aggregate| aggregate["model"] == model)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1);
    (*matches[0]).clone()
}

fn native(connection: SystemOneConnection) -> TypeSafeDecisionEvaluator {
    let capabilities = connection.capabilities().unwrap();
    TypeSafeDecisionEvaluator::from_selected_snapshot(
        connection,
        Some("synthetic-key".into()),
        capabilities,
    )
    .unwrap()
}

fn ignored_instructions(
    result_bytes: usize,
    argument_bytes: usize,
) -> (EffectiveActivation, Value, EvaluationBudget) {
    let active = activation_source(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../.antiburn/checks/ignored-instructions.yml"
    )));
    let instructions = json!([{
        "path":"/synthetic/repo/AGENTS.md", "content":"Keep generated files unchanged. Apply the recorded exception only to src/parser.rs.",
        "scope":{"root":"/synthetic/repo","globs":["src/**"]},"observed_at":"2026-10-20T00:00:00Z"
    }]);
    let messages = (0..4).map(|index|json!({
        "id":format!("result-{index}"),"source":"synthetic","kind":"tool_result","native_order":index,
        "quality":{"source_complete":true,"request_history_complete":true},
        "tool":{"name":"apply_patch","category":"write","arguments":{"path":"src/parser.rs","patch":if index == 0 { "p".repeat(argument_bytes) } else { "protected argument".into() }},
            "result":{"content":"a".repeat(result_bytes),"status":"completed"}}
    })).collect::<Vec<_>>();
    let context = json!({
        "global":{"instructions":instructions,"instructions_complete":true,"instructions_limits":[],
            "skills":[{"name":"unrelated","description":"Do not use this skill as an instruction.","path":"/synthetic/skills/review/SKILL.md"}],
            "skills_complete":true,"skills_limits":[],"snapshot_revision":"synthetic-global-1"},
        "repo":{"instructions":instructions,"instructions_complete":false,"instructions_limits":["unresolved-source"],
            "skills_complete":false,"skills_limits":["unavailable"],"snapshot_revision":"synthetic-repo-1"},
        "session":{"requests":[
            {"id":"request-1","native_order":1,"kind":"user_message","text":"Fix the parser only."},
            {"id":"request-2","native_order":2,"kind":"user_message","text":"Correction: leave generated files unchanged."},
            {"id":"request-3","native_order":3,"kind":"user_message","text":"Exception: src/parser.rs may change."}
        ],"requests_complete":true,"source_coverage":"complete"},
        "window":{"messages":messages}
    });
    let mut budget = EvaluationBudget::default();
    let evaluated = antiburn_checks::preparation::evaluate_decision_input_with_budget(
        &active.compiled.decisions[0],
        &context,
        &mut budget,
    )
    .unwrap();
    let EvaluationValue::Available(input) = evaluated.value else {
        panic!("Synthetic input must be available");
    };
    (active, input, budget)
}

fn prepare_ignored(
    evaluator: &TypeSafeDecisionEvaluator,
    active: &EffectiveActivation,
    input: Value,
    budget: EvaluationBudget,
) -> Result<PreparedAssessment, PreparationFailure> {
    evaluator.prepare(
        active,
        vec![DecisionInput {
            decision: "review".into(),
            input,
        }],
        budget,
    )
}

fn assert_content_only(block: &antiburn_checks::prepared_assessment::PreparedBlock) {
    let mut restored = block.input.clone();
    assert_eq!(restored["activity"].as_array().unwrap().len(), 4);
    for omission in &block.report.omissions {
        assert!(omission.path.starts_with("activity["));
        assert!(omission.path.ends_with("].tool.result.content"));
    }
    for index in 0..4 {
        restored["activity"][index]["tool"]["result"]["content"] =
            block.original_input["activity"][index]["tool"]["result"]["content"].clone();
    }
    assert_eq!(restored, block.original_input);
    assert_eq!(
        block.request["state"]["input_omissions"]["targets"],
        json!(block.report.omissions)
    );
}

#[test]
fn maintained_ignored_instructions_prepares_native_unchanged_and_rejects_synthetic_old_limits() {
    let connection = SystemOneConnection::jev_default();
    let evaluator = native(connection.clone());
    let (active, input, budget) = ignored_instructions(2048, 40_000);
    let prepared = prepare_ignored(&evaluator, &active, input.clone(), budget.clone()).unwrap();
    let block = &prepared.decisions[0];
    assert_eq!(block.original_input, input);
    assert_eq!(block.input, input);
    assert!(block.report.omissions.is_empty());
    assert!(block.request["state"].get("input_omissions").is_none());
    let request: JevRequest = serde_json::from_value(block.request.clone()).unwrap();
    assert!(estimate_jev_request_tokens(&request).unwrap() > 28_672);
    assert_eq!(request.model, NATIVE_MODEL_ALIAS);
    let mut old = connection.capabilities().unwrap();
    old.total_input_tokens.value = Some(65_536);
    old.state_and_longest_question_tokens.value = Some(32_768);
    old.rendering_reserve_tokens = 4096;
    old.request_body_bytes.value = Some(256 * 1024);
    let old = TypeSafeDecisionEvaluator::from_selected_snapshot(
        connection,
        Some("synthetic-key".into()),
        old,
    )
    .unwrap();
    let failure = prepare_ignored(&old, &active, input, budget).unwrap_err();
    assert_eq!(failure.code, PreparationFailureCode::NoFit);
    assert_eq!(
        failure.context.no_fit_reason,
        Some(NoFitReason::ReductionsExhausted)
    );
    let budget = failure.context.budget.unwrap();
    assert_eq!(budget.kind, PreparationBudgetKind::EstimatedTokens);
    assert_eq!(budget.maximum, 28_672);
    assert!(budget.observed > budget.maximum);
    let omissions = failure.report.unwrap().omissions;
    assert_eq!(omissions.len(), 4);
    for (index, omission) in omissions.iter().enumerate() {
        assert_eq!(
            omission.path,
            format!("activity[{index}].tool.result.content")
        );
        assert_eq!(omission.retained_count, 256);
    }
}

#[test]
fn native_preparation_shortens_only_authored_content() {
    let evaluator = native(SystemOneConnection::jev_default());
    let (active, input, budget) = ignored_instructions(76_000, 40_000);
    let prepared = prepare_ignored(&evaluator, &active, input, budget).unwrap();
    let block = &prepared.decisions[0];
    assert!(!block.report.omissions.is_empty());
    assert_content_only(block);
    assert!(prepared.fitting_work_bytes < 64 * 1024 * 1024);
    validate(&evaluator, &active, &prepared).unwrap();
}

#[test]
fn native_preparation_ignores_legacy_lower_overrides() {
    let connection = SystemOneConnection {
        context_override: Some(ContextLimitOverride {
            total_input_tokens: Some(32_768),
            ..Default::default()
        }),
        ..Default::default()
    };
    let evaluator = native(connection);
    let automatic = native(SystemOneConnection::jev_default());
    let (active, input, budget) = ignored_instructions(2048, 40_000);
    let expected = prepare_ignored(&automatic, &active, input.clone(), budget.clone()).unwrap();
    let prepared = prepare_ignored(&evaluator, &active, input.clone(), budget).unwrap();
    let block = &prepared.decisions[0];
    assert_eq!(block.original_input, input);
    assert_eq!(block.input, input);
    assert!(block.report.omissions.is_empty());
    assert_eq!(block.request, expected.decisions[0].request);
    validate(&evaluator, &active, &prepared).unwrap();
}

#[test]
fn native_ignored_instructions_fits_unused_descriptions_and_preserves_all_protected_evidence() {
    let evaluator = native(SystemOneConnection::jev_default());
    let (active, mut input, budget) = ignored_instructions(256, 310_000);
    for scope in ["current_global_scope", "current_repo_scope"] {
        input[scope]["skills"] = json!([{
            "name":"unused-review", "description":"s".repeat(16 * 1024),
            "path":format!("/synthetic/{scope}/review/SKILL.md")
        }]);
        input[scope]["skills_complete"] = json!(false);
        input[scope]["skills_limits"] = json!(["unsupported-entry"]);
    }
    let message = input["activity"][1].clone();
    for index in 4..20 {
        let mut message = message.clone();
        message["id"] = json!(format!("result-{index}"));
        message["native_order"] = json!(index);
        input["activity"].as_array_mut().unwrap().push(message);
    }
    input["request_history_complete"] = json!(false);
    input["source_coverage"] = json!("partial");
    for message in input["activity"].as_array_mut().unwrap() {
        message["quality"]["request_history_complete"] = json!(false);
    }
    let mut protected = active.clone();
    Arc::make_mut(&mut protected.compiled).decisions[0]
        .fit
        .retain(|rule| rule.path.starts_with("activity["));
    assert_eq!(
        prepare_ignored(&evaluator, &protected, input.clone(), budget.clone())
            .unwrap_err()
            .code,
        PreparationFailureCode::NoFit
    );

    let prepared = prepare_ignored(&evaluator, &active, input.clone(), budget).unwrap();
    let block = &prepared.decisions[0];
    assert_eq!(block.original_input, input);
    assert_eq!(block.input["activity"].as_array().unwrap().len(), 20);
    assert_eq!(block.input["activity"], input["activity"]);
    assert_eq!(block.input["requests"], input["requests"]);
    assert_eq!(block.report.omissions.len(), 2);
    let mut restored = block.input.clone();
    for (scope, omission) in ["current_global_scope", "current_repo_scope"]
        .into_iter()
        .zip(&block.report.omissions)
    {
        assert_eq!(omission.path, format!("{scope}.skills[0].description"));
        assert_eq!(
            omission.strategy,
            antiburn_checks::fit::FitStrategy::Truncate
        );
        assert_eq!(omission.original_count, 16 * 1024);
        let description = block.input[scope]["skills"][0]["description"]
            .as_str()
            .unwrap();
        let (head, tail) = description
            .split_once(antiburn_checks::fit::OMISSION_MARKER)
            .unwrap();
        assert_eq!(omission.retained_count, head.len() + tail.len());
        assert_eq!(
            description
                .matches(antiburn_checks::fit::OMISSION_MARKER)
                .count(),
            1
        );
        assert!(
            input[scope]["skills"][0]["description"]
                .as_str()
                .unwrap()
                .starts_with(head)
        );
        assert!(
            input[scope]["skills"][0]["description"]
                .as_str()
                .unwrap()
                .ends_with(tail)
        );
        restored[scope]["skills"][0]["description"] =
            input[scope]["skills"][0]["description"].clone();
    }
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&input).unwrap()
    );
    assert_eq!(block.request["state"]["evidence"], block.input);
    assert_eq!(
        block.request["state"]["input_omissions"]["targets"],
        json!(block.report.omissions)
    );
    assert!(prepared.fitting_work_bytes < 64 * 1024 * 1024);
    validate(&evaluator, &active, &prepared).unwrap();
}

#[test]
fn native_ignored_instructions_preserves_missing_empty_and_partial_scope_evidence() {
    let evaluator = native(SystemOneConnection::jev_default());
    let (active, base, budget) = ignored_instructions(256, 256);
    let unavailable = json!({
        "instructions_complete":false,"instructions_limits":["unavailable"],
        "skills_complete":false,"skills_limits":["unavailable"]
    });
    let empty = json!({
        "instructions":[],"instructions_complete":true,"instructions_limits":[],
        "skills":[],"skills_complete":true,"skills_limits":[],"snapshot_revision":"synthetic-empty-1"
    });
    let mut partial = base["current_repo_scope"].clone();
    partial["skills"] =
        json!([{"name":"review","description":"","path":"/synthetic/review/SKILL.md"}]);
    partial["skills_complete"] = json!(false);
    partial["skills_limits"] = json!(["unsupported-entry"]);
    for (global, repo) in [
        (unavailable.clone(), base["current_repo_scope"].clone()),
        (base["current_global_scope"].clone(), unavailable.clone()),
        (unavailable.clone(), unavailable),
        (empty.clone(), empty),
        (partial.clone(), partial),
    ] {
        let mut input = base.clone();
        input["current_global_scope"] = global;
        input["current_repo_scope"] = repo;
        input["request_history_complete"] = json!(false);
        input["source_coverage"] = json!("partial");
        for message in input["activity"].as_array_mut().unwrap() {
            message["quality"]["request_history_complete"] = json!(false);
        }
        let prepared = prepare_ignored(&evaluator, &active, input.clone(), budget.clone()).unwrap();
        let block = &prepared.decisions[0];
        assert_eq!(
            serde_json::to_vec(&block.input).unwrap(),
            serde_json::to_vec(&input).unwrap()
        );
        assert_eq!(block.original_input, input);
        assert!(block.report.omissions.is_empty());
        assert!(block.request["state"].get("input_omissions").is_none());
        validate(&evaluator, &active, &prepared).unwrap();
    }
}

#[test]
fn native_ignored_instructions_irreducible_evidence_returns_no_preparation_or_send() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let evaluator = native(SystemOneConnection::jev_default())
        .with_native_test_endpoint(format!("http://{}", listener.local_addr().unwrap()))
        .with_usage(Arc::new(|_, _, _| {
            panic!("Rejected input cannot record provider usage")
        }));
    for (field, bytes, kind) in [
        ("argument", 360_000, PreparationBudgetKind::EstimatedTokens),
        ("argument", 400_000, PreparationBudgetKind::RequestBytes),
        ("request", 400_000, PreparationBudgetKind::RequestBytes),
        ("instructions", 400_000, PreparationBudgetKind::RequestBytes),
    ] {
        let (active, mut input, budget) =
            ignored_instructions(256, if field == "argument" { bytes } else { 256 });
        match field {
            "argument" => {}
            "request" => input["requests"][2]["text"] = json!("Exception: ".to_owned() + &"r".repeat(bytes)),
            "instructions" => input["current_repo_scope"]["instructions"] = (0..4).map(|index| json!({
                "path":format!("/synthetic/repo/rules-{index}.md"),"content":"i".repeat(bytes / 4),
                "scope":{"root":"/synthetic/repo","globs":["src/**"]},"observed_at":"2026-10-27T00:00:00Z"
            })).collect(),
            _ => unreachable!(),
        }
        let failure = prepare_ignored(&evaluator, &active, input, budget).unwrap_err();
        assert_eq!(failure.code, PreparationFailureCode::NoFit, "{field}");
        assert_eq!(
            failure.context.no_fit_reason,
            Some(NoFitReason::TargetsAtMinimum)
        );
        let budget = failure.context.budget.unwrap();
        assert_eq!(budget.kind, kind, "{field}");
        assert!(budget.observed > budget.maximum, "{field}");
        assert!(failure.report.unwrap().omissions.is_empty());
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}

#[test]
fn native_original_admission_and_fitting_work_limits_remain_resource_failures() {
    let evaluator = native(SystemOneConnection::jev_default());
    let active = activation_source(&SOURCE.replace("rows", "text").replace("recent", "truncate"));
    for input in [
        json!({"text":"x".repeat(8 * 1024 * 1024)}),
        json!({"text":"x","nodes":vec![Value::Null;17_000]}),
        (0..70).fold(json!({"text":"x"}), |value, _| json!({"nested":value})),
    ] {
        assert_eq!(
            prepare(&evaluator, &active, input).unwrap_err().code,
            PreparationFailureCode::PreparationResourceLimit
        );
    }
    let input = json!({"text":"x".repeat(600_000)});
    let failure = prepare(&evaluator, &active, input).unwrap_err();
    assert_eq!(
        failure.code,
        PreparationFailureCode::PreparationResourceLimit
    );
    assert_eq!(
        failure.context.budget.unwrap().kind,
        PreparationBudgetKind::WorkBytes
    );
    assert_eq!(failure.context.no_fit_reason, None);
}

#[tokio::test]
async fn native_restart_preserves_saved_requests_and_rejects_old_concrete_work_without_sends() {
    let active = activation();
    let evaluator = native(SystemOneConnection::jev_default());
    let prepared = prepare(&evaluator, &active, json!({"rows":["protected"]})).unwrap();
    let encoded = serde_json::to_vec(&prepared).unwrap();
    let restarted = decode_prepared_assessment(&encoded).unwrap();
    validate(&evaluator, &active, &restarted).unwrap();
    assert_eq!(serde_json::to_vec(&restarted).unwrap(), encoded);
    let mut descriptor = evaluator.descriptor.clone();
    descriptor.model = PINNED_MODEL.into();
    let old_caps = ModelCapabilities::jev_default();
    let old = prepare_selected_assessment(
        &active,
        vec![DecisionInput {
            decision: "review".into(),
            input: json!({"rows":["protected"]}),
        }],
        EvaluationBudget::default(),
        &old_caps,
        &descriptor,
    )
    .unwrap();
    validate_prepared_assessment(&active, &old).unwrap();
    assert_eq!(
        prepared_profile_provenance(&old.profile).unwrap().model,
        PINNED_MODEL
    );
    let before = serde_json::to_vec(&old).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut current = evaluator;
    current.transport = crate::jev::dispatch::DecisionTransport::Jev {
        client: crate::jev::client::TypeSafeClient::new("synthetic-key".into()).unwrap(),
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
    };
    let error = current
        .evaluate_prepared(&active, &candidate(&active), &old, &attempt("review", 2))
        .await
        .unwrap_err();
    assert_eq!(error.attempts, 0);
    assert_eq!(error.code, EvaluationFailureCode::InvalidRequest);
    assert_eq!(serde_json::to_vec(&old).unwrap(), before);
    assert!(
        matches!(listener.accept(),Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
fn evaluator_native_alias_policy_does_not_weaken_custom_response_matching() {
    let active = mixed_activation();
    let evaluator = native(SystemOneConnection::jev_default());
    let prepared = prepare(&evaluator, &active, json!({"rows":[]})).unwrap();
    let request = serde_json::from_value(prepared.decisions[0].request.clone()).unwrap();
    for model in ["jev-1.14.0", "future/jev:release-2099", NATIVE_MODEL_ALIAS] {
        let mut response = valid_response(true);
        response["model"] = json!(model);
        let response: JevResponse = serde_json::from_value(response).unwrap();
        validate_response(
            &active.compiled.decisions[0],
            &active.compiled.question_requirements(),
            &request,
            response.clone(),
            &evaluator.capabilities,
            1,
            SystemOneProvider::Jev,
        )
        .unwrap();
        let strict = validate_response(
            &active.compiled.decisions[0],
            &active.compiled.question_requirements(),
            &request,
            response,
            &evaluator.capabilities,
            1,
            SystemOneProvider::Custom,
        );
        assert_eq!(strict.is_ok(), model == NATIVE_MODEL_ALIAS);
    }
}

#[test]
fn resident_admission_selects_native_policy_by_route_not_model() {
    let mut request = JevRequest {
        model: NATIVE_MODEL_ALIAS.into(),
        state: json!({"text":"a".repeat(320_000)}),
        questions: Default::default(),
    };
    request.questions.insert(
        "q".into(),
        antiburn_local::analysis::jev::JevQuestion::Noul {
            instructions: json!("Does it apply?"),
            criteria: None,
        },
    );
    let caps = ModelCapabilities::jev_native();
    let bytes = resident_bytes(&request, &caps, SystemOneProvider::Jev).unwrap();
    assert!(bytes > MAX_RESIDENT_REQUEST_BYTES);
    assert!(bytes < NATIVE_MAX_RESIDENT_REQUEST_BYTES);
    assert!(bytes < crate::jev::dispatch::GLOBAL_REQUEST_BYTES);
    assert!(matches!(
        resident_bytes(&request, &caps, SystemOneProvider::Custom),
        Err(JevError::RequestTooLarge {
            maximum: MAX_RESIDENT_REQUEST_BYTES,
            ..
        })
    ));
}

#[tokio::test]
async fn native_large_prepared_request_passes_shared_admission_and_records_served_model() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut socket = accept(&listener);
        let request = read_request(&mut socket);
        assert!(request.len() > 256 * 1024);
        let mut response = valid_response(false);
        response["model"] = json!("future/jev:release-2099");
        reply(&mut socket, "200 OK", &response.to_string());
        request
    });
    let observed = Arc::new(Mutex::new(Vec::new()));
    let saved = observed.clone();
    let evaluator = native(SystemOneConnection::default())
        .with_native_test_endpoint(endpoint)
        .with_usage(Arc::new(move |_, _, observation| {
            saved.lock().unwrap().push(observation);
            Ok(())
        }));
    let active = activation();
    let prepared = prepare(&evaluator, &active, json!({"rows":["a".repeat(320_000)]})).unwrap();
    assert!(prepared.decisions[0].report.omissions.is_empty());
    evaluator
        .evaluate_prepared(
            &active,
            &candidate(&active),
            &prepared,
            &attempt("review", 1),
        )
        .await
        .unwrap();
    let sent = server.join().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&sent).unwrap(),
        prepared.decisions[0].request
    );
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(
        observed[0].as_ref().unwrap().served_model.as_deref(),
        Some("future/jev:release-2099")
    );
}

#[tokio::test]
async fn native_safe_retry_keeps_exact_alias_requests_and_persists_each_served_release() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, model) in [
            ("200 OK", "jev-1.13.0"),
            ("429 Too Many Requests", "jev-1.14.0"),
            ("200 OK", "jev-1.14.0"),
        ] {
            let mut socket = accept(&listener);
            requests.push(read_native_request(&mut socket));
            let mut response = valid_response(false);
            response["model"] = json!(model);
            reply(&mut socket, status, &response.to_string());
        }
        assert_eq!(requests[1], requests[2]);
        (listener, requests)
    });
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path()).unwrap());
    let callback_store = store.clone();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let saved = observed.clone();
    let evaluator = native(SystemOneConnection::default())
        .with_native_test_endpoint(endpoint)
        .with_usage(Arc::new(move |candidate_id, attempt, observation| {
            callback_store
                .record_burn_check_decision_observation(
                    candidate_id,
                    attempt,
                    "selected-provider",
                    ("typesafe-systemone", NATIVE_MODEL_ALIAS),
                    observation.clone(),
                    100,
                )
                .map_err(|error| error.to_string())?;
            assert_eq!(
                callback_store
                    .burn_check_decision_served_model(candidate_id, attempt)
                    .unwrap(),
                observation
                    .as_ref()
                    .and_then(|value| value.served_model.clone())
            );
            saved.lock().unwrap().push((attempt.clone(), observation));
            Ok(())
        }));
    let mut active = activation();
    let mut second = active.compiled.decisions[0].clone();
    second.name = "second".into();
    Arc::make_mut(&mut active.compiled).decisions.push(second);
    let candidate = candidate(&active);
    let candidate_id = candidate
        .fingerprint
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let prepared = prepare(&evaluator, &active, json!({"rows":["protected"]})).unwrap();
    let before = serde_json::to_vec(&prepared).unwrap();
    let sibling = evaluator
        .evaluate_prepared(&active, &candidate, &prepared, &attempt("review", 1))
        .await
        .unwrap();
    let completed_sibling = serde_json::to_vec(&sibling).unwrap();
    let first_usage = store.burn_check_usage_summary().unwrap();
    assert_eq!(first_usage.confirmed_calls, 1);
    assert_eq!(first_usage.estimated_usd.as_deref(), Some("$0.000000504"));
    let error = evaluator
        .evaluate_prepared(&active, &candidate, &prepared, &attempt("second", 1))
        .await
        .unwrap_err();
    assert_eq!(error.kind, AttemptFailure::SafeTransient);
    assert_eq!(store.burn_check_usage_summary().unwrap(), first_usage);
    let resumed = decode_prepared_assessment(&before).unwrap();
    let result = evaluator
        .evaluate_prepared(&active, &candidate, &resumed, &attempt("second", 2))
        .await
        .unwrap();
    assert_eq!(result.attempts(), 2);
    assert_eq!(sibling.attempts(), 1);
    assert_eq!(serde_json::to_vec(&sibling).unwrap(), completed_sibling);
    assert_eq!(serde_json::to_vec(&resumed).unwrap(), before);
    let (listener, requests) = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    for (index, bytes) in requests.into_iter().enumerate() {
        let expected: JevRequest =
            serde_json::from_value(prepared.decisions[usize::from(index != 0)].request.clone())
                .unwrap();
        assert_eq!(bytes, serde_json::to_vec(&expected).unwrap());
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            request["questions"]["verdict"]["instructions"]["decision"],
            if index == 0 { "review" } else { "second" }
        );
        assert_eq!(request["model"], NATIVE_MODEL_ALIAS);
    }
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0].0, attempt("review", 1));
    assert_eq!(observed[1].0, attempt("second", 2));
    assert_eq!(
        observed[0].1.as_ref().unwrap().served_model.as_deref(),
        Some("jev-1.13.0")
    );
    assert_eq!(
        observed[1].1.as_ref().unwrap().served_model.as_deref(),
        Some("jev-1.14.0")
    );
    drop(observed);
    assert!(
        matches!(listener.accept(),Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    let summary = store.burn_check_usage_summary().unwrap();
    assert_eq!(
        (
            summary.confirmed_calls,
            summary.input_tokens,
            summary.output_tokens,
            summary.unknown_outcomes
        ),
        (2, 24, 6, 0)
    );
    assert_eq!(summary.estimated_usd.as_deref(), Some("$0.000001008"));
    drop(evaluator);
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    assert_eq!(reopened.burn_check_usage_summary().unwrap(), summary);
    for (attempt, model, price_version) in [
        (
            attempt("review", 1),
            "jev-1.13.0",
            "typesafe-model-catalog-2026-09-25",
        ),
        (
            attempt("second", 2),
            "jev-1.14.0",
            "typesafe-model-catalog-2026-10-26",
        ),
    ] {
        assert_eq!(
            reopened
                .burn_check_decision_served_model(&candidate_id, &attempt)
                .unwrap()
                .as_deref(),
            Some(model)
        );
        let aggregate = persisted_usage_aggregate(&reopened, model);
        assert_eq!(aggregate["estimated_cost_nanos"], 504);
        assert_eq!(aggregate["price_version"], price_version);
        assert_eq!(aggregate["input_tokens"], 12);
        assert_eq!(aggregate["output_tokens"], 3);
        assert_eq!(aggregate["confirmed_calls"], 1);
    }
    let rows: i64 = reopened
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_decision_usage WHERE usage_known = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 2);
}

#[tokio::test]
async fn native_callback_precedes_answer_decode_and_keeps_future_usage_unpriced() {
    for (model, callback_error) in [
        ("jev-1.14.0", false),
        ("future/jev:release-2099", false),
        ("future/jev:release-2099", true),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut socket = accept(&listener);
            let request = read_native_request(&mut socket);
            let mut response = valid_response(false);
            response["model"] = json!(model);
            response["answers"]["verdict"]["choice"] = json!(17);
            reply(&mut socket, "200 OK", &response.to_string());
            (listener, request)
        });
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(directory.path()).unwrap());
        let callback_store = store.clone();
        let diagnostic_store = store.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = events.clone();
        let diagnostic_events = events.clone();
        let evaluator = native(SystemOneConnection::default())
            .with_native_test_endpoint(endpoint)
            .with_usage(Arc::new(move |candidate_id, attempt, observation| {
                assert!(callback_events.lock().unwrap().is_empty());
                let observed = observation.as_ref().unwrap();
                assert_eq!(observed.served_model.as_deref(), Some(model));
                assert_eq!(
                    observed.usage,
                    JevUsage {
                        input_tokens: 12,
                        output_tokens: 3
                    }
                );
                callback_store
                    .record_burn_check_decision_observation(
                        candidate_id,
                        attempt,
                        "selected-provider",
                        ("typesafe-systemone", NATIVE_MODEL_ALIAS),
                        observation,
                        100,
                    )
                    .map_err(|error| error.to_string())?;
                assert_eq!(
                    callback_store
                        .burn_check_decision_served_model(candidate_id, attempt)
                        .unwrap()
                        .as_deref(),
                    Some(model)
                );
                callback_events.lock().unwrap().push("usage_committed");
                if callback_error {
                    Err("Synthetic callback acknowledgement failure.".into())
                } else {
                    Ok(())
                }
            }))
            .with_diagnostics(Arc::new(move |diagnostic| {
                assert_eq!(*diagnostic_events.lock().unwrap(), ["usage_committed"]);
                assert_eq!(
                    diagnostic_store
                        .burn_check_usage_summary()
                        .unwrap()
                        .confirmed_calls,
                    1
                );
                let AttemptOutcome::Failed { error, .. } = diagnostic.outcome else {
                    panic!("The invalid answer must fail");
                };
                assert_eq!(
                    error,
                    if callback_error {
                        JevError::ProgressStorageFailure
                    } else {
                        JevError::ResponseDecode
                    }
                );
                diagnostic_events.lock().unwrap().push("answer_rejected");
            }));
        let active = activation();
        let candidate = candidate(&active);
        let candidate_id = candidate
            .fingerprint
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let prepared = prepare(&evaluator, &active, json!({"rows":["protected"]})).unwrap();
        let before = serde_json::to_vec(&prepared).unwrap();
        let error = evaluator
            .evaluate_prepared(&active, &candidate, &prepared, &attempt("review", 1))
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            if callback_error {
                EvaluationFailureCode::StorageFailure
            } else {
                EvaluationFailureCode::InvalidResponse
            }
        );
        assert_eq!(error.attempts, 1);
        assert_eq!(
            *events.lock().unwrap(),
            ["usage_committed", "answer_rejected"]
        );
        assert_eq!(serde_json::to_vec(&prepared).unwrap(), before);
        let (listener, request) = server.join().unwrap();
        let expected: JevRequest =
            serde_json::from_value(prepared.decisions[0].request.clone()).unwrap();
        assert_eq!(request, serde_json::to_vec(&expected).unwrap());
        assert!(
            matches!(listener.accept(),Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        let summary = store.burn_check_usage_summary().unwrap();
        assert_eq!(
            (
                summary.confirmed_calls,
                summary.input_tokens,
                summary.output_tokens,
                summary.unknown_outcomes
            ),
            (1, 12, 3, 0)
        );
        assert_eq!(
            summary.estimated_usd.as_deref(),
            if model == "jev-1.14.0" {
                Some("$0.000000504")
            } else {
                None
            }
        );
        drop(evaluator);
        drop(store);
        let reopened = Store::open(directory.path()).unwrap();
        assert_eq!(reopened.burn_check_usage_summary().unwrap(), summary);
        assert_eq!(
            reopened
                .burn_check_decision_served_model(&candidate_id, &attempt("review", 1))
                .unwrap()
                .as_deref(),
            Some(model)
        );
        let aggregate = persisted_usage_aggregate(&reopened, model);
        assert_eq!(
            aggregate["estimated_cost_nanos"],
            if model == "jev-1.14.0" {
                json!(504)
            } else {
                Value::Null
            }
        );
        assert_eq!(
            aggregate["price_version"],
            if model == "jev-1.14.0" {
                json!("typesafe-model-catalog-2026-10-26")
            } else {
                Value::Null
            }
        );
    }
}
