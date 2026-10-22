//! Exact current OpenCode controls. Transcript source versions do not select this resolver.

use serde_json::Value;
use std::{
    fmt,
    path::{Path, PathBuf},
};

use super::filesystem::{path_entry_exists, read_checked};
use super::opencode_config_authority::{ConfigEntry, CurrentConfigAuthority};
use super::vendors::OperationSelector;
use super::{ConfigContext, ConfigScope, ConfigSetting, ConfigUnavailableReason};

#[path = "opencode_current/inputs.rs"]
mod inputs;
#[path = "opencode_current/inventory.rs"]
mod inventory;
pub(crate) use inventory::current_inventory;
#[path = "opencode_current/normalize.rs"]
mod normalize;
#[path = "opencode_current/skills.rs"]
mod skills;
#[cfg(test)]
#[path = "opencode_current/tests.rs"]
mod tests;
#[path = "opencode_current/write.rs"]
pub(super) mod write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenCodeConfigSubject {
    FutureDefaultModel,
    AgentModel(String),
    AutomaticCompaction,
    McpServer(String),
    SkillAdvertising(String),
}

#[derive(Clone, PartialEq, Eq)]
struct Dependency {
    path: PathBuf,
    digest: Option<[u8; 32]>,
    bytes: usize,
}

#[derive(Clone)]
pub(crate) struct OpenCodeResolvedTarget {
    authority: CurrentConfigAuthority,
    context: ConfigContext,
    subject: OpenCodeConfigSubject,
    path: PathBuf,
    safety_root: PathBuf,
    scope: ConfigScope,
    selector: OperationSelector,
    value: Value,
    dependencies: Vec<Dependency>,
}

