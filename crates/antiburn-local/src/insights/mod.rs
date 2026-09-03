//! Provider-neutral local insights report contracts and reduction.

mod badges;
mod detectors;
mod quota;
mod report;
mod status;

pub use badges::{BadgeId, BadgeStatus, SessionBadge, session_badges};
pub use detectors::{
    DetectorFindings, DetectorStatus, EffortPolicy, FamilyPolicy, ModelFamily, ModelRegistry,
    ModelReplacementEntry, ModelReplacementRule, NotAssessedReason, PremiumPolicy,
    REGISTRY_REVISION, ReportCatalogs, SpeedPolicy, model_family,
};
pub use quota::{
    MAX_QUOTA_AFFECTED_MODELS, MAX_QUOTA_OBSERVED_TIMES, MAX_QUOTA_SESSION_EXAMPLES,
    QuotaPressureFindings, QuotaPressureSection,
};
pub use report::{
    CoverageCounts, DetectorCounts, DetectorRequirements, EfficiencyReport,
    EfficiencyReportAccumulator, Fact, FactState, MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS,
    MAX_EXAMPLES_PER_DETECTOR, MAX_REPORT_UNRECOGNIZED_TYPES, ReportContext, ReportWindow,
    SessionExample, UnrecognizedRecords, clean_facts_complete, eligible, requirements,
};
pub use status::{CoverageBucket, DetectorId};
