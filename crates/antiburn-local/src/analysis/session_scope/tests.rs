use super::*;
use crate::analysis::jev::{JevInputWindow, JevQuestion};
use crate::analysis::jev_evidence::{
    non_authorizing_context_digest, scope_answer_fixture, scope_plan_fixture,
    JevNativeFieldContainer, JevNativeFieldRange, JevNonAuthorizingContextKind,
    JevNonAuthorizingContextProducer, JevNonAuthorizingContextProof, JevSelectedSkillNormalization,
    JevSelectedSkillProducer, JevSelectedSkillProof, JevSelectedSkillStatus,
};
use crate::analysis::{ContentAuthority, ContentPart, ContentQueryCoverage, PublishedContentPart};

fn part(index: u64, kind: ContentKind, text: &str) -> PublishedContentPart {
    PublishedContentPart {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: index,
        role: if kind == ContentKind::UserText {
            "user"
        } else {
            "assistant"
        },
        scope: "main".into(),
        ts_ms: Some(index as i64),
        uuid: Some(format!("event-{index}")),
        message_id: None,
        part_index: 0,
        part: ContentPart::new(kind, text),
        context_only: false,
        stable_event_identity: true,
    }
}

fn page(parts: Vec<PublishedContentPart>, more: bool) -> PublishedContent {
    PublishedContent {
        publication_fence: 4,
        source_generation: Some(3),
        parts,
        coverage: ContentQueryCoverage {
            more_parts: more,
            parts_capped: more,
            ..Default::default()
        },
        next_offset: 0,
    }
}

fn builder(boundary: u64) -> SessionScopeBuilder {
    builder_for_source(boundary, SourceFormat::ClaudeJsonl)
}

fn builder_for_source(boundary: u64, format: SourceFormat) -> SessionScopeBuilder {
    SessionScopeBuilder::new(
        format,
        SessionScopeBoundary {
            source_key: "transcript".into(),
            thread_id: "branch".into(),
            turn_index: boundary,
            part_index: 0,
            branch: SessionScopeBranch::ProvenLinear,
        },
        4,
        3,
        true,
    )
    .unwrap()
}

fn generated_context(skill: bool) -> PublishedContentPart {
    let text = if skill {
        "<skill>\n<name>verify</name>\n<path>/synthetic/verify/SKILL.md</path>\nCheck the sample.\n</skill>"
    } else {
        "Recorded non-authorizing environment context."
    };
    let mut record = part(1, ContentKind::UserText, text);
    record.part.authority = ContentAuthority::Unknown;
    let range = JevNativeFieldRange {
        native_record_id: record.uuid.clone(),
        field: JevInputField::UserMessage,
        container: JevNativeFieldContainer::Record,
        pointer: "/payload/content/0/text".into(),
        start: 0,
        end: text.len(),
    };
    record.part.metadata.bindings.push(range.clone());
    if skill {
        record.part.metadata.selected_skill = Some(JevSelectedSkillProof {
            source_format: SourceFormat::CodexRolloutJsonl,
            session_id: "native-session".into(),
            message_id: "event-1".into(),
            name: "verify".into(),
            location: "/synthetic/verify/SKILL.md".into(),
            producer: JevSelectedSkillProducer::CodexSelectedSkillInstructions,
            normalization: JevSelectedSkillNormalization::CodexSkillDocument,
            normalization_revision: 1,
            ranges: vec![range],
            status: JevSelectedSkillStatus::DocumentSelected,
            complete: true,
        });
    } else {
        record.part.metadata.non_authorizing_context_proof = Some(JevNonAuthorizingContextProof {
            kind: JevNonAuthorizingContextKind::Environment,
            producer: JevNonAuthorizingContextProducer::CodexEnvironmentContext01601,
            source_format: SourceFormat::CodexRolloutJsonl,
            session_id: "native-session".into(),
            message_id: "event-1".into(),
            range,
            text_digest: non_authorizing_context_digest(text),
            complete: true,
            normalization_revision: 1,
        });
    }
    record
}

