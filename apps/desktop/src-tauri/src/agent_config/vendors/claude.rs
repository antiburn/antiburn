use std::path::{Path, PathBuf};

use serde_json::Value;

use super::json::parse_strict;
use super::{OperationSelector, Target, VendorConfig, VendorPolicy};
use crate::agent_config::filesystem::{path_entry_exists, read_checked};
use crate::agent_config::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

pub(super) struct Claude;

pub(super) static CLAUDE: Claude = Claude;

impl VendorConfig for Claude {
    fn policy(&self, setting: ConfigSetting) -> VendorPolicy {
        match setting {
            ConfigSetting::Model
            | ConfigSetting::Reasoning
            | ConfigSetting::Compaction
            | ConfigSetting::FastMode => VendorPolicy::AutomaticEdit,
            ConfigSetting::SubagentModel => VendorPolicy::AutomaticEdit,
            ConfigSetting::McpServer | ConfigSetting::BuiltInTool | ConfigSetting::Skill => {
                VendorPolicy::AutomaticEdit
            }
        }
    }

    fn resolve_target(
        &self,
        setting: ConfigSetting,
        home: &Path,
        _workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        reject_runtime_overrides(setting)?;
        if setting == ConfigSetting::Reasoning {
            return self.resolve_reasoning(home, trusted_workspace_root);
        }
        let mut operation = match setting {
            ConfigSetting::Model => OperationSelector::JsonKey("model"),
            ConfigSetting::Compaction => OperationSelector::JsonKey("autoCompactEnabled"),
            ConfigSetting::FastMode => OperationSelector::JsonKey("fastMode"),
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        };
        if setting == ConfigSetting::Compaction {
            let choose =
                |path: &Path,
                 root: &Path|
                 -> Result<Option<OperationSelector>, ConfigUnavailableReason> {
                    let document = parse_strict(&read_checked(path, root)?.bytes)?;
                    match document.get("autoCompactEnabled") {
                        Some(Value::Bool(false)) => {
                            Ok(Some(OperationSelector::JsonKey("autoCompactEnabled")))
                        }
                        Some(Value::Bool(true)) => match document.get("autoCompactWindow") {
                            Some(Value::Number(_)) => {
                                Ok(Some(OperationSelector::JsonKey("autoCompactWindow")))
                            }
                            Some(_) => Err(ConfigUnavailableReason::MalformedConfig),
                            None => Ok(None),
                        },
                        Some(_) => Err(ConfigUnavailableReason::MalformedConfig),
                        None => Ok(None),
                    }
                };
            if let Some(root) = trusted_workspace_root {
                for path in [
                    root.join(".claude/settings.local.json"),
                    root.join(".claude/settings.json"),
                ] {
                    if path_entry_exists(&path)?
                        && let Some(value) = choose(&path, root)?
                    {
                        operation = value;
                        break;
                    }
                }
            }
            let path = home.join(".claude/settings.json");
            if path_entry_exists(&path)?
                && let Some(value) = choose(&path, home)?
            {
                operation = value;
            }
        }
        if let Some(root) = trusted_workspace_root {
            for path in [
                root.join(".claude/settings.local.json"),
                root.join(".claude/settings.json"),
            ] {
                if path_entry_exists(&path)?
                    && self
                        .read_value(&read_checked(&path, root)?.bytes, &operation)?
                        .is_some()
                {
                    return Ok(Target {
                        path,
                        safety_root: root.to_owned(),
                        scope: ConfigScope::Project,
                        operation,
                    });
                }
            }
        }

        let path = home.join(".claude/settings.json");
        if !path_entry_exists(&path)? {
            return Err(ConfigUnavailableReason::MissingConfig);
        }
        if self
            .read_value(&read_checked(&path, home)?.bytes, &operation)?
            .is_none()
        {
            return Err(ConfigUnavailableReason::MissingTarget);
        }
        Ok(Target {
            path,
            safety_root: home.to_owned(),
            scope: ConfigScope::Global,
            operation,
        })
    }

