use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use antiburn_local::analysis::{CoverageReason, EvidenceValue, SessionEvidence, SourceOrigin};
use antiburn_local::model::AgentKind;
use serde_json::{Map, Value};

use super::filesystem::{canonical_root, path_entry_exists, read_checked};
use super::vendors::{codex, json, opencode, pi};
use super::{ConfigContext, ConfigUnavailableReason};

const MAX_RESOURCES: usize = 512;
const MAX_DIRECTORY_ENTRIES: usize = 256;
const MAX_SKILL_DIRECTORIES: usize = 4096;
const MAX_RESOURCE_NAME_BYTES: usize = 256;
const PI_MCP_PACKAGE: &str = "pi-mcp-extension";
const PI_MCP_VERSION: &str = "1.5.0";
const PI_MCP_ENTRY: &str = "./src/index.ts";
const PI_DEFAULT_TOOLS: [&str; 4] = ["read", "bash", "edit", "write"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    McpServer,
    BuiltInTool,
    Skill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceScope {
    Global,
    Project,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnabledState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceProvenance {
    StandardConfig,
    StandardDirectory,
    BuiltInCatalog,
    PiMcpExtension,
    IndexedSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryIssueReason {
    Config(ConfigUnavailableReason),
    DynamicSource,
    UnsupportedShape,
    ConflictingDefinition,
    ResourceCapExceeded,
    InvalidResourceName,
    PartialIndexedEvidence(CoverageReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryIssue {
    pub kind: Option<ResourceKind>,
    pub scope: ResourceScope,
    pub reason: InventoryIssueReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisoryResource {
    pub agent: AgentKind,
    pub kind: ResourceKind,
    pub canonical_name: String,
    pub enabled: EnabledState,
    pub scope: ResourceScope,
    pub provenance: Vec<ResourceProvenance>,
    /// One reviewed definition or listing size. Resource bodies are never counted.
    pub definition_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceInventory {
    pub agent: AgentKind,
    pub resources: Vec<AdvisoryResource>,
    pub issues: Vec<InventoryIssue>,
}

#[derive(Debug, Clone, Copy)]
pub struct IndexedResourceEvidence<'a> {
    pub evidence: &'a SessionEvidence,
    pub scope: ResourceScope,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceKey {
    kind: ResourceKind,
    scope: ResourceScope,
    normalized_name: String,
}

struct InventoryBuilder {
    agent: AgentKind,
    resources: BTreeMap<ResourceKey, AdvisoryResource>,
    issues: Vec<InventoryIssue>,
}

impl InventoryBuilder {
    fn new(agent: AgentKind) -> Self {
        Self {
            agent,
            resources: BTreeMap::new(),
            issues: Vec::new(),
        }
    }

    fn issue(
        &mut self,
        kind: Option<ResourceKind>,
        scope: ResourceScope,
        reason: InventoryIssueReason,
    ) {
        let issue = InventoryIssue {
            kind,
            scope,
            reason,
        };
        if !self.issues.contains(&issue) {
            self.issues.push(issue);
        }
    }

    fn add(
        &mut self,
        kind: ResourceKind,
        name: &str,
        enabled: EnabledState,
        scope: ResourceScope,
        provenance: ResourceProvenance,
    ) {
        self.add_with_tokens(kind, name, enabled, scope, provenance, None);
    }

    fn add_with_tokens(
        &mut self,
        kind: ResourceKind,
        name: &str,
        enabled: EnabledState,
        scope: ResourceScope,
        provenance: ResourceProvenance,
        definition_tokens: Option<u64>,
    ) {
        if name.is_empty()
            || name.len() > MAX_RESOURCE_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            self.issue(Some(kind), scope, InventoryIssueReason::InvalidResourceName);
            return;
        }
        let normalized_name = name.to_ascii_lowercase();
        let key = ResourceKey {
            kind,
            scope,
            normalized_name,
        };
        if let Some(current) = self.resources.get_mut(&key) {
            if current.canonical_name != name {
                self.issue(
                    Some(kind),
                    scope,
                    InventoryIssueReason::ConflictingDefinition,
                );
                return;
            }
            if current.enabled == EnabledState::Unknown && enabled != EnabledState::Unknown {
                current.enabled = enabled;
            }
            if !current.provenance.contains(&provenance) {
                current.provenance.push(provenance);
                current.provenance.sort_unstable();
            }
            if current.definition_tokens.is_none() {
                current.definition_tokens = definition_tokens;
            } else if definition_tokens.is_some() && current.definition_tokens != definition_tokens
            {
                self.issue(
                    Some(kind),
                    scope,
                    InventoryIssueReason::ConflictingDefinition,
                );
            }
            return;
        }
        if self
            .resources
            .keys()
            .filter(|key| key.kind == kind && key.scope == scope)
            .count()
            >= MAX_RESOURCES
        {
            self.issue(Some(kind), scope, InventoryIssueReason::ResourceCapExceeded);
            return;
        }
        self.resources.insert(
            key,
            AdvisoryResource {
                agent: self.agent,
                kind,
                canonical_name: name.to_owned(),
                enabled,
                scope,
                provenance: vec![provenance],
                definition_tokens,
            },
        );
    }

    fn replace(
        &mut self,
        kind: ResourceKind,
        name: &str,
        enabled: EnabledState,
        scope: ResourceScope,
        provenance: ResourceProvenance,
    ) {
        let key = ResourceKey {
            kind,
            scope,
            normalized_name: name.to_ascii_lowercase(),
        };
        self.resources.remove(&key);
        self.add(kind, name, enabled, scope, provenance);
    }

    fn set_state(
        &mut self,
        kind: ResourceKind,
        name: &str,
        enabled: EnabledState,
        scope: ResourceScope,
        provenance: ResourceProvenance,
    ) {
        let key = ResourceKey {
            kind,
            scope,
            normalized_name: name.to_ascii_lowercase(),
        };
        if let Some(current) = self.resources.get_mut(&key) {
            current.enabled = enabled;
            if !current.provenance.contains(&provenance) {
                current.provenance.push(provenance);
                current.provenance.sort_unstable();
            }
        } else {
            self.add(kind, name, enabled, scope, provenance);
        }
    }

    fn finish(mut self) -> ResourceInventory {
        self.issues
            .sort_by_key(|issue| (issue.kind, issue.scope, format!("{:?}", issue.reason)));
        ResourceInventory {
            agent: self.agent,
            resources: self.resources.into_values().collect(),
            issues: self.issues,
        }
    }
}

pub fn advisory_resource_inventory<'a>(
    context: &ConfigContext,
    indexed: impl IntoIterator<Item = IndexedResourceEvidence<'a>>,
) -> Result<ResourceInventory, ConfigUnavailableReason> {
    if !context.native_environment {
        return Err(ConfigUnavailableReason::UnsupportedEnvironment);
    }
    if !matches!(
        context.agent,
        AgentKind::Claude
            | AgentKind::Codex
            | AgentKind::OpenCode
            | AgentKind::Pi
            | AgentKind::Cursor
            | AgentKind::Copilot
            | AgentKind::Cline
            | AgentKind::Kiro
            | AgentKind::AmpCode
            | AgentKind::Antigravity
            | AgentKind::Windsurf
    ) {
        return Err(ConfigUnavailableReason::UnsupportedAgent);
    }
    let home = canonical_root(&context.home_root)?;
    let (cwd, trusted_root) = canonical_workspace(context)?;
    let mut builder = InventoryBuilder::new(context.agent);
    if context.runtime_override_present {
        builder.issue(
            None,
            ResourceScope::Unknown,
            InventoryIssueReason::DynamicSource,
        );
    }
    if context.managed_configuration_present {
        builder.issue(
            None,
            ResourceScope::Unknown,
            InventoryIssueReason::DynamicSource,
        );
    }
    match context.agent {
        AgentKind::Claude => {
            claude_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Codex => {
            codex_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::OpenCode => {
            opencode_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Pi => pi_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref()),
        AgentKind::Cursor => {
            cursor_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Copilot => {
            copilot_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Cline => {
            cline_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Kiro => {
            kiro_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::AmpCode => {
            amp_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Antigravity => {
            antigravity_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Windsurf => {
            devin_inventory(&mut builder, &home, cwd.as_deref(), trusted_root.as_deref())
        }
        AgentKind::Omp => {}
    }
    merge_indexed(&mut builder, indexed);
    Ok(builder.finish())
}

fn canonical_workspace(
    context: &ConfigContext,
) -> Result<(Option<PathBuf>, Option<PathBuf>), ConfigUnavailableReason> {
    match (&context.workspace_cwd, &context.trusted_workspace_root) {
        (None, None) => Ok((None, None)),
        (Some(cwd), Some(root)) => {
            let root = canonical_root(root)?;
            let cwd = canonical_root(cwd)?;
            if !cwd.starts_with(&root) {
                return Err(ConfigUnavailableReason::UnsafePath);
            }
            Ok((Some(cwd), Some(root)))
        }
        _ => Err(ConfigUnavailableReason::UnsafePath),
    }
}

fn scope_root<'a>(
    scope: ResourceScope,
    home: &'a Path,
    trusted_root: Option<&'a Path>,
) -> Option<&'a Path> {
    match scope {
        ResourceScope::Global => Some(home),
        ResourceScope::Project => trusted_root,
        ResourceScope::Unknown => None,
    }
}

fn optional_json(
    builder: &mut InventoryBuilder,
    path: &Path,
    root: &Path,
    scope: ResourceScope,
    strict: bool,
) -> Option<Value> {
    match path_entry_exists(path) {
        Ok(false) => None,
        Ok(true) => match read_checked(path, root).and_then(|file| {
            if strict {
                json::parse_strict(&file.bytes)
            } else {
                json::parse(&file.bytes)
            }
        }) {
            Ok(value) => Some(value),
            Err(reason) => {
                builder.issue(None, scope, InventoryIssueReason::Config(reason));
                None
            }
        },
        Err(reason) => {
            builder.issue(None, scope, InventoryIssueReason::Config(reason));
            None
        }
    }
}

fn object<'a>(
    builder: &mut InventoryBuilder,
    value: Option<&'a Value>,
    kind: ResourceKind,
    scope: ResourceScope,
) -> Option<&'a Map<String, Value>> {
    match value {
        None => None,
        Some(Value::Object(value)) => Some(value),
        Some(_) => {
            builder.issue(Some(kind), scope, InventoryIssueReason::UnsupportedShape);
            None
        }
    }
}

fn enumerate_skill_root(
    builder: &mut InventoryBuilder,
    directory: &Path,
    safety_root: &Path,
    scope: ResourceScope,
) {
    let mut directories_remaining = MAX_SKILL_DIRECTORIES;
    enumerate_skill_directory(
        builder,
        directory,
        safety_root,
        scope,
        &mut directories_remaining,
    );
}

fn enumerate_skill_directory(
    builder: &mut InventoryBuilder,
    directory: &Path,
    safety_root: &Path,
    scope: ResourceScope,
    directories_remaining: &mut usize,
) {
    if *directories_remaining == 0 {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::ResourceCapExceeded,
        );
        return;
    }
    *directories_remaining -= 1;
    let metadata = match std::fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::Config(
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        ConfigUnavailableReason::PermissionDenied
                    } else {
                        ConfigUnavailableReason::UnsafePath
                    },
                ),
            );
            return;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::Config(if metadata.file_type().is_symlink() {
                ConfigUnavailableReason::SymlinkTarget
            } else {
                ConfigUnavailableReason::NonRegularFile
            }),
        );
        return;
    }
    let Ok(canonical) = directory.canonicalize() else {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::Config(ConfigUnavailableReason::UnsafePath),
        );
        return;
    };
    if !canonical.starts_with(safety_root) {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::Config(ConfigUnavailableReason::UnsafePath),
        );
        return;
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::Config(
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        ConfigUnavailableReason::PermissionDenied
                    } else {
                        ConfigUnavailableReason::UnsafePath
                    },
                ),
            );
            return;
        }
    };
    let mut retained = BTreeMap::new();
    let mut capped = false;
    for entry in entries {
        let Ok(entry) = entry else {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::Config(ConfigUnavailableReason::UnsafePath),
            );
            continue;
        };
        let name = entry.file_name();
        retained.insert(name, entry.path());
        if retained.len() > MAX_DIRECTORY_ENTRIES {
            retained.pop_last();
            capped = true;
        }
    }
    if capped {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::ResourceCapExceeded,
        );
    }
    for (name, path) in retained {
        let Some(name) = name.to_str().map(str::to_owned) else {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::InvalidResourceName,
            );
            continue;
        };
        let skill = path.join("SKILL.md");
        match path_entry_exists(&skill) {
            Ok(true) => match read_checked(&skill, safety_root) {
                Ok(file) => builder.add_with_tokens(
                    ResourceKind::Skill,
                    &name,
                    EnabledState::Enabled,
                    scope,
                    ResourceProvenance::StandardDirectory,
                    skill_listing_tokens(&file.bytes),
                ),
                Err(reason) => builder.issue(
                    Some(ResourceKind::Skill),
                    scope,
                    InventoryIssueReason::Config(reason),
                ),
            },
            Ok(false) => {
                // Cursor supports category directories below a skill root.
                if matches!(
                    std::fs::symlink_metadata(&path),
                    Ok(metadata) if metadata.is_dir()
                ) {
                    enumerate_skill_directory(
                        builder,
                        &path,
                        safety_root,
                        scope,
                        directories_remaining,
                    );
                }
            }
            Err(reason) => builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::Config(reason),
            ),
        }
    }
}

