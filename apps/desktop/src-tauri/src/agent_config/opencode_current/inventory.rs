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
    if context.agent != antiburn_local::model::AgentKind::OpenCode || !context.native_environment {
        return Err(ConfigUnavailableReason::UnsupportedEnvironment);
    }
    let cwd = context
        .workspace_cwd
        .as_deref()
        .ok_or(ConfigUnavailableReason::UnsafePath)?
        .canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if authority.location() != cwd {
        return Err(ConfigUnavailableReason::InvalidPrecedence);
    }
    let entries = authority
        .entries()
        .map_err(|_| ConfigUnavailableReason::RuntimeOverride)?;
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
        let mut enabled = true;
        let mut search_enabled = true;
        for entry in entries {
            if let Some(info) = &entry.info {
                if tool == "websearch"
                    && let Some(value) = info.get("websearch")
                {
                    search_enabled = value != &Value::Bool(false);
                }
                if let Some(rules) = info.get("permissions") {
                    for rule in rules
                        .as_array()
                        .ok_or(ConfigUnavailableReason::MalformedConfig)?
                    {
                        let action = rule
                            .get("action")
                            .and_then(Value::as_str)
                            .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                        if action != "*" && action.contains(['*', '?']) {
                            return Err(ConfigUnavailableReason::InvalidPrecedence);
                        }
                        if (action == tool || action == "*")
                            && rule.get("resource").and_then(Value::as_str) == Some("*")
                        {
                            enabled = match rule.get("effect").and_then(Value::as_str) {
                                Some("deny") => false,
                                Some("ask" | "allow") => true,
                                _ => return Err(ConfigUnavailableReason::MalformedConfig),
                            };
                        }
                    }
                }
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
        let target = resolve(
            context,
            authority,
            OpenCodeConfigSubject::McpServer(id.clone()),
        )?;
        resources.push(resource(
            id,
            ResourceKind::McpServer,
            target.current_value() == Value::Bool(true),
            target.scope(),
            ResourceProvenance::StandardConfig,
        ));
    }
    for id in skills::ids(context, entries)? {
        if resources.len() >= 512 {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        let target = resolve(
            context,
            authority,
            OpenCodeConfigSubject::SkillAdvertising(id.clone()),
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
