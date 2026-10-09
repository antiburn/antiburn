use super::policy::ReportCatalogs;
use crate::analysis::{EvidenceValue, SessionEvidence};
use crate::checks::DetectorId;
use crate::insights::SessionTokenBurnEvidence;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observation {
    Finding,
    NoFinding,
    ContractIncomplete,
    SignalMissing,
}

/// One allocation-free detector result used by aggregate report evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetectorEvaluation {
    pub observation: Observation,
}

impl PartialEq<Observation> for DetectorEvaluation {
    fn eq(&self, other: &Observation) -> bool {
        self.observation == *other
    }
}

impl PartialEq<DetectorEvaluation> for Observation {
    fn eq(&self, other: &DetectorEvaluation) -> bool {
        *self == other.observation
    }
}

/// Runs one detector rule over one eligible session.
pub(crate) fn evaluate(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> DetectorEvaluation {
    DetectorEvaluation {
        observation: match detector {
            DetectorId::SessionsOverDepth => super::session_overdepth::evaluate(evidence, catalogs),
            DetectorId::ModelOverthinking => {
                super::model_overthinking::evaluate(evidence, catalogs)
            }
            DetectorId::OverpoweredSubagents => {
                super::overpowered_subagents::evaluate(evidence, catalogs)
            }
            DetectorId::UnusedMcpServers => super::unused_mcp_servers::evaluate(evidence),
            DetectorId::UnusedBuiltInTools => super::unused_built_in_tools::evaluate(evidence),
            DetectorId::UnusedSkills => super::unused_skills::evaluate(evidence),
            DetectorId::OldModelUsage => super::old_model_usage::evaluate(evidence, catalogs),
            DetectorId::OveruseOfFastMode => super::fast_mode_overuse::evaluate(evidence, catalogs),
            DetectorId::CacheChurn => super::cache_churn::evaluate(evidence, catalogs),
            DetectorId::IgnoredInstructions => Observation::NoFinding,
            DetectorId::SkillOpportunities => Observation::NoFinding,
            DetectorId::OverExploring => Observation::NoFinding,
            DetectorId::ScopeCreep => Observation::NoFinding,
        },
    }
}

pub(crate) fn source_assessable(
    detector: DetectorId,
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> bool {
    match detector {
        DetectorId::UnusedBuiltInTools => {
            super::unused_built_in_tools::source_assessable(evidence, source_evidence)
        }
        DetectorId::UnusedMcpServers => {
            super::unused_mcp_servers::source_assessable(evidence, source_evidence)
        }
        DetectorId::UnusedSkills => {
            super::unused_skills::source_assessable(evidence, source_evidence)
        }
        _ => false,
    }
}

pub(crate) fn evaluate_with_source_evidence(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> DetectorEvaluation {
    DetectorEvaluation {
        observation: match detector {
            DetectorId::UnusedBuiltInTools => {
                super::unused_built_in_tools::evaluate_with_source_evidence(
                    evidence,
                    source_evidence,
                )
            }
            DetectorId::UnusedMcpServers => {
                super::unused_mcp_servers::evaluate_with_source_evidence(evidence, source_evidence)
            }
            DetectorId::UnusedSkills => {
                super::unused_skills::evaluate_with_source_evidence(evidence, source_evidence)
            }
            _ => evaluate(detector, evidence, catalogs).observation,
        },
    }
}

/// Returns whether this session belongs in the detector's eligible denominator.
pub(crate) fn in_denominator(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    match detector {
        DetectorId::IgnoredInstructions
        | DetectorId::SkillOpportunities
        | DetectorId::OverExploring
        | DetectorId::ScopeCreep => false,
        DetectorId::UnusedMcpServers | DetectorId::UnusedSkills => complete(&evidence.eligibility)
            .is_none_or(|eligibility| eligibility.assistant_turns > 0),
        _ => true,
    }
}

/// Returns the observed value from complete or partial evidence.
pub(crate) fn observed<T>(value: &EvidenceValue<T>) -> Option<&T> {
    match value {
        EvidenceValue::Unsupported => None,
        EvidenceValue::Partial { observed, .. } => Some(observed),
        EvidenceValue::Complete(value) => Some(value),
    }
}

/// Returns the value only when the evidence is complete.
pub(crate) fn complete<T>(value: &EvidenceValue<T>) -> Option<&T> {
    match value {
        EvidenceValue::Complete(value) => Some(value),
        _ => None,
    }
}
