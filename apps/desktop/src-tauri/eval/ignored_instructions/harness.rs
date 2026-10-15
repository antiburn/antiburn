use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use antiburn_local::analysis::jev::{
    JevCheck, JevRunProgress, admit_jev_orchestration, run_jev_check_prepared,
};
use antiburn_local::checks::ignored_instructions::{
    AssessmentResult, IgnoredInstructionsCheck, SamplingLedger, build_jev_context_with_capabilities,
};
use serde_json::{Value, json};

use super::{evidence, fixtures, scoring, support};
use fixtures::Case;
use support::run::RunUsage;

fn outcome(result: &AssessmentResult, complete: bool) -> &'static str {
    if !result.findings.is_empty() {
        "finding"
    } else if !result.pending_rules.is_empty() {
        "pending"
    } else if !complete
        || !result.unassessed_comparisons.is_empty()
        || result.coverage.selected_comparisons == 0
    {
        "unassessed"
    } else {
        "no_finding"
    }
}

async fn evaluate(
    case: &Case,
    client: &support::provider::EvalClient,
    usage: &Arc<Mutex<RunUsage>>,
) -> Value {
    let started = Instant::now();
    let input_started = Instant::now();
    let input = evidence::input(case);
    let mut effective_capabilities = support::provider::configuration().capabilities.clone();
    if let Ok(bytes) = std::env::var("ANTIBURN_IGNORED_INSTRUCTIONS_EVAL_REQUEST_BYTES") {
        let bytes = bytes
            .parse::<u64>()
            .expect("Eval request byte limit must be a positive integer");
        assert!(bytes > 0);
        let bound = effective_capabilities
            .request_body_bytes
            .value
            .map_or(bytes, |known| known.min(bytes));
        effective_capabilities.request_body_bytes =
            antiburn_local::analysis::jev::capabilities::CapabilityLimit::known(
                bound,
                antiburn_local::analysis::jev::capabilities::CapabilitySource::Manual,
            );
    }
    let capabilities = &effective_capabilities;
    let context =
        build_jev_context_with_capabilities(&input, &SamplingLedger::default(), capabilities)
            .expect("runtime fixture builds production context");
    let input_context_ms = input_started.elapsed().as_millis();
    let preparation_started = Instant::now();
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let mut plan = IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, capabilities)
        .expect("Synthetic instruction evidence prepares");
    let preparation_ms = preparation_started.elapsed().as_millis();
    let work_item_count = plan.work_items.len();
    let eligible_count = plan
        .work_items
        .iter()
        .map(|item| item.questions.len())
        .sum::<usize>();
    let request_start = usage.lock().expect("usage mutex is not poisoned").requests;
    let runner_started = Instant::now();
    let execution = run_jev_check_prepared(
        &IgnoredInstructionsCheck,
        &context,
        &mut plan,
        JevRunProgress::default(),
        permit,
        |batch| {
            let client = client.clone();
            let usage = Arc::clone(usage);
            async move {
                support::run::evaluate_batch(&client, &usage, &case.id, "assessment", &batch).await
            }
        },
        |_| Ok(()),
    )
    .await;
    let runner_ms = runner_started.elapsed().as_millis();
    let request_end = usage.lock().expect("usage mutex is not poisoned").requests;
    let call_metrics = {
        let usage = usage.lock().expect("usage mutex is not poisoned");
        let calls = usage
            .calls
            .iter()
            .filter(|call| call["case_id"] == case.id)
            .collect::<Vec<_>>();
        (
            calls.len(),
            calls
                .iter()
                .filter_map(|call| call["elapsed_ms"].as_u64())
                .sum::<u64>(),
        )
    };
    let mut row = empty_row(case);
    row["policy_admitted"] = json!(!plan.work_items.is_empty());
    row["expected_policy_admitted"] = json!(case.scenario.policy_admitted);
    row["capabilities"] = json!(capabilities);
    let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
        &plan.work_items,
        capabilities,
    );
    row["prepared_requests"] = json!(
        packed
            .batches
            .iter()
            .map(|batch| &batch.request)
            .collect::<Vec<_>>()
    );
    row["request_bytes"] = json!(
        packed
            .batches
            .iter()
            .map(|batch| batch.serialized_bytes)
            .sum::<usize>()
    );
    row["estimated_input_tokens"] = json!(
        packed
            .batches
            .iter()
            .map(|batch| capabilities
                .estimate_text_tokens(&serde_json::to_string(&batch.request).expect("request")))
            .sum::<u64>()
    );
    row["input_token_estimate_basis"] = json!("serialized_request");
    row["largest_rendered_input_tokens"] = if capabilities.uses_ollama_rendered_accounting() {
        json!(
            packed
                .batches
                .iter()
                .map(|batch| {
                    antiburn_local::analysis::jev::estimate_jev_rendered_question_tokens(
                        &batch.request,
                        capabilities,
                    )
                    .expect("Prepared request uses the discovered renderer")
                })
                .max()
        )
    } else {
        Value::Null
    };
    if let Some(baseline) =
        crate::baseline::result(&IgnoredInstructionsCheck, &plan, usage, &case.id)
    {
        match baseline {
            Ok(result) => {
                row["baseline_observed"] = json!(outcome(&result, true));
                row["baseline_result"] = json!(result);
                row["baseline_evidence_valid"] = json!(evidence::findings_valid(&result, &input));
                row["baseline_observed_references"] = json!(
                    result
                        .findings
                        .iter()
                        .map(|finding| evidence::reference(&finding.reference))
                        .collect::<BTreeSet<_>>()
                );
            }
            Err(error) => row["baseline_failure"] = json!(error),
        }
    }
    row["stage_timing_ms"] = json!({
        "synthetic_input_context": input_context_ms,
        "check_preparation": preparation_ms,
        "execution_runner_including_packing_and_reduction": runner_ms,
    });
    row["stage_counts"] = json!({
        "work_items": work_item_count,
        "eligible_questions": eligible_count,
        "requests": request_end.saturating_sub(request_start),
        "measured_assessment_calls": call_metrics.0,
        "request_elapsed_ms": call_metrics.1,
    });
    row["policy_limited"] =
        json!(plan.work_items.is_empty() && !case.scenario.expected.actions.is_empty());
    match execution {
        Ok(execution) => {
            let missing = plan
                .work_items
                .iter()
                .map(|item| {
                    item.questions
                        .keys()
                        .filter(|key| {
                            execution
                                .progress
                                .results
                                .get(&item.id)
                                .is_none_or(|result| !result.answers.contains_key(*key))
                        })
                        .count()
                })
                .sum::<usize>();
            row["missing_answers"] = json!(missing.max(execution.progress.failed_item_ids.len()));
            row["observed"] = json!(outcome(&execution.result, execution.complete));
            row["actual_input_tokens"] = json!(execution.result.input_tokens);
            row["actual_output_tokens"] = json!(execution.result.output_tokens);
            row["finding_count"] = json!(execution.result.findings.len());
            row["clean_eligible"] = json!(
                execution.result.findings.is_empty()
                    && execution.result.unassessed_comparisons.is_empty()
                    && execution.result.coverage.limitations.is_empty()
                    && execution.result.coverage.unselected_pairs == 0
                    && execution.result.coverage.skipped_rules.is_empty()
                    && execution.result.coverage.skipped_actions.is_empty()
                    && execution.result.coverage.selected_comparisons > 0
            );
            if !execution.result.findings.is_empty() {
                row["observed_reason"] = json!("ignored_instruction_violation");
            }
            row["observed_references"] = json!(
                execution
                    .result
                    .findings
                    .iter()
                    .map(|finding| evidence::reference(&finding.reference))
                    .collect::<BTreeSet<_>>()
            );
            row["evidence_valid"] = json!(evidence::findings_valid(&execution.result, &input));
            row["failure"] = json!(execution.failure.map(|error| error.to_string()));
            row["execution_complete"] = json!(execution.complete);
            if std::env::var_os("ANTIBURN_EVAL_RAW").is_some() {
                row["result"] = json!(execution.result);
                row["answers"] = json!(execution.progress.results);
            }
        }
        Err(error) => {
            row["failure"] = json!(error.to_string());
            row["missing_answers"] = json!(plan.work_items.len());
        }
    }
    row["stage_timing_ms"]["post_runner_result_and_reporting"] = json!(
        started
            .elapsed()
            .as_millis()
            .saturating_sub(input_context_ms)
            .saturating_sub(preparation_ms)
            .saturating_sub(runner_ms)
    );
    row["elapsed_ms"] = json!(started.elapsed().as_millis());
    row
}

