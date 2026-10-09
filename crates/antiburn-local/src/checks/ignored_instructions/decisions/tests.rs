use super::*;
use crate::checks::ignored_instructions::assessment::tests::{event, input};
use crate::checks::ignored_instructions::{
    INPUT_SELECTION, build_assessment_plan, build_jev_context, select_session_content,
};

fn anchored(actions: Vec<ContentAction>) -> (AssessmentPlan, CandidateComparison) {
    let plan = build_assessment_plan(input(actions, "Request validation before publishing."));
    let comparison = plan
        .comparisons
        .iter()
        .find(|comparison| comparison.reference.action_id == "publish")
        .unwrap()
        .clone();
    (plan, comparison)
}

#[test]
fn coherent_history_preserves_distant_events_and_one_anchor() {
    let mut actions = vec![event(
        "validation",
        1,
        "assistant",
        "main",
        "Requested validation for package alpha.",
    )];
    actions.extend((2..12).map(|index| {
        event(
            &format!("note-{index}"),
            index,
            "assistant",
            "main",
            "Reviewed a separate note.",
        )
    }));
    actions.push(event(
        "sibling",
        5,
        "assistant",
        "other",
        "Published another package.",
    ));
    actions.push(event(
        "publish",
        12,
        "assistant",
        "main",
        "Requested publication for package alpha.",
    ));
    actions.push(event(
        "later",
        13,
        "assistant",
        "main",
        "Requested validation after publication.",
    ));
    let (_, comparison) = anchored(actions.clone());
    let before = comparison.reference.clone();
    let history = episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
    );
    assert!(history.complete_selected_history);
    assert_eq!(history.events.first().unwrap().action_id, "validation");
    assert_eq!(history.events.len(), 11);
    assert_eq!(history.events.len(), history.identities.len());
    assert!(
        history
            .events
            .iter()
            .all(|event| !matches!(event.action_id.as_str(), "publish" | "sibling" | "later"))
    );
    assert!(
        history
            .events
            .windows(2)
            .all(|events| events[0].source_order < events[1].source_order)
    );
    assert_eq!(comparison.reference, before);
}

#[test]
fn selected_episode_excludes_thinking_and_cannot_use_unproven_results_or_authorization() {
    let mut thinking = event("private", 1, "assistant", "main", "PRIVATE_THINKING");
    thinking.kind = "thinking".to_owned();
    let mut result = event("result", 2, "tool", "main", "SECRET_RESULT");
    result.kind = "tool_result".to_owned();
    result.tool_name = Some("Bash".to_owned());
    let mut user = event("approval", 3, "user", "main", "PRIVATE_APPROVAL");
    user.kind = "user_text".to_owned();
    let source = input(
        vec![
            thinking,
            result,
            user,
            event("publish", 4, "assistant", "main", "Publish requested."),
        ],
        "Request validation before publishing.",
    );
    let selected = select_session_content(&source.content, INPUT_SELECTION);
    assert_eq!(selected.actions.len(), 3);
    let context = build_jev_context(&source).unwrap();
    let pool = context.check_context["episode_actions"].to_string();
    assert!(!pool.contains("PRIVATE_THINKING"));
    assert!(pool.contains("SECRET_RESULT"));
    assert!(pool.contains("PRIVATE_APPROVAL"));
    let plan = build_assessment_plan(source.clone());
    let comparison = &plan.comparisons[0];
    let history = episode(
        comparison,
        &selected.actions,
        &ModelCapabilities::jev_default(),
        true,
    );
    assert!(history.events.is_empty());
    assert!(!history.complete_selected_history);
    assert!(!history.authorization_available());
    assert!(!history.results_available());
}

#[test]
fn budget_or_incomplete_source_never_proves_absence() {
    let mut actions = (1..70)
        .map(|index| {
            event(
                &format!("event-{index}"),
                index,
                "assistant",
                "main",
                "Recorded request.",
            )
        })
        .collect::<Vec<_>>();
    actions.push(event(
        "publish",
        70,
        "assistant",
        "main",
        "Publish requested.",
    ));
    let (_, comparison) = anchored(actions.clone());
    let bounded = episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
    );
    assert!(!bounded.complete_selected_history);
    assert!(bounded.events.len() <= 64);
    let huge = vec![event("huge", 1, "assistant", "main", &"é".repeat(20_000))];
    let bounded = episode(&comparison, &huge, &ModelCapabilities::jev_default(), true);
    assert!(!bounded.complete_selected_history);
    assert_eq!(bounded.events.len(), 1);
    assert!(bounded.events[0].truncated);
    assert!(bounded.events[0].text.len() <= 4096);
    assert_eq!(bounded.selected_actions[0].text, huge[0].text);
    assert!(bounded.has_source_bindings(comparison.source_binding.as_ref().unwrap()));
    assert!(
        !episode(&comparison, &[], &ModelCapabilities::jev_default(), false)
            .complete_selected_history
    );
}

