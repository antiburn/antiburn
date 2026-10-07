use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::analysis::jev::*;
use crate::checks::sampling::StableId;

use super::planning::{ScopeCreepCheck, ScopeCreepPrepared, WorkBinding};
use super::{DECISION_THRESHOLD, REVISIONS, ScopeAnswer, ScopeQuestion};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeCreepStatus {
    Finding,
    Clean,
    Unassessed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeCreepDecision {
    pub group_id: String,
    pub status: ScopeCreepStatus,
    pub judgments: BTreeMap<String, BTreeMap<ScopeQuestion, ScopeAnswer>>,
    #[serde(default)]
    pub accepted_questions: BTreeMap<String, BTreeSet<ScopeQuestion>>,
    #[serde(default)]
    pub proof_questions: Vec<ScopeQuestion>,
    pub limitation: Option<String>,
    /// The shell commits these answer IDs only for a settled candidate.
    pub reduced_answer_ids: Vec<StableId>,
}

/// A bounded retrospective claim. This record has no savings or auto-fix fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeCreepFinding {
    pub id: String,
    pub group_id: String,
    pub work: Vec<WorkBinding>,
    /// The claim binds the complete scope, not a fabricated decisive excerpt.
    pub task_scope: Vec<JevEvidenceReference>,
    pub scope_digest: String,
    pub model: String,
    pub model_revision: Option<String>,
    pub revisions: JevCheckRevisions,
    pub source_generation: i64,
    pub publication_fence: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeCreepResult {
    pub input_revision: String,
    pub scope_digest: String,
    pub model: String,
    pub revisions: JevCheckRevisions,
    pub findings: Vec<ScopeCreepFinding>,
    pub decisions: Vec<ScopeCreepDecision>,
    pub coverage: JevCoverage,
    pub session_limitation: Option<crate::analysis::session_scope::SessionScopeError>,
    pub assessed_candidates: usize,
    pub remaining_candidates: usize,
    pub request_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub(super) fn reduce(
    check: &ScopeCreepCheck,
    plan: &JevCheckPlan<ScopeCreepPrepared>,
    results: &[JevWorkItemResult],
    complete: bool,
) -> Result<ScopeCreepResult, JevError> {
    if plan.check_id != check.id()
        || plan.input_revision != check.context().input_revision
        || plan.revisions != REVISIONS
    {
        return Err(JevError::InvalidCheckPlan);
    }
    // Validate the admitted projection, including sampled subsets. A restored
    // plan cannot publish against a new source or scope publication.
    let expected = check.prepare_with_capabilities(check.context(), &plan.capabilities)?;
    let group_ids: BTreeSet<_> = plan.prepared.groups.iter().map(|group| &group.id).collect();
    let window_ids: BTreeSet<_> = plan
        .prepared
        .groups
        .iter()
        .flat_map(|group| group.window_ids.iter())
        .collect();
    let item_ids: BTreeSet<_> = plan.work_items.iter().map(|item| &item.id).collect();
    let canonical = if plan.prepared.groups == expected.prepared.groups {
        expected.clone()
    } else {
        ScopeCreepCheck::select_candidates(
            &expected,
            &group_ids
                .iter()
                .map(|id| StableId::new("scope_work", &[id.as_bytes()]))
                .collect(),
        )
    };
    let mut coverage = canonical.coverage.clone();
    if !complete
        && plan
            .coverage
            .limitations
            .iter()
            .any(|limit| limit == "assessment_incomplete")
    {
        coverage.limitations.push("assessment_incomplete".into());
        coverage.limitations.sort();
        coverage.limitations.dedup();
    }
    if plan.shared_context != expected.shared_context
        || plan.coverage != coverage
        || plan.skipped_item_ids != canonical.skipped_item_ids
        || group_ids.len() != plan.prepared.groups.len()
        || item_ids.len() != plan.work_items.len()
        || window_ids != item_ids
        || plan.prepared.scope_digest != expected.prepared.scope_digest
        || plan.prepared.scope_bindings != expected.prepared.scope_bindings
        || plan.prepared.source_generation != expected.prepared.source_generation
        || plan.prepared.publication_fence != expected.prepared.publication_fence
        || plan.prepared.semantic_epoch != expected.prepared.semantic_epoch
        || plan.prepared.session_limitation != expected.prepared.session_limitation
        || plan
            .prepared
            .groups
            .iter()
            .any(|group| !expected.prepared.groups.contains(group))
        || plan
            .work_items
            .iter()
            .any(|item| !expected.work_items.contains(item))
    {
        return Err(JevError::InvalidCheckPlan);
    }
    let mut by_id = BTreeMap::new();
    for result in results {
        if result.request_id.is_empty()
            || by_id.insert(result.work_item_id.as_str(), result).is_some()
        {
            return Err(JevError::InvalidCheckPlan);
        }
    }
    let allowed: BTreeSet<_> = plan
        .work_items
        .iter()
        .flat_map(|item| [item.id.clone(), format!("{}::followup", item.id)])
        .collect();
    if results
        .iter()
        .any(|result| !allowed.contains(&result.work_item_id))
    {
        return Err(JevError::InvalidCheckPlan);
    }
    validate_results(plan, results)?;
    let mut decisions = Vec::new();
    let mut findings = Vec::new();
    for group in &plan.prepared.groups {
        let mut decision = ScopeCreepDecision {
            group_id: group.id.clone(),
            status: ScopeCreepStatus::Unassessed,
            judgments: BTreeMap::new(),
            accepted_questions: BTreeMap::new(),
            proof_questions: Vec::new(),
            limitation: group.limitation.clone(),
            reduced_answer_ids: Vec::new(),
        };
        if plan.prepared.session_limitation.is_some() || group.window_ids.is_empty() {
            decision
                .limitation
                .get_or_insert("scope_unassessable".into());
            decisions.push(decision);
            continue;
        }
        let mut valid = true;
        for window in &group.window_ids {
            let item = plan
                .work_items
                .iter()
                .find(|item| &item.id == window)
                .ok_or(JevError::InvalidCheckPlan)?;
            let mut proof = plan
                .shared_context
                .as_ref()
                .map(|shared| shared.evidence.clone())
                .unwrap_or_default();
            proof.extend(item.window.evidence.clone());
            let mut judgments = BTreeMap::new();
            let mut accepted = BTreeSet::new();
            for question in ScopeQuestion::ALL {
                let result_id = if question == ScopeQuestion::Performed {
                    window.clone()
                } else {
                    format!("{window}::followup")
                };
                let Some(result) = by_id.get(result_id.as_str()) else {
                    valid = false;
                    continue;
                };
                if result.model != plan.capabilities.model || result.evidence != proof {
                    return Err(JevError::InvalidCheckPlan);
                }
                let (judgment, probability) = if question == ScopeQuestion::Authority {
                    let mut risk: f64 = 0.0;
                    for key in question.answer_keys() {
                        let JevAnswer::Noul { noul } = &result.answers[*key] else {
                            return Err(JevError::ResponseAnswerTypeMismatch);
                        };
                        risk = risk.max(*noul);
                    }
                    if risk >= DECISION_THRESHOLD {
                        (ScopeAnswer::Ambiguous, risk)
                    } else if 1.0 - risk >= DECISION_THRESHOLD {
                        (ScopeAnswer::Resolved, 1.0 - risk)
                    } else {
                        (ScopeAnswer::Unknown, 0.0)
                    }
                } else {
                    match &result.answers[question.key()] {
                        JevAnswer::Score { probabilities, .. }
                            if question == ScopeQuestion::Materiality =>
                        {
                            if probabilities["1"] >= DECISION_THRESHOLD {
                                (ScopeAnswer::Substantial, probabilities["1"])
                            } else if probabilities["0"] >= DECISION_THRESHOLD {
                                (ScopeAnswer::Minor, probabilities["0"])
                            } else {
                                (ScopeAnswer::Unknown, 0.0)
                            }
                        }
                        JevAnswer::Choice {
                            choice,
                            probabilities,
                            ..
                        } => (
                            ScopeAnswer::parse(choice)
                                .ok_or(JevError::InvalidChoiceDistribution)?,
                            probabilities[choice],
                        ),
                        JevAnswer::Noul { noul } if question.noul_answers().is_some() => {
                            let (positive, negative) = question
                                .noul_answers()
                                .ok_or(JevError::ResponseAnswerTypeMismatch)?;
                            if *noul >= DECISION_THRESHOLD {
                                (positive, *noul)
                            } else if 1.0 - noul >= DECISION_THRESHOLD {
                                (negative, 1.0 - noul)
                            } else {
                                (ScopeAnswer::Unknown, 0.0)
                            }
                        }
                        _ => return Err(JevError::ResponseAnswerTypeMismatch),
                    }
                };
                judgments.insert(question, judgment);
                if probability >= DECISION_THRESHOLD {
                    accepted.insert(question);
                }
            }
            decision.judgments.insert(window.clone(), judgments);
            decision.accepted_questions.insert(window.clone(), accepted);
        }
        if !complete || !valid {
            decision.limitation = Some("incomplete_or_uncertain_answers".into());
        } else {
            (decision.status, decision.proof_questions) =
                settle(&decision.judgments, &decision.accepted_questions);
            if decision.status == ScopeCreepStatus::Unassessed {
                decision.limitation = Some("ambiguous_scope_or_necessity".into());
            } else {
                decision.reduced_answer_ids = group
                    .window_ids
                    .iter()
                    .flat_map(|window| {
                        ScopeQuestion::ALL.into_iter().map(move |question| {
                            ScopeCreepCheck::answer_identity(plan, window, question)
                        })
                    })
                    .collect();
            }
        }
        if decision.status == ScopeCreepStatus::Finding {
            let id = super::planning::digest(&(
                &group.id,
                &plan.prepared.scope_digest,
                &plan.capabilities.model,
                &plan.capabilities.model_revision,
                REVISIONS,
            ))?;
            findings.push(ScopeCreepFinding {
                id,
                group_id: group.id.clone(),
                work: group.work.clone(),
                task_scope: plan.prepared.scope_bindings.clone(),
                scope_digest: plan.prepared.scope_digest.clone(),
                model: plan.capabilities.model.clone(),
                model_revision: plan.capabilities.model_revision.clone(),
                revisions: REVISIONS,
                source_generation: plan.prepared.source_generation,
                publication_fence: plan.prepared.publication_fence,
            });
        }
        decisions.push(decision);
    }
    let mut requests = BTreeMap::new();
    for result in results {
        if let Some(previous) = requests.insert(&result.request_id, result.usage)
            && previous != result.usage
        {
            return Err(JevError::InvalidCheckPlan);
        }
    }
    Ok(ScopeCreepResult {
        input_revision: plan.input_revision.clone(),
        scope_digest: plan.prepared.scope_digest.clone(),
        model: plan.capabilities.model.clone(),
        revisions: REVISIONS,
        assessed_candidates: decisions
            .iter()
            .filter(|decision| decision.status != ScopeCreepStatus::Unassessed)
            .count(),
        remaining_candidates: plan.coverage.not_selected_items
            + decisions
                .iter()
                .filter(|decision| decision.status == ScopeCreepStatus::Unassessed)
                .count(),
        findings,
        decisions,
        coverage: plan.coverage.clone(),
        session_limitation: plan.prepared.session_limitation.clone(),
        request_count: requests.len(),
        input_tokens: requests.values().map(|usage| usage.input_tokens).sum(),
        output_tokens: requests.values().map(|usage| usage.output_tokens).sum(),
    })
}

fn settle(
    windows: &BTreeMap<String, BTreeMap<ScopeQuestion, ScopeAnswer>>,
    accepted: &BTreeMap<String, BTreeSet<ScopeQuestion>>,
) -> (ScopeCreepStatus, Vec<ScopeQuestion>) {
    if windows.is_empty() {
        return (ScopeCreepStatus::Unassessed, Vec::new());
    }
    let supports = |question: ScopeQuestion, expected: &str| {
        windows.iter().all(|(window, judgments)| {
            accepted
                .get(window)
                .is_some_and(|questions| questions.contains(&question))
                && judgments.get(&question).map(|answer| answer.key()) == Some(expected)
        })
    };
    let guards = [
        ScopeQuestion::Authority,
        ScopeQuestion::Sufficiency,
        ScopeQuestion::Coherence,
    ];
    if !guards
        .into_iter()
        .all(|question| supports(question, question.positive()))
    {
        return (ScopeCreepStatus::Unassessed, Vec::new());
    }
    // A clean proof uses only its relevant branch. All answers still pass strict
    // response validation. Unused uncertainty is retained in the decision record.
    for (question, negative) in [
        (ScopeQuestion::Performed, "not_performed"),
        (ScopeQuestion::Approval, "authorized"),
        (ScopeQuestion::Necessity, "necessary"),
        (ScopeQuestion::OptionalWork, "not_optional"),
        (ScopeQuestion::Materiality, "minor"),
        (ScopeQuestion::LaterAcceptance, "accepted"),
    ] {
        if supports(question, negative) {
            let mut proof = guards.to_vec();
            proof.push(question);
            return (ScopeCreepStatus::Clean, proof);
        }
    }
    if ScopeQuestion::ALL
        .into_iter()
        .all(|question| supports(question, question.positive()))
    {
        return (ScopeCreepStatus::Finding, ScopeQuestion::ALL.to_vec());
    }
    (ScopeCreepStatus::Unassessed, Vec::new())
}

fn validate_results(
    plan: &JevCheckPlan<ScopeCreepPrepared>,
    results: &[JevWorkItemResult],
) -> Result<(), JevError> {
    for result in results {
        let initial_id = result
            .work_item_id
            .strip_suffix("::followup")
            .unwrap_or(&result.work_item_id);
        let item = plan
            .work_items
            .iter()
            .find(|item| item.id == initial_id)
            .ok_or(JevError::InvalidCheckPlan)?;
        let questions = if result.work_item_id == item.id {
            item.questions.clone()
        } else {
            super::questions::questions(false)
        };
        validate_jev_response_with_capabilities(
            &JevResponse {
                model: result.model.clone(),
                answers: result.answers.clone(),
                usage: result.usage,
            },
            &JevRequest {
                model: plan.capabilities.model.clone(),
                state: serde_json::Value::Null,
                questions,
            },
            &plan.capabilities,
        )?;
        for answer in result.answers.values() {
            let JevAnswer::Choice {
                choice,
                probabilities,
                ..
            } = answer
            else {
                continue;
            };
            // The shared validator permits rounded sums. Do not replace an
            // inconsistent selected label with a more favorable distribution label.
            if highest_probability_choice(choice, probabilities) != Some(choice.as_str()) {
                return Err(JevError::InvalidChoiceDistribution);
            }
        }
    }
    Ok(())
}
