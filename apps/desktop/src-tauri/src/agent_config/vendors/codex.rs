use std::path::Path;

use toml_edit::DocumentMut;
#[cfg(not(windows))]
use toml_edit::value;

use super::{OperationSelector, Target, VendorConfig, VendorPolicy};
use crate::agent_config::filesystem::{path_entry_exists, read_checked};
use crate::agent_config::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

pub(super) struct Codex;

pub(super) static CODEX: Codex = Codex;

impl VendorConfig for Codex {
    fn policy(&self, setting: ConfigSetting) -> VendorPolicy {
        match setting {
            ConfigSetting::Model
            | ConfigSetting::Reasoning
            | ConfigSetting::Compaction
            | ConfigSetting::FastMode => VendorPolicy::AutomaticEdit,
            ConfigSetting::SubagentModel => VendorPolicy::AutomaticEdit,
            ConfigSetting::McpServer => VendorPolicy::AutomaticEdit,
            ConfigSetting::BuiltInTool => {
                VendorPolicy::Unsupported(ConfigUnavailableReason::UnsupportedSetting)
            }
            ConfigSetting::Skill => VendorPolicy::AutomaticEdit,
        }
    }

    fn resolve_target(
        &self,
        setting: ConfigSetting,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let operation = selector(setting);
        let global = home.join(".codex/config.toml");
        if let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root) {
            if !cwd.starts_with(root) {
                return Err(ConfigUnavailableReason::UnsafePath);
            }
            if project_is_trusted(&global, home, root)? {
                let mut winner = None;
                for directory in project_hierarchy(cwd, root)? {
                    let path = directory.join(".codex/config.toml");
                    if path_entry_exists(&path)?
                        && self
                            .read_value(&read_checked(&path, root)?.bytes, &operation)?
                            .is_some()
                    {
                        winner = Some(path);
                    }
                }
                if let Some(path) = winner {
                    return Ok(Target {
                        path,
                        safety_root: root.to_owned(),
                        scope: ConfigScope::Project,
                        operation,
                    });
                }
            }
        } else if workspace_cwd.is_some() || trusted_workspace_root.is_some() {
            return Err(ConfigUnavailableReason::UnsafePath);
        }