#[test]
fn episode_revision_tracks_content_and_completeness_with_stable_anchor() {
    let mut actions = vec![
        event("earlier", 1, "assistant", "main", "Requested checks."),
        event("publish", 2, "assistant", "main", "Publish requested."),
    ];
    let (_, comparison) = anchored(actions.clone());
    let first = episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
    );
    actions[0].text = "Requested a different check.".to_owned();
    let changed = episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
    );
    let partial = episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        false,
    );
    assert_ne!(first.revision, changed.revision);
    assert_ne!(changed.revision, partial.revision);
    assert_eq!(comparison.reference.action_id, "publish");
}

#[test]
fn oversized_support_reaches_production_as_exact_structural_ranges() {
    use crate::analysis::jev::{JevAnswer, JevCheck, JevUsage, JevWorkItemResult};
    use crate::checks::ignored_instructions::IgnoredInstructionsCheck;

    let text = format!(
        "FIRST_CONTEXT\n{}\nRequest validation before publishing. MIDDLE_CONTEXT\n{}\nLAST_CONTEXT",
        "Unrelated earlier work. 界é🦀\n".repeat(1000),
        "Unrelated later work. 界é🦀\n".repeat(1000),
    );
    let mut private = event("private", 0, "assistant", "main", "PRIVATE_THINKING");
    private.kind = "thinking".to_owned();
    let source = input(
        vec![
            private,
            event("support", 1, "assistant", "main", &text),
            event("publish", 2, "assistant", "main", "Publish requested."),
        ],
        "Request validation before publishing.",
    );
    let context = build_jev_context(&source).unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let comparison = plan
        .prepared
        .comparisons
        .iter()
        .find(|item| item.reference.action_id == "publish")
        .unwrap();
    let history = comparison.prerequisite_episode.as_ref().unwrap();
    assert!(!history.complete_selected_history);
    assert!(history.has_source_bindings(comparison.source_binding.as_ref().unwrap()));
    let window = plan
        .work_items
        .iter()
        .find(|item| item.window.fields["candidate_action"]["text"] == "Publish requested.")
        .unwrap();
    let support = &window.window.fields["instruction_targets"][0]["earlier_counterevidence"][0];
    let selected = support["text"].as_str().unwrap();
    for marker in ["FIRST_CONTEXT", "MIDDLE_CONTEXT", "LAST_CONTEXT"] {
        assert!(selected.contains(marker), "{marker}: {support}");
    }
    assert!(selected.len() <= 4096);
    assert_eq!(selected, history.events[0].text);
    let ranges = support["selected_text_ranges"].as_array().unwrap();
    assert!(ranges.len() > 1);
    assert_eq!(ranges.len(), history.identities.len());
    assert!(
        !window
            .window
            .fields
            .to_string()
            .contains("PRIVATE_THINKING")
    );
    for (range, identity) in ranges.iter().zip(&history.identities) {
        let start = range["start"].as_u64().unwrap() as usize;
        let end = range["end"].as_u64().unwrap() as usize;
        let selected_start = range["selected_text_start"].as_u64().unwrap() as usize;
        let selected_end = range["selected_text_end"].as_u64().unwrap() as usize;
        assert_eq!(&selected[selected_start..selected_end], &text[start..end]);
        assert_eq!((identity.start_byte, identity.end_byte), (start, end));
        assert_eq!(identity.source.id, "support");
        assert_eq!(
            identity.content_digest,
            super::super::content_action_digest(&source.content.actions[1])
        );
    }
    let answers = window
        .questions
        .keys()
        .map(|key| {
            (
                key.clone(),
                JevAnswer::Choice {
                    choice: "conflict".to_owned(),
                    probabilities: [
                        ("conflict".to_owned(), 0.90),
                        ("no_issue".to_owned(), 0.04),
                        ("pending_completion".to_owned(), 0.03),
                        ("uncertain".to_owned(), 0.03),
                    ]
                    .into(),
                    confidence: 0.90,
                },
            )
        })
        .collect();
    let result = JevWorkItemResult {
        request_id: "citation-regression".to_owned(),
        work_item_id: window.id.clone(),
        answers,
        evidence: window.window.evidence.clone(),
        model: plan.prepared.model_version.clone(),
        usage: JevUsage {
            input_tokens: 10,
            output_tokens: 2,
        },
    };
    let reduced = IgnoredInstructionsCheck
        .reduce(&plan, &[result], true)
        .unwrap();
    let finding = reduced
        .findings
        .iter()
        .find(|finding| finding.reference.action_id == "publish")
        .unwrap();
    let decision = finding.decision_record().unwrap();
    assert_eq!(decision.selected_evidence, history.identities);
    assert_eq!(
        decision
            .citations
            .iter()
            .find(|citation| citation.claim == CitationClaim::ObservedContext)
            .unwrap()
            .source_ids,
        ["support".to_owned()]
    );
    let stored = serde_json::to_string(decision).unwrap();
    let restored: DecisionRecord = serde_json::from_str(&stored).unwrap();
    assert!(restored.has_citation_proof());
    assert_eq!(restored.selected_evidence, history.identities);
    let mut invalid = restored.clone();
    invalid.selected_evidence[1].start_byte = 0;
    assert!(!invalid.has_citation_proof());
    for (identity, range) in restored.selected_evidence.iter().zip(ranges) {
        let selected_start = range["selected_text_start"].as_u64().unwrap() as usize;
        let selected_end = range["selected_text_end"].as_u64().unwrap() as usize;
        assert_eq!(
            &text[identity.start_byte..identity.end_byte],
            &selected[selected_start..selected_end]
        );
    }
}

