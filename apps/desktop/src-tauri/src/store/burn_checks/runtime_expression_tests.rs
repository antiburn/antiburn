use super::*;
use std::sync::atomic::AtomicUsize;

use antiburn_checks::engine::{EvaluationFailureCode, RunSettlement};
use antiburn_checks::source::{
    EvidenceValidity, PublishedContext, PublishedEvidenceBatch, PublishedEvidenceChange,
    PublishedEvidenceEvent, PublishedEvidenceSource, PublishedResultStatus, PublishedToolResult,
};
use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION};

use crate::insights_report::ReportRequest;
use crate::insights_report::installed_checks::{
    InstalledCheckAvailability, InstalledCheckDefinition, InstalledCheckEvent,
    InstalledCheckReportPayload, InstalledCheckScope, InstalledCheckSource, InstalledCheckTrigger,
    read_evidence, read_installed_checks_in_window,
};
use crate::remediation::BurnCheckEvidenceStatus;

#[derive(Default)]
struct ExpressionEvaluator {
    prepared: AtomicUsize,
    sent: Mutex<Vec<PreparedAssessment>>,
}

impl DecisionEvaluator for ExpressionEvaluator {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        Ok(prepare_inputs(inputs, budget, "synthetic-expression-v1"))
    }

    fn validate_prepared(
        &self,
        active: &EffectiveActivation,
        prepared: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        validate_fixture_preparation(active, prepared, "synthetic-expression-v1")
    }

    fn evaluate_prepared<'a>(
        &'a self,
        active: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        assert_eq!(prepared.profile, "synthetic-expression-v1");
        prepared.evaluation_budget.validate_for_resume().unwrap();
        self.sent.lock().unwrap().push(prepared.clone());
        Box::pin(async move { Ok(review_result(active, attempt.number, 0.99)) })
    }
}

fn definition_source(input: &str, actions: &str) -> String {
    format!(
        "name: Host expressions\non:\n  tool_result:\ndecisions:\n  review:\n    input:\n{input}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {{yes: Yes., no: No.}}\nactions:\n{actions}\n"
    )
}

fn expression_fixture(source: &str) -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/host-expressions")).unwrap()),
        source,
    )
}

fn expression_page(store: &Store, sequence: u64, arguments: serde_json::Value) -> SourceBatch {
    let mut page = event(store, sequence, &format!("tool-{sequence}"), "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        unreachable!();
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        unreachable!();
    };
    message.value["tool"]["arguments"] = arguments;
    store
        .lock()
        .execute(
            "UPDATE session SET updated_at_epoch = 100, started_at_epoch = 100
             WHERE environment_key = 'native' AND agent = 'fixture' AND session_id = 'branch'",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "INSERT OR IGNORE INTO session_evidence (environment_key, agent, session_id, status,
           analyzed_generation, parser_revision, analyzer_revision, evidence_schema_revision)
         VALUES ('native', 'fixture', 'branch', 'ready', 0, ?1, ?2, ?3)",
            params![PARSER_REVISION, ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION],
        )
        .unwrap();
    page
}

fn settle_expression(
    engine: &CheckEngine<StoreCheckStore, ExpressionEvaluator>,
    active: &EffectiveActivation,
    sequence: u64,
) -> RunSettlement {
    let StartOutcome::Started(task) = engine.start_one_due(active, sequence * 1_000_000).unwrap()
    else {
        panic!("the synthetic candidate must start");
    };
    poll_settlement(task)
}

fn report_definition(active: &EffectiveActivation) -> InstalledCheckDefinition {
    InstalledCheckDefinition {
        effective_id: "synthetic:host-expressions".into(),
        check_id: active.id.definition_id.clone(),
        activation_scope_key: StoreCheckStore::activation_scope_key(&active.id.scope),
        generation: Some(active.id.generation.0.try_into().unwrap()),
        definition_revision: hex(&active.revision),
        name: "Host expressions".into(),
        aliases: vec![],
        description: None,
        is_local: active.compiled.is_local(),
        decision_name: Some("review".into()),
        trigger: Some(InstalledCheckTrigger {
            event: InstalledCheckEvent::ToolResult,
            minimum: 1,
        }),
        source: InstalledCheckSource {
            scope: InstalledCheckScope::Global,
            path: None,
        },
        availability: InstalledCheckAvailability::Available,
        diagnostic: None,
        diagnostics: vec![],
        definition_sources: vec![],
    }
}

fn report_request() -> ReportRequest {
    ReportRequest {
        environment_key: "native".into(),
        window: antiburn_local::insights::ReportWindow {
            start_epoch: 90,
            end_epoch: 200,
        },
        computed_at_epoch: 200,
    }
}

