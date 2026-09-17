use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use antiburn_local::analysis::{
    ContextSourceEvidence, CoverageReason, EvidenceSource, EvidenceValue, LoadedSource,
    SessionEvidenceAccumulator, SourceCapabilities, SourceKind, SourceOrigin, ToolDefinition,
    TurnFacts,
};

use super::*;

fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let project = temporary.path().join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    (temporary, home, project)
}

fn write(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn resource<'a>(
    inventory: &'a ResourceInventory,
    kind: ResourceKind,
    name: &str,
    scope: ResourceScope,
) -> &'a AdvisoryResource {
    inventory
        .resources
        .iter()
        .find(|resource| {
            resource.kind == kind && resource.canonical_name == name && resource.scope == scope
        })
        .unwrap_or_else(|| panic!("missing {kind:?} {name} {scope:?}"))
}

#[test]
fn claude_inventory_reads_standard_resources_and_controls() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers":{"global-docs":{"command":"docs"}}}"#,
    );
    write(
        &project.join(".mcp.json"),
        r#"{"mcpServers":{"project-docs":{"command":"docs"}}}"#,
    );
    write(
        &home.join(".claude/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review code.\n---\nPrivate body.\n",
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"skillOverrides":{"review":"off"},"permissions":{"deny":["WebSearch"]}}"#,
    );

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [],
    )
    .unwrap();

    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "global-docs",
            ResourceScope::Global
        )
        .enabled,
        EnabledState::Enabled
    );
    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "project-docs",
            ResourceScope::Project
        )
        .enabled,
        EnabledState::Enabled
    );
    let skill = resource(
        &inventory,
        ResourceKind::Skill,
        "review",
        ResourceScope::Global,
    );
    assert_eq!(skill.enabled, EnabledState::Disabled);
    assert_eq!(skill.definition_tokens, Some(6));
    assert_eq!(
        skill.provenance,
        vec![
            ResourceProvenance::StandardConfig,
            ResourceProvenance::StandardDirectory
        ]
    );
    assert_eq!(
        resource(
            &inventory,
            ResourceKind::BuiltInTool,
            "WebSearch",
            ResourceScope::Global
        )
        .enabled,
        EnabledState::Disabled
    );
    assert!(!home.join(".claude/settings.json.bak").exists());
}

#[test]
fn skill_estimate_uses_only_frontmatter_listing_fields() {
    let first =
        skill_listing_tokens(b"---\ntitle: Review\ndescription: Review code.\n---\nShort body.\n");
    let second = skill_listing_tokens(
        b"---\ntitle: Review\ndescription: Review code.\n---\nA much longer private body that must not affect the listing estimate.\n",
    );

    assert_eq!(first, Some(6));
    assert_eq!(second, first);
    assert_eq!(
        skill_listing_tokens(b"---\nname: Review\n---\nPrivate body.\n"),
        None
    );
}

#[test]
fn claude_exact_mcp_tool_permission_is_not_a_built_in_candidate() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".claude/settings.json"),
        r#"{"permissions":{"allow":["mcp__docs__search"]}}"#,
    );

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [],
    )
    .unwrap();

    assert!(!inventory.resources.iter().any(|candidate| {
        candidate.kind == ResourceKind::BuiltInTool
            && candidate.canonical_name == "mcp__docs__search"
    }));
}

#[test]
fn opencode_direct_server_named_servers_is_not_treated_as_a_nested_map() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".config/opencode/opencode.json"),
        r#"{"mcp":{"servers":{"command":"serve","enabled":true}}}"#,
    );

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::OpenCode, &home, Some(project)),
        [],
    )
    .unwrap();

    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "servers",
            ResourceScope::Global,
        )
        .enabled,
        EnabledState::Enabled
    );
}

