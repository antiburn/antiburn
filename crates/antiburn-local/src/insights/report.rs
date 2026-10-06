use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::{CoverageReason, EvidenceCoverage, SessionEvidence, SourceAcceptance};
use crate::checks::requirements::{clean_facts_complete, eligible};
use crate::checks::{self as detectors, DetectorFold, DetectorStatus, ReportCatalogs, complete};

use super::provider_incidents::{ProviderIncidentsAccumulator, ProviderIncidentsSection};
use super::quota::{QuotaPressureAccumulator, QuotaPressureSection};
use super::token_burn::{SessionTokenBurnEvidence, TokenBurnAccumulator};
use super::{CoverageBucket, DetectorId};

pub const MAX_EXAMPLES_PER_DETECTOR: usize = 3;
pub const MAX_REPORT_UNRECOGNIZED_TYPES: usize = 16;
pub(super) const UNRECOGNIZED_TYPES_DIAGNOSTIC: &str = "diagnostics.unrecognized_types";

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
    pub(super) token_burn_denominator: Option<u128>,
    pub(super) token_burn_by_detector_by_session: [Option<Vec<u128>>; DetectorId::COUNT],
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
        let mut cache_assessed = false;

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
                    cache_assessed |= detector == DetectorId::CacheChurn;
                }
                detectors::Observation::NoFinding if clean_facts_complete(detector, &evidence) => {
                    self.clean_agents[detector.index()].insert(evidence.identity.agent.clone());
                    counts.clean += 1;
                    counts.assessed += 1;
                    cache_assessed |= detector == DetectorId::CacheChurn;
                }
                detectors::Observation::NoFinding
                | detectors::Observation::ContractIncomplete
                | detectors::Observation::SignalMissing => counts.unavailable += 1,
            }
            self.folds[detector.index()].observe(observation, &evidence);
        }
        self.token_burn
            .observe(token_evidence, findings, source_eligible, cache_assessed);
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
            token_burn_by_detector_by_session,
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
            token_burn_by_detector_by_session,
        }
    }
}