    fn resolve_target_for_value(
        &self,
        setting: ConfigSetting,
        expected: Option<&str>,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        if setting == ConfigSetting::McpServer {
            return self.mcp_target(
                expected.ok_or(ConfigUnavailableReason::MissingTarget)?,
                home,
                trusted_workspace_root,
            );
        }
        if setting == ConfigSetting::BuiltInTool {
            return self.built_in_tool_target(
                expected.ok_or(ConfigUnavailableReason::MissingTarget)?,
                home,
                trusted_workspace_root,
            );
        }
        if setting == ConfigSetting::Skill {
            return self.skill_target(
                expected.ok_or(ConfigUnavailableReason::MissingTarget)?,
                home,
                trusted_workspace_root,
            );
        }
        if setting != ConfigSetting::SubagentModel {
            return self.resolve_target(setting, home, workspace_cwd, trusted_workspace_root);
        }
        let expected = expected.ok_or(ConfigUnavailableReason::MissingTarget)?;
        named_markdown_target(
            &[
                home.join(".claude/agents"),
                trusted_workspace_root
                    .map(|root| root.join(".claude/agents"))
                    .unwrap_or_default(),
            ],
            &[home, trusted_workspace_root.unwrap_or(home)],
            expected,
        )
    }

    #[cfg(not(windows))]
    fn resolve_targets(
        &self,
        setting: ConfigSetting,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Vec<Target>, ConfigUnavailableReason> {
        let primary = self.resolve_target(setting, home, workspace_cwd, trusted_workspace_root)?;
        if setting == ConfigSetting::FastMode {
            return Ok(vec![primary]);
        }
        let operation = match setting {
            ConfigSetting::Model => OperationSelector::JsonKey("model"),
            ConfigSetting::Compaction => OperationSelector::JsonKey("autoCompactEnabled"),
            ConfigSetting::FastMode => OperationSelector::JsonKey("fastMode"),
            ConfigSetting::Reasoning => primary.operation.clone(),
            _ => return Ok(vec![primary]),
        };
        let mut targets = vec![primary];
        for (path, safety_root, scope) in [
            (
                home.join(".claude/settings.json"),
                home.to_owned(),
                ConfigScope::Global,
            ),
            (
                trusted_workspace_root
                    .map(|root| root.join(".claude/settings.json"))
                    .unwrap_or_default(),
                trusted_workspace_root.unwrap_or(home).to_owned(),
                ConfigScope::Project,
            ),
            (
                trusted_workspace_root
                    .map(|root| root.join(".claude/settings.local.json"))
                    .unwrap_or_default(),
                trusted_workspace_root.unwrap_or(home).to_owned(),
                ConfigScope::Project,
            ),
        ] {
            if path_entry_exists(&path)?
                && !targets.iter().any(|target| target.path == path)
                && self
                    .read_value(&read_checked(&path, &safety_root)?.bytes, &operation)?
                    .is_some()
            {
                targets.push(Target {
                    path,
                    safety_root,
                    scope,
                    operation: operation.clone(),
                });
            }
        }
        Ok(targets)
    }

    #[cfg(not(windows))]
    fn standalone_global(
        &self,
        setting: ConfigSetting,
        home: &Path,
        proposed: &str,
    ) -> Result<(std::path::PathBuf, Vec<u8>), ConfigUnavailableReason> {
        let value = match setting {
            ConfigSetting::Model => serde_json::json!({ "model": proposed }),
            ConfigSetting::Reasoning => serde_json::json!({ "effortLevel": proposed }),
            ConfigSetting::FastMode => serde_json::json!({ "fastMode": proposed == "fast" }),
            ConfigSetting::BuiltInTool => {
                let name = proposed
                    .strip_suffix("=false")
                    .ok_or(ConfigUnavailableReason::InvalidTarget)?;
                serde_json::json!({ "permissions": { "deny": [name] } })
            }
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        };
        Ok((
            home.join(".claude/settings.json"),
            serde_json::to_vec_pretty(&value)
                .map_err(|_| ConfigUnavailableReason::MalformedConfig)?,
        ))
    }

    #[cfg(not(windows))]
    fn standalone_selector(&self, setting: ConfigSetting) -> &'static str {
        match setting {
            ConfigSetting::Model => "model",
            ConfigSetting::Reasoning => "effortLevel",
            ConfigSetting::FastMode => "fastMode",
            ConfigSetting::BuiltInTool => "permissions.deny.<tool>",
            _ => "standalone",
        }
    }

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        if matches!(operation, OperationSelector::NamedMarkdownModel(_)) {
            return markdown_model(bytes);
        }
        let document = parse_strict(bytes)?;
        match operation {
            OperationSelector::JsonKey(key) => {
                reject_settings_overrides(
                    &document,
                    if *key == "model" {
                        ConfigSetting::Model
                    } else if *key == "autoCompactEnabled" || *key == "autoCompactWindow" {
                        ConfigSetting::Compaction
                    } else if *key == "fastMode" {
                        ConfigSetting::FastMode
                    } else {
                        ConfigSetting::Reasoning
                    },
                )?;
                if *key == "autoCompactEnabled" || *key == "fastMode" {
                    document
                        .get(*key)
                        .map(|value| {
                            value
                                .as_bool()
                                .map(|value| value.to_string())
                                .ok_or(ConfigUnavailableReason::MalformedConfig)
                        })
                        .transpose()
                } else if *key == "autoCompactWindow" {
                    document
                        .get(*key)
                        .map(|value| {
                            value
                                .as_u64()
                                .map(|value| value.to_string())
                                .ok_or(ConfigUnavailableReason::MalformedConfig)
                        })
                        .transpose()
                } else {
                    string_property(&document, key)
                }
            }
            OperationSelector::ClaudeModelEffort(model) => {
                reject_settings_overrides(&document, ConfigSetting::Reasoning)?;
                document
                    .get("modelSettings")
                    .and_then(Value::as_object)
                    .and_then(|settings| settings.get(model))
                    .and_then(Value::as_object)
                    .and_then(|settings| settings.get("effortLevel"))
                    .map(|value| {
                        value
                            .as_str()
                            .map(ToOwned::to_owned)
                            .ok_or(ConfigUnavailableReason::MalformedConfig)
                    })
                    .transpose()
            }
            OperationSelector::NamedMarkdownModel(_) => unreachable!(),
            OperationSelector::NamedClaudeMcpServer(name) => claude_mcp_value(&document, name),
            OperationSelector::NamedClaudeBuiltInTool(name) => {
                claude_built_in_tool_value(&document, name)
            }
            OperationSelector::NamedClaudeSkill(name) => claude_skill_value(&document, name),
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
        if matches!(operation, OperationSelector::NamedMarkdownModel(_)) {
            return edit_markdown_model(bytes, proposed);
        }
        let mut root = parse_strict(bytes)?;
        match operation {
            OperationSelector::JsonKey(key) => {
                let root = root
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if *key == "fastMode" && proposed == "remove" {
                    root.remove(*key);
                    return serde_json::to_vec_pretty(&root)
                        .map_err(|_| ConfigUnavailableReason::MalformedConfig)
                        .map(|mut output| {
                            output.push(b'\n');
                            output
                        });
                }
                root.insert(
                    (*key).into(),
                    if *key == "autoCompactEnabled" || *key == "fastMode" {
                        Value::Bool(
                            proposed
                                .parse()
                                .map_err(|_| ConfigUnavailableReason::InvalidTarget)?,
                        )
                    } else if *key == "autoCompactWindow" {
                        Value::Number(
                            proposed
                                .parse::<u64>()
                                .map_err(|_| ConfigUnavailableReason::InvalidTarget)?
                                .into(),
                        )
                    } else {
                        Value::String(proposed.into())
                    },
                );
            }
            OperationSelector::ClaudeModelEffort(model) => {
                let setting = root
                    .get_mut("modelSettings")
                    .and_then(Value::as_object_mut)
                    .and_then(|settings| settings.get_mut(model))
                    .and_then(Value::as_object_mut)
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if !setting.contains_key("effortLevel") {
                    return Err(ConfigUnavailableReason::MissingTarget);
                }
                setting.insert("effortLevel".into(), Value::String(proposed.into()));
            }
            OperationSelector::NamedMarkdownModel(_) => unreachable!(),
            OperationSelector::NamedClaudeMcpServer(name) => {
                let root = root
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                let permissions = root
                    .get_mut("permissions")
                    .and_then(Value::as_object_mut)
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                let deny = permissions
                    .get_mut("deny")
                    .and_then(Value::as_array_mut)
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                let rule = format!("mcp__{name}__*");
                if deny.iter().any(|value| value.as_str() == Some(&rule)) {
                    return Err(ConfigUnavailableReason::CurrentValueMismatch);
                }
                if deny.iter().any(|value| !value.is_string()) {
                    return Err(ConfigUnavailableReason::MalformedConfig);
                }
                deny.push(Value::String(rule));
            }
            OperationSelector::NamedClaudeBuiltInTool(name) => {
                let root = root
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                let permissions = root
                    .entry("permissions")
                    .or_insert_with(|| Value::Object(Default::default()))
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                let deny = permissions
                    .entry("deny")
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                if deny.iter().any(|value| value.as_str() == Some(name)) {
                    return Err(ConfigUnavailableReason::CurrentValueMismatch);
                }
                if deny.iter().any(|value| !value.is_string()) {
                    return Err(ConfigUnavailableReason::MalformedConfig);
                }
                deny.push(Value::String(name.clone()));
            }
            OperationSelector::NamedClaudeSkill(name) => {
                let root = root
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                let overrides = root
                    .entry("skillOverrides")
                    .or_insert_with(|| Value::Object(Default::default()))
                    .as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if overrides.get(name).is_some_and(|value| value != "on") {
                    return Err(ConfigUnavailableReason::CurrentValueMismatch);
                }
                overrides.insert(name.clone(), Value::String("off".into()));
            }
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        }
        let mut output = serde_json::to_vec_pretty(&root)
            .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
        output.push(b'\n');
        Ok(output)
    }
}

