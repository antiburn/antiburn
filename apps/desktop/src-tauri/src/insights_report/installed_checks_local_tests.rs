use super::*;
use std::task::{Context as TaskContext, Poll, Waker};

use crate::store::native_tool_results::{evidence, observers};
use antiburn_checks::check_store::{CandidateClaim, RecoveryWork};
use antiburn_checks::domain::{EvidenceVersion, NativeOrderKey, ObservationDraft, ObservationKind};
use antiburn_checks::engine::{
    CheckEngine, DecisionEvaluator, DecisionFuture, RunOutcome, StartOutcome,
};
use antiburn_checks::source::{PublicationCursor, SourceBatch, SourceEvent};

const LOCAL_CHECK: &str = "name: Local report fixture\non:\n  tool_result:\nactions:\n  - type: status\n    if: message.tool.arguments.write\n    input:\n      result: failure\n      message: ${{ message.tool.arguments.explanation }}\n";

pub(super) struct NoModel;

impl DecisionEvaluator for NoModel {
    fn preparation_recorded(&self, _: &antiburn_checks::fit::PreparationRecord) {
        panic!("a local check must not report model preparation")
    }

    fn validate_prepared(
        &self,
        _: &EffectiveActivation,
        _: &antiburn_checks::prepared_assessment::PreparedAssessment,
        _: antiburn_checks::engine::PreparationValidationPurpose,
    ) -> Result<(), antiburn_checks::fit::PreparationFailure> {
        panic!("a local check must not validate model preparation")
    }

    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<antiburn_checks::decision::DecisionInput>,
        _: antiburn_checks::expression_eval::EvaluationBudget,
    ) -> Result<
        antiburn_checks::prepared_assessment::PreparedAssessment,
        antiburn_checks::fit::PreparationFailure,
    > {
        panic!("a local check must not prepare a model request")
    }

    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a antiburn_checks::prepared_assessment::PreparedAssessment,
        _: &'a antiburn_checks::decision::DecisionAttempt,
    ) -> DecisionFuture<'a> {
        panic!("a local check must not call a model")
    }
}

fn local_fixture() -> (
    Arc<Store>,
    Arc<StoreCheckStore>,
    EffectiveActivation,
    InstalledCheckDefinition,
) {
    local_fixture_with_source(LOCAL_CHECK)
}

pub(super) fn local_fixture_with_source(
    source: &str,
) -> (
    Arc<Store>,
    Arc<StoreCheckStore>,
    EffectiveActivation,
    InstalledCheckDefinition,
) {
    let store = store();
    let definition = definition("/workspace/demo");
    let scope = scope("/workspace/demo");
    let adapter = Arc::new(
        StoreCheckStore::new(store.clone(), "local-report".into(), scope.clone()).unwrap(),
    );
    let activation = EffectiveActivation {
        id: ActivationId {
            scope,
            definition_id: definition.check_id.clone(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Project,
        source_paths: vec![],
        project_root: Some("/workspace/demo".into()),
        compiled: Arc::new(compile(source).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    };
    assert!(activation.compiled.is_local());
    let definition =
        crate::installed_checks::tests::definition_metadata_for_activation(&activation);
    assert!(definition.is_local);
    assert_eq!(definition.decision_name, None);
    adapter
        .activate(
            &definition.check_id,
            &LifecycleTransition::Activated(activation.clone()),
            Some(&activation),
        )
        .unwrap();
    publish_runtime_definition(&store, &definition);
    (store, adapter, activation, definition)
}

pub(super) fn source_page(
    store: &Store,
    activation: &EffectiveActivation,
    sequence: u64,
    identity: &str,
    revision: u64,
    write: bool,
    text: &str,
) -> SourceBatch {
    session(store, "local-session", 100);
    let boundary = seed_current_claude_origin(store, "local-session");
    let mut page = publication_page(store, &activation.id);
    assert!(!page.has_more);
    let sequence = sequence + u64::try_from(boundary).unwrap();
    assert!(page.next.0 < sequence);
    store.lock().execute(
        "INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key,
          agent, session_id, source_format, native_identity, native_position_json, native_revision,
          payload_json, complete, source_generation, claim_fence, project_cwd, session_incarnation)
         SELECT ?1, 'update', environment_key, agent, session_id, source_format,
           ?2, '{}', ?3, '{}', 1, source_generation, 1, cwd, incarnation
          FROM session WHERE environment_key = 'native' AND agent = 'claude-code'
            AND session_id = 'local-session'",
        params![sequence, identity, revision],
    ).unwrap();
    page.events.push(SourceEvent::Observation {
            source_evidence_version: None,
            cursor: PublicationCursor(sequence),
            identity: identity.into(),
            revision,
            observation: Box::new(ObservationDraft {
                first_eligible_publication: None,
                first_terminal_publication: None,
                branch: BranchId("local-session".into()),
                native_order: NativeOrderKey(identity.into()),
                version: EvidenceVersion(format!("{identity}:{revision}")),
                kind: ObservationKind::ToolResult {
                    message: Message {
                        id: identity.into(),
                        value: json!({"tool": {"arguments": {"write": write, "explanation": text}, "result": {"status": "error", "content": text}}}),
                    },
                    category: "shell".into(),
                    status: "error".into(),
                },
                context: FrozenContext {
                    value: json!({"session": {"requests": [{"text": "Available request."}], "requests_complete": false}, "available_but_unused": text}),
                    dependencies: vec![],
                },
                complete: true,
            }),
        });
    page.next = PublicationCursor(sequence);
    page
}

fn settle(task: antiburn_checks::engine::EvaluationTask<'_>) {
    let mut task = task;
    let Poll::Ready(result) = task
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()))
    else {
        panic!("local actions must settle without model I/O");
    };
    let settled = result.unwrap();
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!(settled.attempts, 0);
}

