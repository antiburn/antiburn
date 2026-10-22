use super::{
    ConfigContext, ConfigEntry, ConfigUnavailableReason, Dependency, OpenCodeConfigSubject,
    OperationSelector, dependency, hash,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) const MAX_DEPENDENCIES: usize = 4096;
const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn track(
    path: &Path,
    dependencies: &mut Vec<Dependency>,
) -> Result<(), ConfigUnavailableReason> {
    let new = dependency(path)?;
    if let Some(old) = dependencies.iter().find(|old| old.path == path) {
        return if old == &new {
            Ok(())
        } else {
            Err(ConfigUnavailableReason::ChangedIdentity)
        };
    }
    if dependencies.len() >= MAX_DEPENDENCIES
        || dependencies
            .iter()
            .map(|entry| entry.bytes)
            .sum::<usize>()
            .saturating_add(new.bytes)
            > MAX_INPUT_BYTES
    {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    dependencies.push(new);
    Ok(())
}

pub(super) fn read_dependency(
    path: &Path,
    dependencies: &mut Vec<Dependency>,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    track(path, dependencies)?;
    let bytes = super::read_checked(
        path,
        path.parent().ok_or(ConfigUnavailableReason::UnsafePath)?,
    )?
    .bytes;
    if !dependencies
        .iter()
        .any(|entry| entry.path == path && entry.digest == Some(hash(&bytes)))
    {
        return Err(ConfigUnavailableReason::ChangedIdentity);
    }
    Ok(bytes)
}

pub(super) fn dependencies(
    context: &ConfigContext,
    entries: &[ConfigEntry],
) -> Result<Vec<Dependency>, ConfigUnavailableReason> {
    let cwd = context
        .workspace_cwd
        .as_deref()
        .ok_or(ConfigUnavailableReason::UnsafePath)?;
    let mut candidates = BTreeSet::new();
    for directory in cwd.ancestors() {
        if candidates.len() > MAX_DEPENDENCIES {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        for file in ["opencode.json", "opencode.jsonc"] {
            candidates.insert(directory.join(file));
            candidates.insert(directory.join(".opencode").join(file));
        }
        candidates.insert(directory.join(".opencode"));
    }
    for entry in entries {
        if let Some(path) = &entry.path {
            candidates.insert(path.clone());
            if entry.info.is_none() {
                for file in ["opencode.json", "opencode.jsonc"] {
                    candidates.insert(path.join(file));
                }
            }
        }
    }
    let documents: BTreeSet<_> = entries
        .iter()
        .filter(|entry| entry.info.is_some())
        .filter_map(|entry| entry.path.as_ref())
        .collect();
    let mut dependencies = Vec::new();
    for path in candidates {
        track(&path, &mut dependencies)?;
        if path
            .file_name()
            .is_some_and(|name| name == "opencode.json" || name == "opencode.jsonc")
            && super::path_entry_exists(&path)?
            && !documents.contains(&path)
        {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
    }
    Ok(dependencies)
}

pub(super) fn directory_entries(path: &Path) -> Result<Vec<PathBuf>, ConfigUnavailableReason> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ConfigUnavailableReason::SymlinkTarget);
    }
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|_| ConfigUnavailableReason::PermissionDenied)? {
        if paths.len() >= 256 {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        let entry = entry.map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        if entry.path().to_str().is_none() {
            return Err(ConfigUnavailableReason::UnsafePath);
        }
        if entry
            .file_type()
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?
            .is_symlink()
        {
            return Err(ConfigUnavailableReason::SymlinkTarget);
        }
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

pub(super) fn markdown(
    bytes: &[u8],
) -> Result<(serde_yaml_ng::Value, &str), ConfigUnavailableReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let (header, body) = text
        .strip_prefix("---\n")
        .and_then(|text| text.split_once("\n---\n"))
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(header).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    if !value.is_mapping() {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    Ok((value, body))
}

#[cfg(not(windows))]
pub(super) fn markdown_edit(
    bytes: &[u8],
    update: impl FnOnce(&mut serde_yaml_ng::Value) -> Result<(), ConfigUnavailableReason>,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let (mut value, body) = markdown(bytes)?;
    update(&mut value)?;
    let header =
        serde_yaml_ng::to_string(&value).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    Ok(format!("---\n{header}---\n{body}").into_bytes())
}

pub(super) fn files(
    directory: &Path,
    depth: usize,
    dependencies: &mut Vec<Dependency>,
) -> Result<Vec<PathBuf>, ConfigUnavailableReason> {
    track(directory, dependencies)?;
    if !super::path_entry_exists(directory)? {
        return Ok(Vec::new());
    }
    if depth > 16 {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let mut files = Vec::new();
    for path in directory_entries(directory)? {
        if std::fs::symlink_metadata(&path)
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?
            .is_dir()
        {
            files.extend(self::files(&path, depth + 1, dependencies)?);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
        if files.len() > MAX_DEPENDENCIES {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
    }
    files.sort();
    Ok(files)
}

pub(super) fn agent_documents(
    directory: &Path,
    subject: &OpenCodeConfigSubject,
    dependencies: &mut Vec<Dependency>,
    winner: &mut Option<(PathBuf, OperationSelector, Value)>,
) -> Result<(), ConfigUnavailableReason> {
    let OpenCodeConfigSubject::AgentModel(id) = subject else {
        return Ok(());
    };
    for field in ["agent", "agents", "mode", "modes"] {
        for path in files(&directory.join(field), 0, dependencies)? {
            let relative = path
                .strip_prefix(directory.join(field))
                .map_err(|_| ConfigUnavailableReason::UnsafePath)?
                .with_extension("");
            if matches!(field, "mode" | "modes") && relative.components().count() != 1 {
                continue;
            }
            if relative.to_string_lossy().replace('\\', "/") != *id {
                continue;
            }
            let bytes = read_dependency(&path, dependencies)?;
            let (header, _) = markdown(&bytes)?;
            validate_agent_header(&header)?;
            if header
                .get("disabled")
                .and_then(serde_yaml_ng::Value::as_bool)
                == Some(true)
                || header
                    .get("disable")
                    .and_then(serde_yaml_ng::Value::as_bool)
                    == Some(true)
            {
                *winner = None;
                continue;
            }
            if let Some(value) = header.get("model") {
                let value = serde_json::to_value(value)
                    .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
                let mut normalized = super::normalize::model(&value)?;
                if let Some(variant) = header.get("variant") {
                    normalized["variant"] = variant
                        .as_str()
                        .filter(|value| !value.is_empty() && !value.contains('#'))
                        .ok_or(ConfigUnavailableReason::InvalidTarget)?
                        .into();
                }
                *winner = Some((
                    path,
                    OperationSelector::OpenCodeMarkdownAgentModel(id.clone()),
                    normalized,
                ));
            }
        }
    }
    Ok(())
}

fn validate_agent_header(header: &serde_yaml_ng::Value) -> Result<(), ConfigUnavailableReason> {
    for (key, value) in header
        .as_mapping()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?
    {
        let valid = match key.as_str() {
            Some("model") => serde_json::to_value(value)
                .ok()
                .is_some_and(|value| super::normalize::model(&value).is_ok()),
            Some("description" | "variant") => value.is_string(),
            Some("mode") => matches!(value.as_str(), Some("primary" | "subagent" | "all")),
            Some("hidden" | "disabled") => value.is_bool(),
            Some("steps") => value.as_u64().is_some_and(|steps| steps > 0),
            Some("color") => value.as_str().is_some_and(|color| {
                color.len() == 7
                    && color.starts_with('#')
                    && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            }),
            _ => false,
        };
        if !valid {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
    }
    Ok(())
}
