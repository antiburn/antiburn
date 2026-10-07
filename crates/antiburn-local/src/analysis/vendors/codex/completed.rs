//! Completed paginated operations bind work and results to the same native item.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::analysis::SourceFormat;
use crate::analysis::framing::PartialReason;
use crate::analysis::interface::{
    ContentAuthority, ContentKind, ContentPart, NormalizedRecord, RecordSink, TurnContent,
};
use crate::analysis::jev::{JevInputField, JevNormalizedCategory, JevNormalizedFields};
use crate::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, JevOperationState, JevReadExtent, JevReadRequest,
    JevReadResult, JevReadResultKind, JevReadStatus, JevReadUnit, JevSelectedSkillNormalization,
    JevSelectedSkillProducer, JevSelectedSkillProof, JevSelectedSkillStatus, UserTextHistoryProof,
};
use crate::analysis::model::NormalizedEvent;

const PRODUCER: &str = "openai/codex@d27764b82f7118f674371e6d6e76271d9d606edb";
const MAX_ITEMS: usize = 4096;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct CompletedState {
    session_id: Option<String>,
    native_header: bool,
    paginated: bool,
    root: bool,
    ordinal: Option<u64>,
    items: BTreeMap<String, String>,
    user_projection: Option<(String, String)>,
    agent_projection: Option<(String, String, String)>,
    replaced_calls: BTreeSet<String>,
    messages: BTreeMap<String, String>,
}

