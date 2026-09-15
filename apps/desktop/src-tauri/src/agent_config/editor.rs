#[cfg(not(windows))]
use std::fs;
#[cfg(not(windows))]
use std::io::Write;
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(windows))]
use super::config::{ApplyConflict, ApplyError, ApplyReadbackError};
use super::config::{
    ConfigChange, ConfigContext, ConfigOperation, ConfigSetting, ConfigUnavailableReason,
    EffectiveConfig, PreparedOperation,
};
#[cfg(not(windows))]
use super::config::{ConfigScope, PreparedChange};
use super::filesystem::{canonical_root, read_checked};
#[cfg(not(windows))]
use super::filesystem::{create_temporary, file_identity, file_ownership, map_write_error};
use super::vendors::{VendorPolicy, vendor_for};

#[derive(Debug, Default)]
pub struct AgentConfigEditor;

impl AgentConfigEditor {
    pub const fn new() -> Self {
        Self
    }

    pub fn effective(
        &self,
        context: &ConfigContext,
        setting: ConfigSetting,
    ) -> Result<EffectiveConfig, ConfigUnavailableReason> {
        self.effective_for_value(context, setting, None)
    }

    pub fn effective_for_value(
        &self,
        context: &ConfigContext,
        setting: ConfigSetting,
        expected: Option<&str>,
    ) -> Result<EffectiveConfig, ConfigUnavailableReason> {
        let vendor = validate_read_context(context, setting, current_platform())?;
        let home = canonical_root(&context.home_root)?;
        let (workspace_cwd, trusted_workspace_root) = canonical_workspace(context)?;
        let target = vendor.resolve_target_for_value(
            setting,
            expected,
            &home,
            workspace_cwd.as_deref(),
            trusted_workspace_root.as_deref(),
        )?;
        let file = read_checked(&target.path, &target.safety_root)?;
        let value = vendor
            .read_value(&file.bytes, &target.operation)?
            .ok_or(ConfigUnavailableReason::MissingTarget)?;
        Ok(EffectiveConfig {
            setting,
            scope: target.scope,
            value,
            selector: target.operation.physical_selector(),
            path: target.path,
        })
    }

    pub fn effective_model(
        &self,
        context: &ConfigContext,
    ) -> Result<EffectiveConfig, ConfigUnavailableReason> {
        self.effective(context, ConfigSetting::Model)
    }

    #[cfg(windows)]
    pub fn prepare_operation(
        &self,
        context: &ConfigContext,
        operation: &ConfigOperation,
    ) -> Result<PreparedOperation, ConfigUnavailableReason> {
        validate_write_context(context, operation.setting, current_platform())?;
        Err(ConfigUnavailableReason::AutomaticApplyUnsupported)
    }