fn expression_report(store: &Store, active: &EffectiveActivation) -> InstalledCheckReportPayload {
    let mut rows =
        read_installed_checks_in_window(store, &[report_definition(active)], &report_request())
            .unwrap();
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

fn table_count(store: &Store, table: &str) -> i64 {
    store
        .lock()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn saved_preparation(store: &Store, position: u64) -> PreparedAssessment {
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE trigger_position = ?1",
            [position],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Ready(prepared) = serde_json::from_str(&saved).unwrap() else {
        panic!("the input must have a saved successful preparation");
    };
    prepared
}

fn assert_review_evidence(
    evidence: &crate::remediation::BurnCheckTargetEvidence,
    expected: &serde_json::Value,
) {
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert_eq!(
        evidence
            .items
            .iter()
            .map(|item| item.reference.as_str())
            .collect::<Vec<_>>(),
        [
            "original-input:review",
            "assessed-input:review",
            "input-omissions:review"
        ]
    );
    for item in &evidence.items {
        let actual: serde_json::Value = serde_json::from_str(&item.excerpt).unwrap();
        if item.reference == "input-omissions:review" {
            assert_eq!(actual, json!({"omissions": []}));
        } else {
            assert_eq!(&actual, expected);
        }
    }
}

fn saved_plan(store: &Store, position: u64) -> serde_json::Value {
    let saved: String = store
        .lock()
        .query_row(
            "SELECT actions_json FROM burn_check_assessment_history WHERE trigger_position = ?1",
            [position],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&saved).unwrap()
}

fn status_snapshot(store: &Store) -> serde_json::Value {
    let (assessment, position, updated, status, valid, actions, frozen, prepared): (
        String,
        u64,
        i64,
        String,
        bool,
        String,
        String,
        String,
    ) = store
        .lock()
        .query_row(
            "SELECT s.assessment_id, s.position, s.updated_at_epoch, s.status, s.evidence_valid,
               h.actions_json, c.frozen_input_json, c.preparation_json
             FROM burn_check_current_status s
             JOIN burn_check_assessment_history h USING(assessment_id)
             JOIN burn_check_candidate c USING(candidate_id)",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .unwrap();
    json!({
        "assessment": assessment, "position": position, "updated": updated,
        "status": status, "valid": valid, "actions": actions,
        "frozen": frozen, "prepared": prepared,
    })
}

fn watermark(store: &Store) -> u64 {
    store
        .lock()
        .query_row(
            "SELECT position FROM burn_check_acceptance_watermark",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn nested_maps_projections_conversions_and_dynamic_status_recover_after_atomic_acceptance_failure()
{
    let source = definition_source(
        r#"      group_names: ${{ message.tool.arguments.groups.*.name }}
      converted: ${{ fromJSON(message.tool.arguments.payload) }}
      groups:
        map: ${{ message.tool.arguments.groups }}
        as: group
        if: group.enabled
        value:
          name: ${{ group.name }}
          labels: ${{ group.members.*.label }}
          members:
            map: ${{ group.members }}
            as: member
            if: member.enabled
            value:
              label: ${{ format('{0}/{1}', group.name, member.label) }}
              parsed: ${{ fromJSON(member.payload) }}
              encoded: ${{ toJSON(member.label) }}
              text: "Selected: ${{ member.label }}""#,
        r#"  - type: status
    if: decisions.review.outputs.verdict == 'yes'
    input:
      result: ${{ message.tool.arguments.status }}
      message: "Review: ${{ message.tool.arguments.explanation }}""#,
    );
    for status in ["success", "failure"] {
        let (store, adapter, active) = expression_fixture(&source);
        let evaluator = Arc::new(ExpressionEvaluator::default());
        let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
        let page = expression_page(
            &store,
            1,
            json!({
                "status": status,
                "explanation": "${{ message.tool.arguments.never_evaluate }}",
                "payload": "{\"count\":2,\"enabled\":true,\"optional\":null}",
                "groups": [
                    {"name": "active", "enabled": true, "members": [
                        {"label": "${{ unavailable.path }}", "enabled": true, "payload": "[1,false,null]"},
                        {"label": "excluded", "enabled": false}
                    ]},
                    {"name": "excluded", "enabled": false}
                ]
            }),
        );
        let selected = engine.apply_source_page(&active, &page).unwrap();
        assert_eq!((selected.observations, selected.candidates), (1, 1));
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_expression_status BEFORE INSERT ON burn_check_current_status
             BEGIN SELECT RAISE(ABORT, 'synthetic acceptance failure'); END;",
            )
            .unwrap();
        let StartOutcome::Started(task) = engine.start_one_due(&active, 1_000_000).unwrap() else {
            panic!("the selected input must start");
        };
        assert!(poll_task_result(task).is_err());
        for table in [
            "burn_check_assessment_history",
            "burn_check_acceptance_watermark",
            "burn_check_current_status",
        ] {
            assert_eq!(table_count(&store, table), 0, "{table}");
        }
        let expected = json!({
            "group_names": ["active", "excluded"],
            "converted": {"count": 2, "enabled": true, "optional": null},
            "groups": [{
                "name": "active",
                "labels": ["${{ unavailable.path }}", "excluded"],
                "members": [{
                    "label": "active/${{ unavailable.path }}",
                    "parsed": [1, false, null],
                    "encoded": "\"${{ unavailable.path }}\"",
                    "text": "Selected: ${{ unavailable.path }}",
                }],
            }],
        });
        let prepared = saved_preparation(&store, 51);
        assert_eq!(prepared.decisions[0].input, expected);
        assert_eq!(
            evaluator.sent.lock().unwrap().as_slice(),
            std::slice::from_ref(&prepared)
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_expression_status")
            .unwrap();
        engine.recover_pending().unwrap();
        assert_eq!(evaluator.prepared.load(Ordering::SeqCst), 1);
        assert_eq!(*evaluator.sent.lock().unwrap(), [prepared]);
        assert_eq!(table_count(&store, "burn_check_evaluation_attempt"), 1);
        assert_eq!(table_count(&store, "burn_check_assessment_history"), 1);
        assert_eq!(watermark(&store), 51);
        let message = "Review: ${{ message.tool.arguments.never_evaluate }}";
        let plan = saved_plan(&store, 51);
        let expected_status = if status == "success" {
            StatusResult::Success
        } else {
            StatusResult::Failure
        };
        assert_eq!(
            plan["outcomes"][0]["outcome"]["Applied"]["result"],
            json!(expected_status)
        );
        assert_eq!(
            plan["outcomes"][0]["outcome"]["Applied"]["message"],
            message
        );
        let report = expression_report(&store, &active);
        assert_eq!(report.finding, u64::from(status == "failure"));
        assert_eq!(report.clean, u64::from(status == "success"));
        assert_eq!(report.review_failed, 0);
        assert_eq!(report.reviewed_without_status, 0);
        assert_eq!(report.checking_count, 0);
        assert_eq!(report.explanation.as_deref(), Some(message));
        if status == "failure" {
            assert_eq!(report.targets.len(), 1);
            assert_eq!(report.targets[0].observation.as_deref(), Some(message));
            let evidence = read_evidence(
                &store,
                &[report_definition(&active)],
                &report_request(),
                &report.targets[0].id,
            )
            .unwrap();
            assert_review_evidence(&evidence, &expected);
        } else {
            assert!(report.targets.is_empty());
        }
    }
}

#[test]
fn invalid_status_inputs_leave_later_actions_independent_and_no_write_retains_prior_evidence() {
    let source = definition_source(
        "      activity: ${{ message.tool.arguments.activity }}",
        r#"  - type: status
    input: {result: "${{ message.tool.arguments.invalid_result }}"}
  - type: status
    input: {result: success, message: "${{ message.tool.arguments.blank }}"}
  - type: status
    input: {result: success, message: "${{ message.tool.arguments.nontext }}"}
  - type: status
    input: {result: "${{ message.tool.arguments.missing }}"}
  - type: status
    if: message.tool.arguments.write
    input:
      result: ${{ message.tool.arguments.status }}
      message: "Saved: ${{ message.tool.arguments.explanation }}"
  - type: status
    if: false
    input: {result: success}"#,
    );
    for invalid in [
        json!("SUCCESS"),
        json!(" failure"),
        json!(true),
        json!(null),
        json!(1),
    ] {
        let (store, adapter, active) = expression_fixture(&source);
        let evaluator = Arc::new(ExpressionEvaluator::default());
        let engine = CheckEngine::new(adapter, evaluator.clone());
        engine
            .apply_source_page(
                &active,
                &expression_page(
                    &store,
                    1,
                    json!({
                        "activity": "original assessed evidence", "invalid_result": invalid,
                        "blank": " \t\n", "nontext": 7, "write": true, "status": "failure",
                        "explanation": "${{ not.an.expression }}",
                    }),
                ),
            )
            .unwrap();
        assert_eq!(
            settle_expression(&engine, &active, 1).outcome,
            RunOutcome::Accepted
        );
        let first_plan = saved_plan(&store, 51);
        for index in 0..3 {
            assert_eq!(
                first_plan["outcomes"][index]["outcome"]["Failed"]["code"],
                "RuntimeInvalidContext"
            );
        }
        assert_eq!(
            first_plan["outcomes"][3]["outcome"],
            "SkippedInputUnavailable"
        );
        assert_eq!(
            first_plan["outcomes"][4]["outcome"]["Applied"]["result"],
            json!(StatusResult::Failure)
        );
        assert_eq!(
            first_plan["outcomes"][5]["outcome"],
            "SkippedConditionFalse"
        );
        let retained = status_snapshot(&store);
        let before = expression_report(&store, &active);
        assert_eq!(
            (before.finding, before.clean, before.review_failed),
            (1, 0, 0),
            "report: {before:#?}"
        );
        assert_eq!(before.targets.len(), 1);
        let target = before.targets[0].id.clone();
        let evidence_before = read_evidence(
            &store,
            &[report_definition(&active)],
            &report_request(),
            &target,
        )
        .unwrap();
        assert_eq!(evidence_before.status, BurnCheckEvidenceStatus::Available);
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_no_write_insert BEFORE INSERT ON burn_check_current_status
             BEGIN SELECT RAISE(ABORT, 'no-write must not insert status'); END;
             CREATE TRIGGER reject_no_write_update BEFORE UPDATE ON burn_check_current_status
             BEGIN SELECT RAISE(ABORT, 'no-write must not update status'); END;",
            )
            .unwrap();
        engine.apply_source_page(&active, &expression_page(&store, 2, json!({
            "activity": "later evidence must not replace the status evidence", "invalid_result": invalid,
            "blank": " ", "nontext": {"text": "not a string"},
            "write": false, "status": "success", "explanation": "not applied",
        }))).unwrap();
        assert_eq!(
            settle_expression(&engine, &active, 2).outcome,
            RunOutcome::Accepted
        );
        assert_eq!(status_snapshot(&store), retained);
        assert_eq!(watermark(&store), 52);
        let no_write = saved_plan(&store, 52);
        assert_eq!(no_write["current_status"], json!(StatusResult::Failure));
        assert_eq!(no_write["outcomes"][4]["outcome"], "SkippedConditionFalse");
        assert!(
            no_write["outcomes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|action| action["outcome"].get("Applied").is_none())
        );
        let report = expression_report(&store, &active);
        assert_eq!(
            (report.finding, report.clean, report.reviewed_without_status),
            (1, 0, 1)
        );
        assert_eq!(report.review_failed, 0);
        assert_eq!(
            report.explanation.as_deref(),
            Some("Saved: ${{ not.an.expression }}")
        );
        assert_eq!(report.targets[0].id, target);
        let evidence_after = read_evidence(
            &store,
            &[report_definition(&active)],
            &report_request(),
            &report.targets[0].id,
        )
        .unwrap();
        assert_eq!(evidence_after, evidence_before);
        assert_eq!(evaluator.prepared.load(Ordering::SeqCst), 2);
        assert_eq!(evaluator.sent.lock().unwrap().len(), 2);
    }
}

fn budget_source() -> String {
    definition_source(
        "      selected: ${{ message.tool.arguments.items.*.value }}",
        r#"  - type: status
    input: {result: failure, message: Tentative first action.}
  - type: status
    if: contains(message.tool.arguments.items.*.value, 'selected')
    input:
      result: ${{ message.tool.arguments.status }}
      message: "Accepted: ${{ message.tool.arguments.explanation }}""#,
    )
}

fn budget_arguments(count: usize, status: &str, explanation: &str) -> serde_json::Value {
    json!({
        "items": (0..count).map(|_| json!({"value": "selected"})).collect::<Vec<_>>(),
        "status": status,
        "explanation": explanation,
    })
}

fn assert_budget_failure(store: &Store, position: u64) {
    let (state, diagnostic): (String, String) = store
        .lock()
        .query_row(
            "SELECT state, diagnostic_json FROM burn_check_candidate WHERE trigger_position = ?1",
            [position],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "execution_failed");
    let diagnostic: EvaluationDiagnostic = serde_json::from_str(&diagnostic).unwrap();
    assert_eq!(diagnostic.code, EvaluationFailureCode::ActionFailed);
    assert!(diagnostic.detail.contains("visited-item limit"));
    let plan = saved_plan(store, position);
    assert!(plan.get("outcomes").is_none());
    assert!(plan.get("current_status").is_none());
    let accepted: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_assessment_history
         WHERE trigger_position = ?1 AND outcome = 'assessed'",
            [position],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(accepted, 0);
    assert_eq!(
        saved_preparation(store, position)
            .evaluation_budget
            .visited_items(),
        601
    );
}

#[test]
fn shared_input_and_action_budget_exhaustion_publishes_no_tentative_status_or_action_outcomes() {
    let (store, adapter, active) = expression_fixture(&budget_source());
    let evaluator = Arc::new(ExpressionEvaluator::default());
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(
            &active,
            &expression_page(&store, 1, budget_arguments(600, "success", "not accepted")),
        )
        .unwrap();
    let settled = settle_expression(&engine, &active, 1);
    assert!(matches!(settled.outcome, RunOutcome::Failed(_)));
    assert_eq!(settled.attempts, 1);
    assert_budget_failure(&store, 51);
    assert_eq!(table_count(&store, "burn_check_current_status"), 0);
    assert_eq!(table_count(&store, "burn_check_acceptance_watermark"), 0);
    assert_eq!(table_count(&store, "burn_check_evaluation_attempt"), 1);
    assert_eq!(evaluator.sent.lock().unwrap().len(), 1);
    let report = expression_report(&store, &active);
    assert_eq!(
        (report.finding, report.clean, report.review_failed),
        (0, 0, 1)
    );
    assert_eq!(report.reviewed_without_status, 0);
    assert_eq!(report.checking_count, 0);
    assert_eq!(report.explanation, None);
    assert!(report.targets.is_empty());
}

#[test]
fn deferred_answer_keeps_saved_budget_and_prior_status_across_file_store_restart() {
    let directory = tempfile::tempdir().unwrap();
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open(directory.path()).unwrap()),
        &budget_source(),
    );
    let evaluator = Arc::new(ExpressionEvaluator::default());
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    engine
        .apply_source_page(
            &active,
            &expression_page(&store, 1, budget_arguments(1, "failure", "prior finding")),
        )
        .unwrap();
    assert_eq!(
        settle_expression(&engine, &active, 1).outcome,
        RunOutcome::Accepted
    );
    let prior = status_snapshot(&store);
    let prior_target = expression_report(&store, &active).targets[0].id.clone();
    engine
        .apply_source_page(
            &active,
            &expression_page(
                &store,
                2,
                budget_arguments(400, "success", "resumed answer"),
            ),
        )
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 2_000_000).unwrap() else {
        panic!("the second candidate must start");
    };
    let mut later = expression_page(&store, 3, json!({}));
    later.events = vec![SourceEvent::Ignored {
        cursor: PublicationCursor(3),
    }];
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Deferred);
    let prepared = saved_preparation(&store, 52);
    assert_eq!(prepared.evaluation_budget.visited_items(), 401);
    assert_eq!(status_snapshot(&store), prior);
    assert_eq!(watermark(&store), 51);
    assert_eq!(table_count(&store, "burn_check_assessment_history"), 1);
    drop(engine);
    drop(adapter);
    drop(store);

    let store = Arc::new(Store::open(directory.path()).unwrap());
    let adapter = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "expression-restart".into(),
            active.id.scope.clone(),
        )
        .unwrap(),
    );
    adapter.register_activation(&active).unwrap();
    let evaluator = Arc::new(ExpressionEvaluator::default());
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    let recovered = adapter.load_recovery(None, 10).unwrap();
    assert_eq!(recovered.len(), 1);
    let RecoveryWork::Answered { claimed, .. } = &recovered[0].work else {
        panic!("the exact answer must survive restart");
    };
    assert_eq!(claimed.prior_status, Some(StatusResult::Failure));
    assert_eq!(saved_preparation(&store, 52), prepared);
    engine.recover_pending().unwrap();
    assert_eq!(status_snapshot(&store), prior);
    assert_eq!(watermark(&store), 51);
    engine.apply_source_page(&active, &later).unwrap();
    for (key, blocked, restored) in [
        ("internal:yamlBurnChecksDiscoveryEnabledV1", "false", "true"),
        (
            "internal:typesafeCredentialChangePendingV1",
            "true",
            "false",
        ),
        (
            "internal:smartChecksConnectionChangePendingV1",
            "synthetic-change",
            "",
        ),
        (
            "internal:smartChecksCredentialRemovalPendingV1",
            "synthetic-removal",
            "",
        ),
        ("internal:typesafeAuthRejectedV1", "true", "false"),
    ] {
        store.set_internal_value_checked(key, blocked).unwrap();
        engine.recover_pending().unwrap();
        assert_eq!(status_snapshot(&store), prior, "{key}");
        assert_eq!(watermark(&store), 51, "{key}");
        assert_eq!(saved_preparation(&store, 52), prepared, "{key}");
        assert_eq!(
            table_count(&store, "burn_check_assessment_history"),
            1,
            "{key}"
        );
        store.set_internal_value_checked(key, restored).unwrap();
    }
    assert_eq!(
        expression_report(&store, &active).targets[0].id,
        prior_target
    );
    engine.recover_pending().unwrap();
    assert_eq!(evaluator.prepared.load(Ordering::SeqCst), 0);
    assert!(evaluator.sent.lock().unwrap().is_empty());
    assert_eq!(table_count(&store, "burn_check_evaluation_attempt"), 2);
    assert_eq!(saved_preparation(&store, 52), prepared);
    assert_eq!(watermark(&store), 52);
    assert_eq!(
        saved_plan(&store, 52)["outcomes"][1]["outcome"]["Applied"]["result"],
        json!(StatusResult::Success)
    );
    let report = expression_report(&store, &active);
    assert_eq!(
        (report.finding, report.clean, report.review_failed),
        (0, 1, 0)
    );
    assert_eq!(report.checking_count, 0);
    assert_eq!(
        report.explanation.as_deref(),
        Some("Accepted: resumed answer")
    );
    assert!(report.targets.is_empty());
}

