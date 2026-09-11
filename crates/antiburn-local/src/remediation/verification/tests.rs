use super::*;
use crate::insights::ReportCatalogs;

fn old_model_target() -> OldModelVerificationTarget {
    OldModelVerificationTarget {
        scope: "workspace-a".to_owned(),
        provider: Some("anthropic".to_owned()),
        api: Some("messages".to_owned()),
        old_model: "claude-opus-4-8".to_owned(),
        replacement: "claude-opus-5".to_owned(),
    }
}

fn model_observation(
    timestamp_ms: i64,
    scope: &str,
    provider: &str,
    api: &str,
    model: &str,
) -> ModelVerificationObservation {
    ModelVerificationObservation {
        timestamp_ms,
        scope: scope.to_owned(),
        provider: Some(provider.to_owned()),
        api: Some(api.to_owned()),
        model: model.to_owned(),
    }
}

#[test]
fn old_model_verification_requires_post_boundary_exact_route_and_model() {
    let observations = vec![
        model_observation(100, "workspace-a", "anthropic", "messages", "claude-opus-5"),
        model_observation(101, "workspace-b", "anthropic", "messages", "claude-opus-5"),
        model_observation(101, "workspace-a", "gateway", "messages", "claude-opus-5"),
        model_observation(
            101,
            "workspace-a",
            "anthropic",
            "responses",
            "claude-opus-5",
        ),
        model_observation(101, "workspace-a", "anthropic", "messages", "other-model"),
    ];
    assert_eq!(
        verify_old_model(
            &old_model_target(),
            VerificationStage::Watching,
            100,
            &observations,
        )
        .outcome,
        VerificationOutcome::Unknown(VerificationUnknownReason::MissingPostBoundaryEvidence)
    );
    let result = verify_old_model(
        &old_model_target(),
        VerificationStage::Watching,
        100,
        &[model_observation(
            101,
            "workspace-a",
            "anthropic",
            "messages",
            "claude-opus-5",
        )],
    );
    assert_eq!(result.outcome, VerificationOutcome::Fixed);
    assert_eq!(result.method_revision, VERIFICATION_METHOD_REVISION);
}

#[test]
fn a_matching_bad_model_recurs_only_after_the_fix_was_verified() {
    let observations = [model_observation(
        101,
        "workspace-a",
        "anthropic",
        "messages",
        "claude-opus-4-8",
    )];
    assert_eq!(
        verify_old_model(
            &old_model_target(),
            VerificationStage::Watching,
            100,
            &observations,
        )
        .outcome,
        VerificationOutcome::StillUnresolved
    );
    assert_eq!(
        verify_old_model(
            &old_model_target(),
            VerificationStage::Fixed,
            100,
            &observations,
        )
        .outcome,
        VerificationOutcome::Recurred
    );
}

#[test]
fn latest_exact_observation_determines_the_watching_result() {
    let resolved = [
        model_observation(
            101,
            "workspace-a",
            "anthropic",
            "messages",
            "claude-opus-4-8",
        ),
        model_observation(102, "workspace-a", "anthropic", "messages", "claude-opus-5"),
    ];
    assert_eq!(
        verify_old_model(
            &old_model_target(),
            VerificationStage::Watching,
            100,
            &resolved,
        )
        .outcome,
        VerificationOutcome::Fixed
    );
    let regressed = [
        model_observation(101, "workspace-a", "anthropic", "messages", "claude-opus-5"),
        model_observation(
            102,
            "workspace-a",
            "anthropic",
            "messages",
            "claude-opus-4-8",
        ),
    ];
    assert_eq!(
        verify_old_model(
            &old_model_target(),
            VerificationStage::Watching,
            100,
            &regressed,
        )
        .outcome,
        VerificationOutcome::StillUnresolved
    );
}

#[test]
fn old_model_rows_before_the_qualifying_replacement_do_not_recur() {
    let observations = [
        model_observation(
            101,
            "workspace-a",
            "anthropic",
            "messages",
            "claude-opus-4-8",
        ),
        model_observation(102, "workspace-a", "anthropic", "messages", "claude-opus-5"),
    ];
    let fixed = verify_old_model(
        &old_model_target(),
        VerificationStage::Watching,
        100,
        &observations,
    );
    assert_eq!(fixed.outcome, VerificationOutcome::Fixed);
    assert_eq!(fixed.observed_at_ms, Some(102));
    assert_eq!(
        verify_old_model(
            &old_model_target(),
            VerificationStage::Fixed,
            fixed.observed_at_ms.unwrap(),
            &observations,
        )
        .outcome,
        VerificationOutcome::Unknown(VerificationUnknownReason::MissingPostBoundaryEvidence)
    );
}

