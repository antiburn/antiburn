use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

#[cfg(not(windows))]
use super::json::edit_top_level_string;
use super::json::parse;
use super::{OperationSelector, Target, VendorConfig, VendorPolicy};
use crate::agent_config::filesystem::{path_entry_exists, read_checked};
use crate::agent_config::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

pub(super) struct OpenCode;

pub(super) static OPENCODE: OpenCode = OpenCode;

const RUNTIME_OVERRIDE_NAMES: [&str; 6] = [
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_AUTH_CONTENT",
    "OPENCODE_DB",
    "OPENCODE_DATA_DIR",
];

impl VendorConfig for OpenCode {
    fn policy(&self, setting: ConfigSetting) -> VendorPolicy {
        match setting {
            ConfigSetting::Model | ConfigSetting::Compaction => VendorPolicy::AutomaticEdit,
            ConfigSetting::Reasoning => {
                VendorPolicy::Unsupported(ConfigUnavailableReason::UnsupportedSetting)
            }
            ConfigSetting::SubagentModel => VendorPolicy::AutomaticEdit,
            ConfigSetting::McpServer | ConfigSetting::BuiltInTool | ConfigSetting::Skill => {
                VendorPolicy::AutomaticEdit
            }
            ConfigSetting::FastMode => {
                VendorPolicy::Unsupported(ConfigUnavailableReason::UnsupportedSetting)
            }
        }
    }

