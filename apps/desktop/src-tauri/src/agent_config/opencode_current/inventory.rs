use super::*;
use crate::agent_config::{
    AdvisoryResource, EnabledState, InventoryIssue, InventoryIssueReason, ResourceInventory,
    ResourceKind, ResourceProvenance, ResourceScope,
};
use std::collections::BTreeSet;

pub(crate) fn current_inventory(
    context: &ConfigContext,
    authority: &CurrentConfigAuthority,
) -> Result<ResourceInventory, ConfigUnavailableReason> {
    let entries = validated_entries(context, authority)?;
    let mut snapshot = inputs::dependencies(context, entries)?;
    inventory_with_inputs(context, authority, entries, &mut snapshot)
}

fn inventory_with_inputs(
    context: &ConfigContext,
    authority: &CurrentConfigAuthority,
    entries: &[ConfigEntry],
    snapshot: &mut inputs::Snapshot,
) -> Result<ResourceInventory, ConfigUnavailableReason> {
    let mut servers = BTreeSet::new();
    for entry in entries {
        if let Some(info) = &entry.info {
            normalize::reject_unresolved(info)?;
            if let Some(items) = info.pointer("/mcp/servers").and_then(Value::as_object) {
                servers.extend(items.keys().cloned());
                if servers.len() > 512 {
                    return Err(ConfigUnavailableReason::FileTooLarge);
                }
            }
        }
    }
    let mut resources = Vec::new();
    for tool in ["webfetch", "websearch"] {
        let enabled = normalize::permission_allowed(entries, tool, "*")?;
        let mut search_enabled = true;
        for entry in entries {
            if let Some(info) = &entry.info
                && tool == "websearch"
                && let Some(value) = info.get("websearch")
            {
                search_enabled = value != &Value::Bool(false);
            }
        }
        resources.push(resource(
            tool.into(),
            ResourceKind::BuiltInTool,
            enabled && search_enabled,
            ConfigScope::Global,
            ResourceProvenance::BuiltInCatalog,
        ));
    }
    for id in servers {
        let target = resolve_with_inputs(
            context,
            authority,
            OpenCodeConfigSubject::McpServer(id.clone()),
            entries,
            snapshot,
        )?;
        resources.push(resource(
            id,
            ResourceKind::McpServer,
            target.current_value() == Value::Bool(true),
            target.scope(),
            ResourceProvenance::StandardConfig,
        ));
    }
    for (id, (path, advertised)) in skills::winners(context, entries, snapshot)? {
        if resources.len() >= 512 {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        let target = target(
            context,
            authority,
            OpenCodeConfigSubject::SkillAdvertising(id.clone()),
            path,
            OperationSelector::OpenCodeSkillAdvertising(id.clone()),
            Value::Bool(advertised),
            Vec::new(),
        )?;
        resources.push(resource(
            id,
            ResourceKind::Skill,
            target.current_value() == Value::Bool(true),
            target.scope(),
            ResourceProvenance::StandardDirectory,
        ));
    }
    // Current controls do not prove historical visibility or request-schema cost.
    let issues = [
        ResourceKind::McpServer,
        ResourceKind::BuiltInTool,
        ResourceKind::Skill,
    ]
    .map(|kind| InventoryIssue {
        kind: Some(kind),
        scope: ResourceScope::Unknown,
        reason: InventoryIssueReason::UnsupportedShape,
    })
    .to_vec();
    Ok(ResourceInventory {
        agent: context.agent,
        resources,
        issues,
    })
}

fn resource(
    name: String,
    kind: ResourceKind,
    enabled: bool,
    scope: ConfigScope,
    provenance: ResourceProvenance,
) -> AdvisoryResource {
    AdvisoryResource {
        agent: antiburn_local::model::AgentKind::OpenCode,
        kind,
        canonical_name: name,
        enabled: if enabled {
            EnabledState::Enabled
        } else {
            EnabledState::Disabled
        },
        scope: match scope {
            ConfigScope::Global => ResourceScope::Global,
            ConfigScope::Project => ResourceScope::Project,
        },
        provenance: vec![provenance],
        definition_tokens: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn inventory_reads_each_skill_file_and_directory_once() {
        for count in [16, 96] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let home = root.join("home");
            let cwd = root.join("workspace");
            let global = home.join(".config/opencode");
            fs::create_dir_all(&global).unwrap();
            fs::create_dir(&cwd).unwrap();
            let path = global.join("opencode.json");
            fs::write(&path, "{}").unwrap();
            for index in 0..count {
                let path = global.join(format!("skills/skill-{index}/SKILL.md"));
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "---\ndescription: Review changes\n---\nProcedure.\n").unwrap();
            }
            let authority = CurrentConfigAuthority::fixture(
                &cwd,
                json!([
                    {"type":"directory", "path":global},
                    {"type":"document", "path":path, "info":{}}
                ]),
            );
            let context = ConfigContext::native_workspace(
                antiburn_local::model::AgentKind::OpenCode,
                home,
                &cwd,
                &cwd,
            );
            let entries = validated_entries(&context, &authority).unwrap();
            let mut snapshot = inputs::dependencies(&context, entries).unwrap();
            let directory_reads = snapshot.directory_reads;
            let file_reads = snapshot.file_reads;
            let inventory =
                inventory_with_inputs(&context, &authority, entries, &mut snapshot).unwrap();
            assert_eq!(inventory.resources.len(), count + 2);
            assert_eq!(snapshot.file_reads - file_reads, count);
            assert_eq!(snapshot.directory_reads - directory_reads, count + 1);
            assert_eq!(snapshot.file_reads, count + 1);
            let reads = (snapshot.file_reads, snapshot.directory_reads);
            inputs::document(&path, &mut snapshot).unwrap();
            skills::winners(&context, entries, &mut snapshot).unwrap();
            assert_eq!((snapshot.file_reads, snapshot.directory_reads), reads);
        }
    }

    #[test]
    fn built_in_tools_and_skills_share_permission_validation() {
        let temporary = tempfile::tempdir().unwrap();
        let cwd = temporary.path().canonicalize().unwrap();
        for (rule, expected) in [
            (
                json!({"action":"webfetch","effect":"deny"}),
                Err(ConfigUnavailableReason::MalformedConfig),
            ),
            (
                json!({"action":"webfetch","resource":"https://*","effect":"deny"}),
                Err(ConfigUnavailableReason::InvalidPrecedence),
            ),
            (
                json!({"action":"webfetch","resource":"specific","effect":"deny"}),
                Ok(true),
            ),
            (
                json!({"action":"webfetch","resource":"*","effect":"deny"}),
                Ok(false),
            ),
        ] {
            let authority = CurrentConfigAuthority::fixture(
                &cwd,
                json!([{"type":"document","info":{"permissions":[rule]}}]),
            );
            let entries = authority.entries().unwrap();
            let context = ConfigContext::native_workspace(
                antiburn_local::model::AgentKind::OpenCode,
                &cwd,
                &cwd,
                &cwd,
            );
            let result = current_inventory(&context, &authority).map(|inventory| {
                inventory
                    .resources
                    .iter()
                    .find(|resource| resource.canonical_name == "webfetch")
                    .unwrap()
                    .enabled
                    == EnabledState::Enabled
            });
            assert_eq!(
                normalize::permission_allowed(entries, "webfetch", "*"),
                expected
            );
            assert_eq!(result, expected);
        }
    }
}
