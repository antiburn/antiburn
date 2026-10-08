//! Exact burn-check targets and direct remediation actions.

#[cfg(test)]
mod citation_tests;
mod config;
#[cfg(test)]
mod coverage_contract;
mod display;
mod models;
mod recovery;
mod stored;
mod target;
mod vendors;
mod watch;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, SourceFormat};
use antiburn_local::insights::DetectorId;
use antiburn_local::model::AgentKind;
use antiburn_local::model_catalog::{
    ModelCatalog, ModelState, ModelTarget, ReviewedModelCatalog, Support, model_control_target,
};
use antiburn_local::pricing::ModelPricing;
use antiburn_local::remediation::{
    Finding, FindingAssessment, FindingCause, FindingUnavailableReason, NamedResourceAssessment,
    NamedResourceVerificationTarget, OldModelSavingsEstimate, OldModelSavingsInput,
    OldModelSavingsUnknownReason, OldModelVerificationTarget, REMEDIATION_POLICY_REVISION,
    RemediationUnavailableReason, SAVINGS_METHOD_REVISION, SavingsEstimateInput,
    SavingsEstimateMethod, SavingsInterval, SavingsValue, TargetAssessment,
    VERIFICATION_METHOD_REVISION, VerificationOutcome, VerificationStage,
    VerificationUnknownReason, built_in_tool_remediation_supported, estimate_old_model_savings,
    estimate_savings, fallback_remediation_prompt, remediation_prompt,
    verification_evidence_supported, verify_named_resource_watch, verify_old_model,
    verify_prompt_watch,
};
use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::json;
use sha2::Sha256;

use crate::agent_config::{
    AgentConfigEditor, ConfigContext, ConfigOperation, ConfigScope, ConfigSetting,
    PreparedOperation,
};
use crate::dto::{ChecksCategoryLifecyclePayload, ChecksReportPayload};
use crate::insights_report::{self, CurrentFinding, CurrentFindingsRequest};
use crate::store::{PassiveRemediation, SessionKey};
use crate::store::{
    Remediation, RemediationContribution, RemediationDisplaySnapshot, RemediationEvidenceGuard,
    RemediationRecord, RemediationResult, RemediationState, Store,
};
use vendors::{ActionSupport, RemediationAction, vendor_policy};

use config::*;
pub(crate) use config::{PublicationSettingAttribution, publication_config_attribution};
pub(crate) use config::{config_context, managed_configuration_present, runtime_override_present};
#[cfg(test)]
pub(crate) use config::{hashed_workspace_key, publication_config_attribution_with_home};
use display::*;
pub use models::*;
pub(crate) use recovery::recover_uncertain_write;
pub(crate) use stored::WatchDefinition;
use stored::{
    StoredDisplaySnapshot, parse_display_snapshot, parse_watch_definition, stored_result,
    validate_envelope_version,
};
pub(crate) use target::passive_remediations;
use target::*;
pub(crate) use watch::evaluate_dirty_remediation;

fn representative_paths(
    store: &Store,
    keys: impl IntoIterator<Item = SessionKey>,
) -> Result<Vec<String>, ControllerError> {
    let keys = keys
        .into_iter()
        .take(MAX_PASSIVE_CANDIDATES)
        .collect::<Vec<_>>();
    let records = store
        .session_records_for_session_keys(&keys)
        .map_err(|_| ControllerError::Internal)?;
    let mut paths = Vec::new();
    for key in keys {
        let Some(record) = records.iter().find(|record| record.key == key) else {
            continue;
        };
        if record.source_kind != "file"
            || !(Path::new(&record.source_label).is_absolute()
                || record.source_label.starts_with('/'))
        {
            continue;
        }
        if !paths.contains(&record.source_label) {
            paths.push(record.source_label.clone());
            if paths.len() == 3 {
                break;
            }
        }
    }
    Ok(paths)
}

const ID_TTL: Duration = Duration::from_secs(10 * 60);
/// Per-detector cap on cached target ids. Two full listings fit, so the ids a
/// window still holds survive one background relist of the same check.
const TARGET_CACHE_LIMIT: usize = 6 * MAX_TARGETS;
const MAX_TARGETS: usize = 100;
const MAX_CHECK_PROMPT_TARGETS: usize = 100;
const MAX_PASSIVE_CANDIDATES: usize = 512;
const MAX_REMEDIATION_PROGRESS_RECORDS: usize = 1_000;
const PREPARED_CACHE_LIMIT: usize = 8;
const PREPARED_CACHE_BYTES: usize = 4 * 1024 * 1024;
const TARGET_DOMAIN: &[u8] = b"antiburn/remediation-target/v2\0";
const PROMPT_REFERENCE_PREFIX: &str = "Remediation reference: ABR-";
const SMART_CHECKS_ENABLED_AT_KEY: &str = "internal:burnChecksEnabledAtEpochV1";

fn smart_check_master_generation(store: &Store, detector: DetectorId) -> Option<String> {
    crate::jev::worker::registered_check_ids()
        .contains(&detector)
        .then(|| store.internal_value(SMART_CHECKS_ENABLED_AT_KEY))
        .flatten()
}

fn smart_check_master_available(store: &Store, detector: DetectorId) -> bool {
    !crate::jev::worker::registered_check_ids().contains(&detector)
        || store.internal_value(SMART_CHECKS_ENABLED_AT_KEY).is_some()
}

fn unavailable_instruction_evidence() -> BurnCheckTargetEvidence {
    BurnCheckTargetEvidence {
        decision_proof: None,
        status: BurnCheckEvidenceStatus::Unavailable,
        items: Vec::new(),
        occurrences: Vec::new(),
    }
}

fn stored_skill_opportunity_evidence(cause: &FindingCause) -> Option<BurnCheckTargetEvidence> {
    let FindingCause::SkillOpportunity {
        evidence: Some(evidence),
        ..
    } = cause
    else {
        return None;
    };
    let comparison = &evidence.comparison;
    let mut skill_limits = vec!["The current skill does not prove past availability."];
    if comparison.skill.reference.partial {
        skill_limits.push("Only selected byte ranges support this recommendation. The full reference was not reviewed.");
    }
    if comparison.limitations.contains(
        &antiburn_local::checks::skill_opportunities::SkillOpportunityLimit::CreationTimeUnknown,
    ) {
        skill_limits.push("Creation time is unavailable.");
    }
    if comparison.limitations.contains(
        &antiburn_local::checks::skill_opportunities::SkillOpportunityLimit::WorkTimeUnknown,
    ) {
        skill_limits.push("The recorded work time is unavailable.");
    }
    let mut items = vec![BurnCheckEvidenceItem {
        label: BurnCheckEvidenceLabel::Instruction,
        source_label: comparison.skill.name.clone(),
        reference: comparison.skill.identity.clone(),
        observed_at_ms: None,
        start_line: None,
        end_line: None,
        excerpt: bounded_evidence_excerpt(&comparison.skill.description),
        explanation: match comparison.skill.reference.source {
            antiburn_local::checks::skill_opportunities::SkillReferenceSource::Description => "This is selected text from the current skill description.",
            antiburn_local::checks::skill_opportunities::SkillReferenceSource::MarkdownFallback => "This is selected text from the current skill Markdown file. The skill has no description.",
        }.to_owned(),
        limitation: Some(skill_limits.join(" ")),
    }];
    items.extend(
        comparison.work.iter().map(|work| BurnCheckEvidenceItem {
            label: BurnCheckEvidenceLabel::ObservedAction,
            source_label: "Recorded work".to_owned(),
            reference: work.reference.id.clone(),
            observed_at_ms: work.timestamp_ms,
            start_line: None,
            end_line: None,
            excerpt: bounded_evidence_excerpt(&work.text),
            explanation:
                "This observed work could benefit from the selected current skill reference."
                    .to_owned(),
            limitation: Some(evidence.absence_limit.clone()),
        }),
    );
    items.extend(
        comparison
            .used_current_skills
            .iter()
            .map(|skill| BurnCheckEvidenceItem {
                label: BurnCheckEvidenceLabel::Context,
                source_label: format!("Current used-skill reference · {}", skill.name),
                reference: skill.identity.clone(),
                observed_at_ms: None,
                start_line: None,
                end_line: None,
                excerpt: bounded_evidence_excerpt(&skill.description),
                explanation: match skill.reference.source {
                    antiburn_local::checks::skill_opportunities::SkillReferenceSource::Description => "This selected current description supplies context about a recorded skill.",
                    antiburn_local::checks::skill_opportunities::SkillReferenceSource::MarkdownFallback => "This selected current Markdown supplies context about a recorded skill without a description.",
                }.into(),
                limitation: Some(
                    if skill.reference.partial {
                        "Only selected byte ranges were reviewed. Current text does not prove the reference at the time of work."
                    } else {
                        "Current text does not prove the reference at the time of work."
                    }.into(),
                ),
            }),
    );
    Some(BurnCheckTargetEvidence {
        decision_proof: None,
        status: BurnCheckEvidenceStatus::Available,
        items,
        occurrences: Vec::new(),
    })
}

fn complete_skill_opportunity_evidence(
    mut evidence: BurnCheckTargetEvidence,
    skill: &antiburn_local::checks::skill_opportunities::SkillOpportunityFinding,
    actions: &[antiburn_local::analysis::jev_evidence::ContentAction],
) -> BurnCheckTargetEvidence {
    for reference in &skill.comparison.use_citations {
        let Some(action) = actions.iter().find(|action| &action.reference == reference) else {
            return unavailable_instruction_evidence();
        };
        evidence.items.push(BurnCheckEvidenceItem {
            label: BurnCheckEvidenceLabel::Context,
            source_label: "Recorded skill use".into(),
            reference: reference.id.clone(),
            observed_at_ms: action.timestamp_ms,
            start_line: None,
            end_line: None,
            excerpt: bounded_evidence_excerpt(&action.text),
            explanation: "This event supports the equivalent-skill comparison.".into(),
            limitation: Some(
                "A recorded request does not prove successful skill execution.".into(),
            ),
        });
    }
    if skill.evidence.iter().any(|citation| {
        !evidence
            .items
            .iter()
            .any(|item| item.reference == citation.source_id)
    }) {
        return unavailable_instruction_evidence();
    }
    evidence
}

fn stored_instruction_evidence(cause: &FindingCause) -> Option<BurnCheckTargetEvidence> {
    let FindingCause::IgnoredInstructionConflict(evidence) = cause else {
        return None;
    };
    if evidence.instruction_excerpt.is_empty() && evidence.action_excerpt.is_empty() {
        return None;
    }
    let mut instruction_limits = Vec::new();
    if evidence.instruction_excerpt.is_empty() {
        instruction_limits.push("The instruction text was not saved with this finding.");
    }
    if evidence.instruction_excerpt_truncated || evidence.instruction_excerpt.len() > 4096 {
        instruction_limits.push("The saved instruction excerpt is incomplete.");
    }
    match evidence.provenance {
        antiburn_local::analysis::ignored_instructions::InstructionProvenance::CurrentFileComparison =>
            instruction_limits.push("Note: this session may have run on outdated instructions."),
        antiburn_local::analysis::ignored_instructions::InstructionProvenance::ObservedRead =>
            instruction_limits.push("We saw the instruction in the session, but cannot tell if it was active before the action."),
        antiburn_local::analysis::ignored_instructions::InstructionProvenance::RecordedInjection => {},
    }
    if !evidence.limitations.is_empty() {
        instruction_limits.push("The assessment records additional evidence limits. Review the action and section before deciding.");
    }
    Some(BurnCheckTargetEvidence {
        decision_proof: IgnoredInstructionDecisionProof::from_cause(cause),
        status: BurnCheckEvidenceStatus::Available,
        occurrences: Vec::new(),
        items: vec![
            BurnCheckEvidenceItem {
                label: BurnCheckEvidenceLabel::Instruction,
                source_label: evidence
                    .source
                    .strip_prefix("project:")
                    .or_else(|| evidence.source.strip_prefix("home:"))
                    .unwrap_or(&evidence.source)
                    .to_owned(),
                reference: if evidence.decision_record().is_some() {
                    format!("{}:{}", evidence.instruction_id, evidence.rule_id)
                } else {
                    evidence.rule_heading.clone()
                },
                observed_at_ms: None,
                start_line: Some(evidence.start_line),
                end_line: Some(evidence.end_line),
                excerpt: if evidence.instruction_excerpt.is_empty() {
                    "Instruction text unavailable.".to_owned()
                } else {
                    bounded_evidence_excerpt(&evidence.instruction_excerpt)
                },
                explanation: format!("Saved instruction text used for this comparison. {} conflict · {}.",
                    match evidence.certainty {
                        antiburn_local::analysis::ignored_instructions::FindingCertainty::Likely => "Likely",
                        antiburn_local::analysis::ignored_instructions::FindingCertainty::Possible => "Possible",
                    },
                    match evidence.provenance {
                        antiburn_local::analysis::ignored_instructions::InstructionProvenance::RecordedInjection => "recorded instruction",
                        antiburn_local::analysis::ignored_instructions::InstructionProvenance::ObservedRead => "instruction seen in the session",
                        antiburn_local::analysis::ignored_instructions::InstructionProvenance::CurrentFileComparison => "current instruction file comparison; historical activation is unconfirmed",
                    }),
                limitation: (!instruction_limits.is_empty()).then(|| instruction_limits.join(" ")),
            },
            BurnCheckEvidenceItem {
                label: BurnCheckEvidenceLabel::ObservedAction,
                source_label: "Session action".to_owned(),
                reference: evidence.action_id.clone(),
                observed_at_ms: evidence.action_timestamp_ms,
                start_line: None,
                end_line: None,
                excerpt: if evidence.action_excerpt.is_empty() {
                    "Action text unavailable.".to_owned()
                } else {
                    bounded_evidence_excerpt(&evidence.action_excerpt)
                },
                explanation: evidence.decision_record().and_then(|decision| decision.contrast_template())
                    .unwrap_or("Saved action text that Antiburn compared with the instruction.").to_owned(),
                limitation: if evidence.action_excerpt.is_empty() {
                    Some("The action text was not saved with this finding.".to_owned())
                } else { (evidence.action_excerpt_truncated || evidence.action_excerpt.len() > 4096)
                    .then(|| "The saved action excerpt is incomplete.".to_owned())
                },
            },
        ],
    })
}

