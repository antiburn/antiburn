use super::*;
use crate::jev::config::{
    SystemOneConnection, SystemOneEndpoint, SystemOneProvider, SystemOneResponseMode,
};
use crate::jev::decision_evaluator::TypeSafeDecisionEvaluator;
use antiburn_checks::decision::DecisionInput;
use antiburn_checks::engine::DecisionEvaluator;
use antiburn_checks::fit::PreparationRecord;
use antiburn_checks::prepared_assessment::PreparedAssessment;

const MULTI_CHECK: &str = "name: Multiple report inputs
on:
  tool_result:
decisions:
  first:
    input: {note: first}
    fit:
      note: truncate
    output:
      verdict:
        type: choice
        question: Is this useful?
        criteria: {clear: Clear., concern: Concern.}
  second:
    input: {note: second}
    output:
      verdict:
        type: choice
        question: Is this useful?
        criteria: {clear: Clear., concern: Concern.}
actions:
  - type: status
    input: {result: failure}
";

fn fixture() -> (
    Arc<Store>,
    InstalledCheckDefinition,
    PreparedAssessment,
    EvaluatedAssessment,
) {
    let store = store();
    let definition = definition("/workspace/demo");
    let activation = EffectiveActivation {
        id: ActivationId {
            scope: scope("/workspace/demo"),
            definition_id: definition.check_id.clone(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Project,
        source_paths: vec![],
        project_root: None,
        compiled: Arc::new(compile(MULTI_CHECK).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: Default::default(),
    };
    let definition =
        crate::installed_checks::tests::definition_metadata_for_activation(&activation);
    assert!(!definition.is_local);
    assert_eq!(definition.decision_name, None);
    let adapter = StoreCheckStore::new(
        store.clone(),
        "multi-report".into(),
        activation.id.scope.clone(),
    )
    .unwrap();
    adapter
        .activate(
            &definition.check_id,
            &LifecycleTransition::Activated(activation.clone()),
            Some(&activation),
        )
        .unwrap();
    publish_runtime_definition(&store, &definition);
    let connection = SystemOneConnection {
        provider: SystemOneProvider::Cloudflare,
        endpoint: SystemOneEndpoint::CloudflareAccount("syntheticaccount".into()),
        model: "clef".into(),
        model_revision: None,
        response_mode: SystemOneResponseMode::CloudflareEnvelope,
        credential: None,
        revision: 7,
        context_override: None,
    };
    let mut capabilities = connection.capabilities().unwrap();
    capabilities.request_body_bytes.value = Some(4096);
    let evaluator = TypeSafeDecisionEvaluator::from_selected_snapshot(
        connection,
        Some("SECRET_CANARY".into()),
        capabilities,
    )
    .unwrap();
    let prepared = evaluator
        .prepare(
            &activation,
            vec![
                DecisionInput {
                    decision: "first".into(),
                    input: json!({"note": "Original text. ".repeat(600)}),
                },
                DecisionInput {
                    decision: "second".into(),
                    input: json!({"note": "Unchanged second input."}),
                },
            ],
            Default::default(),
        )
        .unwrap();
    assert_ne!(
        prepared.decisions[0].original_input,
        prepared.decisions[0].input
    );
    assert_eq!(
        prepared.decisions[1].original_input,
        prepared.decisions[1].input
    );
    assert!(prepared.decisions[1].report.omissions.is_empty());
    let requirements = activation.compiled.question_requirements();
    let results = activation
        .compiled
        .decisions
        .iter()
        .map(|declaration| {
            DecisionResult::new(
                declaration,
                &requirements,
                vec![
                    NamedAnswer::new(
                        "verdict",
                        QuestionResult::Choice(
                            ChoiceResult::new(
                                &declaration.outputs[0].question,
                                "concern",
                                None,
                                None,
                            )
                            .unwrap(),
                        ),
                    )
                    .unwrap(),
                ],
                1,
            )
            .unwrap()
        })
        .collect();
    let evaluated =
        EvaluatedAssessment::new(&activation.compiled.decisions, &requirements, results).unwrap();
    (store, definition, prepared, evaluated)
}

fn save_preparation(store: &Store, position: i64, prepared: &PreparedAssessment) {
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = ?2",
            params![
                serde_json::to_string(&PreparationRecord::Ready(prepared.clone())).unwrap(),
                position
            ],
        )
        .unwrap();
}

fn value(evidence: &BurnCheckTargetEvidence, reference: &str) -> serde_json::Value {
    serde_json::from_str(
        &evidence
            .items
            .iter()
            .find(|item| item.reference == reference)
            .unwrap()
            .excerpt,
    )
    .unwrap()
}

#[test]
fn multiple_inputs_keep_exact_status_provenance_after_settings_no_write_and_pruning() {
    let (store, definition, prepared, evaluated) = fixture();
    accept_evaluated_with_message(
        &store,
        &definition,
        "multi",
        1,
        Some(StatusResult::Failure),
        Some("Original finding."),
        &evaluated,
    );
    save_preparation(&store, 1, &prepared);
    let report = rows(&store, std::slice::from_ref(&definition));
    assert!(report[0].has_started);
    assert_eq!(report[0].checking_count, 0);
    assert_eq!(report[0].diagnostics.len(), 1);
    let diagnostic = &report[0].diagnostics[0];
    assert_eq!(diagnostic.code, "input-shortened");
    for (label, expected) in [
        ("Decision", "first"),
        ("Provider", "Cloudflare"),
        ("Model", "clef"),
    ] {
        assert!(
            diagnostic
                .details
                .iter()
                .any(|detail| detail.label == label && detail.value == expected)
        );
    }
    let target = report[0].targets[0].id.clone();
    let evidence = read_evidence(
        &store,
        std::slice::from_ref(&definition),
        &request(),
        &target,
    )
    .unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert!(
        evidence
            .items
            .iter()
            .all(|item| item.reference != "local-context")
    );
    for block in &prepared.decisions {
        assert_eq!(
            value(&evidence, &format!("original-input:{}", block.decision)),
            block.original_input
        );
        assert_eq!(
            value(&evidence, &format!("assessed-input:{}", block.decision)),
            block.input
        );
        assert_eq!(
            value(&evidence, &format!("input-omissions:{}", block.decision)),
            serde_json::to_value(&block.report).unwrap()
        );
    }
    assert!(
        evidence
            .items
            .iter()
            .find(|item| item.reference == "assessed-input:first")
            .unwrap()
            .limitation
            .is_some()
    );
    assert!(
        evidence
            .items
            .iter()
            .find(|item| item.reference == "assessed-input:second")
            .unwrap()
            .limitation
            .is_none()
    );
    let provenance = value(&evidence, "decision-connection");
    assert_eq!(provenance["provider"], "Cloudflare");
    assert_eq!(provenance["requestedModel"], "clef");
    assert_eq!(provenance["connectionRevision"], 7);
    let snapshot = || {
        store.lock().query_row(
        "SELECT assessment_id, updated_at_epoch, position, status FROM burn_check_current_status", [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, u64>(2)?, row.get::<_, String>(3)?)),
    ).unwrap()
    };
    let original_status = snapshot();
    store
        .set_internal_value_checked(
            "internal:smartChecksActiveConnectionV1",
            &serde_json::to_string(&SystemOneConnection::jev_default()).unwrap(),
        )
        .unwrap();
    accept_evaluated_with_message(&store, &definition, "multi", 2, None, None, &evaluated);
    let mut later = prepared.clone();
    later.decisions[0].input = json!({"note": "LATER_INPUT_CANARY"});
    later.decisions[0].original_input = later.decisions[0].input.clone();
    later.decisions[0].report.omissions.clear();
    save_preparation(&store, 2, &later);
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), i64::MAX).unwrap();
    assert_eq!(snapshot(), original_status);
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(
        (report[0].finding, report[0].reviewed_without_status),
        (1, 1)
    );
    assert_eq!(report[0].targets[0].id, target);
    assert!(report[0].diagnostics.is_empty());
    let retained = read_evidence(
        &store,
        std::slice::from_ref(&definition),
        &request(),
        &target,
    )
    .unwrap();
    assert_eq!(retained, evidence);
    let serialized = serde_json::to_string(&retained).unwrap();
    for private in [
        "LATER_INPUT_CANARY",
        "SECRET_CANARY",
        "syntheticaccount",
        "api.cloudflare.com",
    ] {
        assert!(!serialized.contains(private));
    }
    accept_evaluated_with_message(
        &store,
        &definition,
        "multi",
        3,
        Some(StatusResult::Failure),
        None,
        &evaluated,
    );
    save_preparation(&store, 3, &later);
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), i64::MAX).unwrap();
    let pruned: (String, Option<String>) = store.lock().query_row(
        "SELECT frozen_input_json, preparation_json FROM burn_check_candidate WHERE trigger_position = 1", [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(pruned.0, "{}");
    let pruned_preparation: serde_json::Value =
        serde_json::from_str(pruned.1.as_deref().unwrap()).unwrap();
    assert!(
        pruned_preparation["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|block| {
                ["original_input", "input", "request"]
                    .iter()
                    .all(|field| block.get(field).is_none())
            })
    );
    assert_eq!(
        read_evidence(
            &store,
            std::slice::from_ref(&definition),
            &request(),
            &target
        )
        .unwrap()
        .status,
        BurnCheckEvidenceStatus::Unavailable
    );
    let current_target = rows(&store, std::slice::from_ref(&definition))[0].targets[0]
        .id
        .clone();
    store.lock().execute("UPDATE burn_check_assessment_history SET evidence_valid = 0 WHERE trigger_position = 3", []).unwrap();
    assert_eq!(
        read_evidence(&store, &[definition], &request(), &current_target)
            .unwrap()
            .status,
        BurnCheckEvidenceStatus::Unavailable
    );
}

