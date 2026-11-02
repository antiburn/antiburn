use super::*;
use crate::jev::config::{
    SystemOneConnection, SystemOneEndpoint, SystemOneProvider, SystemOneResponseMode,
};
use crate::jev::decision_evaluator::TypeSafeDecisionEvaluator;
use antiburn_checks::engine::{CheckEngine, RunOutcome, StartOutcome};
use antiburn_checks::fit::{
    NoFitReason, PreparationBudgetKind, PreparationFailureCode, PreparationRecord,
};
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

const MODEL: &str = "synthetic-report-model";
const MODEL_DIGEST: &str = "synthetic-report-digest";

const CHECK: &str = "name: Input size report
on:
  tool_result:
decisions:
  review:
    input: {activity: '${{ message.tool.result.content }}'}
    output:
      verdict:
        type: choice
        question: Is this useful?
        criteria: {clear: Clear., concern: Concern.}
actions:
  - type: status
    input: {result: failure}
";

fn capabilities() -> ModelCapabilities {
    ModelCapabilities {
        model: MODEL.into(),
        model_revision: Some(MODEL_DIGEST.into()),
        ..ModelCapabilities::jev_default()
    }
}

fn evaluator(endpoint: &str, capabilities: ModelCapabilities) -> TypeSafeDecisionEvaluator {
    TypeSafeDecisionEvaluator::from_selected_snapshot(
        SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl(endpoint.into()),
            model: capabilities.model.clone(),
            model_revision: capabilities.model_revision.clone(),
            response_mode: SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: None,
        },
        None,
        capabilities,
    )
    .unwrap()
}

fn fixture(
    source: &str,
    evaluator: TypeSafeDecisionEvaluator,
) -> (
    Arc<Store>,
    EffectiveActivation,
    InstalledCheckDefinition,
    CheckEngine<StoreCheckStore, TypeSafeDecisionEvaluator>,
) {
    let store = store();
    let definition = definition("/workspace/demo");
    let activation = EffectiveActivation {
        id: ActivationId {
            scope: scope("/workspace/demo"),
            definition_id: definition.check_id.clone(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Project,
        source_paths: vec![],
        project_root: Some("/workspace/demo".into()),
        compiled: Arc::new(compile(source).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: Default::default(),
    };
    let adapter = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "input-size-report".into(),
            activation.id.scope.clone(),
        )
        .unwrap(),
    );
    adapter
        .activate(
            &definition.check_id,
            &LifecycleTransition::Activated(activation.clone()),
            Some(&activation),
        )
        .unwrap();
    publish_runtime_definition(&store, &definition);
    let engine = CheckEngine::new(adapter, Arc::new(evaluator));
    (store, activation, definition, engine)
}

async fn run(
    store: &Store,
    activation: &EffectiveActivation,
    engine: &CheckEngine<StoreCheckStore, TypeSafeDecisionEvaluator>,
    input: &str,
) -> antiburn_checks::engine::RunSettlement {
    engine
        .apply_source_page(
            activation,
            &local::source_page(store, activation, 1, "one", 1, true, input),
        )
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(activation, 1_001).unwrap() else {
        panic!("the input-size fixture must start");
    };
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
}

fn mock_provider(
    status: &'static str,
    body: serde_json::Value,
) -> (String, std::thread::JoinHandle<(serde_json::Value, usize)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut inference_request = None;
        let mut inference_requests = 0;
        for route in ["GET /api/tags HTTP/1.1", "POST /v1/systemone HTTP/1.1"] {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "the mock provider must receive {route}"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("the mock provider could not accept a request: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let request = loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).unwrap();
                assert!(count > 0);
                assert!(received.len() + count <= 512 * 1024);
                received.extend_from_slice(&chunk[..count]);
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
                        assert!(headers.starts_with(route), "{headers}");
                        break received[end + 4..end + 4 + length].to_vec();
                    }
                }
            };
            let (response_status, response_body) = if route.starts_with("GET ") {
                assert!(request.is_empty());
                (
                    "200 OK",
                    json!({"models": [{"name": MODEL, "digest": MODEL_DIGEST}]}).to_string(),
                )
            } else {
                inference_requests += 1;
                inference_request = Some(serde_json::from_slice(&request).unwrap());
                (status, body.to_string())
            };
            write!(socket,
                "HTTP/1.1 {response_status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            ).unwrap();
            socket.flush().unwrap();
        }
        (
            inference_request.expect("the mock provider must receive inference"),
            inference_requests,
        )
    });
    (endpoint, server)
}