fn idle_source_page(store: &Store, active: &EffectiveActivation) -> SourceBatch {
    session(store, "idle-session", 100);
    seed_current_claude_origin(store, "idle-session");
    assert_eq!(
        store
            .initialize_session_observers(&active.id, 1_000)
            .unwrap(),
        1
    );
    {
        let mut connection = store.lock();
        let transaction = connection.transaction().unwrap();
        observers::frontier_in(
            &transaction,
            &SessionKey::new("native", "claude-code", "idle-session"),
            "idle-report-receipt",
            true,
            121_000,
        )
        .unwrap();
        transaction.commit().unwrap();
    }
    publication_page(store, &active.id)
}

const INVALID_PROGRESS_BINDINGS: [&str; 3] = [
    "{}",
    r#"{"SessionActivity":{"activity_revision":0}}"#,
    "{bad",
];

fn idle_report_with_siblings() -> (
    Arc<Store>,
    EffectiveActivation,
    InstalledCheckDefinition,
    CheckEngine<StoreCheckStore, NoModel>,
) {
    let (store, adapter, active, mut definition) = local_fixture_with_source(
        "name: Progress data isolation\non:\n  session_idle:\n  cooldown: 0s\nactions:\n  - type: status\n    input:\n      result: ${{ session.id == 'healthy-success' && 'success' || 'failure' }}\n      message: Saved status for ${{ session.id }}.\n",
    );
    definition.trigger.as_mut().unwrap().event = InstalledCheckEvent::SessionIdle;
    let ids = ["healthy-failure", "healthy-success", "idle-session"];
    for id in ids {
        session(&store, id, 100);
        seed_current_claude_origin(&store, id);
    }
    assert_eq!(
        store
            .initialize_session_observers(&active.id, 1_000)
            .unwrap(),
        3
    );
    {
        let mut connection = store.lock();
        let transaction = connection.transaction().unwrap();
        for id in ids {
            observers::frontier_in(
                &transaction,
                &SessionKey::new("native", "claude-code", id),
                "isolation-report-receipt",
                true,
                121_000,
            )
            .unwrap();
        }
        transaction.commit().unwrap();
    }
    let page = publication_page(&store, &active.id);
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    assert_eq!(
        engine.apply_source_page(&active, &page).unwrap().candidates,
        3
    );
    let healthy = ["healthy-failure", "healthy-success"]
        .map(|id| BranchId(SessionKey::new("native", "claude-code", id).to_identity_json()))
        .into_iter()
        .collect();
    for _ in 0..2 {
        let StartOutcome::Started(task) = engine
            .start_one_due_for_branches(&active, 121_001, &healthy)
            .unwrap()
        else {
            panic!("a healthy sibling must start");
        };
        settle(task);
    }
    (store, active, definition, engine)
}

fn damage_idle_dependency(store: &Store, binding: &str) {
    assert_eq!(
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate_dependency SET dependency_binding_json = ?1
         WHERE observation_id LIKE '@activity:%' AND candidate_id IN (
           SELECT candidate_id FROM burn_check_candidate
           WHERE session_key_json = json_array('native', 'claude-code', 'idle-session'))",
                [binding],
            )
            .unwrap(),
        1
    );
}

fn add_native_current_dependency(store: &Store) {
    let connection = store.lock();
    connection
        .execute(
            "INSERT INTO native_tool_result_current
            (environment_key, agent, session_id, source_format, native_identity,
             native_position_json, native_revision, payload_json, complete, is_removed,
             publication_sequence, source_generation, claim_fence, event_kind)
         SELECT environment_key, agent, session_id, source_format, native_identity,
             native_position_json, native_revision, payload_json, complete, 0,
             publication_sequence, source_generation, claim_fence, event_kind
           FROM native_tool_result_publication WHERE native_identity = 'one'",
            [],
        )
        .unwrap();
    let binding = evidence::current_binding_in(
        &connection,
        &SessionKey::new("native", "claude-code", "local-session"),
        "one",
    )
    .unwrap()
    .unwrap();
    let version = binding.version();
    let dependency = serde_json::to_string(&observers::DependencyBinding::NativeCurrent(
        binding.clone(),
    ))
    .unwrap();
    connection
        .execute(
            "INSERT INTO burn_check_candidate_dependency
            (candidate_id, observation_id, session_key_json, source_format, native_revision,
             evidence_version, removed, payload_sha256, event_kind, dependency_binding_json)
         SELECT candidate_id, '@source:one', session_key_json, ?1, ?2, ?3, 0, ?4, 'tool_result', ?5
           FROM burn_check_candidate",
            params![
                binding.source_format,
                binding.revision,
                version.0,
                binding.payload_sha256,
                dependency
            ],
        )
        .unwrap();
}

