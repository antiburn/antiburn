use std::collections::BTreeMap;

use antiburn_local::analysis::{
    ANALYZER_REVISION, CoverageReason, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage, EvidenceSource,
    EvidenceValue, ModelTokens, PARSER_REVISION, SessionEvidence, SessionEvidenceAccumulator,
    SignalCoverage, SourceCapabilities, SourceFormat, SourceKind, TurnFacts,
};
use antiburn_local::insights::{
    BadgeId, BadgeStatus, CoverageCounts, DetectorId, DetectorStatus, EfficiencyReport,
    EfficiencyReportAccumulator, ModelReplacementEntry, ReportCatalogs, ReportContext,
    ReportWindow, clean_facts_complete, session_badges,
};

macro_rules! source_formats {
    ($($variant:ident => $wire:literal),+ $(,)?) => {
        const SOURCE_FORMATS: &[SourceFormat] = &[$(SourceFormat::$variant),+];

        fn source_keys(format: SourceFormat) -> (&'static str, &'static str) {
            match format {
                $(SourceFormat::$variant => (stringify!($variant), $wire)),+
            }
        }
    };
}

source_formats! {
    ClaudeJsonl => "claude_jsonl",
    CodexRolloutJsonl => "codex_rollout_jsonl",
    OpenCodeJsonl => "open_code_jsonl",
    OpenCodeSqliteV2 => "open_code_sqlite_v2",
    PiV3Jsonl => "pi_v3_jsonl",
    CursorJsonl => "cursor_jsonl",
    CursorCliAgentJsonl => "cursor_cli_agent_jsonl",
    CursorCliStoreDb => "cursor_cli_store_db",
    CursorIdeComposer => "cursor_ide_composer",
    CursorLegacyChatJson => "cursor_legacy_chat_json",
    AntigravityJson => "antigravity_json",
    AntigravityBrainJsonl => "antigravity_brain_jsonl",
    AntigravityCascadeJson => "antigravity_cascade_json",
    AntigravityWorkspaceChatJson => "antigravity_workspace_chat_json",
    AntigravitySqlite => "antigravity_sqlite",
    CopilotCliJsonl => "copilot_cli_jsonl",
    CopilotIdeChatJson => "copilot_ide_chat_json",
    ClineSessionJson => "cline_session_json",
    KiroSessionJson => "kiro_session_json",
    KiroChat => "kiro_chat",
    AmpThreadJson => "amp_thread_json",
    AmpFileChanges => "amp_file_changes",
    WindsurfWorkspaceJson => "windsurf_workspace_json",
    WindsurfMirrorJson => "windsurf_mirror_json",
    WindsurfCascadeProtobuf => "windsurf_cascade_protobuf",
    Uncharacterized => "uncharacterized",
}

fn complete_evidence(format: SourceFormat) -> SessionEvidence {
    // The format changes without changing facts, so source gates cannot hide behind missing capabilities.
    let mut facts = TurnFacts::default();
    facts.eligibility.assistant_turns = 1;
    let mut row = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: source_keys(format).1.to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&facts);
    row.capabilities.source_format = format;
    let EvidenceValue::Complete(sources) = &mut row.context_sources else {
        panic!("complete context sources");
    };
    sources.skill_coverage = EvidenceValue::Complete(());
    sources.mcp_coverage = EvidenceValue::Complete(());
    sources.tool_definitions = EvidenceValue::Complete(BTreeMap::new());
    let EvidenceValue::Complete(models) = &mut row.models else {
        panic!("complete models");
    };
    models.effort_signal = SignalCoverage {
        eligible_turns: 1,
        present_turns: 1,
    };
    models.speed_signal = models.effort_signal;
    row
}

fn partial<T>(value: &mut EvidenceValue<T>) {
    let EvidenceValue::Complete(observed) = std::mem::take(value) else {
        panic!("the test must degrade complete evidence");
    };
    *value = EvidenceValue::Partial {
        observed,
        reason: CoverageReason::MalformedRecord,
    };
}

fn report(row: SessionEvidence, catalogs: ReportCatalogs) -> EfficiencyReport {
    let mut accumulator = EfficiencyReportAccumulator::with_catalogs(catalogs);
    accumulator.observe_session(row);
    accumulator.finish(ReportContext {
        environment_key: "contract".to_owned(),
        window: ReportWindow {
            start_epoch: 0,
            end_epoch: 1,
        },
        computed_at_epoch: 1,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage: CoverageCounts {
            ready: 1,
            discovered: 1,
            ..CoverageCounts::default()
        },
    })
}

