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
    pub(crate) fn new(groups: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("opencode.db");
        let source = rusqlite::Connection::open(&source_path).unwrap();
        source.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
            INSERT INTO session VALUES ('scope', NULL, 1000, 9000);").unwrap();
        let store = Store::open(directory.path()).unwrap();
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
    if let Some((positive, _)) = question.noul_answers() {
        return JevAnswer::Noul {
            noul: if key == positive.key() { 1.0 } else { 0.0 },
        };
    }
    if question == ScopeQuestion::Materiality {
        let antiburn_local::analysis::jev::JevQuestion::Score { criteria, .. } =
            question.question()
        else {
            panic!("expected score");
        };
        return JevAnswer::Score {
            score: 1.0,
            legend: criteria
                .into_iter()
                .enumerate()
                .map(|(index, criterion)| {
                    (index.to_string(), criterion.as_str().unwrap().to_owned())
                })
                .collect(),
            confidence: 1.0,
            probabilities: BTreeMap::from([("0".into(), 0.0), ("1".into(), 1.0)]),
        };
    }
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
        .flat_map(|item| {
            let mut evidence = input.plan.shared_context.as_ref().unwrap().evidence.clone();
            evidence.extend(item.window.evidence.clone());
            [true, false].map(|initial| JevWorkItemResult {
                request_id: format!("{}:{initial}", item.id),
                work_item_id: if initial {
                    item.id.clone()
                } else {
                    format!("{}::followup", item.id)
                },
                model: input.plan.capabilities.model.clone(),
                evidence: evidence.clone(),
                usage: JevUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                },
                answers: ScopeQuestion::ALL
                    .into_iter()
                    .filter(|question| (*question == ScopeQuestion::Performed) == initial)
                    .flat_map(|question| {
                        question.answer_keys().iter().map(move |key| {
                            (
                                (*key).into(),
                                answer(
                                    question,
                                    if acceptance && question == ScopeQuestion::LaterAcceptance {
                                        "accepted"
                                    } else {
                                        question.positive()
                                    },
                                ),
                            )
                        })
                    })
                    .collect(),
            })
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
    let shared = latest.plan.shared_context.as_ref().unwrap();
    assert!(shared.fields.to_string().contains("I approve and accept"));
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
fn every_selected_and_followup_request_retains_full_latest_scope() {
    let fixture = NativeFixture::new(10);
    let candidate = fixture.publish();
    let input = load_input(
        &fixture.store,
        &candidate,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
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
            let initial = results(&input, false)
                .into_iter()
                .find(|result| result.work_item_id == item.id)
                .unwrap();
            let followup = input
                .check
                .reconcile(item, &initial, input.check.context())
                .unwrap()
                .unwrap();
            assert_eq!(followup.window, item.window);
        }
        let selected_results = results(&input, false)
            .into_iter()
            .filter(|result| {
                plan.work_items.iter().any(|item| {
                    result.work_item_id == item.id
                        || result.work_item_id == format!("{}::followup", item.id)
                })
            })
            .collect::<Vec<_>>();
        let reduction = input.check.reduce(&plan, &selected_results, true).unwrap();
        record_completion(&mut sampling, &job, &reduction).unwrap();
    }
    assert_eq!(selected.len(), 8);
    let mut restored: SamplingProgress =
        serde_json::from_str(&serde_json::to_string(&sampling).unwrap()).unwrap();
    assert!(restored.choose_job().is_none());
    restored.begin_run();
    let mut next = BTreeSet::new();
    while let Some(job) = restored.choose_job() {
        next.insert(job.candidate);
    }
    assert_eq!(selected.union(&next).count(), 10);
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
fn oversized_full_scope_has_typed_limitation_and_no_dispatch_work() {
    let fixture = NativeFixture::new(1);
    let candidate = fixture.publish();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.request_body_bytes.value = Some(1);
    let input = load_input(&fixture.store, &candidate, &capabilities).unwrap();
    assert_eq!(
        input.plan.prepared.session_limitation,
        Some(SessionScopeError::ScopeTooLarge)
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
            "scope_context_too_large",
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
        "scope-creep-adapter-v1:7:2:22:10"
    );
}

#[test]
fn persisted_cursor_resumes_a_bounded_run_and_provider_change_keeps_fair_order() {
    let fixture = NativeFixture::new(10);
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
    assert_eq!(remaining, 7);
    let switched = restore_cursor(Some(&saved), &input.durable, 8);
    assert!(switched.active_job.is_none());
    assert!(switched.result.is_none());
    assert_eq!(switched.sampling, cursor.sampling);
}

#[test]
fn actual_native_requests_repeat_full_scope_and_fit_failure_makes_zero_requests() {
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
                assert_eq!(
                    batch.request.state["shared_context"],
                    input.plan.shared_context.as_ref().unwrap().fields
                );
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
                            let question = ScopeQuestion::ALL
                                .into_iter()
                                .find(|question| question.answer_keys().contains(&key.as_str()))
                                .unwrap();
                            (id.clone(), answer(question, question.positive()))
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
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(outcome.result.findings.len(), 1);
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
    assert_eq!(
        outcome.result.session_limitation,
        Some(SessionScopeError::ScopeTooLarge)
    );
}
