use std::collections::BTreeSet;

use crate::analysis::jev_evidence::ContentEventReference;
use crate::analysis::{SessionEvidence, SourceFormat};
use crate::checks::ignored_instructions::{
    AssessmentFinding, FindingCertainty, InstructionProvenance, InstructionScope,
};
use crate::checks::skill_opportunities::SkillOpportunityFinding;
use crate::insights::{
    DetectorId, ReportCatalogs, SessionTokenBurnEvidence, clean_facts_complete, eligible,
};
use crate::model::AgentKind;

use super::{MAX_DISPLAY_LABEL_BYTES, MAX_PROMPT_IDENTITIES, RemediationUnavailableReason};

/// One bounded request fact that contributed to a finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestFact {
    pub model: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub value: u64,
}

/// Identifies the token quantity available for an unused built-in tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInToolTokens {
    /// Tokens in one catalog-backed tool definition.
    Definition(u64),
    /// Definition tokens repeated across compatible main turns.
    Replicated(u128),
}

/// Typed evidence for one actionable finding target.
#[derive(Debug, Clone, PartialEq)]
pub enum FindingCause {
    SessionsOverDepth {
        maximum_tokens: u64,
        limit_tokens: u64,
        requests: Vec<RequestFact>,
        omitted_requests: Option<u64>,
    },
    ModelOverthinking {
        provider: Option<String>,
        api: Option<String>,
        model: String,
        reasoning: String,
        turns: u64,
    },
    OverpoweredSubagents {
        parent_model: String,
        worker_model: String,
        worker_ordinal: u32,
        parent_call_id: Option<String>,
    },
    UnusedMcpServer {
        server: String,
        /// Replicated definition tokens from source evidence, when available.
        tokens: Option<u128>,
        /// The priced cost of `tokens`, when both price and revision resolve.
        cost_usd: Option<f64>,
        /// The pricing table generation that priced `cost_usd`.
        pricing_revision: Option<String>,
    },
    UnusedBuiltInTool {
        tool: String,
        tokens: BuiltInToolTokens,
        /// The priced cost of a `BuiltInToolTokens::Replicated` count.
        cost_usd: Option<f64>,
        /// The pricing table generation that priced `cost_usd`.
        pricing_revision: Option<String>,
    },
    UnusedSkill {
        skill: String,
        /// Replicated definition tokens from source evidence, when available.
        tokens: Option<u128>,
        /// The priced cost of `tokens`, when both price and revision resolve.
        cost_usd: Option<f64>,
        /// The pricing table generation that priced `cost_usd`.
        pricing_revision: Option<String>,
    },
    SkillOpportunity {
        evidence: Option<Box<SkillOpportunityFinding>>,
        skill_name: String,
        skill_description: String,
        cited_work_context: String,
        work_provenance: String,
        selected_window_limit: String,
    },
    OldModelUsage {
        provider: Option<String>,
        api: Option<String>,
        model: String,
        replacement: String,
        turns: u64,
    },
    OveruseOfFastMode {
        provider: Option<String>,
        api: Option<String>,
        model: String,
        delegated_turns: u64,
    },
    CacheChurn {
        model: String,
        repeated_tokens: u64,
        paid_tokens: u64,
        threshold_basis_points: u32,
    },
    IgnoredInstructionConflict(Box<IgnoredInstructionConflictEvidence>),
    OverExploring(Box<crate::checks::over_exploring::Decision>),
    ScopeCreep(Box<crate::checks::scope_creep::ScopeCreepFinding>),
}

/// Evidence for one ignored-instruction conflict.
#[derive(Debug, Clone, PartialEq)]
pub struct IgnoredInstructionConflictEvidence {
    pub decision: Option<crate::checks::ignored_instructions::DecisionRecord>,
    pub assessment_revision: String,
    pub assessment_finding_id: String,
    pub instruction_id: String,
    pub instruction_digest: String,
    pub instruction_excerpt: String,
    pub instruction_excerpt_truncated: bool,
    pub rule_id: String,
    pub rule_heading: String,
    pub start_line: u32,
    pub end_line: u32,
    pub source: String,
    pub provenance: InstructionProvenance,
    pub instruction_scope: InstructionScope,
    pub action_id: String,
    pub action_digest: String,
    pub action_excerpt: String,
    pub action_excerpt_truncated: bool,
    pub action_timestamp_ms: Option<i64>,
    pub nearby_context_ids: Vec<String>,
    pub counterevidence_ids: Vec<String>,
    pub certainty: FindingCertainty,
    pub limitations: Box<Vec<String>>,
}

