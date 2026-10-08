//! Reduction of one joint semantic decision per rule/action pair.

use super::*;

#[cfg(test)]
#[path = "reduction/tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonDiagnostic {
    pub comparison_id: String,
    pub action_id: String,
    pub stage: String,
    pub reason: String,
    pub probabilities: BTreeMap<String, BTreeMap<String, f64>>,
}

pub fn reduce_assessment(
    plan: &AssessmentPlan,
    responses: &BTreeMap<String, JevWorkItemResult>,
    processing_complete: bool,
) -> AssessmentResult {
    reduce_traced(plan, responses, processing_complete, &mut None)
}

pub(super) fn reduce_traced(
    plan: &AssessmentPlan,
    responses: &BTreeMap<String, JevWorkItemResult>,
    processing_complete: bool,
    diagnostics: &mut Option<&mut Vec<ComparisonDiagnostic>>,
) -> AssessmentResult {
    let mut findings = BTreeMap::<String, AssessmentFinding>::new();
    let mut pending_rules = Vec::new();
    let mut unassessed_comparisons = Vec::new();
    let mut coverage = plan.coverage.clone();
    coverage.reassessed_comparison_ids.clear();
    coverage.reassessed_rule_ids.clear();
    coverage.reassessed_finding_ids.clear();
    let mut request_count = 0;
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut seen_requests = BTreeSet::new();
    for comparison in &plan.comparisons {
        let response = responses.get(&comparison.id);
        if let Some(response) = response {
            record_usage(
                response,
                &mut seen_requests,
                &mut request_count,
                &mut input_tokens,
                &mut output_tokens,
            );
        }
        let binding_valid = comparison.source_binding.as_ref().is_some_and(|binding| {
            binding.matches(comparison)
                && comparison
                    .prerequisite_episode
                    .as_ref()
                    .is_none_or(|episode| episode.has_source_bindings(binding))
        }) && comparison.reference.scope != InstructionScope::Unknown;
        let decision = response.and_then(valid_decision);
        let reason = if !binding_valid {
            "source_binding_invalid"
        } else if let Some((choice, probability)) = decision {
            coverage
                .reassessed_comparison_ids
                .push(comparison.id.clone());
            coverage
                .reassessed_rule_ids
                .push(comparison.reference.rule_id.clone());
            let id = finding_id_for_reference(&comparison.reference);
            match choice {
                "conflict" if probability >= DECISION_THRESHOLD => {
                    coverage.reassessed_finding_ids.push(id.clone());
                    let limitations = comparison_limitations(plan, comparison);
                    let finding = AssessmentFinding {
                        decision: super::super::decisions::record(
                            plan,
                            comparison,
                            None,
                            limitations.clone(),
                        ),
                        id,
                        reference: comparison.reference.clone(),
                        instruction_excerpt: rule_text_fragment(comparison).to_owned(),
                        instruction_excerpt_truncated: comparison.rule_text_start != 0
                            || comparison.rule_text_end != comparison.rule_text.len(),
                        action_excerpt: comparison.action.text.clone(),
                        action_excerpt_truncated: comparison.action.truncated,
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
                        certainty: if matches!(
                            comparison.reference.provenance,
                            InstructionProvenance::CurrentFileComparison
                                | InstructionProvenance::ObservedRead
                        ) || comparison.action.truncated
                        {
                            FindingCertainty::Possible
                        } else {
                            FindingCertainty::Likely
                        },
                        composite_probability: probability,
                        limitations,
                    };
                    findings
                        .entry(finding.id.clone())
                        .and_modify(|previous| {
                            if finding_order(&finding, previous).is_gt() {
                                *previous = finding.clone();
                            }
                        })
                        .or_insert(finding);
                    "conflict"
                }
                "no_issue" => {
                    coverage.reassessed_finding_ids.push(id);
                    "no_issue"
                }
                "pending_completion" => {
                    pending_rules.push(PendingRule {
                        instruction_id: comparison.reference.instruction_id.clone(),
                        instruction_digest: comparison.reference.instruction_digest.clone(),
                        rule_id: comparison.reference.rule_id.clone(),
                        heading: comparison.reference.rule_heading.clone(),
                        reason: "completion_not_observed".to_owned(),
                    });
                    "pending_completion"
                }
                _ => "uncertain",
            }
        } else {
            "decision_missing_or_invalid"
        };
        if matches!(
            reason,
            "source_binding_invalid" | "decision_missing_or_invalid"
        ) {
            unassessed_comparisons.push(comparison.id.clone());
        }
        if reason == "uncertain" {
            unassessed_comparisons.push(comparison.id.clone());
            coverage
                .limitations
                .push("semantic_decision_uncertain".to_owned());
        }
        if let Some(diagnostics) = diagnostics {
            diagnostics.push(ComparisonDiagnostic {
                comparison_id: comparison.id.clone(),
                action_id: comparison.reference.action_id.clone(),
                stage: "semantic".to_owned(),
                reason: reason.to_owned(),
                probabilities: response
                    .and_then(|result| result.answers.get(QUESTION_DECISION))
                    .and_then(|answer| match answer {
                        JevAnswer::Choice { probabilities, .. } => Some(BTreeMap::from([(
                            QUESTION_DECISION.to_owned(),
                            probabilities.clone(),
                        )])),
                        _ => None,
                    })
                    .unwrap_or_default(),
            });
        }
    }
    pending_rules.sort_by(|a, b| {
        (&a.instruction_id, &a.instruction_digest, &a.rule_id).cmp(&(
            &b.instruction_id,
            &b.instruction_digest,
            &b.rule_id,
        ))
    });
    pending_rules.dedup_by(|a, b| {
        a.instruction_id == b.instruction_id
            && a.instruction_digest == b.instruction_digest
            && a.rule_id == b.rule_id
    });
    unassessed_comparisons.sort();
    unassessed_comparisons.dedup();
    for ids in [
        &mut coverage.reassessed_comparison_ids,
        &mut coverage.reassessed_rule_ids,
        &mut coverage.reassessed_finding_ids,
    ] {
        ids.sort();
        ids.dedup();
    }
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

fn valid_decision(result: &JevWorkItemResult) -> Option<(&str, f64)> {
    let JevAnswer::Choice {
        choice,
        probabilities,
        ..
    } = result.answers.get(QUESTION_DECISION)?
    else {
        return None;
    };
    let options = ["conflict", "no_issue", "pending_completion", "uncertain"];
    if !options.contains(&choice.as_str())
        || probabilities.len() != options.len()
        || options.iter().any(|option| {
            !probabilities
                .get(*option)
                .is_some_and(|p| p.is_finite() && (0.0..=1.0).contains(p))
        })
        || (probabilities.values().sum::<f64>() - 1.0).abs() > 0.001
    {
        return None;
    }
    let selected =
        crate::analysis::jev::classification::confident_choice(result, QUESTION_DECISION, 0.0)?;
    Some((selected, probabilities[selected]))
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
            reference.scope
        )
        .as_bytes(),
    )
}

