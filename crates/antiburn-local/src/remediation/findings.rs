use std::collections::BTreeSet;

use crate::analysis::{SessionEvidence, SourceFormat};
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    },
    UnusedBuiltInTool {
        tool: String,
        tokens: BuiltInToolTokens,
    },
    UnusedSkill {
        skill: String,
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
            Self::OldModelUsage { .. } => DetectorId::OldModelUsage,
            Self::OveruseOfFastMode { .. } => DetectorId::OveruseOfFastMode,
            Self::CacheChurn { .. } => DetectorId::CacheChurn,
        }
    }

    fn display_labels(&self) -> Vec<&str> {
        match self {
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
            Self::UnusedSkill { skill } => vec![skill],
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
        }
    }
}

/// One current per-target finding with exact private selectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub detector: DetectorId,
    pub source_format: SourceFormat,
    agent: String,
    session_id: String,
    cause: FindingCause,
}

impl Finding {
    /// Returns the exact agent identity for trusted backend selector binding.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// Returns the exact session identity for trusted backend selector binding.
    pub fn session_id(&self) -> &str {
        &self.session_id
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
            FindingCause::UnusedSkill { skill } => serde_json::json!({
                "base": base("resource"), "scope": scope, "resource": skill,
            })
            .to_string(),
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
            observation: super::prompts::prompt_parts(&self.cause).0,
            facts: display_facts(&self.cause),
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    if !crate::insights::detectors::in_denominator(detector, evidence)
        || (detector == DetectorId::UnusedBuiltInTools
            && complete_assistant_turns(evidence) == Some(0))
    {
        return FindingAssessment::NotApplicable;
    }
    if !eligible(detector, evidence)
        && !crate::insights::detectors::built_in_source_assessable(
            detector,
            evidence,
            source_evidence,
        )
    {
        return FindingAssessment::Unavailable(FindingUnavailableReason::CapabilityMissing);
    }
    let observation = crate::insights::detectors::evaluate_with_source_evidence(
        detector,
        evidence,
        catalogs,
        source_evidence,
    )
    .observation;
    if observation == crate::insights::detectors::Observation::Finding {
        let causes = crate::insights::detectors::finding_causes_with_source_evidence(
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
        crate::insights::detectors::Observation::Finding => {
            FindingAssessment::Unavailable(FindingUnavailableReason::EvidenceContractIncomplete)
        }
        crate::insights::detectors::Observation::NoFinding
            if clean_facts_complete(detector, evidence) =>
        {
            FindingAssessment::Clean
        }
        crate::insights::detectors::Observation::NoFinding => {
            FindingAssessment::Unavailable(FindingUnavailableReason::IncompleteEvidence)
        }
        crate::insights::detectors::Observation::ContractIncomplete => {
            FindingAssessment::Unavailable(FindingUnavailableReason::EvidenceContractIncomplete)
        }
        crate::insights::detectors::Observation::SignalMissing => {
            FindingAssessment::Unavailable(FindingUnavailableReason::SignalMissing)
        }
    }
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
