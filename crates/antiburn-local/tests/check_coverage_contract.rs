use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::analysis::{
    ANALYZER_REVISION, CoverageReason, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage, EvidenceSource,
    EvidenceValue, PARSER_REVISION, SessionEvidence, SessionEvidenceAccumulator, SignalCoverage,
    SourceCapabilities, SourceFormat, SourceKind, TurnFacts,
};
use antiburn_local::insights::{
    BadgeStatus, CoverageCounts, DetectorId, DetectorStatus, EfficiencyReport,
    EfficiencyReportAccumulator, ReportCatalogs, ReportContext, ReportWindow, clean_facts_complete,
    session_badges,
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
    CursorChatStoreDb => "cursor_chat_store_db",
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
    ClineMessagesContractV1 => "cline_messages_contract_v1",
    KiroSessionJson => "kiro_session_json",
    KiroChat => "kiro_chat",
    KiroCliV2Bundle => "kiro_cli_v2_bundle",
    KiroCliV3Bundle => "kiro_cli_v3_bundle",
    KiroChatSaveExport => "kiro_chat_save_export",
    AmpThreadJson => "amp_thread_json",
    AmpFileChanges => "amp_file_changes",
    WindsurfWorkspaceJson => "windsurf_workspace_json",
    WindsurfMirrorJson => "windsurf_mirror_json",
    WindsurfCascadeProtobuf => "windsurf_cascade_protobuf",
    Uncharacterized => "uncharacterized",
}

fn complete_evidence(format: SourceFormat) -> SessionEvidence {
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

fn source_capabilities(format: SourceFormat) -> SourceCapabilities {
    let mut capabilities = match format {
        SourceFormat::ClaudeJsonl => SourceCapabilities::claude(),
        SourceFormat::CodexRolloutJsonl => SourceCapabilities::codex(),
        SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2 => {
            SourceCapabilities::opencode()
        }
        SourceFormat::PiV3Jsonl => SourceCapabilities::pi(),
        SourceFormat::CursorJsonl
        | SourceFormat::CursorCliAgentJsonl
        | SourceFormat::CursorCliStoreDb
        | SourceFormat::CursorChatStoreDb
        | SourceFormat::CursorIdeComposer
        | SourceFormat::CursorLegacyChatJson => SourceCapabilities::cursor(),
        SourceFormat::AntigravityJson
        | SourceFormat::AntigravityBrainJsonl
        | SourceFormat::AntigravityCascadeJson
        | SourceFormat::AntigravityWorkspaceChatJson
        | SourceFormat::AntigravitySqlite => SourceCapabilities::antigravity(),
        SourceFormat::CopilotCliJsonl
        | SourceFormat::CopilotIdeChatJson
        | SourceFormat::ClineSessionJson
        | SourceFormat::KiroSessionJson
        | SourceFormat::KiroChat
        | SourceFormat::KiroCliV2Bundle
        | SourceFormat::KiroCliV3Bundle
        | SourceFormat::KiroChatSaveExport
        | SourceFormat::AmpThreadJson
        | SourceFormat::AmpFileChanges
        | SourceFormat::WindsurfWorkspaceJson
        | SourceFormat::WindsurfMirrorJson
        | SourceFormat::WindsurfCascadeProtobuf
        | SourceFormat::Uncharacterized => SourceCapabilities::uncharacterized(format),
        SourceFormat::ClineMessagesContractV1 => SourceCapabilities::cline_messages_contract_v1(),
    };
    capabilities.source_format = format;
    capabilities
}

fn evidence_from_source_contract(format: SourceFormat) -> SessionEvidence {
    let facts = TurnFacts::default();
    SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "contract".to_owned(),
        session_id: source_keys(format).1.to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: source_capabilities(format),
    })
    .evidence(&facts)
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
        let mut row = evidence_from_source_contract(format);
        row.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);
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
fn source_formats_outside_the_clean_allowlist_deny_clean_with_synthetic_complete_facts() {
    for &format in SOURCE_FORMATS {
        if matches!(
            format,
            SourceFormat::ClaudeJsonl
                | SourceFormat::CodexRolloutJsonl
                | SourceFormat::OpenCodeJsonl
                | SourceFormat::OpenCodeSqliteV2
                | SourceFormat::PiV3Jsonl
                | SourceFormat::CopilotCliJsonl
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
fn coverage_documents_list_every_source_format_once_with_valid_statuses() {
    const CHECK_COVERAGE: &str = include_str!("../../../docs/check-coverage.md");
    const SESSION_COVERAGE: &str = include_str!("../../../docs/session-coverage.md");
    const CHECK_STATUSES: &[&str] = &["Assessable", "Partial", "Unsupported", "Unknown"];

    let expected: BTreeSet<_> = SOURCE_FORMATS
        .iter()
        .map(|format| source_keys(*format).0)
        .collect();

    let check_inventory =
        markdown_table_rows(CHECK_COVERAGE, "## Source Inventory", "## Coverage Matrix");
    assert_table_source_formats(&check_inventory, &expected, "check source inventory");

    let check_matrix = markdown_table_rows(
        CHECK_COVERAGE,
        "## Coverage Matrix",
        "## Evidence Boundaries",
    );
    assert_table_source_formats(&check_matrix, &expected, "check coverage matrix");
    for row in check_matrix {
        assert_eq!(row.len(), 10, "check coverage matrix has nine check cells");
        for status in &row[1..] {
            assert!(
                CHECK_STATUSES.contains(&status.as_str()),
                "invalid check coverage status {status:?}"
            );
        }
    }

    let session_matrix =
        markdown_table_rows(SESSION_COVERAGE, "## Source Matrix", "## Provider Routes");
    assert_table_source_formats(&session_matrix, &expected, "session source matrix");
}

#[test]
fn public_burn_check_table_keeps_fail_closed_readers_unavailable() {
    const SUPPORT: &str = include_str!("../../../docs/support.md");
    let rows = markdown_table_rows(SUPPORT, "## Burn Check remediation", "## Cost estimates");
    let results: BTreeMap<_, _> = rows
        .into_iter()
        .map(|row| {
            assert_eq!(row.len(), 4, "Burn Check support row has four cells");
            (row[0].clone(), row[1].clone())
        })
        .collect();

    for agent in ["Cline", "Kiro", "Amp", "Windsurf"] {
        assert_eq!(
            results.get(agent),
            Some(&"Unavailable".to_owned()),
            "{agent}"
        );
    }
    assert_eq!(
        results.get("GitHub Copilot"),
        Some(&"Supported S/O".to_owned())
    );
}

fn markdown_table_rows(document: &str, start: &str, end: &str) -> Vec<Vec<String>> {
    let section = document
        .split_once(start)
        .unwrap_or_else(|| panic!("missing section {start}"))
        .1
        .split_once(end)
        .unwrap_or_else(|| panic!("missing section end {end}"))
        .0;
    section
        .lines()
        .filter(|line| line.starts_with('|') && !line.contains("---"))
        .skip(1)
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').to_owned())
                .collect()
        })
        .collect()
}

fn assert_table_source_formats(rows: &[Vec<String>], expected: &BTreeSet<&str>, table: &str) {
    let actual: BTreeSet<_> = rows.iter().map(|row| row[0].as_str()).collect();
    assert_eq!(actual, *expected, "{table}");
    assert_eq!(
        rows.len(),
        expected.len(),
        "{table} has duplicate source formats"
    );
}
