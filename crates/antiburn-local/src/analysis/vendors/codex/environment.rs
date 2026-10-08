//! Accept the pinned single-local-environment context, not human permission text.

use serde_json::Value;

use crate::analysis::SourceFormat;
use crate::analysis::interface::{ContentAuthority, ContentKind, ContentPart};
use crate::analysis::jev::JevInputField;
use crate::analysis::jev_evidence::{JevNativeFieldContainer, JevNativeFieldRange};
use crate::analysis::jev_evidence::{
    JevNonAuthorizingContextKind, JevNonAuthorizingContextProducer, JevNonAuthorizingContextProof,
    non_authorizing_context_digest,
};

pub(super) fn capture(record: &Value, session_id: &str, parts: &mut [ContentPart]) {
    if record["type"] != "response_item"
        || record["payload"]["type"] != "message"
        || record["payload"]["role"] != "user"
        || record
            .pointer("/metadata/inherited_user_message")
            .is_some_and(|flag| flag != false)
    {
        return;
    }
    let Some(content) = record["payload"]["content"].as_array() else {
        return;
    };
    let Some(kinds) = record
        .pointer("/payload/internal_chat_message_metadata_passthrough/content_item_kinds")
        .and_then(Value::as_array)
    else {
        return;
    };
    let Some(message_id) = record["payload"]["id"].as_str() else {
        return;
    };
    let Some(turn_id) = record
        .pointer("/payload/internal_chat_message_metadata_passthrough/turn_id")
        .and_then(Value::as_str)
    else {
        return;
    };
    if content.len() != 1
        || parts.len() != 1
        || kinds.len() != 1
        || kinds[0] != "environments.environment_context"
        || turn_id.is_empty()
        || turn_id.len() > 256
        || content[0]["type"] != "input_text"
        || !record
            .pointer("/payload/internal_chat_message_metadata_passthrough/create_time")
            .and_then(Value::as_f64)
            .is_some_and(|time| time.is_finite() && time > 0.0)
    {
        return;
    }
    let part = &mut parts[0];
    if part.kind != ContentKind::UserText
        || part.truncated
        || content[0]["text"].as_str() != Some(part.text.as_str())
        || !single_environment_shape(&part.text)
    {
        return;
    }
    let range = JevNativeFieldRange {
        native_record_id: Some(message_id.into()),
        field: JevInputField::UserMessage,
        container: JevNativeFieldContainer::Record,
        pointer: "/payload/content/0/text".into(),
        start: 0,
        end: part.text.len(),
    };
    let proof = JevNonAuthorizingContextProof {
        kind: JevNonAuthorizingContextKind::Environment,
        producer: JevNonAuthorizingContextProducer::CodexEnvironmentContext01601,
        source_format: SourceFormat::CodexRolloutJsonl,
        session_id: session_id.into(),
        message_id: message_id.into(),
        range: range.clone(),
        text_digest: non_authorizing_context_digest(&part.text),
        complete: true,
        normalization_revision: 1,
    };
    if proof.is_bounded() {
        part.authority = ContentAuthority::Unknown;
        part.metadata.bindings.push(range);
        part.metadata.non_authorizing_context_proof = Some(proof);
    }
}

fn single_environment_shape(text: &str) -> bool {
    let Some(body) = text
        .strip_prefix("<environment_context>\n")
        .and_then(|body| body.strip_suffix("\n</environment_context>"))
    else {
        return false;
    };
    let mut lines = body.lines();
    for field in ["cwd", "shell", "current_date", "timezone"] {
        let Some(line) = lines.next().and_then(|line| line.strip_prefix("  ")) else {
            return false;
        };
        let Some(value) = element(line, field) else {
            return false;
        };
        if !xml_text(value) {
            return false;
        }
    }
    let Some(filesystem) = lines
        .next()
        .and_then(|line| line.strip_prefix("  "))
        .and_then(|line| element(line, "filesystem"))
    else {
        return false;
    };
    lines.next().is_none() && filesystem_shape(filesystem)
}

fn filesystem_shape(mut text: &str) -> bool {
    if let Some(rest) = text.strip_prefix("<workspace_roots>") {
        let Some((roots, remainder)) = rest.split_once("</workspace_roots>") else {
            return false;
        };
        let mut roots = roots;
        let mut count = 0;
        while let Some(rest) = roots.strip_prefix("<root>") {
            let Some((value, remainder)) = rest.split_once("</root>") else {
                return false;
            };
            if !xml_text(value) || count >= 4096 {
                return false;
            }
            roots = remainder;
            count += 1;
        }
        if !roots.is_empty() || count == 0 {
            return false;
        }
        text = remainder;
    }
    let Some(entries) = text
        .strip_prefix("<permission_profile type=\"managed\"><file_system type=\"restricted\">")
        .and_then(|body| body.strip_suffix("</file_system></permission_profile>"))
    else {
        return false;
    };
    let mut entries = entries;
    let mut count = 0;
    while !entries.is_empty() {
        let Some(rest) = entries
            .strip_prefix("<entry access=\"read\">")
            .or_else(|| entries.strip_prefix("<entry access=\"write\">"))
        else {
            return false;
        };
        let Some((entry, remainder)) = rest.split_once("</entry>") else {
            return false;
        };
        let Some(value) = element(entry, "path").or_else(|| element(entry, "special")) else {
            return false;
        };
        if !xml_text(value) || count >= 4096 {
            return false;
        }
        entries = remainder;
        count += 1;
    }
    count > 0
}

fn element<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.strip_prefix(&format!("<{name}>"))
        .and_then(|body| body.strip_suffix(&format!("</{name}>")))
}

fn xml_text(mut text: &str) -> bool {
    if text.is_empty()
        || text.contains(['<', '>', '\n', '\r'])
        || text.chars().any(char::is_control)
    {
        return false;
    }
    while let Some((_, rest)) = text.split_once('&') {
        let Some(remainder) = ["amp;", "lt;", "gt;", "quot;", "apos;"]
            .iter()
            .find_map(|escape| rest.strip_prefix(escape))
        else {
            return false;
        };
        text = remainder;
    }
    true
}
