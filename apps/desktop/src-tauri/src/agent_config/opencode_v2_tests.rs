use std::fs;

use antiburn_local::analysis::SourceFormat;
use antiburn_local::model::AgentKind;

use super::*;

const V2_SOURCES: [SourceFormat; 2] = [
    SourceFormat::OpenCodeSqliteV2,
    SourceFormat::OpenCodeV2SessionExport,
];

#[test]
fn v2_config_controls_do_not_enter_the_v1_editor() {
    let cases = [
        (
            ConfigSetting::Model,
            "openai/old",
            r#"{"model":"openai/old"}"#,
        ),
        (
            ConfigSetting::Compaction,
            "false",
            r#"{"compaction":{"auto":false,"buffer":20000,"keep":{"tokens":15000}}}"#,
        ),
        (
            ConfigSetting::Compaction,
            "40000",
            r#"{"compaction":{"auto":true,"reserved":40000,"buffer":20000,"tail_turns":8,"prune":true}}"#,
        ),
        (
            ConfigSetting::SubagentModel,
            "openai/old",
            r#"{"agent":{"review":{"model":"openai/old"}},"agents":{"review":{"model":"openai/new"}}}"#,
        ),
        (
            ConfigSetting::McpServer,
            "docs",
            r#"{"mcp":{"docs":{"type":"local","command":["docs"],"enabled":true},"servers":{"docs":{"type":"remote","url":"https://example.invalid/mcp","disabled":false}}}}"#,
        ),
        (
            ConfigSetting::McpServer,
            "docs",
            r#"{"mcp":{"docs":{"enabled":true}}}"#,
        ),
        (
            ConfigSetting::BuiltInTool,
            "websearch",
            r#"{"permissions":[{"action":"websearch","resource":"*","effect":"allow"}]}"#,
        ),
        (
            ConfigSetting::Skill,
            "review",
            r#"{"permissions":[{"action":"skill","resource":"*","effect":"deny"},{"action":"skill","resource":"review","effect":"allow"}],"policies":[{"action":"integration.use","resource":"skill:review","effect":"deny"}]}"#,
        ),
    ];
    for source in V2_SOURCES {
        for (setting, expected, contents) in cases {
            let temporary = tempfile::tempdir().unwrap();
            let home = temporary.path().join("home");
            let config = home.join(".config/opencode/opencode.jsonc");
            fs::create_dir_all(config.parent().unwrap()).unwrap();
            fs::write(&config, contents).unwrap();
            let mut context = ConfigContext::native(AgentKind::OpenCode, &home, None);
            context.source_format = Some(source);
            let editor = AgentConfigEditor::new();
            assert_eq!(
                editor.effective_for_value(&context, setting, Some(expected)),
                Err(ConfigUnavailableReason::UnsupportedSetting),
                "{setting:?}"
            );
            assert!(matches!(
                editor.prepare_operation(
                    &context,
                    &ConfigOperation {
                        setting,
                        expected_value: expected.into(),
                        proposed_value: "replacement".into(),
                    }
                ),
                Err(ConfigUnavailableReason::UnsupportedSetting)
            ));
            assert_eq!(fs::read_to_string(&config).unwrap(), contents);
            assert!(!config.with_extension("jsonc.bak").exists());
        }
    }
}

#[test]
fn v2_inventory_requires_native_context_and_keeps_export_unsupported() {
    let temporary = tempfile::tempdir().unwrap();
    let mut context = ConfigContext::native(AgentKind::OpenCode, temporary.path(), None);
    for (source, expected) in [
        (
            SourceFormat::OpenCodeSqliteV2,
            ConfigUnavailableReason::UnsafePath,
        ),
        (
            SourceFormat::OpenCodeV2SessionExport,
            ConfigUnavailableReason::UnsupportedSetting,
        ),
    ] {
        context.source_format = Some(source);
        assert_eq!(advisory_resource_inventory(&context, []), Err(expected));
        assert!(matches!(
            skill_opportunity_snapshot(&context),
            Err(SkillSnapshotError::Config(
                ConfigUnavailableReason::UnsupportedSetting
            ))
        ));
    }
}

#[test]
fn v1_sources_keep_the_existing_model_and_compaction_targets() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path();
    let config = home.join(".config/opencode/opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        r#"{"model":"openai/old","compaction":{"auto":false}}"#,
    )
    .unwrap();
    for source in [SourceFormat::OpenCodeJsonl, SourceFormat::OpenCodeSqliteV1] {
        let mut context = ConfigContext::native(AgentKind::OpenCode, home, None);
        context.source_format = Some(source);
        let editor = AgentConfigEditor::new();
        assert_eq!(
            editor.effective_model(&context).unwrap().value,
            "openai/old"
        );
        assert_eq!(
            editor
                .effective(&context, ConfigSetting::Compaction)
                .unwrap()
                .value,
            "false"
        );
    }
}