fn skill_listing_tokens(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let frontmatter = text.strip_prefix("---\n")?.split_once("\n---")?.0;
    let mut title = None;
    let mut name = None;
    let mut description = None;
    for line in frontmatter.lines() {
        if line.starts_with(char::is_whitespace) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = yaml_plain_scalar(value.trim())?;
        match key.trim() {
            "title" => title = Some(value),
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }
    let title = title.or(name)?.trim().to_owned();
    let description = description?.trim().to_owned();
    if title.is_empty() || description.is_empty() {
        return None;
    }
    Some(antiburn_local::analysis::estimate_proportional_tokens(
        &format!("- {title}: {description}\n"),
    ))
}

fn yaml_plain_scalar(value: &str) -> Option<String> {
    if value.is_empty() || matches!(value, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
        return None;
    }
    if value.starts_with('"') {
        return serde_json::from_str(value).ok();
    }
    if let Some(value) = value.strip_prefix('\'') {
        let value = value.strip_suffix('\'')?;
        return Some(value.replace("''", "'"));
    }
    Some(value.split(" #").next().unwrap_or(value).trim().to_owned())
}

fn claude_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    trusted_root: Option<&Path>,
) {
    if path_entry_exists(&home.join(".claude/plugins")).unwrap_or(true) {
        builder.issue(
            None,
            ResourceScope::Global,
            InventoryIssueReason::DynamicSource,
        );
    }
    if let Some(document) = optional_json(
        builder,
        &home.join(".claude.json"),
        home,
        ResourceScope::Global,
        true,
    ) {
        add_json_mcp_map(
            builder,
            document.get("mcpServers"),
            ResourceScope::Global,
            ResourceProvenance::StandardConfig,
        );
        if let (Some(cwd), Some(projects)) =
            (cwd, document.get("projects").and_then(Value::as_object))
            && let Some(project) = cwd.to_str().and_then(|cwd| projects.get(cwd))
        {
            add_json_mcp_map(
                builder,
                project.get("mcpServers"),
                ResourceScope::Project,
                ResourceProvenance::StandardConfig,
            );
        }
    }
    if let Some(root) = trusted_root
        && let Some(document) = optional_json(
            builder,
            &root.join(".mcp.json"),
            root,
            ResourceScope::Project,
            true,
        )
    {
        add_json_mcp_map(
            builder,
            document.get("mcpServers"),
            ResourceScope::Project,
            ResourceProvenance::StandardConfig,
        );
    }
    let mut settings = Vec::new();
    if let Some(document) = optional_json(
        builder,
        &home.join(".claude/settings.json"),
        home,
        ResourceScope::Global,
        true,
    ) {
        settings.push((ResourceScope::Global, document));
    }
    if let Some(root) = trusted_root {
        for relative in [".claude/settings.json", ".claude/settings.local.json"] {
            if let Some(document) = optional_json(
                builder,
                &root.join(relative),
                root,
                ResourceScope::Project,
                true,
            ) {
                settings.push((ResourceScope::Project, document));
            }
        }
    }
    for (scope, document) in &settings {
        add_claude_controls(builder, document, *scope);
    }
    enumerate_skill_root(
        builder,
        &home.join(".claude/skills"),
        home,
        ResourceScope::Global,
    );
    if let (Some(cwd), Some(root)) = (cwd, trusted_root) {
        for directory in hierarchy(cwd, root) {
            enumerate_skill_root(
                builder,
                &directory.join(".claude/skills"),
                root,
                ResourceScope::Project,
            );
        }
    }
    apply_claude_skill_overrides(builder, &settings);
}