impl IgnoredInstructionConflictEvidence {
    pub fn decision_record(&self) -> Option<&crate::checks::ignored_instructions::DecisionRecord> {
        self.decision.as_ref().filter(|decision| {
            let reference = &decision.rule_action;
            decision.has_citation_proof()
                && reference.instruction_id == self.instruction_id
                && reference.instruction_digest == self.instruction_digest
                && reference.rule_id == self.rule_id
                && reference.rule_heading == self.rule_heading
                && reference.start_line == self.start_line
                && reference.end_line == self.end_line
                && reference.source == self.source
                && reference.provenance == self.provenance
                && reference.scope == self.instruction_scope
                && reference.action_id == self.action_id
                && reference.action_digest == self.action_digest
                && reference.action_timestamp_ms == self.action_timestamp_ms
        })
    }
}

impl FindingCause {
    pub const fn detector(&self) -> DetectorId {
        match self {
            Self::SessionsOverDepth { .. } => DetectorId::SessionsOverDepth,
            Self::ModelOverthinking { .. } => DetectorId::ModelOverthinking,
            Self::OverpoweredSubagents { .. } => DetectorId::OverpoweredSubagents,
            Self::UnusedMcpServer { .. } => DetectorId::UnusedMcpServers,
            Self::UnusedBuiltInTool { .. } => DetectorId::UnusedBuiltInTools,
            Self::UnusedSkill { .. } => DetectorId::UnusedSkills,
            Self::SkillOpportunity { .. } => DetectorId::SkillOpportunities,
            Self::OldModelUsage { .. } => DetectorId::OldModelUsage,
            Self::OveruseOfFastMode { .. } => DetectorId::OveruseOfFastMode,
            Self::CacheChurn { .. } => DetectorId::CacheChurn,
            Self::IgnoredInstructionConflict(_) => DetectorId::IgnoredInstructions,
            Self::OverExploring(_) => DetectorId::OverExploring,
            Self::ScopeCreep(_) => DetectorId::ScopeCreep,
        }
    }

    fn display_labels(&self) -> Vec<&str> {
        match self {
            Self::OverExploring(_) | Self::ScopeCreep(_) => Vec::new(),
            Self::SessionsOverDepth { requests, .. } => requests
                .iter()
                .filter_map(|request| request.model.as_deref())
                .collect(),
            Self::ModelOverthinking {
                provider,
                api,
                model,
                reasoning,
                ..
            } => provider
                .iter()
                .chain(api.iter())
                .map(String::as_str)
                .chain([model.as_str(), reasoning.as_str()])
                .collect(),
            Self::OverpoweredSubagents {
                parent_model,
                worker_model,
                ..
            } => vec![parent_model, worker_model],
            Self::UnusedMcpServer { server, .. } => vec![server],
            Self::UnusedBuiltInTool { tool, .. } => vec![tool],
            Self::UnusedSkill { skill, .. } => vec![skill],
            Self::SkillOpportunity {
                skill_name,
                skill_description,
                ..
            } => vec![skill_name, skill_description],
            Self::OldModelUsage {
                provider,
                api,
                model,
                replacement,
                ..
            } => provider
                .iter()
                .chain(api.iter())
                .map(String::as_str)
                .chain([model.as_str(), replacement.as_str()])
                .collect(),
            Self::OveruseOfFastMode {
                provider,
                api,
                model,
                ..
            } => provider
                .iter()
                .chain(api.iter())
                .map(String::as_str)
                .chain([model.as_str()])
                .collect(),
            Self::CacheChurn { model, .. } => vec![model],
            Self::IgnoredInstructionConflict(evidence) => vec![
                &evidence.source,
                &evidence.rule_heading,
                &evidence.rule_id,
                &evidence.action_id,
            ],
        }
    }
}

