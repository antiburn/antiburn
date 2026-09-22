use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use antiburn_local::analysis::{
    ANALYZER_REVISION, CoverageReason, EVIDENCE_SCHEMA_REVISION, EvidenceCoverage, EvidenceSource,
    EvidenceValue, PARSER_REVISION, SessionEvidence, SessionEvidenceAccumulator, SignalCoverage,
    SourceCapabilities, SourceFormat, SourceKind, TurnFacts,
};
use antiburn_local::insights::{
    BadgeStatus, CoverageCounts, DetectorId, DetectorStatus, EfficiencyReport,
    EfficiencyReportAccumulator, ReportCatalogs, ReportContext, ReportWindow, clean_facts_complete,
    eligible, fallback_token_burn_basis_points, session_badges,
};
use antiburn_local::model::AgentKind;
use antiburn_local::remediation::{
    BuiltInToolTokens, Finding, FindingCause, SavingsEstimateMethod, remediation_prompt,
    verification_evidence_supported,
};

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
    for detector in [DetectorId::OverpoweredSubagents, DetectorId::OldModelUsage] {
        assert!(clean_facts_complete(detector, &row), "{detector:?}");
    }
    assert!(!eligible(DetectorId::SessionsOverDepth, &row));
    assert!(!clean_facts_complete(DetectorId::SessionsOverDepth, &row));
    row.coverage = EvidenceCoverage::Partial(CoverageReason::MalformedRecord);
    for detector in [DetectorId::OverpoweredSubagents, DetectorId::OldModelUsage] {
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
    let check_coverage = coverage_document("check-coverage.md");
    let session_coverage = coverage_document("session-coverage.md");
    const CHECK_STATUSES: &[&str] = &["Assessable", "Partial", "Unsupported", "Unknown"];

    let expected: BTreeSet<_> = SOURCE_FORMATS
        .iter()
        .map(|format| source_keys(*format).0)
        .collect();

    let check_inventory =
        markdown_table_rows(&check_coverage, "## Source Inventory", "## Coverage Matrix");
    assert_table_source_formats(&check_inventory, &expected, "check source inventory");

    let check_matrix = markdown_table_rows(
        &check_coverage,
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
        markdown_table_rows(&session_coverage, "## Source Matrix", "## Provider Routes");
    assert_table_source_formats(&session_matrix, &expected, "session source matrix");
}

#[test]
fn first_tier_product_matrix_has_one_typed_row_per_agent_and_check() {
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
    assert_eq!(
        rows.len(),
        AGENTS.len() * DetectorId::COUNT,
        "one row per first-tier agent and check"
    );

    let mut keys = BTreeSet::new();
    for row in &rows {
        assert_eq!(row.len(), 8, "first-tier row has eight typed columns");
        assert!(
            detector_from_code(&row[1]).is_some(),
            "invalid check: {row:?}"
        );
        assert!(
            FINDINGS.contains(&row[2].as_str()),
            "invalid finding: {row:?}"
        );
        assert!(
            PROMPTS.contains(&row[3].as_str()),
            "invalid prompt: {row:?}"
        );
        assert!(
            AUTO_FIXES.contains(&row[4].as_str()),
            "invalid Auto Fix: {row:?}"
        );
        assert!(
            VERIFICATIONS.contains(&row[5].as_str()),
            "invalid verification: {row:?}"
        );
        assert!(
            BURN_ESTIMATES.contains(&row[6].as_str()),
            "invalid estimate: {row:?}"
        );
        assert!(
            !row[7].is_empty(),
            "missing adjacent reachability limit: {row:?}"
        );
        assert!(
            keys.insert((row[0].clone(), row[1].clone())),
            "duplicate row: {row:?}"
        );
    }
}

#[test]
fn first_tier_matrix_matches_engine_gates_and_reachable_routes() {
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

    let remediation_rows = remediation_matrix_rows(CHECK_COVERAGE);
    for (agent_index, (label, agent, source)) in agents.into_iter().enumerate() {
        for (detector_index, detector) in DetectorId::ALL.into_iter().enumerate() {
            let row = rows
                .iter()
                .find(|row| row[0] == label && detector_from_code(&row[1]) == Some(detector))
                .unwrap_or_else(|| panic!("missing first-tier row {label}/{detector:?}"));
            let documented = parse_product_support_columns(row);

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
            if documented.verification == VerificationSupport::Supported {
                assert!(
                    verification_evidence_supported(detector, source),
                    "engine verification evidence for {label}/{detector:?}"
                );
            }
            assert_eq!(
                documented.burn_estimate == BurnEstimateSupport::Supported,
                documented.finding != FindingSupport::Unavailable
                    && SavingsEstimateMethod::for_detector(detector)
                        == ESTIMATE_METHODS[detector_index]
                    && fallback_token_burn_basis_points(detector, 1, 1).is_some(),
                "burn estimate gate for {label}/{detector:?}"
            );
            assert!(
                documented.prompt == PromptSupport::Unavailable
                    || documented.finding != FindingSupport::Unavailable,
                "prompt requires a reachable finding for {label}/{detector:?}"
            );

            let source_row = remediation_rows
                .iter()
                .find(|row| row[0] == source_keys(source).0)
                .unwrap_or_else(|| panic!("missing remediation row for {source:?}"));
            let prompt_codes = parse_check_set(&source_row[1]);
            assert_eq!(
                prompt_codes.contains(&detector),
                documented.prompt == PromptSupport::Supported,
                "prompt recommendation and reachability for {label}/{detector:?}"
            );
            if matches!(
                detector,
                DetectorId::UnusedMcpServers
                    | DetectorId::UnusedBuiltInTools
                    | DetectorId::UnusedSkills
            ) {
                let finding = Finding::advisory_resource(
                    agent,
                    source,
                    resource_prompt_cause(agent, detector),
                )
                .expect("resource prompt fixture");
                assert_eq!(
                    remediation_prompt(&finding).is_ok(),
                    documented.prompt == PromptSupport::Supported,
                    "production recommendation for reachable resource {label}/{detector:?}"
                );
            }
        }
    }

    for (agent, check, expected) in [
        ("OpenCode", "M", AutoFixSupport::Conditional),
        ("OpenCode", "B", AutoFixSupport::PromptOnly),
        ("OpenCode", "K", AutoFixSupport::Conditional),
        ("Pi", "M", AutoFixSupport::PromptOnly),
        ("Pi", "B", AutoFixSupport::Unavailable),
        ("Pi", "K", AutoFixSupport::PromptOnly),
        ("Pi", "C", AutoFixSupport::Unavailable),
    ] {
        let row = rows
            .iter()
            .find(|row| row[0] == agent && row[1] == check)
            .unwrap_or_else(|| panic!("missing policy fixture {agent}/{check}"));
        assert_eq!(parse_product_support_columns(row).auto_fix, expected);
    }
}

#[test]
fn source_format_remediation_matrix_is_exhaustive_and_typed() {
    const CHECK_COVERAGE: &str = include_str!("../../../docs/check-coverage.md");
    let rows = remediation_matrix_rows(CHECK_COVERAGE);
    let expected: BTreeSet<_> = SOURCE_FORMATS
        .iter()
        .map(|format| source_keys(*format).0)
        .collect();
    assert_table_source_formats(&rows, &expected, "source-format remediation matrix");

    for row in rows {
        assert_eq!(row.len(), 6, "remediation row has six typed columns");
        for cell in &row[1..] {
            parse_check_set(cell);
        }
        let prompts = parse_check_set(&row[1]);
        let model = parse_check_set(&row[2]);
        let reasoning = parse_check_set(&row[3]);
        let other = parse_check_set(&row[4]);
        let verification = parse_check_set(&row[5]);
        assert!(
            model
                .iter()
                .all(|check| *check == DetectorId::OldModelUsage)
        );
        assert!(
            reasoning
                .iter()
                .all(|check| *check == DetectorId::ModelOverthinking)
        );
        assert!(model.is_subset(&prompts));
        assert!(reasoning.is_subset(&prompts));
        assert!(other.is_subset(&prompts));
        assert!(verification.is_subset(&prompts));
    }
}

#[test]
fn public_burn_check_table_keeps_fail_closed_readers_unavailable() {
    let support = coverage_document("support.md");
    let rows = markdown_table_rows(&support, "## Burn Check remediation", "## Cost estimates");
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
        Some(&"S/O on accepted CLI v1 bundles".to_owned())
    );
}

