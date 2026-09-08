//! Safe edits to existing native coding-agent configuration files.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
#[cfg(not(windows))]
use std::fs::OpenOptions;
#[cfg(not(windows))]
use std::io::Write;
use std::path::{Component, Path, PathBuf};
#[cfg(windows)]
use std::time::SystemTime;
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

use antiburn_local::model::AgentKind;
use antiburn_local::remediation::ChangeOperation;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item, value};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// The activation point after a successful configuration write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationBoundary {
    NextRequest,
    NextSession,
    AgentRestart,
}

/// A native environment and its trusted filesystem roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigContext {
    pub agent: AgentKind,
    pub home_root: PathBuf,
    pub workspace_root: Option<PathBuf>,
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
            workspace_root,
            native_environment: true,
            runtime_override_present: false,
            managed_configuration_present: false,
        }
    }
}

/// One exact supported configuration intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigChange {
    SetModel {
        proposed_value: String,
    },
    SetReasoning {
        model: Option<String>,
        proposed_value: String,
    },
    SetWorkerModel {
        worker: String,
        proposed_value: String,
    },
    SetWorkerServiceTier {
        worker: String,
        proposed_value: String,
    },
    DisableMcpServer {
        server: String,
    },
}

impl ConfigChange {
    pub const fn operation(&self) -> ChangeOperation {
        match self {
            Self::SetModel { .. } => ChangeOperation::SetModel,
            Self::SetReasoning { .. } => ChangeOperation::SetReasoning,
            Self::SetWorkerModel { .. } => ChangeOperation::SetWorkerModel,
            Self::SetWorkerServiceTier { .. } => ChangeOperation::SetWorkerServiceTier,
            Self::DisableMcpServer { .. } => ChangeOperation::DisableMcpServer,
        }
    }
}

/// Public, sanitized facts about the selected configuration value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigInspection {
    summary: &'static str,
    current_value: Option<String>,
    activation_boundary: ActivationBoundary,
}

impl ConfigInspection {
    pub const fn summary(&self) -> &'static str {
        self.summary
    }

    pub fn current_value(&self) -> Option<&str> {
        self.current_value.as_deref()
    }

    pub const fn activation_boundary(&self) -> ActivationBoundary {
        self.activation_boundary
    }
}

/// A checked edit that retains private file data only inside Rust.
pub struct PreparedChange {
    summary: &'static str,
    current_value: Option<String>,
    proposed_value: String,
    activation_boundary: ActivationBoundary,
    path: PathBuf,
    trusted_root: PathBuf,
    original_bytes: Vec<u8>,
    proposed_bytes: Vec<u8>,
    identity: FileIdentity,
    permissions: fs::Permissions,
    semantic_target: SemanticTarget,
}

impl fmt::Debug for PreparedChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedChange")
            .field("summary", &self.summary)
            .field("activation_boundary", &self.activation_boundary)
            .field("private_file_data", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl PreparedChange {
    pub const fn summary(&self) -> &'static str {
        self.summary
    }

    pub fn current_value(&self) -> Option<&str> {
        self.current_value.as_deref()
    }

    pub fn proposed_value(&self) -> &str {
        &self.proposed_value
    }

    pub const fn activation_boundary(&self) -> ActivationBoundary {
        self.activation_boundary
    }

    pub fn inspection(&self) -> ConfigInspection {
        ConfigInspection {
            summary: self.summary,
            current_value: self.current_value.clone(),
            activation_boundary: self.activation_boundary,
        }
    }
}

/// The result of checking a requested configuration change.
#[derive(Debug)]
pub enum PrepareOutcome {
    NoOp(ConfigInspection),
    Ready(Box<PreparedChange>),
}

impl PrepareOutcome {
    pub fn inspection(&self) -> ConfigInspection {
        match self {
            Self::NoOp(inspection) => inspection.clone(),
            Self::Ready(prepared) => prepared.inspection(),
        }
    }

    pub const fn prepared(&self) -> Option<&PreparedChange> {
        match self {
            Self::NoOp(_) => None,
            Self::Ready(prepared) => Some(prepared),
        }
    }
}

/// The result of an approved configuration write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
}

/// A stale prepared change that must be prepared and approved again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyConflict {
    ChangedContent,
    ChangedIdentity,
}

/// A failed verification after the replacement completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyReadbackError {
    ReadFailed(ConfigUnavailableReason),
    InvalidContent(ConfigUnavailableReason),
    ChangedContent,
    ChangedIdentity,
    SemanticMismatch,
}

/// A categorized apply failure for an integration boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    Conflict(ApplyConflict),
    Readback(ApplyReadbackError),
    Unavailable(ConfigUnavailableReason),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(reason) => write!(formatter, "conflict:{reason}"),
            Self::Readback(reason) => write!(formatter, "readback:{reason}"),
            Self::Unavailable(reason) => write!(formatter, "unavailable:{reason}"),
        }
    }
}

impl std::error::Error for ApplyError {}

impl fmt::Display for ApplyConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ChangedContent => "changed_content",
            Self::ChangedIdentity => "changed_identity",
        })
    }
}

impl fmt::Display for ApplyReadbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReadFailed(reason) => return write!(formatter, "read_failed:{reason}"),
            Self::InvalidContent(reason) => return write!(formatter, "invalid_content:{reason}"),
            Self::ChangedContent => "changed_content",
            Self::ChangedIdentity => "changed_identity",
            Self::SemanticMismatch => "semantic_mismatch",
        })
    }
}

/// A public reason that contains no path or configuration content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigUnavailableReason {
    UnsupportedAgent,
    UnsupportedEnvironment,
    UnsupportedOperation,
    UnsupportedFormat,
    ManagedConfiguration,
    RuntimeOverride,
    MissingConfig,
    MissingTarget,
    AmbiguousTarget,
    InvalidTarget,
    UnsafePath,
    SymlinkTarget,
    NonRegularFile,
    FileTooLarge,
    UnsupportedOwner,
    UnprovenPrecedence,
    AutomaticApplyUnsupported,
    MalformedConfig,
    DuplicateDefinition,
    ChangedIdentity,
    PermissionDenied,
    WriteFailed,
}

impl std::fmt::Display for ConfigUnavailableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedAgent => "unsupported_agent",
            Self::UnsupportedEnvironment => "unsupported_environment",
            Self::UnsupportedOperation => "unsupported_operation",
            Self::UnsupportedFormat => "unsupported_format",
            Self::ManagedConfiguration => "managed_configuration",
            Self::RuntimeOverride => "runtime_override",
            Self::MissingConfig => "missing_config",
            Self::MissingTarget => "missing_target",
            Self::AmbiguousTarget => "ambiguous_target",
            Self::InvalidTarget => "invalid_target",
            Self::UnsafePath => "unsafe_path",
            Self::SymlinkTarget => "symlink_target",
            Self::NonRegularFile => "non_regular_file",
            Self::FileTooLarge => "file_too_large",
            Self::UnsupportedOwner => "unsupported_owner",
            Self::UnprovenPrecedence => "unproven_precedence",
            Self::AutomaticApplyUnsupported => "automatic_apply_unsupported",
            Self::MalformedConfig => "malformed_config",
            Self::DuplicateDefinition => "duplicate_definition",
            Self::ChangedIdentity => "changed_identity",
            Self::PermissionDenied => "permission_denied",
            Self::WriteFailed => "write_failed",
        })
    }
}

impl std::error::Error for ConfigUnavailableReason {}

/// Dispatches supported changes to one of four native configuration editors.
#[derive(Debug, Default)]
pub struct AgentConfigEditor;

impl AgentConfigEditor {
    pub const fn new() -> Self {
        Self
    }

    pub fn inspect(
        &self,
        context: &ConfigContext,
        change: &ConfigChange,
    ) -> Result<ConfigInspection, ConfigUnavailableReason> {
        Ok(self.prepare(context, change)?.inspection())
    }