#[test]
fn answered_file_store_recovery_does_not_reset_exhausted_action_budget_or_replace_prior_finding() {
    let directory = tempfile::tempdir().unwrap();
    let (store, adapter, active) = fixture_with_definition(
        Arc::new(Store::open(directory.path()).unwrap()),
        &budget_source(),
    );
    let evaluator = Arc::new(ExpressionEvaluator::default());
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    engine
        .apply_source_page(
            &active,
            &expression_page(
                &store,
                1,
                budget_arguments(1, "failure", "retained finding"),
            ),
        )
        .unwrap();
    assert_eq!(
        settle_expression(&engine, &active, 1).outcome,
        RunOutcome::Accepted
    );
    let prior = status_snapshot(&store);
    let target = expression_report(&store, &active).targets[0].id.clone();
    engine
        .apply_source_page(
            &active,
            &expression_page(&store, 2, budget_arguments(600, "success", "not accepted")),
        )
        .unwrap();
    store.lock().execute_batch(
        "CREATE TRIGGER interrupt_expression_failure BEFORE INSERT ON burn_check_assessment_history
         WHEN NEW.outcome = 'execution_failed'
         BEGIN SELECT RAISE(ABORT, 'synthetic interruption after durable answer'); END;",
    ).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 2_000_000).unwrap() else {
        panic!("the candidate must reach action planning");
    };
    assert!(poll_task_result(task).is_err());
    let prepared = saved_preparation(&store, 52);
    assert_eq!(prepared.evaluation_budget.visited_items(), 601);
    assert!(matches!(
        adapter.load_recovery(None, 10).unwrap()[0].work,
        RecoveryWork::Answered { .. }
    ));
    assert_eq!(status_snapshot(&store), prior);
    assert_eq!(watermark(&store), 51);
    store
        .lock()
        .execute_batch("DROP TRIGGER interrupt_expression_failure")
        .unwrap();
    drop(engine);
    drop(adapter);
    drop(store);

    let store = Arc::new(Store::open(directory.path()).unwrap());
    let adapter = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "expression-restart".into(),
            active.id.scope.clone(),
        )
        .unwrap(),
    );
    adapter.register_activation(&active).unwrap();
    let evaluator = Arc::new(ExpressionEvaluator::default());
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    engine.recover_pending().unwrap();
    assert_budget_failure(&store, 52);
    assert_eq!(saved_preparation(&store, 52), prepared);
    assert_eq!(status_snapshot(&store), prior);
    assert_eq!(watermark(&store), 51);
    assert_eq!(table_count(&store, "burn_check_evaluation_attempt"), 2);
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(evaluator.prepared.load(Ordering::SeqCst), 0);
    assert!(evaluator.sent.lock().unwrap().is_empty());
    let report = expression_report(&store, &active);
    assert_eq!(
        (report.finding, report.clean, report.review_failed),
        (1, 0, 1)
    );
    assert_eq!(report.reviewed_without_status, 0);
    assert_eq!(report.checking_count, 0);
    assert_eq!(
        report.explanation.as_deref(),
        Some("Accepted: retained finding")
    );
    assert_eq!(report.targets[0].id, target);
}