#[cfg(not(windows))]
#[test]
fn v1_source_cannot_authorize_a_current_config_edit() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path();
    let config = home.join(".config/opencode/opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = r#"{"model":"openai/old","compaction":{"tail_turns":8,"prune":true},"mcp":{"docs":{"type":"local","command":["docs"],"enabled":true}}}"#;
    fs::write(&config, original).unwrap();
    let mut context = ConfigContext::native(AgentKind::OpenCode, home, None);
    context.source_format = Some(SourceFormat::OpenCodeSqliteV1);
    let editor = AgentConfigEditor::new();
    let result = editor.prepare(
        &context,
        &ConfigChange {
            expected_value: "openai/old".into(),
            proposed_value: "openai/new".into(),
        },
    );
    assert!(matches!(
        result,
        Err(ConfigUnavailableReason::RuntimeOverride)
    ));
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!config.with_extension("json.bak").exists());
}

#[test]
fn native_agents_block_a_legacy_root_model_target() {
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join(".config/opencode/opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        r#"{"model":"openai/old","agents":{"build":{"model":"openai/new"}}}"#,
    )
    .unwrap();
    let context = ConfigContext::native(AgentKind::OpenCode, temporary.path(), None);
    assert_eq!(
        AgentConfigEditor::new().effective_model(&context),
        Err(ConfigUnavailableReason::InvalidPrecedence)
    );
}

#[test]
fn mixed_native_controls_cannot_select_a_losing_legacy_key() {
    for (setting, expected, contents) in [
        (
            ConfigSetting::McpServer,
            Some("docs"),
            r#"{"mcp":{"docs":{"type":"local","command":["docs"],"enabled":true},"servers":{"docs":{"type":"local","command":["native"],"disabled":true}}}}"#,
        ),
        (
            ConfigSetting::Compaction,
            None,
            r#"{"compaction":{"auto":true,"reserved":40000,"buffer":20000}}"#,
        ),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join(".config/opencode/opencode.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        let context = ConfigContext::native(AgentKind::OpenCode, temporary.path(), None);
        assert_eq!(
            AgentConfigEditor::new().effective_for_value(&context, setting, expected),
            Err(ConfigUnavailableReason::InvalidPrecedence)
        );
        assert_eq!(fs::read_to_string(path).unwrap(), contents);
    }
}

#[test]
fn v1_numeric_compaction_keeps_its_existing_selector() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join(".config/opencode/opencode.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, r#"{"compaction":{"auto":true,"reserved":40000}}"#).unwrap();
    let mut context = ConfigContext::native(AgentKind::OpenCode, temporary.path(), None);
    context.source_format = Some(SourceFormat::OpenCodeSqliteV1);
    let effective = AgentConfigEditor::new()
        .effective(&context, ConfigSetting::Compaction)
        .unwrap();
    assert_eq!(effective.value, "40000");
    assert_eq!(effective.selector, "compaction.reserved");
}

#[test]
fn v2_indexed_resources_do_not_claim_v1_inventory_or_schema_overhead() {
    use antiburn_local::analysis::{
        ContextSourceEvidence, EvidenceSource, EvidenceValue, LoadedSource,
        SessionEvidenceAccumulator, SourceCapabilities, SourceKind, SourceOrigin, TurnFacts,
    };
    for source in V2_SOURCES {
        let temporary = tempfile::tempdir().unwrap();
        let mut capabilities = SourceCapabilities::opencode();
        capabilities.source_format = source;
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "opencode".into(),
            session_id: "session".into(),
            kind: SourceKind::File,
            capabilities,
        })
        .evidence(&TurnFacts::default());
        evidence.context_sources = EvidenceValue::Complete(ContextSourceEvidence {
            mcp_servers: std::collections::BTreeMap::from([(
                "docs".into(),
                LoadedSource {
                    description: None,
                    configured: true,
                    available: true,
                    injected: true,
                    invoked: false,
                    token_count: Some(100),
                    origin: EvidenceValue::Complete(SourceOrigin::User),
                },
            )]),
            skills: Default::default(),
            skill_coverage: EvidenceValue::Complete(()),
            mcp_coverage: EvidenceValue::Complete(()),
            tool_definitions: EvidenceValue::Complete(Default::default()),
        });
        let inventory = advisory_resource_inventory(
            &ConfigContext::native(AgentKind::OpenCode, temporary.path(), None),
            [IndexedResourceEvidence {
                evidence: &evidence,
                scope: ResourceScope::Global,
            }],
        );
        if source == SourceFormat::OpenCodeSqliteV2 {
            assert_eq!(inventory, Err(ConfigUnavailableReason::UnsafePath));
            continue;
        }
        let inventory = inventory.unwrap();
        assert!(
            !inventory
                .resources
                .iter()
                .any(|resource| resource.canonical_name == "docs")
        );
        assert!(inventory.issues.contains(&InventoryIssue {
            kind: None,
            scope: ResourceScope::Global,
            reason: InventoryIssueReason::UnsupportedShape,
        }));
    }
}
