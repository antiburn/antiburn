//! Nine detector rule sets over assessed-cohort session evidence.
//!
//! Each detector produces exactly one status per report: findings, clean,
//! or not assessed with a structured reason. Clean requires that every
//! eligible session carries complete required evidence and shows no
//! finding. Incomplete absence of a signal never produces clean.
//! Thresholds and catalogs are report-time policy inputs. Evidence stays
//! rule-neutral (Locked Decision 2).

mod cache_churn;
mod model_overthinking;
mod old_model_usage;
mod overpowered_subagents;
mod overuse_of_fast_mode;
mod sessions_over_depth;
mod unused_built_in_tools;
mod unused_mcp_servers;
mod unused_skills;

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::{EvidenceValue, SessionEvidence};

use super::report::{DetectorCounts, MAX_EXAMPLES_PER_DETECTOR, SessionExample};
use super::status::DetectorId;

/// One report-level status for one detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectorStatus {
    Findings(DetectorFindings),
    Clean,
    NotAssessed(NotAssessedReason),
}

/// States why a detector could not assess its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAssessedReason {
    /// The assessed cohort holds no session for the window.
    NoSessionsInWindow,
    /// Sessions exist, but none carries the required capabilities.
    CapabilityMissing,
    /// Eligible sessions exist, but incomplete evidence coverage
    /// prevents a clean conclusion, and no finding was observed.
    IncompleteEvidence,
    /// The evidence schema does not yet carry the payload the rule
    /// needs, so neither a finding nor clean is expressible.
    EvidenceContractIncomplete,
}

/// Bounded finding summary for one detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorFindings {
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
}

/// One per-session rule result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observation {
    /// The rule observed at least one finding in this session.
    Finding,
    /// The rule observed no finding. Only complete required evidence
    /// lets the report turn this into a clean claim.
    NoFinding,
    /// The evidence contract cannot express the fact the rule needs.
    ContractIncomplete,
}

/// Bounded per-detector fold state across the assessed cohort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DetectorFold {
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
    pub contract_incomplete: u64,
}

impl DetectorFold {
    pub(crate) fn observe(&mut self, observation: Observation, evidence: &SessionEvidence) {
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
        }
    }
}

/// One curated replacement entry for a deprecated model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReplacement {
    pub replacement: String,
    pub available_since_ts_ms: i64,
}

/// Report-time policy inputs. Catalogs change without reparsing
/// transcripts and without touching persisted evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportCatalogs {
    pub revision: i64,
    /// A request whose observed context depth exceeds this cap is a
    /// Sessions Over Depth finding.
    pub depth_cap_tokens: u64,
    /// Effort tier labels above the recommended cap.
    pub effort_tiers_above_cap: BTreeSet<String>,
    /// Curated deprecated models keyed by normalized model name.
    pub model_replacements: BTreeMap<String, ModelReplacement>,
    /// Delegated fast-tier turns at or above this count are a finding.
    /// Zero observed delegated turns never fire, whatever the value.
    pub fast_mode_delegated_turns_threshold: u64,
    /// An idle gap at or above this duration counts as cache expiry.
    pub cache_idle_expiry_ms: i64,
}