fn published_request_page(
    store: &Store,
    requests: Vec<(String, serde_json::Value)>,
    requests_complete: bool,
    arguments: serde_json::Value,
) -> SourceBatch {
    expression_page(store, 1, arguments.clone());
    let page = PublishedEvidenceSource::new(
        PublishedReader(PublishedEvidenceBatch {
            events: vec![PublishedEvidenceEvent {
                source_evidence_version: None,
                cursor: PublicationCursor(1),
                branch: "branch".into(),
                context: Some(PublishedContext {
                    session_selection: None,
                    inventories: Default::default(),
                    requests,
                    session_id: "branch".into(),
                    agent_name: "fixture".into(),
                    project_root: Some("/repo".into()),
                    version: "synthetic-request-prefix".into(),
                    main_session: true,
                    complete_through_event: requests_complete,
                }),
                change: PublishedEvidenceChange::Upsert(PublishedToolResult {
                    metadata: Box::default(),
                    identity: "tool-1".into(),
                    native_order: NativeOrderKey("tool-1".into()),
                    first_terminal_publication: None,
                    revision: 1,
                    tool_name: Some("synthetic_tool".into()),
                    category: Some("shell".into()),
                    arguments: Some(arguments),
                    content: Some("Available tool result.".into()),
                    outcome: None,
                    provider_executed: None,
                    status: PublishedResultStatus::Error,
                    finality: true,
                    payload_complete: true,
                    validity: EvidenceValidity::Valid,
                }),
            }],
            next: PublicationCursor(1),
            has_more: false,
        }),
        1,
    )
    .unwrap()
    .read_batch(PublicationCursor(0))
    .unwrap();
    let SourceEvent::Observation { observation, .. } = &page.events[0] else {
        panic!("the valid synthetic publication must normalize");
    };
    assert_eq!(
        observation.context.value["session"]["requests_complete"],
        requests_complete
    );
    assert_eq!(
        observation.context.value["session"]["source_coverage"],
        if requests_complete {
            "complete"
        } else {
            "partial"
        }
    );
    page
}

