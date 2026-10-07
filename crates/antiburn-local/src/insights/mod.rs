//! Provider-neutral local insights report contracts and reduction.

mod badges;
mod provider_incidents;
mod quota;
mod report;
mod status;
mod token_burn;

pub use crate::checks::requirements::{
    DetectorRequirements, Fact, FactState, clean_fact_unsupported, clean_facts_complete, eligible,
    requirements,
};
pub use crate::checks::{
    DetectorFindings, DetectorStatus, EffortPolicy, FamilyPolicy, ModelFamily, ModelRegistry,
    ModelReplacementEntry, ModelReplacementRule, NotAssessedReason, PremiumPolicy,
    REGISTRY_REVISION, ReportCatalogs, SpeedPolicy, model_family,
};
pub use crate::checks::{DetectorId, DetectorSelection};
pub use badges::{
    BadgeId, BadgeStatus, SessionBadge, session_badges, session_badges_with_selection,
};
pub use provider_incidents::{
    MAX_PROVIDER_AFFECTED_MODELS, MAX_PROVIDER_OBSERVED_TIMES, MAX_PROVIDER_SESSION_EXAMPLES,
    ProviderIncidentFindings, ProviderIncidentsSection,
};
pub use quota::{
    MAX_QUOTA_AFFECTED_MODELS, MAX_QUOTA_OBSERVED_TIMES, MAX_QUOTA_SESSION_EXAMPLES,
    QuotaPressureFindings, QuotaPressureSection,
};
pub use report::{
    CoverageCounts, DetectorCounts, EfficiencyReport, EfficiencyReportAccumulator,
    MAX_EXAMPLES_PER_DETECTOR, MAX_REPORT_UNRECOGNIZED_TYPES, ReportContext, ReportWindow,
    SessionExample, UnrecognizedRecords,
};
pub use status::CoverageBucket;
pub use token_burn::{
    MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS, ResourceTokenBurnAssessment, SessionTokenBurnEvidence,
    TokenBurnSourceEvidence, TokenBurnTurnAccumulator, TokenBurnTurnEvidence,
    fallback_token_burn_basis_points,
};
