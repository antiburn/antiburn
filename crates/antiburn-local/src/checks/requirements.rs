use super::observed;
use crate::analysis::{CacheEvidence, EvidenceCoverage, EvidenceValue, SessionEvidence};
use crate::checks::DetectorId;

/// One fact a detector's finding or clean claim depends on. Each fact's
/// state comes from the evidence the sink already wrote — a static
/// capability boolean gates a fact only where no evidence value carries
/// it, per [`Fact::state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fact {
    MainLoopContext,
    ModelIdentity,
    EffortSignal,
    SpeedSignal,
    ToolInvocations,
    SkillInventory,
    McpInventory,
    ToolDefinitions,
    SubagentRelationships,
    DelegatedModels,
    RepeatedContextAccounting,
    RecordLinkage,
    ThreadMembership,
    CompactionBoundaries,
    TimeRange,
    Eligibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactState {
    Unsupported,
    Partial,
    Complete,
}

impl Fact {
    pub fn state(self, evidence: &SessionEvidence) -> FactState {
        match self {
            Self::MainLoopContext => state(&evidence.context),
            Self::ModelIdentity => state(&evidence.models),
            Self::EffortSignal => {
                if !evidence.capabilities.reasoning_effort_tier {
                    FactState::Unsupported
                } else {
                    state(&evidence.models)
                }
            }
            Self::SpeedSignal => {
                if !(evidence.capabilities.fast_tier || evidence.capabilities.service_tier) {
                    FactState::Unsupported
                } else {
                    state(&evidence.models)
                }
            }
            Self::ToolInvocations => state(&evidence.tools),
            Self::SkillInventory | Self::McpInventory | Self::ToolDefinitions => {
                match &evidence.context_sources {
                    EvidenceValue::Complete(sources)
                    | EvidenceValue::Partial {
                        observed: sources, ..
                    } => match self {
                        Self::SkillInventory => state(&sources.skill_coverage),
                        Self::McpInventory => state(&sources.mcp_coverage),
                        _ => state(&sources.tool_definitions),
                    },
                    EvidenceValue::Unsupported => FactState::Unsupported,
                }
            }
            Self::SubagentRelationships => state(&evidence.subagents),
            Self::DelegatedModels => {
                if !evidence.capabilities.subagent_models
                    && !observed(&evidence.subagents)
                        .is_some_and(|subagents| !subagents.delegated_models.is_empty())
                {
                    FactState::Unsupported
                } else {
                    state(&evidence.subagents)
                }
            }
            // `repeated_context` carries the accounting gate, like `previous_turn`.
            Self::RepeatedContextAccounting => {
                match cache_group_and_repeated_context(&evidence.cache) {
                    None => FactState::Unsupported,
                    Some((_, FactState::Unsupported)) => FactState::Unsupported,
                    Some((group, marker)) => weaker(group, marker),
                }
            }
            Self::RecordLinkage => match cache_group_and_marker(&evidence.cache) {
                None => FactState::Unsupported,
                Some((_, FactState::Unsupported)) => FactState::Unsupported,
                Some((group, marker)) => weaker(group, marker),
            },
            // No row fact for thread membership exists yet.
            Self::ThreadMembership => {
                if evidence.capabilities.thread_identity {
                    FactState::Complete
                } else {
                    FactState::Unsupported
                }
            }
            Self::CompactionBoundaries => state(&evidence.compactions),
            Self::TimeRange => state(&evidence.time_range),
            Self::Eligibility => state(&evidence.eligibility),
        }
    }
}

fn state<T>(value: &EvidenceValue<T>) -> FactState {
    match value {
        EvidenceValue::Unsupported => FactState::Unsupported,
        EvidenceValue::Partial { .. } => FactState::Partial,
        EvidenceValue::Complete(_) => FactState::Complete,
    }
}

fn weaker(a: FactState, b: FactState) -> FactState {
    match (a, b) {
        (FactState::Unsupported, _) | (_, FactState::Unsupported) => FactState::Unsupported,
        (FactState::Partial, _) | (_, FactState::Partial) => FactState::Partial,
        (FactState::Complete, FactState::Complete) => FactState::Complete,
    }
}

