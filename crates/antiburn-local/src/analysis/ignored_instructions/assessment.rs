//! Bounded request preparation and deterministic whole-scope reduction.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::evidence::{ContentAction, SessionContentEvidence};
use super::input::{
    InstructionContentClass, InstructionProvenance, InstructionRuleSection, InstructionScope,
    InstructionSnapshot, sha256_hex,
};
use crate::analysis::jev::{
    DEFAULT_CONTEXT_CHUNK_BYTES, JevAnswer, JevCheck, JevCheckPlan, JevContextUnit, JevCoverage,
    JevError, JevQuestion, JevRequest, JevRequestStage, JevResponse, JevSessionContext,
    JevWorkItem, JevWorkItemResult, chunk_context_units, pack_work_items, validate_jev_request,
    validate_jev_response,
};

pub const ASSESSMENT_MODEL: &str = crate::analysis::jev::PINNED_MODEL;
pub const ASSESSMENT_PREPARATION_REVISION: u32 = 1;
pub const ASSESSMENT_QUESTION_REVISION: u32 = 1;
pub const ASSESSMENT_REDUCER_REVISION: u32 = 1;
pub const MAX_ASSESSMENT_REQUESTS: usize = 64;
pub const MAX_ASSESSMENT_CANDIDATES: usize = MAX_ASSESSMENT_REQUESTS / 2;
const MAX_CONTEXT_EVENTS: usize = 4;
const MAX_COUNTER_EVIDENCE: usize = 8;
const LIKELY_THRESHOLD: f64 = 0.80;
const POSSIBLE_THRESHOLD: f64 = 0.55;

const QUESTION_APPLICABILITY: &str = "applicability";
const QUESTION_RELATIONSHIP: &str = "relationship";
const QUESTION_EXCEPTION: &str = "exception";
const QUESTION_REASON: &str = "reason";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssessmentInput {
    pub content: SessionContentEvidence,
    /// Actions at or after this watermark can produce findings. Earlier
    /// actions remain available as context for approvals and prerequisites.
    pub activity_after_ms: Option<i64>,
    pub source_generation: i64,
    pub source_fingerprint: Option<String>,
    pub incarnation: u64,
}

