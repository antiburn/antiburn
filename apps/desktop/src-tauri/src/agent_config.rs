//! Safe edits to existing Claude Code and Codex model settings.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
#[cfg(not(windows))]
use std::fs::OpenOptions;
#[cfg(not(windows))]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

use antiburn_local::model::AgentKind;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use toml_edit::DocumentMut;
#[cfg(not(windows))]
use toml_edit::value;

const MAX_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Global,
    Project,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChange {
    pub expected_value: String,
    pub proposed_value: String,
}

pub struct PreparedChange {
    path: PathBuf,
    scope: ConfigScope,
    #[cfg(not(windows))]
    trusted_root: PathBuf,
    #[cfg(not(windows))]
    original_bytes: Vec<u8>,
    #[cfg(not(windows))]
    proposed_bytes: Vec<u8>,
    #[cfg(not(windows))]
    identity: FileIdentity,
    #[cfg(not(windows))]
    permissions: fs::Permissions,
    #[cfg(not(windows))]
    format: ConfigFormat,
}

impl fmt::Debug for PreparedChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedChange")
            .field("scope", &self.scope)
            .field("private_file_data", &"<redacted>")
            .finish()
    }
}

impl PreparedChange {
    pub const fn scope(&self) -> ConfigScope {
        self.scope
    }