pub(super) fn start_and_accept(
    engine: &CheckEngine<StoreCheckStore, NoModel>,
    active: &EffectiveActivation,
    now: u64,
) {
    let StartOutcome::Started(task) = engine.start_one_due(active, now).unwrap() else {
        panic!("the local candidate must start");
    };
    settle(task);
}

fn count(store: &Store, table: &str) -> i64 {
    store
        .lock()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn current_target(store: &Store, definition: &InstalledCheckDefinition) -> String {
    let report = rows(store, std::slice::from_ref(definition));
    assert_eq!(
        (report[0].finding, report[0].clean, report[0].review_failed),
        (1, 0, 0)
    );
    assert_eq!(report[0].targets.len(), 1);
    report[0].targets[0].id.clone()
}

#[test]
fn local_checking_starts_only_after_atomic_local_start_without_attempts_or_history() {
    let (store, adapter, active, definition) = local_fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoModel));
    engine
        .apply_source_page(
            &active,
            &source_page(&store, &active, 1, "one", 1, true, "Local finding."),
        )
        .unwrap();
    let before = rows(&store, std::slice::from_ref(&definition));
    assert!(!before[0].has_started);
    assert_eq!(before[0].checking_count, 0);
    let RecoveryWork::Pending { candidate, .. } =
        adapter.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("the source must create a pending candidate");
    };
    let CandidateClaim::Claimed(claimed) = adapter.claim_due(&candidate, 1_001, true).unwrap()
    else {
        panic!("the candidate must be claimable");
    };
    let claimed_report = rows(&store, std::slice::from_ref(&definition));
    assert!(!claimed_report[0].has_started);
    assert_eq!(claimed_report[0].checking_count, 0);
    adapter.release_pre_dispatch_claim(&claimed).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1_001).unwrap() else {
        panic!("the local start must be saved before polling actions");
    };
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
    assert_eq!(count(&store, "burn_check_decision_usage"), 0);
    assert_eq!(count(&store, "burn_check_assessment_history"), 0);
    let changes = store.lock().total_changes();
    let started = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(store.lock().total_changes(), changes);
    assert!(started[0].has_started);
    assert_eq!(started[0].checking_count, 1);
    assert_eq!((started[0].finding, started[0].clean), (0, 0));
    settle(task);
    let accepted = rows(&store, &[definition]);
    assert!(accepted[0].has_started);
    assert_eq!(accepted[0].checking_count, 0);
    assert_eq!(accepted[0].finding, 1);
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
}