    pub fn prepare(
        &self,
        context: &ConfigContext,
        change: &ConfigChange,
    ) -> Result<PrepareOutcome, ConfigUnavailableReason> {
        if !context.native_environment {
            return Err(ConfigUnavailableReason::UnsupportedEnvironment);
        }
        if context.managed_configuration_present {
            return Err(ConfigUnavailableReason::ManagedConfiguration);
        }
        if context.runtime_override_present {
            return Err(ConfigUnavailableReason::RuntimeOverride);
        }
        let home_root = canonical_root(&context.home_root)?;
        let workspace_root = context
            .workspace_root
            .as_deref()
            .map(canonical_root)
            .transpose()?;
        let (path, trusted_root, format, semantic_target, summary, boundary) = match context.agent {
            AgentKind::Claude => claude_target(&home_root, workspace_root.as_deref(), change)?,
            AgentKind::Codex => codex_target(&home_root, workspace_root.as_deref(), change)?,
            AgentKind::OpenCode => opencode_target(&home_root, workspace_root.as_deref(), change)?,
            AgentKind::Pi => pi_target(&home_root, workspace_root.as_deref(), change)?,
            _ => return Err(ConfigUnavailableReason::UnsupportedAgent),
        };
        let file = read_checked(&path, &trusted_root)?;
        validate_change(context.agent, change)?;
        let (proposed_bytes, current_value, proposed_value) = match format {
            ConfigFormat::Json => edit_json(&file.bytes, &semantic_target)?,
            ConfigFormat::Toml => edit_toml(&file.bytes, &semantic_target)?,
            ConfigFormat::Markdown => edit_markdown(&file.bytes, &semantic_target)?,
        };
        validate_public_value(&proposed_value, ConfigUnavailableReason::InvalidTarget)?;
        if let Some(current_value) = &current_value {
            validate_public_value(current_value, ConfigUnavailableReason::MalformedConfig)?;
        }
        let inspection = ConfigInspection {
            summary,
            current_value: current_value.clone(),
            activation_boundary: boundary,
        };
        if current_value.as_deref() == Some(proposed_value.as_str()) {
            return Ok(PrepareOutcome::NoOp(inspection));
        }
        Ok(PrepareOutcome::Ready(Box::new(PreparedChange {
            summary,
            current_value,
            proposed_value,
            activation_boundary: boundary,
            path,
            trusted_root,
            original_bytes: file.bytes,
            proposed_bytes,
            identity: file.identity,
            permissions: file.permissions,
            semantic_target,
        })))
    }

    pub fn apply(&self, prepared: &PreparedChange) -> Result<ApplyOutcome, ApplyError> {
        self.apply_with_readback(prepared, |_| {})
    }

    #[cfg(not(windows))]
    fn apply_with_readback(
        &self,
        prepared: &PreparedChange,
        after_replace: impl FnOnce(&Path),
    ) -> Result<ApplyOutcome, ApplyError> {
        let parent = prepared
            .path
            .parent()
            .ok_or(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath))?;
        let file_name = prepared
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(ApplyError::Unavailable(ConfigUnavailableReason::UnsafePath))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::WriteFailed))?
            .as_nanos();
        let temporary = parent.join(format!(".{file_name}.antiburn-{nonce}.tmp"));
        let write_result: Result<FileIdentity, ApplyError> = (|| {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            output
                .set_permissions(prepared.permissions.clone())
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            output
                .write_all(&prepared.proposed_bytes)
                .and_then(|()| output.sync_all())
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            drop(output);
            let replacement_metadata = fs::symlink_metadata(&temporary)
                .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::WriteFailed))?;
            let replacement_identity = file_identity(&replacement_metadata);
            let current = read_checked(&prepared.path, &prepared.trusted_root)
                .map_err(ApplyError::Unavailable)?;
            if current.identity != prepared.identity {
                return Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity));
            }
            if current.bytes != prepared.original_bytes {
                return Err(ApplyError::Conflict(ApplyConflict::ChangedContent));
            }
            atomic_replace(&temporary, &prepared.path).map_err(ApplyError::Unavailable)?;
            Ok(replacement_identity)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        let replacement_identity = write_result?;
        after_replace(&prepared.path);

        let readback = read_checked(&prepared.path, &prepared.trusted_root)
            .map_err(|reason| ApplyError::Readback(ApplyReadbackError::ReadFailed(reason)))?;
        if readback.identity != replacement_identity {
            return Err(ApplyError::Readback(ApplyReadbackError::ChangedIdentity));
        }
        if readback.bytes != prepared.proposed_bytes {
            return Err(ApplyError::Readback(ApplyReadbackError::ChangedContent));
        }
        let actual = semantic_value(&readback.bytes, &prepared.semantic_target)
            .map_err(|reason| ApplyError::Readback(ApplyReadbackError::InvalidContent(reason)))?;
        if actual.as_deref() != Some(prepared.proposed_value.as_str()) {
            return Err(ApplyError::Readback(ApplyReadbackError::SemanticMismatch));
        }
        Ok(ApplyOutcome::Applied)
    }

    #[cfg(windows)]
    fn apply_with_readback(
        &self,
        _: &PreparedChange,
        _: impl FnOnce(&Path),
    ) -> Result<ApplyOutcome, ApplyError> {
        Err(ApplyError::Unavailable(
            ConfigUnavailableReason::AutomaticApplyUnsupported,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
enum ConfigFormat {
    Json,
    Toml,
    Markdown,
}

#[derive(Debug, Clone)]
enum SemanticTarget {
    JsonPath {
        keys: Vec<String>,
        proposed: String,
    },
    JsonPair {
        first_key: String,
        first_value: String,
        second_key: String,
        second_value: String,
    },
    JsonStringArrayEntry {
        key: String,
        entry: String,
    },
    TomlPath {
        keys: Vec<String>,
        proposed: toml_edit::Value,
        display: String,
    },
    MarkdownScalar {
        key: &'static str,
        proposed: String,
    },
}

#[derive(Debug)]
struct CheckedFile {
    bytes: Vec<u8>,
    identity: FileIdentity,
    permissions: fs::Permissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: Option<SystemTime>,
}

fn canonical_root(path: &Path) -> Result<PathBuf, ConfigUnavailableReason> {
    path.canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)
}

fn read_checked(path: &Path, trusted_root: &Path) -> Result<CheckedFile, ConfigUnavailableReason> {
    let link_metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ConfigUnavailableReason::MissingConfig
        } else if error.kind() == std::io::ErrorKind::PermissionDenied {
            ConfigUnavailableReason::PermissionDenied
        } else {
            ConfigUnavailableReason::UnsafePath
        }
    })?;
    if link_metadata.file_type().is_symlink() {
        return Err(ConfigUnavailableReason::SymlinkTarget);
    }
    if !link_metadata.is_file() {
        return Err(ConfigUnavailableReason::NonRegularFile);
    }
    if link_metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let canonical_path = path
        .canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if !canonical_path.starts_with(trusted_root) {
        return Err(ConfigUnavailableReason::UnsafePath);
    }
    check_owner(&link_metadata, trusted_root)?;
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            ConfigUnavailableReason::PermissionDenied
        } else {
            ConfigUnavailableReason::UnsafePath
        }
    })?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let final_metadata =
        fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if final_metadata.file_type().is_symlink() {
        return Err(ConfigUnavailableReason::SymlinkTarget);
    }
    if file_identity(&final_metadata) != file_identity(&link_metadata) {
        return Err(ConfigUnavailableReason::ChangedIdentity);
    }
    Ok(CheckedFile {
        bytes,
        identity: file_identity(&link_metadata),
        permissions: link_metadata.permissions(),
    })
}