    fn resolve_target(
        &self,
        setting: ConfigSetting,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        reject_runtime_overrides()?;
        reject_remote_inputs(home)?;
        reject_managed_config()?;

        let global_root = global_config_root(home)?;
        reject_directory_overrides(&global_root)?;
        let mut winner = None;
        merge_directory_config(
            &global_root,
            &global_root,
            ConfigScope::Global,
            setting,
            &mut winner,
            true,
        )?;

        if !project_config_disabled()
            && let Some(cwd) = workspace_cwd
        {
            let safety_root = trusted_workspace_root.ok_or(ConfigUnavailableReason::UnsafePath)?;
            let roots = project_hierarchy(cwd, safety_root)?;
            for root in &roots {
                merge_directory_config(
                    root,
                    safety_root,
                    ConfigScope::Project,
                    setting,
                    &mut winner,
                    false,
                )?;
            }
            for root in roots.iter().rev() {
                let directory = root.join(".opencode");
                if path_entry_exists(&directory)? {
                    reject_directory_overrides(&directory)?;
                    merge_directory_config(
                        &directory,
                        safety_root,
                        ConfigScope::Project,
                        setting,
                        &mut winner,
                        false,
                    )?;
                }
            }
        }

        let home_directory = home.join(".opencode");
        if path_entry_exists(&home_directory)? {
            reject_directory_overrides(&home_directory)?;
            merge_directory_config(
                &home_directory,
                home,
                ConfigScope::Global,
                setting,
                &mut winner,
                false,
            )?;
        }

        winner.ok_or(ConfigUnavailableReason::MissingTarget)
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
            let name = expected.ok_or(ConfigUnavailableReason::MissingTarget)?;
            return self.mcp_target(name, home, workspace_cwd, trusted_workspace_root);
        }
        if setting == ConfigSetting::BuiltInTool {
            return self.built_in_tool_target(
                expected.ok_or(ConfigUnavailableReason::MissingTarget)?,
                home,
                workspace_cwd,
                trusted_workspace_root,
            );
        }
        if setting == ConfigSetting::Skill {
            return self.skill_target(
                expected.ok_or(ConfigUnavailableReason::MissingTarget)?,
                home,
                workspace_cwd,
                trusted_workspace_root,
            );
        }
        if setting != ConfigSetting::SubagentModel {
            return self.resolve_target(setting, home, workspace_cwd, trusted_workspace_root);
        }
        reject_runtime_overrides()?;
        reject_remote_inputs(home)?;
        reject_managed_config()?;
        let expected = expected.ok_or(ConfigUnavailableReason::MissingTarget)?;
        let directories = [
            home.join(".opencode/agents"),
            trusted_workspace_root
                .map(|root| root.join(".opencode/agents"))
                .unwrap_or_default(),
        ];
        let roots = [home, trusted_workspace_root.unwrap_or(home)];
        let mut matches = Vec::new();
        for (index, (directory, root)) in directories.iter().zip(roots).enumerate() {
            if !path_entry_exists(directory)? {
                continue;
            }
            for entry in std::fs::read_dir(directory)
                .map_err(|_| ConfigUnavailableReason::PermissionDenied)?
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
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or(ConfigUnavailableReason::UnsafePath)?
            .to_owned();
        Ok(Target {
            path,
            safety_root: root,
            scope,
            operation: OperationSelector::NamedMarkdownModel(name),
        })
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
        let mut targets = vec![primary];
        let global_root = global_config_root(home)?;
        collect_directory_targets(
            &global_root,
            &global_root,
            ConfigScope::Global,
            true,
            &mut targets,
        )?;
        if !project_config_disabled()
            && let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root)
        {
            for directory in project_hierarchy(cwd, root)? {
                collect_directory_targets(
                    &directory,
                    root,
                    ConfigScope::Project,
                    false,
                    &mut targets,
                )?;
            }
            for directory in project_hierarchy(cwd, root)?.into_iter().rev() {
                let directory = directory.join(".opencode");
                if path_entry_exists(&directory)? {
                    collect_directory_targets(
                        &directory,
                        root,
                        ConfigScope::Project,
                        false,
                        &mut targets,
                    )?;
                }
            }
        }
        let home_directory = home.join(".opencode");
        if path_entry_exists(&home_directory)? {
            collect_directory_targets(
                &home_directory,
                home,
                ConfigScope::Global,
                false,
                &mut targets,
            )?;
        }
        Ok(targets)
    }

    #[cfg(not(windows))]
    fn standalone_global(
        &self,
        setting: ConfigSetting,
        home: &Path,
        proposed: &str,
    ) -> Result<(PathBuf, Vec<u8>), ConfigUnavailableReason> {
        if setting != ConfigSetting::Model {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        Ok((
            global_config_root(home)?.join("opencode.json"),
            serde_json::to_vec_pretty(&match setting { ConfigSetting::Model => serde_json::json!({ "model": proposed }), ConfigSetting::Compaction => serde_json::json!({ "compaction": { "auto": proposed.parse::<bool>().map_err(|_| ConfigUnavailableReason::InvalidTarget)? } }), _ => return Err(ConfigUnavailableReason::UnsupportedSetting) })
                .map_err(|_| ConfigUnavailableReason::MalformedConfig)?,
        ))
    }

    #[cfg(not(windows))]
    fn standalone_selector(&self, setting: ConfigSetting) -> &'static str {
        match setting {
            ConfigSetting::Model => "model",
            ConfigSetting::Compaction => "compaction.auto",
            _ => "standalone",
        }
    }

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        match operation {
            OperationSelector::JsonKey("model") => model(bytes),
            OperationSelector::JsonPath(path)
                if path.as_slice() == ["compaction", "auto"]
                    || path.as_slice() == ["compaction", "reserved"] =>
            {
                let document = parse(bytes)?;
                document
                    .get("compaction")
                    .and_then(serde_json::Value::as_object)
                    .and_then(|value| value.get(path[1]))
                    .map(|value| match value {
                        serde_json::Value::Bool(value) => Ok(value.to_string()),
                        serde_json::Value::Number(value) => Ok(value.to_string()),
                        _ => Err(ConfigUnavailableReason::MalformedConfig),
                    })
                    .transpose()
            }
            OperationSelector::NamedMarkdownModel(_) => markdown_model(bytes),
            OperationSelector::NamedJsonMcpServer(name) => mcp_value(bytes, name),
            OperationSelector::NamedOpenCodeBuiltInTool(name) => {
                opencode_built_in_tool_value(bytes, name)
            }
            OperationSelector::NamedOpenCodeSkill(name) => opencode_skill_value(bytes, name),
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
        match operation {
            OperationSelector::JsonKey("model") => edit_top_level_string(bytes, "model", proposed),
            OperationSelector::JsonPath(path)
                if path.as_slice() == ["compaction", "auto"]
                    || path.as_slice() == ["compaction", "reserved"] =>
            {
                let mut document = parse(bytes)?;
                let compaction = document
                    .get_mut("compaction")
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                let value = if path[1] == "auto" {
                    serde_json::Value::Bool(
                        proposed
                            .parse()
                            .map_err(|_| ConfigUnavailableReason::InvalidTarget)?,
                    )
                } else {
                    serde_json::Value::Number(
                        proposed
                            .parse::<u64>()
                            .map_err(|_| ConfigUnavailableReason::InvalidTarget)?
                            .into(),
                    )
                };
                if !compaction.contains_key(path[1]) {
                    return Err(ConfigUnavailableReason::MissingTarget);
                }
                compaction.insert(path[1].into(), value);
                let mut output = serde_json::to_vec_pretty(&document)
                    .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
                output.push(b'\n');
                Ok(output)
            }
            OperationSelector::NamedMarkdownModel(_) => edit_markdown_model(bytes, proposed),
            OperationSelector::NamedJsonMcpServer(name) => edit_mcp_value(bytes, name),
            OperationSelector::NamedOpenCodeBuiltInTool(name) => {
                edit_opencode_built_in_tool(bytes, name)
            }
            OperationSelector::NamedOpenCodeSkill(name) => edit_opencode_skill(bytes, name),
            _ => Err(ConfigUnavailableReason::UnsupportedSetting),
        }
    }
}

