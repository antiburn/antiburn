use std::path::Path;
use std::sync::Arc;

use antiburn_checks::actions::{ActionOutcome, ActionOutcomeKind, ActionPlan, ChoiceResult};
use antiburn_checks::check_store::{AcceptanceOutcome, CheckStore, EvaluatedCandidate};
use antiburn_checks::compiler::{StatusResult, compile};
use antiburn_checks::definition_lifecycle::{EffectiveActivation, LifecycleTransition};
use antiburn_checks::discovery::DefinitionScope;
use antiburn_checks::domain::{
    ActivationId, BranchId, FrozenContext, Generation, Message, ObservationPosition,
};
use serde_json::json;

use super::*;
use crate::store::burn_checks::{BurnCheckCandidateClaim, StoreCheckStore};

pub(crate) fn definition(project: &str) -> InstalledCheckDefinition {
    InstalledCheckDefinition {
        effective_id: format!("private:{project}:retry"),
        check_id: "failed-retry-loop".into(),
        activation_scope_key: serde_json::to_string(&scope(project)).unwrap(),
        generation: Some(1),
        definition_revision: "01".repeat(32),
        name: "Failed retry loop".into(),
        aliases: vec!["repeated failures".into()],
        description: Some("Detects repeated failures without recovery.".into()),
        decision_name: Some("review".into()),
        trigger: Some(InstalledCheckTrigger {
            event: InstalledCheckEvent::ToolResult,
            minimum: 1,
        }),
        source: InstalledCheckSource {
            scope: InstalledCheckScope::Project,
            path: Some(".antiburn/checks/retries.yaml".into()),
        },
        availability: InstalledCheckAvailability::Available,
        diagnostic: None,
        diagnostics: Vec::new(),
        definition_sources: Vec::new(),
    }
}

fn scope(project: &str) -> ActivationScope {
    ActivationScope {
        environment: "native".into(),
        session_cwd: project.into(),
        run: RunScope::Live,
    }
}

fn request() -> ReportRequest {
    ReportRequest {
        environment_key: "native".into(),
        window: antiburn_local::insights::ReportWindow {
            start_epoch: 90,
            end_epoch: 200,
        },
        computed_at_epoch: 200,
    }
}

fn store() -> Arc<Store> {
    Arc::new(Store::open_in_memory(Path::new("/tmp/check-facade-test")).unwrap())
}

