use super::*;
use crate::analysis::{
    ANALYZER_REVISION, ContextEvidence, EVIDENCE_SCHEMA_REVISION, EvidenceSource, FAST_SPEED_KEY,
    LoadedSource, ModelTokens, PARSER_REVISION, ProviderIncident, ProviderIncidentKind,
    QuotaConfidence, QuotaHitSeverity, QuotaIncident, QuotaLimitKind, RepeatedContext,
    RepeatedContextAccounting, SessionEvidenceAccumulator, SessionProviderEvidence,
    SessionQuotaEvidence, SignalCoverage, SourceCapabilities, SourceFormat, SourceKind,
    ToolDefinition, TurnCounts, TurnFacts,
};
use crate::insights::detectors::{ModelFamily, ModelReplacementEntry, NotAssessedReason};
use crate::insights::provider_incidents::ProviderIncidentsSection;
use crate::insights::quota::QuotaPressureSection;

#[test]
fn source_format_serde_keys_are_stable() {
    let formats = [
        (SourceFormat::ClaudeJsonl, "claude_jsonl"),
        (SourceFormat::CodexRolloutJsonl, "codex_rollout_jsonl"),
        (SourceFormat::OpenCodeJsonl, "open_code_jsonl"),
        (SourceFormat::OpenCodeSqliteV2, "open_code_sqlite_v2"),
        (SourceFormat::PiV3Jsonl, "pi_v3_jsonl"),
        (SourceFormat::CursorJsonl, "cursor_jsonl"),
        (SourceFormat::CursorCliAgentJsonl, "cursor_cli_agent_jsonl"),
        (SourceFormat::CursorCliStoreDb, "cursor_cli_store_db"),
        (SourceFormat::CursorChatStoreDb, "cursor_chat_store_db"),
        (SourceFormat::CursorIdeComposer, "cursor_ide_composer"),
        (
            SourceFormat::CursorLegacyChatJson,
            "cursor_legacy_chat_json",
        ),
        (SourceFormat::AntigravityJson, "antigravity_json"),
        (
            SourceFormat::AntigravityBrainJsonl,
            "antigravity_brain_jsonl",
        ),
        (
            SourceFormat::AntigravityCascadeJson,
            "antigravity_cascade_json",
        ),
        (
            SourceFormat::AntigravityWorkspaceChatJson,
            "antigravity_workspace_chat_json",
        ),
        (SourceFormat::AntigravitySqlite, "antigravity_sqlite"),
        (SourceFormat::CopilotCliJsonl, "copilot_cli_jsonl"),
        (SourceFormat::CopilotIdeChatJson, "copilot_ide_chat_json"),
        (SourceFormat::ClineSessionJson, "cline_session_json"),
        (
            SourceFormat::ClineMessagesContractV1,
            "cline_messages_contract_v1",
        ),
        (SourceFormat::KiroSessionJson, "kiro_session_json"),
        (SourceFormat::KiroChat, "kiro_chat"),
        (SourceFormat::KiroCliV2Bundle, "kiro_cli_v2_bundle"),
        (SourceFormat::KiroCliV3Bundle, "kiro_cli_v3_bundle"),
        (SourceFormat::KiroChatSaveExport, "kiro_chat_save_export"),
        (SourceFormat::AmpThreadJson, "amp_thread_json"),
        (SourceFormat::AmpFileChanges, "amp_file_changes"),
        (
            SourceFormat::WindsurfWorkspaceJson,
            "windsurf_workspace_json",
        ),
        (SourceFormat::WindsurfMirrorJson, "windsurf_mirror_json"),
        (
            SourceFormat::WindsurfCascadeProtobuf,
            "windsurf_cascade_protobuf",
        ),
        (SourceFormat::DevinLocalSqlite, "devin_local_sqlite"),
        (SourceFormat::Uncharacterized, "uncharacterized"),
    ];

    for (format, key) in formats {
        assert_eq!(serde_json::to_value(format).unwrap(), key);
        assert_eq!(
            serde_json::from_str::<SourceFormat>(&format!("\"{key}\"")).unwrap(),
            format
        );
    }
}

fn evidence(session_id: &str) -> SessionEvidence {
    let mut row = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: session_id.to_owned(),
        kind: SourceKind::File,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    let EvidenceValue::Complete(sources) = &mut row.context_sources else {
        unreachable!()
    };
    sources.skill_coverage = EvidenceValue::Complete(());
    sources.mcp_coverage = EvidenceValue::Complete(());
    row
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
fn agent_inventory_covers_all_assessed_sessions_and_excludes_unavailable() {
    let mut accumulator = EfficiencyReportAccumulator::new();
    for (index, agent) in ["codex", "claude-code", "cursor", "opencode", "codex"]
        .iter()
        .enumerate()
    {
        let mut row = evidence_with_work(&format!("finding-{index}"));
        row.identity.agent = (*agent).to_owned();
        row.context = EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: 400_001,
            top_depth_examples: Vec::new(),
        });
        accumulator.observe_session(row);
    }
    let mut clean = evidence_with_work("clean");
    clean.identity.agent = "clean-agent".to_owned();
    clean.context = EvidenceValue::Complete(ContextEvidence {
        max_request_context_tokens: 10,
        top_depth_examples: Vec::new(),
    });
    accumulator.observe_session(clean);
    let mut unavailable = evidence_with_work("unavailable");
    unavailable.identity.agent = "unavailable-agent".to_owned();
    unavailable.context = EvidenceValue::Unsupported;
    accumulator.observe_session(unavailable);
    let report = accumulator.finish(context(CoverageCounts::default()));
    let index = DetectorId::SessionsOverDepth.index();
    assert_eq!(report.detectors[index].finding, 5);
    assert_eq!(report.detectors[index].clean, 1);
    assert_eq!(
        report.finding_agents[index]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["claude-code", "codex", "cursor", "opencode"]
    );
    assert_eq!(
        report.clean_agents[index]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["clean-agent"]
    );
}

#[test]
fn token_burn_estimates_use_measured_avoidable_tokens() {
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
    accumulator.observe_session_with_token_burn(
        finding,
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            overdepth_avoidable_tokens: Some(150),
            ..SessionTokenBurnEvidence::default()
        },
    );
    accumulator.observe_session_with_token_burn(
        clean,
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            overdepth_avoidable_tokens: Some(0),
            ..SessionTokenBurnEvidence::default()
        },
    );
    let report = accumulator.finish(context(CoverageCounts {
        ready: 2,
        discovered: 2,
        ..CoverageCounts::default()
    }));

    assert_eq!(report.estimated_token_burn_basis_points, Some(750));
    assert_eq!(
        report.estimated_token_burn_for_attributed_tokens(150),
        Some(750)
    );
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            (1 << DetectorId::SessionsOverDepth.index()) | (1 << DetectorId::UnusedSkills.index()),
            &[ResourceTokenBurnAssessment {
                detector: DetectorId::UnusedSkills,
                finding_count: 1,
                clean: false,
                tokens_by_session: Some(&[(0, 300)]),
            }],
        ),
        Some(1_500)
    );
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            (1 << DetectorId::SessionsOverDepth.index()) | (1 << DetectorId::UnusedSkills.index()),
            &[ResourceTokenBurnAssessment {
                detector: DetectorId::UnusedSkills,
                finding_count: 1,
                clean: false,
                tokens_by_session: Some(&[(1, 300)]),
            }],
        ),
        Some(2_250)
    );
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            (1 << DetectorId::SessionsOverDepth.index())
                | (1 << DetectorId::UnusedMcpServers.index())
                | (1 << DetectorId::UnusedSkills.index()),
            &[
                ResourceTokenBurnAssessment {
                    detector: DetectorId::UnusedMcpServers,
                    finding_count: 1,
                    clean: false,
                    tokens_by_session: Some(&[(0, 200)]),
                },
                ResourceTokenBurnAssessment {
                    detector: DetectorId::UnusedSkills,
                    finding_count: 1,
                    clean: false,
                    tokens_by_session: Some(&[(0, 200)]),
                },
            ],
        ),
        Some(2_000)
    );
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            1 << DetectorId::SessionsOverDepth.index(),
            &[],
        ),
        Some(750)
    );
    assert_eq!(
        report.detector_estimated_token_burn_basis_points[DetectorId::SessionsOverDepth.index()],
        Some(750)
    );
    assert_eq!(
        report.detector_estimated_token_burn_basis_points[DetectorId::ModelOverthinking.index()],
        Some(1_000)
    );
    assert_eq!(
        report.detector_estimated_token_burn_basis_points[DetectorId::UnusedSkills.index()],
        None
    );
}

