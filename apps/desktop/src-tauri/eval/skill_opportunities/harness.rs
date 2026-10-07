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
    let context = case.check.session_context();
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let mut plan = case
        .check
        .prepare_with_capabilities(&context, &support::provider::configuration().capabilities)
        .expect("Synthetic skill work prepares");
    let started = Instant::now();
    let outcome = run_jev_check_prepared(
        &case.check,
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
            json!({"score":scoring::score(case, &outcome.result, &plan.work_items, plan.skipped_item_ids.len()),
            "failure":outcome.failure.map(|error|error.to_string()),"decisions":outcome.result.decisions.iter().map(|decision|json!({"comparison":decision.comparison.id,"outcome":decision.outcome,"judgments":decision.judgments})).collect::<Vec<_>>()})
        }
        Err(error) => {
            json!({"score":scoring::failed(case, plan.work_items.len(), plan.skipped_item_ids.len()),"failure":error.to_string()})
        }
    };
    row["id"] = json!(case.id);
    row["family"] = json!(case.family);
    row["expected"] = json!(case.label);
    row["elapsed_ms"] = json!(started.elapsed().as_millis());
    row
}

pub(crate) async fn run() -> Result<(), String> {
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
            row["score"]["unsafe_publications"]
                .as_u64()
                .is_some_and(|count| count > 0)
                || row["score"]["historical_claims"]
                    .as_u64()
                    .is_some_and(|count| count > 0),
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
