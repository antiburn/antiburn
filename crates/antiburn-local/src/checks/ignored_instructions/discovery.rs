//! Bounded, agent-specific discovery of current instruction files.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncReadExt as _;

use super::instructions::{
    InstructionProvenance, InstructionScope, InstructionSnapshot, MAX_INSTRUCTION_BYTES,
    snapshot_from_text,
};
use crate::analysis::SourceFormat;

pub const MAX_INSTRUCTION_FILES: usize = 64;
pub const MAX_INSTRUCTION_TOTAL_BYTES: usize = 512 * 1024;
const MAX_RULE_DIRECTORY_DEPTH: usize = 4;
const MAX_RULE_DIRECTORIES: usize = 128;
const MAX_RULE_DIRECTORY_ENTRIES: usize = 256;
const MAX_WORKTREE_DEPTH: usize = 64;
const MAX_IMPORT_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionAdapter {
    Claude,
    Codex,
    Pi,
    OpenCode,
    Cursor,
    Antigravity,
}

impl InstructionAdapter {
    pub fn for_source(agent: &str, format: SourceFormat) -> Option<Self> {
        match (agent.to_ascii_lowercase().as_str(), format) {
            ("claude", SourceFormat::ClaudeJsonl) => Some(Self::Claude),
            ("codex", SourceFormat::CodexRolloutJsonl) => Some(Self::Codex),
            ("pi", SourceFormat::PiV3Jsonl) => Some(Self::Pi),
            ("opencode", SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2) => {
                Some(Self::OpenCode)
            }
            (
                "cursor",
                SourceFormat::CursorCliAgentJsonl
                | SourceFormat::CursorCliStoreDb
                | SourceFormat::CursorChatStoreDb
                | SourceFormat::CursorIdeComposer,
            ) => Some(Self::Cursor),
            (
                "antigravity",
                SourceFormat::AntigravityBrainJsonl
                | SourceFormat::AntigravityCascadeJson
                | SourceFormat::AntigravitySqlite,
            ) => Some(Self::Antigravity),
            _ => None,
        }
    }

