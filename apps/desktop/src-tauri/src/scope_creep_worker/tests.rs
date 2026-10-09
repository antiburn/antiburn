use super::*;
use std::sync::Arc;

use crate::store::{
    AnalysisRecord, EvidenceCompletion, FencedTurnRowStore, PublishedEvidence, SessionRecord,
};
use antiburn_local::analysis::jev::{JevAnswer, JevUsage, JevWorkItemResult};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceKind, TurnRowSink, TurnRowStore, reader_for,
};
use serde_json::json;

pub(crate) mod native_sources;

#[test]
fn ignored_work_change_reopens_zero_runnable_completed_and_running_scope() {
    for status in ["completed", "running", "superseded"] {
        let fixture = NativeFixture::new(1);
        let candidate = fixture.publish();
        let capabilities = ModelCapabilities::jev_default();
        let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
        let result = input
            .check
            .reduce(&input.plan, &results(&input, true), true)
            .unwrap();
        persist(&fixture.store, &input, result);
        fixture
            .store
            .save_burn_check_scheduling(&input.durable, Some(1), 1, 0)
            .unwrap();
        assert!(
            current_publication(&fixture.store.lock(), &SourceFence::from(&candidate))
                .unwrap()
                .is_some()
        );
        if status == "running" {
            fixture.store.lock().execute("UPDATE burn_check_assessment SET status = 'running', lease_expires_at_epoch = ?1 WHERE check_id = ?2", params![unix_now() + 300, CHECK_ID]).unwrap();
        }
        if status == "superseded" {
            fixture.store.lock().execute("UPDATE burn_check_assessment SET status = 'superseded', last_error_category = 'cancelled' WHERE check_id = ?1", [CHECK_ID]).unwrap();
        }
        let work = &input.plan.prepared.groups[0].work[0];
        publish_ignored_work(&fixture.store, &input.durable, work);
        assert!(
            current_publication(&fixture.store.lock(), &SourceFence::from(&candidate))
                .unwrap()
                .is_none()
        );
        if status == "running" {
            assert!(!input_is_current(&fixture.store, &capabilities, &input).unwrap());
        } else {
            assert!(reconcile_scope_dependencies(&fixture.store, &candidate).unwrap());
        }
        assert!(!reconcile_scope_dependencies(&fixture.store, &candidate).unwrap());
        assert_eq!(
            fixture
                .store
                .burn_check_candidates_for_revision(
                    CHECK_ID,
                    &CHECK.evaluator_revision(),
                    unix_now(),
                    POLICY.idle_secs,
                    1
                )
                .unwrap()
                .len(),
            1
        );
        let mut next = load_descriptor_input(&fixture.store, &candidate, &capabilities).unwrap();
        assert_eq!(
            next.ignored_work,
            BTreeSet::from([work.reference.id.clone()])
        );
        assert_ne!(next.durable.input_revision, input.durable.input_revision);
        let saved = fixture
            .store
            .burn_check_assessment(&candidate.session.key, CHECK_ID)
            .unwrap()
            .unwrap();
        let mut cursor = restore_cursor(Some(&saved), &next.durable, 1);
        cursor.sampling = Some(new_sampling().unwrap());
        enumerate_scope_turn(&mut next, &mut cursor).unwrap();
        assert!(next.plan.prepared.groups.is_empty());
        assert!(valid_publication(&publication(
            &next,
            cursor.result.unwrap()
        )));
    }
}