fn add_json_mcp_map(
    builder: &mut InventoryBuilder,
    value: Option<&Value>,
    scope: ResourceScope,
    provenance: ResourceProvenance,
) {
    let Some(servers) = object(builder, value, ResourceKind::McpServer, scope) else {
        return;
    };
    for (name, definition) in servers {
        let Some(definition) = definition.as_object() else {
            builder.issue(
                Some(ResourceKind::McpServer),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
            continue;
        };
        if definition.contains_key("url") || definition.contains_key("serverUrl") {
            builder.issue(
                Some(ResourceKind::McpServer),
                scope,
                InventoryIssueReason::DynamicSource,
            );
        }
        let enabled = definition.get("enabled").map(Value::as_bool);
        let disabled = definition.get("disabled").map(Value::as_bool);
        let enabled_state = if enabled.is_some_and(|value| value.is_none())
            || disabled.is_some_and(|value| value.is_none())
        {
            builder.issue(
                Some(ResourceKind::McpServer),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
            EnabledState::Unknown
        } else {
            match (enabled.flatten(), disabled.flatten()) {
                (Some(enabled), Some(disabled)) if enabled == disabled => {
                    builder.issue(
                        Some(ResourceKind::McpServer),
                        scope,
                        InventoryIssueReason::ConflictingDefinition,
                    );
                    EnabledState::Unknown
                }
                (Some(enabled), Some(_)) => enabled_state_from_enabled(enabled),
                (Some(enabled), None) => enabled_state_from_enabled(enabled),
                (None, Some(disabled)) => enabled_state_from_enabled(!disabled),
                (None, None) => EnabledState::Enabled,
            }
        };
        builder.replace(
            ResourceKind::McpServer,
            name,
            enabled_state,
            scope,
            provenance,
        );
    }
}

fn enabled_state_from_enabled(enabled: bool) -> EnabledState {
    if enabled {
        EnabledState::Enabled
    } else {
        EnabledState::Disabled
    }
}

fn add_json_mcp_document(
    builder: &mut InventoryBuilder,
    document: &Value,
    scope: ResourceScope,
    provenance: ResourceProvenance,
) {
    let map = document
        .get("mcpServers")
        .or_else(|| document.get("amp.mcpServers"));
    if map.is_none()
        && document
            .as_object()
            .is_some_and(|document| document.keys().any(|key| key.starts_with("amp.")))
    {
        return;
    }
    add_json_mcp_map(builder, map.or(Some(document)), scope, provenance);
}

fn inventory_json_file(
    builder: &mut InventoryBuilder,
    path: &Path,
    root: &Path,
    scope: ResourceScope,
) {
    if let Some(document) = optional_json(builder, path, root, scope, true) {
        add_json_mcp_document(
            builder,
            &document,
            scope,
            ResourceProvenance::StandardConfig,
        );
    }
}

fn inventory_github_mcp_file(
    builder: &mut InventoryBuilder,
    path: &Path,
    root: &Path,
    scope: ResourceScope,
) {
    if let Some(document) = optional_json(builder, path, root, scope, true) {
        add_json_mcp_map(
            builder,
            document.get("servers"),
            scope,
            ResourceProvenance::StandardConfig,
        );
    }
}

fn inventory_project_roots(
    cwd: Option<&Path>,
    root: Option<&Path>,
) -> impl Iterator<Item = PathBuf> {
    cwd.into_iter()
        .zip(root)
        .flat_map(|(cwd, root)| hierarchy(cwd, root))
}

fn inventory_skills(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
    global: &[&str],
    project: &[&str],
) {
    let global_root = home.to_owned();
    for relative in global {
        enumerate_skill_root(
            builder,
            &home.join(relative),
            &global_root,
            ResourceScope::Global,
        );
    }
    if let (Some(cwd), Some(root)) = (cwd, root) {
        for directory in hierarchy(cwd, root) {
            for relative in project {
                enumerate_skill_root(
                    builder,
                    &directory.join(relative),
                    root,
                    ResourceScope::Project,
                );
            }
        }
    }
}

fn cursor_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
) {
    inventory_json_file(
        builder,
        &home.join(".cursor/mcp.json"),
        home,
        ResourceScope::Global,
    );
    for directory in inventory_project_roots(cwd, root) {
        inventory_json_file(
            builder,
            &directory.join(".cursor/mcp.json"),
            root.unwrap_or(directory.as_path()),
            ResourceScope::Project,
        );
    }
    inventory_skills(
        builder,
        home,
        cwd,
        root,
        &[
            ".agents/skills",
            ".cursor/skills",
            ".claude/skills",
            ".codex/skills",
        ],
        &[
            ".agents/skills",
            ".cursor/skills",
            ".claude/skills",
            ".codex/skills",
        ],
    );
}

fn copilot_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
) {
    inventory_json_file(
        builder,
        &home.join(".copilot/mcp-config.json"),
        home,
        ResourceScope::Global,
    );
    if let (Some(cwd), Some(root)) = (cwd, root) {
        for directory in hierarchy(cwd, root) {
            inventory_github_mcp_file(
                builder,
                &directory.join(".github/mcp.json"),
                root,
                ResourceScope::Project,
            );
            inventory_json_file(
                builder,
                &directory.join(".mcp.json"),
                root,
                ResourceScope::Project,
            );
        }
    }
    inventory_skills(
        builder,
        home,
        cwd,
        root,
        &[".agents/skills", ".copilot/skills"],
        &[".agents/skills", ".github/skills", ".claude/skills"],
    );
}