fn available_request() -> Vec<(String, serde_json::Value)> {
    vec![(
        "request-1".into(),
        json!({"source": "fixture", "kind": "user_message", "text": "Available request."}),
    )]
}

fn assert_request_membership_blocks_dispatch(
    store: &Store,
    evaluator: &ExpressionEvaluator,
    active: &EffectiveActivation,
    settled: RunSettlement,
    expression: &str,
) {
    assert!(
        matches!(settled.outcome, RunOutcome::NotAssessed(_)),
        "{expression}: {settled:?}"
    );
    assert_eq!(settled.attempts, 0);
    assert_eq!(evaluator.prepared.load(Ordering::SeqCst), 0);
    assert!(evaluator.sent.lock().unwrap().is_empty());
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
    ] {
        assert_eq!(table_count(store, table), 0, "{table}");
    }
    let (state, saved): (String, String) = store
        .lock()
        .query_row(
            "SELECT state, preparation_json FROM burn_check_candidate WHERE trigger_position = 51",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "not_assessed");
    let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
        panic!("unknown request membership must retain its preparation rejection");
    };
    assert_eq!(
        failure.code,
        antiburn_checks::fit::PreparationFailureCode::InputUnavailable
    );
    let report = expression_report(store, active);
    assert_eq!(
        (report.finding, report.clean, report.review_failed),
        (0, 0, 1)
    );
    assert_eq!(report.checking_count, 0);
    assert_eq!(report.reviewed_without_status, 0);
    assert!(report.targets.is_empty());
}