fn activate(store: &Arc<Store>, definition: &InstalledCheckDefinition) {
    let scope: ActivationScope = serde_json::from_str(&definition.activation_scope_key).unwrap();
    let adapter = StoreCheckStore::new(store.clone(), "test".into(), scope.clone()).unwrap();
    let activation = EffectiveActivation {
        id: ActivationId {
            scope: scope.clone(),
            definition_id: definition.check_id.clone(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Project,
        source_paths: vec![],
        project_root: Some(scope.session_cwd.into()),
        compiled: Arc::new(compile("name: Report fixture\non:\n  tool_result:\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input: {activity: \"${{ message.tool.result.status }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {clear: Clear., concern: Concern.}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: Default::default(),
    };
    adapter
        .activate(
            &definition.check_id,
            &LifecycleTransition::Activated(activation.clone()),
            Some(&activation),
        )
        .unwrap();
    let mut connection = store.lock();
    let mut runtime_definitions = connection
        .prepare(
            "SELECT check_id, activation_scope_key, generation, definition_revision, name, availability
             FROM burn_check_runtime_definition",
        )
        .unwrap()
        .query_map([], |row| {
            let availability: String = row.get(5)?;
            Ok(antiburn_checks_store::RuntimeDefinition {
                check_id: row.get(0)?,
                activation_scope_key: row.get(1)?,
                generation: row.get(2)?,
                definition_revision: row.get(3)?,
                name: row.get(4)?,
                availability: match availability.as_str() {
                    "paused" => antiburn_checks_store::DefinitionAvailability::Paused,
                    "invalid" => antiburn_checks_store::DefinitionAvailability::Invalid,
                    "unsupported" => antiburn_checks_store::DefinitionAvailability::Unsupported,
                    _ => antiburn_checks_store::DefinitionAvailability::Available,
                },
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    runtime_definitions.retain(|saved| {
        saved.check_id != definition.check_id
            || saved.activation_scope_key != definition.activation_scope_key
    });
    runtime_definitions.push(antiburn_checks_store::RuntimeDefinition {
        check_id: definition.check_id.clone(),
        activation_scope_key: definition.activation_scope_key.clone(),
        generation: definition.generation,
        definition_revision: definition.definition_revision.clone(),
        name: definition.name.clone(),
        availability: antiburn_checks_store::DefinitionAvailability::Available,
    });
    antiburn_checks_store::replace_runtime_definitions(&mut connection, &runtime_definitions)
        .unwrap();
}

fn session(store: &Store, id: &str, epoch: i64) {
    session_in_project(store, id, epoch, "/workspace/demo");
}

fn session_in_project(store: &Store, id: &str, epoch: i64, project: &str) {
    store.lock().execute(
        "INSERT OR IGNORE INTO session (environment_key, agent, session_id, source_kind, source_label,
          cwd, updated_at_epoch, started_at_epoch, first_seen_at, last_seen_at, incarnation)
          VALUES ('native', 'claude-code', ?1, 'local', 'test', ?3, ?2, ?2, '', '', 7)",
        params![id, epoch, project],
    ).unwrap();
    store
        .lock()
        .execute(
            "INSERT OR IGNORE INTO session_evidence (environment_key, agent, session_id, status,
          analyzed_generation, parser_revision, analyzer_revision, evidence_schema_revision)
          VALUES ('native', 'claude-code', ?1, 'ready', 0, ?2, ?3, ?4)",
            params![
                id,
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION
            ],
        )
        .unwrap();
}

fn accept(
    store: &Store,
    definition: &InstalledCheckDefinition,
    branch: &str,
    position: i64,
    status: Option<StatusResult>,
) {
    accept_with_message(
        store,
        definition,
        branch,
        position,
        status,
        Some("Review the recorded work."),
    );
}

fn accept_with_message(
    store: &Store,
    definition: &InstalledCheckDefinition,
    branch: &str,
    position: i64,
    status: Option<StatusResult>,
    message: Option<&str>,
) {
    let scope: ActivationScope = serde_json::from_str(&definition.activation_scope_key).unwrap();
    session_in_project(store, branch, 100, &scope.session_cwd);
    let id = format!("{}:{branch}:{position}", definition.activation_scope_key);
    let label = if status == Some(StatusResult::Success) {
        "clear"
    } else {
        "concern"
    };
    let evaluated = EvaluatedCandidate {
        choices: vec![
            ChoiceResult::validate(
                "review",
                "verdict",
                &["clear".into(), "concern".into()],
                label,
                BTreeMap::from([
                    ("clear".into(), if label == "clear" { 0.99 } else { 0.01 }),
                    (
                        "concern".into(),
                        if label == "concern" { 0.99 } else { 0.01 },
                    ),
                ]),
            )
            .unwrap(),
        ],
        attempts: 1,
    };
    let plan = ActionPlan {
        current_status: status,
        outcomes: vec![ActionOutcome {
            index: 0,
            outcome: match status {
                Some(result) => ActionOutcomeKind::Applied {
                    result,
                    message: message.map(str::to_owned),
                },
                None => ActionOutcomeKind::SkippedConditionFalse,
            },
        }],
    };
    let frozen = FrozenCandidate {
        activation: ActivationId {
            scope: serde_json::from_str(&definition.activation_scope_key).unwrap(),
            definition_id: definition.check_id.clone(),
            generation: Generation(1),
        },
        branch: BranchId(branch.into()),
        trigger_position: ObservationPosition(position as u64),
        messages: vec![Message {
            id: "tool-1".into(),
            value: json!({"tool":{"result":{"status":"error","content":"test failed"}}}),
        }],
        context: FrozenContext {
            value: json!({"session":{"requests":[{"content":"Run the tests"}]}}),
            dependencies: vec![],
        },
        dependencies: vec![],
        fingerprint: [0; 32],
    };
    store.lock().execute(
        "INSERT INTO burn_check_candidate (candidate_id, check_id, activation_scope_key, generation, branch_key,
           session_key_json, trigger_position, selected_publication_head, dependencies_json, frozen_input_json,
           answer_json, input_fingerprint, state, claim_owner, claim_fence, created_at_epoch, updated_at_epoch)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, 0, '{}', ?7, ?8, ?1, 'answered', 'test', 1, 100, 100)",
        params![id, definition.check_id, definition.activation_scope_key, branch, serde_json::to_string(&("native", "claude-code", branch)).unwrap(), position, serde_json::to_string(&frozen).unwrap(), serde_json::to_string(&evaluated).unwrap()],
    ).unwrap();
    assert_eq!(
        store
            .try_accept_burn_check_answer(
                &BurnCheckCandidateClaim {
                    candidate_id: id,
                    owner: "test".into(),
                    fence: 1
                },
                &format!("assessment-{position}"),
                &serde_json::to_value(&evaluated).unwrap(),
                &serde_json::to_value(&plan).unwrap(),
                status.map(|value| match value {
                    StatusResult::Failure => "failure",
                    StatusResult::Success => "success",
                }),
                101 + position
            )
            .unwrap(),
        AcceptanceOutcome::Accepted
    );
}

fn populated() -> (Arc<Store>, InstalledCheckDefinition) {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(
        &store,
        &definition,
        "branch-a",
        1,
        Some(StatusResult::Failure),
    );
    accept(&store, &definition, "branch-a", 2, None);
    accept(
        &store,
        &definition,
        "branch-b",
        3,
        Some(StatusResult::Success),
    );
    (store, definition)
}

fn rows(
    store: &Store,
    definitions: &[InstalledCheckDefinition],
) -> Vec<InstalledCheckReportPayload> {
    read_installed_checks_in_window(store, definitions, &request()).unwrap()
}

fn queue(store: &Store, definition: &InstalledCheckDefinition, id: &str, position: i64) {
    session(store, id, 190);
    store.lock().execute(
        "INSERT INTO burn_check_candidate (candidate_id, check_id, activation_scope_key, generation,
          branch_key, session_key_json, trigger_position, selected_publication_head, dependencies_json,
          frozen_input_json, input_fingerprint, state, created_at_epoch, updated_at_epoch)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, 0, '{}', '{}', ?1, 'pending', 190, 190)",
        params![format!("queued:{id}:{position}"), definition.check_id, definition.activation_scope_key,
            id, serde_json::to_string(&("native", "claude-code", id)).unwrap(), position],
    ).unwrap();
}

#[test]
fn has_started_counts_first_queued_work_without_an_attempt_or_status() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    assert!(!rows(&store, std::slice::from_ref(&definition))[0].has_started);
    queue(&store, &definition, "first", 1);
    let before = store.lock().total_changes();
    let report = rows(&store, &[definition]);
    assert_eq!(store.lock().total_changes(), before);
    assert!(report[0].has_started);
    assert_eq!(report[0].checking_count, 1);
    assert_eq!((report[0].finding, report[0].clean), (0, 0));
    let saved: i64 = store
        .lock()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM burn_check_evaluation_attempt)
          + (SELECT COUNT(*) FROM burn_check_assessment_history)
          + (SELECT COUNT(*) FROM burn_check_current_status)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, 0);
}

#[test]
fn has_started_retains_uncertain_assessments_while_waiting_and_resets_with_generation() {
    let store = store();
    let mut definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "uncertain", 1, None);
    for epoch in [200, 1_000] {
        let mut request = request();
        request.computed_at_epoch = epoch;
        let report =
            read_installed_checks_in_window(&store, std::slice::from_ref(&definition), &request)
                .unwrap();
        assert!(report[0].has_started);
        assert_eq!(report[0].checking_count, 0);
        assert_eq!(report[0].reviewed_without_status, 1);
        assert_eq!((report[0].finding, report[0].clean), (0, 0));
    }
    store
        .lock()
        .execute("UPDATE burn_check_activation SET generation = 2", [])
        .unwrap();
    definition.generation = Some(2);
    assert!(!rows(&store, &[definition])[0].has_started);
}

#[test]
fn has_started_preserves_failed_status_while_rechecking() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(
        &store,
        &definition,
        "failed",
        1,
        Some(StatusResult::Failure),
    );
    queue(&store, &definition, "failed", 2);
    let report = rows(&store, &[definition]);
    assert!(report[0].has_started);
    assert_eq!(report[0].checking_count, 1);
    assert_eq!((report[0].finding, report[0].clean), (1, 0));
}

#[test]
fn has_started_distinguishes_cancelled_queue_from_a_dispatched_attempt() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    queue(&store, &definition, "cancelled", 1);
    store
        .lock()
        .execute("UPDATE burn_check_candidate SET state = 'cancelled'", [])
        .unwrap();
    assert!(!rows(&store, std::slice::from_ref(&definition))[0].has_started);
    store.lock().execute(
        "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, attempt_number, state, updated_at_epoch)
         VALUES ('attempt', 'queued:cancelled:1', 1, 'prepared', 190)", [],
    ).unwrap();
    assert!(!rows(&store, std::slice::from_ref(&definition))[0].has_started);
    store
        .lock()
        .execute(
            "UPDATE burn_check_evaluation_attempt SET state = 'dispatch_intent'",
            [],
        )
        .unwrap();
    assert!(rows(&store, std::slice::from_ref(&definition))[0].has_started);
    store
        .lock()
        .execute(
            "INSERT INTO burn_check_assessment_history
         SELECT 'cancelled-history', candidate_id, check_id, activation_scope_key, generation,
           branch_key, trigger_position, 'cancelled', NULL, '{}', 0, 190 FROM burn_check_candidate",
            [],
        )
        .unwrap();
    assert!(rows(&store, std::slice::from_ref(&definition))[0].has_started);
    store
        .lock()
        .execute("UPDATE burn_check_candidate SET state = 'stale'", [])
        .unwrap();
    assert!(!rows(&store, &[definition])[0].has_started);
}