fn cline_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
) {
    for relative in [
        ".cline/mcp.json",
        ".cline/data/settings/cline_mcp_settings.json",
    ] {
        inventory_json_file(builder, &home.join(relative), home, ResourceScope::Global);
    }
    if let (Some(cwd), Some(root)) = (cwd, root) {
        for directory in hierarchy(cwd, root) {
            inventory_json_file(
                builder,
                &directory.join(".cline/mcp.json"),
                root,
                ResourceScope::Project,
            );
        }
    }
    inventory_skills(
        builder,
        home,
        cwd,
        root,
        &[".cline/skills"],
        &[".cline/skills", ".clinerules/skills"],
    );
}

fn kiro_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
) {
    inventory_json_file(
        builder,
        &home.join(".kiro/settings/mcp.json"),
        home,
        ResourceScope::Global,
    );
    if let Some(root) = root {
        inventory_json_file(
            builder,
            &root.join(".kiro/settings/mcp.json"),
            root,
            ResourceScope::Project,
        );
    }
    inventory_skills(
        builder,
        home,
        cwd,
        root,
        &[".kiro/skills"],
        &[".kiro/skills"],
    );
}

fn amp_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
) {
    inventory_json_file(
        builder,
        &home.join(".config/amp/settings.json"),
        home,
        ResourceScope::Global,
    );
    if let Some(root) = root {
        inventory_json_file(
            builder,
            &root.join(".amp/settings.json"),
            root,
            ResourceScope::Project,
        );
    }
    inventory_skills(
        builder,
        home,
        cwd,
        root,
        &[
            ".config/agents/skills",
            ".agents/skills",
            ".config/amp/skills",
            ".claude/skills",
        ],
        &[".agents/skills", ".claude/skills"],
    );
}

fn antigravity_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
) {
    inventory_json_file(
        builder,
        &home.join(".gemini/config/mcp_config.json"),
        home,
        ResourceScope::Global,
    );
    if let Some(root) = root {
        inventory_json_file(
            builder,
            &root.join(".agents/mcp_config.json"),
            root,
            ResourceScope::Project,
        );
    }
    inventory_skills(
        builder,
        home,
        cwd,
        root,
        &[
            ".agents/skills",
            ".gemini/config/skills",
            ".gemini/antigravity-cli/skills",
        ],
        &[".agents/skills"],
    );
}

fn devin_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    root: Option<&Path>,
) {
    inventory_json_file(
        builder,
        &home.join(".config/devin/mcp_config.json"),
        home,
        ResourceScope::Global,
    );
    if let Some(root) = root {
        for relative in [".devin/mcp_config.json", ".devin/mcp_config.local.json"] {
            inventory_json_file(builder, &root.join(relative), root, ResourceScope::Project);
        }
    }
    inventory_skills(
        builder,
        home,
        cwd,
        root,
        &[
            ".agents/skills",
            ".config/devin/skills",
            ".codeium/windsurf/skills",
        ],
        &[".agents/skills", ".devin/skills", ".windsurf/skills"],
    );
}

fn add_claude_controls(builder: &mut InventoryBuilder, document: &Value, scope: ResourceScope) {
    let Some(permissions) = document.get("permissions").and_then(Value::as_object) else {
        return;
    };
    for (key, enabled) in [("allow", true), ("deny", false)] {
        let Some(values) = permissions.get(key) else {
            continue;
        };
        let Some(values) = values.as_array() else {
            builder.issue(
                Some(ResourceKind::BuiltInTool),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
            continue;
        };
        for value in values {
            let Some(name) = value.as_str() else {
                builder.issue(
                    Some(ResourceKind::BuiltInTool),
                    scope,
                    InventoryIssueReason::UnsupportedShape,
                );
                continue;
            };
            if let Some(server) = name
                .strip_prefix("mcp__")
                .and_then(|name| name.strip_suffix("__*"))
            {
                builder.set_state(
                    ResourceKind::McpServer,
                    server,
                    if enabled {
                        EnabledState::Enabled
                    } else {
                        EnabledState::Disabled
                    },
                    scope,
                    ResourceProvenance::StandardConfig,
                );
            } else if name.starts_with("mcp__") {
                continue;
            } else if name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                builder.set_state(
                    ResourceKind::BuiltInTool,
                    name,
                    if enabled {
                        EnabledState::Enabled
                    } else {
                        EnabledState::Disabled
                    },
                    scope,
                    ResourceProvenance::StandardConfig,
                );
            }
        }
    }
}

