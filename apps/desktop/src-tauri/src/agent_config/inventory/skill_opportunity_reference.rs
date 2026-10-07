use std::path::Path;
use std::time::UNIX_EPOCH;

use antiburn_local::checks::skill_opportunities::{
    MAX_SKILL_CANDIDATES, MAX_SKILL_DESCRIPTION_BYTES, MAX_SKILL_FRONTMATTER_BYTES,
    SkillDefinition, SkillInputError, SkillOpportunitySnapshot, SkillScope,
};
use sha2::{Digest, Sha256};

use super::{
    ConfigContext, ConfigUnavailableReason, EnabledState, InventoryBuilder, InventoryIssueReason,
    ResourceKey, ResourceKind, ResourceScope, discover_inventory,
};

pub(super) struct DiscoveredSkill {
    directory_name: String,
    resource_scope: ResourceScope,
    identity: String,
    revision: String,
    name: String,
    description: String,
    frontmatter: serde_json::Value,
    created_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSnapshotError {
    Config(ConfigUnavailableReason),
    Input(SkillInputError),
}

/// Read current local definitions inside the existing agent discovery boundary.
/// The returned snapshot has no file handles and needs no subsequent file reads.
pub fn skill_opportunity_snapshot(
    context: &ConfigContext,
) -> Result<SkillOpportunitySnapshot, SkillSnapshotError> {
    let builder = discover_inventory(context, [], true).map_err(SkillSnapshotError::Config)?;
    let (cwd, _) = super::canonical_workspace(context).map_err(SkillSnapshotError::Config)?;
    let home = super::canonical_root(&context.home_root).map_err(SkillSnapshotError::Config)?;
    let scope = SkillScope {
        agent: context.agent,
        project_identity: cwd.as_deref().map(path_identity),
        environment_identity: path_identity(&home),
    };
    let mut definitions = Vec::new();
    let mut inventory_complete = !builder
        .issues
        .iter()
        .any(|issue| issue.kind.is_none() || issue.kind == Some(ResourceKind::Skill));
    for discovered in builder.skill_definitions {
        let key = ResourceKey {
            kind: ResourceKind::Skill,
            scope: discovered.resource_scope,
            normalized_name: discovered.directory_name.to_ascii_lowercase(),
        };
        let Some(resource) = builder.resources.get(&key) else {
            inventory_complete = false;
            continue;
        };
        if resource.enabled == EnabledState::Unknown {
            inventory_complete = false;
        }
        let semantic_key = ResourceKey {
            kind: ResourceKind::Skill,
            scope: discovered.resource_scope,
            normalized_name: discovered.name.to_ascii_lowercase(),
        };
        let semantic_disabled = builder
            .resources
            .get(&semantic_key)
            .is_some_and(|resource| resource.enabled != EnabledState::Enabled);
        let uncertain_configuration = context.runtime_override_present
            || context.managed_configuration_present
            || builder.issues.iter().any(|issue| {
                (issue.kind.is_none() || issue.kind == Some(ResourceKind::Skill))
                    && (issue.scope == discovered.resource_scope
                        || issue.scope == ResourceScope::Unknown)
                    && matches!(
                        issue.reason,
                        InventoryIssueReason::Config(_)
                            | InventoryIssueReason::DynamicSource
                            | InventoryIssueReason::ConflictingDefinition
                    )
            });
        if definitions.len() == MAX_SKILL_CANDIDATES {
            return Err(SkillSnapshotError::Input(SkillInputError::LimitExceeded));
        }
        let aliases = if discovered.name == discovered.directory_name {
            Vec::new()
        } else {
            vec![discovered.directory_name]
        };
        definitions.push(SkillDefinition {
            identity: discovered.identity,
            revision: discovered.revision,
            name: discovered.name,
            aliases,
            description: discovered.description,
            frontmatter: discovered.frontmatter,
            scope: scope.clone(),
            enabled: resource.enabled == EnabledState::Enabled
                && !semantic_disabled
                && !uncertain_configuration,
            created_at_ms: discovered.created_at_ms,
        });
    }
    SkillOpportunitySnapshot::new(scope, definitions, inventory_complete)
        .map_err(SkillSnapshotError::Input)
}

pub(super) fn retain_definition(
    builder: &mut InventoryBuilder,
    directory_name: &str,
    path: &Path,
    scope: ResourceScope,
    bytes: &[u8],
    created_at_ms: Option<i64>,
) {
    let Some((name, description, frontmatter)) = parse_frontmatter(bytes, directory_name) else {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::UnsupportedShape,
        );
        return;
    };
    if builder.skill_definitions.len() >= MAX_SKILL_CANDIDATES {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::ResourceCapExceeded,
        );
        return;
    }
    let identity = path_identity(path);
    if builder
        .skill_definitions
        .iter()
        .any(|skill| skill.identity == identity)
    {
        return;
    }
    builder.skill_definitions.push(DiscoveredSkill {
        directory_name: directory_name.to_owned(),
        resource_scope: scope,
        identity,
        revision: digest(frontmatter.to_string().as_bytes()),
        name,
        description,
        frontmatter,
        created_at_ms,
    });
}

pub(super) fn read_definition(
    path: &Path,
    root: &Path,
) -> Result<(crate::agent_config::filesystem::CheckedFile, Option<i64>), ConfigUnavailableReason> {
    let before =
        std::fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    let file = super::read_checked(path, root)?;
    let after =
        std::fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::ChangedIdentity)?;
    if !same_file(&before, &after) || after.file_type().is_symlink() {
        return Err(ConfigUnavailableReason::ChangedIdentity);
    }
    // created() is birth time when the filesystem supports it. Other file times
    // do not establish creation or historical visibility.
    let created_at_ms = before
        .created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    Ok((file, created_at_ms))
}

fn same_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return false;
        }
    }
    before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && before.created().ok() == after.created().ok()
}

fn parse_frontmatter(
    bytes: &[u8],
    directory_name: &str,
) -> Option<(String, String, serde_json::Value)> {
    let text = std::str::from_utf8(bytes).ok()?.replace("\r\n", "\n");
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if line == "---" || line == "..." {
            closed = true;
            break;
        }
        if yaml.len().saturating_add(line.len() + 1) > MAX_SKILL_FRONTMATTER_BYTES {
            return None;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return None;
    }
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&yaml).ok()?;
    let frontmatter = serde_json::to_value(parsed).ok()?;
    let object = frontmatter.as_object()?;
    let name = match object.get("name") {
        Some(value) => value.as_str()?,
        None => directory_name,
    }
    .trim();
    let description = object.get("description")?.as_str()?.trim();
    if name.is_empty()
        || name.len() > 256
        || name.chars().any(char::is_control)
        || description.is_empty()
        || description.len() > MAX_SKILL_DESCRIPTION_BYTES
        || frontmatter.to_string().len() > MAX_SKILL_FRONTMATTER_BYTES
    {
        return None;
    }
    Some((name.to_owned(), description.to_owned(), frontmatter))
}

fn path_identity(path: &Path) -> String {
    digest(path.as_os_str().as_encoded_bytes())
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