impl Claude {
    fn skill_target(
        &self,
        name: &str,
        home: &Path,
        project: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let source = skill_definition(name, home, project)?;
        let (path, root, scope) = match source {
            ConfigScope::Project => {
                let root = project.ok_or(ConfigUnavailableReason::MissingTarget)?;
                let path = [
                    root.join(".claude/settings.local.json"),
                    root.join(".claude/settings.json"),
                ]
                .into_iter()
                .find(|path| path_entry_exists(path).unwrap_or(false))
                .ok_or(ConfigUnavailableReason::MissingConfig)?;
                (path, root, ConfigScope::Project)
            }
            ConfigScope::Global => (
                home.join(".claude/settings.json"),
                home,
                ConfigScope::Global,
            ),
        };
        if !path_entry_exists(&path)? {
            return Err(ConfigUnavailableReason::MissingConfig);
        }
        Ok(Target {
            path,
            safety_root: root.to_owned(),
            scope,
            operation: OperationSelector::NamedClaudeSkill(name.to_owned()),
        })
    }
    fn built_in_tool_target(
        &self,
        name: &str,
        home: &Path,
        project: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let candidates = [
            project.map(|root| (root.join(".claude/settings.local.json"), root)),
            project.map(|root| (root.join(".claude/settings.json"), root)),
            Some((home.join(".claude/settings.json"), home)),
        ];
        for candidate in candidates {
            let Some((path, root)) = candidate else {
                continue;
            };
            if !path_entry_exists(&path)? {
                continue;
            }
            let document = parse_strict(&read_checked(&path, root)?.bytes)?;
            claude_deny_list(&document)?;
            return Ok(Target {
                path,
                safety_root: root.to_owned(),
                scope: if root == home {
                    ConfigScope::Global
                } else {
                    ConfigScope::Project
                },
                operation: OperationSelector::NamedClaudeBuiltInTool(name.to_owned()),
            });
        }
        Err(ConfigUnavailableReason::MissingTarget)
    }

