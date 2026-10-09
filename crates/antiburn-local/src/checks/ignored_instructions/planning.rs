//! Bounded construction of ignored-instruction assessment plans.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use super::assessment::{
    ASSESSMENT_CHUNKING_REVISION, ASSESSMENT_MODEL, ASSESSMENT_PROJECTION_REVISION,
    ASSESSMENT_QUESTION_REVISION, ASSESSMENT_REDUCER_REVISION, AssessmentCoverage, AssessmentInput,
    AssessmentPlan, CandidateComparison, CounterEvidence, INPUT_SELECTION,
    InstructionSourceCoverage, MAX_ASSESSMENT_CANDIDATES, MAX_SAMPLED_COMPARISONS_PER_PASS,
    RuleActionRef,
};
use super::evidence::{ContentAction, content_action_digest};
use super::instructions::{
    InstructionContentClass, InstructionRuleSection, InstructionSnapshot, sha256_hex,
};
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::exact_facts::ExactActionFacts;
use crate::analysis::jev::{JevError, JevSessionContext};

const MAX_CONTEXT_EVENTS: usize = 3;
pub(super) const MAX_COUNTER_EVIDENCE: usize = 4;
pub(super) const MAX_RULE_TEXT_BYTES: usize = 2 * 1024;
pub(super) const MAX_ACTION_TEXT_BYTES: usize = 1024;
const MAX_CONTEXT_TEXT_BYTES: usize = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EvidenceTextLimits {
    rule: usize,
    action: usize,
    context: usize,
}

impl EvidenceTextLimits {
    const LEGACY: Self = Self {
        rule: MAX_RULE_TEXT_BYTES,
        action: MAX_ACTION_TEXT_BYTES,
        context: MAX_CONTEXT_TEXT_BYTES,
    };

    fn from_capabilities(capabilities: &ModelCapabilities) -> Self {
        // Use one UTF-8 byte per token for this allocation bound. The packer
        // checks the serialized request with the selected token estimator.
        let budget = [
            capabilities.request_body_bytes.value,
            capabilities.state_and_longest_question_bytes.value,
            capabilities.usable_state_tokens(),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(0)
        .saturating_sub(8 * 1024);
        // Eight targets share one action and reserve space for earlier context.
        let unit = usize::try_from(budget / 80).unwrap_or(usize::MAX);
        // Keep the minimum evidence when limits are too small. The packer
        // rejects an oversized item instead of removing required context.
        Self {
            rule: unit.saturating_mul(4).clamp(MAX_RULE_TEXT_BYTES, 8 * 1024),
            action: unit
                .saturating_mul(8)
                .clamp(MAX_ACTION_TEXT_BYTES, 32 * 1024),
            context: unit.clamp(MAX_CONTEXT_TEXT_BYTES, 2 * 1024),
        }
    }
}
type RuleRange<'a> = (
    &'a InstructionSnapshot,
    &'a InstructionRuleSection,
    usize,
    usize,
);
type ActionRange<'a> = (&'a ContentAction, usize, usize);
const SELECTOR_REVISION: u32 = 2;