#[test]
fn codex_inventory_uses_trusted_layers_and_default_enabled_mcp() {
    let (_temporary, home, project) = roots();
    let canonical_project = project.canonicalize().unwrap();
    write(
        &home.join(".codex/config.toml"),
        &format!(
            "[mcp_servers.global]\ncommand = \"global\"\n\n[projects.{}]\ntrust_level = \"trusted\"\n",
            toml_edit::Value::from(canonical_project.to_string_lossy().as_ref())
        ),
    );
    write(
        &project.join(".codex/config.toml"),
        "[mcp_servers.project]\ncommand = \"project\"\n",
    );
    write(
        &project.join(".agents/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review code.\n---\n",
    );

    let inventory = advisory_resource_inventory(
        &ConfigContext::native_workspace(AgentKind::Codex, &home, &project, &project),
        [],
    )
    .unwrap();

    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "global",
            ResourceScope::Global
        )
        .enabled,
        EnabledState::Enabled
    );
    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "project",
            ResourceScope::Project
        )
        .enabled,
        EnabledState::Enabled
    );
    assert_eq!(
        resource(
            &inventory,
            ResourceKind::Skill,
            "review",
            ResourceScope::Project
        )
        .enabled,
        EnabledState::Enabled
    );
}

#[test]
fn opencode_inventory_merges_jsonc_mcp_tools_and_skill_roots() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".config/opencode/opencode.jsonc"),
        r#"{
          // Current resource controls.
          "mcp": {"docs": {"command": "docs"}},
          "tools": {"websearch": false, "read": true},
        }"#,
    );
    write(
        &project.join(".opencode/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review code.\n---\n",
    );

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::OpenCode, &home, Some(project)),
        [],
    )
    .unwrap();

    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "docs",
            ResourceScope::Global
        )
        .enabled,
        EnabledState::Enabled
    );
    assert_eq!(
        resource(
            &inventory,
            ResourceKind::BuiltInTool,
            "websearch",
            ResourceScope::Global
        )
        .enabled,
        EnabledState::Disabled
    );
    resource(
        &inventory,
        ResourceKind::Skill,
        "review",
        ResourceScope::Project,
    );
}

#[test]
fn pi_inventory_replaces_global_tools_and_requires_the_pinned_mcp_package() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".pi/agent/settings.json"),
        r#"{"defaultTools":["read","bash"]}"#,
    );
    write(
        &project.join(".pi/settings.json"),
        r#"{"defaultTools":["edit"],"packages":["npm:pi-mcp-extension@1.5.0"]}"#,
    );
    write(
        &project.join(".pi/npm/node_modules/pi-mcp-extension/package.json"),
        r#"{"name":"pi-mcp-extension","version":"1.5.0","pi":{"extensions":["./src/index.ts"]}}"#,
    );
    write(
        &project.join(".pi/mcp.json"),
        r#"{"mcpServers":{"docs":{"command":"docs","lifecycle":"eager"},"lazy":{"command":"lazy"}}}"#,
    );
    write(
        &project.join(".pi/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review code.\n---\n",
    );

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Pi, &home, Some(project)),
        [],
    )
    .unwrap();

    assert!(inventory.resources.iter().all(|resource| {
        resource.kind != ResourceKind::BuiltInTool || resource.canonical_name == "edit"
    }));
    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "docs",
            ResourceScope::Project
        )
        .enabled,
        EnabledState::Enabled
    );
    assert_eq!(
        resource(
            &inventory,
            ResourceKind::McpServer,
            "lazy",
            ResourceScope::Project
        )
        .enabled,
        EnabledState::Unknown
    );
    assert!(inventory.issues.contains(&InventoryIssue {
        kind: Some(ResourceKind::McpServer),
        scope: ResourceScope::Project,
        reason: InventoryIssueReason::DynamicSource,
    }));
    resource(
        &inventory,
        ResourceKind::Skill,
        "review",
        ResourceScope::Project,
    );
}