#[test]
fn report_assesses_built_in_tools_from_report_time_source_evidence() {
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session_with_token_burn(
        evidence_with_work("built-in"),
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            built_in_tool_sources: Some(vec![TokenBurnSourceEvidence {
                scope: "claude:bundled".to_owned(),
                name: "web_search".to_owned(),
                replicated_tokens: 100,
                invoked: false,
                replicated_cost_usd: None,
            }]),
            ..SessionTokenBurnEvidence::default()
        },
    );

    let report = accumulator.finish(context(CoverageCounts {
        ready: 1,
        discovered: 1,
        ..CoverageCounts::default()
    }));

    assert!(matches!(
        report.detector_statuses[DetectorId::UnusedBuiltInTools.index()],
        DetectorStatus::Findings(_)
    ));
    assert_eq!(
        report.detector_estimated_token_burn_basis_points[DetectorId::UnusedBuiltInTools.index()],
        Some(1_000)
    );
}

#[test]
fn report_assesses_unused_sources_from_report_time_evidence() {
    for detector in [
        DetectorId::UnusedMcpServers,
        DetectorId::UnusedBuiltInTools,
        DetectorId::UnusedSkills,
    ] {
        let mut row = evidence_with_work("report-time-source");
        let EvidenceValue::Complete(sources) = &mut row.context_sources else {
            unreachable!()
        };
        match detector {
            DetectorId::UnusedMcpServers => sources.mcp_coverage = EvidenceValue::Unsupported,
            DetectorId::UnusedBuiltInTools => {
                sources.tool_definitions = EvidenceValue::Unsupported;
            }
            DetectorId::UnusedSkills => sources.skill_coverage = EvidenceValue::Unsupported,
            _ => unreachable!(),
        }
        let source = TokenBurnSourceEvidence {
            scope: "claude:bundled".to_owned(),
            name: if detector == DetectorId::UnusedBuiltInTools {
                "web_search".to_owned()
            } else {
                "unused".to_owned()
            },
            replicated_tokens: 100,
            invoked: false,
            replicated_cost_usd: None,
        };
        let mut token_evidence = SessionTokenBurnEvidence::default();
        match detector {
            DetectorId::UnusedMcpServers => token_evidence.mcp_sources = Some(vec![source]),
            DetectorId::UnusedBuiltInTools => {
                token_evidence.built_in_tool_sources = Some(vec![source]);
            }
            DetectorId::UnusedSkills => token_evidence.skill_sources = Some(vec![source]),
            _ => unreachable!(),
        }

        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session_with_token_burn(row, token_evidence);
        let report = accumulator.finish(context(CoverageCounts::default()));

        assert_eq!(
            report.detectors[detector.index()].eligible,
            1,
            "{detector:?}"
        );
        assert!(
            matches!(
                report.detector_statuses[detector.index()],
                DetectorStatus::Findings(_)
            ),
            "{detector:?}"
        );
    }
}

#[test]
fn report_time_sources_respect_built_in_applicability() {
    for (name, tokens, definitions, expected) in [
        (
            "Skill",
            100,
            EvidenceValue::Unsupported,
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
        ),
        (
            "read",
            0,
            EvidenceValue::Unsupported,
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
        ),
        (
            "read",
            100,
            EvidenceValue::Complete(BTreeMap::from([(
                "read".to_owned(),
                ToolDefinition {
                    tokens: 100,
                    invoked: false,
                    deferred: true,
                },
            )])),
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
        ),
        (
            "read",
            100,
            EvidenceValue::Partial {
                observed: BTreeMap::new(),
                reason: CoverageReason::AttributionIncomplete,
            },
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
        ),
    ] {
        let mut row = evidence_with_work("built-in-applicability");
        let EvidenceValue::Complete(sources) = &mut row.context_sources else {
            unreachable!()
        };
        sources.tool_definitions = definitions;
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session_with_token_burn(
            row,
            SessionTokenBurnEvidence {
                total_tokens: Some(1_000),
                built_in_tool_sources: Some(vec![TokenBurnSourceEvidence {
                    scope: "claude:bundled".to_owned(),
                    name: name.to_owned(),
                    replicated_tokens: tokens,
                    invoked: false,
                    replicated_cost_usd: None,
                }]),
                ..SessionTokenBurnEvidence::default()
            },
        );
        let report = accumulator.finish(context(CoverageCounts::default()));
        assert_eq!(
            report.detector_statuses[DetectorId::UnusedBuiltInTools.index()],
            expected,
            "{name}: {tokens}"
        );
    }
}

#[test]
fn idle_sessions_exclude_built_in_tools_without_source_attribution() {
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence("idle-built-in"));

    let report = accumulator.finish(context(CoverageCounts {
        ready: 1,
        discovered: 1,
        ..CoverageCounts::default()
    }));
    let counts = report.detectors[DetectorId::UnusedBuiltInTools.index()];

    assert_eq!(counts.not_applicable, 1);
    assert_eq!(counts.unavailable, 0);
}

#[test]
fn findings_without_a_denominator_use_a_bounded_fallback() {
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
        report.detector_estimated_token_burn_basis_points[DetectorId::SessionsOverDepth.index()],
        Some(500)
    );
    assert_eq!(report.estimated_token_burn_basis_points, Some(500));
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            1 << DetectorId::SessionsOverDepth.index(),
            &[],
        ),
        Some(500)
    );
    assert_eq!(report.estimated_token_burn_for_attributed_tokens(1), None);
}

#[test]
fn fallback_estimates_cover_each_detector_and_stay_bounded() {
    for detector in DetectorId::ALL {
        let estimate = fallback_token_burn_basis_points(detector, u64::MAX, 1).unwrap();
        assert!((1..=MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS).contains(&estimate));
    }
}

#[test]
fn positive_fallback_estimate_is_at_least_one_basis_point() {
    assert_eq!(
        fallback_token_burn_basis_points(DetectorId::SessionsOverDepth, 1, u64::MAX),
        Some(1)
    );
    assert_eq!(
        fallback_token_burn_basis_points(DetectorId::SessionsOverDepth, 0, u64::MAX),
        None
    );
}

#[test]
fn token_burn_estimates_use_attributed_tokens_from_an_incomplete_ready_cohort() {
    let mut evidence = evidence_with_work("observed");
    evidence.context = EvidenceValue::Complete(ContextEvidence {
        max_request_context_tokens: 400_001,
        top_depth_examples: Vec::new(),
    });

    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session_with_token_burn(
        evidence,
        SessionTokenBurnEvidence {
            total_tokens: Some(100),
            overdepth_avoidable_tokens: Some(20),
            ..SessionTokenBurnEvidence::default()
        },
    );
    let report = accumulator.finish(context(CoverageCounts {
        ready: 2,
        discovered: 2,
        ..CoverageCounts::default()
    }));

    assert_eq!(report.estimated_token_burn_basis_points, Some(2_000));
    assert_eq!(
        report.detector_estimated_token_burn_basis_points[DetectorId::SessionsOverDepth.index()],
        Some(2_000)
    );
}