fn empty_row(case: &Case) -> Value {
    let mut row = fixtures::inventory(case);
    row.as_object_mut().expect("case inventory").extend([
        ("observed".to_owned(), Value::Null),
        ("observed_reason".to_owned(), Value::Null),
        ("observed_references".to_owned(), json!([])),
        ("evidence_valid".to_owned(), json!(false)),
        ("failure".to_owned(), Value::Null),
        ("elapsed_ms".to_owned(), json!(0)),
    ]);
    row
}

pub(crate) async fn run() -> Result<(), String> {
    let suite = support::selection::suite();
    let cases = support::selection::select(fixtures::cases(&suite), |case| &case.id);
    let client = match support::provider::EvalClient::from_environment().await {
        Ok(client) => client,
        Err(error) => {
            let report = json!({"check":"ignored_instructions","suite":suite,"provider":support::provider::configuration().identity(),
                "failure":error.to_string(),"scheduled_cases":cases.len(),"executed_cases":0});
            let path = support::capture::report("ignored_instructions", &suite, &report);
            return Err(format!(
                "Provider setup failed: {error}; report={}",
                path.display()
            ));
        }
    };
    let usage = Arc::new(Mutex::new(RunUsage::default()));
    let mut rows = Vec::new();
    let mut stopped = None;
    for case in &cases {
        let row = evaluate(case, &client, &usage).await;
        let invalid_evidence = row["observed"] == "finding" && row["evidence_valid"] != true;
        let unsafe_publication = invalid_evidence
            || (case.scenario.authority_control
                && matches!(row["observed"].as_str(), Some("finding" | "no_finding")));
        stopped = support::run::stop_reason(
            unsafe_publication,
            row["observed"].is_null()
                || row["missing_answers"]
                    .as_u64()
                    .is_some_and(|count| count > 0),
            !row["failure"].is_null(),
        );
        eprintln!(
            "{} expected={} observed={}",
            case.id, case.scenario.expected.verdict, row["observed"]
        );
        rows.push(row);
        if stopped.is_some() {
            break;
        }
    }
    let inventory = cases.iter().map(fixtures::inventory).collect::<Vec<_>>();
    let metrics = scoring::report(&inventory, &rows);
    println!("Ignored Instructions: {}", metrics["overall"]);
    let report = json!({"check":"ignored_instructions","suite":suite,"provider":support::provider::configuration().identity(),
        "revisions":IgnoredInstructionsCheck.revisions(),"production_runner":true,
        "cases":inventory,"metrics":metrics,"stopped":stopped,"measurements":support::run::measurements(&rows,&usage.lock().expect("Usage lock")),"rows":rows});
    let path = support::capture::report("ignored_instructions", &suite, &report);
    match stopped {
        Some(reason) => Err(format!(
            "Diagnostic stopped: {reason}; report={}",
            path.display()
        )),
        None => Ok(()),
    }
}

