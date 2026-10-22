//! Tool semantics for the pinned OpenCode v2.0.32 message projection.

use serde_json::Value;

use super::{CapturedContent, SnapshotMessage};
use crate::analysis::jev::{JevInputField, JevNormalizedCategory, JevNormalizedFields};
use crate::analysis::jev_evidence::{
    JevAnswerSelection, JevNativeFieldContainer, JevNativeFieldRange, JevNativeSkillBody,
    JevNativeSkillOrigin, JevNativeSkillProducer, JevNativeSkillProof, JevOperationState,
    JevQuestionOption, JevReadExtent, JevReadRequest, JevReadResult, JevReadResultKind,
    JevReadStatus, JevReadUnit, JevRecordedSkillResult, JevScopeEvidenceProvenance,
    JevScopeEvidenceRole, JevScopeEvidenceSource, JevUserAnswer, JevUserAnswerOrigin,
    JevUserAnswerStatus,
};
use crate::analysis::{
    ContentAuthority, ContentKind, ContentPart, EVIDENCE_STRING_CAP, EvidenceObservation,
    NativeToolOutcome, NormalizedEvent, NormalizedRecord, RecordSink, RelationProvenance,
    SourceFormat, ToolCall, ToolCategory,
};

const PRODUCER: &str = "opencode@843a6c514e606b436a7fe713422ace0a93ad3d61";
const READ_CONTRACT: &str = "opencode/tool/plugin/read.ts@843a6c514e606b436a7fe713422ace0a93ad3d61";

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= 256)
}

fn unavailable_output(body: &str) -> bool {
    body.starts_with("Shell command output is no longer available.")
        || body == "[Old tool result content cleared]"
}

pub(super) fn valid_skill_name(name: &str) -> bool {
    crate::analysis::jev_evidence::skill_name_is_valid(name)
}

fn operation_state(status: &str) -> Option<JevOperationState> {
    Some(match status {
        "streaming" => JevOperationState::Pending,
        "running" => JevOperationState::Running,
        "completed" => JevOperationState::Completed,
        "error" => JevOperationState::Error,
        _ => return None,
    })
}

fn tool_call(name: &str, input: &Value) -> ToolCall {
    let mut tool = ToolCall::with_command(name, input.get("command").and_then(Value::as_str));
    tool.category = match name {
        "execute" => ToolCategory::Other,
        "patch" => ToolCategory::Edit,
        _ => tool.category,
    };
    tool
}

