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
                let (source, operation) =
                    effective_reasoning_source(project.as_ref(), global.as_ref(), &route)?
                        .ok_or(ConfigUnavailableReason::MissingTarget)?;
                match source {
                    ModelSource::Project(path) => Ok(target(
                        path,
                        trusted_workspace_root.ok_or(ConfigUnavailableReason::UnsafePath)?,
                        ConfigScope::Project,
                        operation,
                    )),
                    ModelSource::Global(path) => {
                        Ok(target(path, &global_root, ConfigScope::Global, operation))
                    }
                }
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
    let provider = model_leaf_source(project, global, "defaultProvider")?;
    let model = model_leaf_source(project, global, "defaultModel")?;
    match (provider, model) {
        (Some(ModelSource::Project(provider)), Some(ModelSource::Project(model)))
            if provider == model =>
        {
            Ok(Some(ModelSource::Project(provider)))
        }
        (Some(ModelSource::Global(provider)), Some(ModelSource::Global(model)))
            if provider == model =>
        {
            Ok(Some(ModelSource::Global(provider)))
        }
        (None, None) => Ok(None),
        _ => Err(ConfigUnavailableReason::SplitModelRoute),
    }
}

fn model_leaf_source<'a>(
    project: Option<&'a (std::path::PathBuf, Value)>,
    global: Option<&'a (std::path::PathBuf, Value)>,
    key: &str,
) -> Result<Option<ModelSource<'a>>, ConfigUnavailableReason> {
    if let Some((path, document)) = project
        && string_property(document, key)?.is_some()
    {
        return Ok(Some(ModelSource::Project(path)));
    }
    if let Some((path, document)) = global
        && string_property(document, key)?.is_some()
    {
        return Ok(Some(ModelSource::Global(path)));
    }
    Ok(None)
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

fn effective_reasoning_source<'a>(
    project: Option<&'a (std::path::PathBuf, Value)>,
    global: Option<&'a (std::path::PathBuf, Value)>,
    route: &str,
) -> Result<Option<(ModelSource<'a>, OperationSelector)>, ConfigUnavailableReason> {
    let effective = merged_settings(
        global.map(|(_, document)| document),
        project.map(|(_, document)| document),
    );
    let Some(operation) = reasoning_selector(&effective, route)? else {
        return Ok(None);
    };

    let source = match &operation {
        OperationSelector::PiModelThinkingLevel(route) => {
            reasoning_leaf_source(project, global, "modelThinkingLevels", Some(route))?
        }
        OperationSelector::JsonKey("defaultThinkingLevel") => {
            reasoning_leaf_source(project, global, "defaultThinkingLevel", None)?
        }
        _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
    }
    .ok_or(ConfigUnavailableReason::MissingTarget)?;
    Ok(Some((source, operation)))
}

fn reasoning_leaf_source<'a>(
    project: Option<&'a (std::path::PathBuf, Value)>,
    global: Option<&'a (std::path::PathBuf, Value)>,
    key: &str,
    route: Option<&str>,
) -> Result<Option<ModelSource<'a>>, ConfigUnavailableReason> {
    for (source, document) in [
        project.map(|(path, document)| (ModelSource::Project(path.as_path()), document)),
        global.map(|(path, document)| (ModelSource::Global(path.as_path()), document)),
    ]
    .into_iter()
    .flatten()
    {
        let value = match route {
            Some(route) => document
                .get(key)
                .and_then(Value::as_object)
                .and_then(|levels| levels.get(route)),
            None => document.get(key),
        };
        if value.is_some() {
            return Ok(Some(source));
        }
    }
    Ok(None)
}

fn merged_settings(global: Option<&Value>, project: Option<&Value>) -> Value {
    let mut effective = global.cloned().unwrap_or(Value::Null);
    if let Some(project) = project {
        merge_settings(&mut effective, project);
    }
    effective
}

fn merge_settings(base: &mut Value, override_value: &Value) {
    if let (Some(base), Some(override_value)) = (base.as_object_mut(), override_value.as_object()) {
        for (key, value) in override_value {
            match base.get_mut(key) {
                Some(base_value) => merge_settings(base_value, value),
                None => {
                    base.insert(key.clone(), value.clone());
                }
            }
        }
    } else {
        *base = override_value.clone();
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

    fn settings(path: &str, document: Value) -> (std::path::PathBuf, Value) {
        (path.into(), document)
    }

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

    #[test]
    fn global_route_thinking_level_beats_project_default() {
        let global = settings(
            "global.json",
            serde_json::json!({"modelThinkingLevels": {"a/m": "high"}}),
        );
        let project = settings(
            "project.json",
            serde_json::json!({"defaultThinkingLevel": "low"}),
        );

        let (source, operation) = effective_reasoning_source(Some(&project), Some(&global), "a/m")
            .unwrap()
            .unwrap();

        assert!(matches!(source, ModelSource::Global(path) if path == global.0));
        assert_eq!(
            operation,
            OperationSelector::PiModelThinkingLevel("a/m".into())
        );
    }

    #[test]
    fn project_route_thinking_level_overrides_global_route() {
        let global = settings(
            "global.json",
            serde_json::json!({"modelThinkingLevels": {"a/m": "high"}}),
        );
        let project = settings(
            "project.json",
            serde_json::json!({"modelThinkingLevels": {"a/m": "low"}}),
        );

        let (source, operation) = effective_reasoning_source(Some(&project), Some(&global), "a/m")
            .unwrap()
            .unwrap();

        assert!(matches!(source, ModelSource::Project(path) if path == project.0));
        assert_eq!(
            operation,
            OperationSelector::PiModelThinkingLevel("a/m".into())
        );
    }

    #[test]
    fn project_arrays_replace_global_objects() {
        let merged = merged_settings(
            Some(&serde_json::json!({"modelThinkingLevels": {"a/m": "high"}})),
            Some(&serde_json::json!({"modelThinkingLevels": []})),
        );

        assert_eq!(merged, serde_json::json!({"modelThinkingLevels": []}));
    }
}
