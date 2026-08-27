use crate::analysis::{EvidenceCoverage, SessionEvidence};

use super::DetectorId;
use super::detectors::{self, NotAssessedReason, Observation, ReportCatalogs};
use super::report::{GroupState, requirements};

/// Identifies one session-level hygiene badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BadgeId {
    ReasoningOverkill,
    ExcessCacheRehydration,
    BloatedInitialContext,
}

impl BadgeId {
    pub const ALL: [Self; 3] = [
        Self::ReasoningOverkill,
        Self::ExcessCacheRehydration,
        Self::BloatedInitialContext,
    ];

    const fn detector(self) -> DetectorId {
        match self {
            Self::ReasoningOverkill => DetectorId::ModelOverthinking,
            Self::ExcessCacheRehydration => DetectorId::CacheChurn,
            Self::BloatedInitialContext => DetectorId::SessionsOverDepth,
        }
    }
}

/// States one session-level hygiene result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeStatus {
    Finding,
    Clean,
    NotAssessed(NotAssessedReason),
}

/// Holds one badge identifier and its honest status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBadge {
    pub id: BadgeId,
    pub status: BadgeStatus,
}

/// Reduces one session's stored evidence into the three v1 badges.
pub fn session_badges(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> [SessionBadge; 3] {
    BadgeId::ALL.map(|id| SessionBadge {
        id,
        status: badge_status(id.detector(), evidence, catalogs),
    })
}

fn badge_status(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> BadgeStatus {
    let required = requirements(detector);
    let capabilities_hold = required.capabilities.iter().all(|clause| {
        clause
            .iter()
            .any(|flag| flag.is_set(&evidence.capabilities))
    });
    let groups_supported = required
        .groups
        .iter()
        .all(|group| group.state(evidence) != GroupState::Unsupported);
    if !capabilities_hold || !groups_supported {
        return BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing);
    }

    match detectors::evaluate(detector, evidence, catalogs) {
        Observation::Finding => BadgeStatus::Finding,
        Observation::ContractIncomplete => {
            BadgeStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete)
        }
        Observation::NoFinding => {
            let groups_complete = required
                .groups
                .iter()
                .all(|group| group.state(evidence) == GroupState::Complete);
            if groups_complete && evidence.coverage == EvidenceCoverage::Complete {
                BadgeStatus::Clean
            } else {
                BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::analysis::{
        ANALYZER_REVISION, ContextEvidence, CoverageReason, EVIDENCE_SCHEMA_REVISION,
        EvidenceValue, ModelTransition, PARSER_REVISION, SessionEvidence, TurnCounts,
    };
    use crate::insights::detectors::test_support::claude_evidence;
    use crate::insights::{
        CoverageCounts, DetectorStatus, EfficiencyReportAccumulator, ReportContext, ReportWindow,
    };

    use super::*;

    fn make_partial<T>(value: EvidenceValue<T>) -> EvidenceValue<T> {
        let EvidenceValue::Complete(observed) = value else {
            panic!("the synthetic evidence must be complete");
        };
        EvidenceValue::Partial {
            observed,
            reason: CoverageReason::MalformedRecord,
        }
    }

    fn finding_evidence(id: BadgeId, partial: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("synthetic-badge");
        match id {
            BadgeId::ReasoningOverkill => {
                let EvidenceValue::Complete(models) = &mut evidence.models else {
                    unreachable!()
                };
                models.effort_tiers.insert(
                    "max".to_owned(),
                    TurnCounts {
                        main_loop: 1,
                        delegated: 0,
                    },
                );
                if partial {
                    evidence.models = make_partial(evidence.models);
                }
            }
            BadgeId::ExcessCacheRehydration => {
                let EvidenceValue::Complete(cache) = &mut evidence.cache else {
                    unreachable!()
                };
                cache.cache_creation_tokens = 1;
                cache.model_transitions.push(ModelTransition {
                    ts_ms: 1,
                    from_model: "model-a".to_owned(),
                    to_model: "model-b".to_owned(),
                });
                if partial {
                    evidence.cache = make_partial(evidence.cache);
                }
            }
            BadgeId::BloatedInitialContext => {
                evidence.context = EvidenceValue::Complete(ContextEvidence {
                    max_request_context_tokens: ReportCatalogs::default().depth_cap_tokens + 1,
                    top_depth_examples: Vec::new(),
                });
                if partial {
                    evidence.context = make_partial(evidence.context);
                }
            }
        }
        if partial {
            evidence.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);
        }
        evidence
    }

    fn badge(evidence: &SessionEvidence, id: BadgeId) -> SessionBadge {
        session_badges(evidence, &ReportCatalogs::default())
            .into_iter()
            .find(|badge| badge.id == id)
            .unwrap()
    }

    #[test]
    fn each_badge_accepts_a_finding_from_partial_evidence() {
        for id in BadgeId::ALL {
            assert_eq!(
                badge(&finding_evidence(id, true), id).status,
                BadgeStatus::Finding
            );
        }
    }

    #[test]
    fn partial_coverage_without_a_signal_never_reads_clean() {
        let mut evidence = claude_evidence("synthetic-partial");
        evidence.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);

        for result in session_badges(&evidence, &ReportCatalogs::default()) {
            assert_eq!(
                result.status,
                BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
            );
        }
    }

    #[test]
    fn a_missing_capability_is_not_assessed() {
        let mut evidence = claude_evidence("synthetic-capability");
        evidence.capabilities.model_identity = false;

        for id in BadgeId::ALL {
            assert_eq!(
                badge(&evidence, id).status,
                BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing)
            );
        }
    }

    #[test]
    fn complete_evidence_without_a_signal_reads_clean() {
        let evidence = claude_evidence("synthetic-clean");

        for result in session_badges(&evidence, &ReportCatalogs::default()) {
            assert_eq!(result.status, BadgeStatus::Clean);
        }
    }

    fn report_status(evidence: SessionEvidence, detector: DetectorId) -> DetectorStatus {
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence);
        accumulator
            .finish(ReportContext {
                environment_key: "native".to_owned(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: 1,
                },
                computed_at_epoch: 1,
                parser_revision: PARSER_REVISION,
                analyzer_revision: ANALYZER_REVISION,
                evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                coverage: CoverageCounts::default(),
            })
            .detector_statuses[detector.index()]
        .clone()
    }

    #[test]
    fn badges_and_report_folds_agree_when_session_coverage_is_complete() {
        let catalogs = ReportCatalogs::default();
        let cohort = [
            claude_evidence("synthetic-clean"),
            finding_evidence(BadgeId::ReasoningOverkill, false),
            finding_evidence(BadgeId::ExcessCacheRehydration, false),
            finding_evidence(BadgeId::BloatedInitialContext, false),
        ];

        for evidence in cohort {
            let statuses: BTreeMap<BadgeId, BadgeStatus> = session_badges(&evidence, &catalogs)
                .map(|badge| (badge.id, badge.status))
                .into_iter()
                .collect();
            for id in BadgeId::ALL {
                let report = report_status(evidence.clone(), id.detector());
                let expected = match report {
                    DetectorStatus::Findings(_) => BadgeStatus::Finding,
                    DetectorStatus::Clean => BadgeStatus::Clean,
                    DetectorStatus::NotAssessed(reason) => BadgeStatus::NotAssessed(reason),
                };
                assert_eq!(statuses[&id], expected, "{id:?}");
            }
        }
    }

    /// The badge is a session-scope integrity signal. The report is
    /// detector-scope. A truncated group that no detector reads keeps the
    /// report Clean for that detector. The badge for the same session stays
    /// NotAssessed. This asymmetry is intentional. Issue #229 tracks the
    /// report-wide coverage policy.
    #[test]
    fn session_wide_partial_coverage_diverges_from_report_by_design() {
        let mut evidence = claude_evidence("synthetic-divergent");
        evidence.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);

        for badge in session_badges(&evidence, &ReportCatalogs::default()) {
            assert_eq!(
                badge.status,
                BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
                "{:?}",
                badge.id
            );
        }
        for id in BadgeId::ALL {
            assert_eq!(
                report_status(evidence.clone(), id.detector()),
                DetectorStatus::Clean,
                "{id:?}"
            );
        }
    }
}