fn normalized_input(name: &str, input: &Value, raw: &str) -> JevNormalizedFields {
    let keys: &[&str] = match name {
        "read" => &["path", "offset", "limit"],
        "shell" => &["command", "workdir", "timeout", "background"],
        "patch" => &["patchText"],
        "write" => &["path", "content"],
        "edit" => &["path", "oldString", "newString", "replaceAll"],
        "grep" => &[
            "pattern",
            "path",
            "include",
            "literal",
            "caseSensitive",
            "limit",
        ],
        "glob" => &["pattern", "path", "hidden", "limit"],
        _ => {
            return JevNormalizedFields {
                category: Some(JevNormalizedCategory::OtherTool),
                values: [(JevInputField::OtherToolInput, raw.to_owned())]
                    .into_iter()
                    .collect(),
                malformed: false,
            };
        }
    };
    let selected: serde_json::Map<String, Value> = keys
        .iter()
        .filter_map(|key| {
            input
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect();
    crate::analysis::jev_evidence::normalize_tool_input(name, &Value::Object(selected).to_string())
}

fn native_input_valid(name: &str, input: &Value) -> bool {
    let required: &[&str] = match name {
        "read" => &["path"],
        "shell" => &["command"],
        "patch" => &["patchText"],
        "write" => &["path", "content"],
        "edit" => &["path", "oldString", "newString"],
        "grep" | "glob" => &["pattern"],
        "skill" => &["id"],
        "execute" => &["code"],
        "subagent" => &["agent", "description", "prompt"],
        _ => &[],
    };
    required
        .iter()
        .all(|key| input.get(*key).is_some_and(Value::is_string))
        && match name {
            "read" => ["offset", "limit"]
                .iter()
                .all(|key| input.get(*key).is_none_or(|value| value.as_u64().is_some())),
            "shell" => {
                input
                    .get("timeout")
                    .is_none_or(|value| value.as_u64().is_some())
                    && input.get("workdir").is_none_or(Value::is_string)
                    && input.get("background").is_none_or(Value::is_boolean)
            }
            "question" => input.get("questions").is_some_and(Value::is_array),
            "skill" => {
                input.as_object().is_some_and(|input| input.len() == 1)
                    && text(input.get("id")).is_some_and(|id| !id.chars().any(char::is_control))
            }
            _ => true,
        }
}

fn bind_input(part: &mut ContentPart, input: &Value, pointer: &str, message_id: &Option<String>) {
    if !part.truncated
        && let Some(fields) = &part.normalized_fields
    {
        part.metadata.bindings = crate::analysis::jev_evidence::native_input_bindings(
            input,
            pointer,
            fields,
            JevNativeFieldContainer::Record,
        );
        for binding in &mut part.metadata.bindings {
            binding.native_record_id = message_id.clone();
        }
    }
}

pub(super) fn capture_tool(
    value: &Value,
    index: usize,
    message: &SnapshotMessage,
    event: &mut NormalizedEvent,
    output: &mut CapturedContent,
) -> bool {
    let Some(name) = text(value.get("name")) else {
        return false;
    };
    let Some(call_id) = text(value.get("id")) else {
        return false;
    };
    let Some(state) = value.get("state").and_then(Value::as_object) else {
        return false;
    };
    let Some(status) = state.get("status").and_then(Value::as_str) else {
        return false;
    };
    let Some(operation) = operation_state(status) else {
        return false;
    };
    let Some(input) = state.get("input") else {
        return false;
    };
    if (status == "streaming" && !input.is_string())
        || (status != "streaming" && !input.is_object())
    {
        return false;
    }
    let pointer = format!("/content/{index}/state");
    let metadata = state.get("metadata");
    let provider_executed = value.get("executed").and_then(Value::as_bool);
    let local_execution = provider_executed != Some(true);
    let local_contract = local_execution && native_input_valid(name, input);
    if value
        .get("executed")
        .is_some_and(|executed| !executed.is_boolean())
    {
        return false;
    }
    let mut valid = metadata.is_none_or(Value::is_object);
    if status != "streaming" && local_execution {
        valid &= native_input_valid(name, input);
    }
    let first = output.parts.len();
    let input_text = input
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| input.to_string());
    output.push(
        ContentKind::ToolInput,
        &input_text,
        Some(name),
        Some(call_id),
    );
    if let Some(part) = output.parts.get_mut(first) {
        part.metadata.state = operation;
        if !part.truncated {
            part.normalized_fields = Some(normalized_input(name, input, &part.text));
        }
        // Provider execution does not prove the local built-in argument contract.
        if !local_execution {
            part.normalized_fields = None;
            output.incomplete = true;
        } else if !part.truncated {
            bind_input(part, input, &format!("{pointer}/input"), &event.uuid);
            if local_contract && name == "read" {
                part.metadata.read_request = read_request(input, &message.session_id);
                if let Some(path) = input
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|path| !path.is_empty() && path.len() <= 4096 && !path.contains('\0'))
                {
                    part.metadata.bindings.push(JevNativeFieldRange {
                        native_record_id: event.uuid.clone(),
                        field: JevInputField::ReadFileRequest,
                        container: JevNativeFieldContainer::Record,
                        pointer: format!("{pointer}/input/path"),
                        start: 0,
                        end: path.len(),
                    });
                }
            }
            if local_contract
                && name == "skill"
                && let Some(id) = text(input.get("id")).filter(|id| !id.contains('\0'))
            {
                part.metadata.bindings.push(JevNativeFieldRange {
                    native_record_id: event.uuid.clone(),
                    field: JevInputField::OtherToolInput,
                    container: JevNativeFieldContainer::Record,
                    pointer: format!("{pointer}/input/id"),
                    start: 0,
                    end: id.len(),
                });
                if let Some(message_id) = event.uuid.as_deref() {
                    part.metadata.recorded_skill_result =
                        Some(JevRecordedSkillResult::opencode_v2(
                            part,
                            if message.session.transfer {
                                SourceFormat::OpenCodeV2SessionExport
                            } else {
                                SourceFormat::OpenCodeSqliteV2
                            },
                            &message.session_id,
                            message_id,
                            first as u32,
                            JevNativeSkillProof {
                                producer: JevNativeSkillProducer::OpenCodeV2Skill843a6c51,
                                origin: JevNativeSkillOrigin::ToolRequest,
                                id: id.to_owned(),
                                display_name: None,
                                body: JevNativeSkillBody::NotApplicable,
                            },
                        ));
                }
            }
        }
    }
    let (nested, nested_valid) = if local_contract && name == "execute" {
        capture_nested_activity(metadata, &pointer, event, output)
    } else {
        (0, true)
    };
    valid &= nested_valid;
    if nested == 0 {
        if event.tools.len() < 1024 {
            let mut tool = if local_execution {
                tool_call(name, input)
            } else {
                let mut tool = ToolCall::new(name);
                tool.category = ToolCategory::Other;
                tool
            };
            if local_contract && name == "skill" {
                tool.detail = text(input.get("id"))
                    .filter(|id| valid_skill_name(id))
                    .map(str::to_owned);
            }
            event.tools.push(tool);
        } else {
            output.oversized = true;
        }
    } else {
        event.wrapper_tool = Some("execute".to_owned());
    }

    let result_start = output.parts.len();
    let incomplete_output = metadata
        .and_then(|value| value.get("truncated"))
        .and_then(Value::as_bool)
        == Some(true)
        || metadata
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            == Some("running");
    let content = state.get("content").and_then(Value::as_array);
    if (status == "completed" || state.contains_key("content")) && content.is_none() {
        valid = false;
    }
    if let Some(content) = content {
        if content.is_empty() {
            valid = false;
        }
        for (result_index, result) in content.iter().take(1024).enumerate() {
            match result.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let Some(body) = result.get("text").and_then(Value::as_str) else {
                        valid = false;
                        continue;
                    };
                    let part_index = output.parts.len();
                    output.push(ContentKind::ToolResult, body, Some(name), Some(call_id));
                    if let Some(part) = output.parts.get_mut(part_index) {
                        part.metadata.state = operation;
                        part.truncated |= incomplete_output || unavailable_output(body);
                        if !part.truncated && !body.is_empty() {
                            let field = if local_contract && name == "read" {
                                JevInputField::ReadFileResult
                            } else if name == "shell" {
                                JevInputField::BashCommandOutput
                            } else {
                                JevInputField::OtherToolOutput
                            };
                            part.metadata.bindings.push(JevNativeFieldRange {
                                native_record_id: event.uuid.clone(),
                                field,
                                container: JevNativeFieldContainer::Record,
                                pointer: format!("{pointer}/content/{result_index}/text"),
                                start: 0,
                                end: body.len(),
                            });
                        }
                        if local_contract && name == "read" {
                            part.metadata.read_result = Some(read_result(
                                body,
                                operation,
                                part.truncated,
                                input.get("path").and_then(Value::as_str),
                            ));
                        }
                        if local_contract && name == "skill" {
                            capture_skill_result(
                                part,
                                part_index,
                                message,
                                &pointer,
                                input,
                                metadata,
                                Some(&format!("{pointer}/content/{result_index}/text")),
                            );
                        }
                    }
                }
                Some("file") => output.omitted_non_text = true,
                _ => {
                    output.omitted_non_text = true;
                    valid = false;
                }
            }
        }
        if content.len() > 1024 {
            output.oversized = true;
        }
    }
    if status == "error" && content.is_none() {
        if let Some(error) = state
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            let part_index = output.parts.len();
            output.push(ContentKind::ToolResult, error, Some(name), Some(call_id));
            if let Some(part) = output.parts.get_mut(part_index) {
                part.metadata.state = JevOperationState::Error;
                part.truncated |= incomplete_output || unavailable_output(error);
                if !part.truncated {
                    part.metadata.bindings.push(JevNativeFieldRange {
                        native_record_id: event.uuid.clone(),
                        field: JevInputField::OtherToolOutput,
                        container: JevNativeFieldContainer::Record,
                        pointer: format!("{pointer}/error/message"),
                        start: 0,
                        end: error.len(),
                    });
                }
                if local_contract && name == "skill" {
                    capture_skill_result(
                        part,
                        part_index,
                        message,
                        &pointer,
                        input,
                        metadata,
                        Some(&format!("{pointer}/error/message")),
                    );
                }
            }
        } else {
            valid = false;
            output.incomplete = true;
        }
    }
    if local_contract
        && name == "skill"
        && matches!(status, "completed" | "error")
        && output.parts.len() == result_start
    {
        output.push(ContentKind::ToolResult, "", Some(name), Some(call_id));
        if let Some(part) = output.parts.get_mut(result_start) {
            part.metadata.state = operation;
            part.truncated |= incomplete_output;
            capture_skill_result(part, result_start, message, &pointer, input, metadata, None);
        }
    }
    let outcome = local_contract
        .then(|| {
            NativeToolOutcome::opencode_v2_tool(
                name,
                metadata,
                provider_executed,
                status == "error",
            )
        })
        .flatten();
    if local_contract && output.parts.len() == result_start && outcome.is_some() {
        output.push(ContentKind::ToolResult, "", Some(name), Some(call_id));
    }
    for part in &mut output.parts[result_start..] {
        part.truncated |= incomplete_output;
        part.metadata.outcome = outcome.clone();
        part.metadata.provider_executed = provider_executed;
        if outcome.is_some() || provider_executed.is_some() {
            part.metadata.tool_output_field =
                Some(crate::analysis::jev_evidence::tool_output_field(name));
        }
    }
    if local_contract && name == "read" {
        let file = output.parts[result_start..].iter().any(|part| {
            part.metadata
                .read_result
                .as_ref()
                .is_some_and(|result| result.kind == JevReadResultKind::File)
        });
        if file
            && let Some(request) = output
                .parts
                .get_mut(first)
                .and_then(|part| part.metadata.read_request.as_mut())
        {
            request.extent.unit = JevReadUnit::Lines;
        }
        for part in &mut output.parts[result_start..] {
            if part
                .metadata
                .read_result
                .as_ref()
                .is_some_and(|result| result.truncated)
            {
                part.truncated = true;
                part.metadata.bindings.clear();
                output.incomplete = true;
            }
        }
    }
    if local_contract
        && name == "question"
        && status == "completed"
        && let Some(part) = output.parts.get_mut(result_start)
    {
        match question_answers(input, metadata, message, call_id, part.truncated) {
            Some(answers)
                if serde_json::to_vec(&answers).is_ok_and(|bytes| bytes.len() <= 128 * 1024) =>
            {
                part.metadata.user_answers = answers
            }
            _ => valid = false,
        }
    }
    if incomplete_output
        || output.parts[result_start..]
            .iter()
            .any(|part| part.truncated)
    {
        output.incomplete = true;
    }
    valid
}