impl fmt::Debug for OpenCodeResolvedTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCodeResolvedTarget")
            .field("setting", &self.setting())
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl OpenCodeResolvedTarget {
    pub(crate) fn reviewed_model_route(&self) -> bool {
        self.value.get("variant").is_none() && self.reviewed_provider_route(&self.value)
    }

    pub(crate) fn reviewed_proposed_model_route(&self, value: &Value) -> bool {
        normalize::model(value).is_ok_and(|value| {
            value.get("variant").is_none() && self.reviewed_provider_route(&value)
        })
    }

    fn reviewed_model_operation_routes(&self, proposed: &Value) -> bool {
        let current_route = match self.setting() {
            ConfigSetting::Model => self.reviewed_default_route(),
            ConfigSetting::SubagentModel => self.reviewed_provider_route(&self.value),
            _ => return true,
        };
        current_route
            && normalize::model(proposed).is_ok_and(|value| self.reviewed_provider_route(&value))
    }

    fn reviewed_provider_route(&self, value: &Value) -> bool {
        let Some(provider) = value.get("providerID").and_then(Value::as_str) else {
            return false;
        };
        let Some(model) = value.get("model").and_then(Value::as_str) else {
            return false;
        };
        if !["openai", "anthropic", "google"].contains(&provider) {
            return false;
        }
        self.authority.entries().is_ok_and(|entries| {
            entries.iter().all(|entry| {
                let route = entry
                    .info
                    .as_ref()
                    .and_then(|info| info.get("providers"))
                    .and_then(|providers| providers.get(provider));
                !route.is_some_and(|route| {
                    route.get("settings").is_some()
                        || ["body", "canonical", "package", "headers"]
                            .iter()
                            .any(|key| route.get(key).is_some())
                        || route
                            .get("models")
                            .and_then(|models| models.get(model))
                            .is_some()
                })
            })
        })
    }

    pub(crate) fn reviewed_default_route(&self) -> bool {
        if !self.reviewed_model_route() {
            return false;
        }
        if self
            .dependencies
            .iter()
            .filter(|dependency| {
                dependency
                    .path
                    .extension()
                    .is_some_and(|extension| extension == "md")
            })
            .any(|dependency| {
                inputs::read_dependency(&dependency.path, &mut Vec::new())
                    .and_then(|bytes| {
                        let (header, _) = inputs::markdown(&bytes)?;
                        Ok(header.get("model").is_some()
                            && header.get("mode").and_then(serde_yaml_ng::Value::as_str)
                                != Some("subagent"))
                    })
                    .unwrap_or(true)
            })
        {
            return false;
        }
        self.authority.entries().is_ok_and(|entries| {
            entries.iter().all(|entry| {
                let Some(info) = &entry.info else {
                    return true;
                };
                let agent_model =
                    info.get("agents")
                        .and_then(Value::as_object)
                        .is_some_and(|agents| {
                            agents.values().any(|agent| {
                                agent.get("model").is_some()
                                    && agent.get("mode").and_then(Value::as_str) != Some("subagent")
                            })
                        });
                !agent_model
            })
        })
    }

    pub(crate) fn setting(&self) -> ConfigSetting {
        match self.subject {
            OpenCodeConfigSubject::FutureDefaultModel => ConfigSetting::Model,
            OpenCodeConfigSubject::AgentModel(_) => ConfigSetting::SubagentModel,
            OpenCodeConfigSubject::AutomaticCompaction => ConfigSetting::Compaction,
            OpenCodeConfigSubject::McpServer(_) => ConfigSetting::McpServer,
            OpenCodeConfigSubject::SkillAdvertising(_) => ConfigSetting::Skill,
        }
    }

    #[cfg(test)]
    pub(crate) fn raw_selector(&self) -> super::PhysicalSelector {
        let path = self.selector.exact_path();
        if matches!(
            self.selector,
            OperationSelector::OpenCodeSkillAdvertising(_)
        ) || self.path.extension().is_some_and(|ext| ext == "md")
        {
            super::PhysicalSelector::MarkdownFrontmatter(path)
        } else if self.path.extension().is_some_and(|ext| ext == "jsonc") {
            super::PhysicalSelector::Jsonc(path)
        } else {
            super::PhysicalSelector::Json(path)
        }
    }

    pub(crate) fn physical_path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn selector_label(&self) -> &'static str {
        self.selector.physical_selector()
    }

    pub(crate) fn physical_identity(&self) -> (&Path, &'static str) {
        (&self.path, self.selector.physical_selector())
    }

    pub(crate) fn selector_qualifier(&self) -> Option<&str> {
        match &self.subject {
            OpenCodeConfigSubject::AgentModel(id)
            | OpenCodeConfigSubject::McpServer(id)
            | OpenCodeConfigSubject::SkillAdvertising(id) => Some(id),
            _ => None,
        }
    }

    pub(crate) fn current_value_display(&self) -> Option<String> {
        match self.setting() {
            ConfigSetting::Model | ConfigSetting::SubagentModel => {
                let provider = self.value.get("providerID")?.as_str()?;
                let model = self.value.get("model")?.as_str()?;
                let variant = self
                    .value
                    .get("variant")
                    .and_then(Value::as_str)
                    .map(|variant| format!("#{variant}"))
                    .unwrap_or_default();
                Some(format!("{provider}/{model}{variant}"))
            }
            ConfigSetting::Compaction => self.value.as_bool().map(|value| value.to_string()),
            ConfigSetting::Reasoning
            | ConfigSetting::FastMode
            | ConfigSetting::McpServer
            | ConfigSetting::BuiltInTool
            | ConfigSetting::Skill => None,
        }
    }

    pub(crate) fn scope(&self) -> ConfigScope {
        self.scope
    }
    pub(crate) fn current_value(&self) -> Value {
        if matches!(self.subject, OpenCodeConfigSubject::McpServer(_)) {
            return Value::Bool(self.value.get("disabled").and_then(Value::as_bool) != Some(true));
        }
        self.value.clone()
    }

    pub(crate) fn matches_operation(&self, operation: &super::ConfigOperation) -> bool {
        if operation.setting != self.setting() {
            return false;
        }
        match &operation.expected_value {
            super::ConfigOperationValue::Scalar(value) => {
                self.current_value_display().as_ref() == Some(value)
            }
            super::ConfigOperationValue::Boolean(value) => {
                self.current_value() == Value::Bool(*value)
            }
            super::ConfigOperationValue::MapEntry { key, value } => {
                self.selector_qualifier() == Some(key.as_str())
                    && self
                        .current_value()
                        .as_bool()
                        .is_some_and(|current| value == &current.to_string())
            }
            _ => false,
        }
    }

    pub(crate) fn resolution_digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        for dependency in &self.dependencies {
            let path = dependency.path.to_string_lossy();
            digest.update(path.len().to_le_bytes());
            digest.update(path.as_bytes());
            digest.update([u8::from(dependency.digest.is_some())]);
            if let Some(bytes) = dependency.digest {
                digest.update(bytes);
            }
        }
        digest.finalize().into()
    }

    fn same_binding(&self, other: &Self) -> bool {
        self.subject == other.subject
            && self.path == other.path
            && self.scope == other.scope
            && self.selector == other.selector
            && self.safety_root == other.safety_root
    }

    fn retained_bytes(&self) -> usize {
        self.authority
            .retained_bytes()
            .saturating_add(self.value.to_string().len().saturating_mul(64))
            .saturating_add(
                [
                    Some(self.path.as_path()),
                    Some(self.safety_root.as_path()),
                    Some(self.context.home_root.as_path()),
                    self.context.workspace_cwd.as_deref(),
                    self.context.trusted_workspace_root.as_deref(),
                ]
                .into_iter()
                .flatten()
                .map(|path| path.as_os_str().len().saturating_mul(2))
                .sum::<usize>(),
            )
            .saturating_add(
                self.dependencies
                    .iter()
                    .map(|entry| {
                        entry
                            .path
                            .as_os_str()
                            .len()
                            .saturating_mul(2)
                            .saturating_add(128)
                    })
                    .sum::<usize>(),
            )
    }
}

