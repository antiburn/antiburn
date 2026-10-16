use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll, Waker};

use antiburn_checks::actions::{ChoiceResult, plan_status_actions};
use antiburn_checks::compiler::compile;
use antiburn_checks::domain::{
    BranchId, EvidenceVersion, FrozenContext, Message, NativeOrderKey, ObservationDraft,
    ObservationKind,
};
use antiburn_checks::engine::{
    CheckEngine, DecisionEvaluator, DecisionFuture, EvaluationFailure, RunOutcome, StartOutcome,
};
use antiburn_checks::source::SourceEvent;
use serde_json::json;

use super::*;

#[path = "runtime_failure_tests.rs"]
mod failures;
#[path = "runtime_preparation_tests.rs"]
mod preparation;
#[path = "runtime_selection_tests.rs"]
mod selection;

const CHECK: &str = "name: Retry\non:\n  tool_result:\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input: {activity: \"${{ message.tool.result.status }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    if: ${{ decisions.review.details.verdict.probabilities.yes > 0.9 }}\n    input: {result: failure}\n";

struct NoEvaluator;
impl DecisionEvaluator for NoEvaluator {
    fn evaluate<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a serde_json::Value,
        _: u8,
    ) -> DecisionFuture<'a> {
        panic!("store tests do not send provider requests")
    }
}

fn fixture() -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    let store = Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-test")).unwrap());
    fixture_with_store(store)
}

fn fixture_with_store(
    store: Arc<Store>,
) -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    fixture_with_definition(store, CHECK)
}

fn fixture_with_definition(
    store: Arc<Store>,
    source: &str,
) -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    let scope = ActivationScope {
        environment: "native".into(),
        session_cwd: "/repo".into(),
        run: RunScope::Live,
    };
    let adapter =
        Arc::new(StoreCheckStore::new(store.clone(), "owner-1".into(), scope.clone()).unwrap());
    let active = EffectiveActivation {
        id: ActivationId {
            scope,
            definition_id: "retry".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Global,
        source_paths: vec![],
        project_root: Some(PathBuf::from("/repo")),
        compiled: Arc::new(compile(source).unwrap()),
        bootstrap_observation_position: ObservationPosition(50),
        bootstrap_publication: PublicationCursor(0),
    };
    adapter
        .activate(
            "retry",
            &LifecycleTransition::Activated(active.clone()),
            Some(&active),
        )
        .unwrap();
    (store, adapter, active)
}

