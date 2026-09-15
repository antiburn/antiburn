use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;

#[cfg(not(windows))]
use super::json::edit_top_level_string;
use super::json::parse_strict;
use super::{OperationSelector, Target, VendorConfig, VendorPolicy};
use crate::agent_config::filesystem::{path_entry_exists, read_checked};
use crate::agent_config::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

pub(super) struct Cursor;

pub(super) static CURSOR: Cursor = Cursor;

impl VendorConfig for Cursor {
    fn policy(&self, setting: ConfigSetting) -> VendorPolicy {
        match setting {
            ConfigSetting::Model => VendorPolicy::AutomaticEdit,
            ConfigSetting::Reasoning
            | ConfigSetting::Compaction
            | ConfigSetting::SubagentModel
            | ConfigSetting::McpServer
            | ConfigSetting::BuiltInTool
            | ConfigSetting::Skill
            | ConfigSetting::FastMode => {
                VendorPolicy::Unsupported(ConfigUnavailableReason::UnsupportedSetting)
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
        if setting != ConfigSetting::Model {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        let operation = OperationSelector::JsonKey("model");
        if let Some(root) = trusted_workspace_root {
            let path = root.join(".cursor/cli.json");
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
        let path = home.join(".cursor/cli-config.json");
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

    #[cfg(not(windows))]
    fn resolve_targets(
        &self,
        setting: ConfigSetting,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Vec<Target>, ConfigUnavailableReason> {
        let primary = self.resolve_target(setting, home, workspace_cwd, trusted_workspace_root)?;
        let operation = OperationSelector::JsonKey("model");
        let mut targets = vec![primary];
        for (path, safety_root, scope) in [
            (
                home.join(".cursor/cli-config.json"),
                home.to_owned(),
                ConfigScope::Global,
            ),
            (
                trusted_workspace_root
                    .map(|root| root.join(".cursor/cli.json"))
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
    ) -> Result<(PathBuf, Vec<u8>), ConfigUnavailableReason> {
        if setting != ConfigSetting::Model {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        Ok((
            home.join(".cursor/cli-config.json"),
            serde_json::to_vec_pretty(&serde_json::json!({ "model": proposed }))
                .map_err(|_| ConfigUnavailableReason::MalformedConfig)?,
        ))
    }

    #[cfg(not(windows))]
    fn standalone_selector(&self, setting: ConfigSetting) -> &'static str {
        match setting {
            ConfigSetting::Model => "model",
            _ => "standalone",
        }
    }

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        if operation != &OperationSelector::JsonKey("model") {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        parse_strict(bytes)?
            .get("model")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
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
        if operation != &OperationSelector::JsonKey("model") {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        edit_top_level_string(bytes, "model", proposed)
    }
}
