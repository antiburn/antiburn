use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::analysis::jev::*;
use crate::checks::sampling::StableId;

use super::planning::{ScopeCreepCheck, ScopeCreepPrepared, WorkBinding, WorkObservationKind};
use super::{DECISION_THRESHOLD, REVISIONS, ScopeAnswer, ScopeQuestion};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeCreepStatus {
    Finding,
    Clean,
    Uncertain,
    Unassessed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeCreepDecision {
    pub group_id: String,
    pub status: ScopeCreepStatus,
    pub outcome: Option<ScopeAnswer>,
    pub decision_probability: Option<f64>,
    pub limitation: Option<String>,
    pub reduced_answer_ids: Vec<StableId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeCreepFinding {
    pub id: String,
    pub group_id: String,
    pub work: Vec<WorkBinding>,
    pub task_scope: Vec<JevEvidenceReference>,
    pub observation_kind: WorkObservationKind,
    pub decision_probability: f64,
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
    _complete: bool,
) -> Result<ScopeCreepResult, JevError> {
    if plan.check_id != check.id()
        || plan.input_revision != check.context().input_revision
        || plan.revisions != REVISIONS
    {
        return Err(JevError::InvalidCheckPlan);
    }
    let mut groups = Vec::new();
    for group in &plan.prepared.groups {
        groups.push(check.canonical_group(group)?);
    }
    let mut canonical = check.prepare_groups(check.context(), &plan.capabilities, groups)?;
    canonical.coverage.not_selected_items = plan.coverage.not_selected_items;
    if plan
        .coverage
        .limitations
        .iter()
        .any(|limit| limit == "descriptor_enumeration_incomplete")
    {
        canonical.coverage.processing_limit_reached = true;
        canonical
            .coverage
            .limitations
            .push("descriptor_enumeration_incomplete".into());
    }
    if plan
        .coverage
        .limitations
        .iter()
        .any(|limit| limit == "descriptor_storage_budget_reached")
    {
        canonical.coverage.processing_limit_reached = true;
        canonical
            .coverage
            .limitations
            .push("descriptor_storage_budget_reached".into());
    }
    if plan.prepared != canonical.prepared
        || plan.work_items != canonical.work_items
        || plan.shared_context != canonical.shared_context
        || plan.skipped_item_ids != canonical.skipped_item_ids
    {
        return Err(JevError::InvalidCheckPlan);
    }
    let mut by_id = BTreeMap::new();
    let mut requests = BTreeMap::new();
    for result in results {
        let item = plan
            .work_items
            .iter()
            .find(|item| item.id == result.work_item_id)
            .ok_or(JevError::InvalidCheckPlan)?;
        if result.request_id.is_empty()
            || result.evidence != item.window.evidence
            || by_id.insert(result.work_item_id.as_str(), result).is_some()
        {
            return Err(JevError::InvalidCheckPlan);
        }
        validate_jev_response_with_capabilities(
            &JevResponse {
                model: result.model.clone(),
                answers: result.answers.clone(),
                usage: result.usage,
            },
            &JevRequest {
                model: plan.capabilities.model.clone(),
                state: serde_json::Value::Null,
                questions: item.questions.clone(),
            },
            &plan.capabilities,
        )?;
        if let Some(previous) = requests.insert(&result.request_id, result.usage)
            && previous != result.usage
        {
            return Err(JevError::InvalidCheckPlan);
        }
    }
    let mut decisions = Vec::new();
    let mut findings = Vec::new();
    for group in &plan.prepared.groups {
        let mut decision = ScopeCreepDecision {
            group_id: group.id.clone(),
            status: ScopeCreepStatus::Unassessed,
            outcome: None,
            decision_probability: None,
            limitation: group.limitation.clone(),
            reduced_answer_ids: Vec::new(),
        };
        if let Some(window) = group.window_ids.first()
            && let Some(result) = by_id.get(window.as_str())
        {
            let JevAnswer::Choice {
                choice,
                probabilities,
                ..
            } = &result.answers[ScopeQuestion::Decision.key()]
            else {
                return Err(JevError::ResponseAnswerTypeMismatch);
            };
            if highest_probability_choice(choice, probabilities) != Some(choice.as_str()) {
                return Err(JevError::InvalidChoiceDistribution);
            }
            let outcome = ScopeAnswer::parse(choice).ok_or(JevError::InvalidChoiceDistribution)?;
            let probability = probabilities[choice];
            decision.outcome = Some(outcome);
            decision.decision_probability = Some(probability);
            decision.status = match outcome {
                ScopeAnswer::LikelyScopeExpansion if probability >= DECISION_THRESHOLD => {
                    ScopeCreepStatus::Finding
                }
                ScopeAnswer::NoIssue if group.limitation.is_none() => ScopeCreepStatus::Clean,
                _ => ScopeCreepStatus::Uncertain,
            };
            decision.reduced_answer_ids = vec![ScopeCreepCheck::answer_identity(
                plan,
                window,
                ScopeQuestion::Decision,
            )];
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
                    task_scope: group.task_scope.clone(),
                    observation_kind: group.observation_kind,
                    decision_probability: probability,
                    scope_digest: plan.prepared.scope_digest.clone(),
                    model: plan.capabilities.model.clone(),
                    model_revision: plan.capabilities.model_revision.clone(),
                    revisions: REVISIONS,
                    source_generation: plan.prepared.source_generation,
                    publication_fence: plan.prepared.publication_fence,
                });
            }
        }
        decisions.push(decision);
    }
    let assessed_candidates = decisions
        .iter()
        .filter(|decision| decision.outcome.is_some())
        .count();
    let mut coverage = canonical.coverage;
    let uncertain = decisions
        .iter()
        .any(|decision| decision.status == ScopeCreepStatus::Uncertain);
    if uncertain {
        coverage.limitations.push("uncertain_decision".into());
    }
    let remaining_candidates = coverage.not_selected_items + decisions.len() - assessed_candidates;
    if remaining_candidates > 0 {
        coverage.limitations.push("assessment_incomplete".into());
    }
    coverage.limitations.sort();
    coverage.limitations.dedup();
    let unique: BTreeSet<_> = decisions
        .iter()
        .map(|decision| &decision.group_id)
        .collect();
    if unique.len() != decisions.len() {
        return Err(JevError::InvalidCheckPlan);
    }
    Ok(ScopeCreepResult {
        input_revision: plan.input_revision.clone(),
        scope_digest: plan.prepared.scope_digest.clone(),
        model: plan.capabilities.model.clone(),
        revisions: REVISIONS,
        findings,
        decisions,
        coverage,
        session_limitation: plan.prepared.session_limitation.clone(),
        assessed_candidates,
        remaining_candidates,
        request_count: requests.len(),
        input_tokens: requests.values().map(|usage| usage.input_tokens).sum(),
        output_tokens: requests.values().map(|usage| usage.output_tokens).sum(),
    })
}