#[test]
fn saved_decision_requires_complete_exact_citations_and_legacy_has_no_record() {
    let (plan, comparison) = anchored(vec![event(
        "publish",
        1,
        "assistant",
        "main",
        "Publish requested.",
    )]);
    let record = record(&plan, &comparison, None, Vec::new()).unwrap();
    assert!(record.contrast_template().is_some());
    let basis = record.explanation_basis.as_ref().unwrap();
    assert_eq!(basis.relationship, InstructionMismatch::RequirementConflict);
    assert_eq!(
        basis.instruction,
        super::super::planning::rule_text_fragment(&comparison)
    );
    assert_eq!(basis.action, comparison.action.text);
    let saved = serde_json::to_string(&record).unwrap();
    assert_eq!(
        serde_json::from_str::<DecisionRecord>(&saved).unwrap(),
        record
    );
    let mut invalid = record.clone();
    invalid.action_authority = "unknown".to_owned();
    assert!(invalid.contrast_template().is_none());
    let mut invalid = record.clone();
    invalid.action_anchor.source.id = "different-action".to_owned();
    assert!(invalid.contrast_template().is_none());
    let mut invalid = record.clone();
    invalid.action_anchor.source.stable = false;
    assert!(invalid.contrast_template().is_none());
    let mut invalid = record;
    invalid
        .citations
        .retain(|proof| proof.claim != CitationClaim::RuleRequirement);
    assert!(invalid.contrast_template().is_none());
    let mut legacy = serde_json::to_value(comparison).unwrap();
    legacy.as_object_mut().unwrap().remove("source_binding");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("prerequisite_episode");
    let legacy: CandidateComparison = serde_json::from_value(legacy).unwrap();
    assert!(super::record(&plan, &legacy, None, Vec::new()).is_none());
}

#[test]
fn episode_proof_rejects_changed_events_ranges_order_and_completeness() {
    let actions = vec![
        event("first", 1, "assistant", "main", "Requested first check."),
        event("second", 2, "assistant", "main", "Requested second check."),
        event("publish", 3, "assistant", "main", "Requested publication."),
    ];
    let (_, comparison) = anchored(actions.clone());
    let history = episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
    );
    let binding = comparison.source_binding.as_ref().unwrap();
    assert!(history.has_source_bindings(binding));
    for mutation in [
        "text", "range", "identity", "order", "complete", "role", "kind",
    ] {
        let mut invalid = history.clone();
        match mutation {
            "text" => invalid.events[0].text.push_str(" Changed."),
            "range" => invalid.identities[0].end_byte += 1,
            "identity" => invalid.identities[0].source.id = "unselected".to_owned(),
            "order" => invalid.identities.reverse(),
            "complete" => invalid.complete_selected_history = false,
            "role" => invalid.events[0].role = "user".to_owned(),
            "kind" => invalid.events[0].kind = "thinking".to_owned(),
            _ => unreachable!(),
        }
        assert!(!invalid.has_source_bindings(binding), "{mutation}");
    }
}

