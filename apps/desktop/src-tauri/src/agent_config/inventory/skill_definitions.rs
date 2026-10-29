use std::path::Path;

use super::ConfigUnavailableReason;
use crate::agent_config::filesystem::{CheckedFile, read_checked};

const MAX_SKILL_FILE_BYTES: usize = 256 * 1024;
const MAX_SKILL_DESCRIPTION_BYTES: usize = 16 * 1024;
const MAX_SKILL_FRONTMATTER_BYTES: usize = 32 * 1024;

/// Declared file fields do not prove that an agent loaded the skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFrontmatter {
    pub name: String,
    pub declared_description: String,
    pub frontmatter: serde_json::Value,
}

pub(super) fn read_definition(
    path: &Path,
    root: &Path,
) -> Result<CheckedFile, ConfigUnavailableReason> {
    let before =
        std::fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    let file = read_checked(path, root)?;
    let after =
        std::fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::ChangedIdentity)?;
    if !same_file(&before, &after) || after.file_type().is_symlink() {
        return Err(ConfigUnavailableReason::ChangedIdentity);
    }
    Ok(file)
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

/// Parse declared frontmatter. Missing descriptions stay empty; bodies are excluded.
pub fn parse_frontmatter(bytes: &[u8], directory_name: &str) -> Option<SkillFrontmatter> {
    if bytes.len() > MAX_SKILL_FILE_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return Some(SkillFrontmatter {
            name: validated_name(directory_name)?.to_owned(),
            declared_description: String::new(),
            frontmatter: serde_json::json!({}),
        });
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
    let frontmatter = if yaml.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::to_value(parsed).ok()?
    };
    let object = frontmatter.as_object()?;
    let name = validated_name(match object.get("name") {
        Some(value) => value.as_str()?,
        None => directory_name,
    })?;
    let description = match object.get("description") {
        None | Some(serde_json::Value::Null) => "",
        Some(value) => value.as_str()?.trim(),
    };
    if description.len() > MAX_SKILL_DESCRIPTION_BYTES
        || frontmatter.to_string().len() > MAX_SKILL_FRONTMATTER_BYTES
    {
        return None;
    }
    Some(SkillFrontmatter {
        name: name.to_owned(),
        declared_description: description.to_owned(),
        frontmatter,
    })
}

fn validated_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty()
        && name.len() <= super::MAX_RESOURCE_NAME_BYTES
        && !name.chars().any(char::is_control))
    .then_some(name)
}

#[cfg(test)]
mod tests;