#[cfg(unix)]
fn check_owner(
    metadata: &fs::Metadata,
    trusted_root: &Path,
) -> Result<(), ConfigUnavailableReason> {
    use std::os::unix::fs::MetadataExt;
    let root_metadata =
        fs::metadata(trusted_root).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if root_metadata.uid() != metadata.uid() {
        return Err(ConfigUnavailableReason::UnsupportedOwner);
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_owner(_: &fs::Metadata, _: &Path) -> Result<(), ConfigUnavailableReason> {
    Ok(())
}

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            created: metadata.created().ok(),
        }
    }
}

#[cfg(not(windows))]
fn map_write_error(error: std::io::Error) -> ConfigUnavailableReason {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        ConfigUnavailableReason::PermissionDenied
    } else {
        ConfigUnavailableReason::WriteFailed
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, target: &Path) -> Result<(), ConfigUnavailableReason> {
    fs::rename(source, target).map_err(map_write_error)
}

fn path_entry_exists(path: &Path) -> Result<bool, ConfigUnavailableReason> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(ConfigUnavailableReason::PermissionDenied)
        }
        Err(_) => Err(ConfigUnavailableReason::UnsafePath),
    }
}

type Target = (
    PathBuf,
    PathBuf,
    ConfigFormat,
    SemanticTarget,
    &'static str,
    ActivationBoundary,
);

fn claude_target(
    home: &Path,
    workspace: Option<&Path>,
    change: &ConfigChange,
) -> Result<Target, ConfigUnavailableReason> {
    match change {
        ConfigChange::SetModel { proposed_value } => {
            let (path, root) = highest_existing(
                home,
                workspace,
                &[
                    ".claude/settings.local.json",
                    ".claude/settings.json",
                    ".claude/settings.json",
                ],
            )?;
            Ok(json_target(
                path,
                root,
                &["model"],
                proposed_value,
                "Change Claude model",
                ActivationBoundary::NextSession,
            ))
        }
        ConfigChange::SetReasoning {
            model,
            proposed_value,
        } => {
            let model = valid_name(
                model
                    .as_deref()
                    .ok_or(ConfigUnavailableReason::MissingTarget)?,
            )?;
            let (path, root) = highest_existing(
                home,
                workspace,
                &[
                    ".claude/settings.local.json",
                    ".claude/settings.json",
                    ".claude/settings.json",
                ],
            )?;
            Ok(json_target(
                path,
                root,
                &["modelSettings", model, "effortLevel"],
                proposed_value,
                "Change Claude model effort",
                ActivationBoundary::NextSession,
            ))
        }
        ConfigChange::SetWorkerModel {
            worker,
            proposed_value,
        } => claude_worker_target(home, workspace, worker, "model", proposed_value),
        ConfigChange::SetWorkerServiceTier { .. } => {
            Err(ConfigUnavailableReason::UnsupportedOperation)
        }
        ConfigChange::DisableMcpServer { server } => {
            let workspace = workspace.ok_or(ConfigUnavailableReason::MissingTarget)?;
            let server = valid_name(server)?;
            verify_claude_mcp(workspace, server)?;
            let path = workspace.join(".claude/settings.json");
            Ok((
                path,
                workspace.to_owned(),
                ConfigFormat::Json,
                SemanticTarget::JsonStringArrayEntry {
                    key: "disabledMcpjsonServers".into(),
                    entry: server.into(),
                },
                "Disable Claude project MCP server",
                ActivationBoundary::AgentRestart,
            ))
        }
    }
}

fn highest_existing(
    home: &Path,
    workspace: Option<&Path>,
    paths: &[&str; 3],
) -> Result<(PathBuf, PathBuf), ConfigUnavailableReason> {
    if let Some(workspace) = workspace {
        for relative in &paths[..2] {
            let path = workspace.join(relative);
            if path_entry_exists(&path)? {
                return Ok((path, workspace.to_owned()));
            }
        }
    }
    let path = home.join(paths[2]);
    if path_entry_exists(&path)? {
        Ok((path, home.to_owned()))
    } else {
        Err(ConfigUnavailableReason::MissingConfig)
    }
}

fn claude_worker_target(
    home: &Path,
    workspace: Option<&Path>,
    worker: &str,
    key: &'static str,
    proposed: &str,
) -> Result<Target, ConfigUnavailableReason> {
    let worker = valid_name(worker)?;
    let mut candidates = vec![(
        home.join(".claude/agents").join(format!("{worker}.md")),
        home,
    )];
    if let Some(workspace) = workspace {
        candidates.insert(
            0,
            (
                workspace
                    .join(".claude/agents")
                    .join(format!("{worker}.md")),
                workspace,
            ),
        );
    }
    let existing: Vec<_> = candidates
        .into_iter()
        .map(|candidate| path_entry_exists(&candidate.0).map(|exists| (candidate, exists)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(candidate, exists)| exists.then_some(candidate))
        .collect();
    if existing.len() > 1 {
        return Err(ConfigUnavailableReason::AmbiguousTarget);
    }
    let (path, root) = existing
        .into_iter()
        .next()
        .ok_or(ConfigUnavailableReason::MissingTarget)?;
    Ok((
        path,
        root.to_owned(),
        ConfigFormat::Markdown,
        SemanticTarget::MarkdownScalar {
            key,
            proposed: proposed.into(),
        },
        if key == "model" {
            "Change Claude subagent model"
        } else {
            "Change Claude subagent effort"
        },
        ActivationBoundary::NextRequest,
    ))
}

fn verify_claude_mcp(workspace: &Path, server: &str) -> Result<(), ConfigUnavailableReason> {
    let file = read_checked(&workspace.join(".mcp.json"), workspace)?;
    let value = parse_json(&file.bytes)?;
    if value
        .pointer(&format!("/mcpServers/{}", json_pointer_token(server)))
        .is_none()
    {
        return Err(ConfigUnavailableReason::MissingTarget);
    }
    Ok(())
}

fn codex_target(
    home: &Path,
    workspace: Option<&Path>,
    change: &ConfigChange,
) -> Result<Target, ConfigUnavailableReason> {
    let config = if let Some(workspace) = workspace {
        let path = workspace.join(".codex/config.toml");
        if path_entry_exists(&path)? {
            (path, workspace)
        } else {
            (home.join(".codex/config.toml"), home)
        }
    } else {
        (home.join(".codex/config.toml"), home)
    };
    match change {
        ConfigChange::SetModel { proposed_value } => Ok(toml_target(
            config.0,
            config.1.to_owned(),
            &["model"],
            proposed_value,
            "Change Codex model",
            ActivationBoundary::NextSession,
        )),
        ConfigChange::SetReasoning {
            model: _,
            proposed_value,
        } => Ok(toml_target(
            config.0,
            config.1.to_owned(),
            &["model_reasoning_effort"],
            proposed_value,
            "Change Codex reasoning effort",
            ActivationBoundary::NextSession,
        )),
        ConfigChange::DisableMcpServer { server } => {
            let server = valid_name(server)?;
            require_toml_table(&config.0, config.1, &["mcp_servers", server])?;
            Ok((
                config.0,
                config.1.to_owned(),
                ConfigFormat::Toml,
                SemanticTarget::TomlPath {
                    keys: vec!["mcp_servers".into(), server.into(), "enabled".into()],
                    proposed: toml_edit::Value::from(false),
                    display: "false".into(),
                },
                "Disable Codex MCP server",
                ActivationBoundary::NextSession,
            ))
        }
        ConfigChange::SetWorkerModel {
            worker,
            proposed_value,
        } => codex_worker_target(&config.0, config.1, worker, "model", proposed_value),
        ConfigChange::SetWorkerServiceTier {
            worker,
            proposed_value,
        } => codex_worker_target(&config.0, config.1, worker, "service_tier", proposed_value),
    }
}

fn codex_worker_target(
    config_path: &Path,
    root: &Path,
    worker: &str,
    key: &str,
    proposed: &str,
) -> Result<Target, ConfigUnavailableReason> {
    let worker = valid_name(worker)?;
    let file = read_checked(config_path, root)?;
    let text =
        std::str::from_utf8(&file.bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let document = parse_toml(text)?;
    let relative = document
        .get("agents")
        .and_then(|item| item.get(worker))
        .and_then(|item| item.get("config_file"))
        .and_then(Item::as_str)
        .ok_or(ConfigUnavailableReason::MissingTarget)?;
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ConfigUnavailableReason::UnsafePath);
    }
    let path = config_path
        .parent()
        .ok_or(ConfigUnavailableReason::UnsafePath)?
        .join(relative);
    Ok(toml_target(
        path,
        root.to_owned(),
        &[key],
        proposed,
        if key == "model" {
            "Change Codex custom agent model"
        } else {
            "Change Codex custom agent service tier"
        },
        ActivationBoundary::NextSession,
    ))
}

