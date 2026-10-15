use super::{fixtures, scoring, support};
use antiburn_local::analysis::jev::*;
use antiburn_local::checks::scope_creep::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

async fn evaluate(
    case: &fixtures::Case,
    client: &support::provider::EvalClient,
    usage: &Arc<Mutex<support::run::RunUsage>>,
) -> Value {
    let total_started = Instant::now();
    let input_started = Instant::now();
    let check = fixtures::check(case);
    let context = check.context();
    let input_context_ms = input_started.elapsed().as_millis();
    let preparation_started = Instant::now();
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let mut plan = check
        .prepare_with_capabilities(context, &support::provider::configuration().capabilities)
        .expect("Synthetic scope prepares");
    let check_preparation_ms = preparation_started.elapsed().as_millis();
    let packed = pack_work_items_with_capabilities(&plan.work_items, &plan.capabilities);
    assert!(packed.batches.len() <= 1);
    let started = Instant::now();
    let outcome = run_jev_check_prepared(
        &check,
        context,
        &mut plan,
        JevRunProgress::default(),
        permit,
        |batch| async move {
            support::run::evaluate_batch(client, usage, &case.id, "assessment", &batch).await
        },
        |_| Ok(()),
    )
    .await;
    let runner_elapsed_ms = started.elapsed().as_millis();
    let calls = usage
        .lock()
        .expect("usage mutex is not poisoned")
        .calls
        .iter()
        .filter(|call| call["case_id"] == case.id && call["stage"] == "assessment")
        .cloned()
        .collect::<Vec<_>>();
    let request_elapsed_ms = calls
        .iter()
        .filter_map(|call| call["elapsed_ms"].as_u64())
        .sum::<u64>();
    let request_count = calls.len();
    let row = match outcome {
        Ok(outcome) => {
            let result = &outcome.result;
            let observed = if !result.findings.is_empty() {
                "finding"
            } else if !result.decisions.is_empty()
                && result
                    .decisions
                    .iter()
                    .all(|decision| decision.status == ScopeCreepStatus::Clean)
            {
                "clean"
            } else {
                "unassessed"
            };
            let exact = !result.findings.is_empty()
                && result.findings.iter().all(|finding| {
                    plan.prepared.groups.iter().any(|group| {
                        finding.group_id == group.id
                            && finding.work == group.work
                            && finding.task_scope == group.task_scope
                            && finding.observation_kind == group.observation_kind
                    }) && finding.decision_probability >= DECISION_THRESHOLD
                        && finding.scope_digest == plan.prepared.scope_digest
                        && finding.revisions == REVISIONS
                        && finding.model == plan.capabilities.model
                        && finding.model_revision == plan.capabilities.model_revision
                        && finding.publication_fence == plan.prepared.publication_fence
                        && finding.source_generation == plan.prepared.source_generation
                });
            let missing = plan
                .work_items
                .iter()
                .map(|item| {
                    item.questions
                        .keys()
                        .filter(|key| {
                            outcome
                                .progress
                                .results
                                .get(&item.id)
                                .is_none_or(|result| !result.answers.contains_key(*key))
                        })
                        .count()
                })
                .sum::<usize>();
            json!({"observed":observed,"finding_count":result.findings.len(),"findings":result.findings,"binding_exact":exact,
                "actual_input_tokens":result.input_tokens,"actual_output_tokens":result.output_tokens,
                "decision_binding_exact":result.decisions.iter().map(|decision| &decision.group_id).collect::<BTreeSet<_>>() == plan.prepared.groups.iter().map(|group| &group.id).collect::<BTreeSet<_>>(),
                "missing_scheduled_answers":missing,"failure":outcome.failure.map(|error|error.to_string()),
                "decisions":result.decisions,"limitation":result.session_limitation})
        }
        Err(error) => json!({"observed":"unassessed","finding_count":0,"binding_exact":false,
            "missing_scheduled_answers":plan.work_items.len(),"failure":error.to_string()}),
    };
    let mut row = row;
    if let Some(baseline) = crate::baseline::result(&check, &plan, usage, &case.id) {
        match baseline {
            Ok(result) => row["baseline_result"] = json!(result),
            Err(error) => row["baseline_failure"] = json!(error),
        }
    }
    row["id"] = json!(case.id);
    row["expected"] = json!(case.expected);
    row["expected_semantic"] = json!(case.expected);
    row["policy_admitted"] = json!(case.policy_admitted);
    row["scenario"] = json!(case.scenario);
    row["source"] = json!(case.format);
    row["unsafe_authority_publication"] =
        json!(case.authority_fixture && row["observed"] != "unassessed");
    row["eligible"] = json!(!plan.work_items.is_empty());
    row["effective_capabilities"] = json!(plan.capabilities);
    row["request_bytes"] = json!(
        packed
            .batches
            .iter()
            .map(|batch| batch.serialized_bytes)
            .sum::<usize>()
    );
    row["estimated_aggregate_input_tokens"] = json!(
        packed
            .batches
            .iter()
            .map(|batch| plan.capabilities.estimate_text_tokens(
                &serde_json::to_string(&batch.request).expect("request serializes")
            ))
            .sum::<u64>()
    );
    row["estimated_largest_rendered_inputs"] = json!(
        packed
            .batches
            .iter()
            .map(
                |batch| estimate_jev_rendered_question_tokens(&batch.request, &plan.capabilities)
                    .ok()
            )
            .collect::<Vec<_>>()
    );
    row["selected_input"] = json!(
        packed
            .batches
            .iter()
            .map(|batch| &batch.request.state)
            .collect::<Vec<_>>()
    );
    row["elapsed_ms"] = json!(runner_elapsed_ms);
    row["stage_metrics"] = json!({
        "input_context_ms":input_context_ms,
        "check_preparation_ms":check_preparation_ms,
        "packing_request_execution_ms":request_elapsed_ms,
        "request_count":request_count,
        "reduction_and_runner_overhead_ms":runner_elapsed_ms.saturating_sub(request_elapsed_ms as u128),
        "total_ms":total_started.elapsed().as_millis()
    });
    row
}