#[test]
fn indexed_resources_merge_with_current_candidates_and_keep_partial_limits() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".claude/skills/review/SKILL.md"),
        "---\nname: review\n---\n",
    );
    let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".into(),
        session_id: "session".into(),
        kind: SourceKind::File,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    let loaded = LoadedSource {
        description: None,
        configured: false,
        available: true,
        injected: true,
        invoked: false,
        token_count: Some(10),
        origin: EvidenceValue::Complete(SourceOrigin::User),
    };
    let mut skills = BTreeMap::new();
    skills.insert("review".into(), loaded.clone());
    let mut mcp_servers = BTreeMap::new();
    mcp_servers.insert("docs".into(), loaded);
    let mut tools = BTreeMap::new();
    tools.insert(
        "Read".into(),
        ToolDefinition {
            tokens: 50,
            invoked: false,
            deferred: false,
        },
    );
    evidence.context_sources = EvidenceValue::Partial {
        observed: ContextSourceEvidence {
            skills,
            mcp_servers,
            skill_coverage: EvidenceValue::Complete(()),
            mcp_coverage: EvidenceValue::Complete(()),
            tool_definitions: EvidenceValue::Complete(tools),
        },
        reason: CoverageReason::CapExceeded,
    };

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [IndexedResourceEvidence {
            evidence: &evidence,
            scope: ResourceScope::Project,
        }],
    )
    .unwrap();

    let skill = resource(
        &inventory,
        ResourceKind::Skill,
        "review",
        ResourceScope::Global,
    );
    assert_eq!(skill.enabled, EnabledState::Enabled);
    assert_eq!(
        skill.provenance,
        vec![
            ResourceProvenance::StandardDirectory,
            ResourceProvenance::IndexedSession
        ]
    );
    resource(
        &inventory,
        ResourceKind::McpServer,
        "docs",
        ResourceScope::Global,
    );
    resource(
        &inventory,
        ResourceKind::BuiltInTool,
        "Read",
        ResourceScope::Project,
    );
    for kind in [
        ResourceKind::McpServer,
        ResourceKind::BuiltInTool,
        ResourceKind::Skill,
    ] {
        assert!(inventory.issues.contains(&InventoryIssue {
            kind: Some(kind),
            scope: ResourceScope::Project,
            reason: InventoryIssueReason::PartialIndexedEvidence(CoverageReason::CapExceeded),
        }));
    }
}

#[test]
fn indexed_evidence_from_another_agent_is_ignored() {
    let (_temporary, home, project) = roots();
    let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "codex".into(),
        session_id: "session".into(),
        kind: SourceKind::File,
        capabilities: SourceCapabilities::codex(),
    })
    .evidence(&TurnFacts::default());
    let mut mcp_servers = BTreeMap::new();
    mcp_servers.insert(
        "wrong-agent".into(),
        LoadedSource {
            description: None,
            configured: false,
            available: true,
            injected: true,
            invoked: false,
            token_count: None,
            origin: EvidenceValue::Unsupported,
        },
    );
    evidence.context_sources = EvidenceValue::Complete(ContextSourceEvidence {
        skills: BTreeMap::new(),
        mcp_servers,
        skill_coverage: EvidenceValue::Complete(()),
        mcp_coverage: EvidenceValue::Complete(()),
        tool_definitions: EvidenceValue::Unsupported,
    });

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [IndexedResourceEvidence {
            evidence: &evidence,
            scope: ResourceScope::Project,
        }],
    )
    .unwrap();

    assert!(
        !inventory
            .resources
            .iter()
            .any(|resource| resource.canonical_name == "wrong-agent")
    );
}

#[test]
fn malformed_sources_keep_valid_candidates_and_add_a_limit() {
    let (_temporary, home, project) = roots();
    write(&home.join(".claude.json"), r#"{"mcpServers":{"docs":{}}}"#);
    write(&project.join(".mcp.json"), "not json");

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [],
    )
    .unwrap();

    resource(
        &inventory,
        ResourceKind::McpServer,
        "docs",
        ResourceScope::Global,
    );
    assert!(inventory.issues.iter().any(|issue| {
        issue.scope == ResourceScope::Project
            && issue.reason
                == InventoryIssueReason::Config(ConfigUnavailableReason::MalformedConfig)
    }));
}

