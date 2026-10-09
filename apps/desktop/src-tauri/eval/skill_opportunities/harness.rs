use super::{fixtures, scoring, support};
use antiburn_local::analysis::jev::{
    JevCheck, JevRunProgress, admit_jev_orchestration, run_jev_check_prepared,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Instant;

async fn evaluate(
    case: &fixtures::Case,
    client: &support::provider::EvalClient,
    usage: &Arc<Mutex<support::run::RunUsage>>,
) -> Value {
    let total_started = Instant::now();
    let context_started = Instant::now();
    let context = case.check.session_context();
    let context_construction_ms = context_started.elapsed().as_millis();
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let preparation_started = Instant::now();
    let mut plan = case
        .check
        .prepare_with_capabilities(&context, &support::provider::configuration().capabilities)
        .expect("Synthetic skill work prepares");
    let preparation_ms = preparation_started.elapsed().as_millis();
    let work_item_count = plan.work_items.len();
    let request_count_before = usage.lock().expect("Usage lock").requests;
    let request_elapsed = Arc::new(Mutex::new(std::time::Duration::ZERO));
    let measured_request_elapsed = Arc::clone(&request_elapsed);
    let runner_started = Instant::now();
    let outcome = run_jev_check_prepared(
        &case.check,
        &context,
        &mut plan,
        JevRunProgress::default(),
        permit,
        |batch| {
            let request_elapsed = Arc::clone(&measured_request_elapsed);
            async move {
                let started = Instant::now();
                let result =
                    support::run::evaluate_batch(client, usage, &case.id, "assessment", &batch)
                        .await;
                *request_elapsed.lock().expect("Request timing lock") += started.elapsed();
                result
            }
        },
        |_| Ok(()),
    )
    .await;
    let runner_ms = runner_started.elapsed().as_millis();
    let request_execution_ms = request_elapsed
        .lock()
        .expect("Request timing lock")
        .as_millis();
    let request_count = usage
        .lock()
        .expect("Usage lock")
        .requests
        .saturating_sub(request_count_before);
    let mut row = match outcome {
        Ok(outcome) => {
            json!({"score":scoring::score(case, &outcome.result, &plan.work_items, plan.skipped_item_ids.len()),
            "failure":outcome.failure.map(|error|error.to_string()),"decisions":outcome.result.decisions.iter().map(|decision|json!({"comparison":decision.comparison.id,"outcome":decision.outcome,"judgments":decision.judgments})).collect::<Vec<_>>()})
        }
        Err(error) => {
            json!({"score":scoring::failed(case, plan.work_items.len(), plan.skipped_item_ids.len()),"failure":error.to_string()})
        }
    };
    row["id"] = json!(case.id);
    if let Some(baseline) = crate::baseline::result(&case.check, &plan, usage, &case.id) {
        match baseline {
            Ok(result) => {
                row["baseline_score"] =
                    scoring::score(case, &result, &plan.work_items, plan.skipped_item_ids.len());
                row["baseline_result"] = json!(result);
            }
            Err(error) => row["baseline_failure"] = json!(error),
        }
    }
    row["family"] = json!(case.family);
    row["expected"] = json!(case.label);
    let total_ms = total_started.elapsed().as_millis();
    row["elapsed_ms"] = json!(total_ms);
    row["timing"] = json!({
        "context_construction_ms":context_construction_ms,
        "preparation_ms":preparation_ms,
        "runner_ms":runner_ms,
        "request_execution_ms":request_execution_ms,
        "packing_ms":null,
        "reduction_ms":null,
        "total_ms":total_ms,
        "work_items":work_item_count,
        "requests":request_count,
    });
    row
}

pub(crate) async fn run() -> Result<(), String> {
    run_selected(false).await
}

pub(crate) async fn benchmark_session() -> Result<(), String> {
    run_selected(true).await
}

async fn run_selected(benchmark: bool) -> Result<(), String> {
    let suite = support::selection::suite();
    let cases = support::selection::select(fixtures::cases(&suite), |case| &case.id);
    let client = match support::provider::EvalClient::from_environment().await {
        Ok(client) => client,
        Err(error) => {
            let report = json!({"check":"skill_opportunities","suite":suite,"provider":support::provider::configuration().identity(),
                "failure":error.to_string(),"scheduled_cases":cases.len(),"executed_cases":0});
            let path = support::capture::report("skill_opportunities", &suite, &report);
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
            !benchmark
                && (row["score"]["unsafe_publications"]
                    .as_u64()
                    .is_some_and(|count| count > 0)
                    || row["score"]["historical_claims"]
                        .as_u64()
                        .is_some_and(|count| count > 0)),
            row["score"]["missing"]
                .as_u64()
                .is_some_and(|count| count > 0),
            !row["failure"].is_null(),
        );
        rows.push(row);
        if stopped.is_some() {
            break;
        }
    }
    let metrics = scoring::metrics(&cases, &rows);
    println!("Skill Opportunities: {metrics}");
    let report = json!({"check":"skill_opportunities","suite":suite,"provider":support::provider::configuration().identity(),
        "revisions":antiburn_local::checks::skill_opportunities::SKILL_OPPORTUNITIES_REVISIONS,"production_runner":true,
        "metrics":metrics,"stopped":stopped,"measurements":support::run::measurements(&rows,&usage.lock().expect("Usage lock")),"rows":rows});
    let path = support::capture::report("skill_opportunities", &suite, &report);
    match stopped {
        Some(reason) => Err(format!(
            "Diagnostic stopped: {reason}; report={}",
            path.display()
        )),
        None => Ok(()),
    }
}

#[test]
fn missing_judgments_do_not_publish_or_remove_positive_labels() {
    for suite in ["development", "controls"] {
        for case in fixtures::cases(suite) {
            let plan = case.check.prepare(&case.check.session_context()).unwrap();
            let result = case.check.reduce(&plan, &[], false).unwrap();
            let score = scoring::score(
                &case,
                &result,
                &plan.work_items,
                plan.skipped_item_ids.len(),
            );
            assert!(result.findings.is_empty());
            assert_eq!(score["clean"], 0);
            if case.label == fixtures::Label::Advisory {
                assert_eq!(score["fn"], 1);
            }
        }
    }
}

#[test]
fn single_pair_choices_preserve_partial_findings_and_complete_uncertainty_sampling() {
    use antiburn_local::analysis::jev::{JevAnswer, JevUsage, JevWorkItemResult};
    use antiburn_local::checks::sampling::{SamplingLimits, SamplingProgress};
    use antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome;
    for case in fixtures::cases("development") {
        let context = case.check.session_context();
        let mut sampling = SamplingProgress::new(SamplingLimits {
            checks: 1,
            candidates_per_check: 4096,
            answers_per_candidate: 1,
            judgments_per_run: 4,
        })
        .unwrap();
        case.check.synchronize_sampling(&mut sampling).unwrap();
        sampling.begin_run();
        let Some(job) = sampling.choose_job() else {
            continue;
        };
        let plan = case
            .check
            .prepare_sampled(
                &context,
                &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
                std::slice::from_ref(&job),
            )
            .unwrap();
        if plan.work_items.is_empty() {
            continue;
        }
        assert_eq!(plan.work_items.len(), 1);
        let item = &plan.work_items[0];
        assert_eq!(item.questions.len(), 1);
        assert!(item.questions.contains_key("opportunity"));
        let mut answer = JevWorkItemResult {
            request_id: item.id.clone(),
            work_item_id: item.id.clone(),
            model: plan.capabilities.model.clone(),
            answers: std::collections::BTreeMap::from([(
                "opportunity".into(),
                JevAnswer::Choice {
                    choice: "uncertain".into(),
                    probabilities: std::collections::BTreeMap::from([
                        ("useful_opportunity".into(), 0.05),
                        ("no_opportunity".into(), 0.05),
                        ("specialist_check".into(), 0.0),
                        ("already_covered".into(), 0.0),
                        ("uncertain".into(), 0.9),
                    ]),
                    confidence: 0.1,
                },
            )]),
            evidence: plan
                .shared_context
                .as_ref()
                .unwrap()
                .evidence
                .iter()
                .chain(&item.window.evidence)
                .cloned()
                .collect(),
            usage: JevUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        };
        let result = case
            .check
            .reduce(&plan, std::slice::from_ref(&answer), true)
            .unwrap();
        assert_eq!(
            result.decisions[0].outcome,
            SkillOpportunityOutcome::Uncertain
        );
        assert!(!result.complete);
        case.check
            .record_sampling_result(&mut sampling, &job, &result)
            .unwrap();
        let mut restored: SamplingProgress =
            serde_json::from_value(serde_json::to_value(sampling).unwrap()).unwrap();
        case.check.synchronize_sampling(&mut restored).unwrap();
        restored.begin_run();
        assert_eq!(
            restored
                .coverage(case.check.sampling_identity())
                .unwrap()
                .completed,
            1
        );
        assert!(
            restored
                .choose_job()
                .is_none_or(|next| next.candidate != job.candidate)
        );
        for (probability, expected) in [
            (0.74, SkillOpportunityOutcome::Uncertain),
            (0.75, SkillOpportunityOutcome::Advisory),
        ] {
            answer.answers.insert(
                "opportunity".into(),
                JevAnswer::Choice {
                    choice: "useful_opportunity".into(),
                    probabilities: std::collections::BTreeMap::from([
                        ("useful_opportunity".into(), probability),
                        ("no_opportunity".into(), (1.0 - probability) / 2.0),
                        ("specialist_check".into(), 0.0),
                        ("already_covered".into(), 0.0),
                        ("uncertain".into(), (1.0 - probability) / 2.0),
                    ]),
                    confidence: 0.1,
                },
            );
            let result = case
                .check
                .reduce(&plan, std::slice::from_ref(&answer), true)
                .unwrap();
            assert_eq!(result.decisions[0].outcome, expected, "{}", case.id);
            assert_eq!(
                result.findings.len(),
                usize::from(expected == SkillOpportunityOutcome::Advisory)
            );
            if case.family == "missing_history" && expected == SkillOpportunityOutcome::Advisory {
                let score = scoring::score(
                    &case,
                    &result,
                    &plan.work_items,
                    plan.skipped_item_ids.len(),
                );
                assert_eq!(score["unsafe_publications"], 0);
                assert_eq!(score["historical_claims"], 0);
            }
        }
    }
}