fn capture_skill_result(
    part: &mut ContentPart,
    part_index: usize,
    message: &SnapshotMessage,
    pointer: &str,
    input: &Value,
    metadata: Option<&Value>,
    body_pointer: Option<&str>,
) {
    let Some(id) = text(input.get("id")) else {
        return;
    };
    let Some(message_id) = message.value.get("id").and_then(Value::as_str) else {
        return;
    };
    let display_name = text(metadata.and_then(|metadata| metadata.get("name")))
        .filter(|name| !name.chars().any(char::is_control));
    part.metadata.bindings = vec![JevNativeFieldRange {
        native_record_id: Some(message_id.to_owned()),
        field: JevInputField::OtherToolOutput,
        container: JevNativeFieldContainer::Record,
        pointer: format!("{pointer}/input/id"),
        start: 0,
        end: id.len(),
    }];
    if let Some(name) = display_name {
        part.metadata.bindings.push(JevNativeFieldRange {
            native_record_id: Some(message_id.to_owned()),
            field: JevInputField::OtherToolOutput,
            container: JevNativeFieldContainer::Record,
            pointer: format!("{pointer}/metadata/name"),
            start: 0,
            end: name.len(),
        });
    }
    if let Some(body_pointer) = body_pointer.filter(|_| !part.text.is_empty()) {
        part.metadata.bindings.push(JevNativeFieldRange {
            native_record_id: Some(message_id.to_owned()),
            field: JevInputField::OtherToolOutput,
            container: JevNativeFieldContainer::Record,
            pointer: body_pointer.to_owned(),
            start: 0,
            end: part.text.len(),
        });
    }
    part.metadata.recorded_skill_result = Some(JevRecordedSkillResult::opencode_v2(
        part,
        if message.session.transfer {
            SourceFormat::OpenCodeV2SessionExport
        } else {
            SourceFormat::OpenCodeSqliteV2
        },
        &message.session_id,
        message_id,
        part_index as u32,
        JevNativeSkillProof {
            producer: JevNativeSkillProducer::OpenCodeV2Skill843a6c51,
            origin: JevNativeSkillOrigin::ToolResult,
            id: id.to_owned(),
            display_name: display_name.map(str::to_owned),
            body: if part.truncated {
                JevNativeSkillBody::Truncated
            } else if part.text.is_empty() {
                JevNativeSkillBody::Unavailable
            } else {
                JevNativeSkillBody::Recorded
            },
        },
    ));
}

