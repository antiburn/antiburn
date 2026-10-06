use super::{JevInputField, JevInputSelection};
use serde::{Deserialize, Serialize};

/// The native lifecycle label does not prove an output or a successful command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevOperationState {
    #[default]
    Unknown,
    Pending,
    Running,
    Completed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevNativeFieldRange {
    pub field: JevInputField,
    pub container: JevNativeFieldContainer,
    /// JSON pointer within the native container, not within normalized text.
    pub pointer: String,
    /// UTF-8 offsets within the decoded native string, not serialized JSON bytes.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNativeFieldContainer {
    Record,
    Part,
    ToolBlock,
    Step,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevOperationMetadata {
    pub state: JevOperationState,
    pub bindings: Vec<JevNativeFieldRange>,
}

impl JevOperationMetadata {
    pub fn selected(&self, selection: JevInputSelection) -> Self {
        Self {
            state: self.state,
            bindings: self
                .bindings
                .iter()
                .filter(|binding| selection.includes(binding.field))
                .cloned()
                .collect(),
        }
    }
}

pub(crate) fn native_input_bindings(
    input: &serde_json::Value,
    pointer: &str,
    fields: &crate::analysis::jev::JevNormalizedFields,
    container: JevNativeFieldContainer,
) -> Vec<JevNativeFieldRange> {
    let mut bindings = Vec::new();
    let Some(object) = input.as_object() else {
        if let Some(text) = input.as_str()
            && fields
                .values
                .get(&JevInputField::BashCommandInput)
                .is_some_and(|selected| selected == text)
        {
            bindings.push(JevNativeFieldRange {
                field: JevInputField::BashCommandInput,
                container,
                pointer: pointer.to_owned(),
                start: 0,
                end: text.len(),
            });
        }
        return bindings;
    };
    for (key, value) in object.iter().take(256) {
        let Some(text) = value.as_str() else {
            continue;
        };
        if matches!(key.as_str(), "patchText" | "patch") {
            let selected_paths = fields
                .values
                .get(&JevInputField::FileEditPath)
                .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
                .and_then(|value| {
                    value
                        .get("paths")
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                })
                .unwrap_or_default();
            let selected_content = fields
                .values
                .get(&JevInputField::FileEditContent)
                .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
                .and_then(|value| {
                    value
                        .get("patch_content")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                });
            let mut offset = 0;
            for line in text.split_inclusive('\n').take(256) {
                let trimmed = line.trim_end_matches('\n');
                let path = [
                    "*** Update File: ",
                    "*** Add File: ",
                    "*** Delete File: ",
                    "*** Move to: ",
                ]
                .iter()
                .find_map(|prefix| trimmed.strip_prefix(prefix));
                let selected = path
                    .filter(|path| {
                        selected_paths
                            .iter()
                            .any(|item| item.as_str() == Some(*path))
                    })
                    .map(|path| {
                        (
                            JevInputField::FileEditPath,
                            offset + trimmed.len() - path.len(),
                            path.len(),
                        )
                    })
                    .or_else(|| {
                        selected_content
                            .as_ref()
                            .filter(|content| {
                                (trimmed.starts_with('+')
                                    || trimmed.starts_with('-')
                                    || trimmed.starts_with(' '))
                                    && content.lines().any(|part| part == trimmed)
                            })
                            .map(|_| (JevInputField::FileEditContent, offset, trimmed.len()))
                    });
                if let Some((field, start, len)) = selected {
                    bindings.push(JevNativeFieldRange {
                        field,
                        container,
                        pointer: format!("{pointer}/{key}"),
                        start,
                        end: start + len,
                    });
                }
                offset += line.len();
            }
            continue;
        }
        let field = match key.as_str() {
            "command" | "cmd" | "script" | "code" => JevInputField::BashCommandInput,
            "cwd" | "workdir" => {
                if fields.values.contains_key(&JevInputField::BashCommandInput) {
                    JevInputField::BashCommandInput
                } else if fields.values.contains_key(&JevInputField::FileEditPath) {
                    JevInputField::FileEditPath
                } else if fields.values.contains_key(&JevInputField::ReadFilePath) {
                    JevInputField::ReadFilePath
                } else {
                    JevInputField::SearchFilesQuery
                }
            }
            "file_path" | "filePath" | "filepath" | "path" | "target_file" | "old_path"
            | "new_path" | "oldPath" | "newPath" | "from" | "to" => {
                if fields.values.contains_key(&JevInputField::FileEditPath) {
                    JevInputField::FileEditPath
                } else if fields.values.contains_key(&JevInputField::ReadFilePath) {
                    JevInputField::ReadFilePath
                } else {
                    JevInputField::SearchFilesQuery
                }
            }
            "old_string" | "new_string" | "oldString" | "newString" | "oldText" | "newText"
            | "content" | "text" => JevInputField::FileEditContent,
            "query" | "search_query" | "searchQuery" | "pattern" | "regex" | "search_term"
            | "searchTerm" | "glob" | "include" | "exclude" => JevInputField::SearchFilesQuery,
            _ => continue,
        };
        if fields
            .values
            .get(&field)
            .is_some_and(|selected| native_string_is_selected(field, key, text, selected))
            && text.len() <= crate::analysis::interface::MAX_CONTENT_PART_BYTES
        {
            bindings.push(JevNativeFieldRange {
                field,
                container,
                pointer: format!("{pointer}/{key}"),
                start: 0,
                end: text.len(),
            });
        }
    }
    bindings
}

fn native_string_is_selected(field: JevInputField, key: &str, text: &str, selected: &str) -> bool {
    let command_key = matches!(key, "command" | "cmd" | "script" | "code");
    if field == JevInputField::BashCommandInput && command_key && selected == text {
        return true;
    }
    let Ok(selected) = serde_json::from_str::<serde_json::Value>(selected) else {
        return false;
    };
    selected.get(key).and_then(serde_json::Value::as_str) == Some(text)
        || (matches!(
            field,
            JevInputField::FileEditPath | JevInputField::ReadFilePath
        ) && selected
            .get("paths")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|paths| paths.iter().any(|path| path.as_str() == Some(text))))
        || (field == JevInputField::BashCommandInput
            && command_key
            && selected.get("command").and_then(serde_json::Value::as_str) == Some(text))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevPathPlatform {
    Posix,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JevGlobSyntax {
    PosixSegmentWildcards,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevGlobConstraint {
    Pattern,
    Include,
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevRecordedGlob {
    pub pattern: String,
    pub constraint: JevGlobConstraint,
}

/// These facts come only from a selected request. They do not access the filesystem.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevRecordedPathFacts {
    pub cwd: Option<String>,
    pub platform: Option<JevPathPlatform>,
    pub paths: Vec<String>,
    pub globs: Vec<JevRecordedGlob>,
    pub truncated: bool,
}

impl JevRecordedPathFacts {
    pub fn from_selected(
        field: JevInputField,
        text: &str,
        platform: Option<JevPathPlatform>,
    ) -> Self {
        let mut facts = Self {
            platform,
            ..Self::default()
        };
        if !matches!(
            field,
            JevInputField::BashCommandInput
                | JevInputField::FileEditPath
                | JevInputField::ReadFilePath
                | JevInputField::SearchFilesQuery
        ) {
            return facts;
        }
        if text.len() > crate::analysis::interface::MAX_CONTENT_PART_BYTES {
            facts.truncated = true;
            return facts;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return facts;
        };
        let contexts: Vec<_> = ["cwd", "workdir"]
            .iter()
            .filter_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
            .collect();
        facts.truncated = contexts.iter().any(|value| value.len() > 4096);
        facts.cwd = contexts
            .first()
            .filter(|first| {
                !first.is_empty()
                    && first.len() <= 4096
                    && contexts.iter().all(|value| value == *first)
            })
            .map(|value| (*value).to_owned());
        for key in ["paths", "globs"] {
            if let Some(values) = value.get(key).and_then(serde_json::Value::as_array) {
                facts.truncated |= values.len() > 256
                    || values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|value| value.len() > 4096);
                for item in values
                    .iter()
                    .take(256)
                    .filter_map(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty() && value.len() <= 4096)
                {
                    if key == "paths" {
                        facts.paths.push(item.to_owned());
                    } else {
                        facts.globs.push(JevRecordedGlob {
                            pattern: item.to_owned(),
                            constraint: JevGlobConstraint::Pattern,
                        });
                    }
                }
            }
        }
        for key in [
            "glob",
            "glob_pattern",
            "include",
            "include_glob",
            "includeGlob",
            "file_glob",
            "exclude",
            "exclude_glob",
            "excludeGlob",
        ] {
            if let Some(values) = value.get(key) {
                let items = values.as_str().into_iter().chain(
                    values
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_str),
                );
                for value in items {
                    if value.len() > 4096 || facts.globs.len() == 256 {
                        facts.truncated = true;
                        continue;
                    }
                    if value.is_empty() {
                        continue;
                    }
                    let constraint = if key.starts_with("exclude") {
                        JevGlobConstraint::Exclude
                    } else if key.starts_with("include") {
                        JevGlobConstraint::Include
                    } else {
                        JevGlobConstraint::Pattern
                    };
                    facts.globs.push(JevRecordedGlob {
                        pattern: value.to_owned(),
                        constraint,
                    });
                }
            }
        }
        if let Some(path) = value.get("path").and_then(serde_json::Value::as_str) {
            if path.len() <= 4096 && facts.paths.len() < 256 {
                facts.paths.push(path.to_owned());
            } else {
                facts.truncated = true;
            }
        }
        facts
    }

    /// Resolve only absolute POSIX paths and relative paths with a recorded absolute CWD.
    /// Windows, shell expansions, and parent traversal remain unknown.
    pub fn resolve(&self, path: &str) -> Option<String> {
        if self.platform != Some(JevPathPlatform::Posix)
            || path.is_empty()
            || path.len() > 4096
            || path.contains(['\0', '\\', '$', '~'])
            || path.split('/').any(|part| part == "..")
        {
            return None;
        }
        let joined = if path.starts_with('/') {
            path.to_owned()
        } else {
            let cwd = self.cwd.as_deref()?;
            if !cwd.starts_with('/')
                || cwd.len() > 4096
                || cwd.split('/').any(|part| part == "..")
                || cwd.contains(['\0', '\\', '$', '~'])
            {
                return None;
            }
            format!("{cwd}/{path}")
        };
        if joined.len() > 4096 {
            return None;
        }
        Some(format!(
            "/{}",
            joined
                .split('/')
                .filter(|part| !part.is_empty() && *part != ".")
                .collect::<Vec<_>>()
                .join("/")
        ))
    }

    pub fn matches_glob(&self, path: &str, pattern: &str, syntax: JevGlobSyntax) -> Option<bool> {
        match syntax {
            JevGlobSyntax::PosixSegmentWildcards => {}
        }
        if pattern.contains("**") || pattern.contains(['[', ']', '{', '}', '\\']) {
            return None;
        }
        let path = self.resolve(path)?;
        let pattern = self.resolve(pattern)?;
        let path: Vec<_> = path.chars().collect();
        let mut previous = vec![false; path.len() + 1];
        previous[0] = true;
        for token in pattern.chars() {
            let mut current = vec![false; path.len() + 1];
            current[0] = token == '*' && previous[0];
            for index in 1..=path.len() {
                current[index] = if token == '*' {
                    previous[index] || (path[index - 1] != '/' && current[index - 1])
                } else {
                    previous[index - 1]
                        && (token == path[index - 1] || (token == '?' && path[index - 1] != '/'))
                };
            }
            previous = current;
        }
        previous.last().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ranges_bind_only_retained_strings_and_do_not_invent_nested_offsets() {
        let input =
            serde_json::json!({"command":"printf é", "cmd":"EXCLUDED_ALIAS", "cwd":"/repo"});
        let fields =
            crate::analysis::jev_evidence::normalize_tool_input("bash", &input.to_string());
        let bindings = native_input_bindings(
            &input,
            "/state/input",
            &fields,
            JevNativeFieldContainer::Part,
        );
        assert_eq!(bindings.len(), 2);
        for binding in bindings {
            let key = binding.pointer.strip_prefix("/state/input/").unwrap();
            assert_ne!(key, "cmd");
            let text = input.get(key).unwrap().as_str().unwrap();
            assert_eq!(text.get(binding.start..binding.end), Some(text));
        }
        let encoded = serde_json::Value::String(input.to_string());
        assert!(
            native_input_bindings(
                &encoded,
                "/payload/arguments",
                &fields,
                JevNativeFieldContainer::Record
            )
            .is_empty()
        );
        let nested = serde_json::json!({"arguments": input});
        assert!(
            native_input_bindings(
                &nested,
                "/input",
                &fields,
                JevNativeFieldContainer::ToolBlock
            )
            .is_empty()
        );
        let patch = serde_json::json!({"patch":"*** Begin Patch\n*** Update File: src/a.rs\n+é\n*** End Patch"});
        let fields =
            crate::analysis::jev_evidence::normalize_tool_input("apply_patch", &patch.to_string());
        let bindings = native_input_bindings(
            &patch,
            "/input",
            &fields,
            JevNativeFieldContainer::ToolBlock,
        );
        assert_eq!(bindings.len(), 2);
        let original = patch["patch"].as_str().unwrap();
        assert_eq!(
            original.get(bindings[0].start..bindings[0].end),
            Some("src/a.rs")
        );
        assert_eq!(original.get(bindings[1].start..bindings[1].end), Some("+é"));
    }

    #[test]
    fn recorded_path_resolution_and_globs_keep_unknown_context_unknown() {
        let facts = JevRecordedPathFacts::from_selected(
            JevInputField::BashCommandInput,
            r#"{"command":"pwd","cwd":"/repo"}"#,
            Some(JevPathPlatform::Posix),
        );
        assert_eq!(facts.resolve("./src/é.rs"), Some("/repo/src/é.rs".into()));
        assert_eq!(
            facts.matches_glob("src/é.rs", "src/?.rs", JevGlobSyntax::PosixSegmentWildcards),
            Some(true)
        );
        assert_eq!(
            facts.matches_glob(
                "src/nested/a.rs",
                "src/*.rs",
                JevGlobSyntax::PosixSegmentWildcards
            ),
            Some(false)
        );
        assert_eq!(
            facts.matches_glob("src/a.rs", "**/*.rs", JevGlobSyntax::PosixSegmentWildcards),
            None
        );
        for path in ["../secret", "~/secret", "$HOME/secret", ""] {
            assert_eq!(facts.resolve(path), None);
        }
        assert_eq!(JevRecordedPathFacts::default().resolve("/repo/a"), None);
        let windows = JevRecordedPathFacts {
            platform: Some(JevPathPlatform::Windows),
            ..facts.clone()
        };
        assert_eq!(windows.resolve("C:\\repo\\a"), None);
        let no_cwd = JevRecordedPathFacts { cwd: None, ..facts };
        assert_eq!(no_cwd.resolve("src/a"), None);
        assert_eq!(no_cwd.resolve("/repo/a"), Some("/repo/a".into()));
    }

    #[test]
    fn path_facts_are_bounded_and_require_a_selected_path_request() {
        let text = serde_json::json!({"cwd": "x".repeat(4097), "paths": vec!["a"; 300], "globs": ["*.rs"]}).to_string();
        let facts = JevRecordedPathFacts::from_selected(JevInputField::FileEditPath, &text, None);
        assert_eq!(facts.cwd, None);
        assert_eq!(facts.paths.len(), 256);
        assert_eq!(
            facts.globs,
            [JevRecordedGlob {
                pattern: "*.rs".into(),
                constraint: JevGlobConstraint::Pattern
            }]
        );
        assert!(facts.truncated);
        let facts = JevRecordedPathFacts::from_selected(
            JevInputField::SearchFilesQuery,
            r#"{"cwd":"/a","workdir":"/b","include":"*.rs","exclude":"test*"}"#,
            None,
        );
        assert!(facts.cwd.is_none());
        assert_eq!(facts.globs[0].constraint, JevGlobConstraint::Include);
        assert_eq!(facts.globs[1].constraint, JevGlobConstraint::Exclude);
        assert_eq!(
            JevRecordedPathFacts::from_selected(JevInputField::ReadFileOutput, &text, None),
            JevRecordedPathFacts::default()
        );
    }
}
