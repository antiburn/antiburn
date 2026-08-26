// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::BTreeMap;

use crate::analysis::{
    CoverageReason, EvidenceCoverage, EvidenceValue, SessionEvidence, SourceAcceptance,
    SourceCapabilities,
};

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
    pub coverage_reasons: BTreeMap<CoverageReason, u64>,
    pub capability_gaps: BTreeMap<DetectorId, u64>,
    pub capability_gap_examples: BTreeMap<DetectorId, Vec<SessionExample>>,
}

pub struct EfficiencyReportAccumulator {
    assessed_sessions: u64,
    detectors: [DetectorCounts; 9],
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
        Self {
            assessed_sessions: 0,
            detectors: [DetectorCounts {
                eligible: 0,
                assessed: 0,
            }; 9],
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
        EfficiencyReport {
            context,
            assessed_sessions: self.assessed_sessions,
            detectors: self.detectors,
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
        SessionEvidenceAccumulator, SourceKind,
    };

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