impl CompletedState {
    pub(super) fn observe(
        &mut self,
        record: &Value,
        owned: bool,
        sink: &mut dyn RecordSink,
    ) -> bool {
        if record["type"] == "session_meta" {
            if self.session_id.is_some() {
                self.root = false;
                if self.paginated {
                    sink.record(NormalizedRecord::Unusable(
                        PartialReason::AttributionIncomplete,
                    ));
                }
                return false;
            }
            let meta = &record["payload"];
            self.native_header = meta["cli_version"] == "0.160.1"
                && matches!(meta["history_mode"].as_str(), Some("legacy" | "paginated"));
            self.paginated =
                meta["history_mode"] == "paginated" && meta["cli_version"] == "0.160.1";
            self.root = self.native_header
                && meta["thread_source"] == "user"
                && ["forked_from_id", "parent_thread_id", "history_base"]
                    .iter()
                    .all(|key| meta.get(key).is_none_or(Value::is_null));
            self.session_id = meta["id"]
                .as_str()
                .filter(|id| valid_id(id))
                .map(str::to_owned);
            self.root &= self.session_id.is_some();
        }
        if !self.native_header {
            return false;
        }
        let ordinal = record["ordinal"].as_u64();
        if self.paginated
            && (ordinal.is_none()
                || ordinal
                    != Some(
                        self.ordinal
                            .map_or(0, |previous| previous.saturating_add(1)),
                    ))
        {
            self.root = false;
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
        }
        self.ordinal = ordinal;
        if record["type"] == "response_item"
            && (record["payload"]["type"] == "custom_tool_call"
                && record["payload"]["name"] == "exec"
                || record["payload"]["type"] == "function_call"
                    && matches!(
                        record["payload"]["name"].as_str(),
                        Some("exec_command" | "apply_patch")
                    ))
        {
            if let Some(id) = record["payload"]["call_id"]
                .as_str()
                .filter(|id| valid_id(id))
                && self.replaced_calls.len() < MAX_ITEMS
            {
                self.replaced_calls.insert(id.into());
            } else {
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
            }
        }
        if let Some((id, turn, digest)) = self.agent_projection.take() {
            let payload = &record["payload"];
            let matches = record["type"] == "response_item"
                && payload["type"] == "message"
                && payload["role"] == "assistant"
                && payload["id"] == id
                && payload
                    .pointer("/internal_chat_message_metadata_passthrough/turn_id")
                    .and_then(Value::as_str)
                    == Some(turn.as_str())
                && payload["content"]
                    .as_array()
                    .and_then(|content| {
                        content
                            .iter()
                            .map(|part| part["text"].as_str())
                            .collect::<Option<Vec<_>>>()
                    })
                    .is_some_and(|text| format!("{:x}", Sha256::digest(text.join(""))) == digest);
            if !matches {
                self.root = false;
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
            }
        }
        if record["type"] == "response_item"
            && record["payload"]["type"] == "message"
            && record["payload"]["role"] == "user"
        {
            self.user_projection = record
                .pointer("/payload/internal_chat_message_metadata_passthrough/turn_id")
                .and_then(Value::as_str)
                .zip(record["payload"]["content"].as_array())
                .and_then(|(turn, content)| {
                    let text = content
                        .iter()
                        .map(|part| part["text"].as_str())
                        .collect::<Option<Vec<_>>>()?
                        .join("");
                    Some((turn.into(), format!("{:x}", Sha256::digest(text))))
                });
        }
        if record["type"] == "response_item"
            && record["payload"]["type"] == "message"
            && let Some(id) = record["payload"]["id"].as_str().filter(|id| valid_id(id))
        {
            let digest = format!(
                "{:x}",
                Sha256::digest(json!([record["payload"], record["metadata"]]).to_string())
            );
            if let Some(previous) = self.messages.get(id) {
                if previous != &digest {
                    self.root = false;
                    sink.record(NormalizedRecord::Unusable(
                        PartialReason::AttributionIncomplete,
                    ));
                }
                return true;
            }
            if self.messages.len() >= MAX_ITEMS {
                self.root = false;
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
            } else {
                self.messages.insert(id.into(), digest);
            }
        }
        if record["type"] == "compacted"
            || record["type"] == "history_base"
            || record["payload"]["type"] == "thread_rolled_back"
        {
            self.root = false;
        }
        if record["type"] != "event_msg" || record["payload"]["type"] != "item_completed" {
            return false;
        }
        if !self.paginated {
            if record["payload"]["item"]["type"] != "Plan" {
                self.root = false;
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
                return true;
            }
            return false;
        }
        let payload = &record["payload"];
        let item = &payload["item"];
        if item["type"] == "UserMessage" {
            self.capture_picker(record, owned, sink);
            return true;
        }
        if item["type"] == "AgentMessage" {
            self.agent_projection = item["id"]
                .as_str()
                .filter(|id| valid_id(id))
                .zip(payload["turn_id"].as_str().filter(|id| valid_id(id)))
                .zip(item["content"].as_array())
                .and_then(|((id, turn), content)| {
                    let text = content
                        .iter()
                        .map(|part| part["text"].as_str())
                        .collect::<Option<Vec<_>>>()?
                        .join("");
                    Some((
                        id.into(),
                        turn.into(),
                        format!("{:x}", Sha256::digest(text)),
                    ))
                });
            if self.agent_projection.is_none()
                || !owned
                || payload["thread_id"].as_str() != self.session_id.as_deref()
            {
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
            }
            return true;
        }
        match item["type"].as_str() {
            // Response items retain the canonical messages and reasoning. Picker
            // content does not authorize work and must not duplicate user text.
            Some("Reasoning") => return true,
            Some("CommandExecution" | "FileChange") => {}
            Some("Plan") => return false,
            _ => {
                self.root = false;
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
                return true;
            }
        }
        let valid_context = owned
            && payload["thread_id"].as_str() == self.session_id.as_deref()
            && payload["turn_id"].as_str().is_some_and(valid_id)
            && record
                .get("timestamp")
                .and_then(crate::analysis::records::parse_ts)
                .is_some();
        let Some(id) = item["id"].as_str().filter(|id| valid_id(id)) else {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
            return true;
        };
        if !valid_context {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
            return true;
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(json!([payload["turn_id"], item]).to_string())
        );
        if let Some(previous) = self.items.get(id) {
            if previous != &digest {
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
            }
            return true;
        }
        if self.items.len() >= MAX_ITEMS {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
            return true;
        }
        self.items.insert(id.into(), digest);
        let parts = match item["type"].as_str() {
            Some("CommandExecution") => command_parts(item),
            Some("FileChange") => file_parts(item),
            _ => unreachable!(),
        };
        match parts {
            Some(parts) => {
                let mut event = if item["type"] == "CommandExecution" {
                    super::named_tool_event(
                        "exec_command",
                        Some(&json!({"cmd":item["command"][2]})),
                        record
                            .get("timestamp")
                            .and_then(crate::analysis::records::parse_ts),
                    )
                } else {
                    super::named_tool_event(
                        "apply_patch",
                        None,
                        record
                            .get("timestamp")
                            .and_then(crate::analysis::records::parse_ts),
                    )
                };
                event.message_id = Some(id.into());
                sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
                sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                    parts,
                })));
            }
            None => sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord)),
        }
        true
    }

    pub(super) fn replaces_exec_content(&self, record: &Value) -> bool {
        self.paginated
            && record["type"] == "response_item"
            && (matches!(
                record["payload"]["type"].as_str(),
                Some("custom_tool_call_output" | "function_call_output")
            ) && record["payload"]["call_id"]
                .as_str()
                .is_some_and(|id| self.replaced_calls.contains(id))
                || record["payload"]["type"] == "custom_tool_call"
                    && record["payload"]["name"] == "exec"
                || record["payload"]["type"] == "function_call"
                    && matches!(
                        record["payload"]["name"].as_str(),
                        Some("exec_command" | "apply_patch")
                    ))
    }

    pub(super) fn finish(&self, sink: &mut dyn RecordSink) {
        if self.agent_projection.is_some() {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
        }
    }

    fn capture_picker(&mut self, record: &Value, owned: bool, sink: &mut dyn RecordSink) {
        let payload = &record["payload"];
        let item = &payload["item"];
        let Some(content) = item["content"].as_array() else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        };
        if content
            .iter()
            .any(|part| !matches!(part["type"].as_str(), Some("text" | "skill")))
        {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
        }
        let text = content
            .iter()
            .filter(|part| part["type"] == "text")
            .map(|part| part["text"].as_str())
            .collect::<Option<Vec<_>>>();
        let matching = text.is_some_and(|text| {
            self.user_projection.take().is_some_and(|(turn, digest)| {
                payload["turn_id"] == turn
                    && format!("{:x}", Sha256::digest(text.join(""))) == digest
            })
        });
        if !item["id"].as_str().is_some_and(valid_id) {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
            return;
        }
        if !owned || !matching || payload["thread_id"].as_str() != self.session_id.as_deref() {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
            return;
        }
        for selected in content.iter().filter(|part| part["type"] == "skill") {
            if !selected["name"].as_str().is_some_and(valid_id)
                || !selected["path"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty() && path.len() <= 4096)
            {
                sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            }
        }
    }

    pub(super) fn capture_message(&self, record: &Value, parts: &mut [ContentPart]) {
        if record["payload"]["role"] != "user" {
            return;
        }
        if self.native_header {
            for part in parts.iter_mut() {
                part.authority = ContentAuthority::Unknown;
            }
            if let Some(session_id) = self.session_id.as_deref() {
                super::environment::capture(record, session_id, parts);
            }
        }
        let kinds = record
            .pointer("/payload/internal_chat_message_metadata_passthrough/content_item_kinds")
            .and_then(Value::as_array);
        if let Some(kinds) = kinds {
            for (index, part) in parts.iter_mut().enumerate() {
                if kinds.get(index).is_none_or(|kind| kind != "user.text") {
                    part.authority = ContentAuthority::Unknown;
                }
                if kinds
                    .get(index)
                    .is_some_and(|kind| kind == "skills.selected_skill_instructions")
                {
                    self.capture_selected_skill(record, index, part);
                }
            }
        }
        if !self.root {
            return;
        }
        let retained = &record["metadata"]["retained_source"];
        let Some(id) = record["payload"]["id"].as_str().filter(|id| valid_id(id)) else {
            return;
        };
        let turn = record
            .pointer("/payload/internal_chat_message_metadata_passthrough/turn_id")
            .and_then(Value::as_str);
        if retained["complete"] != true
            || retained["id"]["message_id"] != id
            || retained["id"]["role"] != "user"
            || retained["id"]["turn_id"].as_str() != turn
            || !turn.is_some_and(valid_id)
            || !retained["revision"].as_str().is_some_and(valid_id)
            || record["metadata"]["client_authored"] != false
            || record
                .pointer("/metadata/inherited_user_message")
                .is_some_and(|value| value != false)
            || !kinds.is_some_and(|kinds| {
                kinds.len() == parts.len() && kinds.iter().all(|kind| kind == "user.text")
            })
            || !record["payload"]["content"]
                .as_array()
                .is_some_and(|content| {
                    content.len() == parts.len()
                        && content
                            .iter()
                            .all(|part| part["type"] == "input_text" && part["text"].is_string())
                })
        {
            return;
        }
        for (index, part) in parts
            .iter_mut()
            .enumerate()
            .filter(|(_, part)| !part.truncated)
        {
            part.authority = ContentAuthority::User;
            part.metadata.bindings.push(binding(
                id,
                JevInputField::UserMessage,
                &format!("/payload/content/{index}/text"),
                &part.text,
            ));
            part.metadata.user_text_history = Some(UserTextHistoryProof {
                source_format: SourceFormat::CodexRolloutJsonl,
                session_id: self.session_id.clone().expect("root session"),
                message_id: id.into(),
                revision: 1,
            });
        }
    }

    fn capture_selected_skill(&self, record: &Value, index: usize, part: &mut ContentPart) {
        if !self.native_header
            || part.truncated
            || record
                .pointer("/metadata/inherited_user_message")
                .is_some_and(|value| value != false)
            || record["payload"]["content"][index]["type"] != "input_text"
            || record["payload"]["content"][index]["text"].as_str() != Some(part.text.as_str())
        {
            return;
        }
        let Some(session_id) = self.session_id.as_deref() else {
            return;
        };
        let Some(message_id) = record["payload"]["id"].as_str() else {
            return;
        };
        let Some(name) = super::selected_skill_name(&part.text) else {
            return;
        };
        let Some(location) = part
            .text
            .split_once("</name>\n<path>")
            .and_then(|(_, body)| body.split_once("</path>\n"))
            .map(|(path, _)| path)
        else {
            return;
        };
        let range = binding(
            message_id,
            JevInputField::UserMessage,
            &format!("/payload/content/{index}/text"),
            &part.text,
        );
        let proof = JevSelectedSkillProof {
            source_format: SourceFormat::CodexRolloutJsonl,
            session_id: session_id.into(),
            message_id: message_id.into(),
            name: name.into(),
            location: location.into(),
            producer: JevSelectedSkillProducer::CodexSelectedSkillInstructions,
            normalization: JevSelectedSkillNormalization::CodexSkillDocument,
            normalization_revision: 1,
            ranges: vec![range.clone()],
            status: JevSelectedSkillStatus::DocumentSelected,
            complete: true,
        };
        if proof.is_bounded() {
            part.metadata.bindings.push(range);
            part.metadata.selected_skill = Some(proof);
        }
    }
}

