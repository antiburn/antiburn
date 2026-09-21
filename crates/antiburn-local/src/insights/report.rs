use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::{
    CacheEvidence, CoverageReason, EvidenceCoverage, EvidenceValue, SessionEvidence,
    SourceAcceptance, lookup_turn_pricing, strip_window_tag,
};
use crate::pricing::{ModelPricing, canonical_model_key};

use super::detectors::{self, DetectorFold, DetectorStatus, ReportCatalogs, complete};
use super::provider_incidents::{ProviderIncidentsAccumulator, ProviderIncidentsSection};
use super::quota::{QuotaPressureAccumulator, QuotaPressureSection};
use super::{CoverageBucket, DetectorId};

pub const MAX_EXAMPLES_PER_DETECTOR: usize = 3;
pub const MAX_REPORT_UNRECOGNIZED_TYPES: usize = 16;
pub const MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS: u16 = 10_000;
/// Maximum normalized turns retained for one session's effort comparison.
const MAX_TOKEN_BURN_COMPARISON_TURNS: usize = 4_096;
const UNRECOGNIZED_TYPES_DIAGNOSTIC: &str = "diagnostics.unrecognized_types";
const BASIS_POINTS_SCALE: u16 = 10_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DetectorCounts {
    /// Sessions with the facts needed to detect a finding.
    pub eligible: u64,
    /// Applicable sessions with a confirmed finding or clean result.
    pub assessed: u64,
    /// Applicable sessions with a confirmed finding.
    pub finding: u64,
    /// Applicable sessions with complete facts and no finding.
    pub clean: u64,
    /// Applicable sessions without enough evidence for an outcome.
    pub unavailable: u64,
    /// Sessions excluded by a proven detector denominator rule.
    pub not_applicable: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportWindow {
    pub start_epoch: i64,
    pub end_epoch: i64,
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;

/// Names one session without transcript content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionExample {
    pub agent: String,
    pub session_id: String,
}

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
                    && !detectors::observed(&evidence.subagents)
                        .is_some_and(|subagents| !subagents.delegated_models.is_empty())
                {
                    FactState::Unsupported
                } else {
                    state(&evidence.subagents)
                }
            }
            // `repeated_context`'s own `EvidenceValue` already carries the
            // accounting gate (`Unsupported` when neither cache-write nor
            // uncached-input accounting applies), the same way
            // `RecordLinkage` reads `previous_turn`.
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
            // No row fact for thread membership exists yet: the source
            // either promises it outright or it stays unsupported.
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

/// Returns the cache group's own state alongside its nested
/// `previous_turn` marker's state, or `None` when the cache group itself
/// is `Unsupported` (no `CacheEvidence` to read a marker from).
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

/// Returns the cache group's own state alongside its nested
/// `repeated_context` marker's state, or `None` when the cache group
/// itself is `Unsupported` (no `CacheEvidence` to read a marker from).
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
    }
}

/// A session is eligible for `detector` when every finding fact is not
/// `Unsupported`. Eligibility is the sole gate for a finding: a directly
/// observed finding needs no more than this.
pub fn eligible(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    source_supports_finding(detector, evidence.capabilities.source_format)
        && requirements(detector)
            .finding
            .iter()
            .all(|fact| fact.state(evidence) != FactState::Unsupported)
}

/// Mirrors the finding column of the maintained source matrix. Evidence fields
/// cannot opt an uncharacterized source into a detector contract.
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
            DetectorId::SessionsOverDepth
                | DetectorId::OverpoweredSubagents
                | DetectorId::OldModelUsage,
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

/// A session supports a clean claim for `detector` when every clean fact
/// is `Complete`. Only complete evidence can prove absence.
pub fn clean_facts_complete(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    // No current reader proves a full historical resource inventory.
    !matches!(
        detector,
        DetectorId::UnusedSkills | DetectorId::UnusedMcpServers | DetectorId::UnusedBuiltInTools
    ) && evidence.coverage == EvidenceCoverage::Complete
        && source_supports_clean(evidence.capabilities.source_format)
        && requirements(detector)
            .clean
            .iter()
            .all(|fact| fact.state(evidence) == FactState::Complete)
}

/// Complete session facts permit clean results only for characterized source contracts.
fn source_supports_clean(format: crate::analysis::SourceFormat) -> bool {
    use crate::analysis::SourceFormat;
    match format {
        SourceFormat::ClaudeJsonl
        | SourceFormat::CodexRolloutJsonl
        | SourceFormat::OpenCodeJsonl
        | SourceFormat::OpenCodeSqliteV2
        | SourceFormat::PiV3Jsonl
        | SourceFormat::CopilotCliJsonl => true,
        SourceFormat::CursorJsonl
        | SourceFormat::CursorCliAgentJsonl
        | SourceFormat::CursorCliStoreDb
        | SourceFormat::CursorChatStoreDb
        | SourceFormat::CursorIdeComposer
        | SourceFormat::CursorLegacyChatJson
        | SourceFormat::AntigravityJson
        | SourceFormat::AntigravityBrainJsonl
        | SourceFormat::AntigravityCascadeJson
        | SourceFormat::AntigravityWorkspaceChatJson
        | SourceFormat::AntigravitySqlite
        | SourceFormat::CopilotIdeChatJson
        | SourceFormat::ClineSessionJson
        | SourceFormat::ClineMessagesContractV1
        | SourceFormat::KiroSessionJson
        | SourceFormat::KiroChat
        | SourceFormat::KiroCliV2Bundle
        | SourceFormat::KiroCliV3Bundle
        | SourceFormat::KiroChatSaveExport
        | SourceFormat::AmpThreadJson
        | SourceFormat::AmpFileChanges
        | SourceFormat::WindsurfWorkspaceJson
        | SourceFormat::WindsurfMirrorJson
        | SourceFormat::WindsurfCascadeProtobuf
        | SourceFormat::DevinLocalSqlite
        | SourceFormat::Uncharacterized => false,
    }
}