fn apply_claude_skill_overrides(
    builder: &mut InventoryBuilder,
    settings: &[(ResourceScope, Value)],
) {
    for (scope, document) in settings {
        let Some(overrides) = document.get("skillOverrides") else {
            continue;
        };
        let Some(overrides) = overrides.as_object() else {
            builder.issue(
                Some(ResourceKind::Skill),
                *scope,
                InventoryIssueReason::UnsupportedShape,
            );
            continue;
        };
        for (name, value) in overrides {
            let enabled = match value {
                Value::Bool(value) => Some(*value),
                Value::String(value) if value == "off" => Some(false),
                Value::String(value)
                    if matches!(value.as_str(), "on" | "name-only" | "user-invocable-only") =>
                {
                    Some(true)
                }
                _ => None,
            };
            if let Some(enabled) = enabled {
                builder.set_state(
                    ResourceKind::Skill,
                    name,
                    if enabled {
                        EnabledState::Enabled
                    } else {
                        EnabledState::Disabled
                    },
                    *scope,
                    ResourceProvenance::StandardConfig,
                );
            } else {
                builder.issue(
                    Some(ResourceKind::Skill),
                    *scope,
                    InventoryIssueReason::UnsupportedShape,
                );
            }
        }
    }
}

fn codex_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    trusted_root: Option<&Path>,
) {
    let global_path = home.join(".codex/config.toml");
    let global = optional_toml(builder, &global_path, home, ResourceScope::Global);
    if let Some(document) = global.as_ref() {
        add_codex_mcp(builder, document, ResourceScope::Global);
        add_codex_skill_controls(builder, document, ResourceScope::Global);
        if document.get("profile").is_some() {
            builder.issue(
                None,
                ResourceScope::Global,
                InventoryIssueReason::DynamicSource,
            );
        }
    }
    enumerate_skill_root(
        builder,
        &home.join(".agents/skills"),
        home,
        ResourceScope::Global,
    );
    enumerate_skill_root(
        builder,
        &home.join(".codex/skills"),
        home,
        ResourceScope::Global,
    );
    if let (Some(cwd), Some(root)) = (cwd, trusted_root) {
        let trusted = codex::project_is_trusted(&global_path, home, root).unwrap_or(false);
        if trusted {
            for directory in hierarchy(cwd, root) {
                if let Some(document) = optional_toml(
                    builder,
                    &directory.join(".codex/config.toml"),
                    root,
                    ResourceScope::Project,
                ) {
                    add_codex_mcp(builder, &document, ResourceScope::Project);
                    add_codex_skill_controls(builder, &document, ResourceScope::Project);
                }
                for relative in [".agents/skills", ".codex/skills"] {
                    enumerate_skill_root(
                        builder,
                        &directory.join(relative),
                        root,
                        ResourceScope::Project,
                    );
                }
            }
        } else {
            builder.issue(
                None,
                ResourceScope::Project,
                InventoryIssueReason::DynamicSource,
            );
        }
    }
}

fn add_codex_skill_controls(
    builder: &mut InventoryBuilder,
    document: &toml_edit::DocumentMut,
    scope: ResourceScope,
) {
    let Some(config) = document
        .get("skills")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|skills| skills.get("config"))
    else {
        return;
    };
    if let Some(skills) = config.as_table_like() {
        for (name, skill) in skills.iter() {
            let enabled = skill
                .as_table_like()
                .and_then(|skill| skill.get("enabled"))
                .and_then(toml_edit::Item::as_bool);
            if let Some(enabled) = enabled {
                builder.set_state(
                    ResourceKind::Skill,
                    name,
                    if enabled {
                        EnabledState::Enabled
                    } else {
                        EnabledState::Disabled
                    },
                    scope,
                    ResourceProvenance::StandardConfig,
                );
            }
        }
        return;
    }
    let Some(skills) = config.as_array_of_tables() else {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::UnsupportedShape,
        );
        return;
    };
    for skill in skills.iter() {
        let (Some(path), Some(enabled)) = (
            skill.get("path").and_then(toml_edit::Item::as_str),
            skill.get("enabled").and_then(toml_edit::Item::as_bool),
        ) else {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
            continue;
        };
        let Some(name) = Path::new(path)
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        else {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::InvalidResourceName,
            );
            continue;
        };
        builder.set_state(
            ResourceKind::Skill,
            name,
            if enabled {
                EnabledState::Enabled
            } else {
                EnabledState::Disabled
            },
            scope,
            ResourceProvenance::StandardConfig,
        );
    }
}

fn optional_toml(
    builder: &mut InventoryBuilder,
    path: &Path,
    root: &Path,
    scope: ResourceScope,
) -> Option<toml_edit::DocumentMut> {
    match path_entry_exists(path) {
        Ok(false) => None,
        Ok(true) => {
            match read_checked(path, root).and_then(|file| codex::parse_document(&file.bytes)) {
                Ok(value) => Some(value),
                Err(reason) => {
                    builder.issue(None, scope, InventoryIssueReason::Config(reason));
                    None
                }
            }
        }
        Err(reason) => {
            builder.issue(None, scope, InventoryIssueReason::Config(reason));
            None
        }
    }
}

