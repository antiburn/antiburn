use super::provider::EvalClient;
use antiburn_local::analysis::jev::{JevError, JevRequest, JevResponse};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub(crate) async fn evaluate_request(
    client: &EvalClient,
    usage: &Arc<Mutex<RunUsage>>,
    case_id: &str,
    stage: &str,
    request: &JevRequest,
) -> Result<JevResponse, JevError> {
    let started = Instant::now();
    usage.lock().expect("usage mutex is not poisoned").requests += 1;
    let response = client.evaluate(request).await;
    let elapsed_ms = started.elapsed().as_millis();
    let capabilities = client.capabilities();
    let mut totals = usage.lock().expect("usage mutex is not poisoned");
    if let Ok(response) = &response {
        totals.input_tokens += response.usage.input_tokens;
        totals.output_tokens += response.usage.output_tokens;
    }
    let payload = serde_json::to_string(request).expect("Request serializes");
    totals.calls.push(json!({
        "case_id":case_id,
        "request_bytes":payload.len(),
        "questions":request.questions.len(),
        "question_ids":request.questions.keys().collect::<Vec<_>>(),
        "stage":stage,
        "elapsed_ms":elapsed_ms,
        "estimated_payload_tokens":capabilities.estimate_text_tokens(&payload),
        "request":std::env::var_os("ANTIBURN_EVAL_RAW").map(|_| request),
        "transport_validated":response.is_ok(),
        "response_validated":response.is_ok(),
        "response":response.as_ref().ok(),
        "status":if response.is_ok() { "strictly_validated" } else { "failed" },
        "failure":response.as_ref().err().map(ToString::to_string),
        "usage":response.as_ref().ok().map(|response| &response.usage),
    }));
    response
}

pub(crate) fn usage_report(usage: &RunUsage) -> Value {
    json!({"requests":usage.requests,"input_tokens":usage.input_tokens,
        "output_tokens":usage.output_tokens,"calls":usage.calls})
}

pub(crate) fn stop_reason(
    unsafe_publication: bool,
    missing: bool,
    failure: bool,
) -> Option<&'static str> {
    if unsafe_publication {
        Some("unsafe_publication")
    } else if missing {
        Some("missing_outcome")
    } else if failure {
        Some("execution_failure")
    } else {
        None
    }
}

#[derive(Default)]
pub(crate) struct RunUsage {
    pub(crate) requests: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) calls: Vec<Value>,
}