        if !path_entry_exists(&global)? {
            return Err(ConfigUnavailableReason::MissingConfig);
        }
        if self
            .read_value(&read_checked(&global, home)?.bytes, &operation)?
            .is_none()
        {
            return Err(ConfigUnavailableReason::MissingTarget);
        }
        Ok(Target {
            path: global,
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
            let name = expected.ok_or(ConfigUnavailableReason::MissingTarget)?;
            return self.mcp_target(name, home, workspace_cwd, trusted_workspace_root);
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
        let expected = expected.ok_or(ConfigUnavailableReason::MissingTarget)?;
        let directories = [
            home.join(".codex/agents"),
            trusted_workspace_root
                .map(|root| root.join(".codex/agents"))
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
                if path.extension().and_then(|value| value.to_str()) != Some("toml") {
                    continue;
                }
                let document = parse_document(&read_checked(&path, root)?.bytes)?;
                if document.get("model").and_then(toml_edit::Item::as_str) == Some(expected) {
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
            operation: OperationSelector::NamedTomlModel(name),
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
        if setting == ConfigSetting::FastMode {
            return Ok(vec![primary]);
        }
        let operation = selector(setting);
        let mut targets = vec![primary];
        let global = home.join(".codex/config.toml");
        if path_entry_exists(&global)? && !targets.iter().any(|target| target.path == global) {
            targets.push(Target {
                path: global,
                safety_root: home.to_owned(),
                scope: ConfigScope::Global,
                operation: operation.clone(),
            });
        }
        if let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root)
            && project_is_trusted(&home.join(".codex/config.toml"), home, root)?
        {
            for directory in project_hierarchy(cwd, root)? {
                let path = directory.join(".codex/config.toml");
                if path_entry_exists(&path)? && !targets.iter().any(|target| target.path == path) {
                    targets.push(Target {
                        path,
                        safety_root: root.to_owned(),
                        scope: ConfigScope::Project,
                        operation: operation.clone(),
                    });
                }
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
        let key = match setting {
            ConfigSetting::Model => "model",
            ConfigSetting::Reasoning => "model_reasoning_effort",
            ConfigSetting::Compaction => "model_auto_compact_token_limit",
            ConfigSetting::FastMode => "service_tier",
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        };
        Ok((
            home.join(".codex/config.toml"),
            format!("{key} = {proposed:?}\n").into_bytes(),
        ))
    }

    #[cfg(not(windows))]
    fn standalone_selector(&self, setting: ConfigSetting) -> &'static str {
        match setting {
            ConfigSetting::Model => "model",
            ConfigSetting::Reasoning => "model_reasoning_effort",
            ConfigSetting::Compaction => "model_auto_compact_token_limit",
            ConfigSetting::FastMode => "service_tier",
            _ => "standalone",
        }
    }

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        let document = parse_document(bytes)?;
        reject_active_profile(&document)?;
        let key = match operation {
            OperationSelector::TomlKey(key) => *key,
            OperationSelector::NamedTomlModel(_) => "model",
            OperationSelector::NamedTomlMcpServer(name) => return mcp_value(&document, name),
            OperationSelector::NamedTomlSkill(name) => return skill_value(&document, name),
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        };
        document
            .get(key)
            .map(|item| {
                item.as_str()
                    .map(ToOwned::to_owned)
                    .or_else(|| item.as_integer().map(|value| value.to_string()))
                    .ok_or(ConfigUnavailableReason::MalformedConfig)
            })
            .transpose()
    }

    #[cfg(not(windows))]
    fn edit_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
        proposed: &str,
    ) -> Result<Vec<u8>, ConfigUnavailableReason> {
        let key = match operation {
            OperationSelector::TomlKey(key) => *key,
            OperationSelector::NamedTomlModel(_) => "model",
            OperationSelector::NamedTomlMcpServer(name) => {
                let mut document = parse_document(bytes)?;
                reject_active_profile(&document)?;
                let server = document
                    .get_mut("mcp_servers")
                    .and_then(|item| item.as_table_like_mut())
                    .and_then(|servers| servers.get_mut(name))
                    .and_then(|item| item.as_table_like_mut())
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                if server.get("enabled").and_then(toml_edit::Item::as_bool) != Some(true) {
                    return Err(ConfigUnavailableReason::CurrentValueMismatch);
                }
                server.insert("enabled", value(false));
                return Ok(document.to_string().into_bytes());
            }
            OperationSelector::NamedTomlSkill(name) => {
                let mut document = parse_document(bytes)?;
                reject_active_profile(&document)?;
                let skill = document
                    .get_mut("skills")
                    .and_then(|item| item.as_table_like_mut())
                    .and_then(|skills| skills.get_mut("config"))
                    .and_then(|item| item.as_table_like_mut())
                    .and_then(|skills| skills.get_mut(name))
                    .and_then(|item| item.as_table_like_mut())
                    .ok_or(ConfigUnavailableReason::MissingTarget)?;
                if skill.get("enabled").and_then(toml_edit::Item::as_bool) != Some(true) {
                    return Err(ConfigUnavailableReason::CurrentValueMismatch);
                }
                skill.insert("enabled", value(false));
                return Ok(document.to_string().into_bytes());
            }
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        };
        let mut document = parse_document(bytes)?;
        reject_active_profile(&document)?;
        document[key] = if key == "model_auto_compact_token_limit" {
            value(
                proposed
                    .parse::<i64>()
                    .map_err(|_| ConfigUnavailableReason::InvalidTarget)?,
            )
        } else {
            value(proposed)
        };
        Ok(document.to_string().into_bytes())
    }
}

impl Codex {
    fn skill_target(
        &self,
        name: &str,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let (path, root, scope) =
            self.current_skill_definition(name, home, workspace_cwd, trusted_workspace_root)?;
        let document = parse_document(&read_checked(&path, &root)?.bytes)?;
        let expected = format!("{name}=true");
        if skill_value(&document, name)?.as_deref() != Some(expected.as_str()) {
            return Err(ConfigUnavailableReason::MissingTarget);
        }
        Ok(Target {
            path,
            safety_root: root,
            scope,
            operation: OperationSelector::NamedTomlSkill(name.to_owned()),
        })
    }