fn event(store: &Store, sequence: u64, id: &str, branch: &str, revision: u64) -> SourceBatch {
    store.lock().execute("INSERT INTO session (environment_key, agent, session_id, source_kind, source_label, first_seen_at, last_seen_at, cwd)
        VALUES ('native', 'fixture', ?1, 'file', 'fixture', 'now', 'now', '/repo') ON CONFLICT DO NOTHING", [branch]).unwrap();
    store.lock().execute("INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence)
        VALUES (?1, 'update', 'native', 'fixture', ?2, 'fixture', ?3, '{}', ?4, '{}', 1, 1, 1)", params![sequence, branch, id, revision]).unwrap();
    SourceBatch {
        events: vec![SourceEvent::Observation {
            cursor: PublicationCursor(sequence),
            identity: id.into(),
            revision,
            observation: Box::new(ObservationDraft {
                first_terminal_publication: None,
                branch: BranchId(branch.into()),
                native_order: NativeOrderKey(id.into()),
                version: EvidenceVersion(format!("{id}:{revision}")),
                kind: ObservationKind::ToolResult {
                    message: Message {
                        id: id.into(),
                        value: json!({"tool": {"result": {"status": "error"}}}),
                    },
                    category: "shell".into(),
                    status: "error".into(),
                },
                context: FrozenContext {
                    value: json!({"session": {"requests": []}}),
                    dependencies: vec![],
                },
                complete: true,
            }),
        }],
        next: PublicationCursor(sequence),
        has_more: false,
    }
}

fn apply(
    adapter: &Arc<StoreCheckStore>,
    active: &EffectiveActivation,
    page: &SourceBatch,
) -> FrozenCandidate {
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    engine.apply_source_page(active, page).unwrap();
    let position = adapter
        .load_source_progress(active)
        .unwrap()
        .observation_position;
    adapter
        .load_recovery(None, 10)
        .unwrap()
        .into_iter()
        .find_map(|record| match record.work {
            RecoveryWork::Pending(candidate) if candidate.trigger_position == position => {
                Some(candidate)
            }
            _ => None,
        })
        .unwrap()
}

fn claim(adapter: &StoreCheckStore, candidate: &FrozenCandidate) -> ClaimedCandidate {
    let CandidateClaim::Claimed(claimed) = adapter.claim_due(candidate, 100_000, true).unwrap()
    else {
        panic!("candidate must be claimable");
    };
    *claimed
}

fn transient_diagnostic() -> EvaluationDiagnostic {
    EvaluationDiagnostic {
        code: antiburn_checks::engine::EvaluationFailureCode::RateLimited,
        detail: "Synthetic rate limit.".into(),
    }
}

fn answer(
    adapter: &StoreCheckStore,
    active: &EffectiveActivation,
    claimed: ClaimedCandidate,
    yes: f64,
) -> AssessmentAcceptance {
    adapter
        .record_dispatch_intent(
            &claimed,
            100_000,
            Duration::from_secs(60),
            claimed.next_attempt,
        )
        .unwrap();
    let evaluated = EvaluatedCandidate {
        choices: vec![
            ChoiceResult::validate(
                "review",
                "verdict",
                &["yes".into(), "no".into()],
                if yes > 0.5 { "yes" } else { "no" },
                BTreeMap::from([("yes".into(), yes), ("no".into(), 1.0 - yes)]),
            )
            .unwrap(),
        ],
        attempts: claimed.next_attempt,
    };
    adapter.record_answer(&claimed, &evaluated).unwrap();
    let actions = plan_status_actions(
        &active.compiled.actions,
        &claimed.candidate.context.value,
        &evaluated.choices,
        claimed.prior_status,
    )
    .unwrap();
    AssessmentAcceptance {
        claimed,
        evaluated,
        actions,
    }
}

#[test]
fn page_checkpoint_coalescing_and_generation_are_durable_and_atomic() {
    let (store, adapter, mut active) = fixture();
    Arc::make_mut(&mut active.compiled).trigger.window =
        Some(antiburn_checks::compiler::RollingWindow {
            size: 1,
            minimum: 1,
        });
    adapter.register_activation(&active).unwrap();
    let page = event(&store, 11, "one", "branch", 1);
    store.lock().execute_batch("CREATE TRIGGER reject_candidate BEFORE INSERT ON burn_check_candidate BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    assert!(engine.apply_source_page(&active, &page).is_err());
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .observation_position,
        ObservationPosition(50)
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_observation", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_candidate")
        .unwrap();
    let first = apply(&adapter, &active, &page);
    let second = apply(&adapter, &active, &event(&store, 29, "two", "branch", 1));
    assert_eq!(second.trigger_position, ObservationPosition(52));
    assert_eq!(
        adapter.claim_due(&first, 100_000, false).unwrap(),
        CandidateClaim::Stale
    );
    assert_eq!(adapter.load_recovery(None, 10).unwrap().len(), 1);
    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    assert_eq!(
        restarted
            .load_lifecycle("retry", active.compiled.clone())
            .unwrap(),
        LifecycleState::Active(active.clone())
    );
    restarted.register_activation(&active).unwrap();
    assert_eq!(
        restarted.load_source_progress(&active).unwrap(),
        adapter.load_source_progress(&active).unwrap()
    );
    let mut wrong = active.clone();
    wrong.id.generation = Generation(7);
    assert!(
        restarted
            .activate(
                "retry",
                &LifecycleTransition::Activated(wrong.clone()),
                Some(&wrong)
            )
            .is_err()
    );
    assert_eq!(claim(&restarted, &second).candidate, second);
}

#[test]
fn answered_recovery_defers_without_resend_and_preserves_last_status_write() {
    let source = CHECK.replace(
        "input: {result: failure}",
        "input: {result: failure, message: Initial failure explanation.}",
    );
    let store = Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-test")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &first), 0.99);
    let next_page = event(&store, 2, "two", "branch", 1);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Deferred
    );
    let restarted = Arc::new(
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let RecoveryWork::Answered { claimed, evaluated } =
        restarted.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("saved answer must recover");
    };
    assert_eq!(evaluated, assessment.evaluated);
    let second = apply(&restarted, &active, &next_page);
    let recovered = AssessmentAcceptance {
        claimed,
        evaluated,
        actions: assessment.actions,
    };
    store.lock().execute_batch("CREATE TRIGGER reject_status BEFORE INSERT ON burn_check_current_status BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(restarted.accept(&recovered).is_err());
    for table in [
        "burn_check_assessment_history",
        "burn_check_acceptance_watermark",
    ] {
        let count: i64 = store
            .lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "failed acceptance must roll back {table}");
    }
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_status")
        .unwrap();
    assert_eq!(
        restarted.accept(&recovered).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_eq!(
        restarted.accept(&recovered).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
    let original: (String, i64, String, String) = store
        .lock()
        .query_row(
            "SELECT s.assessment_id, s.updated_at_epoch,
                json_extract(h.actions_json, '$.outcomes[0].outcome.Applied.message'), s.status
             FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(original.2, "Initial failure explanation.");
    assert_eq!(original.3, "failure");
    let first_watermark: (u64, u64) = store
        .lock()
        .query_row(
            "SELECT generation, position FROM burn_check_acceptance_watermark",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        first_watermark,
        (active.id.generation.0, first.trigger_position.0)
    );
    let claimed = claim(&restarted, &second);
    assert_eq!(claimed.prior_status, Some(StatusResult::Failure));
    let no_write = answer(&restarted, &active, claimed, 0.4);
    assert_eq!(no_write.status_write(), None);
    assert_eq!(
        restarted.accept(&no_write).unwrap(),
        AcceptanceOutcome::Accepted
    );
    let retained: (String, i64, String, String) = store
        .lock()
        .query_row(
            "SELECT s.assessment_id, s.updated_at_epoch,
                json_extract(h.actions_json, '$.outcomes[0].outcome.Applied.message'), s.status
             FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(original, retained);
    let no_write_watermark: (u64, u64) = store
        .lock()
        .query_row(
            "SELECT generation, position FROM burn_check_acceptance_watermark",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        no_write_watermark,
        (active.id.generation.0, second.trigger_position.0)
    );
    assert!(no_write_watermark.1 > first_watermark.1);
    let (state, position, actions): (String, u64, String) = store
        .lock()
        .query_row(
            "SELECT c.state, h.trigger_position, h.actions_json FROM burn_check_candidate c
            JOIN burn_check_assessment_history h USING(candidate_id) WHERE c.candidate_id = ?1",
            [hex(&second.fingerprint)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, "accepted");
    assert_eq!(position, second.trigger_position.0);
    let actions: serde_json::Value = serde_json::from_str(&actions).unwrap();
    assert_eq!(actions["outcomes"][0]["outcome"], "SkippedConditionFalse");
    assert_eq!(actions["current_status"], json!(StatusResult::Failure));
    assert_eq!(
        restarted.accept(&no_write).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn safe_retry_number_survives_restart_and_unknown_dispatch_never_resends() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let first = claim(&adapter, &candidate);
    adapter
        .record_dispatch_intent(&first, 100_000, Duration::from_secs(60), 1)
        .unwrap();
    adapter
        .record_attempt_failure(
            &first,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert!(matches!(
        restarted.claim_due(&candidate, 120_000, false).unwrap(),
        CandidateClaim::NotDue {
            retry_at_unix_ms: 160_000
        }
    ));
    let retry = claim(&restarted, &candidate);
    assert_eq!(retry.next_attempt, 2);
    assert_eq!(
        restarted
            .record_dispatch_intent(&first, 100_000, Duration::ZERO, 2)
            .unwrap(),
        ClaimOutcome::Stale
    );
    restarted
        .record_dispatch_intent(&retry, 100_000, Duration::ZERO, 2)
        .unwrap();
    let recovered =
        StoreCheckStore::new(store.clone(), "owner-3".into(), active.id.scope.clone()).unwrap();
    recovered.register_activation(&active).unwrap();
    assert!(recovered.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        recovered.claim_due(&candidate, 200_000, true).unwrap(),
        CandidateClaim::Stale
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT outcome FROM burn_check_evaluation_attempt WHERE attempt_number = 2",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "unknown"
    );
    let saved: String = store
        .lock()
        .query_row(
            "SELECT diagnostic_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<EvaluationDiagnostic>(&saved)
            .unwrap()
            .code,
        antiburn_checks::engine::EvaluationFailureCode::DeliveryUnknown
    );
}

#[test]
fn correction_revokes_answer_and_exact_claim_does_not_substitute_branches() {
    let (store, adapter, active) = fixture();
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch-a", 1));
    let second = apply(&adapter, &active, &event(&store, 2, "two", "branch-b", 1));
    let second_claim = claim(&adapter, &second);
    assert_eq!(second_claim.id, CandidateId::from(&second));
    let assessment = answer(&adapter, &active, claim(&adapter, &first), 0.99);
    let mut correction = event(&store, 3, "one", "branch-a", 1);
    let SourceEvent::Observation { observation, .. } = &mut correction.events[0] else {
        unreachable!()
    };
    observation.version = EvidenceVersion("same-revision-correction".into());
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    assert_eq!(
        engine
            .apply_source_page(&active, &correction)
            .unwrap()
            .candidates,
        0
    );
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Stale
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
        0
    );
}

#[test]
fn same_revision_correction_or_removal_invalidates_accepted_evidence() {
    for removed in [false, true] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
        let page = if removed {
            store.lock().execute("INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence)
                SELECT 2, 'removal', environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, '{}', 1, source_generation, claim_fence
                FROM native_tool_result_publication WHERE publication_sequence = 1", []).unwrap();
            SourceBatch {
                events: vec![SourceEvent::Removal {
                    cursor: PublicationCursor(2),
                    identity: "one".into(),
                    revision: 1,
                    branch: BranchId("branch".into()),
                }],
                next: PublicationCursor(2),
                has_more: false,
            }
        } else {
            let mut page = event(&store, 2, "one", "branch", 1);
            let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
                unreachable!()
            };
            let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
                unreachable!()
            };
            message.value["tool"]["result"]["status"] = json!("corrected");
            observation.version = EvidenceVersion("same-revision-corrected-content".into());
            page
        };
        CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&active, &page)
            .unwrap();
        let valid: (bool, bool) = store.lock().query_row(
            "SELECT s.evidence_valid, h.evidence_valid FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(valid, (false, false));
        let mut next = active.clone();
        next.id.generation = Generation(2);
        next.revision = [2; 32];
        adapter
            .activate(
                "retry",
                &LifecycleTransition::Activated(next.clone()),
                Some(&next),
            )
            .unwrap();
        assert_eq!(
            adapter.claim_due(&candidate, 200_000, true).unwrap(),
            CandidateClaim::Stale
        );
        assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn later_correction_in_one_page_prevents_storing_the_earlier_candidate() {
    let (store, adapter, active) = fixture();
    let mut page = event(&store, 1, "one", "branch", 1);
    let mut correction = event(&store, 2, "one", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut correction.events[0] else {
        unreachable!()
    };
    observation.version = EvidenceVersion("corrected".into());
    page.events.extend(correction.events);
    page.next = PublicationCursor(2);
    let outcome = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
        .apply_source_page(&active, &page)
        .unwrap();
    assert_eq!(outcome.candidates, 0);
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        0
    );
}

#[test]
fn unapplied_publication_for_another_session_does_not_block_acceptance() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    event(&store, 2, "two", "unrelated-session", 1);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
}

#[test]
fn overflow_and_failure_keep_history_without_status_or_watermark() {
    let (store, adapter, active) = fixture();
    let overflow = apply(&adapter, &active, &event(&store, 1, "one", "a", 1));
    adapter.record_overflow(&overflow).unwrap();
    let failed = apply(&adapter, &active, &event(&store, 2, "two", "b", 1));
    adapter
        .record_failure(&claim(&adapter, &failed), "provider failed")
        .unwrap();
    let outcomes = store
        .lock()
        .prepare("SELECT outcome FROM burn_check_assessment_history ORDER BY trigger_position")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(outcomes, ["not_assessed", "execution_failed"]);
    for table in [
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_evaluation_attempt",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn retention_prunes_unused_frozen_inputs_and_preserves_current_status_evidence() {
    let (store, adapter, active) = fixture();
    for (sequence, branch, yes) in [(1, "no-status", 0.4), (2, "failure", 0.99)] {
        let candidate = apply(
            &adapter,
            &active,
            &event(&store, sequence, "one", branch, 1),
        );
        let claimed = claim(&adapter, &candidate);
        let prepared = antiburn_checks::fit::PreparedDecision {
            profile: "fixture-v1".into(),
            input: json!({"text": "retained evidence"}),
            request: Some(json!({"exact": "provider request"})),
            report: antiburn_checks::fit::FitReport {
                omissions: vec![antiburn_checks::fit::FitOmission {
                    path: "text".into(),
                    strategy: antiburn_checks::fit::FitStrategy::Truncate,
                    original_count: 2048,
                    retained_count: 1024,
                }],
            },
        };
        adapter
            .record_preparation(&claimed, &PreparationRecord::Ready(prepared))
            .unwrap();
        let assessment = answer(&adapter, &active, claimed, yes);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
    }
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), now_epoch() + 1).unwrap();
    let payloads = store.lock().prepare("SELECT branch_key, frozen_input_json <> '{}' FROM burn_check_candidate ORDER BY branch_key").unwrap()
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(
        payloads,
        [("failure".into(), true), ("no-status".into(), false)]
    );
    let prepared = store
        .lock()
        .prepare(
            "SELECT branch_key, preparation_json FROM burn_check_candidate ORDER BY branch_key",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for (branch, saved) in prepared {
        let PreparationRecord::Ready(prepared) = serde_json::from_str(&saved).unwrap() else {
            panic!("ready report remains available");
        };
        assert_eq!(prepared.report.omissions[0].retained_count, 1024);
        if branch == "failure" {
            assert!(prepared.request.is_some());
            assert_eq!(prepared.input["text"], "retained evidence");
        } else {
            assert!(prepared.request.is_none());
            assert_eq!(prepared.input, json!({}));
        }
    }
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn source_page_read_before_deletion_cannot_restore_deleted_check_data() {
    let (store, adapter, active) = fixture();
    let page = event(&store, 1, "one", "branch", 1);
    store
        .delete_session(&crate::store::SessionKey::new(
            "native", "fixture", "branch",
        ))
        .unwrap()
        .unwrap();
    assert!(
        CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&active, &page)
            .is_err()
    );
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .publication_cursor,
        PublicationCursor(0)
    );
    for table in ["burn_check_observation", "burn_check_candidate"] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn v75_migration_preserves_existing_data_and_supports_runtime_acceptance() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &migration in &crate::store::schema::MIGRATIONS[..75] {
        connection.execute_batch(migration).unwrap();
    }
    connection.pragma_update(None, "user_version", 75).unwrap();
    connection.execute_batch(
        "INSERT INTO setting (key, value) VALUES ('legacy-setting', 'preserved');
         INSERT INTO session (environment_key, agent, session_id, source_kind, source_label, first_seen_at, last_seen_at)
           VALUES ('native', 'claude-code', 'migration-session', 'file', 'fixture', 'now', 'now');
         INSERT INTO burn_check_assessment (environment_key, agent, session_id, check_id, incarnation, created_at_epoch, updated_at_epoch)
           VALUES ('native', 'claude-code', 'migration-session', 'existing-detector', 1, 1, 1);",
    ).unwrap();
    let store =
        Store::from_connection(connection, PathBuf::from("/tmp/antiburn-runtime-migration"))
            .unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        i64::try_from(crate::store::schema::MIGRATIONS.len()).unwrap()
    );
    let (store, adapter, active) = fixture_with_store(Arc::new(store));
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    let connection = store.lock();
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM setting WHERE key = 'legacy-setting'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "preserved"
    );
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM burn_check_assessment WHERE check_id = 'existing-detector' AND session_id = 'migration-session'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert!(
        connection
            .execute(
                "UPDATE native_tool_result_publication SET source_generation = 2",
                []
            )
            .is_err()
    );
    assert!(
        connection
            .execute("DELETE FROM native_tool_result_publication", [])
            .is_err()
    );
    drop(connection);
    store
        .delete_session(&crate::store::SessionKey::new(
            "native", "fixture", "branch",
        ))
        .unwrap()
        .unwrap();
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recovery_uses_keyset_pages_and_replay_has_an_independent_scope() {
    let (store, adapter, active) = fixture();
    for (sequence, branch) in [(1, "a"), (2, "b"), (3, "c")] {
        apply(
            &adapter,
            &active,
            &event(&store, sequence, "same-native-id", branch, 1),
        );
    }
    let mut after = None;
    let mut ids = Vec::new();
    loop {
        let records = adapter.load_recovery(after.as_ref(), 1).unwrap();
        if records.is_empty() {
            break;
        }
        let RecoveryWork::Pending(candidate) = &records[0].work else {
            panic!("pending candidate expected");
        };
        let id = CandidateId::from(candidate);
        assert!(after.as_ref().is_none_or(|after| after < &id));
        after = Some(id.clone());
        ids.push(id);
    }
    assert_eq!(ids.len(), 3);
    let mut replay = active.clone();
    replay.id.scope.run = RunScope::Replay("isolated-run".into());
    replay.bootstrap_observation_position = ObservationPosition(0);
    let replay_store = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "replay-owner".into(),
            replay.id.scope.clone(),
        )
        .unwrap(),
    );
    replay_store
        .activate(
            "retry",
            &LifecycleTransition::Activated(replay.clone()),
            Some(&replay),
        )
        .unwrap();
    let page = SourceBatch {
        events: vec![SourceEvent::Observation {
            cursor: PublicationCursor(1),
            identity: "same-native-id".into(),
            revision: 1,
            observation: Box::new(ObservationDraft {
                first_terminal_publication: None,
                branch: BranchId("a".into()),
                native_order: NativeOrderKey("one".into()),
                version: EvidenceVersion("one".into()),
                kind: ObservationKind::ToolResult {
                    message: Message {
                        id: "same-native-id".into(),
                        value: json!({"tool": {"result": {"status": "error"}}}),
                    },
                    category: "shell".into(),
                    status: "error".into(),
                },
                context: FrozenContext {
                    value: json!({"session": {"requests": []}}),
                    dependencies: vec![],
                },
                complete: true,
            }),
        }],
        next: PublicationCursor(1),
        has_more: false,
    };
    let candidate = apply(&replay_store, &replay, &page);
    let assessment = answer(
        &replay_store,
        &replay,
        claim(&replay_store, &candidate),
        0.99,
    );
    assert_eq!(
        replay_store.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_ne!(
        StoreCheckStore::activation_scope_key(&replay.id.scope),
        StoreCheckStore::activation_scope_key(&active.id.scope)
    );
    assert_eq!(adapter.load_recovery(None, 10).unwrap().len(), 3);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_current_status WHERE activation_scope_key = ?1",
                [&adapter.activation_scope_key],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn clear_and_session_deletion_remove_frozen_selector_payloads() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let RecoveryWork::Pending(candidate) = adapter.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("pending candidate expected");
    };
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    store
        .delete_session(&crate::store::SessionKey::new(
            "native", "fixture", "branch",
        ))
        .unwrap();
    let progress = adapter.load_source_progress(&active).unwrap();
    assert!(progress.selection.branches.is_empty());
    assert!(progress.selection.seen_results.is_empty());
    for table in [
        "burn_check_observation",
        "burn_check_candidate",
        "burn_check_candidate_dependency",
        "burn_check_evaluation_attempt",
        "burn_check_assessment_history",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_cooldown",
    ] {
        let count: i64 = store
            .lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "session deletion must clear {table}");
    }
    let next_page = event(&store, 2, "two", "new-branch", 1);
    assert!(engine.apply_source_page(&active, &next_page).is_err());
    engine.apply_source_page(&active, &next_page).unwrap();
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .selection
            .branches
            .len(),
        1
    );
    store.clear_local_session_data().unwrap();
    assert!(adapter.register_activation(&active).is_err());
    assert_eq!(
        adapter.load_lifecycle("retry", None).unwrap(),
        LifecycleState::Removed {
            generation: Generation(1)
        }
    );
    let mut next = active.clone();
    next.id.generation = Generation(2);
    adapter
        .activate(
            "retry",
            &LifecycleTransition::Activated(next.clone()),
            Some(&next),
        )
        .unwrap();
    assert_eq!(
        adapter.load_lifecycle("retry", None).unwrap(),
        LifecycleState::Uncompiled {
            generation: Generation(2)
        }
    );
}

