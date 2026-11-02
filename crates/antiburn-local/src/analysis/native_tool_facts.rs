//! Check facts stay separate from metric categories and current session settings.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::SourceFormat;

const MAX_FACT_STRING_BYTES: usize = 4096;
const MAX_OPERATION_PATHS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckToolCategory {
    Shell,
    Read,
    Search,
    Edit,
    Write,
    Delegate,
    Skill,
    Mcp,
    Other,
}

/// Attribution belongs to the generating request, never to the result text.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRequestAttribution {
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Recorded mode evidence does not prove the provider's served tier.
    pub fast_mode: Option<bool>,
}

impl NativeRequestAttribution {
    pub(crate) fn observed(provider: Option<&str>, model: Option<&str>) -> Self {
        Self {
            provider: provider.and_then(identity),
            model: model.and_then(identity),
            fast_mode: None,
        }
    }

    /// Return the exact case-sensitive provider/model key, if both are known.
    pub fn model_key(&self) -> Option<String> {
        let provider = self
            .provider
            .as_deref()
            .filter(|value| !value.contains('/'))?;
        identity(provider)?;
        let model = self.model.as_deref()?;
        identity(model)?;
        Some(format!("{provider}/{model}"))
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.provider.as_ref().map_or(0, String::len) + self.model.as_ref().map_or(0, String::len)
    }
}

/// Complete supported operation targets with the exact recorded working directory.
/// Paths are lexical facts. They do not prove filesystem state or resolve symlinks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeOperationPaths {
    pub cwd: String,
    pub paths: Vec<String>,
}

impl NativeOperationPaths {
    pub(crate) fn observed(cwd: Option<&str>, paths: Vec<String>) -> Option<Self> {
        let cwd = cwd?;
        absolute_segments(cwd)?;
        if paths.len() > MAX_OPERATION_PATHS
            || paths.iter().any(|path| !valid_path(path))
            || paths.iter().map(String::len).sum::<usize>() > 64 * 1024
        {
            return None;
        }
        Some(Self {
            cwd: cwd.to_owned(),
            paths,
        })
    }

