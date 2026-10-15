//! Bounded request preparation and deterministic whole-scope reduction.

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::jev::compact_ids;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[path = "matching.rs"]
mod matching;
pub(super) use matching::select_comparison_episode;
use matching::*;
#[path = "reduction.rs"]
mod reduction;
use reduction::record_usage;
pub use reduction::reduce_assessment;
#[path = "windows.rs"]
mod windows;
use windows::*;

#[cfg(test)]
use super::evidence::ContentAction;
use super::evidence::SessionContentEvidence;
#[cfg(test)]
use super::instructions::InstructionContentClass;
use super::instructions::{InstructionProvenance, InstructionScope, sha256_hex};
#[cfg(test)]
use super::planning::{
    MAX_COUNTER_EVIDENCE, build_jev_context, extend_comparison_with_history, meaningful_terms,
};
use super::planning::{action_meaning, branch_order_index, rule_text_fragment};
#[cfg(test)]
use super::planning::{build_assessment_plan, history_relevance};
use super::questions::{QUESTION_DECISION, target_question_key, window_questions};
use crate::analysis::jev::{
    JevAnswer, JevCheck, JevCheckPlan, JevCheckRevisions, JevCoverage, JevError,
    JevEvidenceReference, JevEvidenceRequirements, JevEvidenceRole, JevInputField,
    JevInputSelection, JevInputWindow, JevSessionContext, JevWorkItem, JevWorkItemResult,
};

pub const ASSESSMENT_MODEL: &str = crate::analysis::jev::PINNED_MODEL;
pub const ASSESSMENT_PROJECTION_REVISION: u32 = 20;
pub const ASSESSMENT_CHUNKING_REVISION: u32 = 35;
pub const ASSESSMENT_QUESTION_REVISION: u32 = 57;
pub const ASSESSMENT_REDUCER_REVISION: u32 = 47;
pub const MAX_ASSESSMENT_CANDIDATES: usize = 2;
pub const MAX_SAMPLED_COMPARISONS_PER_PASS: usize = MAX_ASSESSMENT_CANDIDATES;
pub const INPUT_SELECTION: JevInputSelection = JevInputSelection::from_fields(&[
    JevInputField::UserMessage,
    JevInputField::AssistantMessage,
    JevInputField::BashCommandInput,
    JevInputField::BashCommandOutput,
    JevInputField::FileEditPath,
    JevInputField::ReadFilePath,
    JevInputField::SearchFilesQuery,
    JevInputField::OtherToolInput,
]);
const MAX_TARGETS_PER_WINDOW: usize = 1;

pub(super) fn earlier_read_only_actions(
    comparisons: &[CandidateComparison],
    content: &SessionContentEvidence,
) -> BTreeMap<String, usize> {
    matching::earlier_read_only_actions(comparisons, content)
}

pub(super) fn exact_read_orders(
    comparisons: &[CandidateComparison],
    content: &SessionContentEvidence,
    prior_history_complete: bool,
) -> BTreeMap<String, Vec<crate::analysis::jev::obligations::ReadRequestOrder>> {
    matching::exact_read_orders(comparisons, content, prior_history_complete)
}