impl OpenCode {
    fn skill_target(
        &self,
        name: &str,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        reject_runtime_overrides()?;
        reject_remote_inputs(home)?;
        reject_managed_config()?;
        let mut winner = None;
        let mut collect = |skill_directory: &Path,
                           config_directory: &Path,
                           root: &Path,
                           scope|
         -> Result<(), ConfigUnavailableReason> {
            let skill = skill_directory.join(name).join("SKILL.md");
            if !path_entry_exists(&skill)? {
                return Ok(());
            }
            for file in ["opencode.json", "opencode.jsonc"] {
                let path = config_directory.join(file);
                if path_entry_exists(&path)?
                    && opencode_skill_value(&read_checked(&path, root)?.bytes, name)?.is_some()
                {
                    winner = Some((path, root.to_owned(), scope));
                }
            }
            Ok(())
        };
        let global = global_config_root(home)?;
        let home_config = home.join(".opencode");
        collect(
            &home_config.join("skills"),
            &home_config,
            home,
            ConfigScope::Global,
        )?;
        collect(
            &global.join("skills"),
            &global,
            &global,
            ConfigScope::Global,
        )?;
        if !project_config_disabled()
            && let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root)
        {
            for directory in project_hierarchy(cwd, root)? {
                let config = directory.join(".opencode");
                collect(&config.join("skills"), &config, root, ConfigScope::Project)?;
            }
        }
        let (path, safety_root, scope) = winner.ok_or(ConfigUnavailableReason::MissingTarget)?;
        Ok(Target {
            path,
            safety_root,
            scope,
            operation: OperationSelector::NamedOpenCodeSkill(name.to_owned()),
        })
    }
    fn built_in_tool_target(
        &self,
        name: &str,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        reject_runtime_overrides()?;
        reject_remote_inputs(home)?;
        reject_managed_config()?;
        let mut winner = None;
        let mut collect =
            |directory: &Path, root: &Path, scope| -> Result<(), ConfigUnavailableReason> {
                for file in ["opencode.json", "opencode.jsonc"] {
                    let path = directory.join(file);
                    if path_entry_exists(&path)?
                        && opencode_built_in_tool_value(&read_checked(&path, root)?.bytes, name)?
                            .is_some()
                    {
                        winner = Some((path, root.to_owned(), scope));
                    }
                }
                Ok(())
            };
        let global = global_config_root(home)?;
        collect(&global, &global, ConfigScope::Global)?;
        if !project_config_disabled()
            && let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root)
        {
            let directories = project_hierarchy(cwd, root)?;
            for directory in &directories {
                collect(directory, root, ConfigScope::Project)?;
            }
            for directory in directories.iter().rev() {
                let nested = directory.join(".opencode");
                if path_entry_exists(&nested)? {
                    collect(&nested, root, ConfigScope::Project)?;
                }
            }
        }
        let home_directory = home.join(".opencode");
        if path_entry_exists(&home_directory)? {
            collect(&home_directory, home, ConfigScope::Global)?;
        }
        let (path, safety_root, scope) = winner.ok_or(ConfigUnavailableReason::MissingTarget)?;
        Ok(Target {
            path,
            safety_root,
            scope,
            operation: OperationSelector::NamedOpenCodeBuiltInTool(name.to_owned()),
        })
    }

    fn mcp_target(
        &self,
        name: &str,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        reject_runtime_overrides()?;
        reject_remote_inputs(home)?;
        reject_managed_config()?;
        let mut matches = Vec::new();
        let mut collect =
            |directory: &Path, root: &Path, scope| -> Result<(), ConfigUnavailableReason> {
                for file in ["opencode.json", "opencode.jsonc"] {
                    let path = directory.join(file);
                    if path_entry_exists(&path)?
                        && mcp_value(&read_checked(&path, root)?.bytes, name)?.is_some()
                    {
                        matches.push((path, root.to_owned(), scope));
                    }
                }
                Ok(())
            };
        let global = global_config_root(home)?;
        collect(&global, &global, ConfigScope::Global)?;
        if !project_config_disabled()
            && let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root)
        {
            for directory in project_hierarchy(cwd, root)? {
                collect(&directory, root, ConfigScope::Project)?;
                let nested = directory.join(".opencode");
                if path_entry_exists(&nested)? {
                    collect(&nested, root, ConfigScope::Project)?;
                }
            }
        }
        let home_directory = home.join(".opencode");
        if path_entry_exists(&home_directory)? {
            collect(&home_directory, home, ConfigScope::Global)?;
        }
        if matches.len() != 1 {
            return Err(ConfigUnavailableReason::MissingTarget);
        }
        let (path, safety_root, scope) = matches.pop().expect("one exact MCP server");
        Ok(Target {
            path,
            safety_root,
            scope,
            operation: OperationSelector::NamedJsonMcpServer(name.to_owned()),
        })
    }
}