    fn project_names(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[
                "CLAUDE.md",
                "CLAUDE.local.md",
                ".claude/CLAUDE.md",
                "AGENTS.md",
                ".claude/AGENTS.md",
            ],
            Self::Codex => &["AGENTS.override.md", "AGENTS.md"],
            Self::Pi => &[
                "AGENTS.override.md",
                "AGENTS.md",
                "AGENTS.MD",
                "CLAUDE.md",
                "CLAUDE.MD",
                "SYSTEM.md",
                "APPEND_SYSTEM.md",
                ".pi/SYSTEM.md",
                ".pi/APPEND_SYSTEM.md",
            ],
            Self::OpenCode => &["AGENTS.md", "CLAUDE.md"],
            Self::Cursor => &["AGENTS.md"],
            Self::Antigravity => &["GEMINI.md"],
        }
    }

    fn global_names(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[".claude/CLAUDE.md"],
            Self::Codex => &["AGENTS.override.md", "AGENTS.md"],
            Self::Pi => &[
                "AGENTS.override.md",
                "AGENTS.md",
                "AGENTS.MD",
                "CLAUDE.md",
                "CLAUDE.MD",
                "SYSTEM.md",
                "APPEND_SYSTEM.md",
            ],
            Self::OpenCode => &["AGENTS.md", ".config/opencode/AGENTS.md"],
            Self::Cursor => &["AGENTS.md"],
            Self::Antigravity => &[".gemini/GEMINI.md", ".gemini/antigravity-cli/rules"],
        }
    }

    fn rule_directory(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some(".claude/rules"),
            Self::Cursor => Some(".cursor/rules"),
            Self::Antigravity => Some(".agents/rules"),
            Self::Codex | Self::Pi | Self::OpenCode => None,
        }
    }

    fn rule_extension(self) -> &'static str {
        match self {
            Self::Claude | Self::Antigravity => "md",
            Self::Cursor => "mdc",
            Self::Codex | Self::Pi | Self::OpenCode => "",
        }
    }

    fn supports_imports(self) -> bool {
        matches!(self, Self::Claude | Self::Antigravity)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstructionDiscovery {
    pub snapshots: Vec<InstructionSnapshot>,
    pub limitations: Vec<String>,
    /// True only when the adapter completed its bounded known-path scan.
    /// It does not prove historical activation or completeness of unmodeled config.
    pub scan_complete: bool,
}

/// Reads bounded current files from the session's actual worktree and home.
/// Current-file evidence remains distinct from content known to have entered
/// the historical model context.
pub async fn discover_current_instructions(
    agent: &str,
    format: SourceFormat,
    worktree_root: &Path,
    session_cwd: &Path,
    home: &Path,
) -> InstructionDiscovery {
    let Some(adapter) = InstructionAdapter::for_source(agent, format) else {
        return InstructionDiscovery {
            limitations: vec!["unsupported_source_format".to_owned()],
            ..InstructionDiscovery::default()
        };
    };
    let Ok(root) = fs::canonicalize(worktree_root).await else {
        return InstructionDiscovery {
            limitations: vec!["worktree_unavailable".to_owned()],
            ..InstructionDiscovery::default()
        };
    };
    let Ok(home) = fs::canonicalize(home).await else {
        return InstructionDiscovery {
            limitations: vec!["home_directory_unavailable".to_owned()],
            ..InstructionDiscovery::default()
        };
    };
    let Ok(cwd) = fs::canonicalize(session_cwd).await else {
        return InstructionDiscovery {
            limitations: vec!["session_working_directory_unavailable".to_owned()],
            ..InstructionDiscovery::default()
        };
    };
    if !cwd.starts_with(&root) {
        return InstructionDiscovery {
            limitations: vec!["session_working_directory_outside_worktree".to_owned()],
            ..InstructionDiscovery::default()
        };
    }

    let mut candidates = BTreeSet::new();
    let mut limitations = Vec::new();
    let codex_home = if adapter == InstructionAdapter::Codex {
        match std::env::var_os("CODEX_HOME") {
            Some(configured) => {
                let configured = PathBuf::from(configured);
                match fs::canonicalize(&configured).await {
                    Ok(path) if path.starts_with(&home) => path,
                    Ok(_) => {
                        limitations.push("codex_home_outside_user_home".to_owned());
                        home.join(".codex")
                    }
                    Err(_) => {
                        limitations.push("codex_home_unavailable".to_owned());
                        home.join(".codex")
                    }
                }
            }
            None => home.join(".codex"),
        }
    } else {
        home.join(".codex")
    };
    let codex_fallbacks = if adapter == InstructionAdapter::Codex {
        codex_fallback_names(&codex_home, &mut limitations).await
    } else {
        Vec::new()
    };
    let (directory_chain, chain_truncated) = directory_chain(&cwd, &root);
    if chain_truncated {
        limitations.push("instruction_directory_depth_limit".to_owned());
    }
    for base in &directory_chain {
        if !matches!(adapter, InstructionAdapter::Codex | InstructionAdapter::Pi) {
            let path = base.join("AGENTS.md");
            let scope = if path == home.join("AGENTS.md") {
                InstructionScope::Global
            } else if adapter == InstructionAdapter::Claude {
                InstructionScope::Conditional
            } else if base != &root {
                InstructionScope::Nested
            } else {
                InstructionScope::Project
            };
            candidates.insert((path, scope));
        }
    }
    candidates.insert((home.join("AGENTS.md"), InstructionScope::Global));
    for base in directory_chain {
        match adapter {
            InstructionAdapter::Codex => {
                add_first_existing(
                    &mut candidates,
                    &base,
                    &["AGENTS.override.md", "AGENTS.md"],
                    &codex_fallbacks,
                    if base == root {
                        InstructionScope::Project
                    } else {
                        InstructionScope::Nested
                    },
                )
                .await
            }
            InstructionAdapter::Pi => {
                add_first_existing(
                    &mut candidates,
                    &base,
                    &[
                        "AGENTS.override.md",
                        "AGENTS.md",
                        "AGENTS.MD",
                        "CLAUDE.md",
                        "CLAUDE.MD",
                    ],
                    &[],
                    if base == root {
                        InstructionScope::Project
                    } else {
                        InstructionScope::Nested
                    },
                )
                .await;
                for name in [
                    "SYSTEM.md",
                    "APPEND_SYSTEM.md",
                    ".pi/SYSTEM.md",
                    ".pi/APPEND_SYSTEM.md",
                ] {
                    candidates.insert((
                        base.join(name),
                        if base == root {
                            InstructionScope::Project
                        } else {
                            InstructionScope::Nested
                        },
                    ));
                }
            }
            InstructionAdapter::OpenCode => {
                add_first_existing(
                    &mut candidates,
                    &base,
                    &["AGENTS.md", "CLAUDE.md"],
                    &[],
                    if base == root {
                        InstructionScope::Project
                    } else {
                        InstructionScope::Nested
                    },
                )
                .await
            }
            _ => {
                for relative in adapter.project_names() {
                    let scope =
                        if adapter == InstructionAdapter::Claude && relative.contains("AGENTS") {
                            InstructionScope::Conditional
                        } else if base != root {
                            InstructionScope::Nested
                        } else {
                            InstructionScope::Project
                        };
                    candidates.insert((base.join(relative), scope));
                }
            }
        }
    }
    match adapter {
        InstructionAdapter::Codex => {
            add_first_existing(
                &mut candidates,
                &codex_home,
                &["AGENTS.override.md", "AGENTS.md"],
                &codex_fallbacks,
                InstructionScope::Global,
            )
            .await
        }
        InstructionAdapter::Pi => {
            let agent_home = pi_agent_home(&home);
            add_first_existing(
                &mut candidates,
                &agent_home,
                &[
                    "AGENTS.override.md",
                    "AGENTS.md",
                    "AGENTS.MD",
                    "CLAUDE.md",
                    "CLAUDE.MD",
                ],
                &[],
                InstructionScope::Global,
            )
            .await;
            for name in ["SYSTEM.md", "APPEND_SYSTEM.md"] {
                candidates.insert((agent_home.join(name), InstructionScope::Global));
            }
        }
        InstructionAdapter::Antigravity => {
            let gemini_home = gemini_home(&home);
            for name in [
                "GEMINI.md",
                "antigravity-cli/rules",
                "antigravity-ide/rules",
                "antigravity/rules",
            ] {
                let path = gemini_home.join(name);
                if path_is_directory(&path).await {
                    for rule in bounded_markdown_tree(&path, &mut limitations, "md").await {
                        candidates.insert((rule, InstructionScope::Global));
                    }
                } else {
                    candidates.insert((path, InstructionScope::Global));
                }
            }
        }
        InstructionAdapter::OpenCode => {
            add_first_existing(
                &mut candidates,
                &home.join(".config/opencode"),
                &["AGENTS.md"],
                &[],
                InstructionScope::Global,
            )
            .await;
            add_first_existing(
                &mut candidates,
                &home.join(".claude"),
                &["CLAUDE.md"],
                &[],
                InstructionScope::Conditional,
            )
            .await;
        }
        _ => {
            for relative in adapter.global_names() {
                let path = home.join(relative);
                if path_is_directory(&path).await {
                    for rule in
                        bounded_markdown_tree(&path, &mut limitations, adapter.rule_extension())
                            .await
                    {
                        candidates.insert((rule, InstructionScope::Global));
                    }
                } else {
                    candidates.insert((path, InstructionScope::Global));
                }
            }
        }
    }
    if adapter == InstructionAdapter::OpenCode {
        for (path, scope) in opencode_instruction_inputs(&root, &home, &mut limitations).await {
            candidates.insert((path, scope));
        }
    }
    match adapter {
        InstructionAdapter::Claude => {
            limitations.push("claude_managed_inline_policy_not_read".to_owned())
        }
        InstructionAdapter::Cursor => {
            limitations.push("cursor_user_and_team_ui_rules_not_read".to_owned())
        }
        _ => {}
    }
    if let Some(directory) = adapter.rule_directory() {
        for base in [&root, &home] {
            let scope = if base == &root {
                if matches!(
                    adapter,
                    InstructionAdapter::Cursor | InstructionAdapter::Antigravity
                ) {
                    InstructionScope::Conditional
                } else {
                    InstructionScope::Project
                }
            } else {
                InstructionScope::Global
            };
            for path in bounded_markdown_tree(
                &base.join(directory),
                &mut limitations,
                adapter.rule_extension(),
            )
            .await
            {
                candidates.insert((path, scope));
            }
        }
    }

    let mut state = DiscoveryState {
        adapter,
        root: root.clone(),
        home: home.clone(),
        snapshots: Vec::new(),
        limitations,
        visited: HashSet::new(),
        total_bytes: 0,
    };
    for (path, scope) in candidates {
        if state.snapshots.len() >= MAX_INSTRUCTION_FILES {
            state
                .limitations
                .push("instruction_file_count_limit".to_owned());
            break;
        }
        state.read_file(&path, scope, 0, false).await;
    }
    let scan_complete = state.limitations.is_empty();
    InstructionDiscovery {
        snapshots: state.snapshots,
        limitations: state.limitations,
        scan_complete,
    }
}

fn pi_agent_home(home: &Path) -> PathBuf {
    pi_agent_home_in(
        home,
        crate::discovery::env_path_when_real_home(home, "PI_AGENT_DIR").as_deref(),
        crate::discovery::env_path_when_real_home(home, "PI_CODING_AGENT_DIR").as_deref(),
    )
}

fn gemini_home(home: &Path) -> PathBuf {
    gemini_home_in(
        home,
        crate::discovery::env_path_when_real_home(home, "GEMINI_HOME").as_deref(),
    )
}

fn pi_agent_home_in(home: &Path, legacy: Option<&Path>, current: Option<&Path>) -> PathBuf {
    current
        .or(legacy)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".pi/agent"))
}