/// One current per-target finding with exact private selectors.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub detector: DetectorId,
    pub source_format: SourceFormat,
    agent: String,
    session_id: String,
    cause: FindingCause,
}

impl Finding {
    /// Builds one advisory resource finding without inventing session evidence.
    pub fn advisory_resource(
        agent: AgentKind,
        source_format: SourceFormat,
        cause: FindingCause,
    ) -> Option<Self> {
        matches!(
            cause,
            FindingCause::UnusedMcpServer { .. }
                | FindingCause::UnusedBuiltInTool { .. }
                | FindingCause::UnusedSkill { .. }
        )
        .then(|| Self {
            detector: cause.detector(),
            source_format,
            agent: agent.slug().to_owned(),
            session_id: String::new(),
            cause,
        })
    }

    /// Returns the exact agent identity for trusted backend selector binding.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// Returns the exact session identity for trusted backend selector binding.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns true when the finding comes from the target-based resource assessment.
    pub fn is_advisory_resource(&self) -> bool {
        self.session_id.is_empty()
            && matches!(
                self.cause,
                FindingCause::UnusedMcpServer { .. }
                    | FindingCause::UnusedBuiltInTool { .. }
                    | FindingCause::UnusedSkill { .. }
            )
    }

    /// Returns the exact detector cause for trusted backend selector binding.
    pub fn cause(&self) -> &FindingCause {
        &self.cause
    }

    /// Returns the exact stable identity for grouping and later verification.
    pub fn canonical_identity(&self, scope: &str) -> String {
        let base = |kind: &str| {
            serde_json::json!({
                "detector": self.detector.key(),
                "sourceFormat": self.source_format,
                "agent": self.agent,
                "kind": kind,
            })
        };
        match &self.cause {
            FindingCause::SessionsOverDepth { limit_tokens, .. } => serde_json::json!({
                "base": base("depthPolicy"), "scope": scope, "limitTokens": limit_tokens,
            })
            .to_string(),
            FindingCause::ModelOverthinking {
                provider,
                api,
                model,
                reasoning,
                ..
            } => serde_json::json!({
                "base": base("route"), "scope": scope, "provider": provider,
                "api": api, "model": model, "reasoning": reasoning,
            })
            .to_string(),
            FindingCause::OverpoweredSubagents {
                parent_model,
                worker_model,
                ..
            } => serde_json::json!({
                "base": base("workerPolicy"), "scope": scope,
                "parentModel": parent_model, "workerModel": worker_model,
            })
            .to_string(),
            FindingCause::UnusedMcpServer { server, .. } => serde_json::json!({
                "base": base("resource"), "scope": scope, "resource": server,
            })
            .to_string(),
            FindingCause::UnusedBuiltInTool { tool, .. } => serde_json::json!({
                "base": base("resource"), "scope": scope, "resource": tool,
            })
            .to_string(),
            FindingCause::UnusedSkill { skill, .. } => serde_json::json!({
                "base": base("resource"), "scope": scope, "resource": skill,
            })
            .to_string(),
            FindingCause::SkillOpportunity { skill_name, cited_work_context, work_provenance, selected_window_limit, .. } => serde_json::json!({
                "base": base("skillOpportunity"), "scope": scope, "skill": skill_name,
                "episode": crate::checks::ignored_instructions::sha256_hex(
                    format!("{cited_work_context}\0{work_provenance}\0{selected_window_limit}").as_bytes()
                ),
            })
            .to_string(),
            FindingCause::OverExploring(evidence) => serde_json::json!({
                "base": base("investigation"), "scope": scope,
                "episode": evidence.episode_id, "reason": evidence.reason,
                "reads": evidence.reads,
            }).to_string(),
            FindingCause::ScopeCreep(evidence) => serde_json::json!({
                "base": base("scopeWork"), "scope": scope,
                "group": evidence.group_id, "approved_scope": evidence.scope_digest,
                "work": evidence.work,
            }).to_string(),
            FindingCause::OldModelUsage {
                provider,
                api,
                model,
                replacement,
                ..
            } => serde_json::json!({
                "base": base("route"), "scope": scope, "provider": provider,
                "api": api, "model": model, "replacement": replacement,
            })
            .to_string(),
            FindingCause::OveruseOfFastMode {
                provider,
                api,
                model,
                ..
            } => serde_json::json!({
                "base": base("delegatedWorker"), "scope": scope,
                "provider": provider, "api": api, "model": model,
            })
            .to_string(),
            FindingCause::CacheChurn {
                model,
                threshold_basis_points,
                ..
            } => serde_json::json!({
                "base": base("accountingRoute"), "scope": scope,
                "model": model, "thresholdBasisPoints": threshold_basis_points,
            })
            .to_string(),
            FindingCause::IgnoredInstructionConflict(evidence) => {
                let IgnoredInstructionConflictEvidence {
                    rule_id,
                    rule_heading,
                    source,
                    ..
                } = evidence.as_ref();
                let heading = rule_heading.trim();
                if heading.is_empty() {
                    serde_json::json!({
                        "base": base("instructionRule"),
                        "scope": scope,
                        "rule": rule_id,
                        "source": source,
                    })
                } else {
                    serde_json::json!({
                        "base": base("instructionSection"),
                        "scope": scope,
                        "section": heading,
                        "source": source,
                    })
                }
                .to_string()
            }
        }
    }

