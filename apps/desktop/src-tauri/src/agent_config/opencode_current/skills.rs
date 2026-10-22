use super::{ConfigContext, ConfigEntry, ConfigUnavailableReason, Dependency, inputs};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) fn winner(
    context: &ConfigContext,
    entries: &[ConfigEntry],
    id: &str,
    dependencies: &mut Vec<Dependency>,
) -> Result<(PathBuf, bool), ConfigUnavailableReason> {
    if id.is_empty() || id.len() > 256 || id.contains(['/', '\\']) {
        return Err(ConfigUnavailableReason::InvalidTarget);
    }
    let sources = sources(context, entries, dependencies)?;
    let permitted = super::normalize::permission_allowed(entries, "skill", id)?;
    let mut seen = BTreeSet::new();
    let mut winner = None;
    for directory in sources {
        if !seen.insert(directory.clone()) {
            continue;
        }
        for path in inputs::files(&directory, 0, dependencies)? {
            if skill_id(&directory, &path) != Some(id) {
                continue;
            }
            let bytes = inputs::read_dependency(&path, dependencies)?;
            let (header, _) = inputs::markdown(&bytes)?;
            let advertised = header
                .get("description")
                .and_then(serde_yaml_ng::Value::as_str)
                .is_some()
                && advertising(&header)
                && permitted;
            winner = Some((path, advertised));
        }
    }
    winner.ok_or(ConfigUnavailableReason::MissingTarget)
}

fn skill_id<'a>(directory: &Path, path: &'a Path) -> Option<&'a str> {
    if path.file_name()?.to_str()? == "SKILL.md" {
        path.parent()?.file_name()?.to_str()
    } else if path.parent() == Some(directory) {
        path.file_stem()?.to_str()
    } else {
        None
    }
}

pub(super) fn ids(
    context: &ConfigContext,
    entries: &[ConfigEntry],
) -> Result<BTreeSet<String>, ConfigUnavailableReason> {
    let mut dependencies = Vec::new();
    let mut ids = BTreeSet::new();
    for directory in sources(context, entries, &mut dependencies)? {
        for path in inputs::files(&directory, 0, &mut dependencies)? {
            if let Some(id) = skill_id(&directory, &path) {
                ids.insert(id.to_owned());
            }
        }
    }
    Ok(ids)
}

fn sources(
    context: &ConfigContext,
    entries: &[ConfigEntry],
    dependencies: &mut Vec<Dependency>,
) -> Result<Vec<PathBuf>, ConfigUnavailableReason> {
    let cwd = context
        .workspace_cwd
        .as_deref()
        .ok_or(ConfigUnavailableReason::UnsafePath)?;
    let mut sources = Vec::new();
    // Native sources follow compatibility sources. A native winner does not need
    // the unreported compatibility-discovery options.
    for entry in entries {
        if let Some(info) = &entry.info {
            super::normalize::reject_unresolved(info)?;
            if let Some(path) = &entry.path {
                let raw = super::super::vendors::json::parse(&inputs::read_dependency(
                    path,
                    dependencies,
                )?)?;
                super::normalize::reject_unresolved(&raw)?;
            }
        } else if let Some(directory) = &entry.path {
            sources.extend([directory.join("skill"), directory.join("skills")]);
        }
    }
    for entry in entries {
        if let Some(skills) = entry.info.as_ref().and_then(|info| info.get("skills")) {
            for source in skills
                .as_array()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?
            {
                let source = source
                    .as_str()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if source.contains("://") || source.contains("{env:") || source.contains("{file:") {
                    return Err(ConfigUnavailableReason::DynamicValue);
                }
                sources.push(if let Some(relative) = source.strip_prefix("~/") {
                    context.home_root.join(relative)
                } else if Path::new(source).is_absolute() {
                    source.into()
                } else {
                    cwd.join(source)
                });
            }
        }
    }
    Ok(sources)
}

fn boolean(value: &serde_yaml_ng::Value) -> Option<bool> {
    if let Some(value) = value.as_bool() {
        return Some(value);
    }
    if let Some(value) = value.as_i64() {
        return match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    match value.as_str()?.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn advertising(header: &serde_yaml_ng::Value) -> bool {
    header
        .get("metadata")
        .and_then(|metadata| metadata.get("opencode/autoinvoke"))
        .and_then(boolean)
        .unwrap_or_else(|| {
            !header
                .get("disable-model-invocation")
                .and_then(boolean)
                .unwrap_or(false)
        })
}

#[cfg(not(windows))]
pub(super) fn edit(bytes: &[u8]) -> Result<Vec<u8>, ConfigUnavailableReason> {
    inputs::markdown_edit(bytes, |header| {
        let mapping = header
            .as_mapping_mut()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?;
        let metadata = mapping
            .entry(serde_yaml_ng::Value::String("metadata".into()))
            .or_insert_with(|| serde_yaml_ng::Value::Mapping(Default::default()));
        metadata
            .as_mapping_mut()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?
            .insert(
                serde_yaml_ng::Value::String("opencode/autoinvoke".into()),
                serde_yaml_ng::Value::Bool(false),
            );
        Ok(())
    })
}

pub(super) fn value(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
    Ok(Value::Bool(advertising(&inputs::markdown(bytes)?.0)))
}
