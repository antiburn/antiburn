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
    fn policy(&self, _: ConfigSetting) -> VendorPolicy {
        VendorPolicy::AutomaticEdit
    }

    fn resolve_target(
        &self,
        setting: ConfigSetting,
        home: &Path,
        _workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        if let (Some(cwd), Some(root)) = (_workspace_cwd, trusted_workspace_root)
            && cwd != root
        {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
        let operation = selector(setting);
        let global = home.join(".codex/config.toml");
        if let Some(root) = trusted_workspace_root {
            let path = root.join(".codex/config.toml");
            if path_entry_exists(&path)?
                && self
                    .read_value(&read_checked(&path, root)?.bytes, &operation)?
                    .is_some()
                && project_is_trusted(&global, home, root)?
            {
                return Ok(Target {
                    path,
                    safety_root: root.to_owned(),
                    scope: ConfigScope::Project,
                    operation,
                });
            }
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

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        let OperationSelector::TomlKey(key) = operation else {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        };
        let document = parse_document(bytes)?;
        reject_active_profile(&document)?;
        document
            .get(key)
            .map(|item| {
                item.as_str()
                    .map(ToOwned::to_owned)
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
        let OperationSelector::TomlKey(key) = operation else {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        };
        let mut document = parse_document(bytes)?;
        reject_active_profile(&document)?;
        document[*key] = value(proposed);
        Ok(document.to_string().into_bytes())
    }
}

fn selector(setting: ConfigSetting) -> OperationSelector {
    match setting {
        ConfigSetting::Model => OperationSelector::TomlKey("model"),
        ConfigSetting::Reasoning => OperationSelector::TomlKey("model_reasoning_effort"),
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