fn gemini_home_in(home: &Path, configured: Option<&Path>) -> PathBuf {
    configured
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".gemini"))
}

fn stable_relative_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

struct DiscoveryState {
    adapter: InstructionAdapter,
    root: PathBuf,
    home: PathBuf,
    snapshots: Vec<InstructionSnapshot>,
    limitations: Vec<String>,
    visited: HashSet<PathBuf>,
    total_bytes: usize,
}

impl DiscoveryState {
    async fn read_file(
        &mut self,
        path: &Path,
        mut scope: InstructionScope,
        depth: usize,
        required: bool,
    ) {
        if depth > MAX_IMPORT_DEPTH {
            self.limitations
                .push("instruction_import_depth_limit".to_owned());
            return;
        }
        let canonical = match fs::canonicalize(path).await {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.missing_import(required);
                return;
            }
            Err(_) => {
                self.limitations
                    .push("instruction_path_unavailable".to_owned());
                return;
            }
        };
        if !canonical.starts_with(&self.root) && !canonical.starts_with(&self.home) {
            self.limitations
                .push("instruction_path_outside_allowed_roots".to_owned());
            return;
        }
        if !self.visited.insert(canonical.clone()) {
            return;
        }
        if self.visited.len() > MAX_INSTRUCTION_FILES {
            self.limitations
                .push("instruction_file_count_limit".to_owned());
            return;
        }
        let metadata = match fs::metadata(&canonical).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.missing_import(required);
                return;
            }
            Err(_) => {
                self.limitations
                    .push("instruction_metadata_unavailable".to_owned());
                return;
            }
        };
        if !metadata.is_file() {
            return;
        }
        let size = metadata.len() as usize;
        if size > MAX_INSTRUCTION_BYTES
            || size > MAX_INSTRUCTION_TOTAL_BYTES.saturating_sub(self.total_bytes)
        {
            self.limitations.push("instruction_byte_limit".to_owned());
            return;
        }
        let file = match fs::File::open(&canonical).await {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.missing_import(required);
                return;
            }
            Err(_) => {
                self.limitations.push("instruction_read_failed".to_owned());
                return;
            }
        };
        let mut bytes = Vec::with_capacity(size.min(MAX_INSTRUCTION_BYTES));
        if file
            .take((MAX_INSTRUCTION_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .is_err()
            || bytes.len() > MAX_INSTRUCTION_BYTES
            || bytes.len() > MAX_INSTRUCTION_TOTAL_BYTES.saturating_sub(self.total_bytes)
        {
            self.limitations.push("instruction_byte_limit".to_owned());
            return;
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let Ok(text) = String::from_utf8(bytes) else {
            self.limitations
                .push("instruction_file_not_utf8".to_owned());
            return;
        };
        let source = if let Ok(relative) = canonical.strip_prefix(&self.root) {
            format!("project:{}", stable_relative_path(relative))
        } else if let Ok(relative) = canonical.strip_prefix(&self.home) {
            format!("home:{}", stable_relative_path(relative))
        } else {
            "unknown_instruction_source".to_owned()
        };

        let mut imports = Vec::new();
        if self.adapter.supports_imports() {
            for import in local_instruction_imports(&text) {
                if import.starts_with("http:") || import.starts_with("https:") {
                    self.limitations
                        .push("remote_instruction_import_not_fetched".to_owned());
                    continue;
                }
                let import_path = if let Some(path) = import.strip_prefix("~/") {
                    self.home.join(path)
                } else {
                    let path = PathBuf::from(&import);
                    if path.is_absolute() {
                        path
                    } else {
                        canonical.parent().unwrap_or(&self.root).join(path)
                    }
                };
                imports.push(import.clone());
                Box::pin(self.read_file(&import_path, InstructionScope::Nested, depth + 1, true))
                    .await;
            }
        }
        if let Some(frontmatter) = cursor_frontmatter(&text) {
            if frontmatter.contains("globs:") || frontmatter.contains("alwaysApply: false") {
                scope = InstructionScope::Conditional;
            } else if frontmatter.contains("alwaysApply: true") {
                scope = if canonical.starts_with(&self.root) {
                    InstructionScope::Project
                } else {
                    InstructionScope::Global
                };
            }
        }
        if self.adapter == InstructionAdapter::Claude
            && cursor_frontmatter(&text).is_some_and(|frontmatter| frontmatter.contains("paths:"))
        {
            scope = InstructionScope::Conditional;
        }
        if self.adapter == InstructionAdapter::Antigravity
            && cursor_frontmatter(&text).is_some_and(|frontmatter| {
                frontmatter.contains("activation") && !frontmatter.contains("activation: always")
            })
        {
            scope = InstructionScope::Conditional;
        }
        if let Ok(mut snapshot) = snapshot_from_text(
            source,
            text,
            InstructionProvenance::CurrentFileComparison,
            scope,
        ) {
            snapshot.imports = imports;
            self.snapshots.push(snapshot);
        } else {
            self.limitations
                .push("instruction_markdown_segmentation_limit".to_owned());
        }
    }

    fn missing_import(&mut self, required: bool) {
        if required
            && !self
                .limitations
                .iter()
                .any(|item| item == "instruction_import_unavailable")
        {
            self.limitations
                .push("instruction_import_unavailable".to_owned());
        }
    }
}

fn directory_chain(cwd: &Path, root: &Path) -> (Vec<PathBuf>, bool) {
    let mut chain = Vec::new();
    let mut current = cwd;
    let mut truncated = false;
    loop {
        chain.push(current.to_path_buf());
        if current == root {
            break;
        }
        if chain.len() >= MAX_WORKTREE_DEPTH {
            truncated = true;
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    chain.reverse();
    (chain, truncated)
}

async fn bounded_markdown_tree(
    root: &Path,
    limitations: &mut Vec<String>,
    extension: &str,
) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited_directories = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        visited_directories += 1;
        if depth > MAX_RULE_DIRECTORY_DEPTH
            || result.len() >= MAX_INSTRUCTION_FILES
            || visited_directories > MAX_RULE_DIRECTORIES
        {
            limitations.push("instruction_rule_tree_limit".to_owned());
            break;
        }
        let mut entries = match fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                limitations.push("instruction_rule_directory_unavailable".to_owned());
                continue;
            }
        };
        let mut collected = Vec::new();
        loop {
            if collected.len() == MAX_RULE_DIRECTORY_ENTRIES {
                limitations.push("instruction_rule_entry_limit".to_owned());
                break;
            }
            match entries.next_entry().await {
                Ok(Some(entry)) => collected.push(entry),
                Ok(None) => break,
                Err(_) => {
                    limitations.push("instruction_rule_entry_unavailable".to_owned());
                    break;
                }
            }
        }
        let mut entries = collected;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_type = match entry.file_type().await {
                Ok(file_type) => file_type,
                Err(_) => {
                    limitations.push("instruction_rule_entry_unavailable".to_owned());
                    continue;
                }
            };
            if file_type.is_dir() {
                stack.push((path, depth + 1));
            } else if file_type.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some(extension)
            {
                result.push(path);
                if result.len() >= MAX_INSTRUCTION_FILES {
                    break;
                }
            }
        }
    }
    result
}

