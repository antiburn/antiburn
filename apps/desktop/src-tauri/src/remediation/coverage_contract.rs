use super::*;
use antiburn_local::insights::fallback_token_burn_basis_points;

const CHECK_COVERAGE: &str = include_str!("../../../../../docs/check-coverage.md");

#[test]
fn first_tier_matrix_matches_reachable_desktop_capabilities() {
    let rows = markdown_table_rows(
        CHECK_COVERAGE,
        "## First-Tier Product Matrix",
        "## Second-Tier Product Coverage",
    );
    let agents = [
        ("Claude Code", AgentKind::Claude, SourceFormat::ClaudeJsonl),
        ("Codex", AgentKind::Codex, SourceFormat::CodexRolloutJsonl),
        (
            "OpenCode",
            AgentKind::OpenCode,
            SourceFormat::OpenCodeSqliteV2,
        ),
        ("Pi", AgentKind::Pi, SourceFormat::PiV3Jsonl),
        ("Cursor", AgentKind::Cursor, SourceFormat::CursorJsonl),
        (
            "Antigravity",
            AgentKind::Antigravity,
            SourceFormat::AntigravityBrainJsonl,
        ),
    ];

    for (label, agent, source) in agents {
        for detector in DetectorId::ALL {
            let row = rows
                .iter()
                .find(|row| row[0] == label && detector_from_code(&row[1]) == Some(detector))
                .unwrap_or_else(|| panic!("missing first-tier row {label}/{detector:?}"));
            let finding_reachable = row[2] != "N";

            assert_eq!(
                row[5] == "Y",
                verification_evidence_supported(detector, source)
                    && desktop_watch_verification_supported(detector, source),
                "verification for {label}/{detector:?}"
            );

            if let Some(cause) = resource_cause(agent, detector) {
                let desktop_finding = Finding::advisory_resource(agent, source, cause);
                assert_eq!(
                    desktop_finding.is_some(),
                    finding_reachable,
                    "desktop inventory finding for {label}/{detector:?}"
                );
                assert_eq!(
                    desktop_finding
                        .as_ref()
                        .is_some_and(|finding| remediation_prompt(finding).is_ok()),
                    row[3] == "Y",
                    "recommendation and prompt reachability for {label}/{detector:?}"
                );
            }

            let editor_supported = auto_fix_setting(agent, detector).is_some_and(|setting| {
                automatic_editor_supported(agent, setting, source, "global", "native", "macos")
            });
            assert_eq!(
                matches!(row[4].as_str(), "Y" | "C"),
                editor_supported,
                "production editor policy for {label}/{detector:?}"
            );
            if matches!(row[4].as_str(), "Y" | "C")
                && let Some(cause) = resource_cause(agent, detector)
            {
                assert_eq!(
                    reviewed_config_operation(agent, &cause).map(|operation| operation.setting),
                    auto_fix_setting(agent, detector),
                    "resource target binding for {label}/{detector:?}"
                );
            }

            assert_eq!(
                row[6] == "Y",
                finding_reachable && fallback_token_burn_basis_points(detector, 1, 1).is_some(),
                "burn estimate path for {label}/{detector:?}"
            );
        }
    }
}

fn resource_cause(agent: AgentKind, detector: DetectorId) -> Option<FindingCause> {
    match detector {
        DetectorId::UnusedMcpServers => Some(FindingCause::UnusedMcpServer {
            server: "docs".into(),
            tokens: Some(1),
            cost_usd: None,
            pricing_revision: None,
        }),
        DetectorId::UnusedBuiltInTools => {
            optional_tool(agent).map(|tool| FindingCause::UnusedBuiltInTool {
                tool: tool.into(),
                tokens: antiburn_local::remediation::BuiltInToolTokens::Replicated(1),
                cost_usd: None,
                pricing_revision: None,
            })
        }
        DetectorId::UnusedSkills => Some(FindingCause::UnusedSkill {
            skill: "review".into(),
            tokens: Some(1),
            cost_usd: None,
            pricing_revision: None,
        }),
        _ => None,
    }
}

const fn optional_tool(agent: AgentKind) -> Option<&'static str> {
    match agent {
        AgentKind::Claude => Some("WebSearch"),
        AgentKind::Codex => Some("web_search"),
        AgentKind::OpenCode => Some("websearch"),
        _ => None,
    }
}

fn auto_fix_setting(agent: AgentKind, detector: DetectorId) -> Option<ConfigSetting> {
    let setting = match detector {
        DetectorId::SessionsOverDepth => ConfigSetting::Compaction,
        DetectorId::ModelOverthinking => ConfigSetting::Reasoning,
        DetectorId::OverpoweredSubagents => ConfigSetting::SubagentModel,
        DetectorId::UnusedMcpServers => ConfigSetting::McpServer,
        DetectorId::UnusedBuiltInTools => ConfigSetting::BuiltInTool,
        DetectorId::UnusedSkills => ConfigSetting::Skill,
        DetectorId::OldModelUsage => ConfigSetting::Model,
        DetectorId::OveruseOfFastMode => ConfigSetting::FastMode,
        DetectorId::CacheChurn | DetectorId::IgnoredInstructions => return None,
    };
    if detector == DetectorId::UnusedBuiltInTools && optional_tool(agent).is_none() {
        None
    } else {
        Some(setting)
    }
}

fn markdown_table_rows(document: &str, start: &str, end: &str) -> Vec<Vec<String>> {
    document
        .split_once(start)
        .unwrap_or_else(|| panic!("missing section {start}"))
        .1
        .split_once(end)
        .unwrap_or_else(|| panic!("missing section end {end}"))
        .0
        .lines()
        .filter(|line| line.starts_with('|') && !line.contains("---"))
        .skip(1)
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect()
        })
        .collect()
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
        "I" => Some(DetectorId::IgnoredInstructions),
        _ => None,
    }
}
