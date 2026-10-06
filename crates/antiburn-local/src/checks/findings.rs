//! Detector outcomes, aggregation, and finding evidence dispatch.
use super::assessment::Observation;
use super::policy::ReportCatalogs;
use crate::analysis::SessionEvidence;
use crate::checks::DetectorId;
use crate::insights::SessionTokenBurnEvidence;
use crate::insights::{DetectorCounts, MAX_EXAMPLES_PER_DETECTOR, SessionExample};
use crate::remediation::FindingCause;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectorStatus {
    Findings(DetectorFindings),
    Clean,
    NotAssessed(NotAssessedReason),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAssessedReason {
    NoSessionsInWindow,
    CapabilityMissing,
    IncompleteEvidence,
    EvidenceContractIncomplete,
    SignalMissing,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorFindings {
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DetectorFold {
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
    pub contract_incomplete: u64,
    pub signal_missing: u64,
    pub partial_sessions: u64,
}
impl DetectorFold {
    pub(crate) fn observe(&mut self, observation: Observation, evidence: &SessionEvidence) {
        self.partial_sessions += u64::from(matches!(
            evidence.coverage,
            crate::analysis::EvidenceCoverage::Partial(_)
        ));
        match observation {
            Observation::Finding => {
                self.finding_sessions += 1;
                if self.examples.len() < MAX_EXAMPLES_PER_DETECTOR {
                    self.examples.push(SessionExample {
                        agent: evidence.identity.agent.clone(),
                        session_id: evidence.identity.session_id.clone(),
                    });
                }
            }
            Observation::NoFinding => {}
            Observation::ContractIncomplete => self.contract_incomplete += 1,
            Observation::SignalMissing => self.signal_missing += 1,
        }
    }
}
pub(crate) fn finding_causes(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> Vec<FindingCause> {
    let causes = match detector {
        DetectorId::SessionsOverDepth => {
            super::session_overdepth::finding_causes(evidence, catalogs)
        }
        DetectorId::ModelOverthinking => {
            super::model_overthinking::finding_causes(evidence, catalogs)
        }
        DetectorId::OverpoweredSubagents => {
            super::overpowered_subagents::finding_causes(evidence, catalogs)
        }
        DetectorId::UnusedMcpServers => super::unused_mcp_servers::finding_causes(evidence),
        DetectorId::UnusedBuiltInTools => super::unused_built_in_tools::finding_causes(evidence),
        DetectorId::UnusedSkills => super::unused_skills::finding_causes(evidence),
        DetectorId::OldModelUsage => super::old_model_usage::finding_causes(evidence, catalogs),
        DetectorId::OveruseOfFastMode => {
            super::fast_mode_overuse::finding_causes(evidence, catalogs)
        }
        DetectorId::CacheChurn => super::cache_churn::finding_causes(evidence, catalogs),
        DetectorId::IgnoredInstructions => Vec::new(),
    };
    debug_assert!(causes.iter().all(|cause| cause.detector() == detector));
    causes
}
pub(crate) fn finding_causes_with_source_evidence(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
    source: Option<&SessionTokenBurnEvidence>,
) -> Vec<FindingCause> {
    let causes = match detector {
        DetectorId::UnusedBuiltInTools => {
            super::unused_built_in_tools::finding_causes_with_source_evidence(evidence, source)
        }
        DetectorId::UnusedMcpServers => {
            super::unused_mcp_servers::finding_causes_with_source_evidence(evidence, source)
        }
        DetectorId::UnusedSkills => {
            super::unused_skills::finding_causes_with_source_evidence(evidence, source)
        }
        DetectorId::IgnoredInstructions => Vec::new(),
        _ => finding_causes(detector, evidence, catalogs),
    };
    debug_assert!(causes.iter().all(|cause| cause.detector() == detector));
    causes
}
pub(crate) fn status(
    counts: DetectorCounts,
    fold: DetectorFold,
    assessed_sessions: u64,
) -> DetectorStatus {
    if fold.finding_sessions > 0 {
        return DetectorStatus::Findings(DetectorFindings {
            finding_sessions: fold.finding_sessions,
            examples: fold.examples,
        });
    }
    if assessed_sessions == 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::NoSessionsInWindow);
    }
    if counts.eligible == 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing);
    }
    if fold.contract_incomplete > 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete);
    }
    if fold.signal_missing > 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::SignalMissing);
    }
    if counts.clean == counts.eligible && fold.partial_sessions == 0 {
        return DetectorStatus::Clean;
    }
    DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(eligible: u64, clean: u64) -> DetectorCounts {
        DetectorCounts {
            eligible,
            assessed: clean,
            finding: 0,
            clean,
            unavailable: eligible - clean,
            not_applicable: 0,
        }
    }

    #[test]
    fn findings_win_over_incomplete_coverage() {
        let fold = DetectorFold {
            finding_sessions: 1,
            partial_sessions: 1,
            ..DetectorFold::default()
        };
        assert!(matches!(
            status(counts(2, 0), fold, 2),
            DetectorStatus::Findings(DetectorFindings {
                finding_sessions: 1,
                ..
            })
        ));
    }

    #[test]
    fn incomplete_absence_cannot_be_clean() {
        assert_eq!(
            status(counts(2, 1), DetectorFold::default(), 2),
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
        assert_eq!(
            status(counts(2, 2), DetectorFold::default(), 2),
            DetectorStatus::Clean
        );
    }
}