    fn current_skill_definition(
        &self,
        name: &str,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<(std::path::PathBuf, std::path::PathBuf, ConfigScope), ConfigUnavailableReason>
    {
        if let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root)
            && project_is_trusted(&home.join(".codex/config.toml"), home, root)?
        {
            let mut winner = None;
            for directory in project_hierarchy(cwd, root)? {
                let skill = directory.join(".codex/skills").join(name).join("SKILL.md");
                if path_entry_exists(&skill)? {
                    winner = Some((
                        directory.join(".codex/config.toml"),
                        root.to_owned(),
                        ConfigScope::Project,
                    ));
                }
            }
            if let Some(value) = winner {
                return Ok(value);
            }
        }
        let skill = home.join(".codex/skills").join(name).join("SKILL.md");
        if path_entry_exists(&skill)? {
            Ok((
                home.join(".codex/config.toml"),
                home.to_owned(),
                ConfigScope::Global,
            ))
        } else {
            Err(ConfigUnavailableReason::MissingTarget)
        }
    }
    fn mcp_target(
        &self,
        name: &str,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        let global = home.join(".codex/config.toml");
        let mut matches = Vec::new();
        if path_entry_exists(&global)?
            && mcp_value(&parse_document(&read_checked(&global, home)?.bytes)?, name)?.is_some()
        {
            matches.push((global, home.to_owned(), ConfigScope::Global));
        }
        if let (Some(cwd), Some(root)) = (workspace_cwd, trusted_workspace_root) {
            if !project_is_trusted(&home.join(".codex/config.toml"), home, root)? {
                return Err(ConfigUnavailableReason::MissingTarget);
            }
            for directory in project_hierarchy(cwd, root)? {
                let path = directory.join(".codex/config.toml");
                if path_entry_exists(&path)?
                    && mcp_value(&parse_document(&read_checked(&path, root)?.bytes)?, name)?
                        .is_some()
                {
                    matches.push((path, root.to_owned(), ConfigScope::Project));
                }
            }
        }
        if matches.len() != 1 {
            return Err(ConfigUnavailableReason::MissingTarget);
        }
        let (path, safety_root, scope) = matches.pop().expect("one exact MCP server");
        Ok(Target {
            path,
            safety_root,
            scope,
            operation: OperationSelector::NamedTomlMcpServer(name.to_owned()),
        })
    }
}

fn mcp_value(
    document: &DocumentMut,
    name: &str,
) -> Result<Option<String>, ConfigUnavailableReason> {
    reject_active_profile(document)?;
    let Some(server) = document
        .get("mcp_servers")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|servers| servers.get(name))
    else {
        return Ok(None);
    };
    let server = server
        .as_table_like()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    server
        .get("enabled")
        .map(|enabled| {
            enabled
                .as_bool()
                .map(|enabled| format!("{name}={enabled}"))
                .ok_or(ConfigUnavailableReason::MalformedConfig)
        })
        .transpose()
}

fn skill_value(
    document: &DocumentMut,
    name: &str,
) -> Result<Option<String>, ConfigUnavailableReason> {
    reject_active_profile(document)?;
    let Some(skill) = document
        .get("skills")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|skills| skills.get("config"))
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|skills| skills.get(name))
    else {
        return Ok(None);
    };
    skill
        .as_table_like()
        .and_then(|skill| skill.get("enabled"))
        .map(|enabled| {
            enabled
                .as_bool()
                .map(|value| format!("{name}={value}"))
                .ok_or(ConfigUnavailableReason::MalformedConfig)
        })
        .transpose()
}

fn project_hierarchy(
    cwd: &Path,
    root: &Path,
) -> Result<Vec<std::path::PathBuf>, ConfigUnavailableReason> {
    let mut directories = vec![cwd.to_owned()];
    let mut current = cwd;
    while current != root {
        let parent = current
            .parent()
            .ok_or(ConfigUnavailableReason::UnsafePath)?;
        if !parent.starts_with(root) {
            return Err(ConfigUnavailableReason::UnsafePath);
        }
        directories.push(parent.to_owned());
        current = parent;
    }
    directories.reverse();
    Ok(directories)
}

