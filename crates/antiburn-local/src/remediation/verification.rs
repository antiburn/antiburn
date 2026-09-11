use crate::analysis::SourceFormat;
use crate::insights::DetectorId;

use super::{FindingAssessment, VERIFICATION_METHOD_REVISION};

/// The lifecycle state before a pure verification pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStage {
    Watching,
    Fixed,
}

/// The exact old-model target of the verification method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldModelVerificationTarget {
    pub scope: String,
    pub provider: Option<String>,
    pub api: Option<String>,
    pub old_model: String,
    pub replacement: String,
}

/// One actual model use observed after or before an activation boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelVerificationObservation {
    pub timestamp_ms: i64,
    pub scope: String,
    pub provider: Option<String>,
    pub api: Option<String>,
    pub model: String,
}

/// States why supported evidence cannot yet verify a remediation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationUnknownReason {
    MissingPostBoundaryEvidence,
    UnsupportedEvidence,
}

/// The pure result of one post-boundary verification pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationOutcome {
    Fixed,
    StillUnresolved,
    Recurred,
    Unknown(VerificationUnknownReason),
}

/// One verifier result pinned to the method revision that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub method_revision: u32,
    pub outcome: VerificationOutcome,
    pub observed_at_ms: Option<i64>,
}

/// Verifies an old-model remediation against actual normalized model uses.
pub fn verify_old_model(
    target: &OldModelVerificationTarget,
    stage: VerificationStage,
    boundary_ms: i64,
    observations: &[ModelVerificationObservation],
) -> VerificationResult {
    let (outcome, observed_at_ms) =
        verify_old_model_outcome(target, stage, boundary_ms, observations);
    VerificationResult {
        method_revision: VERIFICATION_METHOD_REVISION,
        outcome,
        observed_at_ms,
    }
}

fn verify_old_model_outcome(
    target: &OldModelVerificationTarget,
    stage: VerificationStage,
    boundary_ms: i64,
    observations: &[ModelVerificationObservation],
) -> (VerificationOutcome, Option<i64>) {
    let mut matching = observations
        .iter()
        .filter(|observation| {
            observation.timestamp_ms > boundary_ms
                && observation.scope == target.scope
                && observation.provider == target.provider
                && observation.api == target.api
                && (observation.model == target.old_model
                    || observation.model == target.replacement)
        })
        .collect::<Vec<_>>();
    matching.sort_by_key(|observation| observation.timestamp_ms);
    if stage == VerificationStage::Fixed {
        return matching
            .into_iter()
            .find(|observation| observation.model == target.old_model)
            .map_or(
                (
                    VerificationOutcome::Unknown(
                        VerificationUnknownReason::MissingPostBoundaryEvidence,
                    ),
                    None,
                ),
                |observation| {
                    (
                        VerificationOutcome::Recurred,
                        Some(observation.timestamp_ms),
                    )
                },
            );
    }
    if let Some(latest_ms) = matching.last().map(|observation| observation.timestamp_ms) {
        let latest_uses_replacement = matching.iter().any(|observation| {
            observation.timestamp_ms == latest_ms && observation.model == target.replacement
        });
        let latest_uses_old = matching.iter().any(|observation| {
            observation.timestamp_ms == latest_ms && observation.model == target.old_model
        });
        if latest_uses_replacement && !latest_uses_old {
            return (VerificationOutcome::Fixed, Some(latest_ms));
        }
        return (VerificationOutcome::StillUnresolved, Some(latest_ms));
    }
    (
        VerificationOutcome::Unknown(VerificationUnknownReason::MissingPostBoundaryEvidence),
        None,
    )
}

/// One detector assessment for an exact canonical target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetAssessment {
    pub observed_at_ms: i64,
    pub identity: String,
    pub target_present: bool,
    pub assessment: FindingAssessment,
}

/// Verifies a prompt watch from complete, fresh detector assessments.
pub fn verify_prompt_watch(
    detector: DetectorId,
    source_format: SourceFormat,
    identity: &str,
    stage: VerificationStage,
    boundary_ms: i64,
    assessments: &[TargetAssessment],
) -> VerificationResult {
    if !generic_verification_supported(detector, source_format) {
        return VerificationResult {
            method_revision: VERIFICATION_METHOD_REVISION,
            outcome: VerificationOutcome::Unknown(VerificationUnknownReason::UnsupportedEvidence),
            observed_at_ms: None,
        };
    }
    if stage == VerificationStage::Fixed
        && let Some(recurrence) = assessments
            .iter()
            .filter(|assessment| {
                assessment.observed_at_ms > boundary_ms
                    && assessment.identity == identity
                    && assessment.target_present
            })
            .min_by_key(|assessment| assessment.observed_at_ms)
    {
        return VerificationResult {
            method_revision: VERIFICATION_METHOD_REVISION,
            outcome: VerificationOutcome::Recurred,
            observed_at_ms: Some(recurrence.observed_at_ms),
        };
    }
    let latest = assessments
        .iter()
        .filter(|assessment| {
            assessment.observed_at_ms > boundary_ms && assessment.identity == identity
        })
        .max_by_key(|assessment| assessment.observed_at_ms);
    let (outcome, observed_at_ms) = match latest {
        Some(assessment) if assessment.target_present => (
            VerificationOutcome::StillUnresolved,
            Some(assessment.observed_at_ms),
        ),
        Some(assessment) if assessment.assessment == FindingAssessment::Clean => {
            (VerificationOutcome::Fixed, Some(assessment.observed_at_ms))
        }
        _ => (
            VerificationOutcome::Unknown(VerificationUnknownReason::MissingPostBoundaryEvidence),
            None,
        ),
    };
    VerificationResult {
        method_revision: VERIFICATION_METHOD_REVISION,
        outcome,
        observed_at_ms,
    }
}

/// Returns true when the accepted source can prove the detector's exact target improved.
pub const fn verification_evidence_supported(
    detector: DetectorId,
    source_format: SourceFormat,
) -> bool {
    match detector {
        DetectorId::ModelOverthinking => matches!(
            source_format,
            SourceFormat::ClaudeJsonl | SourceFormat::CodexRolloutJsonl | SourceFormat::PiV3Jsonl
        ),
        DetectorId::OldModelUsage => matches!(
            source_format,
            SourceFormat::ClaudeJsonl
                | SourceFormat::CodexRolloutJsonl
                | SourceFormat::OpenCodeJsonl
                | SourceFormat::OpenCodeSqliteV2
                | SourceFormat::PiV3Jsonl
        ),
        DetectorId::OveruseOfFastMode => matches!(
            source_format,
            SourceFormat::ClaudeJsonl | SourceFormat::CodexRolloutJsonl
        ),
        _ => false,
    }
}

const fn generic_verification_supported(detector: DetectorId, source_format: SourceFormat) -> bool {
    matches!(
        detector,
        DetectorId::ModelOverthinking | DetectorId::OveruseOfFastMode
    ) && verification_evidence_supported(detector, source_format)
}

#[cfg(test)]
mod tests;