fn capture_nested_activity(
    metadata: Option<&Value>,
    pointer: &str,
    event: &mut NormalizedEvent,
    output: &mut CapturedContent,
) -> (usize, bool) {
    let Some(calls) = metadata.and_then(|value| value.get("toolCalls")) else {
        return (0, true);
    };
    let Some(calls) = calls.as_array() else {
        return (0, false);
    };
    let mut nested = 0;
    let mut valid = true;
    for (index, call) in calls.iter().take(1024).enumerate() {
        let Some(tool) = text(call.get("tool")) else {
            valid = false;
            continue;
        };
        let Some(state) = call
            .get("status")
            .and_then(Value::as_str)
            .filter(|status| matches!(*status, "running" | "completed" | "error"))
            .and_then(operation_state)
        else {
            valid = false;
            continue;
        };
        let input = call.get("input");
        if input.is_some_and(|input| !input.is_object()) {
            valid = false;
            continue;
        }
        if event.tools.len() >= 1024 {
            output.oversized = true;
            break;
        }
        event
            .tools
            .push(tool_call(tool, input.unwrap_or(&Value::Null)));
        nested += 1;
        // A nested row records activity in start order. It has no result identity or body.
        if let Some(input) = input {
            let part_index = output.parts.len();
            output.push(ContentKind::ToolInput, &input.to_string(), Some(tool), None);
            if let Some(part) = output.parts.get_mut(part_index) {
                part.metadata.state = state;
                if !part.truncated {
                    part.normalized_fields = Some(normalized_input(tool, input, &part.text));
                }
                bind_input(
                    part,
                    input,
                    &format!("{pointer}/metadata/toolCalls/{index}/input"),
                    &event.uuid,
                );
            }
        }
    }
    if calls.len() > 1024 {
        output.oversized = true;
    }
    (nested, valid)
}