pub fn evaluator_revision() -> String {
    format!(
        "{}:{}:{}:{}:{}:direct-rule-action-v2",
        ASSESSMENT_MODEL,
        ASSESSMENT_PROJECTION_REVISION,
        ASSESSMENT_CHUNKING_REVISION,
        ASSESSMENT_QUESTION_REVISION,
        ASSESSMENT_REDUCER_REVISION
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssessmentInput {
    pub content: SessionContentEvidence,
    /// Whether this input covers all earlier in-scope session content.
    pub prior_history_complete: bool,
    /// Actions at or after this watermark can produce findings. Earlier
    /// actions remain available as context for approvals and prerequisites.
    pub activity_after_ms: Option<i64>,
    /// Source positions recorded at enablement for timestamp-less actions.
    pub boundary_positions: BTreeMap<String, u64>,
    pub source_generation: i64,
    pub source_fingerprint: Option<String>,
    pub incarnation: u64,
    /// Cursor for the next comparison page in the same sampling pass.
    #[serde(default)]
    pub comparison_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleActionRef {
    pub instruction_id: String,
    pub instruction_digest: String,
    pub rule_id: String,
    pub rule_heading: String,
    pub start_line: u32,
    pub end_line: u32,
    pub source: String,
    pub provenance: InstructionProvenance,
    pub scope: InstructionScope,
    pub action_id: String,
    #[serde(default)]
    pub action_digest: String,
    pub action_timestamp_ms: Option<i64>,
    pub action_stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CounterEvidence {
    pub action_id: String,
    pub source_order: u64,
    pub role: String,
    pub kind: String,
    pub timestamp_ms: Option<i64>,
    pub tool_name: Option<String>,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateComparison {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_binding: Option<super::decisions::ActionSourceBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerequisite_episode: Option<super::decisions::PrerequisiteEpisode>,
    pub id: String,
    pub reference: RuleActionRef,
    pub source_thread_digest: String,
    pub source_turn_index: u64,
    pub source_turn_scope: String,
    pub rule_text: String,
    #[serde(default)]
    pub instruction_context: Vec<super::instructions::InstructionContextRange>,
    pub rule_text_start: usize,
    pub rule_text_end: usize,
    pub action: CounterEvidence,
    pub action_text_start: usize,
    pub action_text_end: usize,
    /// Nearby events in the same branch and stored source order.
    pub context: Vec<CounterEvidence>,
    pub context_truncated: bool,
    /// Earlier same-branch events for bounded conditional reconciliation.
    pub counterevidence: Vec<CounterEvidence>,
    pub earlier_history_truncated: bool,
    pub prior_history_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssessmentCoverage {
    pub eligible_rules: usize,
    /// Upper bound on complete rule/action pairs before direct-anchor selection.
    pub candidate_pairs: usize,
    pub selected_comparisons: usize,
    /// Upper bound on pairs omitted by this snapshot's policy.
    pub unselected_pairs: usize,
    pub skipped_rules: Vec<String>,
    pub skipped_actions: Vec<String>,
    pub processing_limit_reached: bool,
    /// True when the selected pass is complete but did not inspect every possible pair.
    #[serde(default)]
    pub sampled_pass: bool,
    #[serde(default)]
    pub selector_revision: u32,
    pub limitations: Vec<String>,
    #[serde(skip)]
    pub reassessed_comparison_ids: Vec<String>,
    #[serde(skip)]
    pub reassessed_rule_ids: Vec<String>,
    #[serde(skip)]
    pub reassessed_finding_ids: Vec<String>,
    #[serde(default)]
    pub instruction_sources: Vec<InstructionSourceCoverage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionSourceCoverage {
    pub source: String,
    pub eligible_rules: usize,
    pub candidate_pairs: usize,
    pub selected_comparisons: usize,
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
    pub projection_revision: u32,
    pub chunking_revision: u32,
    pub question_revision: u32,
    pub reducer_revision: u32,
    pub complete_input: bool,
    pub comparisons: Vec<CandidateComparison>,
    #[serde(default)]
    pub current_action_digests: BTreeMap<String, String>,
    #[serde(default)]
    pub current_rule_ids: BTreeSet<(String, String, String)>,
    pub next_comparison_cursor: Option<String>,
    pub coverage: AssessmentCoverage,
    #[serde(default)]
    pub observable_obligations: BTreeMap<String, ObservableObligation>,
    #[serde(default)]
    pub read_request_orders:
        BTreeMap<String, Vec<crate::analysis::jev::obligations::ReadRequestOrder>>,
    #[serde(default)]
    pub earlier_read_only_actions: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservableObligation {
    #[serde(default)]
    pub path_change_policy: super::PathChangePolicy,
    #[serde(default)]
    pub path_change_conflict: Option<bool>,
    #[serde(default)]
    pub literal_policies: Vec<crate::analysis::jev::exact_facts::LiteralPolicyBinding>,
    #[serde(default)]
    pub condition_evidence: crate::analysis::jev::obligations::ConditionEvidence,
    #[serde(default)]
    pub prerequisite_required: bool,
    pub permission: crate::analysis::jev::obligations::PermissionRequirement,
    pub read_request_order: Option<crate::analysis::jev::obligations::ReadRequestOrder>,
    pub read_order_required: bool,
    pub read_order_unknown: bool,
    #[serde(default)]
    pub read_success_required: bool,
    #[serde(default)]
    pub read_prerequisite_absent: bool,
    pub candidate_family: String,
    pub edit_scope_matches: Option<bool>,
    pub recorded_edit_only: bool,
    pub edit_scope_unknown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCertainty {
    Likely,
    Possible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssessmentFinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<super::decisions::DecisionRecord>,
    pub id: String,
    pub reference: RuleActionRef,
    #[serde(default)]
    pub instruction_excerpt: String,
    #[serde(default)]
    pub instruction_excerpt_truncated: bool,
    #[serde(default)]
    pub action_excerpt: String,
    #[serde(default)]
    pub action_excerpt_truncated: bool,
    pub nearby_context_ids: Vec<String>,
    pub counterevidence_ids: Vec<String>,
    pub certainty: FindingCertainty,
    pub composite_probability: f64,
    pub limitations: Vec<String>,
}

impl AssessmentFinding {
    pub fn decision_record(&self) -> Option<&super::decisions::DecisionRecord> {
        self.decision.as_ref().filter(|decision| {
            decision.rule_action == self.reference
                && decision.has_citation_proof()
                && decision.action_is_request
                && self.certainty == FindingCertainty::Possible
                && self.composite_probability.is_finite()
                && (0.95..=1.0).contains(&self.composite_probability)
                && decision.explanation_basis.as_ref().is_some_and(|basis| {
                    basis.schema_revision == 2
                        && basis.instruction == self.instruction_excerpt
                        && basis.action == self.action_excerpt
                })
        })
    }
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
    #[serde(with = "compact_ids")]
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
    pub evidence_basis: String,
    pub completion: CompletionCoverage,
    pub probabilities: BTreeMap<String, BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionCoverage {
    NotObligation,
    BoundaryNotObserved,
    BoundaryObserved,
    Uncertain,
}

/// The check-specific adapter for the reusable Jev execution contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct IgnoredInstructionsCheck;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStatus {
    Likely,
    Possible,
    NoIssue,
    Unassessed,
}

impl JevCheck for IgnoredInstructionsCheck {
    type Prepared = AssessmentPlan;
    type Result = AssessmentResult;

    fn id(&self) -> &'static str {
        "ignored_instructions"
    }

    fn revisions(&self) -> JevCheckRevisions {
        JevCheckRevisions {
            projection: ASSESSMENT_PROJECTION_REVISION,
            chunking: ASSESSMENT_CHUNKING_REVISION,
            questions: ASSESSMENT_QUESTION_REVISION,
            reducer: ASSESSMENT_REDUCER_REVISION,
        }
    }

    fn input_selection(&self) -> JevInputSelection {
        INPUT_SELECTION
    }

    fn supports_incremental_reuse(&self) -> bool {
        true
    }

    fn incremental_identity(&self, context: &JevSessionContext) -> Value {
        context.check_context["incremental_identity"].clone()
    }

    fn evidence_requirements(&self) -> JevEvidenceRequirements {
        JevEvidenceRequirements {
            fields: INPUT_SELECTION,
            references: vec!["instruction_snapshot".to_owned()],
        }
    }

    fn prepare(
        &self,
        context: &JevSessionContext,
    ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
        self.prepare_with_capabilities(
            context,
            &crate::analysis::jev::capabilities::ModelCapabilities::jev_default(),
        )
    }

    fn prepare_with_capabilities(
        &self,
        context: &JevSessionContext,
        capabilities: &crate::analysis::jev::capabilities::ModelCapabilities,
    ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
        let mut assessment: AssessmentPlan =
            serde_json::from_value(context.check_context["assessment_plan"].clone())
                .map_err(|_| JevError::InvalidCheckContext)?;
        if assessment.input_revision != context.input_revision
            || assessment.projection_revision != ASSESSMENT_PROJECTION_REVISION
            || assessment.chunking_revision != ASSESSMENT_CHUNKING_REVISION
            || assessment.question_revision != ASSESSMENT_QUESTION_REVISION
            || assessment.reducer_revision != ASSESSMENT_REDUCER_REVISION
        {
            return Err(JevError::InvalidCheckContext);
        }
        assessment.model_version.clone_from(&capabilities.model);
        select_context(&mut assessment, context, capabilities)?;
        let work_items = assessment_work_items(&assessment.comparisons, context, capabilities);
        let selected_item_count = work_items.len();
        if selected_item_count < assessment.comparisons.len() {
            assessment
                .coverage
                .limitations
                .push("evidence_fit_omitted".to_owned());
        }
        Ok(JevCheckPlan {
            check_id: self.id().to_owned(),
            input_revision: assessment.input_revision.clone(),
            revisions: self.revisions(),
            work_items,
            skipped_item_ids: assessment
                .coverage
                .skipped_rules
                .iter()
                .chain(&assessment.coverage.skipped_actions)
                .cloned()
                .collect(),
            coverage: JevCoverage {
                selected_items: selected_item_count,
                skipped_items: assessment.coverage.skipped_rules.len()
                    + assessment.coverage.skipped_actions.len(),
                not_selected_items: assessment.coverage.unselected_pairs,
                processing_limit_reached: assessment.coverage.processing_limit_reached,
                limitations: assessment.coverage.limitations.clone(),
            },
            capabilities: capabilities.clone(),
            shared_context: None,
            prepared: assessment,
        })
    }

    fn reconcile(
        &self,
        _work_item: &JevWorkItem,
        _initial: &JevWorkItemResult,
        _context: &JevSessionContext,
    ) -> Result<Option<JevWorkItem>, JevError> {
        Ok(None)
    }

    fn reduce(
        &self,
        plan: &JevCheckPlan<Self::Prepared>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError> {
        reduce_check_plan(plan, results, complete, &mut None)
    }
}

pub use reduction::ComparisonDiagnostic;

pub fn diagnose_assessment(
    plan: &JevCheckPlan<AssessmentPlan>,
    results: &[JevWorkItemResult],
    complete: bool,
) -> Result<(AssessmentResult, Vec<ComparisonDiagnostic>), JevError> {
    let mut diagnostics = Vec::new();
    let result = reduce_check_plan(plan, results, complete, &mut Some(&mut diagnostics))?;
    Ok((result, diagnostics))
}

fn reduce_check_plan(
    plan: &JevCheckPlan<AssessmentPlan>,
    results: &[JevWorkItemResult],
    complete: bool,
    diagnostics: &mut Option<&mut Vec<ComparisonDiagnostic>>,
) -> Result<AssessmentResult, JevError> {
    if plan.check_id != super::CHECK_ID {
        return Err(JevError::InvalidCheckPlan);
    }
    let assessment = &plan.prepared;
    if assessment.input_revision != plan.input_revision
        || plan.revisions != IgnoredInstructionsCheck.revisions()
        || assessment.projection_revision != ASSESSMENT_PROJECTION_REVISION
        || assessment.chunking_revision != ASSESSMENT_CHUNKING_REVISION
        || assessment.question_revision != ASSESSMENT_QUESTION_REVISION
        || assessment.reducer_revision != ASSESSMENT_REDUCER_REVISION
    {
        return Err(JevError::InvalidCheckPlan);
    }
    let results_by_window = results
        .iter()
        .map(|result| (result.work_item_id.as_str(), result))
        .collect::<BTreeMap<_, _>>();
    let mut target_results = BTreeMap::new();
    for comparison in &assessment.comparisons {
        let question_key = target_question_key(&comparison.id, QUESTION_DECISION);
        let Some(work_item) = plan
            .work_items
            .iter()
            .find(|item| item.questions.contains_key(&question_key))
        else {
            continue;
        };
        let Some(window_result) = results_by_window.get(work_item.id.as_str()) else {
            continue;
        };
        if window_result.evidence != work_item.window.evidence
            || window_result.model != plan.capabilities.model
        {
            continue;
        }
        let prefix = format!("target-{}::", comparison.id);
        let answers = window_result
            .answers
            .iter()
            .filter_map(|(key, answer)| {
                key.strip_prefix(&prefix)
                    .map(|question| (question.to_owned(), answer.clone()))
            })
            .collect();
        target_results.insert(
            comparison.id.clone(),
            JevWorkItemResult {
                request_id: window_result.request_id.clone(),
                work_item_id: comparison.id.clone(),
                answers,
                evidence: window_result.evidence.clone(),
                model: window_result.model.clone(),
                usage: window_result.usage,
            },
        );
    }
    let mut reduced = reduction::reduce_traced(assessment, &target_results, complete, diagnostics);
    let mut seen_requests = BTreeSet::new();
    reduced.request_count = 0;
    reduced.input_tokens = 0;
    reduced.output_tokens = 0;
    for result in results {
        record_usage(
            result,
            &mut seen_requests,
            &mut reduced.request_count,
            &mut reduced.input_tokens,
            &mut reduced.output_tokens,
        );
    }
    Ok(reduced)
}

#[cfg(test)]
fn comparison_evidence(comparison: &CandidateComparison) -> Vec<JevEvidenceReference> {
    let mut evidence = vec![
        JevEvidenceReference {
            part_id: "instruction".to_owned(),
            source_id: format!(
                "{}:{}",
                comparison.reference.instruction_id, comparison.reference.rule_id
            ),
            content_kind: "instruction_rule".to_owned(),
            role: JevEvidenceRole::Instruction,
        },
        JevEvidenceReference {
            part_id: "candidate_action".to_owned(),
            source_id: comparison.action.action_id.clone(),
            content_kind: comparison.action.kind.clone(),
            role: JevEvidenceRole::Candidate,
        },
    ];
    evidence.extend(comparison.context.iter().enumerate().map(|(index, event)| {
        JevEvidenceReference {
            part_id: format!("nearby_context[{index}]"),
            source_id: event.action_id.clone(),
            content_kind: event.kind.clone(),
            role: JevEvidenceRole::SupportingContext,
        }
    }));
    evidence.extend(
        comparison
            .counterevidence
            .iter()
            .enumerate()
            .map(|(index, event)| JevEvidenceReference {
                part_id: format!("earlier_counterevidence[{index}]"),
                source_id: event.action_id.clone(),
                content_kind: event.kind.clone(),
                role: JevEvidenceRole::SupportingContext,
            }),
    );
    evidence
}

#[cfg(test)]
#[path = "capability_tests.rs"]
mod capability_tests;

#[cfg(test)]
#[path = "development_tests.rs"]
mod development_tests;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::analysis::jev::JevQuestion;
    use crate::analysis::jev::JevUsage;
    use crate::checks::ignored_instructions::snapshot_from_text;

    fn applicable_result(item: &JevWorkItem) -> JevWorkItemResult {
        JevWorkItemResult {
            request_id: "initial-batch".to_owned(),
            work_item_id: item.id.clone(),
            answers: item
                .questions
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        JevAnswer::Choice {
                            choice: "conflict".to_owned(),
                            probabilities: BTreeMap::from([
                                ("conflict".to_owned(), 0.97),
                                ("no_conflict".to_owned(), 0.015),
                                ("uncertain".to_owned(), 0.015),
                            ]),
                            confidence: 0.95,
                        },
                    )
                })
                .collect(),
            evidence: item.window.evidence.clone(),
            model: ASSESSMENT_MODEL.to_owned(),
            usage: JevUsage {
                input_tokens: 10,
                output_tokens: 2,
            },
        }
    }

    #[tokio::test]
    async fn single_round_runner_processes_all_three_decisions() {
        use crate::analysis::jev::{JevResponse, JevRunProgress, run_jev_check};

        for (decision, expected_requests, expected_findings, expected_unassessed) in [
            ("no_conflict", 1, 0, 0),
            ("conflict", 1, 1, 0),
            ("uncertain", 1, 0, 1),
        ] {
            let context = build_jev_context(&input(
                vec![command("action", 10, "git push --force")],
                "Do not run `git push --force`.",
            ))
            .unwrap();
            let outcome = run_jev_check(
                &IgnoredInstructionsCheck,
                &context,
                JevRunProgress::default(),
                |batch| async move {
                    let answers = batch
                        .request
                        .questions
                        .iter()
                        .map(|(id, question)| {
                            let JevQuestion::Choice { criteria, .. } = question else {
                                unreachable!()
                            };
                            assert_eq!(criteria.len(), 3);
                            let selected = decision;
                            assert!(criteria.contains_key(selected), "{id}: {selected}");
                            let other = 0.03 / (criteria.len() - 1) as f64;
                            (
                                id.clone(),
                                JevAnswer::Choice {
                                    choice: selected.to_owned(),
                                    probabilities: criteria
                                        .keys()
                                        .map(|option| {
                                            (
                                                option.clone(),
                                                if option == selected { 0.97 } else { other },
                                            )
                                        })
                                        .collect(),
                                    confidence: 0.96,
                                },
                            )
                        })
                        .collect();
                    Ok(JevResponse {
                        model: ASSESSMENT_MODEL.to_owned(),
                        answers,
                        usage: JevUsage {
                            input_tokens: 10,
                            output_tokens: 5,
                        },
                    })
                },
                |_| Ok(()),
            )
            .await
            .unwrap();
            assert!(outcome.complete);
            assert_eq!(outcome.result.request_count, expected_requests);
            assert_eq!(
                outcome.result.input_tokens,
                u64::from(expected_requests) * 10
            );
            assert_eq!(outcome.result.findings.len(), expected_findings);
            assert_eq!(
                outcome.result.unassessed_comparisons.len(),
                expected_unassessed
            );
        }
    }

    pub(in crate::checks::ignored_instructions) fn event(
        id: &str,
        timestamp_ms: i64,
        role: &str,
        thread: &str,
        text: &str,
    ) -> ContentAction {
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
            authority: if role == "assistant" {
                "assistant"
            } else {
                "user"
            }
            .to_owned(),
            kind: "assistant_text".to_owned(),
            text: text.to_owned(),
            tool_name: None,
            tool_call_id: None,
            normalized_fields: None,
            metadata: Default::default(),
            truncated: false,
            context_only: false,
        }
    }

    pub(in crate::checks::ignored_instructions) fn input(
        mut actions: Vec<ContentAction>,
        rule_text: &str,
    ) -> AssessmentInput {
        for action in &mut actions {
            if action.kind == "tool_input"
                && action.normalized_fields.is_none()
                && let Some(name) = action.tool_name.as_deref()
            {
                action.normalized_fields = Some(
                    crate::analysis::jev_evidence::normalize_tool_input(name, &action.text),
                );
            }
        }
        crate::analysis::jev_evidence::normalize_context(
            &mut actions,
            crate::analysis::SourceFormat::ClaudeJsonl,
        );
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
                field_availability: Vec::new(),
            },
            prior_history_complete: true,
            activity_after_ms: None,
            boundary_positions: BTreeMap::new(),
            source_generation: 2,
            source_fingerprint: Some("fingerprint".to_owned()),
            incarnation: 1,
            comparison_after: None,
        }
    }

    pub(in crate::checks::ignored_instructions) fn command(
        id: &str,
        timestamp_ms: i64,
        text: &str,
    ) -> ContentAction {
        let mut action = event(id, timestamp_ms, "assistant", "main", text);
        action.kind = "tool_input".to_owned();
        action.tool_name = Some("Bash".to_owned());
        action.tool_call_id = Some(format!("call-{id}"));
        action.normalized_fields = Some(crate::analysis::jev_evidence::normalize_tool_input(
            "Bash", text,
        ));
        action
    }

    fn compact_capabilities() -> crate::analysis::jev::capabilities::ModelCapabilities {
        use crate::analysis::jev::capabilities::{
            CapabilityLimit, CapabilitySource, ModelCapabilities, OLLAMA_TEV1_ESTIMATOR,
            TokenizerIdentity,
        };
        let mut capabilities = ModelCapabilities::jev_default();
        capabilities.runtime_context_tokens =
            CapabilityLimit::known(2048, CapabilitySource::Manual);
        capabilities.rendering_reserve_tokens = 304;
        capabilities.tokenizer = Some(TokenizerIdentity::ConservativeEstimator(
            OLLAMA_TEV1_ESTIMATOR.to_owned(),
        ));
        capabilities
    }

    #[test]
    fn heldout_prose_cases_remain_scope_diagnostics_without_tool_evidence() {
        #[derive(Deserialize)]
        struct Case {
            rule: String,
            actions: Vec<String>,
            expected: String,
        }
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/ignored_instructions/selector_heldout.json"
        ))
        .unwrap();
        for case in cases {
            let mut actions = (0..300)
                .map(|index| {
                    event(
                        &format!("filler-{index}"),
                        index,
                        "assistant",
                        "main",
                        "Reviewed an unrelated detail.",
                    )
                })
                .collect::<Vec<_>>();
            actions.extend(case.actions.iter().enumerate().map(|(index, text)| {
                event(
                    &format!("heldout-{index}"),
                    (index + 300) as i64,
                    "assistant",
                    "main",
                    text,
                )
            }));
            let expected = case
                .actions
                .iter()
                .position(|text| text == &case.expected)
                .unwrap();
            let plan = build_assessment_plan(input(actions, &case.rule));
            assert_eq!(case.actions[expected], case.expected);
            assert!(plan.comparisons.is_empty(), "{}", case.rule);
            let result = reduce_assessment(&plan, &BTreeMap::new(), true);
            assert!(result.findings.is_empty());
            assert_eq!(result.request_count, 0);
            assert_eq!(result.coverage.selected_comparisons, 0);
        }
    }

    #[test]
    fn appended_relevant_action_enters_the_next_sample() {
        let mut assessment_input = input(
            (0..300)
                .map(|index| command(&format!("action-{index}"), index, "git push --force"))
                .collect(),
            "Do not run `git push --force`.",
        );
        let first = build_assessment_plan(assessment_input.clone());
        assessment_input.content.actions.push(command(
            "new-publication",
            300,
            "git push --force origin main",
        ));
        assessment_input.content.selected_input_digest = "new-selected-input".to_owned();
        let second = build_assessment_plan(assessment_input.clone());
        assert!(
            second
                .comparisons
                .iter()
                .any(|comparison| comparison.reference.action_id == "new-publication")
        );
        assert_ne!(first.input_revision, second.input_revision);
        assert_eq!(
            second
                .comparisons
                .iter()
                .map(|comparison| &comparison.id)
                .collect::<Vec<_>>(),
            build_assessment_plan(assessment_input)
                .comparisons
                .iter()
                .map(|comparison| &comparison.id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn terminal_reviews_do_not_page_through_policy_omitted_comparisons() {
        let mut source = input(
            (0..300)
                .map(|index| command(&format!("action-{index}"), index, "git push --force"))
                .collect(),
            &(0..8)
                .map(|index| format!("- Do not run `git push --force` for step {index}."))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let mut ledger = super::super::planning::SamplingLedger::default();
        let mut seen = BTreeSet::new();
        for pass in 0..3 {
            source.comparison_after = None;
            let mut pass_count = 0;
            let mut pass_comparisons = BTreeMap::new();
            loop {
                let plan = super::super::planning::build_assessment_plan_with_sampling(
                    source.clone(),
                    &ledger,
                );
                assert_eq!(plan.coverage.candidate_pairs, 2400);
                assert!(plan.comparisons.len() <= MAX_ASSESSMENT_CANDIDATES);
                for comparison in &plan.comparisons {
                    assert!(seen.insert(comparison.id.clone()));
                    pass_count += 1;
                    pass_comparisons.insert(
                        comparison.id.clone(),
                        super::super::planning::SavedComparison::from(comparison),
                    );
                }
                assert!(plan.next_comparison_cursor.is_none());
                source.comparison_after = plan.next_comparison_cursor;
                if source.comparison_after.is_none() {
                    break;
                }
            }
            assert_eq!(pass_count, if pass == 0 { 2 } else { 0 });
            ledger.comparison_ids = seen.clone();
            ledger.comparisons.extend(pass_comparisons);
        }
        let empty = super::super::planning::build_assessment_plan_with_sampling(source, &ledger);
        assert!(empty.comparisons.is_empty());
        assert_eq!(empty.coverage.unselected_pairs, 2398);
    }

    #[test]
    fn new_actions_precede_old_unsampled_pairs() {
        let mut source = input(
            (0..1200)
                .map(|index| command(&format!("old-{index}"), index, "git push --force"))
                .collect(),
            "Do not run `git push --force`.",
        );
        let first = build_assessment_plan(source.clone());
        let mut ledger = super::super::planning::SamplingLedger {
            comparison_ids: first
                .comparisons
                .iter()
                .map(|comparison| comparison.id.clone())
                .collect(),
            known_action_ids: source
                .content
                .actions
                .iter()
                .map(|action| action.reference.id.clone())
                .collect(),
            comparisons: first
                .comparisons
                .iter()
                .map(|comparison| {
                    (
                        comparison.id.clone(),
                        super::super::planning::SavedComparison::from(comparison),
                    )
                })
                .collect(),
        };
        source
            .content
            .actions
            .push(command("new-action", 1200, "git push --force"));
        let next = super::super::planning::build_assessment_plan_with_sampling(source, &ledger);
        assert_eq!(next.comparisons[0].reference.action_id, "new-action");
        assert!(
            next.comparisons
                .iter()
                .all(|comparison| !ledger.comparison_ids.contains(&comparison.id))
        );
        ledger.comparison_ids.extend(
            next.comparisons
                .iter()
                .map(|comparison| comparison.id.clone()),
        );
    }

    #[test]
    fn high_priority_action_enters_the_first_page_before_backlog() {
        let mut actions = (0..1500)
            .map(|index| command(&format!("filler-{index}"), index, "npm test"))
            .collect::<Vec<_>>();
        actions.push(command("matching", 1500, "git push --force"));
        let plan = build_assessment_plan(input(actions, "Do not run `git push --force`."));
        assert_eq!(plan.comparisons.len(), 1);
        assert_eq!(plan.comparisons[0].reference.action_id, "matching");
        assert_eq!(plan.coverage.unselected_pairs, 1500);
    }

    #[test]
    fn reducer_keeps_sampled_coverage_after_selected_work_completes() {
        let assessment_input = input(
            (0..300)
                .map(|index| command(&format!("action-{index}"), index, "git push --force"))
                .collect(),
            "Do not run `git push --force`.",
        );
        let plan = build_assessment_plan(assessment_input);
        let result = reduce_assessment(&plan, &BTreeMap::new(), true);
        assert!(result.coverage.sampled_pass);
        assert_eq!(
            result.coverage.selector_revision,
            plan.coverage.selector_revision
        );
        assert_eq!(
            result.coverage.unselected_pairs,
            300 - MAX_ASSESSMENT_CANDIDATES
        );
        assert!(!result.coverage.processing_limit_reached);
        assert!(
            result
                .coverage
                .limitations
                .contains(&"sampled_candidate_selection".to_owned())
        );
    }

    #[test]
    #[ignore = "offline 57-rule, 7000-action planning benchmark"]
    fn semantic_selector_benchmark() {
        let actions = (0..7000)
            .map(|index| {
                event(
                    &format!("action-{index}"),
                    index,
                    "assistant",
                    &format!("branch-{}", index % 20),
                    &format!(
                        "Changed module {} and ran tests for release {}.",
                        index % 57,
                        index % 11
                    ),
                )
            })
            .collect();
        let rules = (0..57).map(|index| format!("- Check module {index} before publishing a release and request approval for changes.")).collect::<Vec<_>>().join("\n");
        let started = std::time::Instant::now();
        let mut source = input(actions, &rules);
        let mut selected = BTreeSet::new();
        let mut pages = 0;
        loop {
            let plan = build_assessment_plan(source.clone());
            assert_eq!(plan.coverage.candidate_pairs, 57 * 7000);
            selected.extend(
                plan.comparisons
                    .iter()
                    .map(|comparison| comparison.id.clone()),
            );
            pages += 1;
            source.comparison_after = plan.next_comparison_cursor;
            if source.comparison_after.is_none() {
                break;
            }
        }
        assert_eq!(selected.len(), MAX_SAMPLED_COMPARISONS_PER_PASS);
        assert_eq!(pages, 4);
        eprintln!(
            "sampling 57x7000: {:?}, selected={}, pages={pages}",
            started.elapsed(),
            selected.len()
        );
    }

    #[test]
    fn unevaluable_requirement_stays_visible_as_skipped_coverage() {
        let mut assessment_input = input(Vec::new(), "Never run the release command.");
        let instruction = &mut assessment_input.content.instructions[0];
        let rule = instruction
            .sections
            .iter_mut()
            .find(|rule| rule.content_class == InstructionContentClass::RequirementCandidate)
            .unwrap();
        rule.evaluable = false;
        let expected_id = format!("{}:{}", instruction.id, rule.id);

        let plan = build_assessment_plan(assessment_input);

        assert_eq!(plan.coverage.skipped_rules, [expected_id]);
        assert!(
            plan.coverage
                .limitations
                .contains(&"instruction_rule_not_evaluable".to_owned())
        );
        assert_eq!(plan.coverage.candidate_pairs, 0);
        assert!(plan.comparisons.is_empty());
    }

    #[test]
    fn empty_selected_action_is_an_explicit_coverage_gap() {
        let plan = build_assessment_plan(input(
            vec![command("empty-reply", 10, "")],
            "Do not run `git push --force`.",
        ));
        assert!(plan.comparisons.is_empty());
        assert_eq!(plan.coverage.skipped_actions, ["empty-reply"]);
        assert!(
            plan.coverage
                .limitations
                .contains(&"empty_selected_action_content".to_owned())
        );
    }

    #[test]
    fn separate_instructions_in_one_section_produce_distinct_rule_comparisons() {
        let plan = build_assessment_plan(input(
            vec![command("commit-1", 100, "git commit --amend")],
            "# Workflow\n- Do not run `git commit --amend`.\n- Use `git commit -s`.",
        ));
        assert_eq!(plan.coverage.eligible_rules, 2);
        assert_eq!(plan.comparisons.len(), 2);
        assert_ne!(
            plan.comparisons[0].reference.rule_id,
            plan.comparisons[1].reference.rule_id
        );
        assert_eq!(plan.comparisons[0].reference.start_line, 2);
        assert_eq!(plan.comparisons[1].reference.start_line, 3);
        assert!(plan.comparisons[0].rule_text.contains("Do not run"));
        assert!(
            !plan.comparisons[0]
                .rule_text
                .contains("Use `git commit -s`")
        );
        assert!(
            plan.comparisons[1]
                .rule_text
                .contains("Use `git commit -s`")
        );
    }

    #[test]
    fn one_action_has_two_bound_questions_in_one_request() {
        let assessment_input = input(
            vec![command("one-action", 10, "git commit --amend")],
            "- Do not run `git commit --amend`.\n- Use `git commit -s`.",
        );
        let context = build_jev_context(&assessment_input).unwrap();
        let plan = IgnoredInstructionsCheck
            .prepare_with_capabilities(&context, &compact_capabilities())
            .unwrap();

        assert_eq!(plan.work_items.len(), 2);
        assert_eq!(plan.work_items[0].questions.len(), 1);
        assert_eq!(
            plan.work_items[0].window.fields["instruction_targets"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            plan.work_items[0].window.fields["candidate_action"]["text"],
            "git commit --amend"
        );
        assert_eq!(
            plan.work_items[0]
                .window
                .evidence
                .iter()
                .filter(|part| part.role == JevEvidenceRole::Candidate)
                .count(),
            1
        );

        let mut packed = crate::analysis::jev::pack_work_items_with_capabilities(
            &plan.work_items,
            &plan.capabilities,
        );
        assert!(packed.skipped_item_ids.is_empty());
        assert_eq!(packed.batches.len(), 1);
        let batch = packed.batches.remove(0);
        assert_eq!(batch.request.questions.len(), 2);
        let followup = IgnoredInstructionsCheck
            .reconcile(
                &plan.work_items[0],
                &applicable_result(&plan.work_items[0]),
                &context,
            )
            .unwrap();
        assert!(followup.is_none());
        assert_eq!(batch.evidence_owners[&plan.work_items[0].id].len(), 3);
        assert_eq!(
            batch.evidence_owners[&plan.work_items[0].id]
                .iter()
                .filter(|reference| reference.part_id.contains("surrounding_context"))
                .count(),
            1
        );
        assert!(
            batch
                .request
                .state
                .to_string()
                .contains("git commit --amend")
        );
        assert!(!batch.request.state.to_string().contains("one-action"));
    }

    #[test]
    fn confident_negative_target_does_not_request_followup_for_another_target() {
        let context = build_jev_context(&input(
            vec![command("action", 10, "git commit --amend")],
            "- Do not run `git commit --amend`.\n- Use `git commit -s`.",
        ))
        .unwrap();
        let plan = IgnoredInstructionsCheck
            .prepare_with_capabilities(&context, &compact_capabilities())
            .unwrap();
        let item = &plan.work_items[0];
        assert_eq!(plan.work_items.len(), 2);
        assert_eq!(item.questions.len(), 1);
        let mut initial = applicable_result(item);
        let negative = item.questions.keys().next().unwrap();
        initial.answers.insert(
            negative.clone(),
            JevAnswer::Choice {
                choice: "no_conflict".to_owned(),
                probabilities: BTreeMap::from([
                    ("no_conflict".to_owned(), 0.97),
                    ("conflict".to_owned(), 0.015),
                    ("uncertain".to_owned(), 0.015),
                ]),
                confidence: 0.95,
            },
        );
        let followup = IgnoredInstructionsCheck
            .reconcile(item, &initial, &context)
            .unwrap();
        assert!(followup.is_none());
        assert_eq!(initial.answers.len(), 1);
        let result = IgnoredInstructionsCheck
            .reduce(&plan, &[initial], true)
            .unwrap();
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons.len(), 1);
    }

    #[test]
    fn literal_policy_does_not_add_a_followup_round() {
        let context = build_jev_context(&input(
            vec![command("report", 10, "git push --force")],
            "Never run `git push --force`.",
        ))
        .unwrap();
        let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
        let mut item = plan.work_items[0].clone();
        item.window.fields["instruction_targets"][0]["literal_policies"] =
            json!([{"policy":"construct_ban","identifier":"useEffect"}]);
        let mut initial = applicable_result(&item);
        let key = item.questions.keys().next().unwrap().clone();
        initial.answers.insert(
            key,
            JevAnswer::Choice {
                choice: "no_conflict".to_owned(),
                probabilities: BTreeMap::from([
                    ("no_conflict".to_owned(), 0.97),
                    ("conflict".to_owned(), 0.015),
                    ("uncertain".to_owned(), 0.015),
                ]),
                confidence: 0.95,
            },
        );
        let followup = IgnoredInstructionsCheck
            .reconcile(&item, &initial, &context)
            .unwrap();
        assert!(followup.is_none());
        assert_eq!(item.questions.len(), 1);
    }

    #[test]
    fn oversized_unanchored_rule_scopes_do_not_create_probe_requests() {
        let actions = (0..12)
            .map(|index| command(&format!("action-{index}"), index, "npm test"))
            .collect();
        let rules = (0..57)
            .map(|index| format!("- Follow procedure {index} for a release."))
            .collect::<Vec<_>>()
            .join("\n");
        let context = build_jev_context(&input(actions, &rules)).unwrap();
        let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
        assert_eq!(plan.prepared.coverage.skipped_rules.len(), 57);
        assert!(plan.prepared.comparisons.is_empty());
        assert!(plan.work_items.is_empty());
        let result = IgnoredInstructionsCheck.reduce(&plan, &[], true).unwrap();
        assert_eq!(result.request_count, 0);
        assert!(result.findings.is_empty());
        assert!(
            result
                .coverage
                .limitations
                .contains(&"instruction_rule_not_evaluable".to_owned())
        );
    }

    #[test]
    fn normalized_assistant_text_does_not_prove_a_typed_action() {
        let mut normalized = event(
            "native-assistant-text",
            100,
            "assistant",
            "main",
            "I added useEffect instead of deriving the value during render.",
        );
        normalized.kind = "assistant".to_owned();

        let plan = build_assessment_plan(input(
            vec![normalized],
            "Do not add useEffect when the value can be derived during render.",
        ));

        assert_eq!(plan.coverage.candidate_pairs, 0);
        assert_eq!(plan.coverage.selected_comparisons, 0);
        assert!(plan.comparisons.is_empty());
    }

    #[test]
    fn prior_history_coverage_is_bound_to_the_assessment_revision() {
        let complete = input(
            vec![command("release", 10, "git push --force")],
            "Do not run `git push --force`.",
        );
        let complete_plan = build_assessment_plan(complete.clone());
        let mut partial = complete;
        partial.prior_history_complete = false;
        let partial_plan = build_assessment_plan(partial);

        assert_ne!(complete_plan.input_revision, partial_plan.input_revision);
        assert!(complete_plan.comparisons[0].prior_history_complete);
        assert!(!partial_plan.comparisons[0].prior_history_complete);
        assert_eq!(
            initial_context(&partial_plan.comparisons[0])["assessment_limits"]["prior_history_complete"],
            false
        );
    }

    #[test]
    fn selected_event_windows_send_one_joint_question() {
        let context = build_jev_context(&input(
            vec![command("action", 10, "git push --force")],
            "- Do not run `git push --force`.",
        ))
        .unwrap();
        let plan = IgnoredInstructionsCheck
            .prepare(&context)
            .expect("assessment plan");
        let questions = &plan.work_items[0].questions;

        assert_eq!(questions.len(), 1);
        assert!(questions.keys().any(|key| key.ends_with("::decision")));
        let followup = IgnoredInstructionsCheck
            .reconcile(
                &plan.work_items[0],
                &applicable_result(&plan.work_items[0]),
                &context,
            )
            .unwrap();
        assert!(followup.is_none());
        assert_eq!(
            plan.work_items[0].window.fields["candidate_action"]["kind"],
            "tool_input"
        );
        assert_eq!(
            plan.work_items[0].window.fields["candidate_action"]["text"],
            "git push --force"
        );
    }

    #[test]
    fn assessment_window_question_keys_remain_stable_across_windows() {
        let comparisons = (0..3)
            .map(|index| {
                candidate(
                    &format!("comparison-{index}"),
                    &format!("rule-{index}"),
                    "Follow the documented rule.",
                    "shared-action",
                    "Used the documented rule.",
                    index as i64 + 1,
                )
            })
            .collect::<Vec<_>>();
        let windows = assessment_windows(&comparisons);
        assert_eq!(windows.len(), 3);

        let mut keys = BTreeSet::new();
        for window in windows {
            let questions = window_questions(&window.comparisons);
            for comparison in window.comparisons {
                let key = target_question_key(&comparison.id, QUESTION_DECISION);
                assert!(questions.contains_key(&key));
                assert!(keys.insert(key));
            }
        }
        assert_eq!(keys.len(), comparisons.len());
    }

    #[test]
    fn assessment_questions_keep_typed_choice_contracts() {
        let context = build_jev_context(&input(
            vec![command("action", 10, "git push --force")],
            "Do not run `git push --force` unless recovery is required.",
        ))
        .unwrap();
        let plan = IgnoredInstructionsCheck
            .prepare(&context)
            .expect("assessment plan");

        let questions = &plan.work_items[0].questions;
        assert_eq!(questions.len(), 1);
        let assert_options = |question: &JevQuestion, expected: &[&str]| {
            let JevQuestion::Choice { criteria, .. } = question else {
                panic!("assessment questions use typed choices")
            };
            assert_eq!(
                criteria.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                expected.iter().copied().collect()
            );
        };
        assert_options(
            plan.work_items[0].questions.values().next().unwrap(),
            &["conflict", "no_conflict", "uncertain"],
        );
        assert!(questions.keys().all(|key| key.ends_with("::decision")));
    }

    #[test]
    fn large_synthetic_plan_uses_fewer_requests_per_selected_comparison() {
        let actions = (0..40)
            .map(|index| command(&format!("action-{index}"), index, "git push --force"))
            .collect();
        let rules = "Do not run `git push --force`.";
        let context = build_jev_context(&input(actions, rules)).unwrap();
        let plan = IgnoredInstructionsCheck
            .prepare_with_capabilities(&context, &compact_capabilities())
            .expect("large synthetic assessment plan");
        let request_count = plan
            .work_items
            .iter()
            .map(|item| item.questions.len())
            .sum::<usize>();
        let selected_targets = request_count;
        assert_eq!(selected_targets, MAX_ASSESSMENT_CANDIDATES);
        assert_eq!(plan.coverage.selected_items, plan.work_items.len());
        assert_eq!(plan.work_items.len(), selected_targets);
        assert!(plan.prepared.coverage.sampled_pass);
        assert_eq!(request_count, selected_targets);
        let packed = crate::analysis::jev::pack_work_items_with_capabilities(
            &plan.work_items,
            &plan.capabilities,
        );
        assert!(packed.skipped_item_ids.is_empty());
        assert_eq!(packed.batches.len(), 1);
        assert!(packed.batches.iter().all(|batch| {
            crate::analysis::jev::validate_jev_request_with_capabilities(
                &batch.request,
                &plan.capabilities,
            )
            .is_ok()
        }));
        assert!(
            plan.work_items
                .iter()
                .all(|item| { item.questions.len() <= MAX_TARGETS_PER_WINDOW })
        );
    }

    #[test]
    fn tool_name_anchors_exclude_other_families_and_unbound_requests() {
        let mut actions = ["Read", "Edit", "Bash", "Search"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                let mut action = event(
                    &format!("tool-{index}"),
                    index as i64,
                    "assistant",
                    "main",
                    match name {
                        "Read" | "Edit" => r#"{"file_path":"src/a.rs"}"#,
                        "Bash" => r#"{"command":"git status"}"#,
                        _ => r#"{"query":"token"}"#,
                    },
                );
                action.kind = "tool_input".to_owned();
                action.tool_name = Some(name.to_owned());
                action.tool_call_id = Some(format!("tool-call-{index}"));
                action
            })
            .collect::<Vec<_>>();
        actions.push(event("report", 5, "assistant", "main", "I called Read."));
        let mut unknown = actions[1].clone();
        unknown.reference.id = "unknown".to_owned();
        unknown.reference.stable = false;
        actions.push(unknown);
        let mut truncated = actions[2].clone();
        truncated.reference.id = "truncated".to_owned();
        truncated.truncated = true;
        actions.push(truncated);
        let plan =
            build_assessment_plan(input(actions.clone(), "Do not call the tool named `Read`."));
        let ids = plan
            .comparisons
            .iter()
            .map(|comparison| comparison.reference.action_id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(ids.contains("tool-0"));
        assert_eq!(ids, BTreeSet::from(["tool-0"]));
        assert_eq!(plan.coverage.candidate_pairs, 4);
        assert_eq!(plan.coverage.unselected_pairs, 3);
        assert_eq!(plan.coverage.skipped_actions, ["unknown", "truncated"]);
        let variant = build_assessment_plan(input(actions.clone(), "Do not call the `Read` tool."));
        assert_eq!(variant.comparisons.len(), 1);
        for rule in [
            "Do not call the tool named `Read` when editing.",
            "Do not call the tool named `Read` or `Edit`.",
        ] {
            let fallback = build_assessment_plan(input(actions.clone(), rule));
            assert!(!fallback.comparisons.is_empty());
            assert!(fallback.comparisons.iter().all(|comparison| matches!(
                comparison.action.tool_name.as_deref(),
                Some("Read" | "Edit")
            )));
        }
    }

    #[test]
    fn mixed_tool_names_are_sampled_in_one_pass() {
        let actions = (0..300)
            .map(|index| {
                let mut action = event(
                    &format!("action-{index}"),
                    index,
                    "assistant",
                    "main",
                    r#"{"file_path":"src/a.rs"}"#,
                );
                action.kind = "tool_input".to_owned();
                action.tool_name = Some(if index % 3 == 0 { "Read" } else { "Edit" }.to_owned());
                action.tool_call_id = Some(format!("call-{index}"));
                if index % 17 == 0 {
                    action.reference.stable = false;
                }
                action
            })
            .collect::<Vec<_>>();
        let rules = "- Do not call the tool named `Read`.\n- Do not call the tool named `Edit`.\n- Follow the review procedure.";
        let plan = build_assessment_plan(input(actions, rules));
        assert_eq!(plan.coverage.candidate_pairs, 3 * (300 - 18));
        assert_eq!(plan.comparisons.len(), MAX_ASSESSMENT_CANDIDATES);
        assert!(plan.coverage.sampled_pass);
        assert!(plan.next_comparison_cursor.is_none());
        assert_eq!(
            plan.comparisons
                .iter()
                .map(|comparison| comparison.reference.action_id.as_str())
                .collect::<Vec<_>>(),
            ["action-299", "action-298"]
        );
        assert!(plan.comparisons.iter().all(|comparison| {
            comparison
                .rule_text
                .contains(comparison.action.tool_name.as_deref().unwrap())
        }));
    }

    #[test]
    fn conditional_tool_rules_keep_all_actions_eligible() {
        let mut edit = event(
            "edit",
            1,
            "assistant",
            "main",
            r#"{"file_path":"src/a.rs"}"#,
        );
        edit.kind = "tool_input".to_owned();
        edit.tool_name = Some("Edit".to_owned());
        edit.tool_call_id = Some("edit-call".to_owned());
        let mut read = edit.clone();
        read.reference.id = "read".to_owned();
        read.tool_name = Some("Read".to_owned());
        let mut incomplete = edit.clone();
        incomplete.reference.id = "incomplete".to_owned();
        incomplete.truncated = true;
        let report = event("report", 2, "assistant", "main", "I invoked Edit.");
        let actions = vec![edit, read, incomplete, report];
        for rule in [
            "Do not call the `Edit` tool.",
            "Never invoke the tool named `Edit`.",
        ] {
            let plan = build_assessment_plan(input(actions.clone(), rule));
            let ids = plan
                .comparisons
                .iter()
                .map(|c| c.reference.action_id.as_str())
                .collect::<BTreeSet<_>>();
            assert_eq!(ids, BTreeSet::from(["edit"]));
        }
        for rule in [
            "Do not call the `Edit` tool when saving.",
            "Do not call the `Edit` tool or `Read`.",
            "Never invoke the tool named `Edit` unless approved.",
        ] {
            assert_eq!(
                build_assessment_plan(input(actions.clone(), rule))
                    .coverage
                    .candidate_pairs,
                2
            );
        }
    }

    #[test]
    fn typed_edit_paths_score_without_exclusion() {
        use crate::analysis::jev::{JevInputField, JevNormalizedFields};
        let mut actions = Vec::new();
        for (index, paths) in [
            r#"{"paths":["src/other.rs"]}"#,
            r#"{"paths":["src/target.rs","src/other.rs"]}"#,
        ]
        .into_iter()
        .enumerate()
        {
            let mut action = event(
                &format!("edit-{index}"),
                index as i64,
                "assistant",
                "main",
                "{} ",
            );
            action.kind = "tool_input".to_owned();
            action.tool_name = Some("apply_patch".to_owned());
            action.tool_call_id = Some(format!("edit-call-{index}"));
            action.normalized_fields = Some(JevNormalizedFields {
                category: None,
                values: BTreeMap::from([(JevInputField::FileEditPath, paths.to_owned())]),
                malformed: false,
            });
            actions.push(action);
        }
        let mut malformed = actions[0].clone();
        malformed.reference.id = "malformed".to_owned();
        malformed.normalized_fields.as_mut().unwrap().malformed = true;
        actions.push(malformed);
        let mut unnamed = actions[0].clone();
        unnamed.reference.id = "unnamed".to_owned();
        unnamed.tool_name = None;
        actions.push(unnamed);
        actions.push(event(
            "report",
            3,
            "assistant",
            "main",
            "I edited src/target.rs.",
        ));
        let plan = build_assessment_plan(input(
            actions.clone(),
            "Do not request an edit to the literal path `src/target.rs`.",
        ));
        assert_eq!(plan.coverage.candidate_pairs, 3);
        assert_eq!(plan.comparisons.len(), 1);
        assert_eq!(plan.comparisons[0].reference.action_id, "edit-1");
        for rule in [
            "Do not edit `src/target.rs`.",
            "Do not request an edit to the literal path `src/target.rs` when publishing.",
        ] {
            assert_eq!(
                build_assessment_plan(input(actions.clone(), rule))
                    .coverage
                    .candidate_pairs,
                3
            );
        }
    }

    #[test]
    fn jev_context_omits_local_source_paths_and_line_locations() {
        let plan = build_assessment_plan(input(
            vec![command("action", 10, "git push --force")],
            "# Workflow\n- Do not run `git push --force`.",
        ));
        let comparison = &plan.comparisons[0];
        let context = initial_context(comparison);
        let instruction = &context["instruction"];

        assert!(instruction.get("text").is_some());
        assert!(instruction.get("provenance").is_some());
        assert!(instruction.get("scope").is_some());
        assert!(instruction.get("source").is_none());
        assert!(instruction.get("start_line").is_none());
        assert!(instruction.get("end_line").is_none());

        let mut previous_context = context.clone();
        previous_context["instruction"]["source"] = json!(comparison.reference.source);
        previous_context["instruction"]["start_line"] = json!(comparison.reference.start_line);
        previous_context["instruction"]["end_line"] = json!(comparison.reference.end_line);
        let current_bytes = serde_json::to_vec(&context).unwrap().len();
        let previous_bytes = serde_json::to_vec(&previous_context).unwrap().len();
        assert!(current_bytes < previous_bytes);
    }

    #[test]
    fn capped_comparisons_prioritize_later_actions() {
        let actions: Vec<_> = (0..MAX_ASSESSMENT_CANDIDATES + 5)
            .map(|index| command(&format!("action-{index}"), index as i64, "git push --force"))
            .collect();
        let plan = build_assessment_plan(input(actions.clone(), "Do not run `git push --force`."));
        assert!(plan.coverage.sampled_pass);
        assert_eq!(plan.comparisons.len(), MAX_ASSESSMENT_CANDIDATES);
        assert!(
            plan.comparisons
                .iter()
                .any(|comparison| comparison.reference.action_id
                    == format!("action-{}", MAX_ASSESSMENT_CANDIDATES + 4))
        );
        let mut next = input(actions, "Do not run `git push --force`.");
        next.comparison_after = plan.next_comparison_cursor.clone();
        let next = build_assessment_plan(next);
        assert!(plan.next_comparison_cursor.is_none());
        assert_eq!(next.comparisons, plan.comparisons);
        assert!(!next.coverage.processing_limit_reached);
        assert_eq!(next.coverage.unselected_pairs, 5);
    }

    #[test]
    fn bounded_selection_keeps_direct_matches_for_two_rules_and_actions() {
        let instruction = snapshot_from_text(
            "AGENTS.md",
            "- Do not run `git push --force`.\n- Do not run `npm publish`.".to_owned(),
            InstructionProvenance::RecordedInjection,
            InstructionScope::Project,
        )
        .unwrap();
        let mut input = input(
            (0..48)
                .map(|index| {
                    command(
                        &format!("action-{index}"),
                        index,
                        if index % 2 == 0 {
                            "git push --force"
                        } else {
                            "npm publish"
                        },
                    )
                })
                .collect(),
            "unused",
        );
        input.content.instructions = vec![instruction];
        let plan = build_assessment_plan(input);
        let rules = plan
            .comparisons
            .iter()
            .map(|comparison| comparison.reference.rule_id.as_str())
            .collect::<BTreeSet<_>>();
        let actions = plan
            .comparisons
            .iter()
            .map(|comparison| comparison.reference.action_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(plan.comparisons.len(), MAX_ASSESSMENT_CANDIDATES);
        assert!(rules.len() > 1);
        assert!(actions.len() > 1);
        assert!(plan.coverage.sampled_pass);
        assert_eq!(plan.coverage.candidate_pairs, 2 * 48);
        assert!(plan.coverage.unselected_pairs > 0);
    }

    #[test]
    fn unbound_tool_results_are_not_candidates_or_assessment_context() {
        let mut tool_result = event(
            "tool-result",
            1,
            "tool",
            "main",
            "The command completed successfully.",
        );
        tool_result.kind = "tool_result".to_owned();
        tool_result.tool_name = Some("shell".to_owned());
        tool_result.tool_call_id = Some("call-1".to_owned());
        let mut tool_input = event("tool-input", 2, "tool", "main", "rg -n pattern src");
        tool_input.authority = "assistant".to_owned();
        tool_result.authority = "tool".to_owned();
        tool_input.kind = "tool_input".to_owned();
        tool_input.tool_name = Some("Bash".to_owned());
        tool_input.tool_call_id = Some("call-1".to_owned());

        let plan = build_assessment_plan(input(vec![tool_result, tool_input], "Do not run `rg`."));

        assert!(!plan.comparisons.is_empty());
        assert!(
            plan.comparisons
                .iter()
                .all(|comparison| { comparison.reference.action_id != "tool-result" })
        );
        assert!(plan.comparisons.iter().all(|comparison| {
            comparison
                .context
                .iter()
                .all(|context| context.action_id != "tool-result")
        }));
    }

    #[test]
    fn large_global_instruction_does_not_starve_project_instruction_on_first_page() {
        let mut assessment_input = input(
            vec![
                command("action", 1, "git push --force"),
                command("action-two", 2, "git push --force"),
                command("action-three", 3, "git push --force"),
                command("action-four", 4, "git push --force"),
            ],
            "",
        );
        let global = snapshot_from_text(
            "home:.config/opencode/AGENTS.md",
            (0..80)
                .map(|index| format!("- Do not run `git push --force` for global policy {index}."))
                .collect::<Vec<_>>()
                .join("\n"),
            InstructionProvenance::CurrentFileComparison,
            InstructionScope::Global,
        )
        .unwrap();
        let project = snapshot_from_text(
            "project:AGENTS.md",
            "- Do not run `git push --force`.".to_owned(),
            InstructionProvenance::CurrentFileComparison,
            InstructionScope::Project,
        )
        .unwrap();
        assessment_input.content.instructions = vec![global, project];

        let plan = build_assessment_plan(assessment_input.clone());

        assert!(plan.coverage.sampled_pass);
        assert_eq!(plan.coverage.skipped_rules.len(), 80);
        assert!(
            plan.comparisons
                .iter()
                .all(|comparison| comparison.reference.source == "project:AGENTS.md")
        );
        assert!(
            plan.comparisons
                .iter()
                .any(|comparison| { comparison.reference.source == "project:AGENTS.md" })
        );
        assert_eq!(plan.coverage.instruction_sources.len(), 2);
        assert!(
            plan.coverage
                .instruction_sources
                .iter()
                .any(|source| source.source == "project:AGENTS.md"
                    && source.selected_comparisons == 2)
        );
        assert!(
            plan.coverage
                .instruction_sources
                .iter()
                .any(|source| source.selected_comparisons < source.candidate_pairs)
        );
    }

    #[test]
    fn sampled_pass_stops_after_bounded_selection() {
        let assessment_input = input(
            (0..100)
                .map(|index| command(&format!("action-{index}"), index, "git push --force"))
                .collect(),
            &(0..8)
                .map(|index| format!("- Do not run `git push --force` for step {index}."))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let plan = build_assessment_plan(assessment_input);
        assert_eq!(plan.coverage.candidate_pairs, 800);
        assert_eq!(plan.comparisons.len(), MAX_ASSESSMENT_CANDIDATES);
        assert!(plan.coverage.sampled_pass);
        assert_eq!(
            plan.coverage.unselected_pairs,
            800 - MAX_ASSESSMENT_CANDIDATES
        );
        assert!(plan.next_comparison_cursor.is_none());
    }

    #[test]
    fn cached_top_k_history_matches_the_full_sort_reference() {
        let mut actions = (0..16)
            .map(|index| {
                let text = match index {
                    0 => "The release checklist is ready.",
                    3 => "Approval was recorded for the release.",
                    7 => "The release tests passed.",
                    10 => "A release note was drafted.",
                    _ => "Reviewed an unrelated documentation detail.",
                };
                event(
                    &format!("history-{index}"),
                    i64::from(index),
                    "assistant",
                    "main",
                    text,
                )
            })
            .collect::<Vec<_>>();
        actions.push(command("release-action", 20, "git push --force"));
        let assessment = input(actions.clone(), "Do not run `git push --force`.");

        let plan = build_assessment_plan(assessment);
        let comparison = plan
            .comparisons
            .iter()
            .find(|comparison| comparison.reference.action_id == "release-action")
            .unwrap();
        let rule_terms = meaningful_terms(rule_text_fragment(comparison));
        let context_ids = comparison
            .context
            .iter()
            .map(|event| event.action_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut reference = actions
            .iter()
            .take(actions.len() - 1)
            .enumerate()
            .filter(|(_, action)| !context_ids.contains(action.reference.id.as_str()))
            .collect::<Vec<_>>();
        reference.sort_by(|(left_index, left), (right_index, right)| {
            history_relevance(&rule_terms, right)
                .cmp(&history_relevance(&rule_terms, left))
                .then_with(|| right_index.cmp(left_index))
        });
        let expected = reference
            .into_iter()
            .take(MAX_COUNTER_EVIDENCE)
            .map(|(_, action)| action.reference.id.as_str())
            .collect::<Vec<_>>();
        let actual = comparison
            .counterevidence
            .iter()
            .map(|event| event.action_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
        assert!(comparison.earlier_history_truncated);
    }

    #[test]
    fn long_action_text_abstains_without_sending_a_fragment() {
        let assessment_input = input(
            vec![command(
                "large-action",
                1,
                &format!("git push --force {}", "界".repeat(100_000)),
            )],
            "Do not run `git push --force`.",
        );
        let context = build_jev_context(&assessment_input).unwrap();
        let check = IgnoredInstructionsCheck;
        let plan = check.prepare(&context).unwrap();
        assert!(plan.work_items.is_empty());
        assert!(plan.prepared.comparisons.is_empty());
        assert!(
            !plan.prepared.coverage.limitations.is_empty(),
            "{:?}",
            plan.prepared.coverage
        );
        let packed = crate::analysis::jev::pack_work_items(&plan.work_items);
        assert!(packed.skipped_item_ids.is_empty());
        assert!(packed.batches.is_empty());
        assert_eq!(check.reduce(&plan, &[], true).unwrap().request_count, 0);
    }

    #[test]
    fn long_instruction_rule_abstains_with_its_late_exception_intact_locally() {
        let rule_text = format!(
            "- Do not run `git push --force`. {}\nException: recovery work is allowed.",
            "This rule applies to ordinary work. ".repeat(200)
        );
        let assessment_input = input(vec![command("action", 1, "git push --force")], &rule_text);
        let context = build_jev_context(&assessment_input).unwrap();
        let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
        assert!(plan.prepared.comparisons.is_empty());
        assert!(plan.work_items.is_empty());
        assert_eq!(plan.prepared.coverage.skipped_rules.len(), 1);
        assert!(
            assessment_input.content.instructions[0].sections[0]
                .text
                .ends_with("Exception: recovery work is allowed.")
        );
        assert_eq!(
            IgnoredInstructionsCheck
                .reduce(&plan, &[], true)
                .unwrap()
                .request_count,
            0
        );
    }

    #[test]
    fn timestamp_less_action_requires_a_captured_position_after_enablement() {
        let mut before = command("before", 1, "git push --force");
        before.reference.source_key_digest = sha256_hex(b"source");
        before.reference.turn_index = 2;
        before.timestamp_ms = None;
        let mut after = command("after", 2, "git push --force");
        after.reference.source_key_digest = sha256_hex(b"source");
        after.reference.turn_index = 3;
        after.timestamp_ms = None;
        let mut input = input(vec![before, after], "Do not run `git push --force`.");
        input.activity_after_ms = Some(1_000);
        input.boundary_positions.insert("source".to_owned(), 2);
        let plan = build_assessment_plan(input);
        assert_eq!(plan.comparisons.len(), 1);
        assert_eq!(plan.comparisons[0].reference.action_id, "after");
    }

    #[test]
    fn page_overlap_is_context_only_and_does_not_repeat_candidate_work() {
        let mut earlier = event(
            "earlier-approval",
            1,
            "user",
            "main",
            "Approval was denied for the release command.",
        );
        earlier.context_only = true;
        let plan = build_assessment_plan(input(
            vec![earlier, command("later-release", 2, "git push --force")],
            "Do not run `git push --force` without approval.",
        ));
        assert_eq!(plan.coverage.candidate_pairs, 1);
        assert_eq!(plan.comparisons.len(), 1);
        assert_eq!(plan.comparisons[0].reference.action_id, "later-release");
        assert!(
            plan.comparisons[0]
                .context
                .iter()
                .all(|event| event.action_id != "earlier-approval")
        );
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
            instruction_digest: sha256_hex(rule_text.as_bytes()),
            rule_id: rule_id.to_owned(),
            rule_heading: "Requirements".to_owned(),
            start_line: 1,
            end_line: 2,
            source: "AGENTS.md".to_owned(),
            provenance: InstructionProvenance::RecordedInjection,
            scope: InstructionScope::Project,
            action_id: action_id.to_owned(),
            action_digest: sha256_hex(action_text.as_bytes()),
            action_timestamp_ms: Some(timestamp_ms),
            action_stable: true,
        };
        let action = CounterEvidence {
            action_id: action_id.to_owned(),
            source_order: timestamp_ms as u64,
            role: "assistant".to_owned(),
            kind: "tool_input".to_owned(),
            timestamp_ms: Some(timestamp_ms),
            tool_name: Some("Bash".to_owned()),
            text: action_text.to_owned(),
            truncated: false,
        };
        CandidateComparison {
            source_binding: Some(super::super::decisions::ActionSourceBinding {
                source: event(action_id, timestamp_ms, "assistant", "thread", action_text)
                    .reference,
                authority: "assistant".to_owned(),
                content_digest: reference.action_digest.clone(),
                excerpt: Some(super::super::decisions::ActionExcerptBinding {
                    start_byte: 0,
                    end_byte: action_text.len(),
                    source_bytes: action_text.len(),
                    source_truncated: false,
                    text_digest: sha256_hex(action_text.as_bytes()),
                }),
            }),
            prerequisite_episode: None,
            id: id.to_owned(),
            reference,
            source_thread_digest: "thread".to_owned(),
            source_turn_index: timestamp_ms as u64,
            source_turn_scope: "main".to_owned(),
            rule_text: rule_text.to_owned(),
            instruction_context: Vec::new(),
            rule_text_start: 0,
            rule_text_end: rule_text.len(),
            action,
            action_text_start: 0,
            action_text_end: action_text.len(),
            context: Vec::new(),
            context_truncated: false,
            counterevidence: Vec::new(),
            earlier_history_truncated: false,
            prior_history_complete: true,
        }
    }

    fn plan(comparisons: Vec<CandidateComparison>) -> AssessmentPlan {
        AssessmentPlan {
            input_revision: "input-revision".to_owned(),
            observable_obligations: BTreeMap::new(),
            read_request_orders: BTreeMap::new(),
            earlier_read_only_actions: BTreeMap::new(),
            session_identity_digest: "session".to_owned(),
            source_generation: 2,
            source_fingerprint: Some("fingerprint".to_owned()),
            publication_fence: 4,
            activity_after_ms: None,
            model_version: ASSESSMENT_MODEL.to_owned(),
            projection_revision: ASSESSMENT_PROJECTION_REVISION,
            chunking_revision: ASSESSMENT_CHUNKING_REVISION,
            question_revision: ASSESSMENT_QUESTION_REVISION,
            reducer_revision: ASSESSMENT_REDUCER_REVISION,
            complete_input: true,
            current_action_digests: comparisons
                .iter()
                .map(|comparison| {
                    (
                        comparison.reference.action_id.clone(),
                        comparison.reference.action_digest.clone(),
                    )
                })
                .collect(),
            current_rule_ids: comparisons
                .iter()
                .map(|comparison| {
                    (
                        comparison.reference.instruction_id.clone(),
                        comparison.reference.instruction_digest.clone(),
                        comparison.reference.rule_id.clone(),
                    )
                })
                .collect(),
            coverage: AssessmentCoverage {
                eligible_rules: 1,
                candidate_pairs: comparisons.len(),
                selected_comparisons: comparisons.len(),
                unselected_pairs: 0,
                skipped_rules: Vec::new(),
                skipped_actions: Vec::new(),
                processing_limit_reached: false,
                sampled_pass: false,
                selector_revision: 0,
                limitations: Vec::new(),
                reassessed_comparison_ids: Vec::new(),
                reassessed_rule_ids: Vec::new(),
                reassessed_finding_ids: Vec::new(),
                instruction_sources: Vec::new(),
            },
            comparisons,
            next_comparison_cursor: None,
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
        evidence_basis: &str,
    ) -> JevWorkItemResult {
        let decision = if evidence_basis != "self_contained" {
            "uncertain"
        } else if relationship == "conflict" {
            "conflict"
        } else if matches!(relationship, "follows" | "unrelated" | "no_conflict") {
            "no_conflict"
        } else {
            "uncertain"
        };
        let answers = BTreeMap::from([(
            QUESTION_DECISION.to_owned(),
            choice_answer(decision, &["conflict", "no_conflict", "uncertain"]),
        )]);
        JevWorkItemResult {
            request_id: format!("request-{work_item_id}"),
            work_item_id: work_item_id.to_owned(),
            answers,
            evidence: Vec::new(),
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
    fn mismatched_choice_uses_probabilities_and_weak_answers_do_not_fail() {
        let comparison = build_assessment_plan(input(
            vec![command("action", 10, "git push --force")],
            "Do not run `git push --force`.",
        ))
        .comparisons
        .remove(0);
        let work_item_id = comparison.id.clone();
        let mut result = judgment_result(&work_item_id, "follows", "self_contained");
        let JevAnswer::Choice {
            choice,
            probabilities,
            ..
        } = result.answers.get_mut(QUESTION_DECISION).unwrap()
        else {
            panic!("relationship is a Choice")
        };
        *choice = "conflict".to_owned();
        assert_eq!(probabilities["no_conflict"], 0.97);
        let follows = reduce_one(comparison.clone(), vec![result.clone()]);
        assert!(follows.findings.is_empty());
        assert!(follows.unassessed_comparisons.is_empty());

        *probabilities_for(&mut result, QUESTION_DECISION) = BTreeMap::from([
            ("conflict".to_owned(), 0.6),
            ("no_conflict".to_owned(), 0.4),
            ("uncertain".to_owned(), 0.0),
        ]);
        let weak = reduce_one(comparison.clone(), vec![result.clone()]);
        assert!(weak.findings.is_empty());
        assert_eq!(weak.unassessed_comparisons, vec![comparison.id.clone()]);

        *probabilities_for(&mut result, QUESTION_DECISION) = BTreeMap::from([
            ("conflict".to_owned(), 0.97),
            ("no_conflict".to_owned(), 0.015),
            ("uncertain".to_owned(), 0.015),
        ]);
        let JevAnswer::Choice { choice, .. } = result.answers.get_mut(QUESTION_DECISION).unwrap()
        else {
            unreachable!()
        };
        *choice = "no_conflict".to_owned();
        let conflict = reduce_one(comparison, vec![result]);
        assert_eq!(conflict.findings.len(), 1);
    }

    #[test]
    fn many_unassessed_comparisons_fit_in_a_bounded_result() {
        let result = AssessmentResult {
            input_revision: "revision".to_owned(),
            model_version: ASSESSMENT_MODEL.to_owned(),
            findings: Vec::new(),
            pending_rules: Vec::new(),
            unassessed_comparisons: (0..16_000)
                .map(|index| sha256_hex(format!("comparison-{index}").as_bytes()))
                .collect(),
            coverage: AssessmentCoverage {
                eligible_rules: 57,
                candidate_pairs: 16_000,
                selected_comparisons: 16_000,
                unselected_pairs: 0,
                skipped_rules: Vec::new(),
                skipped_actions: Vec::new(),
                processing_limit_reached: false,
                sampled_pass: false,
                selector_revision: 0,
                limitations: vec!["some_comparisons_unassessed".to_owned()],
                reassessed_comparison_ids: Vec::new(),
                reassessed_rule_ids: Vec::new(),
                reassessed_finding_ids: Vec::new(),
                instruction_sources: Vec::new(),
            },
            request_count: 1000,
            input_tokens: 1_000_000,
            output_tokens: 250_000,
        };
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(
            serialized.len() < 1024 * 1024,
            "result is {} bytes",
            serialized.len()
        );
        let restored: AssessmentResult = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            restored.unassessed_comparisons,
            result.unassessed_comparisons
        );
    }

    fn probabilities_for<'a>(
        result: &'a mut JevWorkItemResult,
        question: &str,
    ) -> &'a mut BTreeMap<String, f64> {
        let JevAnswer::Choice { probabilities, .. } = result.answers.get_mut(question).unwrap()
        else {
            panic!("test question uses Choice")
        };
        probabilities
    }

    #[test]
    fn unproven_earlier_approval_does_not_enter_assessment_context() {
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
        let mut install = command("install", 20, "npm install");
        install.reference.thread_digest = "branch".to_owned();
        actions.push(install);
        let built =
            build_assessment_plan(input(actions, "Do not run `npm install` without approval."));
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
                .all(|event| event.action_id != "approval")
        );
        assert!(
            initial_context(comparison)["earlier_counterevidence"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["text"] != "The user approved installing the dependency.")
        );
        assert!(
            initial_context(comparison)["earlier_counterevidence"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event.get("action_id").is_none())
        );
        assert!(comparison.prior_history_complete);
        assert!(comparison.counterevidence.is_empty());
    }

    #[test]
    fn an_exception_near_a_split_boundary_removes_the_conflict() {
        let mut comparison = candidate(
            "generated-file",
            "generated-rule",
            "Do not edit generated files except for an approved snapshot update.",
            "edit-generated",
            "Edited a generated file.",
            20,
        );
        comparison.counterevidence.push(CounterEvidence {
            action_id: "exception".to_owned(),
            source_order: 19,
            role: "user".to_owned(),
            kind: "assistant_text".to_owned(),
            timestamp_ms: Some(19),
            tool_name: None,
            text: "Exception: edit this generated file for the required snapshot update."
                .to_owned(),
            truncated: false,
        });
        let result = reduce_assessment(
            &plan(vec![comparison.clone()]),
            &BTreeMap::from([(
                comparison.id.clone(),
                judgment_result(&comparison.id, "follows", "self_contained"),
            )]),
            true,
        );
        assert!(result.findings.is_empty());
    }

    #[test]
    fn prerequisite_follows_judgment_not_keyword_mentions() {
        let mut before = candidate(
            "commit-before-tests",
            "test-before-commit",
            "Do not run `git commit --amend`.",
            "commit",
            "git commit --amend",
            10,
        );
        before.context.push(CounterEvidence {
            action_id: "tests".to_owned(),
            source_order: 9,
            role: "tool".to_owned(),
            kind: "tool_result".to_owned(),
            timestamp_ms: Some(9),
            tool_name: Some("test".to_owned()),
            text: "Tests failed; approval denied.".to_owned(),
            truncated: false,
        });
        let rejected = reduce_one(
            before.clone(),
            vec![judgment_result(&before.id, "conflict", "self_contained")],
        );
        assert_eq!(rejected.findings.len(), 1);

        before.context[0].text = "Focused tests passed.".to_owned();
        let accepted = reduce_one(
            before.clone(),
            vec![judgment_result(&before.id, "follows", "self_contained")],
        );
        assert!(accepted.findings.is_empty());
    }

    #[test]
    fn ordered_requirement_with_incomplete_earlier_history_stays_unassessed() {
        let mut comparison = candidate(
            "search-order",
            "search-first",
            "Use a semantic search before a text search when exploring an unfamiliar codebase.",
            "text-search",
            "Used rg to find a known symbol in the familiar project.",
            10,
        );
        comparison.earlier_history_truncated = true;
        let result = reduce_one(
            comparison.clone(),
            vec![judgment_result(
                &comparison.id,
                "conflict",
                "evidence_incomplete",
            )],
        );
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons, [comparison.id]);
    }

    #[test]
    fn direct_conditional_conflict_survives_omitted_irrelevant_history() {
        let mut comparison = candidate(
            "effect-conflict",
            "derive-during-render",
            "Do not run `git push --force` when the command includes `--force`.",
            "effect-action",
            "git push --force",
            10,
        );
        comparison.earlier_history_truncated = true;
        comparison.prior_history_complete = false;

        let result = reduce_one(
            comparison.clone(),
            vec![judgment_result(
                &comparison.id,
                "conflict",
                "self_contained",
            )],
        );

        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].reference.action_id, "effect-action");
        assert!(result.unassessed_comparisons.is_empty());
    }

    #[test]
    fn missing_prior_approval_history_stays_unassessed() {
        let mut comparison = candidate(
            "release-without-history",
            "release-approval",
            "Get maintainer approval before publishing a release.",
            "release-action",
            "Published the release.",
            10,
        );
        comparison.prior_history_complete = false;

        let result = reduce_one(
            comparison.clone(),
            vec![judgment_result(
                &comparison.id,
                "conflict",
                "evidence_incomplete",
            )],
        );

        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons, [comparison.id]);
    }

    #[test]
    fn joint_probability_threshold_filters_weak_conflicts() {
        let comparison = candidate(
            "weak-conflict",
            "prohibition",
            "Do not use a text search before semantic search.",
            "search",
            "Used rg after the repository behavior was already understood.",
            10,
        );
        let mut response = judgment_result(&comparison.id, "conflict", "self_contained");
        if let JevAnswer::Choice { probabilities, .. } =
            response.answers.get_mut(QUESTION_DECISION).unwrap()
        {
            probabilities.insert("conflict".to_owned(), 0.74);
            probabilities.insert("no_conflict".to_owned(), 0.20);
            probabilities.insert("uncertain".to_owned(), 0.06);
        }
        let result = reduce_one(comparison.clone(), vec![response]);
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons, [comparison.id]);
    }

    #[test]
    fn older_content_page_keeps_unproven_authority_unassessed() {
        let mut comparison = candidate(
            "release-candidate",
            "release-approval",
            "Get maintainer approval before publishing a release.",
            "release-action",
            "Published the release.",
            20,
        );
        comparison.prior_history_complete = false;
        let approval = event(
            "approval-on-older-page",
            10,
            "user",
            "thread",
            "The maintainer approved the release.",
        );

        let completed = extend_comparison_with_history(&comparison, &[approval], true);
        assert!(completed.prior_history_complete);
        assert!(
            completed
                .counterevidence
                .iter()
                .any(|item| item.action_id == "approval-on-older-page")
        );
        assert!(
            initial_context(&completed)["earlier_counterevidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["text"] == "The maintainer approved the release.")
        );

        let result = reduce_one(
            completed.clone(),
            vec![judgment_result(&completed.id, "follows", "self_contained")],
        );
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons, [completed.id]);
    }

    #[test]
    fn uncertain_applicability_or_exception_cannot_become_a_finding() {
        let mut comparison = candidate(
            "uncertain-context",
            "conditional-rule",
            "Run the action only when the condition applies.",
            "action",
            "Ran the action.",
            10,
        );
        let mut applicability = judgment_result(&comparison.id, "conflict", "self_contained");
        if let JevAnswer::Choice { probabilities, .. } =
            applicability.answers.get_mut(QUESTION_DECISION).unwrap()
        {
            probabilities.insert("conflict".to_owned(), 0.74);
            probabilities.insert("no_issue".to_owned(), 0.10);
            probabilities.insert("pending_completion".to_owned(), 0.10);
            probabilities.insert("uncertain".to_owned(), 0.06);
        }
        let result = reduce_one(comparison.clone(), vec![applicability]);
        assert!(result.findings.is_empty());
        assert_eq!(
            result.unassessed_comparisons.as_slice(),
            std::slice::from_ref(&comparison.id)
        );

        comparison.prior_history_complete = false;
        let mut basis = judgment_result(&comparison.id, "conflict", "self_contained");
        if let JevAnswer::Choice { probabilities, .. } =
            basis.answers.get_mut(QUESTION_DECISION).unwrap()
        {
            probabilities.insert("conflict".to_owned(), 0.74);
            probabilities.insert("no_issue".to_owned(), 0.10);
            probabilities.insert("pending_completion".to_owned(), 0.10);
            probabilities.insert("uncertain".to_owned(), 0.06);
        }
        let result = reduce_one(comparison.clone(), vec![basis]);
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons, [comparison.id]);
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
                    judgment_result(&first.id, "conflict", "self_contained"),
                ),
                (
                    second.id.clone(),
                    judgment_result(&second.id, "follows", "self_contained"),
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
                judgment_result(&comparison.id, "conflict", "self_contained"),
            )]),
            true,
        );
        assert_eq!(result.findings.len(), 1);
    }

    #[test]
    fn complete_action_has_one_bound_finding_without_fragment_comparisons() {
        let source = input(
            vec![command("same-action", 10, "git push --force")],
            "Do not run `git push --force`.",
        );
        let comparisons = build_assessment_plan(source).comparisons;
        assert_eq!(comparisons.len(), 1);
        let comparison = &comparisons[0];
        assert_eq!(comparison.action.text, "git push --force");
        assert_eq!(comparison.action_text_start, 0);
        assert_eq!(comparison.action_text_end, comparison.action.text.len());
        assert!(
            comparison
                .source_binding
                .as_ref()
                .unwrap()
                .matches(comparison)
        );
        let result = reduce_assessment(
            &plan(comparisons.clone()),
            &BTreeMap::from([(
                comparison.id.clone(),
                judgment_result(&comparison.id, "conflict", "self_contained"),
            )]),
            true,
        );

        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].reference.action_id, "same-action");
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
                judgment_result(
                    &comparison.id,
                    "insufficient_evidence",
                    "evidence_incomplete",
                ),
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
    fn current_file_comparison_never_establishes_a_clean_history_claim() {
        let mut comparison = candidate(
            "current-file-rule",
            "format-action",
            "Use the documented format.",
            "format",
            "Used the documented format.",
            10,
        );
        comparison.reference.provenance = InstructionProvenance::CurrentFileComparison;
        let result = reduce_assessment(
            &plan(vec![comparison.clone()]),
            &BTreeMap::from([(
                comparison.id.clone(),
                judgment_result(&comparison.id, "follows", "self_contained"),
            )]),
            true,
        );

        assert!(result.findings.is_empty());
        assert!(
            result
                .coverage
                .limitations
                .contains(&"current_file_not_historical_proof".to_owned())
        );
    }

    #[test]
    fn obsolete_completion_answers_abstain_instead_of_creating_pending_rules() {
        let comparison = candidate(
            "eventual-tests",
            "eventual-tests",
            "Run tests eventually before completion.",
            "change",
            "Changed the implementation.",
            10,
        );
        let mut judgment = judgment_result("eventual-tests", "follows", "self_contained");
        judgment.answers.insert(
            QUESTION_DECISION.to_owned(),
            choice_answer(
                "pending_completion",
                &["conflict", "no_issue", "pending_completion", "uncertain"],
            ),
        );
        let result = reduce_one(comparison.clone(), vec![judgment]);
        assert!(result.pending_rules.is_empty());
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons, [comparison.id]);
    }

    #[test]
    fn instruction_wording_alone_does_not_create_a_pending_completion_rule() {
        let comparison = candidate(
            "completion-not-implied",
            "tests",
            "Run tests eventually before completion.",
            "change",
            "Changed the implementation.",
            10,
        );
        let result = reduce_one(
            comparison.clone(),
            vec![judgment_result(&comparison.id, "follows", "self_contained")],
        );

        assert!(result.pending_rules.is_empty());
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
            "Do not run the release command in a production environment.",
            "release-action",
            "Ran the release command.",
            10,
        );
        let result = reduce_assessment(
            &plan(vec![old.clone(), new.clone()]),
            &BTreeMap::from([
                (
                    old.id.clone(),
                    judgment_result(&old.id, "conflict", "self_contained"),
                ),
                (
                    new.id.clone(),
                    judgment_result(&new.id, "conflict", "self_contained"),
                ),
            ]),
            true,
        );
        assert_eq!(result.findings.len(), 2);
        assert_ne!(result.findings[0].id, result.findings[1].id);
    }

    #[test]
    fn findings_with_the_same_rule_id_from_distinct_sources_stay_distinct() {
        let first = candidate(
            "project-rule",
            "approval-rule",
            "Do not run `git push --force`.",
            "release-action",
            "git push --force",
            10,
        );
        let mut second = first.clone();
        second.id = "global-rule".to_owned();
        second.reference.instruction_id = "global-instruction".to_owned();
        second.reference.instruction_digest = "global-digest".to_owned();
        second.reference.source = "global/AGENTS.md".to_owned();
        let result = reduce_assessment(
            &plan(vec![first.clone(), second.clone()]),
            &BTreeMap::from([
                (
                    first.id.clone(),
                    judgment_result(&first.id, "conflict", "self_contained"),
                ),
                (
                    second.id.clone(),
                    judgment_result(&second.id, "conflict", "self_contained"),
                ),
            ]),
            true,
        );

        assert_eq!(result.findings.len(), 2);
        assert_ne!(result.findings[0].id, result.findings[1].id);
    }

    #[test]
    fn initial_requests_include_bounded_earlier_evidence_and_exact_citations() {
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
                source_order: index as u64,
                role: "user".to_owned(),
                kind: "assistant_text".to_owned(),
                timestamp_ms: Some(index),
                tool_name: None,
                text: "approval context ".repeat(20),
                truncated: false,
            })
            .collect();
        comparison.earlier_history_truncated = true;
        let item = JevWorkItem {
            id: comparison.id.clone(),
            window: JevInputWindow {
                fields: initial_context(&comparison),
                evidence: comparison_evidence(&comparison),
            },
            questions: window_questions(&[&comparison]),
        };
        let packed = crate::analysis::jev::pack_work_items(&[item]);
        assert!(packed.skipped_item_ids.is_empty());
        let batch = &packed.batches[0];
        assert!(batch.serialized_bytes <= crate::analysis::jev::MAX_REQUEST_BYTES);
        assert_eq!(batch.request.questions.len(), 1);
        let evidence = &batch.evidence_owners[&comparison.id];
        assert!(evidence.iter().any(|part| {
            part.role == JevEvidenceRole::Instruction
                && part.source_id
                    == format!(
                        "{}:{}",
                        comparison.reference.instruction_id, comparison.reference.rule_id
                    )
        }));
        assert!(evidence.iter().any(|part| {
            part.role == JevEvidenceRole::Candidate && part.source_id == "install"
        }));
        assert!(!batch.request.state.to_string().contains("\"install\""));
        let state_events =
            batch.request.state["work_items"][0]["context"]["earlier_counterevidence"]
                .as_array()
                .unwrap()
                .iter()
                .collect::<Vec<_>>();
        assert_eq!(state_events.len(), 8);
        assert!(
            state_events
                .iter()
                .all(|event| event.get("action_id").is_none())
        );
        assert_eq!(
            batch.evidence_owners[&comparison.id]
                .iter()
                .filter(|evidence| evidence.role == JevEvidenceRole::SupportingContext)
                .count(),
            8
        );
        assert_ne!(batch.request.state["work_items"][0]["id"], comparison.id);
    }

    #[test]
    fn context_from_another_branch_does_not_count_as_an_exception() {
        let mut install = command("branch-install", 2, "npm install");
        install.reference.thread_digest = "branch-main".to_owned();
        let actions = vec![
            event(
                "other-branch-approval",
                1,
                "user",
                "branch-other",
                "Approved installing the dependency.",
            ),
            install,
        ];
        let built =
            build_assessment_plan(input(actions, "Do not run `npm install` without approval."));
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