#[test]
fn partial_empty_request_map_cannot_dispatch_or_publish_a_known_empty_success() {
    let source = definition_source(
        r#"      requests:
        map: ${{ session.requests }}
        as: request
        value: ${{ request.text }}"#,
        "  - type: status\n    input: {result: success}",
    );
    for complete in [false, true] {
        let (store, adapter, active) = expression_fixture(&source);
        let evaluator = Arc::new(ExpressionEvaluator::default());
        let engine = CheckEngine::new(adapter, evaluator.clone());
        let selected = engine
            .apply_source_page(
                &active,
                &published_request_page(&store, vec![], complete, json!({})),
            )
            .unwrap();
        assert_eq!(selected.candidates, 1);
        let settled = settle_expression(&engine, &active, 1);
        if complete {
            assert_eq!(settled.outcome, RunOutcome::Accepted);
            assert_eq!(settled.attempts, 1);
            assert_eq!(
                saved_preparation(&store, 51).decisions[0].input,
                json!({"requests": []})
            );
            assert_eq!(evaluator.sent.lock().unwrap().len(), 1);
            let report = expression_report(&store, &active);
            assert_eq!(
                (report.finding, report.clean, report.review_failed),
                (0, 1, 0)
            );
        } else {
            assert_request_membership_blocks_dispatch(
                &store,
                &evaluator,
                &active,
                settled,
                "map over empty session.requests",
            );
        }
    }
}