pub(super) fn observe_tool_relations(
    message: &SnapshotMessage,
    event: &mut NormalizedEvent,
    launched_children: &mut std::collections::HashSet<String>,
    sink: &mut dyn RecordSink,
) {
    event.proven_subagent_launches = Some(0);
    let Some(content) = message.value.get("content").and_then(Value::as_array) else {
        return;
    };
    let mut call_counts = std::collections::HashMap::new();
    for tool in content
        .iter()
        .take(1024)
        .filter(|tool| tool["type"] == "tool")
    {
        if let Some(id) = tool.get("id").and_then(Value::as_str) {
            *call_counts.entry(id).or_insert(0_u16) += 1;
        }
    }
    for tool in content
        .iter()
        .take(1024)
        .filter(|tool| tool["type"] == "tool")
    {
        if tool
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| call_counts.get(id))
            != Some(&1)
        {
            continue;
        }
        if tool
            .get("executed")
            .is_some_and(|executed| executed.as_bool() != Some(false))
            || !tool.pointer("/state/input").is_some_and(Value::is_object)
            || !matches!(
                tool.pointer("/state/status").and_then(Value::as_str),
                Some("running" | "completed" | "error")
            )
        {
            continue;
        }
        if !tool
            .get("name")
            .and_then(Value::as_str)
            .zip(tool.pointer("/state/input"))
            .is_some_and(|(name, input)| native_input_valid(name, input))
        {
            continue;
        }
        if tool["name"] == "skill"
            && tool.pointer("/state/status").and_then(Value::as_str) == Some("completed")
            && let Some(name) = tool
                .pointer("/state/input/id")
                .and_then(Value::as_str)
                .filter(|name| valid_skill_name(name))
        {
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::SkillInjection {
                    name: name.to_owned(),
                    invoked: true,
                },
            )));
        }
        if tool["name"] != "subagent" {
            continue;
        }
        let Some(child) = text(tool.pointer("/state/metadata/sessionID")) else {
            continue;
        };
        let Some(observed) = message.session.child_models.get(child) else {
            continue;
        };
        let Some(call_id) = text(tool.get("id")) else {
            continue;
        };
        let new_session = match tool.pointer("/state/input/sessionID") {
            None => true,
            Some(Value::String(id)) => id.is_empty(),
            _ => false,
        };
        if new_session
            && tool.pointer("/state/status").and_then(Value::as_str) == Some("completed")
            && matches!(
                tool.pointer("/state/metadata/status")
                    .and_then(Value::as_str),
                Some("running" | "completed")
            )
            && tool
                .pointer("/time/ran")
                .and_then(Value::as_i64)
                .zip(tool.pointer("/time/completed").and_then(Value::as_i64))
                .is_some_and(|(ran, completed)| completed >= ran)
            && launched_children.insert(child.to_owned())
        {
            event.proven_subagent_launches = Some(event.subagent_launch_count().saturating_add(1));
        }
        let new_foreground = tool.pointer("/state/input/sessionID").is_none()
            && tool
                .pointer("/state/metadata/status")
                .and_then(Value::as_str)
                == Some("completed");
        let child_model = if new_foreground
            && observed
                .first_step_ms
                .zip(tool.pointer("/time/ran").and_then(Value::as_i64))
                .zip(tool.pointer("/time/completed").and_then(Value::as_i64))
                .is_some_and(|((created, ran), completed)| created >= ran && created <= completed)
        {
            observed.model.clone()
        } else {
            None
        };
        let configured_agent = text(tool.pointer("/state/input/agent"));
        let config_binding = tool
            .pointer("/state/input/model")
            .is_none()
            .then_some(child_model.as_deref())
            .flatten()
            .and_then(|model_id| {
                let agent_id = configured_agent?;
                let provider_id = observed.provider.as_deref()?;
                (valid_config_agent_id(agent_id)
                    && model_id.len() <= EVIDENCE_STRING_CAP
                    && !model_id.is_empty()
                    && !provider_id.is_empty()
                    && provider_id.len() <= EVIDENCE_STRING_CAP
                    && !provider_id.chars().any(char::is_control))
                .then(|| crate::analysis::SubagentConfigBinding {
                    agent_id: agent_id.to_owned(),
                    provider_id: provider_id.to_owned(),
                    model_id: model_id.to_owned(),
                })
            });
        sink.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::SubagentSpawn {
                ts_ms: tool.pointer("/time/ran").and_then(Value::as_i64),
                parent_model: event.model.clone(),
                parent_call_id: Some(call_id.to_owned()),
                child_model,
                provenance: RelationProvenance::SessionParentLink,
                config_binding,
            },
        )));
    }
}

