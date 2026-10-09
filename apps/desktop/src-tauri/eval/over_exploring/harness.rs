use super::{fixtures, scoring, support};
use antiburn_local::analysis::jev::{
    JevCheck, JevRunProgress, admit_jev_orchestration, run_jev_check_prepared,
};
use antiburn_local::checks::over_exploring::{
    MAX_TARGETS_PER_TURN, OverExploringCheck, PreparedAssessment, build_jev_context,
    synchronize_sampling,
};
use antiburn_local::checks::sampling::{SamplingLimits, SamplingProgress, StableId};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Instant;

async fn evaluate(
    case: &fixtures::Case,
    client: &support::provider::EvalClient,
    usage: &Arc<Mutex<support::run::RunUsage>>,
) -> Value {
    let started = Instant::now();
    let request_start = usage.lock().expect("Usage lock").requests;
    let Some(input) = &case.input else {
        let mut row = scoring::row(case, None, true, None);
        row["preparation_error"] = json!(case.preparation_error);
        row["elapsed_ms"] = json!(started.elapsed().as_millis());
        row["stage_timings"] = json!({"total_ms":started.elapsed().as_millis()});
        return row;
    };
    let context_started = Instant::now();
    let context = build_jev_context(input).expect("Synthetic read context builds");
    let context_ms = context_started.elapsed().as_millis();
    let preparation_started = Instant::now();
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let mut plan = OverExploringCheck
        .prepare_with_capabilities(&context, &support::provider::configuration().capabilities)
        .expect("Synthetic reads prepare");
    let preparation_ms = preparation_started.elapsed().as_millis();
    drop(permit);
    let sampling_overflow = plan
        .prepared
        .candidates
        .len()
        .saturating_sub(fixtures::MAX_SAMPLING_CANDIDATES);
    plan.prepared
        .candidates
        .truncate(fixtures::MAX_SAMPLING_CANDIDATES);
    let sampling_started = Instant::now();
    let check = StableId::new("smart-check", &[b"over_exploring"]);
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: fixtures::MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1,
        judgments_per_run: MAX_TARGETS_PER_TURN,
    })
    .expect("Evaluation sampling limits are valid");
    synchronize_sampling(&plan, &mut sampling).expect("Evaluation targets synchronize");
    let sampling_ms = sampling_started.elapsed().as_millis();
    let reduction_started = Instant::now();
    let mut result = OverExploringCheck
        .reduce(&plan, &[], false)
        .expect("Empty reduction succeeds");
    let initial_reduction_ms = reduction_started.elapsed().as_millis();
    let mut baseline_result = result.clone();
    let mut baseline_failure = None;
    result.unassessed = plan.prepared.unassessed.clone();
    let mut failure = None;
    let mut missing = 0;
    let mut job_execution_ms = 0u128;
    let mut selected_jobs = 0u64;
    'runs: while sampling
        .coverage(check)
        .expect("Synchronized check")
        .remaining
        > 0
    {
        sampling.begin_run();
        while let Some(job) = sampling.choose_job() {
            let job_started = Instant::now();
            selected_jobs += 1;
            let mut selected = plan.clone();
            PreparedAssessment::select_jobs(&mut selected, std::slice::from_ref(&job))
                .expect("Selected evaluation target materializes");
            let permit = admit_jev_orchestration()
                .await
                .expect("Admit selected evaluation target");
            let execution = run_jev_check_prepared(
                &OverExploringCheck,
                &context,
                &mut selected,
                JevRunProgress::default(),
                permit,
                |batch| async move {
                    support::run::evaluate_batch(client, usage, &case.id, "assessment", &batch)
                        .await
                },
                |_| Ok(()),
            )
            .await;
            job_execution_ms += job_started.elapsed().as_millis();
            let outcome = match execution {
                Ok(outcome) => outcome,
                Err(error) => {
                    failure = Some(error.to_string());
                    break 'runs;
                }
            };
            if let Some(baseline) =
                crate::baseline::result(&OverExploringCheck, &selected, usage, &case.id)
            {
                match baseline {
                    Ok(result) => {
                        baseline_result.findings.extend(result.findings);
                        baseline_result
                            .completed_work_item_ids
                            .extend(result.completed_work_item_ids);
                        baseline_result.unassessed.extend(
                            result
                                .unassessed
                                .into_iter()
                                .filter(|item| item.work_item_id.is_some()),
                        );
                    }
                    Err(error) => baseline_failure = Some(error),
                }
            }
            missing += selected
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
                                .is_none_or(|answer| !answer.answers.contains_key(*key))
                        })
                        .count()
                })
                .sum::<usize>();
            result.findings.extend(outcome.result.findings.clone());
            result
                .completed_work_item_ids
                .extend(outcome.result.completed_work_item_ids.clone());
            result
                .completed_episode_ids
                .extend(outcome.result.completed_episode_ids.clone());
            result
                .clean_episode_ids
                .extend(outcome.result.clean_episode_ids.clone());
            result.unassessed.extend(
                outcome
                    .result
                    .unassessed
                    .iter()
                    .filter(|item| item.work_item_id.is_some())
                    .cloned(),
            );
            if let Some(error) = outcome.failure {
                failure = Some(error.to_string());
                break 'runs;
            }
            if let Err(error) =
                selected
                    .prepared
                    .record_completion(&outcome.result, &job, &mut sampling)
            {
                failure = Some(format!("Target did not finish: {error:?}"));
                break 'runs;
            }
        }
    }
    let coverage = sampling.coverage(check).expect("Synchronized check");
    result.coverage.selected_items = coverage.completed;
    result.coverage.not_selected_items = coverage.remaining + sampling_overflow;
    if sampling_overflow > 0 {
        result.coverage.processing_limit_reached = true;
        result
            .coverage
            .limitations
            .push("sampling_inventory_limit".into());
    }
    let mut row = scoring::row(
        case,
        Some(&result),
        coverage.remaining == 0 && sampling_overflow == 0 && failure.is_none(),
        failure,
    );
    row["missing_answers"] = json!(missing);
    row["elapsed_ms"] = json!(started.elapsed().as_millis());
    row["stage_timings"] = json!({
        "context_ms":context_ms,
        "preparation_ms":preparation_ms,
        "sampling_setup_ms":sampling_ms,
        "selected_job_runner_ms":job_execution_ms,
        "selected_jobs":selected_jobs,
        "provider_requests":usage.lock().expect("Usage lock").requests.saturating_sub(request_start),
        "initial_reduction_ms":initial_reduction_ms,
        "total_ms":started.elapsed().as_millis()
    });
    row["production_runner"] = json!(true);
    if std::env::var_os("ANTIBURN_EVAL_BASELINE_QUESTIONS").is_some() {
        baseline_result.coverage = result.coverage.clone();
        row["baseline_row"] = scoring::row(
            case,
            Some(&baseline_result),
            coverage.remaining == 0 && baseline_failure.is_none(),
            baseline_failure,
        );
    }
    row
}