    /// Builds the only finding shape intended for display or IPC conversion.
    pub fn display(&self) -> Result<FindingDisplay, RemediationUnavailableReason> {
        let agent =
            display_agent(&self.agent).ok_or(RemediationUnavailableReason::DeferredAgent)?;
        Ok(FindingDisplay {
            detector: self.detector,
            agent,
            source_format: self.source_format,
            observation: match &self.cause {
                FindingCause::SkillOpportunity { .. } => {
                    "This work matches a skill you have installed.".to_owned()
                }
                _ => super::prompts::prompt_parts(&self.cause).0,
            },
            facts: display_facts(&self.cause),
            certainty: match &self.cause {
                FindingCause::IgnoredInstructionConflict(evidence) => Some(evidence.certainty),
                _ => None,
            },
            instruction_provenance: match &self.cause {
                FindingCause::IgnoredInstructionConflict(evidence) => Some(evidence.provenance),
                _ => None,
            },
        })
    }
}

/// A bounded finding shape that excludes backend session and call identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingDisplay {
    pub detector: DetectorId,
    pub agent: AgentKind,
    pub source_format: SourceFormat,
    pub observation: String,
    pub facts: DisplayFacts,
    pub certainty: Option<FindingCertainty>,
    pub instruction_provenance: Option<InstructionProvenance>,
}

/// Sanitized labels and omission count for display and IPC conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayFacts {
    pub labels: Vec<String>,
    pub omitted: u64,
}

/// States why one detector cannot return a current per-session result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingUnavailableReason {
    CapabilityMissing,
    IncompleteEvidence,
    EvidenceContractIncomplete,
    SignalMissing,
}

/// One detector result before report aggregation.
#[derive(Debug, Clone, PartialEq)]
pub enum FindingAssessment {
    Findings(Vec<Finding>),
    Clean,
    NotApplicable,
    Unavailable(FindingUnavailableReason),
}