fn cache_group_and_marker(cache: &EvidenceValue<CacheEvidence>) -> Option<(FactState, FactState)> {
    match cache {
        EvidenceValue::Unsupported => None,
        EvidenceValue::Partial { observed, .. } => {
            Some((FactState::Partial, state(&observed.previous_turn)))
        }
        EvidenceValue::Complete(observed) => {
            Some((FactState::Complete, state(&observed.previous_turn)))
        }
    }
}

fn cache_group_and_repeated_context(
    cache: &EvidenceValue<CacheEvidence>,
) -> Option<(FactState, FactState)> {
    match cache {
        EvidenceValue::Unsupported => None,
        EvidenceValue::Partial { observed, .. } => {
            Some((FactState::Partial, state(&observed.repeated_context)))
        }
        EvidenceValue::Complete(observed) => {
            Some((FactState::Complete, state(&observed.repeated_context)))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectorRequirements {
    /// Facts a finding needs. Every one must not be `Unsupported` for
    /// the session to be eligible.
    pub finding: &'static [Fact],
    /// Facts a clean claim needs. Every one must be `Complete`. A
    /// superset of `finding`.
    pub clean: &'static [Fact],
}

pub fn requirements(detector: DetectorId) -> DetectorRequirements {
    match detector {
        DetectorId::SessionsOverDepth => DetectorRequirements {
            finding: &[Fact::MainLoopContext],
            clean: &[
                Fact::MainLoopContext,
                Fact::ThreadMembership,
                Fact::ModelIdentity,
                Fact::TimeRange,
            ],
        },
        DetectorId::ModelOverthinking => DetectorRequirements {
            finding: &[Fact::EffortSignal],
            clean: &[Fact::EffortSignal, Fact::Eligibility],
        },
        DetectorId::OverpoweredSubagents => DetectorRequirements {
            finding: &[
                Fact::SubagentRelationships,
                Fact::DelegatedModels,
                Fact::ModelIdentity,
            ],
            clean: &[
                Fact::SubagentRelationships,
                Fact::DelegatedModels,
                Fact::ModelIdentity,
            ],
        },
        DetectorId::UnusedMcpServers => DetectorRequirements {
            finding: &[Fact::McpInventory, Fact::ToolInvocations],
            clean: &[Fact::McpInventory, Fact::ToolInvocations, Fact::Eligibility],
        },
        DetectorId::UnusedBuiltInTools => DetectorRequirements {
            finding: &[Fact::ToolDefinitions, Fact::ToolInvocations],
            clean: &[
                Fact::ToolDefinitions,
                Fact::ToolInvocations,
                Fact::Eligibility,
            ],
        },
        DetectorId::UnusedSkills => DetectorRequirements {
            finding: &[Fact::SkillInventory, Fact::ToolInvocations],
            clean: &[
                Fact::SkillInventory,
                Fact::ToolInvocations,
                Fact::Eligibility,
            ],
        },
        DetectorId::OldModelUsage => DetectorRequirements {
            finding: &[Fact::ModelIdentity],
            clean: &[Fact::ModelIdentity, Fact::TimeRange],
        },
        DetectorId::OveruseOfFastMode => DetectorRequirements {
            finding: &[Fact::SpeedSignal],
            clean: &[Fact::SpeedSignal, Fact::SubagentRelationships],
        },
        DetectorId::CacheChurn => DetectorRequirements {
            finding: &[Fact::RepeatedContextAccounting],
            clean: &[
                Fact::RepeatedContextAccounting,
                Fact::RecordLinkage,
                Fact::CompactionBoundaries,
                Fact::ModelIdentity,
                Fact::TimeRange,
            ],
        },
        DetectorId::IgnoredInstructions | DetectorId::OverExploring | DetectorId::ScopeCreep => {
            DetectorRequirements {
                finding: &[],
                clean: &[],
            }
        }
        DetectorId::SkillOpportunities => DetectorRequirements {
            finding: &[Fact::SkillInventory, Fact::ToolInvocations],
            clean: &[
                Fact::SkillInventory,
                Fact::ToolInvocations,
                Fact::Eligibility,
            ],
        },
    }
}

/// A session is eligible when every finding fact is not `Unsupported`.
pub fn eligible(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    source_supports_finding(detector, evidence.capabilities.source_format)
        && requirements(detector)
            .finding
            .iter()
            .all(|fact| fact.state(evidence) != FactState::Unsupported)
}

fn source_supports_finding(detector: DetectorId, format: crate::analysis::SourceFormat) -> bool {
    use crate::analysis::SourceFormat;
    matches!(
        (format, detector),
        (
            SourceFormat::ClaudeJsonl | SourceFormat::CodexRolloutJsonl,
            DetectorId::SessionsOverDepth
                | DetectorId::ModelOverthinking
                | DetectorId::OverpoweredSubagents
                | DetectorId::UnusedMcpServers
                | DetectorId::UnusedBuiltInTools
                | DetectorId::UnusedSkills
                | DetectorId::OldModelUsage
                | DetectorId::OveruseOfFastMode
                | DetectorId::CacheChurn,
        ) | (
            SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2,
            DetectorId::SessionsOverDepth
                | DetectorId::OverpoweredSubagents
                | DetectorId::UnusedSkills
                | DetectorId::OldModelUsage
                | DetectorId::CacheChurn,
        ) | (
            SourceFormat::PiV3Jsonl,
            DetectorId::SessionsOverDepth
                | DetectorId::ModelOverthinking
                | DetectorId::OverpoweredSubagents
                | DetectorId::OldModelUsage
                | DetectorId::CacheChurn,
        ) | (
            // OMP subagent evidence lives in sibling files this reader does not open.
            SourceFormat::OmpV3Jsonl,
            DetectorId::SessionsOverDepth
                | DetectorId::ModelOverthinking
                | DetectorId::OldModelUsage,
        ) | (
            SourceFormat::CursorJsonl
                | SourceFormat::CursorCliAgentJsonl
                | SourceFormat::CursorCliStoreDb
                | SourceFormat::CursorChatStoreDb
                | SourceFormat::CursorIdeComposer,
            DetectorId::OldModelUsage,
        ) | (
            SourceFormat::AntigravityJson
                | SourceFormat::AntigravityBrainJsonl
                | SourceFormat::AntigravityCascadeJson
                | SourceFormat::AntigravitySqlite,
            DetectorId::SessionsOverDepth | DetectorId::OldModelUsage,
        ) | (
            SourceFormat::CopilotCliJsonl,
            DetectorId::OverpoweredSubagents | DetectorId::OldModelUsage,
        ) | (
            SourceFormat::ClineMessagesContractV1,
            DetectorId::OverpoweredSubagents | DetectorId::OldModelUsage,
        ) | (
            SourceFormat::AmpThreadJson,
            DetectorId::SessionsOverDepth | DetectorId::OldModelUsage,
        ) | (
            SourceFormat::DevinLocalSqlite,
            DetectorId::OverpoweredSubagents
        ) | (
            SourceFormat::WindsurfWorkspaceJson | SourceFormat::WindsurfMirrorJson,
            DetectorId::UnusedMcpServers
                | DetectorId::UnusedBuiltInTools
                | DetectorId::OldModelUsage,
        )
    )
}

/// Only complete evidence from characterized sources can prove a clean result.
pub fn clean_facts_complete(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    !matches!(
        detector,
        DetectorId::UnusedSkills
            | DetectorId::UnusedMcpServers
            | DetectorId::UnusedBuiltInTools
            | DetectorId::SkillOpportunities
    ) && evidence.coverage == EvidenceCoverage::Complete
        && source_supports_clean(detector, evidence.capabilities.source_format)
        && requirements(detector)
            .clean
            .iter()
            .all(|fact| fact.state(evidence) == FactState::Complete)
}

fn source_supports_clean(detector: DetectorId, format: crate::analysis::SourceFormat) -> bool {
    use crate::analysis::SourceFormat;
    matches!(
        (format, detector),
        (
            SourceFormat::ClaudeJsonl
                | SourceFormat::CodexRolloutJsonl
                | SourceFormat::OpenCodeJsonl
                | SourceFormat::OpenCodeSqliteV2
                | SourceFormat::PiV3Jsonl,
            _,
        ) | (
            SourceFormat::CopilotCliJsonl,
            DetectorId::OverpoweredSubagents | DetectorId::OldModelUsage,
        )
    )
}

/// A clean claim is out of reach when a clean fact is `Unsupported`.
pub fn clean_fact_unsupported(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    requirements(detector)
        .clean
        .iter()
        .any(|fact| fact.state(evidence) == FactState::Unsupported)
}
