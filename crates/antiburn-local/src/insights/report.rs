use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::{
    CacheEvidence, CoverageReason, EvidenceCoverage, EvidenceValue, SessionEvidence,
    SourceAcceptance,
};

use super::detectors::{self, DetectorFold, DetectorStatus, ReportCatalogs};
use super::quota::{QuotaPressureAccumulator, QuotaPressureSection};
use super::{CoverageBucket, DetectorId};

pub const MAX_EXAMPLES_PER_DETECTOR: usize = 3;
pub const MAX_REPORT_UNRECOGNIZED_TYPES: usize = 16;
pub const MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS: u16 = 5_000;
const UNRECOGNIZED_TYPES_DIAGNOSTIC: &str = "diagnostics.unrecognized_types";
const BASIS_POINTS_SCALE: u16 = 10_000;

const fn detector_token_burn_share(detector: DetectorId) -> u16 {
    match detector {
        DetectorId::SessionsOverDepth => 1_000,
        DetectorId::ModelOverthinking => 2_500,
        DetectorId::OverpoweredSubagents => 3_500,
        DetectorId::UnusedMcpServers => 500,
        DetectorId::UnusedBuiltInTools => 500,
        DetectorId::UnusedSkills => 300,
        DetectorId::OldModelUsage => 2_000,
        DetectorId::OveruseOfFastMode => 2_000,
        DetectorId::CacheChurn => 2_500,
    }
}

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
    SkillMcpAttribution,
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
            // `evidence.context_sources` already reports `Unsupported`
            // when `capabilities.skill_mcp_attribution` is unset (the
            // sink's own gate), so this fact does not test the flag again.
            Self::SkillMcpAttribution => state(&evidence.context_sources),
            // Unlike `SkillMcpAttribution`, the sink never gates
            // `context_sources` on `capabilities.tool_definitions` — that
            // group stays supported (Claude, for example) while its
            // nested `tool_definitions` marker is always `Unsupported`.
            // This fact must test the flag itself.
            Self::ToolDefinitions => {
                if !evidence.capabilities.tool_definitions {
                    FactState::Unsupported
                } else {
                    state(&evidence.context_sources)
                }
            }
            Self::SubagentRelationships => state(&evidence.subagents),
            Self::DelegatedModels => {
                if !evidence.capabilities.subagent_models {
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
            finding: &[Fact::SkillMcpAttribution, Fact::ToolInvocations],
            clean: &[
                Fact::SkillMcpAttribution,
                Fact::ToolInvocations,
                Fact::Eligibility,
            ],
        },
        DetectorId::UnusedBuiltInTools => DetectorRequirements {
            finding: &[Fact::ToolDefinitions, Fact::ToolInvocations],
            clean: &[Fact::ToolDefinitions, Fact::ToolInvocations],
        },
        DetectorId::UnusedSkills => DetectorRequirements {
            finding: &[Fact::SkillMcpAttribution, Fact::ToolInvocations],
            clean: &[
                Fact::SkillMcpAttribution,
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
    requirements(detector)
        .finding
        .iter()
        .all(|fact| fact.state(evidence) != FactState::Unsupported)
}

/// A session supports a clean claim for `detector` when every clean fact
/// is `Complete`. Only complete evidence can prove absence.
pub fn clean_facts_complete(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    requirements(detector)
        .clean
        .iter()
        .all(|fact| fact.state(evidence) == FactState::Complete)
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
    pub detectors: [DetectorCounts; 9],
    pub detector_statuses: [DetectorStatus; 9],
    pub quota_pressure: QuotaPressureSection,
    pub catalog_revision: i64,
    pub coverage_reasons: BTreeMap<CoverageReason, u64>,
    pub unrecognized_records: UnrecognizedRecords,
    pub capability_gaps: BTreeMap<DetectorId, u64>,
    pub capability_gap_examples: BTreeMap<DetectorId, Vec<SessionExample>>,
    /// Token burn is estimated avoidable tokens divided by total used tokens.
    pub estimated_token_burn_basis_points: Option<u16>,
    /// Each detector's token burn uses the same ratio.
    pub detector_estimated_token_burn_basis_points: [Option<u16>; 9],
}

#[derive(Default)]
struct TokenBurnAccumulator {
    complete: bool,
    total_tokens: u128,
    combined_weighted_basis_points: u128,
    detector_weighted_basis_points: [u128; 9],
}

impl TokenBurnAccumulator {
    fn new() -> Self {
        Self {
            complete: true,
            ..Self::default()
        }
    }

    fn observe(&mut self, evidence: &SessionEvidence, findings: [bool; 9]) {
        let EvidenceValue::Complete(models) = &evidence.models else {
            self.complete = false;
            return;
        };
        if models.unattributed_turns != 0 {
            self.complete = false;
            return;
        }

        let Some(session_tokens) = models.by_model.values().try_fold(0_u128, |total, tokens| {
            total
                .checked_add(u128::from(tokens.input))?
                .checked_add(u128::from(tokens.output))?
                .checked_add(u128::from(tokens.cache_read))?
                .checked_add(u128::from(tokens.cache_creation))
        }) else {
            self.complete = false;
            return;
        };
        let Some(total_tokens) = self.total_tokens.checked_add(session_tokens) else {
            self.complete = false;
            return;
        };
        self.total_tokens = total_tokens;

        let mut remaining_basis_points = BASIS_POINTS_SCALE;
        for detector in DetectorId::ALL {
            if !findings[detector.index()] {
                continue;
            }
            let share = detector_token_burn_share(detector);
            let Some(weighted) = session_tokens.checked_mul(u128::from(share)) else {
                self.complete = false;
                return;
            };
            let detector_total = &mut self.detector_weighted_basis_points[detector.index()];
            let Some(total) = detector_total.checked_add(weighted) else {
                self.complete = false;
                return;
            };
            *detector_total = total;
            remaining_basis_points = u16::try_from(
                u32::from(remaining_basis_points) * u32::from(BASIS_POINTS_SCALE - share)
                    / u32::from(BASIS_POINTS_SCALE),
            )
            .expect("the complement product stays within the basis-point scale");
        }

        let combined_basis_points = (BASIS_POINTS_SCALE - remaining_basis_points)
            .min(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS);
        let Some(weighted) = session_tokens.checked_mul(u128::from(combined_basis_points)) else {
            self.complete = false;
            return;
        };
        let Some(total) = self.combined_weighted_basis_points.checked_add(weighted) else {
            self.complete = false;
            return;
        };
        self.combined_weighted_basis_points = total;
    }

    fn finish(self, covers_ready_cohort: bool) -> (Option<u16>, [Option<u16>; 9]) {
        if !covers_ready_cohort || !self.complete || self.total_tokens == 0 {
            return (None, [None; 9]);
        }
        let percentage = |weighted: u128| {
            u16::try_from(weighted / self.total_tokens)
                .ok()
                .map(|value| value.min(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS))
        };
        (
            percentage(self.combined_weighted_basis_points),
            core::array::from_fn(|index| percentage(self.detector_weighted_basis_points[index])),
        )
    }
}

fn finding_rate_token_burn_estimate(
    detectors: [DetectorCounts; 9],
    statuses: &[DetectorStatus; 9],
) -> (u16, [Option<u16>; 9]) {
    let estimates = core::array::from_fn(|index| match &statuses[index] {
        DetectorStatus::Findings(findings) if detectors[index].eligible > 0 => {
            let share = u128::from(detector_token_burn_share(DetectorId::ALL[index]));
            let finding_sessions = u128::from(findings.finding_sessions);
            let eligible = u128::from(detectors[index].eligible);
            Some(u16::try_from(share * finding_sessions / eligible).unwrap_or(0))
        }
        DetectorStatus::Clean => Some(0),
        DetectorStatus::Findings(_) | DetectorStatus::NotAssessed(_) => None,
    });
    let remaining = estimates
        .iter()
        .flatten()
        .fold(BASIS_POINTS_SCALE, |remaining, estimate| {
            u16::try_from(
                u32::from(remaining) * u32::from(BASIS_POINTS_SCALE - estimate)
                    / u32::from(BASIS_POINTS_SCALE),
            )
            .expect("the complement product stays within the basis-point scale")
        });
    (
        (BASIS_POINTS_SCALE - remaining).min(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS),
        estimates,
    )
}

pub struct EfficiencyReportAccumulator {
    assessed_sessions: u64,
    detectors: [DetectorCounts; 9],
    folds: [DetectorFold; 9],
    quota: QuotaPressureAccumulator,
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
            detectors: [DetectorCounts::default(); 9],
            folds: core::array::from_fn(|_| DetectorFold::default()),
            quota: QuotaPressureAccumulator::default(),
            catalogs,
            coverage_reasons: BTreeMap::new(),
            unrecognized_records: UnrecognizedRecords::default(),
            capability_gaps: BTreeMap::new(),
            capability_gap_examples: BTreeMap::new(),
            actively_growing: 0,
            token_burn: TokenBurnAccumulator::new(),
        }
    }

    /// Observes one session from the ready-and-current cohort.
    pub fn observe_session(&mut self, evidence: SessionEvidence) {
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

        // The quota section reads every cohort session. It stays
        // outside the nine-category eligibility loop below.
        self.quota
            .observe_session(&evidence.identity, &evidence.quota_incidents);

        // Lazily allocate the identity example only if this session has a detector gap.
        let mut bounded_example: Option<SessionExample> = None;
        let mut findings = [false; 9];

        for detector in DetectorId::ALL {
            let counts = &mut self.detectors[detector.index()];
            if !detectors::in_denominator(detector, &evidence) {
                counts.not_applicable += 1;
                continue;
            }
            if !eligible(detector, &evidence) {
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
            let observation = detectors::evaluate(detector, &evidence, &self.catalogs);
            match observation {
                detectors::Observation::Finding => {
                    counts.finding += 1;
                    counts.assessed += 1;
                    findings[detector.index()] = true;
                }
                detectors::Observation::NoFinding if clean_facts_complete(detector, &evidence) => {
                    counts.clean += 1;
                    counts.assessed += 1;
                }
                detectors::Observation::NoFinding
                | detectors::Observation::ContractIncomplete
                | detectors::Observation::SignalMissing => counts.unavailable += 1,
            }
            self.folds[detector.index()].observe(observation, &evidence);
        }
        self.token_burn.observe(&evidence, findings);
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
        let covers_ready_cohort = self.assessed_sessions == context.coverage.ready;
        let (mut estimated_token_burn_basis_points, mut detector_estimates) =
            self.token_burn.finish(covers_ready_cohort);
        if estimated_token_burn_basis_points.is_none() {
            let (fallback, fallback_detectors) =
                finding_rate_token_burn_estimate(self.detectors, &detector_statuses);
            estimated_token_burn_basis_points = Some(fallback);
            detector_estimates = fallback_detectors;
        }
        EfficiencyReport {
            context,
            assessed_sessions: self.assessed_sessions,
            detectors: self.detectors,
            detector_statuses,
            quota_pressure: self.quota.finish(),
            catalog_revision: self.catalogs.revision,
            coverage_reasons: self.coverage_reasons,
            unrecognized_records: self.unrecognized_records,
            capability_gaps: self.capability_gaps,
            capability_gap_examples: self.capability_gap_examples,
            estimated_token_burn_basis_points,
            detector_estimated_token_burn_basis_points: detector_estimates,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{
        ANALYZER_REVISION, ContextEvidence, EVIDENCE_SCHEMA_REVISION, EvidenceSource,
        FAST_SPEED_KEY, LoadedSource, ModelTokens, PARSER_REVISION, QuotaConfidence,
        QuotaHitSeverity, QuotaIncident, QuotaLimitKind, RepeatedContext,
        RepeatedContextAccounting, SessionEvidenceAccumulator, SessionQuotaEvidence,
        SignalCoverage, SourceCapabilities, SourceKind, TurnCounts, TurnFacts,
    };
    use crate::insights::detectors::{ModelFamily, ModelReplacementEntry, NotAssessedReason};
    use crate::insights::quota::QuotaPressureSection;

    fn evidence(session_id: &str) -> SessionEvidence {
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default())
    }

    /// The same claude evidence with one observed assistant turn, so
    /// the zero-work denominator exclusion does not remove the session
    /// from the absence detectors' eligible denominators. The one turn
    /// carries an effort and a speed value. Model Overthinking and
    /// Overuse of Fast Mode can read clean from it. Neither detector
    /// needs every eligible turn to carry the signal. Each needs only
    /// one turn to carry it.
    fn evidence_with_work(session_id: &str) -> SessionEvidence {
        let mut row = evidence(session_id);
        let EvidenceValue::Complete(eligibility) = &mut row.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 1;
        let EvidenceValue::Complete(models) = &mut row.models else {
            unreachable!()
        };
        models.effort_signal = SignalCoverage {
            eligible_turns: 1,
            present_turns: 1,
        };
        models.speed_signal = SignalCoverage {
            eligible_turns: 1,
            present_turns: 1,
        };
        row
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
    fn token_burn_estimates_weight_session_findings_by_all_token_kinds() {
        let mut finding = evidence_with_work("finding");
        finding.context = EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: 400_001,
            top_depth_examples: Vec::new(),
        });
        let EvidenceValue::Complete(models) = &mut finding.models else {
            unreachable!()
        };
        models.effort_tiers.insert(
            "max".to_owned(),
            TurnCounts {
                main_loop: 1,
                delegated: 0,
            },
        );
        models.by_model.insert(
            "claude-sonnet-4-5".to_owned(),
            ModelTokens {
                input: 100,
                output: 200,
                cache_read: 300,
                cache_creation: 400,
                ..ModelTokens::default()
            },
        );

        let mut clean = evidence_with_work("clean");
        let EvidenceValue::Complete(models) = &mut clean.models else {
            unreachable!()
        };
        models.by_model.insert(
            "claude-sonnet-4-5".to_owned(),
            ModelTokens {
                input: 250,
                output: 250,
                cache_read: 250,
                cache_creation: 250,
                ..ModelTokens::default()
            },
        );

        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(finding);
        accumulator.observe_session(clean);
        let report = accumulator.finish(context(CoverageCounts {
            ready: 2,
            discovered: 2,
            ..CoverageCounts::default()
        }));

        assert_eq!(report.estimated_token_burn_basis_points, Some(1_625));
        assert_eq!(
            report.detector_estimated_token_burn_basis_points
                [DetectorId::SessionsOverDepth.index()],
            Some(500)
        );
        assert_eq!(
            report.detector_estimated_token_burn_basis_points
                [DetectorId::ModelOverthinking.index()],
            Some(1_250)
        );
        assert_eq!(
            report.detector_estimated_token_burn_basis_points[DetectorId::UnusedSkills.index()],
            Some(0)
        );
    }

    #[test]
    fn token_burn_estimates_fall_back_to_finding_rates_for_incomplete_tokens() {
        let complete = evidence_with_work("complete");
        let mut unattributed = evidence_with_work("unattributed");
        unattributed.context = EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: 400_001,
            top_depth_examples: Vec::new(),
        });
        let EvidenceValue::Complete(models) = &mut unattributed.models else {
            unreachable!()
        };
        models.unattributed_turns = 1;

        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(complete);
        accumulator.observe_session(unattributed);
        let report = accumulator.finish(context(CoverageCounts {
            ready: 2,
            discovered: 2,
            ..CoverageCounts::default()
        }));

        assert_eq!(
            report.detector_estimated_token_burn_basis_points
                [DetectorId::SessionsOverDepth.index()],
            Some(500)
        );
        assert!(report.estimated_token_burn_basis_points.unwrap() >= 500);
    }

    #[test]
    fn token_burn_estimates_fall_back_when_the_ready_cohort_is_incomplete() {
        let mut evidence = evidence_with_work("observed");
        let EvidenceValue::Complete(models) = &mut evidence.models else {
            unreachable!()
        };
        models.by_model.insert(
            "claude-sonnet-4-5".to_owned(),
            ModelTokens {
                input: 100,
                ..ModelTokens::default()
            },
        );

        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence);
        let report = accumulator.finish(context(CoverageCounts {
            ready: 2,
            discovered: 2,
            ..CoverageCounts::default()
        }));

        assert_eq!(report.estimated_token_burn_basis_points, Some(0));
    }

    #[test]
    fn detector_token_burn_shares_match_the_fixed_policy() {
        assert_eq!(
            DetectorId::ALL.map(detector_token_burn_share),
            [1_000, 2_500, 3_500, 500, 500, 300, 2_000, 2_000, 2_500]
        );
    }

    #[test]
    fn combined_token_burn_is_capped_per_session() {
        let mut evidence = evidence_with_work("finding");
        let EvidenceValue::Complete(models) = &mut evidence.models else {
            unreachable!()
        };
        models.by_model.insert(
            "claude-sonnet-4-5".to_owned(),
            ModelTokens {
                input: 100,
                ..ModelTokens::default()
            },
        );
        let mut findings = [false; 9];
        findings[DetectorId::SessionsOverDepth.index()] = true;
        findings[DetectorId::ModelOverthinking.index()] = true;
        findings[DetectorId::OverpoweredSubagents.index()] = true;

        let mut token_burn = TokenBurnAccumulator::new();
        token_burn.observe(&evidence, findings);
        let (combined, per_detector) = token_burn.finish(true);

        assert_eq!(combined, Some(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS));
        assert_eq!(
            per_detector[DetectorId::OverpoweredSubagents.index()],
            Some(3_500)
        );
    }

    #[test]
    fn unrecognized_records_summarizes_the_cohort() {
        let mut inert = evidence("inert");
        inert
            .diagnostics
            .unrecognized_types
            .insert("alpha".to_owned());
        inert.diagnostics.records_unrecognized_inert = 1;

        let mut bearing = evidence("bearing");
        bearing
            .diagnostics
            .unrecognized_types
            .insert("beta".to_owned());
        bearing
            .diagnostics
            .unusable_reasons
            .insert(CoverageReason::UnrecognizedRecordType, 1);

        let mut mixed = evidence("mixed");
        mixed
            .diagnostics
            .unrecognized_types
            .insert("gamma".to_owned());
        mixed.diagnostics.records_unrecognized_inert = 1;
        mixed
            .diagnostics
            .unusable_reasons
            .insert(CoverageReason::UnrecognizedRecordType, 1);

        let mut capped = evidence("capped");
        capped
            .diagnostics
            .unrecognized_types
            .insert("delta".to_owned());
        capped.diagnostics.records_unrecognized_inert = 1;
        capped
            .diagnostics
            .capped_collections
            .insert(UNRECOGNIZED_TYPES_DIAGNOSTIC.to_owned());

        let mut truncated = evidence("truncated");
        truncated
            .diagnostics
            .unrecognized_types
            .insert("epsilon".to_owned());
        truncated.diagnostics.records_unrecognized_inert = 1;
        truncated
            .diagnostics
            .truncated_strings
            .insert(UNRECOGNIZED_TYPES_DIAGNOSTIC.to_owned());

        let mut accumulator = EfficiencyReportAccumulator::new();
        for row in [inert, bearing, mixed, capped, truncated] {
            accumulator.observe_session(row);
        }
        let summary = accumulator
            .finish(context(CoverageCounts::default()))
            .unrecognized_records;

        assert_eq!(summary.sessions_with_types, 5);
        assert_eq!(summary.inert_sessions, 4);
        assert_eq!(summary.evidence_bearing_sessions, 2);
        assert_eq!(summary.capped_sessions, 1);
        assert_eq!(summary.truncated_sessions, 1);
        assert!(summary.types_truncated);
        assert_eq!(
            summary.types,
            BTreeSet::from([
                "alpha".to_owned(),
                "beta".to_owned(),
                "delta".to_owned(),
                "epsilon".to_owned(),
                "gamma".to_owned(),
            ])
        );
    }

    #[test]
    fn the_report_type_set_is_capped() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        for index in 0..=MAX_REPORT_UNRECOGNIZED_TYPES {
            let mut row = evidence(&format!("session-{index}"));
            row.diagnostics
                .unrecognized_types
                .insert(format!("type-{index:02}"));
            row.diagnostics.records_unrecognized_inert = 1;
            accumulator.observe_session(row);
        }

        let summary = accumulator
            .finish(context(CoverageCounts::default()))
            .unrecognized_records;
        assert_eq!(summary.types.len(), MAX_REPORT_UNRECOGNIZED_TYPES);
        assert!(summary.types_truncated);
        assert_eq!(summary.types.first().map(String::as_str), Some("type-00"));
        assert_eq!(summary.types.last().map(String::as_str), Some("type-15"));

        let mut session_capped = evidence("session-capped");
        for index in 0..MAX_REPORT_UNRECOGNIZED_TYPES {
            session_capped
                .diagnostics
                .unrecognized_types
                .insert(format!("session-type-{index:02}"));
        }
        session_capped
            .diagnostics
            .capped_collections
            .insert(UNRECOGNIZED_TYPES_DIAGNOSTIC.to_owned());
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(session_capped);
        let summary = accumulator
            .finish(context(CoverageCounts::default()))
            .unrecognized_records;
        assert!(summary.types_truncated);
        assert_eq!(summary.capped_sessions, 1);
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
        // Either flag admits the session, but only a fast-tier source
        // can read the evidence the rule needs: a service-tier-only
        // source is eligible yet reports the contract gap instead of
        // a verdict the evidence cannot support.
        let cases = [
            (true, false, DetectorStatus::Clean),
            (
                false,
                true,
                DetectorStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete),
            ),
        ];
        for (fast_tier, service_tier, expected_status) in cases {
            let mut row = evidence_with_work("mode");
            row.capabilities.fast_tier = fast_tier;
            row.capabilities.service_tier = service_tier;
            let mut accumulator = EfficiencyReportAccumulator::new();
            accumulator.observe_session(row);
            let report = accumulator.finish(context(CoverageCounts::default()));

            assert_eq!(
                report.detectors[DetectorId::OveruseOfFastMode.index()].eligible,
                1
            );
            assert_eq!(
                report.detector_statuses[DetectorId::OveruseOfFastMode.index()],
                expected_status
            );
        }
    }

    #[test]
    fn timestampless_catalogued_turns_report_the_contract_gap_at_report_level() {
        // Catalogued-model turns without an observed timestamp cannot
        // be placed relative to the replacement's availability, so
        // Old Model Usage must surface the contract gap, never clean.
        let mut catalogs = ReportCatalogs::default();
        catalogs.model_replacements.entries.insert(
            "old-model-1".to_owned(),
            ModelReplacementEntry {
                replacement: "new-model-2".to_owned(),
                available_since_ts_ms: 100,
                rationale: "test rule".to_owned(),
                source_url: "https://example.invalid/old-model-1".to_owned(),
            },
        );
        let mut row = evidence("timestampless");
        let EvidenceValue::Complete(models) = &mut row.models else {
            unreachable!()
        };
        models.by_model.insert(
            "old-model-1".to_owned(),
            ModelTokens {
                turns: 4,
                last_ts_ms: 0,
                ..ModelTokens::default()
            },
        );
        let mut accumulator = EfficiencyReportAccumulator::with_catalogs(catalogs);
        accumulator.observe_session(row);
        let report = accumulator.finish(context(CoverageCounts::default()));

        assert_eq!(
            report.detector_statuses[DetectorId::OldModelUsage.index()],
            DetectorStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete)
        );
    }

    #[test]
    fn group_states_separate_eligibility_from_assessment() {
        let mut unsupported = evidence_with_work("unsupported");
        unsupported.models = EvidenceValue::Unsupported;
        let mut partial = evidence_with_work("partial");
        partial.models = match partial.models {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::AttributionIncomplete,
            },
            _ => unreachable!(),
        };
        let complete = evidence_with_work("complete");
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(unsupported);
        accumulator.observe_session(partial);
        accumulator.observe_session(complete);
        let report = accumulator.finish(context(CoverageCounts::default()));
        let counts = report.detectors[DetectorId::ModelOverthinking.index()];

        assert_eq!(counts.eligible, 2);
        assert_eq!(counts.assessed, 1);
        assert_eq!(counts.clean, 1);
        assert_eq!(counts.unavailable, 2);
    }

    #[test]
    fn claude_matrix_has_the_exact_eligible_detector_set() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence_with_work("matrix"));
        let report = accumulator.finish(context(CoverageCounts::default()));
        let eligible: Vec<_> = DetectorId::ALL
            .into_iter()
            .filter(|detector| report.detectors[detector.index()].eligible == 1)
            .collect();

        assert_eq!(
            eligible,
            vec![
                DetectorId::SessionsOverDepth,
                DetectorId::ModelOverthinking,
                DetectorId::OverpoweredSubagents,
                DetectorId::UnusedMcpServers,
                DetectorId::UnusedSkills,
                DetectorId::OldModelUsage,
                DetectorId::OveruseOfFastMode,
                DetectorId::CacheChurn,
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
        // counts and never through observe_session. This test pins
        // that data-path property: the same cohort with and without
        // the denominator-only rows must produce identical detector
        // counts and statuses. The population-side exclusion is
        // CH-010's job, proven by the population tests in
        // apps/desktop/src-tauri/src/insights_report.rs.
        let mut baseline = EfficiencyReportAccumulator::new();
        baseline.observe_session(evidence("cohort-only"));
        let mut baseline_coverage = CoverageCounts::default();
        baseline_coverage.observe(CoverageBucket::Ready, 1);
        let baseline_report = baseline.finish(context(baseline_coverage));

        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence("cohort-only"));
        let mut coverage = CoverageCounts::default();
        coverage.observe(CoverageBucket::Ready, 1);
        coverage.observe(CoverageBucket::UnknownStart, 4);
        coverage.observe(CoverageBucket::Pending, 5);
        let report = accumulator.finish(context(coverage));

        assert_eq!(report.assessed_sessions, 1);
        assert_eq!(report.detectors, baseline_report.detectors);
        assert_eq!(report.detector_statuses, baseline_report.detector_statuses);
        for detector in DetectorId::ALL {
            let counts = report.detectors[detector.index()];
            assert!(counts.eligible <= report.assessed_sessions);
            assert!(counts.assessed <= counts.eligible);
            assert_eq!(counts.assessed, counts.finding + counts.clean);
            assert_eq!(
                report.assessed_sessions,
                counts.finding + counts.clean + counts.unavailable + counts.not_applicable
            );
        }
        assert_eq!(report.context.coverage.unknown_start, 4);
        assert_eq!(report.context.coverage.pending, 5);
    }

    #[test]
    fn each_detector_produces_exactly_one_status_for_the_claude_matrix() {
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence_with_work("matrix"));
        let report = accumulator.finish(context(CoverageCounts::default()));

        // Complete empty evidence proves absence for the eligible
        // detectors that can express their rule.
        for detector in [
            DetectorId::SessionsOverDepth,
            DetectorId::ModelOverthinking,
            DetectorId::OverpoweredSubagents,
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedSkills,
            DetectorId::OveruseOfFastMode,
            DetectorId::CacheChurn,
            // The reviewed production registry has entries, and this
            // session carries zero observed models, so no catalogued
            // model can have run.
            DetectorId::OldModelUsage,
        ] {
            assert_eq!(
                report.detector_statuses[detector.index()],
                DetectorStatus::Clean
            );
        }
        // The capability gap stays not assessed with a structured reason.
        assert_eq!(
            report.detector_statuses[DetectorId::UnusedBuiltInTools.index()],
            DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing)
        );
    }

    #[test]
    fn an_all_idle_cohort_cannot_read_clean_for_the_absence_detectors() {
        // Every session carries zero assistant turns: none can support
        // a finding, so none may support absence either. The sessions
        // stay out of the eligible denominator entirely.
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence("idle-1"));
        accumulator.observe_session(evidence("idle-2"));
        let report = accumulator.finish(context(CoverageCounts::default()));

        for detector in [DetectorId::UnusedMcpServers, DetectorId::UnusedSkills] {
            let counts = report.detectors[detector.index()];
            assert_eq!(counts.eligible, 0);
            assert_eq!(counts.not_applicable, 2);
            assert_eq!(counts.unavailable, 0);
            assert_eq!(
                report.detector_statuses[detector.index()],
                DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing)
            );
        }
    }

    #[test]
    fn a_partial_zero_turn_session_blocks_clean_for_the_absence_detectors() {
        // The session's work-bearing records were lost: eligibility
        // degraded to partial and the surviving records observe zero
        // assistant turns. Absence read from partial evidence is
        // untrustworthy, so the session must stay in the eligible
        // denominator as unassessed and block clean — it must not
        // vanish and let the cohort read clean.
        let mut degraded = evidence("partial-idle");
        degraded.eligibility = match degraded.eligibility {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::IncompleteTail,
            },
            _ => unreachable!(),
        };
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence_with_work("working"));
        accumulator.observe_session(degraded);
        let report = accumulator.finish(context(CoverageCounts::default()));

        for detector in [DetectorId::UnusedMcpServers, DetectorId::UnusedSkills] {
            let counts = report.detectors[detector.index()];
            assert_eq!(counts.eligible, 2, "{detector:?}");
            assert_eq!(counts.assessed, 1, "{detector:?}");
            assert_eq!(counts.clean, 1, "{detector:?}");
            assert_eq!(counts.unavailable, 1, "{detector:?}");
            assert_eq!(
                report.detector_statuses[detector.index()],
                DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
                "{detector:?}"
            );
        }
    }

    /// Complete claude evidence with every capability force-set and one
    /// observed assistant turn, so every detector is eligible and fully
    /// assessed before degradation.
    fn complete_row(session_id: &str) -> SessionEvidence {
        let mut row = evidence_with_work(session_id);
        row.capabilities = SourceCapabilities {
            request_context_tokens: true,
            cache_write_tokens: true,
            timestamps_and_order: true,
            tool_invocations: true,
            skill_mcp_attribution: true,
            tool_definitions: true,
            model_identity: true,
            token_classes: true,
            reasoning_effort_tier: true,
            fast_tier: true,
            service_tier: true,
            subagent_relationships: true,
            subagent_models: true,
            compaction_boundaries: true,
            thread_identity: true,
            record_identity: true,
            linear_record_order: true,
            quota_incidents: true,
            harness_version: true,
        };
        row
    }

    fn to_partial<T>(slot: &mut EvidenceValue<T>) {
        let value = std::mem::replace(slot, EvidenceValue::Unsupported);
        let EvidenceValue::Complete(observed) = value else {
            panic!("the complete row must carry complete evidence");
        };
        *slot = EvidenceValue::Partial {
            observed,
            reason: CoverageReason::MalformedRecord,
        };
    }

    /// Degrades one fact's backing evidence from `Complete` to `Partial`.
    /// `ThreadMembership` has no partial state (`Fact::state` maps its
    /// capability flag straight to `Complete`/`Unsupported`), so this
    /// unsets the capability instead: the fact still stops being
    /// `Complete`, which is all a clean-only degrade needs to prove.
    fn degrade_fact_to_partial(row: &mut SessionEvidence, fact: Fact) {
        match fact {
            Fact::MainLoopContext => to_partial(&mut row.context),
            Fact::ModelIdentity | Fact::EffortSignal | Fact::SpeedSignal => {
                to_partial(&mut row.models)
            }
            Fact::ToolInvocations => to_partial(&mut row.tools),
            Fact::SkillMcpAttribution | Fact::ToolDefinitions => {
                to_partial(&mut row.context_sources)
            }
            Fact::SubagentRelationships | Fact::DelegatedModels => to_partial(&mut row.subagents),
            Fact::RepeatedContextAccounting | Fact::RecordLinkage => to_partial(&mut row.cache),
            Fact::ThreadMembership => row.capabilities.thread_identity = false,
            Fact::CompactionBoundaries => to_partial(&mut row.compactions),
            Fact::TimeRange => to_partial(&mut row.time_range),
            Fact::Eligibility => to_partial(&mut row.eligibility),
        }
    }

    /// Sets one fact's state to `Unsupported`, either by clearing the
    /// capability the fact tests directly or by unsupporting its
    /// backing evidence group.
    fn degrade_fact_to_unsupported(row: &mut SessionEvidence, fact: Fact) {
        match fact {
            Fact::MainLoopContext => row.context = EvidenceValue::Unsupported,
            Fact::ModelIdentity => row.models = EvidenceValue::Unsupported,
            Fact::EffortSignal => row.capabilities.reasoning_effort_tier = false,
            Fact::SpeedSignal => {
                row.capabilities.fast_tier = false;
                row.capabilities.service_tier = false;
            }
            Fact::ToolInvocations => row.tools = EvidenceValue::Unsupported,
            Fact::SkillMcpAttribution => row.context_sources = EvidenceValue::Unsupported,
            Fact::ToolDefinitions => row.capabilities.tool_definitions = false,
            Fact::SubagentRelationships => row.subagents = EvidenceValue::Unsupported,
            Fact::DelegatedModels => row.capabilities.subagent_models = false,
            // `RepeatedContextAccounting` and `RecordLinkage` both read a
            // marker nested inside `CacheEvidence`: unsupporting the whole
            // group is the only way to force either marker `Unsupported`,
            // since the marker's state comes from the stored evidence, not
            // a capability flag re-read at fact-evaluation time.
            Fact::RepeatedContextAccounting | Fact::RecordLinkage => {
                row.cache = EvidenceValue::Unsupported
            }
            Fact::ThreadMembership => row.capabilities.thread_identity = false,
            Fact::CompactionBoundaries => row.compactions = EvidenceValue::Unsupported,
            Fact::TimeRange => row.time_range = EvidenceValue::Unsupported,
            Fact::Eligibility => row.eligibility = EvidenceValue::Unsupported,
        }
    }

    fn status_for_with_catalogs(
        row: SessionEvidence,
        detector: DetectorId,
        catalogs: ReportCatalogs,
    ) -> DetectorStatus {
        let mut accumulator = EfficiencyReportAccumulator::with_catalogs(catalogs);
        accumulator.observe_session(row);
        let report = accumulator.finish(context(CoverageCounts::default()));
        report.detector_statuses[detector.index()].clone()
    }

    fn status_for(row: SessionEvidence, detector: DetectorId) -> DetectorStatus {
        status_for_with_catalogs(row, detector, ReportCatalogs::default())
    }

    #[test]
    fn clean_facts_are_a_superset_of_finding_facts() {
        for detector in DetectorId::ALL {
            let required = requirements(detector);
            for fact in required.finding {
                assert!(
                    required.clean.contains(fact),
                    "{detector:?}'s clean facts must contain finding fact {fact:?}"
                );
            }
        }
    }

    #[test]
    fn degrading_a_clean_only_fact_to_partial_blocks_clean() {
        // (a) Every clean fact, degraded to Partial (or unsupported for
        // ThreadMembership, which has no partial state), must stop the
        // detector from reading Clean.
        for detector in DetectorId::ALL {
            let baseline = status_for(complete_row("complete"), detector);
            if matches!(detector, DetectorId::UnusedBuiltInTools) {
                // Unused Built-In Tools carries a permanent contract
                // gap independent of fact degradation: it has no
                // definition-name payload yet.
                assert_eq!(
                    baseline,
                    DetectorStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete),
                    "baseline for {detector:?}"
                );
            } else {
                assert_eq!(
                    baseline,
                    DetectorStatus::Clean,
                    "baseline for {detector:?} must read clean so the degraded assertion distinguishes"
                );
            }

            for fact in requirements(detector).clean {
                let mut row = complete_row("degraded");
                degrade_fact_to_partial(&mut row, *fact);
                let status = status_for(row, detector);
                assert_ne!(
                    status,
                    DetectorStatus::Clean,
                    "degrading {fact:?} for {detector:?} must not read clean"
                );
            }
        }
    }

    #[test]
    fn unsupporting_a_finding_fact_makes_the_session_ineligible() {
        // (b) Every finding fact, set to Unsupported, must make the
        // session ineligible for that detector.
        for detector in DetectorId::ALL {
            assert!(
                eligible(detector, &complete_row("complete")),
                "the undegraded row must be eligible for {detector:?}"
            );
            for fact in requirements(detector).finding {
                let mut row = complete_row("degraded");
                degrade_fact_to_unsupported(&mut row, *fact);
                assert!(
                    !eligible(detector, &row),
                    "unsupporting {fact:?} for {detector:?} must clear eligibility"
                );
            }
        }
    }

    /// Builds evidence carrying a concrete finding for `detector`, using
    /// `complete_row` as the base so every fact starts `Complete`.
    /// `UnusedBuiltInTools` and `OverpoweredSubagents` are absent: the
    /// former can never produce a finding (permanent contract gap), and
    /// the latter's clean facts equal its finding facts, so it has no
    /// clean-only fact left to degrade in test (c) below.
    fn trigger_finding(detector: DetectorId, catalogs: &ReportCatalogs) -> SessionEvidence {
        let mut row = complete_row("finding");
        match detector {
            DetectorId::SessionsOverDepth => {
                let EvidenceValue::Complete(context) = &mut row.context else {
                    unreachable!()
                };
                context.max_request_context_tokens = catalogs.depth_cap_tokens + 1;
            }
            DetectorId::ModelOverthinking => {
                let EvidenceValue::Complete(models) = &mut row.models else {
                    unreachable!()
                };
                let (family, policy) = catalogs
                    .families
                    .iter()
                    .find(|(_, policy)| !policy.effort.above_cap.is_empty())
                    .expect("caller must supply a family with an above-cap effort tier");
                let tier = policy.effort.above_cap.first().unwrap().clone();
                let model = match family {
                    ModelFamily::Claude => "claude-sonnet-4-6",
                    ModelFamily::OpenAi => "gpt-5.6",
                    ModelFamily::Unknown => unreachable!("Unknown family never recognizes a tier"),
                };
                // A `by_model` entry establishes the family as present,
                // which the reviewed policy needs to classify `tier` as
                // above the cap.
                models
                    .by_model
                    .insert(model.to_owned(), ModelTokens::default());
                models.effort_tiers.insert(
                    tier,
                    TurnCounts {
                        main_loop: 1,
                        delegated: 0,
                    },
                );
            }
            DetectorId::UnusedMcpServers => {
                let EvidenceValue::Complete(sources) = &mut row.context_sources else {
                    unreachable!()
                };
                sources.mcp_servers.insert(
                    "server-a".to_owned(),
                    LoadedSource {
                        description: None,
                        invoked: false,
                        origin: EvidenceValue::Unsupported,
                    },
                );
            }
            DetectorId::UnusedSkills => {
                let EvidenceValue::Complete(sources) = &mut row.context_sources else {
                    unreachable!()
                };
                sources.skills.insert(
                    "skill-a".to_owned(),
                    LoadedSource {
                        description: None,
                        invoked: false,
                        origin: EvidenceValue::Unsupported,
                    },
                );
            }
            DetectorId::OldModelUsage => {
                let EvidenceValue::Complete(models) = &mut row.models else {
                    unreachable!()
                };
                let (model, replacement) = catalogs
                    .model_replacements
                    .entries
                    .iter()
                    .next()
                    .expect("caller must supply a non-empty replacement catalog");
                models.by_model.insert(
                    model.clone(),
                    ModelTokens {
                        turns: 4,
                        last_ts_ms: replacement.available_since_ts_ms + 1,
                        ..ModelTokens::default()
                    },
                );
            }
            DetectorId::OveruseOfFastMode => {
                let EvidenceValue::Complete(models) = &mut row.models else {
                    unreachable!()
                };
                models.fast_modes.insert(
                    FAST_SPEED_KEY.to_owned(),
                    TurnCounts {
                        main_loop: 0,
                        delegated: 2,
                    },
                );
            }
            DetectorId::CacheChurn => {
                let EvidenceValue::Complete(models) = &mut row.models else {
                    unreachable!()
                };
                // A `by_model` entry establishes a reviewed Claude family,
                // the same way `ModelOverthinking`'s branch above does.
                models
                    .by_model
                    .insert("claude-sonnet-4-6".to_owned(), ModelTokens::default());
                let EvidenceValue::Complete(cache) = &mut row.cache else {
                    unreachable!()
                };
                // Every paid token is a repeat: the overpay multiple is
                // infinite, a finding at any reviewed family's bound.
                cache.repeated_context = EvidenceValue::Complete(RepeatedContext {
                    accounting: RepeatedContextAccounting::CacheWrite,
                    repeated_tokens: 5_000,
                    paid_tokens: 5_000,
                    pairs_considered: 1,
                    pairs_skipped: 0,
                });
            }
            DetectorId::OverpoweredSubagents | DetectorId::UnusedBuiltInTools => {
                unreachable!("no clean-only fact exists for {detector:?}")
            }
        }
        row
    }

    #[test]
    fn a_finding_wins_over_a_partial_clean_only_fact_at_report_level() {
        // (c) A finding observed alongside a Partial clean-only fact
        // must still report Findings.
        let mut catalogs = ReportCatalogs::default();
        catalogs.model_replacements.entries.insert(
            "old-model-1".to_owned(),
            ModelReplacementEntry {
                replacement: "new-model-2".to_owned(),
                available_since_ts_ms: 100,
                rationale: "test rule".to_owned(),
                source_url: "https://example.invalid/old-model-1".to_owned(),
            },
        );

        for detector in DetectorId::ALL {
            if matches!(
                detector,
                DetectorId::OverpoweredSubagents
                    | DetectorId::UnusedBuiltInTools
                    | DetectorId::UnusedMcpServers
                    | DetectorId::UnusedSkills
            ) {
                // OverpoweredSubagents and UnusedBuiltInTools have no
                // clean-only fact (see `trigger_finding`'s doc comment).
                // UnusedMcpServers and UnusedSkills have exactly one,
                // Eligibility, but their own `evaluate` bodies (unchanged
                // in this seam) require a complete eligibility group to
                // report any finding at all — Partial eligibility reads
                // NoFinding there, not Finding, by that rule's own
                // documented partial-evidence policy. Degrading their
                // only clean-only fact cannot demonstrate (c).
                continue;
            }
            let required = requirements(detector);
            let clean_only: Vec<Fact> = required
                .clean
                .iter()
                .copied()
                .filter(|fact| !required.finding.contains(fact))
                .collect();
            assert!(
                !clean_only.is_empty(),
                "{detector:?} must have a clean-only fact to degrade"
            );
            for fact in clean_only {
                let mut row = trigger_finding(detector, &catalogs);
                degrade_fact_to_partial(&mut row, fact);
                let status = status_for_with_catalogs(row, detector, catalogs.clone());
                assert!(
                    matches!(status, DetectorStatus::Findings(_)),
                    "degrading clean-only fact {fact:?} for {detector:?} must still report a finding, got {status:?}"
                );
            }
        }
    }

    #[test]
    fn incomplete_absence_never_yields_clean_at_report_level() {
        // One of two eligible sessions carries only partial model
        // evidence and shows no finding. Overthinking must not read
        // clean from that incomplete absence.
        let mut partial = evidence_with_work("partial");
        partial.models = match partial.models {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::IncompleteTail,
            },
            _ => unreachable!(),
        };
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(evidence_with_work("complete"));
        accumulator.observe_session(partial);
        let report = accumulator.finish(context(CoverageCounts::default()));
        let counts = report.detectors[DetectorId::ModelOverthinking.index()];

        assert_eq!(counts.clean, 1);
        assert_eq!(counts.unavailable, 1);
        assert_eq!(counts.assessed, 1);
        assert_eq!(
            report.detector_statuses[DetectorId::ModelOverthinking.index()],
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
    }

    #[test]
    fn partial_findings_are_assessed_and_keep_clean_session_counts() {
        let clean = complete_row("clean");
        let mut finding = complete_row("partial-finding");
        let EvidenceValue::Complete(context_evidence) = &mut finding.context else {
            unreachable!()
        };
        context_evidence.max_request_context_tokens =
            ReportCatalogs::default().depth_cap_tokens + 1;
        to_partial(&mut finding.context);

        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session(clean);
        accumulator.observe_session(finding);
        let report = accumulator.finish(context(CoverageCounts::default()));
        let counts = report.detectors[DetectorId::SessionsOverDepth.index()];

        assert_eq!(counts.finding, 1);
        assert_eq!(counts.clean, 1);
        assert_eq!(counts.assessed, 2);
        assert_eq!(counts.unavailable, 0);
        assert!(matches!(
            report.detector_statuses[DetectorId::SessionsOverDepth.index()],
            DetectorStatus::Findings(_)
        ));
    }

    #[test]
    fn detector_outcomes_partition_the_ready_cohort() {
        let mut partial = complete_row("partial");
        to_partial(&mut partial.models);
        let cohort = [
            complete_row("complete"),
            evidence("idle-with-capability-gaps"),
            partial,
        ];
        let mut accumulator = EfficiencyReportAccumulator::new();
        for row in cohort {
            accumulator.observe_session(row);
        }
        let report = accumulator.finish(context(CoverageCounts {
            ready: 3,
            discovered: 3,
            ..CoverageCounts::default()
        }));

        for detector in DetectorId::ALL {
            let counts = report.detectors[detector.index()];
            assert_eq!(
                counts.assessed,
                counts.finding + counts.clean,
                "{detector:?}"
            );
            assert_eq!(
                report.assessed_sessions,
                counts.finding + counts.clean + counts.unavailable + counts.not_applicable,
                "{detector:?}"
            );
        }
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
        let detector = DetectorId::UnusedBuiltInTools;

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