/// Assesses one selected detector and builds details only for that detector.
pub fn assess_detector(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> FindingAssessment {
    assess_detector_with_source_evidence(detector, evidence, catalogs, None)
}

/// Assesses one selected detector with optional report-time source attribution.
pub fn assess_detector_with_source_evidence(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> FindingAssessment {
    if detector == DetectorId::IgnoredInstructions {
        return FindingAssessment::Unavailable(FindingUnavailableReason::CapabilityMissing);
    }
    if !crate::checks::in_denominator(detector, evidence)
        || (detector == DetectorId::UnusedBuiltInTools
            && complete_assistant_turns(evidence) == Some(0))
    {
        return FindingAssessment::NotApplicable;
    }
    if !eligible(detector, evidence)
        && !crate::checks::source_assessable(detector, evidence, source_evidence)
    {
        return FindingAssessment::Unavailable(FindingUnavailableReason::CapabilityMissing);
    }
    let observation =
        crate::checks::evaluate_with_source_evidence(detector, evidence, catalogs, source_evidence)
            .observation;
    if observation == crate::checks::Observation::Finding {
        let causes = crate::checks::finding_causes_with_source_evidence(
            detector,
            evidence,
            catalogs,
            source_evidence,
        );
        if causes.is_empty() {
            return FindingAssessment::Unavailable(
                FindingUnavailableReason::EvidenceContractIncomplete,
            );
        }
        return FindingAssessment::Findings(
            causes
                .into_iter()
                .map(|cause| finding(evidence, cause))
                .collect(),
        );
    }
    match observation {
        crate::checks::Observation::Finding => {
            FindingAssessment::Unavailable(FindingUnavailableReason::EvidenceContractIncomplete)
        }
        crate::checks::Observation::NoFinding if clean_facts_complete(detector, evidence) => {
            FindingAssessment::Clean
        }
        crate::checks::Observation::NoFinding => {
            FindingAssessment::Unavailable(FindingUnavailableReason::IncompleteEvidence)
        }
        crate::checks::Observation::ContractIncomplete => {
            FindingAssessment::Unavailable(FindingUnavailableReason::EvidenceContractIncomplete)
        }
        crate::checks::Observation::SignalMissing => {
            FindingAssessment::Unavailable(FindingUnavailableReason::SignalMissing)
        }
    }
}

/// Returns true when complete scoped resource evidence has no finding.
///
/// This does not make the session report clean. M/B/K lack a complete
/// historical resource inventory, but a later complete observed scope can
/// verify one already-scoped remediation target.
pub fn scoped_resource_no_finding(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> bool {
    if !matches!(
        detector,
        DetectorId::UnusedMcpServers | DetectorId::UnusedBuiltInTools | DetectorId::UnusedSkills
    ) || !crate::checks::in_denominator(detector, evidence)
        || (detector == DetectorId::UnusedBuiltInTools
            && complete_assistant_turns(evidence) == Some(0))
        || {
            let source_assessable =
                crate::checks::source_assessable(detector, evidence, source_evidence);
            !source_assessable
                && (!eligible(detector, evidence) || !clean_facts_complete(detector, evidence))
        }
    {
        return false;
    }
    crate::checks::evaluate_with_source_evidence(detector, evidence, catalogs, source_evidence)
        .observation
        == crate::checks::Observation::NoFinding
}

fn complete_assistant_turns(evidence: &SessionEvidence) -> Option<u64> {
    match &evidence.eligibility {
        crate::analysis::EvidenceValue::Complete(value) => Some(value.assistant_turns),
        _ => None,
    }
}

fn finding(evidence: &SessionEvidence, cause: FindingCause) -> Finding {
    let detector = cause.detector();
    Finding {
        detector,
        source_format: evidence.capabilities.source_format,
        agent: evidence.identity.agent.clone(),
        session_id: evidence.identity.session_id.clone(),
        cause,
    }
}

impl Finding {
    /// Builds a session-scoped skill opportunity from current typed evidence.
    pub fn skill_opportunity(
        evidence: &SessionEvidence,
        result: &SkillOpportunityFinding,
    ) -> Option<Self> {
        let comparison = &result.comparison;
        if !crate::analysis::smart_check_source_supported(
            &evidence.identity.agent,
            evidence.capabilities.source_format,
        ) || evidence.identity.session_id.is_empty()
            || comparison.id.is_empty()
            || comparison.episode_id.is_empty()
            || comparison.skill.identity.trim().is_empty()
            || comparison.skill.name.trim().is_empty()
            || comparison.skill.description.trim().is_empty()
            || result.revisions != crate::checks::skill_opportunities::SKILL_OPPORTUNITIES_REVISIONS
            || comparison.work.is_empty()
        {
            return None;
        }
        let first = &comparison.work[0].reference;
        if !reference_is_complete(first)
            || first.source_key_digest.is_empty()
            || first.thread_digest.is_empty()
        {
            return None;
        }
        if comparison.work.iter().any(|work| {
            !reference_is_complete(&work.reference)
                || work.reference.source_key_digest != first.source_key_digest
                || work.reference.thread_digest != first.thread_digest
                || work.text.trim().is_empty()
        }) {
            return None;
        }
        let work_context = comparison
            .work
            .iter()
            .map(|work| work.text.trim())
            .collect::<Vec<_>>()
            .join("\n");
        let episode_material = serde_json::to_vec(
            &comparison
                .work
                .iter()
                .map(|work| (&work.reference, work.text.trim()))
                .collect::<Vec<_>>(),
        )
        .ok()?;
        let episode_digest = crate::checks::ignored_instructions::sha256_hex(&episode_material);
        let cause = FindingCause::SkillOpportunity {
            evidence: Some(Box::new(result.clone())),
            skill_name: comparison.skill.name.clone(),
            skill_description: comparison.skill.description.clone(),
            cited_work_context: work_context,
            work_provenance: format!("Selected session work {episode_digest}"),
            selected_window_limit: result.absence_limit.clone(),
        };
        Some(finding(evidence, cause))
    }

    pub fn over_exploring(
        evidence: &SessionEvidence,
        decision: &crate::checks::over_exploring::Decision,
    ) -> Option<Self> {
        use crate::checks::over_exploring::SemanticOutcome;
        if !crate::analysis::smart_check_source_supported(
            &evidence.identity.agent,
            evidence.capabilities.source_format,
        ) || decision.reads.is_empty()
            || decision.task_evidence.is_empty()
            || decision.semantic_revision.is_empty()
            || decision.model.is_empty()
            || decision.judgments.sufficiency != SemanticOutcome::Supported
            || decision.reads.iter().any(|read| {
                read.request_id.is_empty()
                    || read.result_id.is_empty()
                    || read.output_digest.is_empty()
            })
        {
            return None;
        }
        Some(finding(
            evidence,
            FindingCause::OverExploring(Box::new(decision.clone())),
        ))
    }

    /// Convert a finding from a validated, current scope publication.
    pub fn scope_creep(
        evidence: &SessionEvidence,
        result: &crate::checks::scope_creep::ScopeCreepFinding,
    ) -> Option<Self> {
        if !crate::analysis::smart_check_source_supported(
            &evidence.identity.agent,
            evidence.capabilities.source_format,
        ) || result.id.is_empty()
            || result.group_id.is_empty()
            || result.scope_digest.is_empty()
            || result.model.is_empty()
            || result.revisions != crate::checks::scope_creep::REVISIONS
            || result.work.is_empty()
            || !result.decision_probability.is_finite()
            || !(crate::checks::scope_creep::DECISION_THRESHOLD..=1.0)
                .contains(&result.decision_probability)
            || result.work.iter().any(|work| {
                !work.reference.stable || work.reference.id.is_empty() || work.digest.is_empty()
            })
            || result
                .task_scope
                .iter()
                .any(|source| source.source_id.is_empty() || source.part_id.is_empty())
        {
            return None;
        }
        Some(finding(
            evidence,
            FindingCause::ScopeCreep(Box::new(result.clone())),
        ))
    }

    /// Builds one session-scoped instruction conflict from a validated result.
    pub fn ignored_instruction(
        evidence: &SessionEvidence,
        assessment_revision: &str,
        assessment_finding: &AssessmentFinding,
    ) -> Option<Self> {
        let reference = &assessment_finding.reference;
        if !crate::checks::ignored_instructions::source_supported(
            evidence.capabilities.source_format,
        ) || assessment_revision.is_empty()
            || assessment_finding.id.is_empty()
            || reference.action_id.is_empty()
            || reference.rule_id.is_empty()
            || reference.instruction_id.is_empty()
            || reference.instruction_digest.is_empty()
            || reference.start_line == 0
            || reference.end_line < reference.start_line
        {
            return None;
        }
        let cause = FindingCause::IgnoredInstructionConflict(Box::new(
            IgnoredInstructionConflictEvidence {
                decision: assessment_finding.decision_record().cloned(),
                assessment_revision: assessment_revision.to_owned(),
                assessment_finding_id: assessment_finding.id.clone(),
                instruction_id: reference.instruction_id.clone(),
                instruction_digest: reference.instruction_digest.clone(),
                instruction_excerpt: assessment_finding.instruction_excerpt.clone(),
                instruction_excerpt_truncated: assessment_finding.instruction_excerpt_truncated,
                rule_id: reference.rule_id.clone(),
                rule_heading: reference.rule_heading.clone(),
                start_line: reference.start_line,
                end_line: reference.end_line,
                source: reference.source.clone(),
                provenance: reference.provenance,
                instruction_scope: reference.scope,
                action_id: reference.action_id.clone(),
                action_digest: reference.action_digest.clone(),
                action_excerpt: assessment_finding.action_excerpt.clone(),
                action_excerpt_truncated: assessment_finding.action_excerpt_truncated,
                action_timestamp_ms: reference.action_timestamp_ms,
                nearby_context_ids: assessment_finding.nearby_context_ids.clone(),
                counterevidence_ids: assessment_finding.counterevidence_ids.clone(),
                certainty: assessment_finding.certainty,
                limitations: Box::new(assessment_finding.limitations.clone()),
            },
        ));
        Some(finding(evidence, cause))
    }
}

fn reference_is_complete(reference: &ContentEventReference) -> bool {
    reference.stable
        && !reference.id.is_empty()
        && !reference.source_key_digest.is_empty()
        && !reference.thread_digest.is_empty()
        && reference
            .native_record_id
            .as_deref()
            .is_none_or(|id| !id.trim().is_empty())
}

#[cfg(test)]
pub(super) fn finding_for_test(evidence: &SessionEvidence, cause: FindingCause) -> Finding {
    finding(evidence, cause)
}

pub(super) fn display_facts(cause: &FindingCause) -> DisplayFacts {
    let labels: BTreeSet<String> = cause
        .display_labels()
        .into_iter()
        .filter_map(sanitize_display_value)
        .collect();
    let total = labels.len();
    DisplayFacts {
        labels: labels.into_iter().take(MAX_PROMPT_IDENTITIES).collect(),
        omitted: total.saturating_sub(MAX_PROMPT_IDENTITIES) as u64,
    }
}

fn display_agent(agent: &str) -> Option<AgentKind> {
    let normalized = agent.trim().to_ascii_lowercase();
    if normalized == "claude" {
        Some(AgentKind::Claude)
    } else {
        AgentKind::from_slug(&normalized)
    }
}

/// Returns a bounded display value with private values redacted.
pub fn sanitize_display_value(value: &str) -> Option<String> {
    Some(match sanitize_value(value)? {
        SanitizedValue::Safe(value) => value.value,
        SanitizedValue::Private => "[private value]".to_owned(),
    })
}

pub(super) struct SafeValue {
    pub(super) value: String,
    pub(super) truncated: bool,
}

pub(super) enum SanitizedValue {
    Safe(SafeValue),
    Private,
}

pub(super) fn sanitize_value(value: &str) -> Option<SanitizedValue> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let compact: String = lower
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if looks_private(&lower, trimmed)
        || [
            "password",
            "secret",
            "token",
            "api_key",
            "apikey",
            "authorization",
        ]
        .iter()
        .any(|key| compact.contains(&format!("{key}=")) || compact.contains(&format!("{key}:")))
        || lower.starts_with("bearer ")
        || lower.starts_with("sk-")
    {
        return Some(SanitizedValue::Private);
    }
    let clean: String = trimmed
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let clean = clean.trim();
    let truncated = clean.len() > MAX_DISPLAY_LABEL_BYTES;
    let clean = truncate_utf8(clean, MAX_DISPLAY_LABEL_BYTES);
    (!clean.is_empty()).then_some(SanitizedValue::Safe(SafeValue {
        value: clean,
        truncated,
    }))
}

fn looks_private(lower: &str, value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with(".\\")
        || value.starts_with("..\\")
        || value.starts_with("\\\\")
        || value.as_bytes().get(1) == Some(&b':')
        || lower.starts_with("file:")
}

fn truncate_utf8(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests;
