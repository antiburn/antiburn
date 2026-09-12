use std::path::Path;

use serde_json::Value;

use super::json::parse_strict;
use super::{OperationSelector, Target, VendorConfig, VendorPolicy};
use crate::agent_config::filesystem::{path_entry_exists, read_checked};
use crate::agent_config::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

pub(super) struct Pi;

pub(super) static PI: Pi = Pi;

impl VendorConfig for Pi {
    fn policy(&self, _: ConfigSetting) -> VendorPolicy {
        VendorPolicy::AutomaticEdit
    }

    fn resolve_target(
        &self,
        setting: ConfigSetting,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let global_root = global_root(home)?;
        let global_path = global_root.join("settings.json");
        let global = read_optional(&global_path, &global_root)?;
        let project = workspace_cwd
            .map(|cwd| {
                read_optional(
                    &cwd.join(".pi/settings.json"),
                    trusted_workspace_root.ok_or(ConfigUnavailableReason::UnsafePath)?,
                )
            })
            .transpose()?
            .flatten();

        match setting {
            ConfigSetting::Model => {
                let source = effective_model_source(project.as_ref(), global.as_ref())?
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                match source {
                    ModelSource::Project(path) => Ok(target(
                        path,
                        trusted_workspace_root.ok_or(ConfigUnavailableReason::UnsafePath)?,
                        ConfigScope::Project,
                        OperationSelector::PiModel,
                    )),
                    ModelSource::Global(path) => Ok(target(
                        path,
                        &global_root,
                        ConfigScope::Global,
                        OperationSelector::PiModel,
                    )),
                }
            }
            ConfigSetting::Reasoning => {
                let route = effective_model(project.as_ref(), global.as_ref())?
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                if let Some((path, document)) = project.as_ref()
                    && let Some(operation) = reasoning_selector(document, &route)?
                {
                    return Ok(target(
                        path,
                        trusted_workspace_root.ok_or(ConfigUnavailableReason::UnsafePath)?,
                        ConfigScope::Project,
                        operation,
                    ));
                }
                let (path, document) = global.ok_or(ConfigUnavailableReason::MissingConfig)?;
                let operation = reasoning_selector(&document, &route)?
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                Ok(target(&path, &global_root, ConfigScope::Global, operation))
            }
        }
    }

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        let document = parse_strict(bytes)?;
        match operation {
            OperationSelector::PiModel => model(&document),
            OperationSelector::JsonKey("defaultThinkingLevel") => {
                string_property(&document, "defaultThinkingLevel")
            }
            OperationSelector::PiModelThinkingLevel(route) => document
                .get("modelThinkingLevels")
                .and_then(Value::as_object)
                .and_then(|levels| levels.get(route))
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or(ConfigUnavailableReason::MalformedConfig)
                })
                .transpose(),
            _ => Err(ConfigUnavailableReason::UnsupportedSetting),
        }
    }

    #[cfg(not(windows))]
    fn edit_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
        proposed: &str,
    ) -> Result<Vec<u8>, ConfigUnavailableReason> {
        let mut document = parse_strict(bytes)?;
        match operation {
            OperationSelector::PiModel => {
                let (provider, model) = split_route(proposed)?;
                let object = document
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if !object.contains_key("defaultProvider") || !object.contains_key("defaultModel") {
                    return Err(ConfigUnavailableReason::MissingTarget);
                }
                object.insert("defaultProvider".into(), Value::String(provider.into()));
                object.insert("defaultModel".into(), Value::String(model.into()));
            }
            OperationSelector::JsonKey("defaultThinkingLevel") => {
                let object = document
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if !object.contains_key("defaultThinkingLevel") {
                    return Err(ConfigUnavailableReason::MissingTarget);
                }
                object.insert(
                    "defaultThinkingLevel".into(),
                    Value::String(proposed.into()),
                );
            }
            OperationSelector::PiModelThinkingLevel(route) => {
                let levels = document
                    .get_mut("modelThinkingLevels")
                    .and_then(Value::as_object_mut)
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if !levels.contains_key(route) {
                    return Err(ConfigUnavailableReason::MissingTarget);
                }
                levels.insert(route.clone(), Value::String(proposed.into()));
            }
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        }
        let mut output = serde_json::to_vec_pretty(&document)
            .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
        output.push(b'\n');
        Ok(output)
    }
}

fn read_optional(
    path: &Path,
    root: &Path,
) -> Result<Option<(std::path::PathBuf, Value)>, ConfigUnavailableReason> {
    if !path_entry_exists(path)? {
        return Ok(None);
    }
    Ok(Some((
        path.to_owned(),
        parse_strict(&read_checked(path, root)?.bytes)?,
    )))
}

fn target(path: &Path, root: &Path, scope: ConfigScope, operation: OperationSelector) -> Target {
    Target {
        path: path.to_owned(),
        safety_root: root.to_owned(),
        scope,
        operation,
    }
}

fn effective_model(
    project: Option<&(std::path::PathBuf, Value)>,
    global: Option<&(std::path::PathBuf, Value)>,
) -> Result<Option<String>, ConfigUnavailableReason> {
    match effective_model_source(project, global)? {
        Some(ModelSource::Project(_)) => model(&project.expect("project source exists").1),
        Some(ModelSource::Global(_)) => model(&global.expect("global source exists").1),
        None => Ok(None),
    }
}