fn add_codex_mcp(
    builder: &mut InventoryBuilder,
    document: &toml_edit::DocumentMut,
    scope: ResourceScope,
) {
    let Some(servers) = document
        .get("mcp_servers")
        .and_then(toml_edit::Item::as_table_like)
    else {
        return;
    };
    for (name, definition) in servers.iter() {
        let Some(definition) = definition.as_table_like() else {
            builder.issue(
                Some(ResourceKind::McpServer),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
            continue;
        };
        let enabled = match definition.get("enabled") {
            None => EnabledState::Enabled,
            Some(value) => match value.as_bool() {
                Some(true) => EnabledState::Enabled,
                Some(false) => EnabledState::Disabled,
                None => {
                    builder.issue(
                        Some(ResourceKind::McpServer),
                        scope,
                        InventoryIssueReason::UnsupportedShape,
                    );
                    EnabledState::Unknown
                }
            },
        };
        builder.replace(
            ResourceKind::McpServer,
            name,
            enabled,
            scope,
            ResourceProvenance::StandardConfig,
        );
    }
}

fn opencode_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    trusted_root: Option<&Path>,
) {
    let global_root = match opencode::global_config_root(home) {
        Ok(root) => root,
        Err(reason) => {
            builder.issue(
                None,
                ResourceScope::Global,
                InventoryIssueReason::Config(reason),
            );
            home.join(".config/opencode")
        }
    };
    let mut layers = vec![(
        global_root.clone(),
        global_root.clone(),
        ResourceScope::Global,
        true,
    )];
    if let (Some(cwd), Some(root)) = (cwd, trusted_root) {
        for directory in opencode::project_hierarchy(cwd, root).unwrap_or_default() {
            layers.push((
                directory.clone(),
                root.to_owned(),
                ResourceScope::Project,
                false,
            ));
        }
        for directory in opencode::project_hierarchy(cwd, root)
            .unwrap_or_default()
            .into_iter()
            .rev()
        {
            layers.push((
                directory.join(".opencode"),
                root.to_owned(),
                ResourceScope::Project,
                false,
            ));
        }
    }
    layers.push((
        home.join(".opencode"),
        home.to_owned(),
        ResourceScope::Global,
        false,
    ));
    for (directory, safety_root, scope, include_legacy) in layers {
        let names: &[&str] = if include_legacy {
            &["config.json", "opencode.json", "opencode.jsonc"]
        } else {
            &["opencode.json", "opencode.jsonc"]
        };
        for name in names {
            if let Some(document) =
                optional_json(builder, &directory.join(name), &safety_root, scope, false)
            {
                add_opencode_document(builder, &document, scope);
            }
        }
    }
    for name in [
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_AUTH_CONTENT",
        "OPENCODE_DB",
        "OPENCODE_DATA_DIR",
    ] {
        if std::env::var_os(name).is_some() {
            builder.issue(
                None,
                ResourceScope::Unknown,
                InventoryIssueReason::DynamicSource,
            );
        }
    }
    enumerate_skill_root(
        builder,
        &global_root.join("skills"),
        &global_root,
        ResourceScope::Global,
    );
    enumerate_skill_root(
        builder,
        &home.join(".opencode/skills"),
        home,
        ResourceScope::Global,
    );
    enumerate_skill_root(
        builder,
        &home.join(".claude/skills"),
        home,
        ResourceScope::Global,
    );
    enumerate_skill_root(
        builder,
        &home.join(".agents/skills"),
        home,
        ResourceScope::Global,
    );
    if let (Some(cwd), Some(root)) = (cwd, trusted_root) {
        for directory in hierarchy(cwd, root) {
            for relative in [".opencode/skills", ".claude/skills", ".agents/skills"] {
                enumerate_skill_root(
                    builder,
                    &directory.join(relative),
                    root,
                    ResourceScope::Project,
                );
            }
        }
    }
}

