use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use antiburn_local::model::AgentKind;

use super::filesystem::{FileIdentity, FileOwnership};
use super::vendors::OperationSelector;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Global,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSetting {
    Model,
    Reasoning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigContext {
    pub agent: AgentKind,
    pub home_root: PathBuf,
    pub workspace_cwd: Option<PathBuf>,
    pub trusted_workspace_root: Option<PathBuf>,
    pub native_environment: bool,
    pub runtime_override_present: bool,
    pub managed_configuration_present: bool,
}

impl ConfigContext {
    pub fn native(
        agent: AgentKind,
        home_root: impl Into<PathBuf>,
        workspace_root: Option<PathBuf>,
    ) -> Self {
        Self {
            agent,
            home_root: home_root.into(),
            workspace_cwd: workspace_root.clone(),
            trusted_workspace_root: workspace_root,
            native_environment: true,
            runtime_override_present: false,
            managed_configuration_present: false,
        }
    }

    pub fn native_workspace(
        agent: AgentKind,
        home_root: impl Into<PathBuf>,
        workspace_cwd: impl Into<PathBuf>,
        trusted_workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            agent,
            home_root: home_root.into(),
            workspace_cwd: Some(workspace_cwd.into()),
            trusted_workspace_root: Some(trusted_workspace_root.into()),
            native_environment: true,
            runtime_override_present: false,
            managed_configuration_present: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChange {
    pub expected_value: String,
    pub proposed_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOperation {
    pub setting: ConfigSetting,
    pub expected_value: String,
    pub proposed_value: String,
}

impl ConfigOperation {
    pub fn model(change: &ConfigChange) -> Self {
        Self {
            setting: ConfigSetting::Model,
            expected_value: change.expected_value.clone(),
            proposed_value: change.proposed_value.clone(),
        }
    }
}

pub struct PreparedChange {
    pub(super) agent: AgentKind,
    pub(super) setting: ConfigSetting,
    pub(super) selector: &'static str,
    pub(super) operation: OperationSelector,
    pub(super) path: PathBuf,
    pub(super) scope: ConfigScope,
    #[cfg(not(windows))]
    pub(super) resolution_home_root: PathBuf,
    #[cfg(not(windows))]
    pub(super) workspace_cwd: Option<PathBuf>,
    #[cfg(not(windows))]
    pub(super) trusted_workspace_root: Option<PathBuf>,
    #[cfg(not(windows))]
    pub(super) safety_root: PathBuf,
    #[cfg(not(windows))]
    pub(super) original_bytes: Vec<u8>,
    #[cfg(not(windows))]
    pub(super) proposed_bytes: Vec<u8>,
    #[cfg(not(windows))]
    pub(super) identity: FileIdentity,
    #[cfg(not(windows))]
    pub(super) permissions: fs::Permissions,
    #[cfg(unix)]
    pub(super) ownership: FileOwnership,
}

impl fmt::Debug for PreparedChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedChange")
            .field("setting", &self.setting)
            .field("selector", &self.selector)
            .field("scope", &self.scope)
            .field("private_file_data", &"<redacted>")
            .finish()
    }
}

impl PreparedChange {
    pub const fn scope(&self) -> ConfigScope {
        self.scope
    }

    pub const fn setting(&self) -> ConfigSetting {
        self.setting
    }

    pub(crate) fn physical_identity(&self) -> (&Path, &'static str) {
        (&self.path, self.selector)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        #[cfg(not(windows))]
        {
            self.original_bytes
                .len()
                .saturating_add(self.proposed_bytes.len())
        }
        #[cfg(windows)]
        {
            0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub setting: ConfigSetting,
    pub scope: ConfigScope,
    pub value: String,
    pub(super) selector: &'static str,
    pub(super) path: PathBuf,
}

impl EffectiveConfig {
    pub(crate) fn physical_identity(&self) -> (&Path, &'static str) {
        (&self.path, self.selector)
    }
}

pub type EffectiveModel = EffectiveConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyConflict {
    ChangedContent,
    ChangedIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyReadbackError {
    DirectorySync,
    ReadFailed,
    ChangedContent,
    ChangedIdentity,
    InvalidContent,
    SemanticMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    Conflict(ApplyConflict),
    Readback(ApplyReadbackError),
    Unavailable(ConfigUnavailableReason),
}

impl ApplyError {
    pub const fn replacement_may_have_occurred(self) -> bool {
        matches!(self, Self::Readback(_))
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(reason) => write!(formatter, "conflict:{reason:?}"),
            Self::Readback(reason) => write!(formatter, "readback:{reason:?}"),
            Self::Unavailable(reason) => write!(formatter, "unavailable:{reason}"),
        }
    }
}

impl std::error::Error for ApplyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigUnavailableReason {
    UnsupportedAgent,
    UnsupportedSetting,
    UnsupportedEnvironment,
    ManagedConfiguration,
    RuntimeOverride,
    MissingConfig,
    MissingTarget,
    CurrentValueMismatch,
    InvalidTarget,
    InvalidPrecedence,
    SplitModelRoute,
    DynamicValue,
    UnsafePath,
    SymlinkTarget,
    NonRegularFile,
    FileTooLarge,
    UnsupportedOwner,
    MalformedConfig,
    DuplicateDefinition,
    ChangedIdentity,
    PermissionDenied,
    WriteFailed,
    AutomaticApplyUnsupported,
}

impl fmt::Display for ConfigUnavailableReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", format!("{self:?}").to_ascii_lowercase())
    }
}

impl std::error::Error for ConfigUnavailableReason {}
