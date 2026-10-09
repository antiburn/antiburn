use super::provider::EvalClient;
use antiburn_local::analysis::jev::{
    JevAnswer, JevError, JevQuestion, JevRequest, JevRequestBatch, JevResponse, JevWorkItemResult,
    unpack_jev_response_with_capabilities, validate_jev_request_with_capabilities,
};
use serde_json::Value;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn baseline_request(
    current: &JevRequest,
    templates: &std::collections::BTreeMap<String, JevQuestion>,
) -> JevRequest {
    let mut request = current.clone();
    for question in request.questions.values_mut() {
        let serialized = serde_json::to_string(question).expect("Question serializes");
        let key = if templates.contains_key("default") {
            "default"
        } else {
            let JevQuestion::Choice { instructions, .. } = &*question else {
                panic!("Baseline comparison requires Choice questions")
            };
            let index = instructions["context_path"]
                .as_str()
                .expect("Packed context route")
                .split("work_items[")
                .nth(1)
                .and_then(|suffix| suffix.split(']').next())
                .expect("Packed work item index")
                .parse::<usize>()
                .expect("Numeric work item index");
            match current.state["work_items"][index]["context"]["reason"]
                .as_str()
                .expect("Read reason")
            {
                "unrelated_files" => "unrelated",
                "excessive_file_breadth" => "breadth",
                "excessive_within_file_reading" => "extent",
                _ => panic!("Unsupported read reason"),
            }
        };
        let target_index = serialized
            .split("instruction_targets[")
            .nth(1)
            .and_then(|suffix| suffix.split(']').next());
        let mut template = templates[key].clone();
        if let (Some(index), JevQuestion::Choice { instructions, .. }) =
            (target_index, &mut template)
            && let Some(text) = instructions.as_str()
        {
            *instructions = json!(text.replace(
                "instruction_targets[0]",
                &format!("instruction_targets[{index}]")
            ));
        }
        if let (
            JevQuestion::Choice {
                instructions: current,
                ..
            },
            JevQuestion::Choice {
                instructions: old, ..
            },
        ) = (&*question, &mut template)
            && current.get("context_path").is_some()
        {
            let mut routed = current.clone();
            routed["question"] = old.clone();
            *old = routed;
        }
        *question = template;
    }
    request
}

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
    let elapsed_ms = started.elapsed().as_millis();
    let capabilities = client.capabilities();
    let baseline = match std::env::var_os("ANTIBURN_EVAL_BASELINE_QUESTIONS") {
        Some(path) => {
            assert!(
                matches!(
                    super::provider::configuration().connection.provider,
                    super::config::SystemOneProvider::Jev
                        | super::config::SystemOneProvider::Ollama
                ),
                "Baseline comparisons require a native Jev or Ollama preset"
            );
            let templates: std::collections::BTreeMap<String, JevQuestion> =
                serde_json::from_slice(
                    &std::fs::read(path).expect("Read baseline question fixture"),
                )
                .expect("Baseline questions match the production schema");
            let request = baseline_request(&batch.request, &templates);
            let payload = serde_json::to_string(&request).expect("Baseline request serializes");
            let fit = validate_jev_request_with_capabilities(&request, capabilities);
            let baseline_started = Instant::now();
            let baseline_response = match &fit {
                Ok(_) => Some(client.evaluate(&request, false).await),
                Err(_) => None,
            };
            if let Some(Ok(response)) = &baseline_response {
                let mut mapped = response.clone();
                for (id, answer) in &mut mapped.answers {
                    if let (
                        JevAnswer::Choice { probabilities, .. },
                        JevQuestion::Choice { criteria, .. },
                    ) = (answer, &batch.request.questions[id])
                    {
                        for key in criteria.keys() {
                            probabilities.entry(key.clone()).or_insert(0.0);
                        }
                    }
                }
                let mapped = unpack_jev_response_with_capabilities(batch, &mapped, capabilities);
                let mut totals = usage.lock().expect("usage mutex is not poisoned");
                match mapped {
                    Ok(results) => totals
                        .baseline_results
                        .entry(case_id.into())
                        .or_default()
                        .extend(results),
                    Err(error) => {
                        totals
                            .baseline_errors
                            .insert(case_id.into(), error.to_string());
                    }
                }
            } else {
                let error = baseline_response
                    .as_ref()
                    .and_then(|response| response.as_ref().err())
                    .or(fit.as_ref().err())
                    .expect("Baseline did not dispatch or validate");
                usage
                    .lock()
                    .expect("usage mutex is not poisoned")
                    .baseline_errors
                    .insert(case_id.into(), error.to_string());
            }
            Some(
                json!({"fits":fit.is_ok(),"fit_error":fit.err().map(|error|error.to_string()),
                "request_bytes":payload.len(),"estimated_payload_tokens":capabilities.estimate_text_tokens(&payload),
                "dispatched":baseline_response.is_some(),"elapsed_ms":baseline_started.elapsed().as_millis(),
                "request":std::env::var_os("ANTIBURN_EVAL_RAW").map(|_| &request),
                "response":baseline_response.as_ref().and_then(|response|response.as_ref().ok()),
                "failure":baseline_response.as_ref().and_then(|response|response.as_ref().err()).map(ToString::to_string)}),
            )
        }
        None => None,
    };
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
        "elapsed_ms":elapsed_ms,
        "estimated_payload_tokens":capabilities.estimate_text_tokens(&serde_json::to_string(&batch.request).expect("Request serializes")),
        "request":std::env::var_os("ANTIBURN_EVAL_RAW").map(|_| &batch.request),
        "baseline":baseline,
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
    let baseline_calls = usage
        .calls
        .iter()
        .filter_map(|call| call["baseline"].as_object())
        .collect::<Vec<_>>();
    json!({"requests":usage.requests,"input_tokens":usage.input_tokens,
    "output_tokens":usage.output_tokens,"calls":usage.calls,
    "baseline_usage":{
        "requests":baseline_calls.iter().filter(|call| call["dispatched"] == true).count(),
        "input_tokens":baseline_calls.iter().filter_map(|call| call["response"]["usage"]["input_tokens"].as_u64()).sum::<u64>(),
        "output_tokens":baseline_calls.iter().filter_map(|call| call["response"]["usage"]["output_tokens"].as_u64()).sum::<u64>(),
        "unknown_usage_requests":baseline_calls.iter().filter(|call| call["dispatched"] == true && call["response"].is_null()).count(),
    }})
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
    pub(crate) baseline_results: std::collections::BTreeMap<String, Vec<JevWorkItemResult>>,
    pub(crate) baseline_errors: std::collections::BTreeMap<String, String>,
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
            ..Self::default()
        }
    }
}

