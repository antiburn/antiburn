use std::path::{Component, Path, PathBuf};

use antiburn_local::instructions::{CurrentInstruction, CurrentInstructionScope};
use serde_json::Value;

use super::{
    Collector, Producer,
    admission::{PathAdmission, has_parent_component},
    hierarchy,
    skills::SkillTraversal,
};

impl Collector<'_> {
    pub(super) fn collect_global(&mut self, producer: Producer, home: &Path, cwd: Option<&Path>) {
        match producer {
            Producer::Claude => {
                self.instruction(&home.join(".claude/CLAUDE.md"), home, true, None, false);
                self.rules(&home.join(".claude/rules"), home, true, cwd, 0);
                self.skill_source(
                    &home.join(".claude/skills"),
                    home,
                    true,
                    SkillTraversal::Direct,
                    0,
                );
                self.limit(true, false, "managed_inline_policy_not_read");
                self.limit(
                    true,
                    false,
                    "imports_and_configured_exclusions_not_resolved",
                );
                self.limit(true, true, "plugins_and_skill_controls_not_resolved");
            }
            Producer::Codex => {
                self.first_instruction(
                    &home.join(".codex"),
                    home,
                    true,
                    None,
                    &["AGENTS.override.md", "AGENTS.md"],
                );
                self.skill_source(
                    &home.join(".agents/skills"),
                    home,
                    true,
                    SkillTraversal::Direct,
                    0,
                );
                self.limit(true, false, "codex_home_and_config_overrides_not_resolved");
                self.limit(
                    true,
                    true,
                    "admin_system_plugins_and_skill_controls_not_resolved",
                );
            }
            Producer::Pi => {
                let directory = home.join(".pi/agent");
                self.first_instruction(&directory, home, true, None, &PI_INSTRUCTIONS);
                for name in ["SYSTEM.md", "APPEND_SYSTEM.md"] {
                    self.instruction(&directory.join(name), home, true, None, false);
                }
                for directory in [directory.join("skills"), home.join(".agents/skills")] {
                    self.skill_source(&directory, home, true, SkillTraversal::Grouped, 0);
                }
                self.limit(
                    true,
                    false,
                    "pi_agent_directory_and_runtime_overrides_not_resolved",
                );
                self.limit(
                    true,
                    true,
                    "pi_settings_packages_and_runtime_sources_not_resolved",
                );
            }
            Producer::OpenCodeV1 | Producer::OpenCodeV2 => {
                let found = self.instruction(
                    &home.join(".config/opencode/AGENTS.md"),
                    home,
                    true,
                    None,
                    false,
                );
                if producer == Producer::OpenCodeV1 && !found {
                    self.instruction(&home.join(".claude/CLAUDE.md"), home, true, None, false);
                }
                for relative in [
                    ".config/opencode/skills",
                    ".claude/skills",
                    ".agents/skills",
                ] {
                    self.skill_source(
                        &home.join(relative),
                        home,
                        true,
                        SkillTraversal::for_opencode(producer),
                        0,
                    );
                }
                self.limit(
                    true,
                    false,
                    "server_system_runtime_and_config_directory_overrides_not_resolved",
                );
                self.limit(
                    true,
                    true,
                    "server_builtin_system_and_skill_controls_not_resolved",
                );
            }
        }
    }

    pub(super) fn collect_repo(&mut self, producer: Producer, root: &Path, cwd: &Path) {
        let directories = match hierarchy(cwd, root) {
            Ok(directories) => directories,
            Err(reason) => {
                self.unavailable(false, reason);
                return;
            }
        };
        for directory in directories {
            match producer {
                Producer::Claude => {
                    for name in ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"] {
                        self.instruction(
                            &directory.join(name),
                            root,
                            false,
                            Some(&directory),
                            false,
                        );
                    }
                    self.rules(
                        &directory.join(".claude/rules"),
                        root,
                        false,
                        Some(&directory),
                        0,
                    );
                    self.skill_source(
                        &directory.join(".claude/skills"),
                        root,
                        false,
                        SkillTraversal::Direct,
                        0,
                    );
                }
                Producer::Codex => {
                    self.first_instruction(
                        &directory,
                        root,
                        false,
                        Some(&directory),
                        &["AGENTS.override.md", "AGENTS.md"],
                    );
                    self.skill_source(
                        &directory.join(".agents/skills"),
                        root,
                        false,
                        SkillTraversal::Direct,
                        0,
                    );
                }
                Producer::Pi => {
                    self.first_instruction(
                        &directory,
                        root,
                        false,
                        Some(&directory),
                        &PI_INSTRUCTIONS,
                    );
                    for relative in [".pi/skills", ".agents/skills"] {
                        self.skill_source(
                            &directory.join(relative),
                            root,
                            false,
                            SkillTraversal::Grouped,
                            0,
                        );
                    }
                }
                Producer::OpenCodeV1 | Producer::OpenCodeV2 => {
                    let names: &[&str] = if producer == Producer::OpenCodeV2 {
                        &["AGENTS.md"]
                    } else {
                        &["AGENTS.md", "CLAUDE.md"]
                    };
                    self.first_instruction(&directory, root, false, Some(&directory), names);
                    for relative in [".opencode/skills", ".claude/skills", ".agents/skills"] {
                        self.skill_source(
                            &directory.join(relative),
                            root,
                            false,
                            SkillTraversal::for_opencode(producer),
                            0,
                        );
                    }
                }
            }
        }
        match producer {
            Producer::Claude => {
                self.limit(
                    false,
                    false,
                    "ancestor_nested_imports_and_instruction_file_controls_not_resolved",
                );
                self.limit(false, true, "plugins_and_skill_controls_not_resolved");
            }
            Producer::Codex => {
                self.limit(
                    false,
                    false,
                    "codex_fallback_names_and_byte_policy_not_resolved",
                );
                self.limit(false, true, "skill_controls_not_resolved");
            }
            Producer::Pi => {
                for name in ["SYSTEM.md", "APPEND_SYSTEM.md"] {
                    self.instruction(&cwd.join(".pi").join(name), root, false, Some(cwd), false);
                }
                self.limit(false, false, "pi_ancestor_and_runtime_sources_not_resolved");
                self.limit(
                    false,
                    true,
                    "pi_settings_packages_and_runtime_sources_not_resolved",
                );
            }
            Producer::OpenCodeV1 | Producer::OpenCodeV2 => {
                self.limit(
                    false,
                    false,
                    "ancestor_nested_and_runtime_instruction_sources_not_resolved",
                );
                self.limit(false, true, "runtime_and_skill_controls_not_resolved");
            }
        }
    }

    fn first_instruction(
        &mut self,
        directory: &Path,
        safety: &Path,
        global: bool,
        scope: Option<&Path>,
        names: &[&str],
    ) {
        for name in names {
            if self.instruction(&directory.join(name), safety, global, scope, false) {
                break;
            }
        }
    }

    fn instruction(
        &mut self,
        path: &Path,
        safety: &Path,
        global: bool,
        scope: Option<&Path>,
        rule: bool,
    ) -> bool {
        if self.instructions_left == 0 {
            self.limit(global, false, "instruction_file_limit");
            return true;
        }
        let bytes = match self.reads.read(path, safety, false) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return false,
            Err(reason) => {
                self.limit(global, false, reason);
                return true;
            }
        };
        let Ok(content) = String::from_utf8(bytes) else {
            self.limit(global, false, "instruction_not_utf8");
            return true;
        };
        if content.trim().is_empty() {
            return false;
        }
        let globs = if rule {
            match rule_globs(&content) {
                Ok(globs) => globs,
                Err(reason) => {
                    self.limit(global, false, reason);
                    return true;
                }
            }
        } else {
            Vec::new()
        };
        let absolute: PathBuf = path.components().collect();
        let Some(path) = absolute.to_str() else {
            self.limit(global, false, "path_not_utf8");
            return true;
        };
        let scope = match instruction_scope(global, scope, globs) {
            Ok(scope) => scope,
            Err(reason) => {
                self.limit(global, false, reason);
                return true;
            }
        };
        let instruction = CurrentInstruction {
            path: path.to_owned(),
            content,
            observed_at: self.observed_at.clone(),
            scope,
        };
        if let Some(entries) = &mut self.root(global).instructions {
            entries.push(instruction);
            self.instructions_left -= 1;
        }
        true
    }

    fn rules(
        &mut self,
        directory: &Path,
        safety: &Path,
        global: bool,
        scope: Option<&Path>,
        depth: usize,
    ) {
        if depth > 4 {
            self.limit(global, false, "rule_depth_limit");
            return;
        }
        for path in self.entries(directory, safety, global, false) {
            if let Err(reason) = self.reads.admission.check(&path) {
                self.limit(global, false, reason);
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                self.limit(global, false, "rule_entry_unavailable");
                continue;
            };
            if metadata.file_type().is_symlink() {
                self.limit(global, false, "symlink_not_read");
            } else if metadata.is_dir() {
                self.rules(&path, safety, global, scope, depth + 1);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                self.instruction(&path, safety, global, scope, true);
            }
        }
    }

    pub(super) fn collect_opencode_config(
        &mut self,
        producer: Producer,
        home: Option<&Path>,
        workspace: Option<(&Path, &Path)>,
    ) {
        let mut configs = Vec::new();
        if let Some(home) = home {
            for name in ["opencode.json", "opencode.jsonc"] {
                configs.push((home.join(".config/opencode").join(name), home, true));
            }
        }
        if let Some((root, cwd)) = workspace
            && let Ok(directories) = hierarchy(cwd, root)
        {
            for directory in directories {
                for name in [
                    "opencode.json",
                    "opencode.jsonc",
                    ".opencode/opencode.json",
                    ".opencode/opencode.jsonc",
                ] {
                    configs.push((directory.join(name), root, false));
                }
            }
        }
        for (path, safety, global) in configs {
            let bytes = match self.reads.read(&path, safety, false) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => continue,
                Err(reason) => {
                    self.unavailable(global, reason);
                    continue;
                }
            };
            let document = match super::super::vendors::json::parse(&bytes) {
                Ok(value) if value.is_object() => value,
                _ => {
                    self.unavailable(global, "opencode_config_invalid");
                    continue;
                }
            };
            if let Some(value) = document.get("instructions") {
                for entry in self.config_paths(value, global, false) {
                    let targets = self.instruction_targets(&entry, home, workspace, global);
                    for target in targets {
                        match allowed_root(&target, home, workspace, self.reads.admission) {
                            Ok(Some(safety)) => {
                                let scope = if global {
                                    None
                                } else {
                                    workspace.map(|(root, _)| root)
                                };
                                self.instruction(&target, &safety, global, scope, false);
                            }
                            Ok(None) => {
                                self.limit(global, false, "configured_path_outside_allowed_roots");
                            }
                            Err(reason) => self.limit(global, false, reason),
                        }
                    }
                }
            }
            if let Some(value) = document.get("skills") {
                let value = if producer == Producer::OpenCodeV1 {
                    match value.as_object() {
                        Some(object) => {
                            if object.contains_key("urls") {
                                self.limit(global, true, "remote_skill_catalog_not_read");
                            }
                            object.get("paths")
                        }
                        None => {
                            self.limit(global, true, "opencode_v1_skills_shape_unsupported");
                            None
                        }
                    }
                } else {
                    Some(value)
                };
                if let Some(value) = value {
                    for entry in self.config_paths(value, global, true) {
                        let target = if let Some(relative) = entry.strip_prefix("~/") {
                            home.map(|home| home.join(relative))
                        } else if Path::new(&entry).is_absolute() {
                            Some(PathBuf::from(&entry))
                        } else {
                            workspace.map(|(_, cwd)| cwd.join(&entry))
                        };
                        if let Some(target) = target {
                            match allowed_root(&target, home, workspace, self.reads.admission) {
                                Ok(Some(safety)) => self.skill_source(
                                    &target,
                                    &safety,
                                    global,
                                    SkillTraversal::for_opencode(producer),
                                    0,
                                ),
                                Ok(None) => {
                                    self.limit(
                                        global,
                                        true,
                                        "configured_path_outside_allowed_roots",
                                    );
                                }
                                Err(reason) => self.limit(global, true, reason),
                            }
                        } else {
                            self.limit(global, true, "configured_path_root_unavailable");
                        }
                    }
                }
            }
        }
    }

    fn config_paths(&mut self, value: &Value, global: bool, skills: bool) -> Vec<String> {
        let Some(values) = value.as_array() else {
            self.limit(global, skills, "configured_sources_shape_unsupported");
            return Vec::new();
        };
        let mut paths = Vec::new();
        let cap = self.config_entries_left.min(64);
        if values.len() > cap {
            self.limit(global, skills, "configured_source_limit");
        }
        for value in values.iter().take(cap) {
            self.config_entries_left -= 1;
            match value.as_str() {
                Some(path)
                    if !path.is_empty()
                        && path.len() <= 4096
                        && !path.chars().any(char::is_control) =>
                {
                    if path.contains("://") {
                        self.limit(global, skills, "remote_source_not_read");
                    } else if has_parent_component(Path::new(path)) {
                        // Windows verbatim joins remove parent components.
                        self.limit(global, skills, "path_outside_allowed_root");
                    } else if Path::new(path).components().any(|component| {
                        !matches!(component, Component::Prefix(_))
                            && component
                                .as_os_str()
                                .as_encoded_bytes()
                                .iter()
                                .any(|byte| b"*?[{!".contains(byte))
                    }) {
                        self.limit(global, skills, "dynamic_or_glob_source_not_read");
                    } else {
                        paths.push(path.to_owned());
                    }
                }
                _ => self.limit(global, skills, "configured_source_unsupported"),
            }
        }
        paths
    }

    fn instruction_targets(
        &mut self,
        entry: &str,
        home: Option<&Path>,
        workspace: Option<(&Path, &Path)>,
        global: bool,
    ) -> Vec<PathBuf> {
        if let Some(relative) = entry.strip_prefix("~/") {
            match home {
                Some(home) => vec![home.join(relative)],
                None => {
                    self.limit(global, false, "configured_path_root_unavailable");
                    Vec::new()
                }
            }
        } else if Path::new(entry).is_absolute() {
            vec![entry.into()]
        } else if let Some((root, cwd)) = workspace {
            match hierarchy(cwd, root) {
                Ok(directories) => directories
                    .into_iter()
                    .map(|directory| directory.join(entry))
                    .collect(),
                Err(reason) => {
                    self.limit(global, false, reason);
                    Vec::new()
                }
            }
        } else {
            self.limit(global, false, "configured_path_root_unavailable");
            Vec::new()
        }
    }
}