fn valid_config_agent_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= EVIDENCE_STRING_CAP && !value.chars().any(char::is_control)
}

pub(super) fn capture_shell_message(value: &Value, output: &mut CapturedContent) -> bool {
    let Some(command) = value.get("command").and_then(Value::as_str) else {
        return false;
    };
    let Some(id) = text(value.get("shellID")) else {
        return false;
    };
    let Some(status) = value.get("status").and_then(Value::as_str) else {
        return false;
    };
    let operation = match status {
        "running" => JevOperationState::Running,
        "exited" => JevOperationState::Completed,
        "timeout" | "killed" => JevOperationState::Error,
        _ => return false,
    };
    output.push(
        ContentKind::ToolInput,
        &serde_json::json!({"command":command}).to_string(),
        Some("shell"),
        Some(id),
    );
    if let Some(part) = output.parts.last_mut() {
        part.authority = ContentAuthority::User;
        part.metadata.state = operation;
    }
    if let Some(body) = value.pointer("/output/output").and_then(Value::as_str) {
        output.push(ContentKind::ToolResult, body, Some("shell"), Some(id));
        if let Some(part) = output.parts.last_mut() {
            part.metadata.state = operation;
            part.metadata.outcome = NativeToolOutcome::opencode_v2_shell_message(value);
            part.metadata.tool_output_field = Some(JevInputField::BashCommandOutput);
            part.truncated |= value.pointer("/output/truncated").and_then(Value::as_bool)
                == Some(true)
                || unavailable_output(body)
                || status == "running";
            output.incomplete |= part.truncated;
        }
    } else if status != "running" {
        output.incomplete = true;
    }
    if status == "running" {
        output.incomplete = true;
    }
    true
}