#[test]
fn combined_token_burn_uses_the_largest_overlapping_contribution() {
    let mut findings = [false; DetectorId::COUNT];
    findings[DetectorId::SessionsOverDepth.index()] = true;
    findings[DetectorId::CacheChurn.index()] = true;

    let mut token_burn = TokenBurnAccumulator::new();
    token_burn.observe(
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            overdepth_avoidable_tokens: Some(800),
            repeated_context_avoidable_tokens: Some(700),
            ..SessionTokenBurnEvidence::default()
        },
        findings,
        [false; 3],
        true,
    );
    let mut statuses = core::array::from_fn(|_| {
        DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
    });
    for detector in [DetectorId::SessionsOverDepth, DetectorId::CacheChurn] {
        statuses[detector.index()] = DetectorStatus::Findings(detectors::DetectorFindings {
            finding_sessions: 1,
            examples: Vec::new(),
        });
    }
    let (combined, per_detector, _) = token_burn.finish(&statuses);

    assert_eq!(combined, Some(8_000));
    assert_eq!(
        per_detector[DetectorId::SessionsOverDepth.index()],
        Some(8_000)
    );
    assert_eq!(per_detector[DetectorId::CacheChurn.index()], Some(7_000));
}

#[test]
fn active_detector_mask_removes_one_overlapping_contribution() {
    let mut evidence = evidence_with_work("overlap");
    evidence.context = EvidenceValue::Complete(ContextEvidence {
        max_request_context_tokens: 400_001,
        top_depth_examples: Vec::new(),
    });
    let EvidenceValue::Complete(models) = &mut evidence.models else {
        unreachable!()
    };
    models.effort_tiers.insert(
        "max".to_owned(),
        TurnCounts {
            main_loop: 1,
            delegated: 0,
        },
    );
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session_with_token_burn(
        evidence,
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            overdepth_avoidable_tokens: Some(800),
            model_overthinking: Some(700),
            ..SessionTokenBurnEvidence::default()
        },
    );
    let mut report = accumulator.finish(context(CoverageCounts::default()));
    report.detector_statuses[DetectorId::ModelOverthinking.index()] =
        DetectorStatus::Findings(detectors::DetectorFindings {
            finding_sessions: 1,
            examples: Vec::new(),
        });
    report.token_burn_by_detector_by_session[DetectorId::ModelOverthinking.index()] =
        Some(vec![700]);

    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            (1 << DetectorId::SessionsOverDepth.index())
                | (1 << DetectorId::ModelOverthinking.index()),
            &[],
        ),
        Some(8_000)
    );
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            1 << DetectorId::ModelOverthinking.index(),
            &[],
        ),
        Some(7_000)
    );
}

#[test]
fn active_detector_aggregate_preserves_fallback_zero_and_null_semantics() {
    let mut finding = evidence_with_work("finding");
    finding.context = EvidenceValue::Complete(ContextEvidence {
        max_request_context_tokens: 400_001,
        top_depth_examples: Vec::new(),
    });
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session_with_token_burn(
        finding,
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            overdepth_avoidable_tokens: None,
            ..SessionTokenBurnEvidence::default()
        },
    );
    let report = accumulator.finish(context(CoverageCounts::default()));

    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            1 << DetectorId::SessionsOverDepth.index(),
            &[],
        ),
        Some(1_000)
    );
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(
            1 << DetectorId::UnusedMcpServers.index(),
            &[],
        ),
        None
    );
    assert_eq!(
        report.estimated_token_burn_for_active_detectors(0, &[]),
        None
    );

    let mut clean_accumulator = EfficiencyReportAccumulator::new();
    clean_accumulator.observe_session(evidence_with_work("clean"));
    let clean_report = clean_accumulator.finish(context(CoverageCounts::default()));
    assert_eq!(
        clean_report.estimated_token_burn_for_active_detectors(
            1 << DetectorId::SessionsOverDepth.index(),
            &[],
        ),
        Some(0)
    );
}

#[test]
fn token_burn_percentage_caps_before_the_wire_type_conversion() {
    let mut findings = [false; DetectorId::COUNT];
    findings[DetectorId::SessionsOverDepth.index()] = true;
    let mut token_burn = TokenBurnAccumulator::new();
    token_burn.observe(
        SessionTokenBurnEvidence {
            total_tokens: Some(1),
            overdepth_avoidable_tokens: Some(100_000),
            ..SessionTokenBurnEvidence::default()
        },
        findings,
        [false; 3],
        true,
    );
    let statuses = finding_statuses(&[DetectorId::SessionsOverDepth]);

    let (combined, estimates, _) = token_burn.finish(&statuses);

    assert_eq!(combined, Some(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS));
    assert_eq!(
        estimates[DetectorId::SessionsOverDepth.index()],
        Some(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS)
    );
}

fn finding_statuses(detectors: &[DetectorId]) -> [DetectorStatus; DetectorId::COUNT] {
    let mut statuses = core::array::from_fn(|_| {
        DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
    });
    for detector in detectors {
        statuses[detector.index()] = DetectorStatus::Findings(detectors::DetectorFindings {
            finding_sessions: 1,
            examples: Vec::new(),
        });
    }
    statuses
}

fn token_turn(
    scope: &str,
    model: &str,
    effort: Option<&str>,
    speed: Option<&str>,
    output_tokens: u64,
) -> TokenBurnTurnEvidence {
    TokenBurnTurnEvidence {
        scope: scope.to_owned(),
        model: model.to_owned(),
        effort: effort.map(str::to_owned),
        speed: speed.map(str::to_owned),
        ts_ms: Some(2_000_000_000_000),
        input_tokens: 0,
        output_tokens,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cache_write_1h_tokens: 0,
    }
}

fn turn_evidence(
    turns: impl IntoIterator<Item = TokenBurnTurnEvidence>,
    catalogs: &ReportCatalogs,
) -> SessionTokenBurnEvidence {
    let mut accumulator = TokenBurnTurnAccumulator::new(catalogs);
    for turn in turns {
        accumulator.observe(turn);
    }
    let mut evidence = SessionTokenBurnEvidence::default();
    accumulator.finish_into(&mut evidence);
    evidence
}

#[test]
fn token_cost_prices_one_hour_cache_writes_at_double_the_input_rate() {
    let pricing = ModelPricing {
        input_cost_per_token: 0.000_003,
        output_cost_per_token: 0.000_015,
        cache_read_cost_per_token: 0.000_000_3,
        cache_write_cost_per_token: 0.000_003_75,
    };
    let mut default_rate_turn = token_turn("main", "claude-sonnet-5", None, None, 0);
    default_rate_turn.cache_write_tokens = 1_000;
    default_rate_turn.cache_write_1h_tokens = 0;
    let mut one_hour_turn = token_turn("main", "claude-sonnet-5", None, None, 0);
    one_hour_turn.cache_write_tokens = 1_000;
    one_hour_turn.cache_write_1h_tokens = 1_000;

    let default_rate_cost = token_cost(&default_rate_turn, &pricing);
    let one_hour_cost = token_cost(&one_hour_turn, &pricing);

    let expected_delta =
        1_000.0 * (pricing.input_cost_per_token * 2.0 - pricing.cache_write_cost_per_token);
    assert!(
        (one_hour_cost - default_rate_cost - expected_delta).abs() < 1e-12,
        "expected cost delta {expected_delta}, got {}",
        one_hour_cost - default_rate_cost
    );
}