fn publish_ignored_work(
    store: &Store,
    scope: &BurnCheckInput,
    work: &antiburn_local::checks::scope_creep::WorkBinding,
) {
    use antiburn_local::analysis::ignored_instructions::{AssessmentResult, evaluator_revision};
    let reference = json!({"instruction_id":"instruction", "instruction_digest":"rule-digest", "rule_id":"rule",
        "rule_heading":"Scope", "start_line":1, "end_line":1, "source":"synthetic.md",
        "provenance":"recorded_injection", "scope":"project", "action_id":work.reference.id,
        "action_digest":work.digest, "action_timestamp_ms":null, "action_stable":true});
    let decision = json!({"schema_revision":1, "source_generation":scope.source_generation,
        "source_fingerprint":scope.source_fingerprint, "publication_fence":scope.published_fence,
        "rule_action":reference, "rule_start_byte":0, "rule_end_byte":1,
        "action_anchor":{"source":work.reference, "content_digest":work.digest, "start_byte":0, "end_byte":1},
        "action_authority":"assistant", "action_is_request":false, "prerequisite":"not_required",
        "selected_evidence":[], "coverage":{"source_complete":true,"selected_history_complete":true,
            "read_request_inventory_complete":true,"results_excluded":true,"user_authority_excluded":true,"limitations":[]},
        "citations":[{"claim":"rule_requirement","source_ids":["instruction:rule"]},
            {"claim":"anchored_action","source_ids":[work.reference.id]}],
        "context_revision":"context", "evaluator_revision":evaluator_revision(), "model":"synthetic-model"});
    let result: AssessmentResult = serde_json::from_value(json!({"input_revision":"ignored-v1", "model_version":"synthetic-model",
        "findings":[{"id":"ignored-finding", "reference":reference, "decision":decision,
            "nearby_context_ids":[], "counterevidence_ids":[], "certainty":"likely", "composite_probability":1.0, "limitations":[]}],
        "pending_rules":[], "unassessed_comparisons":[], "coverage":{"eligible_rules":1,"candidate_pairs":1,
            "selected_comparisons":1,"unselected_pairs":0,"skipped_rules":[],"skipped_actions":[],
            "processing_limit_reached":false,"limitations":[]}, "request_count":1,"input_tokens":1,"output_tokens":1})).unwrap();
    assert!(result.findings[0].decision_record().is_some());
    let input = BurnCheckInput {
        check_id: "ignored_instructions".into(),
        input_revision: "ignored-v1".into(),
        evaluator_revision: evaluator_revision(),
        ..scope.clone()
    };
    store
        .set_check_enabled(
            antiburn_local::checks::DetectorId::IgnoredInstructions,
            true,
        )
        .unwrap();
    assert!(
        store
            .queue_burn_check_assessment(&input, unix_now(), POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, unix_now(), 300, POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        store
            .complete_burn_check_assessment(
                &input,
                &serde_json::to_string(&result).unwrap(),
                unix_now(),
                POLICY.idle_secs
            )
            .unwrap()
    );
}

#[test]
fn publication_budget_stops_before_dispatch_and_keeps_accepted_findings() {
    let fixture = NativeFixture::paged();
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let mut input = load_descriptor_input(&fixture.store, &candidate, &capabilities).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 1);
    cursor.sampling = Some(new_sampling().unwrap());
    enumerate_scope_turn(&mut input, &mut cursor).unwrap();
    cursor.sampling.as_mut().unwrap().begin_run();
    let first = cursor.sampling.as_mut().unwrap().choose_job().unwrap();
    let (mut input, mut cursor, first_plan, fits) =
        prepare_scope_target(input, cursor, first.candidate).unwrap();
    assert!(fits);
    let answers: Vec<_> = first_plan
        .work_items
        .iter()
        .map(|item| JevWorkItemResult {
            request_id: "accepted".into(),
            work_item_id: item.id.clone(),
            model: capabilities.model.clone(),
            answers: BTreeMap::from([(
                "scope_decision".into(),
                answer(ScopeQuestion::Decision, "likely_scope_expansion"),
            )]),
            evidence: item.window.evidence.clone(),
            usage: JevUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
        })
        .collect();
    let accepted = input.check.reduce(&first_plan, &answers, true).unwrap();
    record_completion(
        cursor.sampling.as_mut().unwrap(),
        &first,
        &accepted,
        &first_plan,
    )
    .unwrap();
    merge_result(cursor.result.as_mut().unwrap(), accepted, &first_plan);
    let next = cursor.sampling.as_mut().unwrap().choose_job().unwrap();
    let group = cursor
        .inventory
        .groups
        .iter()
        .find(|group| StableId::new("scope_work", &[group.id.as_bytes()]) == next.candidate)
        .unwrap();
    input
        .citations
        .insert(group.work[0].reference.id.clone(), "x".repeat(1024 * 1024));
    let (mut input, cursor, _, fits) = prepare_scope_target(input, cursor, next.candidate).unwrap();
    assert!(!fits);
    let result = cursor.result.as_ref().unwrap();
    assert_eq!(result.findings.len(), 1);
    assert!(
        result
            .coverage
            .limitations
            .contains(&"descriptor_storage_budget_reached".into())
    );
    assert!(!publication_has_clean_coverage(result));
    assert!(valid_publication(&publication(&input, result.clone())));
    assert_eq!(scope_scheduling_counts(&input, &cursor).2, 0);
    assert!(
        fixture
            .store
            .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        fixture
            .store
            .claim_burn_check_assessment(&input.durable, unix_now(), 300, POLICY.idle_secs)
            .unwrap()
    );
    save_scheduling(&fixture.store, &input, &cursor).unwrap();
    assert!(save_failure(&fixture.store, &input, &cursor, "sampling_incomplete", None).unwrap());
    assert_eq!(
        current_publication(&fixture.store.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .unwrap()
            .assessment
            .findings
            .len(),
        1
    );
    let saved = fixture
        .store
        .burn_check_assessment(&candidate.session.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = restore_cursor(Some(&saved), &input.durable, 99);
    enumerate_scope_turn(&mut input, &mut restored).unwrap();
    assert_eq!(restored.result.unwrap().findings.len(), 1);
}

#[test]
fn no_issue_with_incomplete_work_has_valid_uncertain_publication() {
    let fixture = NativeFixture::new(0);
    fixture.append(
        2,
        "assistant",
        json!({"type":"tool","tool":"bash","callID":"pending",
        "state":{"status":"running","input":{"command":"touch /synthetic/billing.rs"}}}),
    );
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    assert!(
        input
            .plan
            .prepared
            .groups
            .iter()
            .any(|group| group.limitation.is_some())
    );
    let result = input
        .check
        .reduce(&input.plan, &results(&input, true), true)
        .unwrap();
    assert!(
        result
            .decisions
            .iter()
            .any(|decision| decision.outcome == Some(ScopeAnswer::NoIssue)
                && decision.status == ScopeCreepStatus::Uncertain)
    );
    assert!(valid_publication(&publication(&input, result.clone())));
    assert!(!publication_has_clean_coverage(&result));
    let mut forged = publication(&input, result);
    for decision in &mut forged.assessment.decisions {
        if decision.status == ScopeCreepStatus::Uncertain {
            decision.status = ScopeCreepStatus::Clean;
        }
    }
    assert!(!valid_publication(&forged));
}

#[test]
fn descriptor_storage_budget_preserves_accepted_answers_and_partial_publication() {
    let fixture = NativeFixture::paged_groups(1000);
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let mut input = load_descriptor_input(&fixture.store, &candidate, &capabilities).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 7);
    cursor.sampling = Some(new_sampling().unwrap());
    while scope_enumeration_pending(&cursor) {
        enumerate_scope_turn(&mut input, &mut cursor).unwrap();
    }
    assert!(cursor.inventory.limitation.is_some());
    assert!(cursor.inventory.groups.len() < 1000);
    assert!(!cursor.inventory.complete);
    cursor.sampling.as_mut().unwrap().begin_run();
    let job = cursor.sampling.as_mut().unwrap().choose_job().unwrap();
    let selected = input
        .check
        .prepare_descriptors(
            &cursor.inventory,
            &capabilities,
            &BTreeSet::from([job.candidate]),
        )
        .unwrap();
    let accepted: Vec<_> = selected
        .work_items
        .iter()
        .map(|item| JevWorkItemResult {
            request_id: "accepted-before-restart".into(),
            work_item_id: item.id.clone(),
            answers: BTreeMap::from([(
                "scope_decision".into(),
                answer(ScopeQuestion::Decision, "no_issue"),
            )]),
            evidence: item.window.evidence.clone(),
            model: capabilities.model.clone(),
            usage: JevUsage {
                input_tokens: 10,
                output_tokens: 2,
            },
        })
        .collect();
    let result = input.check.reduce(&selected, &accepted, true).unwrap();
    record_completion(cursor.sampling.as_mut().unwrap(), &job, &result, &selected).unwrap();
    merge_result(cursor.result.as_mut().unwrap(), result, &selected);
    for group in selected.prepared.groups {
        let current = input
            .plan
            .prepared
            .groups
            .iter_mut()
            .find(|current| current.id == group.id)
            .unwrap();
        *current = group;
    }
    cursor.prepared = Some(input.plan.prepared.clone());
    cursor.active_job = Some(job);
    cursor.run_progress.results = accepted
        .into_iter()
        .map(|result| (result.work_item_id.clone(), result))
        .collect();
    update_result_counts(&mut cursor, &input);
    let mut legacy = serde_json::to_value(&cursor).unwrap();
    legacy["inventory"] = serde_json::to_value(&cursor.inventory).unwrap();
    let legacy = serde_json::to_string(&legacy).unwrap();
    let compact = serialize_cursor(&cursor).unwrap();
    assert!(
        compact.len() < 2 * 1024 * 1024,
        "compact bytes: {}",
        compact.len()
    );
    assert!(compact.len() < legacy.len());
    let now = unix_now();
    assert!(
        fixture
            .store
            .queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        fixture
            .store
            .claim_burn_check_assessment(&input.durable, now, POLICY.lease_secs, POLICY.idle_secs)
            .unwrap()
    );
    assert!(save_cursor(&fixture.store, &input.durable, &cursor).unwrap());
    let published = serde_json::to_string(&publication(
        &input,
        cursor.result.as_ref().unwrap().clone(),
    ))
    .unwrap();
    assert!(
        published.len() < 1024 * 1024,
        "publication bytes: {}",
        published.len()
    );
    assert!(valid_publication(
        &serde_json::from_str(&published).unwrap()
    ));
    let mut legacy_publication: serde_json::Value = serde_json::from_str(&published).unwrap();
    legacy_publication["prepared"] = serde_json::to_value(&input.plan.prepared).unwrap();
    assert!(valid_publication(
        &serde_json::from_value(legacy_publication).unwrap()
    ));
    let mut invalid: serde_json::Value = serde_json::from_str(&compact).unwrap();
    invalid["inventory"]["groups"][0][2] = json!([usize::MAX]);
    assert!(serde_json::from_value::<AssessmentCursor>(invalid).is_err());
    assert!(save_failure(&fixture.store, &input, &cursor, "continuing", Some(now + 1)).unwrap());
    let mut saved = fixture
        .store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = restore_cursor(Some(&saved), &input.durable, 99);
    assert_eq!(restored.inventory, cursor.inventory);
    assert_eq!(
        serde_json::to_value(&restored.run_progress).unwrap(),
        serde_json::to_value(&cursor.run_progress).unwrap()
    );
    enumerate_scope_turn(&mut input, &mut restored).unwrap();
    assert_eq!(
        restored.prepared.as_ref().unwrap().groups,
        cursor.prepared.as_ref().unwrap().groups
    );
    assert_eq!(
        restored.prepared.as_ref().unwrap().semantic_epoch,
        cursor.prepared.as_ref().unwrap().semantic_epoch
    );
    assert_eq!(
        serde_json::to_value(&restored.sampling).unwrap(),
        serde_json::to_value(&cursor.sampling).unwrap()
    );
    assert!(valid_publication(&publication(
        &input,
        restored.result.unwrap()
    )));
    saved.progress_json = legacy;
    assert_eq!(
        restore_cursor(Some(&saved), &input.durable, 100).inventory,
        cursor.inventory
    );
}

#[test]
fn expired_zero_runnable_lease_recovers_and_candidate_errors_settle_once() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    let now = unix_now();
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(&input.durable, now, 1, POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .save_burn_check_scheduling(&input.durable, Some(1), 1, 0)
        .unwrap();
    let reopened = Store::open(fixture.directory.path()).unwrap();
    assert_eq!(
        reopened
            .burn_check_candidates_for_revision(
                CHECK_ID,
                &CHECK.evaluator_revision(),
                now + 2,
                POLICY.idle_secs,
                1
            )
            .unwrap()
            .len(),
        1
    );
    assert!(
        reopened
            .queue_burn_check_assessment(&input.durable, now + 2, POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        reopened
            .claim_burn_check_assessment(&input.durable, now + 2, 300, POLICY.idle_secs)
            .unwrap()
    );
    reopened
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET lease_expires_at_epoch = NULL WHERE check_id = ?1",
            [CHECK_ID],
        )
        .unwrap();
    assert_eq!(
        reopened
            .burn_check_candidates_for_revision(
                CHECK_ID,
                &CHECK.evaluator_revision(),
                now + 2,
                POLICY.idle_secs,
                1
            )
            .unwrap()
            .len(),
        1
    );
    assert!(
        reopened
            .claim_burn_check_assessment(&input.durable, now + 2, 300, POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        !reopened
            .settle_burn_check_candidate_error(
                &candidate,
                CHECK_ID,
                "obsolete",
                now + 3,
                "candidate_error",
                None
            )
            .unwrap()
    );
    assert!(
        reopened
            .settle_burn_check_candidate_error(
                &candidate,
                CHECK_ID,
                &CHECK.evaluator_revision(),
                now + 3,
                "candidate_error",
                None
            )
            .unwrap()
    );
    let saved = reopened
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, "failed");
    let lease: Option<i64> = reopened
        .lock()
        .query_row(
            "SELECT lease_expires_at_epoch FROM burn_check_assessment WHERE check_id = ?1",
            [CHECK_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(lease, None);
    assert!(
        reopened
            .burn_check_candidates_for_revision(
                CHECK_ID,
                &CHECK.evaluator_revision(),
                now + 600,
                POLICY.idle_secs,
                1
            )
            .unwrap()
            .is_empty()
    );
    fixture.append(
        3,
        "user",
        json!({"type":"text","text":"Also check the parser tests."}),
    );
    let changed = fixture.publish();
    assert_ne!(changed.source_generation, candidate.source_generation);
    assert_eq!(
        reopened
            .burn_check_candidates_for_revision(
                CHECK_ID,
                &CHECK.evaluator_revision(),
                now + 600,
                POLICY.idle_secs,
                1
            )
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn freshness_uses_exact_metadata_and_configuration_without_reading_content() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let mut input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    assert!(input_is_current(&fixture.store, &capabilities, &input).unwrap());
    for changed in [
        BurnCheckInput {
            incarnation: input.durable.incarnation + 1,
            ..input.durable.clone()
        },
        BurnCheckInput {
            source_generation: input.durable.source_generation + 1,
            ..input.durable.clone()
        },
        BurnCheckInput {
            published_fence: input.durable.published_fence + 1,
            ..input.durable.clone()
        },
        BurnCheckInput {
            source_fingerprint: Some("changed".into()),
            ..input.durable.clone()
        },
        BurnCheckInput {
            activity_cursor: "changed".into(),
            ..input.durable.clone()
        },
    ] {
        assert!(!Store::burn_check_input_is_current(&fixture.store.lock(), &changed).unwrap());
    }
    fixture
        .store
        .lock()
        .execute_batch("ALTER TABLE turn_content RENAME TO unavailable_content")
        .unwrap();
    assert!(input_is_current(&fixture.store, &capabilities, &input).unwrap());
    fixture
        .store
        .lock()
        .execute_batch("ALTER TABLE unavailable_content RENAME TO turn_content")
        .unwrap();
    input.ignored_work.insert("changed-ignored-work".into());
    assert!(!input_is_current(&fixture.store, &capabilities, &input).unwrap());
    input.ignored_work.clear();
    for column in [
        "parser_revision",
        "analyzer_revision",
        "evidence_schema_revision",
        "analyzed_generation",
    ] {
        let connection = fixture.store.lock();
        connection.execute_batch("SAVEPOINT stale_test").unwrap();
        connection
            .execute(&format!("UPDATE session_evidence SET {column} = NULL"), [])
            .unwrap();
        assert!(!Store::burn_check_input_is_current(&connection, &input.durable).unwrap());
        connection
            .execute_batch("ROLLBACK TO stale_test; RELEASE stale_test")
            .unwrap();
    }
    fixture
        .store
        .set_internal_value_checked("internal:smartChecksConnectionChangePendingV1", "true")
        .unwrap();
    assert!(!input_is_current(&fixture.store, &capabilities, &input).unwrap());
}

fn complete_cursor(input: &PreparedInput, generation: u64) -> AssessmentCursor {
    let mut cursor = restore_cursor(None, &input.durable, generation);
    input
        .check
        .enumerate_descriptors(&mut cursor.inventory)
        .unwrap();
    assert!(cursor.inventory.complete);
    cursor
}

#[test]
fn descriptor_pages_resume_with_unknown_totals_and_only_selected_scope_hydration() {
    let fixture = NativeFixture::paged_groups(270);
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let mut input = load_descriptor_input(&fixture.store, &candidate, &capabilities).unwrap();
    assert!(input.plan.work_items.is_empty());
    assert!(input.plan.prepared.groups.is_empty());
    let mut cursor = restore_cursor(None, &input.durable, 7);
    cursor.sampling = Some(new_sampling().unwrap());
    enumerate_scope_turn(&mut input, &mut cursor).unwrap();
    assert!(!cursor.inventory.complete);
    assert!(!cursor.inventory.groups.is_empty());
    assert!(cursor.inventory.groups.len() <= 256);
    assert!(input.plan.work_items.is_empty());
    assert!(
        input
            .plan
            .prepared
            .groups
            .iter()
            .all(|group| group.window_ids.is_empty())
    );
    cursor.sampling.as_mut().unwrap().begin_run();
    let jobs: Vec<_> =
        std::iter::from_fn(|| cursor.sampling.as_mut().unwrap().choose_job()).collect();
    assert_eq!(jobs.len(), 4);
    let selected = input
        .check
        .prepare_descriptors(
            &cursor.inventory,
            &capabilities,
            &jobs.iter().map(|job| job.candidate).collect(),
        )
        .unwrap();
    assert_eq!(selected.prepared.groups.len(), 4);
    assert_eq!(selected.work_items.len(), 4);
    let answers: Vec<_> = selected
        .work_items
        .iter()
        .map(|item| JevWorkItemResult {
            request_id: "synthetic-accepted-page".into(),
            work_item_id: item.id.clone(),
            answers: BTreeMap::from([(
                "scope_decision".into(),
                answer(ScopeQuestion::Decision, "uncertain"),
            )]),
            evidence: item.window.evidence.clone(),
            model: capabilities.model.clone(),
            usage: JevUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        })
        .collect();
    let accepted = input.check.reduce(&selected, &answers, true).unwrap();
    for job in &jobs {
        record_completion(cursor.sampling.as_mut().unwrap(), job, &accepted, &selected).unwrap();
    }
    merge_result(cursor.result.as_mut().unwrap(), accepted, &selected);
    for group in &selected.prepared.groups {
        *input
            .plan
            .prepared
            .groups
            .iter_mut()
            .find(|current| current.id == group.id)
            .unwrap() = group.clone();
    }
    cursor.prepared = Some(input.plan.prepared.clone());
    update_result_counts(&mut cursor, &input);
    let now = unix_now();
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(&input.durable, now, POLICY.lease_secs, POLICY.idle_secs)
        .unwrap();
    save_scheduling(&fixture.store, &input, &cursor).unwrap();
    save_cursor(&fixture.store, &input.durable, &cursor).unwrap();
    fixture
        .store
        .release_failed_burn_check_lease(&input.durable, "continuing", now + 1)
        .unwrap();
    let first_position = cursor.inventory.next_action;
    let first_groups = cursor.inventory.groups.clone();
    let reopened = Store::open(fixture.directory.path()).unwrap();
    let counts: (Option<usize>, usize, usize) = reopened.lock().query_row(
        "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1",
        [CHECK_ID], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(counts, (None, 4, first_groups.len() - 4));
    let saved = reopened
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = restore_cursor(Some(&saved), &input.durable, 99);
    assert_eq!(restored.inventory, cursor.inventory);
    let mut resumed_input = load_descriptor_input(&reopened, &candidate, &capabilities).unwrap();
    enumerate_scope_turn(&mut resumed_input, &mut restored).unwrap();
    assert!(restored.inventory.next_action > first_position);
    assert!(restored.inventory.groups.starts_with(&first_groups));
    assert!(restored.inventory.groups.len() - first_groups.len() <= 256);
    while !restored.inventory.complete {
        enumerate_scope_turn(&mut resumed_input, &mut restored).unwrap();
    }
    assert_eq!(restored.inventory.groups.len(), 270);
    save_scheduling(&reopened, &resumed_input, &restored).unwrap();
    let counts: (Option<usize>, usize, usize) = reopened.lock().query_row(
        "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1",
        [CHECK_ID], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(counts, (Some(270), 4, 266));
    assert!(!publication_has_clean_coverage(
        restored.result.as_ref().unwrap()
    ));
    assert!(valid_publication(&publication(
        &resumed_input,
        restored.result.as_ref().unwrap().clone()
    )));
    let mut saved_after_pages = saved;
    saved_after_pages.progress_json = serde_json::to_string(&restored).unwrap();
    fixture
        .store
        .lock()
        .execute(
            "UPDATE burn_check_assessment SET next_attempt_at_epoch = 0 WHERE check_id = ?1",
            [CHECK_ID],
        )
        .unwrap();
    fixture.append(
        545,
        "assistant",
        json!({"type":"text","text":"Unrelated status update."}),
    );
    let appended = fixture.publish();
    let mut appended_input =
        load_descriptor_input(&fixture.store, &appended, &capabilities).unwrap();
    let mut appended_cursor =
        restore_cursor(Some(&saved_after_pages), &appended_input.durable, 100);
    enumerate_scope_turn(&mut appended_input, &mut appended_cursor).unwrap();
    assert!(!appended_cursor.inventory.complete);
    while !appended_cursor.inventory.complete {
        enumerate_scope_turn(&mut appended_input, &mut appended_cursor).unwrap();
    }
    assert_eq!(
        appended_cursor.result.as_ref().unwrap().assessed_candidates,
        4
    );
    assert_eq!(
        scope_scheduling_counts(&appended_input, &appended_cursor),
        (Some(271), 4, 267)
    );
    assert!(appended_input.plan.work_items.is_empty());
    assert!(valid_publication(&publication(
        &appended_input,
        appended_cursor.result.as_ref().unwrap().clone()
    )));
}

#[test]
fn descriptor_known_gap_is_terminal_and_unreviewed_after_process_restart() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.request_body_bytes.value = Some(1);
    let mut input = load_descriptor_input(&fixture.store, &candidate, &capabilities).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 7);
    cursor.sampling = Some(new_sampling().unwrap());
    enumerate_scope_turn(&mut input, &mut cursor).unwrap();
    cursor.sampling.as_mut().unwrap().begin_run();
    let job = cursor.sampling.as_mut().unwrap().choose_job().unwrap();
    let selected = input
        .check
        .prepare_descriptors(
            &cursor.inventory,
            &capabilities,
            &BTreeSet::from([job.candidate]),
        )
        .unwrap();
    assert!(selected.work_items.is_empty());
    assert_eq!(
        selected.prepared.groups[0].limitation.as_deref(),
        Some("work_context_too_large")
    );
    let gap = input.check.reduce(&selected, &[], false).unwrap();
    merge_result(cursor.result.as_mut().unwrap(), gap, &selected);
    input.plan.prepared.groups = selected.prepared.groups.clone();
    cursor.prepared = Some(input.plan.prepared.clone());
    cursor
        .sampling
        .as_mut()
        .unwrap()
        .terminate_candidate(&job)
        .unwrap();
    update_result_counts(&mut cursor, &input);
    assert_eq!(cursor.result.as_ref().unwrap().assessed_candidates, 0);
    assert_eq!(cursor.result.as_ref().unwrap().coverage.skipped_items, 1);
    let now = unix_now();
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(&input.durable, now, POLICY.lease_secs, POLICY.idle_secs)
        .unwrap();
    save_scheduling(&fixture.store, &input, &cursor).unwrap();
    save_cursor(&fixture.store, &input.durable, &cursor).unwrap();
    let saved = fixture
        .store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = restore_cursor(Some(&saved), &input.durable, 99);
    let mut resumed_input =
        load_descriptor_input(&fixture.store, &candidate, &capabilities).unwrap();
    enumerate_scope_turn(&mut resumed_input, &mut restored).unwrap();
    restored.sampling.as_mut().unwrap().begin_run();
    assert!(restored.sampling.as_mut().unwrap().choose_job().is_none());
    assert_eq!(restored.result.as_ref().unwrap().coverage.skipped_items, 1);
    assert_eq!(restored.result.as_ref().unwrap().assessed_candidates, 0);
    assert!(!publication_has_clean_coverage(
        restored.result.as_ref().unwrap()
    ));
}

#[test]
fn scheduler_counts_include_terminal_targets_without_reviewing_them() {
    let fixture = NativeFixture::paged();
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    let mut cursor = complete_cursor(&input, 7);
    let mut sampling = new_sampling().unwrap();
    synchronize_sampling(&input, &mut sampling).unwrap();
    sampling.begin_run();
    let reviewed = sampling.choose_job().unwrap();
    let plan =
        ScopeCreepCheck::select_candidates(&input.plan, &BTreeSet::from([reviewed.candidate]));
    let mut answers = results(&input, false);
    answers.retain(|answer| {
        plan.work_items
            .iter()
            .any(|item| item.id == answer.work_item_id)
    });
    for result in &mut answers {
        result.answers = BTreeMap::from([(
            "scope_decision".into(),
            answer(ScopeQuestion::Decision, "uncertain"),
        )]);
    }
    let reduction = input.check.reduce(&plan, &answers, true).unwrap();
    record_completion(&mut sampling, &reviewed, &reduction, &plan).unwrap();
    let terminal = sampling.choose_job().unwrap();
    sampling.terminate_candidate(&terminal).unwrap();
    cursor.sampling = Some(sampling);
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
        .unwrap();
    assert!(save_scheduling(&fixture.store, &input, &cursor).unwrap());
    let counts = || {
        fixture.store.lock().query_row(
        "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1",
        [CHECK_ID], |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?, row.get::<_, usize>(2)?))).unwrap()
    };
    assert_eq!(counts(), (9, 1, 7));
    let sampling = cursor.sampling.as_mut().unwrap();
    while sampling.runnable_count(ScopeCreepCheck::check_identity()) > 0 {
        sampling.begin_run();
        while let Some(job) = sampling.choose_job() {
            sampling.terminate_candidate(&job).unwrap();
        }
    }
    save_scheduling(&fixture.store, &input, &cursor).unwrap();
    assert_eq!(counts(), (9, 1, 0));
    synchronize_sampling(&input, cursor.sampling.as_mut().unwrap()).unwrap();
    cursor.sampling.as_mut().unwrap().begin_run();
    assert!(cursor.sampling.as_mut().unwrap().choose_job().is_none());
}

#[test]
fn per_target_readiness_keeps_unknown_siblings_out_and_uses_resolved_model_revision() {
    use crate::store::BurnCheckRequestAdmission;
    let fixture = NativeFixture::paged();
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    let handle = crate::jev::worker::WorkerHandle::default();
    let mut connection = handle.system_one_connection();
    connection
        .model_revision
        .clone_from(&capabilities.model_revision);
    let inventory = ScopeCreepCheck::sampling_candidates(&input.plan);
    let first = ScopeCreepCheck::select_candidates(&input.plan, &BTreeSet::from([inventory[0].id]));
    let sibling =
        ScopeCreepCheck::select_candidates(&input.plan, &BTreeSet::from([inventory[1].id]));
    let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
        &first.work_items,
        &capabilities,
    );
    let identities = crate::jev::worker::batch_request_identities(
        &connection,
        &input.durable,
        &packed.batches[0],
    );
    fixture
        .store
        .track_burn_check_requests(&identities, "unknown-delivery", unix_now())
        .unwrap();
    assert_eq!(
        dispatch_readiness(
            &fixture.store,
            &handle,
            &input.durable,
            &capabilities,
            &first,
            &JevRunProgress::default()
        )
        .unwrap(),
        BurnCheckRequestAdmission::Unresolved
    );
    assert_eq!(
        dispatch_readiness(
            &fixture.store,
            &handle,
            &input.durable,
            &capabilities,
            &sibling,
            &JevRunProgress::default()
        )
        .unwrap(),
        BurnCheckRequestAdmission::Admitted
    );
    let mut progress = JevRunProgress::default();
    for result in results(&input, false).into_iter().filter(|result| {
        first
            .work_items
            .iter()
            .any(|item| item.id == result.work_item_id)
    }) {
        progress.results.insert(result.work_item_id.clone(), result);
    }
    assert_eq!(
        dispatch_readiness(
            &fixture.store,
            &handle,
            &input.durable,
            &capabilities,
            &first,
            &progress
        )
        .unwrap(),
        BurnCheckRequestAdmission::Admitted
    );
}

#[test]
fn unrelated_append_reuses_exact_scope_answers_but_configuration_change_reopens_them() {
    let fixture = NativeFixture::paged();
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    let mut cursor = complete_cursor(&input, 7);
    cursor.sampling = Some(new_sampling().unwrap());
    synchronize_sampling(&input, cursor.sampling.as_mut().unwrap()).unwrap();
    cursor.sampling.as_mut().unwrap().begin_run();
    let job = cursor.sampling.as_mut().unwrap().choose_job().unwrap();
    let plan = ScopeCreepCheck::select_candidates(&input.plan, &BTreeSet::from([job.candidate]));
    let answers: Vec<_> = results(&input, false)
        .into_iter()
        .filter(|result| {
            plan.work_items
                .iter()
                .any(|item| item.id == result.work_item_id)
        })
        .collect();
    let result = input.check.reduce(&plan, &answers, true).unwrap();
    record_completion(cursor.sampling.as_mut().unwrap(), &job, &result, &plan).unwrap();
    cursor.result = Some(input.check.reduce(&input.plan, &[], false).unwrap());
    merge_result(cursor.result.as_mut().unwrap(), result, &plan);
    update_result_counts(&mut cursor, &input);
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(
            &input.durable,
            unix_now(),
            POLICY.lease_secs,
            POLICY.idle_secs,
        )
        .unwrap();
    save_failure(&fixture.store, &input, &cursor, "continuing", None).unwrap();
    let saved = fixture
        .store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    fixture.append(
        25,
        "assistant",
        json!({"type":"text","text":"Unrelated status update."}),
    );
    let next_candidate = fixture.publish();
    let mut next = load_input(&fixture.store, &next_candidate, &capabilities).unwrap();
    assert_ne!(next.durable.input_revision, input.durable.input_revision);
    let mut restored = restore_cursor(Some(&saved), &next.durable, 99);
    synchronize_sampling(&next, restored.sampling.as_mut().unwrap()).unwrap();
    assert_eq!(
        restored
            .sampling
            .as_ref()
            .unwrap()
            .coverage(ScopeCreepCheck::check_identity())
            .unwrap()
            .completed,
        1
    );
    restored.result = Some(next.check.reduce(&next.plan, &[], false).unwrap());
    reuse_accepted_result(Some(&saved), &next, &mut restored).unwrap();
    assert_eq!(
        restored
            .result
            .as_ref()
            .unwrap()
            .decisions
            .iter()
            .filter(|decision| decision.outcome.is_some())
            .count(),
        1
    );
    next.configuration_fence = "changed-active-connection".into();
    synchronize_sampling(&next, restored.sampling.as_mut().unwrap()).unwrap();
    assert_eq!(
        restored
            .sampling
            .as_ref()
            .unwrap()
            .coverage(ScopeCreepCheck::check_identity())
            .unwrap()
            .completed,
        0
    );
}

#[test]
fn transport_retry_keeps_the_store_timestamp_instead_of_the_check_fallback() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    let now = unix_now();
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(&input.durable, now, POLICY.lease_secs, POLICY.idle_secs)
        .unwrap();
    let mut cursor = complete_cursor(&input, 7);
    cursor.result = Some(input.check.reduce(&input.plan, &[], false).unwrap());
    let handle = crate::jev::worker::WorkerHandle::default();
    let mut connection = handle.system_one_connection();
    connection
        .model_revision
        .clone_from(&capabilities.model_revision);
    let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
        &input.plan.work_items,
        &capabilities,
    );
    let identities = crate::jev::worker::batch_request_identities(
        &connection,
        &input.durable,
        &packed.batches[0],
    );
    fixture.store.lock().execute("INSERT INTO burn_check_dispatch_attempt
        (request_identity, environment_key, agent, session_id, attempts) VALUES (?1, ?2, ?3, ?4, 1)",
        params![identities[0], input.durable.key.environment_key, input.durable.key.agent, input.durable.key.session_id]).unwrap();
    for delay in [5, 30] {
        fixture
            .store
            .defer_burn_check_dispatch(&input.durable, &identities, Some(now + delay))
            .unwrap();
        assert_eq!(
            dispatch_readiness(
                &fixture.store,
                &handle,
                &input.durable,
                &capabilities,
                &input.plan,
                &JevRunProgress::default()
            )
            .unwrap(),
            crate::store::BurnCheckRequestAdmission::Deferred
        );
        let retry = fixture
            .store
            .burn_check_next_attempt_at(&input.durable)
            .unwrap();
        save_failure(
            &fixture.store,
            &input,
            &cursor,
            "provider_unavailable",
            retry,
        )
        .unwrap();
        assert_eq!(
            fixture
                .store
                .burn_check_next_attempt_at(&input.durable)
                .unwrap(),
            Some(now + delay)
        );
    }
}