#[test]
fn multiple_decisions_without_saved_preparation_keep_legacy_source_evidence() {
    let (store, definition, _, evaluated) = fixture();
    accept_evaluated_with_message(
        &store,
        &definition,
        "legacy",
        1,
        Some(StatusResult::Failure),
        None,
        &evaluated,
    );
    let report = rows(&store, std::slice::from_ref(&definition));
    assert!(report[0].diagnostics.is_empty());
    let evidence =
        read_evidence(&store, &[definition], &request(), &report[0].targets[0].id).unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert_eq!(evidence.items.len(), 2);
    assert!(evidence.items.iter().all(|item| {
        item.source_label.starts_with("Original source:") && item.limitation.is_some()
    }));
}

#[test]
fn accepted_legacy_input_larger_than_the_new_set_bound_remains_read_only() {
    let (store, definition) = populated();
    let accepted_status = || {
        store.lock().query_row(
        "SELECT assessment_id, updated_at_epoch, position, status FROM burn_check_current_status WHERE branch_key = 'branch-a'", [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?)),
    ).unwrap()
    };
    let before_status = accepted_status();
    let input = json!({"text": "x".repeat(4 * 1024 * 1024 + 128)});
    let (input, report) = antiburn_checks::fit::fit_input(input, &[], |_, _| Ok(())).unwrap();
    let legacy = json!({
        "status": "ready", "profile": "fixture", "input": input,
        "request": {"state": {"evidence": input}}, "report": report,
    })
    .to_string();
    assert!(legacy.len() > 8 * 1024 * 1024);
    assert!(antiburn_checks::preparation::decode_preparation_record(legacy.as_bytes()).is_err());
    assert!(preparation::decode(legacy.as_bytes(), false).is_err());
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = 1",
            [&legacy],
        )
        .unwrap();
    let before = store.lock().total_changes();
    let report = rows(&store, std::slice::from_ref(&definition));
    let evidence =
        read_evidence(&store, &[definition], &request(), &report[0].targets[0].id).unwrap();
    assert_eq!(store.lock().total_changes(), before);
    assert_eq!(accepted_status(), before_status);
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert_eq!(value(&evidence, "assessed-input"), input);
    assert!(
        evidence
            .items
            .iter()
            .all(|item| !item.reference.starts_with("original-input"))
    );
}