enum ModelSource<'a> {
    Project(&'a Path),
    Global(&'a Path),
}

fn effective_model_source<'a>(
    project: Option<&'a (std::path::PathBuf, Value)>,
    global: Option<&'a (std::path::PathBuf, Value)>,
) -> Result<Option<ModelSource<'a>>, ConfigUnavailableReason> {
    let project_provider = project
        .map(|(_, document)| string_property(document, "defaultProvider"))
        .transpose()?
        .flatten();
    let project_model = project
        .map(|(_, document)| string_property(document, "defaultModel"))
        .transpose()?
        .flatten();
    if project_provider.is_some() != project_model.is_some() {
        return Err(ConfigUnavailableReason::SplitModelRoute);
    }
    if project_provider.is_some() {
        return Ok(Some(ModelSource::Project(
            &project.expect("project values exist").0,
        )));
    }

    let Some((path, document)) = global else {
        return Ok(None);
    };
    match (
        string_property(document, "defaultProvider")?,
        string_property(document, "defaultModel")?,
    ) {
        (Some(_), Some(_)) => Ok(Some(ModelSource::Global(path))),
        (None, None) => Ok(None),
        _ => Err(ConfigUnavailableReason::SplitModelRoute),
    }
}

fn model(document: &Value) -> Result<Option<String>, ConfigUnavailableReason> {
    let provider = string_property(document, "defaultProvider")?;
    let model = string_property(document, "defaultModel")?;
    match (provider, model) {
        (Some(provider), Some(model)) => Ok(Some(format!("{provider}/{model}"))),
        (None, None) => Ok(None),
        _ => Err(ConfigUnavailableReason::InvalidPrecedence),
    }
}

fn reasoning_selector(
    document: &Value,
    route: &str,
) -> Result<Option<OperationSelector>, ConfigUnavailableReason> {
    if let Some(levels) = document.get("modelThinkingLevels") {
        let levels = levels
            .as_object()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?;
        if let Some(value) = levels.get(route) {
            if !value.is_string() {
                return Err(ConfigUnavailableReason::MalformedConfig);
            }
            return Ok(Some(OperationSelector::PiModelThinkingLevel(
                route.to_owned(),
            )));
        }
    }
    if string_property(document, "defaultThinkingLevel")?.is_some() {
        Ok(Some(OperationSelector::JsonKey("defaultThinkingLevel")))
    } else {
        Ok(None)
    }
}

fn string_property(document: &Value, key: &str) -> Result<Option<String>, ConfigUnavailableReason> {
    document
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or(ConfigUnavailableReason::MalformedConfig)
        })
        .transpose()
}

#[cfg(not(windows))]
fn split_route(route: &str) -> Result<(&str, &str), ConfigUnavailableReason> {
    let (provider, model) = route
        .split_once('/')
        .ok_or(ConfigUnavailableReason::InvalidTarget)?;
    if provider.is_empty() || model.is_empty() {
        Err(ConfigUnavailableReason::InvalidTarget)
    } else {
        Ok((provider, model))
    }
}

fn global_root(home: &Path) -> Result<std::path::PathBuf, ConfigUnavailableReason> {
    global_root_for(
        home,
        std::env::var_os("PI_AGENT_DIR").as_deref(),
        std::env::var_os("PI_CODING_AGENT_DIR").as_deref(),
    )
}

fn global_root_for(
    home: &Path,
    legacy: Option<&std::ffi::OsStr>,
    current: Option<&std::ffi::OsStr>,
) -> Result<std::path::PathBuf, ConfigUnavailableReason> {
    let legacy = legacy.filter(|value| !value.is_empty());
    let current = current.filter(|value| !value.is_empty());
    if let (Some(legacy), Some(current)) = (legacy, current)
        && legacy != current
    {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    }
    let value = current.or(legacy);
    let Some(value) = value else {
        return Ok(home.join(".pi/agent"));
    };
    let value = Path::new(&value);
    let root = if value.is_absolute() {
        value.to_owned()
    } else if let Ok(relative) = value.strip_prefix("~") {
        home.join(relative)
    } else {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    };
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_agent_dir_cases_are_explicit() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let custom = temporary.path().join("custom");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&custom).unwrap();
        let cases = [
            (None, None, Ok(home.join(".pi/agent"))),
            (Some(""), None, Ok(home.join(".pi/agent"))),
            (custom.to_str(), None, Ok(custom.clone())),
            (None, custom.to_str(), Ok(custom.clone())),
            (custom.to_str(), custom.to_str(), Ok(custom.clone())),
            (
                custom.to_str(),
                Some("/different"),
                Err(ConfigUnavailableReason::RuntimeOverride),
            ),
            (
                Some("relative/path"),
                None,
                Err(ConfigUnavailableReason::RuntimeOverride),
            ),
        ];
        for (legacy, current, expected) in cases {
            assert_eq!(
                global_root_for(
                    &home,
                    legacy.map(std::ffi::OsStr::new),
                    current.map(std::ffi::OsStr::new),
                ),
                expected
            );
        }
    }
}