pub(crate) struct NativeFixture {
    pub(crate) directory: tempfile::TempDir,
    source_path: std::path::PathBuf,
    pub(crate) store: Store,
}

#[test]
fn production_loader_uses_original_enrollment_and_keeps_pre_enrollment_scope() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let original = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    let snapshot = fixture
        .store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            DetectorInput::ScopeCreep,
        )
        .unwrap();
    let mut latest_boundary = candidate.clone();
    latest_boundary.boundary_at_epoch = 999_999;
    latest_boundary
        .boundary_positions
        .insert(snapshot.boundary().source_key.clone(), 999_999);
    let loaded = load_input(&fixture.store, &latest_boundary, &capabilities).unwrap();
    assert_eq!(
        loaded.durable.input_revision,
        original.durable.input_revision
    );
    assert_eq!(loaded.plan.prepared.groups, original.plan.prepared.groups);
    assert_eq!(loaded.plan.shared_context, original.plan.shared_context);
    let mut future_only = candidate.clone();
    future_only
        .boundary_positions
        .insert(snapshot.boundary().source_key.clone(), 999_999);
    let context_only = prepare(
        &future_only,
        snapshot,
        &capabilities,
        BTreeSet::new(),
        configuration_fence(&fixture.store.lock()).unwrap(),
    )
    .unwrap();
    assert!(context_only.plan.prepared.groups.is_empty());
    assert_eq!(
        context_only.plan.shared_context,
        original.plan.shared_context
    );
}

