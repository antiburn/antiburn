//! Exact text slices from the reviewed Pi 0.84.4 read producer.

use serde_json::Value;

use crate::analysis::interface::ContentPart;
use crate::analysis::jev_evidence::{
    JevOperationState, JevReadExtent, JevReadResult, JevReadResultKind, JevReadStatus, JevReadUnit,
};

const CONTRACT: &str = "pi/core/tools/read.ts@b79e4cc834970cca69daebffab7df1da7d1e52c4";

pub(super) fn bind_read(value: &Value, arguments: &str, part: &mut ContentPart) {
    let status = match part.metadata.state {
        JevOperationState::Completed => JevReadStatus::Success,
        JevOperationState::Error => JevReadStatus::Failed,
        _ => JevReadStatus::Unknown,
    };
    let mut result = JevReadResult {
        reference_id: String::new(),
        request_reference_id: None,
        status,
        kind: JevReadResultKind::Unknown,
        recorded_output_bytes: part.text.len() as u64,
        recorded_output_digest: crate::checks::ignored_instructions::sha256_hex(
            part.text.as_bytes(),
        ),
        returned_extent: None,
        truncated: part.truncated,
        recorded_file_version: None,
        extent_contract: Some(CONTRACT.to_owned()),
    };
    if status == JevReadStatus::Success
        && let Ok(args) = serde_json::from_str::<Value>(arguments)
    {
        decode_text(value, &args, &part.text, &mut result);
    }
    if result.returned_extent.is_some() {
        let body_end = part
            .text
            .rsplit_once("\n\n[")
            .filter(|(_, notice)| notice.ends_with("to continue.]"))
            .map_or(part.text.len(), |(body, _)| body.len());
        for binding in &mut part.metadata.bindings {
            if binding.field == crate::analysis::jev::JevInputField::ReadFileOutput {
                binding.end = body_end;
            }
        }
    }
    part.metadata.read_result = Some(result);
}

fn decode_text(value: &Value, args: &Value, text: &str, result: &mut JevReadResult) {
    let Some(path) = args["path"].as_str().filter(|path| !path.is_empty()) else {
        return;
    };
    let offset = match args.get("offset") {
        None => 1,
        Some(value) => match value.as_u64().filter(|offset| *offset > 0) {
            Some(offset) => offset,
            None => return,
        },
    };
    let limit = match args.get("limit") {
        None => None,
        Some(value) => match value.as_u64().filter(|limit| *limit > 0) {
            Some(limit) => Some(limit),
            None => return,
        },
    };
    if text.starts_with("Read image file [") {
        return;
    }
    let truncation = value.pointer("/message/details/truncation");
    if text.starts_with("[Line ") {
        result.truncated = true;
        if truncation.is_some_and(|t| t["firstLineExceedsLimit"] == true)
            && text.ends_with(&format!("p' {path} | head -c 51200]"))
        {
            result.kind = JevReadResultKind::File;
        }
        return;
    }
    let (body, notice) = match text.rsplit_once("\n\n[") {
        Some((body, notice)) if notice.ends_with("to continue.]") => (body, Some(notice)),
        _ => (text, None),
    };
    let mut lines = if body.is_empty() {
        0
    } else {
        body.split('\n').count() as u64
    };
    if let Some(t) = truncation {
        result.truncated = true;
        if t["truncated"] != true
            || t["lastLinePartial"] != false
            || t["firstLineExceedsLimit"] != false
            || t["content"].as_str() != Some(body)
            || t["outputBytes"].as_u64() != Some(body.len() as u64)
            || t["outputLines"].as_u64() != Some(lines)
            || lines > 2000
            || body.len() > 51200
            || t["maxLines"].as_u64() != Some(2000)
            || t["maxBytes"].as_u64() != Some(51200)
            || t["totalLines"]
                .as_u64()
                .is_none_or(|total| total <= lines || limit.is_some_and(|limit| total > limit))
            || t["totalBytes"]
                .as_u64()
                .is_none_or(|total| total <= body.len() as u64)
        {
            return;
        }
        let Some(notice) = notice else { return };
        let Some(end) = offset.checked_add(lines).and_then(|n| n.checked_sub(1)) else {
            return;
        };
        let Some(next) = end.checked_add(1) else {
            return;
        };
        let prefix = format!("Showing lines {offset}-{end} of ");
        let suffix = match t["truncatedBy"].as_str() {
            Some("lines") if lines == 2000 => format!(". Use offset={next} to continue.]"),
            Some("bytes") if t["totalBytes"].as_u64().is_some_and(|bytes| bytes > 51200) => {
                format!(" (50.0KB limit). Use offset={next} to continue.]")
            }
            _ => return,
        };
        if notice
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(&suffix))
            .and_then(|s| s.parse::<u64>().ok())
            .is_none_or(|total| total <= end)
        {
            return;
        }
    } else if let Some(notice) = notice {
        result.truncated = true;
        let Some(limit) = limit else { return };
        // A user-limited slice retains a final empty line when the selected lines include one.
        if body.is_empty() {
            lines = 1;
        }
        let Some(next) = offset.checked_add(lines) else {
            return;
        };
        let suffix = format!(" more lines in file. Use offset={next} to continue.]");
        if lines != limit
            || notice
                .strip_suffix(&suffix)
                .and_then(|s| s.parse::<u64>().ok())
                .is_none_or(|remaining| remaining == 0)
        {
            return;
        }
    } else {
        // The producer keeps the final newline. It is a retained empty file line, not clipping.
        lines = body.split('\n').count() as u64;
        if body.len() > 51200
            || body.lines().count() > 2000
            || body.contains("[Showing lines ")
            || body.contains("more lines in file. Use offset=")
        {
            result.truncated = true;
            return;
        }
    }
    if lines == 0 || limit.is_some_and(|limit| lines > limit) {
        return;
    }
    let Some(end) = offset.checked_add(lines).and_then(|n| n.checked_sub(1)) else {
        return;
    };
    result.kind = JevReadResultKind::File;
    result.returned_extent = Some(JevReadExtent {
        unit: JevReadUnit::Lines,
        offset: Some(offset),
        limit: Some(lines),
        end_inclusive: Some(end),
    });
}