#[test]
fn idle_local_start_and_no_write_history_report_started_work() {
    let (store, adapter, active, mut definition) = local_fixture_with_source(
        "name: Idle report\non:\n  session_idle:\nactions:\n  - type: status\n    if: false\n    input: {result: failure}\n",
    );
    definition.trigger.as_mut().unwrap().event = InstalledCheckEvent::SessionIdle;
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    assert_eq!(
        engine
            .apply_source_page(&active, &idle_source_page(&store, &active))
            .unwrap()
            .candidates,
        1
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_candidate_dependency
             WHERE json_type(dependency_binding_json, '$.SessionActivity') = 'object'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    let queued = rows(&store, std::slice::from_ref(&definition)).remove(0);
    assert!(!queued.has_started);
    assert_eq!(queued.checking_count, 0);
    let StartOutcome::Started(task) = engine.start_one_due(&active, 121_001).unwrap() else {
        panic!("the idle check must save its local start");
    };
    let changes = store.lock().total_changes();
    let started = rows(&store, std::slice::from_ref(&definition)).remove(0);
    assert_eq!(store.lock().total_changes(), changes);
    assert!(started.has_started);
    assert_eq!(started.checking_count, 1);
    assert_eq!((started.finding, started.clean), (0, 0));
    assert_eq!(count(&store, "burn_check_assessment_history"), 0);
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
    assert!(started.diagnostics.is_empty());

    settle(task);
    let accepted = rows(&store, &[definition]).remove(0);
    assert!(accepted.has_started);
    assert_eq!(accepted.checking_count, 0);
    assert_eq!(accepted.reviewed_without_status, 1);
    assert_eq!((accepted.finding, accepted.clean), (0, 0));
    assert!(accepted.diagnostics.is_empty());
}

#[test]
fn stale_idle_bindings_hide_local_start_and_history_progress_without_state_changes() {
    for accepted in [false, true] {
        for mutation in [
            "UPDATE burn_check_session_attachment SET activity_revision = activity_revision + 1",
            "UPDATE burn_check_session_attachment SET continuity_eligible = 0",
            "UPDATE burn_check_session_attachment SET observer_epoch = 'changed'",
        ] {
            let (store, adapter, active, definition) = local_fixture_with_source(
                "name: Idle progress\non:\n  session_idle:\nactions:\n  - type: status\n    if: false\n    input: {result: failure}\n",
            );
            let engine = CheckEngine::new(adapter, Arc::new(NoModel));
            engine
                .apply_source_page(&active, &idle_source_page(&store, &active))
                .unwrap();
            let StartOutcome::Started(task) = engine.start_one_due(&active, 121_001).unwrap()
            else {
                panic!("the idle check must start");
            };
            let pending = if accepted {
                settle(task);
                None
            } else {
                Some(task)
            };
            let before = rows(&store, std::slice::from_ref(&definition)).remove(0);
            assert!(before.has_started);
            assert_eq!(before.checking_count, u64::from(!accepted));
            store.lock().execute(mutation, []).unwrap();

            let report = rows(&store, &[definition]).remove(0);
            assert!(!report.has_started, "accepted={accepted}: {mutation}");
            assert_eq!(report.checking_count, 0, "accepted={accepted}: {mutation}");
            assert!(report.diagnostics.is_empty());
            let state: String = store
                .lock()
                .query_row("SELECT state FROM burn_check_candidate", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(state, if accepted { "accepted" } else { "answered" });
            drop(pending);
        }
    }
}

#[test]
fn native_current_bindings_gate_local_start_and_history_progress() {
    for accepted in [false, true] {
        let (store, adapter, active, definition) = local_fixture();
        let engine = CheckEngine::new(adapter, Arc::new(NoModel));
        engine
            .apply_source_page(
                &active,
                &source_page(&store, &active, 1, "one", 1, true, "Native dependency."),
            )
            .unwrap();
        add_native_current_dependency(&store);
        let StartOutcome::Started(task) = engine.start_one_due(&active, 1_001).unwrap() else {
            panic!("the local check must start with its current native dependency");
        };
        let pending = if accepted {
            settle(task);
            None
        } else {
            Some(task)
        };
        let before = rows(&store, std::slice::from_ref(&definition)).remove(0);
        assert!(before.has_started);
        assert_eq!(before.checking_count, u64::from(!accepted));
        store
            .lock()
            .execute(
                "UPDATE native_tool_result_current SET native_revision = native_revision + 1",
                [],
            )
            .unwrap();

        let report = rows(&store, &[definition]).remove(0);
        assert!(!report.has_started, "accepted={accepted}");
        assert_eq!(report.checking_count, 0, "accepted={accepted}");
        assert!(report.diagnostics.is_empty());
        let state: String = store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(state, if accepted { "accepted" } else { "answered" });
        drop(pending);
    }
}

#[test]
fn malformed_progress_bindings_preserve_saved_results_and_sibling_evidence() {
    for accepted in [false, true] {
        for binding in INVALID_PROGRESS_BINDINGS {
            let (store, active, definition, engine) = idle_report_with_siblings();
            let StartOutcome::Started(task) = engine.start_one_due(&active, 121_001).unwrap()
            else {
                panic!("the damaged session must save its local start");
            };
            let pending = if accepted {
                settle(task);
                None
            } else {
                Some(task)
            };
            let before = rows(&store, std::slice::from_ref(&definition)).remove(0);
            assert_eq!((before.finding, before.clean), (1 + u64::from(accepted), 1));
            assert_eq!(before.checking_count, u64::from(!accepted));
            assert_eq!(before.targets.len(), 1 + usize::from(accepted));
            assert!(before.diagnostics.is_empty());
            let evidence = before
                .targets
                .iter()
                .map(|target| {
                    read_evidence(
                        &store,
                        std::slice::from_ref(&definition),
                        &request(),
                        &target.id,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            damage_idle_dependency(&store, binding);

            let changes = store.lock().total_changes();
            let report = rows(&store, std::slice::from_ref(&definition)).remove(0);
            assert_eq!(
                (report.finding, report.clean, report.unavailable),
                (before.finding, before.clean, before.unavailable)
            );
            assert_eq!(report.checking_count, 0);
            assert_eq!(report.explanation, before.explanation);
            assert_eq!(report.targets.len(), before.targets.len());
            assert_eq!(report.diagnostics.len(), 1);
            assert_eq!(report.diagnostics[0].code, "storage-failed");
            assert_eq!(report.diagnostics[0].affected_session_count, 1);
            for ((target, original), expected_evidence) in
                report.targets.iter().zip(&before.targets).zip(&evidence)
            {
                assert_eq!(target.id, original.id);
                assert_eq!(target.observation, original.observation);
                assert_eq!(expected_evidence.status, BurnCheckEvidenceStatus::Available);
                assert_eq!(
                    read_evidence(
                        &store,
                        std::slice::from_ref(&definition),
                        &request(),
                        &target.id
                    )
                    .unwrap(),
                    *expected_evidence,
                );
            }
            let branches = current_branches(&store.lock(), &definition, &request()).unwrap();
            let damaged = branches
                .iter()
                .find(|branch| branch.session_id == "idle-session")
                .unwrap();
            assert!(!damaged.has_started);
            assert!(!damaged.checking);
            assert!(damaged.progress_data_invalid);
            for branch in branches
                .iter()
                .filter(|branch| branch.session_id != "idle-session")
            {
                assert!(branch.has_started);
                assert!(!branch.progress_data_invalid);
                assert_eq!(
                    status_message(branch.actions.as_deref(), branch.status.as_deref().unwrap())
                        .unwrap(),
                    Some(format!("Saved status for {}.", branch.session_id))
                );
            }
            assert_eq!(store.lock().total_changes(), changes);
            drop(pending);
        }
    }
}

#[test]
fn malformed_history_bindings_preserve_valid_progress_on_the_same_branch() {
    for position in [1, 2] {
        for binding in INVALID_PROGRESS_BINDINGS {
            let (store, adapter, active, definition) = local_fixture();
            let engine = CheckEngine::new(adapter, Arc::new(NoModel));
            for (sequence, identity) in [(1, "one"), (2, "two")] {
                engine
                    .apply_source_page(
                        &active,
                        &source_page(
                            &store,
                            &active,
                            sequence,
                            identity,
                            1,
                            true,
                            "Saved local finding.",
                        ),
                    )
                    .unwrap();
                start_and_accept(&engine, &active, sequence * 1_000 + 1);
            }
            assert_eq!(count(&store, "burn_check_assessment_history"), 2);
            let before = rows(&store, std::slice::from_ref(&definition)).remove(0);
            assert!(before.has_started);
            assert!(before.diagnostics.is_empty());
            let target = &before.targets[0];
            let evidence = read_evidence(
                &store,
                std::slice::from_ref(&definition),
                &request(),
                &target.id,
            )
            .unwrap();
            assert_eq!(
                store
                    .lock()
                    .execute(
                        "UPDATE burn_check_candidate_dependency SET dependency_binding_json = ?1
                     WHERE candidate_id IN (SELECT candidate_id FROM burn_check_candidate
                       WHERE trigger_position = ?2)",
                        params![binding, position],
                    )
                    .unwrap(),
                1
            );

            let changes = store.lock().total_changes();
            let damaged_candidate: String = store
                .lock()
                .query_row(
                    "SELECT candidate_id FROM burn_check_candidate WHERE trigger_position = ?1",
                    [position],
                    |row| row.get(0),
                )
                .unwrap();
            let mut saved_data_invalid = false;
            assert!(
                !progress_dependencies_current(
                    &store.lock(),
                    &damaged_candidate,
                    &mut saved_data_invalid,
                )
                .unwrap()
            );
            assert!(saved_data_invalid);
            let report = rows(&store, std::slice::from_ref(&definition)).remove(0);
            assert!(report.has_started);
            assert_eq!(
                (report.finding, report.clean, report.unavailable),
                (1, 0, 0)
            );
            assert_eq!(report.checking_count, 0);
            assert_eq!(report.explanation, before.explanation);
            assert_eq!(report.targets.len(), 1);
            assert_eq!(report.targets[0].id, target.id);
            assert_eq!(report.targets[0].observation, target.observation);
            if position == 2 {
                assert_eq!(report.diagnostics.len(), 1);
                assert_eq!(report.diagnostics[0].code, "storage-failed");
                assert_eq!(report.diagnostics[0].affected_session_count, 1);
            } else {
                assert!(report.diagnostics.is_empty());
            }
            let branches = current_branches(&store.lock(), &definition, &request()).unwrap();
            assert_eq!(branches.len(), 1);
            assert!(branches[0].has_started);
            assert_eq!(branches[0].progress_data_invalid, position == 2);
            assert_eq!(
                read_evidence(&store, &[definition], &request(), &target.id).unwrap(),
                evidence,
            );
            assert_eq!(store.lock().total_changes(), changes);
        }
    }
}

#[test]
fn malformed_local_start_outside_the_report_window_does_not_affect_visible_results() {
    for binding in INVALID_PROGRESS_BINDINGS {
        let (store, active, definition, engine) = idle_report_with_siblings();
        let StartOutcome::Started(task) = engine.start_one_due(&active, 121_001).unwrap() else {
            panic!("the old session must save its local start");
        };
        let before = rows(&store, std::slice::from_ref(&definition)).remove(0);
        let target = &before.targets[0];
        let evidence = read_evidence(
            &store,
            std::slice::from_ref(&definition),
            &request(),
            &target.id,
        )
        .unwrap();
        damage_idle_dependency(&store, binding);
        store
            .lock()
            .execute(
                "UPDATE session SET updated_at_epoch = 89 WHERE session_id = 'idle-session'",
                [],
            )
            .unwrap();

        let changes = store.lock().total_changes();
        let report = rows(&store, std::slice::from_ref(&definition)).remove(0);
        assert_eq!(
            (report.finding, report.clean, report.unavailable),
            (1, 1, 0)
        );
        assert_eq!(report.checking_count, 0);
        assert_eq!(report.explanation, before.explanation);
        assert_eq!(report.targets.len(), 1);
        assert_eq!(report.targets[0].id, target.id);
        assert_eq!(report.targets[0].observation, target.observation);
        assert!(report.diagnostics.is_empty());
        assert_eq!(
            read_evidence(&store, &[definition], &request(), &target.id).unwrap(),
            evidence,
        );
        assert_eq!(store.lock().total_changes(), changes);
        drop(task);
    }
}

#[test]
fn progress_dependency_database_errors_remain_errors() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let mut saved_data_invalid = false;
    let error = progress_dependencies_current(&connection, "candidate", &mut saved_data_invalid)
        .unwrap_err();
    assert!(error.downcast_ref::<rusqlite::Error>().is_some());
    assert!(!saved_data_invalid);
}

#[test]
fn blocked_v2_source_hides_an_atomic_local_start_without_model_attempts() {
    let (store, adapter, active, definition) = local_fixture();
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    engine
        .apply_source_page(
            &active,
            &source_page(&store, &active, 1, "one", 1, true, "Unfinished local work."),
        )
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1_001).unwrap() else {
        panic!("the local start must be saved before polling actions");
    };
    let record = super::opencode_v2::v2_session(&store, "blocked");
    store
        .lock()
        .execute_batch(
            "UPDATE session SET cwd = '/workspace/other' WHERE agent = 'claude-code';
             UPDATE burn_check_candidate
             SET session_key_json = json_array('native', 'opencode', 'blocked');",
        )
        .unwrap();
    store
        .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::ServiceUnavailable, 100)
        .unwrap();

    let report = rows(&store, &[definition]);
    assert!(!report[0].has_started);
    assert_eq!(report[0].checking_count, 0);
    assert!(report[0].waiting_for_events.is_none());
    assert_eq!(
        report[0].diagnostics[0].code,
        "opencode-v2-service-unavailable"
    );
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
    assert_eq!(count(&store, "burn_check_assessment_history"), 0);
    drop(task);
}

#[test]
fn local_failure_evidence_is_exact_frozen_context_without_model_or_legacy_claims() {
    let (store, adapter, active, definition) = local_fixture();
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    engine
        .apply_source_page(
            &active,
            &source_page(&store, &active, 1, "one", 1, true, "Frozen local evidence."),
        )
        .unwrap();
    start_and_accept(&engine, &active, 1_001);
    let target = current_target(&store, &definition);
    let evidence = read_evidence(
        &store,
        std::slice::from_ref(&definition),
        &request(),
        &target,
    )
    .unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert_eq!(evidence.items.len(), 1);
    let item = &evidence.items[0];
    assert_eq!(item.reference, "local-context");
    assert_eq!(item.source_label, "Frozen local context");
    assert_eq!(
        item.explanation,
        "These frozen values were available to the local status actions. No model request was sent."
    );
    assert!(item.limitation.is_none());
    let frozen: String = store
        .lock()
        .query_row(
            "SELECT frozen_input_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let frozen: FrozenCandidate = serde_json::from_str(&frozen).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&item.excerpt).unwrap(),
        frozen.context.value
    );
    let serialized = serde_json::to_string(&evidence).unwrap();
    for excluded in [
        "Decision connection",
        "assessed-input",
        "input-omissions",
        "Exact assessed input was not saved",
    ] {
        assert!(!serialized.contains(excluded), "{excluded}");
    }
    assert!(rows(&store, &[definition])[0].diagnostics.is_empty());
}

#[test]
fn local_no_write_and_pruning_preserve_the_prior_finding_context_target_message_and_time() {
    let (store, adapter, active, definition) = local_fixture();
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    engine
        .apply_source_page(
            &active,
            &source_page(
                &store,
                &active,
                1,
                "one",
                1,
                true,
                "Original local finding.",
            ),
        )
        .unwrap();
    start_and_accept(&engine, &active, 1_001);
    let target = current_target(&store, &definition);
    let original = read_evidence(
        &store,
        std::slice::from_ref(&definition),
        &request(),
        &target,
    )
    .unwrap();
    let snapshot = || {
        store.lock().query_row(
        "SELECT assessment_id, updated_at_epoch, position, status FROM burn_check_current_status", [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, u64>(2)?, row.get::<_, String>(3)?)),
    ).unwrap()
    };
    let prior = snapshot();
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_local_no_write_insert BEFORE INSERT ON burn_check_current_status
         BEGIN SELECT RAISE(ABORT, 'no-write must not insert status'); END;
         CREATE TRIGGER reject_local_no_write_update BEFORE UPDATE ON burn_check_current_status
         BEGIN SELECT RAISE(ABORT, 'no-write must not update status'); END;",
        )
        .unwrap();
    engine
        .apply_source_page(
            &active,
            &source_page(
                &store,
                &active,
                2,
                "two",
                1,
                false,
                "Later context must not replace evidence.",
            ),
        )
        .unwrap();
    start_and_accept(&engine, &active, 2_001);
    assert_eq!(snapshot(), prior);
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), i64::MAX).unwrap();
    assert_eq!(snapshot(), prior);
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(
        (
            report[0].finding,
            report[0].clean,
            report[0].reviewed_without_status
        ),
        (1, 0, 1)
    );
    assert_eq!(
        report[0].explanation.as_deref(),
        Some("Original local finding.")
    );
    assert_eq!(report[0].targets[0].id, target);
    let retained = read_evidence(&store, &[definition], &request(), &target).unwrap();
    assert_eq!(retained, original);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT position FROM burn_check_acceptance_watermark",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
}