#[test]
fn has_started_excludes_invalid_evidence_and_out_of_cohort_history() {
    for invalidation in [
        "UPDATE burn_check_assessment_history SET evidence_valid = 0",
        "UPDATE session SET source_generation = source_generation + 1",
        "UPDATE session SET updated_at_epoch = 200",
        "UPDATE session SET updated_at_epoch = 89",
        "UPDATE session SET cwd = '/workspace/other'",
        "UPDATE burn_check_activation SET enabled = 0",
        "UPDATE burn_check_activation SET definition_revision = 'changed'",
        "DELETE FROM session",
    ] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        accept(&store, &definition, "uncertain", 1, None);
        assert!(rows(&store, std::slice::from_ref(&definition))[0].has_started);
        store.lock().execute(invalidation, []).unwrap();
        assert!(
            !rows(&store, &[definition])[0].has_started,
            "{invalidation}"
        );
    }
}

#[test]
fn rust_facade_matches_frontend_fixture_and_retains_the_actual_failure_write() {
    let (store, definition) = populated();
    let before = store.lock().total_changes();
    let rows = rows(&store, &[definition]);
    assert_eq!(store.lock().total_changes(), before);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../src/views/main-window/burn-checks/tests/fixtures/installed-check-report.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(&rows[0]).unwrap(), expected);
    let evidence = read_evidence(
        &store,
        &[self::definition("/workspace/demo")],
        &request(),
        &rows[0].targets[0].id,
    )
    .unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert_eq!(evidence.items.len(), 2);
    assert!(evidence.items[0].excerpt.contains("Run the tests"));
    assert_eq!(
        evidence.items[0].source_label,
        "Original source: requested task"
    );
    assert!(evidence.items.iter().all(|item| {
        item.limitation
            .as_deref()
            .is_some_and(|text| text.contains("Exact assessed input was not saved"))
    }));
}

#[test]
fn ready_session_without_a_candidate_waits_for_results_without_checking() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    session(&store, "active-session", 190);
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].checking_count, 0);
    assert!(!report[0].has_started);
    assert_eq!(report[0].waiting_for_results_count, 1);
    assert_eq!(report[0].waiting_for_data_count, 0);
    assert_eq!(
        (report[0].finding, report[0].clean, report[0].review_failed),
        (0, 0, 0)
    );
}

#[test]
fn exposes_a_candidate_that_failed_without_a_status_write() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "session-failed", 1, None);
    let reviewed = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(reviewed[0].reviewed_without_status, 1);
    assert!(reviewed[0].has_started);
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET state = 'execution_failed' WHERE branch_key = 'session-failed'",
            [],
        )
        .unwrap();
    store.lock().execute("UPDATE burn_check_assessment_history SET outcome = 'execution_failed', evidence_valid = 0 WHERE branch_key = 'session-failed'", []).unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].review_failed, 1);
    assert!(report[0].has_started);
    assert_eq!(report[0].reviewed_without_status, 0);
    assert_eq!(report[0].checking_count, 0);
    assert_eq!(report[0].waiting_for_results_count, 1);
}

#[test]
fn applied_status_without_a_message_clears_the_previous_report_message() {
    for status in [StatusResult::Success, StatusResult::Failure] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        accept(
            &store,
            &definition,
            "message-cleared",
            1,
            Some(StatusResult::Failure),
        );
        let before = rows(&store, std::slice::from_ref(&definition));
        assert_eq!(
            before[0].explanation.as_deref(),
            Some("Review the recorded work.")
        );
        assert_eq!(before[0].targets.len(), 1);
        assert_eq!(
            before[0].targets[0].observation.as_deref(),
            Some("Review the recorded work.")
        );

        accept_with_message(
            &store,
            &definition,
            "message-cleared",
            2,
            Some(status),
            None,
        );
        let after = rows(&store, std::slice::from_ref(&definition));
        assert_eq!(after[0].explanation, None, "{status:?}");
        assert_eq!(after[0].finding, u64::from(status == StatusResult::Failure));
        assert_eq!(after[0].clean, u64::from(status == StatusResult::Success));
        assert_eq!(after[0].reviewed_without_status, 0);
        if status == StatusResult::Failure {
            assert_eq!(after[0].targets.len(), 1);
            assert_eq!(after[0].targets[0].observation, None);
            let target: serde_json::Value = serde_json::from_str(&after[0].targets[0].id).unwrap();
            assert_eq!(target["assessment"], "assessment-2");
        } else {
            assert!(after[0].targets.is_empty());
        }
    }
}

#[test]
fn retained_status_and_latest_assessment_are_independent() {
    for status in [StatusResult::Failure, StatusResult::Success] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        accept(&store, &definition, "retained", 1, Some(status));
        accept(&store, &definition, "retained", 2, None);
        let no_write = rows(&store, std::slice::from_ref(&definition));
        assert_eq!(no_write[0].reviewed_without_status, 1);
        assert_eq!(
            no_write[0].finding,
            u64::from(status == StatusResult::Failure)
        );
        assert_eq!(
            no_write[0].clean,
            u64::from(status == StatusResult::Success)
        );
        assert_eq!(
            no_write[0].explanation.as_deref(),
            Some("Review the recorded work.")
        );
        if status == StatusResult::Failure {
            assert_eq!(no_write[0].targets.len(), 1);
            assert_eq!(
                no_write[0].targets[0].observation.as_deref(),
                Some("Review the recorded work.")
            );
        }
        store.lock().execute("UPDATE burn_check_candidate SET state = 'execution_failed' WHERE trigger_position = 2", []).unwrap();
        store.lock().execute("UPDATE burn_check_assessment_history SET outcome = 'execution_failed', evidence_valid = 0, actions_json = '{\"detail\":\"failed\"}' WHERE trigger_position = 2", []).unwrap();
        let failed = rows(&store, std::slice::from_ref(&definition));
        assert_eq!(failed[0].review_failed, 1);
        assert_eq!(failed[0].reviewed_without_status, 0);
        assert_eq!(failed[0].finding, no_write[0].finding);
        assert_eq!(failed[0].clean, no_write[0].clean);
        assert_eq!(failed[0].checking_count, 0);
    }
}