#[cfg(test)]
std::thread_local! {
    static PREPARATION_COUNTS: std::cell::Cell<(usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
    static REFERENCE_SELECTION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(super) fn build_reference_context(
    input: &AssessmentInput,
    ledger: &SamplingLedger,
    capabilities: &ModelCapabilities,
) -> Result<JevSessionContext, JevError> {
    REFERENCE_SELECTION.with(|enabled| enabled.set(true));
    let result = build_jev_context_with_capabilities(input, ledger, capabilities);
    REFERENCE_SELECTION.with(|enabled| enabled.set(false));
    result
}

#[cfg(test)]
pub(super) fn build_reference_plan(
    input: AssessmentInput,
    ledger: &SamplingLedger,
) -> AssessmentPlan {
    REFERENCE_SELECTION.with(|enabled| enabled.set(true));
    let result = build_assessment_plan_with_sampling(input, ledger);
    REFERENCE_SELECTION.with(|enabled| enabled.set(false));
    result
}

#[cfg(test)]
pub(super) fn take_preparation_counts() -> (usize, usize, usize) {
    PREPARATION_COUNTS.with(|counts| counts.replace((0, 0, 0)))
}

#[cfg(test)]
pub(super) fn take_selection_counts() -> (usize, usize, usize) {
    selection::take_selection_counts()
}

/// Completed comparisons and actions from earlier reviews. Keep the ledger
/// unchanged while advancing pages of the current pass.
#[derive(Debug, Clone, Default)]
pub struct SamplingLedger {
    pub comparison_ids: BTreeSet<String>,
    pub known_action_ids: BTreeSet<String>,
    /// Add coordinates only after exact dependency validation. Identity alone
    /// does not prove that the saved judgment still applies.
    pub comparisons: BTreeMap<String, SavedComparison>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComparisonCoordinate {
    pub instruction_id: String,
    pub rule_id: String,
    pub rule_range: (usize, usize),
    pub action_range: (usize, usize),
}

#[derive(Debug, Clone)]
pub struct SavedComparison {
    pub id: String,
    pub action_id: String,
    pub instruction_digest: String,
    pub coordinate: Option<ComparisonCoordinate>,
}

impl From<&CandidateComparison> for SavedComparison {
    fn from(comparison: &CandidateComparison) -> Self {
        Self {
            id: comparison.id.clone(),
            action_id: comparison.reference.action_id.clone(),
            instruction_digest: comparison.reference.instruction_digest.clone(),
            coordinate: Some(ComparisonCoordinate {
                instruction_id: comparison.reference.instruction_id.clone(),
                rule_id: comparison.reference.rule_id.clone(),
                rule_range: (comparison.rule_text_start, comparison.rule_text_end),
                action_range: (comparison.action_text_start, comparison.action_text_end),
            }),
        }
    }
}

/// Project source content once and reuse it for validation and request preparation.
pub struct PreparedAssessmentInput {
    input: AssessmentInput,
    ranking: Option<SelectedComparisons>,
}

struct SelectedComparisons {
    limits: EvidenceTextLimits,
    comparison_ids: BTreeSet<String>,
    known_action_ids: BTreeSet<String>,
    reviewed: BTreeSet<(usize, usize)>,
    coordinates: Vec<(usize, usize)>,
}

impl PreparedAssessmentInput {
    pub fn new(input: &AssessmentInput) -> Self {
        Self {
            input: selected_input(input),
            ranking: None,
        }
    }

    /// Accept content already projected with `INPUT_SELECTION` by the source reader.
    pub fn from_selected_input(input: AssessmentInput) -> Self {
        Self {
            input,
            ranking: None,
        }
    }

    /// Reconstruct saved coordinates independently of sampling order.
    /// This path does not serialize context or construct provider requests.
    pub fn dependency_comparisons(
        &mut self,
        saved: &[SavedComparison],
        capabilities: &ModelCapabilities,
    ) -> AssessmentPlan {
        let mut plan = build_plan(
            &self.input,
            &SamplingLedger::default(),
            EvidenceTextLimits::from_capabilities(capabilities),
            Some(saved),
            &mut self.ranking,
        );
        plan.model_version.clone_from(&capabilities.model);
        for comparison in &mut plan.comparisons {
            comparison.prerequisite_episode =
                Some(super::PrerequisiteContextPolicy::CoherentEpisode.select(
                    comparison,
                    &self.input.content.actions,
                    capabilities,
                    plan.complete_input,
                ));
        }
        plan
    }

    pub fn build_context(
        &mut self,
        ledger: &SamplingLedger,
        capabilities: &ModelCapabilities,
    ) -> Result<JevSessionContext, JevError> {
        let mut plan = build_plan(
            &self.input,
            ledger,
            EvidenceTextLimits::from_capabilities(capabilities),
            None,
            &mut self.ranking,
        );
        plan.model_version.clone_from(&capabilities.model);
        build_context(
            &self.input,
            plan,
            super::PrerequisiteContextPolicy::CoherentEpisode,
        )
    }
}

/// Attach the bounded rule/action plan without duplicating its private content.
pub fn build_jev_context(input: &AssessmentInput) -> Result<JevSessionContext, JevError> {
    build_jev_context_with_sampling(input, &SamplingLedger::default())
}

pub fn build_jev_context_with_sampling(
    input: &AssessmentInput,
    ledger: &SamplingLedger,
) -> Result<JevSessionContext, JevError> {
    let selected_input = selected_input(input);
    build_context(
        &selected_input,
        build_plan(
            &selected_input,
            ledger,
            EvidenceTextLimits::LEGACY,
            None,
            &mut None,
        ),
        super::PrerequisiteContextPolicy::CoherentEpisode,
    )
}

/// Apply model limits before sampling. Later preparation cannot recover text
/// that the selected ranges omit.
pub fn build_jev_context_with_capabilities(
    input: &AssessmentInput,
    ledger: &SamplingLedger,
    capabilities: &ModelCapabilities,
) -> Result<JevSessionContext, JevError> {
    build_jev_context_with_context_policy(
        input,
        ledger,
        capabilities,
        super::PrerequisiteContextPolicy::CoherentEpisode,
    )
}

/// Compare context policies with the same projection, rules, questions, and reducer.
/// Product preparation always uses the coherent episode policy.
pub fn build_jev_context_with_context_policy(
    input: &AssessmentInput,
    ledger: &SamplingLedger,
    capabilities: &ModelCapabilities,
    policy: super::PrerequisiteContextPolicy,
) -> Result<JevSessionContext, JevError> {
    let selected_input = selected_input(input);
    let mut plan = build_plan(
        &selected_input,
        ledger,
        EvidenceTextLimits::from_capabilities(capabilities),
        None,
        &mut None,
    );
    plan.model_version.clone_from(&capabilities.model);
    build_context(&selected_input, plan, policy)
}

fn selected_input(input: &AssessmentInput) -> AssessmentInput {
    #[cfg(test)]
    PREPARATION_COUNTS.with(|counts| {
        let (projections, rankings, comparisons) = counts.get();
        counts.set((projections + 1, rankings, comparisons));
    });
    AssessmentInput {
        content: super::select_session_content(&input.content, INPUT_SELECTION),
        prior_history_complete: input.prior_history_complete,
        activity_after_ms: input.activity_after_ms,
        boundary_positions: input.boundary_positions.clone(),
        source_generation: input.source_generation,
        source_fingerprint: input.source_fingerprint.clone(),
        incarnation: input.incarnation,
        comparison_after: input.comparison_after.clone(),
    }
}

fn build_context(
    input: &AssessmentInput,
    mut plan: AssessmentPlan,
    policy: super::PrerequisiteContextPolicy,
) -> Result<JevSessionContext, JevError> {
    plan.input_revision = sha256_hex(
        &serde_json::to_vec(&(&plan.input_revision, policy))
            .map_err(|_| JevError::InvalidCheckContext)?,
    );
    let input_revision = plan.input_revision.clone();
    let session_identity = plan.session_identity_digest.clone();
    let limitations = plan.coverage.limitations.clone();
    let episode_actions = &input.content.actions;
    let check_context = serde_json::json!({"assessment_plan": plan, "episode_actions": episode_actions, "prerequisite_context_policy": policy, "incremental_identity": {
        "prerequisite_context_policy": policy,
        "incarnation": input.incarnation,
        "source_format": input.content.source_format,
        "activity_after_ms": input.activity_after_ms,
        "boundary_positions": input.boundary_positions,
    }});
    let evidence_store = super::evidence::selected_evidence_store(
        &input.content.actions,
        INPUT_SELECTION,
        input.content.publication_fence,
    )?;
    let reference_fields = serde_json::to_value(&input.content.instructions)
        .map_err(|_| JevError::InvalidCheckContext)?;
    let reference_identity = sha256_hex(
        &serde_json::to_vec(&(
            input
                .content
                .instructions
                .iter()
                .map(|snapshot| (&snapshot.id, &snapshot.digest))
                .collect::<Vec<_>>(),
            &input_revision,
        ))
        .map_err(|_| JevError::InvalidCheckContext)?,
    );
    Ok(JevSessionContext {
        input_revision: input_revision.clone(),
        session_identity,
        check_context,
        limitations,
        evidence_store,
        reference_snapshots: vec![crate::analysis::jev::JevReferenceSnapshot {
            kind: "instruction_snapshot".to_owned(),
            identity: reference_identity,
            revision: input_revision,
            fields: reference_fields,
        }],
    })
}

/// Add saved candidates to a continuation page and include earlier events from
/// that page. Keep the source identities local to the check context.
pub fn extend_jev_context_with_history(
    context: &mut JevSessionContext,
    carried_comparisons: &mut [CandidateComparison],
    page_actions: &[ContentAction],
    prior_history_complete: bool,
) -> Result<(), JevError> {
    extend_context_history(
        context,
        carried_comparisons,
        page_actions,
        prior_history_complete,
        EvidenceTextLimits::LEGACY,
    )
}

/// Apply the selected model limits to context from an older content page.
pub fn extend_jev_context_with_history_and_capabilities(
    context: &mut JevSessionContext,
    carried_comparisons: &mut [CandidateComparison],
    page_actions: &[ContentAction],
    prior_history_complete: bool,
    capabilities: &ModelCapabilities,
) -> Result<(), JevError> {
    extend_context_history(
        context,
        carried_comparisons,
        page_actions,
        prior_history_complete,
        EvidenceTextLimits::from_capabilities(capabilities),
    )
}

fn extend_context_history(
    context: &mut JevSessionContext,
    carried_comparisons: &mut [CandidateComparison],
    page_actions: &[ContentAction],
    prior_history_complete: bool,
    text_limits: EvidenceTextLimits,
) -> Result<(), JevError> {
    if carried_comparisons.is_empty() {
        return Ok(());
    }
    let mut assessment: AssessmentPlan =
        serde_json::from_value(context.check_context["assessment_plan"].clone())
            .map_err(|_| JevError::InvalidCheckContext)?;
    if assessment.input_revision != context.input_revision {
        return Err(JevError::InvalidCheckContext);
    }
    let mut comparison_ids = assessment
        .comparisons
        .iter()
        .map(|comparison| comparison.id.clone())
        .collect::<BTreeSet<_>>();
    let mut revision_material = assessment.input_revision.clone();
    for carried in carried_comparisons {
        *carried = extend_history(carried, page_actions, prior_history_complete, text_limits);
        revision_material.push('\0');
        revision_material.push_str(&carried.id);
        revision_material.push('\0');
        revision_material
            .push_str(&serde_json::to_string(&carried).map_err(|_| JevError::InvalidCheckContext)?);
        if comparison_ids.insert(carried.id.clone()) {
            assessment.current_action_digests.insert(
                carried.reference.action_id.clone(),
                carried.reference.action_digest.clone(),
            );
            assessment.current_rule_ids.insert((
                carried.reference.instruction_id.clone(),
                carried.reference.instruction_digest.clone(),
                carried.reference.rule_id.clone(),
            ));
            assessment.comparisons.push(carried.clone());
        } else if let Some(existing) = assessment
            .comparisons
            .iter_mut()
            .find(|value| value.id == carried.id)
        {
            *existing = carried.clone();
        }
    }
    let mut actions: Vec<ContentAction> =
        serde_json::from_value(context.check_context["episode_actions"].clone())
            .map_err(|_| JevError::InvalidCheckContext)?;
    let page = super::SessionContentEvidence {
        actions: page_actions.to_vec(),
        session_identity_digest: context.session_identity.clone(),
        source_format: serde_json::from_value(
            context.check_context["incremental_identity"]["source_format"].clone(),
        )
        .map_err(|_| JevError::InvalidCheckContext)?,
        publication_fence: 0,
        selected_input_digest: String::new(),
        instructions: Vec::new(),
        complete: prior_history_complete,
        limitations: Vec::new(),
        excluded_thinking_parts: 0,
        field_availability: Vec::new(),
    };
    actions.extend(super::select_session_content(&page, INPUT_SELECTION).actions);
    actions.sort_by(|left, right| left.reference.id.cmp(&right.reference.id));
    actions.dedup_by(|left, right| left.reference.id == right.reference.id);
    revision_material.push_str(
        &serde_json::to_string(&(&actions, prior_history_complete))
            .map_err(|_| JevError::InvalidCheckContext)?,
    );
    assessment.input_revision = sha256_hex(revision_material.as_bytes());
    context
        .input_revision
        .clone_from(&assessment.input_revision);
    context.limitations = assessment.coverage.limitations.clone();
    context.check_context["assessment_plan"] =
        serde_json::to_value(assessment).map_err(|_| JevError::InvalidCheckContext)?;
    context.check_context["episode_actions"] =
        serde_json::to_value(actions).map_err(|_| JevError::InvalidCheckContext)?;
    Ok(())
}

/// Prepare one immutable comparison per selected rule/action pair.
pub fn build_assessment_plan(input: AssessmentInput) -> AssessmentPlan {
    build_assessment_plan_with_sampling(input, &SamplingLedger::default())
}

pub fn build_assessment_plan_with_sampling(
    input: AssessmentInput,
    ledger: &SamplingLedger,
) -> AssessmentPlan {
    build_plan(&input, ledger, EvidenceTextLimits::LEGACY, None, &mut None)
}

/// Build the comparison inventory with model limits and local memory bounds.
pub fn build_assessment_plan_with_capabilities(
    input: AssessmentInput,
    ledger: &SamplingLedger,
    capabilities: &ModelCapabilities,
) -> AssessmentPlan {
    let mut plan = build_plan(
        &input,
        ledger,
        EvidenceTextLimits::from_capabilities(capabilities),
        None,
        &mut None,
    );
    plan.model_version.clone_from(&capabilities.model);
    plan
}

fn build_plan(
    input: &AssessmentInput,
    ledger: &SamplingLedger,
    text_limits: EvidenceTextLimits,
    dependencies: Option<&[SavedComparison]>,
    ranking: &mut Option<SelectedComparisons>,
) -> AssessmentPlan {
    let content = &input.content;
    let mut limitations = content.limitations.clone();
    let skipped_rules = content
        .instructions
        .iter()
        .flat_map(|instruction| {
            instruction
                .sections
                .iter()
                .filter(|rule| {
                    rule.content_class == InstructionContentClass::RequirementCandidate
                        && !rule.evaluable
                })
                .map(move |rule| format!("{}:{}", instruction.id, rule.id))
        })
        .collect::<Vec<_>>();
    if !skipped_rules.is_empty() {
        limitations.push("instruction_rule_not_evaluable".to_owned());
    }
    let skipped_actions = content
        .actions
        .iter()
        .filter(|action| is_agent_action(action) && !has_selected_action_content(action))
        .map(|action| action.reference.id.clone())
        .collect::<Vec<_>>();
    if !skipped_actions.is_empty() {
        limitations.push("empty_selected_action_content".to_owned());
    }
    let rule_groups: Vec<Vec<_>> = content
        .instructions
        .iter()
        .map(|instruction| {
            instruction
                .sections
                .iter()
                .filter(move |rule| {
                    rule.content_class == InstructionContentClass::RequirementCandidate
                })
                .map(move |rule| (instruction, rule))
                .filter(|(_, rule)| rule.evaluable)
                .flat_map(|(instruction, rule)| {
                    text_ranges(&rule.text, text_limits.rule).into_iter().map(
                        move |(text_start, text_end)| (instruction, rule, text_start, text_end),
                    )
                })
                .collect()
        })
        .collect();
    let mut rules = Vec::new();
    for index in 0..rule_groups.iter().map(Vec::len).max().unwrap_or_default() {
        for group in &rule_groups {
            if let Some(rule) = group.get(index) {
                rules.push(*rule);
            }
        }
    }
    let actions: Vec<_> = content
        .actions
        .iter()
        .filter(|action| {
            is_agent_action(action)
                && input
                    .activity_after_ms
                    .is_none_or(|watermark| match action.timestamp_ms {
                        Some(timestamp) => timestamp >= watermark,
                        None => input.boundary_positions.iter().any(|(source, position)| {
                            (source == "*"
                                || sha256_hex(source.as_bytes())
                                    == action.reference.source_key_digest)
                                && action.reference.turn_index > *position
                        }),
                    })
        })
        .filter(|action| has_selected_action_content(action))
        .flat_map(|action| {
            (if atomic_command(action) {
                vec![(0, action.text.len())]
            } else {
                text_ranges(&action.text, text_limits.action)
            })
            .into_iter()
            .map(move |(text_start, text_end)| (action, text_start, text_end))
        })
        .collect();
    let mut action_digests = BTreeMap::new();
    let mut action_terms_by_id = BTreeMap::new();
    for action in &content.actions {
        action_terms_by_id
            .entry(action.reference.id.clone())
            .or_insert_with(|| action_meaningful_terms(action));
    }
    for (action, _, _) in &actions {
        action_digests
            .entry(action.reference.id.clone())
            .or_insert_with(|| content_action_digest(action));
    }
    let branch_order = branch_order_index(&content.actions);
    let rule_terms = rules
        .iter()
        .map(|(_, rule, start, end)| {
            meaningful_terms(rule.text.get(*start..*end).unwrap_or(&rule.text))
        })
        .collect::<Vec<_>>();
    let evaluable_rules = rules.as_slice();
    let candidate_pairs = rules.len().saturating_mul(actions.len());
    let reviewed = resolve_saved_coordinates(
        &rules,
        &actions,
        ledger
            .comparisons
            .values()
            .filter(|saved| ledger.comparison_ids.contains(&saved.id)),
    );
    let previously_sampled = reviewed.len();
    let selection = select_comparisons(
        evaluable_rules,
        &actions,
        ledger,
        if dependencies.is_some() {
            None
        } else {
            input.comparison_after.as_deref()
        },
        &ComparisonIndex {
            text_limits,
            prior_history_complete: input.prior_history_complete,
            branch_order: &branch_order.actions_by_branch,
            branch_positions: &branch_order.positions_by_action,
            action_digests: &action_digests,
            rule_terms: &rule_terms,
            action_terms: &action_terms_by_id,
            reviewed: &reviewed,
        },
        dependencies,
        ranking,
    );
    let comparisons = selection.comparisons;
    let mut rules_by_source = BTreeMap::<String, BTreeSet<(String, String)>>::new();
    let mut ranges_by_source = BTreeMap::<String, usize>::new();
    for instruction in &content.instructions {
        rules_by_source
            .entry(instruction.source.clone())
            .or_default();
        ranges_by_source
            .entry(instruction.source.clone())
            .or_default();
    }
    for (instruction, rule, _, _) in evaluable_rules.iter() {
        rules_by_source
            .entry(instruction.source.clone())
            .or_default()
            .insert((instruction.id.clone(), rule.id.clone()));
        *ranges_by_source
            .entry(instruction.source.clone())
            .or_default() += actions.len();
    }
    let mut instruction_sources = ranges_by_source
        .into_iter()
        .map(|(source, candidate_pairs)| InstructionSourceCoverage {
            eligible_rules: rules_by_source.get(&source).map_or(0, BTreeSet::len),
            candidate_pairs,
            selected_comparisons: 0,
            source,
        })
        .collect::<Vec<_>>();
    for comparison in &comparisons {
        if let Some(source) = instruction_sources
            .iter_mut()
            .find(|source| source.source == comparison.reference.source)
        {
            source.selected_comparisons = source.selected_comparisons.saturating_add(1);
        }
    }
    let current_action_digests = action_digests;
    let current_rule_ids = rules
        .iter()
        .map(|(instruction, rule, _, _)| {
            (
                instruction.id.clone(),
                instruction.digest.clone(),
                rule.id.clone(),
            )
        })
        .collect();
    let unselected_pairs = candidate_pairs.saturating_sub(previously_sampled + selection.page_end);
    let sampled_pass = unselected_pairs > 0;
    if sampled_pass {
        limitations.push("sampled_candidate_selection".to_owned());
    }
    let next_comparison_cursor = selection.next_cursor;

    let mut unique_rules = BTreeSet::new();
    for (instruction, rule, _, _) in rules {
        unique_rules.insert((instruction.id.as_str(), rule.id.as_str()));
    }
    let mut revision_hasher = Sha256::new();
    revision_hasher.update(content.selected_input_digest.as_bytes());
    revision_hasher.update(b"\0");
    revision_hasher.update(input.incarnation.to_string().as_bytes());
    revision_hasher.update(input.source_generation.to_string().as_bytes());
    revision_hasher.update(b"\0");
    revision_hasher.update(content.publication_fence.to_string().as_bytes());
    revision_hasher.update(b"\0");
    revision_hasher.update(
        input
            .activity_after_ms
            .unwrap_or_default()
            .to_string()
            .as_bytes(),
    );
    revision_hasher.update(b"\0");
    revision_hasher.update(if input.prior_history_complete {
        "prior-history-complete"
    } else {
        "prior-history-incomplete"
    });
    for (source, position) in &input.boundary_positions {
        revision_hasher.update(source.as_bytes());
        revision_hasher.update(position.to_string().as_bytes());
    }
    revision_hasher.update(b"\0");
    revision_hasher.update(ASSESSMENT_MODEL.as_bytes());
    revision_hasher.update(b"\0");
    revision_hasher.update(ASSESSMENT_PROJECTION_REVISION.to_string().as_bytes());
    revision_hasher.update(b"\0");
    revision_hasher.update(ASSESSMENT_CHUNKING_REVISION.to_string().as_bytes());
    revision_hasher.update(b"\0");
    revision_hasher.update(ASSESSMENT_QUESTION_REVISION.to_string().as_bytes());
    revision_hasher.update(b"\0");
    revision_hasher.update(ASSESSMENT_REDUCER_REVISION.to_string().as_bytes());
    revision_hasher.update(b"\0selector:");
    revision_hasher.update(text_limits.rule.to_string().as_bytes());
    revision_hasher.update(b":");
    revision_hasher.update(text_limits.action.to_string().as_bytes());
    revision_hasher.update(b":");
    revision_hasher.update(text_limits.context.to_string().as_bytes());
    revision_hasher.update(SELECTOR_REVISION.to_string().as_bytes());
    for id in &ledger.comparison_ids {
        revision_hasher.update(id.as_bytes());
    }
    for id in &ledger.known_action_ids {
        revision_hasher.update(id.as_bytes());
    }
    if let Some(cursor) = &input.comparison_after {
        revision_hasher.update(cursor.as_bytes());
    }
    let input_revision = digest_to_hex(revision_hasher.finalize().as_slice());
    limitations.sort();
    limitations.dedup();
    AssessmentPlan {
        input_revision,
        observable_obligations: BTreeMap::new(),
        read_request_orders: super::assessment::exact_read_orders(
            &comparisons,
            content,
            input.prior_history_complete,
        ),
        earlier_read_only_actions: super::assessment::earlier_read_only_actions(
            &comparisons,
            content,
        ),
        session_identity_digest: content.session_identity_digest.clone(),
        source_generation: input.source_generation,
        source_fingerprint: input.source_fingerprint.clone(),
        publication_fence: content.publication_fence,
        activity_after_ms: input.activity_after_ms,
        model_version: ASSESSMENT_MODEL.to_owned(),
        projection_revision: ASSESSMENT_PROJECTION_REVISION,
        chunking_revision: ASSESSMENT_CHUNKING_REVISION,
        question_revision: ASSESSMENT_QUESTION_REVISION,
        reducer_revision: ASSESSMENT_REDUCER_REVISION,
        complete_input: content.complete,
        next_comparison_cursor,
        current_action_digests,
        current_rule_ids,
        coverage: AssessmentCoverage {
            eligible_rules: unique_rules.len(),
            candidate_pairs,
            selected_comparisons: comparisons.len(),
            unselected_pairs,
            skipped_rules,
            skipped_actions,
            processing_limit_reached: false,
            sampled_pass,
            selector_revision: SELECTOR_REVISION,
            limitations,
            reassessed_comparison_ids: Vec::new(),
            reassessed_rule_ids: Vec::new(),
            reassessed_finding_ids: Vec::new(),
            instruction_sources,
        },
        comparisons,
    }
}

struct ComparisonIndex<'a> {
    text_limits: EvidenceTextLimits,
    prior_history_complete: bool,
    branch_order: &'a BTreeMap<(String, String), Vec<&'a ContentAction>>,
    branch_positions: &'a BTreeMap<(String, String, String), usize>,
    action_digests: &'a BTreeMap<String, String>,
    rule_terms: &'a [BTreeSet<String>],
    action_terms: &'a BTreeMap<String, BTreeSet<String>>,
    reviewed: &'a BTreeSet<(usize, usize)>,
}

fn select_comparisons<'a>(
    rules: &[RuleRange<'a>],
    actions: &[ActionRange<'a>],
    ledger: &SamplingLedger,
    cursor: Option<&str>,
    index: &ComparisonIndex<'a>,
    dependencies: Option<&[SavedComparison]>,
    ranking: &mut Option<SelectedComparisons>,
) -> SampleSelection {
    if let Some(saved) = dependencies {
        let coordinates = resolve_saved_coordinates(rules, actions, saved.iter());
        return SampleSelection {
            comparisons: coordinates
                .into_iter()
                .map(|(rule, action)| comparison_at(rules, actions, rule, action, index))
                .collect(),
            ..SampleSelection::default()
        };
    }
    if rules.is_empty() || actions.is_empty() {
        return SampleSelection::default();
    }
    #[cfg(test)]
    if REFERENCE_SELECTION.with(|enabled| enabled.get()) {
        return select_comparisons_reference(rules, actions, ledger, cursor, index, None);
    }
    let needs_selection = ranking.as_ref().is_none_or(|cached| {
        cached.limits != index.text_limits
            || cached.comparison_ids != ledger.comparison_ids
            || cached.known_action_ids != ledger.known_action_ids
            || cached.reviewed != *index.reviewed
    });
    if needs_selection {
        #[cfg(test)]
        PREPARATION_COUNTS.with(|counts| {
            let (projections, rankings, comparisons) = counts.get();
            counts.set((projections, rankings + 1, comparisons));
        });
        *ranking = Some(SelectedComparisons {
            limits: index.text_limits,
            comparison_ids: ledger.comparison_ids.clone(),
            known_action_ids: ledger.known_action_ids.clone(),
            reviewed: index.reviewed.clone(),
            coordinates: selection::select_coordinates(
                rules,
                actions,
                ledger,
                index.reviewed,
                index,
            ),
        });
    }
    let chosen = &ranking
        .as_ref()
        .expect("initialize comparison selection")
        .coordinates;
    let offset = cursor
        .and_then(|value| value.strip_prefix("sample:"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let end = offset
        .saturating_add(MAX_ASSESSMENT_CANDIDATES)
        .min(chosen.len());
    SampleSelection {
        page_end: end,
        next_cursor: (end < chosen.len()).then(|| format!("sample:{end}")),
        comparisons: chosen
            .get(offset..end)
            .unwrap_or_default()
            .iter()
            .map(|&(rule, action)| comparison_at(rules, actions, rule, action, index))
            .collect(),
    }
}

#[path = "selection.rs"]
mod selection;

fn resolve_saved_coordinates<'a>(
    rules: &[RuleRange<'a>],
    actions: &[ActionRange<'a>],
    saved: impl Iterator<Item = &'a SavedComparison>,
) -> BTreeSet<(usize, usize)> {
    let mut rules_by_digest = BTreeMap::<&str, Vec<usize>>::new();
    let mut actions_by_id = BTreeMap::<&str, Vec<usize>>::new();
    let mut rules_by_coordinate = BTreeMap::new();
    let mut actions_by_coordinate = BTreeMap::new();
    for (index, (instruction, rule, start, end)) in rules.iter().enumerate() {
        rules_by_coordinate.insert(
            (
                instruction.digest.as_str(),
                instruction.id.as_str(),
                rule.id.as_str(),
                *start,
                *end,
            ),
            index,
        );
        rules_by_digest
            .entry(&instruction.digest)
            .or_default()
            .push(index);
    }
    for (index, (action, start, end)) in actions.iter().enumerate() {
        actions_by_coordinate.insert((action.reference.id.as_str(), *start, *end), index);
        actions_by_id
            .entry(&action.reference.id)
            .or_default()
            .push(index);
    }
    let mut coordinates = BTreeSet::new();
    for saved in saved {
        if let Some(coordinate) = &saved.coordinate {
            let rule_key = (
                saved.instruction_digest.as_str(),
                coordinate.instruction_id.as_str(),
                coordinate.rule_id.as_str(),
                coordinate.rule_range.0,
                coordinate.rule_range.1,
            );
            let action_key = (
                saved.action_id.as_str(),
                coordinate.action_range.0,
                coordinate.action_range.1,
            );
            if let (Some(&rule_index), Some(&action_index)) = (
                rules_by_coordinate.get(&rule_key),
                actions_by_coordinate.get(&action_key),
            ) {
                let (instruction, rule, start, end) = rules[rule_index];
                let (action, action_start, action_end) = actions[action_index];
                if comparison_identity(
                    instruction,
                    rule,
                    action,
                    (start, end),
                    (action_start, action_end),
                ) == saved.id
                {
                    coordinates.insert((rule_index, action_index));
                }
            }
            continue;
        }
        let Some(rule_indices) = rules_by_digest.get(saved.instruction_digest.as_str()) else {
            continue;
        };
        let Some(action_indices) = actions_by_id.get(saved.action_id.as_str()) else {
            continue;
        };
        for &rule_index in rule_indices {
            let (instruction, rule, start, end) = rules[rule_index];
            if saved.coordinate.as_ref().is_some_and(|coordinate| {
                coordinate.instruction_id != instruction.id
                    || coordinate.rule_id != rule.id
                    || coordinate.rule_range != (start, end)
            }) {
                continue;
            }
            for &action_index in action_indices {
                let (action, action_start, action_end) = actions[action_index];
                if saved
                    .coordinate
                    .as_ref()
                    .is_some_and(|coordinate| coordinate.action_range != (action_start, action_end))
                {
                    continue;
                }
                if comparison_identity(
                    instruction,
                    rule,
                    action,
                    (start, end),
                    (action_start, action_end),
                ) == saved.id
                {
                    coordinates.insert((rule_index, action_index));
                }
            }
        }
    }
    coordinates
}

#[cfg(test)]
fn select_comparisons_reference<'a>(
    rules: &[RuleRange<'a>],
    actions: &[ActionRange<'a>],
    ledger: &SamplingLedger,
    cursor: Option<&str>,
    index: &ComparisonIndex<'a>,
    dependency_ids: Option<&BTreeSet<String>>,
) -> SampleSelection {
    if rules.is_empty() || actions.is_empty() {
        return SampleSelection::default();
    }
    {
        #[cfg(test)]
        PREPARATION_COUNTS.with(|counts| {
            let (projections, rankings, comparisons) = counts.get();
            counts.set((projections, rankings + 1, comparisons));
        });
        let action_terms = actions
            .iter()
            .map(|(action, start, end)| {
                let mut terms = meaningful_terms(&action.text[*start..*end]);
                terms.extend(index.action_terms[&action.reference.id].iter().cloned());
                if let Some(fields) = &action.normalized_fields {
                    for value in fields.values.values() {
                        terms.extend(meaningful_terms(value));
                    }
                }
                terms
            })
            .collect::<Vec<_>>();
        let mut document_frequency = BTreeMap::<&str, usize>::new();
        for terms in &action_terms {
            for term in terms {
                *document_frequency.entry(term).or_default() += 1;
            }
        }
        let action_paths = actions
            .iter()
            .map(|(action, _, _)| {
                ExactActionFacts::from_selected(
                    action.tool_name.as_deref(),
                    action.normalized_fields.as_ref(),
                )
                .paths
            })
            .collect::<Vec<_>>();
        let mut ranked = Vec::with_capacity(rules.len());
        for (rule_index, (_, rule, start, end)) in rules.iter().enumerate() {
            let terms = &index.rule_terms[rule_index];
            let rule_text = &rule.text[*start..*end];
            let mut scores = (0..actions.len())
                .map(|action_index| {
                    let (action, _, _) = actions[action_index];
                    let overlap = terms
                        .iter()
                        .filter(|term| action_terms[action_index].contains(*term))
                        .map(|term| 1000 / (1 + document_frequency[term.as_str()]))
                        .sum::<usize>();
                    let tool_match = action.tool_name.as_deref().is_some_and(|name| {
                        rule_text
                            .to_ascii_lowercase()
                            .contains(&name.to_ascii_lowercase())
                    });
                    let path_match = action_paths[action_index]
                        .iter()
                        .any(|path| rule_text.contains(path));
                    let risk = usize::from(
                        ["never", "Never", "Do not", "must not"]
                            .iter()
                            .any(|word| rule_text.contains(word)),
                    ) * usize::from(action.kind == "tool_input");
                    let fresh = action_index * 16 / actions.len();
                    let score = overlap.saturating_mul(8)
                        + usize::from(tool_match) * 800
                        + usize::from(path_match) * 1200
                        + risk * 12
                        + fresh;
                    (score, action_index)
                })
                .collect::<Vec<_>>();
            scores.sort_unstable_by(|left, right| {
                right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1))
            });
            ranked.push(scores);
        }
        select_ranked_reference(
            rules,
            actions,
            ledger,
            cursor,
            index,
            dependency_ids,
            &ranked,
        )
    }
}

#[cfg(test)]
fn select_ranked_reference<'a>(
    rules: &[RuleRange<'a>],
    actions: &[ActionRange<'a>],
    ledger: &SamplingLedger,
    cursor: Option<&str>,
    index: &ComparisonIndex<'a>,
    dependency_ids: Option<&BTreeSet<String>>,
    ranked: &[Vec<(usize, usize)>],
) -> SampleSelection {
    let limit = MAX_SAMPLED_COMPARISONS_PER_PASS.min(rules.len().saturating_mul(actions.len()));
    let mut chosen = Vec::with_capacity(limit);
    let mut used = BTreeSet::new();
    let mut action_counts = vec![0usize; actions.len()];
    // Complete the new-action tier before drawing from the older backlog.
    for new_only in [true, false] {
        if new_only && ledger.known_action_ids.is_empty() {
            continue;
        }
        let eligible = |rule_index: usize, action_index: usize, used: &BTreeSet<(usize, usize)>| {
            let (instruction, rule, start, end) = rules[rule_index];
            let (action, action_start, action_end) = actions[action_index];
            !used.contains(&(rule_index, action_index))
                && (!new_only || !ledger.known_action_ids.contains(&action.reference.id))
                && (ledger.comparison_ids.is_empty()
                    || !ledger.comparison_ids.contains(&comparison_identity(
                        instruction,
                        rule,
                        action,
                        (start, end),
                        (action_start, action_end),
                    )))
        };
        // Reserve one relevant candidate and one low-overlap probe per rule.
        for probe in [false, true] {
            for (rule_index, scores) in ranked.iter().enumerate() {
                if chosen.len() == limit {
                    break;
                }
                if let Some(&(_, action_index)) = scores.iter().find(|(score, action_index)| {
                    (*score < 16) == probe
                        && action_counts[*action_index] < 3
                        && eligible(rule_index, *action_index, &used)
                }) {
                    used.insert((rule_index, action_index));
                    action_counts[action_index] += 1;
                    chosen.push((rule_index, action_index));
                }
            }
        }
        let mut positions = vec![0usize; rules.len()];
        while chosen.len() < limit {
            let mut advanced = false;
            for (rule_index, scores) in ranked.iter().enumerate() {
                while let Some(&(_, action_index)) = scores.get(positions[rule_index]) {
                    positions[rule_index] += 1;
                    if eligible(rule_index, action_index, &used) {
                        used.insert((rule_index, action_index));
                        action_counts[action_index] += 1;
                        chosen.push((rule_index, action_index));
                        advanced = true;
                        break;
                    }
                }
                if chosen.len() == limit {
                    break;
                }
            }
            if !advanced {
                break;
            }
        }
    }
    let offset = cursor
        .and_then(|value| value.strip_prefix("sample:"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let end = offset
        .saturating_add(if dependency_ids.is_some() {
            MAX_SAMPLED_COMPARISONS_PER_PASS
        } else {
            MAX_ASSESSMENT_CANDIDATES
        })
        .min(chosen.len());
    SampleSelection {
        page_end: end,
        next_cursor: (end < chosen.len()).then(|| format!("sample:{end}")),
        comparisons: chosen
            .get(offset..end)
            .unwrap_or_default()
            .iter()
            .filter(|&&(rule_index, action_index)| {
                dependency_ids.is_none_or(|ids| {
                    let (instruction, rule, start, end) = rules[rule_index];
                    let (action, action_start, action_end) = actions[action_index];
                    ids.contains(&comparison_identity(
                        instruction,
                        rule,
                        action,
                        (start, end),
                        (action_start, action_end),
                    ))
                })
            })
            .map(|&(rule, action)| comparison_at(rules, actions, rule, action, index))
            .collect(),
    }
}

#[derive(Default)]
struct SampleSelection {
    comparisons: Vec<CandidateComparison>,
    page_end: usize,
    next_cursor: Option<String>,
}

fn comparison_at<'a>(
    rules: &[RuleRange<'a>],
    actions: &[ActionRange<'a>],
    rule_index: usize,
    action_index: usize,
    index: &ComparisonIndex<'a>,
) -> CandidateComparison {
    let (instruction, rule, rule_start, rule_end) = rules[rule_index];
    let (action, action_start, action_end) = actions[action_index];
    make_comparison(
        CandidateCoordinate {
            instruction,
            rule,
            rule_range: (rule_start, rule_end),
            action,
            action_range: (action_start, action_end),
        },
        index,
        &index.rule_terms[rule_index],
    )
}

pub(super) struct BranchOrderIndex<'a> {
    pub(super) actions_by_branch: BTreeMap<(String, String), Vec<&'a ContentAction>>,
    pub(super) positions_by_action: BTreeMap<(String, String, String), usize>,
}

pub(super) fn branch_order_index(actions: &[ContentAction]) -> BranchOrderIndex<'_> {
    let mut index = BranchOrderIndex {
        actions_by_branch: BTreeMap::new(),
        positions_by_action: BTreeMap::new(),
    };
    for action in actions {
        let branch_key = (
            action.reference.thread_digest.clone(),
            action.turn_scope.clone(),
        );
        let branch = index
            .actions_by_branch
            .entry(branch_key.clone())
            .or_default();
        index.positions_by_action.insert(
            (branch_key.0, branch_key.1, action.reference.id.clone()),
            branch.len(),
        );
        branch.push(action);
    }
    index
}

/// Add earlier normalized events from an older content page to one saved
/// comparison. Source order decides which events are earlier; relevance only
/// chooses which bounded events to keep.
pub fn extend_comparison_with_history(
    comparison: &CandidateComparison,
    page_actions: &[ContentAction],
    prior_history_complete: bool,
) -> CandidateComparison {
    extend_history(
        comparison,
        page_actions,
        prior_history_complete,
        EvidenceTextLimits::LEGACY,
    )
}

/// Apply the selected model limits to new earlier events in one comparison.
pub fn extend_comparison_with_history_and_capabilities(
    comparison: &CandidateComparison,
    page_actions: &[ContentAction],
    prior_history_complete: bool,
    capabilities: &ModelCapabilities,
) -> CandidateComparison {
    extend_history(
        comparison,
        page_actions,
        prior_history_complete,
        EvidenceTextLimits::from_capabilities(capabilities),
    )
}

fn extend_history(
    comparison: &CandidateComparison,
    page_actions: &[ContentAction],
    prior_history_complete: bool,
    text_limits: EvidenceTextLimits,
) -> CandidateComparison {
    let context_ids = comparison
        .context
        .iter()
        .chain(&comparison.counterevidence)
        .map(|event| event.action_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut earlier = page_actions
        .iter()
        .filter(|action| {
            action.kind != "thinking"
                && action.reference.thread_digest == comparison.source_thread_digest
                && action.turn_scope == comparison.source_turn_scope
                && action.reference.turn_index < comparison.source_turn_index
                && action.reference.id != comparison.reference.action_id
                && !context_ids.contains(action.reference.id.as_str())
        })
        .enumerate()
        .collect::<Vec<_>>();
    let rule_terms = meaningful_terms(rule_text_fragment(comparison));
    earlier.sort_by(|(left_index, left), (right_index, right)| {
        history_relevance(&rule_terms, right)
            .cmp(&history_relevance(&rule_terms, left))
            .then_with(|| right.reference.turn_index.cmp(&left.reference.turn_index))
            .then_with(|| right_index.cmp(left_index))
    });
    let omitted = earlier.len() > MAX_COUNTER_EVIDENCE;
    let mut comparison = comparison.clone();
    comparison.counterevidence.extend(
        earlier
            .into_iter()
            .take(MAX_COUNTER_EVIDENCE)
            .map(|(_, action)| counter_event(action, text_limits.context)),
    );
    comparison.counterevidence.sort_by(|left, right| {
        left.source_order
            .cmp(&right.source_order)
            .then_with(|| left.action_id.cmp(&right.action_id))
    });
    comparison
        .counterevidence
        .dedup_by(|left, right| left.action_id == right.action_id);
    if comparison.counterevidence.len() > MAX_COUNTER_EVIDENCE {
        comparison
            .counterevidence
            .drain(..comparison.counterevidence.len() - MAX_COUNTER_EVIDENCE);
        comparison.earlier_history_truncated = true;
    }
    comparison.earlier_history_truncated |= omitted
        || comparison
            .counterevidence
            .iter()
            .any(|event| event.truncated);
    comparison.prior_history_complete = prior_history_complete;
    comparison
}

pub(super) fn history_relevance(rule_terms: &BTreeSet<String>, action: &ContentAction) -> usize {
    let action_terms = meaningful_terms(&format!(
        "{} {} {}",
        action.text,
        action.tool_name.as_deref().unwrap_or_default(),
        action.tool_call_id.as_deref().unwrap_or_default()
    ));
    rule_terms
        .iter()
        .filter(|term| action_terms.contains(*term))
        .count()
}

pub(super) fn action_meaning(action: &CounterEvidence) -> &'static str {
    match action.kind.as_str() {
        "tool_input" => {
            "The assistant asked a tool to run this command or use these arguments. The text is the tool request, not the tool result."
        }
        _ => {
            "This is text the assistant wrote. A past-tense report of work done describes that completed action; a plan does not. It is not a tool result."
        }
    }
}

fn is_agent_action(action: &ContentAction) -> bool {
    !action.context_only
        && matches!(action.turn_role.as_str(), "assistant" | "tool")
        && matches!(
            action.kind.as_str(),
            "assistant" | "assistant_text" | "tool_input"
        )
}

fn has_selected_action_content(action: &ContentAction) -> bool {
    !action.text.is_empty()
        || action
            .normalized_fields
            .as_ref()
            .is_some_and(|fields| !fields.values.is_empty())
}

pub(super) fn atomic_command(action: &ContentAction) -> bool {
    action.normalized_fields.as_ref().is_some_and(|fields| {
        fields.category == Some(crate::analysis::jev::JevNormalizedCategory::BashCommand)
    })
}

fn action_meaningful_terms(action: &ContentAction) -> BTreeSet<String> {
    meaningful_terms(&format!(
        "{} {} {}",
        action.text,
        action.tool_name.as_deref().unwrap_or_default(),
        action.tool_call_id.as_deref().unwrap_or_default()
    ))
}

fn overlap_score(rule_terms: &BTreeSet<String>, action_terms: &BTreeSet<String>) -> usize {
    rule_terms
        .iter()
        .filter(|term| action_terms.contains(*term))
        .count()
}

pub(super) fn meaningful_terms(text: &str) -> BTreeSet<String> {
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

fn comparison_identity(
    instruction: &InstructionSnapshot,
    rule: &InstructionRuleSection,
    action: &ContentAction,
    rule_text_range: (usize, usize),
    action_text_range: (usize, usize),
) -> String {
    #[cfg(test)]
    selection::record_identity();
    sha256_hex(
        format!(
            "{}\0{}\0{}\0{:?}\0{}\0{}\0{}\0{}\0{}\0{}",
            instruction.id,
            instruction.digest,
            instruction.provenance.as_str(),
            instruction.scope,
            rule.id,
            action.reference.id,
            rule_text_range.0,
            rule_text_range.1,
            action_text_range.0,
            action_text_range.1
        )
        .as_bytes(),
    )
}

fn digest_to_hex(digest: &[u8]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

struct CandidateCoordinate<'a> {
    instruction: &'a InstructionSnapshot,
    rule: &'a InstructionRuleSection,
    rule_range: (usize, usize),
    action: &'a ContentAction,
    action_range: (usize, usize),
}

fn make_comparison(
    coordinate: CandidateCoordinate<'_>,
    index: &ComparisonIndex<'_>,
    rule_terms: &BTreeSet<String>,
) -> CandidateComparison {
    #[cfg(test)]
    PREPARATION_COUNTS.with(|counts| {
        let (projections, rankings, comparisons) = counts.get();
        counts.set((projections, rankings, comparisons + 1));
    });
    let CandidateCoordinate {
        instruction,
        rule,
        rule_range: rule_text_range,
        action,
        action_range: action_text_range,
    } = coordinate;
    let action_digest = index
        .action_digests
        .get(&action.reference.id)
        .map(String::as_str)
        .unwrap_or_default();
    let id = comparison_identity(
        instruction,
        rule,
        action,
        rule_text_range,
        action_text_range,
    );
    let (rule_text_start, rule_text_end) = rule_text_range;
    let (action_text_start, action_text_end) = action_text_range;
    let selected_action = counter_event_with_range(
        action,
        index.text_limits.action,
        Some((action_text_start, action_text_end)),
    );
    let action_text_end = action_text_start + selected_action.text.len();
    let branch_order = index
        .branch_order
        .get(&(
            action.reference.thread_digest.clone(),
            action.turn_scope.clone(),
        ))
        .map(Vec::as_slice)
        .unwrap_or_default();
    let candidate_position = index
        .branch_positions
        .get(&(
            action.reference.thread_digest.clone(),
            action.turn_scope.clone(),
            action.reference.id.clone(),
        ))
        .copied()
        .unwrap_or(0);
    let context_indices = context_indices(candidate_position, branch_order.len());
    let context: Vec<CounterEvidence> = context_indices
        .into_iter()
        .filter_map(|index| branch_order.get(index).copied())
        .filter(|event| event.authority == "assistant")
        .take(MAX_CONTEXT_EVENTS)
        .map(|event| counter_event(event, index.text_limits.context))
        .collect();
    let context_truncated = branch_order.len() > context.len().saturating_add(1);
    let context_ids: BTreeSet<_> = context
        .iter()
        .map(|event| event.action_id.as_str())
        .collect();
    let mut earlier = branch_order
        .iter()
        .take(candidate_position)
        .enumerate()
        .filter(|(_, event)| {
            event.reference.id != action.reference.id
                && event.authority == "assistant"
                && !context_ids.contains(event.reference.id.as_str())
        })
        .map(|(index, event)| (index, *event))
        .collect::<Vec<_>>();
    let relevance_order =
        |(left_index, left): &(usize, &ContentAction),
         (right_index, right): &(usize, &ContentAction)| {
            overlap_score(rule_terms, &index.action_terms[&left.reference.id])
                .cmp(&overlap_score(
                    rule_terms,
                    &index.action_terms[&right.reference.id],
                ))
                .reverse()
                .then_with(|| right_index.cmp(left_index))
        };
    let mut earlier_history_truncated = earlier.len() > MAX_COUNTER_EVIDENCE;
    if earlier_history_truncated {
        earlier.select_nth_unstable_by(MAX_COUNTER_EVIDENCE, relevance_order);
        earlier.truncate(MAX_COUNTER_EVIDENCE);
    }
    earlier.sort_by(relevance_order);
    let counterevidence = earlier
        .into_iter()
        .take(MAX_COUNTER_EVIDENCE)
        .map(|(_, event)| counter_event(event, index.text_limits.context))
        .collect::<Vec<_>>();
    earlier_history_truncated |= counterevidence.iter().any(|event| event.truncated);
    let reference = RuleActionRef {
        instruction_id: instruction.id.clone(),
        instruction_digest: instruction.digest.clone(),
        rule_id: rule.id.clone(),
        rule_heading: rule.heading.clone(),
        start_line: rule.start_line,
        end_line: rule.end_line,
        source: instruction.source.clone(),
        provenance: instruction.provenance,
        scope: instruction.scope,
        action_id: action.reference.id.clone(),
        action_digest: action_digest.to_owned(),
        action_timestamp_ms: action.timestamp_ms,
        action_stable: action.reference.stable,
    };
    CandidateComparison {
        source_binding: Some(super::ActionSourceBinding {
            source: action.reference.clone(),
            authority: action.authority.clone(),
            content_digest: action_digest.to_owned(),
            excerpt: Some(super::ActionExcerptBinding {
                start_byte: action_text_start,
                end_byte: action_text_end,
                source_bytes: action.text.len(),
                source_truncated: action.truncated,
                text_digest: sha256_hex(
                    &action.text.as_bytes()[action_text_start..action_text_end],
                ),
            }),
        }),
        prerequisite_episode: None,
        id: id.clone(),
        reference,
        source_thread_digest: action.reference.thread_digest.clone(),
        source_turn_index: action.reference.turn_index,
        source_turn_scope: action.turn_scope.clone(),
        rule_text: rule.text.clone(),
        instruction_context: rule.context.clone(),
        rule_text_start,
        rule_text_end,
        action: selected_action,
        action_text_start,
        action_text_end,
        context,
        context_truncated,
        counterevidence,
        earlier_history_truncated,
        prior_history_complete: index.prior_history_complete,
    }
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

pub(super) fn counter_event(action: &ContentAction, max_text_bytes: usize) -> CounterEvidence {
    counter_event_with_range(action, max_text_bytes, None)
}

fn counter_event_with_range(
    action: &ContentAction,
    max_text_bytes: usize,
    range: Option<(usize, usize)>,
) -> CounterEvidence {
    let source_text = range
        .and_then(|(start, end)| action.text.get(start..end))
        .unwrap_or(&action.text);
    let (text, text_truncated) = if range.is_some() && atomic_command(action) {
        (source_text.to_owned(), false)
    } else {
        bounded_text(source_text, max_text_bytes)
    };
    CounterEvidence {
        action_id: action.reference.id.clone(),
        source_order: action.reference.turn_index,
        role: action.turn_role.clone(),
        kind: action.kind.clone(),
        timestamp_ms: action.timestamp_ms,
        tool_name: action.tool_name.clone(),
        text,
        truncated: action.truncated
            || text_truncated
            || range.is_some_and(|(start, end)| start != 0 || end != action.text.len()),
    }
}

#[cfg(test)]
pub(super) fn action_text_ranges(text: &str) -> Vec<(usize, usize)> {
    advancing_text_ranges(text, MAX_ACTION_TEXT_BYTES)
}

pub(super) fn bounded_text(text: &str, max_bytes: usize) -> (String, bool) {
    let end = text
        .char_indices()
        .take_while(|(index, character)| index.saturating_add(character.len_utf8()) <= max_bytes)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    (text[..end].to_owned(), end < text.len())
}

pub(super) fn rule_text_fragment(comparison: &CandidateComparison) -> &str {
    comparison
        .rule_text
        .get(comparison.rule_text_start..comparison.rule_text_end)
        .unwrap_or(&comparison.rule_text)
}

pub(super) fn text_ranges(text: &str, window_bytes: usize) -> Vec<(usize, usize)> {
    advancing_text_ranges(text, window_bytes)
}

fn advancing_text_ranges(text: &str, window_bytes: usize) -> Vec<(usize, usize)> {
    crate::analysis::jev::text_ranges::text_ranges(text, window_bytes, MAX_CONTEXT_TEXT_BYTES)
}