#[test]
fn corrected_local_source_cannot_open_the_old_failure_context() {
    let (store, adapter, active, definition) = local_fixture();
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    engine
        .apply_source_page(
            &active,
            &source_page(&store, &active, 1, "one", 1, true, "Original source."),
        )
        .unwrap();
    start_and_accept(&engine, &active, 1_001);
    let target = current_target(&store, &definition);
    engine
        .apply_source_page(
            &active,
            &source_page(&store, &active, 2, "one", 2, false, "Corrected source."),
        )
        .unwrap();
    assert_eq!(
        rows(&store, std::slice::from_ref(&definition))[0].finding,
        0
    );
    let evidence = read_evidence(&store, &[definition], &request(), &target).unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Unavailable);
    assert!(evidence.items.is_empty());
}

#[test]
fn invalid_local_preparation_keeps_the_finding_and_returns_friendly_unavailable_evidence() {
    for damage in [
        "kind",
        "version",
        "missing_budget",
        "budget",
        "model",
        "model_rejected",
        "missing",
    ] {
        let (store, adapter, active, definition) = local_fixture();
        let engine = CheckEngine::new(adapter, Arc::new(NoModel));
        engine
            .apply_source_page(
                &active,
                &source_page(&store, &active, 1, "one", 1, true, "Preserved finding."),
            )
            .unwrap();
        start_and_accept(&engine, &active, 1_001);
        let target = current_target(&store, &definition);
        let saved: String = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
        match damage {
            "kind" => saved["status"] = json!("unknown-local-kind"),
            "version" => saved["version"] = json!(99),
            "missing_budget" => {
                saved.as_object_mut().unwrap().remove("evaluation_budget");
            }
            "budget" => saved["evaluation_budget"]["visited_items"] = json!(1001),
            "model" => {
                saved = json!({"status": "ready", "profile": "synthetic", "input": {}, "request": null, "report": {"omissions": []}})
            }
            "model_rejected" => {
                saved = json!({"status": "rejected", "code": "no_fit", "detail": "INTERNAL_CANARY", "context": {"decision": "review"}, "report": {"omissions": []}})
            }
            "missing" => saved = serde_json::Value::Null,
            _ => unreachable!(),
        }
        let value = (!saved.is_null()).then(|| saved.to_string());
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET preparation_json = ?1",
                [value],
            )
            .unwrap();
        let report = rows(&store, std::slice::from_ref(&definition));
        assert_eq!(report[0].finding, 1, "{damage}");
        assert_eq!(report[0].targets[0].id, target, "{damage}");
        assert!(
            report[0]
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "local-state-invalid"),
            "{damage}"
        );
        assert!(
            report[0]
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "storage-failed"),
            "{damage}"
        );
        assert!(
            report[0]
                .diagnostics
                .iter()
                .flat_map(|diagnostic| &diagnostic.details)
                .all(|detail| !matches!(detail.label.as_str(), "Decision" | "Provider" | "Model")),
            "{damage}"
        );
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(
            !serialized.contains("No model request was sent"),
            "{damage}"
        );
        assert!(!serialized.contains("Nothing was sent"), "{damage}");
        assert!(!serialized.contains("INTERNAL_CANARY"), "{damage}");
        let evidence = read_evidence(&store, &[definition], &request(), &target).unwrap();
        assert_eq!(
            evidence.status,
            BurnCheckEvidenceStatus::Unavailable,
            "{damage}"
        );
        assert!(evidence.items.is_empty(), "{damage}");
    }
}