#[test]
fn checking_uses_any_queued_or_in_flight_candidate_and_stops_at_terminal_states() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "active", 1, None);
    accept(&store, &definition, "active", 2, None);
    for state in ["pending", "claimed", "dispatching", "answered"] {
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET state = ?1 WHERE trigger_position = 1",
                [state],
            )
            .unwrap();
        let report = rows(&store, std::slice::from_ref(&definition));
        assert_eq!(report[0].checking_count, 1, "{state}");
        assert_eq!(report[0].waiting_for_results_count, 0);
    }
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1",
            [],
        )
        .unwrap();
    let stale_but_running = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(stale_but_running[0].checking_count, 1);
    assert_eq!(stale_but_running[0].waiting_for_data_count, 0);
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation - 1",
            [],
        )
        .unwrap();
    for state in [
        "accepted",
        "execution_failed",
        "not_assessed",
        "cancelled",
        "stale",
        "coalesced",
    ] {
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET state = ?1 WHERE trigger_position = 1",
                [state],
            )
            .unwrap();
        assert_eq!(
            rows(&store, std::slice::from_ref(&definition))[0].checking_count,
            0,
            "{state}"
        );
    }
}

#[test]
fn stale_session_evidence_waits_for_data_and_cannot_expose_retained_status_or_evidence() {
    let (store, definition) = populated();
    let target = rows(&store, std::slice::from_ref(&definition))[0].targets[0]
        .id
        .clone();
    store.lock().execute("UPDATE session SET source_generation = source_generation + 1 WHERE session_id = 'branch-a'", []).unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!((report[0].finding, report[0].clean), (0, 1));
    assert_eq!(report[0].checking_count, 0);
    assert_eq!(report[0].waiting_for_data_count, 1);
    assert_eq!(report[0].waiting_for_results_count, 1);
    assert_eq!(report[0].reviewed_without_status, 0);
    assert_eq!(
        read_evidence(&store, &[definition], &request(), &target)
            .unwrap()
            .status,
        BurnCheckEvidenceStatus::Unavailable
    );
}

#[test]
fn disabled_activation_exposes_no_status_checking_or_waiting_counts_with_old_metadata() {
    let (store, definition) = populated();
    store
        .lock()
        .execute("UPDATE burn_check_activation SET enabled = 0", [])
        .unwrap();
    let report = rows(&store, &[definition]);
    let row = &report[0];
    assert_eq!(
        (
            row.finding,
            row.clean,
            row.checking_count,
            row.waiting_for_data_count,
            row.waiting_for_results_count
        ),
        (0, 0, 0, 0, 0)
    );
    assert!(row.targets.is_empty());
}

#[test]
fn aggregates_project_winners_once_and_keeps_failures_across_projects() {
    let (store, first) = populated();
    let mut second = definition("/workspace/other");
    second.source.scope = InstalledCheckScope::Global;
    activate(&store, &second);
    accept(
        &store,
        &second,
        "other-session",
        4,
        Some(StatusResult::Success),
    );
    let rows = rows(&store, &[first.clone(), second.clone()]);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].finding, rows[0].clean), (1, 2));
    assert_eq!(availability_rows(&[first, second]).len(), 1);
}

#[test]
fn invalidation_and_revision_changes_remove_status_without_manufacturing_success() {
    let (store, mut definition) = populated();
    let target = rows(&store, &[definition.clone()])[0].targets[0].id.clone();
    store
        .lock()
        .execute(
            "UPDATE burn_check_current_status SET evidence_valid = 0 WHERE branch_key = 'branch-a'",
            [],
        )
        .unwrap();
    let current = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(
        (current[0].finding, current[0].clean, current[0].unavailable),
        (0, 1, 1)
    );
    assert_eq!(
        read_evidence(&store, &[definition.clone()], &request(), &target)
            .unwrap()
            .status,
        BurnCheckEvidenceStatus::Unavailable
    );
    definition.definition_revision = "changed".into();
    let current = rows(&store, &[definition]);
    assert_eq!((current[0].finding, current[0].clean), (0, 0));
}

#[test]
fn native_window_excludes_old_and_deleted_sessions_and_replay_or_remote_definitions() {
    let (store, definition) = populated();
    store
        .lock()
        .execute(
            "UPDATE session SET updated_at_epoch = 89 WHERE session_id = 'branch-a'",
            [],
        )
        .unwrap();
    let current = rows(&store, std::slice::from_ref(&definition));
    assert_eq!((current[0].finding, current[0].clean), (0, 1));
    store
        .lock()
        .execute("DELETE FROM session WHERE session_id = 'branch-b'", [])
        .unwrap();
    assert_eq!(rows(&store, std::slice::from_ref(&definition))[0].clean, 0);
    for scope in [
        ActivationScope {
            environment: "ssh:test".into(),
            ..self::scope("/workspace/demo")
        },
        ActivationScope {
            run: RunScope::Replay("test".into()),
            ..self::scope("/workspace/demo")
        },
    ] {
        let definition = InstalledCheckDefinition {
            activation_scope_key: serde_json::to_string(&scope).unwrap(),
            ..definition.clone()
        };
        assert!(rows(&store, std::slice::from_ref(&definition)).is_empty());
        assert!(availability_rows(&[definition]).is_empty());
    }
}

#[test]
fn exact_counts_do_not_depend_on_the_target_display_cap() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    for position in 1..=(MAX_TARGETS + 2) {
        accept(
            &store,
            &definition,
            &format!("session-{position}"),
            position as i64,
            Some(StatusResult::Failure),
        );
    }
    let current = rows(&store, &[definition]);
    assert_eq!(current[0].finding, (MAX_TARGETS + 2) as u64);
    assert_eq!(current[0].targets.len(), MAX_TARGETS);
    assert!(current[0].targets_truncated);
}

#[test]
fn unavailable_winners_remain_searchable_without_current_results() {
    let (store, mut definition) = populated();
    definition.availability = InstalledCheckAvailability::Invalid {
        reason: Some("Invalid YAML.".into()),
    };
    definition.diagnostics = vec![crate::installed_checks::diagnostics::payload(
        "yaml-syntax",
        InstalledCheckDiagnosticSeverity::Error,
        "Fix the YAML syntax",
        "Check indentation, quotes, and brackets.",
        None,
        Vec::new(),
    )];
    let current = rows(&store, &[definition.clone()]);
    assert_eq!(current[0].finding, 0);
    assert_eq!(current[0].clean, 0);
    assert!(current[0].diagnostic.is_none());
    assert_eq!(current[0].diagnostics[0].code, "yaml-syntax");
    assert_eq!(
        availability_rows(&[definition])[0].effective_id,
        "installed:failed-retry-loop"
    );
}