/// A clean claim is out of reach for `detector` when a clean fact is
/// `Unsupported`. The source does not record what the claim needs.
pub fn clean_fact_unsupported(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    requirements(detector)
        .clean
        .iter()
        .any(|fact| fact.state(evidence) == FactState::Unsupported)
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

/// Summarizes unknown record vocabulary across the current cohort.
///
/// The session counts are not exclusive. The evidence string cap already limits each type.
/// The engine also bounds the diagnostic marker set, so both limit counts are best-effort.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnrecognizedRecords {
    pub types: BTreeSet<String>,
    pub types_truncated: bool,
    pub sessions_with_types: u64,
    pub inert_sessions: u64,
    pub evidence_bearing_sessions: u64,
    pub capped_sessions: u64,
    pub truncated_sessions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EfficiencyReport {
    pub context: ReportContext,
    pub assessed_sessions: u64,
    pub detectors: [DetectorCounts; DetectorId::COUNT],
    /// Distinct agents with findings, collected from the complete report cohort.
    pub finding_agents: [BTreeSet<String>; DetectorId::COUNT],
    /// Distinct agents with complete clean results in the report cohort.
    pub clean_agents: [BTreeSet<String>; DetectorId::COUNT],
    pub detector_statuses: [DetectorStatus; DetectorId::COUNT],
    pub quota_pressure: QuotaPressureSection,
    pub provider_incidents: ProviderIncidentsSection,
    pub catalog_revision: i64,
    pub coverage_reasons: BTreeMap<CoverageReason, u64>,
    pub unrecognized_records: UnrecognizedRecords,
    pub capability_gaps: BTreeMap<DetectorId, u64>,
    pub capability_gap_examples: BTreeMap<DetectorId, Vec<SessionExample>>,
    /// Token burn is estimated avoidable tokens divided by total used tokens.
    pub estimated_token_burn_basis_points: Option<u16>,
    /// Each detector's token burn uses the same ratio.
    pub detector_estimated_token_burn_basis_points: [Option<u16>; DetectorId::COUNT],
    token_burn_denominator: Option<u128>,
    non_resource_token_burn_by_session: Option<Vec<u128>>,
}

impl EfficiencyReport {
    /// Returns this report's burn percentage for complete attributed tokens.
    pub fn estimated_token_burn_for_attributed_tokens(&self, tokens: u128) -> Option<u16> {
        token_burn_basis_points(tokens, self.token_burn_denominator?)
    }

    /// Replaces resource-source burn while preserving the measured non-resource burn.
    pub fn estimated_token_burn_with_resource_tokens_by_session(
        &self,
        resource_tokens_by_session: Option<&[(usize, u128)]>,
    ) -> Option<u16> {
        let tokens = match (
            self.non_resource_token_burn_by_session.as_deref(),
            resource_tokens_by_session,
        ) {
            (Some(non_resource), Some(resources)) => {
                let mut total = non_resource
                    .iter()
                    .copied()
                    .try_fold(0_u128, u128::checked_add)?;
                for (index, resource) in resources {
                    let non_resource = non_resource.get(*index).copied().unwrap_or(0);
                    total = total.checked_add(resource.saturating_sub(non_resource))?;
                }
                total
            }
            (Some(non_resource), None) => non_resource
                .iter()
                .copied()
                .try_fold(0_u128, u128::checked_add)?,
            (None, Some(resources)) => resources
                .iter()
                .map(|(_, tokens)| *tokens)
                .try_fold(0_u128, u128::checked_add)?,
            (None, None) => return None,
        };
        self.estimated_token_burn_for_attributed_tokens(tokens)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenBurnSourceEvidence {
    // Collectors omit source groups when deferred loading prevents token attribution.
    /// The agent and installation scope used for window-level grouping.
    pub scope: String,
    /// The normalized source name used for window-level grouping.
    pub name: String,
    /// Definition tokens repeated across compatible main turns.
    pub replicated_tokens: u128,
    /// Whether this session invoked the source after loading it.
    pub invoked: bool,
    /// The priced cost of `replicated_tokens`, at the cache-read rate for
    /// each contributing turn's model. `None` when no contributing turn's
    /// model has a resolvable price; tokens and cost fail independently.
    pub replicated_cost_usd: Option<f64>,
}

/// One attributed assistant turn used only for report-time token estimates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBurnTurnEvidence {
    pub scope: String,
    pub model: String,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub ts_ms: Option<i64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// The subset of `cache_write_tokens` that the provider keeps for one hour.
    pub cache_write_1h_tokens: u64,
}

impl TokenBurnTurnEvidence {
    fn total_tokens(&self) -> Option<u128> {
        u128::from(self.input_tokens)
            .checked_add(u128::from(self.output_tokens))?
            .checked_add(u128::from(self.cache_read_tokens))?
            .checked_add(u128::from(self.cache_write_tokens))
    }

    fn context_tokens(&self) -> Option<u128> {
        u128::from(self.input_tokens)
            .checked_add(u128::from(self.cache_read_tokens))?
            .checked_add(u128::from(self.cache_write_tokens))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ComparisonCandidate {
    context_tokens: u128,
    output_tokens: u64,
    assumed_tokens: u128,
}

#[derive(Debug, Default)]
struct ComparisonGroup {
    lower_effort: Vec<ComparisonCandidate>,
    above_cap: Vec<ComparisonCandidate>,
}

/// Streams one session's report-time turn estimates.
///
/// The accumulator retains at most 4,096 normalized turns for exact observed
/// effort comparisons. After that limit, it drops the retained comparisons and
/// uses the accumulated effort-tier assumptions for the whole session.
#[derive(Debug)]
pub struct TokenBurnTurnAccumulator<'a> {
    catalogs: &'a ReportCatalogs,
    comparison_groups: BTreeMap<String, BTreeMap<String, ComparisonGroup>>,
    retained_comparison_turns: usize,
    comparison_bound_exceeded: bool,
    overthinking_complete: bool,
    overthinking_assumed: Option<u128>,
    overpowered_subagents: Option<u128>,
    old_model: Option<u128>,
    fast_mode: Option<u128>,
}

impl<'a> TokenBurnTurnAccumulator<'a> {
    pub fn new(catalogs: &'a ReportCatalogs) -> Self {
        Self {
            catalogs,
            comparison_groups: BTreeMap::new(),
            retained_comparison_turns: 0,
            comparison_bound_exceeded: false,
            overthinking_assumed: Some(0),
            overthinking_complete: true,
            overpowered_subagents: Some(0),
            old_model: Some(0),
            fast_mode: Some(0),
        }
    }

    pub fn observe(&mut self, turn: TokenBurnTurnEvidence) {
        let canonical_model = canonical_model_key(&turn.model);
        let family = model_family_from_canonical(&canonical_model);
        let effort = turn
            .effort
            .as_deref()
            .map(|value| value.trim().to_lowercase());
        let context_tokens = turn.context_tokens();

        if let Some(effort) = effort.as_deref() {
            if let Some(policy) = self.catalogs.families.get(&family) {
                let above_cap = policy.effort.above_cap.contains(effort);
                let lower_effort = policy.effort.recognized.contains(effort) && !above_cap;
                let assumed_tokens = if above_cap {
                    match effort {
                        "xhigh" => percentage_of_tokens(u128::from(turn.output_tokens), 20),
                        "max" | "ultra" => percentage_of_tokens(u128::from(turn.output_tokens), 35),
                        _ => percentage_of_tokens(u128::from(turn.output_tokens), 10),
                    }
                } else {
                    Some(0)
                };
                if above_cap {
                    self.overthinking_assumed =
                        checked_accumulate(self.overthinking_assumed, assumed_tokens);
                }
                if (above_cap || lower_effort)
                    && let Some(context_tokens) = context_tokens
                {
                    self.retain_comparison(
                        turn.scope.clone(),
                        canonical_model.clone(),
                        ComparisonCandidate {
                            context_tokens,
                            output_tokens: turn.output_tokens,
                            assumed_tokens: assumed_tokens.unwrap_or(0),
                        },
                        above_cap,
                    );
                }
            } else {
                self.overthinking_complete = false;
            }
        }

        if turn.scope == "delegated"
            && let Some(replacement) = premium_replacement(family, &canonical_model, self.catalogs)
        {
            self.overpowered_subagents = checked_accumulate(
                self.overpowered_subagents,
                priced_saving(&turn, &canonical_model, replacement),
            );
        }
        if let Some(replacement) = self
            .catalogs
            .model_replacements
            .entries
            .get(&canonical_model)
            && turn
                .ts_ms
                .is_some_and(|timestamp| timestamp >= replacement.available_since_ts_ms)
        {
            self.old_model = checked_accumulate(
                self.old_model,
                priced_saving(&turn, &canonical_model, &replacement.replacement),
            );
        }
        if turn.scope == "delegated"
            && turn
                .speed
                .as_deref()
                .is_some_and(|speed| speed.trim().eq_ignore_ascii_case("fast"))
        {
            self.fast_mode =
                checked_accumulate(self.fast_mode, fast_mode_saving(&turn, &canonical_model));
        }
    }

    fn retain_comparison(
        &mut self,
        scope: String,
        canonical_model: String,
        candidate: ComparisonCandidate,
        above_cap: bool,
    ) {
        if self.comparison_bound_exceeded {
            return;
        }
        if self.retained_comparison_turns == MAX_TOKEN_BURN_COMPARISON_TURNS {
            self.comparison_bound_exceeded = true;
            self.comparison_groups.clear();
            self.retained_comparison_turns = 0;
            return;
        }
        self.retained_comparison_turns += 1;
        let group = self
            .comparison_groups
            .entry(scope)
            .or_default()
            .entry(canonical_model)
            .or_default();
        if above_cap {
            group.above_cap.push(candidate);
        } else {
            group.lower_effort.push(candidate);
        }
    }

    pub fn finish_into(self, evidence: &mut SessionTokenBurnEvidence) {
        let model_overthinking = if !self.overthinking_complete {
            None
        } else if self.comparison_bound_exceeded {
            self.overthinking_assumed
        } else {
            self.comparison_groups
                .into_values()
                .flat_map(BTreeMap::into_values)
                .try_fold(0_u128, |total, group| {
                    total.checked_add(overthinking_group_tokens(group).tokens?)
                })
        };
        evidence.model_overthinking = model_overthinking;
        evidence.overpowered_subagents = self.overpowered_subagents;
        evidence.old_model = self.old_model;
        evidence.fast_mode = self.fast_mode;
    }

    #[cfg(test)]
    fn retained_comparison_turns(&self) -> usize {
        self.retained_comparison_turns
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverthinkingGroupEstimate {
    tokens: Option<u128>,
    operations: usize,
}

fn overthinking_group_tokens(mut group: ComparisonGroup) -> OverthinkingGroupEstimate {
    group
        .lower_effort
        .sort_unstable_by_key(|turn| (turn.output_tokens, turn.context_tokens));
    group.above_cap.sort_unstable_by_key(|turn| {
        (turn.output_tokens, turn.context_tokens, turn.assumed_tokens)
    });
    let mut active = BTreeMap::<u128, ComparisonCandidate>::new();
    let mut lower_index = 0;
    let mut total = Some(0_u128);
    let mut operations = 0;
    for turn in group.above_cap {
        while lower_index < group.lower_effort.len()
            && group.lower_effort[lower_index].output_tokens < turn.output_tokens
        {
            let candidate = group.lower_effort[lower_index];
            active
                .entry(candidate.context_tokens)
                .and_modify(|current| {
                    if candidate.output_tokens > current.output_tokens {
                        *current = candidate;
                    }
                })
                .or_insert(candidate);
            lower_index += 1;
            operations += 1;
        }
        let lower_bound = turn.context_tokens - turn.context_tokens / 5;
        let upper_bound = turn.context_tokens.saturating_add(turn.context_tokens / 4);
        let left = active.range(lower_bound..=turn.context_tokens).next_back();
        let right = active.range(turn.context_tokens..=upper_bound).next();
        operations += 2;
        let observed = [left, right]
            .into_iter()
            .flatten()
            .map(|(_, candidate)| {
                (
                    turn.context_tokens.abs_diff(candidate.context_tokens),
                    u64::MAX - candidate.output_tokens,
                    candidate.context_tokens,
                    u128::from(turn.output_tokens - candidate.output_tokens),
                )
            })
            .min_by_key(|(difference, reverse_output, context, _)| {
                (*difference, *reverse_output, *context)
            })
            .map(|(_, _, _, tokens)| tokens);
        total = checked_accumulate(total, observed.or(Some(turn.assumed_tokens)));
    }
    OverthinkingGroupEstimate {
        tokens: total,
        operations,
    }
}

fn checked_accumulate(total: Option<u128>, value: Option<u128>) -> Option<u128> {
    total?.checked_add(value?)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTokenBurnEvidence {
    /// All attributed input, output, cache-read, and cache-write tokens.
    pub total_tokens: Option<u128>,
    /// Cache token events that a context cap would remove.
    pub overdepth_avoidable_tokens: Option<u128>,
    /// Paid context token events beyond positive context growth.
    pub repeated_context_avoidable_tokens: Option<u128>,
    model_overthinking: Option<u128>,
    overpowered_subagents: Option<u128>,
    old_model: Option<u128>,
    fast_mode: Option<u128>,
    pub mcp_sources: Option<Vec<TokenBurnSourceEvidence>>,
    pub built_in_tool_sources: Option<Vec<TokenBurnSourceEvidence>>,
    pub skill_sources: Option<Vec<TokenBurnSourceEvidence>>,
    /// The pricing table generation active while this report ran, stamped
    /// once regardless of whether any source priced. `None` before the
    /// report-time pricing pass runs.
    pub pricing_revision: Option<String>,
}

impl SessionTokenBurnEvidence {
    pub fn from_session(evidence: &SessionEvidence) -> Self {
        let models = match &evidence.models {
            EvidenceValue::Partial {
                observed: models, ..
            }
            | EvidenceValue::Complete(models) => Some(models),
            _ => None,
        };
        let total_tokens = models.and_then(|models| {
            models.by_model.values().try_fold(0_u128, |total, tokens| {
                total
                    .checked_add(u128::from(tokens.input))?
                    .checked_add(u128::from(tokens.output))?
                    .checked_add(u128::from(tokens.cache_read))?
                    .checked_add(u128::from(tokens.cache_creation))
            })
        });
        let repeated_context_avoidable_tokens = models
            .filter(|models| models.unattributed_turns == 0)
            .and_then(|_| match &evidence.cache {
                EvidenceValue::Partial {
                    observed: cache, ..
                }
                | EvidenceValue::Complete(cache) => match &cache.repeated_context {
                    EvidenceValue::Partial {
                        observed: repeated, ..
                    }
                    | EvidenceValue::Complete(repeated) => {
                        Some(u128::from(repeated.repeated_tokens))
                    }
                    _ => None,
                },
                _ => None,
            });
        Self {
            total_tokens,
            repeated_context_avoidable_tokens,
            ..Self::default()
        }
    }
}

fn percentage_of_tokens(tokens: u128, percentage: u128) -> Option<u128> {
    tokens
        .checked_mul(percentage)?
        .checked_add(99)
        .map(|scaled| scaled / 100)
}

fn token_cost(tokens: &TokenBurnTurnEvidence, pricing: &ModelPricing) -> f64 {
    tokens.input_tokens as f64 * pricing.input_cost_per_token
        + tokens.output_tokens as f64 * pricing.output_cost_per_token
        + tokens.cache_read_tokens as f64 * pricing.cache_read_cost_per_token
        + tokens
            .cache_write_tokens
            .saturating_sub(tokens.cache_write_1h_tokens) as f64
            * pricing.cache_write_cost_per_token
        + tokens.cache_write_1h_tokens as f64 * pricing.input_cost_per_token * 2.0
}

fn cost_saving_tokens(
    turn: &TokenBurnTurnEvidence,
    actual: &ModelPricing,
    replacement: &ModelPricing,
) -> Option<u128> {
    let total_tokens = turn.total_tokens()?;
    let actual_cost = token_cost(turn, actual);
    let replacement_cost = token_cost(turn, replacement);
    if !actual_cost.is_finite()
        || !replacement_cost.is_finite()
        || actual_cost <= replacement_cost
        || actual_cost <= 0.0
    {
        return None;
    }
    let equivalent = total_tokens as f64 * (actual_cost - replacement_cost) / actual_cost;
    if !equivalent.is_finite() || equivalent <= 0.0 {
        return None;
    }
    Some((equivalent.round() as u128).clamp(1, total_tokens))
}

fn report_turn_pricing(
    model: &str,
    canonical_model: &str,
    speed: Option<&str>,
) -> Option<ModelPricing> {
    lookup_turn_pricing(model, speed).or_else(|| lookup_turn_pricing(canonical_model, speed))
}

fn priced_saving(
    turn: &TokenBurnTurnEvidence,
    canonical_model: &str,
    replacement: &str,
) -> Option<u128> {
    let replacement_canonical = canonical_model_key(replacement);
    report_turn_pricing(&turn.model, canonical_model, turn.speed.as_deref())
        .zip(report_turn_pricing(
            replacement,
            &replacement_canonical,
            turn.speed.as_deref(),
        ))
        .and_then(|(actual, replacement)| cost_saving_tokens(turn, &actual, &replacement))
}

fn model_family_from_canonical(canonical: &str) -> detectors::ModelFamily {
    if canonical.starts_with("claude-") {
        detectors::ModelFamily::Claude
    } else if canonical.starts_with("gpt-")
        || canonical.starts_with("o1")
        || canonical.starts_with("o3")
        || canonical.starts_with("o4")
    {
        detectors::ModelFamily::OpenAi
    } else if canonical.starts_with("gemini-") {
        detectors::ModelFamily::Google
    } else {
        detectors::ModelFamily::Unknown
    }
}

fn premium_replacement(
    family: detectors::ModelFamily,
    canonical_model: &str,
    catalogs: &ReportCatalogs,
) -> Option<&'static str> {
    let policy = &catalogs.families.get(&family)?.premium;
    if !policy.reviewed || !policy.is_premium(canonical_model) {
        return None;
    }
    match family {
        detectors::ModelFamily::OpenAi => Some("gpt-5.6-luna"),
        detectors::ModelFamily::Claude => Some("claude-sonnet-5"),
        detectors::ModelFamily::Google => Some("gemini-3.8-flash"),
        detectors::ModelFamily::Unknown => None,
    }
}

fn fast_mode_saving(turn: &TokenBurnTurnEvidence, canonical_model: &str) -> Option<u128> {
    let observed_model = strip_window_tag(&turn.model).trim();
    let observed_model = crate::pricing::normalize_model_key(observed_model);
    let standard_model = observed_model
        .strip_suffix("-fast")
        .unwrap_or(observed_model);
    let standard_canonical = canonical_model
        .strip_suffix("-fast")
        .unwrap_or(canonical_model);
    report_turn_pricing(standard_model, standard_canonical, Some("fast"))
        .zip(report_turn_pricing(
            standard_model,
            standard_canonical,
            None,
        ))
        .and_then(|(fast, standard)| cost_saving_tokens(turn, &fast, &standard))
}

#[derive(Default)]
struct SourceAggregate {
    invoked: bool,
    by_session: BTreeMap<usize, u128>,
}

#[derive(Default)]
struct SessionTokenContribution {
    overdepth: Option<u128>,
    repeated_context: Option<u128>,
    model_overthinking: Option<u128>,
    overpowered_subagents: Option<u128>,
    old_model: Option<u128>,
    fast_mode: Option<u128>,
}

#[derive(Default)]
struct TokenBurnAccumulator {
    total_complete: bool,
    total_tokens: u128,
    // Exact overlap needs one compact contribution per session and source/session pair.
    sessions: Vec<SessionTokenContribution>,
    sources: [BTreeMap<(String, String), SourceAggregate>; 3],
    source_complete: [bool; 3],
}

impl TokenBurnAccumulator {
    fn new() -> Self {
        Self {
            total_complete: true,
            source_complete: [true; 3],
            ..Self::default()
        }
    }

    fn observe(
        &mut self,
        token_evidence: SessionTokenBurnEvidence,
        findings: [bool; DetectorId::COUNT],
        source_eligible: [bool; 3],
    ) {
        if let Some(session_tokens) = token_evidence.total_tokens {
            if let Some(total_tokens) = self.total_tokens.checked_add(session_tokens) {
                self.total_tokens = total_tokens;
            } else {
                self.total_complete = false;
            }
        }
        let session_index = self.sessions.len();
        self.sessions.push(SessionTokenContribution {
            overdepth: if findings[DetectorId::SessionsOverDepth.index()] {
                token_evidence.overdepth_avoidable_tokens
            } else {
                Some(0)
            },
            repeated_context: if findings[DetectorId::CacheChurn.index()] {
                token_evidence.repeated_context_avoidable_tokens
            } else {
                Some(0)
            },
            model_overthinking: if findings[DetectorId::ModelOverthinking.index()] {
                token_evidence.model_overthinking
            } else {
                Some(0)
            },
            overpowered_subagents: if findings[DetectorId::OverpoweredSubagents.index()] {
                token_evidence.overpowered_subagents
            } else {
                Some(0)
            },
            old_model: if findings[DetectorId::OldModelUsage.index()] {
                token_evidence.old_model
            } else {
                Some(0)
            },
            fast_mode: if findings[DetectorId::OveruseOfFastMode.index()] {
                token_evidence.fast_mode
            } else {
                Some(0)
            },
        });

        let source_groups = [
            token_evidence.mcp_sources,
            token_evidence.built_in_tool_sources,
            token_evidence.skill_sources,
        ];
        for (index, sources) in source_groups.into_iter().enumerate() {
            let detector = [
                DetectorId::UnusedMcpServers,
                DetectorId::UnusedBuiltInTools,
                DetectorId::UnusedSkills,
            ][index];
            let Some(sources) = sources else {
                continue;
            };
            for source in sources {
                let aggregate = self.sources[index]
                    .entry((source.scope, source.name))
                    .or_default();
                aggregate.invoked |= source.invoked;
                if !source_eligible[index] || !findings[detector.index()] {
                    continue;
                }
                let entry = aggregate.by_session.entry(session_index).or_default();
                let Some(total) = entry.checked_add(source.replicated_tokens) else {
                    self.source_complete[index] = false;
                    continue;
                };
                *entry = total;
            }
        }
    }

    fn denominator(&self) -> Option<u128> {
        (self.total_complete && self.total_tokens > 0).then_some(self.total_tokens)
    }

    fn finish(
        self,
        statuses: &[DetectorStatus; DetectorId::COUNT],
    ) -> (
        Option<u16>,
        [Option<u16>; DetectorId::COUNT],
        Option<Vec<u128>>,
    ) {
        let mut numerators = [None; DetectorId::COUNT];
        let mut combined_by_session = vec![0_u128; self.sessions.len()];
        let mut source_combined_by_session = vec![0_u128; self.sessions.len()];
        let mut source_detector_by_session = vec![0_u128; self.sessions.len()];
        let denominator = self.denominator();
        let can_measure = denominator.is_some();

        for (detector, value_for) in [
            (
                DetectorId::SessionsOverDepth,
                (|session: &SessionTokenContribution| session.overdepth)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::CacheChurn,
                (|session: &SessionTokenContribution| session.repeated_context)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::ModelOverthinking,
                (|session: &SessionTokenContribution| session.model_overthinking)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::OverpoweredSubagents,
                (|session: &SessionTokenContribution| session.overpowered_subagents)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::OldModelUsage,
                (|session: &SessionTokenContribution| session.old_model)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::OveruseOfFastMode,
                (|session: &SessionTokenContribution| session.fast_mode)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
        ] {
            if !matches!(statuses[detector.index()], DetectorStatus::Findings(_)) {
                if matches!(statuses[detector.index()], DetectorStatus::Clean) {
                    numerators[detector.index()] = Some(0);
                }
                continue;
            }
            if !can_measure {
                continue;
            }
            if self
                .sessions
                .iter()
                .any(|session| value_for(session).is_none())
            {
                continue;
            }
            let Some(total) = self.sessions.iter().try_fold(0_u128, |total, session| {
                total.checked_add(value_for(session).unwrap_or(0))
            }) else {
                return (None, [None; DetectorId::COUNT], None);
            };
            numerators[detector.index()] = Some(total);
            for (index, session) in self.sessions.iter().enumerate() {
                combined_by_session[index] =
                    combined_by_session[index].max(value_for(session).unwrap_or(0));
            }
        }

        for (source_index, detector) in [
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools,
            DetectorId::UnusedSkills,
        ]
        .into_iter()
        .enumerate()
        {
            if matches!(statuses[detector.index()], DetectorStatus::Clean) {
                numerators[detector.index()] = Some(0);
                continue;
            }
            if !matches!(statuses[detector.index()], DetectorStatus::Findings(_))
                || !can_measure
                || !self.source_complete[source_index]
            {
                continue;
            }
            source_detector_by_session.fill(0);
            let mut qualifying_source = false;
            for aggregate in self.sources[source_index].values() {
                if aggregate.invoked {
                    continue;
                }
                qualifying_source = true;
                for (session, tokens) in &aggregate.by_session {
                    let Some(total) = source_detector_by_session[*session].checked_add(*tokens)
                    else {
                        return (None, [None; DetectorId::COUNT], None);
                    };
                    source_detector_by_session[*session] = total;
                }
            }
            if !qualifying_source {
                continue;
            }
            let Some(total) = source_detector_by_session
                .iter()
                .try_fold(0_u128, |total, value| total.checked_add(*value))
            else {
                return (None, [None; DetectorId::COUNT], None);
            };
            numerators[detector.index()] = Some(total);
            for (index, value) in source_detector_by_session.iter().enumerate() {
                let Some(total) = source_combined_by_session[index].checked_add(*value) else {
                    return (None, [None; DetectorId::COUNT], None);
                };
                source_combined_by_session[index] = total;
            }
        }
        let has_measured_non_resource_finding = [
            DetectorId::SessionsOverDepth,
            DetectorId::ModelOverthinking,
            DetectorId::OverpoweredSubagents,
            DetectorId::OldModelUsage,
            DetectorId::OveruseOfFastMode,
            DetectorId::CacheChurn,
        ]
        .into_iter()
        .any(|detector| {
            matches!(statuses[detector.index()], DetectorStatus::Findings(_))
                && numerators[detector.index()].is_some()
        });
        let non_resource_token_burn_by_session =
            has_measured_non_resource_finding.then(|| combined_by_session.clone());
        for (index, source_tokens) in source_combined_by_session.into_iter().enumerate() {
            combined_by_session[index] = combined_by_session[index].max(source_tokens);
        }

        let percentage =
            |numerator| denominator.and_then(|total| token_burn_basis_points(numerator, total));
        let assessed_sessions = self.sessions.len() as u64;
        let estimates = core::array::from_fn(|index| match &statuses[index] {
            DetectorStatus::Findings(findings) => {
                numerators[index].and_then(percentage).or_else(|| {
                    fallback_token_burn_basis_points(
                        DetectorId::ALL[index],
                        findings.finding_sessions,
                        assessed_sessions,
                    )
                })
            }
            DetectorStatus::Clean => Some(0),
            DetectorStatus::NotAssessed(_) => None,
        });
        let has_measured_finding = statuses.iter().enumerate().any(|(index, status)| {
            matches!(status, DetectorStatus::Findings(_)) && numerators[index].is_some()
        });
        let combined = if has_measured_finding {
            combined_by_session
                .into_iter()
                .try_fold(0_u128, u128::checked_add)
                .and_then(percentage)
        } else {
            estimates.iter().flatten().copied().max()
        };
        (combined, estimates, non_resource_token_burn_by_session)
    }
}

/// Returns a conservative proxy when a finding has no attributable token or pricing evidence.
/// The proxy scales a detector-specific workload share by the finding rate.
pub fn fallback_token_burn_basis_points(
    detector: DetectorId,
    finding_sessions: u64,
    assessed_sessions: u64,
) -> Option<u16> {
    if finding_sessions == 0 || assessed_sessions == 0 {
        return None;
    }
    let detector_share: u16 = match detector {
        DetectorId::SessionsOverDepth => 1_000,
        DetectorId::ModelOverthinking => 2_000,
        DetectorId::OverpoweredSubagents => 2_500,
        DetectorId::UnusedMcpServers
        | DetectorId::UnusedBuiltInTools
        | DetectorId::UnusedSkills => 500,
        DetectorId::OldModelUsage => 1_500,
        DetectorId::OveruseOfFastMode => 1_000,
        DetectorId::CacheChurn => 1_000,
    };
    let scaled = u128::from(detector_share)
        .checked_mul(u128::from(finding_sessions))?
        .checked_add(u128::from(assessed_sessions / 2))?
        / u128::from(assessed_sessions);
    Some(scaled.clamp(1, u128::from(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS)) as u16)
}

fn token_burn_basis_points(numerator: u128, denominator: u128) -> Option<u16> {
    numerator
        .checked_mul(u128::from(BASIS_POINTS_SCALE))
        .and_then(|scaled| scaled.checked_add(denominator / 2))
        .map(|rounded| {
            (rounded / denominator).min(u128::from(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS)) as u16
        })
}

pub struct EfficiencyReportAccumulator {
    assessed_sessions: u64,
    detectors: [DetectorCounts; DetectorId::COUNT],
    finding_agents: [BTreeSet<String>; DetectorId::COUNT],
    clean_agents: [BTreeSet<String>; DetectorId::COUNT],
    folds: [DetectorFold; DetectorId::COUNT],
    quota: QuotaPressureAccumulator,
    provider: ProviderIncidentsAccumulator,
    catalogs: ReportCatalogs,
    coverage_reasons: BTreeMap<CoverageReason, u64>,
    unrecognized_records: UnrecognizedRecords,
    capability_gaps: BTreeMap<DetectorId, u64>,
    capability_gap_examples: BTreeMap<DetectorId, Vec<SessionExample>>,
    actively_growing: u64,
    token_burn: TokenBurnAccumulator,
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
            detectors: [DetectorCounts::default(); DetectorId::COUNT],
            finding_agents: core::array::from_fn(|_| BTreeSet::new()),
            clean_agents: core::array::from_fn(|_| BTreeSet::new()),
            folds: core::array::from_fn(|_| DetectorFold::default()),
            quota: QuotaPressureAccumulator::default(),
            provider: ProviderIncidentsAccumulator::default(),
            catalogs,
            coverage_reasons: BTreeMap::new(),
            unrecognized_records: UnrecognizedRecords::default(),
            capability_gaps: BTreeMap::new(),
            capability_gap_examples: BTreeMap::new(),
            actively_growing: 0,
            token_burn: TokenBurnAccumulator::new(),
        }
    }

    /// Returns the immutable catalogs used by every reduction in this report.
    pub fn catalogs(&self) -> &ReportCatalogs {
        &self.catalogs
    }

    /// Observes one session from the ready-and-current cohort.
    pub fn observe_session(&mut self, evidence: SessionEvidence) {
        let token_evidence = SessionTokenBurnEvidence::from_session(&evidence);
        self.observe_session_with_token_burn(evidence, token_evidence);
    }

    /// Observes one session with report-time token attribution that is not
    /// part of the detector evidence contract.
    pub fn observe_session_with_token_burn(
        &mut self,
        evidence: SessionEvidence,
        mut token_evidence: SessionTokenBurnEvidence,
    ) {
        if let Some(sources) = &mut token_evidence.built_in_tool_sources {
            use crate::analysis::tool_catalog::{comparable_tool_name, situational_tools};
            let situational = situational_tools(&evidence.identity.agent);
            sources.retain(|source| {
                !situational
                    .iter()
                    .any(|name| comparable_tool_name(name) == comparable_tool_name(&source.name))
            });
        }
        let built_in_not_applicable =
            complete(&evidence.eligibility).is_some_and(|value| value.assistant_turns == 0);
        let source_eligible = [
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools,
            DetectorId::UnusedSkills,
        ]
        .map(|detector| {
            eligible(detector, &evidence)
                || detectors::source_assessable(detector, &evidence, Some(&token_evidence))
        });
        self.assessed_sessions += 1;
        if let EvidenceCoverage::Partial(reason) = evidence.coverage {
            *self.coverage_reasons.entry(reason).or_default() += 1;
        }
        self.observe_unrecognized_records(&evidence);
        if matches!(
            evidence.provenance.source_acceptance,
            SourceAcceptance::AcceptedPrefix { .. }
        ) {
            self.actively_growing += 1;
        }

        // The quota and provider-incidents sections read every cohort
        // session. They stay outside the nine-category eligibility loop
        // below.
        self.quota
            .observe_session(&evidence.identity, &evidence.quota_incidents);
        self.provider
            .observe_session(&evidence.identity, &evidence.provider_incidents);

        // Lazily allocate the identity example only if this session has a detector gap.
        let mut bounded_example: Option<SessionExample> = None;
        let mut findings = [false; DetectorId::COUNT];

        for detector in DetectorId::ALL {
            let counts = &mut self.detectors[detector.index()];
            if !detectors::in_denominator(detector, &evidence)
                || (detector == DetectorId::UnusedBuiltInTools && built_in_not_applicable)
            {
                counts.not_applicable += 1;
                continue;
            }
            let detector_eligible = eligible(detector, &evidence)
                || detectors::source_assessable(detector, &evidence, Some(&token_evidence));
            if !detector_eligible {
                counts.unavailable += 1;
                *self.capability_gaps.entry(detector).or_default() += 1;
                let examples = self.capability_gap_examples.entry(detector).or_default();
                if examples.len() < MAX_EXAMPLES_PER_DETECTOR {
                    let example = bounded_example.get_or_insert_with(|| SessionExample {
                        agent: evidence.identity.agent.clone(),
                        session_id: evidence.identity.session_id.clone(),
                    });
                    examples.push(example.clone());
                }
                continue;
            }

            counts.eligible += 1;
            let observation = detectors::evaluate_with_source_evidence(
                detector,
                &evidence,
                &self.catalogs,
                Some(&token_evidence),
            )
            .observation;
            match observation {
                detectors::Observation::Finding => {
                    self.finding_agents[detector.index()].insert(evidence.identity.agent.clone());
                    counts.finding += 1;
                    counts.assessed += 1;
                    findings[detector.index()] = true;
                }
                detectors::Observation::NoFinding if clean_facts_complete(detector, &evidence) => {
                    self.clean_agents[detector.index()].insert(evidence.identity.agent.clone());
                    counts.clean += 1;
                    counts.assessed += 1;
                }
                detectors::Observation::NoFinding
                | detectors::Observation::ContractIncomplete
                | detectors::Observation::SignalMissing => counts.unavailable += 1,
            }
            self.folds[detector.index()].observe(observation, &evidence);
        }
        self.token_burn
            .observe(token_evidence, findings, source_eligible);
    }

    fn observe_unrecognized_records(&mut self, evidence: &SessionEvidence) {
        let diagnostics = &evidence.diagnostics;
        if diagnostics.unrecognized_types.is_empty() {
            return;
        }

        self.unrecognized_records.sessions_with_types += 1;
        self.unrecognized_records.inert_sessions +=
            u64::from(diagnostics.records_unrecognized_inert > 0);
        self.unrecognized_records.evidence_bearing_sessions += u64::from(
            diagnostics
                .unusable_reasons
                .contains_key(&CoverageReason::UnrecognizedRecordType),
        );
        let capped = diagnostics
            .capped_collections
            .contains(UNRECOGNIZED_TYPES_DIAGNOSTIC);
        let truncated = diagnostics
            .truncated_strings
            .contains(UNRECOGNIZED_TYPES_DIAGNOSTIC);
        self.unrecognized_records.capped_sessions += u64::from(capped);
        self.unrecognized_records.truncated_sessions += u64::from(truncated);
        self.unrecognized_records.types_truncated |= capped;

        for kind in &diagnostics.unrecognized_types {
            if self.unrecognized_records.types.contains(kind) {
                continue;
            }
            if self.unrecognized_records.types.len() == MAX_REPORT_UNRECOGNIZED_TYPES {
                self.unrecognized_records.types_truncated = true;
                continue;
            }
            self.unrecognized_records.types.insert(kind.clone());
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
        let token_burn_denominator = self.token_burn.denominator();
        let (
            estimated_token_burn_basis_points,
            detector_estimates,
            non_resource_token_burn_by_session,
        ) = self.token_burn.finish(&detector_statuses);
        EfficiencyReport {
            context,
            assessed_sessions: self.assessed_sessions,
            detectors: self.detectors,
            finding_agents: self.finding_agents,
            clean_agents: self.clean_agents,
            detector_statuses,
            quota_pressure: self.quota.finish(),
            provider_incidents: self.provider.finish(),
            catalog_revision: self.catalogs.revision,
            coverage_reasons: self.coverage_reasons,
            unrecognized_records: self.unrecognized_records,
            capability_gaps: self.capability_gaps,
            capability_gap_examples: self.capability_gap_examples,
            estimated_token_burn_basis_points,
            detector_estimated_token_burn_basis_points: detector_estimates,
            token_burn_denominator,
            non_resource_token_burn_by_session,
        }
    }
}