#[test]
fn local_checking_rejects_malformed_start_and_changed_dependency_without_a_history_row() {
    for damage in [
        "kind",
        "version",
        "budget",
        "answer",
        "claim_owner",
        "claim_fence",
        "frozen_branch",
        "dependency",
    ] {
        let (store, adapter, active, definition) = local_fixture();
        let engine = CheckEngine::new(adapter, Arc::new(NoModel));
        engine
            .apply_source_page(
                &active,
                &source_page(&store, &active, 1, "one", 1, true, "Unfinished local work."),
            )
            .unwrap();
        let StartOutcome::Started(task) = engine.start_one_due(&active, 1_001).unwrap() else {
            panic!("the local candidate must start");
        };
        match damage {
            "kind" => store.lock().execute("UPDATE burn_check_candidate SET preparation_json = json_set(preparation_json, '$.status', 'unknown-local-kind')", []).unwrap(),
            "version" => store.lock().execute("UPDATE burn_check_candidate SET preparation_json = json_set(preparation_json, '$.version', 99)", []).unwrap(),
            "budget" => store.lock().execute("UPDATE burn_check_candidate SET preparation_json = json_set(preparation_json, '$.evaluation_budget.visited_items', 1001)", []).unwrap(),
            "answer" => store.lock().execute("UPDATE burn_check_candidate SET answer_json = json_set(answer_json, '$.attempts', 1)", []).unwrap(),
            "claim_owner" => store.lock().execute("UPDATE burn_check_candidate SET claim_owner = ''", []).unwrap(),
            "claim_fence" => store.lock().execute("UPDATE burn_check_candidate SET claim_fence = 0", []).unwrap(),
            "frozen_branch" => store.lock().execute("UPDATE burn_check_candidate SET frozen_input_json = json_set(frozen_input_json, '$.branch', 'another-branch')", []).unwrap(),
            "dependency" => store.lock().execute("UPDATE burn_check_observation SET native_revision = native_revision + 1", []).unwrap(),
            _ => unreachable!(),
        };
        let report = rows(&store, &[definition]);
        assert!(!report[0].has_started, "{damage}");
        assert_eq!(report[0].checking_count, 0, "{damage}");
        assert!(
            report[0]
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "storage-failed"),
            "{damage}"
        );
        assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
        assert_eq!(count(&store, "burn_check_assessment_history"), 0);
        drop(task);
    }
}