    #[cfg(not(windows))]
    pub fn prepare_operation(
        &self,
        context: &ConfigContext,
        operation: &ConfigOperation,
    ) -> Result<PreparedOperation, ConfigUnavailableReason> {
        let vendor = validate_write_context(context, operation.setting, current_platform())?;
        let expected = operation.expected_value.display_value();
        let proposed = operation.proposed_value.display_value();
        validate_value(operation.setting, &expected)?;
        validate_value(operation.setting, &proposed)?;
        if operation.expected_value == operation.proposed_value {
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
        let home = canonical_root(&context.home_root)?;
        let (workspace_cwd, trusted_workspace_root) = canonical_workspace(context)?;
        let targets = if matches!(
            operation.setting,
            ConfigSetting::SubagentModel
                | ConfigSetting::McpServer
                | ConfigSetting::BuiltInTool
                | ConfigSetting::Skill
        ) {
            vendor
                .resolve_target_for_value(
                    operation.setting,
                    operation
                        .expected_value
                        .scalar()
                        .or_else(|| operation.expected_value.key()),
                    &home,
                    workspace_cwd.as_deref(),
                    trusted_workspace_root.as_deref(),
                )
                .map(|target| vec![target])
        } else {
            vendor.resolve_targets(
                operation.setting,
                &home,
                workspace_cwd.as_deref(),
                trusted_workspace_root.as_deref(),
            )
        };
        let mut changes = Vec::new();
        let mut creations = Vec::new();
        let targets = match targets {
            Ok(targets) => targets,
            Err(
                ConfigUnavailableReason::MissingConfig | ConfigUnavailableReason::MissingTarget,
            ) if !matches!(
                operation.setting,
                ConfigSetting::SubagentModel | ConfigSetting::McpServer | ConfigSetting::Skill
            ) =>
            {
                let (path, bytes) =
                    vendor.standalone_global(operation.setting, &home, &proposed)?;
                creations.push(super::config::PreparedCreation {
                    setting: operation.setting,
                    selector: vendor.standalone_selector(operation.setting),
                    scope: ConfigScope::Global,
                    safety_root: home.clone(),
                    path,
                    bytes,
                });
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        for (index, target) in targets.into_iter().enumerate() {
            let file = read_checked(&target.path, &target.safety_root)?;
            let current = vendor.read_value(&file.bytes, &target.operation)?;
            if index == 0 && current.as_deref() != Some(expected.as_str()) {
                return Err(ConfigUnavailableReason::CurrentValueMismatch);
            }
            if current.as_deref() == Some(proposed.as_str()) {
                continue;
            }
            let proposed_bytes = vendor.edit_value(&file.bytes, &target.operation, &proposed)?;
            changes.push(PreparedChange {
                agent: context.agent,
                setting: operation.setting,
                selector: target.operation.physical_selector(),
                operation: target.operation,
                expected_value: expected.clone(),
                path: target.path,
                scope: target.scope,
                resolution_home_root: home.clone(),
                workspace_cwd: workspace_cwd.clone(),
                trusted_workspace_root: trusted_workspace_root.clone(),
                safety_root: target.safety_root,
                original_bytes: file.bytes,
                proposed_bytes,
                identity: file.identity,
                permissions: file.permissions,
                #[cfg(unix)]
                ownership: file.ownership,
            });
        }
        if changes.is_empty() && creations.is_empty() {
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
        Ok(PreparedOperation {
            warning: context.runtime_override_present || context.managed_configuration_present,
            changes,
            creations,
        })
    }

    pub fn prepare(
        &self,
        context: &ConfigContext,
        change: &ConfigChange,
    ) -> Result<PreparedOperation, ConfigUnavailableReason> {
        self.prepare_operation(context, &ConfigOperation::model(change))
    }

    #[cfg(not(windows))]
    pub fn apply(&self, prepared: &PreparedOperation) -> Result<(), ApplyError> {
        let mut applied = Vec::new();
        for change in &prepared.changes {
            let result = if applied.is_empty() {
                self.apply_change(change)
            } else {
                self.apply_change_without_resolution(change)
            };
            if let Err(error) = result {
                let mut rollback_failed = false;
                for completed in applied.into_iter().rev() {
                    rollback_failed |= self.rollback_change(completed).is_err();
                }
                return Err(if rollback_failed {
                    ApplyError::Readback(ApplyReadbackError::ReadFailed)
                } else {
                    error
                });
            }
            applied.push(change);
        }
        for creation in &prepared.creations {
            if let Err(error) = self.apply_creation(creation) {
                let mut rollback_failed = false;
                for created in prepared
                    .creations
                    .iter()
                    .take_while(|item| item.path != creation.path)
                {
                    rollback_failed |= self.rollback_creation(created).is_err();
                }
                for completed in applied.into_iter().rev() {
                    rollback_failed |= self.rollback_change(completed).is_err();
                }
                return Err(if rollback_failed {
                    ApplyError::Readback(ApplyReadbackError::ReadFailed)
                } else {
                    error
                });
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply_creation(&self, creation: &super::config::PreparedCreation) -> Result<(), ApplyError> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let parent = creation
            .path
            .parent()
            .ok_or(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath))?;
        if !parent.starts_with(&creation.safety_root) {
            return Err(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath));
        }
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
        if std::fs::symlink_metadata(&creation.path).is_ok() {
            return Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity));
        }
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&creation.path)
            .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
        output
            .write_all(&creation.bytes)
            .and_then(|()| output.sync_all())
            .map_err(|error| ApplyError::Unavailable(map_write_error(error)))
    }

    #[cfg(not(windows))]
    fn rollback_creation(
        &self,
        creation: &super::config::PreparedCreation,
    ) -> Result<(), ApplyError> {
        let bytes = std::fs::read(&creation.path)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?;
        if bytes != creation.bytes {
            return Err(ApplyError::Conflict(ApplyConflict::ChangedContent));
        }
        std::fs::remove_file(&creation.path)
            .map_err(|error| ApplyError::Unavailable(map_write_error(error)))
    }

    #[cfg(not(windows))]
    fn apply_change(&self, prepared: &PreparedChange) -> Result<(), ApplyError> {
        let vendor = vendor_for(prepared.agent);
        let resource_name = match &prepared.operation {
            super::vendors::OperationSelector::NamedClaudeMcpServer(name)
            | super::vendors::OperationSelector::NamedTomlMcpServer(name)
            | super::vendors::OperationSelector::NamedJsonMcpServer(name)
            | super::vendors::OperationSelector::NamedClaudeBuiltInTool(name)
            | super::vendors::OperationSelector::NamedOpenCodeBuiltInTool(name)
            | super::vendors::OperationSelector::NamedPiDefaultTool(name)
            | super::vendors::OperationSelector::NamedClaudeSkill(name)
            | super::vendors::OperationSelector::NamedTomlSkill(name)
            | super::vendors::OperationSelector::NamedOpenCodeSkill(name) => Some(name.as_str()),
            _ => None,
        };
        let current_target = vendor
            .resolve_target_for_value(
                prepared.setting,
                resource_name.or(Some(&prepared.expected_value)),
                &prepared.resolution_home_root,
                prepared.workspace_cwd.as_deref(),
                prepared.trusted_workspace_root.as_deref(),
            )
            .map_err(ApplyError::Unavailable)?;
        if current_target.path != prepared.path
            || current_target.scope != prepared.scope
            || current_target.safety_root != prepared.safety_root
            || current_target.operation != prepared.operation
        {
            return Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity));
        }
        let parent = prepared
            .path
            .parent()
            .ok_or(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath))?;
        let file_name = prepared
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::WriteFailed))?
            .as_nanos();
        let temporary = parent.join(format!(".{file_name}.antiburn-{nonce}.tmp"));
        let pre_replace = (|| {
            let mut output = create_temporary(&temporary, &prepared.permissions)
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            #[cfg(unix)]
            if file_ownership(
                &output
                    .metadata()
                    .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::WriteFailed))?,
            ) != prepared.ownership
            {
                return Err(ApplyError::Unavailable(
                    ConfigUnavailableReason::UnsupportedOwner,
                ));
            }
            output
                .write_all(&prepared.proposed_bytes)
                .and_then(|()| output.sync_all())
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            drop(output);
            let replacement_identity = file_identity(
                &fs::symlink_metadata(&temporary)
                    .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::WriteFailed))?,
            );
            let current = read_checked(&prepared.path, &prepared.safety_root)
                .map_err(ApplyError::Unavailable)?;
            if current.identity != prepared.identity {
                return Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity));
            }
            if current.bytes != prepared.original_bytes {
                return Err(ApplyError::Conflict(ApplyConflict::ChangedContent));
            }
            fs::rename(&temporary, &prepared.path)
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            Ok(replacement_identity)
        })();
        if pre_replace.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        let replacement_identity = pre_replace?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::DirectorySync))?;
        let readback = read_checked(&prepared.path, &prepared.safety_root)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?;
        if readback.identity != replacement_identity {
            return Err(ApplyError::Readback(ApplyReadbackError::ChangedIdentity));
        }
        if readback.bytes != prepared.proposed_bytes {
            return Err(ApplyError::Readback(ApplyReadbackError::ChangedContent));
        }
        let actual = vendor
            .read_value(&readback.bytes, &current_target.operation)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::InvalidContent))?;
        let expected = vendor
            .read_value(&prepared.proposed_bytes, &current_target.operation)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::InvalidContent))?;
        if actual != expected {
            return Err(ApplyError::Readback(ApplyReadbackError::SemanticMismatch));
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn rollback_change(&self, prepared: &PreparedChange) -> Result<(), ApplyError> {
        let rollback = PreparedChange {
            agent: prepared.agent,
            setting: prepared.setting,
            selector: prepared.selector,
            operation: prepared.operation.clone(),
            expected_value: prepared.expected_value.clone(),
            path: prepared.path.clone(),
            scope: prepared.scope,
            resolution_home_root: prepared.resolution_home_root.clone(),
            workspace_cwd: prepared.workspace_cwd.clone(),
            trusted_workspace_root: prepared.trusted_workspace_root.clone(),
            safety_root: prepared.safety_root.clone(),
            original_bytes: prepared.proposed_bytes.clone(),
            proposed_bytes: prepared.original_bytes.clone(),
            identity: file_identity(
                &fs::symlink_metadata(&prepared.path)
                    .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?,
            ),
            permissions: prepared.permissions.clone(),
            #[cfg(unix)]
            ownership: prepared.ownership,
        };
        self.apply_change_without_resolution(&rollback)
    }

    #[cfg(not(windows))]
    fn apply_change_without_resolution(&self, prepared: &PreparedChange) -> Result<(), ApplyError> {
        let parent = prepared
            .path
            .parent()
            .ok_or(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath))?;
        let file_name = prepared
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::WriteFailed))?
            .as_nanos();
        let temporary = parent.join(format!(".{file_name}.antiburn-rollback-{nonce}.tmp"));
        let result = (|| {
            let mut output = create_temporary(&temporary, &prepared.permissions)
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            output
                .write_all(&prepared.proposed_bytes)
                .and_then(|()| output.sync_all())
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            drop(output);
            let current = read_checked(&prepared.path, &prepared.safety_root)
                .map_err(ApplyError::Unavailable)?;
            if current.identity != prepared.identity {
                return Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity));
            }
            if current.bytes != prepared.original_bytes {
                return Err(ApplyError::Conflict(ApplyConflict::ChangedContent));
            }
            fs::rename(&temporary, &prepared.path)
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn validate_read_context(
    context: &ConfigContext,
    setting: ConfigSetting,
    platform: &str,
) -> Result<&'static dyn super::vendors::VendorConfig, ConfigUnavailableReason> {
    if !context.native_environment || !editor_access_supported(platform, false) {
        return Err(ConfigUnavailableReason::UnsupportedEnvironment);
    }
    if context.managed_configuration_present {
        return Err(ConfigUnavailableReason::ManagedConfiguration);
    }
    if context.runtime_override_present {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    }
    let vendor = vendor_for(context.agent);
    match vendor.policy(setting) {
        VendorPolicy::AutomaticEdit => {}
        VendorPolicy::Unsupported(reason) => return Err(reason),
    }
    Ok(vendor)
}