#[test]
fn selected_capabilities_reach_preparation_and_packing() {
    let case = fixtures::select("core", Some("command-ban:claude"))
        .expect("case")
        .remove(0);
    let input = evidence::input(&case);
    let capabilities = &support::provider::configuration().capabilities;
    let context =
        build_jev_context_with_capabilities(&input, &SamplingLedger::default(), capabilities)
            .expect("context");
    let plan = IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, capabilities)
        .expect("plan");
    assert_eq!(&plan.capabilities, capabilities);
    assert_eq!(
        plan.work_items.len(),
        1,
        "The capability test must admit evidence"
    );
    let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
        &plan.work_items,
        &plan.capabilities,
    );
    assert!(packed.skipped_item_ids.is_empty());
    assert_eq!(packed.batches.len(), 1);
    for batch in packed.batches {
        assert_eq!(batch.request.model, capabilities.model);
        antiburn_local::analysis::jev::validate_jev_request_with_capabilities(
            &batch.request,
            capabilities,
        )
        .expect("provider limits");
        let payload = serde_json::to_string(&batch.request).expect("request");
        assert!(!payload.contains(&case.id));
        assert!(!payload.contains("expected_references"));
        assert!(!payload.contains("expected_reason"));
    }
}

#[test]
fn compact_inventory_admission_matches_the_production_policy() {
    use antiburn_local::analysis::jev::capabilities::{
        CapabilityLimit, CapabilitySource, ModelCapabilities, OLLAMA_TEV1_ESTIMATOR,
        TokenizerIdentity,
    };
    let mut limits = ModelCapabilities::jev_default();
    limits.runtime_context_tokens = CapabilityLimit::known(2048, CapabilitySource::RuntimeMetadata);
    limits.rendering_reserve_tokens = 304;
    limits.tokenizer = Some(TokenizerIdentity::ConservativeEstimator(
        OLLAMA_TEV1_ESTIMATOR.to_owned(),
    ));
    for case in fixtures::select("compact", None).unwrap() {
        let input = evidence::input(&case);
        let context =
            build_jev_context_with_capabilities(&input, &SamplingLedger::default(), &limits)
                .unwrap();
        let plan = IgnoredInstructionsCheck
            .prepare_with_capabilities(&context, &limits)
            .unwrap();
        assert_eq!(
            !plan.work_items.is_empty(),
            case.scenario.policy_admitted,
            "{}",
            case.id
        );
        let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
            &plan.work_items,
            &limits,
        );
        assert!(packed.skipped_item_ids.is_empty(), "{}", case.id);
        assert_eq!(
            packed.batches.len(),
            usize::from(case.scenario.policy_admitted),
            "{}",
            case.id
        );
        for batch in packed.batches {
            antiburn_local::analysis::jev::validate_jev_request_with_capabilities(
                &batch.request,
                &limits,
            )
            .unwrap();
            assert!(batch.serialized_bytes <= 6144);
            assert!(
                antiburn_local::analysis::jev::estimate_jev_rendered_question_tokens(
                    &batch.request,
                    &limits
                )
                .unwrap()
                    <= 1536
            );
        }
    }
}