fn coverage_document(name: &str) -> String {
    read_coverage_document(Path::new(env!("CARGO_MANIFEST_DIR")), name)
        .unwrap_or_else(|error| panic!("cannot read coverage document {name}: {error}"))
}

fn read_coverage_document(manifest_dir: &Path, name: &str) -> std::io::Result<String> {
    let bundled_docs = manifest_dir.join("docs");
    let docs = if bundled_docs.is_dir() {
        bundled_docs
    } else {
        manifest_dir.join("../../docs")
    };
    std::fs::read_to_string(docs.join(name))
}

#[test]
fn coverage_documents_support_checkout_and_archive_layouts() {
    let root = tempfile::tempdir().unwrap();
    let manifest_dir = root.path().join("crates/antiburn-local");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/support.md"), "checkout").unwrap();
    assert_eq!(
        read_coverage_document(&manifest_dir, "support.md").unwrap(),
        "checkout"
    );

    std::fs::create_dir(manifest_dir.join("docs")).unwrap();
    std::fs::write(manifest_dir.join("docs/support.md"), "archive").unwrap();
    assert_eq!(
        read_coverage_document(&manifest_dir, "support.md").unwrap(),
        "archive"
    );
}

#[test]
fn missing_archive_documents_do_not_fall_back_to_checkout_documents() {
    let root = tempfile::tempdir().unwrap();
    let manifest_dir = root.path().join("crates/antiburn-local");
    std::fs::create_dir_all(manifest_dir.join("docs")).unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/support.md"), "checkout").unwrap();
    assert_eq!(
        read_coverage_document(&manifest_dir, "support.md")
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
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

fn parse_product_support_columns(row: &[String]) -> ProductSupport {
    assert_eq!(row.len(), 8, "product row has eight values: {row:?}");
    support(
        match row[2].as_str() {
            "Y" => FindingSupport::Supported,
            "FO" => FindingSupport::FindingOnly,
            "N" => FindingSupport::Unavailable,
            value => panic!("invalid finding support {value}"),
        },
        match row[3].as_str() {
            "Y" => PromptSupport::Supported,
            "N" => PromptSupport::Unavailable,
            value => panic!("invalid prompt support {value}"),
        },
        match row[4].as_str() {
            "Y" => AutoFixSupport::Supported,
            "C" => AutoFixSupport::Conditional,
            "P" => AutoFixSupport::PromptOnly,
            "N" => AutoFixSupport::Unavailable,
            value => panic!("invalid Auto Fix support {value}"),
        },
        match row[5].as_str() {
            "Y" => VerificationSupport::Supported,
            "N" => VerificationSupport::Unavailable,
            value => panic!("invalid verification support {value}"),
        },
        match row[6].as_str() {
            "Y" => BurnEstimateSupport::Supported,
            "N" => BurnEstimateSupport::Unavailable,
            value => panic!("invalid burn estimate support {value}"),
        },
    )
}

fn remediation_matrix_rows(document: &str) -> Vec<Vec<String>> {
    markdown_table_rows(
        document,
        "### Source-Format Remediation Matrix",
        "| Scope and environment",
    )
}

fn parse_check_set(value: &str) -> BTreeSet<DetectorId> {
    if value == "None" {
        return BTreeSet::new();
    }
    let values: Vec<_> = value.split('/').collect();
    let checks: BTreeSet<_> = values
        .iter()
        .map(|value| {
            detector_from_code(value).unwrap_or_else(|| panic!("invalid check code {value:?}"))
        })
        .collect();
    assert_eq!(
        checks.len(),
        values.len(),
        "duplicate check code in {value:?}"
    );
    checks
}

fn detector_from_code(value: &str) -> Option<DetectorId> {
    match value {
        "D" => Some(DetectorId::SessionsOverDepth),
        "T" => Some(DetectorId::ModelOverthinking),
        "S" => Some(DetectorId::OverpoweredSubagents),
        "M" => Some(DetectorId::UnusedMcpServers),
        "B" => Some(DetectorId::UnusedBuiltInTools),
        "K" => Some(DetectorId::UnusedSkills),
        "O" => Some(DetectorId::OldModelUsage),
        "F" => Some(DetectorId::OveruseOfFastMode),
        "C" => Some(DetectorId::CacheChurn),
        _ => None,
    }
}

fn resource_prompt_cause(agent: AgentKind, detector: DetectorId) -> FindingCause {
    match detector {
        DetectorId::UnusedMcpServers => FindingCause::UnusedMcpServer {
            server: "docs".to_owned(),
            tokens: Some(1),
            cost_usd: None,
            pricing_revision: None,
        },
        DetectorId::UnusedBuiltInTools => FindingCause::UnusedBuiltInTool {
            tool: match agent {
                AgentKind::Claude => "WebSearch",
                AgentKind::Codex => "web_search",
                AgentKind::OpenCode => "websearch",
                _ => "read",
            }
            .to_owned(),
            tokens: BuiltInToolTokens::Replicated(1),
            cost_usd: None,
            pricing_revision: None,
        },
        DetectorId::UnusedSkills => FindingCause::UnusedSkill {
            skill: "review".to_owned(),
            tokens: Some(1),
            cost_usd: None,
            pricing_revision: None,
        },
        _ => panic!("not a resource detector: {detector:?}"),
    }
}
