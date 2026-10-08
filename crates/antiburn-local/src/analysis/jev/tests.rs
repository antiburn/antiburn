use super::tests::{check_revisions, choice, response_for};
use super::*;
use std::sync::atomic::Ordering;

struct ResumeCheck {
    revisions: JevCheckRevisions,
}

impl JevCheck for ResumeCheck {
    type Prepared = Value;
    type Result = Value;

    fn id(&self) -> &'static str {
        "resume_test"
    }

    fn revisions(&self) -> JevCheckRevisions {
        self.revisions
    }

    fn prepare(
        &self,
        context: &JevSessionContext,
    ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
        let questions = BTreeMap::from([("decision".to_owned(), choice())]);
        let work_items = ["first", "second"]
            .into_iter()
            .map(|id| JevWorkItem {
                id: id.to_owned(),
                window: JevInputWindow {
                    fields: json!({"text": "x".repeat(20_000)}),
                    evidence: vec![JevEvidenceReference {
                        part_id: "text".to_owned(),
                        source_id: format!("event-{id}"),
                        content_kind: "assistant_text".to_owned(),
                        role: JevEvidenceRole::Candidate,
                    }],
                },
                questions: questions.clone(),
            })
            .collect::<Vec<_>>();
        Ok(JevCheckPlan {
            check_id: self.id().to_owned(),
            input_revision: context.input_revision.clone(),
            revisions: self.revisions(),
            work_items,
            skipped_item_ids: Vec::new(),
            coverage: JevCoverage {
                selected_items: 2,
                ..JevCoverage::default()
            },
            capabilities: capabilities::ModelCapabilities::jev_default(),
            shared_context: None,
            prepared: Value::Null,
        })
    }

    fn reduce(
        &self,
        plan: &JevCheckPlan<Self::Prepared>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError> {
        if plan.check_id != self.id() {
            return Err(JevError::InvalidCheckPlan);
        }
        Ok(json!({
            "completed": complete,
            "work_item_ids": results.iter().map(|result| result.work_item_id.as_str()).collect::<Vec<_>>(),
        }))
    }
}

pub(super) struct ManyItemsCheck;

impl JevCheck for ManyItemsCheck {
    type Prepared = Value;
    type Result = usize;

    fn id(&self) -> &'static str {
        "many_items_test"
    }

    fn revisions(&self) -> JevCheckRevisions {
        JevCheckRevisions {
            projection: 1,
            chunking: 1,
            questions: 1,
            reducer: 1,
        }
    }

    fn prepare(
        &self,
        context: &JevSessionContext,
    ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
        let questions = BTreeMap::from([("decision".to_owned(), choice())]);
        let work_items = (0..70)
            .map(|index| JevWorkItem {
                id: format!("item-{index}"),
                window: JevInputWindow {
                    fields: json!({"text": "synthetic evidence ".repeat(280)}),
                    evidence: vec![JevEvidenceReference {
                        part_id: "text".to_owned(),
                        source_id: format!("event-{index}"),
                        content_kind: "assistant_text".to_owned(),
                        role: JevEvidenceRole::Candidate,
                    }],
                },
                questions: questions.clone(),
            })
            .collect::<Vec<_>>();
        Ok(JevCheckPlan {
            check_id: self.id().to_owned(),
            input_revision: context.input_revision.clone(),
            revisions: self.revisions(),
            work_items,
            skipped_item_ids: Vec::new(),
            coverage: JevCoverage::default(),
            capabilities: capabilities::ModelCapabilities::jev_default(),
            shared_context: None,
            prepared: Value::Null,
        })
    }

    fn reduce(
        &self,
        _plan: &JevCheckPlan<Self::Prepared>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError> {
        if !complete {
            return Err(JevError::InvalidCheckPlan);
        }
        Ok(results.len())
    }
}

struct OutputEnabledCheck;

impl JevCheck for OutputEnabledCheck {
    type Prepared = Value;
    type Result = usize;

    fn id(&self) -> &'static str {
        "output_enabled_test"
    }

    fn revisions(&self) -> JevCheckRevisions {
        check_revisions(1, 1, 1, 1)
    }

    fn input_selection(&self) -> JevInputSelection {
        JevInputSelection::from_fields(&[JevInputField::BashCommandOutput])
    }

    fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan<Value>, JevError> {
        let evidence = JevEvidenceReference {
            part_id: "tool_output".to_owned(),
            source_id: "local-event-1".to_owned(),
            content_kind: "tool_result".to_owned(),
            role: JevEvidenceRole::Candidate,
        };
        let output = self
            .retrieve_evidence(context, &evidence, JevInputField::BashCommandOutput)?
            .ok_or(JevError::InvalidCheckContext)?;
        Ok(JevCheckPlan {
            check_id: self.id().to_owned(),
            input_revision: context.input_revision.clone(),
            revisions: self.revisions(),
            work_items: vec![JevWorkItem {
                id: "output-item".to_owned(),
                window: JevInputWindow {
                    fields: json!({"tool_output": output}),
                    evidence: vec![evidence],
                },
                questions: BTreeMap::from([("decision".to_owned(), choice())]),
            }],
            skipped_item_ids: Vec::new(),
            coverage: JevCoverage {
                selected_items: 1,
                ..JevCoverage::default()
            },
            capabilities: capabilities::ModelCapabilities::jev_default(),
            shared_context: None,
            prepared: Value::Null,
        })
    }

    fn reduce(
        &self,
        _plan: &JevCheckPlan<Value>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError> {
        if !complete {
            return Err(JevError::InvalidCheckPlan);
        }
        Ok(results.len())
    }
}