#[test]
fn qualified_generated_context_is_retained_as_an_action_without_scope_authority() {
    for skill in [false, true] {
        let mut scope = builder_for_source(1, SourceFormat::CodexRolloutJsonl);
        scope
            .push_page(
                page(
                    vec![
                        part(0, ContentKind::UserText, "Keep billing unchanged."),
                        generated_context(skill),
                    ],
                    false,
                ),
                false,
            )
            .unwrap();
        assert_eq!(scope.actions.len(), 2);
        assert_eq!(scope.actions[1].authority, "unknown");
        let snapshot = scope.finish().unwrap();
        assert_eq!(snapshot.values(), &[json!("Keep billing unchanged.")]);
        assert_eq!(snapshot.occurrences().len(), 1);
        assert_eq!(snapshot.occurrences()[0].authority, ScopeAuthority::User);
        assert!(snapshot.scope_creep_context().is_ok());
    }
}

#[test]
fn invalid_generated_context_proof_fails_closed_and_stays_failed() {
    for skill in [false, true] {
        for mutation in 0..5 {
            let mut record = generated_context(skill);
            if let Some(proof) = record.part.metadata.selected_skill.as_mut() {
                match mutation {
                    0 => proof.source_format = SourceFormat::PiV3Jsonl,
                    1 => proof.message_id = "another-record".into(),
                    2 => proof.complete = false,
                    3 => proof.ranges[0].end -= 1,
                    _ => record.part.metadata.selected_skill = None,
                }
            } else if let Some(proof) = record.part.metadata.non_authorizing_context_proof.as_mut()
            {
                match mutation {
                    0 => proof.source_format = SourceFormat::PiV3Jsonl,
                    1 => proof.message_id = "another-record".into(),
                    2 => proof.complete = false,
                    3 => proof.range.end -= 1,
                    _ => record.part.metadata.non_authorizing_context_proof = None,
                }
            }
            let mut scope = builder_for_source(1, SourceFormat::CodexRolloutJsonl);
            let expected = SessionScopeError::Missing(ScopeMissingReason::InvalidEvidence);
            assert_eq!(
                scope.push_page(
                    page(
                        vec![
                            part(0, ContentKind::UserText, "Keep billing unchanged."),
                            record
                        ],
                        false
                    ),
                    false
                ),
                Err(expected.clone()),
                "skill={skill}, mutation={mutation}"
            );
            assert_eq!(scope.finish(), Err(expected));
        }
    }
}

#[test]
fn generated_context_cannot_bypass_raw_truncation_or_replay_action_identity() {
    for skill in [false, true] {
        let mut record = generated_context(skill);
        record.part.truncated = true;
        let mut scope = builder_for_source(1, SourceFormat::CodexRolloutJsonl);
        assert_eq!(
            scope.push_page(page(vec![record], false), false),
            Err(SessionScopeError::Missing(
                ScopeMissingReason::TruncatedEvidence
            ))
        );
    }
    let mut record = generated_context(false);
    let prepared = prepare_session_content(
        "native-session",
        SourceFormat::CodexRolloutJsonl,
        page(vec![record.clone()], false),
        Vec::new(),
    );
    record.part.metadata.non_authorizing_context =
        prepared.actions[0].metadata.non_authorizing_context.clone();
    assert!(record.part.metadata.non_authorizing_context.is_some());
    record.part.metadata.non_authorizing_context_proof = None;
    let mut valid = builder_for_source(1, SourceFormat::CodexRolloutJsonl);
    valid
        .push_page(
            page(
                vec![
                    part(0, ContentKind::UserText, "Keep billing unchanged."),
                    record.clone(),
                ],
                false,
            ),
            false,
        )
        .unwrap();
    assert_eq!(
        valid.finish().unwrap().values(),
        &[json!("Keep billing unchanged.")]
    );
    record.turn_index = 2;
    let mut scope = builder_for_source(2, SourceFormat::CodexRolloutJsonl);
    assert_eq!(
        scope.push_page(page(vec![record], false), false),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::InvalidEvidence
        ))
    );
}