#[test]
fn partial_request_projection_index_length_and_json_conversion_keep_unknown_membership() {
    for (expression, expected) in [
        ("session.requests.*.text", json!(["Available request."])),
        ("session.requests[0].text", json!("Available request.")),
        ("session.requests.length", json!(1)),
        (
            "fromJSON(toJSON(session.requests)).*.text",
            json!(["Available request."]),
        ),
        (
            "fromJSON(toJSON(session.requests))[0].text",
            json!("Available request."),
        ),
        ("fromJSON(toJSON(session.requests)).length", json!(1)),
        (
            "fromJSON(toJSON(session)).requests.*.text",
            json!(["Available request."]),
        ),
        (
            "fromJSON(toJSON(session)).requests[0].text",
            json!("Available request."),
        ),
        ("fromJSON(toJSON(session)).requests.length", json!(1)),
        (
            "fromJSON(format('{0}', toJSON(session.requests))).length",
            json!(1),
        ),
        (
            "fromJSON(format('{{\"wrapped\":{0}}}', toJSON(session.requests))).wrapped.length",
            json!(1),
        ),
        (
            "fromJSON(join(toJSON(session.requests), '')).length",
            json!(1),
        ),
    ] {
        let input = format!("      selected: ${{{{ {expression} }}}}");
        let source = definition_source(&input, "  - type: status\n    input: {result: success}");
        for complete in [false, true] {
            let (store, adapter, active) = expression_fixture(&source);
            let evaluator = Arc::new(ExpressionEvaluator::default());
            let engine = CheckEngine::new(adapter, evaluator.clone());
            engine
                .apply_source_page(
                    &active,
                    &published_request_page(&store, available_request(), complete, json!({})),
                )
                .unwrap();
            let settled = settle_expression(&engine, &active, 1);
            if complete {
                assert_eq!(settled.outcome, RunOutcome::Accepted, "{expression}");
                assert_eq!(settled.attempts, 1, "{expression}");
                assert_eq!(
                    saved_preparation(&store, 51).decisions[0].input,
                    json!({"selected": expected}),
                    "{expression}"
                );
                assert_eq!(evaluator.sent.lock().unwrap().len(), 1, "{expression}");
                let report = expression_report(&store, &active);
                assert_eq!(
                    (report.finding, report.clean, report.review_failed),
                    (0, 1, 0),
                    "{expression}"
                );
            } else {
                assert_request_membership_blocks_dispatch(
                    &store, &evaluator, &active, settled, expression,
                );
            }
        }
    }
    for collection in [
        "fromJSON(toJSON(session.requests))",
        "fromJSON(toJSON(session)).requests",
    ] {
        let input = format!(
            "      selected:\n        map: ${{{{ {collection} }}}}\n        as: request\n        value: ${{{{ request.text }}}}"
        );
        let source = definition_source(&input, "  - type: status\n    input: {result: success}");
        for complete in [false, true] {
            let (store, adapter, active) = expression_fixture(&source);
            let evaluator = Arc::new(ExpressionEvaluator::default());
            let engine = CheckEngine::new(adapter, evaluator.clone());
            engine
                .apply_source_page(
                    &active,
                    &published_request_page(&store, available_request(), complete, json!({})),
                )
                .unwrap();
            let settled = settle_expression(&engine, &active, 1);
            if complete {
                assert_eq!(settled.outcome, RunOutcome::Accepted, "{collection}");
                assert_eq!(
                    saved_preparation(&store, 51).decisions[0].input,
                    json!({"selected": ["Available request."]}),
                    "{collection}"
                );
                assert_eq!(evaluator.sent.lock().unwrap().len(), 1, "{collection}");
            } else {
                assert_request_membership_blocks_dispatch(
                    &store, &evaluator, &active, settled, collection,
                );
            }
        }
    }
}