async fn path_is_directory(path: &Path) -> bool {
    fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

async fn add_first_existing(
    candidates: &mut BTreeSet<(PathBuf, InstructionScope)>,
    base: &Path,
    preferred: &[&str],
    fallback: &[String],
    scope: InstructionScope,
) {
    for name in preferred
        .iter()
        .copied()
        .chain(fallback.iter().map(String::as_str))
    {
        let path = base.join(name);
        match fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => {
                candidates.insert((path, scope));
                return;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                candidates.insert((path, scope));
                return;
            }
        }
    }
}

async fn codex_fallback_names(codex_home: &Path, limitations: &mut Vec<String>) -> Vec<String> {
    let path = codex_home.join("config.toml");
    let metadata = match fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(_) => {
            limitations.push("codex_config_unavailable".to_owned());
            return Vec::new();
        }
    };
    if metadata.len() > 64 * 1024 {
        limitations.push("codex_config_size_limit".to_owned());
        return Vec::new();
    }
    let Ok(file) = fs::File::open(path).await else {
        limitations.push("codex_config_unavailable".to_owned());
        return Vec::new();
    };
    let mut bytes = Vec::new();
    if file
        .take((64 * 1024 + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .is_err()
        || bytes.len() > 64 * 1024
    {
        limitations.push("codex_config_size_limit".to_owned());
        return Vec::new();
    }
    let Ok(text) = String::from_utf8(bytes) else {
        limitations.push("codex_config_invalid_utf8".to_owned());
        return Vec::new();
    };
    let Ok(config) = toml::from_str::<toml::Value>(&text) else {
        limitations.push("codex_config_invalid_toml".to_owned());
        return Vec::new();
    };
    let Some(names) = config
        .get("project_doc_fallback_filenames")
        .and_then(toml::Value::as_array)
    else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for name in names.iter().filter_map(toml::Value::as_str) {
        if result.len() == MAX_INSTRUCTION_FILES {
            limitations.push("codex_fallback_filename_limit".to_owned());
            break;
        }
        if name.is_empty()
            || name.len() > 128
            || Path::new(name).components().count() != 1
            || name.chars().any(char::is_control)
        {
            limitations.push("codex_fallback_filename_invalid".to_owned());
            continue;
        }
        if !result.iter().any(|existing| existing == name) {
            result.push(name.to_owned());
        }
    }
    result
}

async fn opencode_instruction_inputs(
    root: &Path,
    home: &Path,
    limitations: &mut Vec<String>,
) -> Vec<(PathBuf, InstructionScope)> {
    let configs = [
        (root.join("opencode.json"), InstructionScope::Project),
        (
            root.join(".opencode/opencode.json"),
            InstructionScope::Project,
        ),
        (
            home.join(".config/opencode/opencode.json"),
            InstructionScope::Global,
        ),
    ];
    let mut result = Vec::new();
    for (config_path, scope) in configs {
        let jsonc_path = config_path.with_extension("jsonc");
        if fs::metadata(&jsonc_path).await.is_ok() {
            limitations.push("opencode_jsonc_config_not_parsed".to_owned());
        }
        let config = match read_bounded_text(&config_path, 64 * 1024, limitations).await {
            Some(config) => config,
            None => continue,
        };
        let Ok(config) = serde_json::from_str::<serde_json::Value>(&config) else {
            limitations.push("opencode_instruction_config_invalid_json".to_owned());
            continue;
        };
        let Some(instructions) = config
            .get("instructions")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for item in instructions.iter().filter_map(serde_json::Value::as_str) {
            if result.len() == MAX_INSTRUCTION_FILES {
                limitations.push("opencode_instruction_path_limit".to_owned());
                break;
            }
            if item.starts_with("https://") || item.starts_with("http://") {
                limitations.push("remote_instruction_import_not_fetched".to_owned());
            } else if item.contains(['*', '?', '[', '{']) {
                limitations.push("opencode_instruction_glob_not_expanded".to_owned());
            } else {
                result.push((config_path.parent().unwrap_or(root).join(item), scope));
            }
        }
    }
    result
}

async fn read_bounded_text(
    path: &Path,
    max_bytes: usize,
    limitations: &mut Vec<String>,
) -> Option<String> {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            limitations.push("instruction_config_unavailable".to_owned());
            return None;
        }
    };
    if !metadata.is_file() || metadata.len() > max_bytes as u64 {
        limitations.push("instruction_config_size_limit".to_owned());
        return None;
    }
    let file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => {
            limitations.push("instruction_config_unavailable".to_owned());
            return None;
        }
    };
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(max_bytes));
    if file
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .await
        .is_err()
        || bytes.len() > max_bytes
    {
        limitations.push("instruction_config_size_limit".to_owned());
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(_) => {
            limitations.push("instruction_config_invalid_utf8".to_owned());
            None
        }
    }
}