    /// Omit targets outside the repository. Reject unsupported path syntax.
    /// This supports POSIX absolute paths only; other platforms stay unavailable.
    pub fn repo_relative_paths(&self, repo_root: &str) -> Option<Vec<String>> {
        let root = absolute_segments(repo_root)?;
        let cwd = absolute_segments(&self.cwd)?;
        if self.paths.len() > MAX_OPERATION_PATHS
            || self.paths.iter().map(String::len).sum::<usize>() > 64 * 1024
        {
            return None;
        }
        let mut paths = Vec::new();
        for path in &self.paths {
            if !valid_path(path) {
                return None;
            }
            let target = if path.starts_with('/') {
                absolute_segments(path)?
            } else {
                let mut target = cwd.clone();
                target.extend(
                    path.split('/')
                        .filter(|part| !part.is_empty() && *part != "."),
                );
                target
            };
            if target == root {
                return None;
            }
            if target.starts_with(&root) {
                let relative = target[root.len()..].join("/");
                if !paths.contains(&relative) {
                    paths.push(relative);
                }
            }
        }
        Some(paths)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeToolFacts {
    pub requester: NativeRequestAttribution,
    pub category: Option<CheckToolCategory>,
    /// None includes unsupported targets and an incomplete target set.
    pub operation_paths: Option<NativeOperationPaths>,
}

impl NativeToolFacts {
    pub(crate) fn for_call(
        source: SourceFormat,
        name: &str,
        arguments: Option<&Value>,
        cwd: Option<&str>,
        requester: NativeRequestAttribution,
    ) -> Self {
        use CheckToolCategory as C;
        let (category, path_field) = match source {
            SourceFormat::ClaudeJsonl => match name {
                "Bash" => (C::Shell, None),
                "Read" => (C::Read, Some("file_path")),
                "Edit" => (C::Edit, Some("file_path")),
                "Write" => (C::Write, Some("file_path")),
                "Glob" | "Grep" => (C::Search, None),
                "Task" | "Agent" => (C::Delegate, None),
                "Skill" => (C::Skill, None),
                _ if claude_mcp_name(name) => (C::Mcp, None),
                _ => (C::Other, None),
            },
            SourceFormat::CodexRolloutJsonl => match name {
                "exec_command" | "shell" | "shell_command" => (C::Shell, None),
                "apply_patch" => (C::Edit, None),
                "spawn_agent" => (C::Delegate, None),
                "tool_search" | "web_search" => (C::Search, None),
                _ if codex_mcp_name(name) => (C::Mcp, None),
                _ => (C::Other, None),
            },
            SourceFormat::OpenCodeSqliteV1 => match name {
                "bash" => (C::Shell, None),
                // V1 records do not distinguish core tools from same-name custom tools.
                "read" => (C::Read, None),
                "edit" => (C::Edit, None),
                "write" => (C::Write, None),
                "grep" | "glob" => (C::Search, None),
                "task" => (C::Delegate, None),
                "skill" => (C::Skill, None),
                _ => (C::Other, None),
            },
            SourceFormat::OpenCodeSqliteV2 => match name {
                "shell" => (C::Shell, None),
                "read" => (C::Read, None),
                "patch" | "edit" => (C::Edit, None),
                "write" => (C::Write, None),
                "grep" | "glob" => (C::Search, None),
                "subagent" => (C::Delegate, None),
                "skill" => (C::Skill, None),
                _ => (C::Other, None),
            },
            SourceFormat::PiV3Jsonl => match name {
                "bash" => (C::Shell, None),
                // V3 records do not distinguish local tools from same-name overrides.
                "read" => (C::Read, None),
                "edit" => (C::Edit, None),
                "write" => (C::Write, None),
                "grep" | "find" | "ls" => (C::Search, None),
                _ => (C::Other, None),
            },
            _ => {
                return Self {
                    requester,
                    ..Self::default()
                };
            }
        };
        let operation_paths = path_field
            .and_then(|field| arguments?.get(field)?.as_str())
            .and_then(|path| NativeOperationPaths::observed(cwd, vec![path.to_owned()]));
        Self {
            requester,
            category: Some(category),
            operation_paths,
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.requester.retained_bytes()
            + self.operation_paths.as_ref().map_or(0, |paths| {
                paths.cwd.len() + paths.paths.iter().map(String::len).sum::<usize>()
            })
    }
}

fn claude_mcp_name(name: &str) -> bool {
    let Some(name) = name.strip_prefix("mcp__") else {
        return false;
    };
    name.split_once("__").is_some_and(|(server, tool)| {
        !server.is_empty() && !tool.is_empty() && !name.chars().any(char::is_whitespace)
    })
}

pub(crate) fn codex_mcp_name(name: &str) -> bool {
    let Some((server, tool)) = name
        .strip_prefix("mcp__")
        .and_then(|name| name.split_once("__"))
    else {
        return false;
    };
    if server.ends_with('_') || tool.starts_with('_') {
        return false;
    }
    [server, tool].iter().all(|part| {
        !part.is_empty()
            && part.len() <= MAX_FACT_STRING_BYTES
            && !part.contains("__")
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

pub(crate) fn codex_namespaced_mcp_name(namespace: &str, name: &str) -> Option<String> {
    let namespace = namespace.strip_prefix("mcp__")?;
    let server = namespace.strip_suffix("__").unwrap_or(namespace);
    let qualified = format!("mcp__{server}__{name}");
    codex_mcp_name(&qualified).then_some(qualified)
}

pub(crate) fn identity(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= MAX_FACT_STRING_BYTES
        && value.split('/').all(|part| !part.is_empty())
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        && !value.starts_with('<'))
    .then(|| value.to_owned())
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_FACT_STRING_BYTES
        && !path.contains(['\\', '\0', ':', '~'])
        && !path.split('/').any(|part| part == "..")
}

fn absolute_segments(path: &str) -> Option<Vec<&str>> {
    (path.starts_with('/') && !path.starts_with("//") && valid_path(path)).then(|| {
        path.split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect()
    })
}