fn validate_write_context(
    context: &ConfigContext,
    setting: ConfigSetting,
    platform: &str,
) -> Result<&'static dyn super::vendors::VendorConfig, ConfigUnavailableReason> {
    if !context.native_environment || !editor_access_supported(platform, false) {
        return Err(ConfigUnavailableReason::UnsupportedEnvironment);
    }
    let vendor = vendor_for(context.agent);
    match vendor.policy(setting) {
        VendorPolicy::AutomaticEdit => {}
        VendorPolicy::Unsupported(reason) => return Err(reason),
    }
    if !editor_access_supported(platform, true) {
        return Err(ConfigUnavailableReason::AutomaticApplyUnsupported);
    }
    Ok(vendor)
}

fn canonical_workspace(
    context: &ConfigContext,
) -> Result<(Option<std::path::PathBuf>, Option<std::path::PathBuf>), ConfigUnavailableReason> {
    let cwd = context
        .workspace_cwd
        .as_deref()
        .map(canonical_root)
        .transpose()?;
    let root = context
        .trusted_workspace_root
        .as_deref()
        .map(canonical_root)
        .transpose()?;
    match (&cwd, &root) {
        (None, None) => Ok((None, None)),
        (Some(cwd), Some(root)) if cwd.starts_with(root) => {
            Ok((Some(cwd.clone()), Some(root.clone())))
        }
        _ => Err(ConfigUnavailableReason::UnsafePath),
    }
}