pub type RequestStage = JevRequestStage;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonRequest {
    pub request_id: String,
    pub stage: RequestStage,
    pub digest: String,
    pub request: JevRequest,
    pub serialized_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleActionRef {
    pub instruction_id: String,
    pub instruction_digest: String,
    pub rule_id: String,
    pub rule_heading: String,
    pub source: String,
    pub provenance: InstructionProvenance,
    pub scope: InstructionScope,
    pub action_id: String,
    pub action_stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterEvidence {
    pub action_id: String,
    pub role: String,
    pub kind: String,
    pub timestamp_ms: Option<i64>,
    pub tool_name: Option<String>,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateComparison {
    pub id: String,
    pub reference: RuleActionRef,
    pub rule_text: String,
    pub action: CounterEvidence,
    /// Nearby events in the same branch/thread, submitted with the initial
    /// comparison. This list does not include cross-chunk evidence.
    pub context: Vec<CounterEvidence>,
    /// Relevant evidence outside the initial request. A separate bounded
    /// reconciliation request uses these only when the initial answer needs it.
    pub counterevidence: Vec<CounterEvidence>,
    pub counterevidence_truncated: bool,
    pub comparison_digest: String,
    pub initial: ComparisonRequest,
    pub reconciliation: Option<ComparisonRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssessmentCoverage {
    pub eligible_rules: usize,
    pub candidate_pairs: usize,
    pub selected_comparisons: usize,
    pub unselected_pairs: usize,
    pub skipped_rules: Vec<String>,
    pub skipped_actions: Vec<String>,
    pub processing_limit_reached: bool,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssessmentPlan {
    pub input_revision: String,
    pub session_identity_digest: String,
    pub source_generation: i64,
    pub source_fingerprint: Option<String>,
    pub publication_fence: i64,
    pub activity_after_ms: Option<i64>,
    pub model_version: String,
    pub preparation_revision: u32,
    pub question_revision: u32,
    pub reducer_revision: u32,
    pub complete_input: bool,
    pub comparisons: Vec<CandidateComparison>,
    pub coverage: AssessmentCoverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCertainty {
    Likely,
    Possible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssessmentFinding {
    pub id: String,
    pub reference: RuleActionRef,
    pub certainty: FindingCertainty,
    pub conflict_probability: f64,
    pub applicability_probability: f64,
    pub exception_probability: f64,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRule {
    pub instruction_id: String,
    pub instruction_digest: String,
    pub rule_id: String,
    pub heading: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssessmentResult {
    pub input_revision: String,
    pub model_version: String,
    pub findings: Vec<AssessmentFinding>,
    pub pending_rules: Vec<PendingRule>,
    pub unassessed_comparisons: Vec<String>,
    pub coverage: AssessmentCoverage,
    pub request_count: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonJudgment {
    pub applicability: String,
    pub relationship: String,
    pub exception: String,
    pub reason: String,
    pub probabilities: BTreeMap<String, BTreeMap<String, f64>>,
}

/// The check-specific adapter for the reusable Jev execution contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct IgnoredInstructionsCheck;

/// Convert private normalized content into bounded shared context chunks and
/// attach this check's immutable rule/action plan.
pub fn build_jev_context(input: &AssessmentInput) -> Result<JevSessionContext, JevError> {
    let plan = build_assessment_plan(input.clone());
    let units = input
        .content
        .actions
        .iter()
        .map(|action| {
            Ok(JevContextUnit {
                event_id: action.reference.id.clone(),
                branch_id: format!("{}:{}", action.reference.thread_digest, action.turn_scope),
                timestamp_ms: action.timestamp_ms,
                context: serde_json::to_value(action)
                    .map_err(|_| JevError::RequestSerialization)?,
                truncated: action.truncated,
            })
        })
        .collect::<Result<Vec<_>, JevError>>()?;
    let chunks = chunk_context_units(&units, DEFAULT_CONTEXT_CHUNK_BYTES, 1);
    let mut limitations = plan.coverage.limitations.clone();
    if chunks
        .iter()
        .any(|chunk| !chunk.omitted_event_ids.is_empty())
    {
        limitations.push("oversized_context_events_omitted".to_owned());
    }
    let input_revision = plan.input_revision.clone();
    let session_identity = plan.session_identity_digest.clone();
    let check_context = serde_json::json!({"assessment_plan": plan});
    Ok(JevSessionContext {
        input_revision,
        session_identity,
        chunks,
        check_context,
        limitations,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStatus {
    Likely,
    Possible,
    NoIssue,
    Unassessed,
}

/// Prepare one immutable comparison per selected rule/action pair.
pub fn build_assessment_plan(input: AssessmentInput) -> AssessmentPlan {
    let content = &input.content;
    let mut comparisons = Vec::new();
    let mut skipped_rules = Vec::new();
    let mut skipped_actions = Vec::new();
    let mut limitations = content.limitations.clone();
    let rules: Vec<_> = content
        .instructions
        .iter()
        .flat_map(|instruction| {
            instruction
                .sections
                .iter()
                .filter(move |rule| {
                    rule.content_class == InstructionContentClass::RequirementCandidate
                })
                .map(move |rule| (instruction, rule))
        })
        .collect();
    let mut candidate_pairs = 0usize;
    let mut unselected_pairs = 0usize;
    let mut processing_limit_reached = false;

    for action in &content.actions {
        if !is_agent_action(action) {
            continue;
        }
        if input
            .activity_after_ms
            .zip(action.timestamp_ms)
            .is_some_and(|(watermark, timestamp)| timestamp < watermark)
        {
            continue;
        }
        if input.activity_after_ms.is_some() && action.timestamp_ms.is_none() {
            skipped_actions.push(action.reference.id.clone());
            limitations.push("action_activity_time_unknown".to_owned());
            continue;
        }
        for (instruction, rule) in &rules {
            candidate_pairs = candidate_pairs.saturating_add(1);
            if !rule.evaluable {
                skipped_rules.push(rule.id.clone());
                limitations.push("instruction_section_exceeds_limit".to_owned());
                continue;
            }
            if !relevant_pair(rule, action) {
                unselected_pairs = unselected_pairs.saturating_add(1);
                continue;
            }
            if comparisons.len() >= MAX_ASSESSMENT_CANDIDATES {
                processing_limit_reached = true;
                limitations.push("assessment_candidate_limit".to_owned());
                break;
            }
            let mut comparison = make_comparison(content, instruction, rule, action);
            while validate_jev_request(&comparison.initial.request).is_err()
                && !comparison.context.is_empty()
            {
                comparison.context.pop();
                comparison.initial = initial_request(&comparison);
                comparison.reconciliation = reconciliation_request(&comparison);
            }
            let initial_fits = validate_jev_request(&comparison.initial.request).is_ok();
            let reconciliation_fits = comparison
                .reconciliation
                .as_ref()
                .is_none_or(|request| validate_jev_request(&request.request).is_ok());
            if !initial_fits {
                skipped_actions.push(action.reference.id.clone());
                limitations.push("comparison_exceeds_request_limit".to_owned());
                continue;
            }
            if !reconciliation_fits {
                comparison.reconciliation = None;
                comparison.counterevidence_truncated = true;
                limitations.push("reconciliation_exceeds_request_limit".to_owned());
            }
            comparisons.push(comparison);
        }
        if processing_limit_reached {
            break;
        }
    }

    let mut unique_rules = BTreeSet::new();
    for (instruction, rule) in rules {
        unique_rules.insert((instruction.id.as_str(), rule.id.as_str()));
    }
    let mut input_revision = String::new();
    input_revision.push_str(&content.selected_input_digest);
    input_revision.push('\0');
    input_revision.push_str(&input.incarnation.to_string());
    input_revision.push('\0');
    input_revision.push_str(&input.source_generation.to_string());
    input_revision.push('\0');
    input_revision.push_str(&content.publication_fence.to_string());
    input_revision.push('\0');
    input_revision.push_str(&input.activity_after_ms.unwrap_or_default().to_string());
    input_revision.push('\0');
    input_revision.push_str(ASSESSMENT_MODEL);
    input_revision.push('\0');
    input_revision.push_str(&ASSESSMENT_PREPARATION_REVISION.to_string());
    input_revision.push('\0');
    input_revision.push_str(&ASSESSMENT_QUESTION_REVISION.to_string());
    input_revision.push('\0');
    input_revision.push_str(&ASSESSMENT_REDUCER_REVISION.to_string());
    let input_revision = sha256_hex(input_revision.as_bytes());
    skipped_rules.sort();
    skipped_rules.dedup();
    skipped_actions.sort();
    skipped_actions.dedup();
    limitations.sort();
    limitations.dedup();
    AssessmentPlan {
        input_revision,
        session_identity_digest: content.session_identity_digest.clone(),
        source_generation: input.source_generation,
        source_fingerprint: input.source_fingerprint,
        publication_fence: content.publication_fence,
        activity_after_ms: input.activity_after_ms,
        model_version: ASSESSMENT_MODEL.to_owned(),
        preparation_revision: ASSESSMENT_PREPARATION_REVISION,
        question_revision: ASSESSMENT_QUESTION_REVISION,
        reducer_revision: ASSESSMENT_REDUCER_REVISION,
        complete_input: content.complete,
        coverage: AssessmentCoverage {
            eligible_rules: unique_rules.len(),
            candidate_pairs,
            selected_comparisons: comparisons.len(),
            unselected_pairs,
            skipped_rules,
            skipped_actions,
            processing_limit_reached,
            limitations,
        },
        comparisons,
    }
}

impl JevCheck for IgnoredInstructionsCheck {
    type Result = AssessmentResult;

    fn id(&self) -> &'static str {
        "ignored_instructions"
    }

    fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan, JevError> {
        let assessment: AssessmentPlan =
            serde_json::from_value(context.check_context["assessment_plan"].clone())
                .map_err(|_| JevError::InvalidCheckContext)?;
        if assessment.input_revision != context.input_revision {
            return Err(JevError::InvalidCheckContext);
        }
        let mut work_items = Vec::with_capacity(assessment.comparisons.len());
        for comparison in &assessment.comparisons {
            let work_context = request_work_context(&comparison.initial.request)
                .ok_or(JevError::InvalidCheckPlan)?;
            let event_ids = std::iter::once(comparison.reference.action_id.clone())
                .chain(
                    comparison
                        .context
                        .iter()
                        .map(|event| event.action_id.clone()),
                )
                .chain(
                    comparison
                        .counterevidence
                        .iter()
                        .map(|event| event.action_id.clone()),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let context_chunk_ids = context
                .chunks
                .iter()
                .filter(|chunk| chunk.event_ids.iter().any(|id| event_ids.contains(id)))
                .map(|chunk| chunk.id.clone())
                .collect();
            work_items.push(JevWorkItem {
                id: comparison.id.clone(),
                context: work_context,
                context_chunk_ids,
                event_ids,
                questions: comparison_questions(),
            });
        }
        let check_data =
            serde_json::to_value(&assessment).map_err(|_| JevError::InvalidCheckPlan)?;
        Ok(JevCheckPlan {
            check_id: self.id().to_owned(),
            input_revision: assessment.input_revision.clone(),
            work_items,
            skipped_item_ids: assessment
                .coverage
                .skipped_rules
                .iter()
                .chain(&assessment.coverage.skipped_actions)
                .cloned()
                .collect(),
            coverage: JevCoverage {
                selected_items: assessment.coverage.selected_comparisons,
                skipped_items: assessment.coverage.skipped_rules.len()
                    + assessment.coverage.skipped_actions.len(),
                not_selected_items: assessment.coverage.unselected_pairs,
                processing_limit_reached: assessment.coverage.processing_limit_reached,
                limitations: assessment.coverage.limitations.clone(),
            },
            check_data,
        })
    }

    fn reconcile(
        &self,
        work_item: &JevWorkItem,
        initial_result: &JevWorkItemResult,
        context: &JevSessionContext,
    ) -> Result<Option<JevWorkItem>, JevError> {
        if initial_result.work_item_id != work_item.id {
            return Err(JevError::InvalidCheckPlan);
        }
        let assessment: AssessmentPlan =
            serde_json::from_value(context.check_context["assessment_plan"].clone())
                .map_err(|_| JevError::InvalidCheckContext)?;
        let Some(comparison) = assessment
            .comparisons
            .iter()
            .find(|comparison| comparison.id == work_item.id)
        else {
            return Err(JevError::InvalidCheckPlan);
        };
        let Some(judgment) = judgment(initial_result) else {
            return Err(JevError::InvalidCheckPlan);
        };
        if !should_reconcile(comparison, &judgment) {
            return Ok(None);
        }
        let Some(reconciliation) = comparison.reconciliation.as_ref() else {
            return Ok(None);
        };
        let item = JevWorkItem {
            id: format!("{}:reconciliation", comparison.id),
            context: request_work_context(&reconciliation.request)
                .ok_or(JevError::InvalidCheckPlan)?,
            context_chunk_ids: work_item.context_chunk_ids.clone(),
            event_ids: work_item.event_ids.clone(),
            questions: comparison_questions(),
        };
        if !pack_work_items(std::slice::from_ref(&item))
            .skipped_item_ids
            .is_empty()
        {
            return Err(JevError::InvalidCheckPlan);
        }
        Ok(Some(item))
    }

    fn reduce(
        &self,
        plan: &JevCheckPlan,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError> {
        if plan.check_id != self.id() {
            return Err(JevError::InvalidCheckPlan);
        }
        let assessment: AssessmentPlan = serde_json::from_value(plan.check_data.clone())
            .map_err(|_| JevError::InvalidCheckPlan)?;
        if assessment.input_revision != plan.input_revision {
            return Err(JevError::InvalidCheckPlan);
        }
        let results = results
            .iter()
            .map(|result| (result.work_item_id.clone(), result.clone()))
            .collect();
        Ok(reduce_assessment(&assessment, &results, complete))
    }
}

fn request_work_context(request: &JevRequest) -> Option<Value> {
    request
        .state
        .get("work_items")?
        .as_array()?
        .first()?
        .get("context")
        .cloned()
}

fn is_agent_action(action: &ContentAction) -> bool {
    matches!(action.turn_role.as_str(), "assistant" | "tool")
        && matches!(
            action.kind.as_str(),
            "assistant_text" | "tool_input" | "tool_result"
        )
}

fn relevant_pair(rule: &InstructionRuleSection, action: &ContentAction) -> bool {
    let rule_terms = meaningful_terms(&rule.text);
    let action_terms = meaningful_terms(&format!(
        "{} {} {}",
        action.text,
        action.tool_name.as_deref().unwrap_or_default(),
        action.tool_call_id.as_deref().unwrap_or_default()
    ));
    if rule_terms.iter().any(|term| action_terms.contains(term)) {
        return true;
    }
    let lower = rule.text.to_ascii_lowercase();
    [
        "always", "every", "must", "required", "before", "prior to", "never", "do not", "don't",
        "avoid",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn make_comparison(
    content: &SessionContentEvidence,
    instruction: &InstructionSnapshot,
    rule: &InstructionRuleSection,
    action: &ContentAction,
) -> CandidateComparison {
    let id = sha256_hex(format!("{}\0{}", rule.id, action.reference.id).as_bytes());
    let same_branch: Vec<_> = content
        .actions
        .iter()
        .filter(|other| {
            other.reference.thread_digest == action.reference.thread_digest
                && other.turn_scope == action.turn_scope
        })
        .collect();
    let mut chronological = same_branch.clone();
    chronological.sort_by_key(|event| event.timestamp_ms.unwrap_or(i64::MAX));
    let candidate_position = chronological
        .iter()
        .position(|event| event.reference.id == action.reference.id)
        .unwrap_or(0);
    let context_indices = context_indices(candidate_position, chronological.len());
    let context: Vec<CounterEvidence> = context_indices
        .into_iter()
        .filter_map(|index| chronological.get(index).copied())
        .take(MAX_CONTEXT_EVENTS)
        .map(counter_event)
        .collect();
    let rule_terms = meaningful_terms(&rule.text);
    let action_terms = meaningful_terms(&action.text);
    let mut counterevidence: Vec<_> = same_branch
        .into_iter()
        .filter(|other| {
            other.reference.id != action.reference.id
                && !context
                    .iter()
                    .any(|event| event.action_id == other.reference.id)
                && is_counterevidence(&rule_terms, &action_terms, other)
        })
        .map(counter_event)
        .collect();
    counterevidence.sort_by_key(|event| event.timestamp_ms.unwrap_or(i64::MAX));
    let counterevidence_truncated = counterevidence.len() > MAX_COUNTER_EVIDENCE;
    counterevidence.truncate(MAX_COUNTER_EVIDENCE);
    let reference = RuleActionRef {
        instruction_id: instruction.id.clone(),
        instruction_digest: instruction.digest.clone(),
        rule_id: rule.id.clone(),
        rule_heading: rule.heading.clone(),
        source: instruction.source.clone(),
        provenance: instruction.provenance,
        scope: instruction.scope,
        action_id: action.reference.id.clone(),
        action_stable: action.reference.stable,
    };
    let mut comparison = CandidateComparison {
        id: id.clone(),
        reference,
        rule_text: rule.text.clone(),
        action: counter_event(action),
        context,
        counterevidence,
        counterevidence_truncated,
        comparison_digest: String::new(),
        initial: placeholder_request(&id, RequestStage::Initial),
        reconciliation: None,
    };
    comparison.initial = initial_request(&comparison);
    comparison.reconciliation = reconciliation_request(&comparison);
    let initial_bytes = serde_json::to_vec(&comparison.initial.request).unwrap_or_default();
    let reconciliation_bytes = comparison
        .reconciliation
        .as_ref()
        .and_then(|request| serde_json::to_vec(&request.request).ok())
        .unwrap_or_default();
    comparison.comparison_digest = sha256_hex(
        [initial_bytes.as_slice(), reconciliation_bytes.as_slice()]
            .concat()
            .as_slice(),
    );
    comparison
}

fn context_indices(candidate: usize, length: usize) -> Vec<usize> {
    let mut indices = Vec::with_capacity(MAX_CONTEXT_EVENTS);
    for distance in 1..=MAX_CONTEXT_EVENTS {
        if let Some(previous) = candidate.checked_sub(distance) {
            indices.push(previous);
        }
        let next = candidate.saturating_add(distance);
        if next < length {
            indices.push(next);
        }
    }
    indices.sort_unstable();
    indices.truncate(MAX_CONTEXT_EVENTS);
    indices
}

fn is_counterevidence(
    rule_terms: &BTreeSet<String>,
    action_terms: &BTreeSet<String>,
    event: &ContentAction,
) -> bool {
    let event_text = format!(
        "{} {}",
        event.text,
        event.tool_name.as_deref().unwrap_or_default()
    );
    let terms = meaningful_terms(&event_text);
    let shares_subject = action_terms.iter().any(|term| terms.contains(term));
    let shares_rule = rule_terms.iter().any(|term| terms.contains(term));
    let lower = event_text.to_ascii_lowercase();
    let contains_counter_marker = [
        "approve",
        "approved",
        "approval",
        "exception",
        "except",
        "generated",
        "override",
        "test",
        "tests",
        "tested",
        "review",
        "commit",
        "merged",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    shares_subject || shares_rule || contains_counter_marker
}

fn meaningful_terms(text: &str) -> BTreeSet<String> {
    text.split(|character: char| {
        !character.is_alphanumeric() && character != '_' && character != '/'
    })
    .filter(|term| term.len() >= 3)
    .map(|term| {
        let mut term = term.to_ascii_lowercase();
        for suffix in ["ing", "ed", "al", "es", "s"] {
            if term.len() > suffix.len() + 3 && term.ends_with(suffix) {
                term.truncate(term.len() - suffix.len());
                break;
            }
        }
        term
    })
    .filter(|term| {
        !matches!(
            term.as_str(),
            "the"
                | "and"
                | "for"
                | "with"
                | "from"
                | "this"
                | "that"
                | "must"
                | "not"
                | "before"
                | "after"
                | "when"
                | "then"
                | "only"
                | "any"
                | "all"
        )
    })
    .collect()
}

fn counter_event(action: &ContentAction) -> CounterEvidence {
    CounterEvidence {
        action_id: action.reference.id.clone(),
        role: action.turn_role.clone(),
        kind: action.kind.clone(),
        timestamp_ms: action.timestamp_ms,
        tool_name: action.tool_name.clone(),
        text: action.text.clone(),
        truncated: action.truncated,
    }
}

fn initial_request(comparison: &CandidateComparison) -> ComparisonRequest {
    let request_id = format!("{}:initial", comparison.id);
    let state = json!({
        "instruction": {
            "section": comparison.reference.rule_heading.as_str(),
            "text": comparison.rule_text.as_str(),
            "provenance": comparison.reference.provenance,
            "scope": comparison.reference.scope,
            "source": comparison.reference.source.as_str(),
        },
        "candidate_action": &comparison.action,
        "nearby_context": &comparison.context,
        "assessment_limits": {
            "context_is_same_branch": true,
            "thinking_content_excluded": true,
            "action_reference": comparison.reference.action_id,
        }
    });
    comparison_request(&request_id, RequestStage::Initial, state)
}

fn reconciliation_request(comparison: &CandidateComparison) -> Option<ComparisonRequest> {
    if comparison.counterevidence.is_empty() {
        return None;
    }
    let request_id = format!("{}:reconciliation", comparison.id);
    let state = json!({
        "instruction": {
            "section": comparison.reference.rule_heading.as_str(),
            "text": comparison.rule_text.as_str(),
            "provenance": comparison.reference.provenance,
            "scope": comparison.reference.scope,
            "source": comparison.reference.source.as_str(),
        },
        "candidate_action": &comparison.action,
        "nearby_context": &comparison.context,
        "cross_chunk_counterevidence": &comparison.counterevidence,
        "assessment_limits": {
            "counterevidence_truncated": comparison.counterevidence_truncated,
            "same_branch_only": true,
            "thinking_content_excluded": true,
            "candidate_action_reference": comparison.reference.action_id,
        }
    });
    Some(comparison_request(
        &request_id,
        RequestStage::Reconciliation,
        state,
    ))
}

fn placeholder_request(id: &str, stage: RequestStage) -> ComparisonRequest {
    let request_id = match stage {
        RequestStage::Initial => format!("{id}:initial"),
        RequestStage::Relevance => format!("{id}:relevance"),
        RequestStage::Reconciliation => format!("{id}:reconciliation"),
    };
    comparison_request(&request_id, stage, Value::Null)
}

fn comparison_request(request_id: &str, stage: RequestStage, state: Value) -> ComparisonRequest {
    let work_item = JevWorkItem {
        id: request_id.to_owned(),
        context: state.clone(),
        context_chunk_ids: Vec::new(),
        event_ids: Vec::new(),
        questions: comparison_questions(),
    };
    let packed = pack_work_items(std::slice::from_ref(&work_item))
        .batches
        .into_iter()
        .next();
    let (request, bytes) = if let Some(batch) = packed {
        let bytes = batch.serialized_bytes;
        (batch.request, bytes)
    } else {
        let questions = work_item.questions.clone();
        let request = JevRequest {
            model: ASSESSMENT_MODEL.to_owned(),
            state: json!({"work_items": [{"id": request_id, "context": state}]}),
            questions,
        };
        let bytes = serde_json::to_vec(&request).map_or(usize::MAX, |value| value.len());
        (request, bytes)
    };
    let digest = serde_json::to_vec(&request)
        .map(|value| sha256_hex(&value))
        .unwrap_or_default();
    ComparisonRequest {
        request_id: request_id.to_owned(),
        stage,
        digest,
        request,
        serialized_bytes: bytes,
    }
}

fn comparison_questions() -> BTreeMap<String, JevQuestion> {
    BTreeMap::from([
        (
            QUESTION_APPLICABILITY.to_owned(),
            choice_question(
                "Does the supplied instruction section apply to this action in the shown scope?",
                [
                    (
                        "applies",
                        "The instruction applies to this action and scope.",
                    ),
                    (
                        "not_applicable",
                        "The instruction does not apply to this action or scope.",
                    ),
                    (
                        "uncertain",
                        "The supplied evidence does not establish applicability.",
                    ),
                ],
            ),
        ),
        (
            QUESTION_RELATIONSHIP.to_owned(),
            choice_question(
                "How does the candidate action relate to the applicable instruction, using the exact evidence and event order shown?",
                [
                    (
                        "conflict",
                        "The candidate action conflicts with a requirement or performs a forbidden action.",
                    ),
                    (
                        "follows",
                        "The candidate action follows the instruction, including any supported prerequisite or exception.",
                    ),
                    (
                        "unrelated",
                        "The instruction does not govern the candidate action.",
                    ),
                    (
                        "insufficient_evidence",
                        "The supplied evidence cannot establish the relationship.",
                    ),
                ],
            ),
        ),
        (
            QUESTION_EXCEPTION.to_owned(),
            choice_question(
                "Does the supplied evidence show an explicit exception or approval that applies to this candidate action?",
                [
                    (
                        "applies",
                        "An explicit exception or approval applies to this action and scope.",
                    ),
                    (
                        "none_observed",
                        "No applicable exception or approval appears in the supplied evidence.",
                    ),
                    (
                        "uncertain",
                        "The evidence does not establish whether an exception or approval applies.",
                    ),
                ],
            ),
        ),
        (
            QUESTION_REASON.to_owned(),
            choice_question(
                "What is the best-supported reason for the relationship judgment?",
                [
                    (
                        "conflicting_action",
                        "The observed action directly conflicts with the instruction.",
                    ),
                    (
                        "unmet_prerequisite",
                        "A required earlier action is not supported before the shown milestone.",
                    ),
                    (
                        "response_mismatch",
                        "The response does not follow a communication requirement.",
                    ),
                    (
                        "instruction_conflict",
                        "Two applicable instructions conflict and the evidence cannot resolve them.",
                    ),
                    (
                        "insufficient_context",
                        "The relevant history, scope, or source evidence is missing.",
                    ),
                    (
                        "no_conflict",
                        "The supplied evidence does not show a conflict.",
                    ),
                ],
            ),
        ),
    ])
}

fn choice_question<const N: usize>(instructions: &str, criteria: [(&str, &str); N]) -> JevQuestion {
    JevQuestion::Choice {
        instructions: json!(instructions),
        criteria: criteria
            .into_iter()
            .map(|(key, value)| (key.to_owned(), json!(value)))
            .collect(),
    }
}

/// Validate the complete set of returned typed answers before any answer can
/// affect a finding.
pub fn validate_response(response: &JevResponse, request: &JevRequest) -> Result<(), JevError> {
    validate_jev_response(response, request)?;
    if request.questions.len() != 4
        || request
            .questions
            .values()
            .any(|question| !matches!(question, JevQuestion::Choice { .. }))
        || response
            .answers
            .values()
            .any(|answer| !matches!(answer, JevAnswer::Choice { .. }))
    {
        return Err(JevError::ResponseAnswerTypeMismatch);
    }
    Ok(())
}

/// Reduce all completed comparison stages into one result for the immutable
/// selected range. Partial request completion cannot produce a clean result.
pub fn reduce_assessment(
    plan: &AssessmentPlan,
    responses: &BTreeMap<String, JevWorkItemResult>,
    processing_complete: bool,
) -> AssessmentResult {
    let mut findings = Vec::new();
    let mut pending_rules = Vec::new();
    let mut unassessed_comparisons = Vec::new();
    let mut request_count = 0u32;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut seen = BTreeSet::new();
    let mut seen_requests = BTreeSet::new();
    for comparison in &plan.comparisons {
        let Some(initial) = responses.get(&comparison.id) else {
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
        let initial_judgment = judgment(initial);
        let Some(initial_judgment) = initial_judgment else {
            unassessed_comparisons.push(comparison.id.clone());
            continue;
        };
        let mut final_judgment = initial_judgment;
        if should_reconcile(comparison, &final_judgment) {
            if comparison.reconciliation.is_none() {
                unassessed_comparisons.push(comparison.id.clone());
                continue;
            }
            let reconciliation_id = format!("{}:reconciliation", comparison.id);
            let Some(reconciliation) = responses.get(&reconciliation_id) else {
                unassessed_comparisons.push(comparison.id.clone());
                continue;
            };
            record_usage(
                reconciliation,
                &mut seen_requests,
                &mut request_count,
                &mut input_tokens,
                &mut output_tokens,
            );
            let Some(reconciled) = judgment(reconciliation) else {
                unassessed_comparisons.push(comparison.id.clone());
                continue;
            };
            final_judgment = reconciled;
        }
        if !seen.insert(comparison.id.clone()) {
            continue;
        }
        match classify_comparison(plan, comparison, &final_judgment) {
            RuleStatus::Likely | RuleStatus::Possible => {
                let certainty = if classify_comparison(plan, comparison, &final_judgment)
                    == RuleStatus::Likely
                {
                    FindingCertainty::Likely
                } else {
                    FindingCertainty::Possible
                };
                let mut finding_limits = comparison_limitations(plan, comparison);
                finding_limits.sort();
                finding_limits.dedup();
                findings.push(AssessmentFinding {
                    id: sha256_hex(
                        format!(
                            "{}\0{}",
                            comparison.reference.rule_id, comparison.reference.action_id
                        )
                        .as_bytes(),
                    ),
                    reference: comparison.reference.clone(),
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
                    exception_probability: probability(
                        &final_judgment,
                        QUESTION_EXCEPTION,
                        "applies",
                    ),
                    limitations: finding_limits,
                });
            }
            RuleStatus::NoIssue => {}
            RuleStatus::Unassessed => unassessed_comparisons.push(comparison.id.clone()),
        }
    }
    add_pending_obligations(plan, &mut pending_rules);
    findings.sort_by(|left, right| left.id.cmp(&right.id));
    unassessed_comparisons.sort();
    unassessed_comparisons.dedup();
    let mut coverage = plan.coverage.clone();
    coverage.limitations.sort();
    coverage.limitations.dedup();
    if !plan.complete_input {
        coverage
            .limitations
            .push("source_evidence_is_partial".to_owned());
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
        findings,
        pending_rules,
        unassessed_comparisons,
        coverage,
        request_count,
        input_tokens,
        output_tokens,
    }
}

fn record_usage(
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

fn judgment(result: &JevWorkItemResult) -> Option<ComparisonJudgment> {
    let probabilities = result
        .answers
        .iter()
        .filter_map(|(local_id, answer)| match answer {
            JevAnswer::Choice { probabilities, .. } => {
                Some((local_id.clone(), probabilities.clone()))
            }
            _ => None,
        })
        .collect();
    Some(ComparisonJudgment {
        applicability: selected(result, QUESTION_APPLICABILITY)?,
        relationship: selected(result, QUESTION_RELATIONSHIP)?,
        exception: selected(result, QUESTION_EXCEPTION)?,
        reason: selected(result, QUESTION_REASON)?,
        probabilities,
    })
}

fn selected(result: &JevWorkItemResult, question: &str) -> Option<String> {
    match result.answers.get(question)? {
        JevAnswer::Choice { choice, .. } => Some(choice.clone()),
        JevAnswer::Noul { .. } | JevAnswer::Score { .. } => None,
    }
}

fn probability(judgment: &ComparisonJudgment, question: &str, option: &str) -> f64 {
    judgment
        .probabilities
        .get(question)
        .and_then(|distribution| distribution.get(option))
        .copied()
        .unwrap_or_default()
}

fn should_reconcile(comparison: &CandidateComparison, judgment: &ComparisonJudgment) -> bool {
    comparison.reconciliation.is_some()
        && (judgment.relationship == "conflict"
            || judgment.relationship == "insufficient_evidence"
            || judgment.exception == "uncertain")
}

fn classify_comparison(
    plan: &AssessmentPlan,
    comparison: &CandidateComparison,
    judgment: &ComparisonJudgment,
) -> RuleStatus {
    let conflict = probability(judgment, QUESTION_RELATIONSHIP, "conflict");
    let applies = probability(judgment, QUESTION_APPLICABILITY, "applies");
    let exception = probability(judgment, QUESTION_EXCEPTION, "applies");
    if judgment.applicability == "not_applicable" && applies < LIKELY_THRESHOLD {
        return RuleStatus::NoIssue;
    }
    if judgment.relationship != "conflict" || conflict < POSSIBLE_THRESHOLD {
        return if judgment.relationship == "follows" || judgment.relationship == "unrelated" {
            RuleStatus::NoIssue
        } else {
            RuleStatus::Unassessed
        };
    }
    if judgment.exception == "applies" && exception >= LIKELY_THRESHOLD {
        return RuleStatus::NoIssue;
    }
    if deterministic_prerequisite_satisfied(comparison) {
        return RuleStatus::NoIssue;
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
        && !(judgment.exception == "uncertain" || exception >= LIKELY_THRESHOLD);
    if likely_evidence {
        RuleStatus::Likely
    } else if applies >= POSSIBLE_THRESHOLD {
        RuleStatus::Possible
    } else {
        let _ = plan;
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
    if comparison.counterevidence_truncated {
        limitations.push("counterevidence_truncated".to_owned());
    }
    if !plan.complete_input {
        limitations.push("source_evidence_is_partial".to_owned());
    }
    limitations
}

fn deterministic_prerequisite_satisfied(comparison: &CandidateComparison) -> bool {
    let rule = comparison.rule_text.to_ascii_lowercase();
    if !(rule.contains("before") || rule.contains("prior to")) {
        return false;
    }
    let milestone = if rule.contains("commit") {
        "commit"
    } else if rule.contains("merge") {
        "merge"
    } else if rule.contains("release") {
        "release"
    } else {
        return false;
    };
    if !comparison
        .action
        .text
        .to_ascii_lowercase()
        .contains(milestone)
    {
        return false;
    }
    let prerequisite = if rule.contains("test") {
        "test"
    } else if rule.contains("approv") {
        "approv"
    } else if rule.contains("review") {
        "review"
    } else {
        return false;
    };
    comparison
        .context
        .iter()
        .chain(comparison.counterevidence.iter())
        .any(|event| {
            event
                .timestamp_ms
                .zip(comparison.action.timestamp_ms)
                .is_some_and(|(event_time, action_time)| {
                    event_time < action_time
                        && event.text.to_ascii_lowercase().contains(prerequisite)
                })
        })
}

fn add_pending_obligations(plan: &AssessmentPlan, pending: &mut Vec<PendingRule>) {
    for comparison in &plan.comparisons {
        let text = comparison.rule_text.to_ascii_lowercase();
        let is_eventual = [
            "eventually",
            "at the end",
            "before finishing",
            "by completion",
        ]
        .iter()
        .any(|phrase| text.contains(phrase));
        if is_eventual {
            pending.push(PendingRule {
                instruction_id: comparison.reference.instruction_id.clone(),
                instruction_digest: comparison.reference.instruction_digest.clone(),
                rule_id: comparison.reference.rule_id.clone(),
                heading: comparison.reference.rule_heading.clone(),
                reason: "completion_boundary_not_observed".to_owned(),
            });
        }
    }
    pending.sort_by(|left, right| left.rule_id.cmp(&right.rule_id));
    pending.dedup_by(|left, right| left.rule_id == right.rule_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::ignored_instructions::snapshot_from_text;

    fn event(id: &str, timestamp_ms: i64, role: &str, thread: &str, text: &str) -> ContentAction {
        ContentAction {
            reference: super::super::evidence::ContentEventReference {
                id: id.to_owned(),
                source_key_digest: "source".to_owned(),
                thread_digest: thread.to_owned(),
                turn_index: timestamp_ms as u64,
                native_record_id: Some(id.to_owned()),
                part_index: 0,
                stable: true,
            },
            timestamp_ms: Some(timestamp_ms),
            turn_role: role.to_owned(),
            turn_scope: "main".to_owned(),
            authority: if role == "assistant" { "agent" } else { "user" }.to_owned(),
            kind: "assistant_text".to_owned(),
            text: text.to_owned(),
            tool_name: None,
            tool_call_id: None,
            truncated: false,
        }
    }

    fn input(actions: Vec<ContentAction>, rule_text: &str) -> AssessmentInput {
        let instruction = snapshot_from_text(
            "AGENTS.md",
            rule_text.to_owned(),
            InstructionProvenance::RecordedInjection,
            InstructionScope::Project,
        )
        .unwrap();
        AssessmentInput {
            content: SessionContentEvidence {
                session_identity_digest: "session".to_owned(),
                source_format: crate::analysis::SourceFormat::ClaudeJsonl,
                publication_fence: 4,
                selected_input_digest: "selected-input".to_owned(),
                actions,
                instructions: vec![instruction],
                complete: true,
                limitations: Vec::new(),
                excluded_thinking_parts: 0,
            },
            activity_after_ms: None,
            source_generation: 2,
            source_fingerprint: Some("fingerprint".to_owned()),
            incarnation: 1,
        }
    }

    fn candidate(
        id: &str,
        rule_id: &str,
        rule_text: &str,
        action_id: &str,
        action_text: &str,
        timestamp_ms: i64,
    ) -> CandidateComparison {
        let reference = RuleActionRef {
            instruction_id: "instruction".to_owned(),
            instruction_digest: "instruction-digest".to_owned(),
            rule_id: rule_id.to_owned(),
            rule_heading: "Requirements".to_owned(),
            source: "AGENTS.md".to_owned(),
            provenance: InstructionProvenance::RecordedInjection,
            scope: InstructionScope::Project,
            action_id: action_id.to_owned(),
            action_stable: true,
        };
        let action = CounterEvidence {
            action_id: action_id.to_owned(),
            role: "assistant".to_owned(),
            kind: "assistant_text".to_owned(),
            timestamp_ms: Some(timestamp_ms),
            tool_name: None,
            text: action_text.to_owned(),
            truncated: false,
        };
        let mut comparison = CandidateComparison {
            id: id.to_owned(),
            reference,
            rule_text: rule_text.to_owned(),
            action,
            context: Vec::new(),
            counterevidence: Vec::new(),
            counterevidence_truncated: false,
            comparison_digest: String::new(),
            initial: placeholder_request(id, RequestStage::Initial),
            reconciliation: None,
        };
        comparison.initial = initial_request(&comparison);
        comparison.reconciliation = reconciliation_request(&comparison);
        comparison
    }

    fn plan(comparisons: Vec<CandidateComparison>) -> AssessmentPlan {
        AssessmentPlan {
            input_revision: "input-revision".to_owned(),
            session_identity_digest: "session".to_owned(),
            source_generation: 2,
            source_fingerprint: Some("fingerprint".to_owned()),
            publication_fence: 4,
            activity_after_ms: None,
            model_version: ASSESSMENT_MODEL.to_owned(),
            preparation_revision: ASSESSMENT_PREPARATION_REVISION,
            question_revision: ASSESSMENT_QUESTION_REVISION,
            reducer_revision: ASSESSMENT_REDUCER_REVISION,
            complete_input: true,
            coverage: AssessmentCoverage {
                eligible_rules: 1,
                candidate_pairs: comparisons.len(),
                selected_comparisons: comparisons.len(),
                unselected_pairs: 0,
                skipped_rules: Vec::new(),
                skipped_actions: Vec::new(),
                processing_limit_reached: false,
                limitations: Vec::new(),
            },
            comparisons,
        }
    }

    fn choice_answer(selected: &str, options: &[&str]) -> JevAnswer {
        let remaining = options.len().saturating_sub(1).max(1) as f64;
        let probabilities = options
            .iter()
            .map(|option| {
                (
                    (*option).to_owned(),
                    if *option == selected {
                        0.97
                    } else {
                        0.03 / remaining
                    },
                )
            })
            .collect();
        JevAnswer::Choice {
            choice: selected.to_owned(),
            probabilities,
            confidence: 0.97,
        }
    }

    fn judgment_result(
        work_item_id: &str,
        relationship: &str,
        exception: &str,
    ) -> JevWorkItemResult {
        let answers = BTreeMap::from([
            (
                QUESTION_APPLICABILITY.to_owned(),
                choice_answer("applies", &["applies", "not_applicable", "uncertain"]),
            ),
            (
                QUESTION_RELATIONSHIP.to_owned(),
                choice_answer(
                    relationship,
                    &["conflict", "follows", "unrelated", "insufficient_evidence"],
                ),
            ),
            (
                QUESTION_EXCEPTION.to_owned(),
                choice_answer(exception, &["applies", "none_observed", "uncertain"]),
            ),
            (
                QUESTION_REASON.to_owned(),
                choice_answer(
                    "conflicting_action",
                    &[
                        "conflicting_action",
                        "unmet_prerequisite",
                        "response_mismatch",
                        "instruction_conflict",
                        "insufficient_context",
                        "no_conflict",
                    ],
                ),
            ),
        ]);
        JevWorkItemResult {
            request_id: format!("request-{work_item_id}"),
            work_item_id: work_item_id.to_owned(),
            answers,
            model: ASSESSMENT_MODEL.to_owned(),
            usage: crate::analysis::jev::JevUsage {
                input_tokens: 100,
                output_tokens: 1,
            },
        }
    }

    fn reduce_one(
        comparison: CandidateComparison,
        results: Vec<JevWorkItemResult>,
    ) -> AssessmentResult {
        reduce_assessment(
            &plan(vec![comparison]),
            &results
                .into_iter()
                .map(|result| (result.work_item_id.clone(), result))
                .collect(),
            true,
        )
    }

    #[test]
    fn approval_in_another_context_chunk_reconciles_the_candidate() {
        let mut actions = vec![event(
            "approval",
            1,
            "user",
            "branch",
            "The user approved installing the dependency.",
        )];
        for index in 0..8 {
            actions.push(event(
                &format!("context-{index}"),
                2 + index,
                "user",
                "branch",
                "Unrelated conversation context.",
            ));
        }
        actions.push(event(
            "install",
            20,
            "assistant",
            "branch",
            "Installed the dependency.",
        ));
        let built = build_assessment_plan(input(
            actions,
            "Get approval before installing a dependency.",
        ));
        let comparison = built
            .comparisons
            .iter()
            .find(|comparison| comparison.reference.action_id == "install")
            .unwrap();
        assert!(
            !comparison
                .context
                .iter()
                .any(|event| event.action_id == "approval")
        );
        assert!(
            comparison
                .counterevidence
                .iter()
                .any(|event| event.action_id == "approval")
        );
        assert!(comparison.reconciliation.is_some());

        let result = reduce_assessment(
            &built,
            &BTreeMap::from([
                (
                    comparison.id.clone(),
                    judgment_result(&comparison.id, "conflict", "none_observed"),
                ),
                (
                    format!("{}:reconciliation", comparison.id),
                    judgment_result(
                        &format!("{}:reconciliation", comparison.id),
                        "follows",
                        "applies",
                    ),
                ),
            ]),
            true,
        );
        assert!(result.findings.is_empty());
    }

    #[test]
    fn an_exception_near_a_split_boundary_removes_the_conflict() {
        let mut comparison = candidate(
            "generated-file",
            "generated-rule",
            "Do not edit generated files.",
            "edit-generated",
            "Edited a generated file.",
            20,
        );
        comparison.counterevidence.push(CounterEvidence {
            action_id: "exception".to_owned(),
            role: "user".to_owned(),
            kind: "assistant_text".to_owned(),
            timestamp_ms: Some(19),
            tool_name: None,
            text: "Exception: edit this generated file for the required snapshot update."
                .to_owned(),
            truncated: false,
        });
        comparison.reconciliation = reconciliation_request(&comparison);
        let result = reduce_assessment(
            &plan(vec![comparison.clone()]),
            &BTreeMap::from([
                (
                    comparison.id.clone(),
                    judgment_result(&comparison.id, "conflict", "none_observed"),
                ),
                (
                    format!("{}:reconciliation", comparison.id),
                    judgment_result(
                        &format!("{}:reconciliation", comparison.id),
                        "conflict",
                        "applies",
                    ),
                ),
            ]),
            true,
        );
        assert!(result.findings.is_empty());
    }

    #[test]
    fn deterministic_event_order_accepts_tests_before_commit_only() {
        let mut before = candidate(
            "commit-before-tests",
            "test-before-commit",
            "Run tests before committing changes.",
            "commit",
            "Committed the changes.",
            10,
        );
        before.context.push(CounterEvidence {
            action_id: "tests".to_owned(),
            role: "tool".to_owned(),
            kind: "tool_result".to_owned(),
            timestamp_ms: Some(9),
            tool_name: Some("test".to_owned()),
            text: "Focused tests passed.".to_owned(),
            truncated: false,
        });
        let accepted = reduce_one(
            before.clone(),
            vec![judgment_result(&before.id, "conflict", "none_observed")],
        );
        assert!(accepted.findings.is_empty());

        before.context[0].timestamp_ms = Some(11);
        let rejected = reduce_one(
            before.clone(),
            vec![judgment_result(&before.id, "conflict", "none_observed")],
        );
        assert_eq!(rejected.findings.len(), 1);
    }

    #[test]
    fn a_later_compliant_action_does_not_cancel_an_earlier_conflict() {
        let first = candidate(
            "first-action",
            "forbidden-command",
            "Do not run the release command.",
            "action-one",
            "Ran the release command.",
            10,
        );
        let second = candidate(
            "later-action",
            "forbidden-command",
            "Do not run the release command.",
            "action-two",
            "Used the documented validation command.",
            20,
        );
        let result = reduce_assessment(
            &plan(vec![first.clone(), second.clone()]),
            &BTreeMap::from([
                (
                    first.id.clone(),
                    judgment_result(&first.id, "conflict", "none_observed"),
                ),
                (
                    second.id.clone(),
                    judgment_result(&second.id, "follows", "none_observed"),
                ),
            ]),
            true,
        );
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].reference.action_id, "action-one");
    }

    #[test]
    fn overlapping_comparisons_publish_one_rule_action_occurrence() {
        let comparison = candidate(
            "overlap-one",
            "forbidden-command",
            "Do not run the release command.",
            "same-action",
            "Ran the release command.",
            10,
        );
        let duplicate = CandidateComparison {
            id: comparison.id.clone(),
            ..comparison.clone()
        };
        let result = reduce_assessment(
            &plan(vec![comparison.clone(), duplicate]),
            &BTreeMap::from([(
                comparison.id.clone(),
                judgment_result(&comparison.id, "conflict", "none_observed"),
            )]),
            true,
        );
        assert_eq!(result.findings.len(), 1);
    }

    #[test]
    fn missing_history_keeps_the_candidate_unassessed() {
        let comparison = candidate(
            "missing-history",
            "test-before-commit",
            "Run tests before committing.",
            "commit",
            "Committed the changes.",
            10,
        );
        let mut incomplete_plan = plan(vec![comparison.clone()]);
        incomplete_plan.complete_input = false;
        incomplete_plan
            .coverage
            .limitations
            .push("tool_history_gap".to_owned());
        let result = reduce_assessment(
            &incomplete_plan,
            &BTreeMap::from([(
                comparison.id.clone(),
                judgment_result(&comparison.id, "insufficient_evidence", "uncertain"),
            )]),
            true,
        );
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons, vec![comparison.id]);
        assert!(
            result
                .coverage
                .limitations
                .contains(&"source_evidence_is_partial".to_owned())
        );
    }

    #[test]
    fn eventual_obligations_remain_pending_without_a_completion_boundary() {
        let comparison = candidate(
            "eventual-tests",
            "eventual-tests",
            "Run tests eventually before completion.",
            "change",
            "Changed the implementation.",
            10,
        );
        let result = reduce_one(
            comparison,
            vec![judgment_result(
                "eventual-tests",
                "follows",
                "none_observed",
            )],
        );
        assert_eq!(result.pending_rules.len(), 1);
        assert_eq!(
            result.pending_rules[0].reason,
            "completion_boundary_not_observed"
        );
    }

    #[test]
    fn a_changed_rule_is_a_distinct_finding_identity() {
        let old = candidate(
            "old-rule-comparison",
            "rule-old",
            "Do not run the release command.",
            "release-action",
            "Ran the release command.",
            10,
        );
        let new = candidate(
            "new-rule-comparison",
            "rule-new",
            "Ask for approval before running the release command.",
            "release-action",
            "Ran the release command.",
            10,
        );
        let result = reduce_assessment(
            &plan(vec![old.clone(), new.clone()]),
            &BTreeMap::from([
                (
                    old.id.clone(),
                    judgment_result(&old.id, "conflict", "none_observed"),
                ),
                (
                    new.id.clone(),
                    judgment_result(&new.id, "conflict", "none_observed"),
                ),
            ]),
            true,
        );
        assert_eq!(result.findings.len(), 2);
        assert_ne!(result.findings[0].id, result.findings[1].id);
    }

    #[test]
    fn reconciliation_requests_remain_bounded_and_keep_exact_citations() {
        let mut comparison = candidate(
            "large-reconciliation",
            "approval-rule",
            "Get approval before adding the dependency.",
            "install",
            "Added the dependency.",
            100,
        );
        comparison.counterevidence = (0..8)
            .map(|index| CounterEvidence {
                action_id: format!("counter-{index}"),
                role: "user".to_owned(),
                kind: "assistant_text".to_owned(),
                timestamp_ms: Some(index),
                tool_name: None,
                text: "approval context ".repeat(20),
                truncated: false,
            })
            .collect();
        comparison.reconciliation = reconciliation_request(&comparison);
        let request = comparison.reconciliation.as_ref().unwrap();
        assert!(validate_jev_request(&request.request).is_ok());
        let state_ids =
            request.request.state["work_items"][0]["context"]["cross_chunk_counterevidence"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|event| event["action_id"].as_str())
                .collect::<BTreeSet<_>>();
        assert_eq!(state_ids.len(), 8);
    }

    #[test]
    fn context_from_another_branch_does_not_count_as_an_exception() {
        let actions = vec![
            event(
                "other-branch-approval",
                1,
                "user",
                "branch-other",
                "Approved installing the dependency.",
            ),
            event(
                "branch-install",
                2,
                "assistant",
                "branch-main",
                "Installed the dependency.",
            ),
        ];
        let built = build_assessment_plan(input(
            actions,
            "Get approval before installing a dependency.",
        ));
        let comparison = built
            .comparisons
            .iter()
            .find(|comparison| comparison.reference.action_id == "branch-install")
            .unwrap();
        assert!(
            !comparison
                .counterevidence
                .iter()
                .any(|event| event.action_id == "other-branch-approval")
        );
    }

    #[test]
    fn processing_limits_are_visible_even_when_selected_work_finishes() {
        let mut limited = plan(Vec::new());
        limited.coverage.processing_limit_reached = true;
        limited
            .coverage
            .limitations
            .push("candidate_limit".to_owned());
        let result = reduce_assessment(&limited, &BTreeMap::new(), true);
        assert!(result.findings.is_empty());
        assert!(result.coverage.processing_limit_reached);
        assert!(
            result
                .coverage
                .limitations
                .contains(&"candidate_limit".to_owned())
        );
    }
}