fn bounded_evidence_excerpt(text: &str) -> String {
    let end = text
        .char_indices()
        .take_while(|(index, character)| *index + character.len_utf8() <= 4096)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    text[..end].to_owned()
}

fn merge_instruction_evidence(
    saved: Option<BurnCheckTargetEvidence>,
    validated: BurnCheckTargetEvidence,
) -> BurnCheckTargetEvidence {
    let Some(mut saved) = saved else {
        return validated;
    };
    if saved.decision_proof.is_some() && validated.status == BurnCheckEvidenceStatus::Unavailable {
        return unavailable_instruction_evidence();
    }
    saved.decision_proof = validated.decision_proof;
    let action = &mut saved.items[1];
    action.explanation = saved.decision_proof.as_ref().map_or_else(
        || "Saved action text that Antiburn compared with the instruction.".to_owned(),
        |proof| proof.contrast.clone(),
    );
    let context_limit = if validated.status == BurnCheckEvidenceStatus::Unavailable {
        Some("Session context is unavailable or cannot be validated against the saved action and source.".to_owned())
    } else {
        validated
            .items
            .iter()
            .find(|item| item.label == BurnCheckEvidenceLabel::ObservedAction)
            .and_then(|item| item.limitation.clone())
    };
    if let Some(limit) = context_limit {
        action.limitation = Some(match action.limitation.take() {
            Some(existing) => format!("{existing} {limit}"),
            None => limit,
        });
    }
    saved.items.extend(
        validated
            .items
            .into_iter()
            .filter(|item| item.label == BurnCheckEvidenceLabel::Context),
    );
    saved
}

fn available_evidence_references(
    action_id: &str,
    context_ids: &BTreeSet<&str>,
    available_ids: &BTreeSet<String>,
) -> Option<(Vec<String>, bool)> {
    if !available_ids.contains(action_id) {
        return None;
    }
    let missing_context = context_ids
        .iter()
        .any(|reference| !available_ids.contains(*reference));
    let mut references = vec![action_id.to_owned()];
    references.extend(
        context_ids
            .iter()
            .filter(|reference| available_ids.contains(**reference))
            .map(|reference| (*reference).to_owned()),
    );
    Some((references, missing_context))
}

fn validated_evidence_references(
    evidence: &antiburn_local::remediation::IgnoredInstructionConflictEvidence,
    actions: &BTreeMap<String, antiburn_local::analysis::jev_evidence::ContentAction>,
) -> Option<(Vec<String>, bool)> {
    use antiburn_local::analysis::ignored_instructions::content_action_digest;
    let anchor = actions.get(&evidence.action_id)?;
    if evidence.action_digest.is_empty()
        || content_action_digest(anchor) != evidence.action_digest
        || (evidence.action_timestamp_ms.is_some()
            && anchor.timestamp_ms != evidence.action_timestamp_ms)
    {
        return None;
    }
    let decision = evidence.decision_record();
    if let Some(decision) = decision
        && (anchor.reference != decision.action_anchor.source
            || anchor.authority != decision.action_authority)
    {
        return None;
    }
    let context_ids = evidence
        .nearby_context_ids
        .iter()
        .chain(&evidence.counterevidence_ids)
        .map(String::as_str)
        .chain(decision.into_iter().flat_map(|decision| {
            decision
                .selected_evidence
                .iter()
                .map(|identity| identity.source.id.as_str())
        }))
        .filter(|id| *id != evidence.action_id)
        .collect();
    let available = actions
        .iter()
        .filter(|(_, action)| {
            action.reference.source_key_digest == anchor.reference.source_key_digest
                && action.reference.thread_digest == anchor.reference.thread_digest
                && decision.is_none_or(|decision| {
                    decision
                        .selected_evidence
                        .iter()
                        .find(|identity| identity.source.id == action.reference.id)
                        .is_none_or(|identity| {
                            identity.source == action.reference
                                && identity.content_digest == content_action_digest(action)
                        })
                })
        })
        .map(|(id, _)| id.clone())
        .collect();
    let (mut references, missing) =
        available_evidence_references(&evidence.action_id, &context_ids, &available)?;
    if decision.is_some() && missing {
        return None;
    }
    references.sort_by_key(|id| {
        let action = &actions[id];
        (
            action.timestamp_ms,
            action.reference.turn_index,
            action.reference.part_index,
        )
    });
    Some((references, missing))
}

fn validated_instruction_content(
    session_identity: &str,
    source_format: SourceFormat,
    published: antiburn_local::analysis::PublishedContent,
    published_fence: i64,
    source_generation: i64,
) -> Option<antiburn_local::analysis::jev_evidence::SessionContentEvidence> {
    use antiburn_local::analysis::ignored_instructions::{
        IgnoredInstructionsCheck, prepare_session_content, select_session_content,
    };
    use antiburn_local::analysis::jev::JevCheck;

    if published.publication_fence != published_fence
        || published.source_generation != Some(source_generation)
    {
        return None;
    }
    let content = prepare_session_content(session_identity, source_format, published, Vec::new());
    Some(select_session_content(
        &content,
        IgnoredInstructionsCheck.input_selection(),
    ))
}

fn prompt_watch_supported(detectors: impl IntoIterator<Item = DetectorId>) -> bool {
    detectors.into_iter().any(|detector| {
        !matches!(
            detector,
            DetectorId::IgnoredInstructions
                | DetectorId::SkillOpportunities
                | DetectorId::OverExploring
                | DetectorId::ScopeCreep
        )
    })
}

fn resource_target_matches(
    expected: &insights_report::UnusedResourceTarget,
    current: &insights_report::UnusedResourceTarget,
) -> bool {
    expected.agent == current.agent
        && expected.kind == current.kind
        && expected
            .canonical_name
            .trim()
            .eq_ignore_ascii_case(current.canonical_name.trim())
        && expected.scope == current.scope
        && (!expected.indexed || current.indexed)
}

#[derive(Clone)]
struct CachedTarget {
    findings: Vec<CurrentFinding>,
    resource: Option<CachedResourceTarget>,
    target_key: String,
    canonical_identity: String,
    workspace_key: Option<String>,
    agent: AgentKind,
    scope_kind: String,
    scope_key: String,
    physical_target_key: Option<String>,
    config: Option<CachedConfig>,
}

/// True when the reduced report still lists this resource target.
fn resource_is_current(
    report: &insights_report::ReducedReport,
    resource: &CachedResourceTarget,
) -> bool {
    report
        .resources
        .detector(resource.finding.detector)
        .into_iter()
        .flat_map(|assessment| &assessment.targets)
        .any(|candidate| resource_target_matches(&resource.target, candidate))
}

#[derive(Clone)]
struct CachedResourceTarget {
    target: insights_report::UnusedResourceTarget,
    context: BurnCheckTargetContext,
    finding: antiburn_local::remediation::Finding,
}

impl CachedTarget {
    fn finding(&self) -> &antiburn_local::remediation::Finding {
        self.resource
            .as_ref()
            .map_or_else(|| &self.findings[0].finding, |resource| &resource.finding)
    }

    fn environment_key(&self) -> &str {
        self.resource.as_ref().map_or_else(
            || self.findings[0].environment_key.as_str(),
            |resource| resource.context.environment_key.as_str(),
        )
    }

    fn sample_sessions(&self) -> Vec<BurnCheckSampleSession> {
        // Resource evidence supports attribution and savings estimates, not sample-session UI.
        self.resource
            .as_ref()
            .map_or_else(|| sample_sessions(&self.findings), |_| Vec::new())
    }

    fn evidence_sessions(&self) -> Vec<BurnCheckSampleSession> {
        self.resource.as_ref().map_or_else(
            || sample_sessions(&self.findings),
            |resource| {
                resource
                    .target
                    .supporting_sessions
                    .iter()
                    .map(|session| BurnCheckSampleSession {
                        environment_key: session.environment_key.clone(),
                        agent: session.agent.clone(),
                        session_id: session.session_id.clone(),
                        observed_at_ms: session.observed_at_ms,
                        incarnation: None,
                    })
                    .collect()
            },
        )
    }
}

#[derive(Clone)]
struct CachedConfig {
    context: ConfigContext,
    additional_contexts: Vec<ConfigContext>,
    operation: ConfigOperation,
    physical_key: String,
    display_path: String,
}

struct TargetIdentity {
    group_key: String,
    target_key: String,
    canonical_identity: String,
    workspace_key: Option<String>,
    scope_kind: String,
    scope_key: String,
    physical_target_key: Option<String>,
}

#[derive(Clone, Copy)]
struct TargetListOptions<'a> {
    now: i64,
    home: Option<&'a Path>,
    cache_actions: bool,
}

struct TimedTarget {
    id: String,
    value: CachedTarget,
    check_preferences_revision: u64,
    smart_master_generation: Option<String>,
    created_at_epoch: i64,
}

struct PreparedAutoFix {
    id: String,
    target: CachedTarget,
    prepared: Option<PreparedOperation>,
    retained_bytes: usize,
    check_preferences_revision: u64,
    smart_master_generation: Option<String>,
    created_at_epoch: i64,
    completed: Option<AutoFixResult>,
}

#[derive(Default)]
struct ControllerState {
    /// Listed target ids, one bounded queue per detector so listing one check
    /// never evicts the ids another check's window still holds.
    targets: BTreeMap<DetectorId, VecDeque<TimedTarget>>,
    prepared: VecDeque<PreparedAutoFix>,
}

/// Caches one detector's freshly listed targets behind its own cap.
fn cache_listed_targets(
    state: &mut ControllerState,
    detector: DetectorId,
    entries: Vec<TimedTarget>,
    now: i64,
) {
    let queue = state.targets.entry(detector).or_default();
    queue.retain(|entry| now.saturating_sub(entry.created_at_epoch) <= ID_TTL.as_secs() as i64);
    for entry in entries {
        while queue.len() >= TARGET_CACHE_LIMIT {
            queue.pop_front();
        }
        queue.push_back(entry);
    }
}

pub struct RemediationController {
    data_dir: PathBuf,
    editor: AgentConfigEditor,
    state: Mutex<ControllerState>,
}