fn selector(setting: ConfigSetting) -> OperationSelector {
    match setting {
        ConfigSetting::Model => OperationSelector::TomlKey("model"),
        ConfigSetting::Reasoning => OperationSelector::TomlKey("model_reasoning_effort"),
        ConfigSetting::Compaction => OperationSelector::TomlKey("model_auto_compact_token_limit"),
        ConfigSetting::FastMode => OperationSelector::TomlKey("service_tier"),
        ConfigSetting::SubagentModel
        | ConfigSetting::McpServer
        | ConfigSetting::BuiltInTool
        | ConfigSetting::Skill => unreachable!("unsupported settings do not resolve targets"),
    }
}

fn project_is_trusted(
    global: &Path,
    home: &Path,
    workspace: &Path,
) -> Result<bool, ConfigUnavailableReason> {
    if !path_entry_exists(global)? {
        return Ok(false);
    }
    let document = parse_document(&read_checked(global, home)?.bytes)?;
    reject_active_profile(&document)?;
    let Some(projects) = document
        .get("projects")
        .and_then(toml_edit::Item::as_table_like)
    else {
        return Ok(false);
    };
    let workspace = workspace
        .to_str()
        .ok_or(ConfigUnavailableReason::UnsafePath)?;
    Ok(projects
        .get(workspace)
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|project| project.get("trust_level"))
        .and_then(toml_edit::Item::as_str)
        == Some("trusted"))
}

fn reject_active_profile(document: &DocumentMut) -> Result<(), ConfigUnavailableReason> {
    if document.get("profile").is_some() {
        Err(ConfigUnavailableReason::RuntimeOverride)
    } else {
        Ok(())
    }
}

fn parse_document(bytes: &[u8]) -> Result<DocumentMut, ConfigUnavailableReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    text.parse::<DocumentMut>().map_err(|error| {
        let message = error.to_string().to_ascii_lowercase();
        if message.contains("duplicate") || message.contains("redefinition") {
            ConfigUnavailableReason::DuplicateDefinition
        } else {
            ConfigUnavailableReason::MalformedConfig
        }
    })
}

