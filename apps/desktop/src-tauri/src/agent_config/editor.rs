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
    EffectiveConfig, PreparedChange,
};
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
        let vendor = validate_read_context(context, setting, current_platform())?;
        let home = canonical_root(&context.home_root)?;
        let (workspace_cwd, trusted_workspace_root) = canonical_workspace(context)?;
        let target = vendor.resolve_target(
            setting,
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

    pub fn prepare_operation(
        &self,
        context: &ConfigContext,
        operation: &ConfigOperation,
    ) -> Result<PreparedChange, ConfigUnavailableReason> {
        let vendor = validate_write_context(context, operation.setting, current_platform())?;
        validate_value(operation.setting, &operation.expected_value)?;
        validate_value(operation.setting, &operation.proposed_value)?;
        if operation.expected_value == operation.proposed_value {
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
        let home = canonical_root(&context.home_root)?;
        let (workspace_cwd, trusted_workspace_root) = canonical_workspace(context)?;
        let target = vendor.resolve_target(
            operation.setting,
            &home,
            workspace_cwd.as_deref(),
            trusted_workspace_root.as_deref(),
        )?;
        let file = read_checked(&target.path, &target.safety_root)?;
        let current = vendor
            .read_value(&file.bytes, &target.operation)?
            .ok_or(ConfigUnavailableReason::MissingTarget)?;
        if current != operation.expected_value {
            return Err(ConfigUnavailableReason::CurrentValueMismatch);
        }
        #[cfg(not(windows))]
        let proposed_bytes =
            vendor.edit_value(&file.bytes, &target.operation, &operation.proposed_value)?;
        Ok(PreparedChange {
            agent: context.agent,
            setting: operation.setting,
            selector: target.operation.physical_selector(),
            operation: target.operation,
            path: target.path,
            scope: target.scope,
            #[cfg(not(windows))]
            resolution_home_root: home,
            #[cfg(not(windows))]
            workspace_cwd,
            #[cfg(not(windows))]
            trusted_workspace_root,
            #[cfg(not(windows))]
            safety_root: target.safety_root,
            #[cfg(not(windows))]
            original_bytes: file.bytes,
            #[cfg(not(windows))]
            proposed_bytes,
            #[cfg(not(windows))]
            identity: file.identity,
            #[cfg(not(windows))]
            permissions: file.permissions,
            #[cfg(unix)]
            ownership: file.ownership,
        })
    }

    pub fn prepare(
        &self,
        context: &ConfigContext,
        change: &ConfigChange,
    ) -> Result<PreparedChange, ConfigUnavailableReason> {
        self.prepare_operation(context, &ConfigOperation::model(change))
    }

    #[cfg(not(windows))]
    pub fn apply(&self, prepared: &PreparedChange) -> Result<(), ApplyError> {
        let vendor = vendor_for(prepared.agent);
        let current_target = vendor
            .resolve_target(
                prepared.setting,
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
    let vendor = validate_read_context(context, setting, platform)?;
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

fn validate_value(setting: ConfigSetting, value: &str) -> Result<(), ConfigUnavailableReason> {
    let max = match setting {
        ConfigSetting::Model => 256,
        ConfigSetting::Reasoning => 64,
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