fn item(text: &str) -> JevWorkItem {
    JevWorkItem {
        id: "candidate".into(),
        window: JevInputWindow {
            fields: json!({"candidate": text}),
            evidence: Vec::new(),
        },
        questions: BTreeMap::from([(
            "scope".into(),
            JevQuestion::Noul {
                instructions: json!("Does this exceed the user scope?"),
                criteria: None,
            },
        )]),
    }
}

#[test]
fn full_followup_preserves_negation_whitespace_headings_and_later_approval() {
    let initial = "# Scope\n- Keep API\n  - Do not change billing\n\n```\n  x = 2\n```";
    let followup = "No. Do not delete tests.\n  Keep retries.\n# Exception\nOnly billing labels.";
    let mut scope = builder(5);
    scope
        .push_page(
            page(
                vec![
                    part(0, ContentKind::UserText, initial),
                    part(1, ContentKind::AssistantText, "Also change billing logic?"),
                    part(2, ContentKind::UserText, followup),
                ],
                true,
            ),
            true,
        )
        .unwrap();
    scope
        .push_page(
            page(
                vec![
                    part(3, ContentKind::AssistantText, "Only update billing labels?"),
                    part(4, ContentKind::UserText, "yes"),
                    part(
                        5,
                        ContentKind::UserText,
                        "Later approval: add one label test.",
                    ),
                ],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    assert_eq!(snapshot.values()[0], initial);
    assert!(snapshot.values().contains(&json!(followup)));
    assert_eq!(
        snapshot
            .occurrences()
            .iter()
            .map(|item| item.reference.turn_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5]
    );
    assert_eq!(
        snapshot.occurrences()[1].authority,
        ScopeAuthority::SupportingContext
    );
    let packed = snapshot
        .pack_scope_creep(&[item("update label")], &ModelCapabilities::jev_default())
        .unwrap();
    assert_eq!(packed.batches.len(), 1);
    assert_eq!(
        packed.batches[0].request.state["shared_context"],
        snapshot.user_context().fields
    );
}

#[test]
fn exact_dedup_retains_each_occurrence_and_does_not_merge_whitespace() {
    let mut scope = builder(2);
    scope
        .push_page(
            page(
                vec![
                    part(0, ContentKind::UserText, "Do not delete."),
                    part(1, ContentKind::UserText, "Do not delete."),
                    part(2, ContentKind::UserText, "Do  not delete.\n"),
                ],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    assert_eq!(snapshot.values().len(), 2);
    assert_eq!(snapshot.occurrences().len(), 3);
    assert_ne!(
        snapshot.occurrences()[0].reference.id,
        snapshot.occurrences()[1].reference.id
    );
    assert_eq!(
        snapshot.occurrences()[0].value_index,
        snapshot.occurrences()[1].value_index
    );
}

#[test]
fn no_thinking_crossbranch_delegated_or_after_boundary_context() {
    let mut other = part(1, ContentKind::UserText, "other branch approval");
    other.thread_id = "other".into();
    let mut delegated = part(2, ContentKind::UserText, "child approval");
    delegated.scope = "delegated".into();
    let mut scope = builder(3);
    scope
        .push_page(
            page(
                vec![
                    part(0, ContentKind::Thinking, "private"),
                    other,
                    delegated,
                    part(3, ContentKind::UserText, "Keep scope"),
                    part(4, ContentKind::UserText, "future approval"),
                ],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    assert_eq!(snapshot.values(), &[json!("Keep scope")]);
}

#[test]
fn question_display_options_and_selections_are_lossless_and_source_bound() {
    let answer = scope_answer_fixture();
    let mut record = part(0, ContentKind::ToolResult, "question result");
    record.part.metadata.user_answers.push(answer.clone());
    let mut scope = builder(0);
    scope.push_page(page(vec![record], false), false).unwrap();
    let snapshot = scope.finish().unwrap();
    assert_eq!(snapshot.values()[0], scope_value(&answer).unwrap());
    assert_eq!(snapshot.occurrences()[0].native_source, Some(answer.source));
    assert_eq!(snapshot.occurrences()[0].authority, ScopeAuthority::User);
}

#[test]
fn unknown_answers_and_mutable_plans_remain_unknown_not_approved() {
    let mut answer = scope_answer_fixture();
    answer.origin = crate::analysis::jev_evidence::JevUserAnswerOrigin::UnknownOrigin;
    let plan = scope_plan_fixture();
    let mut record = part(1, ContentKind::ToolResult, "result");
    record.part.metadata.user_answers.push(answer.clone());
    record.part.metadata.plan_references.push(plan.clone());
    let mut scope = builder(1);
    scope
        .push_page(
            page(
                vec![part(0, ContentKind::UserText, "Keep scope"), record],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    assert_eq!(
        snapshot.scope_creep_context(),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::UnresolvedInfluence
        ))
    );
    assert!(snapshot.values().contains(&scope_value(&answer).unwrap()));
    assert!(snapshot.values().contains(&scope_value(&plan).unwrap()));
}

#[test]
fn huge_scope_and_candidate_or_questions_fail_without_reduction() {
    let mut scope = builder(0);
    scope
        .push_page(
            page(
                vec![part(0, ContentKind::UserText, &"x".repeat(40_000))],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    assert_eq!(
        snapshot.pack_scope_creep(&[item("small")], &ModelCapabilities::jev_default()),
        Err(SessionScopeError::ScopeTooLarge)
    );
    let mut scope = builder(0);
    scope
        .push_page(
            page(vec![part(0, ContentKind::UserText, "Keep scope")], false),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    assert_eq!(
        snapshot
            .pack_scope_creep(
                &[item(&"x".repeat(300_000))],
                &ModelCapabilities::jev_default()
            )
            .unwrap()
            .skipped_item_ids,
        vec!["candidate"]
    );
    let mut candidate = item("small");
    candidate.questions.insert(
        "huge".into(),
        JevQuestion::Noul {
            instructions: json!("x".repeat(300_000)),
            criteria: None,
        },
    );
    assert_eq!(
        snapshot
            .pack_scope_creep(&[candidate], &ModelCapabilities::jev_default())
            .unwrap()
            .skipped_item_ids,
        vec!["candidate"]
    );
}

#[test]
fn missing_pages_boundaries_and_changed_publications_are_typed_and_sticky() {
    assert_eq!(
        builder(0).finish(),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::IncompletePaging
        ))
    );
    let mut scope = builder(1);
    scope
        .push_page(
            page(vec![part(0, ContentKind::UserText, "scope")], false),
            false,
        )
        .unwrap();
    assert_eq!(
        scope.finish(),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::BoundaryMissing
        ))
    );
    let mut scope = builder(0);
    let mut changed = page(vec![part(0, ContentKind::UserText, "scope")], false);
    changed.publication_fence = 5;
    assert_eq!(
        scope.push_page(changed, false),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        ))
    );
    assert_eq!(
        scope.finish(),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        ))
    );
}

#[test]
fn truncated_scope_never_becomes_complete() {
    let mut record = part(0, ContentKind::UserText, "partial scope");
    record.part.truncated = true;
    let mut scope = builder(0);
    assert_eq!(
        scope.push_page(page(vec![record], false), false),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::TruncatedEvidence
        ))
    );
    assert_eq!(
        scope.finish(),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::TruncatedEvidence
        ))
    );
}