#[test]
fn installed_snoozes_preserve_native_records_and_never_enter_native_masks() {
    use crate::dto::{CheckSnoozeId, CheckSnoozePayload};
    let store = store();
    let native: CheckSnoozePayload =
        serde_json::from_value(json!({"detector":"oldModelUsage","scope":"check","until":null}))
            .unwrap();
    let installed: CheckSnoozePayload = serde_json::from_value(
        json!({"detector":"installed:failed-retry-loop","scope":"check","until":123000}),
    )
    .unwrap();
    store.set_check_snooze(&native).unwrap();
    store.set_check_snooze(&installed).unwrap();
    assert_eq!(store.burn_check_snoozes().unwrap().len(), 1);
    assert_eq!(store.check_snoozes().unwrap().len(), 2);
    store.clear_check_snooze(&installed.detector).unwrap();
    assert_eq!(store.check_snoozes().unwrap(), vec![native]);
    assert!(serde_json::from_value::<CheckSnoozeId>(json!("installed:../other")).is_err());
}

fn set_latest_failure(
    store: &Store,
    branch: &str,
    preparation: Option<serde_json::Value>,
    diagnostic: Option<serde_json::Value>,
) {
    store.lock().execute(
        "UPDATE burn_check_candidate SET state = 'not_assessed', preparation_json = ?2, diagnostic_json = ?3 WHERE branch_key = ?1
           AND trigger_position = (SELECT MAX(trigger_position) FROM burn_check_candidate WHERE branch_key = ?1)",
        params![branch, preparation.map(|value| value.to_string()), diagnostic.map(|value| value.to_string())],
    ).unwrap();
    store.lock().execute("UPDATE burn_check_assessment_history SET outcome = 'not_assessed', evidence_valid = 0 WHERE branch_key = ?1 AND trigger_position = (SELECT MAX(trigger_position) FROM burn_check_assessment_history WHERE branch_key = ?1)", [branch]).unwrap();
}

#[test]
fn typed_runtime_failures_group_sessions_without_exposing_internal_detail() {
    use antiburn_checks::engine::EvaluationFailureCode::*;
    for (failure, code) in [
        (AuthenticationRejected, "provider-authentication"),
        (RateLimited, "provider-rate-limited"),
        (ProviderOverloaded, "provider-overloaded"),
        (ProviderUnavailable, "provider-unavailable"),
        (DeliveryUnknown, "provider-delivery-unknown"),
        (InvalidResponse, "provider-invalid-response"),
        (InvalidDecision, "provider-invalid-response"),
        (ContextRejected, "provider-context-rejected"),
        (InvalidRequest, "provider-invalid-request"),
        (Cancelled, "assessment-cancelled"),
        (StorageFailure, "storage-failed"),
        (ActionFailed, "action-failed"),
    ] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        for (branch, position) in [("first", 1), ("second", 2)] {
            accept(&store, &definition, branch, position, None);
            set_latest_failure(
                &store,
                branch,
                None,
                Some(json!({"code": failure, "detail": "INTERNAL_CANARY: os error 13"})),
            );
        }
        let report = rows(&store, &[definition]);
        assert_eq!(report[0].diagnostics.len(), 1, "{failure:?}");
        assert_eq!(report[0].diagnostics[0].code, code);
        assert_eq!(report[0].diagnostics[0].affected_session_count, 2);
        assert!(
            report[0].diagnostics[0]
                .details
                .iter()
                .any(|detail| detail.label == "Diagnostic code" && detail.value == code)
        );
        assert!(
            report[0].diagnostics[0]
                .details
                .iter()
                .any(|detail| detail.label == "Decision" && detail.value == "review")
        );
        assert_eq!(report[0].review_failed, 2);
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("INTERNAL_CANARY")
        );
    }
}

#[test]
fn input_size_failure_retains_a_finding_and_clears_after_a_later_assessment() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "main", 1, Some(StatusResult::Failure));
    accept(&store, &definition, "main", 2, None);
    set_latest_failure(
        &store,
        "main",
        Some(json!({
            "status":"rejected", "code":"no_fit", "detail":"INTERNAL_CANARY",
            "context":{"no_fit_reason":"reductions_exhausted"},
            "report":{"omissions":[{"path":"activity", "strategy":"recent", "original_count":10, "retained_count":1}]}
        })),
        None,
    );
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].finding, 1);
    assert_eq!(report[0].targets.len(), 1);
    assert_eq!(report[0].diagnostics[0].code, "input-too-large");
    assert_eq!(
        report[0].diagnostics[0]
            .details
            .iter()
            .find(|detail| detail.label == "activity")
            .unwrap()
            .value,
        "recent: retained 1 of 10 records"
    );
    assert!(
        report[0].diagnostics[0]
            .message
            .contains("Nothing was sent")
    );
    assert_eq!(
        read_evidence(
            &store,
            std::slice::from_ref(&definition),
            &request(),
            &report[0].targets[0].id
        )
        .unwrap()
        .status,
        BurnCheckEvidenceStatus::Available
    );
    accept(&store, &definition, "main", 3, None);
    let repaired = rows(&store, &[definition]);
    assert!(repaired[0].diagnostics.is_empty());
    assert_eq!(repaired[0].finding, 1);
    assert_eq!(repaired[0].review_failed, 0);
}

#[test]
fn shortened_input_is_informational_without_copying_prepared_content() {
    let (store, definition) = populated();
    store.lock().execute("UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = 3", [json!({
        "status":"ready", "profile":"fixture", "input":{"private":"INPUT_CANARY"}, "request":null,
        "report":{"omissions":[{"path":"activity[0].text", "strategy":"truncate", "original_count":4096, "retained_count":1024}]}
    }).to_string()]).unwrap();
    let report = rows(&store, &[definition]);
    let diagnostic = &report[0].diagnostics[0];
    assert_eq!(diagnostic.code, "input-shortened");
    assert_eq!(diagnostic.title, "Input shortened");
    assert_eq!(diagnostic.severity, InstalledCheckDiagnosticSeverity::Info);
    assert_eq!(diagnostic.affected_session_count, 1);
    assert_eq!(
        diagnostic
            .details
            .iter()
            .find(|detail| detail.label == "activity[0].text")
            .unwrap()
            .value,
        "truncate: retained 1024 of 4096 UTF-8 bytes"
    );
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("INPUT_CANARY")
    );
}

#[test]
fn source_failure_is_actionable_and_clears_after_source_recovery() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    session(&store, "source", 100);
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'failed', last_error = 'OS_CANARY'",
            [],
        )
        .unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].diagnostics[0].code, "source-unavailable");
    assert_eq!(report[0].diagnostics[0].affected_session_count, 1);
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("OS_CANARY")
    );
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'ready', last_error = NULL",
            [],
        )
        .unwrap();
    assert!(rows(&store, &[definition])[0].diagnostics.is_empty());
}