fn require_toml_table(
    path: &Path,
    root: &Path,
    keys: &[&str],
) -> Result<(), ConfigUnavailableReason> {
    let file = read_checked(path, root)?;
    let text =
        std::str::from_utf8(&file.bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let document = parse_toml(text)?;
    let mut item = document.as_item();
    for key in keys {
        item = item
            .get(key)
            .ok_or(ConfigUnavailableReason::MissingTarget)?;
    }
    if !item.is_table_like() {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    Ok(())
}

fn opencode_target(
    home: &Path,
    workspace: Option<&Path>,
    change: &ConfigChange,
) -> Result<Target, ConfigUnavailableReason> {
    if let Some(workspace) = workspace {
        let extension_config = workspace.join(".opencode/opencode.json");
        if path_entry_exists(&extension_config.with_extension("jsonc"))? {
            return Err(ConfigUnavailableReason::UnsupportedFormat);
        }
        if path_entry_exists(&extension_config)? {
            return Err(ConfigUnavailableReason::UnprovenPrecedence);
        }
    }
    let (path, root) = if let Some(workspace) = workspace {
        let path = workspace.join("opencode.json");
        if path_entry_exists(&path.with_extension("jsonc"))? {
            return Err(ConfigUnavailableReason::UnsupportedFormat);
        } else if path_entry_exists(&path)? {
            (path, workspace.to_owned())
        } else {
            (home.join(".config/opencode/opencode.json"), home.to_owned())
        }
    } else {
        (home.join(".config/opencode/opencode.json"), home.to_owned())
    };
    if path_entry_exists(&path.with_extension("jsonc"))? {
        return Err(ConfigUnavailableReason::UnsupportedFormat);
    }
    match change {
        ConfigChange::SetModel { proposed_value } => Ok(json_target(
            path,
            root,
            &["model"],
            proposed_value,
            "Change OpenCode model",
            ActivationBoundary::NextSession,
        )),
        ConfigChange::SetWorkerModel {
            worker,
            proposed_value,
        } => {
            let worker = valid_name(worker)?;
            require_json_object(&path, &root, &["agent", worker])?;
            Ok(json_target(
                path,
                root,
                &["agent", worker, "model"],
                proposed_value,
                "Change OpenCode agent model",
                ActivationBoundary::NextSession,
            ))
        }
        _ => Err(ConfigUnavailableReason::UnsupportedOperation),
    }
}

fn require_json_object(
    path: &Path,
    root: &Path,
    keys: &[&str],
) -> Result<(), ConfigUnavailableReason> {
    let file = read_checked(path, root)?;
    let value = parse_json(&file.bytes)?;
    let mut current = &value;
    for key in keys {
        current = current
            .as_object()
            .and_then(|object| object.get(*key))
            .ok_or(ConfigUnavailableReason::MissingTarget)?;
    }
    if !current.is_object() {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    Ok(())
}

fn pi_target(
    home: &Path,
    workspace: Option<&Path>,
    change: &ConfigChange,
) -> Result<Target, ConfigUnavailableReason> {
    let (path, root) = if let Some(workspace) = workspace {
        let path = workspace.join(".pi/settings.json");
        if path_entry_exists(&path)? {
            (path, workspace.to_owned())
        } else {
            (home.join(".pi/agent/settings.json"), home.to_owned())
        }
    } else {
        (home.join(".pi/agent/settings.json"), home.to_owned())
    };
    match change {
        ConfigChange::SetModel { proposed_value } => {
            let (provider, model) = proposed_value
                .split_once('/')
                .ok_or(ConfigUnavailableReason::InvalidTarget)?;
            valid_name(provider)?;
            valid_name(model)?;
            Ok((
                path,
                root,
                ConfigFormat::Json,
                SemanticTarget::JsonPair {
                    first_key: "defaultProvider".into(),
                    first_value: provider.into(),
                    second_key: "defaultModel".into(),
                    second_value: model.into(),
                },
                "Change Pi default model",
                ActivationBoundary::NextSession,
            ))
        }
        ConfigChange::SetReasoning {
            model,
            proposed_value,
        } => {
            let model = valid_provider_model(
                model
                    .as_deref()
                    .ok_or(ConfigUnavailableReason::MissingTarget)?,
            )?;
            Ok(json_target(
                path,
                root,
                &["modelThinkingLevels", model],
                proposed_value,
                "Change Pi model thinking level",
                ActivationBoundary::NextSession,
            ))
        }
        _ => Err(ConfigUnavailableReason::UnsupportedOperation),
    }
}

fn json_target(
    path: PathBuf,
    root: PathBuf,
    keys: &[&str],
    proposed: &str,
    summary: &'static str,
    boundary: ActivationBoundary,
) -> Target {
    (
        path,
        root,
        ConfigFormat::Json,
        SemanticTarget::JsonPath {
            keys: keys.iter().map(|key| (*key).into()).collect(),
            proposed: proposed.into(),
        },
        summary,
        boundary,
    )
}

fn toml_target(
    path: PathBuf,
    root: PathBuf,
    keys: &[&str],
    proposed: &str,
    summary: &'static str,
    boundary: ActivationBoundary,
) -> Target {
    (
        path,
        root,
        ConfigFormat::Toml,
        SemanticTarget::TomlPath {
            keys: keys.iter().map(|key| (*key).into()).collect(),
            proposed: toml_edit::Value::from(proposed),
            display: proposed.into(),
        },
        summary,
        boundary,
    )
}

fn valid_name(value: &str) -> Result<&str, ConfigUnavailableReason> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
    {
        Err(ConfigUnavailableReason::InvalidTarget)
    } else {
        Ok(value)
    }
}

fn validate_change(agent: AgentKind, change: &ConfigChange) -> Result<(), ConfigUnavailableReason> {
    match change {
        ConfigChange::SetModel { proposed_value }
        | ConfigChange::SetWorkerModel { proposed_value, .. } => match agent {
            AgentKind::OpenCode | AgentKind::Pi => {
                valid_provider_model(proposed_value)?;
            }
            AgentKind::Claude | AgentKind::Codex => {
                valid_model_identifier(proposed_value)?;
            }
            _ => {}
        },
        ConfigChange::SetReasoning {
            model,
            proposed_value,
        } => {
            if let Some(model) = model {
                match agent {
                    AgentKind::Claude => {
                        valid_model_identifier(model)?;
                    }
                    AgentKind::Pi => {
                        valid_provider_model(model)?;
                    }
                    _ => {}
                }
            }
            let allowed: &[&str] = match agent {
                AgentKind::Claude => &["low", "medium", "high", "xhigh"],
                AgentKind::Codex => &["minimal", "low", "medium", "high", "xhigh"],
                AgentKind::Pi => &["off", "minimal", "low", "medium", "high", "xhigh", "max"],
                _ => return Ok(()),
            };
            valid_closed_value(proposed_value, allowed)?;
        }
        ConfigChange::SetWorkerServiceTier { proposed_value, .. } => {
            if agent == AgentKind::Codex {
                valid_closed_value(proposed_value, &["default", "fast", "priority", "flex"])?;
            }
        }
        ConfigChange::DisableMcpServer { .. } => {}
    }
    Ok(())
}

fn valid_closed_value<'a>(
    value: &'a str,
    allowed: &[&str],
) -> Result<&'a str, ConfigUnavailableReason> {
    allowed
        .contains(&value)
        .then_some(value)
        .ok_or(ConfigUnavailableReason::InvalidTarget)
}