fn response() -> serde_json::Value {
    json!({"model": MODEL, "answers": {"verdict": {
        "type": "choice", "choice": "clear", "probabilities": {"clear": 0.9, "concern": 0.1}, "confidence": 0.8
    }}, "usage": {"input_tokens": 12, "output_tokens": 3}})
}

#[test]
fn a_no_fit_definition_has_no_size_or_missing_fit_error_before_assessment() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (store, activation, definition, _) = fixture(
        CHECK,
        evaluator(
            &format!("http://{}", listener.local_addr().unwrap()),
            capabilities(),
        ),
    );
    assert!(activation.compiled.decisions[0].fit.is_empty());
    let report = rows(&store, &[definition]);
    assert!(report[0].diagnostics.is_empty());
    assert!(!report[0].has_started);
    assert_eq!(
        (report[0].review_failed, report[0].finding, report[0].clean),
        (0, 0, 0)
    );
}

#[tokio::test]
async fn a_no_fit_input_that_fits_runs_without_a_size_error() {
    let (endpoint, server) = mock_provider("200 OK", response());
    let (store, activation, definition, engine) =
        fixture(CHECK, evaluator(&endpoint, capabilities()));
    assert!(activation.compiled.decisions[0].fit.is_empty());
    let settled = run(&store, &activation, &engine, "Small input.").await;
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!(settled.attempts, 1);
    let (sent, inference_requests) = server.join().unwrap();
    assert_eq!(inference_requests, 1);
    assert_eq!(sent["state"]["evidence"]["activity"], "Small input.");
    let report = rows(&store, &[definition]);
    assert_eq!(report[0].review_failed, 0);
    assert!(report[0].diagnostics.is_empty());
}

#[tokio::test]
async fn a_real_no_rules_token_failure_shows_the_limit_and_useful_guidance() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut limits = capabilities();
    limits.total_input_tokens.value = Some(2048);
    limits.rendering_reserve_tokens = 0;
    let (store, activation, definition, engine) = fixture(
        CHECK,
        evaluator(
            &format!("http://{}", listener.local_addr().unwrap()),
            limits,
        ),
    );
    let settled = run(&store, &activation, &engine, &"Long input. ".repeat(2000)).await;
    assert_eq!(settled.attempts, 0);
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Rejected(failure) =
        antiburn_checks::preparation::decode_preparation_record(saved.as_bytes()).unwrap()
    else {
        panic!("the oversized input must fail before dispatch");
    };
    assert_eq!(failure.code, PreparationFailureCode::NoFit);
    assert_eq!(failure.context.no_fit_reason, Some(NoFitReason::NoRules));
    assert_eq!(
        failure.context.budget.as_ref().unwrap().kind,
        PreparationBudgetKind::EstimatedTokens
    );
    let report = rows(&store, &[definition]);
    let diagnostic = &report[0].diagnostics[0];
    assert_eq!(diagnostic.code, "input-too-large");
    assert_eq!(diagnostic.title, "The check input is too large");
    assert!(diagnostic.message.contains(
        "This check sends more information than the selected decision model can accept."
    ));
    assert!(
        diagnostic
            .message
            .contains("Shorten the decision input or select a larger decision model")
    );
    assert!(
        diagnostic
            .message
            .contains("If the check can omit some information")
    );
    assert!(diagnostic.message.contains("Nothing was sent"));
    for label in [
        "Decision",
        "Field",
        "Fitting result",
        "Estimated request size",
    ] {
        assert!(
            diagnostic
                .details
                .iter()
                .any(|detail| detail.label == label)
        );
    }
    assert!(
        diagnostic
            .details
            .iter()
            .any(|detail| detail.label == "Field" && detail.value == "decisions.review.input")
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn a_provider_context_rejection_keeps_its_own_failure_without_raw_body_text() {
    let (endpoint, server) = mock_provider(
        "400 Bad Request",
        json!({"error": "context length exceeded PROVIDER_RAW_CANARY"}),
    );
    let (store, activation, definition, engine) =
        fixture(CHECK, evaluator(&endpoint, capabilities()));
    let settled = run(&store, &activation, &engine, "Locally admitted input.").await;
    assert_eq!(settled.attempts, 1);
    let (_, inference_requests) = server.join().unwrap();
    assert_eq!(inference_requests, 1);
    let report = rows(&store, &[definition]);
    let diagnostic = &report[0].diagnostics[0];
    assert_eq!(diagnostic.code, "provider-context-rejected");
    assert!(
        diagnostic
            .message
            .contains("The provider rejected this check's input as too large.")
    );
    assert!(!diagnostic.message.contains("Nothing was sent"));
    assert!(
        report[0]
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "input-too-large")
    );
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("PROVIDER_RAW_CANARY")
    );
}