#[test]
fn local_action_budget_failure_uses_shared_copy_without_a_decision_or_private_error() {
    let source = "name: Local action limit\non:\n  tool_result:\nactions:\n  - type: status\n    input: {result: failure}\n  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {result: success}\n";
    let (store, adapter, active, definition) = local_fixture_with_source(source);
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    let mut page = source_page(&store, &active, 1, "one", 1, true, "Local limit fixture.");
    let SourceEvent::Observation { observation, .. } = page.events.last_mut().unwrap() else {
        panic!("the page must contain an observation");
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        panic!("the observation must contain a tool result");
    };
    message.value["tool"]["arguments"]["items"] = json!(
        (0..1001)
            .map(|_| json!({"value": true}))
            .collect::<Vec<_>>()
    );
    engine.apply_source_page(&active, &page).unwrap();
    let StartOutcome::Started(mut task) = engine.start_one_due(&active, 1_001).unwrap() else {
        panic!("the local candidate must start");
    };
    let Poll::Ready(result) = task
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()))
    else {
        panic!("local actions must settle without model I/O");
    };
    let settled = result.unwrap();
    assert!(matches!(settled.outcome, RunOutcome::Failed(_)));
    assert_eq!(settled.attempts, 0);
    let report = rows(&store, &[definition]);
    assert_eq!(
        (report[0].finding, report[0].clean, report[0].review_failed),
        (0, 0, 1)
    );
    assert!(report[0].targets.is_empty());
    assert_eq!(report[0].diagnostics.len(), 1);
    let diagnostic = &report[0].diagnostics[0];
    assert_eq!(diagnostic.code, "action-failed");
    assert_eq!(
        diagnostic.message,
        "A status action was invalid or reached an evaluation limit. Check the action expression and result in the YAML file."
    );
    assert!(
        diagnostic
            .details
            .iter()
            .all(|detail| !matches!(detail.label.as_str(), "Decision" | "Provider" | "Model"))
    );
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("visited-item limit")
    );
    assert_eq!(count(&store, "burn_check_current_status"), 0);
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
    assert_eq!(count(&store, "burn_check_decision_usage"), 0);
}

#[test]
fn unavailable_local_action_skips_independently_without_required_decision_input_diagnostic() {
    let source = "name: Local unavailable action\non:\n  tool_result:\nactions:\n  - type: status\n    input:\n      result: failure\n      message: ${{ message.tool.arguments.missing }}\n  - type: status\n    input: {result: failure, message: Independent local action.}\n";
    let (store, adapter, active, definition) = local_fixture_with_source(source);
    let engine = CheckEngine::new(adapter, Arc::new(NoModel));
    engine
        .apply_source_page(
            &active,
            &source_page(&store, &active, 1, "one", 1, true, "Available value."),
        )
        .unwrap();
    start_and_accept(&engine, &active, 1_001);
    let report = rows(&store, &[definition]);
    assert_eq!((report[0].finding, report[0].review_failed), (1, 0));
    assert_eq!(
        report[0].explanation.as_deref(),
        Some("Independent local action.")
    );
    assert!(report[0].diagnostics.is_empty());
    assert_eq!(count(&store, "burn_check_evaluation_attempt"), 0);
}