    fn mcp_target(
        &self,
        name: &str,
        home: &Path,
        project: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let mut sources = Vec::new();
        let mut inspect =
            |path: PathBuf, root: &Path, scope| -> Result<(), ConfigUnavailableReason> {
                if path_entry_exists(&path)?
                    && parse_strict(&read_checked(&path, root)?.bytes)?
                        .get("mcpServers")
                        .and_then(Value::as_object)
                        .is_some_and(|servers| servers.contains_key(name))
                {
                    sources.push((root.to_owned(), scope));
                }
                Ok(())
            };
        inspect(home.join(".claude.json"), home, ConfigScope::Global)?;
        if let Some(project) = project {
            inspect(project.join(".mcp.json"), project, ConfigScope::Project)?;
        }
        if sources.len() != 1 {
            return Err(ConfigUnavailableReason::MissingTarget);
        }
        let (root, scope) = sources.pop().expect("one exact MCP source");
        let paths: &[PathBuf] = if scope == ConfigScope::Project {
            &[
                root.join(".claude/settings.local.json"),
                root.join(".claude/settings.json"),
            ]
        } else {
            &[home.join(".claude/settings.json")]
        };
        let mut targets = paths
            .iter()
            .filter(|path| path_entry_exists(path).unwrap_or(false))
            .collect::<Vec<_>>();
        if targets.len() != 1 {
            return Err(ConfigUnavailableReason::MissingTarget);
        }
        Ok(Target {
            path: targets.pop().expect("one settings target").clone(),
            safety_root: root,
            scope,
            operation: OperationSelector::NamedClaudeMcpServer(name.to_owned()),
        })
    }
}

