//! Deterministic judgments, finding publication, and incomplete coverage.

use super::*;

const EVIDENCE_BASIS_THRESHOLD: f64 = 0.75;

#[cfg(test)]
#[path = "reduction/tests.rs"]
mod tests;

pub fn reduce_assessment(
    plan: &AssessmentPlan,
    responses: &BTreeMap<String, JevWorkItemResult>,
    processing_complete: bool,
) -> AssessmentResult {
    reduce_with_completion(plan, responses, processing_complete, &BTreeMap::new())
}

pub(super) fn reduce_with_completion(
    plan: &AssessmentPlan,
    responses: &BTreeMap<String, JevWorkItemResult>,
    processing_complete: bool,
    completion: &BTreeMap<String, CompletionCoverage>,
) -> AssessmentResult {
    reduce_traced(plan, responses, processing_complete, completion, &mut None)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonDiagnostic {
    pub comparison_id: String,
    pub action_id: String,
    pub stage: String,
    pub reason: String,
    pub probabilities: BTreeMap<String, BTreeMap<String, f64>>,
}

fn trace(
    diagnostics: &mut Option<&mut Vec<ComparisonDiagnostic>>,
    comparison: &CandidateComparison,
    stage: &str,
    reason: &str,
    judgment: Option<&ComparisonJudgment>,
) {
    if let Some(diagnostics) = diagnostics {
        diagnostics.push(ComparisonDiagnostic {
            comparison_id: comparison.id.clone(),
            action_id: comparison.reference.action_id.clone(),
            stage: stage.to_owned(),
            reason: reason.to_owned(),
            probabilities: judgment
                .map(|judgment| judgment.probabilities.clone())
                .unwrap_or_default(),
        });
    }
}

pub(super) fn reduce_traced(
    plan: &AssessmentPlan,
    responses: &BTreeMap<String, JevWorkItemResult>,
    processing_complete: bool,
    completion: &BTreeMap<String, CompletionCoverage>,
    diagnostics: &mut Option<&mut Vec<ComparisonDiagnostic>>,
) -> AssessmentResult {
    let mut findings = BTreeMap::<String, AssessmentFinding>::new();
    let mut reassessed_finding_ids = Vec::new();
    let mut pending_rules = Vec::new();
    let mut unassessed_comparisons = Vec::new();
    let mut request_count = 0u32;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut seen = BTreeSet::new();
    let mut seen_requests = BTreeSet::new();
    for comparison in &plan.comparisons {
        if comparison
            .source_binding
            .as_ref()
            .is_some_and(|binding| !binding.matches(comparison))
            || comparison.reference.scope == InstructionScope::Unknown
            || comparison
                .prerequisite_episode
                .as_ref()
                .is_some_and(|episode| {
                    !comparison
                        .source_binding
                        .as_ref()
                        .is_some_and(|binding| episode.has_source_bindings(binding))
                })
        {
            trace(
                diagnostics,
                comparison,
                "source_proof",
                "binding_scope_or_episode_invalid",
                None,
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        }
        let Some(initial) = responses.get(&comparison.id) else {
            trace(
                diagnostics,
                comparison,
                "response",
                "candidate_response_missing",
                None,
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        };
        record_usage(
            initial,
            &mut seen_requests,
            &mut request_count,
            &mut input_tokens,
            &mut output_tokens,
        );
        let Some(mut final_judgment) = judgment(initial, completion.get(&comparison.id).copied())
        else {
            trace(
                diagnostics,
                comparison,
                "response",
                "required_candidate_answers_missing",
                None,
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        };
        if plan
            .observable_obligations
            .get(&comparison.id)
            .is_some_and(|obligation| {
                obligation.condition_evidence
                    == crate::analysis::jev::obligations::ConditionEvidence::Result
                    && !comparison
                        .prerequisite_episode
                        .as_ref()
                        .is_some_and(|episode| {
                            episode.complete_selected_history && episode.results_available()
                        })
            })
        {
            trace(
                diagnostics,
                comparison,
                "condition",
                "required_result_not_selected",
                Some(&final_judgment),
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        }
        if plan
            .observable_obligations
            .get(&comparison.id)
            .is_some_and(|obligation| {
                !obligation.permission.observable_without_authority()
                    && !comparison
                        .prerequisite_episode
                        .as_ref()
                        .is_some_and(|episode| episode.authorization_available())
            })
        {
            trace(
                diagnostics,
                comparison,
                "authority",
                "required_authority_not_selected",
                Some(&final_judgment),
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        }
        if plan
            .observable_obligations
            .get(&comparison.id)
            .is_some_and(|obligation| {
                obligation.candidate_family == "edit" && obligation.edit_scope_unknown
            })
        {
            trace(
                diagnostics,
                comparison,
                "source_proof",
                "edit_scope_identity_unknown",
                Some(&final_judgment),
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        }
        if plan
            .observable_obligations
            .get(&comparison.id)
            .is_some_and(|obligation| {
                obligation.prerequisite_required
                    && !obligation.read_order_required
                    && !obligation.read_prerequisite_absent
                    && !comparison
                        .prerequisite_episode
                        .as_ref()
                        .is_some_and(|episode| episode.complete_selected_history)
            })
            && final_judgment.applicability != "not_applicable"
        {
            trace(
                diagnostics,
                comparison,
                "prerequisite",
                "necessary_episode_incomplete",
                Some(&final_judgment),
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        }
        if final_judgment.applicability == "not_applicable"
            && (comparison.action.truncated
                || comparison.action_text_start != 0
                || comparison.rule_text_start != 0
                || comparison.rule_text_end != comparison.rule_text.len())
            && !initial.answers.contains_key(QUESTION_RELATIONSHIP)
        {
            trace(
                diagnostics,
                comparison,
                "response",
                "partial_candidate_followup_missing",
                Some(&final_judgment),
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        }
        if final_judgment.applicability == "not_applicable"
            && probability(&final_judgment, QUESTION_APPLICABILITY, "not_applicable")
                >= LIKELY_THRESHOLD
            && !comparison.action.truncated
            && comparison.action_text_start == 0
            && comparison.rule_text_start == 0
            && comparison.rule_text_end == comparison.rule_text.len()
            && !initial.answers.contains_key(QUESTION_RELATIONSHIP)
            && !initial.answers.contains_key(QUESTION_EVIDENCE_BASIS)
            && !plan
                .observable_obligations
                .get(&comparison.id)
                .is_some_and(|obligation| {
                    obligation.read_order_required && obligation.edit_scope_matches == Some(true)
                        || (obligation.path_change_conflict == Some(true)
                            && obligation.candidate_family == "edit"
                            && obligation.edit_scope_matches == Some(true))
                        || obligation
                            .literal_policies
                            .iter()
                            .any(|binding| {
                                binding.exact_match == Some(true)
                                    || binding.policy
                                        == crate::analysis::jev::exact_facts::LiteralPolicy::ResponseLiteral
                                    || (super::super::action_context::is_assistant_text(&comparison.action.kind)
                                        && matches!(binding.policy,
                                            crate::analysis::jev::exact_facts::LiteralPolicy::ConstructBan
                                            | crate::analysis::jev::exact_facts::LiteralPolicy::CommandBan))
                            })
                })
        {
            trace(diagnostics, comparison, "semantic", "confident_not_applicable", Some(&final_judgment));
            reassessed_finding_ids.push(finding_id_for_reference(&comparison.reference));
            continue;
        }
        if let Some(obligation) = plan.observable_obligations.get(&comparison.id) {
            apply_literal_facts(comparison, obligation, initial, &mut final_judgment);
            if obligation.read_order_required
                && obligation.candidate_family == "edit"
                && obligation.edit_scope_matches == Some(true)
            {
                final_judgment.applicability = "applies".to_owned();
                final_judgment.probabilities.insert(
                    QUESTION_APPLICABILITY.to_owned(),
                    BTreeMap::from([("applies".to_owned(), 1.0)]),
                );
            }
            if (obligation.condition_evidence
                != crate::analysis::jev::obligations::ConditionEvidence::Selected
                && obligation.condition_evidence
                    != crate::analysis::jev::obligations::ConditionEvidence::Result)
                || (obligation.prerequisite_required
                    && !comparison.prior_history_complete
                    && comparison.action.kind == "tool_input")
            {
                trace(
                    diagnostics,
                    comparison,
                    "condition",
                    "condition_unavailable_or_required_history_missing",
                    Some(&final_judgment),
                );
                unassessed_comparisons.push(comparison.id.clone());
                continue;
            }
            if obligation.path_change_conflict == Some(true)
                && obligation.candidate_family == "edit"
                && obligation.edit_scope_matches == Some(true)
                && !comparison.action.truncated
            {
                final_judgment.applicability = "applies".to_owned();
                final_judgment.relationship = "conflict".to_owned();
                final_judgment.evidence_basis = "self_contained".to_owned();
                for (question, option) in [
                    (QUESTION_APPLICABILITY, "applies"),
                    (QUESTION_RELATIONSHIP, "conflict"),
                    (QUESTION_EVIDENCE_BASIS, "self_contained"),
                ] {
                    final_judgment.probabilities.insert(
                        question.to_owned(),
                        BTreeMap::from([(option.to_owned(), 1.0)]),
                    );
                }
            }
            if obligation.read_order_required
                && obligation.recorded_edit_only
                && obligation.candidate_family == "assistant"
            {
                trace(
                    diagnostics,
                    comparison,
                    "candidate",
                    "recorded_edit_rule_does_not_cover_text",
                    Some(&final_judgment),
                );
                continue;
            }
            if obligation.read_order_required
                && obligation.candidate_family == "edit"
                && obligation.edit_scope_unknown
            {
                trace(
                    diagnostics,
                    comparison,
                    "source_proof",
                    "read_trigger_identity_unknown",
                    Some(&final_judgment),
                );
                unassessed_comparisons.push(comparison.id.clone());
                continue;
            }
            if obligation.read_order_unknown && final_judgment.applicability != "not_applicable" {
                trace(
                    diagnostics,
                    comparison,
                    "prerequisite",
                    "read_requirement_classification_unknown",
                    Some(&final_judgment),
                );
                unassessed_comparisons.push(comparison.id.clone());
                continue;
            }
            if obligation.read_success_required
                && obligation.read_prerequisite_absent
                && obligation.candidate_family == "edit"
                && probability(&final_judgment, QUESTION_APPLICABILITY, "applies")
                    >= LIKELY_THRESHOLD
            {
                final_judgment.relationship = "conflict".to_owned();
                final_judgment.evidence_basis = "self_contained".to_owned();
                for (question, option) in [
                    (QUESTION_RELATIONSHIP, "conflict"),
                    (QUESTION_EVIDENCE_BASIS, "self_contained"),
                ] {
                    final_judgment.probabilities.insert(
                        question.to_owned(),
                        BTreeMap::from([(option.to_owned(), 1.0)]),
                    );
                }
            }
            if obligation.read_success_required
                && final_judgment.applicability != "not_applicable"
                && !(obligation.candidate_family == "edit"
                    && obligation.read_prerequisite_absent
                    && final_judgment.relationship == "conflict")
            {
                trace(
                    diagnostics,
                    comparison,
                    "prerequisite",
                    "read_success_not_proven",
                    Some(&final_judgment),
                );
                unassessed_comparisons.push(comparison.id.clone());
                continue;
            }
            if obligation.read_order_required
                && obligation.candidate_family == "edit"
                && obligation.edit_scope_matches == Some(false)
            {
                trace(
                    diagnostics,
                    comparison,
                    "candidate",
                    "edit_outside_read_trigger_scope",
                    Some(&final_judgment),
                );
                continue;
            }
            if obligation.read_order_required
                && (final_judgment.applicability == "applies"
                    || obligation.candidate_family == "read")
            {
                let relationship = match obligation.candidate_family.as_str() {
                    "edit" => match obligation
                        .read_request_order
                        .as_ref()
                        .map(|order| order.state())
                    {
                        Some(crate::analysis::jev::obligations::ObligationState::Satisfied) => {
                            "follows"
                        }
                        Some(crate::analysis::jev::obligations::ObligationState::Violated) => {
                            "conflict"
                        }
                        _ => {
                            trace(
                                diagnostics,
                                comparison,
                                "prerequisite",
                                "read_request_order_not_proven",
                                Some(&final_judgment),
                            );
                            unassessed_comparisons.push(comparison.id.clone());
                            continue;
                        }
                    },
                    "read" => {
                        final_judgment.applicability = "applies".to_owned();
                        final_judgment.probabilities.insert(
                            QUESTION_APPLICABILITY.to_owned(),
                            BTreeMap::from([("applies".to_owned(), 1.0)]),
                        );
                        "follows"
                    }
                    _ => {
                        trace(
                            diagnostics,
                            comparison,
                            "prerequisite",
                            "read_order_trigger_not_bound",
                            Some(&final_judgment),
                        );
                        unassessed_comparisons.push(comparison.id.clone());
                        continue;
                    }
                };
                final_judgment.relationship = relationship.to_owned();
                final_judgment.evidence_basis = "self_contained".to_owned();
                final_judgment.completion = CompletionCoverage::NotObligation;
                for (question, selected) in [
                    (QUESTION_RELATIONSHIP, relationship),
                    (QUESTION_EVIDENCE_BASIS, "self_contained"),
                ] {
                    final_judgment.probabilities.insert(
                        question.to_owned(),
                        BTreeMap::from([(selected.to_owned(), 1.0)]),
                    );
                }
            }
        }
        if !seen.insert(comparison.id.clone()) {
            continue;
        }
        if final_judgment.completion == CompletionCoverage::BoundaryNotObserved
            && probability(
                &final_judgment,
                QUESTION_COMPLETION,
                "completion_not_observed",
            ) >= POSSIBLE_THRESHOLD
        {
            pending_rules.push(PendingRule {
                instruction_id: comparison.reference.instruction_id.clone(),
                instruction_digest: comparison.reference.instruction_digest.clone(),
                rule_id: comparison.reference.rule_id.clone(),
                heading: comparison.reference.rule_heading.clone(),
                reason: "completion_boundary_not_observed".to_owned(),
            });
        }
        let status = classify_comparison(comparison, &final_judgment);
        if matches!(status, RuleStatus::Likely | RuleStatus::Possible)
            && comparison.source_binding.is_some()
            && super::super::decisions::record(
                plan,
                comparison,
                plan.observable_obligations.get(&comparison.id),
                comparison_limitations(plan, comparison),
            )
            .is_none()
        {
            trace(
                diagnostics,
                comparison,
                "publication",
                "decision_citation_proof_missing",
                Some(&final_judgment),
            );
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        }
        if matches!(status, RuleStatus::Likely | RuleStatus::Possible)
            || (status == RuleStatus::NoIssue
                && comparison.rule_text_start == 0
                && comparison.rule_text_end == comparison.rule_text.len()
                && comparison.action_text_start == 0
                && !comparison.action.truncated)
        {
            reassessed_finding_ids.push(finding_id_for_reference(&comparison.reference));
        }
        match status {
            RuleStatus::Likely | RuleStatus::Possible => {
                trace(
                    diagnostics,
                    comparison,
                    "publication",
                    "finding_published",
                    Some(&final_judgment),
                );
                let certainty = if status == RuleStatus::Likely {
                    FindingCertainty::Likely
                } else {
                    FindingCertainty::Possible
                };
                let mut finding_limits = comparison_limitations(plan, comparison);
                if plan
                    .observable_obligations
                    .get(&comparison.id)
                    .is_some_and(|obligation| obligation.read_order_required)
                {
                    finding_limits.push("recorded_read_request_order_only".to_owned());
                    finding_limits.push("requests_do_not_prove_reading_or_execution".to_owned());
                }
                finding_limits.sort();
                finding_limits.dedup();
                let (instruction_excerpt, instruction_excerpt_truncated) =
                    super::super::planning::bounded_text(
                        super::super::planning::rule_text_fragment(comparison),
                        2048,
                    );
                let action_text = &comparison.action.text;
                let (action_excerpt, action_excerpt_truncated) =
                    super::super::planning::bounded_text(action_text, 2048);
                let finding = AssessmentFinding {
                    decision: super::super::decisions::record(
                        plan,
                        comparison,
                        plan.observable_obligations.get(&comparison.id),
                        finding_limits.clone(),
                    ),
                    id: finding_id_for_reference(&comparison.reference),
                    reference: comparison.reference.clone(),
                    instruction_excerpt,
                    instruction_excerpt_truncated,
                    action_excerpt,
                    action_excerpt_truncated,
                    nearby_context_ids: comparison
                        .context
                        .iter()
                        .map(|event| event.action_id.clone())
                        .collect(),
                    counterevidence_ids: comparison
                        .prerequisite_episode
                        .as_ref()
                        .map(|episode| episode.events.as_slice())
                        .unwrap_or(&comparison.counterevidence)
                        .iter()
                        .map(|event| event.action_id.clone())
                        .collect(),
                    certainty,
                    conflict_probability: probability(
                        &final_judgment,
                        QUESTION_RELATIONSHIP,
                        "conflict",
                    ),
                    applicability_probability: probability(
                        &final_judgment,
                        QUESTION_APPLICABILITY,
                        "applies",
                    ),
                    evidence_basis_probability: probability(
                        &final_judgment,
                        QUESTION_EVIDENCE_BASIS,
                        "self_contained",
                    ),
                    limitations: finding_limits,
                };
                findings
                    .entry(finding.id.clone())
                    .and_modify(|previous| {
                        if finding_order(&finding, previous).is_gt() {
                            *previous = finding.clone();
                        }
                    })
                    .or_insert(finding);
            }
            RuleStatus::NoIssue => trace(
                diagnostics,
                comparison,
                "semantic",
                "clean_comparison",
                Some(&final_judgment),
            ),
            RuleStatus::Unassessed => {
                trace(
                    diagnostics,
                    comparison,
                    "semantic",
                    semantic_blocker(&final_judgment),
                    Some(&final_judgment),
                );
                unassessed_comparisons.push(comparison.id.clone());
            }
        }
    }
    pending_rules.sort_by(|left, right| {
        (
            &left.instruction_id,
            &left.instruction_digest,
            &left.rule_id,
        )
            .cmp(&(
                &right.instruction_id,
                &right.instruction_digest,
                &right.rule_id,
            ))
    });
    pending_rules.dedup_by(|left, right| {
        left.instruction_id == right.instruction_id
            && left.instruction_digest == right.instruction_digest
            && left.rule_id == right.rule_id
    });
    unassessed_comparisons.sort();
    unassessed_comparisons.dedup();
    let mut coverage = plan.coverage.clone();
    coverage.reassessed_comparison_ids = plan
        .comparisons
        .iter()
        .map(|comparison| comparison.id.clone())
        .collect();
    coverage.reassessed_comparison_ids.sort();
    coverage.reassessed_comparison_ids.dedup();
    coverage.reassessed_rule_ids = plan
        .comparisons
        .iter()
        .map(|comparison| comparison.reference.rule_id.clone())
        .collect();
    coverage.reassessed_rule_ids.sort();
    coverage.reassessed_rule_ids.dedup();
    coverage.reassessed_finding_ids = reassessed_finding_ids;
    coverage.reassessed_finding_ids.sort();
    coverage.reassessed_finding_ids.dedup();
    if !plan.complete_input {
        coverage
            .limitations
            .push("source_evidence_is_partial".to_owned());
    }
    if plan.comparisons.iter().any(|comparison| {
        comparison.reference.provenance == InstructionProvenance::CurrentFileComparison
    }) {
        coverage
            .limitations
            .push("current_file_not_historical_proof".to_owned());
    }
    if !processing_complete {
        coverage.processing_limit_reached = true;
        coverage
            .limitations
            .push("assessment_processing_incomplete".to_owned());
    }
    if !unassessed_comparisons.is_empty() {
        coverage
            .limitations
            .push("some_comparisons_unassessed".to_owned());
    }
    if plan.comparisons.iter().any(|comparison| {
        unassessed_comparisons.contains(&comparison.id)
            && plan
                .observable_obligations
                .get(&comparison.id)
                .is_some_and(|obligation| {
                    obligation.candidate_family == "edit" && obligation.edit_scope_unknown
                })
    }) {
        coverage
            .limitations
            .push("edit_path_identity_unavailable".to_owned());
    }
    if plan.comparisons.iter().any(|comparison| {
        unassessed_comparisons.contains(&comparison.id)
            && comparison
                .prerequisite_episode
                .as_ref()
                .is_some_and(|episode| !episode.complete_selected_history)
    }) {
        coverage
            .limitations
            .push("prerequisite_episode_incomplete".to_owned());
    }
    if plan.comparisons.iter().any(|comparison| {
        comparison
            .prerequisite_episode
            .as_ref()
            .is_some_and(|episode| {
                !comparison
                    .source_binding
                    .as_ref()
                    .is_some_and(|binding| episode.has_source_bindings(binding))
            })
    }) {
        coverage
            .limitations
            .push("prerequisite_episode_binding_unavailable".to_owned());
    }
    coverage.limitations.sort();
    coverage.limitations.dedup();
    AssessmentResult {
        input_revision: plan.input_revision.clone(),
        model_version: plan.model_version.clone(),
        findings: findings.into_values().collect(),
        pending_rules,
        unassessed_comparisons,
        coverage,
        request_count,
        input_tokens,
        output_tokens,
    }
}

fn semantic_blocker(judgment: &ComparisonJudgment) -> &'static str {
    if judgment.evidence_basis != "self_contained"
        || probability(judgment, QUESTION_EVIDENCE_BASIS, "self_contained")
            < EVIDENCE_BASIS_THRESHOLD
    {
        "selected_evidence_basis_insufficient"
    } else if matches!(
        judgment.completion,
        CompletionCoverage::BoundaryNotObserved | CompletionCoverage::Uncertain
    ) {
        "completion_boundary_not_proven"
    } else {
        "verdict_or_completion_probability_below_gate"
    }
}

fn finding_id_for_reference(reference: &RuleActionRef) -> String {
    sha256_hex(
        format!(
            "{}\0{}\0{}\0{}\0{}\0{:?}",
            reference.instruction_id,
            reference.instruction_digest,
            reference.rule_id,
            reference.action_id,
            reference.provenance.as_str(),
            reference.scope,
        )
        .as_bytes(),
    )
}

fn apply_literal_facts(
    comparison: &CandidateComparison,
    obligation: &ObservableObligation,
    response: &JevWorkItemResult,
    judgment: &mut ComparisonJudgment,
) {
    use crate::analysis::jev::exact_facts::LiteralPolicy;
    if !comparison.reference.action_stable
        || (comparison.action.truncated
            && !comparison.source_binding.as_ref().is_some_and(|binding| {
                binding.matches(comparison)
                    && binding
                        .excerpt
                        .as_ref()
                        .is_some_and(|excerpt| !excerpt.source_truncated)
            }))
    {
        return;
    }
    let strongest = obligation
        .literal_policies
        .iter()
        .enumerate()
        .filter_map(
            |(index, binding)| match (binding.policy, binding.exact_match) {
                (LiteralPolicy::ToolBan, Some(true)) => Some(("conflict", 1.0)),
                (LiteralPolicy::ToolBan, Some(false)) => Some(("follows", 1.0)),
                (LiteralPolicy::ResponseLiteral, Some(true)) => Some(("follows", 1.0)),
                (LiteralPolicy::ResponseLiteral, Some(false)) => Some(("conflict", 1.0)),
                (LiteralPolicy::ConstructBan | LiteralPolicy::CommandBan, _)
                    if super::super::action_context::is_assistant_text(&comparison.action.kind) =>
                {
                    let question = format!("literal_report_{index}");
                    if crate::analysis::jev::classification::confident_choice(
                        response,
                        &question,
                        LIKELY_THRESHOLD,
                    ) != Some("reports_addition")
                    {
                        return None;
                    }
                    let JevAnswer::Choice { probabilities, .. } =
                        response.answers.get(&question)?
                    else {
                        return None;
                    };
                    Some(("conflict", *probabilities.get("reports_addition")?))
                }
                _ => None,
            },
        )
        .max_by(|left, right| {
            (left.0 == "conflict")
                .cmp(&(right.0 == "conflict"))
                .then_with(|| left.1.total_cmp(&right.1))
        });
    let Some((relationship, probability)) = strongest else {
        return;
    };
    judgment.applicability = "applies".to_owned();
    judgment.relationship = relationship.to_owned();
    judgment.evidence_basis = "self_contained".to_owned();
    judgment.completion = CompletionCoverage::NotObligation;
    for (question, option, probability) in [
        (QUESTION_APPLICABILITY, "applies", probability),
        (QUESTION_RELATIONSHIP, relationship, probability),
        (QUESTION_EVIDENCE_BASIS, "self_contained", 1.0),
    ] {
        judgment.probabilities.insert(
            question.to_owned(),
            BTreeMap::from([(option.to_owned(), probability)]),
        );
    }
}

pub(super) fn record_usage(
    result: &JevWorkItemResult,
    seen_requests: &mut BTreeSet<String>,
    request_count: &mut u32,
    input_tokens: &mut u64,
    output_tokens: &mut u64,
) {
    if seen_requests.insert(result.request_id.clone()) {
        *request_count = request_count.saturating_add(1);
        *input_tokens = input_tokens.saturating_add(result.usage.input_tokens);
        *output_tokens = output_tokens.saturating_add(result.usage.output_tokens);
    }
}

fn finding_order(left: &AssessmentFinding, right: &AssessmentFinding) -> std::cmp::Ordering {
    (left.certainty == FindingCertainty::Likely)
        .cmp(&(right.certainty == FindingCertainty::Likely))
        .then_with(|| {
            left.conflict_probability
                .total_cmp(&right.conflict_probability)
        })
        .then_with(|| {
            left.applicability_probability
                .total_cmp(&right.applicability_probability)
        })
        .then_with(|| {
            left.evidence_basis_probability
                .total_cmp(&right.evidence_basis_probability)
        })
        .then_with(|| left.nearby_context_ids.cmp(&right.nearby_context_ids))
        .then_with(|| left.counterevidence_ids.cmp(&right.counterevidence_ids))
        .then_with(|| left.limitations.cmp(&right.limitations))
        .then_with(|| left.action_excerpt.cmp(&right.action_excerpt))
        .then_with(|| left.instruction_excerpt.cmp(&right.instruction_excerpt))
        .then_with(|| {
            left.action_excerpt_truncated
                .cmp(&right.action_excerpt_truncated)
        })
        .then_with(|| {
            left.instruction_excerpt_truncated
                .cmp(&right.instruction_excerpt_truncated)
        })
}

fn judgment(
    result: &JevWorkItemResult,
    completion: Option<CompletionCoverage>,
) -> Option<ComparisonJudgment> {
    let probabilities: BTreeMap<String, BTreeMap<String, f64>> = result
        .answers
        .iter()
        .filter_map(|(local_id, answer)| match answer {
            JevAnswer::Choice { probabilities, .. } => {
                Some((local_id.clone(), probabilities.clone()))
            }
            _ => None,
        })
        .collect();
    let applicability = selected(result, QUESTION_APPLICABILITY)?;
    if applicability == "not_applicable"
        && crate::analysis::jev::classification::confident_choice(
            result,
            QUESTION_APPLICABILITY,
            LIKELY_THRESHOLD,
        ) == Some("not_applicable")
        && !result.answers.contains_key(QUESTION_RELATIONSHIP)
        && !result.answers.contains_key(QUESTION_EVIDENCE_BASIS)
    {
        return Some(ComparisonJudgment {
            applicability,
            relationship: "unrelated".to_owned(),
            evidence_basis: "self_contained".to_owned(),
            completion: CompletionCoverage::NotObligation,
            probabilities: BTreeMap::from([
                (
                    QUESTION_RELATIONSHIP.to_owned(),
                    BTreeMap::from([("unrelated".to_owned(), 1.0)]),
                ),
                (
                    QUESTION_EVIDENCE_BASIS.to_owned(),
                    BTreeMap::from([("self_contained".to_owned(), 1.0)]),
                ),
                (
                    QUESTION_APPLICABILITY.to_owned(),
                    probabilities[QUESTION_APPLICABILITY].clone(),
                ),
            ]),
        });
    }
    let applies = probabilities
        .get(QUESTION_APPLICABILITY)
        .and_then(|distribution| distribution.get("applies"))
        .copied()
        .unwrap_or_default();
    if applicability == "applies"
        && applies >= POSSIBLE_THRESHOLD
        && [
            QUESTION_RELATIONSHIP,
            QUESTION_EVIDENCE_BASIS,
            QUESTION_COMPLETION,
        ]
        .iter()
        .any(|question| {
            !result.answers.contains_key(*question)
                && (*question != QUESTION_COMPLETION || completion.is_none())
        })
    {
        return None;
    }
    Some(ComparisonJudgment {
        applicability,
        relationship: selected(result, QUESTION_RELATIONSHIP).unwrap_or_default(),
        evidence_basis: selected(result, QUESTION_EVIDENCE_BASIS).unwrap_or_default(),
        completion: if let Some(completion) = completion {
            completion
        } else {
            match selected(result, QUESTION_COMPLETION)
                .as_deref()
                .unwrap_or("uncertain")
            {
                "not_completion_obligation" => CompletionCoverage::NotObligation,
                "completion_not_observed" => CompletionCoverage::BoundaryNotObserved,
                "completion_observed" => CompletionCoverage::BoundaryObserved,
                "uncertain" => CompletionCoverage::Uncertain,
                _ => return None,
            }
        },
        probabilities,
    })
}

pub(super) fn selected(result: &JevWorkItemResult, question: &str) -> Option<String> {
    crate::analysis::jev::classification::confident_choice(result, question, 0.0).map(str::to_owned)
}

pub(super) fn probability(judgment: &ComparisonJudgment, question: &str, option: &str) -> f64 {
    judgment
        .probabilities
        .get(question)
        .and_then(|distribution| distribution.get(option))
        .copied()
        .unwrap_or_default()
}

pub(super) fn classify_comparison(
    comparison: &CandidateComparison,
    judgment: &ComparisonJudgment,
) -> RuleStatus {
    let conflict = probability(judgment, QUESTION_RELATIONSHIP, "conflict");
    let applies = probability(judgment, QUESTION_APPLICABILITY, "applies");
    let history_basis = probability(judgment, QUESTION_EVIDENCE_BASIS, "self_contained");
    if judgment.evidence_basis != "self_contained" || history_basis < EVIDENCE_BASIS_THRESHOLD {
        return RuleStatus::Unassessed;
    }
    let completion_option = match judgment.completion {
        CompletionCoverage::NotObligation => "not_completion_obligation",
        CompletionCoverage::BoundaryObserved => "completion_observed",
        CompletionCoverage::BoundaryNotObserved | CompletionCoverage::Uncertain => {
            return RuleStatus::Unassessed;
        }
    };
    // Rule classification can settle action obligations without a completion question.
    if judgment.probabilities.contains_key(QUESTION_COMPLETION)
        && probability(judgment, QUESTION_COMPLETION, completion_option) < POSSIBLE_THRESHOLD
    {
        return RuleStatus::Unassessed;
    }
    if matches!(judgment.relationship.as_str(), "follows" | "unrelated") {
        return if probability(judgment, QUESTION_RELATIONSHIP, &judgment.relationship)
            >= POSSIBLE_THRESHOLD
        {
            RuleStatus::NoIssue
        } else {
            RuleStatus::Unassessed
        };
    }
    if judgment.relationship != "conflict" || conflict < POSSIBLE_THRESHOLD {
        return RuleStatus::Unassessed;
    }
    if judgment.applicability != "applies" || applies < POSSIBLE_THRESHOLD {
        return RuleStatus::Unassessed;
    }
    let likely_evidence = comparison.reference.provenance
        != InstructionProvenance::CurrentFileComparison
        && comparison.reference.provenance != InstructionProvenance::ObservedRead
        && comparison.action.role == "assistant"
        && comparison.reference.action_stable
        && !comparison.action.truncated
        && comparison.reference.scope != InstructionScope::Unknown
        && applies >= LIKELY_THRESHOLD
        && conflict >= LIKELY_THRESHOLD
        && history_basis >= LIKELY_THRESHOLD
        && (!judgment.probabilities.contains_key(QUESTION_COMPLETION)
            || probability(judgment, QUESTION_COMPLETION, completion_option) >= LIKELY_THRESHOLD);
    if likely_evidence {
        RuleStatus::Likely
    } else if applies >= POSSIBLE_THRESHOLD {
        RuleStatus::Possible
    } else {
        RuleStatus::Unassessed
    }
}

fn comparison_limitations(plan: &AssessmentPlan, comparison: &CandidateComparison) -> Vec<String> {
    let mut limitations = plan.coverage.limitations.clone();
    if comparison.reference.provenance == InstructionProvenance::CurrentFileComparison {
        limitations.push("current_file_not_historical_proof".to_owned());
    }
    if comparison.action.truncated {
        limitations.push("candidate_action_truncated".to_owned());
    }
    if comparison.context_truncated || comparison.context.iter().any(|event| event.truncated) {
        limitations.push("nearby_context_truncated".to_owned());
    }
    if comparison
        .prerequisite_episode
        .as_ref()
        .map(|episode| !episode.complete_selected_history)
        .unwrap_or(comparison.earlier_history_truncated)
    {
        limitations.push("earlier_history_truncated".to_owned());
    }
    if !comparison.prior_history_complete {
        limitations.push("prior_history_incomplete".to_owned());
    }
    if !plan.complete_input {
        limitations.push("source_evidence_is_partial".to_owned());
    }
    limitations
}