fn add_opencode_document(builder: &mut InventoryBuilder, document: &Value, scope: ResourceScope) {
    if document.get("agent").is_some()
        || document.get("mode").is_some()
        || document.get("plugin").is_some()
    {
        builder.issue(None, scope, InventoryIssueReason::DynamicSource);
    }
    let mcp = document.get("mcp");
    let mcp = mcp
        .and_then(|mcp| {
            let object = mcp.as_object()?;
            let servers = object.get("servers")?;
            let direct_server = servers.as_object().is_some_and(|server| {
                ["command", "type", "url", "enabled", "environment"]
                    .iter()
                    .any(|key| server.contains_key(*key))
            });
            (!direct_server).then_some(servers)
        })
        .or(mcp);
    add_json_mcp_map(builder, mcp, scope, ResourceProvenance::StandardConfig);
    if let Some(tools) = document.get("tools") {
        if let Some(tools) = tools.as_object() {
            for (name, value) in tools {
                match value.as_bool() {
                    Some(enabled) => builder.replace(
                        ResourceKind::BuiltInTool,
                        name,
                        if enabled {
                            EnabledState::Enabled
                        } else {
                            EnabledState::Disabled
                        },
                        scope,
                        ResourceProvenance::StandardConfig,
                    ),
                    None => builder.issue(
                        Some(ResourceKind::BuiltInTool),
                        scope,
                        InventoryIssueReason::UnsupportedShape,
                    ),
                }
            }
        } else {
            builder.issue(
                Some(ResourceKind::BuiltInTool),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
        }
    }
    for key in ["permission", "permissions"] {
        let Some(permission) = document.get(key) else {
            continue;
        };
        if let Some(permission) = permission.as_object() {
            for (name, value) in permission {
                if name.contains('*') {
                    builder.issue(None, scope, InventoryIssueReason::DynamicSource);
                    continue;
                }
                let enabled = match value.as_str() {
                    Some("deny") => EnabledState::Disabled,
                    Some("allow" | "ask") => EnabledState::Enabled,
                    _ => EnabledState::Unknown,
                };
                if name == "skill" || name.starts_with("skill_") {
                    continue;
                }
                builder.replace(
                    ResourceKind::BuiltInTool,
                    name,
                    enabled,
                    scope,
                    ResourceProvenance::StandardConfig,
                );
            }
        } else if let Some(rules) = permission.as_array() {
            for rule in rules {
                let Some(rule) = rule.as_object() else {
                    builder.issue(None, scope, InventoryIssueReason::UnsupportedShape);
                    continue;
                };
                let (Some(action), Some(effect)) = (
                    rule.get("action").and_then(Value::as_str),
                    rule.get("effect").and_then(Value::as_str),
                ) else {
                    builder.issue(None, scope, InventoryIssueReason::UnsupportedShape);
                    continue;
                };
                if action == "skill" {
                    if let Some(name) = rule.get("resource").and_then(Value::as_str)
                        && !name.contains('*')
                    {
                        builder.replace(
                            ResourceKind::Skill,
                            name,
                            if effect == "deny" {
                                EnabledState::Disabled
                            } else {
                                EnabledState::Enabled
                            },
                            scope,
                            ResourceProvenance::StandardConfig,
                        );
                    }
                } else {
                    builder.replace(
                        ResourceKind::BuiltInTool,
                        action,
                        if effect == "deny" {
                            EnabledState::Disabled
                        } else {
                            EnabledState::Enabled
                        },
                        scope,
                        ResourceProvenance::StandardConfig,
                    );
                }
            }
        } else {
            builder.issue(None, scope, InventoryIssueReason::UnsupportedShape);
        }
    }
}

fn pi_inventory(
    builder: &mut InventoryBuilder,
    home: &Path,
    cwd: Option<&Path>,
    trusted_root: Option<&Path>,
) {
    let global_root = match pi::global_root(home) {
        Ok(root) => root,
        Err(reason) => {
            builder.issue(
                None,
                ResourceScope::Global,
                InventoryIssueReason::Config(reason),
            );
            home.join(".pi/agent")
        }
    };
    let global = optional_json(
        builder,
        &global_root.join("settings.json"),
        &global_root,
        ResourceScope::Global,
        true,
    );
    let project = match (cwd, trusted_root) {
        (Some(cwd), Some(root)) => optional_json(
            builder,
            &cwd.join(".pi/settings.json"),
            root,
            ResourceScope::Project,
            true,
        ),
        _ => None,
    };
    let tools = project
        .as_ref()
        .and_then(|document| document.get("defaultTools"))
        .map(|value| (value, ResourceScope::Project))
        .or_else(|| {
            global
                .as_ref()
                .and_then(|document| document.get("defaultTools"))
                .map(|value| (value, ResourceScope::Global))
        });
    if let Some((tools, scope)) = tools {
        add_string_array(builder, tools, ResourceKind::BuiltInTool, scope);
    } else {
        for tool in PI_DEFAULT_TOOLS {
            builder.add(
                ResourceKind::BuiltInTool,
                tool,
                EnabledState::Enabled,
                ResourceScope::Global,
                ResourceProvenance::BuiltInCatalog,
            );
        }
    }
    enumerate_skill_root(
        builder,
        &global_root.join("skills"),
        &global_root,
        ResourceScope::Global,
    );
    enumerate_skill_root(
        builder,
        &home.join(".agents/skills"),
        home,
        ResourceScope::Global,
    );
    if let (Some(cwd), Some(root)) = (cwd, trusted_root) {
        enumerate_skill_root(
            builder,
            &cwd.join(".pi/skills"),
            root,
            ResourceScope::Project,
        );
        for directory in hierarchy(cwd, root) {
            enumerate_skill_root(
                builder,
                &directory.join(".agents/skills"),
                root,
                ResourceScope::Project,
            );
        }
    }
    add_pi_configured_skill_paths(
        builder,
        global.as_ref(),
        &global_root,
        ResourceScope::Global,
    );
    if let (Some(document), Some(root)) = (project.as_ref(), trusted_root) {
        add_pi_configured_skill_paths(builder, Some(document), root, ResourceScope::Project);
    }
    let package_scope = if project
        .as_ref()
        .is_some_and(|document| document.get("packages").is_some())
    {
        if project.as_ref().is_some_and(pi_mcp_package_selected) {
            ResourceScope::Project
        } else {
            return;
        }
    } else if global.as_ref().is_some_and(pi_mcp_package_selected) {
        ResourceScope::Global
    } else {
        return;
    };
    let package_root = match package_scope {
        ResourceScope::Project => cwd.map(|cwd| cwd.join(".pi/npm/node_modules/pi-mcp-extension")),
        ResourceScope::Global => Some(global_root.join("npm/node_modules/pi-mcp-extension")),
        ResourceScope::Unknown => None,
    };
    let Some(package_root) = package_root else {
        return;
    };
    let safety_root = scope_root(package_scope, &global_root, trusted_root).unwrap_or(&global_root);
    if !pi_mcp_manifest_matches(builder, &package_root, safety_root, package_scope) {
        return;
    }
    let global_mcp = optional_json(
        builder,
        &global_root.join("mcp.json"),
        &global_root,
        ResourceScope::Global,
        true,
    );
    if let Some(document) = global_mcp.as_ref() {
        add_pi_mcp(builder, document, ResourceScope::Global);
    }
    if let (Some(cwd), Some(root)) = (cwd, trusted_root)
        && let Some(document) = optional_json(
            builder,
            &cwd.join(".pi/mcp.json"),
            root,
            ResourceScope::Project,
            true,
        )
    {
        add_pi_mcp(builder, &document, ResourceScope::Project);
    }
}

fn add_string_array(
    builder: &mut InventoryBuilder,
    value: &Value,
    kind: ResourceKind,
    scope: ResourceScope,
) {
    let Some(values) = value.as_array() else {
        builder.issue(Some(kind), scope, InventoryIssueReason::UnsupportedShape);
        return;
    };
    let mut seen = BTreeSet::new();
    for value in values {
        let Some(name) = value.as_str() else {
            builder.issue(Some(kind), scope, InventoryIssueReason::UnsupportedShape);
            continue;
        };
        if !seen.insert(name) {
            builder.issue(
                Some(kind),
                scope,
                InventoryIssueReason::ConflictingDefinition,
            );
            continue;
        }
        builder.add(
            kind,
            name,
            EnabledState::Enabled,
            scope,
            ResourceProvenance::StandardConfig,
        );
    }
}

fn add_pi_configured_skill_paths(
    builder: &mut InventoryBuilder,
    document: Option<&Value>,
    safety_root: &Path,
    scope: ResourceScope,
) {
    let Some(paths) = document.and_then(|document| document.get("skills")) else {
        return;
    };
    let Some(paths) = paths.as_array() else {
        builder.issue(
            Some(ResourceKind::Skill),
            scope,
            InventoryIssueReason::UnsupportedShape,
        );
        return;
    };
    for path in paths {
        let Some(path) = path.as_str() else {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
            continue;
        };
        if path.contains('*') || path.starts_with('!') || path.starts_with(['+', '-']) {
            builder.issue(
                Some(ResourceKind::Skill),
                scope,
                InventoryIssueReason::DynamicSource,
            );
            continue;
        }
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            safety_root.join(path)
        };
        enumerate_skill_root(builder, &path, safety_root, scope);
    }
}

fn pi_mcp_package_selected(document: &Value) -> bool {
    document
        .get("packages")
        .and_then(Value::as_array)
        .is_some_and(|packages| {
            packages.iter().any(|package| match package {
                Value::String(value) => {
                    matches!(
                        value.as_str(),
                        "npm:pi-mcp-extension@1.5.0" | "pi-mcp-extension@1.5.0"
                    )
                }
                Value::Object(value) => {
                    value
                        .get("source")
                        .and_then(Value::as_str)
                        .is_some_and(|value| {
                            matches!(
                                value,
                                "npm:pi-mcp-extension@1.5.0" | "pi-mcp-extension@1.5.0"
                            )
                        })
                        && value.get("autoload").and_then(Value::as_bool) != Some(false)
                }
                _ => false,
            })
        })
}

fn pi_mcp_manifest_matches(
    builder: &mut InventoryBuilder,
    package_root: &Path,
    safety_root: &Path,
    scope: ResourceScope,
) -> bool {
    let Some(manifest) = optional_json(
        builder,
        &package_root.join("package.json"),
        safety_root,
        scope,
        true,
    ) else {
        return false;
    };
    let matches = manifest.get("name").and_then(Value::as_str) == Some(PI_MCP_PACKAGE)
        && manifest.get("version").and_then(Value::as_str) == Some(PI_MCP_VERSION)
        && manifest
            .get("pi")
            .and_then(Value::as_object)
            .and_then(|pi| pi.get("extensions"))
            .and_then(Value::as_array)
            .is_some_and(|extensions| {
                extensions.len() == 1 && extensions[0].as_str() == Some(PI_MCP_ENTRY)
            });
    if !matches {
        builder.issue(
            Some(ResourceKind::McpServer),
            scope,
            InventoryIssueReason::UnsupportedShape,
        );
    }
    matches
}

