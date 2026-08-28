//! Provider-neutral local insights report contracts and reduction.

mod badges;
mod detectors;
mod quota;
mod report;
mod status;

pub use badges::{BadgeId, BadgeStatus, SessionBadge, session_badges};
pub use detectors::{
    DetectorFindings, DetectorStatus, ModelReplacement, NotAssessedReason, ReportCatalogs,
};
pub use quota::{
    MAX_QUOTA_AFFECTED_MODELS, MAX_QUOTA_OBSERVED_TIMES, MAX_QUOTA_SESSION_EXAMPLES,
    QuotaPressureFindings, QuotaPressureSection,
};
pub use report::{
    CapabilityFlag, CoverageCounts, DetectorCounts, DetectorRequirements, EfficiencyReport,
    EfficiencyReportAccumulator, EvidenceGroup, GroupState, MAX_EXAMPLES_PER_DETECTOR,
    MAX_REPORT_UNRECOGNIZED_TYPES, ReportContext, ReportWindow, SessionExample,
    UnrecognizedRecords, requirements,
};
pub use status::{CoverageBucket, DetectorId};