#[test]
fn unreadable_diagnostic_data_keeps_prior_findings_and_reports_a_storage_error() {
    let (store, definition) = populated();
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = '{}' WHERE trigger_position = 2",
            [],
        )
        .unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].finding, 1);
    assert_eq!(report[0].targets.len(), 1);
    assert_eq!(report[0].diagnostics[0].code, "storage-failed");
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = NULL",
            [],
        )
        .unwrap();
    assert!(rows(&store, &[definition])[0].diagnostics.is_empty());
}

#[test]
fn fitted_evidence_uses_the_status_assessment_input_without_original_or_later_content() {
    let (store, definition) = populated();
    let input = json!({
        "selected_results": [{"summary":"retained head\n[... omitted ...]\nretained tail"}],
        "custom_context": {"flag":true}
    });
    let report = json!({"omissions":[{
        "path":"selected_results[0].summary", "strategy":"truncate", "original_count":4096, "retained_count":256
    }]});
    store.lock().execute("UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = 1", [json!({
        "status":"ready", "profile":"fixture", "input":input, "request":null, "report":report
    }).to_string()]).unwrap();
    store.lock().execute("UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = 2", [json!({
        "status":"ready", "profile":"fixture", "input":{"wrong":"LATER_INPUT_CANARY"}, "request":null,
        "report":{"omissions":[]}
    }).to_string()]).unwrap();
    let rows = rows(&store, std::slice::from_ref(&definition));
    let evidence =
        read_evidence(&store, &[definition], &request(), &rows[0].targets[0].id).unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert_eq!(evidence.items.len(), 2);
    assert_eq!(evidence.items[0].reference, "assessed-input");
    assert_eq!(evidence.items[0].source_label, "Assessed input");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&evidence.items[0].excerpt).unwrap(),
        input
    );
    assert!(
        evidence.items[0]
            .limitation
            .as_deref()
            .unwrap()
            .contains("not assessed")
    );
    assert_eq!(evidence.items[1].reference, "input-omissions");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&evidence.items[1].excerpt).unwrap(),
        report
    );
    let serialized = serde_json::to_string(&evidence).unwrap();
    for original in ["test failed", "Run the tests", "LATER_INPUT_CANARY"] {
        assert!(!serialized.contains(original));
    }
}

#[test]
fn saved_custom_assessment_input_does_not_imply_a_tool_source_mapping() {
    let (store, definition) = populated();
    let input = json!({"status_only":"error", "fixed":42});
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = 1",
            [json!({
                "status":"ready", "profile":"fixture", "input":input, "request":null,
                "report":{"omissions":[]}
            })
            .to_string()],
        )
        .unwrap();
    let rows = rows(&store, std::slice::from_ref(&definition));
    let evidence =
        read_evidence(&store, &[definition], &request(), &rows[0].targets[0].id).unwrap();
    assert_eq!(evidence.items.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&evidence.items[0].excerpt).unwrap(),
        input
    );
    assert!(evidence.items[0].limitation.is_none());
    assert!(!evidence.items[0].excerpt.contains("tool"));
}

#[test]
fn rejected_preparation_cannot_fall_back_to_original_evidence_as_assessed_input() {
    let (store, definition) = populated();
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET preparation_json = ?1 WHERE trigger_position = 1",
            [json!({
                "status":"rejected", "code":"no_fit", "detail":"RAW_CANARY", "report":null
            })
            .to_string()],
        )
        .unwrap();
    let rows = rows(&store, std::slice::from_ref(&definition));
    let evidence =
        read_evidence(&store, &[definition], &request(), &rows[0].targets[0].id).unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Unavailable);
    assert!(evidence.items.is_empty());
}

#[test]
fn typed_fitting_reasons_show_safe_paths_and_estimates_without_parsing_failure_text() {
    use antiburn_checks::fit::NoFitReason;
    for (reason, title) in [
        (
            NoFitReason::NoRules,
            "Allow input fitting or select less data",
        ),
        (NoFitReason::NoMatchingTargets, "No fitting targets matched"),
        (
            NoFitReason::TargetsAtMinimum,
            "Fitting targets cannot be reduced",
        ),
        (
            NoFitReason::ReductionsExhausted,
            "The input is still too large",
        ),
    ] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        accept(&store, &definition, "main", 1, None);
        set_latest_failure(
            &store,
            "main",
            Some(json!({
                "status":"rejected", "code":"no_fit", "detail":"RAW_FAILURE_CANARY",
                "report":{"omissions":[]},
                "context":{
                    "decision":"saved_review", "field":"decisions.saved_review.fit",
                    "input_path":"activity[*].tool.result.content", "no_fit_reason":reason,
                    "budget":{"kind":"estimated_tokens", "observed":40000, "maximum":28672}
                }
            })),
            None,
        );
        let rows = rows(&store, &[definition]);
        let diagnostic = &rows[0].diagnostics[0];
        assert_eq!(diagnostic.code, "input-too-large");
        assert_eq!(diagnostic.title, title);
        for (label, value) in [
            ("Diagnostic code", "input-too-large"),
            ("Decision", "saved_review"),
            ("Field", "decisions.saved_review.fit"),
            ("Input path", "activity[*].tool.result.content"),
            (
                "Estimated request size",
                "40000 estimated tokens; local limit 28672 estimated tokens",
            ),
        ] {
            assert!(
                diagnostic
                    .details
                    .iter()
                    .any(|detail| detail.label == label && detail.value == value),
                "{label}"
            );
        }
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
                .contains("RAW_FAILURE_CANARY")
        );
    }
}

#[test]
fn local_preparation_limits_do_not_claim_the_request_is_too_large_for_the_model() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "main", 1, None);
    set_latest_failure(
        &store,
        "main",
        Some(json!({
            "status":"rejected", "code":"preparation_resource_limit", "detail":"RAW_FAILURE_CANARY",
            "report":{"omissions":[]},
            "context":{"decision":"review", "field":"decisions.review.input", "budget":{
                "kind":"work_bytes", "observed":67108865, "maximum":67108864
            }}
        })),
        None,
    );
    let rows = rows(&store, &[definition]);
    let diagnostic = &rows[0].diagnostics[0];
    assert_eq!(diagnostic.code, "preparation-resource-limit");
    assert!(diagnostic.message.contains("Nothing was sent"));
    assert!(!diagnostic.message.contains("model budget"));
    assert!(
        diagnostic
            .details
            .iter()
            .any(|detail| detail.label == "Cumulative preparation work"
                && detail.value == "67108865 bytes; local limit 67108864 bytes")
    );
    assert!(
        !serde_json::to_string(diagnostic)
            .unwrap()
            .contains("RAW_FAILURE_CANARY")
    );
}