impl RemediationController {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            editor: AgentConfigEditor::new(),
            state: Mutex::new(ControllerState::default()),
        }
    }

    /// Returns bounded retained remediation attempts by exact target and cycle.
    pub fn burn_check_remediation_progress(
        &self,
        store: &Store,
    ) -> Result<BurnCheckRemediationProgress, ControllerError> {
        let mut enabled = store
            .enabled_checks()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        if store.internal_value(SMART_CHECKS_ENABLED_AT_KEY).is_none() {
            enabled
                .retain(|detector| !crate::jev::worker::registered_check_ids().contains(detector));
        }
        let records = store
            .remediations_with_display_snapshots(MAX_REMEDIATION_PROGRESS_RECORDS)
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let mut seen = BTreeSet::new();
        let mut attempts = Vec::new();
        for retained in records {
            let Ok(definition) = parse_watch_definition(&retained.record.definition_json) else {
                continue;
            };
            let Some(detector) = DetectorId::from_key(&definition.detector) else {
                continue;
            };
            if !enabled.contains(&detector) {
                continue;
            }
            let Ok(snapshot) = parse_display_snapshot(&retained.snapshot.display_snapshot_json)
            else {
                continue;
            };
            let Ok(result) = stored_result(&retained.record.result_json) else {
                continue;
            };
            let origin = match retained.snapshot.origin.as_str() {
                "passive" => RemediationOrigin::Passive,
                "action" => RemediationOrigin::Action,
                _ => continue,
            };
            let cycle_id = retained.record.remediation_id.clone();
            if !seen.insert((snapshot.finding_id.clone(), cycle_id.clone())) {
                continue;
            }
            attempts.push(BurnCheckRemediationAttempt {
                detector,
                finding_id: snapshot.finding_id,
                watch_id: cycle_id.clone(),
                remediation_cycle_id: cycle_id,
                display: snapshot.display,
                origin,
                lifecycle: retained.record.state,
                outcome: if retained.record.state == RemediationState::Fixed {
                    BurnCheckRemediationOutcome::Passed
                } else {
                    BurnCheckRemediationOutcome::Failed
                },
                verification: result.verification,
                savings: result.savings,
                effective_boundary_ms: retained.record.effective_boundary_ms,
                verified_boundary_ms: retained.snapshot.verified_boundary_ms,
                recurred_boundary_ms: retained.snapshot.recurred_boundary_ms,
                environment_key: retained.record.environment_key,
                agent: retained.record.agent,
                scope_kind: retained.record.scope_kind,
                scope_key: retained.record.scope_key,
                target_key: retained.record.target_key,
                created_at_epoch: retained.record.created_at_epoch,
                prompt_action: definition.prompt_action,
            });
        }
        attempts.sort_by(|left, right| {
            left.detector
                .index()
                .cmp(&right.detector.index())
                .then_with(|| left.finding_id.cmp(&right.finding_id))
                .then_with(|| left.remediation_cycle_id.cmp(&right.remediation_cycle_id))
        });
        Ok(BurnCheckRemediationProgress { attempts })
    }

    pub fn list_burn_check_targets(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
    ) -> Result<BurnCheckTargetList, ControllerError> {
        self.list_burn_check_targets_at(
            store,
            detector,
            context,
            TargetListOptions {
                now: now_epoch(),
                home: antiburn_local::paths::home_dir().as_deref(),
                cache_actions: true,
            },
        )
    }

    /// Applies the current target lifecycle to report categories.
    pub fn apply_category_lifecycles(
        &self,
        store: &Store,
        report: &mut ChecksReportPayload,
        environment_key: &str,
    ) -> Result<(), ControllerError> {
        let progress = self.burn_check_remediation_progress(store)?;
        for category in &mut report.categories {
            let mut action_boundaries = BTreeMap::new();
            for attempt in progress.attempts.iter().filter(|attempt| {
                attempt.detector == category.id.into()
                    && attempt.origin == RemediationOrigin::Action
                    && attempt.environment_key == environment_key
            }) {
                let Some(boundary_ms) = attempt.effective_boundary_ms else {
                    continue;
                };
                action_boundaries
                    .entry(attempt.agent.as_str())
                    .and_modify(|current: &mut i64| *current = (*current).max(boundary_ms))
                    .or_insert(boundary_ms);
            }
            let mut awaiting_evidence = false;
            for (agent, boundary_ms) in action_boundaries {
                if !insights_report::has_current_evidence_after(
                    &self.data_dir,
                    environment_key,
                    agent,
                    boundary_ms,
                )
                .map_err(|_| ControllerError::Internal)?
                {
                    awaiting_evidence = true;
                    break;
                }
            }
            category.lifecycle = resolve_category_lifecycle(
                category.id.into(),
                category.finding,
                category.clean,
                awaiting_evidence,
            );
        }
        Ok(())
    }

    fn list_burn_check_targets_at(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
        options: TargetListOptions<'_>,
    ) -> Result<BurnCheckTargetList, ControllerError> {
        let (enabled, check_preferences_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        if !enabled.contains(&detector) {
            return Ok(BurnCheckTargetList {
                targets: Vec::new(),
                sample_sessions: Vec::new(),
                truncated: false,
            });
        }
        let smart_master_generation = smart_check_master_generation(store, detector);
        if !smart_check_master_available(store, detector) {
            return Ok(BurnCheckTargetList {
                targets: Vec::new(),
                sample_sessions: Vec::new(),
                truncated: false,
            });
        }
        if matches!(
            detector,
            DetectorId::UnusedMcpServers
                | DetectorId::UnusedBuiltInTools
                | DetectorId::UnusedSkills
        ) {
            return self.list_resource_targets(
                store,
                detector,
                context,
                options,
                check_preferences_revision,
                smart_master_generation,
            );
        }
        let page = insights_report::list_current_findings(
            &self.data_dir,
            CurrentFindingsRequest {
                environment_key: context.environment_key,
                window: context.window,
                detector,
            },
        )
        .map_err(|_| ControllerError::Internal)?;
        let check_samples = sample_sessions(&page.findings);
        let mut grouped: BTreeMap<String, CachedTarget> = BTreeMap::new();
        for finding in page.findings {
            let display = finding
                .finding
                .display()
                .map_err(|_| ControllerError::Internal)?;
            let (group_key, target) =
                self.resolve_target(store, finding, display.agent, options.home)?;
            grouped
                .entry(group_key)
                .and_modify(|entry| {
                    entry.findings.extend(target.findings.clone());
                    match (&mut entry.config, target.config.as_ref()) {
                        (Some(existing), Some(incoming))
                            if existing.operation == incoming.operation
                                && existing.physical_key == incoming.physical_key =>
                        {
                            existing.additional_contexts.push(incoming.context.clone());
                            existing
                                .additional_contexts
                                .extend(incoming.additional_contexts.clone());
                        }
                        (None, None) => {}
                        _ => entry.config = None,
                    }
                })
                .or_insert(target);
        }
        let truncated = page.truncated || grouped.len() > MAX_TARGETS;
        let expires = options.now.saturating_add(ID_TTL.as_secs() as i64);
        let mut targets = Vec::new();
        let mut cached = Vec::new();
        for mut target in grouped.into_values().take(MAX_TARGETS) {
            target.findings.sort_by(|left, right| {
                right
                    .observed_at_ms
                    .cmp(&left.observed_at_ms)
                    .then_with(|| left.session_id.cmp(&right.session_id))
            });
            let display = target.findings[0]
                .finding
                .display()
                .map_err(|_| ControllerError::Internal)?;
            let display_facts = burn_check_display_facts(&target, None);
            let instruction_project = matches!(
                target.findings[0].finding.cause(),
                FindingCause::IgnoredInstructionConflict(evidence)
                    if evidence.source.starts_with("project:")
            );
            let has_project_path = target.scope_kind == "project" || instruction_project;
            let watch = store
                .latest_remediation_for_target(
                    &target.findings[0].environment_key,
                    target.agent.slug(),
                    &target.scope_kind,
                    &target.scope_key,
                    &target.target_key,
                )
                .map_err(|_| ControllerError::Internal)?
                .as_ref()
                .map(|watch| public_watch(store, watch))
                .transpose()?
                .flatten();
            let auto_fix = match (&target.config, watch.as_ref()) {
                (Some(_), Some(watch)) if auto_fix_blocked_by_watch(watch) => {
                    AutoFixAvailability::Unavailable(AutoFixUnavailableReason::ActiveWatch)
                }
                (Some(_), _) => AutoFixAvailability::Available,
                (None, _) => AutoFixAvailability::Unavailable(
                    AutoFixUnavailableReason::UnsupportedOrUnprovenTarget,
                ),
            };
            let id = if options.cache_actions {
                random_id().map_err(|_| ControllerError::Internal)?
            } else {
                String::new()
            };
            let target_samples = sample_sessions(&target.findings);
            targets.push(BurnCheckTarget {
                over_exploring_reason: match target.finding().cause() {
                    FindingCause::OverExploring(decision) => Some(decision.reason),
                    _ => None,
                },
                decision_proof: IgnoredInstructionDecisionProof::from_cause(
                    target.finding().cause(),
                ),
                finding_id: stable_finding_id(&target),
                action_id: id.clone(),
                finding: display,
                display: display_facts,
                occurrences: target.findings.len(),
                affected_sessions: Some(
                    target
                        .findings
                        .iter()
                        .map(|finding| {
                            (
                                &finding.environment_key,
                                &finding.agent,
                                &finding.session_id,
                            )
                        })
                        .collect::<BTreeSet<_>>()
                        .len(),
                ),
                project_name: has_project_path
                    .then(|| {
                        target.findings[0]
                            .workspace_candidate()
                            .and_then(display::project_name)
                    })
                    .flatten(),
                project_location: has_project_path
                    .then(|| {
                        target.findings[0]
                            .workspace_candidate()
                            .and_then(display::project_location)
                    })
                    .flatten(),
                project_path: has_project_path
                    .then(|| {
                        target.findings[0]
                            .workspace_candidate()
                            .and_then(display::project_path)
                    })
                    .flatten(),
                config_file: target
                    .config
                    .as_ref()
                    .map(|config| config.display_path.clone()),
                auto_fix,
                prompt_fix: match remediation_prompt(&target.findings[0].finding) {
                    Ok(_) => PromptFixAvailability::Available,
                    Err(reason) => PromptFixAvailability::Unavailable(reason),
                },
                watch,
                evidence_available: target.findings.iter().any(|finding| {
                    finding.finding.detector == DetectorId::IgnoredInstructions
                        || stored_skill_opportunity_evidence(finding.finding.cause()).is_some()
                        || finding.finding.detector == DetectorId::OverExploring
                        || finding.finding.detector == DetectorId::ScopeCreep
                }),
                coverage_limits: vec![CoverageLimit::CurrentPublishedEvidenceOnly],
                sample_sessions: target_samples,
                expires_at_epoch: expires,
            });
            if options.cache_actions {
                cached.push(TimedTarget {
                    id,
                    value: target,
                    check_preferences_revision,
                    smart_master_generation: smart_master_generation.clone(),
                    created_at_epoch: options.now,
                });
            }
        }
        if !self.check_policy_matches(
            store,
            detector,
            check_preferences_revision,
            smart_master_generation.as_deref(),
        )? {
            return Ok(BurnCheckTargetList {
                targets: Vec::new(),
                sample_sessions: Vec::new(),
                truncated: false,
            });
        }
        if !options.cache_actions {
            return Ok(BurnCheckTargetList {
                targets,
                sample_sessions: check_samples,
                truncated,
            });
        }
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        cache_listed_targets(&mut state, detector, cached, options.now);
        Ok(BurnCheckTargetList {
            targets,
            sample_sessions: check_samples,
            truncated,
        })
    }

    fn list_resource_targets(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
        options: TargetListOptions<'_>,
        check_preferences_revision: u64,
        smart_master_generation: Option<String>,
    ) -> Result<BurnCheckTargetList, ControllerError> {
        let request = insights_report::ReportRequest {
            environment_key: context.environment_key.clone(),
            window: context.window,
            computed_at_epoch: context.window.end_epoch,
        };
        #[cfg(test)]
        let reduced = match options.home {
            Some(home) => {
                insights_report::reduce_report_blocking_with_home(&self.data_dir, request, home)
            }
            None => insights_report::reduce_report_blocking(&self.data_dir, request),
        }
        .map_err(|_| ControllerError::Internal)?;
        #[cfg(not(test))]
        let reduced = insights_report::reduce_report_blocking(&self.data_dir, request)
            .map_err(|_| ControllerError::Internal)?;
        let assessment = reduced
            .resources
            .detector(detector)
            .ok_or(ControllerError::Internal)?;
        let truncated = assessment.truncated || assessment.targets.len() > MAX_TARGETS;
        let expires = options.now.saturating_add(ID_TTL.as_secs() as i64);
        let mut targets = Vec::new();
        let mut cached = Vec::new();
        let mut check_samples = Vec::new();
        let mut seen_samples = BTreeSet::new();
        for resource in assessment.targets.iter().take(MAX_TARGETS) {
            let target =
                self.resolve_resource_target(store, resource, context.clone(), options.home)?;
            let display = target
                .finding()
                .display()
                .map_err(|_| ControllerError::Internal)?;
            let watch = store
                .latest_remediation_for_target(
                    target.environment_key(),
                    target.agent.slug(),
                    &target.scope_kind,
                    &target.scope_key,
                    &target.target_key,
                )
                .map_err(|_| ControllerError::Internal)?
                .as_ref()
                .map(|watch| public_watch(store, watch))
                .transpose()?
                .flatten();
            let auto_fix = match (&target.config, watch.as_ref()) {
                (Some(_), Some(watch)) if auto_fix_blocked_by_watch(watch) => {
                    AutoFixAvailability::Unavailable(AutoFixUnavailableReason::ActiveWatch)
                }
                (Some(_), _) => AutoFixAvailability::Available,
                (None, _) => AutoFixAvailability::Unavailable(
                    AutoFixUnavailableReason::UnsupportedOrUnprovenTarget,
                ),
            };
            let id = if options.cache_actions {
                random_id().map_err(|_| ControllerError::Internal)?
            } else {
                String::new()
            };
            let project_name = match &resource.scope {
                insights_report::ResourceAssessmentScope::Project(root) => project_name(root),
                insights_report::ResourceAssessmentScope::Global => None,
            };
            let target_samples = target.sample_sessions();
            let evidence_samples = target.evidence_sessions();
            check_samples.extend(
                evidence_samples
                    .iter()
                    .filter(|sample| {
                        seen_samples.insert((
                            sample.environment_key.clone(),
                            sample.agent.clone(),
                            sample.session_id.clone(),
                        ))
                    })
                    .cloned(),
            );
            targets.push(BurnCheckTarget {
                over_exploring_reason: None,
                decision_proof: None,
                finding_id: stable_finding_id(&target),
                action_id: id.clone(),
                finding: display,
                display: burn_check_display_facts(&target, Some(&reduced.report)),
                occurrences: usize::try_from(resource.observations).unwrap_or(usize::MAX),
                affected_sessions: None,
                project_name,
                project_location: None,
                project_path: None,
                config_file: target
                    .config
                    .as_ref()
                    .map(|config| config.display_path.clone()),
                auto_fix,
                prompt_fix: match remediation_prompt(target.finding()) {
                    Ok(_) => PromptFixAvailability::Available,
                    Err(reason) => PromptFixAvailability::Unavailable(reason),
                },
                watch,
                evidence_available: false,
                coverage_limits: vec![CoverageLimit::CurrentPublishedEvidenceOnly],
                sample_sessions: target_samples,
                expires_at_epoch: expires,
            });
            if options.cache_actions {
                cached.push(TimedTarget {
                    id,
                    value: target,
                    check_preferences_revision,
                    smart_master_generation: smart_master_generation.clone(),
                    created_at_epoch: options.now,
                });
            }
        }
        if !self.check_policy_matches(
            store,
            detector,
            check_preferences_revision,
            smart_master_generation.as_deref(),
        )? {
            return Ok(BurnCheckTargetList {
                targets: Vec::new(),
                sample_sessions: Vec::new(),
                truncated: false,
            });
        }
        if !options.cache_actions {
            return Ok(BurnCheckTargetList {
                targets,
                sample_sessions: check_samples,
                truncated,
            });
        }
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        cache_listed_targets(&mut state, detector, cached, options.now);
        Ok(BurnCheckTargetList {
            targets,
            sample_sessions: check_samples,
            truncated,
        })
    }

    #[cfg(all(test, not(windows)))]
    pub(crate) fn list_burn_check_targets_with_home(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
        home: &Path,
    ) -> Result<BurnCheckTargetList, ControllerError> {
        self.list_burn_check_targets_at(
            store,
            detector,
            context,
            TargetListOptions {
                now: now_epoch(),
                home: Some(home),
                cache_actions: true,
            },
        )
    }

    /// Resolve a project folder from an unexpired local check action.
    pub fn project_folder(
        &self,
        store: &Store,
        action_id: &str,
    ) -> Result<String, ControllerError> {
        let target = self.cached_target(store, action_id, now_epoch())?;
        if target.scope_kind != "project" {
            return Err(ControllerError::TargetNotFound);
        }
        let finding = target
            .findings
            .first()
            .ok_or(ControllerError::TargetNotFound)?;
        let environment = &finding.environment_key;
        if environment != "native"
            && !environment
                .strip_prefix("wsl:")
                .is_some_and(|distro| !distro.is_empty())
        {
            return Err(ControllerError::TargetNotFound);
        }
        let key = SessionKey::new(environment, &finding.agent, &finding.session_id);
        if store
            .session(&key)
            .map_err(|_| ControllerError::Internal)?
            .is_none()
        {
            return Err(ControllerError::TargetNotFound);
        }
        finding
            .workspace_candidate()
            .and_then(display::project_path)
            .ok_or(ControllerError::TargetNotFound)
    }

    pub fn copy_prompt_fix_burn_check_target(
        &self,
        store: &Store,
        action_id: &str,
    ) -> Result<PromptFixResult, ControllerError> {
        let now = now_epoch();
        let target = self.cached_target(store, action_id, now)?;
        self.revalidate(&target)?;
        let base_prompt = remediation_prompt(target.finding())
            .map_err(ControllerError::PromptUnavailable)?
            .into_string();
        let paths = representative_paths(
            store,
            target.findings.iter().map(|finding| {
                SessionKey::new(
                    finding.environment_key.as_str(),
                    finding.agent.as_str(),
                    finding.session_id.as_str(),
                )
            }),
        )?;
        let (reference, prompt_group_id) =
            self.prompt_reference_for_targets(store, std::slice::from_ref(&target))?;
        let prompt = prompt_with_evidence_paths(&base_prompt, &paths, Some(&reference))
            .map_err(ControllerError::PromptUnavailable)?;
        let (watches, prompt) = self.persist_prompt_watches(
            store,
            std::slice::from_ref(&target),
            prompt_group_id.as_deref(),
            &prompt,
            now,
        )?;
        let watch = watches
            .into_iter()
            .next()
            .map(|watch| public_watch(store, &watch))
            .transpose()?
            .flatten();
        Ok(PromptFixResult { prompt, watch })
    }

    pub fn burn_check_target_evidence(
        &self,
        store: &Store,
        action_id: &str,
    ) -> Result<BurnCheckTargetEvidence, ControllerError> {
        let target = match self.cached_target(store, action_id, now_epoch()) {
            Ok(target) => target,
            Err(ControllerError::TargetNotFound | ControllerError::TargetExpired) => {
                return Ok(unavailable_instruction_evidence());
            }
            Err(error) => return Err(error),
        };
        if !matches!(
            target.finding().detector,
            DetectorId::IgnoredInstructions
                | DetectorId::SkillOpportunities
                | DetectorId::OverExploring
                | DetectorId::ScopeCreep
        ) {
            return Err(ControllerError::TargetNotFound);
        }
        let mut result = unavailable_instruction_evidence();
        for current in target.findings.iter().take(MAX_TARGETS) {
            if let FindingCause::ScopeCreep(scope) = current.finding.cause() {
                let evidence = self.validated_scope_creep_evidence(store, current)?;
                if result.occurrences.is_empty() {
                    result.status = evidence.status;
                    result.items = evidence.items.clone();
                }
                result.occurrences.push(BurnCheckEvidenceOccurrence {
                    decision_proof: None,
                    finding_id: scope.id.clone(),
                    status: evidence.status,
                    items: evidence.items,
                });
                continue;
            }
            if let FindingCause::OverExploring(decision) = current.finding.cause() {
                let evidence = self.validated_over_exploring_evidence(store, current)?;
                if result.occurrences.is_empty() {
                    result.status = evidence.status;
                    result.items = evidence.items.clone();
                }
                result.occurrences.push(BurnCheckEvidenceOccurrence {
                    decision_proof: None,
                    finding_id: decision.work_item_id.clone(),
                    status: evidence.status,
                    items: evidence.items,
                });
                continue;
            }
            if let FindingCause::SkillOpportunity {
                evidence: Some(skill),
                ..
            } = current.finding.cause()
            {
                if !insights_report::revalidate_current_finding(&self.data_dir, current)
                    .map_err(|_| ControllerError::Internal)?
                {
                    return Ok(unavailable_instruction_evidence());
                }
                let evidence = stored_skill_opportunity_evidence(current.finding.cause())
                    .ok_or(ControllerError::TargetChanged)?;
                let snapshot = if !skill.comparison.use_citations.is_empty() {
                    let key = SessionKey::new(
                        &current.environment_key,
                        &current.agent,
                        &current.session_id,
                    );
                    let Ok(snapshot) = store.load_smart_check_inputs(
                        &key,
                        current.published_fence,
                        current.source_generation,
                        crate::smart_check_inputs::DetectorInput::SkillOpportunities,
                    ) else {
                        return Ok(unavailable_instruction_evidence());
                    };
                    Some(snapshot)
                } else {
                    None
                };
                let evidence = complete_skill_opportunity_evidence(
                    evidence,
                    skill,
                    snapshot
                        .as_ref()
                        .map_or(&[], |snapshot| snapshot.content().actions.as_slice()),
                );
                if result.occurrences.is_empty() {
                    result.status = evidence.status;
                    result.items = evidence.items.clone();
                }
                result.occurrences.push(BurnCheckEvidenceOccurrence {
                    decision_proof: None,
                    finding_id: skill.comparison.id.clone(),
                    status: evidence.status,
                    items: evidence.items,
                });
                continue;
            }
            if !insights_report::revalidate_current_finding(&self.data_dir, current)
                .map_err(|_| ControllerError::Internal)?
            {
                return Ok(unavailable_instruction_evidence());
            }
            let saved = stored_instruction_evidence(current.finding.cause());
            let validated = self.validated_instruction_evidence(store, current)?;
            let evidence = merge_instruction_evidence(saved, validated);
            if result.occurrences.is_empty() {
                result.status = evidence.status;
                result.items = evidence.items.clone();
                result.decision_proof = evidence.decision_proof.clone();
            }
            result.occurrences.push(BurnCheckEvidenceOccurrence {
                decision_proof: evidence.decision_proof,
                finding_id: match current.finding.cause() {
                    FindingCause::IgnoredInstructionConflict(evidence) => {
                        evidence.assessment_finding_id.clone()
                    }
                    _ => return Err(ControllerError::TargetChanged),
                },
                status: evidence.status,
                items: evidence.items,
            });
        }
        Ok(result)
    }

    fn validated_scope_creep_evidence(
        &self,
        store: &Store,
        current: &CurrentFinding,
    ) -> Result<BurnCheckTargetEvidence, ControllerError> {
        let FindingCause::ScopeCreep(scope) = current.finding.cause() else {
            return Ok(unavailable_instruction_evidence());
        };
        let key = crate::store::SessionKey::new(
            &current.environment_key,
            &current.agent,
            &current.session_id,
        );
        let citations = crate::scope_creep_worker::saved_finding_citations(
            &store.lock(),
            &crate::scope_creep_worker::SourceFence {
                key: &key,
                incarnation: current.incarnation,
                source_generation: current.source_generation,
                source_fingerprint: current.source_fingerprint.as_deref(),
                published_fence: current.published_fence,
            },
            &scope.id,
        )
        .map_err(|_| ControllerError::Internal)?;
        let Some(citations) = citations else {
            return Ok(unavailable_instruction_evidence());
        };
        let mut items = Vec::new();
        for (id, label) in scope
            .task_scope
            .iter()
            .map(|reference| (&reference.source_id, BurnCheckEvidenceLabel::Instruction))
            .chain(scope.work.iter().map(|binding| {
                (
                    &binding.reference.id,
                    BurnCheckEvidenceLabel::ObservedAction,
                )
            }))
        {
            let Some(text) = citations.get(id) else {
                return Ok(unavailable_instruction_evidence());
            };
            items.push(BurnCheckEvidenceItem {
                label, source_label: if label == BurnCheckEvidenceLabel::Instruction { "Latest recorded task scope" } else { "Recorded work" }.into(),
                reference: id.clone(), observed_at_ms: None, start_line: None, end_line: None,
                excerpt: bounded_evidence_excerpt(text), explanation: if label == BurnCheckEvidenceLabel::Instruction {
                    "This event supplies retained task context. Recorded approvals constrain the finding."
                } else {
                    match scope.observation_kind {
                        antiburn_local::checks::scope_creep::WorkObservationKind::Attempt => "This recorded attempt is assessed against retained task context. It does not prove completed execution.",
                        antiburn_local::checks::scope_creep::WorkObservationKind::Proposal => "This recorded proposal is assessed against retained task context. It does not prove attempted or completed execution.",
                    }
                }.into(),
                limitation: Some(if text.len() > 4096 { "This preview is truncated. The finding uses retained task context, not proof of complete approval history." } else { "The finding uses the current retained root snapshot, not proof of complete approval history." }.into()),
            });
        }
        Ok(BurnCheckTargetEvidence {
            decision_proof: None,
            status: BurnCheckEvidenceStatus::Available,
            items,
            occurrences: Vec::new(),
        })
    }

    fn validated_over_exploring_evidence(
        &self,
        store: &Store,
        current: &CurrentFinding,
    ) -> Result<BurnCheckTargetEvidence, ControllerError> {
        let FindingCause::OverExploring(decision) = current.finding.cause() else {
            return Ok(unavailable_instruction_evidence());
        };
        if !insights_report::revalidate_current_finding(&self.data_dir, current)
            .map_err(|_| ControllerError::Internal)?
        {
            return Ok(unavailable_instruction_evidence());
        }
        let key = crate::store::SessionKey {
            environment_key: current.environment_key.clone(),
            agent: current.agent.clone(),
            session_id: current.session_id.clone(),
        };
        let Ok(snapshot) = store.load_smart_check_inputs(
            &key,
            current.published_fence,
            current.source_generation,
            crate::smart_check_inputs::DetectorInput::OverExploring,
        ) else {
            return Ok(unavailable_instruction_evidence());
        };
        let mut items = Vec::new();
        let scope = snapshot.scope();
        let context = scope.user_context();
        for binding in &decision.task_evidence {
            let Some(index) = context
                .evidence
                .iter()
                .position(|reference| reference == binding)
            else {
                return Ok(unavailable_instruction_evidence());
            };
            let occurrence = &scope.occurrences()[index];
            let Some(value) = scope.values().get(occurrence.value_index) else {
                return Ok(unavailable_instruction_evidence());
            };
            let text = match value.as_str() {
                Some(text) => text.to_owned(),
                None => serde_json::to_string(value).map_err(|_| ControllerError::Internal)?,
            };
            items.push(BurnCheckEvidenceItem {
                label: BurnCheckEvidenceLabel::Context,
                source_label: "Recorded task context".into(),
                reference: occurrence.reference.id.clone(),
                observed_at_ms: None,
                start_line: None,
                end_line: None,
                excerpt: bounded_evidence_excerpt(&text),
                explanation: "This recorded task context supports the read comparison.".into(),
                limitation: (text.len() > 4096).then(|| "This preview is truncated.".into()),
            });
        }
        for binding in &decision.reads {
            let Some(request) = snapshot
                .content()
                .actions
                .iter()
                .find(|action| action.reference.id == binding.request_id)
            else {
                return Ok(unavailable_instruction_evidence());
            };
            if request
                .metadata
                .read_request
                .as_ref()
                .is_none_or(|read| read.reference_id != binding.request_id)
            {
                return Ok(unavailable_instruction_evidence());
            }
            let result = match (&binding.result_id, &binding.output_digest) {
                (None, None) => None,
                (Some(id), Some(digest)) => {
                    let Some(result) = snapshot
                        .content()
                        .actions
                        .iter()
                        .find(|action| &action.reference.id == id)
                    else {
                        return Ok(unavailable_instruction_evidence());
                    };
                    if antiburn_local::analysis::ignored_instructions::sha256_hex(
                        result.text.as_bytes(),
                    ) != *digest
                        || !result.reference.stable
                        || result.tool_call_id.is_none()
                        || result.tool_call_id != request.tool_call_id
                        || result.tool_name != request.tool_name
                        || result.metadata.read_result.as_ref().is_none_or(|read| {
                            read.reference_id != *id
                                || read.request_reference_id.as_deref()
                                    != Some(binding.request_id.as_str())
                                || read.recorded_output_digest != *digest
                        })
                    {
                        return Ok(unavailable_instruction_evidence());
                    }
                    Some(result)
                }
                _ => return Ok(unavailable_instruction_evidence()),
            };
            for action in std::iter::once(request).chain(result) {
                items.push(BurnCheckEvidenceItem {
                    label: BurnCheckEvidenceLabel::ObservedAction, source_label: "Recorded read".into(),
                    reference: action.reference.id.clone(), observed_at_ms: action.timestamp_ms,
                    start_line: None, end_line: None, excerpt: bounded_evidence_excerpt(&action.text),
                    explanation: current.finding.display().map_err(|_| ControllerError::TargetChanged)?.observation,
                    limitation: Some("The assessment uses bounded representative text ranges. A read request alone does not prove returned content. Not every read or token in the episode is waste.".into()),
                });
            }
        }
        Ok(BurnCheckTargetEvidence {
            decision_proof: None,
            status: BurnCheckEvidenceStatus::Available,
            items,
            occurrences: Vec::new(),
        })
    }

    fn validated_instruction_evidence(
        &self,
        store: &Store,
        current: &CurrentFinding,
    ) -> Result<BurnCheckTargetEvidence, ControllerError> {
        let antiburn_local::remediation::FindingCause::IgnoredInstructionConflict(evidence) =
            current.finding.cause()
        else {
            return Ok(unavailable_instruction_evidence());
        };
        let antiburn_local::remediation::IgnoredInstructionConflictEvidence {
            assessment_revision,
            rule_id,
            start_line,
            end_line,
            source,
            provenance,
            action_id,
            nearby_context_ids,
            counterevidence_ids,
            certainty,
            limitations,
            ..
        } = evidence.as_ref();

        let key = SessionKey::new(
            current.environment_key.as_str(),
            current.agent.as_str(),
            current.session_id.as_str(),
        );
        let session_identity = format!(
            "{}\0{}\0{}",
            current.environment_key, current.agent, current.session_id
        );
        let context_ids: BTreeSet<_> = nearby_context_ids
            .iter()
            .chain(counterevidence_ids)
            .map(String::as_str)
            .chain(evidence.decision_record().into_iter().flat_map(|decision| {
                decision
                    .selected_evidence
                    .iter()
                    .map(|identity| identity.source.id.as_str())
            }))
            .filter(|reference| *reference != action_id)
            .collect();
        let mut actions = BTreeMap::new();
        if let Some(decision) = evidence.decision_record() {
            let references: Vec<_> = std::iter::once(&decision.action_anchor.source)
                .chain(
                    decision
                        .selected_evidence
                        .iter()
                        .map(|identity| &identity.source),
                )
                .collect();
            let Some(published) = store
                .published_turn_content_identities_selected(
                    &key,
                    current.source_generation,
                    &references,
                    &context_ids,
                    antiburn_local::analysis::jev::JevCheck::input_selection(
                        &antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck,
                    ),
                )
                .map_err(|_| ControllerError::Internal)?
            else {
                return Ok(unavailable_instruction_evidence());
            };
            let Some(content) = validated_instruction_content(
                &session_identity,
                current.finding.source_format,
                published,
                current.published_fence,
                current.source_generation,
            ) else {
                return Ok(unavailable_instruction_evidence());
            };
            actions.extend(
                content
                    .actions
                    .into_iter()
                    .map(|action| (action.reference.id.clone(), action)),
            );
        } else {
            let mut content_cursor = None;
            for _ in 0..16 {
                let Some(page) = store
                .published_turn_content_keyset_selected(
                    &key,
                    antiburn_local::analysis::SelectedContentRequest {
                        source_generation: current.source_generation,
                        after_ms: Some(i64::MIN),
                        source_positions: &BTreeMap::from([("*".to_owned(), 0)]),
                        cursor: content_cursor.as_ref(),
                        selection: antiburn_local::analysis::jev::JevCheck::input_selection(
                            &antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck,
                        ),
                    },
                )
                .map_err(|_| ControllerError::Internal)?
            else {
                return Ok(unavailable_instruction_evidence());
            };
                let published = page.content;
                let more = published.coverage.more_parts;
                let next_cursor = page.next_cursor;
                let Some(evidence) = validated_instruction_content(
                    &session_identity,
                    current.finding.source_format,
                    published,
                    current.published_fence,
                    current.source_generation,
                ) else {
                    return Ok(unavailable_instruction_evidence());
                };
                for action in evidence.actions {
                    if action.reference.id == *action_id
                        || context_ids.contains(action.reference.id.as_str())
                    {
                        actions.insert(action.reference.id.clone(), action);
                    }
                }
                if actions.len() == context_ids.len() + 1 || !more {
                    break;
                }
                content_cursor = next_cursor;
            }
        }
        let Some((references, context_missing)) = validated_evidence_references(evidence, &actions)
        else {
            return Ok(unavailable_instruction_evidence());
        };
        let mut items = vec![BurnCheckEvidenceItem {
            label: BurnCheckEvidenceLabel::Instruction,
            source_label: source.strip_prefix("project:").or_else(|| source.strip_prefix("home:")).unwrap_or(source).to_owned(),
            reference: if evidence.decision_record().is_some() {
                format!("{}:{}", evidence.instruction_id, rule_id)
            } else {
                rule_id.clone()
            },
            observed_at_ms: None,
            start_line: Some(*start_line),
            end_line: Some(*end_line),
            excerpt: "Instruction text unavailable.".to_owned(),
            explanation: format!(
                "{} conflict · {}.",
                match certainty {
                    antiburn_local::analysis::ignored_instructions::FindingCertainty::Likely => "Likely",
                    antiburn_local::analysis::ignored_instructions::FindingCertainty::Possible => "Possible",
                },
                match provenance {
                    antiburn_local::analysis::ignored_instructions::InstructionProvenance::RecordedInjection => "recorded instruction",
                    antiburn_local::analysis::ignored_instructions::InstructionProvenance::ObservedRead => "instruction seen in the session",
                    antiburn_local::analysis::ignored_instructions::InstructionProvenance::CurrentFileComparison => "current instruction file",
                },
            ),
            limitation: Some("The instruction text was not saved with this finding. Current files cannot fill historical evidence gaps.".to_owned()),
        }];
        for reference in references {
            let Some(action) = actions.get(&reference) else {
                return Ok(unavailable_instruction_evidence());
            };
            let is_observed_action = action.reference.id == *action_id;
            let excerpt = bounded_evidence_excerpt(&action.text);
            items.push(BurnCheckEvidenceItem {
                label: if is_observed_action {
                    BurnCheckEvidenceLabel::ObservedAction
                } else {
                    BurnCheckEvidenceLabel::Context
                },
                source_label: if is_observed_action {
                    "Observed session action".to_owned()
                } else {
                    "Session context".to_owned()
                },
                reference: action.reference.id.clone(),
                observed_at_ms: action.timestamp_ms,
                start_line: None,
                end_line: None,
                excerpt,
                explanation: if is_observed_action {
                    "This is the action cited by the assessment.".to_owned()
                } else if counterevidence_ids.contains(&action.reference.id) {
                    "This event is cited as counterevidence, not as another violation.".to_owned()
                } else {
                    "This nearby event provides context for the assessment, not another violation."
                        .to_owned()
                },
                limitation: if action.truncated || action.text.len() > 4096 {
                    Some("The saved event text is incomplete.".to_owned())
                } else {
                    None
                },
            });
        }
        if context_missing
            && let Some(action) = items
                .iter_mut()
                .find(|item| item.label == BurnCheckEvidenceLabel::ObservedAction)
        {
            action.limitation = Some(match action.limitation.take() {
                Some(existing) => format!("{existing} Some nearby context is no longer available."),
                None => "Some nearby context is no longer available.".to_owned(),
            });
        }
        if !limitations.is_empty() && items[0].limitation.is_none() {
            items[0].limitation = Some(
                "The assessment records additional evidence limits. Review the action and section before deciding.".to_owned(),
            );
        }
        if assessment_revision.is_empty() {
            return Ok(unavailable_instruction_evidence());
        }
        Ok(BurnCheckTargetEvidence {
            decision_proof: IgnoredInstructionDecisionProof::from_cause(current.finding.cause()),
            status: BurnCheckEvidenceStatus::Available,
            items,
            occurrences: Vec::new(),
        })
    }

    pub fn copy_prompt_fix_burn_check(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
    ) -> Result<CheckPromptFixResult, ControllerError> {
        if !store
            .check_enabled(detector)
            .map_err(|_| ControllerError::PersistenceFailed)?
            || !smart_check_master_available(store, detector)
        {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let targets = self.list_burn_check_targets(store, detector, context)?;
        let now = now_epoch();
        if targets.targets.is_empty() {
            if detector == DetectorId::IgnoredInstructions {
                return Ok(CheckPromptFixResult {
                    prompt: fallback_remediation_prompt(detector)
                        .map_err(ControllerError::PromptUnavailable)?
                        .into_string(),
                });
            }
            return Err(ControllerError::CheckPromptUnavailable);
        }
        if targets.truncated {
            let base = fallback_remediation_prompt(detector)
                .map_err(ControllerError::PromptUnavailable)?
                .into_string();
            let paths = representative_paths(
                store,
                targets.sample_sessions.iter().map(|sample| {
                    SessionKey::new(&sample.environment_key, &sample.agent, &sample.session_id)
                }),
            )?;
            let selected = targets
                .targets
                .iter()
                .map(|target| self.cached_target(store, &target.action_id, now))
                .collect::<Result<Vec<_>, _>>()?;
            let (reference, prompt_group_id) =
                self.prompt_reference_for_targets(store, &selected)?;
            let prompt = prompt_with_evidence_paths(&base, &paths, Some(&reference))
                .map_err(ControllerError::PromptUnavailable)?;
            let (_, prompt) = self.persist_prompt_watches(
                store,
                &selected,
                prompt_group_id.as_deref(),
                &prompt,
                now,
            )?;
            return Ok(CheckPromptFixResult { prompt });
        }
        self.copy_prompt_fix_burn_check_targets(
            store,
            &targets
                .targets
                .into_iter()
                .map(|target| target.action_id)
                .collect::<Vec<_>>(),
        )
    }

    pub fn copy_prompt_fix_burn_check_targets(
        &self,
        store: &Store,
        action_ids: &[String],
    ) -> Result<CheckPromptFixResult, ControllerError> {
        if action_ids.is_empty() || action_ids.len() > MAX_CHECK_PROMPT_TARGETS {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let now = now_epoch();
        let mut targets = Vec::with_capacity(action_ids.len());
        for action_id in action_ids {
            if action_ids.iter().filter(|id| *id == action_id).count() != 1 {
                return Err(ControllerError::CheckPromptUnavailable);
            }
            targets.push(self.cached_target(store, action_id, now)?);
        }
        let detector = targets[0].finding().detector;
        if targets
            .iter()
            .any(|target| target.finding().detector != detector)
        {
            return Err(ControllerError::CheckPromptUnavailable);
        }

        // Validate every selected identity before this action records any watch.
        self.revalidate_all(&targets)?;

        if targets
            .iter()
            .all(|target| target.finding().is_advisory_resource())
        {
            let (reference, prompt_group_id) =
                self.prompt_reference_for_targets(store, &targets)?;
            let base = fallback_remediation_prompt(detector)
                .map_err(ControllerError::PromptUnavailable)?
                .into_string();
            let paths = representative_paths(
                store,
                targets.iter().flat_map(|target| {
                    target.evidence_sessions().into_iter().map(|sample| {
                        SessionKey::new(sample.environment_key, sample.agent, sample.session_id)
                    })
                }),
            )?;
            let base = prompt_with_evidence_paths(&base, &paths, None)
                .map_err(ControllerError::PromptUnavailable)?;
            let items =
                targets
                    .iter()
                    .enumerate()
                    .map(|(index, target)| {
                        remediation_prompt(target.finding())
                            .map_err(ControllerError::PromptUnavailable)?;
                        let display = target
                            .finding()
                            .display()
                            .map_err(ControllerError::PromptUnavailable)?;
                        let resource = display.facts.labels.first().ok_or(
                            ControllerError::PromptUnavailable(
                                RemediationUnavailableReason::EssentialIdentityUnavailable,
                            ),
                        )?;
                        let resource = serde_json::to_string(resource)
                            .map_err(|_| ControllerError::Internal)?;
                        Ok(format!(
                            "{}. Agent: {}; scope: {}; resource: {resource}",
                            index + 1,
                            display.agent.slug(),
                            target.scope_kind,
                        ))
                    })
                    .collect::<Result<Vec<_>, ControllerError>>()?;
            let prompt = prompt_with_evidence_paths(
                &format!("{base}\n\nUnused targets\n{}", items.join("\n")),
                &[],
                Some(&reference),
            )
            .map_err(ControllerError::PromptUnavailable)?;
            let (_, prompt) = self.persist_prompt_watches(
                store,
                &targets,
                prompt_group_id.as_deref(),
                &prompt,
                now,
            )?;
            return Ok(CheckPromptFixResult { prompt });
        }

        let (reference, prompt_group_id) = self.prompt_reference_for_targets(store, &targets)?;
        let mut prompt_parts = Vec::with_capacity(targets.len());
        for target in &targets {
            let base = remediation_prompt(target.finding())
                .map_err(ControllerError::PromptUnavailable)?
                .into_string();
            let paths = representative_paths(
                store,
                target.findings.iter().map(|finding| {
                    SessionKey::new(
                        finding.environment_key.as_str(),
                        finding.agent.as_str(),
                        finding.session_id.as_str(),
                    )
                }),
            )?;
            prompt_parts.push((base, paths));
        }
        let mut sections = Vec::with_capacity(targets.len());
        for (index, (base, paths)) in prompt_parts.into_iter().enumerate() {
            let prompt = prompt_with_evidence_paths(&base, &paths, None)
                .map_err(ControllerError::PromptUnavailable)?;
            sections.push(format!("Exact target {}\n{prompt}", index + 1));
        }
        let prompt = prompt_with_evidence_paths(&sections.join("\n\n"), &[], Some(&reference))
            .map_err(ControllerError::PromptUnavailable)?;
        let (_, prompt) =
            self.persist_prompt_watches(store, &targets, prompt_group_id.as_deref(), &prompt, now)?;
        Ok(CheckPromptFixResult { prompt })
    }

    pub fn prepare_auto_fix_burn_check_target(
        &self,
        store: &Store,
        action_id: &str,
    ) -> Result<AutoFixReview, ControllerError> {
        self.prepare_auto_fix_at(store, action_id, now_epoch())
    }

    fn prepare_auto_fix_at(
        &self,
        store: &Store,
        action_id: &str,
        now: i64,
    ) -> Result<AutoFixReview, ControllerError> {
        let initial_check_preferences_revision = store
            .check_preferences_revision()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let initial_master_marker = store.internal_value(SMART_CHECKS_ENABLED_AT_KEY);
        let target = self.cached_target(store, action_id, now)?;
        let initial_smart_master_generation = crate::jev::worker::registered_check_ids()
            .contains(&target.finding().detector)
            .then_some(initial_master_marker)
            .flatten();
        self.revalidate(&target)?;
        if let Some(watch) = store
            .latest_remediation_for_target(
                target.environment_key(),
                target.agent.slug(),
                &target.scope_kind,
                &target.scope_key,
                &target.target_key,
            )
            .map_err(|_| ControllerError::Internal)?
            .as_ref()
            .map(|watch| public_watch(store, watch))
            .transpose()?
            .flatten()
            && auto_fix_blocked_by_watch(&watch)
        {
            return Err(ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::ActiveWatch,
            ));
        }
        let config = target.config.as_ref().ok_or({
            ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::UnsupportedOrUnprovenTarget,
            )
        })?;
        let context = refreshed_config_context(&config.context);
        let prepared = self
            .editor
            .prepare_operation(&context, &config.operation)
            .map_err(|reason| {
                if matches!(
                    reason,
                    crate::agent_config::ConfigUnavailableReason::CurrentValueMismatch
                        | crate::agent_config::ConfigUnavailableReason::ChangedIdentity
                ) {
                    ControllerError::Conflict
                } else {
                    ControllerError::AutoFixUnavailable(AutoFixUnavailableReason::SafetyCheckFailed)
                }
            })?;
        let selector_qualifier =
            physical_selector_qualifier(&self.editor, &context, prepared.physical_identity().1);
        if physical_key(
            store,
            target.agent,
            prepared.physical_identity(),
            selector_qualifier.as_deref(),
        )
        .map_err(|_| ControllerError::Internal)?
            != config.physical_key
            || prepared.scope()
                != scope_from_name(&target.scope_kind).ok_or(ControllerError::TargetChanged)?
        {
            return Err(ControllerError::TargetChanged);
        }
        for context in &config.additional_contexts {
            if !self.prepared_context_matches(
                store,
                target.agent,
                &target.scope_kind,
                config,
                context,
                prepared.creates_file(),
            )? {
                return Err(ControllerError::TargetChanged);
            }
        }
        let prepared_operation_id = random_id().map_err(|_| ControllerError::Internal)?;
        let retained_bytes = prepared.retained_bytes();
        if retained_bytes > PREPARED_CACHE_BYTES {
            return Err(ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::SafetyCheckFailed,
            ));
        }
        let setting = match config.operation.setting {
            ConfigSetting::Model => AutoFixSetting::Model,
            ConfigSetting::Reasoning => AutoFixSetting::Reasoning,
            ConfigSetting::Compaction => AutoFixSetting::Compaction,
            ConfigSetting::FastMode => AutoFixSetting::FastMode,
            ConfigSetting::SubagentModel => AutoFixSetting::SubagentModel,
            ConfigSetting::McpServer => AutoFixSetting::McpServer,
            ConfigSetting::BuiltInTool => AutoFixSetting::BuiltInTool,
            ConfigSetting::Skill => AutoFixSetting::Skill,
        };
        let review = AutoFixReview {
            prepared_operation_id: prepared_operation_id.clone(),
            expires_at_epoch: now.saturating_add(ID_TTL.as_secs() as i64),
            agent: target.agent,
            scope: scope_display(&target.scope_kind),
            setting,
            config_file: display_config_file(prepared.physical_identity().0, &context.home_root),
            selector_label: prepared.physical_identity().1.to_owned(),
            current_value: config.operation.expected_value.display_value(),
            proposed_value: config.operation.proposed_value.display_value(),
            behavior_override_warning: prepared.behavior_override_warning(),
            effect: match setting {
                AutoFixSetting::Model => AutoFixEffect::ModelSelection,
                AutoFixSetting::Reasoning => AutoFixEffect::ReasoningEffort,
                AutoFixSetting::Compaction => AutoFixEffect::SessionCompaction,
                AutoFixSetting::SubagentModel => AutoFixEffect::WorkerModelSelection,
                AutoFixSetting::McpServer => AutoFixEffect::McpAvailability,
                AutoFixSetting::BuiltInTool => AutoFixEffect::ToolAvailability,
                AutoFixSetting::Skill => AutoFixEffect::SkillAvailability,
                AutoFixSetting::FastMode => AutoFixEffect::ServiceTierSelection,
            },
            side_effect: match setting {
                AutoFixSetting::Model => AutoFixSideEffect::ModelBehaviorMayChange,
                AutoFixSetting::Reasoning => AutoFixSideEffect::ResponsesMayUseLessReasoning,
                AutoFixSetting::Compaction => AutoFixSideEffect::EarlierSessionSummarization,
                AutoFixSetting::SubagentModel => AutoFixSideEffect::WorkerBehaviorMayChange,
                AutoFixSetting::McpServer => AutoFixSideEffect::ServerWillNotBeAvailable,
                AutoFixSetting::BuiltInTool => AutoFixSideEffect::ToolWillNotBeAvailable,
                AutoFixSetting::Skill => AutoFixSideEffect::SkillWillNotBeAvailable,
                AutoFixSetting::FastMode => AutoFixSideEffect::ResponsesMayTakeLonger,
            },
        };
        let (enabled, check_preferences_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        if check_preferences_revision != initial_check_preferences_revision {
            return Err(ControllerError::TargetChanged);
        }
        if smart_check_master_generation(store, target.finding().detector)
            != initial_smart_master_generation
        {
            return Err(ControllerError::TargetChanged);
        }
        if !smart_check_master_available(store, target.finding().detector) {
            return Err(ControllerError::TargetNotFound);
        }
        if !enabled.contains(&target.finding().detector) {
            return Err(ControllerError::TargetNotFound);
        }
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        prune_prepared(&mut state, now);
        while state.prepared.len() >= PREPARED_CACHE_LIMIT
            || prepared_retained_bytes(&state).saturating_add(retained_bytes) > PREPARED_CACHE_BYTES
        {
            if state.prepared.pop_front().is_none() {
                return Err(ControllerError::AutoFixUnavailable(
                    AutoFixUnavailableReason::SafetyCheckFailed,
                ));
            }
        }
        state.prepared.push_back(PreparedAutoFix {
            id: prepared_operation_id,
            target,
            prepared: Some(prepared),
            retained_bytes,
            check_preferences_revision,
            smart_master_generation: initial_smart_master_generation,
            created_at_epoch: now,
            completed: None,
        });
        Ok(review)
    }

    pub fn apply_prepared_burn_check_operation(
        &self,
        store: &Store,
        prepared_operation_id: &str,
    ) -> Result<AutoFixResult, ControllerError> {
        self.apply_prepared_at(store, prepared_operation_id, now_epoch())
    }

    fn apply_prepared_at(
        &self,
        store: &Store,
        prepared_operation_id: &str,
        now: i64,
    ) -> Result<AutoFixResult, ControllerError> {
        let (enabled, check_preferences_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let current_smart_master_generation = store.internal_value(SMART_CHECKS_ENABLED_AT_KEY);
        let (
            target,
            prepared,
            prepared_check_preferences_revision,
            prepared_smart_master_generation,
        ) = {
            let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
            let index = state
                .prepared
                .iter()
                .position(|entry| entry.id == prepared_operation_id)
                .ok_or(ControllerError::TargetNotFound)?;
            if now.saturating_sub(state.prepared[index].created_at_epoch) > ID_TTL.as_secs() as i64
            {
                state.prepared.remove(index);
                return Err(ControllerError::TargetExpired);
            }
            let entry = &mut state.prepared[index];
            if let Some(result) = &entry.completed {
                return Ok(result.clone());
            }
            if entry.check_preferences_revision != check_preferences_revision {
                state.prepared.remove(index);
                return Err(ControllerError::TargetChanged);
            }
            if !enabled.contains(&entry.target.finding().detector) {
                state.prepared.remove(index);
                return Err(ControllerError::TargetNotFound);
            }
            if crate::jev::worker::registered_check_ids().contains(&entry.target.finding().detector)
                && entry.smart_master_generation != current_smart_master_generation
            {
                state.prepared.remove(index);
                return Err(ControllerError::TargetChanged);
            }
            let prepared = entry.prepared.take().ok_or(ControllerError::Conflict)?;
            (
                entry.target.clone(),
                prepared,
                entry.check_preferences_revision,
                entry.smart_master_generation.clone(),
            )
        };
        if let Err(error) = self.revalidate_prepared(store, &target, &prepared) {
            self.remove_prepared(prepared_operation_id);
            return Err(error);
        }
        if !self.check_policy_matches(
            store,
            target.finding().detector,
            prepared_check_preferences_revision,
            prepared_smart_master_generation.as_deref(),
        )? {
            self.remove_prepared(prepared_operation_id);
            return Err(ControllerError::TargetChanged);
        }
        let (remediation, guards) =
            self.watch_input(&target, RemediationState::Reserved, None, now, None)?;
        let watch = match store
            .create_or_reuse_remediations_for_enabled_check(
                &[(remediation, guards)],
                target.finding().detector,
                prepared_check_preferences_revision,
                prepared_smart_master_generation.as_deref(),
            )
            .map_err(|_| ControllerError::PersistenceFailed)?
            .and_then(|mut watches| watches.pop())
            .ok_or(ControllerError::TargetChanged)
        {
            Ok(watch) => watch,
            Err(error) => {
                self.remove_prepared(prepared_operation_id);
                return Err(error);
            }
        };
        if watch.state != RemediationState::Reserved {
            self.remove_prepared(prepared_operation_id);
            return Err(ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::ActiveWatch,
            ));
        }
        if let Err(error) = self.persist_display_snapshot(
            store,
            &watch,
            &target,
            "action",
            now.saturating_mul(1_000),
        ) {
            let _ = store.cancel_remediation_reservation(&watch.remediation_id);
            self.remove_prepared(prepared_operation_id);
            return Err(error);
        }
        if !store
            .begin_remediation_write(&watch.remediation_id, now)
            .map_err(|_| ControllerError::PersistenceFailed)?
        {
            let _ = store.cancel_remediation_reservation(&watch.remediation_id);
            return Err(ControllerError::PersistenceFailed);
        }
        let result = (|| match apply_prepared_change(&self.editor, &prepared) {
            Ok(()) => {
                let readback_epoch = now_epoch().max(now);
                let boundary_ms = readback_epoch.saturating_mul(1_000);
                let definition = watch_definition(&target);
                let verification_available = watch_verification_available(
                    &definition,
                    &target.scope_kind,
                    target.agent.slug(),
                    target.finding().detector,
                );
                if !store
                    .finalize_remediation_write(
                        &watch.remediation_id,
                        boundary_ms,
                        readback_epoch,
                        verification_available,
                    )
                    .map_err(|_| ControllerError::RecoveryNeeded {
                        watch_id: watch.remediation_id.clone(),
                    })?
                {
                    return Err(ControllerError::RecoveryNeeded {
                        watch_id: watch.remediation_id,
                    });
                }
                Ok(AutoFixResult {
                    watch_id: watch.remediation_id,
                    verification_available,
                })
            }
            Err(error) if error.replacement_may_have_occurred() => {
                let _ = store.mark_remediation_recovery_needed(
                    &watch.remediation_id,
                    "writeOutcomeUnknown",
                    now_epoch(),
                );
                Err(ControllerError::RecoveryNeeded {
                    watch_id: watch.remediation_id,
                })
            }
            Err(error) => {
                // The editor reports these failures only before the atomic replacement.
                let _ = store.cancel_pre_replacement_write(&watch.remediation_id);
                Err(ControllerError::ApplyFailed(error))
            }
        })();
        match &result {
            Ok(completed) => {
                let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
                if let Some(entry) = state
                    .prepared
                    .iter_mut()
                    .find(|entry| entry.id == prepared_operation_id)
                {
                    entry.completed = Some(completed.clone());
                    entry.retained_bytes = 0;
                }
            }
            Err(_) => self.remove_prepared(prepared_operation_id),
        }
        result
    }

    pub fn aggregate_wins(&self, store: &Store) -> Result<AggregateWins, ControllerError> {
        let now_ms = now_epoch().saturating_mul(1_000);
        let mut enabled = store
            .enabled_checks()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        if store.internal_value(SMART_CHECKS_ENABLED_AT_KEY).is_none() {
            enabled
                .retain(|detector| !crate::jev::worker::registered_check_ids().contains(detector));
        }
        let snoozed = store
            .burn_check_snoozes()
            .map_err(|_| ControllerError::PersistenceFailed)?
            .into_iter()
            .filter(|snooze| snooze.until.is_none_or(|until| until > now_ms))
            .map(|snooze| DetectorId::from(snooze.detector))
            .collect::<BTreeSet<_>>();
        let rows = store
            .passed_remediation_contributions(1_000)
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let wins = rows
            .into_iter()
            .filter(|row| {
                enabled
                    .iter()
                    .any(|detector| detector.key() == row.contribution.detector_id)
                    && !snoozed
                        .iter()
                        .any(|detector| detector.key() == row.contribution.detector_id)
            })
            .map(|row| {
                let verified_boundary_ms = row.verified_boundary_ms;
                let row = row.contribution;
                let snapshot: StoredDisplaySnapshot =
                    serde_json::from_str(&row.display_snapshot_json)
                        .map_err(|_| ControllerError::Internal)?;
                let savings: AggregateSavings =
                    serde_json::from_str(&row.facts_json).map_err(|_| ControllerError::Internal)?;
                if snapshot.version != 1 || savings.version != 1 {
                    return Err(ControllerError::Internal);
                }
                let detector = DetectorId::ALL
                    .into_iter()
                    .find(|detector| detector.key() == row.detector_id)
                    .ok_or(ControllerError::Internal)?;
                Ok(AggregateWin {
                    finding_id: snapshot.finding_id,
                    remediation_cycle_id: row.remediation_id,
                    detector,
                    origin: row.origin,
                    display: snapshot.display,
                    savings,
                    verified_boundary_ms,
                    starts_at_ms: row.starts_at_ms,
                    ends_at_ms: row.ends_at_ms,
                })
            })
            .collect::<Result<Vec<_>, ControllerError>>()?;
        Ok(AggregateWins { wins })
    }

    fn resolve_target(
        &self,
        store: &Store,
        finding: CurrentFinding,
        agent: AgentKind,
        home: Option<&Path>,
    ) -> Result<(String, CachedTarget), ControllerError> {
        let project_root = finding
            .workspace_candidate()
            .and_then(|path| trusted_workspace(store, path).ok().flatten());
        let secret = store
            .provider_account_secret()
            .map_err(|_| ControllerError::Internal)?;
        let mut identity = target_identity(&secret, &finding, project_root.as_deref());
        let attributed_physical_key = identity.physical_target_key.clone();
        let mut config = None;
        let operation = reviewed_config_operation(agent, finding.finding.cause());
        if let Some(home) = home
            && (operation.is_some()
                || matches!(
                    finding.finding.cause(),
                    FindingCause::SessionsOverDepth { .. }
                ))
            && automatic_editor_supported(
                agent,
                operation
                    .as_ref()
                    .map_or(ConfigSetting::Compaction, |operation| operation.setting),
                finding.finding.source_format,
                &identity.scope_kind,
                &finding.environment_key,
                current_editor_platform(),
            )
            && (finding.workspace_candidate().is_none() || project_root.is_some())
            && workspace_precedence_supported(
                agent,
                finding.workspace_candidate(),
                project_root.as_deref(),
            )
        {
            let Some(mut context) = config_context(
                agent,
                home,
                finding.workspace_candidate(),
                project_root.as_deref(),
            ) else {
                return Ok((
                    identity.group_key,
                    CachedTarget {
                        findings: vec![finding],
                        resource: None,
                        target_key: identity.target_key,
                        canonical_identity: identity.canonical_identity,
                        workspace_key: identity.workspace_key,
                        agent,
                        scope_kind: identity.scope_kind,
                        scope_key: identity.scope_key,
                        physical_target_key: identity.physical_target_key,
                        config: None,
                    },
                ));
            };
            context.runtime_override_present = runtime_override_present(agent);
            context.managed_configuration_present = managed_configuration_present(agent, home);
            if let Some(operation) = operation.or_else(|| {
                self.editor
                    .effective(&context, ConfigSetting::Compaction)
                    .ok()
                    .and_then(|effective| {
                        compaction_operation(finding.finding.cause(), &effective.value)
                    })
            }) {
                let effective = self.editor.effective_for_value(
                    &context,
                    operation.setting,
                    operation
                        .expected_value
                        .scalar()
                        .or_else(|| operation.expected_value.key()),
                );
                if let Ok(effective) = effective
                    && operation.expected_value.display_value() == effective.value
                {
                    let selector_qualifier = physical_selector_qualifier(
                        &self.editor,
                        &context,
                        effective.physical_identity().1,
                    );
                    let key = physical_key(
                        store,
                        agent,
                        effective.physical_identity(),
                        selector_qualifier.as_deref(),
                    )
                    .map_err(|_| ControllerError::Internal)?;
                    if let Some(attributed_key) = attributed_physical_key.as_ref() {
                        if attributed_key == &key
                            && identity.scope_kind == scope_name(effective.scope)
                        {
                            let display_path = display_config_file(
                                effective.physical_identity().0,
                                &context.home_root,
                            );
                            config = Some(CachedConfig {
                                context: context.clone(),
                                additional_contexts: Vec::new(),
                                operation: operation.clone(),
                                physical_key: key,
                                display_path,
                            });
                        }
                    } else {
                        bind_current_config_identity(
                            &mut identity,
                            &secret,
                            &finding.environment_key,
                            &finding.finding,
                            agent,
                            effective.scope,
                            &key,
                        );
                        let display_path = display_config_file(
                            effective.physical_identity().0,
                            &context.home_root,
                        );
                        config = Some(CachedConfig {
                            context: context.clone(),
                            additional_contexts: Vec::new(),
                            operation: operation.clone(),
                            physical_key: key,
                            display_path,
                        });
                    }
                } else if attributed_physical_key.is_none()
                    && operation.setting == ConfigSetting::BuiltInTool
                    && let Ok(prepared) = self.editor.prepare_operation(&context, &operation)
                    && prepared.creates_file()
                {
                    let key = physical_key(store, agent, prepared.physical_identity(), None)
                        .map_err(|_| ControllerError::Internal)?;
                    bind_current_config_identity(
                        &mut identity,
                        &secret,
                        &finding.environment_key,
                        &finding.finding,
                        agent,
                        ConfigScope::Global,
                        &key,
                    );
                    let display_path = display_config_file(prepared.physical_identity().0, home);
                    config = Some(CachedConfig {
                        context,
                        additional_contexts: Vec::new(),
                        operation,
                        physical_key: key,
                        display_path,
                    });
                }
            }
        }
        Ok((
            identity.group_key,
            CachedTarget {
                findings: vec![finding],
                resource: None,
                target_key: identity.target_key,
                canonical_identity: identity.canonical_identity,
                workspace_key: identity.workspace_key,
                agent,
                scope_kind: identity.scope_kind,
                scope_key: identity.scope_key,
                physical_target_key: identity.physical_target_key,
                config,
            },
        ))
    }

    fn resolve_resource_target(
        &self,
        store: &Store,
        resource: &insights_report::UnusedResourceTarget,
        context: BurnCheckTargetContext,
        home: Option<&Path>,
    ) -> Result<CachedTarget, ControllerError> {
        let source_format = match resource.agent {
            AgentKind::Claude => SourceFormat::ClaudeJsonl,
            AgentKind::Codex => SourceFormat::CodexRolloutJsonl,
            AgentKind::Cursor => SourceFormat::CursorCliAgentJsonl,
            AgentKind::Copilot => SourceFormat::CopilotIdeChatJson,
            AgentKind::Cline => SourceFormat::ClineSessionJson,
            AgentKind::OpenCode => SourceFormat::OpenCodeJsonl,
            AgentKind::Kiro => SourceFormat::KiroSessionJson,
            AgentKind::AmpCode => SourceFormat::AmpThreadJson,
            AgentKind::Antigravity => SourceFormat::AntigravityCascadeJson,
            AgentKind::Windsurf => SourceFormat::DevinLocalSqlite,
            AgentKind::Pi => SourceFormat::PiV3Jsonl,
            AgentKind::Omp => SourceFormat::OmpV3Jsonl,
        };
        let cause = match resource.kind {
            crate::agent_config::ResourceKind::McpServer => FindingCause::UnusedMcpServer {
                server: resource.canonical_name.clone(),
                tokens: resource.replicated_tokens,
                cost_usd: None,
                pricing_revision: None,
            },
            crate::agent_config::ResourceKind::BuiltInTool => FindingCause::UnusedBuiltInTool {
                tool: resource.canonical_name.clone(),
                tokens: antiburn_local::remediation::BuiltInToolTokens::Replicated(
                    resource.replicated_tokens.unwrap_or(0),
                ),
                cost_usd: None,
                pricing_revision: None,
            },
            crate::agent_config::ResourceKind::Skill => FindingCause::UnusedSkill {
                skill: resource.canonical_name.clone(),
                tokens: resource.replicated_tokens,
                cost_usd: None,
                pricing_revision: None,
            },
        };
        let finding = Finding::advisory_resource(resource.agent, source_format, cause)
            .ok_or(ControllerError::Internal)?;
        let secret = store
            .provider_account_secret()
            .map_err(|_| ControllerError::Internal)?;
        let mut identity =
            resource_target_identity(&secret, &context.environment_key, &finding, &resource.scope)
                .ok_or(ControllerError::Internal)?;
        let mut config = None;
        let operation = reviewed_config_operation(resource.agent, finding.cause());
        let project_root = match &resource.scope {
            insights_report::ResourceAssessmentScope::Global => None,
            insights_report::ResourceAssessmentScope::Project(root) => Some(root.as_path()),
        };
        if resource.indexed
            && let (Some(home), Some(operation)) = (home, operation)
            && automatic_editor_supported(
                resource.agent,
                operation.setting,
                source_format,
                &identity.scope_kind,
                &context.environment_key,
                current_editor_platform(),
            )
            && workspace_precedence_supported(resource.agent, project_root, project_root)
            && let Some(mut config_context) =
                config_context(resource.agent, home, project_root, project_root)
        {
            config_context.runtime_override_present = runtime_override_present(resource.agent);
            config_context.managed_configuration_present =
                managed_configuration_present(resource.agent, home);
            let effective = self.editor.effective_for_value(
                &config_context,
                operation.setting,
                operation
                    .expected_value
                    .scalar()
                    .or_else(|| operation.expected_value.key()),
            );
            if let Ok(effective) = effective
                && effective.value == operation.expected_value.display_value()
                && scope_name(effective.scope) == identity.scope_kind
            {
                let selector_qualifier = physical_selector_qualifier(
                    &self.editor,
                    &config_context,
                    effective.physical_identity().1,
                );
                let key = physical_key(
                    store,
                    resource.agent,
                    effective.physical_identity(),
                    selector_qualifier.as_deref(),
                )
                .map_err(|_| ControllerError::Internal)?;
                bind_current_config_identity(
                    &mut identity,
                    &secret,
                    &context.environment_key,
                    &finding,
                    resource.agent,
                    effective.scope,
                    &key,
                );
                let display_path = display_config_file(effective.physical_identity().0, home);
                config = Some(CachedConfig {
                    context: config_context,
                    additional_contexts: Vec::new(),
                    operation,
                    physical_key: key,
                    display_path,
                });
            }
        }
        Ok(CachedTarget {
            findings: Vec::new(),
            resource: Some(CachedResourceTarget {
                target: resource.clone(),
                context,
                finding,
            }),
            target_key: identity.target_key,
            canonical_identity: identity.canonical_identity,
            workspace_key: identity.workspace_key,
            agent: resource.agent,
            scope_kind: identity.scope_kind,
            scope_key: identity.scope_key,
            physical_target_key: identity.physical_target_key,
            config,
        })
    }

    fn cached_target(
        &self,
        store: &Store,
        id: &str,
        now: i64,
    ) -> Result<CachedTarget, ControllerError> {
        let (enabled, check_preferences_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let current_smart_master_generation = store.internal_value(SMART_CHECKS_ENABLED_AT_KEY);
        let state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        let entry = state
            .targets
            .values()
            .flatten()
            .find(|entry| entry.id == id)
            .ok_or(ControllerError::TargetNotFound)?;
        if now.saturating_sub(entry.created_at_epoch) > ID_TTL.as_secs() as i64 {
            return Err(ControllerError::TargetExpired);
        }
        if entry.check_preferences_revision != check_preferences_revision {
            return Err(ControllerError::TargetChanged);
        }
        if crate::jev::worker::registered_check_ids().contains(&entry.value.finding().detector)
            && entry.smart_master_generation != current_smart_master_generation
        {
            return Err(ControllerError::TargetChanged);
        }
        if !smart_check_master_available(store, entry.value.finding().detector) {
            return Err(ControllerError::TargetNotFound);
        }
        if !enabled.contains(&entry.value.finding().detector) {
            return Err(ControllerError::TargetNotFound);
        }
        Ok(entry.value.clone())
    }

    fn check_policy_matches(
        &self,
        store: &Store,
        detector: DetectorId,
        expected_revision: u64,
        expected_smart_master_generation: Option<&str>,
    ) -> Result<bool, ControllerError> {
        let (enabled, current_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        Ok(enabled.contains(&detector)
            && current_revision == expected_revision
            && smart_check_master_available(store, detector)
            && smart_check_master_generation(store, detector).as_deref()
                == expected_smart_master_generation)
    }

    fn reduce_resource_report(
        &self,
        context: &BurnCheckTargetContext,
    ) -> Result<insights_report::ReducedReport, ControllerError> {
        insights_report::reduce_report_blocking(
            &self.data_dir,
            insights_report::ReportRequest {
                environment_key: context.environment_key.clone(),
                window: context.window,
                computed_at_epoch: context.window.end_epoch,
            },
        )
        .map_err(|_| ControllerError::Internal)
    }

    /// Revalidates a whole batch with one report reduce per distinct context,
    /// instead of one reduce per target.
    fn revalidate_all(&self, targets: &[CachedTarget]) -> Result<(), ControllerError> {
        let mut reports: Vec<(BurnCheckTargetContext, insights_report::ReducedReport)> = Vec::new();
        for target in targets {
            let Some(resource) = &target.resource else {
                self.revalidate(target)?;
                continue;
            };
            let index = match reports
                .iter()
                .position(|(context, _)| *context == resource.context)
            {
                Some(index) => index,
                None => {
                    reports.push((
                        resource.context.clone(),
                        self.reduce_resource_report(&resource.context)?,
                    ));
                    reports.len() - 1
                }
            };
            if !resource_is_current(&reports[index].1, resource) {
                return Err(ControllerError::TargetChanged);
            }
        }
        Ok(())
    }

    fn revalidate(&self, target: &CachedTarget) -> Result<(), ControllerError> {
        if let Some(resource) = &target.resource {
            let report = self.reduce_resource_report(&resource.context)?;
            return resource_is_current(&report, resource)
                .then_some(())
                .ok_or(ControllerError::TargetChanged);
        }
        for finding in &target.findings {
            match insights_report::revalidate_current_finding(&self.data_dir, finding) {
                Ok(true) => {}
                Ok(false) => return Err(ControllerError::TargetChanged),
                Err(_) => return Err(ControllerError::Internal),
            }
        }
        Ok(())
    }

    fn revalidate_prepared(
        &self,
        store: &Store,
        target: &CachedTarget,
        prepared: &PreparedOperation,
    ) -> Result<(), ControllerError> {
        self.revalidate(target)?;
        let config = target
            .config
            .as_ref()
            .ok_or(ControllerError::TargetChanged)?;
        if prepared.creates_file() {
            let selector_qualifier = physical_selector_qualifier(
                &self.editor,
                &config.context,
                prepared.physical_identity().1,
            );
            let current_key = physical_key(
                store,
                target.agent,
                prepared.physical_identity(),
                selector_qualifier.as_deref(),
            )
            .map_err(|_| ControllerError::Internal)?;
            if prepared.setting() != config.operation.setting
                || prepared.scope()
                    != scope_from_name(&target.scope_kind).ok_or(ControllerError::TargetChanged)?
                || current_key != config.physical_key
            {
                return Err(ControllerError::Conflict);
            }
            for context in std::iter::once(&config.context).chain(&config.additional_contexts) {
                if !self.prepared_context_matches(
                    store,
                    target.agent,
                    &target.scope_kind,
                    config,
                    context,
                    true,
                )? {
                    return Err(ControllerError::Conflict);
                }
            }
            return Ok(());
        }
        for context in std::iter::once(&config.context).chain(&config.additional_contexts) {
            if !self.config_context_matches(store, target, config, context)? {
                return Err(ControllerError::Conflict);
            }
        }
        let effective = self
            .editor
            .effective_for_value(
                &refreshed_config_context(&config.context),
                config.operation.setting,
                config
                    .operation
                    .expected_value
                    .scalar()
                    .or_else(|| config.operation.expected_value.key()),
            )
            .map_err(|_| ControllerError::Conflict)?;
        let selector_qualifier = physical_selector_qualifier(
            &self.editor,
            &config.context,
            effective.physical_identity().1,
        );
        let current_key = physical_key(
            store,
            target.agent,
            effective.physical_identity(),
            selector_qualifier.as_deref(),
        )
        .map_err(|_| ControllerError::Internal)?;
        if config.operation.expected_value.display_value() != effective.value
            || effective.setting != prepared.setting()
            || effective.scope != prepared.scope()
            || effective.scope
                != scope_from_name(&target.scope_kind).ok_or(ControllerError::TargetChanged)?
            || current_key != config.physical_key
            || physical_key(
                store,
                target.agent,
                prepared.physical_identity(),
                selector_qualifier.as_deref(),
            )
            .map_err(|_| ControllerError::Internal)?
                != config.physical_key
        {
            return Err(ControllerError::Conflict);
        }
        Ok(())
    }

    fn config_context_matches(
        &self,
        store: &Store,
        target: &CachedTarget,
        config: &CachedConfig,
        context: &ConfigContext,
    ) -> Result<bool, ControllerError> {
        let Ok(effective) = self.editor.effective_for_value(
            &refreshed_config_context(context),
            config.operation.setting,
            config
                .operation
                .expected_value
                .scalar()
                .or_else(|| config.operation.expected_value.key()),
        ) else {
            return Ok(false);
        };
        let selector_qualifier =
            physical_selector_qualifier(&self.editor, context, effective.physical_identity().1);
        let key = physical_key(
            store,
            target.agent,
            effective.physical_identity(),
            selector_qualifier.as_deref(),
        )
        .map_err(|_| ControllerError::Internal)?;
        Ok(
            config.operation.expected_value.display_value() == effective.value
                && scope_name(effective.scope) == target.scope_kind
                && key == config.physical_key,
        )
    }

    fn prepared_context_matches(
        &self,
        store: &Store,
        agent: AgentKind,
        scope_kind: &str,
        config: &CachedConfig,
        context: &ConfigContext,
        creates_file: bool,
    ) -> Result<bool, ControllerError> {
        let Ok(prepared) = self
            .editor
            .prepare_operation(&refreshed_config_context(context), &config.operation)
        else {
            return Ok(false);
        };
        let selector_qualifier =
            physical_selector_qualifier(&self.editor, context, prepared.physical_identity().1);
        let key = physical_key(
            store,
            agent,
            prepared.physical_identity(),
            selector_qualifier.as_deref(),
        )
        .map_err(|_| ControllerError::Internal)?;
        Ok(prepared.creates_file() == creates_file
            && prepared.setting() == config.operation.setting
            && scope_name(prepared.scope()) == scope_kind
            && key == config.physical_key)
    }

    fn persist_display_snapshot(
        &self,
        store: &Store,
        watch: &RemediationRecord,
        target: &CachedTarget,
        origin: &str,
        fallback_boundary_ms: i64,
    ) -> Result<(), ControllerError> {
        let current = store
            .remediation(&watch.remediation_id)
            .map_err(|_| ControllerError::PersistenceFailed)?
            .ok_or(ControllerError::PersistenceFailed)?;
        validate_envelope_version(&current.result_json, "remediation result")
            .map_err(|_| ControllerError::Internal)?;
        let reservation: serde_json::Value =
            serde_json::from_str(&current.result_json).map_err(|_| ControllerError::Internal)?;
        let boundary = current
            .effective_boundary_ms
            .or_else(|| {
                reservation
                    .get("priorBoundaryMs")
                    .and_then(serde_json::Value::as_i64)
            })
            .unwrap_or(fallback_boundary_ms);
        let snapshot = StoredDisplaySnapshot {
            version: 1,
            finding_id: stable_finding_id(target),
            display: burn_check_display_facts(target, None),
        };
        let existing = store
            .remediation_display_snapshot(&current.remediation_id)
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let saved = RemediationDisplaySnapshot {
            remediation_id: current.remediation_id,
            origin: existing
                .as_ref()
                .map(|snapshot| snapshot.origin.clone())
                .or_else(|| {
                    reservation
                        .get("priorOrigin")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| origin.to_owned()),
            display_snapshot_json: reservation
                .get("priorDisplaySnapshot")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .unwrap_or(
                    serde_json::to_string(&snapshot).map_err(|_| ControllerError::Internal)?,
                ),
            effective_boundary_ms: boundary,
            verified_boundary_ms: existing
                .as_ref()
                .and_then(|snapshot| snapshot.verified_boundary_ms)
                .or_else(|| {
                    reservation
                        .get("priorVerifiedBoundaryMs")
                        .and_then(serde_json::Value::as_i64)
                }),
            recurred_boundary_ms: existing
                .as_ref()
                .and_then(|snapshot| snapshot.recurred_boundary_ms)
                .or_else(|| {
                    reservation
                        .get("priorRecurredBoundaryMs")
                        .and_then(serde_json::Value::as_i64)
                }),
        };
        if !store
            .upsert_remediation_display_snapshot(&saved)
            .map_err(|_| ControllerError::PersistenceFailed)?
        {
            return Err(ControllerError::PersistenceFailed);
        }
        Ok(())
    }

    fn remove_prepared(&self, id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.prepared.retain(|entry| entry.id != id);
        }
    }

    fn prompt_reference_for_targets(
        &self,
        store: &Store,
        targets: &[CachedTarget],
    ) -> Result<(String, Option<String>), ControllerError> {
        if targets.is_empty() {
            let group = random_id().map_err(|_| ControllerError::Internal)?;
            return Ok((group.clone(), Some(group.clone())));
        }
        let existing = targets
            .iter()
            .map(|target| {
                store
                    .latest_action_remediation_for_target(
                        target.environment_key(),
                        target.agent.slug(),
                        &target.scope_kind,
                        &target.scope_key,
                        &target.target_key,
                    )
                    .map_err(|_| ControllerError::Internal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let active = existing
            .iter()
            .filter_map(Option::as_ref)
            .filter(|watch| watch.state != RemediationState::Recurred)
            .collect::<Vec<_>>();
        if active.is_empty() {
            let group = random_id().map_err(|_| ControllerError::Internal)?;
            return Ok((group.clone(), Some(group)));
        }
        if active
            .iter()
            .any(|watch| watch.state != RemediationState::WaitingForPromptUse)
        {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let first_group = active[0].prompt_group_id.clone();
        if let Some(group) = first_group.as_ref().filter(|group| {
            active
                .iter()
                .all(|watch| watch.prompt_group_id.as_deref() == Some(group.as_str()))
        }) {
            return Ok((group.clone(), Some(group.clone())));
        }
        if targets.len() == 1 && active.len() == 1 {
            return Ok((active[0].remediation_id.clone(), None));
        }
        Err(ControllerError::CheckPromptUnavailable)
    }

    fn persist_prompt_watches(
        &self,
        store: &Store,
        targets: &[CachedTarget],
        prompt_group_id: Option<&str>,
        prompt: &str,
        now: i64,
    ) -> Result<(Vec<RemediationRecord>, String), ControllerError> {
        if !prompt_watch_supported(targets.iter().map(|target| target.finding().detector)) {
            return Ok((Vec::new(), prompt.to_owned()));
        }
        let detector = targets
            .first()
            .map(|target| target.finding().detector)
            .ok_or(ControllerError::TargetNotFound)?;
        let (enabled, check_preferences_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        if !enabled.contains(&detector) {
            return Err(ControllerError::TargetNotFound);
        }
        let smart_master_generation = smart_check_master_generation(store, detector);
        if !smart_check_master_available(store, detector) {
            return Err(ControllerError::TargetNotFound);
        }
        let watch_inputs = targets
            .iter()
            .map(|target| {
                let (mut remediation, guards) = self.watch_input(
                    target,
                    RemediationState::WaitingForPromptUse,
                    None,
                    now,
                    prompt_group_id,
                )?;
                let mut result: serde_json::Value = serde_json::from_str(&remediation.result_json)
                    .map_err(|_| ControllerError::Internal)?;
                result["promptText"] = serde_json::Value::String(prompt.to_owned());
                remediation.result_json = result.to_string();
                Ok((remediation, guards))
            })
            .collect::<Result<Vec<_>, ControllerError>>()?;
        let watches = store
            .create_or_reuse_remediations_for_enabled_check(
                &watch_inputs,
                detector,
                check_preferences_revision,
                smart_master_generation.as_deref(),
            )
            .map_err(|_| ControllerError::PersistenceFailed)?
            .ok_or(ControllerError::TargetChanged)?;
        for (target, watch) in targets.iter().zip(watches.iter()) {
            self.persist_display_snapshot(
                store,
                watch,
                target,
                "action",
                now.saturating_mul(1_000),
            )?;
        }
        let stored_prompt = watches
            .first()
            .and_then(|watch| stored_string(&watch.result_json, "promptText"))
            .ok_or(ControllerError::PersistenceFailed)?;
        if watches.iter().any(|watch| {
            stored_string(&watch.result_json, "promptText").as_deref()
                != Some(stored_prompt.as_str())
        }) {
            return Err(ControllerError::PersistenceFailed);
        }
        Ok((watches, stored_prompt))
    }

    #[cfg(all(test, not(windows)))]
    fn start_watch(
        &self,
        store: &Store,
        target: &CachedTarget,
        state: RemediationState,
        boundary_ms: Option<i64>,
        now: i64,
        prompt_group_id: Option<&str>,
    ) -> Result<RemediationRecord, ControllerError> {
        let (enabled, check_preferences_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        if !enabled.contains(&target.finding().detector) {
            return Err(ControllerError::TargetNotFound);
        }
        let smart_master_generation =
            smart_check_master_generation(store, target.finding().detector);
        if !smart_check_master_available(store, target.finding().detector) {
            return Err(ControllerError::TargetNotFound);
        }
        let (remediation, guards) =
            self.watch_input(target, state, boundary_ms, now, prompt_group_id)?;
        store
            .create_or_reuse_remediations_for_enabled_check(
                &[(remediation, guards)],
                target.finding().detector,
                check_preferences_revision,
                smart_master_generation.as_deref(),
            )
            .map_err(|_| ControllerError::PersistenceFailed)?
            .and_then(|mut watches| watches.pop())
            .ok_or(ControllerError::TargetChanged)
    }

    fn watch_input(
        &self,
        target: &CachedTarget,
        state: RemediationState,
        boundary_ms: Option<i64>,
        now: i64,
        prompt_group_id: Option<&str>,
    ) -> Result<(Remediation, Vec<RemediationEvidenceGuard>), ControllerError> {
        let definition = watch_definition(target);
        let result = if state == RemediationState::Reserved {
            json!({"version": 1, "verification": {"status": "reserved"}, "savings": {"status": "pending"}})
        } else if !watch_verification_available(
            &definition,
            &target.scope_kind,
            target.agent.slug(),
            target.finding().detector,
        ) {
            json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}})
        } else if definition.old_model.is_none() {
            json!({"version": 1, "verification": {"status": "watching", "methodRevision": VERIFICATION_METHOD_REVISION}, "savings": {"status": "unavailable"}})
        } else if definition.physical_target_key.is_some() {
            json!({"version": 1, "verification": {"status": "watching", "methodRevision": VERIFICATION_METHOD_REVISION}, "savings": {"status": "pending", "methodRevision": SAVINGS_METHOD_REVISION}})
        } else {
            json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}})
        };
        let remediation = Remediation {
            remediation_id: random_id().map_err(|_| ControllerError::Internal)?,
            target_key: target.target_key.clone(),
            environment_key: target.environment_key().to_owned(),
            agent: target.agent.slug().into(),
            scope_kind: target.scope_kind.clone(),
            scope_key: target.scope_key.clone(),
            state,
            origin: "action".into(),
            prompt_group_id: prompt_group_id.map(str::to_owned),
            definition_json: serde_json::to_string(&definition)
                .map_err(|_| ControllerError::Internal)?,
            result_json: result.to_string(),
            created_at_epoch: now,
            effective_boundary_ms: boundary_ms,
        };
        let guards = target
            .findings
            .iter()
            .map(evidence_guard)
            .collect::<Vec<_>>();
        Ok((remediation, guards))
    }
}

fn resolve_category_lifecycle(
    detector: DetectorId,
    finding: u64,
    clean: u64,
    awaiting_evidence: bool,
) -> Option<ChecksCategoryLifecyclePayload> {
    if finding > 0
        && matches!(
            detector,
            DetectorId::IgnoredInstructions
                | DetectorId::SkillOpportunities
                | DetectorId::OverExploring
                | DetectorId::ScopeCreep
        )
    {
        Some(ChecksCategoryLifecyclePayload::Failing)
    } else if awaiting_evidence {
        Some(ChecksCategoryLifecyclePayload::AwaitingVerification)
    } else if clean > 0 {
        Some(ChecksCategoryLifecyclePayload::Passing)
    } else if finding > 0 {
        Some(ChecksCategoryLifecyclePayload::Failing)
    } else {
        None
    }
}

fn auto_fix_blocked_by_watch(watch: &WatchStatus) -> bool {
    watch.origin == RemediationOrigin::Action
        && !matches!(watch.lifecycle, RemediationState::Recurred)
}

fn display_config_file(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

fn stored_string(value: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

const fn remediation_policy_is_current(definition: &WatchDefinition) -> bool {
    definition.version == 1
        && matches!(definition.remediation_policy_revision, Some(value) if value == REMEDIATION_POLICY_REVISION)
        && definition.verification_method_revision == VERIFICATION_METHOD_REVISION
        && definition.savings_method_revision == SAVINGS_METHOD_REVISION
}

fn watch_verification_available(
    definition: &WatchDefinition,
    scope_kind: &str,
    agent: &str,
    detector: DetectorId,
) -> bool {
    if !matches!(scope_kind, "global" | "project")
        || definition.detector != detector.key()
        || !verification_source_matches_agent(agent, definition.source_format.value())
        || !desktop_watch_verification_supported(detector, definition.source_format.value())
    {
        return false;
    }
    match detector {
        DetectorId::OldModelUsage => {
            definition.resource.is_none()
                && definition.old_model.is_some()
                && definition.replacement.is_some()
                && definition.physical_target_key.is_some()
                && definition.config_setting.as_deref() == Some("model")
        }
        DetectorId::ModelOverthinking | DetectorId::OveruseOfFastMode => {
            definition.resource.is_none()
                && definition.target_model.is_some()
                && definition.target_control.is_some()
        }
        DetectorId::UnusedMcpServers
        | DetectorId::UnusedBuiltInTools
        | DetectorId::UnusedSkills => false,
        DetectorId::OverpoweredSubagents => false,
        _ => false,
    }
}

fn desktop_watch_verification_supported(detector: DetectorId, source: SourceFormat) -> bool {
    !matches!(
        detector,
        DetectorId::UnusedMcpServers | DetectorId::UnusedBuiltInTools | DetectorId::UnusedSkills
    ) && verification_evidence_supported(detector, source)
}

fn verification_source_matches_agent(agent: &str, source_format: SourceFormat) -> bool {
    matches!(
        (agent, source_format),
        ("claude-code", SourceFormat::ClaudeJsonl)
            | ("codex", SourceFormat::CodexRolloutJsonl)
            | (
                "opencode",
                SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2
            )
            | ("pi", SourceFormat::PiV3Jsonl)
            | ("omp", SourceFormat::OmpV3Jsonl)
    )
}

fn bind_current_config_identity(
    identity: &mut TargetIdentity,
    secret: &[u8; 32],
    environment_key: &str,
    finding: &Finding,
    agent: AgentKind,
    scope: ConfigScope,
    physical_key: &str,
) {
    identity.scope_kind = scope_name(scope).to_owned();
    identity.scope_key = if scope == ConfigScope::Global {
        physical_key.to_owned()
    } else {
        identity
            .workspace_key
            .clone()
            .unwrap_or_else(|| physical_key.to_owned())
    };
    identity.canonical_identity = finding.canonical_identity(&identity.scope_key);
    identity.physical_target_key = Some(physical_key.to_owned());
    identity.group_key = hashed_parts_with_secret(
        secret,
        TARGET_DOMAIN,
        &[
            environment_key,
            agent.slug(),
            &identity.scope_kind,
            &identity.scope_key,
            physical_key,
            &identity.canonical_identity,
        ],
    );
    identity.target_key = hashed_parts_with_secret(
        secret,
        TARGET_DOMAIN,
        &[
            environment_key,
            agent.slug(),
            physical_key,
            &identity.canonical_identity,
        ],
    );
}

fn prune_prepared(state: &mut ControllerState, now: i64) {
    state
        .prepared
        .retain(|entry| now.saturating_sub(entry.created_at_epoch) <= ID_TTL.as_secs() as i64);
}

fn prepared_retained_bytes(state: &ControllerState) -> usize {
    state
        .prepared
        .iter()
        .map(|entry| entry.retained_bytes)
        .fold(0, usize::saturating_add)
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).context("random id generation failed")?;
    Ok(hex(&bytes))
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