    pub(crate) fn physical_identity(&self) -> (&Path, &'static str) {
        (&self.path, "model")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveModel {
    pub scope: ConfigScope,
    pub value: String,
    path: PathBuf,
}

impl EffectiveModel {
    pub(crate) fn physical_identity(&self) -> (&Path, &'static str) {
        (&self.path, "model")
    }
}

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
    UnsupportedEnvironment,
    ManagedConfiguration,
    RuntimeOverride,
    MissingConfig,
    MissingTarget,
    CurrentValueMismatch,
    InvalidTarget,
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

#[derive(Debug, Default)]
pub struct AgentConfigEditor;

impl AgentConfigEditor {
    pub const fn new() -> Self {
        Self
    }

    pub fn effective_model(
        &self,
        context: &ConfigContext,
    ) -> Result<EffectiveModel, ConfigUnavailableReason> {
        validate_context(context)?;
        let home = canonical_root(&context.home_root)?;
        let workspace = context
            .workspace_root
            .as_deref()
            .map(canonical_root)
            .transpose()?;
        let target = resolve_target(context.agent, &home, workspace.as_deref())?;
        let file = read_checked(&target.path, &target.root)?;
        let value = semantic_value(&file.bytes, target.format)?
            .ok_or(ConfigUnavailableReason::MissingTarget)?;
        Ok(EffectiveModel {
            scope: target.scope,
            value,
            path: target.path,
        })
    }

    pub fn prepare(
        &self,
        context: &ConfigContext,
        change: &ConfigChange,
    ) -> Result<PreparedChange, ConfigUnavailableReason> {
        validate_context(context)?;
        validate_model(&change.expected_value)?;
        validate_model(&change.proposed_value)?;
        if change.expected_value == change.proposed_value {
            return Err(ConfigUnavailableReason::InvalidTarget);
        }
        let home = canonical_root(&context.home_root)?;
        let workspace = context
            .workspace_root
            .as_deref()
            .map(canonical_root)
            .transpose()?;
        let target = resolve_target(context.agent, &home, workspace.as_deref())?;
        let file = read_checked(&target.path, &target.root)?;
        let current = semantic_value(&file.bytes, target.format)?
            .ok_or(ConfigUnavailableReason::MissingTarget)?;
        if current != change.expected_value {
            return Err(ConfigUnavailableReason::CurrentValueMismatch);
        }
        #[cfg(not(windows))]
        let proposed_bytes = edit_model(&file.bytes, target.format, &change.proposed_value)?;
        Ok(PreparedChange {
            path: target.path,
            scope: target.scope,
            #[cfg(not(windows))]
            trusted_root: target.root,
            #[cfg(not(windows))]
            original_bytes: file.bytes,
            #[cfg(not(windows))]
            proposed_bytes,
            #[cfg(not(windows))]
            identity: file.identity,
            #[cfg(not(windows))]
            permissions: file.permissions,
            #[cfg(not(windows))]
            format: target.format,
        })
    }

    #[cfg(not(windows))]
    pub fn apply(&self, prepared: &PreparedChange) -> Result<(), ApplyError> {
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
            output
                .write_all(&prepared.proposed_bytes)
                .and_then(|()| output.sync_all())
                .map_err(|error| ApplyError::Unavailable(map_write_error(error)))?;
            drop(output);
            let replacement_identity = file_identity(
                &fs::symlink_metadata(&temporary)
                    .map_err(|_| ApplyError::Unavailable(ConfigUnavailableReason::WriteFailed))?,
            );
            let current = read_checked(&prepared.path, &prepared.trusted_root)
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
        let readback = read_checked(&prepared.path, &prepared.trusted_root)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::ReadFailed))?;
        if readback.identity != replacement_identity {
            return Err(ApplyError::Readback(ApplyReadbackError::ChangedIdentity));
        }
        if readback.bytes != prepared.proposed_bytes {
            return Err(ApplyError::Readback(ApplyReadbackError::ChangedContent));
        }
        let actual = semantic_value(&readback.bytes, prepared.format)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::InvalidContent))?;
        let expected = semantic_value(&prepared.proposed_bytes, prepared.format)
            .map_err(|_| ApplyError::Readback(ApplyReadbackError::InvalidContent))?;
        if actual != expected {
            return Err(ApplyError::Readback(ApplyReadbackError::SemanticMismatch));
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn apply(&self, _: &PreparedChange) -> Result<(), ApplyError> {
        Err(ApplyError::Unavailable(
            ConfigUnavailableReason::AutomaticApplyUnsupported,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
enum ConfigFormat {
    Json,
    Toml,
}

struct Target {
    path: PathBuf,
    root: PathBuf,
    scope: ConfigScope,
    format: ConfigFormat,
}

fn validate_context(context: &ConfigContext) -> Result<(), ConfigUnavailableReason> {
    if !context.native_environment {
        return Err(ConfigUnavailableReason::UnsupportedEnvironment);
    }
    if context.managed_configuration_present {
        return Err(ConfigUnavailableReason::ManagedConfiguration);
    }
    if context.runtime_override_present {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    }
    if !matches!(context.agent, AgentKind::Claude | AgentKind::Codex) {
        return Err(ConfigUnavailableReason::UnsupportedAgent);
    }
    Ok(())
}

fn resolve_target(
    agent: AgentKind,
    home: &Path,
    workspace: Option<&Path>,
) -> Result<Target, ConfigUnavailableReason> {
    let (format, global, projects): (ConfigFormat, PathBuf, Vec<PathBuf>) = match agent {
        AgentKind::Claude => (
            ConfigFormat::Json,
            home.join(".claude/settings.json"),
            workspace
                .map(|root| {
                    vec![
                        root.join(".claude/settings.local.json"),
                        root.join(".claude/settings.json"),
                    ]
                })
                .unwrap_or_default(),
        ),
        AgentKind::Codex => (
            ConfigFormat::Toml,
            home.join(".codex/config.toml"),
            workspace
                .map(|root| vec![root.join(".codex/config.toml")])
                .unwrap_or_default(),
        ),
        _ => return Err(ConfigUnavailableReason::UnsupportedAgent),
    };
    if let Some(root) = workspace {
        for path in projects {
            if path_entry_exists(&path)? {
                let file = read_checked(&path, root)?;
                if semantic_value(&file.bytes, format)?.is_some() {
                    return Ok(Target {
                        path,
                        root: root.to_owned(),
                        scope: ConfigScope::Project,
                        format,
                    });
                }
            }
        }
    }
    if !path_entry_exists(&global)? {
        return Err(ConfigUnavailableReason::MissingConfig);
    }
    let file = read_checked(&global, home)?;
    if semantic_value(&file.bytes, format)?.is_none() {
        return Err(ConfigUnavailableReason::MissingTarget);
    }
    Ok(Target {
        path: global,
        root: home.to_owned(),
        scope: ConfigScope::Global,
        format,
    })
}

#[cfg(not(windows))]
fn edit_model(
    bytes: &[u8],
    format: ConfigFormat,
    proposed: &str,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    match format {
        ConfigFormat::Json => {
            let mut root = parse_json(bytes)?;
            root.as_object_mut()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?
                .insert("model".into(), Value::String(proposed.into()));
            let mut output = serde_json::to_vec_pretty(&root)
                .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
            output.push(b'\n');
            Ok(output)
        }
        ConfigFormat::Toml => {
            let text =
                std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
            let mut document = parse_toml(text)?;
            document["model"] = value(proposed);
            Ok(document.to_string().into_bytes())
        }
    }
}

fn semantic_value(
    bytes: &[u8],
    format: ConfigFormat,
) -> Result<Option<String>, ConfigUnavailableReason> {
    match format {
        ConfigFormat::Json => parse_json(bytes)?
            .get("model")
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or(ConfigUnavailableReason::MalformedConfig)
            })
            .transpose(),
        ConfigFormat::Toml => {
            let text =
                std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
            let document = parse_toml(text)?;
            if document.get("profile").is_some() {
                return Err(ConfigUnavailableReason::RuntimeOverride);
            }
            document
                .get("model")
                .map(|item| {
                    item.as_str()
                        .map(ToOwned::to_owned)
                        .ok_or(ConfigUnavailableReason::MalformedConfig)
                })
                .transpose()
        }
    }
}

fn validate_model(value: &str) -> Result<(), ConfigUnavailableReason> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Err(ConfigUnavailableReason::InvalidTarget)
    } else {
        Ok(())
    }
}