struct EmptyCountingCheck {
    prepare_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl JevCheck for EmptyCountingCheck {
    type Prepared = Value;
    type Result = usize;

    fn id(&self) -> &'static str {
        "empty_counting_test"
    }

    fn revisions(&self) -> JevCheckRevisions {
        check_revisions(1, 1, 1, 1)
    }

    fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan<Value>, JevError> {
        self.prepare_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(JevCheckPlan {
            check_id: self.id().to_owned(),
            input_revision: context.input_revision.clone(),
            revisions: self.revisions(),
            work_items: Vec::new(),
            skipped_item_ids: Vec::new(),
            coverage: JevCoverage::default(),
            capabilities: capabilities::ModelCapabilities::jev_default(),
            shared_context: None,
            prepared: Value::Null,
        })
    }

    fn reduce(
        &self,
        _plan: &JevCheckPlan<Value>,
        results: &[JevWorkItemResult],
        _complete: bool,
    ) -> Result<usize, JevError> {
        Ok(results.len())
    }
}

#[tokio::test]
async fn prepared_runner_uses_the_admitted_plan_without_preparing_again() {
    let check = EmptyCountingCheck {
        prepare_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let context = JevSessionContext {
        input_revision: "prepared-input".to_owned(),
        session_identity: "prepared-session".to_owned(),
        check_context: Value::Null,
        limitations: Vec::new(),
        evidence_store: JevEvidenceStore::default(),
        reference_snapshots: Vec::new(),
    };
    let orchestration = admit_jev_orchestration().await.unwrap();
    let mut plan = check.prepare(&context).unwrap();
    let calls = check.prepare_calls.clone();
    let result = run_jev_check_prepared(
        &check,
        &context,
        &mut plan,
        JevRunProgress::default(),
        orchestration,
        |_| async { unreachable!("empty prepared plan must not dispatch") },
        |_| Ok(()),
    )
    .await
    .unwrap();

    assert!(result.complete);
    assert_eq!(result.result, 0);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn generic_runner_resumes_partial_progress_without_repeating_completed_batches() {
    let context = JevSessionContext {
        input_revision: "immutable-input".to_owned(),
        session_identity: "synthetic-session".to_owned(),
        check_context: Value::Null,
        limitations: Vec::new(),
        evidence_store: JevEvidenceStore::default(),
        reference_snapshots: Vec::new(),
    };
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let mut saved = Vec::new();
    let check = ResumeCheck {
        revisions: check_revisions(1, 1, 1, 1),
    };
    let first = run_jev_check(
        &check,
        &context,
        JevRunProgress::default(),
        |batch| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if call == 2 {
                    Err(JevError::ProviderUnavailable)
                } else {
                    Ok(response_for(&batch.request))
                }
            }
        },
        |progress| {
            saved.push(progress.clone());
            Ok(())
        },
    )
    .await
    .unwrap();
    assert!(!first.complete);
    assert_eq!(first.failure, None);
    assert_eq!(first.progress.results.len(), 1);
    assert_eq!(
        first
            .progress
            .results
            .values()
            .map(|result| &result.request_id)
            .collect::<BTreeSet<_>>()
            .len(),
        1
    );
    assert_eq!(saved.len(), 2);

    let resumed_calls = std::sync::atomic::AtomicUsize::new(0);
    let resumed = run_jev_check(
        &check,
        &context,
        first.progress,
        |batch| {
            resumed_calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(response_for(&batch.request)) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(resumed.complete);
    assert_eq!(resumed_calls.load(Ordering::SeqCst), 1);
    assert_eq!(resumed.progress.results.len(), 2);
    assert_eq!(resumed.progress.request_count, 2);
    assert_eq!(
        resumed.progress.results["first"].evidence[0].source_id,
        "event-first"
    );
    assert_eq!(resumed.result["completed"], true);
}

#[tokio::test]
async fn check_contract_revisions_invalidate_saved_work() {
    let context = JevSessionContext {
        input_revision: "immutable-input".to_owned(),
        session_identity: "synthetic-session".to_owned(),
        check_context: Value::Null,
        limitations: Vec::new(),
        evidence_store: JevEvidenceStore::default(),
        reference_snapshots: Vec::new(),
    };
    let original_check = ResumeCheck {
        revisions: check_revisions(1, 1, 1, 1),
    };
    let original = run_jev_check(
        &original_check,
        &context,
        JevRunProgress::default(),
        |batch| async move { Ok(response_for(&batch.request)) },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(original.complete);
    assert_eq!(original.progress.results.len(), 2);
    assert_ne!(original.progress.input_revision, context.input_revision);

    for revisions in [
        check_revisions(2, 1, 1, 1),
        check_revisions(1, 2, 1, 1),
        check_revisions(1, 1, 2, 1),
        check_revisions(1, 1, 1, 2),
    ] {
        let changed_check = ResumeCheck { revisions };
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let changed = run_jev_check(
            &changed_check,
            &context,
            original.progress.clone(),
            |batch| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(response_for(&batch.request)) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(changed.complete);
        assert_eq!(changed.progress.results.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    let mut changed_reference = context.clone();
    changed_reference
        .reference_snapshots
        .push(JevReferenceSnapshot {
            kind: "policy".to_owned(),
            identity: "policy-file".to_owned(),
            revision: "revision-2".to_owned(),
            fields: json!({"rule": "changed policy"}),
        });
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let changed = run_jev_check(
        &original_check,
        &changed_reference,
        original.progress.clone(),
        |batch| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(response_for(&batch.request)) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(changed.complete);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn runner_rejects_saved_answers_with_changed_source_bindings() {
    let context = JevSessionContext {
        input_revision: "immutable-input".to_owned(),
        session_identity: "synthetic-session".to_owned(),
        check_context: Value::Null,
        limitations: Vec::new(),
        evidence_store: JevEvidenceStore::default(),
        reference_snapshots: Vec::new(),
    };
    let check = ResumeCheck {
        revisions: check_revisions(1, 1, 1, 1),
    };
    let original = run_jev_check(
        &check,
        &context,
        JevRunProgress::default(),
        |batch| async move { Ok(response_for(&batch.request)) },
        |_| Ok(()),
    )
    .await
    .unwrap();
    let mut stale = original.progress;
    stale.results.get_mut("first").unwrap().evidence[0].source_id = "other-source".to_owned();

    let calls = std::sync::atomic::AtomicUsize::new(0);
    let resumed = run_jev_check(
        &check,
        &context,
        stale,
        |batch| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(response_for(&batch.request)) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(resumed.complete);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        resumed.progress.results["first"].evidence[0].source_id,
        "event-first"
    );
}

#[tokio::test]
async fn generic_runner_packs_many_items_into_bounded_parallel_requests() {
    let context = JevSessionContext {
        input_revision: "many-items-input".to_owned(),
        session_identity: "synthetic-session".to_owned(),
        check_context: Value::Null,
        limitations: Vec::new(),
        evidence_store: JevEvidenceStore::default(),
        reference_snapshots: Vec::new(),
    };
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let maximum_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let outcome = run_jev_check(
        &ManyItemsCheck,
        &context,
        JevRunProgress::default(),
        |batch| {
            calls.fetch_add(1, Ordering::SeqCst);
            let in_flight = std::sync::Arc::clone(&in_flight);
            let maximum_in_flight = std::sync::Arc::clone(&maximum_in_flight);
            async move {
                let active = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                maximum_in_flight.fetch_max(active, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(response_for(&batch.request))
            }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();

    assert!(outcome.complete);
    let call_count = calls.load(Ordering::SeqCst);
    assert!(call_count > 1);
    assert!(call_count < 64, "larger requests reduce provider calls");
    assert!(maximum_in_flight.load(Ordering::SeqCst) > 1);
    assert!(maximum_in_flight.load(Ordering::SeqCst) <= MAX_PARALLEL_REQUESTS);
    assert_eq!(outcome.progress.request_count, call_count);
    assert_eq!(outcome.result, 70);
}

#[tokio::test]
async fn second_check_selects_output_fields_through_the_shared_runner() {
    let mut context = JevSessionContext {
        input_revision: "output-enabled-input".to_owned(),
        session_identity: "output-enabled-session".to_owned(),
        check_context: Value::Null,
        limitations: Vec::new(),
        evidence_store: JevEvidenceStore::for_publication(7),
        reference_snapshots: Vec::new(),
    };
    context
        .evidence_store
        .insert(
            "local-event-1",
            JevInputField::BashCommandOutput,
            "OUTPUT_ENABLED_SENTINEL".to_owned(),
        )
        .unwrap();
    let requests = std::sync::atomic::AtomicUsize::new(0);
    let outcome = run_jev_check(
        &OutputEnabledCheck,
        &context,
        JevRunProgress::default(),
        |batch| {
            requests.fetch_add(1, Ordering::SeqCst);
            assert!(
                batch
                    .request
                    .state
                    .to_string()
                    .contains("OUTPUT_ENABLED_SENTINEL")
            );
            async move { Ok(response_for(&batch.request)) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();

    assert_eq!(
        OutputEnabledCheck.input_selection().bits(),
        1 << JevInputField::BashCommandOutput as u8
    );
    assert!(outcome.complete);
    assert_eq!(outcome.result, 1);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert_eq!(outcome.progress.request_count, 1);
    assert_eq!(context.evidence_store.publication_fence(), Some(7));
}