#[derive(Default)]
pub(super) struct MetricsSink {
    pub(super) event: Option<NormalizedEvent>,
}

impl RecordSink for MetricsSink {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::MetricsEvent(event) = record {
            self.event = Some(*event);
        }
    }
    fn finish(&mut self, _summary: crate::analysis::interface::SessionSummary) {}
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 1024
}

fn binding(id: &str, field: JevInputField, pointer: &str, text: &str) -> JevNativeFieldRange {
    JevNativeFieldRange {
        native_record_id: Some(id.into()),
        field,
        container: JevNativeFieldContainer::Record,
        pointer: pointer.into(),
        start: 0,
        end: text.len(),
    }
}

fn state(item: &Value) -> JevOperationState {
    match (item["status"].as_str(), item["exit_code"].as_i64()) {
        (Some("completed"), Some(0)) => JevOperationState::Completed,
        (Some("failed"), Some(code)) if code != 0 => JevOperationState::Error,
        _ => JevOperationState::Unknown,
    }
}

fn command_parts(item: &Value) -> Option<Vec<ContentPart>> {
    let id = item["id"].as_str()?;
    let argv = item["command"].as_array()?;
    if argv.len() != 3
        || !matches!(argv[0].as_str(), Some("/bin/zsh" | "/bin/bash" | "/bin/sh"))
        || argv[1] != "-lc"
    {
        return None;
    }
    let command = argv[2].as_str()?;
    let output = item["aggregated_output"].as_str()?;
    let operation_state = state(item);
    let mut input = ContentPart::new(ContentKind::ToolInput, json!({"cmd":command}).to_string())
        .with_tool_identity(Some("exec_command".into()), Some(id.into()));
    input.metadata.bindings.push(binding(
        id,
        JevInputField::BashCommandInput,
        "/payload/item/command/2",
        command,
    ));
    input.metadata.state = operation_state;
    let mut result = ContentPart::new(ContentKind::ToolResult, output)
        .with_tool_identity(Some("exec_command".into()), Some(id.into()));
    result.metadata.state = operation_state;
    result.normalized_fields = Some(JevNormalizedFields::default());
    result.metadata.bindings.push(binding(
        id,
        JevInputField::BashCommandOutput,
        "/payload/item/aggregated_output",
        output,
    ));
    result.truncated |= clipped(output);
    if !input.truncated
        && let Some((path, start, end)) = numbered_slice(command, item)
    {
        attach_read(item, &path, start, end, &mut input, &mut result);
    }
    Some(vec![input, result])
}

