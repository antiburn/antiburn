//! Shared normalized session evidence, field projection, and local citations.

use crate::analysis::SourceFormat;
use crate::analysis::evidence_query::PublishedContent;
use crate::analysis::jev::{
    JevFieldAvailability, JevFieldAvailabilityState, JevFieldCapability, JevInputField,
    JevInputSelection,
};
use crate::checks::ignored_instructions::{InstructionSnapshot, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod metadata;
pub use metadata::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentEventReference {
    pub id: String,
    pub source_key_digest: String,
    pub thread_digest: String,
    pub turn_index: u64,
    pub native_record_id: Option<String>,
    pub part_index: u32,
    pub stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentAction {
    pub reference: ContentEventReference,
    pub timestamp_ms: Option<i64>,
    pub turn_role: String,
    pub turn_scope: String,
    pub authority: String,
    pub kind: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub normalized_fields: Option<crate::analysis::jev::JevNormalizedFields>,
    #[serde(default)]
    pub metadata: JevOperationMetadata,
    pub truncated: bool,
    #[serde(default)]
    pub context_only: bool,
}

/// Return a digest for the exact normalized action content used by a finding.
pub fn content_action_digest(action: &ContentAction) -> String {
    let mut bytes = Vec::new();
    let timestamp = action.timestamp_ms.map(|value| value.to_string());
    for value in [
        timestamp.as_deref(),
        Some(action.turn_role.as_str()),
        Some(action.turn_scope.as_str()),
        Some(action.authority.as_str()),
        Some(action.kind.as_str()),
        Some(action.text.as_str()),
        action.tool_name.as_deref(),
        action.tool_call_id.as_deref(),
    ] {
        if let Some(value) = value {
            bytes.push(1);
            bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        } else {
            bytes.push(0);
        }
    }
    bytes.push(u8::from(action.truncated));
    bytes.extend(serde_json::to_vec(&action.metadata).expect("serialize operation metadata"));
    sha256_hex(&bytes)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContentEvidence {
    pub session_identity_digest: String,
    pub source_format: SourceFormat,
    pub publication_fence: i64,
    pub selected_input_digest: String,
    pub actions: Vec<ContentAction>,
    pub instructions: Vec<InstructionSnapshot>,
    pub complete: bool,
    pub limitations: Vec<String>,
    pub excluded_thinking_parts: u32,
    #[serde(default)]
    pub field_availability: Vec<JevFieldAvailability>,
}

/// Project normalized events to the fields declared by a Jev check.
pub fn select_session_content(
    content: &SessionContentEvidence,
    selection: JevInputSelection,
) -> SessionContentEvidence {
    let mut selected = SessionContentEvidence {
        actions: if source_supported(content.source_format) {
            content
                .actions
                .iter()
                .filter_map(|action| selected_action(action, selection))
                .collect()
        } else {
            Vec::new()
        },
        session_identity_digest: content.session_identity_digest.clone(),
        source_format: content.source_format,
        publication_fence: content.publication_fence,
        selected_input_digest: String::new(),
        instructions: content.instructions.clone(),
        complete: content.complete,
        limitations: content.limitations.clone(),
        excluded_thinking_parts: content.excluded_thinking_parts,
        field_availability: Vec::new(),
    };
    selected.field_availability =
        field_availability(content.source_format, &content.actions, selection);
    selected.limitations.retain(|limitation| {
        !matches!(
            limitation.as_str(),
            "no_usable_content_events"
                | "some_events_have_only_snapshot_local_ordinals"
                | "some_content_authority_is_unknown"
                | "some_tool_parts_lack_native_call_identity"
        )
    });
    if selected
        .actions
        .iter()
        .any(|action| !action.reference.stable)
    {
        selected
            .limitations
            .push("some_events_have_only_snapshot_local_ordinals".to_owned());
    }
    if selected
        .actions
        .iter()
        .any(|action| action.authority == "unknown")
    {
        selected
            .limitations
            .push("some_content_authority_is_unknown".to_owned());
    }
    if selected.actions.iter().any(|action| {
        matches!(action.kind.as_str(), "tool_input" | "tool_result")
            && (action.tool_name.is_none() || action.tool_call_id.is_none())
    }) {
        selected
            .limitations
            .push("some_tool_parts_lack_native_call_identity".to_owned());
    }
    if selected.actions.is_empty() {
        selected
            .limitations
            .push("no_selected_content_events".to_owned());
        selected.limitations.sort();
        selected.limitations.dedup();
        selected.complete = false;
    }
    if selected
        .field_availability
        .iter()
        .any(|field| field.state == JevFieldAvailabilityState::Unsupported)
    {
        selected
            .limitations
            .push("selected_field_unavailable_for_source".to_owned());
    }
    if selected
        .field_availability
        .iter()
        .any(|field| field.malformed_parts > 0)
    {
        selected
            .limitations
            .push("malformed_selected_tool_input".to_owned());
    }
    selected.limitations.sort();
    selected.limitations.dedup();
    selected.complete =
        selected.limitations.is_empty() && (content.complete || !content.limitations.is_empty());
    let digest = serde_json::to_vec(&(
        selection,
        &selected.session_identity_digest,
        selected.source_format,
        selected.publication_fence,
        selected.complete,
        &selected.limitations,
        selected
            .actions
            .iter()
            .map(|action| {
                (
                    &action.reference.id,
                    &action.reference.source_key_digest,
                    &action.reference.thread_digest,
                    action.reference.turn_index,
                    action.reference.part_index,
                    action.reference.stable,
                    action.context_only,
                    content_action_digest(action),
                )
            })
            .collect::<Vec<_>>(),
        selected
            .instructions
            .iter()
            .map(|instruction| {
                (
                    &instruction.id,
                    &instruction.digest,
                    instruction.provenance,
                    instruction.scope,
                )
            })
            .collect::<Vec<_>>(),
        &selected.field_availability,
    ))
    .expect("serialize the selected evidence identity");
    selected.selected_input_digest = sha256_hex(&digest);
    selected
}

fn selected_action(action: &ContentAction, selection: JevInputSelection) -> Option<ContentAction> {
    let field = if matches!(action.kind.as_str(), "assistant" | "assistant_text")
        && action.authority == "assistant"
    {
        JevInputField::AssistantMessage
    } else if matches!(action.kind.as_str(), "user" | "user_text") && action.authority == "user" {
        JevInputField::UserMessage
    } else if action.kind == "tool_input" {
        let tool_name = action.tool_name.as_deref()?;
        tool_input_field(tool_name)
    } else if action.kind == "tool_result" {
        let tool_name = action.tool_name.as_deref()?;
        tool_output_field(tool_name)
    } else {
        return None;
    };
    let mut selected = ContentAction {
        text: String::new(),
        normalized_fields: action.normalized_fields.as_ref().map(|normalized| {
            crate::analysis::jev::JevNormalizedFields {
                category: normalized.category,
                values: normalized
                    .values
                    .iter()
                    .filter(|(field, _)| selection.includes(**field))
                    .map(|(field, value)| (*field, value.clone()))
                    .collect(),
                malformed: normalized.malformed,
            }
        }),
        reference: action.reference.clone(),
        metadata: action.metadata.selected(selection),
        timestamp_ms: action.timestamp_ms,
        turn_role: action.turn_role.clone(),
        turn_scope: action.turn_scope.clone(),
        authority: action.authority.clone(),
        kind: action.kind.clone(),
        tool_name: action.tool_name.clone(),
        tool_call_id: action.tool_call_id.clone(),
        truncated: action.truncated,
        context_only: action.context_only,
    };
    if action.kind == "tool_input" && field == JevInputField::FileEditPath {
        let include_path = selection.includes(JevInputField::FileEditPath);
        let include_content = selection.includes(JevInputField::FileEditContent);
        if let Some(normalized) = &action.normalized_fields {
            let mut fields = Map::new();
            for field in [JevInputField::FileEditPath, JevInputField::FileEditContent] {
                if selection.includes(field)
                    && let Some(text) = normalized.values.get(&field)
                    && let Ok(Value::Object(values)) = serde_json::from_str::<Value>(text)
                {
                    fields.extend(values);
                }
            }
            selected.text = (!fields.is_empty()).then(|| Value::Object(fields).to_string())?;
            return Some(selected);
        }
        selected.text = selected_edit_input(&action.text, include_path, include_content)?;
        return Some(selected);
    }
    if !selection.includes(field) {
        return None;
    }

    if action.kind == "tool_input" {
        if let Some(normalized) = &action.normalized_fields {
            selected.text = normalized.values.get(&field)?.clone();
            return Some(selected);
        }
        selected.text = match field {
            JevInputField::BashCommandInput => selected_bash_input(&action.text)?,
            JevInputField::ReadFilePath => selected_path_input(&action.text)?,
            JevInputField::SearchFilesQuery => selected_search_input(&action.text)?,
            JevInputField::OtherToolInput => action.text.clone(),
            _ => return None,
        };
    } else {
        selected.text = action.text.clone();
    }
    Some(selected)
}

/// Normalize one recorded tool request into only fields checks can select.
/// The raw transcript text remains local for citations.
pub(crate) fn normalize_tool_input(
    name: &str,
    text: &str,
) -> crate::analysis::jev::JevNormalizedFields {
    use crate::analysis::jev::{JevNormalizedCategory, JevNormalizedFields};

    let field = tool_input_field(name);
    let values: std::collections::BTreeMap<JevInputField, String> = match field {
        JevInputField::BashCommandInput => selected_bash_input(text)
            .map(|value| [(field, value)].into_iter().collect())
            .unwrap_or_default(),
        JevInputField::FileEditPath => [
            (
                JevInputField::FileEditPath,
                selected_edit_input(text, true, false),
            ),
            (
                JevInputField::FileEditContent,
                selected_edit_input(text, false, true),
            ),
        ]
        .into_iter()
        .filter_map(|(field, value)| value.map(|value| (field, value)))
        .collect(),
        JevInputField::ReadFilePath => selected_path_input(text)
            .map(|value| [(field, value)].into_iter().collect())
            .unwrap_or_default(),
        JevInputField::SearchFilesQuery => selected_search_input(text)
            .map(|value| [(field, value)].into_iter().collect())
            .unwrap_or_default(),
        JevInputField::OtherToolInput => [(field, text.to_owned())].into_iter().collect(),
        _ => Default::default(),
    };
    let normalized_bytes = values
        .values()
        .fold(0usize, |total, value| total.saturating_add(value.len()));
    let malformed = (field != JevInputField::OtherToolInput && values.is_empty())
        || normalized_bytes > crate::analysis::interface::MAX_CONTENT_PART_BYTES;
    let category = match field {
        JevInputField::BashCommandInput => Some(JevNormalizedCategory::BashCommand),
        JevInputField::FileEditPath => Some(JevNormalizedCategory::FileEdit),
        JevInputField::ReadFilePath => Some(JevNormalizedCategory::ReadFile),
        JevInputField::SearchFilesQuery => Some(JevNormalizedCategory::SearchFiles),
        JevInputField::OtherToolInput => Some(JevNormalizedCategory::OtherTool),
        _ => None,
    };
    JevNormalizedFields {
        category,
        values: if malformed
            && normalized_bytes > crate::analysis::interface::MAX_CONTENT_PART_BYTES
        {
            Default::default()
        } else {
            values
        },
        malformed,
    }
}

fn normalized_tool_name(name: &str) -> String {
    name.rsplit(['.', '/', ':'])
        .next()
        .unwrap_or(name)
        .rsplit("__")
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
}

fn tool_input_field(name: &str) -> JevInputField {
    match normalized_tool_name(name).as_str() {
        "bash"
        | "shell"
        | "terminal"
        | "run_command"
        | "command"
        | "exec_command"
        | "run_terminal_command"
        | "execute_command" => JevInputField::BashCommandInput,
        "edit" | "write" | "edit_file" | "write_file" | "multi_edit" | "multiedit"
        | "apply_patch" | "applypatch" | "patch" => JevInputField::FileEditPath,
        "read" | "read_file" | "readfile" | "view_file" => JevInputField::ReadFilePath,
        "grep" | "glob" | "find" | "search" | "code_search" | "file_search" | "search_files"
        | "grep_search" | "codebase_search" | "list_files" => JevInputField::SearchFilesQuery,
        _ => JevInputField::OtherToolInput,
    }
}

fn tool_output_field(name: &str) -> JevInputField {
    match tool_input_field(name) {
        JevInputField::BashCommandInput => JevInputField::BashCommandOutput,
        JevInputField::FileEditPath => JevInputField::OtherToolOutput,
        JevInputField::ReadFilePath => JevInputField::ReadFileOutput,
        JevInputField::SearchFilesQuery => JevInputField::SearchFilesOutput,
        _ => JevInputField::OtherToolOutput,
    }
}

fn parse_tool_input(text: &str) -> Value {
    if text.len() > crate::analysis::interface::MAX_CONTENT_PART_BYTES {
        return Value::Null;
    }
    let value = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()));
    if tool_argument_shape_is_bounded(&value) {
        value
    } else {
        Value::Null
    }
}

fn tool_argument_shape_is_bounded(value: &Value) -> bool {
    let mut pending = vec![(value, 0)];
    let mut visited = 0;
    while let Some((value, depth)) = pending.pop() {
        visited += 1;
        if depth > 16 || visited > 4096 {
            return false;
        }
        match value {
            Value::Array(items) if items.len() <= 256 => {
                pending.extend(items.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(fields) if fields.len() <= 256 => {
                pending.extend(fields.values().map(|value| (value, depth + 1)));
            }
            Value::Array(_) | Value::Object(_) => return false,
            _ => {}
        }
    }
    true
}

fn unwrap_arguments(mut value: Value) -> Value {
    for _ in 0..4 {
        if let Some(nested) = ["arguments", "args", "params", "input"]
            .iter()
            .find_map(|key| value.get(*key))
        {
            value = nested.clone();
            continue;
        }
        if let Some(text) = value.as_str()
            && let Ok(decoded) = serde_json::from_str(text)
        {
            if !tool_argument_shape_is_bounded(&decoded) {
                return Value::Null;
            }
            value = decoded;
            continue;
        }
        break;
    }
    value
}

fn value_strings(value: &Value, keys: &[&str]) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(object) = value.as_object() {
        for key in keys {
            match object.get(*key) {
                Some(Value::String(value)) if !value.is_empty() => values.push(value.clone()),
                Some(Value::Array(items)) => values.extend(items.iter().flat_map(|item| {
                    let paths = if let Some(path) = item.as_str() {
                        vec![path]
                    } else {
                        [
                            "file_path",
                            "filePath",
                            "path",
                            "target_file",
                            "old_path",
                            "oldPath",
                            "new_path",
                            "newPath",
                        ]
                        .iter()
                        .filter_map(|key| item.get(*key).and_then(Value::as_str))
                        .collect()
                    };
                    paths
                        .into_iter()
                        .filter(|path| !path.is_empty())
                        .map(str::to_owned)
                })),
                _ => {}
            }
        }
    }
    values.sort();
    values.dedup();
    values
}

fn selected_path_input(text: &str) -> Option<String> {
    let value = unwrap_arguments(parse_tool_input(text));
    let patch = patch_text(&value);
    let mut paths = value_strings(
        &value,
        &[
            "file_path",
            "filePath",
            "filepath",
            "path",
            "paths",
            "files",
            "edits",
            "old_path",
            "new_path",
            "oldPath",
            "newPath",
            "target_file",
            "from",
            "to",
        ],
    );
    if paths.is_empty()
        && let Some(patch) = patch
    {
        for line in patch.lines() {
            let path = ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .or_else(|| line.strip_prefix("*** Move to: "))
                .or_else(|| line.strip_prefix("+++ b/"))
                .or_else(|| line.strip_prefix("--- a/"));
            if let Some(path) = path.filter(|path| !path.is_empty() && *path != "/dev/null") {
                paths.push(path.to_owned());
            }
        }
        paths.sort();
        paths.dedup();
    }
    if paths.is_empty() {
        return None;
    }
    let mut selected = serde_json::json!({"paths": paths});
    if let Some(patch) = patch {
        let operations = crate::analysis::jev::edit_hunks::apply_patch_path_operations(patch);
        if !operations.is_empty() {
            selected["operations"] = serde_json::to_value(operations).ok()?;
        }
    }
    for key in ["cwd", "workdir"] {
        if let Some(context) = value.get(key).filter(|value| value.is_string()) {
            selected[key] = context.clone();
        }
    }
    Some(selected.to_string())
}

fn selected_edit_input(text: &str, include_path: bool, include_content: bool) -> Option<String> {
    if !include_path && !include_content {
        return None;
    }
    let value = unwrap_arguments(parse_tool_input(text));
    let mut selected = Map::new();
    if include_path {
        let paths = selected_path_input(text)?;
        selected.extend(
            serde_json::from_str::<Value>(&paths)
                .ok()?
                .as_object()?
                .clone(),
        );
    }
    if include_content {
        let mut has_edit_content = false;
        if let Some(object) = value.as_object() {
            for key in [
                "old_string",
                "new_string",
                "oldString",
                "newString",
                "oldText",
                "newText",
                "content",
                "text",
                "edits",
            ] {
                if let Some(value) = object.get(key) {
                    if key != "edits" && !value.is_string() {
                        continue;
                    }
                    let value = if key == "edits" {
                        let Some(operations) = select_edit_operations(value) else {
                            continue;
                        };
                        operations
                    } else {
                        value.clone()
                    };
                    selected.insert(key.to_owned(), value);
                    has_edit_content = true;
                }
            }
        }
        if !has_edit_content {
            let patch = patch_text(&value);
            if let Some(patch) = patch {
                let patch_content = patch
                    .lines()
                    .filter(|line| {
                        !line.starts_with("*** Update File: ")
                            && !line.starts_with("*** Add File: ")
                            && !line.starts_with("*** Delete File: ")
                            && !line.starts_with("*** Move to: ")
                            && !line.starts_with("+++ ")
                            && !line.starts_with("--- ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !patch_content.is_empty() {
                    selected.insert("patch_content".to_owned(), Value::String(patch_content));
                }
            }
        }
    }
    (!selected.is_empty()).then(|| Value::Object(selected).to_string())
}

fn patch_text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("patchText").and_then(Value::as_str))
        .or_else(|| value.get("patch").and_then(Value::as_str))
        .or_else(|| value.get("input").and_then(Value::as_str))
}

fn select_edit_operations(value: &Value) -> Option<Value> {
    let operations = value.as_array()?;
    Some(Value::Array(
        operations
            .iter()
            .map(|operation| {
                let operation = operation.as_object()?;
                let mut selected = Map::new();
                for key in [
                    "old_string",
                    "new_string",
                    "oldString",
                    "newString",
                    "oldText",
                    "newText",
                    "content",
                    "text",
                ] {
                    if let Some(value) = operation.get(key).filter(|value| value.is_string()) {
                        selected.insert(key.to_owned(), value.clone());
                    }
                }
                (!selected.is_empty()).then_some(Value::Object(selected))
            })
            .collect::<Option<Vec<_>>>()?,
    ))
}

fn selected_bash_input(text: &str) -> Option<String> {
    let value = unwrap_arguments(parse_tool_input(text));
    let command = value.as_str().map(str::to_owned).or_else(|| {
        ["command", "cmd", "script", "code"].iter().find_map(|key| {
            value.get(*key).and_then(|value| {
                value.as_str().map(str::to_owned).or_else(|| {
                    value.as_array().and_then(|items| {
                        (items.len() <= 256 && items.iter().all(Value::is_string))
                            .then(|| value.to_string())
                    })
                })
            })
        })
    })?;
    let mut context = Map::new();
    for key in ["cwd", "workdir", "shell", "login", "timeout", "timeout_ms"] {
        if let Some(context_value) = value.get(key)
            && matches!(
                context_value,
                Value::String(_) | Value::Bool(_) | Value::Number(_)
            )
        {
            context.insert(key.to_owned(), context_value.clone());
        }
    }
    if context.is_empty() {
        Some(command)
    } else {
        context.insert("command".to_owned(), Value::String(command));
        Some(Value::Object(context).to_string())
    }
}

fn selected_search_input(text: &str) -> Option<String> {
    let value = unwrap_arguments(parse_tool_input(text));
    if let Some(query) = value.as_str() {
        return Some(serde_json::json!({"query": query}).to_string());
    }
    let mut selected = Map::new();
    for key in [
        "query",
        "search_query",
        "searchQuery",
        "pattern",
        "regex",
        "search_term",
        "searchTerm",
        "glob",
        "glob_pattern",
        "include",
        "include_glob",
        "includeGlob",
        "file_glob",
        "exclude",
        "exclude_glob",
        "excludeGlob",
        "path",
        "paths",
        "cwd",
        "case_sensitive",
    ] {
        if let Some(value) = value.get(key)
            && (matches!(value, Value::String(_) | Value::Bool(_))
                || value
                    .as_array()
                    .is_some_and(|items| items.len() <= 256 && items.iter().all(Value::is_string)))
        {
            selected.insert(key.to_owned(), value.clone());
        }
    }
    (!selected.is_empty()).then(|| Value::Object(selected).to_string())
}

/// Returns whether the source format has enough characterized content for this check.
pub const fn source_supported(source_format: SourceFormat) -> bool {
    matches!(
        source_format,
        SourceFormat::ClaudeJsonl
            | SourceFormat::CodexRolloutJsonl
            | SourceFormat::PiV3Jsonl
            | SourceFormat::OpenCodeSqliteV2
            | SourceFormat::CursorCliAgentJsonl
            | SourceFormat::CursorCliStoreDb
            | SourceFormat::CursorChatStoreDb
            | SourceFormat::CursorIdeComposer
            | SourceFormat::AntigravityBrainJsonl
            | SourceFormat::AntigravitySqlite
    )
}

const INPUT_FIELDS: [JevInputField; 12] = [
    JevInputField::UserMessage,
    JevInputField::AssistantMessage,
    JevInputField::BashCommandInput,
    JevInputField::BashCommandOutput,
    JevInputField::FileEditPath,
    JevInputField::FileEditContent,
    JevInputField::ReadFilePath,
    JevInputField::ReadFileOutput,
    JevInputField::SearchFilesQuery,
    JevInputField::SearchFilesOutput,
    JevInputField::OtherToolInput,
    JevInputField::OtherToolOutput,
];

/// Reports source-level support separately from fields observed on this page.
pub const fn field_capability(
    source_format: SourceFormat,
    field: JevInputField,
) -> JevFieldCapability {
    if !source_supported(source_format) {
        return JevFieldCapability::Unavailable;
    }
    if matches!(
        source_format,
        SourceFormat::CursorCliStoreDb
            | SourceFormat::CursorChatStoreDb
            | SourceFormat::CursorIdeComposer
    ) && !matches!(
        field,
        JevInputField::AssistantMessage | JevInputField::UserMessage
    ) {
        return JevFieldCapability::Unavailable;
    }
    match field {
        JevInputField::AssistantMessage
        | JevInputField::FileEditPath
        | JevInputField::ReadFilePath => JevFieldCapability::Supported,
        JevInputField::UserMessage
        | JevInputField::BashCommandInput
        | JevInputField::BashCommandOutput
        | JevInputField::FileEditContent
        | JevInputField::ReadFileOutput
        | JevInputField::SearchFilesQuery
        | JevInputField::SearchFilesOutput
        | JevInputField::OtherToolInput
        | JevInputField::OtherToolOutput => JevFieldCapability::Conditional,
    }
}

fn field_availability(
    source_format: SourceFormat,
    actions: &[ContentAction],
    selection: JevInputSelection,
) -> Vec<JevFieldAvailability> {
    INPUT_FIELDS
        .iter()
        .copied()
        .map(|field| {
            let selected = selection.includes(field);
            let field_selection = JevInputSelection::from_fields(&[field]);
            let capability = field_capability(source_format, field);
            let mut availability = JevFieldAvailability {
                field,
                selected,
                capability,
                state: if !selected {
                    JevFieldAvailabilityState::Excluded
                } else if capability == JevFieldCapability::Unavailable {
                    JevFieldAvailabilityState::Unsupported
                } else {
                    JevFieldAvailabilityState::NotObserved
                },
                observed_parts: 0,
                empty_parts: 0,
                malformed_parts: 0,
                truncated_parts: 0,
            };
            for action in actions {
                if !selected || capability == JevFieldCapability::Unavailable {
                    break;
                }
                if let Some(selected) = selected_action(action, field_selection) {
                    availability.observed_parts = availability.observed_parts.saturating_add(1);
                    if capability != JevFieldCapability::Unavailable {
                        availability.state = JevFieldAvailabilityState::Observed;
                    }
                    if selected.text.is_empty()
                        || (field == JevInputField::FileEditContent
                            && serde_json::from_str::<Value>(&selected.text)
                                .is_ok_and(|value| edit_content_is_empty(&value)))
                    {
                        availability.empty_parts = availability.empty_parts.saturating_add(1);
                    }
                    if selected.truncated {
                        availability.truncated_parts =
                            availability.truncated_parts.saturating_add(1);
                    }
                } else if action_matches_input_field(action, field) {
                    availability.malformed_parts = availability.malformed_parts.saturating_add(1);
                }
            }
            availability
        })
        .collect()
}

fn edit_content_is_empty(value: &Value) -> bool {
    match value {
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.iter().all(edit_content_is_empty),
        Value::Object(fields) => fields.values().all(edit_content_is_empty),
        Value::Null => true,
        _ => false,
    }
}

pub(crate) fn selected_evidence_store(
    actions: &[ContentAction],
    selection: JevInputSelection,
    publication_fence: i64,
) -> Result<crate::analysis::jev::JevEvidenceStore, crate::analysis::jev::JevError> {
    let mut store = crate::analysis::jev::JevEvidenceStore::for_publication(publication_fence);
    for action in actions {
        for field in INPUT_FIELDS {
            if selection.includes(field)
                && let Some(selected) =
                    selected_action(action, JevInputSelection::from_fields(&[field]))
            {
                store.insert(action.reference.id.clone(), field, selected.text)?;
            }
        }
    }
    Ok(store)
}

fn action_matches_input_field(action: &ContentAction, field: JevInputField) -> bool {
    if action.kind == "tool_input" {
        let Some(name) = action.tool_name.as_deref() else {
            return false;
        };
        return match tool_input_field(name) {
            JevInputField::FileEditPath => {
                matches!(
                    field,
                    JevInputField::FileEditPath | JevInputField::FileEditContent
                )
            }
            mapped => mapped == field,
        };
    }
    if action.kind == "tool_result" {
        return action
            .tool_name
            .as_deref()
            .is_some_and(|name| tool_output_field(name) == field);
    }
    matches!(
        (action.kind.as_str(), action.authority.as_str(), field),
        (
            "assistant" | "assistant_text",
            "assistant",
            JevInputField::AssistantMessage
        ) | ("user" | "user_text", "user", JevInputField::UserMessage)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentReferenceResolution<'a> {
    Found(&'a ContentAction),
    Stale,
}

/// Resolves a citation only against the exact assessment input that created it.
pub fn resolve_content_reference<'a>(
    evidence: &'a SessionContentEvidence,
    publication_fence: i64,
    selected_input_digest: &str,
    reference_id: &str,
) -> ContentReferenceResolution<'a> {
    if evidence.publication_fence != publication_fence
        || evidence.selected_input_digest != selected_input_digest
    {
        return ContentReferenceResolution::Stale;
    }
    evidence
        .actions
        .iter()
        .find(|action| action.reference.id == reference_id)
        .map(ContentReferenceResolution::Found)
        .unwrap_or(ContentReferenceResolution::Stale)
}

/// Convert bounded normalized content to local evidence. Apply check selection separately.
pub fn prepare_session_content(
    session_id: &str,
    source_format: SourceFormat,
    published: PublishedContent,
    instructions: Vec<InstructionSnapshot>,
) -> SessionContentEvidence {
    let mut actions = Vec::new();
    let mut limitations = Vec::new();
    let mut excluded_thinking_parts = 0u32;

    for item in published.parts {
        if item.part.kind.as_str() == "thinking" {
            excluded_thinking_parts = excluded_thinking_parts.saturating_add(1);
            continue;
        }
        let native_record_id = item.uuid.or(item.message_id);
        let stable = item.stable_event_identity && native_record_id.is_some();
        let source_key_digest = sha256_hex(item.source_key.as_bytes());
        let thread_digest = sha256_hex(item.thread_id.as_bytes());
        let identity = format!(
            "{}\0{}\0{}\0{}",
            source_key_digest,
            native_record_id.as_deref().unwrap_or("ordinal"),
            item.turn_index,
            item.part_index
        );
        let kind = item.part.kind.as_str();
        actions.push(ContentAction {
            reference: ContentEventReference {
                id: sha256_hex(identity.as_bytes()),
                source_key_digest,
                thread_digest,
                turn_index: item.turn_index,
                native_record_id,
                part_index: item.part_index,
                stable,
            },
            timestamp_ms: item.ts_ms,
            turn_role: item.role.to_owned(),
            turn_scope: item.scope,
            authority: item.part.authority.as_str().to_owned(),
            kind: kind.to_owned(),
            text: item.part.text,
            tool_name: item.part.tool_name,
            tool_call_id: item.part.tool_call_id,
            normalized_fields: item.part.normalized_fields,
            metadata: item.part.metadata,
            truncated: item.part.truncated,
            context_only: item.context_only,
        });
    }

    if published.coverage.parts_capped {
        limitations.push("content_part_limit".to_owned());
    }
    if published.coverage.bytes_capped {
        limitations.push("content_byte_limit".to_owned());
    }
    if published.coverage.context_capped {
        limitations.push("branch_context_limit".to_owned());
    }
    if published.coverage.oversized_parts > 0 {
        limitations.push("oversized_content_part".to_owned());
    }
    if published.coverage.stored_truncated_parts > 0 || actions.iter().any(|item| item.truncated) {
        limitations.push("truncated_source_content".to_owned());
    }
    if actions.is_empty() {
        limitations.push("no_usable_content_events".to_owned());
    }
    if actions.iter().any(|item| !item.reference.stable) {
        limitations.push("some_events_have_only_snapshot_local_ordinals".to_owned());
    }
    if actions.iter().any(|item| item.authority == "unknown") {
        limitations.push("some_content_authority_is_unknown".to_owned());
    }
    if actions.iter().any(|item| {
        matches!(item.kind.as_str(), "tool_input" | "tool_result")
            && (item.tool_name.is_none() || item.tool_call_id.is_none())
    }) {
        limitations.push("some_tool_parts_lack_native_call_identity".to_owned());
    }
    for instruction in &instructions {
        if instruction.provenance.as_str() == "current_file_comparison" {
            limitations.push("current_instruction_file_not_historical_proof".to_owned());
        }
        if instruction
            .sections
            .iter()
            .any(|section| !section.evaluable)
        {
            limitations.push("instruction_section_exceeds_limit".to_owned());
        }
        if !instruction.limitations.is_empty() {
            limitations.push("instruction_snapshot_has_limits".to_owned());
        }
    }
    limitations.sort();
    limitations.dedup();
    let complete = limitations.is_empty();

    let session_identity_digest = sha256_hex(session_id.as_bytes());
    let mut digest_input = Vec::with_capacity(
        actions
            .iter()
            .map(|action| action.text.len().saturating_add(128))
            .sum::<usize>()
            .saturating_add(instructions.len().saturating_mul(128)),
    );
    digest_input.extend_from_slice(session_identity_digest.as_bytes());
    digest_input.extend_from_slice(
        format!(
            "{source_format:?}:{}:{}:{}",
            crate::analysis::PARSER_REVISION,
            published.publication_fence,
            published.source_generation.unwrap_or_default()
        )
        .as_bytes(),
    );
    for action in &actions {
        digest_input.extend_from_slice(action.reference.id.as_bytes());
        digest_input.extend_from_slice(
            action
                .timestamp_ms
                .unwrap_or_default()
                .to_le_bytes()
                .as_slice(),
        );
        digest_input.extend_from_slice(action.turn_role.as_bytes());
        digest_input.extend_from_slice(action.turn_scope.as_bytes());
        digest_input.extend_from_slice(action.authority.as_bytes());
        digest_input.extend_from_slice(action.kind.as_bytes());
        digest_input.extend_from_slice(action.text.as_bytes());
        digest_input.extend_from_slice(action.tool_name.as_deref().unwrap_or("").as_bytes());
        digest_input.extend_from_slice(action.tool_call_id.as_deref().unwrap_or("").as_bytes());
        digest_input.push(u8::from(action.context_only));
    }
    for instruction in &instructions {
        digest_input.extend_from_slice(instruction.id.as_bytes());
        digest_input.extend_from_slice(instruction.digest.as_bytes());
        digest_input.extend_from_slice(instruction.provenance.as_str().as_bytes());
        digest_input.extend_from_slice(format!("{:?}", instruction.scope).as_bytes());
    }
    SessionContentEvidence {
        session_identity_digest,
        source_format,
        publication_fence: published.publication_fence,
        selected_input_digest: sha256_hex(&digest_input),
        actions,
        instructions,
        complete,
        limitations,
        excluded_thinking_parts,
        field_availability: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::evidence_query::{ContentQueryCoverage, PublishedContentPart};
    use crate::analysis::interface::{ContentAuthority, ContentKind, ContentPart};

    fn projection_action(
        id: &str,
        kind: &str,
        authority: &str,
        tool_name: Option<&str>,
        text: &str,
    ) -> ContentAction {
        ContentAction {
            reference: ContentEventReference {
                id: id.to_owned(),
                source_key_digest: "source".to_owned(),
                thread_digest: "thread".to_owned(),
                turn_index: 1,
                native_record_id: Some(id.to_owned()),
                part_index: 0,
                stable: true,
            },
            timestamp_ms: Some(1),
            turn_role: authority.to_owned(),
            turn_scope: "main".to_owned(),
            authority: authority.to_owned(),
            kind: kind.to_owned(),
            text: text.to_owned(),
            tool_name: tool_name.map(str::to_owned),
            tool_call_id: tool_name.map(|_| format!("call-{id}")),
            normalized_fields: None,
            metadata: Default::default(),
            truncated: false,
            context_only: false,
        }
    }

    fn projection_fixture() -> SessionContentEvidence {
        SessionContentEvidence {
            session_identity_digest: "session".to_owned(),
            source_format: SourceFormat::ClaudeJsonl,
            publication_fence: 1,
            selected_input_digest: "source-digest".to_owned(),
            actions: vec![
                projection_action("user", "user", "user", None, "USER_SENTINEL"),
                projection_action(
                    "assistant",
                    "assistant",
                    "assistant",
                    None,
                    "ASSISTANT_SENTINEL",
                ),
                projection_action(
                    "private-thinking",
                    "thinking",
                    "assistant",
                    None,
                    "PRIVATE_THINKING_SENTINEL",
                ),
                projection_action(
                    "bash-input",
                    "tool_input",
                    "assistant",
                    Some("Bash"),
                    r#"{"arguments":"{\"command\":\"BASH_SENTINEL\",\"description\":\"EXCLUDED_DESCRIPTION\"}"}"#,
                ),
                projection_action(
                    "bash-output",
                    "tool_result",
                    "tool",
                    Some("Bash"),
                    "BASH_OUTPUT_SENTINEL",
                ),
                projection_action(
                    "read-output",
                    "tool_result",
                    "tool",
                    Some("Read"),
                    "READ_OUTPUT_SENTINEL",
                ),
                projection_action(
                    "search-output",
                    "tool_result",
                    "tool",
                    Some("Grep"),
                    "SEARCH_OUTPUT_SENTINEL",
                ),
                projection_action(
                    "other-output",
                    "tool_result",
                    "tool",
                    Some("mcp__linear__create_issue"),
                    "OTHER_OUTPUT_SENTINEL",
                ),
                projection_action(
                    "edit-input",
                    "tool_input",
                    "assistant",
                    Some("Edit"),
                    r#"{"file_path":"src/edit.rs","old_string":"EDIT_OLD_SENTINEL","new_string":"EDIT_NEW_SENTINEL"}"#,
                ),
                projection_action(
                    "patch-edit",
                    "tool_input",
                    "assistant",
                    Some("apply_patch"),
                    r#"{"patch":"*** Update File: src/patch.rs\n@@\n-old-value\n+new-value\n*** End Patch"}"#,
                ),
                projection_action(
                    "read-input",
                    "tool_input",
                    "assistant",
                    Some("Read"),
                    r#"{"arguments":{"file_path":"src/read.rs","limit":100,"content":"READ_OUTPUT_SENTINEL"}}"#,
                ),
                projection_action(
                    "search-input",
                    "tool_input",
                    "assistant",
                    Some("Grep"),
                    r#"{"arguments":{"pattern":"SEARCH_SENTINEL","path":"src","include":"*.rs","matches":"SEARCH_OUTPUT_SENTINEL"}}"#,
                ),
                projection_action(
                    "other-input",
                    "tool_input",
                    "assistant",
                    Some("mcp__linear__create_issue"),
                    r#"{"title":"OTHER_INPUT_SENTINEL"}"#,
                ),
                projection_action(
                    "malformed-read",
                    "tool_input",
                    "assistant",
                    Some("Read"),
                    r#"{"content":"MALFORMED_KNOWN_TOOL_SENTINEL"}"#,
                ),
                projection_action(
                    "missing-tool-name",
                    "tool_input",
                    "assistant",
                    None,
                    r#"{"content":"MISSING_TOOL_NAME_SENTINEL"}"#,
                ),
            ],
            instructions: Vec::new(),
            complete: true,
            limitations: Vec::new(),
            excluded_thinking_parts: 0,
            field_availability: Vec::new(),
        }
    }

    #[test]
    fn selection_projects_only_the_declared_fields_for_all_4096_masks() {
        let mut content = projection_fixture();
        for action in &mut content.actions {
            if action.kind == "tool_input"
                && let Some(name) = &action.tool_name
            {
                action.normalized_fields = Some(normalize_tool_input(name, &action.text));
            }
        }
        let fields = [
            JevInputField::UserMessage,
            JevInputField::AssistantMessage,
            JevInputField::BashCommandInput,
            JevInputField::BashCommandOutput,
            JevInputField::FileEditPath,
            JevInputField::FileEditContent,
            JevInputField::ReadFilePath,
            JevInputField::ReadFileOutput,
            JevInputField::SearchFilesQuery,
            JevInputField::SearchFilesOutput,
            JevInputField::OtherToolInput,
            JevInputField::OtherToolOutput,
        ];
        let expected_ids: [&[&str]; 12] = [
            &["user"],
            &["assistant"],
            &["bash-input"],
            &["bash-output"],
            &["edit-input", "patch-edit"],
            &["edit-input", "patch-edit"],
            &["read-input"],
            &["read-output"],
            &["search-input"],
            &["search-output"],
            &["other-input"],
            &["other-output"],
        ];
        let field_sentinels: [&[&str]; 12] = [
            &["USER_SENTINEL"],
            &["ASSISTANT_SENTINEL"],
            &["BASH_SENTINEL"],
            &["BASH_OUTPUT_SENTINEL"],
            &["src/edit.rs"],
            &[
                "EDIT_OLD_SENTINEL",
                "EDIT_NEW_SENTINEL",
                "old-value",
                "new-value",
            ],
            &["src/read.rs"],
            &["READ_OUTPUT_SENTINEL"],
            &["SEARCH_SENTINEL"],
            &["SEARCH_OUTPUT_SENTINEL"],
            &["OTHER_INPUT_SENTINEL"],
            &["OTHER_OUTPUT_SENTINEL"],
        ];
        for mask in 0u16..(1 << fields.len()) {
            let selected_fields = fields
                .iter()
                .enumerate()
                .filter_map(|(index, field)| ((mask & (1 << index)) != 0).then_some(*field))
                .collect::<Vec<_>>();
            let selection = JevInputSelection::from_fields(&selected_fields);
            let projected = select_session_content(&content, selection);
            let serialized = serde_json::to_string(&projected.actions).unwrap();
            for (index, sentinels) in field_sentinels.iter().enumerate() {
                for sentinel in *sentinels {
                    assert_eq!(
                        serialized.contains(sentinel),
                        mask & (1 << index) != 0,
                        "sentinel {sentinel} for selection mask {mask:#05x}"
                    );
                }
            }
            assert!(
                !serialized.contains("PRIVATE_THINKING_SENTINEL"),
                "private thinking must not appear for selection mask {mask:#05x}"
            );
            let mut actual_ids = projected
                .actions
                .iter()
                .map(|action| action.reference.id.as_str())
                .collect::<Vec<_>>();
            actual_ids.sort_unstable();
            let expected = expected_ids
                .iter()
                .enumerate()
                .filter(|(index, _)| (mask & (1 << index)) != 0)
                .flat_map(|(_, ids)| ids.iter().copied())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                actual_ids
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>(),
                expected,
                "selection mask {mask:#05x}"
            );
        }
    }

    #[test]
    fn ignored_instructions_selection_excludes_user_messages_and_outputs() {
        let content = projection_fixture();
        let selection = JevInputSelection::from_fields(&[
            JevInputField::AssistantMessage,
            JevInputField::BashCommandInput,
            JevInputField::FileEditPath,
            JevInputField::ReadFilePath,
            JevInputField::SearchFilesQuery,
            JevInputField::OtherToolInput,
        ]);
        assert_eq!(
            selection,
            crate::checks::ignored_instructions::INPUT_SELECTION
        );
        let projected = select_session_content(&content, selection);
        let serialized = serde_json::to_string(&projected.actions).unwrap();
        for excluded in [
            "USER_SENTINEL",
            "BASH_OUTPUT_SENTINEL",
            "EDIT_OLD_SENTINEL",
            "EDIT_NEW_SENTINEL",
            "READ_OUTPUT_SENTINEL",
            "SEARCH_OUTPUT_SENTINEL",
            "EXCLUDED_DESCRIPTION",
            "MALFORMED_KNOWN_TOOL_SENTINEL",
            "MISSING_TOOL_NAME_SENTINEL",
        ] {
            assert!(
                !serialized.contains(excluded),
                "excluded sentinel {excluded}"
            );
        }
        assert!(serialized.contains("ASSISTANT_SENTINEL"));
        assert!(serialized.contains("BASH_SENTINEL"));
        assert!(serialized.contains("src/edit.rs"));
        assert!(serialized.contains("src/read.rs"));
        assert!(serialized.contains("SEARCH_SENTINEL"));
        assert!(serialized.contains("*.rs"));
        assert!(!serialized.contains("SEARCH_OUTPUT_SENTINEL"));
        assert!(serialized.contains("OTHER_INPUT_SENTINEL"));
        assert_eq!(projected.field_availability.len(), 12);
        assert_eq!(
            projected
                .field_availability
                .iter()
                .filter(|field| field.selected)
                .count(),
            6
        );
        assert!(projected.field_availability.iter().all(|field| {
            !field.selected
                || (field.capability != JevFieldCapability::Unavailable && field.observed_parts > 0)
        }));
        assert!(projected.field_availability.iter().any(|field| {
            field.field == JevInputField::BashCommandOutput
                && field.state == JevFieldAvailabilityState::Excluded
        }));
        assert_eq!(
            projected
                .field_availability
                .iter()
                .find(|field| field.field == JevInputField::ReadFilePath)
                .unwrap()
                .malformed_parts,
            1
        );

        let mut changed_excluded = content.clone();
        changed_excluded
            .actions
            .iter_mut()
            .find(|action| action.reference.id == "user")
            .unwrap()
            .text = "CHANGED_EXCLUDED_USER".to_owned();
        changed_excluded
            .actions
            .iter_mut()
            .find(|action| action.reference.id == "bash-output")
            .unwrap()
            .text = "CHANGED_EXCLUDED_OUTPUT".to_owned();
        changed_excluded
            .actions
            .iter_mut()
            .find(|action| action.reference.id == "edit-input")
            .unwrap()
            .text = r#"{"file_path":"src/edit.rs","old_string":"CHANGED_EDIT_BODY","new_string":"CHANGED_EDIT_RESULT"}"#.to_owned();
        assert_eq!(
            projected.selected_input_digest,
            select_session_content(&changed_excluded, selection).selected_input_digest
        );
        let selected_edit_content = select_session_content(
            &content,
            JevInputSelection::from_fields(&[JevInputField::FileEditContent]),
        );
        let changed_edit_content = select_session_content(
            &changed_excluded,
            JevInputSelection::from_fields(&[JevInputField::FileEditContent]),
        );
        assert_ne!(
            selected_edit_content.selected_input_digest, changed_edit_content.selected_input_digest,
            "an edit-content-enabled check can see the body change"
        );

        let edit_content = select_session_content(
            &content,
            JevInputSelection::from_fields(&[JevInputField::FileEditContent]),
        );
        assert!(edit_content.actions[0].text.contains("EDIT_OLD_SENTINEL"));
        assert!(!edit_content.actions[0].text.contains("src/edit.rs"));
        let edit_path_and_content = select_session_content(
            &content,
            JevInputSelection::from_fields(&[
                JevInputField::FileEditPath,
                JevInputField::FileEditContent,
            ]),
        );
        assert!(
            edit_path_and_content.actions[0]
                .text
                .contains("src/edit.rs")
        );
        assert!(
            edit_path_and_content.actions[0]
                .text
                .contains("EDIT_OLD_SENTINEL")
        );
        let patch_content = select_session_content(
            &content,
            JevInputSelection::from_fields(&[JevInputField::FileEditContent]),
        );
        let patch_content = patch_content
            .actions
            .iter()
            .find(|action| action.reference.id == "patch-edit")
            .unwrap();
        assert!(patch_content.text.contains("+new-value"));
        assert!(patch_content.text.contains("-old-value"));
        assert!(!patch_content.text.contains("src/patch.rs"));
    }

    #[test]
    fn path_only_selection_keeps_multi_file_and_rename_paths_and_rejects_missing_paths() {
        let mut content = projection_fixture();
        content.actions.push(projection_action(
            "missing-edit-path",
            "tool_input",
            "assistant",
            Some("Edit"),
            "{}",
        ));
        content
            .actions
            .iter_mut()
            .find(|action| action.reference.id == "edit-input")
            .unwrap()
            .text = r#"{"paths":["src/one.rs","src/two.rs"],"edits":[{"old_path":"src/old.rs","new_path":"src/new.rs"}]}"#.to_owned();

        let projected = select_session_content(
            &content,
            JevInputSelection::from_fields(&[JevInputField::FileEditPath]),
        );
        let path_action = projected
            .actions
            .iter()
            .find(|action| action.reference.id == "edit-input")
            .unwrap();
        for path in ["src/one.rs", "src/two.rs", "src/old.rs", "src/new.rs"] {
            assert!(path_action.text.contains(path), "missing path {path}");
        }
        assert!(
            projected
                .actions
                .iter()
                .all(|action| action.reference.id != "missing-edit-path")
        );
        assert!(projected.field_availability.iter().any(|field| {
            field.field == JevInputField::FileEditPath && field.malformed_parts == 1
        }));
    }

    #[test]
    fn normalized_projection_drops_large_excluded_bodies_and_keeps_field_bindings() {
        let mut content = projection_fixture();
        let edit = content
            .actions
            .iter_mut()
            .find(|action| action.reference.id == "edit-input")
            .unwrap();
        edit.text = serde_json::json!({
            "file_path": "src/edit.rs",
            "new_string": "EXCLUDED_EDIT_BODY".repeat(10_000),
        })
        .to_string();
        edit.normalized_fields = Some(normalize_tool_input("Edit", &edit.text));
        let selection = crate::checks::ignored_instructions::INPUT_SELECTION;
        let selected = select_session_content(&content, selection);
        let serialized = serde_json::to_string(&selected).unwrap();
        assert!(!serialized.contains("EXCLUDED_EDIT_BODY"));
        assert!(serialized.len() < 16_000);
        let store = selected_evidence_store(&selected.actions, selection, 1).unwrap();
        assert!(
            store
                .get("edit-input", JevInputField::FileEditPath)
                .unwrap()
                .contains("src/edit.rs")
        );
        assert!(
            store
                .get("edit-input", JevInputField::FileEditContent)
                .is_none()
        );
        assert!(store.total_bytes() < 1_000);

        let edit = content
            .actions
            .iter_mut()
            .find(|action| action.reference.id == "edit-input")
            .unwrap();
        edit.text =
            r#"{"file_path":"src/edit.rs","new_string":"REPLACED_EXCLUDED_BODY"}"#.to_owned();
        edit.normalized_fields = Some(normalize_tool_input("Edit", &edit.text));
        let changed = select_session_content(&content, selection);
        assert_eq!(
            selected.selected_input_digest,
            changed.selected_input_digest
        );
        assert_eq!(selected.actions, changed.actions);
        content.publication_fence += 1;
        assert_ne!(
            selected.selected_input_digest,
            select_session_content(&content, selection).selected_input_digest
        );
    }

    #[test]
    fn patch_rename_paths_do_not_leak_into_content_selection() {
        let patch = "*** Begin Patch\n*** Update File: src/old.rs\n*** Move to: src/new.rs\n@@\n-old\n+new\n*** End Patch";
        let normalized = normalize_tool_input("apply_patch", patch);
        let paths: Value =
            serde_json::from_str(&normalized.values[&JevInputField::FileEditPath]).unwrap();
        assert_eq!(
            paths["paths"],
            serde_json::json!(["src/new.rs", "src/old.rs"])
        );
        let body = &normalized.values[&JevInputField::FileEditContent];
        assert!(body.contains("+new"));
        assert!(body.contains("-old"));
        assert!(!body.contains("src/old.rs"));
        assert!(!body.contains("src/new.rs"));
    }

    #[test]
    fn opencode_native_patch_text_selects_paths_operations_and_optional_content() {
        let patch = "*** Begin Patch\n*** Update File: src/old.rs\n*** Move to: src/new.rs\n@@\n-old\n+new\n*** Add File: src/added.rs\n+added\n*** Delete File: src/deleted.rs\n*** End Patch";
        let native = serde_json::json!({"patchText":patch}).to_string();
        let fields = normalize_tool_input("apply_patch", &native);
        assert!(!fields.malformed);
        let selected: Value =
            serde_json::from_str(&fields.values[&JevInputField::FileEditPath]).unwrap();
        assert_eq!(
            selected["paths"],
            serde_json::json!(["src/added.rs", "src/deleted.rs", "src/new.rs", "src/old.rs"])
        );
        assert_eq!(
            selected["operations"],
            serde_json::json!([
                {"operation":"move","from":"src/old.rs","to":"src/new.rs"},
                {"operation":"add","path":"src/added.rs"},
                {"operation":"delete","path":"src/deleted.rs"}
            ])
        );
        assert!(!fields.values[&JevInputField::FileEditPath].contains("+added"));
        assert!(fields.values[&JevInputField::FileEditContent].contains("+added"));
        assert!(normalize_tool_input("apply_patch", "{}").malformed);
        assert!(normalize_tool_input("apply_patch", r#"{"patchText":""}"#).malformed);
    }

    #[test]
    fn bash_context_and_search_constraints_exclude_outputs_and_untyped_envelopes() {
        let bash = normalize_tool_input(
            "exec_command",
            r#"{"cmd":"printf synthetic","workdir":"/synthetic/work","shell":"/bin/sh","login":false,"yield_time_ms":1000,"max_output_chars":2000,"output":"EXCLUDED_OUTPUT"}"#,
        );
        let command: Value =
            serde_json::from_str(&bash.values[&JevInputField::BashCommandInput]).unwrap();
        assert_eq!(
            command,
            serde_json::json!({"command":"printf synthetic","workdir":"/synthetic/work","shell":"/bin/sh","login":false})
        );
        let search = normalize_tool_input(
            "Grep",
            r#"{"pattern":"needle","path":"src","include":["*.rs"],"case_sensitive":true,"matches":"EXCLUDED_MATCH","query":{"matches":"NESTED_OUTPUT"}}"#,
        );
        let query: Value =
            serde_json::from_str(&search.values[&JevInputField::SearchFilesQuery]).unwrap();
        assert_eq!(
            query,
            serde_json::json!({"pattern":"needle","path":"src","include":["*.rs"],"case_sensitive":true})
        );
        assert!(normalize_tool_input("Bash", r#"{"cmd":[{"output":"NOT_A_COMMAND"}]}"#).malformed);
        assert!(
            normalize_tool_input(
                "Read",
                &"x".repeat(crate::analysis::interface::MAX_CONTENT_PART_BYTES + 1)
            )
            .malformed
        );
        assert!(normalize_tool_input("Read", r#"{"arguments":{"arguments":{"arguments":{"arguments":{"arguments":{"path":"too-deep"}}}}}}"#).malformed);
    }

    #[test]
    fn edit_content_rejects_non_text_payloads_and_reports_empty_bodies() {
        for tool in ["Write", "Edit", "MultiEdit"] {
            let normalized = normalize_tool_input(
                tool,
                r#"{"path":"src/empty.rs","content":{"output":"NOT_EDIT_TEXT"},"newText":42}"#,
            );
            assert!(normalized.values.contains_key(&JevInputField::FileEditPath));
            assert!(
                !normalized
                    .values
                    .contains_key(&JevInputField::FileEditContent)
            );
        }
        let mut content = projection_fixture();
        content.actions = vec![projection_action(
            "empty-write",
            "tool_input",
            "assistant",
            Some("Write"),
            r#"{"path":"src/empty.rs","content":""}"#,
        )];
        let selected = select_session_content(
            &content,
            JevInputSelection::from_fields(&[JevInputField::FileEditContent]),
        );
        let availability = selected
            .field_availability
            .iter()
            .find(|item| item.field == JevInputField::FileEditContent)
            .unwrap();
        assert_eq!(availability.state, JevFieldAvailabilityState::Observed);
        assert_eq!(availability.empty_parts, 1);
        assert_eq!(availability.malformed_parts, 0);
        assert_eq!(selected.actions[0].text, r#"{"content":""}"#);
    }

    #[test]
    fn action_digest_binds_exact_content_but_not_page_context_status() {
        let action = ContentAction {
            reference: ContentEventReference {
                id: "action".to_owned(),
                source_key_digest: "source".to_owned(),
                thread_digest: "thread".to_owned(),
                turn_index: 1,
                native_record_id: Some("record".to_owned()),
                part_index: 0,
                stable: true,
            },
            timestamp_ms: Some(1),
            turn_role: "assistant".to_owned(),
            turn_scope: "main".to_owned(),
            authority: "agent".to_owned(),
            kind: "assistant_text".to_owned(),
            text: "Original action".to_owned(),
            tool_name: None,
            tool_call_id: None,
            normalized_fields: None,
            metadata: Default::default(),
            truncated: false,
            context_only: false,
        };
        let digest = content_action_digest(&action);
        let mut changed = action.clone();
        changed.text.push('!');
        assert_ne!(digest, content_action_digest(&changed));
        changed = action.clone();
        changed.context_only = true;
        assert_eq!(digest, content_action_digest(&changed));
    }

    #[test]
    fn preparation_is_vendor_neutral_and_excludes_thinking() {
        let content = PublishedContent {
            publication_fence: 1,
            source_generation: None,
            parts: vec![
                PublishedContentPart {
                    source_key: "source".to_owned(),
                    thread_id: "thread".to_owned(),
                    turn_index: 2,
                    role: "assistant",
                    scope: "main".to_owned(),
                    ts_ms: Some(10),
                    uuid: Some("event".to_owned()),
                    message_id: None,
                    part_index: 0,
                    part: ContentPart::new(ContentKind::AssistantText, "hello"),
                    context_only: false,
                    stable_event_identity: true,
                },
                PublishedContentPart {
                    source_key: "source".to_owned(),
                    thread_id: "thread".to_owned(),
                    turn_index: 2,
                    role: "assistant",
                    scope: "main".to_owned(),
                    ts_ms: Some(10),
                    uuid: Some("event".to_owned()),
                    message_id: None,
                    part_index: 1,
                    part: ContentPart::new(ContentKind::Thinking, "private thought"),
                    context_only: false,
                    stable_event_identity: true,
                },
            ],
            coverage: ContentQueryCoverage::default(),
            next_offset: 0,
        };
        let result = prepare_session_content(
            "test-session",
            SourceFormat::ClaudeJsonl,
            content,
            Vec::new(),
        );
        assert_eq!(result.actions.len(), 1);
        assert_eq!(
            result.actions[0].authority,
            ContentAuthority::Assistant.as_str()
        );
        assert_eq!(result.excluded_thinking_parts, 1);
        assert!(result.complete);
        assert!(matches!(
            resolve_content_reference(
                &result,
                result.publication_fence,
                &result.selected_input_digest,
                &result.actions[0].reference.id
            ),
            ContentReferenceResolution::Found(_)
        ));
        assert_eq!(
            resolve_content_reference(
                &result,
                result.publication_fence + 1,
                &result.selected_input_digest,
                &result.actions[0].reference.id
            ),
            ContentReferenceResolution::Stale
        );
    }

    #[test]
    fn unsupported_source_fields_are_reported_and_make_input_incomplete() {
        let content = projection_fixture();
        let selection = JevInputSelection::from_fields(&[JevInputField::AssistantMessage]);
        let projected = select_session_content(
            &SessionContentEvidence {
                source_format: SourceFormat::AntigravityCascadeJson,
                ..content
            },
            selection,
        );
        assert_eq!(projected.field_availability.len(), 12);
        let assistant = projected
            .field_availability
            .iter()
            .find(|field| field.field == JevInputField::AssistantMessage)
            .unwrap();
        assert_eq!(assistant.capability, JevFieldCapability::Unavailable);
        assert_eq!(assistant.state, JevFieldAvailabilityState::Unsupported);
        assert!(projected.actions.is_empty());
        assert!(
            projected
                .limitations
                .contains(&"selected_field_unavailable_for_source".to_owned())
        );
        assert!(!projected.complete);
    }

    #[test]
    fn preparation_preserves_outputs_for_output_enabled_selection() {
        let content = PublishedContent {
            publication_fence: 1,
            source_generation: Some(1),
            parts: vec![
                PublishedContentPart {
                    source_key: "source".to_owned(),
                    thread_id: "thread".to_owned(),
                    turn_index: 1,
                    role: "assistant",
                    scope: "main".to_owned(),
                    ts_ms: Some(1),
                    uuid: Some("edit-call".to_owned()),
                    message_id: None,
                    part_index: 0,
                    part: ContentPart::new(
                        ContentKind::ToolInput,
                        r#"{"file_path":"src/main.rs","old_string":"PRIVATE_OLD","new_string":"PRIVATE_NEW"}"#,
                    )
                    .with_tool_identity(Some("Edit".to_owned()), Some("call-1".to_owned())),
                    context_only: false,
                    stable_event_identity: true,
                },
                PublishedContentPart {
                    source_key: "source".to_owned(),
                    thread_id: "thread".to_owned(),
                    turn_index: 1,
                    role: "tool",
                    scope: "main".to_owned(),
                    ts_ms: Some(2),
                    uuid: Some("read-result".to_owned()),
                    message_id: None,
                    part_index: 1,
                    part: ContentPart::new(ContentKind::ToolResult, "PRIVATE_FILE_CONTENT")
                        .with_tool_identity(Some("Read".to_owned()), Some("call-2".to_owned())),
                    context_only: false,
                    stable_event_identity: true,
                },
                PublishedContentPart {
                    source_key: "source".to_owned(),
                    thread_id: "thread".to_owned(),
                    turn_index: 2,
                    role: "assistant",
                    scope: "main".to_owned(),
                    ts_ms: Some(3),
                    uuid: Some("shell-call".to_owned()),
                    message_id: None,
                    part_index: 0,
                    part: ContentPart::new(ContentKind::ToolInput, "cat private.txt")
                        .with_tool_identity(Some("Bash".to_owned()), Some("call-3".to_owned())),
                    context_only: false,
                    stable_event_identity: true,
                },
            ],
            coverage: ContentQueryCoverage::default(),
            next_offset: 0,
        };

        let result = prepare_session_content(
            "assessment-content-policy",
            SourceFormat::ClaudeJsonl,
            content,
            Vec::new(),
        );

        let serialized = serde_json::to_string(&result.actions).unwrap();
        assert!(serialized.contains("src/main.rs"));
        assert!(serialized.contains("PRIVATE_OLD"));
        assert!(serialized.contains("PRIVATE_NEW"));
        assert!(serialized.contains("PRIVATE_FILE_CONTENT"));
        assert!(serialized.contains("cat private.txt"));
        assert_eq!(
            result
                .actions
                .iter()
                .filter(|action| action.kind == "tool_input")
                .map(|action| action.text.as_str())
                .collect::<Vec<_>>(),
            [
                r#"{"file_path":"src/main.rs","old_string":"PRIVATE_OLD","new_string":"PRIVATE_NEW"}"#,
                "cat private.txt"
            ]
        );
        assert!(
            result
                .actions
                .iter()
                .any(|action| action.kind == "tool_result" && action.text == "PRIVATE_FILE_CONTENT")
        );
        let output_selected = select_session_content(
            &result,
            JevInputSelection::from_fields(&[JevInputField::ReadFileOutput]),
        );
        assert_eq!(output_selected.actions.len(), 1);
        assert_eq!(output_selected.actions[0].text, "PRIVATE_FILE_CONTENT");
        let ignored = select_session_content(
            &result,
            crate::checks::ignored_instructions::INPUT_SELECTION,
        );
        assert!(
            !serde_json::to_string(&ignored)
                .unwrap()
                .contains("PRIVATE_FILE_CONTENT")
        );
    }

    #[test]
    fn preparation_preserves_branch_scope_and_marks_ambiguous_or_truncated_parts() {
        let mut ambiguous = ContentPart::new(ContentKind::ToolInput, "run command")
            .with_authority(ContentAuthority::Unknown);
        ambiguous.truncated = true;
        let content = PublishedContent {
            publication_fence: 8,
            source_generation: Some(4),
            parts: vec![PublishedContentPart {
                source_key: "child-source".to_owned(),
                thread_id: "child-thread".to_owned(),
                turn_index: 3,
                role: "assistant",
                scope: "delegated".to_owned(),
                ts_ms: Some(20),
                uuid: Some("child-event".to_owned()),
                message_id: None,
                part_index: 0,
                part: ambiguous,
                context_only: false,
                stable_event_identity: true,
            }],
            coverage: ContentQueryCoverage {
                stored_truncated_parts: 1,
                ..ContentQueryCoverage::default()
            },
            next_offset: 0,
        };

        let result = prepare_session_content(
            "branch-session",
            SourceFormat::ClaudeJsonl,
            content,
            Vec::new(),
        );
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].turn_scope, "delegated");
        assert_eq!(result.actions[0].authority, "unknown");
        assert_eq!(
            result.actions[0].reference.thread_digest,
            sha256_hex(b"child-thread")
        );
        assert!(
            result
                .limitations
                .contains(&"some_content_authority_is_unknown".to_owned())
        );
        assert!(
            result
                .limitations
                .contains(&"some_tool_parts_lack_native_call_identity".to_owned())
        );
        assert!(
            result
                .limitations
                .contains(&"truncated_source_content".to_owned())
        );
        assert!(!result.complete);
    }

    #[test]
    fn ignored_instruction_source_gate_covers_native_routes_only() {
        for format in [
            SourceFormat::ClaudeJsonl,
            SourceFormat::CodexRolloutJsonl,
            SourceFormat::OpenCodeSqliteV2,
            SourceFormat::PiV3Jsonl,
            SourceFormat::CursorCliAgentJsonl,
            SourceFormat::CursorCliStoreDb,
            SourceFormat::CursorChatStoreDb,
            SourceFormat::CursorIdeComposer,
            SourceFormat::AntigravityBrainJsonl,
            SourceFormat::AntigravitySqlite,
        ] {
            assert!(source_supported(format), "{format:?}");
        }
        for format in [
            SourceFormat::CursorLegacyChatJson,
            SourceFormat::AntigravityWorkspaceChatJson,
            SourceFormat::AntigravityCascadeJson,
        ] {
            assert!(!source_supported(format), "{format:?}");
        }
    }

    #[test]
    fn synthesized_cursor_routes_cannot_claim_complete_tool_evidence() {
        for format in [
            SourceFormat::CursorCliStoreDb,
            SourceFormat::CursorChatStoreDb,
            SourceFormat::CursorIdeComposer,
        ] {
            let mut content = projection_fixture();
            content.source_format = format;
            let prepared = select_session_content(
                &content,
                crate::checks::ignored_instructions::INPUT_SELECTION,
            );
            assert!(!prepared.complete, "{format:?}");
            assert!(
                prepared
                    .limitations
                    .contains(&"selected_field_unavailable_for_source".to_owned())
            );
        }
    }
}
