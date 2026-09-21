use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::analysis::{
    ANALYZER_REVISION, CoverageReason, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage, EvidenceSource,
    EvidenceValue, PARSER_REVISION, SessionEvidence, SessionEvidenceAccumulator, SignalCoverage,
    SourceCapabilities, SourceFormat, SourceKind, TurnFacts,
};
use antiburn_local::insights::{
    BadgeStatus, CoverageCounts, DetectorId, DetectorStatus, EfficiencyReport,
    EfficiencyReportAccumulator, ReportCatalogs, ReportContext, ReportWindow, clean_facts_complete,
    eligible, session_badges,
};
use antiburn_local::model::AgentKind;
use antiburn_local::remediation::{SavingsEstimateMethod, verification_evidence_supported};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindingSupport {
    Supported,
    FindingOnly,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSupport {
    Supported,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoFixSupport {
    Supported,
    Conditional,
    PromptOnly,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationSupport {
    Supported,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BurnEstimateSupport {
    Supported,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProductSupport {
    finding: FindingSupport,
    prompt: PromptSupport,
    auto_fix: AutoFixSupport,
    verification: VerificationSupport,
    burn_estimate: BurnEstimateSupport,
}

const fn support(
    finding: FindingSupport,
    prompt: PromptSupport,
    auto_fix: AutoFixSupport,
    verification: VerificationSupport,
    burn_estimate: BurnEstimateSupport,
) -> ProductSupport {
    ProductSupport {
        finding,
        prompt,
        auto_fix,
        verification,
        burn_estimate,
    }
}

const Y: FindingSupport = FindingSupport::Supported;
const FO: FindingSupport = FindingSupport::FindingOnly;
const N_FINDING: FindingSupport = FindingSupport::Unavailable;
const P: PromptSupport = PromptSupport::Supported;
const N_PROMPT: PromptSupport = PromptSupport::Unavailable;
const AF: AutoFixSupport = AutoFixSupport::Supported;
const C: AutoFixSupport = AutoFixSupport::Conditional;
const PO: AutoFixSupport = AutoFixSupport::PromptOnly;
const N_AUTO_FIX: AutoFixSupport = AutoFixSupport::Unavailable;
const V: VerificationSupport = VerificationSupport::Supported;
const N_VERIFICATION: VerificationSupport = VerificationSupport::Unavailable;
const E: BurnEstimateSupport = BurnEstimateSupport::Supported;
const N_ESTIMATE: BurnEstimateSupport = BurnEstimateSupport::Unavailable;

const ESTIMATE_METHODS: [SavingsEstimateMethod; DetectorId::COUNT] = [
    SavingsEstimateMethod::RepeatedContextAboveDepthCap,
    SavingsEstimateMethod::AssumedOutputReduction,
    SavingsEstimateMethod::WorkerModelPriceDifference,
    SavingsEstimateMethod::McpDefinitionExposure,
    SavingsEstimateMethod::BuiltInDefinitionReplication,
    SavingsEstimateMethod::InjectedSkillDocument,
    SavingsEstimateMethod::OldModelPriceDifference,
    SavingsEstimateMethod::FastTierPricePremium,
    SavingsEstimateMethod::CacheRehydrationPriceDifference,
];

const FIRST_TIER_PRODUCT_SUPPORT: [[ProductSupport; DetectorId::COUNT]; 6] = [
    [
        support(Y, P, AF, N_VERIFICATION, E),
        support(Y, P, AF, V, E),
        support(Y, P, AF, N_VERIFICATION, E),
        support(Y, P, C, N_VERIFICATION, E),
        support(Y, P, AF, N_VERIFICATION, E),
        support(Y, P, C, N_VERIFICATION, E),
        support(Y, P, AF, V, E),
        support(Y, P, AF, V, E),
        support(Y, P, N_AUTO_FIX, N_VERIFICATION, E),
    ],
    [
        support(Y, P, AF, N_VERIFICATION, E),
        support(Y, P, AF, V, E),
        support(Y, P, AF, N_VERIFICATION, E),
        support(Y, P, C, N_VERIFICATION, E),
        support(Y, P, N_AUTO_FIX, N_VERIFICATION, E),
        support(Y, P, C, N_VERIFICATION, E),
        support(Y, P, AF, V, E),
        support(Y, P, AF, V, E),
        support(Y, P, N_AUTO_FIX, N_VERIFICATION, E),
    ],
    [
        support(Y, P, AF, N_VERIFICATION, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(Y, P, AF, N_VERIFICATION, E),
        support(Y, P, PO, N_VERIFICATION, E),
        support(Y, P, PO, N_VERIFICATION, E),
        support(Y, P, C, N_VERIFICATION, E),
        support(Y, P, AF, V, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(Y, P, N_AUTO_FIX, N_VERIFICATION, E),
    ],
    [
        support(Y, P, AF, N_VERIFICATION, E),
        support(Y, P, AF, V, E),
        support(FO, P, N_AUTO_FIX, N_VERIFICATION, E),
        support(Y, P, PO, N_VERIFICATION, E),
        support(Y, P, PO, N_VERIFICATION, E),
        support(Y, P, PO, N_VERIFICATION, E),
        support(Y, P, AF, V, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(Y, P, N_AUTO_FIX, N_VERIFICATION, E),
    ],
    [
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(FO, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(FO, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, E),
        support(FO, P, N_AUTO_FIX, N_VERIFICATION, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
    ],
    [
        support(FO, P, N_AUTO_FIX, N_VERIFICATION, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(FO, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(FO, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, E),
        support(FO, P, PO, N_VERIFICATION, E),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
        support(N_FINDING, N_PROMPT, N_AUTO_FIX, N_VERIFICATION, N_ESTIMATE),
    ],
];

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
    DevinLocalSqlite => "devin_local_sqlite",
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
        SourceFormat::CopilotIdeChatJson
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
        | SourceFormat::DevinLocalSqlite
        | SourceFormat::Uncharacterized => SourceCapabilities::uncharacterized(format),
        SourceFormat::CopilotCliJsonl => SourceCapabilities::copilot(),
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
fn approved_clean_gates_require_complete_facts() {
    let mut row = complete_evidence(SourceFormat::CopilotCliJsonl);
    row.capabilities = SourceCapabilities::copilot();
    row.capabilities.source_format = SourceFormat::CopilotCliJsonl;
    for detector in [
        DetectorId::SessionsOverDepth,
        DetectorId::OverpoweredSubagents,
    ] {
        assert!(clean_facts_complete(detector, &row), "{detector:?}");
    }
    row.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);
    for detector in [
        DetectorId::SessionsOverDepth,
        DetectorId::OverpoweredSubagents,
    ] {
        assert!(!clean_facts_complete(detector, &row), "{detector:?}");
    }
}

#[test]
fn approved_finding_only_limits_never_turn_complete_facts_into_clean() {
    for format in [
        SourceFormat::ClineMessagesContractV1,
        SourceFormat::AmpThreadJson,
        SourceFormat::DevinLocalSqlite,
    ] {
        let row = complete_evidence(format);
        for detector in DetectorId::ALL {
            assert!(
                !clean_facts_complete(detector, &row),
                "{format:?}/{detector:?}"
            );
        }
    }
}

#[test]
fn approved_unavailable_limits_remain_unavailable() {
    for (format, capabilities, detectors) in [
        (
            SourceFormat::CursorJsonl,
            SourceCapabilities::cursor(),
            vec![
                DetectorId::SessionsOverDepth,
                DetectorId::OverpoweredSubagents,
                DetectorId::CacheChurn,
            ],
        ),
        (
            SourceFormat::AntigravityBrainJsonl,
            SourceCapabilities::antigravity(),
            vec![DetectorId::OverpoweredSubagents, DetectorId::CacheChurn],
        ),
        (
            SourceFormat::KiroCliV2Bundle,
            SourceCapabilities::uncharacterized(SourceFormat::KiroCliV2Bundle),
            vec![
                DetectorId::SessionsOverDepth,
                DetectorId::OverpoweredSubagents,
                DetectorId::CacheChurn,
            ],
        ),
    ] {
        let mut row = complete_evidence(format);
        row.capabilities = capabilities;
        row.capabilities.source_format = format;
        for detector in detectors {
            assert!(!eligible(detector, &row), "{format:?}/{detector:?}");
        }
    }
}

#[test]
fn non_core_control_and_cache_checks_do_not_gain_clean_applicability() {
    for format in [
        SourceFormat::CursorJsonl,
        SourceFormat::ClineMessagesContractV1,
        SourceFormat::KiroCliV2Bundle,
        SourceFormat::AmpThreadJson,
        SourceFormat::AntigravityBrainJsonl,
        SourceFormat::DevinLocalSqlite,
        SourceFormat::WindsurfWorkspaceJson,
    ] {
        let row = complete_evidence(format);
        for detector in [
            DetectorId::UnusedBuiltInTools,
            DetectorId::OveruseOfFastMode,
            DetectorId::CacheChurn,
        ] {
            assert!(
                !clean_facts_complete(detector, &row),
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
        "## First-Tier Product Matrix",
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
fn first_tier_product_matrix_has_six_documented_agents_and_valid_cells() {
    const CHECK_COVERAGE: &str = include_str!("../../../docs/check-coverage.md");
    const AGENTS: &[&str] = &[
        "Claude Code",
        "Codex",
        "OpenCode",
        "Pi",
        "Cursor",
        "Antigravity",
    ];
    const FINDINGS: &[&str] = &["Y", "FO", "N"];
    const PROMPTS: &[&str] = &["Y", "N"];
    const AUTO_FIXES: &[&str] = &["Y", "C", "P", "N"];
    const VERIFICATIONS: &[&str] = &["Y", "N"];
    const BURN_ESTIMATES: &[&str] = &["Y", "N"];

    let rows = markdown_table_rows(
        CHECK_COVERAGE,
        "## First-Tier Product Matrix",
        "## Second-Tier Product Coverage",
    );
    let documented: BTreeSet<_> = rows.iter().map(|row| row[0].as_str()).collect();
    let expected: BTreeSet<_> = AGENTS.iter().copied().collect();
    assert_eq!(documented, expected, "first-tier product agents");
    assert_eq!(rows.len(), AGENTS.len(), "first-tier product agent count");

    for row in rows {
        assert_eq!(row.len(), 10, "first-tier matrix has nine check cells");
        for cell in &row[1..] {
            let values: Vec<_> = cell.split('/').collect();
            assert_eq!(values.len(), 5, "product cell has five values: {cell}");
            assert!(
                FINDINGS.contains(&values[0]),
                "invalid finding value: {cell}"
            );
            assert!(PROMPTS.contains(&values[1]), "invalid prompt value: {cell}");
            assert!(
                AUTO_FIXES.contains(&values[2]),
                "invalid Auto Fix value: {cell}"
            );
            assert!(
                VERIFICATIONS.contains(&values[3]),
                "invalid verification value: {cell}"
            );
            assert!(
                BURN_ESTIMATES.contains(&values[4]),
                "invalid burn estimate value: {cell}"
            );
        }
    }
}

#[test]
fn first_tier_matrix_values_are_typed_and_match_engine_gates() {
    const CHECK_COVERAGE: &str = include_str!("../../../docs/check-coverage.md");
    let rows = markdown_table_rows(
        CHECK_COVERAGE,
        "## First-Tier Product Matrix",
        "## Second-Tier Product Coverage",
    );
    let agents = [
        ("Claude Code", AgentKind::Claude, SourceFormat::ClaudeJsonl),
        ("Codex", AgentKind::Codex, SourceFormat::CodexRolloutJsonl),
        ("OpenCode", AgentKind::OpenCode, SourceFormat::OpenCodeJsonl),
        ("Pi", AgentKind::Pi, SourceFormat::PiV3Jsonl),
        ("Cursor", AgentKind::Cursor, SourceFormat::CursorJsonl),
        (
            "Antigravity",
            AgentKind::Antigravity,
            SourceFormat::AntigravityBrainJsonl,
        ),
    ];

    for (agent_index, (label, _agent, source)) in agents.into_iter().enumerate() {
        let row = rows
            .iter()
            .find(|row| row[0] == label)
            .unwrap_or_else(|| panic!("missing first-tier row {label}"));
        for (detector_index, detector) in DetectorId::ALL.into_iter().enumerate() {
            let documented = parse_product_support(&row[detector_index + 1]);
            assert_eq!(
                documented, FIRST_TIER_PRODUCT_SUPPORT[agent_index][detector_index],
                "typed matrix value for {label}/{detector:?}"
            );

            let evidence = complete_evidence(source);
            // M/B/K product cells can be supplied by the desktop's current
            // inventory. The engine session gate covers the other checks.
            if !matches!(
                detector,
                DetectorId::UnusedMcpServers
                    | DetectorId::UnusedBuiltInTools
                    | DetectorId::UnusedSkills
            ) && !(agent_index == 3
                && matches!(
                    detector,
                    DetectorId::OverpoweredSubagents | DetectorId::CacheChurn
                ))
            {
                assert_eq!(
                    eligible(detector, &evidence),
                    !matches!(documented.finding, FindingSupport::Unavailable),
                    "finding gate for {label}/{detector:?}"
                );
            }
            assert_eq!(
                verification_evidence_supported(detector, source),
                documented.verification == VerificationSupport::Supported,
                "verification gate for {label}/{detector:?}"
            );
            assert_eq!(
                documented.burn_estimate == BurnEstimateSupport::Supported,
                documented.finding != FindingSupport::Unavailable
                    && SavingsEstimateMethod::for_detector(detector)
                        == ESTIMATE_METHODS[detector_index],
                "burn estimate gate for {label}/{detector:?}"
            );
        }
    }
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

    assert_eq!(results.get("Kiro"), Some(&"Unavailable".to_owned()));
    assert_eq!(results.get("Cline"), Some(&"Finding-only S/O".to_owned()));
    assert_eq!(results.get("Amp"), Some(&"Finding-only D/O".to_owned()));
    assert_eq!(results.get("Devin"), Some(&"Finding-only S".to_owned()));
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

fn parse_product_support(cell: &str) -> ProductSupport {
    let values: Vec<_> = cell.split('/').collect();
    assert_eq!(values.len(), 5, "product cell has five values: {cell}");
    support(
        match values[0] {
            "Y" => FindingSupport::Supported,
            "FO" => FindingSupport::FindingOnly,
            "N" => FindingSupport::Unavailable,
            value => panic!("invalid finding support {value}"),
        },
        match values[1] {
            "Y" => PromptSupport::Supported,
            "N" => PromptSupport::Unavailable,
            value => panic!("invalid prompt support {value}"),
        },
        match values[2] {
            "Y" => AutoFixSupport::Supported,
            "C" => AutoFixSupport::Conditional,
            "P" => AutoFixSupport::PromptOnly,
            "N" => AutoFixSupport::Unavailable,
            value => panic!("invalid Auto Fix support {value}"),
        },
        match values[3] {
            "Y" => VerificationSupport::Supported,
            "N" => VerificationSupport::Unavailable,
            value => panic!("invalid verification support {value}"),
        },
        match values[4] {
            "Y" => BurnEstimateSupport::Supported,
            "N" => BurnEstimateSupport::Unavailable,
            value => panic!("invalid burn estimate support {value}"),
        },
    )
}