fn mcp_value(bytes: &[u8], name: &str) -> Result<Option<String>, ConfigUnavailableReason> {
    let document = parse(bytes)?;
    let Some(server) = document
        .get("mcp")
        .and_then(serde_json::Value::as_object)
        .and_then(|servers| servers.get(name))
    else {
        return Ok(None);
    };
    let enabled = server
        .as_object()
        .and_then(|server| server.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    Ok(Some(format!("{name}={enabled}")))
}

fn opencode_built_in_tool_value(
    bytes: &[u8],
    name: &str,
) -> Result<Option<String>, ConfigUnavailableReason> {
    let action = opencode_v2_action(name)?;
    let document = parse(bytes)?;
    if document.get("agents").is_some() {
        return Err(ConfigUnavailableReason::InvalidPrecedence);
    }
    let Some(rules) = document.get("permissions") else {
        return Ok(None);
    };
    let rules = rules
        .as_array()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    if rules.iter().any(|rule| !is_v2_permission_rule(rule)) {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    let enabled = !rules.iter().any(|rule| {
        rule.get("action").and_then(serde_json::Value::as_str) == Some(action)
            && rule.get("resource").and_then(serde_json::Value::as_str) == Some("*")
            && rule.get("effect").and_then(serde_json::Value::as_str) == Some("deny")
    });
    Ok(Some(format!("{name}={enabled}")))
}

fn opencode_skill_value(
    bytes: &[u8],
    name: &str,
) -> Result<Option<String>, ConfigUnavailableReason> {
    let document = parse(bytes)?;
    let Some(rules) = document.get("permissions") else {
        return Ok(None);
    };
    let rules = rules
        .as_array()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    if rules.iter().any(|rule| !is_v2_permission_rule(rule)) {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    Ok(Some(format!(
        "{name}={}",
        !rules.iter().any(|rule| {
            rule.get("action").and_then(serde_json::Value::as_str) == Some("skill")
                && rule.get("resource").and_then(serde_json::Value::as_str) == Some(name)
                && rule.get("effect").and_then(serde_json::Value::as_str) == Some("deny")
        })
    )))
}

fn is_v2_permission_rule(rule: &serde_json::Value) -> bool {
    rule.as_object().is_some_and(|rule| {
        rule.get("action")
            .and_then(serde_json::Value::as_str)
            .is_some()
            && rule
                .get("resource")
                .and_then(serde_json::Value::as_str)
                .is_some()
            && matches!(
                rule.get("effect").and_then(serde_json::Value::as_str),
                Some("allow" | "ask" | "deny")
            )
    })
}

fn opencode_v2_action(name: &str) -> Result<&str, ConfigUnavailableReason> {
    match name.to_ascii_lowercase().as_str() {
        "read" => Ok("read"),
        "edit" | "write" | "patch" => Ok("edit"),
        "glob" => Ok("glob"),
        "grep" => Ok("grep"),
        "bash" | "shell" => Ok("shell"),
        "agent" | "task" | "subagent" => Ok("subagent"),
        "skill" => Ok("skill"),
        "question" => Ok("question"),
        "webfetch" => Ok("webfetch"),
        "websearch" => Ok("websearch"),
        _ => Err(ConfigUnavailableReason::UnsupportedSetting),
    }
}

#[cfg(not(windows))]
fn edit_opencode_built_in_tool(
    bytes: &[u8],
    name: &str,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let action = opencode_v2_action(name)?;
    let mut document = parse(bytes)?;
    let rules = document
        .get_mut("permissions")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(ConfigUnavailableReason::MissingTarget)?;
    if rules.iter().any(|rule| !is_v2_permission_rule(rule)) {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    if rules.iter().any(|rule| {
        rule.get("action").and_then(serde_json::Value::as_str) == Some(action)
            && rule.get("resource").and_then(serde_json::Value::as_str) == Some("*")
            && rule.get("effect").and_then(serde_json::Value::as_str) == Some("deny")
    }) {
        return Err(ConfigUnavailableReason::CurrentValueMismatch);
    }
    rules.push(serde_json::json!({ "action": action, "resource": "*", "effect": "deny" }));
    let mut output = serde_json::to_vec_pretty(&document)
        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    output.push(b'\n');
    Ok(output)
}

#[cfg(not(windows))]
fn edit_opencode_skill(bytes: &[u8], name: &str) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let mut document = parse(bytes)?;
    let rules = document
        .get_mut("permissions")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(ConfigUnavailableReason::MissingTarget)?;
    if rules.iter().any(|rule| !is_v2_permission_rule(rule)) {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    if rules.iter().any(|rule| {
        rule.get("action").and_then(serde_json::Value::as_str) == Some("skill")
            && rule.get("resource").and_then(serde_json::Value::as_str) == Some(name)
            && rule.get("effect").and_then(serde_json::Value::as_str) == Some("deny")
    }) {
        return Err(ConfigUnavailableReason::CurrentValueMismatch);
    }
    rules.push(serde_json::json!({ "action": "skill", "resource": name, "effect": "deny" }));
    let mut output = serde_json::to_vec_pretty(&document)
        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    output.push(b'\n');
    Ok(output)
}

#[cfg(not(windows))]
fn edit_mcp_value(bytes: &[u8], name: &str) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let mut document = parse(bytes)?;
    let enabled = document
        .get_mut("mcp")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|servers| servers.get_mut(name))
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|server| server.get_mut("enabled"))
        .ok_or(ConfigUnavailableReason::MissingTarget)?;
    if enabled.as_bool() != Some(true) {
        return Err(ConfigUnavailableReason::CurrentValueMismatch);
    }
    *enabled = serde_json::Value::Bool(false);
    let mut output = serde_json::to_vec_pretty(&document)
        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    output.push(b'\n');
    Ok(output)
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

fn merge_directory_config(
    root: &Path,
    safety_root: &Path,
    scope: ConfigScope,
    setting: ConfigSetting,
    winner: &mut Option<Target>,
    include_legacy: bool,
) -> Result<(), ConfigUnavailableReason> {
    let names: &[&str] = if include_legacy {
        &["config.json", "opencode.json", "opencode.jsonc"]
    } else {
        &["opencode.json", "opencode.jsonc"]
    };
    for name in names {
        let path = root.join(name);
        if !path_entry_exists(&path)? {
            continue;
        }
        if let Some(operation) = operation_for(&read_checked(&path, safety_root)?.bytes, setting)? {
            *winner = Some(Target {
                path,
                safety_root: safety_root.to_owned(),
                scope,
                operation,
            });
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn collect_directory_targets(
    root: &Path,
    safety_root: &Path,
    scope: ConfigScope,
    include_legacy: bool,
    targets: &mut Vec<Target>,
) -> Result<(), ConfigUnavailableReason> {
    let names: &[&str] = if include_legacy {
        &["config.json", "opencode.json", "opencode.jsonc"]
    } else {
        &["opencode.json", "opencode.jsonc"]
    };
    for name in names {
        let path = root.join(name);
        if path_entry_exists(&path)?
            && model(&read_checked(&path, safety_root)?.bytes)?.is_some()
            && !targets.iter().any(|target| target.path == path)
        {
            targets.push(Target {
                path,
                safety_root: safety_root.to_owned(),
                scope,
                operation: OperationSelector::JsonKey("model"),
            });
        }
    }
    Ok(())
}

fn operation(setting: ConfigSetting) -> OperationSelector {
    match setting {
        ConfigSetting::Model => OperationSelector::JsonKey("model"),
        ConfigSetting::Compaction => OperationSelector::JsonPath(vec!["compaction", "auto"]),
        _ => unreachable!(),
    }
}

fn operation_for(
    bytes: &[u8],
    setting: ConfigSetting,
) -> Result<Option<OperationSelector>, ConfigUnavailableReason> {
    if setting == ConfigSetting::Model {
        return model(bytes).map(|value| value.map(|_| operation(setting)));
    }
    let document = parse(bytes)?;
    let Some(compaction) = document
        .get("compaction")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(None);
    };
    match compaction.get("auto") {
        Some(serde_json::Value::Bool(false)) => Ok(Some(OperationSelector::JsonPath(vec![
            "compaction",
            "auto",
        ]))),
        Some(serde_json::Value::Bool(true)) => match compaction.get("reserved") {
            Some(serde_json::Value::Number(_)) => Ok(Some(OperationSelector::JsonPath(vec![
                "compaction",
                "reserved",
            ]))),
            Some(_) => Err(ConfigUnavailableReason::MalformedConfig),
            None => Ok(None),
        },
        Some(_) => Err(ConfigUnavailableReason::MalformedConfig),
        None => Ok(None),
    }
}

fn model(bytes: &[u8]) -> Result<Option<String>, ConfigUnavailableReason> {
    let document = parse(bytes)?;
    reject_agent_overrides(&document)?;
    document
        .get("model")
        .map(|value| {
            let value = value
                .as_str()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            if value.contains("{env:") || value.contains("{file:") {
                Err(ConfigUnavailableReason::DynamicValue)
            } else {
                Ok(value.to_owned())
            }
        })
        .transpose()
}

fn reject_agent_overrides(document: &serde_json::Value) -> Result<(), ConfigUnavailableReason> {
    if document.get("agent").is_some() || document.get("mode").is_some() {
        Err(ConfigUnavailableReason::InvalidPrecedence)
    } else {
        Ok(())
    }
}

fn reject_runtime_overrides() -> Result<(), ConfigUnavailableReason> {
    if RUNTIME_OVERRIDE_NAMES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        Err(ConfigUnavailableReason::RuntimeOverride)
    } else {
        Ok(())
    }
}

fn project_config_disabled() -> bool {
    std::env::var("OPENCODE_DISABLE_PROJECT_CONFIG")
        .is_ok_and(|value| project_config_disabled_value(&value))
}

fn project_config_disabled_value(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "1" | "true")
}

fn global_config_root(home: &Path) -> Result<PathBuf, ConfigUnavailableReason> {
    let process_home = std::env::var_os("HOME");
    let xdg_config_home = process_home
        .as_deref()
        .is_some_and(|process_home| Path::new(process_home) == home)
        .then(|| std::env::var_os("XDG_CONFIG_HOME"))
        .flatten();
    global_config_root_for(home, xdg_config_home.as_deref())
}

fn global_config_root_for(
    home: &Path,
    value: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, ConfigUnavailableReason> {
    let Some(value) = value else {
        return Ok(home.join(".config/opencode"));
    };
    if value.is_empty() {
        return Ok(home.join(".config/opencode"));
    }
    let root = PathBuf::from(value);
    if !root.is_absolute() {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    }
    Ok(root.join("opencode"))
}

fn project_hierarchy(
    cwd: &Path,
    trusted_root: &Path,
) -> Result<Vec<PathBuf>, ConfigUnavailableReason> {
    if !cwd.starts_with(trusted_root) {
        return Err(ConfigUnavailableReason::UnsafePath);
    }
    let mut roots = vec![cwd.to_owned()];
    let mut current = cwd;
    while current != trusted_root {
        let Some(parent) = current.parent() else {
            return Err(ConfigUnavailableReason::UnsafePath);
        };
        if !parent.starts_with(trusted_root) {
            return Err(ConfigUnavailableReason::UnsafePath);
        }
        current = parent;
        roots.push(current.to_owned());
    }
    roots.reverse();
    Ok(roots)
}

fn reject_directory_overrides(root: &Path) -> Result<(), ConfigUnavailableReason> {
    for name in ["agent", "agents", "mode", "modes"] {
        if path_entry_exists(&root.join(name))? {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
    }
    Ok(())
}

fn reject_managed_config() -> Result<(), ConfigUnavailableReason> {
    #[cfg(windows)]
    {
        Ok(())
    }

    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        let root = Path::new("/Library/Application Support/opencode");
        #[cfg(all(unix, not(target_os = "macos")))]
        let root = Path::new("/etc/opencode");
        reject_managed_config_at(root)?;
        #[cfg(target_os = "macos")]
        reject_managed_preferences()?;
        Ok(())
    }
}

#[cfg(not(windows))]
fn reject_managed_config_at(root: &Path) -> Result<(), ConfigUnavailableReason> {
    for path in [root.join("opencode.json"), root.join("opencode.jsonc")] {
        if path_entry_exists(&path)? {
            return Err(ConfigUnavailableReason::ManagedConfiguration);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn reject_managed_preferences() -> Result<(), ConfigUnavailableReason> {
    let preferences = Path::new("/Library/Managed Preferences");
    if path_entry_exists(&preferences.join("ai.opencode.managed.plist"))? {
        return Err(ConfigUnavailableReason::ManagedConfiguration);
    }
    if path_entry_exists(preferences)? {
        let entries = std::fs::read_dir(preferences)
            .map_err(|_| ConfigUnavailableReason::PermissionDenied)?;
        for entry in entries {
            let entry = entry.map_err(|_| ConfigUnavailableReason::UnsafePath)?;
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(true)
                && path_entry_exists(&entry.path().join("ai.opencode.managed.plist"))?
            {
                return Err(ConfigUnavailableReason::ManagedConfiguration);
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn environment_inputs_are_table_driven() {
        let home = Path::new("/home/test");
        let config_cases = [
            (None, Ok(home.join(".config/opencode"))),
            (Some(""), Ok(home.join(".config/opencode"))),
            (Some("/tmp/xdg"), Ok(PathBuf::from("/tmp/xdg/opencode"))),
            (
                Some("relative"),
                Err(ConfigUnavailableReason::RuntimeOverride),
            ),
        ];
        for (value, expected) in config_cases {
            assert_eq!(
                global_config_root_for(home, value.map(std::ffi::OsStr::new)),
                expected
            );
        }
        for (value, expected) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("0", false),
            ("false", false),
            ("", false),
        ] {
            assert_eq!(project_config_disabled_value(value), expected);
        }
        assert_eq!(
            RUNTIME_OVERRIDE_NAMES,
            [
                "OPENCODE_CONFIG",
                "OPENCODE_CONFIG_DIR",
                "OPENCODE_CONFIG_CONTENT",
                "OPENCODE_AUTH_CONTENT",
                "OPENCODE_DB",
                "OPENCODE_DATA_DIR",
            ]
        );
    }

    #[test]
    fn managed_config_files_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        for name in ["opencode.json", "opencode.jsonc"] {
            let root = temporary.path().join(name.replace('.', "-"));
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join(name), "{}").unwrap();
            assert_eq!(
                reject_managed_config_at(&root),
                Err(ConfigUnavailableReason::ManagedConfiguration)
            );
        }
    }
}

fn reject_remote_inputs(home: &Path) -> Result<(), ConfigUnavailableReason> {
    let data_root = match std::env::var_os("XDG_DATA_HOME") {
        Some(value) if !value.is_empty() => {
            let root = PathBuf::from(value);
            if !root.is_absolute() {
                return Err(ConfigUnavailableReason::RuntimeOverride);
            }
            root.join("opencode")
        }
        _ => home.join(".local/share/opencode"),
    };
    let auth = data_root.join("auth.json");
    if path_entry_exists(&auth)? {
        let root = data_root
            .canonicalize()
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        let document = parse(&read_checked(&auth, &root)?.bytes)?;
        let entries = document
            .as_object()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?;
        if entries
            .values()
            .any(|entry| entry.get("type").and_then(serde_json::Value::as_str) == Some("wellknown"))
        {
            return Err(ConfigUnavailableReason::RuntimeOverride);
        }
    }
    if !path_entry_exists(&data_root)? {
        return Ok(());
    }
    let entries =
        std::fs::read_dir(&data_root).map_err(|_| ConfigUnavailableReason::PermissionDenied)?;
    for entry in entries {
        let entry = entry.map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name == "opencode.db" || name.starts_with("opencode-") && name.ends_with(".db")) {
            continue;
        }
        let connection = Connection::open_with_flags(
            entry.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| ConfigUnavailableReason::RuntimeOverride)?;
        let active = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_state WHERE active_org_id IS NOT NULL LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        );
        match active {
            Ok(true) => return Err(ConfigUnavailableReason::RuntimeOverride),
            Ok(false) => {}
            Err(error) if error.to_string().contains("no such table") => {}
            Err(_) => return Err(ConfigUnavailableReason::RuntimeOverride),
        }
    }
    Ok(())
}