fn clipped(text: &str) -> bool {
    text.len() >= 64 * 1024
        || text.to_ascii_lowercase().contains("truncated")
        || text.contains("…")
        || text.contains("... omitted")
}

fn numbered_slice(command: &str, item: &Value) -> Option<(String, u64, u64)> {
    let rest = command.strip_prefix("nl -ba ")?;
    let (path, range) = rest.split_once(" | sed -n '")?;
    if path.is_empty()
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        return None;
    }
    let (start, end) = range.strip_suffix("p'")?.split_once(',')?;
    let (start, end) = (start.parse::<u64>().ok()?, end.parse::<u64>().ok()?);
    if start == 0 || end < start {
        return None;
    }
    let parsed = item["parsed_cmd"].as_array()?;
    if parsed.len() != 1
        || parsed[0]["type"] != "read"
        || parsed[0]["path"] != path
        || parsed[0]["cmd"] != command
    {
        return None;
    }
    Some((path.into(), start, end))
}

fn attach_read(
    item: &Value,
    path: &str,
    start: u64,
    end: u64,
    input: &mut ContentPart,
    result: &mut ContentPart,
) {
    let id = item["id"].as_str().expect("native item ID");
    let request_id = format!("codex-item:{id}:read-request");
    input.metadata.read_request = Some(JevReadRequest {
        reference_id: request_id.clone(),
        paths: vec![path.into()],
        cwd: item["cwd"].as_str().map(str::to_owned),
        extent: JevReadExtent {
            unit: JevReadUnit::Lines,
            offset: Some(start),
            limit: Some(end - start + 1),
            end_inclusive: Some(end),
        },
        native_extent: BTreeMap::new(),
        truncated: input.truncated,
        extent_contract: Some(PRODUCER.into()),
    });
    input.normalized_fields = Some(JevNormalizedFields {
        category: Some(JevNormalizedCategory::ReadFile),
        values: [
            (
                JevInputField::ReadFilePath,
                json!({"paths":[path]}).to_string(),
            ),
            (
                JevInputField::ReadFileRequest,
                serde_json::to_string(input.metadata.read_request.as_ref().expect("request"))
                    .expect("read JSON"),
            ),
        ]
        .into_iter()
        .collect(),
        malformed: input.truncated,
    });
    input.tool_name = Some("read".into());
    input.metadata.bindings.push(binding(
        id,
        JevInputField::ReadFilePath,
        "/payload/item/parsed_cmd/0/path",
        path,
    ));
    input.metadata.bindings.push(binding(
        id,
        JevInputField::ReadFileRequest,
        "/payload/item/command/2",
        item["command"][2].as_str().expect("command"),
    ));
    let output = item["aggregated_output"].as_str().expect("output");
    let mut expected = start;
    let mut count = 0u64;
    let numbered = item["stderr"] == ""
        && item["stdout"] == output
        && output.lines().all(|line| {
            let Some((number, _)) = line.split_once('\t') else {
                return false;
            };
            if number.trim().parse::<u64>().ok() != Some(expected) || expected > end {
                return false;
            }
            expected += 1;
            count += 1;
            true
        });
    let status = match result.metadata.state {
        JevOperationState::Completed => JevReadStatus::Success,
        JevOperationState::Error => JevReadStatus::Failed,
        _ => JevReadStatus::Unknown,
    };
    result.tool_name = Some("read".into());
    result.normalized_fields = Some(JevNormalizedFields::default());
    result.metadata.bindings[0].field = JevInputField::ReadFileResult;
    result.metadata.bindings.push(binding(
        id,
        JevInputField::ReadFileOutput,
        "/payload/item/aggregated_output",
        output,
    ));
    result.metadata.read_result = Some(JevReadResult {
        reference_id: format!("codex-item:{id}:read-result"),
        request_reference_id: Some(request_id),
        status,
        kind: JevReadResultKind::File,
        recorded_output_bytes: output.len() as u64,
        recorded_output_digest: format!("{:x}", Sha256::digest(output)),
        returned_extent: (numbered
            && count > 0
            && !result.truncated
            && status == JevReadStatus::Success)
            .then_some(JevReadExtent {
                unit: JevReadUnit::Lines,
                offset: Some(start),
                limit: Some(count),
                end_inclusive: Some(start + count.saturating_sub(1)),
            }),
        truncated: result.truncated,
        recorded_file_version: None,
        extent_contract: Some(PRODUCER.into()),
    });
}