#[test]
fn findings_without_supported_prices_use_fallbacks() {
    let all_findings = DetectorId::ALL;
    let mut token_burn = TokenBurnAccumulator::new();
    let mut token_evidence = turn_evidence(
        [
            token_turn("main", "claude-sonnet-5", Some("max"), None, 1_000),
            token_turn("delegated", "claude-opus-4-6", None, None, 1_000),
            token_turn("main", "claude-opus-4-6", None, None, 1_000),
            token_turn("delegated", "gpt-5.6-sol", None, Some("fast"), 1_000),
        ],
        &ReportCatalogs::default(),
    );
    token_evidence.total_tokens = Some(10_000);
    token_evidence.overdepth_avoidable_tokens = Some(800);
    token_evidence.repeated_context_avoidable_tokens = Some(700);
    token_evidence.mcp_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "agent:user".to_owned(),
        name: "server".to_owned(),
        replicated_tokens: 100,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    token_evidence.built_in_tool_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "agent:bundled".to_owned(),
        name: "tool".to_owned(),
        replicated_tokens: 100,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    token_evidence.skill_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "agent:user".to_owned(),
        name: "skill".to_owned(),
        replicated_tokens: 100,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    token_burn.observe(token_evidence, [true; DetectorId::COUNT], [true; 3], true);
    let (combined, estimates, _) = token_burn.finish(&finding_statuses(&all_findings));

    assert_eq!(combined, Some(800));
    assert_eq!(
        estimates,
        [
            Some(800),
            Some(350),
            Some(2_500),
            Some(100),
            Some(100),
            Some(100),
            Some(400),
            Some(333),
            Some(700),
        ]
    );
}

#[test]
fn supported_evidence_estimates_each_check_independently() {
    let all_findings = DetectorId::ALL;
    let mut token_burn = TokenBurnAccumulator::new();
    let mut token_evidence = turn_evidence(
        [
            token_turn("main", "claude-sonnet-5", Some("max"), None, 1_000),
            token_turn("delegated", "gpt-6-astra", None, None, 1_000),
            token_turn("main", "gpt-5.4-mini", None, None, 1_000),
            token_turn("delegated", "gpt-5.6-sol", None, Some("fast"), 1_000),
        ],
        &ReportCatalogs::default(),
    );
    token_evidence.total_tokens = Some(10_000);
    token_evidence.overdepth_avoidable_tokens = Some(800);
    token_evidence.repeated_context_avoidable_tokens = Some(700);
    token_evidence.overpowered_subagents = Some(880);
    token_evidence.old_model = Some(400);
    token_evidence.mcp_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "agent:user".to_owned(),
        name: "server".to_owned(),
        replicated_tokens: 100,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    token_evidence.built_in_tool_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "agent:bundled".to_owned(),
        name: "tool".to_owned(),
        replicated_tokens: 100,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    token_evidence.skill_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "agent:user".to_owned(),
        name: "skill".to_owned(),
        replicated_tokens: 100,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    token_burn.observe(token_evidence, [true; DetectorId::COUNT], [true; 3], true);

    let (combined, estimates, _) = token_burn.finish(&finding_statuses(&all_findings));

    assert_eq!(combined, Some(880));
    assert_eq!(
        estimates,
        [
            Some(800),
            Some(350),
            Some(880),
            Some(100),
            Some(100),
            Some(100),
            Some(400),
            Some(333),
            Some(700),
        ]
    );
}

#[test]
fn source_estimate_overflow_does_not_hide_other_known_estimates() {
    let mut findings = [false; DetectorId::COUNT];
    findings[DetectorId::SessionsOverDepth.index()] = true;
    findings[DetectorId::UnusedBuiltInTools.index()] = true;
    let mut token_burn = TokenBurnAccumulator::new();
    token_burn.observe(
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            overdepth_avoidable_tokens: Some(100),
            built_in_tool_sources: Some(vec![
                TokenBurnSourceEvidence {
                    scope: "agent:bundled".to_owned(),
                    name: "tool".to_owned(),
                    replicated_tokens: u128::MAX,
                    invoked: false,
                    replicated_cost_usd: None,
                },
                TokenBurnSourceEvidence {
                    scope: "agent:bundled".to_owned(),
                    name: "tool".to_owned(),
                    replicated_tokens: 1,
                    invoked: false,
                    replicated_cost_usd: None,
                },
            ]),
            ..SessionTokenBurnEvidence::default()
        },
        findings,
        [false, true, false],
        true,
    );

    let (_, estimates, _) = token_burn.finish(&finding_statuses(&[
        DetectorId::SessionsOverDepth,
        DetectorId::UnusedBuiltInTools,
    ]));

    assert_eq!(
        estimates[DetectorId::SessionsOverDepth.index()],
        Some(1_000)
    );
    assert_eq!(estimates[DetectorId::UnusedBuiltInTools.index()], Some(500));
}

#[test]
fn comparable_lower_effort_output_overrides_the_tier_assumption() {
    let estimates = turn_evidence(
        [
            token_turn("main", "claude-sonnet-5", Some("high"), None, 100),
            token_turn("main", "claude-sonnet-5", Some("max"), None, 300),
        ],
        &ReportCatalogs::default(),
    );

    assert_eq!(estimates.model_overthinking, Some(200));
}

#[test]
fn equal_distance_comparisons_are_conservative_and_ignore_input_order() {
    let catalogs = ReportCatalogs::default();
    let mut turns = [
        token_turn("main", "claude-sonnet-5", Some("max"), None, 300),
        token_turn("main", "claude-sonnet-5", Some("high"), None, 100),
        token_turn("main", "claude-sonnet-5", Some("medium"), None, 200),
        token_turn("main", "claude-sonnet-5", Some("high"), None, 250),
    ];
    turns[0].input_tokens = 100;
    turns[1].input_tokens = 90;
    turns[2].input_tokens = 110;
    turns[3].input_tokens = 110;
    let expected = turn_evidence(turns.clone(), &catalogs).model_overthinking;

    turns.reverse();
    let reversed = turn_evidence(turns.clone(), &catalogs).model_overthinking;
    turns.rotate_left(1);
    let rotated = turn_evidence(turns, &catalogs).model_overthinking;

    assert_eq!(expected, Some(50));
    assert_eq!(reversed, expected);
    assert_eq!(rotated, expected);
}

#[test]
fn model_mechanisms_do_not_guess_savings_without_prices() {
    let catalogs = ReportCatalogs::default();
    let premium = token_turn("delegated", "gpt-5.5", None, None, 1_000);
    let fast = token_turn("delegated", "unpriced-model", None, Some("fast"), 1_000);
    let old = token_turn("main", "gpt-5.4-mini", None, None, 1_000);

    assert_eq!(
        turn_evidence([premium], &catalogs).overpowered_subagents,
        None
    );
    assert_eq!(turn_evidence([fast], &catalogs).fast_mode, None);
    assert_eq!(turn_evidence([old], &catalogs).old_model, None);
}

#[test]
fn astra_parent_usage_is_excluded_but_delegated_usage_has_savings() {
    let catalogs = ReportCatalogs::default();
    let evidence = turn_evidence(
        [
            token_turn("main", "gpt-6-astra", None, None, 1_000),
            token_turn("delegated", "gpt-6-astra", None, None, 1_000),
        ],
        &catalogs,
    );

    assert_eq!(evidence.overpowered_subagents, Some(880));
}

#[test]
fn namespaced_astra_fast_savings_use_fast_and_standard_rates() {
    let catalogs = ReportCatalogs::default();
    let turn = token_turn(
        "delegated",
        "openai/gpt-6-astra-20260901[272k]",
        None,
        Some("fast"),
        1_000,
    );

    assert_eq!(turn_evidence([turn], &catalogs).fast_mode, Some(500));
}