#[test]
fn every_source_and_detector_denies_clean_on_partial_facts() {
    for &format in SOURCE_FORMATS {
        let mut row = complete_evidence(format);
        row.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);
        partial(&mut row.context);
        partial(&mut row.models);
        partial(&mut row.tools);
        partial(&mut row.eligibility);
        partial(&mut row.subagents);
        partial(&mut row.cache);
        partial(&mut row.time_range);
        partial(&mut row.compactions);
        for detector in DetectorId::ALL {
            assert!(
                !clean_facts_complete(detector, &row),
                "{format:?}/{detector:?}"
            );
        }
        let catalogs = ReportCatalogs::default();
        for badge in session_badges(&row, &catalogs) {
            assert_ne!(
                badge.status,
                BadgeStatus::Clean,
                "{format:?}/{:?}",
                badge.id
            );
        }
        let report = report(row, catalogs);
        for detector in DetectorId::ALL {
            assert_ne!(
                report.detector_statuses[detector.index()],
                DetectorStatus::Clean,
                "{format:?}/{detector:?}"
            );
            assert_eq!(
                report.detectors[detector.index()].clean,
                0,
                "{format:?}/{detector:?}"
            );
        }
    }
}

#[test]
fn uncharacterized_source_contracts_deny_clean_even_with_complete_facts() {
    for &format in SOURCE_FORMATS {
        if matches!(
            format,
            SourceFormat::ClaudeJsonl
                | SourceFormat::CodexRolloutJsonl
                | SourceFormat::OpenCodeJsonl
                | SourceFormat::OpenCodeSqliteV2
                | SourceFormat::PiV3Jsonl
        ) {
            continue;
        }
        let row = complete_evidence(format);
        for detector in DetectorId::ALL {
            assert!(
                !clean_facts_complete(detector, &row),
                "{format:?}/{detector:?}"
            );
        }
        let report = report(row, ReportCatalogs::default());
        for detector in DetectorId::ALL {
            assert_ne!(
                report.detector_statuses[detector.index()],
                DetectorStatus::Clean,
                "{format:?}/{detector:?}"
            );
        }
    }
}

#[test]
fn every_source_preserves_direct_depth_and_old_model_findings() {
    let mut catalogs = ReportCatalogs::default();
    catalogs.model_replacements.entries.insert(
        "contract-old-model".to_owned(),
        ModelReplacementEntry {
            replacement: "contract-new-model".to_owned(),
            available_since_ts_ms: 100,
            rationale: "Synthetic replacement rule".to_owned(),
            source_url: "https://example.invalid/model".to_owned(),
        },
    );
    for &format in SOURCE_FORMATS {
        for incomplete in [false, true] {
            let mut row = complete_evidence(format);
            let EvidenceValue::Complete(context) = &mut row.context else {
                unreachable!()
            };
            context.max_request_context_tokens = catalogs.depth_cap_tokens + 1;
            let EvidenceValue::Complete(models) = &mut row.models else {
                unreachable!()
            };
            models.by_model.insert(
                "contract-old-model".to_owned(),
                ModelTokens {
                    turns: 1,
                    first_ts_ms: 100,
                    last_ts_ms: 100,
                    ..ModelTokens::default()
                },
            );
            if incomplete {
                row.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);
                partial(&mut row.context);
                partial(&mut row.models);
            }
            for badge in session_badges(&row, &catalogs) {
                if matches!(badge.id, BadgeId::SessionOverdepth | BadgeId::ObsoleteModel) {
                    assert_eq!(
                        badge.status,
                        BadgeStatus::Finding,
                        "{format:?}/{:?}, partial: {incomplete}",
                        badge.id
                    );
                }
            }
            let report = report(row, catalogs.clone());
            for detector in [DetectorId::SessionsOverDepth, DetectorId::OldModelUsage] {
                assert!(
                    matches!(
                        report.detector_statuses[detector.index()],
                        DetectorStatus::Findings(_)
                    ),
                    "{format:?}/{detector:?}, partial: {incomplete}"
                );
                assert_eq!(report.detectors[detector.index()].finding, 1);
            }
        }
    }
}