#[test]
fn partial_whole_request_copy_and_unrelated_tool_input_remain_assessable() {
    for (input, expected) in [
        (
            "      activity: ${{ message.tool.result.content }}",
            json!({"activity": "Available tool result."}),
        ),
        (
            r#"      activity: ${{ message.tool.result.content }}
      requests: ${{ session.requests }}
      requests_complete: ${{ session.requests_complete }}
      request_history_complete: ${{ message.quality.request_history_complete }}"#,
            json!({
                "activity": "Available tool result.",
                "requests": [{"id": "request-1", "source": "fixture", "kind": "user_message", "text": "Available request."}],
                "requests_complete": false,
                "request_history_complete": false,
            }),
        ),
        (
            r#"      activity: ${{ message.tool.result.content }}
      requests: ${{ fromJSON(toJSON(session.requests)) }}
      requests_complete: ${{ session.requests_complete }}"#,
            json!({
                "activity": "Available tool result.",
                "requests": [{"id": "request-1", "source": "fixture", "kind": "user_message", "text": "Available request."}],
                "requests_complete": false,
            }),
        ),
    ] {
        let source = definition_source(
            input,
            "  - type: status\n    input: {result: failure, message: Review the available partial input.}",
        );
        let (store, adapter, active) = expression_fixture(&source);
        let evaluator = Arc::new(ExpressionEvaluator::default());
        let engine = CheckEngine::new(adapter, evaluator.clone());
        engine
            .apply_source_page(
                &active,
                &published_request_page(&store, available_request(), false, json!({})),
            )
            .unwrap();
        let settled = settle_expression(&engine, &active, 1);
        assert_eq!(settled.outcome, RunOutcome::Accepted);
        assert_eq!(settled.attempts, 1);
        assert_eq!(saved_preparation(&store, 51).decisions[0].input, expected);
        assert_eq!(evaluator.prepared.load(Ordering::SeqCst), 1);
        assert_eq!(evaluator.sent.lock().unwrap().len(), 1);
        let report = expression_report(&store, &active);
        assert_eq!(
            (report.finding, report.clean, report.review_failed),
            (1, 0, 0)
        );
        assert_eq!(report.targets.len(), 1);
        let evidence = read_evidence(
            &store,
            &[report_definition(&active)],
            &report_request(),
            &report.targets[0].id,
        )
        .unwrap();
        assert_review_evidence(&evidence, &expected);
    }
}

#[test]
fn partial_request_membership_in_actions_skips_only_dependent_actions() {
    let source = definition_source(
        "      activity: ${{ message.tool.result.content }}",
        r#"  - type: status
    if: session.requests.length == 0
    input: {result: success}
  - type: status
    input:
      result: ${{ session.requests[0].text == 'Available request.' && 'success' || 'failure' }}
  - type: status
    input:
      result: success
      message: "Requests: ${{ join(session.requests.*.text, '; ') }}"
  - type: status
    input: {result: failure, message: Independent tool evidence still applies.}"#,
    );
    let (store, adapter, active) = expression_fixture(&source);
    let evaluator = Arc::new(ExpressionEvaluator::default());
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(
            &active,
            &published_request_page(&store, available_request(), false, json!({})),
        )
        .unwrap();
    let settled = settle_expression(&engine, &active, 1);
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!(settled.attempts, 1);
    assert_eq!(evaluator.sent.lock().unwrap().len(), 1);
    let plan = saved_plan(&store, 51);
    assert_eq!(
        plan["outcomes"][0]["outcome"],
        "SkippedConditionUnavailable"
    );
    for index in [1, 2] {
        assert_eq!(
            plan["outcomes"][index]["outcome"],
            "SkippedInputUnavailable"
        );
    }
    assert_eq!(
        plan["outcomes"][3]["outcome"]["Applied"]["result"],
        json!(StatusResult::Failure)
    );
    let report = expression_report(&store, &active);
    assert_eq!(
        (report.finding, report.clean, report.review_failed),
        (1, 0, 0)
    );
    assert_eq!(report.reviewed_without_status, 0);
    assert_eq!(
        report.explanation.as_deref(),
        Some("Independent tool evidence still applies.")
    );
}

#[test]
fn excluded_map_item_does_not_require_unused_missing_fields_with_partial_request_history() {
    let source = definition_source(
        r#"      selected:
        map: ${{ message.tool.arguments.items }}
        as: item
        if: item.include
        value: ${{ item.text }}
      requests_complete: ${{ session.requests_complete }}"#,
        "  - type: status\n    input: {result: failure, message: Selected tool evidence is available.}",
    );
    let (store, adapter, active) = expression_fixture(&source);
    let evaluator = Arc::new(ExpressionEvaluator::default());
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(
            &active,
            &published_request_page(
                &store,
                vec![],
                false,
                json!({"items": [{"include": false}, {"include": true, "text": "Selected value."}]}),
            ),
        )
        .unwrap();
    let settled = settle_expression(&engine, &active, 1);
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!(settled.attempts, 1);
    assert_eq!(evaluator.sent.lock().unwrap().len(), 1);
    assert_eq!(
        saved_preparation(&store, 51).decisions[0].input,
        json!({"selected": ["Selected value."], "requests_complete": false})
    );
    let report = expression_report(&store, &active);
    assert_eq!(
        (report.finding, report.clean, report.review_failed),
        (1, 0, 0)
    );
    assert_eq!(report.reviewed_without_status, 0);
}
