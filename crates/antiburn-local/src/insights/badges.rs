use crate::analysis::{EvidenceCoverage, SessionEvidence};

use super::DetectorId;
use super::detectors::{self, NotAssessedReason, Observation, ReportCatalogs};
use super::report::{clean_facts_complete, eligible};

/// Identifies one session-level hygiene badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BadgeId {
    SessionOverdepth,
    ModelOverthinking,
    OverpoweredSubagents,
    ObsoleteModel,
    FastModeOveruse,
    ExcessCacheRehydration,
}

impl BadgeId {
    pub const ALL: [Self; 6] = [
        Self::SessionOverdepth,
        Self::ModelOverthinking,
        Self::OverpoweredSubagents,
        Self::ObsoleteModel,
        Self::FastModeOveruse,
        Self::ExcessCacheRehydration,
    ];

    const fn detector(self) -> DetectorId {
        match self {
            Self::SessionOverdepth => DetectorId::SessionsOverDepth,
            Self::ModelOverthinking => DetectorId::ModelOverthinking,
            Self::OverpoweredSubagents => DetectorId::OverpoweredSubagents,
            Self::ObsoleteModel => DetectorId::OldModelUsage,
            Self::FastModeOveruse => DetectorId::OveruseOfFastMode,
            Self::ExcessCacheRehydration => DetectorId::CacheChurn,
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

/// Reduces one session's stored evidence into the six v1 badges.
pub fn session_badges(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> [SessionBadge; 6] {
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
    if !eligible(detector, evidence) {
        return BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing);
    }

    match detectors::evaluate(detector, evidence, catalogs) {
        Observation::Finding => BadgeStatus::Finding,
        Observation::ContractIncomplete => {
            BadgeStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete)
        }
        Observation::SignalMissing => BadgeStatus::NotAssessed(NotAssessedReason::SignalMissing),
        Observation::NoFinding => {
            if clean_facts_complete(detector, evidence)
                && evidence.coverage == EvidenceCoverage::Complete
            {
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
        EvidenceSource, EvidenceValue, ModelTokens, ModelTransition, PARSER_REVISION,
        RelationConfidence, RelationProvenance, SessionEvidence, SessionEvidenceAccumulator,
        SourceCapabilities, SourceKind, SubagentChild, TurnCounts, TurnFacts,
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
            BadgeId::SessionOverdepth => {
                evidence.context = EvidenceValue::Complete(ContextEvidence {
                    max_request_context_tokens: ReportCatalogs::default().depth_cap_tokens + 1,
                    top_depth_examples: Vec::new(),
                });
                if partial {
                    evidence.context = make_partial(evidence.context);
                }
            }
            BadgeId::ModelOverthinking => {
                let EvidenceValue::Complete(models) = &mut evidence.models else {
                    unreachable!()
                };
                // A `by_model` entry establishes that the Claude family
                // is present, which the reviewed policy needs to
                // classify "max" as above the cap.
                models
                    .by_model
                    .insert("claude-sonnet-4-6".to_owned(), ModelTokens::default());
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
            BadgeId::OverpoweredSubagents => {
                let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
                    unreachable!()
                };
                subagents.spawn_count = 1;
                subagents.delegated_turns = 1;
                subagents
                    .delegated_models
                    .insert("claude-opus-4-6".to_owned());
                subagents.children.push(SubagentChild {
                    ordinal: 1,
                    parent_model: Some("claude-opus-4-6".to_owned()),
                    child_model: EvidenceValue::Unsupported,
                    confidence: RelationConfidence::Observed,
                    provenance: RelationProvenance::TaskToolUse,
                });
                if partial {
                    evidence.subagents = make_partial(evidence.subagents);
                }
            }
            BadgeId::ObsoleteModel => {
                let EvidenceValue::Complete(models) = &mut evidence.models else {
                    unreachable!()
                };
                models.by_model.insert(
                    "old-model".to_owned(),
                    ModelTokens {
                        turns: 1,
                        last_ts_ms: 1,
                        ..ModelTokens::default()
                    },
                );
                if partial {
                    evidence.models = make_partial(evidence.models);
                }
            }
            BadgeId::FastModeOveruse => {
                let EvidenceValue::Complete(models) = &mut evidence.models else {
                    unreachable!()
                };
                models.fast_modes.insert(
                    "fast".to_owned(),
                    TurnCounts {
                        main_loop: 0,
                        delegated: 1,
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
        }
        if partial {
            evidence.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);
        }
        evidence
    }

    fn test_catalogs() -> ReportCatalogs {
        let mut catalogs = ReportCatalogs::default();
        catalogs.model_replacements.entries.insert(
            "old-model".to_owned(),
            super::super::detectors::ModelReplacementEntry {
                replacement: "new-model".to_owned(),
                available_since_ts_ms: 0,
                rationale: "test rule".to_owned(),
                source_url: "https://example.invalid/old-model".to_owned(),
            },
        );
        catalogs
    }

    fn badge(evidence: &SessionEvidence, id: BadgeId) -> SessionBadge {
        session_badges(evidence, &test_catalogs())
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

    /// Two badges never carry the generic zero-turn expectation: Model
    /// Overthinking and Fast Mode Overuse report a missing signal,
    /// because the synthetic evidence carries zero eligible turns.
    /// Obsolete Model does not need an override: the reviewed
    /// production registry is non-empty, and zero observed models
    /// means no catalogued model can have run, so it reads clean like
    /// the rest.
    fn zero_turn_override(id: BadgeId) -> Option<BadgeStatus> {
        match id {
            BadgeId::ModelOverthinking | BadgeId::FastModeOveruse => {
                Some(BadgeStatus::NotAssessed(NotAssessedReason::SignalMissing))
            }
            _ => None,
        }
    }

    fn zero_turn_override_detector(id: BadgeId) -> Option<DetectorStatus> {
        match zero_turn_override(id)? {
            BadgeStatus::NotAssessed(reason) => Some(DetectorStatus::NotAssessed(reason)),
            BadgeStatus::Clean => Some(DetectorStatus::Clean),
            BadgeStatus::Finding => None,
        }
    }

    #[test]
    fn partial_coverage_without_a_signal_never_reads_clean() {
        let mut evidence = claude_evidence("synthetic-partial");
        evidence.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);

        for result in session_badges(&evidence, &ReportCatalogs::default()) {
            // Every other badge reports the session-wide partial coverage.
            let expected = zero_turn_override(result.id).unwrap_or(BadgeStatus::NotAssessed(
                NotAssessedReason::IncompleteEvidence,
            ));
            assert_eq!(result.status, expected, "{:?}", result.id);
        }
    }

    #[test]
    fn a_missing_capability_is_not_assessed() {
        // Every fact each badge's finding depends on must be
        // `Unsupported`. Facts a badge's evaluate body reads straight
        // off an evidence group (`MainLoopContext`, `ModelIdentity`,
        // `CacheWriteAccounting`, ...) are derived at evidence-
        // construction time from the capabilities below, so this
        // builds a fresh session instead of mutating an already-built
        // one: mutating `capabilities` after the fact would leave
        // those already-computed groups untouched.
        let mut capabilities = SourceCapabilities::claude();
        capabilities.request_context_tokens = false;
        capabilities.model_identity = false;
        capabilities.reasoning_effort_tier = false;
        capabilities.fast_tier = false;
        capabilities.subagent_models = false;
        capabilities.cache_write_tokens = false;
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "synthetic-capability".to_owned(),
            kind: SourceKind::File,
            capabilities,
        })
        .evidence(&TurnFacts::default());

        for id in BadgeId::ALL {
            assert_eq!(
                badge(&evidence, id).status,
                BadgeStatus::NotAssessed(NotAssessedReason::CapabilityMissing),
                "{id:?}"
            );
        }
    }

    #[test]
    fn complete_evidence_without_a_signal_reads_clean() {
        let evidence = claude_evidence("synthetic-clean");

        for result in session_badges(&evidence, &ReportCatalogs::default()) {
            let expected = zero_turn_override(result.id).unwrap_or(BadgeStatus::Clean);
            assert_eq!(result.status, expected, "{:?}", result.id);
        }
    }

    fn report_status(
        evidence: SessionEvidence,
        detector: DetectorId,
        catalogs: &ReportCatalogs,
    ) -> DetectorStatus {
        let mut accumulator = EfficiencyReportAccumulator::with_catalogs(catalogs.clone());
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
        let catalogs = test_catalogs();
        let cohort = [
            claude_evidence("synthetic-clean"),
            finding_evidence(BadgeId::SessionOverdepth, false),
            finding_evidence(BadgeId::ModelOverthinking, false),
            finding_evidence(BadgeId::OverpoweredSubagents, false),
            finding_evidence(BadgeId::ObsoleteModel, false),
            finding_evidence(BadgeId::FastModeOveruse, false),
            finding_evidence(BadgeId::ExcessCacheRehydration, false),
        ];

        for evidence in cohort {
            let statuses: BTreeMap<BadgeId, BadgeStatus> = session_badges(&evidence, &catalogs)
                .map(|badge| (badge.id, badge.status))
                .into_iter()
                .collect();
            for id in BadgeId::ALL {
                let report = report_status(evidence.clone(), id.detector(), &catalogs);
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
    /// NotAssessed. This asymmetry is intentional. Issue #229 keeps the
    /// detector-scope report rule.
    #[test]
    fn session_wide_partial_coverage_diverges_from_report_by_design() {
        let mut evidence = claude_evidence("synthetic-divergent");
        evidence.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);

        for badge in session_badges(&evidence, &ReportCatalogs::default()) {
            // Obsolete Model, Model Overthinking, and Fast Mode Overuse
            // report their own not-assessed reason on both sides
            // regardless of session-wide coverage, so they sit outside
            // this test's badge-vs-report divergence claim.
            let expected = zero_turn_override(badge.id).unwrap_or(BadgeStatus::NotAssessed(
                NotAssessedReason::IncompleteEvidence,
            ));
            assert_eq!(badge.status, expected, "{:?}", badge.id);
        }
        for id in BadgeId::ALL {
            let expected = zero_turn_override_detector(id).unwrap_or(DetectorStatus::Clean);
            assert_eq!(
                report_status(evidence.clone(), id.detector(), &ReportCatalogs::default()),
                expected,
                "{id:?}"
            );
        }
    }
}