fn valid_model_identifier(value: &str) -> Result<&str, ConfigUnavailableReason> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Err(ConfigUnavailableReason::InvalidTarget)
    } else {
        Ok(value)
    }
}

fn valid_provider_model(value: &str) -> Result<&str, ConfigUnavailableReason> {
    let (provider, model) = value
        .split_once('/')
        .ok_or(ConfigUnavailableReason::InvalidTarget)?;
    valid_model_identifier(provider)?;
    valid_model_identifier(model)?;
    Ok(value)
}

fn validate_public_value(
    value: &str,
    reason: ConfigUnavailableReason,
) -> Result<(), ConfigUnavailableReason> {
    if value.len() > 256 || value.chars().any(char::is_control) {
        Err(reason)
    } else {
        Ok(())
    }
}

fn json_pointer_token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn edit_json(
    bytes: &[u8],
    target: &SemanticTarget,
) -> Result<(Vec<u8>, Option<String>, String), ConfigUnavailableReason> {
    let mut root = parse_json(bytes)?;
    let current = json_semantic_value(&root, target)?;
    let proposed = match target {
        SemanticTarget::JsonPath { keys, proposed } => {
            set_json_path(&mut root, keys, Value::String(proposed.clone()))?;
            proposed.clone()
        }
        SemanticTarget::JsonPair {
            first_key,
            first_value,
            second_key,
            second_value,
        } => {
            let object = root
                .as_object_mut()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            object.insert(first_key.clone(), Value::String(first_value.clone()));
            object.insert(second_key.clone(), Value::String(second_value.clone()));
            format!("{first_value}/{second_value}")
        }
        SemanticTarget::JsonStringArrayEntry { key, entry } => {
            let object = root
                .as_object_mut()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            let array = object
                .entry(key)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            if !array.iter().all(Value::is_string) {
                return Err(ConfigUnavailableReason::MalformedConfig);
            }
            if !array.iter().any(|value| value.as_str() == Some(entry)) {
                array.push(Value::String(entry.clone()));
            }
            "disabled".into()
        }
        _ => return Err(ConfigUnavailableReason::UnsupportedFormat),
    };
    let mut output =
        serde_json::to_vec_pretty(&root).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    output.push(b'\n');
    Ok((output, current, proposed))
}

fn parse_json(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueJson::deserialize(&mut deserializer)
        .map_err(|error| {
            let message = format!("{error:?}");
            if message.contains("duplicate") {
                ConfigUnavailableReason::DuplicateDefinition
            } else {
                ConfigUnavailableReason::MalformedConfig
            }
        })?
        .0;
    deserializer
        .end()
        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    Ok(value)
}

struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

struct UniqueJsonVisitor;

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJson;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Bool(value)))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueJson)
            .ok_or_else(|| E::custom("invalid number"))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value.into())))
    }
    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value)))
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        UniqueJson::deserialize(deserializer)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueJson>()? {
            values.push(value.0);
        }
        Ok(UniqueJson(Value::Array(values)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = Map::new();
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom("duplicate object key"));
            }
            values.insert(key, map.next_value::<UniqueJson>()?.0);
        }
        Ok(UniqueJson(Value::Object(values)))
    }
}

fn set_json_path(
    root: &mut Value,
    keys: &[String],
    proposed: Value,
) -> Result<(), ConfigUnavailableReason> {
    let (last, parents) = keys
        .split_last()
        .ok_or(ConfigUnavailableReason::InvalidTarget)?;
    let mut current = root
        .as_object_mut()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    for key in parents {
        let next = current
            .entry(key)
            .or_insert_with(|| Value::Object(Map::new()));
        current = next
            .as_object_mut()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    }
    current.insert(last.clone(), proposed);
    Ok(())
}

fn json_semantic_value(
    root: &Value,
    target: &SemanticTarget,
) -> Result<Option<String>, ConfigUnavailableReason> {
    match target {
        SemanticTarget::JsonPath { keys, .. } => {
            let mut value = root;
            for key in keys {
                let Some(next) = value.as_object().and_then(|object| object.get(key)) else {
                    return Ok(None);
                };
                value = next;
            }
            value
                .as_str()
                .map(|value| Some(value.into()))
                .ok_or(ConfigUnavailableReason::MalformedConfig)
        }
        SemanticTarget::JsonPair {
            first_key,
            first_value: _,
            second_key,
            second_value: _,
        } => {
            let object = root
                .as_object()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            match (object.get(first_key), object.get(second_key)) {
                (None, None) => Ok(None),
                (Some(first), Some(second)) => Ok(Some(format!(
                    "{}/{}",
                    first
                        .as_str()
                        .ok_or(ConfigUnavailableReason::MalformedConfig)?,
                    second
                        .as_str()
                        .ok_or(ConfigUnavailableReason::MalformedConfig)?
                ))),
                _ => Err(ConfigUnavailableReason::MalformedConfig),
            }
        }
        SemanticTarget::JsonStringArrayEntry { key, entry } => {
            let Some(value) = root.as_object().and_then(|object| object.get(key)) else {
                return Ok(None);
            };
            let array = value
                .as_array()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            if !array.iter().all(Value::is_string) {
                return Err(ConfigUnavailableReason::MalformedConfig);
            }
            Ok(array
                .iter()
                .any(|value| value.as_str() == Some(entry))
                .then(|| "disabled".into()))
        }
        _ => Err(ConfigUnavailableReason::UnsupportedFormat),
    }
}

fn edit_toml(
    bytes: &[u8],
    target: &SemanticTarget,
) -> Result<(Vec<u8>, Option<String>, String), ConfigUnavailableReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let mut document = parse_toml(text)?;
    let SemanticTarget::TomlPath {
        keys,
        proposed,
        display,
    } = target
    else {
        return Err(ConfigUnavailableReason::UnsupportedFormat);
    };
    let current = toml_document_value(&document, keys)?;
    let (last, parents) = keys
        .split_last()
        .ok_or(ConfigUnavailableReason::InvalidTarget)?;
    if parents.is_empty() {
        document[last] = value(proposed.clone());
    } else {
        let mut table = document.as_table_mut();
        for key in parents {
            table = table
                .entry(key)
                .or_insert(Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
        }
        table[last] = value(proposed.clone());
    }
    Ok((document.to_string().into_bytes(), current, display.clone()))
}

fn toml_document_value(
    document: &DocumentMut,
    keys: &[String],
) -> Result<Option<String>, ConfigUnavailableReason> {
    let mut item = document.as_item();
    for key in keys {
        let Some(next) = item.get(key) else {
            return Ok(None);
        };
        item = next;
    }
    if let Some(value) = item.as_str() {
        Ok(Some(value.into()))
    } else if let Some(value) = item.as_bool() {
        Ok(Some(value.to_string()))
    } else {
        Err(ConfigUnavailableReason::MalformedConfig)
    }
}

fn parse_toml(text: &str) -> Result<DocumentMut, ConfigUnavailableReason> {
    text.parse::<DocumentMut>().map_err(|error| {
        let message = error.to_string().to_ascii_lowercase();
        if message.contains("duplicate") || message.contains("redefinition") {
            ConfigUnavailableReason::DuplicateDefinition
        } else {
            ConfigUnavailableReason::MalformedConfig
        }
    })
}