#[cfg(all(test, not(windows)))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use antiburn_local::model::AgentKind;

    use crate::agent_config::{
        AgentConfigEditor, ConfigChange, ConfigContext, ConfigScope, ConfigUnavailableReason,
    };

    #[test]
    fn preserves_formatting_and_inherited_scope() {
        let (_temporary, home, project) = roots();
        write(
            &home.join(".codex/config.toml"),
            "# keep\nmodel = \"old\"\n",
        );
        write(
            &project.join(".codex/config.toml"),
            "approval_policy = \"ask\"\n",
        );
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Codex, &home, Some(project));
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
        #[cfg(not(windows))]
        {
            editor.apply(&prepared).unwrap();
            let text = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
            assert!(text.starts_with("# keep\n"));
            assert!(text.contains("model = \"new\""));
        }
    }

    #[test]
    fn uses_project_model_only_for_an_explicitly_trusted_project() {
        let (_temporary, home, project) = roots();
        let project_key = project.canonicalize().unwrap();
        let project_key = project_key.to_string_lossy();
        write(
            &home.join(".codex/config.toml"),
            &format!(
                "model = \"global\"\n[projects.{}]\ntrust_level = \"trusted\"\n",
                toml_edit::Value::from(project_key.as_ref())
            ),
        );
        write(&project.join(".codex/config.toml"), "model = \"project\"\n");
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Codex, &home, Some(project.clone()));
        let effective = editor.effective_model(&context).unwrap();
        assert_eq!(effective.scope, ConfigScope::Project);
        assert_eq!(effective.value, "project");

        write(&home.join(".codex/config.toml"), "model = \"global\"\n");
        let effective = editor.effective_model(&context).unwrap();
        assert_eq!(effective.scope, ConfigScope::Global);
        assert_eq!(effective.value, "global");
    }

    #[test]
    fn prepares_applies_and_reads_back_a_project_model() {
        let (_temporary, home, project) = roots();
        let project_key = project.canonicalize().unwrap();
        let project_key = project_key.to_string_lossy();
        write(
            &home.join(".codex/config.toml"),
            &format!(
                "model = \"global\"\n[projects.{}]\ntrust_level = \"trusted\"\n",
                toml_edit::Value::from(project_key.as_ref())
            ),
        );
        let path = project.join(".codex/config.toml");
        write(
            &path,
            "# keep\nmodel = \"project-old\"\napproval_policy = \"ask\"\n",
        );
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Codex, &home, Some(project));
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
        let text = fs::read_to_string(path).unwrap();
        assert!(text.starts_with("# keep\n"));
        assert!(text.contains("approval_policy = \"ask\""));
    }

    #[test]
    fn changes_only_an_explicit_fast_service_tier_to_standard() {
        let (_temporary, home, project) = roots();
        let path = home.join(".codex/config.toml");
        write(&path, "service_tier = \"fast\"\nmodel = \"gpt-5.6\"\n");
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Codex, &home, Some(project));
        let prepared = editor
            .prepare_operation(
                &context,
                &crate::agent_config::ConfigOperation {
                    setting: crate::agent_config::ConfigSetting::FastMode,
                    expected_value: "fast".into(),
                    proposed_value: "standard".into(),
                },
            )
            .unwrap();
        editor.apply(&prepared).unwrap();
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .contains("service_tier = \"standard\"")
        );
    }

    #[test]
    fn rejects_an_active_profile_override() {
        let (_temporary, home, project) = roots();
        write(
            &home.join(".codex/config.toml"),
            "model = \"old\"\nprofile = \"work\"\n",
        );
        let context = ConfigContext::native(AgentKind::Codex, &home, Some(project));
        assert_eq!(
            AgentConfigEditor::new().effective_model(&context),
            Err(ConfigUnavailableReason::RuntimeOverride)
        );
    }

    #[test]
    fn rejects_a_global_active_profile_for_a_project_model() {
        let (_temporary, home, project) = roots();
        let project_key = project.canonicalize().unwrap();
        let project_key = project_key.to_string_lossy();
        write(
            &home.join(".codex/config.toml"),
            &format!(
                "model = \"global\"\nprofile = \"work\"\n[projects.{}]\ntrust_level = \"trusted\"\n",
                toml_edit::Value::from(project_key.as_ref())
            ),
        );
        write(&project.join(".codex/config.toml"), "model = \"project\"\n");
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Codex, &home, Some(project));

        assert_eq!(
            editor.effective_model(&context),
            Err(ConfigUnavailableReason::RuntimeOverride)
        );
        assert_eq!(
            editor
                .prepare(
                    &context,
                    &ConfigChange {
                        expected_value: "project".into(),
                        proposed_value: "new".into(),
                    },
                )
                .unwrap_err(),
            ConfigUnavailableReason::RuntimeOverride
        );
    }

    #[test]
    fn apply_rejects_a_new_global_active_profile_for_a_project_model() {
        let (_temporary, home, project) = roots();
        let project_key = project.canonicalize().unwrap();
        let project_key = project_key.to_string_lossy();
        let global = home.join(".codex/config.toml");
        let trusted = format!(
            "model = \"global\"\n[projects.{}]\ntrust_level = \"trusted\"\n",
            toml_edit::Value::from(project_key.as_ref())
        );
        write(&global, &trusted);
        let project_config = project.join(".codex/config.toml");
        write(&project_config, "model = \"project-old\"\n");
        let editor = AgentConfigEditor::new();
        let context = ConfigContext::native(AgentKind::Codex, &home, Some(project));
        let prepared = editor
            .prepare(
                &context,
                &ConfigChange {
                    expected_value: "project-old".into(),
                    proposed_value: "project-new".into(),
                },
            )
            .unwrap();
        write(
            &global,
            &trusted.replacen('\n', "\nprofile = \"work\"\n", 1),
        );

        assert_eq!(
            editor.apply(&prepared),
            Err(crate::agent_config::ApplyError::Unavailable(
                ConfigUnavailableReason::RuntimeOverride
            ))
        );
        assert_eq!(
            fs::read_to_string(project_config).unwrap(),
            "model = \"project-old\"\n"
        );
    }

    fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        (temporary, home, project)
    }

    fn write(path: &Path, value: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
}