#[test]
fn malformed_new_and_legacy_preparations_do_not_hide_other_findings() {
    let (store, definition, prepared, evaluated) = fixture();
    for (branch, position) in [("damaged", 1), ("usable", 2)] {
        accept_evaluated_with_message(
            &store,
            &definition,
            branch,
            position,
            Some(StatusResult::Failure),
            None,
            &evaluated,
        );
        save_preparation(&store, position, &prepared);
    }
    let new_record = serde_json::to_value(PreparationRecord::Ready(prepared)).unwrap();
    for malformed in [
        "{\"status\":\"ready\",\"profile\":\"RAW_CANARY\",\"input\":{},\"input\":{},\"request\":null,\"report\":{\"omissions\":[]}}".into(),
        "{\"status\":\"ready\",\"profile\":\"RAW_CANARY\",\"input\":{},\"request\":null,\"report\":{\"omissions\":[]},\"version\":99}".into(),
        {
            let mut value = new_record.clone();
            value.as_object_mut().unwrap().remove("evaluation_budget");
            value.to_string()
        },
    ] {
        store.lock().execute("UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = 1", [&malformed]).unwrap();
        let report = rows(&store, std::slice::from_ref(&definition));
        assert_eq!(report[0].finding, 2);
        assert!(report[0].diagnostics.iter().any(|diagnostic| diagnostic.code == "storage-failed"));
        assert!(!serde_json::to_string(&report).unwrap().contains("RAW_CANARY"));
        for target in &report[0].targets {
            let evidence = read_evidence(&store, std::slice::from_ref(&definition), &request(), &target.id).unwrap();
            assert_eq!(evidence.status, if target.session.session_id == "damaged" { BurnCheckEvidenceStatus::Unavailable } else { BurnCheckEvidenceStatus::Available });
        }
    }
}