fn claude_mcp_value(
    document: &Value,
    name: &str,
) -> Result<Option<String>, ConfigUnavailableReason> {
    let rule = format!("mcp__{name}__*");
    let Some(deny) = document
        .get("permissions")
        .and_then(Value::as_object)
        .and_then(|permissions| permissions.get("deny"))
    else {
        return Ok(None);
    };
    let deny = deny
        .as_array()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    if deny.iter().any(|value| !value.is_string()) {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    Ok(Some(format!(
        "{name}={}",
        !deny.iter().any(|value| value.as_str() == Some(&rule))
    )))
}

fn claude_built_in_tool_value(
    document: &Value,
    name: &str,
) -> Result<Option<String>, ConfigUnavailableReason> {
    let Some(deny) = claude_deny_list(document)? else {
        return Ok(Some(format!("{name}=true")));
    };
    Ok(Some(format!(
        "{name}={}",
        !deny.iter().any(|value| value.as_str() == Some(name))
    )))
}

fn claude_skill_value(
    document: &Value,
    name: &str,
) -> Result<Option<String>, ConfigUnavailableReason> {
    let Some(overrides) = document.get("skillOverrides") else {
        return Ok(Some(format!("{name}=on")));
    };
    let overrides = overrides
        .as_object()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    match overrides.get(name) {
        None => Ok(Some(format!("{name}=true"))),
        Some(Value::String(value)) if value == "on" => Ok(Some(format!("{name}=true"))),
        Some(Value::String(value)) if value == "off" => Ok(Some(format!("{name}=false"))),
        Some(_) => Err(ConfigUnavailableReason::MalformedConfig),
    }
}

fn skill_definition(
    name: &str,
    home: &Path,
    project: Option<&Path>,
) -> Result<ConfigScope, ConfigUnavailableReason> {
    let project_path = project.map(|root| root.join(".claude/skills").join(name).join("SKILL.md"));
    if let Some(path) = project_path
        && path_entry_exists(&path)?
    {
        return Ok(ConfigScope::Project);
    }
    let global_path = home.join(".claude/skills").join(name).join("SKILL.md");
    if path_entry_exists(&global_path)? {
        Ok(ConfigScope::Global)
    } else {
        Err(ConfigUnavailableReason::MissingTarget)
    }
}

fn claude_deny_list(document: &Value) -> Result<Option<&Vec<Value>>, ConfigUnavailableReason> {
    let Some(deny) = document
        .get("permissions")
        .and_then(Value::as_object)
        .and_then(|permissions| permissions.get("deny"))
    else {
        return Ok(None);
    };
    let deny = deny
        .as_array()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    if deny.iter().any(|value| !value.is_string()) {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    Ok(Some(deny))
}

fn named_markdown_target(
    directories: &[PathBuf],
    roots: &[&Path],
    expected: &str,
) -> Result<Target, ConfigUnavailableReason> {
    let mut matches = Vec::new();
    for (index, (directory, root)) in directories.iter().zip(roots).enumerate() {
        if !path_entry_exists(directory)? {
            continue;
        }
        for entry in
            std::fs::read_dir(directory).map_err(|_| ConfigUnavailableReason::PermissionDenied)?
        {
            let path = entry
                .map_err(|_| ConfigUnavailableReason::UnsafePath)?
                .path();
            if path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            if markdown_model(&read_checked(&path, root)?.bytes)?.as_deref() == Some(expected) {
                matches.push((
                    path,
                    root.to_path_buf(),
                    if index == 0 {
                        ConfigScope::Global
                    } else {
                        ConfigScope::Project
                    },
                ));
            }
        }
    }
    if matches.len() != 1 {
        return Err(ConfigUnavailableReason::MissingTarget);
    }
    let (path, root, scope) = matches.pop().expect("one match");
    Ok(Target {
        operation: OperationSelector::NamedMarkdownModel(
            path.file_stem()
                .and_then(|name| name.to_str())
                .ok_or(ConfigUnavailableReason::UnsafePath)?
                .to_owned(),
        ),
        scope,
        safety_root: root,
        path,
    })
}

fn markdown_model(bytes: &[u8]) -> Result<Option<String>, ConfigUnavailableReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let Some(frontmatter) = text.strip_prefix("---\n").and_then(|text| {
        text.split_once("\n---\n")
            .map(|(frontmatter, _)| frontmatter)
    }) else {
        return Err(ConfigUnavailableReason::MalformedConfig);
    };
    let values = frontmatter
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim() == "model")
        .map(|(_, value)| value.trim().to_owned())
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [value] if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(ConfigUnavailableReason::DuplicateDefinition),
    }
}