#[test]
fn effort_comparison_retention_is_bounded_and_uses_assumptions_after_the_bound() {
    let catalogs = ReportCatalogs::default();
    let mut accumulator = TokenBurnTurnAccumulator::new(&catalogs);
    for index in 0..=MAX_TOKEN_BURN_COMPARISON_TURNS {
        let effort = if index % 2 == 0 { "high" } else { "max" };
        accumulator.observe(token_turn(
            "main",
            "claude-sonnet-5",
            Some(effort),
            None,
            100,
        ));
    }

    assert!(accumulator.comparison_bound_exceeded);
    assert_eq!(accumulator.retained_comparison_turns(), 0);
    assert!(accumulator.comparison_groups.is_empty());
    let above_cap_turns = MAX_TOKEN_BURN_COMPARISON_TURNS / 2;
    let mut evidence = SessionTokenBurnEvidence::default();
    accumulator.finish_into(&mut evidence);
    let assumed_tokens = 35 * above_cap_turns as u128;
    assert_eq!(evidence.model_overthinking, Some(assumed_tokens));

    evidence.total_tokens = Some(409_700);
    let mut findings = [false; DetectorId::COUNT];
    findings[DetectorId::ModelOverthinking.index()] = true;
    let mut token_burn = TokenBurnAccumulator::new();
    token_burn.observe(evidence, findings, [false; 3], true);
    let (_, estimates, _) = token_burn.finish(&finding_statuses(&[DetectorId::ModelOverthinking]));
    assert_eq!(
        estimates[DetectorId::ModelOverthinking.index()],
        Some(1_750)
    );
}

#[test]
fn effort_comparison_uses_linear_index_operations_after_sorting() {
    let mut group = ComparisonGroup::default();
    for value in 0..1_000 {
        group.lower_effort.push(ComparisonCandidate {
            context_tokens: value as u128,
            output_tokens: value as u64,
            assumed_tokens: 0,
        });
        group.above_cap.push(ComparisonCandidate {
            context_tokens: value as u128,
            output_tokens: value as u64 + 1,
            assumed_tokens: 1,
        });
    }

    let estimate = overthinking_group_tokens(group);

    assert_eq!(estimate.operations, 3_000);
    assert_eq!(estimate.tokens, Some(1_000));
}

#[test]
fn indexed_effort_comparison_matches_the_quadratic_estimator_within_the_bound() {
    let catalogs = ReportCatalogs::default();
    let mut turns = Vec::new();
    for index in 0..200_u64 {
        let effort = match index % 4 {
            0 => "high",
            1 => "max",
            2 => "medium",
            _ => "xhigh",
        };
        let mut turn = token_turn(
            if index % 3 == 0 { "delegated" } else { "main" },
            if index % 5 == 0 {
                "anthropic/claude-sonnet-5"
            } else {
                "claude-sonnet-5"
            },
            Some(effort),
            None,
            50 + index % 37,
        );
        turn.input_tokens = 800 + index * 11 % 500;
        turns.push(turn);
    }
    let expected = turns.iter().fold(0_u128, |total, turn| {
        let effort = turn.effort.as_deref().unwrap().trim().to_lowercase();
        if !catalogs.families[&detectors::ModelFamily::Claude]
            .effort
            .above_cap
            .contains(&effort)
        {
            return total;
        }
        let turn_context = turn.context_tokens().unwrap();
        let observed = turns
            .iter()
            .filter(|candidate| {
                let candidate_effort = candidate.effort.as_deref().unwrap().trim().to_lowercase();
                let candidate_context = candidate.context_tokens().unwrap();
                let largest = turn_context.max(candidate_context);
                candidate.scope == turn.scope
                    && canonical_model_key(&candidate.model) == canonical_model_key(&turn.model)
                    && (largest == 0
                        || turn_context
                            .abs_diff(candidate_context)
                            .checked_mul(5)
                            .is_some_and(|difference| difference <= largest))
                    && catalogs.families[&detectors::ModelFamily::Claude]
                        .effort
                        .recognized
                        .contains(&candidate_effort)
                    && !catalogs.families[&detectors::ModelFamily::Claude]
                        .effort
                        .above_cap
                        .contains(&candidate_effort)
                    && candidate.output_tokens < turn.output_tokens
            })
            .map(|candidate| {
                (
                    turn_context.abs_diff(candidate.context_tokens().unwrap()),
                    u64::MAX - candidate.output_tokens,
                    candidate.context_tokens().unwrap(),
                    u128::from(turn.output_tokens - candidate.output_tokens),
                )
            })
            .min_by_key(|(difference, reverse_output, context, _)| {
                (*difference, *reverse_output, *context)
            })
            .map(|(_, _, _, tokens)| tokens);
        let assumed = match effort.as_str() {
            "xhigh" => percentage_of_tokens(u128::from(turn.output_tokens), 20),
            "max" | "ultra" => percentage_of_tokens(u128::from(turn.output_tokens), 35),
            _ => percentage_of_tokens(u128::from(turn.output_tokens), 10),
        }
        .unwrap();
        total + observed.unwrap_or(assumed)
    });

    assert_eq!(
        turn_evidence(turns, &catalogs).model_overthinking,
        Some(expected)
    );
}

#[test]
fn partial_model_totals_are_a_denominator_fallback() {
    let mut evidence = evidence_with_work("partial-models");
    evidence.context = EvidenceValue::Complete(ContextEvidence {
        max_request_context_tokens: 400_001,
        top_depth_examples: Vec::new(),
    });
    let EvidenceValue::Complete(mut models) = evidence.models else {
        unreachable!()
    };
    models.unattributed_turns = 2;
    models.by_model.insert(
        "claude-sonnet-5".to_owned(),
        ModelTokens {
            input: 100,
            output: 200,
            cache_read: 300,
            cache_creation: 400,
            ..ModelTokens::default()
        },
    );
    evidence.models = EvidenceValue::Partial {
        observed: models,
        reason: CoverageReason::AttributionIncomplete,
    };

    let mut token_evidence = SessionTokenBurnEvidence::from_session(&evidence);
    assert_eq!(token_evidence.total_tokens, Some(1_000));
    token_evidence.overdepth_avoidable_tokens = Some(100);
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session_with_token_burn(evidence, token_evidence);
    let report = accumulator.finish(context(CoverageCounts {
        discovered: 1,
        ready: 1,
        ..CoverageCounts::default()
    }));
    assert_eq!(
        report.detector_estimated_token_burn_basis_points[DetectorId::SessionsOverDepth.index()],
        Some(1_000)
    );
}

#[test]
fn partial_cache_evidence_keeps_observed_repeated_tokens() {
    let mut evidence = evidence_with_work("partial-cache");
    let EvidenceValue::Complete(mut cache) = evidence.cache else {
        unreachable!()
    };
    cache.repeated_context = EvidenceValue::Partial {
        observed: RepeatedContext {
            accounting: RepeatedContextAccounting::CacheWrite,
            repeated_tokens: 123,
            paid_tokens: 200,
            pairs_considered: 1,
            pairs_skipped: 0,
            transient_miss_episodes: 0,
            possible_rehydration_episodes: 0,
        },
        reason: CoverageReason::IncompleteTail,
    };
    evidence.cache = EvidenceValue::Partial {
        observed: cache,
        reason: CoverageReason::IncompleteTail,
    };

    assert_eq!(
        SessionTokenBurnEvidence::from_session(&evidence).repeated_context_avoidable_tokens,
        Some(123)
    );

    let EvidenceValue::Complete(models) = &mut evidence.models else {
        unreachable!()
    };
    models.unattributed_turns = 1;
    assert_eq!(
        SessionTokenBurnEvidence::from_session(&evidence).repeated_context_avoidable_tokens,
        None
    );
}

