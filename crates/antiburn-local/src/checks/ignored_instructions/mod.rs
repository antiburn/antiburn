//! Source evidence and instruction snapshots for the Ignored Instructions check.
//!
//! This module only prepares bounded local evidence. It does not perform
//! inference, schedule work, or interpret vendor transcript formats itself.

mod action_context;
mod assessment;
mod context_policy;
mod decisions;
mod discovery;
mod evidence;
mod instructions;
mod planning;
mod questions;
mod selected_context;

pub use planning::{
    SamplingLedger, build_assessment_plan, build_assessment_plan_with_capabilities,
    build_assessment_plan_with_sampling, build_jev_context, build_jev_context_with_capabilities,
    build_jev_context_with_sampling, extend_comparison_with_history,
    extend_comparison_with_history_and_capabilities, extend_jev_context_with_history,
    extend_jev_context_with_history_and_capabilities,
};

pub const CHECK_ID: &str = "ignored_instructions";
pub use action_context::PathChangePolicy;
pub use context_policy::PrerequisiteContextPolicy;
pub use planning::build_jev_context_with_context_policy;

pub use decisions::{
    ActionExcerptBinding, ActionSourceBinding, CitationClaim, CitationProof, DecisionCoverage,
    DecisionRecord, EvidenceIdentity, PrerequisiteEpisode, PrerequisiteOutcome,
};

pub use evidence::{
    ContentAction, ContentEventReference, ContentReferenceResolution, SessionContentEvidence,
    content_action_digest, field_capability, prepare_session_content, resolve_content_reference,
    select_session_content, source_supported,
};

pub use crate::analysis::jev::{
    JevAnswer, JevError, JevEvidenceRequirements, JevFieldAvailability, JevFieldAvailabilityState,
    JevFieldCapability, JevInputField, JevInputSelection, JevNormalizedCategory,
    JevNormalizedFields, JevQuestion, JevRequest, JevResponse, JevUsage,
};
pub use assessment::{
    ASSESSMENT_CHUNKING_REVISION, ASSESSMENT_MODEL, ASSESSMENT_PROJECTION_REVISION,
    ASSESSMENT_QUESTION_REVISION, ASSESSMENT_REDUCER_REVISION, AssessmentCoverage,
    AssessmentFinding, AssessmentInput, AssessmentPlan, AssessmentResult, CandidateComparison,
    ComparisonDiagnostic, ComparisonJudgment, CompletionCoverage, CounterEvidence,
    FindingCertainty, INPUT_SELECTION, IgnoredInstructionsCheck, InstructionSourceCoverage,
    MAX_ASSESSMENT_CANDIDATES, MAX_SAMPLED_COMPARISONS_PER_PASS, PendingRule, RuleActionRef,
    RuleStatus, diagnose_assessment, evaluator_revision, reduce_assessment,
};
pub use discovery::{
    InstructionAdapter, InstructionDiscovery, MAX_INSTRUCTION_FILES, MAX_INSTRUCTION_TOTAL_BYTES,
    discover_current_instructions,
};
pub use instructions::{
    InstructionContentClass, InstructionContextRange, InstructionProvenance,
    InstructionRuleSection, InstructionScope, InstructionSnapshot, MAX_INSTRUCTION_BYTES,
    MAX_RULE_SECTION_BYTES, MarkdownLimit, chunk_text_with_ranges, segment_markdown, sha256_hex,
    snapshot_from_text,
};