pub(crate) async fn run() -> Result<(), String> {
    let suite = support::selection::suite();
    let cases = support::selection::select(fixtures::cases(&suite), |case| &case.id);
    let client = match support::provider::EvalClient::from_environment().await {
        Ok(client) => client,
        Err(error) => {
            let report = json!({"check":"scope_creep","suite":suite,"provider":support::provider::configuration().identity(),
                "failure":error.to_string(),"scheduled_cases":cases.len(),"executed_cases":0});
            let path = support::capture::report("scope_creep", &suite, &report);
            return Err(format!(
                "Provider setup failed: {error}; report={}",
                path.display()
            ));
        }
    };
    let usage = Arc::new(Mutex::new(support::run::RunUsage::default()));
    let mut rows = Vec::new();
    let mut stopped = None;
    for case in &cases {
        let row = evaluate(case, &client, &usage).await;
        stopped = support::run::stop_reason(
            row["unsafe_authority_publication"] == true,
            row["missing_scheduled_answers"]
                .as_u64()
                .is_some_and(|count| count > 0),
            !row["failure"].is_null(),
        );
        rows.push(row);
        if stopped.is_some() {
            break;
        }
    }
    let schedule = cases.iter().map(|case| json!({"id":case.id,"expected":case.expected,"authority_fixture":case.authority_fixture})).collect::<Vec<_>>();
    let metrics = scoring::score(&schedule, &rows);
    println!("Scope Creep: {metrics}");
    let report = json!({"check":"scope_creep","suite":suite,"provider":support::provider::configuration().identity(),
        "revisions":antiburn_local::checks::scope_creep::REVISIONS,"production_runner":true,
        "metrics":metrics,"stopped":stopped,"measurements":support::run::measurements(&rows,&usage.lock().expect("Usage lock")),"rows":rows});
    let path = support::capture::report("scope_creep", &suite, &report);
    match stopped {
        Some(reason) => Err(format!(
            "Diagnostic stopped: {reason}; report={}",
            path.display()
        )),
        None => Ok(()),
    }
}

#[test]
fn preparation_preserves_exact_work_and_unavailable_authority() {
    for suite in ["development", "controls"] {
        for case in fixtures::cases(suite) {
            let check = fixtures::check(&case);
            let plan = check.prepare(check.context()).unwrap();
            assert!(plan.shared_context.is_none());
            assert!(
                plan.work_items.iter().all(|item| item.questions.len() == 1
                    && item.questions.contains_key("scope_decision"))
            );
            assert!(!plan.prepared.groups.is_empty(), "{}", case.id);
            assert!(
                plan.prepared
                    .groups
                    .iter()
                    .all(|group| !group.work.is_empty()),
                "{}",
                case.id
            );
            assert!(plan.work_items.len() <= 2, "{}", case.id);
            if plan.work_items.is_empty() {
                let result = check.reduce(&plan, &[], false).unwrap();
                assert!(result.findings.is_empty());
                assert!(
                    result
                        .decisions
                        .iter()
                        .all(|decision| decision.status == ScopeCreepStatus::Unassessed)
                );
            }
        }
    }
}

#[test]
fn compact_scope_cases_bind_a_small_operation_and_outcome() {
    for scenario in ["compact-extra-endpoint", "compact-required-helper"] {
        let case = fixtures::cases("development")
            .into_iter()
            .find(|case| case.scenario == scenario)
            .unwrap();
        let check = fixtures::check(&case);
        let plan = check.prepare(check.context()).unwrap();
        assert_eq!(plan.prepared.groups.len(), 1, "{scenario}");
        assert_eq!(plan.work_items.len(), 1, "{scenario}");
        assert_eq!(plan.work_items[0].questions.len(), 1);
        let fields = &plan.work_items[0].window.fields;
        assert!(fields["task"].as_str().unwrap().contains(&case.task));
        assert!(fields["work"]["operation"].is_object());
        assert!(fields["work"]["outcome"].is_string());
    }
}
