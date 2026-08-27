// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Provider-neutral local insights report contracts and reduction.

mod detectors;
mod quota;
mod report;
mod status;

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
    ReportContext, ReportWindow, SessionExample, requirements,
};
pub use status::{CoverageBucket, DetectorId};