fn local_instruction_imports(text: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut in_code_span = false;
        let mut previous = ' ';
        let mut chars = line.chars().peekable();
        while let Some(character) = chars.next() {
            if character == '`' {
                in_code_span = !in_code_span;
                previous = character;
                continue;
            }
            if character == '@'
                && !in_code_span
                && (previous.is_whitespace() || matches!(previous, '(' | '[' | '{' | ','))
            {
                let mut target = String::new();
                while let Some(next) = chars.peek().copied() {
                    if next.is_whitespace() || matches!(next, ')' | ']' | '}' | ',' | ';') {
                        break;
                    }
                    target.push(next);
                    chars.next();
                }
                let target = target.trim_end_matches('.');
                if !target.is_empty() && imports.len() < MAX_INSTRUCTION_FILES {
                    imports.push(target.to_owned());
                }
            }
            previous = character;
        }
    }
    imports
}

fn cursor_frontmatter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    rest.split_once("\n---").map(|(frontmatter, _)| frontmatter)
}

#[cfg(test)]
mod tests {
    use super::super::instructions::InstructionProvenance;
    use super::*;
    use std::fs as std_fs;

    #[test]
    fn relative_instruction_source_paths_use_forward_slashes() {
        let path = Path::new("src").join("module").join("AGENTS.md");
        assert_eq!(stable_relative_path(&path), "src/module/AGENTS.md");
    }

    fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("worktree");
        let home = temp.path().join("home");
        std_fs::create_dir_all(&root).unwrap();
        std_fs::create_dir_all(&home).unwrap();
        (temp, root, home)
    }

    #[test]
    fn first_tier_formats_have_narrow_agent_adapters() {
        let cases = [
            ("claude", SourceFormat::ClaudeJsonl),
            ("codex", SourceFormat::CodexRolloutJsonl),
            ("pi", SourceFormat::PiV3Jsonl),
            ("opencode", SourceFormat::OpenCodeSqliteV2),
            ("cursor", SourceFormat::CursorCliAgentJsonl),
            ("cursor", SourceFormat::CursorCliStoreDb),
            ("cursor", SourceFormat::CursorChatStoreDb),
            ("cursor", SourceFormat::CursorIdeComposer),
            ("antigravity", SourceFormat::AntigravityBrainJsonl),
            ("antigravity", SourceFormat::AntigravitySqlite),
        ];
        for (agent, format) in cases {
            assert!(InstructionAdapter::for_source(agent, format).is_some());
        }
    }

    #[test]
    fn global_instruction_roots_match_agent_environment_overrides() {
        let home = Path::new("/home/user");
        let legacy = Path::new("/custom/pi-legacy");
        let current = Path::new("/custom/pi-current");
        let gemini = Path::new("/custom/gemini");

        assert_eq!(pi_agent_home_in(home, Some(legacy), Some(current)), current);
        assert_eq!(pi_agent_home_in(home, Some(legacy), None), legacy);
        assert_eq!(pi_agent_home_in(home, None, None), home.join(".pi/agent"));
        assert_eq!(gemini_home_in(home, Some(gemini)), gemini);
        assert_eq!(gemini_home_in(home, None), home.join(".gemini"));
    }

    #[test]
    fn imports_skip_code_and_preserve_only_explicit_local_references() {
        let imports = local_instruction_imports(
            "See @./rules/base.md and @https://example.test/rule.md.\n`@literal.md`\n```md\n@code.md\n```\nmail name@example.test",
        );
        assert_eq!(imports, ["./rules/base.md", "https://example.test/rule.md"]);
    }

    #[test]
    fn cursor_activation_frontmatter_stays_explicit() {
        assert_eq!(
            cursor_frontmatter("---\nalwaysApply: false\nglobs: src/**\n---\ntext"),
            Some("alwaysApply: false\nglobs: src/**")
        );
    }

    #[tokio::test]
    async fn claude_discovery_reads_project_global_and_local_imports_as_current_files() {
        let (_temp, root, home) = roots();
        std_fs::create_dir_all(root.join("src/nested")).unwrap();
        std_fs::create_dir_all(root.join(".claude")).unwrap();
        std_fs::create_dir_all(home.join(".claude")).unwrap();
        std_fs::write(root.join("AGENTS.md"), "# Project\nUse bounded reads.").unwrap();
        std_fs::write(
            root.join("CLAUDE.md"),
            "# Claude\n@./.claude/shared.md\nKeep the exception with this rule.",
        )
        .unwrap();
        std_fs::write(
            root.join(".claude/shared.md"),
            "# Imported\nPreserve local evidence.",
        )
        .unwrap();
        std_fs::write(
            home.join(".claude/CLAUDE.md"),
            "# User\nDo not use remote imports.",
        )
        .unwrap();

        let result = discover_current_instructions(
            "claude",
            SourceFormat::ClaudeJsonl,
            &root,
            &root.join("src/nested"),
            &home,
        )
        .await;
        assert!(!result.scan_complete);
        assert!(
            result
                .limitations
                .contains(&"claude_managed_inline_policy_not_read".to_owned())
        );
        assert_eq!(result.snapshots.len(), 4);
        assert!(result
            .snapshots
            .iter()
            .all(|snapshot| snapshot.provenance == InstructionProvenance::CurrentFileComparison));
        assert!(
            result
                .snapshots
                .iter()
                .any(|snapshot| snapshot.imports == ["./.claude/shared.md"])
        );
        assert!(
            result
                .snapshots
                .iter()
                .all(|snapshot| !snapshot.digest.is_empty())
        );
    }

    #[tokio::test]
    async fn absent_optional_candidates_do_not_add_a_missing_import_limitation() {
        let (_temp, root, home) = roots();
        let result = discover_current_instructions(
            "codex",
            SourceFormat::CodexRolloutJsonl,
            &root,
            &root,
            &home,
        )
        .await;
        assert!(result.scan_complete, "{:?}", result.limitations);
        assert!(result.limitations.is_empty());
    }

    #[tokio::test]
    async fn missing_direct_and_nested_imports_make_discovery_incomplete_once() {
        let (_temp, root, home) = roots();
        std_fs::write(
            root.join("CLAUDE.md"),
            "# Root\n@./missing.md\n@./missing.md",
        )
        .unwrap();
        std_fs::write(root.join("nested.md"), "# Nested\n@./nested-missing.md").unwrap();
        std_fs::write(root.join("AGENTS.md"), "# Optional\nNo imports.").unwrap();
        std_fs::write(
            root.join("CLAUDE.local.md"),
            "# Nested import\n@./nested.md",
        )
        .unwrap();

        let result =
            discover_current_instructions("claude", SourceFormat::ClaudeJsonl, &root, &root, &home)
                .await;

        assert!(!result.scan_complete);
        assert_eq!(
            result
                .limitations
                .iter()
                .filter(|item| item.as_str() == "instruction_import_unavailable")
                .count(),
            1
        );
        assert!(
            result
                .snapshots
                .iter()
                .any(|snapshot| snapshot.text.contains("# Nested"))
        );
    }

    #[tokio::test]
    async fn successful_import_cycles_and_repeated_imports_assess_each_file_once() {
        let (_temp, root, home) = roots();
        std_fs::write(root.join("CLAUDE.md"), "# Root\n@./shared.md\n@./shared.md").unwrap();
        std_fs::write(root.join("shared.md"), "# Shared\n@./CLAUDE.md").unwrap();

        let result =
            discover_current_instructions("claude", SourceFormat::ClaudeJsonl, &root, &root, &home)
                .await;

        assert_eq!(
            result
                .snapshots
                .iter()
                .filter(|item| item.text.contains("# Root"))
                .count(),
            1
        );
        assert_eq!(
            result
                .snapshots
                .iter()
                .filter(|item| item.text.contains("# Shared"))
                .count(),
            1
        );
        assert!(
            !result
                .limitations
                .contains(&"instruction_import_unavailable".to_owned())
        );
    }

    #[tokio::test]
    async fn cursor_glob_rules_are_retained_with_conditional_scope() {
        let (_temp, root, home) = roots();
        std_fs::create_dir_all(root.join(".cursor/rules")).unwrap();
        std_fs::write(
            root.join(".cursor/rules/db.mdc"),
            "---\nglobs: db/**\n---\nUse transactions.",
        )
        .unwrap();

        let result = discover_current_instructions(
            "cursor",
            SourceFormat::CursorCliAgentJsonl,
            &root,
            &root,
            &home,
        )
        .await;
        let rule = result
            .snapshots
            .iter()
            .find(|snapshot| {
                snapshot.source.split_once(':').is_some_and(|(_, path)| {
                    std::path::Path::new(path)
                        .ends_with(std::path::Path::new(".cursor").join("rules").join("db.mdc"))
                })
            })
            .expect("Cursor rule snapshot");
        assert_eq!(rule.scope, InstructionScope::Conditional);
    }

    #[tokio::test]
    async fn codex_fallback_names_follow_the_user_config_file() {
        let (_temp, root, home) = roots();
        std_fs::create_dir_all(home.join(".codex")).unwrap();
        std_fs::write(
            home.join(".codex/config.toml"),
            "project_doc_fallback_filenames = [\"TEAM_GUIDE.md\"]\n",
        )
        .unwrap();
        std_fs::write(
            root.join("TEAM_GUIDE.md"),
            "# Team\nRun the focused checks.",
        )
        .unwrap();

        let result = discover_current_instructions(
            "codex",
            SourceFormat::CodexRolloutJsonl,
            &root,
            &root,
            &home,
        )
        .await;
        assert!(result.scan_complete, "{:?}", result.limitations);
        assert_eq!(result.snapshots.len(), 1);
        assert!(result.snapshots[0].source.ends_with("TEAM_GUIDE.md"));
    }

    #[tokio::test]
    async fn opencode_configured_literal_instruction_paths_are_loaded_locally() {
        let (_temp, root, home) = roots();
        std_fs::create_dir_all(root.join("docs")).unwrap();
        std_fs::write(
            root.join("opencode.json"),
            r#"{"instructions":["docs/rules.md"]}"#,
        )
        .unwrap();
        std_fs::write(root.join("docs/rules.md"), "# Local\nUse the project API.").unwrap();

        let result = discover_current_instructions(
            "opencode",
            SourceFormat::OpenCodeJsonl,
            &root,
            &root,
            &home,
        )
        .await;
        assert!(result.scan_complete, "{:?}", result.limitations);
        assert!(result.snapshots.iter().any(|snapshot| {
            snapshot.source.split_once(':').is_some_and(|(_, path)| {
                Path::new(path).ends_with(Path::new("docs").join("rules.md"))
            })
        }));
    }

    #[tokio::test]
    async fn opencode_loads_global_and_project_agents_files_together() {
        let (_temp, root, home) = roots();
        std_fs::create_dir_all(home.join(".config/opencode")).unwrap();
        std_fs::write(
            root.join("AGENTS.md"),
            "# Project rules\nUse the project test command.",
        )
        .unwrap();
        std_fs::write(
            home.join(".config/opencode/AGENTS.md"),
            "# Global rules\nUse Semble before exploring an unfamiliar codebase.",
        )
        .unwrap();

        let result = discover_current_instructions(
            "opencode",
            SourceFormat::OpenCodeJsonl,
            &root,
            &root,
            &home,
        )
        .await;

        assert!(result.scan_complete, "{:?}", result.limitations);
        assert_eq!(result.snapshots.len(), 2);
        assert!(result.snapshots.iter().any(|snapshot| {
            snapshot.source.starts_with("project:")
                && snapshot.text.contains("Use the project test command")
        }));
        assert!(result.snapshots.iter().any(|snapshot| {
            snapshot.source.starts_with("home:")
                && snapshot.text.contains("Use Semble before exploring")
        }));
    }

    #[tokio::test]
    async fn nested_instruction_file_retains_directory_scope_and_current_provenance() {
        let (_temp, root, home) = roots();
        std_fs::create_dir_all(root.join("src/module")).unwrap();
        std_fs::write(root.join("AGENTS.md"), "# Root\nRun tests.").unwrap();
        std_fs::write(root.join("src/AGENTS.md"), "# Nested\nUse the local API.").unwrap();

        let result = discover_current_instructions(
            "opencode",
            SourceFormat::OpenCodeJsonl,
            &root,
            &root.join("src/module"),
            &home,
        )
        .await;
        let parent = result
            .snapshots
            .iter()
            .find(|snapshot| snapshot.source == "project:AGENTS.md")
            .unwrap();
        let nested = result
            .snapshots
            .iter()
            .find(|snapshot| snapshot.source == "project:src/AGENTS.md")
            .unwrap();
        assert_eq!(parent.scope, InstructionScope::Project);
        assert_eq!(nested.scope, InstructionScope::Nested);
        assert_eq!(
            nested.provenance,
            InstructionProvenance::CurrentFileComparison
        );
    }

    #[tokio::test]
    async fn every_first_tier_adapter_loads_global_and_project_agents_files() {
        let cases = [
            ("claude", SourceFormat::ClaudeJsonl),
            ("codex", SourceFormat::CodexRolloutJsonl),
            ("pi", SourceFormat::PiV3Jsonl),
            ("opencode", SourceFormat::OpenCodeJsonl),
            ("cursor", SourceFormat::CursorCliAgentJsonl),
            ("antigravity", SourceFormat::AntigravityBrainJsonl),
        ];

        for (agent, format) in cases {
            let (_temp, root, home) = roots();
            std_fs::write(
                root.join("AGENTS.md"),
                "# Project policy\nFollow the project test plan.",
            )
            .unwrap();
            std_fs::write(
                home.join("AGENTS.md"),
                "# User policy\nKeep local changes focused.",
            )
            .unwrap();

            let result = discover_current_instructions(agent, format, &root, &root, &home).await;

            assert!(
                result.snapshots.iter().any(|snapshot| {
                    snapshot.source.starts_with("project:")
                        && snapshot.text.contains("Follow the project test plan")
                }),
                "{agent} project AGENTS.md was not discovered: {:?}",
                result.limitations
            );
            assert!(
                result.snapshots.iter().any(|snapshot| {
                    snapshot.source.starts_with("home:")
                        && snapshot.text.contains("Keep local changes focused")
                }),
                "{agent} global AGENTS.md was not discovered: {:?}",
                result.limitations
            );
        }
    }

    #[tokio::test]
    async fn discovery_rejects_a_session_cwd_outside_the_actual_worktree() {
        let (_temp, root, home) = roots();
        let outside = home.join("other-project");
        std_fs::create_dir_all(&outside).unwrap();
        let result = discover_current_instructions(
            "codex",
            SourceFormat::CodexRolloutJsonl,
            &root,
            &outside,
            &home,
        )
        .await;
        assert!(result.snapshots.is_empty());
        assert_eq!(
            result.limitations,
            ["session_working_directory_outside_worktree"]
        );
    }
}
