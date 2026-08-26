// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::BTreeMap;

use crate::analysis::{
    CoverageReason, EvidenceCoverage, EvidenceValue, SessionEvidence, SourceAcceptance,
    SourceCapabilities,
};

use super::detectors::{self, DetectorFold, DetectorStatus, ReportCatalogs};
use super::quota::{QuotaPressureAccumulator, QuotaPressureSection};
use super::{CoverageBucket, DetectorId};

pub const MAX_EXAMPLES_PER_DETECTOR: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectorCounts {
    pub eligible: u64,
    pub assessed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportWindow {
    pub start_epoch: i64,
    pub end_epoch: i64,
}

/// Names one session without transcript content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionExample {
    pub agent: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityFlag {
    RequestContextTokens,
    CacheWriteTokens,
    TimestampsAndOrder,
    ToolInvocations,
    SkillMcpAttribution,
    ToolDefinitions,
    ModelIdentity,
    TokenClasses,
    ReasoningEffortTier,
    FastTier,
    ServiceTier,
    SubagentRelationships,
    SubagentModels,
    CompactionBoundaries,
    ThreadIdentity,
}

impl CapabilityFlag {
    pub fn is_set(self, capabilities: &SourceCapabilities) -> bool {
        match self {
            Self::RequestContextTokens => capabilities.request_context_tokens,
            Self::CacheWriteTokens => capabilities.cache_write_tokens,
            Self::TimestampsAndOrder => capabilities.timestamps_and_order,
            Self::ToolInvocations => capabilities.tool_invocations,
            Self::SkillMcpAttribution => capabilities.skill_mcp_attribution,
            Self::ToolDefinitions => capabilities.tool_definitions,
            Self::ModelIdentity => capabilities.model_identity,
            Self::TokenClasses => capabilities.token_classes,
            Self::ReasoningEffortTier => capabilities.reasoning_effort_tier,
            Self::FastTier => capabilities.fast_tier,
            Self::ServiceTier => capabilities.service_tier,
            Self::SubagentRelationships => capabilities.subagent_relationships,
            Self::SubagentModels => capabilities.subagent_models,
            Self::CompactionBoundaries => capabilities.compaction_boundaries,
            Self::ThreadIdentity => capabilities.thread_identity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceGroup {
    Context,
    Eligibility,
    Tools,
    ContextSources,
    Models,
    Subagents,
    Cache,
    Compactions,
    TimeRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    Unsupported,
    Partial,
    Complete,
}

impl EvidenceGroup {
    pub fn state(self, evidence: &SessionEvidence) -> GroupState {
        match self {
            Self::Context => state(&evidence.context),
            Self::Eligibility => state(&evidence.eligibility),
            Self::Tools => state(&evidence.tools),
            Self::ContextSources => state(&evidence.context_sources),
            Self::Models => state(&evidence.models),
            Self::Subagents => state(&evidence.subagents),
            Self::Cache => state(&evidence.cache),
            Self::Compactions => state(&evidence.compactions),
            Self::TimeRange => state(&evidence.time_range),
        }
    }
}

fn state<T>(value: &EvidenceValue<T>) -> GroupState {
    match value {
        EvidenceValue::Unsupported => GroupState::Unsupported,
        EvidenceValue::Partial { .. } => GroupState::Partial,
        EvidenceValue::Complete(_) => GroupState::Complete,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectorRequirements {
    pub capabilities: &'static [&'static [CapabilityFlag]],
    pub groups: &'static [EvidenceGroup],
}

const REQUEST_CONTEXT_TOKENS: &[CapabilityFlag] = &[CapabilityFlag::RequestContextTokens];
const CACHE_WRITE_TOKENS: &[CapabilityFlag] = &[CapabilityFlag::CacheWriteTokens];
const TIMESTAMPS_AND_ORDER: &[CapabilityFlag] = &[CapabilityFlag::TimestampsAndOrder];
const TOOL_INVOCATIONS: &[CapabilityFlag] = &[CapabilityFlag::ToolInvocations];
const SKILL_MCP_ATTRIBUTION: &[CapabilityFlag] = &[CapabilityFlag::SkillMcpAttribution];
const TOOL_DEFINITIONS: &[CapabilityFlag] = &[CapabilityFlag::ToolDefinitions];
const MODEL_IDENTITY: &[CapabilityFlag] = &[CapabilityFlag::ModelIdentity];
const TOKEN_CLASSES: &[CapabilityFlag] = &[CapabilityFlag::TokenClasses];
const REASONING_EFFORT_TIER: &[CapabilityFlag] = &[CapabilityFlag::ReasoningEffortTier];
const FAST_OR_SERVICE_TIER: &[CapabilityFlag] =
    &[CapabilityFlag::FastTier, CapabilityFlag::ServiceTier];
const SUBAGENT_RELATIONSHIPS: &[CapabilityFlag] = &[CapabilityFlag::SubagentRelationships];
const SUBAGENT_MODELS: &[CapabilityFlag] = &[CapabilityFlag::SubagentModels];
const COMPACTION_BOUNDARIES: &[CapabilityFlag] = &[CapabilityFlag::CompactionBoundaries];
const THREAD_IDENTITY: &[CapabilityFlag] = &[CapabilityFlag::ThreadIdentity];

pub fn requirements(detector: DetectorId) -> DetectorRequirements {
    use EvidenceGroup as Group;

    match detector {
        DetectorId::SessionsOverDepth => DetectorRequirements {
            capabilities: &[
                REQUEST_CONTEXT_TOKENS,
                MODEL_IDENTITY,
                THREAD_IDENTITY,
                TIMESTAMPS_AND_ORDER,
            ],
            groups: &[Group::Context],
        },
        DetectorId::ModelOverthinking => DetectorRequirements {
            capabilities: &[REASONING_EFFORT_TIER, MODEL_IDENTITY],
            groups: &[Group::Models, Group::Eligibility],
        },
        DetectorId::OverpoweredSubagents => DetectorRequirements {
            capabilities: &[MODEL_IDENTITY, SUBAGENT_MODELS, SUBAGENT_RELATIONSHIPS],
            groups: &[Group::Subagents, Group::Models],
        },
        DetectorId::UnusedMcpServers => DetectorRequirements {
            capabilities: &[SKILL_MCP_ATTRIBUTION, TOOL_INVOCATIONS],
            groups: &[Group::ContextSources, Group::Tools, Group::Eligibility],
        },
        DetectorId::UnusedBuiltInTools => DetectorRequirements {
            capabilities: &[TOOL_DEFINITIONS, TOOL_INVOCATIONS],
            groups: &[Group::ContextSources, Group::Tools],
        },
        DetectorId::UnusedSkills => DetectorRequirements {
            capabilities: &[SKILL_MCP_ATTRIBUTION, TOOL_INVOCATIONS],
            groups: &[Group::ContextSources, Group::Tools, Group::Eligibility],
        },
        DetectorId::OldModelUsage => DetectorRequirements {
            capabilities: &[MODEL_IDENTITY, TIMESTAMPS_AND_ORDER, TOKEN_CLASSES],
            groups: &[Group::Models, Group::TimeRange],
        },
        DetectorId::OveruseOfFastMode => DetectorRequirements {
            capabilities: &[FAST_OR_SERVICE_TIER, SUBAGENT_RELATIONSHIPS],
            groups: &[Group::Models, Group::Subagents],
        },
        DetectorId::CacheChurn => DetectorRequirements {
            capabilities: &[
                TIMESTAMPS_AND_ORDER,
                THREAD_IDENTITY,
                MODEL_IDENTITY,
                CACHE_WRITE_TOKENS,
                COMPACTION_BOUNDARIES,
            ],
            groups: &[Group::Cache, Group::Compactions, Group::Models],
        },
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageCounts {
    pub discovered: u64,
    pub unknown_start: u64,
    pub pending: u64,
    pub processing: u64,
    pub failed: u64,
    pub unsupported: u64,
    pub stale: u64,
    pub ready: u64,
    pub actively_growing: u64,
    pub awaiting_provider_support: u64,
}

impl CoverageCounts {
    pub fn observe(&mut self, bucket: CoverageBucket, count: u64) {
        self.discovered += count;
        match bucket {
            CoverageBucket::UnknownStart => self.unknown_start += count,
            CoverageBucket::Pending => self.pending += count,
            CoverageBucket::Processing => self.processing += count,
            CoverageBucket::Failed => self.failed += count,
            CoverageBucket::Unsupported => self.unsupported += count,
            CoverageBucket::Stale => self.stale += count,
            CoverageBucket::Ready => self.ready += count,
        }
    }

    pub fn is_consistent(&self) -> bool {
        self.discovered
            == self.unknown_start
                + self.pending
                + self.processing
                + self.failed
                + self.unsupported
                + self.stale
                + self.ready
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportContext {
    pub environment_key: String,
    pub window: ReportWindow,
    pub computed_at_epoch: i64,
    pub parser_revision: i64,
    pub analyzer_revision: i64,
    pub evidence_schema_revision: i64,
    pub coverage: CoverageCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EfficiencyReport {
    pub context: ReportContext,
    pub assessed_sessions: u64,
    pub detectors: [DetectorCounts; 9],
    pub detector_statuses: [DetectorStatus; 9],
    pub quota_pressure: QuotaPressureSection,
    pub catalog_revision: i64,
    pub coverage_reasons: BTreeMap<CoverageReason, u64>,
    pub capability_gaps: BTreeMap<DetectorId, u64>,
    pub capability_gap_examples: BTreeMap<DetectorId, Vec<SessionExample>>,
}

pub struct EfficiencyReportAccumulator {
    assessed_sessions: u64,
    detectors: [DetectorCounts; 9],
    folds: [DetectorFold; 9],
    quota: QuotaPressureAccumulator,
    catalogs: ReportCatalogs,
    coverage_reasons: BTreeMap<CoverageReason, u64>,
    capability_gaps: BTreeMap<DetectorId, u64>,
    capability_gap_examples: BTreeMap<DetectorId, Vec<SessionExample>>,
    actively_growing: u64,
}

impl Default for EfficiencyReportAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl EfficiencyReportAccumulator {
    pub fn new() -> Self {
        Self::with_catalogs(ReportCatalogs::default())
    }

    /// Builds an accumulator with report-time catalogs. Catalogs are
    /// applied during reduction only and never touch stored evidence.
    pub fn with_catalogs(catalogs: ReportCatalogs) -> Self {
        Self {
            assessed_sessions: 0,
            detectors: [DetectorCounts {
                eligible: 0,
                assessed: 0,
            }; 9],
            folds: core::array::from_fn(|_| DetectorFold::default()),
            quota: QuotaPressureAccumulator::default(),
            catalogs,
            coverage_reasons: BTreeMap::new(),
            capability_gaps: BTreeMap::new(),
            capability_gap_examples: BTreeMap::new(),
            actively_growing: 0,
        }
    }

    /// Observes one session from the ready-and-current cohort.
    pub fn observe_session(&mut self, evidence: SessionEvidence) {
        self.assessed_sessions += 1;
        if let EvidenceCoverage::Partial(reason) = evidence.coverage {
            *self.coverage_reasons.entry(reason).or_default() += 1;
        }
        if matches!(
            evidence.provenance.source_acceptance,
            SourceAcceptance::AcceptedPrefix { .. }
        ) {
            self.actively_growing += 1;
        }

        // The quota section reads every cohort session. It stays
        // outside the nine-category eligibility loop below.
        self.quota
            .observe_session(&evidence.identity, &evidence.quota_incidents);

        // Lazily allocate the identity example only if this session has a detector gap.
        let mut bounded_example: Option<SessionExample> = None;

        for detector in DetectorId::ALL {
            let requirements = requirements(detector);
            let capabilities_hold = requirements.capabilities.iter().all(|clause| {
                clause
                    .iter()
                    .any(|flag| flag.is_set(&evidence.capabilities))
            });
            let groups_supported = requirements
                .groups
                .iter()
                .all(|group| group.state(&evidence) != GroupState::Unsupported);
            let eligible = capabilities_hold && groups_supported;
            if eligible {
                let counts = &mut self.detectors[detector.index()];
                counts.eligible += 1;
                if requirements
                    .groups
                    .iter()
                    .all(|group| group.state(&evidence) == GroupState::Complete)
                {
                    counts.assessed += 1;
                }
                let observation = detectors::evaluate(detector, &evidence, &self.catalogs);
                self.folds[detector.index()].observe(observation, &evidence);
                continue;
            }

            *self.capability_gaps.entry(detector).or_default() += 1;
            let examples = self.capability_gap_examples.entry(detector).or_default();
            if examples.len() < MAX_EXAMPLES_PER_DETECTOR {
                let example = bounded_example.get_or_insert_with(|| SessionExample {
                    agent: evidence.identity.agent.clone(),
                    session_id: evidence.identity.session_id.clone(),
                });
                examples.push(example.clone());
            }
        }
    }

    pub fn finish(self, mut context: ReportContext) -> EfficiencyReport {
        context.coverage.actively_growing = self.actively_growing;
        let detector_statuses = core::array::from_fn(|index| {
            detectors::status(
                self.detectors[index],
                self.folds[index].clone(),
                self.assessed_sessions,
            )
        });
        EfficiencyReport {
            context,
            assessed_sessions: self.assessed_sessions,
            detectors: self.detectors,
            detector_statuses,
            quota_pressure: self.quota.finish(),
            catalog_revision: self.catalogs.revision,
            coverage_reasons: self.coverage_reasons,
            capability_gaps: self.capability_gaps,
            capability_gap_examples: self.capability_gap_examples,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{
        ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, EvidenceSource, PARSER_REVISION,
        QuotaConfidence, QuotaHitSeverity, QuotaIncident, QuotaLimitKind,
        SessionEvidenceAccumulator, SessionQuotaEvidence, SourceKind, TurnCounts,
    };
    use crate::insights::detectors::NotAssessedReason;
    use crate::insights::quota::QuotaPressureSection;

    fn evidence(session_id: &str) -> SessionEvidence {
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence()
    }

    fn context(coverage: CoverageCounts) -> ReportContext {
        ReportContext {
            environment_key: "native".to_owned(),
            window: ReportWindow {
                start_epoch: 10,
                end_epoch: 20,
            },
            computed_at_epoch: 20,
            parser_revision: PARSER_REVISION,
            analyzer_revision: ANALYZER_REVISION,
            evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
            coverage,
        }
    }

    #[test]
    fn coverage_buckets_partition_discovered_without_overlays() {
        let mut counts = CoverageCounts::default();
        for bucket in [
            CoverageBucket::UnknownStart,
            CoverageBucket::Pending,
            CoverageBucket::Processing,
            CoverageBucket::Failed,
            CoverageBucket::Unsupported,
            CoverageBucket::Stale,
            CoverageBucket::Ready,
        ] {
            counts.observe(bucket, 2);
        }
        counts.actively_growing = 4;
        counts.awaiting_provider_support = 1;

        assert_eq!(counts.discovered, 14);
        assert!(counts.is_consistent());
    }

    #[test]
    fn any_capability_clause_accepts_either_flag() {
        for (fast_tier, service_tier) in [(true, false), (false, true)] {
            let mut row = evidence("mode");
            row.capabilities.fast_tier = fast_tier;
            row.capabilities.service_tier = service_tier;
            let mut accumulator = EfficiencyReportAccumulator::new();
            accumulator.observe_session(row);
            let report = accumulator.finish(context(CoverageCounts::default()));

            assert_eq!(
                report.detectors[DetectorId::OveruseOfFastMode.index()].eligible,
                1
            );
        }
    }

    #[test]
    fn group_states_separate_eligibility_from_assessment() {
        let mut unsupported = evidence("unsupported");
        unsupported.models = EvidenceValue::Unsupported;
        let mut partial = evidence("partial");
        partial.models = match partial.models {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::AttributionIncomplete,
            },
            _ => unreachable!(),
        };
        let complete = evidence("complete");
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(unsupported);
        accumulator.observe_session(partial);
        accumulator.observe_session(complete);
        let report = accumulator.finish(context(CoverageCounts::default()));
        let counts = report.detectors[DetectorId::ModelOverthinking.index()];

        assert_eq!(counts.eligible, 2);
        assert_eq!(counts.assessed, 1);
    }

    #[test]
    fn claude_matrix_has_the_exact_eligible_detector_set() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence("matrix"));
        let report = accumulator.finish(context(CoverageCounts::default()));
        let eligible: Vec<_> = DetectorId::ALL
            .into_iter()
            .filter(|detector| report.detectors[detector.index()].eligible == 1)
            .collect();

        assert_eq!(
            eligible,
            vec![
                DetectorId::ModelOverthinking,
                DetectorId::UnusedMcpServers,
                DetectorId::UnusedSkills,
                DetectorId::OldModelUsage,
                DetectorId::OveruseOfFastMode,
            ]
        );
    }

    #[test]
    fn finish_replaces_only_the_actively_growing_overlay() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        for index in 0..5 {
            let mut row = evidence(&format!("session-{index}"));
            row.provenance.source_acceptance = if index < 3 {
                SourceAcceptance::AcceptedPrefix { boundary: 10 }
            } else {
                SourceAcceptance::AcceptedFull
            };
            accumulator.observe_session(row);
        }
        let mut coverage = CoverageCounts::default();
        coverage.observe(CoverageBucket::Ready, 5);
        coverage.actively_growing = 99;
        coverage.awaiting_provider_support = 2;
        let report = accumulator.finish(context(coverage));

        assert_eq!(report.context.coverage.actively_growing, 3);
        assert_eq!(report.context.coverage.ready, 5);
        assert_eq!(report.context.coverage.awaiting_provider_support, 2);
        assert!(report.context.coverage.actively_growing <= report.context.coverage.ready);
        assert!(report.context.coverage.is_consistent());
    }

    #[test]
    fn an_empty_cohort_reports_every_status_as_not_assessed() {
        let accumulator = EfficiencyReportAccumulator::new();
        let mut coverage = CoverageCounts::default();
        coverage.observe(CoverageBucket::UnknownStart, 2);
        coverage.observe(CoverageBucket::Pending, 3);
        let report = accumulator.finish(context(coverage));

        for status in &report.detector_statuses {
            assert_eq!(
                *status,
                DetectorStatus::NotAssessed(NotAssessedReason::NoSessionsInWindow)
            );
        }
        assert_eq!(report.quota_pressure, QuotaPressureSection::NotAssessed);
    }

    #[test]
    fn unknown_start_and_pending_rows_never_enter_a_detector_denominator() {
        // Denominator-only rows reach the report through coverage
        // counts and never through observe_session. Their presence
        // must not move any detector's eligible or assessed count.
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence("cohort-only"));
        let mut coverage = CoverageCounts::default();
        coverage.observe(CoverageBucket::Ready, 1);
        coverage.observe(CoverageBucket::UnknownStart, 4);
        coverage.observe(CoverageBucket::Pending, 5);
        let report = accumulator.finish(context(coverage));

        assert_eq!(report.assessed_sessions, 1);
        for detector in DetectorId::ALL {
            let counts = report.detectors[detector.index()];
            assert!(counts.eligible <= report.assessed_sessions);
            assert!(counts.assessed <= counts.eligible);
        }
        assert_eq!(report.context.coverage.unknown_start, 4);
        assert_eq!(report.context.coverage.pending, 5);
    }

    #[test]
    fn each_detector_produces_exactly_one_status_for_the_claude_matrix() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence("matrix"));
        let report = accumulator.finish(context(CoverageCounts::default()));

        // Complete empty evidence proves absence for the eligible
        // detectors that can express their rule.
        for detector in [
            DetectorId::ModelOverthinking,
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedSkills,
            DetectorId::OldModelUsage,
            DetectorId::OveruseOfFastMode,
        ] {
            assert_eq!(
                report.detector_statuses[detector.index()],
                DetectorStatus::Clean
            );
        }
        // Capability gaps stay not assessed with a structured reason.
        for detector in [
            DetectorId::SessionsOverDepth,
            DetectorId::OverpoweredSubagents,
            DetectorId::UnusedBuiltInTools,
            DetectorId::CacheChurn,
        ] {
            assert_eq!(
                report.detector_statuses[detector.index()],
                DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing)
            );
        }
    }

    #[test]
    fn incomplete_absence_never_yields_clean_at_report_level() {
        // One of two eligible sessions carries only partial model
        // evidence and shows no finding. Overthinking must not read
        // clean from that incomplete absence.
        let mut partial = evidence("partial");
        partial.models = match partial.models {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::IncompleteTail,
            },
            _ => unreachable!(),
        };
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence("complete"));
        accumulator.observe_session(partial);
        let report = accumulator.finish(context(CoverageCounts::default()));

        assert_eq!(
            report.detector_statuses[DetectorId::ModelOverthinking.index()],
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
    }

    #[test]
    fn an_observed_finding_reaches_the_report_status_with_examples() {
        let mut row = evidence("fast-delegation");
        let EvidenceValue::Complete(models) = &mut row.models else {
            unreachable!()
        };
        models.fast_modes.insert(
            "fast".to_owned(),
            TurnCounts {
                main_loop: 0,
                delegated: 2,
            },
        );
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(row);
        let report = accumulator.finish(context(CoverageCounts::default()));

        let DetectorStatus::Findings(findings) =
            &report.detector_statuses[DetectorId::OveruseOfFastMode.index()]
        else {
            panic!("expected findings");
        };
        assert_eq!(findings.finding_sessions, 1);
        assert_eq!(findings.examples.len(), 1);
        assert_eq!(findings.examples[0].session_id, "fast-delegation");
        assert_eq!(report.catalog_revision, ReportCatalogs::default().revision);
    }

    #[test]
    fn quota_section_is_not_assessed_without_transcript_quota_evidence() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence("no-quota"));
        let report = accumulator.finish(context(CoverageCounts::default()));

        assert_eq!(report.quota_pressure, QuotaPressureSection::NotAssessed);
    }

    #[test]
    fn quota_section_reports_deduplicated_transcript_incidents() {
        let hit = QuotaIncident {
            ts_ms: 700,
            limit_kind: QuotaLimitKind::RollingWindow,
            severity: QuotaHitSeverity::HardHit,
            model: Some("model-a".to_owned()),
            reset_ts_ms: Some(900),
            utilization_pct: None,
            confidence: QuotaConfidence::Observed,
        };
        let mut row = evidence("limited");
        row.quota_incidents = EvidenceValue::Complete(SessionQuotaEvidence {
            incidents: vec![hit.clone(), hit],
        });
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(row);
        let report = accumulator.finish(context(CoverageCounts::default()));

        let QuotaPressureSection::Findings(findings) = &report.quota_pressure else {
            panic!("expected quota findings");
        };
        assert_eq!(findings.total_hits, 1);
        assert_eq!(
            findings.hits_by_limit_kind,
            BTreeMap::from([(QuotaLimitKind::RollingWindow, 1)])
        );
        assert_eq!(findings.affected_session_count, 1);
        assert_eq!(
            findings.affected_models,
            ["model-a".to_owned()].into_iter().collect()
        );
        assert_eq!(findings.observed_times_ms, vec![700]);
    }

    #[test]
    fn capability_gap_examples_keep_the_first_three_sessions() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        for index in 0..5 {
            accumulator.observe_session(evidence(&format!("session-{index}")));
        }
        let report = accumulator.finish(context(CoverageCounts::default()));
        let detector = DetectorId::SessionsOverDepth;

        assert_eq!(report.capability_gaps[&detector], 5);
        assert_eq!(report.capability_gap_examples[&detector].len(), 3);
        assert_eq!(
            report.capability_gap_examples[&detector]
                .iter()
                .map(|example| example.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-0", "session-1", "session-2"]
        );
        assert!(report.capability_gap_examples.len() <= DetectorId::ALL.len());
        assert!(
            report
                .capability_gap_examples
                .values()
                .map(Vec::len)
                .sum::<usize>()
                <= DetectorId::ALL.len() * MAX_EXAMPLES_PER_DETECTOR
        );
    }
}