#[tokio::test]
async fn authored_fitting_success_reports_omissions_without_an_error() {
    let (endpoint, server) = mock_provider("200 OK", response());
    let mut limits = capabilities();
    limits.request_body_bytes.value = Some(4096);
    let source = CHECK.replace("    output:", "    fit: {activity: truncate}\n    output:");
    let (store, activation, definition, engine) = fixture(&source, evaluator(&endpoint, limits));
    let original = "Large permitted input. ".repeat(600);
    let settled = run(&store, &activation, &engine, &original).await;
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    let (sent, inference_requests) = server.join().unwrap();
    assert_eq!(inference_requests, 1);
    assert_ne!(sent["state"]["evidence"]["activity"], original);
    assert_eq!(
        sent["state"]["input_omissions"]["targets"][0]["path"],
        "activity"
    );
    let report = rows(&store, &[definition]);
    assert_eq!(report[0].review_failed, 0);
    assert_eq!(report[0].diagnostics.len(), 1);
    assert_eq!(report[0].diagnostics[0].code, "input-shortened");
    assert_eq!(
        report[0].diagnostics[0].severity,
        InstalledCheckDiagnosticSeverity::Info
    );
    assert!(report[0].diagnostics[0].details.iter().any(|detail| detail.label == "activity" && detail.value.contains("truncate: retained")));
}

#[test]
fn byte_and_resident_no_rules_failures_do_not_claim_a_context_token_limit() {
    for (kind, label, message) in [
        (
            PreparationBudgetKind::RequestBytes,
            "Serialized request size",
            "local request-size limit",
        ),
        (
            PreparationBudgetKind::ResidentBytes,
            "Resident request size",
            "local memory limit",
        ),
    ] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        accept(&store, &definition, "main", 1, None);
        set_latest_failure(
            &store,
            "main",
            Some(json!({
                "status": "rejected", "code": "no_fit", "detail": "RAW_TRANSPORT_CANARY", "report": null,
                "context": {"decision": "review", "field": "decisions.review.input", "no_fit_reason": "no_rules",
                    "budget": {"kind": kind, "observed": 600000, "maximum": 524288}}
            })),
            None,
        );
        let report = rows(&store, &[definition]);
        let diagnostic = &report[0].diagnostics[0];
        assert_eq!(diagnostic.code, "input-too-large");
        assert!(diagnostic.message.contains(message));
        assert!(
            !diagnostic
                .message
                .contains("selected decision model can accept")
        );
        assert!(!diagnostic.message.contains("larger decision model"));
        assert!(diagnostic.details.iter().any(|detail| detail.label == label
            && detail.value == "600000 bytes; local limit 524288 bytes"));
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("RAW_TRANSPORT_CANARY")
        );
    }
}