struct RecordingEvaluator(Mutex<Vec<u8>>);

impl DecisionEvaluator for RecordingEvaluator {
    fn evaluate<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a serde_json::Value,
        number: u8,
    ) -> DecisionFuture<'a> {
        self.0.lock().unwrap().push(number);
        Box::pin(async move {
            Ok(EvaluatedCandidate {
                attempts: 1,
                choices: vec![
                    ChoiceResult::validate(
                        "review",
                        "verdict",
                        &["yes".into(), "no".into()],
                        "yes",
                        BTreeMap::from([("yes".into(), 0.99), ("no".into(), 0.01)]),
                    )
                    .unwrap(),
                ],
            })
        })
    }
}

struct ControlledEvaluator {
    released: AtomicBool,
    failure: Option<AttemptFailure>,
    inputs: Mutex<Vec<(u64, serde_json::Value, u8)>>,
}

impl DecisionEvaluator for ControlledEvaluator {
    fn evaluate<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        input: &'a serde_json::Value,
        number: u8,
    ) -> DecisionFuture<'a> {
        self.inputs
            .lock()
            .unwrap()
            .push((candidate.trigger_position.0, input.clone(), number));
        let first = candidate.trigger_position == ObservationPosition(51);
        Box::pin(std::future::poll_fn(move |_| {
            if first && !self.released.load(Ordering::SeqCst) {
                return Poll::Pending;
            }
            if first && let Some(kind) = self.failure {
                return Poll::Ready(Err(EvaluationFailure {
                    code: antiburn_checks::engine::EvaluationFailureCode::ProviderUnavailable,
                    kind,
                    detail: "scripted provider failure".into(),
                    attempts: number,
                }));
            }
            Poll::Ready(Ok(EvaluatedCandidate {
                attempts: number,
                choices: vec![
                    ChoiceResult::validate(
                        "review",
                        "verdict",
                        &["yes".into(), "no".into()],
                        "yes",
                        BTreeMap::from([("yes".into(), 0.99), ("no".into(), 0.01)]),
                    )
                    .unwrap(),
                ],
            }))
        }))
    }
}