pub(crate) async fn run() -> Result<(), String> {
    run_with_benchmark_policy(false).await
}

pub(crate) async fn run_benchmark_session() -> Result<(), String> {
    run_with_benchmark_policy(true).await
}

async fn run_with_benchmark_policy(continue_after_quality: bool) -> Result<(), String> {
    let suite = support::selection::suite();
    let cases = support::selection::select(fixtures::cases(&suite), |case| &case.id);
    let client = match support::provider::EvalClient::from_environment().await {
        Ok(client) => client,
        Err(error) => {
            let report = json!({"check":"over_exploring","suite":suite,"provider":support::provider::configuration().identity(),
                "failure":error.to_string(),"scheduled_cases":cases.len(),"executed_cases":0});
            let path = support::capture::report("over_exploring", &suite, &report);
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
        stopped = if continue_after_quality {
            support::run::stop_reason(
                false,
                row["missing_answers"]
                    .as_u64()
                    .is_some_and(|count| count > 0),
                !row["failure"].is_null(),
            )
        } else {
            support::run::stop_reason(
                row["unsafe_authority_publication"] == true,
                row["missing_answers"]
                    .as_u64()
                    .is_some_and(|count| count > 0),
                !row["failure"].is_null(),
            )
        };
        rows.push(row);
        if stopped.is_some() {
            break;
        }
    }
    let metrics = scoring::report(&cases, &rows);
    println!("Over Exploring: {}", metrics["overall"]);
    let report = json!({"check":"over_exploring","suite":suite,"provider":support::provider::configuration().identity(),
        "revisions":OverExploringCheck.revisions(),"production_runner":true,
        "metrics":metrics,"stopped":stopped,"measurements":support::run::measurements(&rows,&usage.lock().expect("Usage lock"))});
    let path = support::capture::report("over_exploring", &suite, &report);
    match stopped {
        Some(reason) => Err(format!(
            "Diagnostic stopped: {reason}; report={}",
            path.display()
        )),
        None => Ok(()),
    }
}

#[test]
fn labels_bind_to_prepared_read_targets_and_missing_answers_stay_unassessed() {
    for suite in ["development", "controls"] {
        for case in fixtures::cases(suite) {
            let Some(input) = &case.input else {
                assert!(case.expected.is_empty());
                continue;
            };
            let context = build_jev_context(input).unwrap();
            let plan = OverExploringCheck.prepare(&context).unwrap();
            for item in &plan.work_items {
                assert_eq!(item.questions.len(), 1, "{}", case.id);
                let antiburn_local::analysis::jev::JevQuestion::Choice { criteria, .. } =
                    &item.questions[antiburn_local::checks::over_exploring::QUESTION_ID]
                else {
                    panic!("{}: assessment must use Choice", case.id);
                };
                assert_eq!(
                    criteria.keys().map(String::as_str).collect::<Vec<_>>(),
                    ["justified_or_minor", "likely_excess", "uncertain"],
                    "{}",
                    case.id
                );
            }
            for expected in &case.expected {
                assert!(
                    expected.reads.iter().all(|read| plan
                        .prepared
                        .targets
                        .values()
                        .any(|target| target.reason == expected.reason
                            && target.bindings.contains(read))),
                    "{}",
                    case.id
                );
            }
            let result = OverExploringCheck.reduce(&plan, &[], false).unwrap();
            assert!(result.findings.is_empty());
            assert!(result.clean_episode_ids.is_empty());
        }
    }
}