#[cfg(unix)]
#[test]
fn symlinked_skill_roots_are_limits_and_are_not_followed() {
    use std::os::unix::fs::symlink;

    let (_temporary, home, project) = roots();
    let outside = home.join("outside");
    write(&outside.join("secret/SKILL.md"), "---\nname: secret\n---\n");
    fs::create_dir_all(home.join(".claude")).unwrap();
    symlink(&outside, home.join(".claude/skills")).unwrap();

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [],
    )
    .unwrap();

    assert!(
        !inventory
            .resources
            .iter()
            .any(|resource| resource.canonical_name == "secret")
    );
    assert!(inventory.issues.contains(&InventoryIssue {
        kind: Some(ResourceKind::Skill),
        scope: ResourceScope::Global,
        reason: InventoryIssueReason::Config(ConfigUnavailableReason::SymlinkTarget),
    }));
}

#[test]
fn resource_caps_are_independent_by_kind_and_scope() {
    let mut builder = InventoryBuilder::new(AgentKind::Claude);
    for index in 0..MAX_RESOURCES {
        builder.add(
            ResourceKind::Skill,
            &format!("skill-{index:04}"),
            EnabledState::Enabled,
            ResourceScope::Global,
            ResourceProvenance::StandardDirectory,
        );
        builder.add(
            ResourceKind::McpServer,
            &format!("server-{index:04}"),
            EnabledState::Enabled,
            ResourceScope::Global,
            ResourceProvenance::StandardConfig,
        );
    }

    let inventory = builder.finish();
    assert_eq!(inventory.resources.len(), MAX_RESOURCES * 2);
    assert!(inventory.issues.is_empty());
}

#[test]
fn unsupported_indexed_sources_add_kind_specific_limits() {
    let (_temporary, home, project) = roots();
    let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".into(),
        session_id: "session".into(),
        kind: SourceKind::File,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    evidence.context_sources = EvidenceValue::Unsupported;

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [IndexedResourceEvidence {
            evidence: &evidence,
            scope: ResourceScope::Project,
        }],
    )
    .unwrap();

    for kind in [
        ResourceKind::McpServer,
        ResourceKind::BuiltInTool,
        ResourceKind::Skill,
    ] {
        assert!(inventory.issues.contains(&InventoryIssue {
            kind: Some(kind),
            scope: ResourceScope::Project,
            reason: InventoryIssueReason::UnsupportedShape,
        }));
    }
}

#[test]
fn unknown_indexed_origins_remain_unknown_scope() {
    let (_temporary, home, project) = roots();
    let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".into(),
        session_id: "session".into(),
        kind: SourceKind::File,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    evidence.context_sources = EvidenceValue::Complete(ContextSourceEvidence {
        skills: BTreeMap::from([(
            "plugin:review".into(),
            LoadedSource {
                description: None,
                configured: false,
                available: true,
                injected: true,
                invoked: false,
                token_count: None,
                origin: EvidenceValue::Complete(SourceOrigin::Unknown),
            },
        )]),
        mcp_servers: BTreeMap::new(),
        skill_coverage: EvidenceValue::Complete(()),
        mcp_coverage: EvidenceValue::Complete(()),
        tool_definitions: EvidenceValue::Complete(BTreeMap::new()),
    });

    let inventory = advisory_resource_inventory(
        &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
        [IndexedResourceEvidence {
            evidence: &evidence,
            scope: ResourceScope::Project,
        }],
    )
    .unwrap();

    resource(
        &inventory,
        ResourceKind::Skill,
        "plugin:review",
        ResourceScope::Unknown,
    );
}