#[test]
fn read_baseline_uses_routed_reason_instead_of_prompt_wording() {
    use std::collections::BTreeMap;
    let reasons = [
        "unrelated_files",
        "excessive_file_breadth",
        "excessive_within_file_reading",
    ];
    let keys = ["unrelated", "breadth", "extent"];
    let current = JevRequest {
        model: "jev-test".into(),
        state: json!({"work_items":reasons.iter().map(|reason| json!({"context":{"reason":reason}})).collect::<Vec<_>>()}),
        questions: keys.iter().enumerate().map(|(index,key)| (key.to_string(), JevQuestion::Choice {
            instructions: json!({"context_path":format!("Use work_items[{index}].context"), "question":"Revised wording"}),
            criteria: BTreeMap::from([("new".into(), json!("new option"))]),
        })).collect(),
    };
    let templates = keys
        .iter()
        .map(|key| {
            (
                key.to_string(),
                JevQuestion::Choice {
                    instructions: json!(key),
                    criteria: BTreeMap::from([("old".into(), json!("old option"))]),
                },
            )
        })
        .collect();
    let baseline = baseline_request(&current, &templates);
    for key in keys {
        let JevQuestion::Choice { instructions, .. } = &baseline.questions[key] else {
            panic!("Choice question")
        };
        assert_eq!(instructions["question"], key);
    }
}

#[test]
fn baseline_substitution_preserves_state_routes_ids_and_target_index() {
    use std::collections::BTreeMap;
    let current = JevRequest {
        model: "jev-test".into(),
        state: json!({"work_items":[{"context":{"instruction_targets":["first", "second", "third"]}}]}),
        questions: BTreeMap::from([(
            "q0".into(),
            JevQuestion::Choice {
                instructions: json!({"context_path":"work_items[0].context", "shared_context_path":"shared_context", "question":"Use instruction_targets[2].instruction."}),
                criteria: BTreeMap::from([("new".into(), json!("new option"))]),
            },
        )]),
    };
    let templates = BTreeMap::from([(
        "default".into(),
        JevQuestion::Choice {
            instructions: json!("Old instruction_targets[0].instruction."),
            criteria: BTreeMap::from([("old".into(), json!("old option"))]),
        },
    )]);
    let baseline = baseline_request(&current, &templates);
    assert_eq!(baseline.state, current.state);
    assert_eq!(baseline.model, current.model);
    assert_eq!(
        baseline.questions.keys().collect::<Vec<_>>(),
        current.questions.keys().collect::<Vec<_>>()
    );
    let JevQuestion::Choice {
        instructions,
        criteria,
    } = &baseline.questions["q0"]
    else {
        panic!("Choice question")
    };
    assert_eq!(instructions["context_path"], "work_items[0].context");
    assert_eq!(instructions["shared_context_path"], "shared_context");
    assert_eq!(
        instructions["question"],
        "Old instruction_targets[2].instruction."
    );
    assert_eq!(criteria.keys().collect::<Vec<_>>(), vec!["old"]);
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