impl NativeFixture {
    fn paged() -> Self {
        Self::paged_groups(9)
    }

    pub(crate) fn paged_groups(groups: usize) -> Self {
        let fixture = Self::new(0);
        for index in 0..groups {
            fixture.append(
                2 + index * 2,
                "assistant",
                json!({"type":"tool","tool":"write","callID":format!("write-{index}"),
                    "state":{"status":"completed","input":{"filePath":format!("/synthetic/billing-{index}.rs"),"content":"new billing feature"},"output":"File written successfully."}}),
            );
            fixture.append(
                3 + index * 2,
                "user",
                json!({"type":"text","text":"Continue the parser fix. Do not change billing."}),
            );
        }
        fixture
    }

    pub(crate) fn new(groups: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("opencode.db");
        let source = rusqlite::Connection::open(&source_path).unwrap();
        source.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
            INSERT INTO session VALUES ('scope', NULL, 1000, 9000);").unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .set_check_enabled(antiburn_local::checks::DetectorId::ScopeCreep, true)
            .unwrap();
        store.capture_burn_check_boundaries(&[CHECK_ID], 0).unwrap();
        let fixture = Self {
            directory,
            source_path,
            store,
        };
        fixture.append(
            0,
            "user",
            json!({"type":"text","text":"Fix the parser. Do not change billing."}),
        );
        for index in 1..=groups {
            fixture.append(index, "assistant", json!({"type":"tool","tool":"write","callID":format!("write-{index}"),
                "state":{"status":"completed","input":{"filePath":format!("/synthetic/billing-{index}.rs"),"content":"new billing feature"},"output":"File written successfully."}}));
        }
        fixture.append(
            groups + 1,
            "user",
            json!({"type":"text","text":"Explain the parser fix."}),
        );
        fixture
    }