#[cfg(not(windows))]
fn edit_markdown_model(bytes: &[u8], proposed: &str) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let Some((frontmatter, body)) = text
        .strip_prefix("---\n")
        .and_then(|text| text.split_once("\n---\n"))
    else {
        return Err(ConfigUnavailableReason::MalformedConfig);
    };
    let mut found = false;
    let frontmatter = frontmatter
        .lines()
        .map(|line| {
            if line
                .split_once(':')
                .is_some_and(|(key, _)| key.trim() == "model")
            {
                found = true;
                format!("model: {proposed}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !found {
        return Err(ConfigUnavailableReason::MissingTarget);
    }
    Ok(format!("---\n{frontmatter}\n---\n{body}").into_bytes())
}

impl Claude {
    fn resolve_reasoning(
        &self,
        home: &Path,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let model_target = self.resolve_target(
            ConfigSetting::Model,
            home,
            trusted_workspace_root,
            trusted_workspace_root,
        )?;
        let model = self
            .read_value(
                &read_checked(&model_target.path, &model_target.safety_root)?.bytes,
                &model_target.operation,
            )?
            .ok_or(ConfigUnavailableReason::MissingTarget)?;

        if let Some(root) = trusted_workspace_root {
            for path in [
                root.join(".claude/settings.local.json"),
                root.join(".claude/settings.json"),
            ] {
                if path_entry_exists(&path)?
                    && let Some(operation) = reasoning_selector(
                        &parse_strict(&read_checked(&path, root)?.bytes)?,
                        &model,
                    )?
                {
                    return Ok(Target {
                        path,
                        safety_root: root.to_owned(),
                        scope: ConfigScope::Project,
                        operation,
                    });
                }
            }
        }

        let path = home.join(".claude/settings.json");
        if !path_entry_exists(&path)? {
            return Err(ConfigUnavailableReason::MissingConfig);
        }
        let operation =
            reasoning_selector(&parse_strict(&read_checked(&path, home)?.bytes)?, &model)?
                .ok_or(ConfigUnavailableReason::MissingTarget)?;
        Ok(Target {
            path,
            safety_root: home.to_owned(),
            scope: ConfigScope::Global,
            operation,
        })
    }
}

fn reasoning_selector(
    document: &Value,
    model: &str,
) -> Result<Option<OperationSelector>, ConfigUnavailableReason> {
    reject_settings_overrides(document, ConfigSetting::Reasoning)?;
    if let Some(settings) = document.get("modelSettings") {
        let settings = settings
            .as_object()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?;
        if let Some(model_setting) = settings.get(model) {
            let model_setting = model_setting
                .as_object()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            if let Some(effort) = model_setting.get("effortLevel") {
                if !effort.is_string() {
                    return Err(ConfigUnavailableReason::MalformedConfig);
                }
                return Ok(Some(OperationSelector::ClaudeModelEffort(model.to_owned())));
            }
        }
    }
    if string_property(document, "effortLevel")?.is_some() {
        Ok(Some(OperationSelector::JsonKey("effortLevel")))
    } else {
        Ok(None)
    }
}

fn reject_settings_overrides(
    document: &Value,
    setting: ConfigSetting,
) -> Result<(), ConfigUnavailableReason> {
    let Some(environment) = document.get("env") else {
        return Ok(());
    };
    let environment = environment
        .as_object()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    let overridden = match setting {
        ConfigSetting::Model => [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
        ]
        .iter()
        .any(|name| environment.contains_key(*name)),
        ConfigSetting::Reasoning => ["CLAUDE_CODE_EFFORT_LEVEL", "CLAUDE_EFFORT"]
            .iter()
            .any(|name| environment.contains_key(*name)),
        ConfigSetting::Compaction => ["CLAUDE_CODE_DISABLE_AUTO_COMPACT"]
            .iter()
            .any(|name| environment.contains_key(*name)),
        ConfigSetting::FastMode => false,
        ConfigSetting::SubagentModel
        | ConfigSetting::McpServer
        | ConfigSetting::BuiltInTool
        | ConfigSetting::Skill => return Err(ConfigUnavailableReason::UnsupportedSetting),
    };
    if overridden {
        Err(ConfigUnavailableReason::RuntimeOverride)
    } else {
        Ok(())
    }
}

fn reject_runtime_overrides(setting: ConfigSetting) -> Result<(), ConfigUnavailableReason> {
    let mut names = vec![
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CONFIG_DIR",
    ];
    if setting == ConfigSetting::Reasoning {
        names.extend(["CLAUDE_CODE_EFFORT_LEVEL", "CLAUDE_EFFORT"]);
    }
    if setting == ConfigSetting::Compaction {
        names.push("CLAUDE_CODE_DISABLE_AUTO_COMPACT");
    }
    if names.iter().any(|name| std::env::var_os(name).is_some()) {
        Err(ConfigUnavailableReason::RuntimeOverride)
    } else {
        Ok(())
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

#[cfg(all(test, not(windows)))]
mod tests {
    use std::fs;
    use std::path::Path;

    use antiburn_local::model::AgentKind;

    use crate::agent_config::{AgentConfigEditor, ConfigChange, ConfigContext, ConfigScope};
    use crate::agent_config::{ApplyConflict, ApplyError};

    #[test]
    fn resolves_project_and_inherited_global_scope() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        write(
            &home.join(".claude/settings.json"),
            r#"{"model":"old","theme":"dark"}"#,
        );
        write(
            &project.join(".claude/settings.json"),
            r#"{"permissions":{}}"#,
        );
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Claude, &home, Some(project.clone()));
        assert_eq!(
            editor.effective_model(&context).unwrap().scope,
            ConfigScope::Global
        );
        write(
            &project.join(".claude/settings.local.json"),
            r#"{"model":"project-old"}"#,
        );
        let effective = editor.effective_model(&context).unwrap();
        assert_eq!(effective.scope, ConfigScope::Project);
        assert_eq!(effective.value, "project-old");
    }

    #[test]
    fn prepared_global_change_detects_new_project_precedence() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        write(&home.join(".claude/settings.json"), r#"{"model":"old"}"#);
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Claude, &home, Some(project.clone()));
        let prepared = editor
            .prepare(
                &context,
                &ConfigChange {
                    expected_value: "old".into(),
                    proposed_value: "new".into(),
                },
            )
            .unwrap();
        assert_eq!(prepared.scope(), ConfigScope::Global);

        write(
            &project.join(".claude/settings.local.json"),
            r#"{"model":"project-old"}"#,
        );
        assert_eq!(
            editor.effective_model(&context).unwrap().scope,
            ConfigScope::Project
        );
        #[cfg(not(windows))]
        assert_eq!(
            editor.apply(&prepared),
            Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity))
        );
    }

    #[test]
    fn prepares_applies_and_reads_back_a_project_model() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        write(&home.join(".claude/settings.json"), r#"{"model":"global"}"#);
        let path = project.join(".claude/settings.local.json");
        write(&path, r#"{"model":"project-old","permissions":{}}"#);
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Claude, &home, Some(project));
        let prepared = editor
            .prepare(
                &context,
                &ConfigChange {
                    expected_value: "project-old".into(),
                    proposed_value: "project-new".into(),
                },
            )
            .unwrap();

        assert_eq!(prepared.scope(), ConfigScope::Project);
        editor.apply(&prepared).unwrap();

        let effective = editor.effective_model(&context).unwrap();
        assert_eq!(effective.scope, ConfigScope::Project);
        assert_eq!(effective.value, "project-new");
        let document: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(document["permissions"], serde_json::json!({}));
    }

    #[test]
    fn removes_only_an_explicit_true_fast_mode_winner() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir(&home).unwrap();
        let path = home.join(".claude/settings.json");
        write(&path, r#"{"fastMode":true,"theme":"dark"}"#);
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Claude, &home, None);
        let prepared = editor
            .prepare_operation(
                &context,
                &crate::agent_config::ConfigOperation {
                    setting: crate::agent_config::ConfigSetting::FastMode,
                    expected_value: crate::agent_config::ConfigOperationValue::Boolean(true),
                    proposed_value: crate::agent_config::ConfigOperationValue::Delete,
                },
            )
            .unwrap();
        editor.apply(&prepared).unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "{\n  \"theme\": \"dark\"\n}\n"
        );
    }

    #[test]
    fn keeps_an_inherited_fast_mode_when_a_project_winner_is_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        write(&home.join(".claude/settings.json"), r#"{"fastMode":true}"#);
        let path = project.join(".claude/settings.local.json");
        write(&path, r#"{"fastMode":true}"#);
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Claude, &home, Some(project));
        let prepared = editor
            .prepare_operation(
                &context,
                &crate::agent_config::ConfigOperation {
                    setting: crate::agent_config::ConfigSetting::FastMode,
                    expected_value: crate::agent_config::ConfigOperationValue::Boolean(true),
                    proposed_value: crate::agent_config::ConfigOperationValue::Delete,
                },
            )
            .unwrap();
        editor.apply(&prepared).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "{}\n");
        assert_eq!(
            fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
            r#"{"fastMode":true}"#
        );
    }

    fn write(path: &Path, value: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
}