#[test]
fn per_decision_failure_details_use_authored_paths_without_provider_detail() {
    let (store, definition, _, evaluated) = fixture();
    accept_evaluated_with_message(&store, &definition, "failed", 1, None, None, &evaluated);
    set_latest_failure(
        &store,
        "failed",
        Some(json!({
            "status": "rejected", "code": "invalid_request", "detail": "RAW_PROVIDER_CANARY",
            "report": {"omissions": [{"path": "note", "strategy": "truncate", "original_count": 1000, "retained_count": 256}]},
            "context": {"decision": "second", "field": "decisions.second.output.verdict.criteria"}
        })),
        None,
    );
    let report = rows(&store, &[definition]);
    let diagnostic = &report[0].diagnostics[0];
    assert!(
        diagnostic
            .details
            .iter()
            .any(|detail| detail.label == "Decision" && detail.value == "second")
    );
    assert!(
        diagnostic
            .details
            .iter()
            .any(|detail| detail.label == "Field"
                && detail.value == "decisions.second.output.verdict.criteria")
    );
    assert_eq!(
        diagnostic
            .details
            .iter()
            .filter(|detail| detail.label == "Decision")
            .count(),
        1
    );
    assert!(
        !serde_json::to_string(diagnostic)
            .unwrap()
            .contains("RAW_PROVIDER_CANARY")
    );
}

#[test]
fn a_completed_sibling_does_not_stop_checking_before_the_candidate_settles() {
    let (store, definition, prepared, evaluated) = fixture();
    accept_evaluated_with_message(&store, &definition, "running", 1, None, None, &evaluated);
    save_preparation(&store, 1, &prepared);
    store
        .lock()
        .execute("DELETE FROM burn_check_assessment_history", [])
        .unwrap();
    store
        .lock()
        .execute("UPDATE burn_check_candidate SET state = 'claimed'", [])
        .unwrap();
    store.lock().execute(
        "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name,
            attempt_number, state, outcome, response_json, updated_at_epoch)
         SELECT 'first-attempt', candidate_id, 'first', 1, 'settled', 'answer', ?1, 100 FROM burn_check_candidate",
        [serde_json::to_string(&evaluated.decisions()[0]).unwrap()],
    ).unwrap();
    assert_eq!(
        rows(&store, std::slice::from_ref(&definition))[0].checking_count,
        1
    );
    assert!(current_sessions(&store.lock(), &definition, &request()).unwrap()[0].checking);
    let branches = current_branches(&store.lock(), &definition, &request()).unwrap();
    assert!(branches[0].checking);
    assert!(branches[0].has_started);
    store
        .lock()
        .execute("UPDATE burn_check_candidate SET state = 'dispatching'", [])
        .unwrap();
    store.lock().execute(
        "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name,
            attempt_number, state, updated_at_epoch)
         SELECT 'second-attempt', candidate_id, 'second', 1, 'dispatch_intent', 101 FROM burn_check_candidate", [],
    ).unwrap();
    assert_eq!(
        rows(&store, std::slice::from_ref(&definition))[0].checking_count,
        1
    );
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET state = 'execution_failed'",
            [],
        )
        .unwrap();
    let report = rows(&store, &[definition]);
    assert_eq!(report[0].checking_count, 0);
    assert!(report[0].has_started);
}