fn read_request(input: &Value, session_id: &str) -> Option<JevReadRequest> {
    let path = input.get("path")?.as_str()?;
    let offset = input.get("offset").and_then(Value::as_u64);
    let limit = input.get("limit").and_then(Value::as_u64);
    Some(JevReadRequest {
        reference_id: String::new(),
        paths: vec![path.to_owned()],
        cwd: None,
        extent: JevReadExtent {
            unit: JevReadUnit::Unknown,
            offset,
            limit,
            end_inclusive: None,
        },
        native_extent: ["offset", "limit"]
            .into_iter()
            .filter_map(|key| input.get(key).map(|value| (key.to_owned(), value.clone())))
            .collect(),
        truncated: path.len() > 4096 || session_id.is_empty(),
        extent_contract: Some(READ_CONTRACT.to_owned()),
    })
}

fn read_result(
    body: &str,
    state: JevOperationState,
    truncated: bool,
    requested_path: Option<&str>,
) -> JevReadResult {
    let mut result = JevReadResult {
        reference_id: String::new(),
        request_reference_id: None,
        status: match state {
            JevOperationState::Completed => JevReadStatus::Success,
            JevOperationState::Error => JevReadStatus::Failed,
            _ => JevReadStatus::Unknown,
        },
        kind: JevReadResultKind::Unknown,
        recorded_output_bytes: body.len() as u64,
        recorded_output_digest: crate::checks::ignored_instructions::sha256_hex(body.as_bytes()),
        returned_extent: None,
        truncated,
        recorded_file_version: None,
        extent_contract: Some(READ_CONTRACT.to_owned()),
    };
    if state != JevOperationState::Completed {
        return result;
    }
    let mut lines = body.lines();
    let Some(header) = lines.next() else {
        return result;
    };
    if header.starts_with("Read directory ") {
        result.kind = JevReadResultKind::Directory;
        return result;
    }
    let Some(header) = header.strip_prefix("Read file ") else {
        return result;
    };
    result.kind = JevReadResultKind::File;
    if truncated {
        return result;
    }
    let Some((returned_path, range)) = header.rsplit_once(", lines ") else {
        return result;
    };
    let Some((start, end)) = range.split_once('-') else {
        return result;
    };
    let Some((start, end)) = start
        .parse::<u64>()
        .ok()
        .zip(end.parse::<u64>().ok())
        .filter(|(start, end)| *start > 0 && end >= start && end - start < 2000)
    else {
        return result;
    };
    let mut count = 0;
    for line in lines {
        if line.starts_with("[Output truncated.") {
            result.truncated = true;
            break;
        }
        let Some((number, _)) = line.split_once(": ") else {
            return result;
        };
        if number.parse::<u64>().ok() != start.checked_add(count) {
            return result;
        }
        if line.contains("... (line truncated to 2000 chars)") {
            result.truncated = true;
        }
        count += 1;
    }
    if count == end - start + 1 && requested_path == Some(returned_path) {
        result.returned_extent = Some(JevReadExtent {
            unit: JevReadUnit::Lines,
            offset: Some(start),
            limit: Some(count),
            end_inclusive: Some(end),
        });
    }
    result
}