#[test]
fn supported_generic_verification_requires_a_clean_detector_assessment() {
    let fixed = verify_prompt_watch(
        DetectorId::ModelOverthinking,
        SourceFormat::ClaudeJsonl,
        "session",
        VerificationStage::Watching,
        100,
        &[TargetAssessment {
            observed_at_ms: 101,
            identity: "session".to_owned(),
            target_present: false,
            assessment: FindingAssessment::Clean,
        }],
    );
    assert_eq!(fixed.outcome, VerificationOutcome::Fixed);
    assert_eq!(fixed.observed_at_ms, Some(101));

    let recurrence_evidence =
        crate::insights::detectors::test_support::claude_evidence("recurrence");
    let recurred = verify_prompt_watch(
        DetectorId::ModelOverthinking,
        SourceFormat::ClaudeJsonl,
        "resource",
        VerificationStage::Fixed,
        100,
        &[
            TargetAssessment {
                observed_at_ms: 103,
                identity: "resource".to_owned(),
                target_present: false,
                assessment: FindingAssessment::Clean,
            },
            TargetAssessment {
                observed_at_ms: 101,
                identity: "resource".to_owned(),
                target_present: true,
                assessment: FindingAssessment::Findings(vec![
                    super::super::findings::finding_for_test(
                        &recurrence_evidence,
                        super::super::FindingCause::UnusedSkill {
                            skill: "resource".to_owned(),
                        },
                    ),
                ]),
            },
        ],
    );
    assert_eq!(recurred.outcome, VerificationOutcome::Recurred);
    assert_eq!(recurred.observed_at_ms, Some(101));
}

#[test]
fn incomplete_no_finding_assessment_cannot_verify_a_fix() {
    let mut evidence = crate::insights::detectors::test_support::claude_evidence("incomplete");
    evidence.coverage = crate::analysis::EvidenceCoverage::Partial(
        crate::analysis::CoverageReason::MalformedRecord,
    );
    let assessment = super::super::assess_detector(
        DetectorId::SessionsOverDepth,
        &evidence,
        &ReportCatalogs::default(),
    );
    assert_eq!(
        assessment,
        FindingAssessment::Unavailable(super::super::FindingUnavailableReason::IncompleteEvidence)
    );
}

#[test]
fn verification_matrix_matches_the_documented_positive_proof_cells() {
    use SourceFormat::{
        ClaudeJsonl, CodexRolloutJsonl, OpenCodeJsonl, OpenCodeSqliteV2, PiV3Jsonl,
    };

    let supported = [
        (DetectorId::ModelOverthinking, ClaudeJsonl),
        (DetectorId::ModelOverthinking, CodexRolloutJsonl),
        (DetectorId::ModelOverthinking, PiV3Jsonl),
        (DetectorId::OldModelUsage, ClaudeJsonl),
        (DetectorId::OldModelUsage, CodexRolloutJsonl),
        (DetectorId::OldModelUsage, OpenCodeJsonl),
        (DetectorId::OldModelUsage, OpenCodeSqliteV2),
        (DetectorId::OldModelUsage, PiV3Jsonl),
        (DetectorId::OveruseOfFastMode, ClaudeJsonl),
        (DetectorId::OveruseOfFastMode, CodexRolloutJsonl),
    ];
    let formats = [
        ClaudeJsonl,
        CodexRolloutJsonl,
        OpenCodeJsonl,
        OpenCodeSqliteV2,
        PiV3Jsonl,
        SourceFormat::CursorJsonl,
        SourceFormat::AntigravityJson,
        SourceFormat::Uncharacterized,
    ];
    for detector in DetectorId::ALL {
        for source_format in formats {
            assert_eq!(
                verification_evidence_supported(detector, source_format),
                supported.contains(&(detector, source_format)),
                "{detector:?} {source_format:?}"
            );
        }
    }
}

#[test]
fn a_named_resource_still_injected_and_invoked_cannot_verify_from_clean_assessment() {
    let result = verify_prompt_watch(
        DetectorId::UnusedMcpServers,
        SourceFormat::ClaudeJsonl,
        "mcp:server-a",
        VerificationStage::Watching,
        100,
        &[TargetAssessment {
            observed_at_ms: 101,
            identity: "mcp:server-a".into(),
            target_present: false,
            assessment: FindingAssessment::Clean,
        }],
    );

    assert_eq!(
        result.outcome,
        VerificationOutcome::Unknown(VerificationUnknownReason::UnsupportedEvidence)
    );
}