    pub(crate) fn append(&self, index: usize, role: &str, part: serde_json::Value) {
        let source = rusqlite::Connection::open(&self.source_path).unwrap();
        source
            .execute(
                "INSERT INTO message VALUES (?1, 'scope', ?2, ?2, ?3)",
                params![
                    format!("m{index}"),
                    index + 1001,
                    json!({"role":role}).to_string()
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES (?1, ?2, 'scope', ?3, ?3, ?4)",
                params![
                    format!("p{index}"),
                    format!("m{index}"),
                    index + 1001,
                    part.to_string()
                ],
            )
            .unwrap();
    }

    pub(crate) fn publish(&self) -> BurnCheckCandidate {
        let (latest, rows) = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(
                antiburn_local::discovery::agents::opencode::db_session_fingerprint(
                    self.source_path.clone(),
                    "scope".into(),
                ),
            )
            .unwrap();
        let fingerprint = format!("sv1:db:{latest}:{rows}");
        let key = SessionKey::new("native", "opencode", "scope");
        self.store
            .upsert_sessions(
                &[SessionRecord {
                    key: key.clone(),
                    source_kind: "providerDb".into(),
                    source_label: "synthetic-db".into(),
                    wsl_distro: None,
                    title: None,
                    title_source: None,
                    cwd: None,
                    surface: "cli".into(),
                    updated_at_epoch: Some(9),
                    activity_cursor: format!("activity-{rows}"),
                    activity_source: "event".into(),
                    subagent_count: 0,
                    fork_parent_session_id: None,
                    source_fingerprint: Some(fingerprint.clone()),
                }],
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
        let claim = self
            .store
            .claim_next_evidence(&["opencode"], 100, 60)
            .unwrap()
            .unwrap();
        let writer = Arc::new(FencedTurnRowStore::new(
            self.store.clone(),
            key.clone(),
            claim.claim_fence,
        ));
        let input = SessionInput {
            agent: "opencode".into(),
            session_id: key.session_id.clone(),
            source: RawSource::Sqlite(self.source_path.clone()),
            source_format: SourceFormat::OpenCodeSqliteV2,
            fork_parent_session_id: None,
        };
        let reader = reader_for("opencode");
        let mut sink = CompositeSink::with_turn_rows(
            SessionMetricsAccumulator::new("opencode", &key.session_id),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "opencode".into(),
                session_id: key.session_id.clone(),
                kind: SourceKind::Sqlite,
                capabilities: reader.capabilities(&input),
            }),
            TurnRowSink::new(
                writer.clone() as Arc<dyn TurnRowStore>,
                &key.session_id,
                None,
            ),
        );
        let outcome = reader
            .visit_db_claimed(&input, &fingerprint, &|| false, &mut sink)
            .unwrap();
        sink.observe_source_outcome(outcome);
        writer
            .write_coverage_record(&sink.coverage_record().unwrap())
            .unwrap();
        let revisions = crate::analysis::projection_revisions();
        self.store
            .publish_projections(
                &AnalysisRecord {
                    key: key.clone(),
                    model_breakdown_json: "{}".into(),
                    pricing_breakdown_json: "{}".into(),
                    inclusive_models_json: "[]".into(),
                    initial_context_json: None,
                    source_summaries_json: None,
                    provider_hints_json: None,
                    source_fingerprint: fingerprint,
                    pricing_generation: 1,
                    analyzed_generation: claim.source_generation,
                    parser_revision: revisions.parser_revision,
                    analyzer_revision: revisions.analyzer_revision,
                    metrics_schema_revision: revisions.metrics_schema_revision,
                },
                Some(1),
                &EvidenceCompletion {
                    claim_fence: claim.claim_fence,
                    status: PublishedEvidence::Ready,
                    evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    evidence_json: serde_json::to_string(&sink.evidence().unwrap()).unwrap(),
                },
                &[],
                &[],
            )
            .unwrap();
        self.store
            .burn_check_candidates_for_revision(
                CHECK_ID,
                &CHECK.evaluator_revision(),
                1000,
                POLICY.idle_secs,
                16,
            )
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.session.key == key)
            .unwrap()
    }
    pub(crate) fn publish_finding(&self) -> BurnCheckCandidate {
        self.publish_finding_with_capabilities(ModelCapabilities::jev_default())
    }

    pub(crate) fn publish_finding_with_capabilities(
        &self,
        capabilities: ModelCapabilities,
    ) -> BurnCheckCandidate {
        let candidate = self.publish();
        let input = load_input(&self.store, &candidate, &capabilities).unwrap();
        let result = input
            .check
            .reduce(&input.plan, &results(&input, false), true)
            .unwrap();
        assert!(!result.findings.is_empty());
        persist(&self.store, &input, result);
        candidate
    }
}

fn answer(question: ScopeQuestion, key: &str) -> JevAnswer {
    let antiburn_local::analysis::jev::JevQuestion::Choice { criteria, .. } = question.question()
    else {
        panic!("expected choice");
    };
    JevAnswer::Choice {
        choice: key.into(),
        confidence: 1.0,
        probabilities: criteria
            .keys()
            .map(|choice| (choice.clone(), if choice == key { 1.0 } else { 0.0 }))
            .collect(),
    }
}

fn results(input: &PreparedInput, acceptance: bool) -> Vec<JevWorkItemResult> {
    input
        .plan
        .work_items
        .iter()
        .map(|item| JevWorkItemResult {
            request_id: item.id.clone(),
            work_item_id: item.id.clone(),
            model: input.plan.capabilities.model.clone(),
            evidence: item.window.evidence.clone(),
            usage: JevUsage {
                input_tokens: 10,
                output_tokens: 2,
            },
            answers: BTreeMap::from([(
                "scope_decision".into(),
                answer(
                    ScopeQuestion::Decision,
                    if acceptance {
                        "no_issue"
                    } else {
                        "likely_scope_expansion"
                    },
                ),
            )]),
        })
        .collect()
}