const fn editor_access_supported(platform: &str, write: bool) -> bool {
    match platform.as_bytes() {
        b"macos" | b"linux" => true,
        b"windows" => !write,
        _ => false,
    }
}

const fn current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    return "macos";
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return "unsupported";
}

#[cfg(not(windows))]
fn validate_value(setting: ConfigSetting, value: &str) -> Result<(), ConfigUnavailableReason> {
    let max = match setting {
        ConfigSetting::Model | ConfigSetting::SubagentModel => 256,
        ConfigSetting::Reasoning => 64,
        ConfigSetting::Compaction => {
            if value == "true" || value == "false" || value.parse::<u64>().is_ok() {
                return Ok(());
            }
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
        ConfigSetting::FastMode => {
            if matches!(value, "true" | "false" | "fast" | "standard" | "remove") {
                return Ok(());
            }
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
        ConfigSetting::McpServer => {
            let Some((name, enabled)) = value.split_once('=') else {
                return Err(ConfigUnavailableReason::InvalidTarget);
            };
            if matches!(enabled, "true" | "false")
                && !name.is_empty()
                && name.len() <= 256
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                return Ok(());
            }
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
        ConfigSetting::BuiltInTool | ConfigSetting::Skill => {
            let Some((name, enabled)) = value.split_once('=') else {
                return Err(ConfigUnavailableReason::InvalidTarget);
            };
            if matches!(enabled, "true" | "false")
                && !name.is_empty()
                && name.len() <= 256
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Ok(());
            }
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
    };
    if value.is_empty()
        || value.len() > max
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        Err(ConfigUnavailableReason::InvalidTarget)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn editor_access_policy_is_cfg_independent() {
        for (platform, read, write) in [
            ("macos", true, true),
            ("linux", true, true),
            ("windows", true, false),
            ("unsupported", false, false),
        ] {
            assert_eq!(editor_access_supported(platform, false), read, "{platform}");
            assert_eq!(editor_access_supported(platform, true), write, "{platform}");
        }
    }
}
