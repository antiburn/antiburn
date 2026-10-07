use super::{fixtures, scoring, support};
use antiburn_local::analysis::jev::{
    JevCheck, JevRunProgress, admit_jev_orchestration, run_jev_check_prepared,
};
use antiburn_local::checks::over_exploring::{OverExploringCheck, build_jev_context};
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
    let outcome = run_jev_check_prepared(
        &OverExploringCheck,
        &context,
        &mut plan,
        JevRunProgress::default(),
        permit,
        |batch| async move {
            support::run::evaluate_batch(client, usage, &case.id, "assessment", &batch).await
        },
        |_| Ok(()),
    )
    .await;
    let mut row = match outcome {
        Ok(outcome) => {
            let mut row = scoring::row(
                case,
                Some(&outcome.result),
                outcome.complete,
                outcome.failure.map(|error| error.to_string()),
            );
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
            row["missing_answers"] = json!(missing);
            row
        }
        Err(error) => scoring::row(case, None, false, Some(error.to_string())),
    };
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