fn file_parts(item: &Value) -> Option<Vec<ContentPart>> {
    let id = item["id"].as_str()?;
    let changes = item["changes"].as_object()?;
    if changes.is_empty() || changes.len() > 256 {
        return None;
    }
    let mut paths = Vec::new();
    for (path, change) in changes {
        if path.is_empty() || path.len() > 4096 {
            return None;
        }
        paths.push(path.as_str());
        match change["type"].as_str() {
            Some("add") if change["content"].is_string() => {}
            Some("update") if change["unified_diff"].is_string() => match change.get("move_path") {
                Some(Value::String(path)) if !path.is_empty() && path.len() <= 4096 => {
                    paths.push(path)
                }
                None | Some(Value::Null) => {}
                _ => return None,
            },
            Some("delete") => {}
            _ => return None,
        }
    }
    let mut input = ContentPart::new(ContentKind::ToolInput, item["changes"].to_string())
        .with_tool_identity(Some("apply_patch".into()), Some(id.into()));
    if input.truncated {
        return None;
    }
    input.normalized_fields = Some(JevNormalizedFields {
        category: Some(JevNormalizedCategory::FileEdit),
        values: [
            (
                JevInputField::FileEditPath,
                json!({"paths":paths}).to_string(),
            ),
            (
                JevInputField::FileEditContent,
                json!({"changes":changes}).to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        malformed: input.truncated,
    });
    for (path, change) in changes {
        let path = path.replace('~', "~0").replace('/', "~1");
        for key in ["content", "unified_diff"] {
            if let Some(text) = change[key].as_str() {
                input.metadata.bindings.push(binding(
                    id,
                    JevInputField::FileEditContent,
                    &format!("/payload/item/changes/{path}/{key}"),
                    text,
                ));
            }
        }
    }
    let operation_state = match item["status"].as_str() {
        Some("completed") => JevOperationState::Completed,
        Some("failed") => JevOperationState::Error,
        _ => JevOperationState::Unknown,
    };
    input.metadata.state = operation_state;
    let mut result = ContentPart::new(ContentKind::ToolResult, item["stdout"].as_str()?)
        .with_tool_identity(Some("apply_patch".into()), Some(id.into()));
    result.metadata.state = operation_state;
    result.normalized_fields = Some(JevNormalizedFields::default());
    result.metadata.bindings.push(binding(
        id,
        JevInputField::OtherToolOutput,
        "/payload/item/stdout",
        item["stdout"].as_str()?,
    ));
    Some(vec![input, result])
}
