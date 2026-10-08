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
        let fixture = Self::new(0);
        for index in 0..9 {
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
        let candidate = self.publish();
        let input = load_input(&self.store, &candidate, &ModelCapabilities::jev_default()).unwrap();
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
    sampling
        .synchronize(
            ScopeCreepCheck::check_identity(),
            input.plan.prepared.semantic_epoch,
            &ScopeCreepCheck::sampling_candidates(&input.plan),
        )
        .unwrap();
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
        record_completion(&mut sampling, &job, &reduction).unwrap();
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
    let mut cursor = restore_cursor(None, &input.durable, 7);
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
        "scope-creep-adapter-v2:8:4:24:11"
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
    let mut cursor = restore_cursor(None, &input.durable, 7);
    let mut sampling = new_sampling().unwrap();
    sampling
        .synchronize(
            ScopeCreepCheck::check_identity(),
            input.plan.prepared.semantic_epoch,
            &ScopeCreepCheck::sampling_candidates(&input.plan),
        )
        .unwrap();
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
    assert!(switched.active_job.is_none());
    assert!(switched.result.is_none());
    assert_eq!(switched.sampling, cursor.sampling);
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
            &input.check,
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
            &limited.check,
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
    sampling
        .synchronize(
            ScopeCreepCheck::check_identity(),
            input.plan.prepared.semantic_epoch,
            &ScopeCreepCheck::sampling_candidates(&input.plan),
        )
        .unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    record_completion(&mut sampling, &job, &result).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 7);
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
            &input.check,
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
    sampling
        .synchronize(
            ScopeCreepCheck::check_identity(),
            input.plan.prepared.semantic_epoch,
            &ScopeCreepCheck::sampling_candidates(&input.plan),
        )
        .unwrap();
    sampling.begin_run();
    while let Some(job) = sampling.choose_job() {
        record_completion(&mut sampling, &job, &outcome.result).unwrap();
    }
    let mut cursor = restore_cursor(None, &input.durable, 7);
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
}