const PI_INSTRUCTIONS: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];

pub(super) fn instruction_scope(
    global: bool,
    root: Option<&Path>,
    globs: Vec<String>,
) -> Result<CurrentInstructionScope, &'static str> {
    if global && globs.is_empty() {
        return Ok(CurrentInstructionScope { root: None, globs });
    }
    let root = root.ok_or("conditional_scope_root_unavailable")?;
    let root = root
        .to_str()
        .filter(|_| root.is_absolute())
        .ok_or("scope_root_unavailable")?;
    Ok(CurrentInstructionScope {
        root: Some(root.to_owned()),
        globs,
    })
}

fn allowed_root(
    path: &Path,
    home: Option<&Path>,
    workspace: Option<(&Path, &Path)>,
    admission: PathAdmission<'_>,
) -> Result<Option<PathBuf>, &'static str> {
    for root in workspace.map(|(root, _)| root).into_iter().chain(home) {
        if let Some(root) = admission.configured_root(path, root)? {
            return Ok(Some(root));
        }
    }
    Ok(None)
}

fn rule_globs(content: &str) -> Result<Vec<String>, &'static str> {
    let mut lines = content.lines();
    let first = lines.next().unwrap_or_default();
    if first != "---" {
        if content
            .trim_start_matches(|character: char| {
                character.is_whitespace() || character == '\u{feff}'
            })
            .starts_with("---")
        {
            return Err("rule_frontmatter_marker_unsupported");
        }
        return Ok(Vec::new());
    }
    let mut header = String::new();
    let mut closed = false;
    for line in lines {
        if line == "---" || line == "..." {
            closed = true;
            break;
        }
        if header.len().saturating_add(line.len() + 1) > super::read::MAX_HEADER_BYTES {
            return Err("rule_frontmatter_byte_limit");
        }
        header.push_str(line);
        header.push('\n');
    }
    if !closed {
        return Err("rule_frontmatter_unclosed");
    }
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&header).map_err(|_| "rule_frontmatter_invalid")?;
    if value.is_null() {
        return Ok(Vec::new());
    }
    let mapping = value.as_mapping().ok_or("rule_frontmatter_invalid")?;
    let Some(paths) = mapping.get("paths") else {
        return Ok(Vec::new());
    };
    let paths = match paths {
        serde_yaml_ng::Value::String(paths) => {
            paths.split(',').map(str::trim).map(str::to_owned).collect()
        }
        serde_yaml_ng::Value::Sequence(paths) => paths
            .iter()
            .map(|path| {
                path.as_str()
                    .map(str::to_owned)
                    .ok_or("conditional_scope_unsupported")
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("conditional_scope_unsupported"),
    };
    if paths.is_empty() || paths.len() > 32 {
        return Err("conditional_scope_unsupported");
    }
    for path in &paths {
        if path.is_empty()
            || path.len() > 1024
            || Path::new(path).is_absolute()
            || path.split('/').any(|part| part == "..")
            || path.contains(['{', '}', '[', ']', '\\', '!', ','])
            || path.chars().any(char::is_control)
        {
            return Err("conditional_scope_unsupported");
        }
    }
    Ok(paths)
}