fn poll_settlement(
    mut task: antiburn_checks::engine::EvaluationTask<'_>,
) -> antiburn_checks::engine::RunSettlement {
    let Poll::Ready(result) = task
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()))
    else {
        panic!("scripted evaluator must settle");
    };
    result.unwrap()
}

#[test]
fn provider_failures_after_source_correction_settle_stale_and_release_single_flight() {
    for failure in [
        AttemptFailure::Permanent,
        AttemptFailure::Unknown,
        AttemptFailure::SafeTransient,
    ] {
        let (store, adapter, active) = fixture();
        let evaluator = Arc::new(ControlledEvaluator {
            released: AtomicBool::new(false),
            failure: Some(failure),
            inputs: Mutex::new(vec![]),
        });
        let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
        engine
            .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
            .unwrap();
        let StartOutcome::Started(mut task) = engine.start_one_due(&active, 100_000).unwrap()
        else {
            panic!("first candidate must start")
        };
        assert!(
            task.as_mut()
                .poll(&mut TaskContext::from_waker(Waker::noop()))
                .is_pending()
        );
        engine
            .apply_source_page(&active, &event(&store, 2, "one", "branch", 2))
            .unwrap();
        engine
            .apply_source_page(&active, &event(&store, 3, "two", "branch", 1))
            .unwrap();
        evaluator.released.store(true, Ordering::SeqCst);
        let settled = poll_settlement(task);
        assert_eq!(settled.outcome, RunOutcome::Stale, "{failure:?}");
        assert_eq!(settled.attempts, 1);
        assert_eq!(
            evaluator.inputs.lock().unwrap().len(),
            1,
            "stale transient work must not retry"
        );
        let StartOutcome::Started(task) = engine.start_one_due(&active, 160_000).unwrap() else {
            panic!("settled stale work must release its branch")
        };
        assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
        assert_eq!(
            evaluator
                .inputs
                .lock()
                .unwrap()
                .iter()
                .map(|item| (item.0, item.2))
                .collect::<Vec<_>>(),
            [(51, 1), (53, 1)]
        );
        let attempts: (i64, i64) = store.lock().query_row(
            "SELECT COUNT(*), SUM(state = 'dispatch_intent') FROM burn_check_evaluation_attempt", [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(attempts, (2, 0));
        let outcomes = store
            .lock()
            .prepare("SELECT outcome FROM burn_check_assessment_history ORDER BY trigger_position")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(outcomes, ["stale", "assessed"]);
    }
}

#[test]
fn correction_between_safe_failure_and_retry_intent_prevents_a_second_send() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    assert_eq!(
        adapter
            .record_dispatch_intent(&claimed, 0, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        adapter
            .record_attempt_failure(
                &claimed,
                1,
                AttemptFailure::SafeTransient,
                &transient_diagnostic()
            )
            .unwrap(),
        ClaimOutcome::Current
    );
    CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
        .apply_source_page(&active, &event(&store, 2, "one", "branch", 2))
        .unwrap();
    assert_eq!(
        adapter
            .record_dispatch_intent(&claimed, 0, Duration::ZERO, 2)
            .unwrap(),
        ClaimOutcome::Stale
    );
    let restarted =
        StoreCheckStore::new(store.clone(), "restarted".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

fn content_event(store: &Store, sequence: u64, content: Option<&str>) -> SourceBatch {
    let mut page = event(store, sequence, &format!("result-{sequence}"), "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        unreachable!()
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        unreachable!()
    };
    if let Some(content) = content {
        message.value["tool"]["result"]["content"] = json!(content);
    }
    page
}

#[test]
fn trigger_condition_precedes_window_counting_and_cannot_replace_ready_work() {
    use antiburn_checks::preparation::ConditionOutcome;

    let source = CHECK
        .replace(
            "  tool_result:\n",
            "  tool_result:\n  window: {size: 2, min: 2}\n",
        )
        .replace(
            "decisions:\n",
            "if: message.tool.result.content == 'keep'\ndecisions:\n",
        )
        .replace("message.tool.result.status", "window.messages");
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-condition")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = Arc::new(ControlledEvaluator {
        released: AtomicBool::new(true),
        failure: None,
        inputs: Mutex::new(vec![]),
    });
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    for (sequence, content, expected) in [
        (1, Some("keep"), None),
        (2, Some("drop"), Some(ConditionOutcome::False)),
        (3, None, Some(ConditionOutcome::Unavailable)),
        (4, Some("keep"), None),
    ] {
        let outcome = engine
            .apply_source_page(&active, &content_event(&store, sequence, content))
            .unwrap();
        assert_eq!(outcome.candidates, usize::from(sequence == 4));
        assert_eq!(
            outcome
                .condition_outcomes
                .into_iter()
                .map(|(_, condition)| condition)
                .collect::<Vec<_>>(),
            expected.into_iter().collect::<Vec<_>>()
        );
    }
    for (sequence, content) in [(5, Some("drop")), (6, None)] {
        assert_eq!(
            engine
                .apply_source_page(&active, &content_event(&store, sequence, content))
                .unwrap()
                .coalesced,
            0
        );
    }
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("valid pending work must survive filtered events")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    let inputs = evaluator.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].0, 54);
    assert_eq!(inputs[0].1["activity"].as_array().unwrap().len(), 2);
    assert!(
        inputs[0].1["activity"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["tool"]["result"]["content"] == "keep")
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
}

#[test]
fn non_window_candidates_survive_restart_and_dispatch_in_source_order() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    for sequence in 1..=3 {
        let outcome = engine
            .apply_source_page(
                &active,
                &event(&store, sequence, &format!("result-{sequence}"), "branch", 1),
            )
            .unwrap();
        assert_eq!(outcome.coalesced, 0);
    }
    assert_eq!(adapter.load_recovery(None, 10).unwrap().len(), 3);
    let restarted = Arc::new(
        StoreCheckStore::new(store, "restart-owner".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let evaluator = Arc::new(ControlledEvaluator {
        released: AtomicBool::new(true),
        failure: None,
        inputs: Mutex::new(vec![]),
    });
    let engine = CheckEngine::new(restarted, evaluator.clone());
    assert_eq!(engine.recover_pending().unwrap().candidates, 3);
    for now in [100_000, 160_000, 220_000] {
        let StartOutcome::Started(task) = engine.start_one_due(&active, now).unwrap() else {
            panic!("each event must retain a separate candidate")
        };
        assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    }
    assert_eq!(
        evaluator
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(|input| input.0)
            .collect::<Vec<_>>(),
        [51, 52, 53]
    );
}

#[test]
fn non_window_overflow_is_durable_and_keeps_earlier_pending_events() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let mut overflowed = 0;
    for sequence in 1..=antiburn_checks::engine::MAX_PENDING_CANDIDATES as u64 + 2 {
        overflowed += engine
            .apply_source_page(
                &active,
                &event(&store, sequence, &format!("result-{sequence}"), "branch", 1),
            )
            .unwrap()
            .overflowed;
    }
    assert_eq!(overflowed, 2);
    assert_eq!(
        adapter.load_recovery(None, 500).unwrap().len(),
        antiburn_checks::engine::MAX_PENDING_CANDIDATES
    );
    let rows = store.lock().prepare("SELECT state, COUNT(*), MIN(trigger_position), MAX(trigger_position) FROM burn_check_candidate GROUP BY state ORDER BY state").unwrap()
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?, row.get::<_, u64>(2)?, row.get::<_, u64>(3)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(
        rows,
        [
            ("not_assessed".into(), 2, 179, 180),
            ("pending".into(), 128, 51, 178)
        ]
    );
}

struct PublishedReader(antiburn_checks::source::PublishedEvidenceBatch);

impl antiburn_checks::source::PublishedEvidenceReader for PublishedReader {
    fn read_batch(
        &self,
        _: antiburn_checks::source::PublicationRead,
    ) -> Result<antiburn_checks::source::PublishedEvidenceBatch, antiburn_checks::source::SourceError>
    {
        Ok(self.0.clone())
    }
}

#[test]
fn normalized_partial_result_exposes_authored_identity_and_remains_assessable() {
    use antiburn_checks::source::*;

    let source = CHECK.replace("decisions:\n", "if: message.id == 'one' && message.source == 'fixture' && message.kind == 'tool_result'\ndecisions:\n")
        .replace("input: {activity: \"${{ message.tool.result.status }}\"}", "input: {activity: '${{ message }}', requests: '${{ session.requests }}'}");
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-normalization")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    event(&store, 1, "one", "branch", 1);
    let page = FileEventSource::new(PublishedReader(PublishedEvidenceBatch {
        events: vec![PublishedEvidenceEvent {
            cursor: PublicationCursor(1), branch: "branch".into(),
            context: Some(PublishedContext {
                requests: vec![("request".into(), json!({"source": "fixture", "kind": "user_message", "text": "Review available results."}))],
                session_id: "branch".into(), agent_name: "fixture".into(), project_root: Some("/repo".into()),
                version: "request-prefix-1".into(), main_session: true, complete_through_event: false,
            }),
            change: PublishedEvidenceChange::Upsert(PublishedToolResult {
                first_terminal_publication: None,
                identity: "one".into(), native_order: NativeOrderKey("native-one".into()), revision: 1,
                tool_name: None, category: None, arguments: None, content: Some("Useful partial result".into()),
                status: PublishedResultStatus::Error, finality: true, complete: false, selection_safe: true,
            }),
        }], next: PublicationCursor(1), has_more: false,
    }), 1).unwrap().read_batch(PublicationCursor(0)).unwrap();
    let evaluator = Arc::new(ControlledEvaluator {
        released: AtomicBool::new(true),
        failure: None,
        inputs: Mutex::new(vec![]),
    });
    let engine = CheckEngine::new(adapter, evaluator.clone());
    assert_eq!(
        engine.apply_source_page(&active, &page).unwrap().candidates,
        1
    );
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("safe partial evidence must run")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    let inputs = evaluator.inputs.lock().unwrap();
    let message = &inputs[0].1["activity"];
    assert_eq!(message["id"], "one");
    assert_eq!(message["source"], "fixture");
    assert_eq!(message["kind"], "tool_result");
    assert_eq!(message["quality"]["category_available"], false);
    assert_eq!(message["quality"]["request_history_complete"], false);
    assert!(message["tool"].get("category").is_none());
    assert!(message["tool"].get("arguments").is_none());
    assert_eq!(
        inputs[0].1["requests"][0]["text"],
        "Review available results."
    );
}

#[test]
fn saved_answers_reject_invalid_probabilities_and_wrong_choice_declarations() {
    for corruption in ["probabilities", "declaration"] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
        let mut saved = serde_json::to_value(assessment.evaluated).unwrap();
        if corruption == "probabilities" {
            saved["choices"][0]["probabilities"] = json!({"yes": 1.5, "no": -0.5});
        } else {
            saved["choices"][0]["question"] = json!("other_question");
        }
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET answer_json = ?1 WHERE candidate_id = ?2",
                params![
                    serde_json::to_string(&saved).unwrap(),
                    hex(&candidate.fingerprint)
                ],
            )
            .unwrap();
        let restarted = StoreCheckStore::new(
            store.clone(),
            "restart-owner".into(),
            active.id.scope.clone(),
        )
        .unwrap();
        restarted.register_activation(&active).unwrap();
        assert!(restarted.load_recovery(None, 10).is_err(), "{corruption}");
        assert_eq!(
            restarted.claim_due(&candidate, 200_000, true).unwrap(),
            CandidateClaim::Stale
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
            0
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

#[test]
fn recovered_safe_retry_passes_the_durable_number_to_the_evaluator() {
    use antiburn_checks::engine::{ExecutionPolicy, RunOutcome, StartOutcome};
    use std::task::{Context, Poll, Waker};

    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let evaluator = Arc::new(RecordingEvaluator(Mutex::new(vec![])));
    let antiburn_checks::preparation::PreparationOutcome::Ready(input) =
        antiburn_checks::preparation::prepare_decision_input(&active.compiled, &candidate)
    else {
        panic!("fixture input must be available");
    };
    adapter
        .record_preparation(
            &claimed,
            &PreparationRecord::Ready(evaluator.prepare(&active, input).unwrap()),
        )
        .unwrap();
    adapter
        .record_dispatch_intent(&claimed, 0, Duration::ZERO, 1)
        .unwrap();
    adapter
        .record_attempt_failure(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let restarted =
        Arc::new(StoreCheckStore::new(store, "owner-2".into(), active.id.scope.clone()).unwrap());
    restarted.register_activation(&active).unwrap();
    let engine = CheckEngine::with_policy(
        restarted,
        evaluator.clone(),
        ExecutionPolicy::SequentialReplay,
    );
    assert_eq!(engine.recover_pending().unwrap().candidates, 1);
    let StartOutcome::Started(mut task) = engine.start_one_due(&active, 1).unwrap() else {
        panic!("recovered retry must start");
    };
    let Poll::Ready(settlement) = task.as_mut().poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("scripted evaluator settles immediately");
    };
    assert_eq!(settlement.unwrap().outcome, RunOutcome::Accepted);
    assert_eq!(*evaluator.0.lock().unwrap(), [2]);
}
