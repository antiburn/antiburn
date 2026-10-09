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
    } else if !complete || !result.unassessed_comparisons.is_empty() {
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
    let input = evidence::input(case);
    let capabilities = &support::provider::configuration().capabilities;
    let context =
        build_jev_context_with_capabilities(&input, &SamplingLedger::default(), capabilities)
            .expect("runtime fixture builds production context");
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let mut plan = IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, capabilities)
        .expect("Synthetic instruction evidence prepares");
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
    let mut row = empty_row(case);
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
    row["elapsed_ms"] = json!(started.elapsed().as_millis());
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
        let unsafe_publication = case.scenario.authority_control
            && matches!(row["observed"].as_str(), Some("finding" | "no_finding"))
            || (row["observed"] == "finding" && row["evidence_valid"] != true);
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
    let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
        &plan.work_items,
        &plan.capabilities,
    );
    assert!(packed.skipped_item_ids.is_empty());
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
