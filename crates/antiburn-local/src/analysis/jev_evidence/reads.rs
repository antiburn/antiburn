use super::{ContentAction, SourceFormat, parse_tool_input, tool_input_field, unwrap_arguments};
use crate::analysis::jev::JevInputField;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The unit is unknown unless an accepted producer schema defines it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevReadUnit {
    Lines,
    Bytes,
    #[default]
    Unknown,
}

/// Native numbers retain their producer indexing. No default range is inferred.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevReadExtent {
    pub unit: JevReadUnit,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub end_inclusive: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevReadRequest {
    pub reference_id: String,
    pub paths: Vec<String>,
    pub cwd: Option<String>,
    pub extent: JevReadExtent,
    /// Retain native range arguments, including invalid or conflicting values.
    pub native_extent: BTreeMap<String, Value>,
    pub truncated: bool,
    pub extent_contract: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevReadStatus {
    Success,
    Failed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevReadResultKind {
    File,
    Directory,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevReadResult {
    pub reference_id: String,
    pub request_reference_id: Option<String>,
    pub status: JevReadStatus,
    pub kind: JevReadResultKind,
    /// UTF-8 bytes of recorded result text, including producer framing.
    pub recorded_output_bytes: u64,
    /// This digest identifies the recorded output, not a file version.
    pub recorded_output_digest: String,
    pub returned_extent: Option<JevReadExtent>,
    pub truncated: bool,
    /// A producer-recorded file version, never a current-file reconstruction.
    pub recorded_file_version: Option<String>,
    pub extent_contract: Option<String>,
}

const OPENCODE_READ_CONTRACT: &str = "opencode/read.ts@652c090dc119b5f3dc1e5e0bf1c4b40d9721f0ef";
const PI_READ_CONTRACT: &str = "pi/core/tools/read.ts@b79e4cc834970cca69daebffab7df1da7d1e52c4";
const CLAUDE_READ_CONTRACT: &str = "https://platform.claude.com/docs/en/agent-sdk/typescript#read";
const CODEX_NATIVE_CONTRACT: &str = "openai/codex@d27764b82f7118f674371e6d6e76271d9d606edb";
const CLAUDE_NATIVE_CONTRACT: &str = "claude-code/2.1.278:retained-cli-text-read";
const PI_NATIVE_CONTRACT: &str = "pi/core/tools/read.ts@b79e4cc834970cca69daebffab7df1da7d1e52c4";

const EXTENT_KEYS: &[&str] = &[
    "offset",
    "limit",
    "start_line",
    "end_line",
    "startLine",
    "endLine",
    "start",
    "end",
    "byte_offset",
    "byte_limit",
    "pages",
];

pub(super) fn selected_read_input(text: &str) -> Option<String> {
    let value = unwrap_arguments(parse_tool_input(text));
    let paths = super::value_strings(
        &value,
        &[
            "file_path",
            "filePath",
            "filepath",
            "path",
            "paths",
            "target_file",
        ],
    );
    if paths.is_empty() {
        return None;
    }
    let mut selected = serde_json::json!({"paths": paths});
    for key in EXTENT_KEYS.iter().copied().chain(["cwd", "workdir"]) {
        if let Some(value) = value.get(key) {
            selected[key] = value.clone();
        }
    }
    Some(selected.to_string())
}

/// Build local read facts from recorded actions. Search and shell text are not reads.
/// Match only unique earlier calls within the same source and thread.
pub fn normalize_read_evidence(source: SourceFormat, actions: &mut [ContentAction]) {
    let mut calls: BTreeMap<_, Vec<RecordedReadCall>> = BTreeMap::new();
    let mut call_counts = BTreeMap::new();
    for action in actions.iter().filter(|action| action.kind == "tool_input") {
        if let Some(key) = call_key(action) {
            *call_counts.entry(key).or_insert(0usize) += 1;
        }
    }
    let mut directory_requests = std::collections::BTreeSet::new();
    for action in actions.iter_mut() {
        let native_request = action.metadata.read_request.take();
        let native_result = action.metadata.read_result.take();
        let Some(name) = action.tool_name.as_deref() else {
            continue;
        };
        if tool_input_field(name) != JevInputField::ReadFilePath {
            continue;
        }
        let key = call_key(action);
        if action.kind == "tool_input" {
            action.metadata.read_request = native_request
                .or_else(|| {
                    action
                        .normalized_fields
                        .as_ref()
                        .and_then(|fields| fields.values.get(&JevInputField::ReadFileRequest))
                        .and_then(|text| serde_json::from_str::<JevReadRequest>(text).ok())
                })
                .filter(|request| valid_codex_request(source, action, request))
                .map(|mut request| {
                    request.reference_id = action.reference.id.clone();
                    request
                })
                .or_else(|| read_request(source, action));
            if let Some(key) = key {
                calls.entry(key).or_default().push(RecordedReadCall {
                    reference_id: action.reference.id.clone(),
                    tool_name: name.to_owned(),
                    state: action.metadata.state,
                    request: action.metadata.read_request.clone(),
                });
            }
        } else if action.kind == "tool_result" {
            let matched = key
                .as_ref()
                .filter(|key| call_counts.get(*key) == Some(&1))
                .and_then(|key| calls.get(key))
                .filter(|calls| calls.len() == 1)
                .and_then(|calls| calls.first())
                .filter(|call| call.tool_name == name && call.request.is_some());
            let mut result = JevReadResult {
                reference_id: action.reference.id.clone(),
                request_reference_id: None,
                status: match action.metadata.state {
                    super::JevOperationState::Error => JevReadStatus::Failed,
                    _ => JevReadStatus::Unknown,
                },
                kind: JevReadResultKind::Unknown,
                recorded_output_bytes: action.text.len() as u64,
                recorded_output_digest: crate::checks::ignored_instructions::sha256_hex(
                    action.text.as_bytes(),
                ),
                returned_extent: None,
                truncated: action.truncated,
                recorded_file_version: None,
                extent_contract: None,
            };
            if let Some(native) =
                native_result.filter(|result| valid_native_result(source, action, matched, result))
            {
                result = native;
                result.reference_id = action.reference.id.clone();
            }
            result.request_reference_id = matched.map(|call| call.reference_id.clone());
            if source == SourceFormat::OpenCodeSqliteV2
                && let Some(call) = matched
            {
                result.status = match call.state {
                    super::JevOperationState::Error => JevReadStatus::Failed,
                    super::JevOperationState::Completed => JevReadStatus::Success,
                    _ => result.status,
                };
            }
            if source == SourceFormat::OpenCodeSqliteV2
                && name == "read"
                && result.status != JevReadStatus::Failed
                && action.text.len() <= crate::analysis::interface::MAX_CONTENT_PART_BYTES
                && !action.truncated
            {
                opencode_result(&action.text, &mut result);
            }
            if result.kind == JevReadResultKind::Directory
                && let Some(id) = &result.request_reference_id
            {
                directory_requests.insert(id.clone());
            }
            action.metadata.read_result = Some(result);
        }
    }
    for action in actions {
        if directory_requests.contains(&action.reference.id)
            && let Some(request) = &mut action.metadata.read_request
        {
            request.extent.unit = JevReadUnit::Unknown;
        }
    }
}

struct RecordedReadCall {
    reference_id: String,
    tool_name: String,
    state: super::JevOperationState,
    request: Option<JevReadRequest>,
}

fn call_key(action: &ContentAction) -> Option<(String, String, String)> {
    action
        .tool_call_id
        .as_ref()
        .filter(|id| !id.is_empty())
        .map(|call| {
            (
                action.reference.source_key_digest.clone(),
                action.reference.thread_digest.clone(),
                call.clone(),
            )
        })
}

fn native_binding(action: &ContentAction, field: JevInputField, pointer: &str, end: u64) -> bool {
    action.reference.stable
        && action.reference.native_record_id.is_some()
        && action.metadata.bindings.iter().any(|binding| {
            binding.field == field
                && binding.container == super::JevNativeFieldContainer::Record
                && binding.native_record_id == action.reference.native_record_id
                && binding.pointer == pointer
                && binding.start == 0
                && u64::try_from(binding.end).ok() == Some(end)
        })
}

fn valid_extent(extent: &JevReadExtent) -> bool {
    extent.unit == JevReadUnit::Lines
        && extent
            .offset
            .zip(extent.limit)
            .zip(extent.end_inclusive)
            .is_some_and(|((start, count), end)| {
                start > 0 && count > 0 && start.checked_add(count - 1) == Some(end)
            })
}

fn valid_codex_request(
    source: SourceFormat,
    action: &ContentAction,
    request: &JevReadRequest,
) -> bool {
    if source != SourceFormat::CodexRolloutJsonl
        || action.tool_name.as_deref() != Some("read")
        || request.extent_contract.as_deref() != Some(CODEX_NATIVE_CONTRACT)
        || action.truncated
        || request.truncated
        || request.paths.len() != 1
        || !request.native_extent.is_empty()
        || !valid_extent(&request.extent)
    {
        return false;
    }
    let path = &request.paths[0];
    let expected = format!(
        "nl -ba {path} | sed -n '{},{}p'",
        request.extent.offset.expect("validated start"),
        request.extent.end_inclusive.expect("validated end")
    );
    let command_matches = if action.text.is_empty() {
        action
            .normalized_fields
            .as_ref()
            .and_then(|fields| fields.values.get(&JevInputField::ReadFileRequest))
            .and_then(|text| serde_json::from_str::<JevReadRequest>(text).ok())
            .is_some_and(|mut selected| {
                selected.reference_id = request.reference_id.clone();
                selected == *request
            })
    } else {
        parse_tool_input(&action.text)["cmd"].as_str() == Some(expected.as_str())
    };
    !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
        && command_matches
        && native_binding(
            action,
            JevInputField::ReadFileRequest,
            "/payload/item/command/2",
            expected.len() as u64,
        )
        && native_binding(
            action,
            JevInputField::ReadFilePath,
            "/payload/item/parsed_cmd/0/path",
            path.len() as u64,
        )
}

fn valid_native_result(
    source: SourceFormat,
    action: &ContentAction,
    call: Option<&RecordedReadCall>,
    result: &JevReadResult,
) -> bool {
    let Some(call) = call.filter(|call| call.request.is_some()) else {
        return false;
    };
    let (contract, field, pointer) = match (source, action.tool_name.as_deref()) {
        (SourceFormat::CodexRolloutJsonl, Some("read"))
            if call
                .request
                .as_ref()
                .and_then(|r| r.extent_contract.as_deref())
                == Some(CODEX_NATIVE_CONTRACT) =>
        {
            (
                CODEX_NATIVE_CONTRACT,
                JevInputField::ReadFileResult,
                "/payload/item/aggregated_output".to_owned(),
            )
        }
        (SourceFormat::ClaudeJsonl, Some("Read")) => {
            let Some(pointer) = action.metadata.bindings.iter().find_map(|binding| {
                (binding.field == JevInputField::ReadFileResult
                    && binding
                        .pointer
                        .strip_prefix("/message/content/")
                        .and_then(|rest| rest.strip_suffix("/content"))
                        .is_some_and(|index| index.parse::<u32>().is_ok()))
                .then(|| binding.pointer.clone())
            }) else {
                return false;
            };
            (
                CLAUDE_NATIVE_CONTRACT,
                JevInputField::ReadFileResult,
                pointer,
            )
        }
        (SourceFormat::PiV3Jsonl, Some("read")) => (
            PI_NATIVE_CONTRACT,
            JevInputField::ReadFileOutput,
            "/message/content/0/text".to_owned(),
        ),
        _ => return false,
    };
    let bound_bytes = if source == SourceFormat::PiV3Jsonl && result.returned_extent.is_some() {
        action
            .text
            .rsplit_once("\n\n[")
            .filter(|(_, notice)| notice.ends_with("to continue.]"))
            .map_or(result.recorded_output_bytes, |(body, _)| body.len() as u64)
    } else {
        result.recorded_output_bytes
    };
    let state_status = match action.metadata.state {
        super::JevOperationState::Completed => JevReadStatus::Success,
        super::JevOperationState::Error => JevReadStatus::Failed,
        _ => JevReadStatus::Unknown,
    };
    result.extent_contract.as_deref() == Some(contract)
        && native_binding(action, field, &pointer, bound_bytes)
        && matches!(
            result.kind,
            JevReadResultKind::Unknown | JevReadResultKind::File
        )
        && (source != SourceFormat::CodexRolloutJsonl || result.kind == JevReadResultKind::File)
        && result.status == state_status
        && result.recorded_file_version.is_none()
        && (!action.truncated || result.truncated)
        && result.recorded_output_bytes == action.text.len() as u64
        && result.recorded_output_digest
            == crate::checks::ignored_instructions::sha256_hex(action.text.as_bytes())
        && result.returned_extent.as_ref().is_none_or(|extent| {
            result.kind == JevReadResultKind::File
                && result.status == JevReadStatus::Success
                && !action.truncated
                && valid_extent(extent)
                && (source != SourceFormat::ClaudeJsonl || !result.truncated)
                && extent_matches_text(
                    source,
                    action,
                    call.request.as_ref().expect("request"),
                    extent,
                )
        })
}

fn extent_matches_text(
    source: SourceFormat,
    action: &ContentAction,
    request: &JevReadRequest,
    extent: &JevReadExtent,
) -> bool {
    if source == SourceFormat::PiV3Jsonl {
        let body = action
            .text
            .rsplit_once("\n\n[")
            .filter(|(_, notice)| notice.ends_with("to continue.]"))
            .map_or(action.text.as_str(), |(body, _)| body);
        return extent.offset == Some(request.extent.offset.unwrap_or(1))
            && extent.limit == Some(body.split('\n').count() as u64);
    }
    let mut expected = extent.offset.expect("validated offset");
    let mut count = 0u64;
    let limit = extent.limit.expect("validated count");
    for line in action.text.lines() {
        if source == SourceFormat::ClaudeJsonl && count == limit && line.is_empty() {
            continue;
        }
        let Some((number, _)) = line.split_once('\t') else {
            return false;
        };
        if number.trim().parse::<u64>().ok() != Some(expected) {
            return false;
        }
        count += 1;
        let Some(next) = expected.checked_add(1) else {
            return false;
        };
        expected = next;
    }
    extent.limit == Some(count)
        && (source != SourceFormat::CodexRolloutJsonl
            || (extent.offset == request.extent.offset
                && extent.end_inclusive <= request.extent.end_inclusive))
}

fn read_request(source: SourceFormat, action: &ContentAction) -> Option<JevReadRequest> {
    let text = action
        .normalized_fields
        .as_ref()
        .and_then(|fields| fields.values.get(&JevInputField::ReadFileRequest))
        .map(String::as_str)
        .unwrap_or(&action.text);
    let value = unwrap_arguments(parse_tool_input(text));
    let paths = super::value_strings(
        &value,
        &[
            "file_path",
            "filePath",
            "filepath",
            "path",
            "paths",
            "target_file",
        ],
    );
    if paths.is_empty() {
        return None;
    }
    let contexts: Vec<_> = ["cwd", "workdir"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .collect();
    let cwd = contexts
        .first()
        .filter(|first| contexts.iter().all(|cwd| cwd == *first))
        .map(|cwd| (*cwd).to_owned());
    let native_extent = EXTENT_KEYS
        .iter()
        .copied()
        .filter_map(|key| value.get(key).map(|v| (key.to_owned(), v.clone())))
        .collect();
    let contract = match (source, action.tool_name.as_deref()) {
        (SourceFormat::OpenCodeSqliteV2, Some("read")) => Some(OPENCODE_READ_CONTRACT),
        (SourceFormat::PiV3Jsonl, Some("read")) => Some(PI_READ_CONTRACT),
        (SourceFormat::ClaudeJsonl, Some("Read")) => Some(CLAUDE_READ_CONTRACT),
        _ => None,
    };
    Some(JevReadRequest {
        reference_id: action.reference.id.clone(),
        paths,
        cwd,
        extent: JevReadExtent {
            unit: if contract.is_some() {
                JevReadUnit::Lines
            } else {
                JevReadUnit::Unknown
            },
            offset: value.get("offset").and_then(Value::as_u64),
            limit: value.get("limit").and_then(Value::as_u64),
            end_inclusive: None,
        },
        native_extent,
        truncated: action.truncated,
        extent_contract: contract.map(str::to_owned),
    })
}

fn opencode_result(text: &str, result: &mut JevReadResult) {
    if !text.starts_with("<path>") {
        return;
    }
    if text.contains("\n<type>directory</type>\n<entries>\n") && text.ends_with("</entries>") {
        result.kind = JevReadResultKind::Directory;
        result.status = JevReadStatus::Success;
        result.extent_contract = Some(OPENCODE_READ_CONTRACT.to_owned());
        return;
    }
    let Some((_, content)) = text.split_once("\n<type>file</type>\n<content>\n") else {
        return;
    };
    let Some((content, _)) = content.split_once("\n</content>") else {
        return;
    };
    let content = if content.starts_with("\n\n") {
        content
    } else {
        content.strip_prefix('\n').unwrap_or(content)
    };
    let Some((body, footer)) = content.rsplit_once("\n\n") else {
        return;
    };
    if !(footer.starts_with("(End of file - total ") && footer.ends_with(" lines)"))
        && !(footer.starts_with("(Showing lines ") && footer.ends_with(" to continue.)"))
        && !(footer.starts_with("(Output capped at ") && footer.ends_with(" to continue.)"))
    {
        return;
    }
    let mut first = None;
    let mut last: Option<u64> = None;
    for line in body.lines() {
        let Some((number, _)) = line.split_once(": ") else {
            return;
        };
        let Ok(number) = number.parse::<u64>() else {
            return;
        };
        if number == 0 || last.is_some_and(|last| last.checked_add(1) != Some(number)) {
            return;
        }
        first.get_or_insert(number);
        last = Some(number);
    }
    if !valid_footer(footer, first, last) {
        return;
    }
    result.kind = JevReadResultKind::File;
    result.status = JevReadStatus::Success;
    result.truncated |= !footer.starts_with("(End of file") || body.contains("(line truncated to ");
    result.extent_contract = Some(OPENCODE_READ_CONTRACT.to_owned());
    if !result.truncated || !body.contains("(line truncated to ") {
        result.returned_extent = first.zip(last).map(|(first, last)| JevReadExtent {
            unit: JevReadUnit::Lines,
            offset: Some(first),
            limit: last.checked_sub(first).and_then(|n| n.checked_add(1)),
            end_inclusive: Some(last),
        });
    }
}

fn valid_footer(footer: &str, first: Option<u64>, last: Option<u64>) -> bool {
    if let Some(total) = footer
        .strip_prefix("(End of file - total ")
        .and_then(|s| s.strip_suffix(" lines)"))
    {
        return total
            .parse::<u64>()
            .is_ok_and(|total| last == Some(total) || (first.is_none() && total == 0));
    }
    let Some((first, last)) = first.zip(last) else {
        return false;
    };
    if let Some(rest) = footer.strip_prefix(&format!("(Showing lines {first}-{last} of ")) {
        let Some((total, suffix)) = rest.split_once(". Use offset=") else {
            return false;
        };
        return total.parse::<u64>().is_ok_and(|total| total > last)
            && last
                .checked_add(1)
                .is_some_and(|next| suffix == format!("{next} to continue.)"));
    }
    last.checked_add(1).is_some_and(|next| footer == format!("(Output capped at 50 KB. Showing lines {first}-{last}. Use offset={next} to continue.)"))
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