#[test]
fn invalid_status_writing_actions_withhold_only_the_damaged_branch() {
    for status in [StatusResult::Failure, StatusResult::Success] {
        let opposite = match status {
            StatusResult::Failure => StatusResult::Success,
            StatusResult::Success => StatusResult::Failure,
        };
        for actions in [
            json!({}),
            json!({"current_status": opposite, "outcomes": [{"index": 0,
                "outcome": {"Applied": {"result": status, "message": "DAMAGED_WRITER"}}}]}),
            json!({"current_status": status, "outcomes": [{"index": 0,
                "outcome": {"Applied": {"result": opposite, "message": "DAMAGED_WRITER"}}}]}),
        ] {
            let (store, definition) = populated();
            let healthy = rows(&store, std::slice::from_ref(&definition)).remove(0);
            accept_with_message(
                &store,
                &definition,
                "damaged",
                4,
                Some(status),
                Some("DAMAGED_WRITER"),
            );
            let valid = rows(&store, std::slice::from_ref(&definition));
            assert_eq!(
                valid[0].finding,
                1 + u64::from(status == StatusResult::Failure)
            );
            assert_eq!(
                valid[0].clean,
                1 + u64::from(status == StatusResult::Success)
            );
            let damaged_target = valid[0]
                .targets
                .iter()
                .find(|target| target.session.session_id == "damaged")
                .map(|target| target.id.clone());
            store.lock().execute(
                "UPDATE burn_check_assessment_history SET actions_json = ?1 WHERE trigger_position = 4",
                [actions.to_string()],
            ).unwrap();

            let changes = store.lock().total_changes();
            let report = rows(&store, std::slice::from_ref(&definition)).remove(0);
            assert_eq!(store.lock().total_changes(), changes);
            assert_eq!(
                (report.finding, report.clean, report.unavailable),
                (1, 1, 1)
            );
            assert_eq!(report.explanation, healthy.explanation);
            assert_eq!(report.targets.len(), 1);
            assert_eq!(report.targets[0].id, healthy.targets[0].id);
            assert_eq!(
                report.targets[0].observation,
                healthy.targets[0].observation
            );
            assert!(report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "storage-failed" && diagnostic.affected_session_count == 1
            }));
            assert!(
                !serde_json::to_string(&report)
                    .unwrap()
                    .contains("DAMAGED_WRITER")
            );
            if let Some(target) = damaged_target {
                assert_eq!(
                    read_evidence(
                        &store,
                        std::slice::from_ref(&definition),
                        &request(),
                        &target
                    )
                    .unwrap()
                    .status,
                    BurnCheckEvidenceStatus::Unavailable
                );
            }
            assert_eq!(
                read_evidence(&store, &[definition], &request(), &report.targets[0].id)
                    .unwrap()
                    .status,
                BurnCheckEvidenceStatus::Available
            );
        }
    }
}

#[test]
fn invalid_later_no_write_actions_preserve_the_valid_writer_and_other_branches() {
    for status in [StatusResult::Failure, StatusResult::Success] {
        let (store, definition) = populated();
        accept_with_message(
            &store,
            &definition,
            "retained",
            4,
            Some(status),
            Some("Keep the valid writer."),
        );
        accept(&store, &definition, "retained", 5, None);
        let before = rows(&store, std::slice::from_ref(&definition)).remove(0);
        store.lock().execute(
            "UPDATE burn_check_assessment_history SET actions_json = '{}' WHERE trigger_position = 5",
            [],
        ).unwrap();

        let changes = store.lock().total_changes();
        let report = rows(&store, std::slice::from_ref(&definition)).remove(0);
        assert_eq!(store.lock().total_changes(), changes);
        assert_eq!(
            (report.finding, report.clean, report.unavailable),
            (before.finding, before.clean, 0)
        );
        assert_eq!(report.explanation, before.explanation);
        assert_eq!(
            report
                .targets
                .iter()
                .map(|target| &target.id)
                .collect::<Vec<_>>(),
            before
                .targets
                .iter()
                .map(|target| &target.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            report
                .targets
                .iter()
                .map(|target| &target.observation)
                .collect::<Vec<_>>(),
            before
                .targets
                .iter()
                .map(|target| &target.observation)
                .collect::<Vec<_>>()
        );
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "storage-failed" && diagnostic.affected_session_count == 1
        }));
        for target in &report.targets {
            assert_eq!(
                read_evidence(
                    &store,
                    std::slice::from_ref(&definition),
                    &request(),
                    &target.id
                )
                .unwrap()
                .status,
                BurnCheckEvidenceStatus::Available
            );
        }
    }
}