fn add_pi_mcp(builder: &mut InventoryBuilder, document: &Value, scope: ResourceScope) {
    let Some(servers) = object(
        builder,
        document.get("mcpServers"),
        ResourceKind::McpServer,
        scope,
    ) else {
        return;
    };
    for (name, definition) in servers {
        let Some(definition) = definition.as_object() else {
            builder.issue(
                Some(ResourceKind::McpServer),
                scope,
                InventoryIssueReason::UnsupportedShape,
            );
            continue;
        };
        let eager = definition.get("lifecycle").and_then(Value::as_str) == Some("eager");
        if !eager {
            builder.issue(
                Some(ResourceKind::McpServer),
                scope,
                InventoryIssueReason::DynamicSource,
            );
        }
        builder.replace(
            ResourceKind::McpServer,
            name,
            if eager {
                EnabledState::Enabled
            } else {
                EnabledState::Unknown
            },
            scope,
            ResourceProvenance::PiMcpExtension,
        );
    }
}

fn hierarchy(cwd: &Path, root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![cwd.to_owned()];
    let mut current = cwd;
    while current != root {
        let Some(parent) = current.parent() else {
            return Vec::new();
        };
        if !parent.starts_with(root) {
            return Vec::new();
        }
        directories.push(parent.to_owned());
        current = parent;
    }
    directories.reverse();
    directories
}

fn merge_indexed<'a>(
    builder: &mut InventoryBuilder,
    indexed: impl IntoIterator<Item = IndexedResourceEvidence<'a>>,
) {
    for indexed in indexed {
        if !evidence_agent_matches(builder.agent, &indexed.evidence.identity.agent) {
            continue;
        }
        let sources = match &indexed.evidence.context_sources {
            EvidenceValue::Unsupported => {
                for kind in [
                    ResourceKind::McpServer,
                    ResourceKind::BuiltInTool,
                    ResourceKind::Skill,
                ] {
                    builder.issue(
                        Some(kind),
                        indexed.scope,
                        InventoryIssueReason::UnsupportedShape,
                    );
                }
                continue;
            }
            EvidenceValue::Partial { observed, reason } => {
                for kind in [
                    ResourceKind::McpServer,
                    ResourceKind::BuiltInTool,
                    ResourceKind::Skill,
                ] {
                    builder.issue(
                        Some(kind),
                        indexed.scope,
                        InventoryIssueReason::PartialIndexedEvidence(*reason),
                    );
                }
                observed
            }
            EvidenceValue::Complete(observed) => observed,
        };
        indexed_coverage_issue(
            builder,
            ResourceKind::McpServer,
            indexed.scope,
            &sources.mcp_coverage,
        );
        indexed_coverage_issue(
            builder,
            ResourceKind::Skill,
            indexed.scope,
            &sources.skill_coverage,
        );
        for (name, source) in &sources.mcp_servers {
            builder.add(
                ResourceKind::McpServer,
                name,
                EnabledState::Unknown,
                indexed_scope(&source.origin, indexed.scope),
                ResourceProvenance::IndexedSession,
            );
        }
        for (name, source) in &sources.skills {
            builder.add(
                ResourceKind::Skill,
                name,
                EnabledState::Unknown,
                indexed_scope(&source.origin, indexed.scope),
                ResourceProvenance::IndexedSession,
            );
        }
        match &sources.tool_definitions {
            EvidenceValue::Unsupported => {}
            EvidenceValue::Partial { observed, reason } => {
                builder.issue(
                    Some(ResourceKind::BuiltInTool),
                    indexed.scope,
                    InventoryIssueReason::PartialIndexedEvidence(*reason),
                );
                add_indexed_tools(builder, observed, indexed.scope);
            }
            EvidenceValue::Complete(observed) => {
                add_indexed_tools(builder, observed, indexed.scope);
            }
        }
    }
}

fn evidence_agent_matches(agent: AgentKind, evidence_agent: &str) -> bool {
    match agent {
        AgentKind::Claude => matches!(evidence_agent, "claude" | "claude-code"),
        AgentKind::Codex => evidence_agent == "codex",
        AgentKind::OpenCode => evidence_agent == "opencode",
        AgentKind::Pi => evidence_agent == "pi",
        AgentKind::Omp => evidence_agent == "omp",
        AgentKind::Cursor => matches!(evidence_agent, "cursor" | "cursor-ide"),
        AgentKind::Copilot => matches!(evidence_agent, "copilot" | "github-copilot"),
        AgentKind::Cline => evidence_agent == "cline",
        AgentKind::Kiro => matches!(evidence_agent, "kiro" | "kiro-cli"),
        AgentKind::AmpCode => matches!(evidence_agent, "amp" | "amp-code"),
        AgentKind::Antigravity => evidence_agent == "antigravity",
        AgentKind::Windsurf => matches!(evidence_agent, "windsurf" | "devin"),
    }
}

fn indexed_coverage_issue(
    builder: &mut InventoryBuilder,
    kind: ResourceKind,
    scope: ResourceScope,
    coverage: &EvidenceValue<()>,
) {
    match coverage {
        EvidenceValue::Unsupported => {
            builder.issue(Some(kind), scope, InventoryIssueReason::UnsupportedShape)
        }
        EvidenceValue::Partial { reason, .. } => builder.issue(
            Some(kind),
            scope,
            InventoryIssueReason::PartialIndexedEvidence(*reason),
        ),
        EvidenceValue::Complete(()) => {}
    }
}

fn indexed_scope(origin: &EvidenceValue<SourceOrigin>, _fallback: ResourceScope) -> ResourceScope {
    let origin = match origin {
        EvidenceValue::Complete(origin)
        | EvidenceValue::Partial {
            observed: origin, ..
        } => origin,
        EvidenceValue::Unsupported => return ResourceScope::Unknown,
    };
    match origin {
        SourceOrigin::Bundled | SourceOrigin::User => ResourceScope::Global,
        SourceOrigin::Project => ResourceScope::Project,
        SourceOrigin::Plugin | SourceOrigin::Unknown => ResourceScope::Unknown,
    }
}

fn add_indexed_tools(
    builder: &mut InventoryBuilder,
    tools: &BTreeMap<String, antiburn_local::analysis::ToolDefinition>,
    scope: ResourceScope,
) {
    for (name, definition) in tools {
        if definition.tokens == 0
            || definition.deferred
            || antiburn_local::analysis::tool_catalog::situational_tools(builder.agent.slug())
                .iter()
                .any(|situational| {
                    antiburn_local::analysis::tool_catalog::comparable_tool_name(situational)
                        == antiburn_local::analysis::tool_catalog::comparable_tool_name(name)
                })
        {
            continue;
        }
        builder.add(
            ResourceKind::BuiltInTool,
            name,
            EnabledState::Unknown,
            scope,
            ResourceProvenance::IndexedSession,
        );
    }
}

#[cfg(test)]
mod tests;