impl Default for ReportCatalogs {
    fn default() -> Self {
        Self {
            revision: 1,
            depth_cap_tokens: 160_000,
            effort_tiers_above_cap: ["max", "ultrathink", "xhigh"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            model_replacements: BTreeMap::new(),
            fast_mode_delegated_turns_threshold: 1,
            cache_idle_expiry_ms: 300_000,
        }
    }
}

/// Runs one detector rule over one eligible session.
pub(crate) fn evaluate(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> Observation {
    match detector {
        DetectorId::SessionsOverDepth => sessions_over_depth::evaluate(evidence, catalogs),
        DetectorId::ModelOverthinking => model_overthinking::evaluate(evidence, catalogs),
        DetectorId::OverpoweredSubagents => overpowered_subagents::evaluate(evidence),
        DetectorId::UnusedMcpServers => unused_mcp_servers::evaluate(evidence),
        DetectorId::UnusedBuiltInTools => unused_built_in_tools::evaluate(evidence),
        DetectorId::UnusedSkills => unused_skills::evaluate(evidence),
        DetectorId::OldModelUsage => old_model_usage::evaluate(evidence, catalogs),
        DetectorId::OveruseOfFastMode => overuse_of_fast_mode::evaluate(evidence, catalogs),
        DetectorId::CacheChurn => cache_churn::evaluate(evidence, catalogs),
    }
}

/// Returns whether this session belongs in the detector's eligible
/// denominator. Unused MCP Servers and Unused Skills make absence
/// claims about assistant work; a session is excluded only when
/// complete eligibility evidence proves zero assistant turns, so an
/// all-idle cohort cannot read clean. Absence read from partial
/// evidence is untrustworthy (see `observed`), so a partial-
/// eligibility session stays in the denominator whatever its observed
/// count: the assessed-only-when-complete rule holds it at
/// eligible-but-unassessed, blocking a clean claim.
pub(crate) fn in_denominator(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    match detector {
        DetectorId::UnusedMcpServers | DetectorId::UnusedSkills => complete(&evidence.eligibility)
            .is_none_or(|eligibility| eligibility.assistant_turns > 0),
        _ => true,
    }
}

/// Reduces one detector's counts and fold state to its one status.
///
/// Findings win first: partial coverage may still support an observed
/// finding. Clean requires eligible sessions where every one is fully
/// assessed, no rule hit contract limits, and no finding exists.
/// Everything else is not assessed with a structured reason, so
/// incomplete absence never reads as clean (FR-14).
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
    if counts.assessed == counts.eligible {
        return DetectorStatus::Clean;
    }
    DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
}

/// Returns the observed value from complete or partial evidence.
/// Presence read from partial evidence is trustworthy; absence is not.
pub(crate) fn observed<T>(value: &EvidenceValue<T>) -> Option<&T> {
    match value {
        EvidenceValue::Unsupported => None,
        EvidenceValue::Partial { observed, .. } => Some(observed),
        EvidenceValue::Complete(value) => Some(value),
    }
}

/// Returns the value only when the evidence is complete.
/// Only a complete value can prove that an event did not happen.
pub(crate) fn complete<T>(value: &EvidenceValue<T>) -> Option<&T> {
    match value {
        EvidenceValue::Complete(value) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::analysis::{
        EvidenceSource, SessionEvidence, SessionEvidenceAccumulator, SourceCapabilities, SourceKind,
    };

    /// Builds empty complete evidence with the Claude capability set.
    pub(crate) fn claude_evidence(session_id: &str) -> SessionEvidence {
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(eligible: u64, assessed: u64) -> DetectorCounts {
        DetectorCounts { eligible, assessed }
    }

    #[test]
    fn findings_take_precedence_over_incomplete_coverage() {
        let fold = DetectorFold {
            finding_sessions: 2,
            examples: Vec::new(),
            contract_incomplete: 1,
        };

        assert!(matches!(
            status(counts(3, 1), fold, 3),
            DetectorStatus::Findings(DetectorFindings {
                finding_sessions: 2,
                ..
            })
        ));
    }

    #[test]
    fn empty_cohort_is_not_assessed() {
        assert_eq!(
            status(counts(0, 0), DetectorFold::default(), 0),
            DetectorStatus::NotAssessed(NotAssessedReason::NoSessionsInWindow)
        );
    }

    #[test]
    fn missing_capabilities_are_not_assessed() {
        assert_eq!(
            status(counts(0, 0), DetectorFold::default(), 4),
            DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing)
        );
    }

    #[test]
    fn incomplete_absence_never_yields_clean() {
        // One of two eligible sessions carries only partial evidence.
        // The zero-finding result must not read as clean.
        assert_eq!(
            status(counts(2, 1), DetectorFold::default(), 2),
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
    }

    #[test]
    fn contract_incomplete_sessions_prevent_clean() {
        let fold = DetectorFold {
            finding_sessions: 0,
            examples: Vec::new(),
            contract_incomplete: 1,
        };

        assert_eq!(
            status(counts(2, 2), fold, 2),
            DetectorStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete)
        );
    }

    #[test]
    fn complete_absence_yields_clean() {
        assert_eq!(
            status(counts(2, 2), DetectorFold::default(), 2),
            DetectorStatus::Clean
        );
    }
}
