use std::path::Path;

use serde_json::Value;

use super::json::parse_strict;
use super::{OperationSelector, Target, VendorConfig, VendorPolicy};
use crate::agent_config::filesystem::{path_entry_exists, read_checked};
use crate::agent_config::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

pub(super) struct Claude;

pub(super) static CLAUDE: Claude = Claude;

impl VendorConfig for Claude {
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
        reject_runtime_overrides(setting)?;
        if setting == ConfigSetting::Reasoning {
            return self.resolve_reasoning(home, trusted_workspace_root);
        }
        let operation = OperationSelector::JsonKey("model");
        let mut target = None;
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
                    target.get_or_insert(Target {
                        path,
                        safety_root: root.to_owned(),
                        scope: ConfigScope::Project,
                        operation: operation.clone(),
                    });
                }
            }
        }

        let path = home.join(".claude/settings.json");
        let global_exists = path_entry_exists(&path)?;
        if global_exists
            && self
                .read_value(&read_checked(&path, home)?.bytes, &operation)?
                .is_some()
        {
            target.get_or_insert(Target {
                path,
                safety_root: home.to_owned(),
                scope: ConfigScope::Global,
                operation,
            });
        }
        target.ok_or(if global_exists {
            ConfigUnavailableReason::MissingTarget
        } else {
            ConfigUnavailableReason::MissingConfig
        })
    }

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        let document = parse_strict(bytes)?;
        match operation {
            OperationSelector::JsonKey(key) => {
                reject_settings_overrides(
                    &document,
                    if *key == "model" {
                        ConfigSetting::Model
                    } else {
                        ConfigSetting::Reasoning
                    },
                )?;
                string_property(&document, key)
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
        let mut root = parse_strict(bytes)?;
        match operation {
            OperationSelector::JsonKey(key) => {
                root.as_object_mut()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?
                    .insert((*key).into(), Value::String(proposed.into()));
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
            _ => return Err(ConfigUnavailableReason::UnsupportedSetting),
        }
        let mut output = serde_json::to_vec_pretty(&root)
            .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
        output.push(b'\n');
        Ok(output)
    }
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

        let mut target = None;
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
                    target.get_or_insert(Target {
                        path,
                        safety_root: root.to_owned(),
                        scope: ConfigScope::Project,
                        operation,
                    });
                }
            }
        }

        let path = home.join(".claude/settings.json");
        let global_exists = path_entry_exists(&path)?;
        if global_exists
            && let Some(operation) =
                reasoning_selector(&parse_strict(&read_checked(&path, home)?.bytes)?, &model)?
        {
            target.get_or_insert(Target {
                path,
                safety_root: home.to_owned(),
                scope: ConfigScope::Global,
                operation,
            });
        }
        target.ok_or(if global_exists {
            ConfigUnavailableReason::MissingTarget
        } else {
            ConfigUnavailableReason::MissingConfig
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
    fn rejects_a_global_model_environment_override_after_a_local_model() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        write(
            &project.join(".claude/settings.local.json"),
            r#"{"model":"project-model"}"#,
        );
        write(
            &home.join(".claude/settings.json"),
            r#"{"env":{"ANTHROPIC_MODEL":"global-model"}}"#,
        );

        assert_eq!(
            AgentConfigEditor::new().effective_model(&ConfigContext::native(
                AgentKind::Claude,
                &home,
                Some(project),
            )),
            Err(crate::agent_config::ConfigUnavailableReason::RuntimeOverride)
        );
    }

    #[test]
    fn rejects_a_global_reasoning_environment_override_after_a_local_setting() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        write(
            &project.join(".claude/settings.local.json"),
            r#"{"model":"project-model","effortLevel":"high"}"#,
        );
        write(
            &home.join(".claude/settings.json"),
            r#"{"env":{"CLAUDE_CODE_EFFORT_LEVEL":"low"}}"#,
        );

        assert_eq!(
            AgentConfigEditor::new().effective(
                &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
                crate::agent_config::ConfigSetting::Reasoning,
            ),
            Err(crate::agent_config::ConfigUnavailableReason::RuntimeOverride)
        );
    }

    fn write(path: &Path, value: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
}
