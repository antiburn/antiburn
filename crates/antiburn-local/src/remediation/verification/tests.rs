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
            clean_for_verification: true,
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
                clean_for_verification: true,
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
                            tokens: None,
                            cost_usd: None,
                            pricing_revision: None,
                        },
                    ),
                ]),
                clean_for_verification: false,
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

fn named_resource_target(detector: DetectorId, resource: &str) -> NamedResourceVerificationTarget {
    NamedResourceVerificationTarget {
        detector,
        source_format: SourceFormat::ClaudeJsonl,
        agent: "claude-code".to_owned(),
        project_scope: "project-a".to_owned(),
        resource: resource.to_owned(),
    }
}

fn named_resource_assessment(
    detector: DetectorId,
    evidence: NamedResourceEvidence,
) -> NamedResourceAssessment {
    NamedResourceAssessment {
        observed_at_ms: 101,
        source_format: SourceFormat::ClaudeJsonl,
        agent: "claude-code".to_owned(),
        project_scope: "project-a".to_owned(),
        evidence: match detector {
            DetectorId::UnusedMcpServers
            | DetectorId::UnusedBuiltInTools
            | DetectorId::UnusedSkills => evidence,
            _ => unreachable!(),
        },
    }
}

#[test]
fn named_mcp_built_in_and_skill_targets_pass_when_absent_from_complete_later_inventory() {
    for (detector, resource) in [
        (DetectorId::UnusedMcpServers, "Server-A"),
        (DetectorId::UnusedBuiltInTools, "Web Search"),
        (DetectorId::UnusedSkills, "Team:Review"),
    ] {
        let result = verify_named_resource_watch(
            &named_resource_target(detector, resource),
            VerificationStage::Watching,
            100,
            &[named_resource_assessment(
                detector,
                NamedResourceEvidence::Complete {
                    resources: vec![NamedResourceObservation {
                        resource: "different-resource".to_owned(),
                        used: true,
                    }],
                },
            )],
        );
        assert_eq!(result.outcome, VerificationOutcome::Fixed);
        assert_eq!(result.observed_at_ms, Some(101));
    }
}

#[test]
fn named_targets_match_the_normalized_name_and_remain_unresolved_when_present() {
    for (detector, target_resource, observed_resource) in [
        (DetectorId::UnusedMcpServers, "Server-A", " server-a "),
        (DetectorId::UnusedBuiltInTools, "web-search", "Web Search"),
        (DetectorId::UnusedSkills, "Team:Review", "team:review"),
    ] {
        let result = verify_named_resource_watch(
            &named_resource_target(detector, target_resource),
            VerificationStage::Watching,
            100,
            &[named_resource_assessment(
                detector,
                NamedResourceEvidence::Complete {
                    resources: vec![NamedResourceObservation {
                        resource: observed_resource.to_owned(),
                        used: false,
                    }],
                },
            )],
        );
        assert_eq!(result.outcome, VerificationOutcome::StillUnresolved);
    }
}

#[test]
fn named_target_recurrence_requires_the_exact_target_and_complete_evidence() {
    let target = named_resource_target(DetectorId::UnusedSkills, "review");
    let recurred = verify_named_resource_watch(
        &target,
        VerificationStage::Fixed,
        100,
        &[named_resource_assessment(
            target.detector,
            NamedResourceEvidence::Complete {
                resources: vec![NamedResourceObservation {
                    resource: "review".to_owned(),
                    used: false,
                }],
            },
        )],
    );
    assert_eq!(recurred.outcome, VerificationOutcome::Recurred);

    let unrelated = verify_named_resource_watch(
        &target,
        VerificationStage::Fixed,
        100,
        &[named_resource_assessment(
            target.detector,
            NamedResourceEvidence::Complete {
                resources: vec![NamedResourceObservation {
                    resource: "other".to_owned(),
                    used: false,
                }],
            },
        )],
    );
    assert_eq!(unrelated.outcome, VerificationOutcome::Fixed);
}

#[test]
fn named_target_incomplete_or_mismatched_evidence_stays_awaiting() {
    let target = named_resource_target(DetectorId::UnusedMcpServers, "server-a");
    let evidence = [
        NamedResourceEvidence::Partial,
        NamedResourceEvidence::Capped,
        NamedResourceEvidence::Ambiguous,
    ];
    for evidence in evidence {
        let result = verify_named_resource_watch(
            &target,
            VerificationStage::Watching,
            100,
            &[named_resource_assessment(target.detector, evidence)],
        );
        assert_eq!(
            result.outcome,
            VerificationOutcome::Unknown(VerificationUnknownReason::MissingPostBoundaryEvidence)
        );
    }

    let malformed = verify_named_resource_watch(
        &target,
        VerificationStage::Watching,
        100,
        &[named_resource_assessment(
            target.detector,
            NamedResourceEvidence::Complete {
                resources: vec![NamedResourceObservation {
                    resource: String::new(),
                    used: false,
                }],
            },
        )],
    );
    assert!(matches!(malformed.outcome, VerificationOutcome::Unknown(_)));

    for (source_format, agent, project_scope) in [
        (SourceFormat::CodexRolloutJsonl, "claude-code", "project-a"),
        (SourceFormat::ClaudeJsonl, "codex", "project-a"),
        (SourceFormat::ClaudeJsonl, "claude-code", "project-b"),
    ] {
        let mut assessment = named_resource_assessment(
            target.detector,
            NamedResourceEvidence::Complete {
                resources: Vec::new(),
            },
        );
        assessment.source_format = source_format;
        assessment.agent = agent.to_owned();
        assessment.project_scope = project_scope.to_owned();
        let result =
            verify_named_resource_watch(&target, VerificationStage::Watching, 100, &[assessment]);
        assert!(matches!(result.outcome, VerificationOutcome::Unknown(_)));
    }

    let missing = verify_named_resource_watch(&target, VerificationStage::Watching, 100, &[]);
    assert!(matches!(missing.outcome, VerificationOutcome::Unknown(_)));
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