#[test]
fn same_thread_siblings_require_native_ancestry_and_unresolved_branches_fail() {
    let mut scope = builder(2);
    scope.boundary.branch =
        SessionScopeBranch::NativeRecords(BTreeSet::from(["event-0".into(), "event-2".into()]));
    scope
        .push_page(
            page(
                vec![
                    part(0, ContentKind::UserText, "Do not change billing"),
                    part(1, ContentKind::UserText, "sibling: change billing"),
                    part(2, ContentKind::UserText, "Keep the exclusion"),
                ],
                false,
            ),
            false,
        )
        .unwrap();
    assert_eq!(
        scope.finish().unwrap().values(),
        &[json!("Do not change billing"), json!("Keep the exclusion")]
    );
    let mut boundary = builder(0).boundary;
    boundary.branch = SessionScopeBranch::Unresolved;
    assert!(matches!(
        SessionScopeBuilder::new(SourceFormat::ClaudeJsonl, boundary, 4, 3, true),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::BranchUnresolved
        ))
    ));
}

#[test]
fn recorded_proposal_is_context_and_mismatched_approval_or_companion_is_unknown() {
    for content_status in [
        JevPlanContentStatus::Recorded,
        JevPlanContentStatus::VersionMatchedCompanion,
    ] {
        let mut plan = scope_plan_fixture();
        plan.content_status = content_status;
        plan.status = JevPlanStatus::Approved;
        let mut record = part(1, ContentKind::ToolResult, "result");
        record.part.metadata.plan_references.push(plan);
        let mut scope = builder(1);
        scope
            .push_page(
                page(
                    vec![part(0, ContentKind::UserText, "Keep scope"), record],
                    false,
                ),
                false,
            )
            .unwrap();
        assert_eq!(
            scope.finish().unwrap().scope_creep_context(),
            Err(SessionScopeError::Missing(
                ScopeMissingReason::UnresolvedInfluence
            ))
        );
    }
    let mut plan = scope_plan_fixture();
    plan.content_status = JevPlanContentStatus::Recorded;
    plan.status = JevPlanStatus::Proposed;
    let mut record = part(1, ContentKind::AssistantText, "Proposal");
    record.part.metadata.plan_references.push(plan.clone());
    let mut scope = builder(2);
    scope
        .push_page(
            page(
                vec![
                    part(0, ContentKind::UserText, "Keep scope"),
                    record,
                    part(2, ContentKind::UserText, "yes, except billing"),
                ],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    assert!(snapshot.scope_creep_context().is_ok());
    assert!(snapshot.values().contains(&scope_value(&plan).unwrap()));
    assert_eq!(
        snapshot
            .occurrences()
            .iter()
            .find(|item| item.field == JevInputField::PlanReference)
            .unwrap()
            .authority,
        ScopeAuthority::SupportingContext
    );
}

#[test]
fn every_batch_has_one_complete_scope_and_missing_limits_are_typed() {
    let mut scope = builder(0);
    scope
        .push_page(
            page(
                vec![part(
                    0,
                    ContentKind::UserText,
                    "# Keep scope\n  Do not delete",
                )],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.questions_per_request.value = Some(1);
    let first = item("first");
    let mut second = item("second");
    second.id = "second".into();
    let batches = snapshot
        .pack_user_context(&[first, second], &capabilities)
        .unwrap()
        .batches;
    assert_eq!(batches.len(), 2);
    for batch in batches {
        assert_eq!(
            batch.request.state["shared_context"],
            snapshot.user_context().fields
        );
        assert_eq!(
            batch.request.state["work_items"].as_array().unwrap().len(),
            1
        );
    }
    capabilities.total_input_tokens.value = None;
    assert_eq!(
        snapshot.pack_user_context(&[item("small")], &capabilities),
        Err(SessionScopeError::Missing(
            ScopeMissingReason::MissingCapabilities
        ))
    );
}

#[test]
fn cancelled_and_skipped_workflows_are_exact_non_authorizing_context() {
    for status in [JevUserAnswerStatus::Cancelled, JevUserAnswerStatus::Skipped] {
        let mut answer = scope_answer_fixture();
        answer.status = status;
        answer.origin = JevUserAnswerOrigin::UnknownOrigin;
        let mut record = part(1, ContentKind::ToolResult, "result");
        record.part.metadata.user_answers.push(answer.clone());
        let mut scope = builder(1);
        scope
            .push_page(
                page(
                    vec![part(0, ContentKind::UserText, "Keep scope"), record],
                    false,
                ),
                false,
            )
            .unwrap();
        let snapshot = scope.finish().unwrap();
        assert_eq!(
            snapshot.occurrences()[1].authority,
            ScopeAuthority::NonAuthorizing
        );
        assert_eq!(snapshot.values()[1], scope_value(&answer).unwrap());
        assert_eq!(
            snapshot
                .pack_scope_creep(&[item("small")], &ModelCapabilities::jev_default())
                .unwrap()
                .batches
                .len(),
            1
        );
    }
}

#[test]
fn submitted_unknown_origin_and_pending_unknown_answers_stay_unresolved() {
    for status in [
        JevUserAnswerStatus::Submitted,
        JevUserAnswerStatus::Pending,
        JevUserAnswerStatus::Unknown,
    ] {
        let mut answer = scope_answer_fixture();
        answer.status = status;
        answer.origin = JevUserAnswerOrigin::UnknownOrigin;
        let mut record = part(1, ContentKind::ToolResult, "result");
        record.part.metadata.user_answers.push(answer);
        let mut scope = builder(1);
        scope
            .push_page(
                page(
                    vec![part(0, ContentKind::UserText, "Keep scope"), record],
                    false,
                ),
                false,
            )
            .unwrap();
        assert_eq!(
            scope.finish().unwrap().scope_creep_context(),
            Err(SessionScopeError::Missing(
                ScopeMissingReason::UnresolvedInfluence
            ))
        );
    }
}

#[test]
fn oversized_activity_keeps_other_candidates_and_complete_scope() {
    let mut scope = builder(0);
    scope
        .push_page(
            page(
                vec![part(0, ContentKind::UserText, "Do not delete tests")],
                false,
            ),
            false,
        )
        .unwrap();
    let snapshot = scope.finish().unwrap();
    let mut huge = item(&"x".repeat(300_000));
    huge.id = "huge".into();
    let packed = snapshot
        .pack_scope_creep(&[item("small"), huge], &ModelCapabilities::jev_default())
        .unwrap();
    assert_eq!(packed.skipped_item_ids, vec!["huge"]);
    assert_eq!(packed.batches.len(), 1);
    assert_eq!(packed.batches[0].work_item_ids, vec!["candidate"]);
    assert_eq!(
        packed.batches[0].request.state["shared_context"],
        snapshot.user_context().fields
    );
}

#[test]
fn version_matched_recorded_plan_and_session_linked_bytes_supply_context() {
    for content_status in [
        JevPlanContentStatus::Recorded,
        JevPlanContentStatus::VersionMatchedCompanion,
    ] {
        let mut plan = scope_plan_fixture();
        plan.content_status = content_status;
        plan.status = JevPlanStatus::Approved;
        plan.approved_revision = plan.revision.clone();
        plan.approved_content_digest = plan.content_digest.clone();
        if content_status == JevPlanContentStatus::VersionMatchedCompanion {
            plan.source.provenance = JevScopeEvidenceProvenance::SessionLinkedCompanion;
        }
        let mut record = part(1, ContentKind::ToolResult, "result");
        record.part.metadata.plan_references.push(plan.clone());
        let mut scope = builder(1);
        scope
            .push_page(
                page(
                    vec![part(0, ContentKind::UserText, "Keep scope"), record],
                    false,
                ),
                false,
            )
            .unwrap();
        let snapshot = scope.finish().unwrap();
        assert!(snapshot.scope_creep_context().is_ok());
        assert!(snapshot.values().contains(&scope_value(&plan).unwrap()));
    }
}