#[test]
fn decision_proof_requires_every_selected_prerequisite_citation() {
    let actions = vec![
        event("first", 1, "assistant", "main", "Requested first check."),
        event("publish", 2, "assistant", "main", "Requested publication."),
    ];
    let (plan, mut comparison) = anchored(actions.clone());
    comparison.prerequisite_episode = Some(episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
    ));
    let obligation = ObservableObligation {
        path_change_policy: crate::checks::ignored_instructions::PathChangePolicy::Other,
        path_change_conflict: None,
        literal_policies: Vec::new(),
        condition_evidence: crate::analysis::jev::obligations::ConditionEvidence::Selected,
        prerequisite_required: true,
        permission: crate::analysis::jev::obligations::PermissionRequirement::Independent,
        read_request_order: None,
        read_order_required: false,
        read_order_unknown: false,
        read_success_required: false,
        read_prerequisite_absent: false,
        candidate_family: "assistant".to_owned(),
        edit_scope_matches: None,
        recorded_edit_only: false,
        edit_scope_unknown: false,
    };
    let decision = record(&plan, &comparison, Some(&obligation), Vec::new()).unwrap();
    assert!(decision.has_citation_proof());
    let mut invalid = decision.clone();
    invalid.citations.last_mut().unwrap().source_ids.pop();
    assert!(invalid.contrast_template().is_none());
    let mut invalid = decision;
    invalid.selected_evidence[0].end_byte = 0;
    assert!(invalid.contrast_template().is_none());
    comparison.action_text_end += 1;
    assert!(record(&plan, &comparison, Some(&obligation), Vec::new()).is_none());
}
#[test]
fn source_bound_utf8_suffix_uses_absolute_bounds_once() {
    let text = format!(
        "{}\nI requested a prohibited operation.",
        "界é🦀\n".repeat(800)
    );
    let source = input(
        vec![event("publish", 1, "assistant", "main", &text)],
        "Do not request that operation.",
    );
    let plan = build_assessment_plan(source);
    let comparison = plan
        .comparisons
        .iter()
        .find(|comparison| comparison.action_text_start > 0)
        .unwrap();
    let binding = comparison.source_binding.as_ref().unwrap();
    assert!(binding.matches(comparison));
    let decision = record(&plan, comparison, None, Vec::new()).unwrap();
    let anchor = &decision.action_anchor;
    assert_eq!(
        text.get(anchor.start_byte..anchor.end_byte),
        Some(comparison.action.text.as_str())
    );
    assert!(decision.has_citation_proof());
    for mutation in ["range", "text", "anchor", "digest"] {
        let mut changed = comparison.clone();
        match mutation {
            "range" => changed.action_text_start += 1,
            "text" => changed.action.text.push('x'),
            "anchor" => changed.action.action_id = "another-action".to_owned(),
            "digest" => changed
                .source_binding
                .as_mut()
                .unwrap()
                .excerpt
                .as_mut()
                .unwrap()
                .text_digest
                .clear(),
            _ => unreachable!(),
        }
        assert!(
            record(&plan, &changed, None, Vec::new()).is_none(),
            "{mutation}"
        );
    }
}
#[test]
fn distant_native_read_witness_survives_episode_budget_without_complete_history_claim() {
    let mut actions = vec![event(
        "read-witness",
        1,
        "assistant",
        "main",
        "Recorded required read request.",
    )];
    actions.extend((2..90).map(|order| {
        event(
            &format!("note-{order}"),
            order,
            "assistant",
            "main",
            "Recorded unrelated note.",
        )
    }));
    actions.push(event(
        "publish",
        90,
        "assistant",
        "main",
        "Recorded triggering request.",
    ));
    let (_, comparison) = anchored(actions.clone());
    let baseline = episode(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
    );
    assert!(
        !baseline
            .events
            .iter()
            .any(|event| event.action_id == "read-witness")
    );
    let selected = episode_with_witnesses(
        &comparison,
        &actions,
        &ModelCapabilities::jev_default(),
        true,
        &["read-witness".to_owned()],
    );
    assert!(
        selected
            .events
            .iter()
            .any(|event| event.action_id == "read-witness")
    );
    assert!(!selected.complete_selected_history);
    assert!(selected.has_source_bindings(comparison.source_binding.as_ref().unwrap()));
    assert_eq!(comparison.reference.action_id, "publish");
    assert_ne!(baseline.revision, selected.revision);
}