#[test]
fn offline_normal_reduced_and_minimum_fit_profiles_use_production_preparation() {
    use antiburn_local::analysis::jev::capabilities::{
        CapabilityLimit, CapabilitySource, ModelCapabilities, OLLAMA_TEV1_ESTIMATOR,
        TokenizerIdentity,
    };
    use antiburn_local::analysis::jev::{
        pack_work_items_with_capabilities, validate_jev_request_with_capabilities,
    };
    for mut case in fixtures::select("core", Some("command-ban")).unwrap() {
        case.scenario.events.insert(
            0,
            fixtures::Event::Text((
                fixtures::Kind::Assistant,
                "Inspect the requested release command.".to_owned(),
            )),
        );
        let input = evidence::input(&case);
        let mut normal = ModelCapabilities::jev_default();
        normal.rendering_reserve_tokens = 304;
        normal.tokenizer = Some(TokenizerIdentity::ConservativeEstimator(
            OLLAMA_TEV1_ESTIMATOR.to_owned(),
        ));
        normal.runtime_context_tokens = CapabilityLimit::known(2048, CapabilitySource::Manual);
        let prepare = |limits: &ModelCapabilities| {
            let context =
                build_jev_context_with_capabilities(&input, &SamplingLedger::default(), limits)
                    .unwrap();
            IgnoredInstructionsCheck
                .prepare_with_capabilities(&context, limits)
                .unwrap()
        };
        let normal_plan = prepare(&normal);
        let packed = pack_work_items_with_capabilities(&normal_plan.work_items, &normal);
        assert!(packed.skipped_item_ids.is_empty());
        assert_eq!(packed.batches.len(), 1);
        let normal_bytes = packed.batches[0].serialized_bytes;
        let mut reduced = normal.clone();
        reduced.request_body_bytes.value = Some(normal_bytes as u64 - 1);
        let reduced_plan = prepare(&reduced);
        let reduced_packed = pack_work_items_with_capabilities(&reduced_plan.work_items, &reduced);
        assert!(reduced_packed.skipped_item_ids.is_empty());
        assert_eq!(reduced_packed.batches.len(), 1);
        assert!(reduced_packed.batches[0].serialized_bytes < normal_bytes);
        let request = &reduced_packed.batches[0].request;
        validate_jev_request_with_capabilities(request, &reduced).unwrap();
        assert!(
            serde_json::to_string(request)
                .unwrap()
                .contains("npm publish --access public")
        );
        assert!(
            serde_json::to_string(request)
                .unwrap()
                .contains("Do not request")
        );
        let mut minimum = normal;
        minimum.runtime_context_tokens.value = Some(64);
        let minimum_plan = prepare(&minimum);
        assert!(minimum_plan.work_items.is_empty());
        assert!(
            pack_work_items_with_capabilities(&minimum_plan.work_items, &minimum)
                .batches
                .is_empty()
        );
    }
}