#[test]
fn an_unassessed_cache_ratio_does_not_become_zero_burn() {
    let mut token_burn = TokenBurnAccumulator::new();
    token_burn.observe(
        SessionTokenBurnEvidence {
            total_tokens: Some(34_678),
            repeated_context_avoidable_tokens: Some(19_992),
            ..SessionTokenBurnEvidence::default()
        },
        [false; DetectorId::COUNT],
        [false; 3],
        false,
    );

    assert_eq!(token_burn.sessions[0].repeated_context, None);
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
fn any_window_invocation_suppresses_the_exact_source_estimate() {
    let mut findings = [false; DetectorId::COUNT];
    findings[DetectorId::UnusedMcpServers.index()] = true;
    let mut token_burn = TokenBurnAccumulator::new();
    for index in 0..5 {
        token_burn.observe(
            SessionTokenBurnEvidence {
                total_tokens: Some(1_000),
                mcp_sources: Some(vec![TokenBurnSourceEvidence {
                    scope: "claude:unknown".to_owned(),
                    name: "server-a".to_owned(),
                    replicated_tokens: 100,
                    invoked: index == 4,
                    replicated_cost_usd: None,
                }]),
                ..SessionTokenBurnEvidence::default()
            },
            findings,
            [true, false, false],
            true,
        );
    }
    let mut statuses = core::array::from_fn(|_| {
        DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
    });
    statuses[DetectorId::UnusedMcpServers.index()] =
        DetectorStatus::Findings(detectors::DetectorFindings {
            finding_sessions: 5,
            examples: Vec::new(),
        });

    let (combined, estimates, _) = token_burn.finish(&statuses);

    assert_eq!(combined, Some(500));
    assert_eq!(estimates[DetectorId::UnusedMcpServers.index()], Some(500));
}

#[test]
fn cohort_token_burn_state_keeps_one_session_entry_and_one_entry_per_source_pair() {
    let mut findings = [false; DetectorId::COUNT];
    findings[DetectorId::UnusedMcpServers.index()] = true;
    let mut token_burn = TokenBurnAccumulator::new();
    for _ in 0..100 {
        token_burn.observe(
            SessionTokenBurnEvidence {
                total_tokens: Some(1_000),
                mcp_sources: Some(vec![TokenBurnSourceEvidence {
                    scope: "claude:user".to_owned(),
                    name: "server".to_owned(),
                    replicated_tokens: 100,
                    invoked: false,
                    replicated_cost_usd: None,
                }]),
                ..SessionTokenBurnEvidence::default()
            },
            findings,
            [true, false, false],
            true,
        );
    }

    assert_eq!(token_burn.sessions.len(), 100);
    assert_eq!(token_burn.sources[0].len(), 1);
    assert_eq!(
        token_burn.sources[0]
            .values()
            .map(|source| source.by_session.len())
            .sum::<usize>(),
        100
    );
}

#[test]
fn missing_source_projection_keeps_available_measured_tokens() {
    let mut finding = [false; DetectorId::COUNT];
    finding[DetectorId::UnusedMcpServers.index()] = true;
    let mut token_burn = TokenBurnAccumulator::new();
    token_burn.observe(
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            mcp_sources: Some(vec![TokenBurnSourceEvidence {
                scope: "claude:cwd:/project".to_owned(),
                name: "server-a".to_owned(),
                replicated_tokens: 100,
                invoked: false,
                replicated_cost_usd: None,
            }]),
            ..SessionTokenBurnEvidence::default()
        },
        finding,
        [true, false, false],
        true,
    );
    token_burn.observe(
        SessionTokenBurnEvidence {
            total_tokens: Some(1_000),
            ..SessionTokenBurnEvidence::default()
        },
        [false; DetectorId::COUNT],
        [true, false, false],
        true,
    );
    let statuses = finding_statuses(&[DetectorId::UnusedMcpServers]);

    let (combined, estimates, _) = token_burn.finish(&statuses);

    assert_eq!(combined, Some(500));
    assert_eq!(estimates[DetectorId::UnusedMcpServers.index()], Some(500));
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
fn combined_token_burn_adds_disjoint_unused_source_types() {
    let mut findings = [false; DetectorId::COUNT];
    findings[DetectorId::UnusedMcpServers.index()] = true;
    findings[DetectorId::UnusedSkills.index()] = true;
    let mut token_burn = TokenBurnAccumulator::new();
    for _ in 0..5 {
        token_burn.observe(
            SessionTokenBurnEvidence {
                total_tokens: Some(1_000),
                mcp_sources: Some(vec![TokenBurnSourceEvidence {
                    scope: "claude:unknown".to_owned(),
                    name: "server-a".to_owned(),
                    replicated_tokens: 100,
                    invoked: false,
                    replicated_cost_usd: None,
                }]),
                skill_sources: Some(vec![TokenBurnSourceEvidence {
                    scope: "claude:user".to_owned(),
                    name: "review".to_owned(),
                    replicated_tokens: 50,
                    invoked: false,
                    replicated_cost_usd: None,
                }]),
                ..SessionTokenBurnEvidence::default()
            },
            findings,
            [true, false, true],
            true,
        );
    }
    let mut statuses = core::array::from_fn(|_| {
        DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
    });
    for detector in [DetectorId::UnusedMcpServers, DetectorId::UnusedSkills] {
        statuses[detector.index()] = DetectorStatus::Findings(detectors::DetectorFindings {
            finding_sessions: 5,
            examples: Vec::new(),
        });
    }

    let (combined, estimates, _) = token_burn.finish(&statuses);

    assert_eq!(combined, Some(1_500));
    assert_eq!(estimates[DetectorId::UnusedMcpServers.index()], Some(1_000));
    assert_eq!(estimates[DetectorId::UnusedSkills.index()], Some(500));
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
    assert_eq!(
        report.provider_incidents,
        ProviderIncidentsSection::NotAssessed
    );
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
    // The missing harness version leaves nested tool definitions unsupported.
    assert_eq!(
        report.detector_statuses[DetectorId::UnusedBuiltInTools.index()],
        DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing)
    );
    for detector in [DetectorId::UnusedMcpServers, DetectorId::UnusedSkills] {
        assert_eq!(
            report.detector_statuses[detector.index()],
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
    }
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
        assert_eq!(counts.assessed, 0, "{detector:?}");
        assert_eq!(counts.clean, 0, "{detector:?}");
        assert_eq!(counts.unavailable, 2, "{detector:?}");
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
        source_format: crate::analysis::SourceFormat::ClaudeJsonl,
        request_context_tokens: true,
        cache_write_tokens: true,
        timestamps_and_order: true,
        tool_invocations: true,
        skill_inventory: true,
        mcp_inventory: true,
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
        provider_incidents: true,
        harness_version: true,
        repeated_context_accounting: Some(RepeatedContextAccounting::CacheWrite),
    };
    // An empty map, not a fabricated invoked definition: Unused
    // Built-In Tools reads clean from zero catalogued definitions
    // the same way Unused MCP Servers and Unused Skills read clean
    // from an empty `mcp_servers`/`skills` map, with no need to
    // invent a definition just to mark it invoked.
    let EvidenceValue::Complete(sources) = &mut row.context_sources else {
        unreachable!()
    };
    sources.tool_definitions = EvidenceValue::Complete(BTreeMap::new());
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

#[test]
fn resource_facts_read_independent_nested_coverage_in_either_wrapper() {
    for fact in [
        Fact::SkillInventory,
        Fact::McpInventory,
        Fact::ToolDefinitions,
    ] {
        for wrapper_partial in [false, true] {
            for expected in [
                FactState::Complete,
                FactState::Partial,
                FactState::Unsupported,
            ] {
                let mut row = complete_row("nested-coverage");
                row.capabilities.skill_inventory = false;
                row.capabilities.mcp_inventory = false;
                row.capabilities.tool_definitions = false;
                match expected {
                    FactState::Complete => (),
                    FactState::Partial => degrade_fact_to_partial(&mut row, fact),
                    FactState::Unsupported => degrade_fact_to_unsupported(&mut row, fact),
                }
                if wrapper_partial {
                    to_partial(&mut row.context_sources);
                }
                assert_eq!(
                    fact.state(&row),
                    expected,
                    "{fact:?}, partial wrapper: {wrapper_partial}"
                );
                for other in [
                    Fact::SkillInventory,
                    Fact::McpInventory,
                    Fact::ToolDefinitions,
                ] {
                    if other != fact {
                        assert_eq!(other.state(&row), FactState::Complete);
                    }
                }
            }
        }
        let mut row = complete_row("unsupported-wrapper");
        row.context_sources = EvidenceValue::Unsupported;
        assert_eq!(fact.state(&row), FactState::Unsupported);
    }
}

#[test]
fn coverage_contract_nested_resource_markers_gate_eligibility_but_not_inventory_clean() {
    for (detector, fact) in [
        (DetectorId::UnusedSkills, Fact::SkillInventory),
        (DetectorId::UnusedMcpServers, Fact::McpInventory),
        (DetectorId::UnusedBuiltInTools, Fact::ToolDefinitions),
    ] {
        for wrapper_partial in [false, true] {
            for marker in [
                FactState::Complete,
                FactState::Partial,
                FactState::Unsupported,
            ] {
                let mut row = complete_row("resource-marker-contract");
                match marker {
                    FactState::Complete => (),
                    FactState::Partial => degrade_fact_to_partial(&mut row, fact),
                    FactState::Unsupported => degrade_fact_to_unsupported(&mut row, fact),
                }
                if wrapper_partial {
                    to_partial(&mut row.context_sources);
                }
                assert_eq!(eligible(detector, &row), marker != FactState::Unsupported);
                assert!(!clean_facts_complete(detector, &row));
                assert_ne!(
                    status_for(row, detector),
                    DetectorStatus::Clean,
                    "{detector:?}/{marker:?}, partial wrapper: {wrapper_partial}"
                );
            }
        }
    }
}

#[test]
fn coverage_contract_partial_sessions_never_read_clean_from_complete_groups() {
    let mut failures = Vec::new();
    for format in [
        crate::analysis::SourceFormat::ClaudeJsonl,
        crate::analysis::SourceFormat::CodexRolloutJsonl,
        crate::analysis::SourceFormat::OpenCodeJsonl,
        crate::analysis::SourceFormat::OpenCodeSqliteV2,
        crate::analysis::SourceFormat::PiV3Jsonl,
    ] {
        let mut row = complete_row("partial-session-contract");
        row.capabilities.source_format = format;
        row.coverage = EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType);
        for detector in DetectorId::ALL {
            if status_for(row.clone(), detector) == DetectorStatus::Clean {
                failures.push(format!("{format:?}/{detector:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "partial sessions returned clean: {failures:?}"
    );
}

#[test]
fn coverage_contract_observed_resources_allow_findings_but_not_inventory_clean() {
    use crate::analysis::{ContextSourceKind, EvidenceObservation, NormalizedRecord};

    let mut failures = Vec::new();
    for (detector, observation) in [
        (
            DetectorId::UnusedSkills,
            EvidenceObservation::SkillInjection {
                name: "contract-skill".to_owned(),
                invoked: false,
            },
        ),
        (
            DetectorId::UnusedMcpServers,
            EvidenceObservation::ContextSource {
                kind: ContextSourceKind::McpServer,
                name: "contract-server".to_owned(),
                description: None,
            },
        ),
    ] {
        let mut sink = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "codex".to_owned(),
            session_id: "observed-resource-contract".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::codex(),
        });
        sink.observe(&NormalizedRecord::Observation(Box::new(observation)));
        let mut facts = TurnFacts::default();
        facts.eligibility.assistant_turns = 1;
        let observed = sink.evidence(&facts);
        assert!(!observed.capabilities.skill_inventory);
        assert!(!observed.capabilities.mcp_inventory);
        assert!(
            matches!(
                status_for(observed.clone(), detector),
                DetectorStatus::Findings(_)
            ),
            "{detector:?}"
        );
        let mut invoked = observed;
        let sources = match &mut invoked.context_sources {
            EvidenceValue::Complete(sources)
            | EvidenceValue::Partial {
                observed: sources, ..
            } => sources,
            EvidenceValue::Unsupported => panic!("the resource must be observed"),
        };
        for source in sources
            .skills
            .values_mut()
            .chain(sources.mcp_servers.values_mut())
        {
            source.invoked = true;
        }
        if status_for(invoked, detector) == DetectorStatus::Clean {
            failures.push(detector);
        }
    }
    assert!(
        failures.is_empty(),
        "observed resources cannot prove a complete inventory: {failures:?}"
    );
}

#[test]
fn unrelated_partial_resource_facts_do_not_erase_a_scoped_finding() {
    let mut row = complete_row("scoped-resource-finding");
    let EvidenceValue::Complete(sources) = &mut row.context_sources else {
        unreachable!()
    };
    sources.mcp_servers.insert(
        "server-a".to_owned(),
        LoadedSource {
            description: None,
            configured: true,
            available: true,
            injected: true,
            invoked: false,
            token_count: None,
            origin: EvidenceValue::Unsupported,
        },
    );
    sources.skill_coverage = EvidenceValue::Partial {
        observed: (),
        reason: crate::analysis::CoverageReason::MalformedRecord,
    };

    assert_eq!(
        status_for(row, DetectorId::UnusedMcpServers),
        DetectorStatus::Findings(detectors::DetectorFindings {
            finding_sessions: 1,
            examples: vec![SessionExample {
                agent: "claude".to_owned(),
                session_id: "scoped-resource-finding".to_owned(),
            }],
        })
    );
}

#[test]
fn coverage_contract_report_time_built_in_sources_do_not_prove_inventory_clean() {
    for invoked in [false, true] {
        let row = evidence_with_work("observed-built-in-contract");
        let mut accumulator = EfficiencyReportAccumulator::new();
        accumulator.observe_session_with_token_burn(
            row,
            SessionTokenBurnEvidence {
                built_in_tool_sources: Some(vec![TokenBurnSourceEvidence {
                    scope: "claude:bundled".to_owned(),
                    name: "web_search".to_owned(),
                    replicated_tokens: 100,
                    invoked,
                    replicated_cost_usd: None,
                }]),
                ..SessionTokenBurnEvidence::default()
            },
        );
        let report = accumulator.finish(context(CoverageCounts::default()));
        let status = &report.detector_statuses[DetectorId::UnusedBuiltInTools.index()];
        if invoked {
            assert_ne!(
                *status,
                DetectorStatus::Clean,
                "observed definitions do not prove the effective inventory"
            );
        } else {
            assert!(matches!(status, DetectorStatus::Findings(_)));
        }
    }
}

#[test]
fn coverage_contract_pi_observed_delegation_needs_no_blanket_capabilities() {
    use crate::analysis::{
        RelationConfidence, RelationProvenance, SubagentChild, SubagentEvidence,
    };

    for (worker, finding) in [("claude-opus-4-6", true), ("claude-haiku-4-5", false)] {
        let mut row = complete_row("pi-observed-delegation-contract");
        row.identity.agent = "pi".to_owned();
        row.capabilities = SourceCapabilities::pi();
        assert!(!row.capabilities.subagent_relationships);
        assert!(!row.capabilities.subagent_models);
        let EvidenceValue::Complete(models) = &mut row.models else {
            unreachable!()
        };
        models.dominant_main_model = Some("claude-opus-4-6".to_owned());
        row.subagents = EvidenceValue::Partial {
            observed: SubagentEvidence {
                spawn_count: 1,
                delegated_turns: 1,
                delegated_models: BTreeSet::from([worker.to_owned()]),
                children: vec![SubagentChild {
                    ordinal: 1,
                    parent_model: Some("claude-opus-4-6".to_owned()),
                    parent_call_id: Some("contract-call".to_owned()),
                    observed_child_models: BTreeSet::from([worker.to_owned()]),
                    child_model: EvidenceValue::Unsupported,
                    confidence: RelationConfidence::Observed,
                    provenance: RelationProvenance::TaskToolUse,
                }],
                examples: Vec::new(),
            },
            reason: CoverageReason::AttributionIncomplete,
        };
        let detector = DetectorId::OverpoweredSubagents;
        assert!(eligible(detector, &row));
        assert!(!clean_facts_complete(detector, &row));
        let status = status_for(row.clone(), detector);
        assert_ne!(status, DetectorStatus::Clean);
        assert_eq!(
            matches!(status, DetectorStatus::Findings(_)),
            finding,
            "{worker}"
        );
        row.subagents = EvidenceValue::Unsupported;
        assert!(!eligible(detector, &row));
        assert!(matches!(
            status_for(row, detector),
            DetectorStatus::NotAssessed(_)
        ));
    }
}

/// Degrades one fact's backing evidence from `Complete` to `Partial`.
/// `ThreadMembership` has no partial state (`Fact::state` maps its
/// capability flag straight to `Complete`/`Unsupported`), so this
/// unsets the capability instead: the fact still stops being
/// `Complete`, which is all a clean-only degrade needs to prove.
fn degrade_fact_to_partial(row: &mut SessionEvidence, fact: Fact) {
    match fact {
        Fact::MainLoopContext => to_partial(&mut row.context),
        Fact::ModelIdentity | Fact::EffortSignal | Fact::SpeedSignal => to_partial(&mut row.models),
        Fact::ToolInvocations => to_partial(&mut row.tools),
        Fact::SkillInventory | Fact::McpInventory | Fact::ToolDefinitions => {
            let EvidenceValue::Complete(sources) = &mut row.context_sources else {
                unreachable!()
            };
            match fact {
                Fact::SkillInventory => to_partial(&mut sources.skill_coverage),
                Fact::McpInventory => to_partial(&mut sources.mcp_coverage),
                _ => to_partial(&mut sources.tool_definitions),
            }
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
        Fact::SkillInventory | Fact::McpInventory | Fact::ToolDefinitions => {
            let EvidenceValue::Complete(sources) = &mut row.context_sources else {
                unreachable!()
            };
            match fact {
                Fact::SkillInventory => sources.skill_coverage = EvidenceValue::Unsupported,
                Fact::McpInventory => sources.mcp_coverage = EvidenceValue::Unsupported,
                _ => sources.tool_definitions = EvidenceValue::Unsupported,
            }
        }
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
        if matches!(
            detector,
            DetectorId::UnusedSkills
                | DetectorId::UnusedMcpServers
                | DetectorId::UnusedBuiltInTools
        ) {
            assert_ne!(baseline, DetectorStatus::Clean);
            continue;
        }
        assert_eq!(
            baseline,
            DetectorStatus::Clean,
            "baseline for {detector:?} must read clean so the degraded assertion distinguishes"
        );

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
/// `OverpoweredSubagents` is absent: its clean facts equal its
/// finding facts, so it has no clean-only fact left to degrade in
/// test (c) below.
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
                ModelFamily::Google => {
                    unreachable!("Google family has no effort policy")
                }
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
                    configured: true,
                    available: true,
                    injected: true,
                    invoked: false,
                    token_count: None,
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
                    configured: true,
                    available: true,
                    injected: true,
                    invoked: false,
                    token_count: None,
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
            let turns = TurnCounts {
                main_loop: 0,
                delegated: 2,
            };
            models
                .fast_modes
                .insert(FAST_SPEED_KEY.to_owned(), turns.clone());
            models
                .fast_modes_by_model
                .entry("claude-sonnet-4-6".to_owned())
                .or_default()
                .insert(FAST_SPEED_KEY.to_owned(), turns);
            models
                .by_model
                .insert("claude-sonnet-4-6".to_owned(), ModelTokens::default());
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
                transient_miss_episodes: 0,
                possible_rehydration_episodes: 1,
            });
        }
        DetectorId::UnusedBuiltInTools => {
            let EvidenceValue::Complete(sources) = &mut row.context_sources else {
                unreachable!()
            };
            sources.tool_definitions = EvidenceValue::Complete(BTreeMap::from([(
                "bash".to_owned(),
                ToolDefinition {
                    tokens: 100,
                    invoked: false,
                    deferred: false,
                },
            )]));
        }
        DetectorId::OverpoweredSubagents => {
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
            // OverpoweredSubagents has no clean-only fact (see
            // `trigger_finding`'s doc comment). UnusedBuiltInTools,
            // UnusedMcpServers, and UnusedSkills each have exactly
            // one, Eligibility, but their own `evaluate` bodies
            // require a complete eligibility group to report any
            // finding at all — Partial eligibility reads NoFinding
            // there, not Finding, by each rule's own documented
            // partial-evidence policy. Degrading their only
            // clean-only fact cannot demonstrate (c).
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
            let incomplete_ratio = detector == DetectorId::CacheChurn
                && !matches!(row.cache, EvidenceValue::Complete(_));
            let status = status_for_with_catalogs(row, detector, catalogs.clone());
            if incomplete_ratio {
                assert_eq!(
                    status,
                    DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
                );
                continue;
            }
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
    context_evidence.max_request_context_tokens = ReportCatalogs::default().depth_cap_tokens + 1;
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
    models.fast_modes_by_model.insert(
        "claude-sonnet-4-6".to_owned(),
        BTreeMap::from([(
            "fast".to_owned(),
            TurnCounts {
                main_loop: 0,
                delegated: 2,
            },
        )]),
    );
    models
        .by_model
        .insert("claude-sonnet-4-6".to_owned(), ModelTokens::default());
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
        reset_clock: None,
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
fn provider_section_is_not_assessed_without_transcript_provider_evidence() {
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(evidence("no-provider"));
    let report = accumulator.finish(context(CoverageCounts::default()));

    assert_eq!(
        report.provider_incidents,
        ProviderIncidentsSection::NotAssessed
    );
}

#[test]
fn provider_section_reports_deduplicated_transcript_incidents() {
    let hit = ProviderIncident {
        ts_ms: 700,
        kind: ProviderIncidentKind::Capacity,
        model: Some("model-a".to_owned()),
    };
    let mut row = evidence("provider-limited");
    row.provider_incidents = EvidenceValue::Complete(SessionProviderEvidence {
        incidents: vec![hit.clone(), hit],
    });
    let mut accumulator = EfficiencyReportAccumulator::new();
    accumulator.observe_session(row);
    let report = accumulator.finish(context(CoverageCounts::default()));

    let ProviderIncidentsSection::Findings(findings) = &report.provider_incidents else {
        panic!("expected provider findings");
    };
    assert_eq!(findings.total_hits, 1);
    assert_eq!(
        findings.hits_by_kind,
        BTreeMap::from([(ProviderIncidentKind::Capacity, 1)])
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
        let row = evidence_with_work(&format!("session-{index}"));
        accumulator.observe_session(row);
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