#[test]
fn legacy_no_fit_without_a_typed_reason_does_not_claim_exhausted_reductions() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "main", 1, None);
    set_latest_failure(
        &store,
        "main",
        Some(json!({
            "status":"rejected", "code":"no_fit", "detail":"legacy local work limit", "report":null
        })),
        None,
    );
    let rows = rows(&store, &[definition]);
    assert_eq!(rows[0].diagnostics[0].code, "preparation-incomplete");
    assert!(
        !rows[0].diagnostics[0]
            .message
            .contains("all permitted reductions")
    );
}

#[test]
fn moving_a_session_removes_old_scope_findings_diagnostics_and_evidence() {
    for cwd in [
        None,
        Some(""),
        Some("/workspace/other"),
        Some("/workspace/demo/subdirectory"),
    ] {
        let (store, definition) = populated();
        let target = rows(&store, std::slice::from_ref(&definition))[0].targets[0]
            .id
            .clone();
        set_latest_failure(
            &store,
            "branch-a",
            None,
            Some(json!({"code":"rate_limited", "detail":"RAW_CANARY"})),
        );
        let before = rows(&store, std::slice::from_ref(&definition));
        assert_eq!((before[0].finding, before[0].review_failed), (1, 1));
        assert_eq!(before[0].diagnostics[0].code, "provider-rate-limited");
        store
            .lock()
            .execute(
                "UPDATE session SET cwd = ?1 WHERE session_id = 'branch-a'",
                [cwd],
            )
            .unwrap();
        let after = rows(&store, std::slice::from_ref(&definition));
        let row = &after[0];
        assert_eq!(
            (
                row.finding,
                row.clean,
                row.unavailable,
                row.review_failed,
                row.checking_count
            ),
            (0, 1, 0, 0, 0),
            "{cwd:?}"
        );
        assert!(row.targets.is_empty());
        assert!(row.diagnostics.is_empty());
        assert_eq!(
            read_evidence(&store, &[definition], &request(), &target)
                .unwrap()
                .status,
            BurnCheckEvidenceStatus::Unavailable
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
    }
}

#[test]
fn moving_a_session_removes_old_scope_clean_and_checking_counts() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "main", 1, Some(StatusResult::Success));
    accept(&store, &definition, "main", 2, None);
    store
        .lock()
        .execute(
            "DELETE FROM burn_check_assessment_history WHERE trigger_position = 2",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET state = 'pending' WHERE trigger_position = 2",
            [],
        )
        .unwrap();
    let before = rows(&store, std::slice::from_ref(&definition));
    assert_eq!((before[0].clean, before[0].checking_count), (1, 1));
    store
        .lock()
        .execute("UPDATE session SET cwd = '/workspace/other'", [])
        .unwrap();
    let after = rows(&store, &[definition]);
    assert_eq!(
        (
            after[0].finding,
            after[0].clean,
            after[0].unavailable,
            after[0].checking_count,
            after[0].waiting_for_data_count,
            after[0].waiting_for_results_count
        ),
        (0, 0, 0, 0, 0, 0)
    );
    assert!(after[0].waiting_for_events.is_none());
}

#[test]
fn a_current_scope_success_is_not_overridden_by_a_previous_project_failure() {
    let store = store();
    let old = definition("/workspace/demo");
    let current = definition("/workspace/other");
    activate(&store, &old);
    activate(&store, &current);
    accept(
        &store,
        &old,
        "moving-session",
        1,
        Some(StatusResult::Failure),
    );
    accept(&store, &old, "moving-session", 2, None);
    set_latest_failure(
        &store,
        "moving-session",
        None,
        Some(json!({"code":"rate_limited", "detail":"RAW_CANARY"})),
    );
    let previous = rows(&store, std::slice::from_ref(&old))[0].targets[0]
        .id
        .clone();
    store
        .lock()
        .execute("UPDATE session SET cwd = '/workspace/other'", [])
        .unwrap();
    accept(
        &store,
        &current,
        "moving-session",
        3,
        Some(StatusResult::Success),
    );
    let definitions = [old, current];
    let report = rows(&store, &definitions);
    assert_eq!(
        (
            report[0].finding,
            report[0].clean,
            report[0].unavailable,
            report[0].review_failed
        ),
        (0, 1, 0, 0)
    );
    assert!(report[0].diagnostics.is_empty());
    assert!(report[0].targets.is_empty());
    assert_eq!(
        read_evidence(&store, &definitions, &request(), &previous)
            .unwrap()
            .status,
        BurnCheckEvidenceStatus::Unavailable
    );
}

#[test]
fn trigger_counts_exclude_old_moved_stale_and_missing_checkpoint_sessions() {
    use antiburn_checks::check_store::SourceProgress;
    use antiburn_checks::domain::{EvidenceVersion, NativeOrderKey};
    use antiburn_checks::selection::{BranchSelection, SelectedMessage};
    let store = store();
    let mut definition = definition("/workspace/demo");
    definition.trigger.as_mut().unwrap().minimum = 3;
    activate(&store, &definition);
    let mut progress = SourceProgress::default();
    for (id, epoch, project, count) in [
        ("old-full-window", 89, "/workspace/demo", 5),
        ("future-full-window", 200, "/workspace/demo", 5),
        ("moved-full-window", 100, "/workspace/other", 5),
        ("stale-full-window", 100, "/workspace/demo", 5),
        ("deleted-full-window", 100, "/workspace/demo", 5),
        ("new-needs-two", 100, "/workspace/demo", 1),
    ] {
        session_in_project(&store, id, epoch, project);
        let branch = BranchId(SessionKey::new("native", "claude-code", id).to_identity_json());
        progress.selection.branches.insert(
            branch,
            BranchSelection {
                results: (0..count)
                    .map(|index| SelectedMessage {
                        version: EvidenceVersion(format!("version-{index}")),
                        native_order: NativeOrderKey(index.to_string()),
                        message: Message {
                            id: format!("result-{index}"),
                            value: json!({"status":"error"}),
                        },
                        complete: true,
                    })
                    .collect(),
                coverage_complete: true,
                identity_limit_reached: false,
            },
        );
    }
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = 1 WHERE session_id = 'stale-full-window'",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "DELETE FROM session WHERE session_id = 'deleted-full-window'",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE burn_check_activation SET checkpoint_json = ?1",
            [progress.encode().unwrap()],
        )
        .unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].waiting_for_events.unwrap().count, 2);
    assert_eq!((report[0].finding, report[0].clean), (0, 0));
    let new_branch =
        BranchId(SessionKey::new("native", "claude-code", "new-needs-two").to_identity_json());
    progress.selection.branches.remove(&new_branch);
    store
        .lock()
        .execute(
            "UPDATE burn_check_activation SET checkpoint_json = ?1",
            [progress.encode().unwrap()],
        )
        .unwrap();
    assert_eq!(
        rows(&store, std::slice::from_ref(&definition))[0]
            .waiting_for_events
            .unwrap()
            .count,
        3
    );
    store
        .lock()
        .execute(
            "UPDATE session SET cwd = '/workspace/other' WHERE session_id = 'new-needs-two'",
            [],
        )
        .unwrap();
    let catching_up = rows(&store, &[definition]);
    assert!(catching_up[0].waiting_for_events.unwrap().catching_up);
    assert_eq!(catching_up[0].waiting_for_results_count, 0);
}