fn finding_order(left: &AssessmentFinding, right: &AssessmentFinding) -> std::cmp::Ordering {
    left.composite_probability
        .total_cmp(&right.composite_probability)
        .then_with(|| {
            (left.certainty == FindingCertainty::Likely)
                .cmp(&(right.certainty == FindingCertainty::Likely))
        })
        .then_with(|| left.action_excerpt.cmp(&right.action_excerpt))
        .then_with(|| left.instruction_excerpt.cmp(&right.instruction_excerpt))
        .then_with(|| left.nearby_context_ids.cmp(&right.nearby_context_ids))
        .then_with(|| left.counterevidence_ids.cmp(&right.counterevidence_ids))
        .then_with(|| left.limitations.cmp(&right.limitations))
        .then_with(|| {
            left.decision
                .as_ref()
                .map(|record| &record.context_revision)
                .cmp(
                    &right
                        .decision
                        .as_ref()
                        .map(|record| &record.context_revision),
                )
        })
}

pub(super) fn record_usage(
    result: &JevWorkItemResult,
    seen: &mut BTreeSet<String>,
    request_count: &mut u32,
    input_tokens: &mut u64,
    output_tokens: &mut u64,
) {
    if seen.insert(result.request_id.clone()) {
        *request_count = request_count.saturating_add(1);
        *input_tokens = input_tokens.saturating_add(result.usage.input_tokens);
        *output_tokens = output_tokens.saturating_add(result.usage.output_tokens);
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
    if comparison.context_truncated {
        limitations.push("nearby_context_truncated".to_owned());
    }
    if comparison
        .prerequisite_episode
        .as_ref()
        .is_some_and(|episode| !episode.complete_selected_history)
    {
        limitations.push("earlier_history_truncated".to_owned());
    }
    if !comparison.prior_history_complete {
        limitations.push("prior_history_incomplete".to_owned());
    }
    if !plan.complete_input {
        limitations.push("source_evidence_is_partial".to_owned());
    }
    limitations.sort();
    limitations.dedup();
    limitations
}