fn edit_markdown(
    bytes: &[u8],
    target: &SemanticTarget,
) -> Result<(Vec<u8>, Option<String>, String), ConfigUnavailableReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let SemanticTarget::MarkdownScalar { key, proposed } = target else {
        return Err(ConfigUnavailableReason::UnsupportedFormat);
    };
    if !text.starts_with("---\n") {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    let end = text[4..]
        .find("\n---\n")
        .map(|offset| offset + 4)
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    let frontmatter = &text[4..end];
    let prefix = format!("{key}:");
    let matches: Vec<_> = frontmatter
        .lines()
        .enumerate()
        .filter(|(_, line)| line.starts_with(&prefix))
        .collect();
    if matches.len() > 1 {
        return Err(ConfigUnavailableReason::DuplicateDefinition);
    }
    let current = matches
        .first()
        .map(|(_, line)| line[prefix.len()..].trim().to_owned());
    if current
        .as_deref()
        .is_some_and(|value| value.is_empty() || value.starts_with(['[', '{', '|', '>']))
    {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    let mut lines: Vec<String> = frontmatter.lines().map(str::to_owned).collect();
    if let Some((index, _)) = matches.first() {
        lines[*index] = format!("{key}: {proposed}");
    } else {
        lines.push(format!("{key}: {proposed}"));
    }
    let output = format!("---\n{}\n{}", lines.join("\n"), &text[end..]);
    Ok((output.into_bytes(), current, proposed.clone()))
}

fn semantic_value(
    bytes: &[u8],
    target: &SemanticTarget,
) -> Result<Option<String>, ConfigUnavailableReason> {
    match target {
        SemanticTarget::JsonPath { .. }
        | SemanticTarget::JsonPair { .. }
        | SemanticTarget::JsonStringArrayEntry { .. } => {
            json_semantic_value(&parse_json(bytes)?, target)
        }
        SemanticTarget::TomlPath { keys, .. } => {
            let text =
                std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
            let document = parse_toml(text)?;
            toml_document_value(&document, keys)
        }
        SemanticTarget::MarkdownScalar { .. } => {
            edit_markdown(bytes, target).map(|(_, current, _)| current)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::TempDir;

    fn roots() -> (TempDir, PathBuf, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let workspace = temporary.path().join("workspace");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&workspace).unwrap();
        (temporary, home, workspace)
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn context(agent: AgentKind, home: &Path, workspace: &Path) -> ConfigContext {
        ConfigContext::native(agent, home, Some(workspace.to_owned()))
    }

    #[cfg(not(windows))]
    fn apply(
        agent: AgentKind,
        home: &Path,
        workspace: &Path,
        change: ConfigChange,
    ) -> PreparedChange {
        let editor = AgentConfigEditor::new();
        let prepared = ready(&editor, &context(agent, home, workspace), &change);
        assert_eq!(editor.apply(&prepared).unwrap(), ApplyOutcome::Applied);
        prepared
    }

    fn ready(
        editor: &AgentConfigEditor,
        context: &ConfigContext,
        change: &ConfigChange,
    ) -> PreparedChange {
        match editor.prepare(context, change).unwrap() {
            PrepareOutcome::Ready(prepared) => *prepared,
            PrepareOutcome::NoOp(_) => panic!("the test expected a prepared change"),
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn claude_model_and_per_model_effort_keep_unrelated_json() {
        let (_temporary, home, workspace) = roots();
        let settings = home.join(".claude/settings.json");
        write(&settings, r#"{"model":"old","theme":"dark"}"#);

        apply(
            AgentKind::Claude,
            &home,
            &workspace,
            ConfigChange::SetModel {
                proposed_value: "new".into(),
            },
        );
        let prepared = apply(
            AgentKind::Claude,
            &home,
            &workspace,
            ConfigChange::SetReasoning {
                model: Some("new".into()),
                proposed_value: "low".into(),
            },
        );

        let value = parse_json(&fs::read(settings).unwrap()).unwrap();
        assert_eq!(value["model"], "new");
        assert_eq!(value["modelSettings"]["new"]["effortLevel"], "low");
        assert_eq!(value["theme"], "dark");
        assert_eq!(prepared.current_value(), None);
        assert_eq!(prepared.proposed_value(), "low");
    }

    #[cfg(not(windows))]
    #[test]
    fn claude_subagent_scalars_require_one_exact_file() {
        let (_temporary, home, workspace) = roots();
        let agent = workspace.join(".claude/agents/reviewer.md");
        write(
            &agent,
            "---\nname: reviewer\nmodel: old\neffort: high\n---\nInstructions.\n",
        );

        apply(
            AgentKind::Claude,
            &home,
            &workspace,
            ConfigChange::SetWorkerModel {
                worker: "reviewer".into(),
                proposed_value: "small".into(),
            },
        );
        let contents = fs::read_to_string(&agent).unwrap();
        assert!(contents.contains("model: small"));
        assert!(contents.contains("effort: high"));
        assert!(contents.ends_with("Instructions.\n"));

        write(
            &home.join(".claude/agents/reviewer.md"),
            "---\nmodel: other\n---\n",
        );
        let error = AgentConfigEditor::new()
            .prepare(
                &context(AgentKind::Claude, &home, &workspace),
                &ConfigChange::SetWorkerModel {
                    worker: "reviewer".into(),
                    proposed_value: "next".into(),
                },
            )
            .unwrap_err();
        assert_eq!(error, ConfigUnavailableReason::AmbiguousTarget);
    }

    #[cfg(not(windows))]
    #[test]
    fn claude_project_mcp_disable_requires_the_exact_server() {
        let (_temporary, home, workspace) = roots();
        write(
            &workspace.join(".mcp.json"),
            r#"{"mcpServers":{"docs":{"command":"synthetic"}}}"#,
        );
        let settings = workspace.join(".claude/settings.json");
        write(&settings, r#"{"theme":"dark"}"#);

        apply(
            AgentKind::Claude,
            &home,
            &workspace,
            ConfigChange::DisableMcpServer {
                server: "docs".into(),
            },
        );
        let value = parse_json(&fs::read(settings).unwrap()).unwrap();
        assert_eq!(value["disabledMcpjsonServers"], serde_json::json!(["docs"]));
        assert_eq!(value["theme"], "dark");
    }

    #[cfg(not(windows))]
    #[test]
    fn codex_edits_defaults_custom_agents_and_mcp_with_comments() {
        let (_temporary, home, workspace) = roots();
        let config = home.join(".codex/config.toml");
        let agent = home.join(".codex/agents/reviewer.toml");
        write(
            &config,
            "# keep this comment\nmodel = \"old\"\nmodel_reasoning_effort = \"high\"\n\n[agents.reviewer]\nconfig_file = \"agents/reviewer.toml\"\n\n[mcp_servers.docs]\nenabled = true\ncommand = \"synthetic\"\n",
        );
        write(
            &agent,
            "# agent comment\nmodel = \"large\"\nmodel_reasoning_effort = \"high\"\nservice_tier = \"fast\"\nother = true\n",
        );

        for change in [
            ConfigChange::SetModel {
                proposed_value: "new".into(),
            },
            ConfigChange::SetReasoning {
                model: Some("new".into()),
                proposed_value: "low".into(),
            },
            ConfigChange::DisableMcpServer {
                server: "docs".into(),
            },
            ConfigChange::SetWorkerModel {
                worker: "reviewer".into(),
                proposed_value: "small".into(),
            },
            ConfigChange::SetWorkerServiceTier {
                worker: "reviewer".into(),
                proposed_value: "default".into(),
            },
        ] {
            apply(AgentKind::Codex, &home, &workspace, change);
        }

        let config_text = fs::read_to_string(config).unwrap();
        assert!(config_text.contains("# keep this comment"));
        assert!(config_text.contains("model = \"new\""));
        assert!(config_text.contains("model_reasoning_effort = \"low\""));
        assert!(config_text.contains("enabled = false"));
        assert!(config_text.contains("command = \"synthetic\""));
        let agent_text = fs::read_to_string(agent).unwrap();
        assert!(agent_text.contains("# agent comment"));
        assert!(agent_text.contains("model = \"small\""));
        assert!(agent_text.contains("model_reasoning_effort = \"high\""));
        assert!(agent_text.contains("service_tier = \"default\""));
        assert!(agent_text.contains("other = true"));
    }

    #[cfg(not(windows))]
    #[test]
    fn opencode_supports_strict_json_root_and_exact_agent_models() {
        let (_temporary, home, workspace) = roots();
        let settings = workspace.join("opencode.json");
        write(
            &settings,
            r#"{"model":"old","agent":{"reviewer":{"model":"large","prompt":"keep"}},"theme":"dark"}"#,
        );
        apply(
            AgentKind::OpenCode,
            &home,
            &workspace,
            ConfigChange::SetModel {
                proposed_value: "provider/new".into(),
            },
        );
        apply(
            AgentKind::OpenCode,
            &home,
            &workspace,
            ConfigChange::SetWorkerModel {
                worker: "reviewer".into(),
                proposed_value: "provider/small".into(),
            },
        );
        let value = parse_json(&fs::read(settings).unwrap()).unwrap();
        assert_eq!(value["model"], "provider/new");
        assert_eq!(value["agent"]["reviewer"]["model"], "provider/small");
        assert_eq!(value["agent"]["reviewer"]["prompt"], "keep");
        assert_eq!(value["theme"], "dark");
    }

    #[cfg(not(windows))]
    #[test]
    fn pi_edits_the_model_pair_and_exact_thinking_level() {
        let (_temporary, home, workspace) = roots();
        let settings = home.join(".pi/agent/settings.json");
        write(
            &settings,
            r#"{"defaultProvider":"old","defaultModel":"large","theme":"dark","modelThinkingLevels":{"new/small":"high"}}"#,
        );
        apply(
            AgentKind::Pi,
            &home,
            &workspace,
            ConfigChange::SetModel {
                proposed_value: "new/small".into(),
            },
        );
        apply(
            AgentKind::Pi,
            &home,
            &workspace,
            ConfigChange::SetReasoning {
                model: Some("new/small".into()),
                proposed_value: "low".into(),
            },
        );
        let value = parse_json(&fs::read(settings).unwrap()).unwrap();
        assert_eq!(value["defaultProvider"], "new");
        assert_eq!(value["defaultModel"], "small");
        assert_eq!(value["modelThinkingLevels"]["new/small"], "low");
        assert_eq!(value["theme"], "dark");
    }

    #[test]
    fn malformed_duplicate_and_oversized_files_are_rejected() {
        let (_temporary, home, workspace) = roots();
        let settings = home.join(".claude/settings.json");
        write(&settings, "{not json}");
        let editor = AgentConfigEditor::new();
        let change = ConfigChange::SetModel {
            proposed_value: "new".into(),
        };
        assert_eq!(
            editor
                .prepare(&context(AgentKind::Claude, &home, &workspace), &change)
                .unwrap_err(),
            ConfigUnavailableReason::MalformedConfig
        );
        write(&settings, r#"{"model":"one","model":"two"}"#);
        assert_eq!(
            editor
                .prepare(&context(AgentKind::Claude, &home, &workspace), &change)
                .unwrap_err(),
            ConfigUnavailableReason::DuplicateDefinition
        );
        fs::write(&settings, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert_eq!(
            editor
                .prepare(&context(AgentKind::Claude, &home, &workspace), &change)
                .unwrap_err(),
            ConfigUnavailableReason::FileTooLarge
        );
    }

    #[test]
    fn semantic_no_op_does_not_replace_the_file() {
        let (_temporary, home, workspace) = roots();
        let settings = home.join(".claude/settings.json");
        let contents = r#"{"model":"same","theme":"dark"}"#;
        write(&settings, contents);
        let identity = file_identity(&fs::symlink_metadata(&settings).unwrap());

        let outcome = AgentConfigEditor::new()
            .prepare(
                &context(AgentKind::Claude, &home, &workspace),
                &ConfigChange::SetModel {
                    proposed_value: "same".into(),
                },
            )
            .unwrap();

        let PrepareOutcome::NoOp(inspection) = outcome else {
            panic!("an equal semantic value must be a no-op");
        };
        assert_eq!(inspection.current_value(), Some("same"));
        assert_eq!(fs::read_to_string(&settings).unwrap(), contents);
        assert_eq!(
            file_identity(&fs::symlink_metadata(&settings).unwrap()),
            identity
        );
    }

    #[test]
    fn invalid_models_reasoning_and_service_tiers_are_rejected() {
        let (_temporary, home, workspace) = roots();
        write(&home.join(".claude/settings.json"), "{}");
        write(
            &home.join(".codex/config.toml"),
            "[agents.reviewer]\nconfig_file = \"agents/reviewer.toml\"\n",
        );
        write(&home.join(".codex/agents/reviewer.toml"), "");
        write(&workspace.join("opencode.json"), "{}");
        write(&home.join(".pi/agent/settings.json"), "{}");
        let editor = AgentConfigEditor::new();

        for (agent, change) in [
            (
                AgentKind::Claude,
                ConfigChange::SetModel {
                    proposed_value: "model with spaces".into(),
                },
            ),
            (
                AgentKind::Claude,
                ConfigChange::SetReasoning {
                    model: Some("model".into()),
                    proposed_value: "max".into(),
                },
            ),
            (
                AgentKind::Codex,
                ConfigChange::SetReasoning {
                    model: None,
                    proposed_value: "auto".into(),
                },
            ),
            (
                AgentKind::Codex,
                ConfigChange::SetWorkerServiceTier {
                    worker: "reviewer".into(),
                    proposed_value: "economy".into(),
                },
            ),
            (
                AgentKind::OpenCode,
                ConfigChange::SetModel {
                    proposed_value: "provider/model/extra".into(),
                },
            ),
            (
                AgentKind::Pi,
                ConfigChange::SetReasoning {
                    model: Some("provider/model".into()),
                    proposed_value: "ultra".into(),
                },
            ),
        ] {
            assert_eq!(
                editor
                    .prepare(&context(agent, &home, &workspace), &change)
                    .unwrap_err(),
                ConfigUnavailableReason::InvalidTarget
            );
        }
    }

    #[test]
    fn non_regular_files_and_jsonc_are_rejected() {
        let (_temporary, home, workspace) = roots();
        fs::create_dir_all(home.join(".claude/settings.json")).unwrap();
        let change = ConfigChange::SetModel {
            proposed_value: "new".into(),
        };
        assert_eq!(
            AgentConfigEditor::new()
                .prepare(&context(AgentKind::Claude, &home, &workspace), &change)
                .unwrap_err(),
            ConfigUnavailableReason::NonRegularFile
        );

        write(
            &home.join(".config/opencode/opencode.jsonc"),
            "{ // comment\n}\n",
        );
        assert_eq!(
            AgentConfigEditor::new()
                .prepare(&context(AgentKind::OpenCode, &home, &workspace), &change)
                .unwrap_err(),
            ConfigUnavailableReason::UnsupportedFormat
        );
    }

    #[test]
    fn opencode_rejects_jsonc_that_can_override_strict_json() {
        let (_temporary, home, workspace) = roots();
        write(
            &workspace.join("opencode.json"),
            r#"{"model":"provider/old"}"#,
        );
        write(
            &workspace.join("opencode.jsonc"),
            "{ // same-scope override\n}\n",
        );

        let editor = AgentConfigEditor::new();
        let change = ConfigChange::SetModel {
            proposed_value: "provider/new".into(),
        };
        assert_eq!(
            editor
                .prepare(&context(AgentKind::OpenCode, &home, &workspace), &change,)
                .unwrap_err(),
            ConfigUnavailableReason::UnsupportedFormat
        );

        fs::remove_file(workspace.join("opencode.jsonc")).unwrap();
        write(
            &workspace.join(".opencode/opencode.jsonc"),
            "{ // higher-precedence override\n}\n",
        );

        assert_eq!(
            editor
                .prepare(&context(AgentKind::OpenCode, &home, &workspace), &change,)
                .unwrap_err(),
            ConfigUnavailableReason::UnsupportedFormat
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_targets_and_paths_outside_the_root_are_rejected() {
        let (_temporary, home, workspace) = roots();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("settings.json"), "{}");
        write(&outside.path().join("opencode.json"), "{}");
        fs::create_dir_all(home.join(".claude")).unwrap();
        symlink(
            outside.path().join("settings.json"),
            home.join(".claude/settings.json"),
        )
        .unwrap();
        let change = ConfigChange::SetModel {
            proposed_value: "new".into(),
        };
        assert_eq!(
            AgentConfigEditor::new()
                .prepare(&context(AgentKind::Claude, &home, &workspace), &change)
                .unwrap_err(),
            ConfigUnavailableReason::SymlinkTarget
        );

        fs::create_dir_all(home.join(".config")).unwrap();
        symlink(outside.path(), home.join(".config/opencode")).unwrap();
        assert_eq!(
            AgentConfigEditor::new()
                .prepare(&context(AgentKind::OpenCode, &home, &workspace), &change)
                .unwrap_err(),
            ConfigUnavailableReason::UnsafePath
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn changed_bytes_and_file_identity_require_a_new_preview() {
        let (_temporary, home, workspace) = roots();
        let settings = home.join(".claude/settings.json");
        write(&settings, r#"{"model":"old"}"#);
        let editor = AgentConfigEditor::new();
        let context = context(AgentKind::Claude, &home, &workspace);
        let change = ConfigChange::SetModel {
            proposed_value: "new".into(),
        };
        let prepared = ready(&editor, &context, &change);
        write(&settings, "{\n  \"model\": \"old\"\n}\n");
        assert_eq!(
            editor.apply(&prepared).unwrap_err(),
            ApplyError::Conflict(ApplyConflict::ChangedContent)
        );

        let prepared = ready(&editor, &context, &change);
        let replacement = home.join(".claude/replacement.json");
        write(&replacement, "{\n  \"model\": \"old\"\n}\n");
        fs::rename(replacement, &settings).unwrap();
        assert_eq!(
            editor.apply(&prepared).unwrap_err(),
            ApplyError::Conflict(ApplyConflict::ChangedIdentity)
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_preserves_permissions_and_validates_readback() {
        let (_temporary, home, workspace) = roots();
        let settings = home.join(".claude/settings.json");
        write(&settings, r#"{"model":"old"}"#);
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o640)).unwrap();
        let editor = AgentConfigEditor::new();
        let context = context(AgentKind::Claude, &home, &workspace);
        let change = ConfigChange::SetModel {
            proposed_value: "new".into(),
        };
        let prepared = ready(&editor, &context, &change);
        assert_eq!(editor.apply(&prepared).unwrap(), ApplyOutcome::Applied);
        assert_eq!(
            fs::metadata(&settings).unwrap().permissions().mode() & 0o777,
            0o640
        );

        let prepared = ready(
            &editor,
            &context,
            &ConfigChange::SetModel {
                proposed_value: "next".into(),
            },
        );
        assert_eq!(
            editor
                .apply_with_readback(&prepared, |path| {
                    fs::write(path, r#"{"model":"next","unrelated":true}"#).unwrap();
                })
                .unwrap_err(),
            ApplyError::Readback(ApplyReadbackError::ChangedContent)
        );
    }

    #[test]
    fn missing_configs_and_named_targets_are_never_created() {
        let (_temporary, home, workspace) = roots();
        let editor = AgentConfigEditor::new();
        let missing_config = home.join(".config/opencode/opencode.json");
        assert_eq!(
            editor
                .prepare(
                    &context(AgentKind::OpenCode, &home, &workspace),
                    &ConfigChange::SetModel {
                        proposed_value: "new".into(),
                    },
                )
                .unwrap_err(),
            ConfigUnavailableReason::MissingConfig
        );
        assert!(!missing_config.exists());

        write(&missing_config, r#"{"agent":{"reviewer":{"model":"old"}}}"#);
        assert_eq!(
            editor
                .prepare(
                    &context(AgentKind::OpenCode, &home, &workspace),
                    &ConfigChange::SetWorkerModel {
                        worker: "unknown".into(),
                        proposed_value: "new".into(),
                    },
                )
                .unwrap_err(),
            ConfigUnavailableReason::MissingTarget
        );
        let value = parse_json(&fs::read(missing_config).unwrap()).unwrap();
        assert!(value["agent"].get("unknown").is_none());
    }

    #[test]
    fn unsupported_environments_and_overrides_return_typed_reasons() {
        let (_temporary, home, workspace) = roots();
        let settings = home.join(".claude/settings.json");
        write(&settings, "{}");
        let change = ConfigChange::SetModel {
            proposed_value: "new".into(),
        };
        let editor = AgentConfigEditor::new();
        let mut context = context(AgentKind::Claude, &home, &workspace);
        context.native_environment = false;
        assert_eq!(
            editor.prepare(&context, &change).unwrap_err(),
            ConfigUnavailableReason::UnsupportedEnvironment
        );
        context.native_environment = true;
        context.runtime_override_present = true;
        assert_eq!(
            editor.prepare(&context, &change).unwrap_err(),
            ConfigUnavailableReason::RuntimeOverride
        );
        context.runtime_override_present = false;
        context.managed_configuration_present = true;
        assert_eq!(
            editor.prepare(&context, &change).unwrap_err(),
            ConfigUnavailableReason::ManagedConfiguration
        );
    }

    #[cfg(windows)]
    #[test]
    fn automatic_apply_is_unavailable_on_windows() {
        let (_temporary, home, workspace) = roots();
        write(&home.join(".claude/settings.json"), r#"{"model":"old"}"#);
        let editor = AgentConfigEditor::new();
        let prepared = ready(
            &editor,
            &context(AgentKind::Claude, &home, &workspace),
            &ConfigChange::SetModel {
                proposed_value: "new".into(),
            },
        );

        assert_eq!(
            editor.apply(&prepared).unwrap_err(),
            ApplyError::Unavailable(ConfigUnavailableReason::AutomaticApplyUnsupported)
        );
        assert_eq!(
            fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
            r#"{"model":"old"}"#
        );
    }

    #[test]
    fn public_errors_and_summaries_do_not_include_paths_or_config_bodies() {
        let (_temporary, home, workspace) = roots();
        let secret = "synthetic-secret-value";
        write(
            &home.join(".claude/settings.json"),
            &format!(r#"{{"apiKey":"{secret}","model":"old"}}"#),
        );
        let editor = AgentConfigEditor::new();
        let prepared = ready(
            &editor,
            &context(AgentKind::Claude, &home, &workspace),
            &ConfigChange::SetModel {
                proposed_value: "new".into(),
            },
        );
        assert!(!prepared.summary().contains(secret));
        let debug = format!("{prepared:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(secret));
        assert!(!debug.contains(home.to_string_lossy().as_ref()));
        for reason in [
            ConfigUnavailableReason::MalformedConfig,
            ConfigUnavailableReason::UnsafePath,
            ConfigUnavailableReason::WriteFailed,
        ] {
            let public = reason.to_string();
            assert!(!public.contains(secret));
            assert!(!public.contains(home.to_string_lossy().as_ref()));
        }
    }
}
