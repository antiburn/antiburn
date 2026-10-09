use super::provider::EvalClient;
use antiburn_local::analysis::jev::{JevError, JevRequestBatch, JevResponse};
use serde_json::Value;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub(crate) async fn evaluate_batch(
    client: &EvalClient,
    usage: &Arc<Mutex<RunUsage>>,
    case_id: &str,
    stage: &str,
    batch: &JevRequestBatch,
) -> Result<JevResponse, JevError> {
    let started = Instant::now();
    usage.lock().expect("usage mutex is not poisoned").requests += 1;
    let response = client.evaluate(&batch.request, false).await;
    let mut totals = usage.lock().expect("usage mutex is not poisoned");
    if let Ok(response) = &response {
        totals.input_tokens += response.usage.input_tokens;
        totals.output_tokens += response.usage.output_tokens;
    }
    totals.calls.push(json!({
        "case_id":case_id,
        "request_bytes":batch.serialized_bytes,
        "questions":batch.request.questions.len(),
        "work_item_ids":batch.work_item_ids,
        "stage":stage,
        "elapsed_ms":started.elapsed().as_millis(),
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

pub(crate) fn measurements(rows: &[Value], usage: &RunUsage) -> Value {
    let mut latency = rows
        .iter()
        .filter_map(|row| row["elapsed_ms"].as_u64())
        .collect::<Vec<_>>();
    latency.sort_unstable();
    let percentile = |percent: usize| {
        latency
            .get((latency.len() * percent).div_ceil(100).saturating_sub(1))
            .copied()
    };
    let mut measured_usage = usage_report(usage);
    if std::env::var_os("ANTIBURN_EVAL_RAW").is_none() {
        measured_usage
            .as_object_mut()
            .expect("Usage object")
            .remove("calls");
    }
    json!({"usage":measured_usage,
        "latency_ms":{"measured_cases":latency.len(),"p50":percentile(50),"p95":percentile(95),"max":latency.last(),"sum":latency.iter().sum::<u64>()},
        "unknown_usage_requests":usage.calls.iter().filter(|call|call["usage"].is_null()).count(),
        "uncertainty":"One live execution per scheduled case. No population accuracy or latency guarantee.",
    })
}

#[derive(Default)]
pub(crate) struct RunUsage {
    pub(crate) requests: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) calls: Vec<Value>,
}

impl RunUsage {
    pub(crate) fn from_calls(calls: Vec<Value>) -> Self {
        Self {
            requests: calls.len() as u64,
            input_tokens: calls
                .iter()
                .filter_map(|call| call["usage"]["input_tokens"].as_u64())
                .sum(),
            output_tokens: calls
                .iter()
                .filter_map(|call| call["usage"]["output_tokens"].as_u64())
                .sum(),
            calls,
        }
    }
}

#[test]
fn usage_aggregation_keeps_failed_calls_in_the_request_denominator() {
    let usage = RunUsage::from_calls(vec![
        json!({"usage":{"input_tokens":12,"output_tokens":3}}),
        json!({"usage":null,"failure":"unknown provider outcome"}),
    ]);
    let report = measurements(&[], &usage);
    assert_eq!(report["usage"]["requests"], 2);
    assert_eq!(report["usage"]["input_tokens"], 12);
    assert_eq!(report["usage"]["output_tokens"], 3);
    assert_eq!(report["unknown_usage_requests"], 1);
    assert_eq!(report["latency_ms"]["p95"], Value::Null);
}
