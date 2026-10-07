//! Native results retain observed output and completion status, not test-pass or approval claims.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::analysis::interface::{ContentKind, ContentPart};
use crate::analysis::jev::JevInputField;
use crate::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, JevOperationState, JevReadExtent, JevReadResult,
    JevReadResultKind, JevReadStatus, JevReadUnit,
};

const READ_CONTRACT: &str = "claude-code/2.1.278:retained-cli-text-read";

pub(super) fn capture_result(
    record: &Value,
    block: &Value,
    input: &Value,
    name: &str,
    structured: Option<&Value>,
    order: u32,
    parts: &mut [ContentPart],
) {
    if record["version"] != "2.1.278" || !matches!(name, "Read" | "Bash" | "Skill") {
        return;
    }
    let id = block["tool_use_id"].as_str();
    let matches = parts
        .iter()
        .filter(|part| part.kind == ContentKind::ToolResult && part.tool_call_id.as_deref() == id)
        .count();
    if id.is_none() || matches != 1 {
        return;
    }
    let part = parts
        .iter_mut()
        .find(|part| part.kind == ContentKind::ToolResult && part.tool_call_id.as_deref() == id)
        .expect("unique native result part");
    let Some(text) = block["content"]
        .as_str()
        .filter(|text| *text == part.text || (part.truncated && text.starts_with(&part.text)))
    else {
        return;
    };
    part.tool_name = Some(name.into());
    let interrupted = record["interruptedByShutdown"] == true
        || structured.is_some_and(|result| {
            result["interrupted"] == true || result.get("backgroundTaskId").is_some()
        });
    let injected = ["isMeta", "isSynthetic", "isCompactSummary"]
        .iter()
        .any(|key| record.get(key).is_some_and(|value| value != false));
    let clipped = part.truncated
        || structured.is_some_and(|result| {
            result["truncated"] == true || result.get("persistedOutputPath").is_some()
        })
        || text.contains("PARTIAL view")
        || text.contains("<persisted-output>")
        || text.contains("[truncated]");
    part.metadata.state = if injected || interrupted || clipped {
        JevOperationState::Unknown
    } else {
        match block.get("is_error").and_then(Value::as_bool) {
            Some(true) => JevOperationState::Error,
            Some(false) => JevOperationState::Completed,
            None => JevOperationState::Unknown,
        }
    };
    let field = match name {
        "Read" => JevInputField::ReadFileResult,
        "Bash" => JevInputField::BashCommandOutput,
        _ => JevInputField::OtherToolOutput,
    };
    if !part.truncated {
        part.metadata.bindings.push(JevNativeFieldRange {
            native_record_id: record["uuid"].as_str().map(str::to_owned),
            field,
            container: JevNativeFieldContainer::Record,
            pointer: format!("/message/content/{order}/content"),
            start: 0,
            end: text.len(),
        });
    }
    if name != "Read" {
        return;
    }
    let mut result = JevReadResult {
        reference_id: String::new(),
        request_reference_id: None,
        status: match part.metadata.state {
            JevOperationState::Completed => JevReadStatus::Success,
            JevOperationState::Error => JevReadStatus::Failed,
            _ => JevReadStatus::Unknown,
        },
        kind: JevReadResultKind::Unknown,
        recorded_output_bytes: part.text.len() as u64,
        recorded_output_digest: format!("{:x}", Sha256::digest(part.text.as_bytes())),
        returned_extent: None,
        truncated: clipped,
        recorded_file_version: None,
        extent_contract: Some(READ_CONTRACT.into()),
    };
    if result.status != JevReadStatus::Failed
        && block.get("is_error").is_none_or(|value| value == false)
        && !interrupted
        && !injected
        && !clipped
        && let Some(payload) = structured.filter(|payload| payload["type"] == "text")
        && let Some(extent) = observed_extent(input, payload, text)
    {
        result.status = JevReadStatus::Success;
        part.metadata.state = JevOperationState::Completed;
        result.kind = JevReadResultKind::File;
        result.returned_extent = Some(extent);
    }
    part.metadata.read_result = Some(result);
}

fn observed_extent(input: &Value, payload: &Value, output: &str) -> Option<JevReadExtent> {
    let file = &payload["file"];
    if file["filePath"].as_str()? != input["file_path"].as_str()? {
        return None;
    }
    let start = file["startLine"].as_u64().filter(|start| *start > 0)?;
    let count = file["numLines"].as_u64().filter(|count| *count > 0)?;
    let total = file["totalLines"].as_u64()?;
    let end = start.checked_add(count)?.checked_sub(1)?;
    if end > total {
        return None;
    }
    let content = file["content"].as_str()?;
    let mut lines: Vec<_> = content.split('\n').collect();
    if lines.len() as u64 == count + 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() as u64 != count {
        return None;
    }
    let mut rendered = output.split('\n');
    for (offset, line) in lines.iter().enumerate() {
        let (number, observed) = rendered.next()?.split_once('\t')?;
        if number.trim().parse::<u64>().ok()? != start.checked_add(offset as u64)?
            || observed != *line
        {
            return None;
        }
    }
    if rendered.any(|line| !line.is_empty()) {
        return None;
    }
    Some(JevReadExtent {
        unit: JevReadUnit::Lines,
        offset: Some(start),
        limit: Some(count),
        end_inclusive: Some(end),
    })
}