fn question_answers(
    input: &Value,
    metadata: Option<&Value>,
    message: &SnapshotMessage,
    call_id: &str,
    truncated: bool,
) -> Option<Vec<JevUserAnswer>> {
    let questions = input.get("questions")?.as_array()?;
    let answers = metadata?.get("answers")?.as_array()?;
    if questions.len() != answers.len() || questions.len() > 64 {
        return None;
    }
    questions
        .iter()
        .zip(answers)
        .enumerate()
        .map(|(order, (question, answers))| {
            let options = question.get("options")?.as_array()?;
            let answers = answers.as_array()?;
            if options.len() > 64 || answers.len() > 64 {
                return None;
            }
            let options = options
                .iter()
                .map(|option| {
                    Some(JevQuestionOption {
                        id: None,
                        value: None,
                        label: option.get("label")?.as_str()?.to_owned(),
                        description: Some(option.get("description")?.as_str()?.to_owned()),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let selections = answers
                .iter()
                .map(|answer| {
                    Some(JevAnswerSelection {
                        option_id: None,
                        option_index: None,
                        value: Some(answer.as_str()?.to_owned()),
                        label: None,
                        custom: None,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(JevUserAnswer {
                source: JevScopeEvidenceSource {
                    source_format: if message.session.transfer {
                        SourceFormat::OpenCodeV2SessionExport
                    } else {
                        SourceFormat::OpenCodeSqliteV2
                    },
                    role: JevScopeEvidenceRole::Tool,
                    native_record_id: message
                        .value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    call_id: Some(call_id.to_owned()),
                    question_id: None,
                    order: order as u32,
                    acceptance_order: None,
                    provenance: JevScopeEvidenceProvenance::RecognizedQuestionWorkflow,
                    producer_revision: PRODUCER.to_owned(),
                    normalization_revision: 1,
                    bindings: vec![],
                    truncated,
                },
                prompt: question.get("question")?.as_str()?.to_owned(),
                header: Some(question.get("header")?.as_str()?.to_owned()),
                context: None,
                comment: None,
                options,
                multi_select: question.get("multiple").and_then(Value::as_bool),
                selections,
                free_text: None,
                status: JevUserAnswerStatus::Submitted,
                origin: JevUserAnswerOrigin::UnknownOrigin,
            })
        })
        .collect()
}
