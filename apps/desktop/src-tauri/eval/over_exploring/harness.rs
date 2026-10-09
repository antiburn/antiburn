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
    let Some(input) = &case.input else {
        let mut row = scoring::row(case, None, true, None);
        row["preparation_error"] = json!(case.preparation_error);
        row["elapsed_ms"] = json!(started.elapsed().as_millis());
        return row;
    };
    let context = build_jev_context(input).expect("Synthetic read context builds");
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let mut plan = OverExploringCheck
        .prepare_with_capabilities(&context, &support::provider::configuration().capabilities)
        .expect("Synthetic reads prepare");
    drop(permit);
    let sampling_overflow = plan
        .prepared
        .candidates
        .len()
        .saturating_sub(fixtures::MAX_SAMPLING_CANDIDATES);
    plan.prepared
        .candidates
        .truncate(fixtures::MAX_SAMPLING_CANDIDATES);
    let check = StableId::new("smart-check", &[b"over_exploring"]);
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: fixtures::MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1,
        judgments_per_run: MAX_TARGETS_PER_TURN,
    })
    .expect("Evaluation sampling limits are valid");
    synchronize_sampling(&plan, &mut sampling).expect("Evaluation targets synchronize");
    let mut result = OverExploringCheck
        .reduce(&plan, &[], false)
        .expect("Empty reduction succeeds");
    result.unassessed = plan.prepared.unassessed.clone();
    let mut failure = None;
    let mut missing = 0;
    'runs: while sampling
        .coverage(check)
        .expect("Synchronized check")
        .remaining
        > 0
    {
        sampling.begin_run();
        while let Some(job) = sampling.choose_job() {
            let mut selected = plan.clone();
            PreparedAssessment::select_jobs(&mut selected, std::slice::from_ref(&job))
                .expect("Selected evaluation target materializes");
            let permit = admit_jev_orchestration()
                .await
                .expect("Admit selected evaluation target");
            let outcome = match run_jev_check_prepared(
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
            .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    failure = Some(error.to_string());
                    break 'runs;
                }
            };
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
    row
}

pub(crate) async fn run() -> Result<(), String> {
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
        stopped = support::run::stop_reason(
            row["unsafe_authority_publication"] == true,
            row["missing_answers"]
                .as_u64()
                .is_some_and(|count| count > 0),
            !row["failure"].is_null(),
        );
        rows.push(row);
        if stopped.is_some() {
            break;
        }
    }
    let metrics = scoring::report(&cases, &rows);
    println!("Over Exploring: {}", metrics["overall"]);
    let report = json!({"check":"over_exploring","suite":suite,"provider":support::provider::configuration().identity(),
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
                    plan.prepared
                        .targets
                        .values()
                        .any(|target| target.reason == expected.reason
                            && target.bindings == expected.reads),
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