pub(crate) struct PreparedOpenCodeOperation {
    target: OpenCodeResolvedTarget,
    additional_targets: Vec<OpenCodeResolvedTarget>,
    operation: super::PreparedOperation,
    proposed: Value,
}

impl fmt::Debug for PreparedOpenCodeOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedOpenCodeOperation")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl PreparedOpenCodeOperation {
    pub(crate) fn target(&self) -> &OpenCodeResolvedTarget {
        &self.target
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.operation
            .retained_bytes()
            .saturating_add(self.target.retained_bytes())
            .saturating_add(
                self.additional_targets
                    .iter()
                    .map(OpenCodeResolvedTarget::retained_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(self.proposed.to_string().len().saturating_mul(64))
    }
}

pub(crate) fn resolve(
    context: &ConfigContext,
    authority: &CurrentConfigAuthority,
    subject: OpenCodeConfigSubject,
) -> Result<OpenCodeResolvedTarget, ConfigUnavailableReason> {
    if context.agent != antiburn_local::model::AgentKind::OpenCode || !context.native_environment {
        return Err(ConfigUnavailableReason::UnsupportedEnvironment);
    }
    let cwd = context
        .workspace_cwd
        .as_deref()
        .ok_or(ConfigUnavailableReason::UnsafePath)?
        .canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if authority.location() != cwd {
        return Err(ConfigUnavailableReason::InvalidPrecedence);
    }
    if let Some(root) = &context.trusted_workspace_root {
        let root = root
            .canonicalize()
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        if !cwd.starts_with(root) {
            return Err(ConfigUnavailableReason::UnsafePath);
        }
    }
    let entries = authority
        .entries()
        .map_err(|_| ConfigUnavailableReason::RuntimeOverride)?;
    let mut dependencies = inputs::dependencies(context, entries)?;
    if subject == OpenCodeConfigSubject::FutureDefaultModel {
        for directory in entries
            .iter()
            .filter(|entry| entry.info.is_none())
            .filter_map(|entry| entry.path.as_deref())
        {
            for field in ["agent", "agents", "mode", "modes"] {
                for path in inputs::files(&directory.join(field), 0, &mut dependencies)? {
                    inputs::read_dependency(&path, &mut dependencies)?;
                }
            }
        }
    }
    if let OpenCodeConfigSubject::SkillAdvertising(id) = &subject {
        let (path, value) = skills::winner(context, entries, id, &mut dependencies)?;
        return target(
            context,
            authority,
            subject.clone(),
            path,
            OperationSelector::OpenCodeSkillAdvertising(id.clone()),
            Value::Bool(value),
            dependencies,
        );
    }
    let mut winner = None;
    let mut pathless_winner = false;
    for entry in entries {
        if let Some(info) = &entry.info {
            normalize::reject_unresolved(info)?;
            let disabled_agent = match &subject {
                OpenCodeConfigSubject::AgentModel(id) => {
                    info.get("agents")
                        .and_then(|agents| agents.get(id))
                        .and_then(|agent| agent.get("disabled"))
                        .and_then(Value::as_bool)
                        == Some(true)
                }
                _ => false,
            };
            if disabled_agent {
                winner = None;
                pathless_winner = false;
                continue;
            }
            let raw = match &entry.path {
                Some(path) => {
                    let bytes = inputs::read_dependency(path, &mut dependencies)?;
                    super::vendors::json::parse(&bytes)?
                }
                None => {
                    if normalize::canonical_value(info, &subject)?.is_some() {
                        winner = None;
                        pathless_winner = true;
                    }
                    continue;
                }
            };
            normalize::reject_unresolved(&raw)?;
            let selected = normalize::select(&raw, &subject)?;
            let observed = normalize::canonical_value(info, &subject)?;
            if selected.as_ref().map(|(_, value)| value) != observed.as_ref() {
                return Err(ConfigUnavailableReason::InvalidPrecedence);
            }
            if let Some((selector, value)) = selected {
                pathless_winner = false;
                winner = Some((
                    entry
                        .path
                        .clone()
                        .ok_or(ConfigUnavailableReason::MissingTarget)?,
                    selector,
                    value,
                ));
            }
        } else if matches!(subject, OpenCodeConfigSubject::AgentModel(_)) {
            inputs::agent_documents(
                entry
                    .path
                    .as_deref()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?,
                &subject,
                &mut dependencies,
                &mut winner,
            )?;
            if winner.is_some() {
                pathless_winner = false;
            }
        }
    }
    if pathless_winner {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    }
    let (path, selector, value) = winner.ok_or(ConfigUnavailableReason::MissingTarget)?;
    target(
        context,
        authority,
        subject,
        path,
        selector,
        value,
        dependencies,
    )
}

fn target(
    context: &ConfigContext,
    authority: &CurrentConfigAuthority,
    subject: OpenCodeConfigSubject,
    path: PathBuf,
    selector: OperationSelector,
    value: Value,
    dependencies: Vec<Dependency>,
) -> Result<OpenCodeResolvedTarget, ConfigUnavailableReason> {
    let home = context
        .home_root
        .canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    let workspace = context
        .trusted_workspace_root
        .as_deref()
        .map(|root| root.canonicalize())
        .transpose()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    let (safety_root, scope) = if let Some(root) = workspace.filter(|root| path.starts_with(root)) {
        (root, ConfigScope::Project)
    } else if path.starts_with(&home) {
        (home, ConfigScope::Global)
    } else {
        let root = authority
            .entries()
            .map_err(|_| ConfigUnavailableReason::RuntimeOverride)?
            .iter()
            .find(|entry| entry.info.is_none())
            .and_then(|entry| entry.path.as_ref())
            .filter(|root| path.starts_with(root))
            .filter(|root| {
                !matches!(
                    root.to_str(),
                    Some("/etc/opencode" | "/Library/Application Support/opencode")
                )
            })
            .ok_or(ConfigUnavailableReason::UnsafePath)?
            .canonicalize()
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        (root, ConfigScope::Global)
    };
    read_checked(&path, &safety_root)?;
    Ok(OpenCodeResolvedTarget {
        authority: authority.clone(),
        context: context.clone(),
        subject,
        path,
        safety_root,
        scope,
        selector,
        value,
        dependencies,
    })
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

fn dependency(path: &Path) -> Result<Dependency, ConfigUnavailableReason> {
    let mut bytes = 0;
    let digest = if path_entry_exists(path)? {
        if std::fs::symlink_metadata(path)
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?
            .is_dir()
        {
            let names = inputs::directory_entries(path)?;
            let mut listing = Vec::new();
            for path in names.iter().filter(|path| {
                path.is_dir()
                    || path.extension().is_some_and(|extension| extension == "md")
                    || path
                        .file_name()
                        .is_some_and(|name| name == "opencode.json" || name == "opencode.jsonc")
            }) {
                let name = path.to_str().ok_or(ConfigUnavailableReason::UnsafePath)?;
                listing.extend_from_slice(&name.len().to_le_bytes());
                listing.extend_from_slice(name.as_bytes());
            }
            Some(hash(&listing))
        } else {
            let parent = path.parent().ok_or(ConfigUnavailableReason::UnsafePath)?;
            let file = read_checked(path, parent)?;
            bytes = file.bytes.len();
            Some(hash(&file.bytes))
        }
    } else {
        None
    };
    Ok(Dependency {
        path: path.into(),
        digest,
        bytes,
    })
}
