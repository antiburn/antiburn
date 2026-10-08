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
    let check = fixtures::check(case);
    let permit = admit_jev_orchestration()
        .await
        .expect("Admit evaluation preparation");
    let mut plan = check
        .prepare_with_capabilities(
            check.context(),
            &support::provider::configuration().capabilities,
        )
        .expect("Synthetic scope prepares");
    let started = Instant::now();
    let outcome = run_jev_check_prepared(
        &check,
        check.context(),
        &mut plan,
        JevRunProgress::default(),
        permit,
        |batch| async move {
            support::run::evaluate_batch(client, usage, &case.id, "assessment", &batch).await
        },
        |_| Ok(()),
    )
    .await;
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
            json!({"observed":observed,"finding_count":result.findings.len(),"binding_exact":exact,
                "decision_binding_exact":result.decisions.iter().map(|decision| &decision.group_id).collect::<BTreeSet<_>>() == plan.prepared.groups.iter().map(|group| &group.id).collect::<BTreeSet<_>>(),
                "missing_scheduled_answers":missing,"failure":outcome.failure.map(|error|error.to_string()),
                "decisions":result.decisions,"limitation":result.session_limitation})
        }
        Err(error) => json!({"observed":"unassessed","finding_count":0,"binding_exact":false,
            "missing_scheduled_answers":plan.work_items.len(),"failure":error.to_string()}),
    };
    let mut row = row;
    row["id"] = json!(case.id);
    row["expected"] = json!(case.expected);
    row["scenario"] = json!(case.scenario);
    row["source"] = json!(case.format);
    row["eligible"] = json!(!plan.work_items.is_empty());
    row["elapsed_ms"] = json!(started.elapsed().as_millis());
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
            false,
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
            if case.expected != "unassessed" {
                assert!(!plan.work_items.is_empty(), "{}", case.id);
            }
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
fn live_regression_cases_bind_one_operation_and_keep_approval_and_failure_context() {
    for scenario in [
        "optional_feature",
        "approval_start",
        "approval_middle",
        "approval_end",
        "necessary_dependency",
        "failed_execution",
    ] {
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
        assert!(
            fields
                .to_string()
                .contains("Fix the login token expiry bug")
        );
        if scenario.starts_with("approval_") {
            let records = fields["task_scope"].as_array().unwrap();
            assert_eq!(
                records
                    .iter()
                    .filter(|record| record["occurrence"]["authority"] == "user")
                    .count(),
                34
            );
            assert!(
                records
                    .iter()
                    .any(|record| record["content"].as_str().is_some_and(|text| text
                        .starts_with("I explicitly authorize this entire additional work"))),
                "{scenario}"
            );
        }
        if scenario == "failed_execution" {
            assert!(
                fields["bound_work"]
                    .to_string()
                    .contains("The write failed before any file changed")
            );
        }
    }
}
