//! Provider-neutral local insights report contracts and reduction.

mod report;
mod status;

pub use report::{
    CapabilityFlag, CoverageCounts, DetectorCounts, DetectorRequirements, EfficiencyReport,
    EfficiencyReportAccumulator, EvidenceGroup, GroupState, MAX_EXAMPLES_PER_DETECTOR,
    ReportContext, ReportWindow, SessionExample, requirements,
};
pub use status::{CoverageBucket, DetectorId};