struct CheckedFile {
    bytes: Vec<u8>,
    #[cfg(not(windows))]
    identity: FileIdentity,
    #[cfg(not(windows))]
    permissions: fs::Permissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

fn canonical_root(path: &Path) -> Result<PathBuf, ConfigUnavailableReason> {
    path.canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)
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

fn read_checked(path: &Path, trusted_root: &Path) -> Result<CheckedFile, ConfigUnavailableReason> {
    let metadata = fs::symlink_metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ConfigUnavailableReason::MissingConfig,
        std::io::ErrorKind::PermissionDenied => ConfigUnavailableReason::PermissionDenied,
        _ => ConfigUnavailableReason::UnsafePath,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ConfigUnavailableReason::SymlinkTarget);
    }
    if !metadata.is_file() {
        return Err(ConfigUnavailableReason::NonRegularFile);
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if !canonical.starts_with(trusted_root) {
        return Err(ConfigUnavailableReason::UnsafePath);
    }
    check_owner(&metadata, trusted_root)?;
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
    let after = fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if after.file_type().is_symlink() || file_identity(&after) != file_identity(&metadata) {
        return Err(ConfigUnavailableReason::ChangedIdentity);
    }
    Ok(CheckedFile {
        bytes,
        #[cfg(not(windows))]
        identity: file_identity(&metadata),
        #[cfg(not(windows))]
        permissions: metadata.permissions(),
    })
}

#[cfg(unix)]
fn check_owner(metadata: &fs::Metadata, root: &Path) -> Result<(), ConfigUnavailableReason> {
    use std::os::unix::fs::MetadataExt;
    let root = fs::metadata(root).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if metadata.uid() != root.uid() {
        Err(ConfigUnavailableReason::UnsupportedOwner)
    } else {
        Ok(())
    }
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
            modified: metadata.modified().ok(),
        }
    }
}

#[cfg(not(windows))]
fn create_temporary(path: &Path, permissions: &fs::Permissions) -> std::io::Result<fs::File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(permissions.mode())
        .open(path)?;
    file.set_permissions(permissions.clone())?;
    Ok(file)
}

#[cfg(not(windows))]
fn map_write_error(error: std::io::Error) -> ConfigUnavailableReason {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        ConfigUnavailableReason::PermissionDenied
    } else {
        ConfigUnavailableReason::WriteFailed
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

fn parse_json(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueJson::deserialize(&mut deserializer)
        .map_err(|error| {
            if format!("{error:?}").contains("duplicate") {
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
struct UniqueJsonVisitor;

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJson;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate keys")
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn claude_resolves_project_and_inherited_global_scope() {
        let (_temporary, home, project) = roots();
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
    fn codex_preserves_formatting_and_inherited_scope() {
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
    fn codex_rejects_an_active_profile_override() {
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

    #[cfg(not(windows))]
    #[test]
    fn apply_rejects_a_content_conflict() {
        let (_temporary, home, project) = roots();
        let path = home.join(".claude/settings.json");
        write(&path, r#"{"model":"old"}"#);
        let editor = AgentConfigEditor::new();
        let prepared = editor
            .prepare(
                &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
                &ConfigChange {
                    expected_value: "old".into(),
                    proposed_value: "new".into(),
                },
            )
            .unwrap();
        write(&path, r#"{"model":"other"}"#);
        assert_eq!(
            editor.apply(&prepared),
            Err(ApplyError::Conflict(ApplyConflict::ChangedContent))
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_preserves_the_exact_original_mode() {
        let (_temporary, home, project) = roots();
        let path = home.join(".claude/settings.json");
        write(&path, r#"{"model":"old"}"#);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o764)).unwrap();
        let editor = AgentConfigEditor::new();
        let prepared = editor
            .prepare(
                &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
                &ConfigChange {
                    expected_value: "old".into(),
                    proposed_value: "new".into(),
                },
            )
            .unwrap();
        editor.apply(&prepared).unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o764
        );
    }
}