fn persist(store: &Store, input: &PreparedInput, result: ScopeCreepResult) {
    assert!(
        store
            .queue_burn_check_assessment(&input.durable, 1000, POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input.durable, 1000, POLICY.lease_secs, POLICY.idle_secs)
            .unwrap()
    );
    let publication = publication(input, result);
    assert!(valid_publication(&publication));
    assert!(
        store
            .complete_burn_check_assessment(
                &input.durable,
                &serde_json::to_string(&publication).unwrap(),
                1000,
                POLICY.idle_secs
            )
            .unwrap()
    );
}

#[test]
fn persisted_native_scope_approval_withdraws_findings_and_saved_prompt_citations() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    assert_eq!(input.plan.prepared.groups.len(), 1);
    let result = input
        .check
        .reduce(&input.plan, &results(&input, false), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    let id = result.findings[0].id.clone();
    persist(&fixture.store, &input, result);
    assert!(
        saved_finding_citations(&fixture.store.lock(), &SourceFence::from(&candidate), &id)
            .unwrap()
            .is_some()
    );
    fixture.append(
        3,
        "user",
        json!({"type":"text","text":"I approve and accept the completed billing feature."}),
    );
    let current = fixture.publish();
    assert!(
        current_publication(&fixture.store.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .is_none()
    );
    assert!(
        saved_finding_citations(&fixture.store.lock(), &SourceFence::from(&current), &id)
            .unwrap()
            .is_none()
    );
    let latest = load_input(&fixture.store, &current, &capabilities).unwrap();
    assert_ne!(input.durable.input_revision, latest.durable.input_revision);
    assert!(
        latest.plan.work_items[0]
            .window
            .fields
            .to_string()
            .contains("I approve and accept")
    );
    let result = latest
        .check
        .reduce(&latest.plan, &results(&latest, true), true)
        .unwrap();
    assert!(result.findings.is_empty());
    persist(&fixture.store, &latest, result);
    assert!(
        current_publication(&fixture.store.lock(), &SourceFence::from(&current))
            .unwrap()
            .is_some()
    );
    assert!(
        saved_finding_citations(&fixture.store.lock(), &SourceFence::from(&current), &id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn selected_work_has_one_local_decision_and_resumable_four_target_turns() {
    let fixture = NativeFixture::paged();
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    assert_eq!(input.plan.prepared.groups.len(), 9);
    let all_results = results(&input, false);
    let mut sampling = new_sampling().unwrap();
    synchronize_sampling(&input, &mut sampling).unwrap();
    sampling.begin_run();
    let mut selected = BTreeSet::new();
    while let Some(job) = sampling.choose_job() {
        assert!(selected.insert(job.candidate));
        let plan =
            ScopeCreepCheck::select_candidates(&input.plan, &BTreeSet::from([job.candidate]));
        assert_eq!(plan.shared_context, input.plan.shared_context);
        assert_eq!(
            plan.prepared.scope_bindings,
            input.plan.prepared.scope_bindings
        );
        assert_eq!(plan.prepared.groups.len(), 1);
        for item in &plan.work_items {
            let initial = all_results
                .iter()
                .find(|result| result.work_item_id == item.id)
                .unwrap();
            let followup = input
                .check
                .reconcile(item, initial, input.check.context())
                .unwrap();
            assert!(followup.is_none());
            assert_eq!(item.questions.len(), 1);
        }
        let selected_results = all_results
            .iter()
            .filter(|result| {
                plan.work_items.iter().any(|item| {
                    result.work_item_id == item.id
                        || result.work_item_id == format!("{}::followup", item.id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let reduction = input.check.reduce(&plan, &selected_results, true).unwrap();
        record_completion(&mut sampling, &job, &reduction, &plan).unwrap();
    }
    assert_eq!(selected.len(), 4);
    let mut restored: SamplingProgress =
        serde_json::from_str(&serde_json::to_string(&sampling).unwrap()).unwrap();
    assert!(restored.choose_job().is_none());
    restored.begin_run();
    let mut next = BTreeSet::new();
    while let Some(job) = restored.choose_job() {
        next.insert(job.candidate);
    }
    assert_eq!(selected.union(&next).count(), 8);
}

#[test]
fn restart_config_switch_delete_and_clear_fence_publications() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    persist(
        &fixture.store,
        &input,
        input
            .check
            .reduce(&input.plan, &results(&input, false), true)
            .unwrap(),
    );
    let reopened = Store::open(fixture.directory.path()).unwrap();
    assert!(
        current_publication(&reopened.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .is_some()
    );
    reopened
        .set_internal_value_checked(
            "internal:smartChecksConnectionChangePendingV1",
            "new-provider",
        )
        .unwrap();
    assert!(
        current_publication(&reopened.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .is_none()
    );
    assert!(
        saved_finding_citations(&reopened.lock(), &SourceFence::from(&candidate), "old-id")
            .unwrap()
            .is_none()
    );
    reopened.delete_session(&candidate.session.key).unwrap();
    assert!(
        current_publication(&reopened.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .is_none()
    );
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    persist(
        &fixture.store,
        &input,
        input
            .check
            .reduce(&input.plan, &results(&input, false), true)
            .unwrap(),
    );
    fixture.store.clear_local_session_data().unwrap();
    assert!(
        current_publication(&fixture.store.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .is_none()
    );
}

#[test]
fn oversized_atomic_target_has_typed_limitation_and_no_dispatch_work() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.request_body_bytes.value = Some(1);
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    assert!(input.plan.prepared.session_limitation.is_none());
    assert_eq!(
        input.plan.prepared.groups[0].limitation.as_deref(),
        Some("work_context_too_large")
    );
    assert!(input.plan.work_items.is_empty());
    assert!(ScopeCreepCheck::sampling_candidates(&input.plan).is_empty());
    let result = input.check.reduce(&input.plan, &[], true).unwrap();
    assert!(!publication_has_clean_coverage(&result));
    let mut cursor = complete_cursor(&input, 7);
    cursor.blocked_fit_key = Some(input.fit_key.clone());
    cursor.result = Some(result);
    assert!(
        fixture
            .store
            .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        fixture
            .store
            .claim_burn_check_assessment(
                &input.durable,
                unix_now(),
                POLICY.lease_secs,
                POLICY.idle_secs
            )
            .unwrap()
    );
    assert!(
        save_failure(
            &fixture.store,
            &input,
            &cursor,
            "work_context_too_large",
            None
        )
        .unwrap()
    );
    let reopened = Store::open(fixture.directory.path()).unwrap();
    let saved = reopened
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    assert!(fit_is_blocked(Some(&saved), &input));
    reopened
        .set_internal_value_checked(
            "internal:smartChecksConnectionChangePendingV1",
            "credential-update",
        )
        .unwrap();
    let same_limits = load_input(&reopened, &candidate, &capabilities).unwrap();
    assert!(fit_is_blocked(Some(&saved), &same_limits));
    let changed_limits =
        load_input(&reopened, &candidate, &ModelCapabilities::jev_default()).unwrap();
    assert!(!fit_is_blocked(Some(&saved), &changed_limits));
}

#[test]
fn publication_gate_rejects_partial_clean_and_changed_revisions_or_work_bindings() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    let pending = input.check.reduce(&input.plan, &[], false).unwrap();
    assert!(!publication_has_clean_coverage(&pending));
    let result = input
        .check
        .reduce(&input.plan, &results(&input, false), true)
        .unwrap();
    let mut saved = publication(&input, result);
    assert!(valid_publication(&saved));
    saved.assessment.findings[0].work[0].digest = "changed".into();
    assert!(!valid_publication(&saved));
    saved.assessment.findings[0].work = saved.prepared.groups[0].work.clone();
    saved.assessment.revisions.questions -= 1;
    assert!(!valid_publication(&saved));
    assert_eq!(
        CHECK.evaluator_revision(),
        format!(
            "scope-creep-adapter-v{CURSOR_REVISION}:{}:{}:{}:{}",
            REVISIONS.projection, REVISIONS.chunking, REVISIONS.questions, REVISIONS.reducer
        )
    );
}

#[test]
fn persisted_cursor_resumes_a_bounded_run_and_provider_change_keeps_fair_order() {
    let fixture = NativeFixture::paged();
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    let mut cursor = complete_cursor(&input, 7);
    let mut sampling = new_sampling().unwrap();
    synchronize_sampling(&input, &mut sampling).unwrap();
    sampling.begin_run();
    cursor.active_job = sampling.choose_job();
    cursor.sampling = Some(sampling);
    cursor.run_started = true;
    assert!(
        fixture
            .store
            .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        fixture
            .store
            .claim_burn_check_assessment(
                &input.durable,
                unix_now(),
                POLICY.lease_secs,
                POLICY.idle_secs
            )
            .unwrap()
    );
    assert!(save_cursor(&fixture.store, &input.durable, &cursor).unwrap());
    let reopened = Store::open(fixture.directory.path()).unwrap();
    let saved = reopened
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = restore_cursor(Some(&saved), &input.durable, 7);
    assert_eq!(restored.active_job, cursor.active_job);
    let mut remaining = 0;
    while restored.sampling.as_mut().unwrap().choose_job().is_some() {
        remaining += 1;
    }
    assert_eq!(remaining, 3);
    let switched = restore_cursor(Some(&saved), &input.durable, 8);
    assert_eq!(switched.active_job, cursor.active_job);
    assert!(switched.result.is_none());
    assert_eq!(switched.sampling, cursor.sampling);
}

#[test]
fn mocked_authentication_rejection_preserves_scope_jobs_and_stops_dispatch() {
    use antiburn_local::analysis::jev::run_jev_check_prepared;
    use std::sync::atomic::{AtomicUsize, Ordering};
    for replace_generation in [false, true] {
        let fixture = NativeFixture::paged();
        let candidate = fixture.publish();
        let capabilities = ModelCapabilities::jev_default();
        let mut input = load_descriptor_input(&fixture.store, &candidate, &capabilities).unwrap();
        let handle = crate::jev::worker::WorkerHandle::default();
        handle
            .set_system_one_connection(
                crate::jev::config::SystemOneConnection::jev_default(),
                Some("synthetic-key".into()),
            )
            .unwrap();
        let (_, generation) = handle.execution_client().unwrap();
        let mut cursor = restore_cursor(None, &input.durable, generation);
        cursor.sampling = Some(new_sampling().unwrap());
        enumerate_scope_turn(&mut input, &mut cursor).unwrap();
        cursor.sampling.as_mut().unwrap().begin_run();
        let now = unix_now();
        fixture
            .store
            .queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)
            .unwrap();
        fixture
            .store
            .claim_burn_check_assessment(&input.durable, now, POLICY.lease_secs, POLICY.idle_secs)
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dispatches = AtomicUsize::new(0);
        let terminal_probes = AtomicUsize::new(0);
        let mut connection = handle.system_one_connection();
        connection
            .model_revision
            .clone_from(&capabilities.model_revision);
        while handle.is_available() {
            let job = cursor.sampling.as_mut().unwrap().choose_job().unwrap();
            cursor.active_job = Some(job.clone());
            let mut plan = input
                .check
                .prepare_descriptors(
                    &cursor.inventory,
                    &capabilities,
                    &BTreeSet::from([job.candidate]),
                )
                .unwrap();
            let outcome = runtime.block_on(async {
                run_jev_check_prepared(
                    input.check.as_ref(),
                    input.check.context(),
                    &mut plan,
                    JevRunProgress::default(),
                    admit_jev_orchestration().await.unwrap(),
                    |batch| {
                        dispatches.fetch_add(1, Ordering::SeqCst);
                        mark_authentication_rejected_batch(
                            &fixture.store,
                            &input.durable,
                            &connection,
                            &batch,
                        );
                        async { Err(JevError::AuthenticationRejected) }
                    },
                    |_| Ok(()),
                )
                .await
                .unwrap()
            });
            assert_eq!(outcome.failure, Some(JevError::AuthenticationRejected));
            assert_eq!(
                dispatch_readiness(
                    &fixture.store,
                    &handle,
                    &input.durable,
                    &capabilities,
                    &plan,
                    &outcome.progress
                )
                .unwrap(),
                crate::store::BurnCheckRequestAdmission::Exhausted
            );
            cursor.run_progress = outcome.progress;
            record_completion(
                cursor.sampling.as_mut().unwrap(),
                &job,
                &outcome.result,
                &plan,
            )
            .unwrap();
            merge_result(cursor.result.as_mut().unwrap(), outcome.result, &plan);
            update_result_counts(&mut cursor, &input);
            save_cursor(&fixture.store, &input.durable, &cursor).unwrap();
            save_scheduling(&fixture.store, &input, &cursor).unwrap();
            let error = outcome.failure.unwrap();
            if target_failure_is_terminal(&error, || {
                terminal_probes.fetch_add(1, Ordering::SeqCst);
                dispatch_readiness(
                    &fixture.store,
                    &handle,
                    &input.durable,
                    &capabilities,
                    &plan,
                    &cursor.run_progress,
                )
            })
            .unwrap()
            {
                cursor
                    .sampling
                    .as_mut()
                    .unwrap()
                    .terminate_candidate(&job)
                    .unwrap();
                cursor.active_job = None;
                continue;
            }
            if replace_generation {
                handle
                    .set_system_one_connection(
                        crate::jev::config::SystemOneConnection::jev_default(),
                        Some("replacement-key".into()),
                    )
                    .unwrap();
            }
            handle
                .with_current_generation(generation, || {
                    save_failure(
                        &fixture.store,
                        &input,
                        &cursor,
                        error_category(&error),
                        fixture
                            .store
                            .burn_check_next_attempt_at(&input.durable)
                            .unwrap(),
                    )
                })
                .transpose()
                .unwrap();
            assert_eq!(
                handle
                    .reject_authentication(&fixture.store, generation)
                    .unwrap(),
                !replace_generation
            );
            break;
        }
        assert_eq!(dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(terminal_probes.load(Ordering::SeqCst), 0);
        assert_eq!(handle.authentication_rejected(), !replace_generation);
        assert!(!handle.key_is_current(generation));
        assert_eq!(handle.is_available(), replace_generation);
        assert_eq!(
            fixture
                .store
                .internal_value("internal:typesafeAuthRejectedV1")
                .as_deref(),
            (!replace_generation).then_some("true")
        );
        let reopened = Store::open(fixture.directory.path()).unwrap();
        let saved = reopened
            .burn_check_assessment(&input.durable.key, CHECK_ID)
            .unwrap()
            .unwrap();
        let restored = restore_cursor(Some(&saved), &input.durable, generation + 1);
        assert_eq!(restored.active_job, cursor.active_job);
        assert_eq!(restored.run_progress, cursor.run_progress);
        assert_eq!(
            restored
                .sampling
                .as_ref()
                .unwrap()
                .coverage(ScopeCreepCheck::check_identity())
                .unwrap()
                .completed,
            0
        );
        assert_eq!(
            restored
                .sampling
                .as_ref()
                .unwrap()
                .runnable_count(ScopeCreepCheck::check_identity()),
            9
        );
        let category: Option<String> = reopened
            .lock()
            .query_row(
                "SELECT last_error_category FROM burn_check_assessment WHERE check_id = ?1",
                [CHECK_ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            category.as_deref(),
            (!replace_generation).then_some("authentication_rejected")
        );
    }
}

pub(crate) fn mark_authentication_rejected_batch(
    store: &Store,
    input: &BurnCheckInput,
    connection: &crate::jev::config::SystemOneConnection,
    batch: &antiburn_local::analysis::jev::JevRequestBatch,
) {
    for identity in crate::jev::worker::batch_request_identities(connection, input, batch) {
        store.lock().execute("INSERT INTO burn_check_dispatch_attempt
            (request_identity, environment_key, agent, session_id, attempts, terminal) VALUES (?1, ?2, ?3, ?4, 1, 1)",
            params![identity, input.key.environment_key, input.key.agent, input.key.session_id]).unwrap();
    }
}

#[test]
fn mocked_native_production_request_persists_one_decision_and_fit_failure_makes_zero_requests() {
    use antiburn_local::analysis::jev::{JevResponse, run_jev_check_prepared};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let count = AtomicUsize::new(0);
    let mut plan = input.plan.clone();
    let outcome = runtime.block_on(async {
        run_jev_check_prepared(
            input.check.as_ref(),
            input.check.context(),
            &mut plan,
            JevRunProgress::default(),
            admit_jev_orchestration().await.unwrap(),
            |batch| {
                assert_eq!(batch.request.questions.len(), 1);
                assert!(
                    batch
                        .request
                        .state
                        .to_string()
                        .contains("Do not change billing")
                );
                assert!(
                    batch
                        .request
                        .state
                        .to_string()
                        .contains("Explain the parser fix")
                );
                count.fetch_add(1, Ordering::SeqCst);
                let response = JevResponse {
                    model: batch.request.model.clone(),
                    usage: JevUsage {
                        input_tokens: 10,
                        output_tokens: 2,
                    },
                    answers: batch
                        .answer_owners
                        .iter()
                        .map(|(id, (_, key))| {
                            assert_eq!(key, "scope_decision");
                            (
                                id.clone(),
                                answer(ScopeQuestion::Decision, "likely_scope_expansion"),
                            )
                        })
                        .collect(),
                };
                async move { Ok(response) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap()
    });
    assert!(outcome.complete, "{:?}", outcome.failure);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(outcome.result.findings.len(), 1);
    let id = outcome.result.findings[0].id.clone();
    persist(&fixture.store, &input, outcome.result);
    let saved = current_publication(&fixture.store.lock(), &SourceFence::from(&candidate))
        .unwrap()
        .unwrap();
    assert_eq!(saved.assessment.findings[0].decision_probability, 1.0);
    assert_eq!(
        saved.assessment.decisions[0].outcome,
        Some(ScopeAnswer::LikelyScopeExpansion)
    );
    assert!(
        saved_finding_citations(&fixture.store.lock(), &SourceFence::from(&candidate), &id)
            .unwrap()
            .is_some()
    );
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.request_body_bytes.value = Some(1);
    let limited = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    let mut plan = limited.plan.clone();
    count.store(0, Ordering::SeqCst);
    let outcome = runtime.block_on(async {
        run_jev_check_prepared(
            limited.check.as_ref(),
            limited.check.context(),
            &mut plan,
            JevRunProgress::default(),
            admit_jev_orchestration().await.unwrap(),
            |_| {
                count.fetch_add(1, Ordering::SeqCst);
                async { Err(JevError::InvalidCheckPlan) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap()
    });
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(outcome.result.session_limitation.is_none());
    assert_eq!(outcome.result.coverage.skipped_items, 1);
}

#[test]
fn valid_uncertainty_is_reviewed_and_durable_without_clean_coverage() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    let mut answers = results(&input, false);
    answers[0].answers = BTreeMap::from([(
        "scope_decision".into(),
        answer(ScopeQuestion::Decision, "uncertain"),
    )]);
    let result = input.check.reduce(&input.plan, &answers, true).unwrap();
    assert_eq!(result.assessed_candidates, 1);
    assert_eq!(result.remaining_candidates, 0);
    assert!(!publication_has_clean_coverage(&result));
    assert!(valid_publication(&publication(&input, result.clone())));
    let mut sampling = new_sampling().unwrap();
    synchronize_sampling(&input, &mut sampling).unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    record_completion(&mut sampling, &job, &result, &input.plan).unwrap();
    let mut cursor = complete_cursor(&input, 7);
    cursor.sampling = Some(sampling);
    cursor.result = Some(result);
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(
            &input.durable,
            unix_now(),
            POLICY.lease_secs,
            POLICY.idle_secs,
        )
        .unwrap();
    assert!(save_failure(&fixture.store, &input, &cursor, "sampling_incomplete", None).unwrap());
    let reopened = Store::open(fixture.directory.path()).unwrap();
    let saved = reopened
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = restore_cursor(Some(&saved), &input.durable, 7);
    restored.sampling.as_mut().unwrap().begin_run();
    assert!(restored.sampling.as_mut().unwrap().choose_job().is_none());
    assert!(
        current_publication(&reopened.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .is_some()
    );
}

#[test]
fn failed_attempt_publication_uses_same_partial_decision_semantics() {
    let fixture = NativeFixture::new(0);
    fixture.append(2, "assistant", json!({"type":"tool","tool":"write","callID":"failed-billing", "state":{"status":"error", "input":{"filePath":"/synthetic/billing.rs","content":"Implement an independent billing API"},"error":"Permission denied; no file changed."}}));
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    assert_eq!(input.plan.work_items.len(), 1);
    let result = input
        .check
        .reduce(&input.plan, &results(&input, false), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert_eq!(
        result.findings[0].observation_kind,
        antiburn_local::checks::scope_creep::WorkObservationKind::Attempt
    );
    assert!(publishable_finding(
        &result.findings[0],
        &publication(&input, result.clone())
    ));
    let mut changed = publication(&input, result);
    changed.assessment.findings[0].decision_probability = 0.74;
    assert!(!valid_publication(&changed));
}

#[test]
fn accepted_positive_survives_a_failed_sibling_and_remains_non_clean_after_restart() {
    use antiburn_local::analysis::jev::{JevResponse, run_jev_check_prepared};
    let fixture = NativeFixture::new(2);
    let candidate = fixture.publish();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.questions_per_request.value = Some(1);
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    assert_eq!(input.plan.work_items.len(), 2);
    let mut plan = input.plan.clone();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let outcome = tokio::runtime::Runtime::new().unwrap().block_on(async {
        run_jev_check_prepared(
            input.check.as_ref(),
            input.check.context(),
            &mut plan,
            JevRunProgress::default(),
            admit_jev_orchestration().await.unwrap(),
            |batch| {
                let response = if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(JevResponse {
                        model: batch.request.model.clone(),
                        usage: JevUsage {
                            input_tokens: 10,
                            output_tokens: 2,
                        },
                        answers: batch
                            .answer_owners
                            .keys()
                            .map(|id| {
                                (
                                    id.clone(),
                                    answer(ScopeQuestion::Decision, "likely_scope_expansion"),
                                )
                            })
                            .collect(),
                    })
                } else {
                    Err(JevError::InvalidCheckPlan)
                };
                async move { response }
            },
            |_| Ok(()),
        )
        .await
        .unwrap()
    });
    assert!(outcome.failure.is_some());
    assert!(!outcome.complete);
    assert_eq!(outcome.result.findings.len(), 1);
    let id = outcome.result.findings[0].id.clone();
    let mut sampling = new_sampling().unwrap();
    synchronize_sampling(&input, &mut sampling).unwrap();
    sampling.begin_run();
    while let Some(job) = sampling.choose_job() {
        record_completion(&mut sampling, &job, &outcome.result, &input.plan).unwrap();
    }
    let mut cursor = complete_cursor(&input, 7);
    cursor.sampling = Some(sampling);
    cursor.result = Some(input.check.reduce(&input.plan, &[], false).unwrap());
    cursor.run_progress = outcome.progress;
    merge_result(cursor.result.as_mut().unwrap(), outcome.result, &plan);
    for result in cursor.run_progress.results.values() {
        cursor
            .accepted_request_usage
            .insert(result.request_id.clone(), result.usage);
    }
    update_result_counts(&mut cursor, &input);
    assert_eq!(cursor.result.as_ref().unwrap().assessed_candidates, 1);
    assert_eq!(cursor.result.as_ref().unwrap().remaining_candidates, 1);
    assert!(!publication_has_clean_coverage(
        cursor.result.as_ref().unwrap()
    ));
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(
            &input.durable,
            unix_now(),
            POLICY.lease_secs,
            POLICY.idle_secs,
        )
        .unwrap();
    assert!(save_failure(&fixture.store, &input, &cursor, "provider_error", None).unwrap());
    let reopened = Store::open(fixture.directory.path()).unwrap();
    let saved = current_publication(&reopened.lock(), &SourceFence::from(&candidate))
        .unwrap()
        .unwrap();
    assert_eq!(saved.assessment.findings.len(), 1);
    assert!(!publication_has_clean_coverage(&saved.assessment));
    assert!(
        saved_finding_citations(&reopened.lock(), &SourceFence::from(&candidate), &id)
            .unwrap()
            .is_some()
    );
    cursor.sampling.as_mut().unwrap().begin_run();
    let blocked = cursor.sampling.as_mut().unwrap().choose_job().unwrap();
    mark_terminal_scope_decision(&mut cursor, &blocked, "work_context_too_large");
    cursor
        .sampling
        .as_mut()
        .unwrap()
        .terminate_candidate(&blocked)
        .unwrap();
    update_result_counts(&mut cursor, &input);
    let result = cursor.result.as_ref().unwrap();
    assert_eq!(result.assessed_candidates, 1);
    assert_eq!(result.remaining_candidates, 0);
    assert_eq!(result.coverage.skipped_items, 1);
    assert_eq!(result.coverage.not_selected_items, 0);
    assert!(valid_publication(&publication(&input, result.clone())));
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(
            &input.durable,
            unix_now(),
            POLICY.lease_secs,
            POLICY.idle_secs,
        )
        .unwrap();
    assert!(save_failure(&fixture.store, &input, &cursor, "sampling_incomplete", None).unwrap());
    let saved = current_publication(&reopened.lock(), &SourceFence::from(&candidate))
        .unwrap()
        .unwrap();
    assert_eq!(saved.assessment.findings.len(), 1);
    assert!(!publication_has_clean_coverage(&saved.assessment));
    assert!(
        saved_finding_citations(&reopened.lock(), &SourceFence::from(&candidate), &id)
            .unwrap()
            .is_some()
    );
}
#[test]
fn scope_explanation_requires_exact_prepared_refs_digests_ranges_and_authority() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    let result = input
        .check
        .reduce(&input.plan, &results(&input, false), true)
        .unwrap();
    let publication = publication(&input, result);
    let finding = publication
        .assessment
        .findings
        .first()
        .expect("source-bound positive reaches publication");
    assert!(publishable_finding(finding, &publication));
    assert!(
        !finding
            .explanation_basis
            .as_ref()
            .unwrap()
            .excerpts
            .is_empty()
    );
    for mutation in 0..12 {
        let mut changed = finding.clone();
        let basis = changed.explanation_basis.as_mut().unwrap();
        let excerpt = &mut basis.excerpts[0];
        match mutation {
            0 => excerpt.source.id.push_str("-wrong"),
            1 => excerpt.source.turn_index += 1,
            2 => excerpt.content_digest.push_str("-wrong"),
            3 => excerpt.start_byte += 1,
            4 => excerpt.end_byte += 1,
            5 => excerpt.authority.push_str("-wrong"),
            6 => excerpt.text.push_str("wrong"),
            7 => excerpt.range_source.push_str("-wrong"),
            8 => excerpt.source.source_key_digest.push_str("-wrong"),
            9 => excerpt.scope_field = Some(antiburn_local::analysis::jev::JevInputField::UserAnswer),
            10 => excerpt.native_source = Some(antiburn_local::analysis::jev_evidence::JevScopeEvidenceSource {
                source_format: antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2,
                role: antiburn_local::analysis::jev_evidence::JevScopeEvidenceRole::User,
                native_record_id: Some("wrong-record".into()), call_id: None, question_id: None,
                order: 0, acceptance_order: None,
                provenance: antiburn_local::analysis::jev_evidence::JevScopeEvidenceProvenance::RecordedUser,
                producer_revision: "wrong-producer".into(), normalization_revision: 1,
                bindings: Vec::new(), truncated: false,
            }),
            11 => excerpt.source.native_record_id = Some("wrong-record".into()),
            _ => unreachable!(),
        }
        assert!(
            !publishable_finding(&changed, &publication),
            "mutation {mutation}"
        );
    }
}

#[test]
fn unchanged_request_fit_failures_are_terminal_before_dispatch_readiness() {
    for error in [
        JevError::RequestTooLarge {
            bytes: 4096,
            maximum: 1024,
        },
        JevError::RequestTokenLimitExceeded {
            tokens: 4096,
            maximum: 2050,
        },
    ] {
        assert!(
            target_failure_is_terminal(&error, || panic!(
                "fit failure must not retry unchanged input"
            ))
            .unwrap()
        );
    }
}