#[test]
fn missing_initial_receipt_reports_catchup_without_a_result_countdown() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    session(&store, "starting", 100);
    store
        .lock()
        .execute("DELETE FROM session_evidence", [])
        .unwrap();

    let report = rows(&store, &[definition]);
    assert_eq!(report[0].waiting_for_data_count, 1);
    assert_eq!(report[0].waiting_for_results_count, 0);
    assert_eq!((report[0].finding, report[0].clean), (0, 0));
    assert!(report[0].waiting_for_events.unwrap().catching_up);
}

#[test]
fn publication_catchup_is_bound_to_activation_scope_and_current_incarnation() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    session(&store, "starting", 100);
    let publish = |cwd: &str, incarnation: i64| {
        store
            .lock()
            .execute(
                "INSERT INTO native_tool_result_publication
                (outcome, environment_key, agent, session_id, source_format, complete,
                 source_generation, claim_fence, project_cwd, session_incarnation)
             VALUES ('coverage_complete', 'native', 'claude-code', 'starting', 'ClaudeJsonl',
                1, 0, 1, ?1, ?2)",
                params![cwd, incarnation],
            )
            .unwrap();
    };
    publish("/workspace/other", 7);
    publish("/workspace/demo", 6);
    let ready = rows(&store, std::slice::from_ref(&definition));
    assert!(!ready[0].waiting_for_events.unwrap().catching_up);
    assert_eq!(ready[0].waiting_for_results_count, 1);

    publish("/workspace/demo", 7);
    let catching_up = rows(&store, std::slice::from_ref(&definition));
    assert!(catching_up[0].waiting_for_events.unwrap().catching_up);
    assert_eq!(catching_up[0].waiting_for_data_count, 1);
    assert_eq!(catching_up[0].waiting_for_results_count, 0);
    assert_eq!((catching_up[0].finding, catching_up[0].clean), (0, 0));

    store
        .lock()
        .execute(
            "UPDATE burn_check_activation SET applied_sequence =
            (SELECT MAX(publication_sequence) FROM native_tool_result_publication)",
            [],
        )
        .unwrap();
    let ready = rows(&store, &[definition]);
    assert!(!ready[0].waiting_for_events.unwrap().catching_up);
    assert_eq!(ready[0].waiting_for_data_count, 0);
    assert_eq!(ready[0].waiting_for_results_count, 1);
}

#[test]
fn deterministic_request_failure_shows_the_typed_criterion_limit() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accept(&store, &definition, "main", 1, None);
    set_latest_failure(
        &store,
        "main",
        Some(json!({
            "status":"rejected", "code":"invalid_request", "detail":"RAW_CANARY",
            "report":null, "context":{"decision":"review", "field":"decisions.review.output.verdict.criteria",
                "budget":{"kind":"criteria", "observed":27, "maximum":26}}
        })),
        None,
    );
    let report = rows(&store, &[definition]);
    let diagnostic = &report[0].diagnostics[0];
    assert_eq!(diagnostic.code, "provider-invalid-request");
    assert!(
        diagnostic
            .details
            .iter()
            .any(|detail| detail.label == "Choice criteria"
                && detail.value == "27 criteria; local limit 26 criteria")
    );
    assert!(diagnostic.message.contains("Nothing was sent"));
    assert!(
        !serde_json::to_string(diagnostic)
            .unwrap()
            .contains("RAW_CANARY")
    );
}

#[test]
fn native_progress_matches_reset_activity_and_admitted_pending_delivery() {
    for agent in ["opencode", "claude-code", "codex", "pi"] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        let journal = agent == "opencode";
        store.lock().execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
              cwd, updated_at_epoch, started_at_epoch, first_seen_at, last_seen_at, incarnation)
             VALUES ('native', ?1, 'main', ?2, 'test', '/workspace/demo', 100, 100, '', '', 7)",
            params![agent, if journal { "providerDb" } else { "file" }],
        ).unwrap();
        let mut request = request();
        request.computed_at_epoch = 280;
        let report = |request: &ReportRequest| {
            read_installed_checks_in_window(&store, std::slice::from_ref(&definition), request)
                .unwrap()
                .remove(0)
        };
        assert_eq!(
            report(&request).waiting_for_data_count,
            1,
            "{agent}: cutoff is inclusive"
        );
        request.computed_at_epoch = 281;
        let stale = report(&request);
        assert_eq!(
            stale.waiting_for_data_count, 0,
            "{agent}: no stale cold catchup"
        );
        assert!(stale.waiting_for_events.is_none());
        if journal {
            assert!(
                store
                    .claim_next_opencode_journal(281, 30)
                    .unwrap()
                    .is_none()
            );
        } else {
            assert!(store.claim_next_native_file(281, 30).unwrap().is_none());
        }
        assert!(
            stale.diagnostics.is_empty(),
            "A missing receipt does not prove a blocked source"
        );

        let table = if journal {
            "opencode_journal_delivery"
        } else {
            "native_file_delivery"
        };
        let extra = if journal {
            "main_session"
        } else {
            "active_until_epoch"
        };
        store
            .lock()
            .execute(
                &format!(
                    "INSERT INTO {table} (environment_key, agent, session_id, incarnation,
              source_label, cwd, parser_revision, pending, {extra})
             VALUES ('native', ?1, 'main', 7, 'test', '/workspace/demo', ?2, 1, 1)"
                ),
                params![agent, PARSER_REVISION],
            )
            .unwrap();
        assert_eq!(
            report(&request).waiting_for_data_count,
            1,
            "{agent}: admitted pending work survives activity expiry"
        );
        if journal {
            assert!(
                store
                    .claim_next_opencode_journal(281, 30)
                    .unwrap()
                    .is_some()
            );
        } else {
            assert!(store.claim_next_native_file(281, 30).unwrap().is_some());
        }
        store
            .lock()
            .execute(&format!("UPDATE {table} SET blocked = 1"), [])
            .unwrap();
        assert_eq!(
            report(&request).waiting_for_data_count,
            0,
            "{agent}: blocked is not pending work"
        );
        store
            .lock()
            .execute(
                &format!("UPDATE {table} SET blocked = 0, parser_revision = -1"),
                [],
            )
            .unwrap();
        assert!(
            report(&request).waiting_for_events.is_none(),
            "{agent}: old parser needs fresh reset admission"
        );
        request.computed_at_epoch = 280;
        assert_eq!(
            report(&request).waiting_for_data_count,
            1,
            "{agent}: active reset can recover"
        );
    }
}
