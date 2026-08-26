// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared JSON-record parsing used by the JSONL-family adapters and, for cells
//! that contain embedded JSON, by the generic SQLite adapter.
//!
//! Handles the Anthropic transcript shape (`message.content[]` with `text` /
//! `thinking` / `tool_use` parts, `message.usage`), the OpenAI shape (`role` +
//! string `content`, `tool_calls[].function.name`, `usage.prompt_tokens` /
//! `completion_tokens`), and Pi's generic JSONL shape (`toolCall` content blocks
//! with `arguments`, `toolResult` messages, and `input` / `output` /
//! `cacheRead` / `cacheWrite` usage keys). Unrecognized records are skipped.

use std::collections::HashSet;

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::analysis::interface::{ContextSourceKind, EvidenceObservation};
use crate::analysis::model::{CompactionTrigger, NormalizedEvent, Role, ToolCall, Usage};

/// Parse line-delimited JSON into normalized events, skipping blank/malformed
/// lines.
///
/// Two passes over the transcript: a cheap pre-scan collects the set of skills that
/// actually ran (their "Base directory for this skill:" markers), then the main pass
/// builds events and — for any user-typed skill *slash command* (a `<command-name>`
/// tag, which is **not** a `Skill` tool_use) — synthesizes a `skill` tool call so
/// the engine emits a marker for it too. Cross-referencing the base-name set keeps
/// non-skill commands (`/clear`, `/compact`, custom commands) from ever qualifying.
pub fn parse_jsonl(content: &str) -> Vec<NormalizedEvent> {
    // Only the few marker-bearing records are parsed in the pre-scan; the substring
    // gate skips the rest, and parsing (rather than scanning raw JSON) keeps Windows
    // paths intact through their `\\` escaping.
    let mut skill_base_names: HashSet<String> = HashSet::new();
    for line in content.lines() {
        if line.contains(SKILL_BASE_MARKER)
            && let Ok(value) = serde_json::from_str::<Value>(line.trim())
        {
            collect_skill_base_names_from_text(&record_text(&value), &mut skill_base_names);
        }
    }

    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(mut ev) = parse_record(&value)
        {
            if !skill_base_names.is_empty() && line.contains("<command-name>") {
                synthesize_command_skills(&value, &skill_base_names, &mut ev);
            }
            events.push(ev);
        }
    }
    events
}

pub(super) const SKILL_BASE_MARKER: &str = "Base directory for this skill:";

/// Flatten a record's message text — string content, or the `text` of its content
/// blocks — for scanning `<command-name>` tags and skill base-directory markers.
/// Only Claude transcripts carry these conventions, so it's inert for other vendors.
pub(super) fn record_text(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return String::new();
    };
    let content = obj
        .get("message")
        .and_then(|m| m.as_object())
        .and_then(|m| m.get("content"))
        .or_else(|| obj.get("content"));
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                    out.push('\n');
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// Record the skill name from every "Base directory for this skill: <path>" marker
/// in `text` — the set of skills that actually loaded this session.
pub(super) fn collect_skill_base_names_from_text(text: &str, out: &mut HashSet<String>) {
    for line in text.lines() {
        if let Some((_, rest)) = line.split_once(SKILL_BASE_MARKER)
            && let Some(name) = skill_base_name_from_path(rest)
        {
            out.insert(name);
        }
    }
}

/// Skill name from a base-directory marker path: the final path segment, or its
/// parent when the path points straight at the `SKILL.md` file. Cross-platform
/// (splits on `/` and `\`).
pub(super) fn skill_base_name_from_path(path: &str) -> Option<String> {
    let mut segments: Vec<&str> = path
        .trim()
        .trim_matches(['`', '"', '\''])
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect();
    let last = segments.pop()?;
    if last.eq_ignore_ascii_case("SKILL.md") {
        return segments.pop().map(str::to_string);
    }
    Some(last.to_string())
}

/// Synthesize a `skill` tool call on `ev` for each user-typed `<command-name>` that
/// names a skill in `skill_base_names`, so the engine emits a marker for it just
/// like a `Skill` tool_use.
fn synthesize_command_skills(
    value: &Value,
    skill_base_names: &HashSet<String>,
    ev: &mut NormalizedEvent,
) {
    for command in command_names_in_text(&record_text(value)) {
        if let Some(skill) = command_skill_name(&command, skill_base_names) {
            let mut call = ToolCall::new("skill");
            call.detail = Some(skill);
            ev.tools.push(call);
        }
    }
}

/// The `<command-name>` values in `text`, leading `/` stripped:
/// `<command-name>/code-review</command-name>` → `"code-review"`.
pub(super) fn command_names_in_text(text: &str) -> Vec<String> {
    const OPEN: &str = "<command-name>";
    const CLOSE: &str = "</command-name>";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            break;
        };
        let name = after[..end].trim().trim_start_matches('/').trim();
        if !name.is_empty() {
            out.push(name.to_string());
        }
        rest = &after[end + CLOSE.len()..];
    }
    out
}

/// The skill base name a command resolves to, if any: a direct hit, or a
/// `plugin:skill` whose bare segment ran. `None` for non-skill commands.
pub(super) fn command_skill_name(
    command: &str,
    skill_base_names: &HashSet<String>,
) -> Option<String> {
    if skill_base_names.contains(command) {
        return Some(command.to_string());
    }
    let bare = command.rsplit(':').next().unwrap_or(command);
    skill_base_names.contains(bare).then(|| bare.to_string())
}

pub(super) fn evidence_observations(value: &Value) -> Vec<EvidenceObservation> {
    let Some(attachment) = value.get("attachment") else {
        return Vec::new();
    };
    match attachment.get("type").and_then(Value::as_str) {
        Some("skill_listing") => attachment
            .get("content")
            .and_then(Value::as_str)
            .into_iter()
            .flat_map(|content| content.lines())
            .filter_map(|line| {
                let (name, description) = line.trim().strip_prefix("- ")?.split_once(':')?;
                let name = name.trim();
                if name.is_empty() {
                    return None;
                }
                let description = description.trim();
                Some(EvidenceObservation::ContextSource {
                    kind: ContextSourceKind::Skill,
                    name: name.to_owned(),
                    description: (!description.is_empty()).then(|| description.to_owned()),
                })
            })
            .collect(),
        Some("mcp_instructions_delta") => {
            let names = attachment
                .get("addedNames")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            let blocks = attachment
                .get("addedBlocks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            names
                .zip(blocks)
                .filter_map(|(name, block)| {
                    let name = name.as_str()?.trim();
                    if name.is_empty() {
                        return None;
                    }
                    Some(EvidenceObservation::ContextSource {
                        kind: ContextSourceKind::McpServer,
                        name: name.to_owned(),
                        description: block
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned),
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

pub(super) fn is_recognized_eventless(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("attachment" | "summary" | "file-history-snapshot")
    )
}

pub(super) fn record_discriminator(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
        .to_owned()
}

/// Parse a single JSON record into a normalized event, or `None` if the record
/// carries no analyzable signal (titles, summaries, metadata lines, …).
pub fn parse_record(value: &Value) -> Option<NormalizedEvent> {
    let obj = value.as_object()?;
    let msg = obj.get("message").and_then(|m| m.as_object());
    let top_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");

    let role = resolve_role(msg, obj, top_type)?;
    let mut ev = NormalizedEvent::new(role);

    ev.ts_ms = obj
        .get("timestamp")
        .or_else(|| obj.get("ts"))
        .or_else(|| obj.get("created_at"))
        .or_else(|| obj.get("createdAt"))
        .and_then(parse_ts);

    // Usage may live under message.usage (Anthropic) or top-level usage (OpenAI).
    let usage_value = msg
        .and_then(|m| m.get("usage"))
        .or_else(|| obj.get("usage"));
    ev.usage = parse_usage(usage_value);

    // The response speed (Claude's "standard"/"fast" fast-mode signal), when
    // the transcript records it: message.usage.speed, top-level usage.speed,
    // or a bare top-level speed field.
    ev.speed = usage_value
        .and_then(|u| u.get("speed"))
        .or_else(|| obj.get("speed"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|speed| !speed.is_empty())
        .map(str::to_string);

    // Model that produced this turn, when recorded: message.model (Anthropic) or
    // top-level model (OpenAI shape). `<synthetic>` is Claude's sentinel for
    // injected, unbilled turns — skip it so it never becomes a pricing key.
    ev.model = msg
        .and_then(|m| m.get("model"))
        .or_else(|| obj.get("model"))
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty() && *m != "<synthetic>")
        .map(str::to_string);

    ev.thinking_mode = obj
        .get("effort")
        .or_else(|| obj.get("reasoning_effort"))
        .or_else(|| msg.and_then(|m| m.get("effort")))
        .or_else(|| msg.and_then(|m| m.get("reasoning_effort")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .map(str::to_string);

    // Provider message id (Anthropic `message.id`), used by the Claude adapter to
    // de-duplicate re-logged copies of the same assistant message.
    ev.message_id = msg
        .and_then(|m| m.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    // Claude marks a compaction boundary with a top-level system record, and
    // (on most records) names the trigger and before/after size in
    // compactMetadata.
    if top_type == "system"
        && obj.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
    {
        ev.is_compaction_boundary = true;
        if let Some(meta) = obj.get("compactMetadata").and_then(|m| m.as_object()) {
            ev.compaction_trigger =
                meta.get("trigger")
                    .and_then(Value::as_str)
                    .and_then(|t| match t {
                        "manual" => Some(CompactionTrigger::Manual),
                        "auto" => Some(CompactionTrigger::Auto),
                        _ => None,
                    });
            ev.compaction_pre_tokens = meta.get("preTokens").and_then(Value::as_u64);
            ev.compaction_post_tokens = meta.get("postTokens").and_then(Value::as_u64);
        }
    }

    // Standalone, top-level tool-shaped records.
    match top_type {
        "tool_use" => push_named_tool(obj.get("name"), obj.get("input"), &mut ev),
        "function_call" => push_named_tool(
            obj.get("name").or_else(|| {
                obj.get("payload")
                    .and_then(|p| p.as_object())
                    .and_then(|p| p.get("name"))
            }),
            obj.get("input").or_else(|| obj.get("arguments")),
            &mut ev,
        ),
        "tool_result" | "toolResult" | "function_call_output" => {}
        _ => {}
    }

    // Content block: Anthropic array, OpenAI string, or nested under message.
    let content = msg
        .and_then(|m| m.get("content"))
        .or_else(|| obj.get("content"));
    process_content(content, &mut ev);
    // Claude Code writes a tool result as a `user` record whose content is a
    // `tool_result` block. That is the tool's turn, not a user prompt.
    if ev.role == Role::User && has_tool_result_block(content) {
        ev.role = Role::Tool;
    }

    // OpenAI-style tool calls.
    if let Some(calls) = msg
        .and_then(|m| m.get("tool_calls"))
        .or_else(|| obj.get("tool_calls"))
        .and_then(|c| c.as_array())
    {
        for call in calls {
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .or_else(|| call.get("name"))
                .and_then(|n| n.as_str());
            // OpenAI carries the args as a JSON string under function.arguments.
            let args = call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .or_else(|| call.get("arguments"))
                .or_else(|| call.get("input"));
            push_named_tool_str(name, args, &mut ev);
        }
    }

    Some(ev)
}

fn resolve_role(
    msg: Option<&serde_json::Map<String, Value>>,
    obj: &serde_json::Map<String, Value>,
    top_type: &str,
) -> Option<Role> {
    let role_str = msg
        .and_then(|m| m.get("role"))
        .or_else(|| obj.get("role"))
        .and_then(|r| r.as_str());
    let role = match role_str {
        Some("assistant") => Role::Assistant,
        Some("user") => Role::User,
        Some("system") => Role::System,
        Some("tool" | "toolResult") => Role::Tool,
        _ => match top_type {
            "assistant" => Role::Assistant,
            "user" => Role::User,
            "system" => Role::System,
            "tool_use" | "function_call" => Role::Assistant,
            "tool_result" | "toolResult" | "function_call_output" => Role::Tool,
            _ => return None,
        },
    };
    Some(role)
}

fn has_tool_result_block(content: Option<&Value>) -> bool {
    let Some(Value::Array(items)) = content else {
        return false;
    };
    items
        .iter()
        .any(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
}

fn process_content(content: Option<&Value>, ev: &mut NormalizedEvent) {
    if let Some(Value::Array(items)) = content {
        for item in items {
            let kind = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match kind {
                "tool_use" => push_named_tool(item.get("name"), item.get("input"), ev),
                "toolCall" => push_named_tool(
                    item.get("name"),
                    item.get("arguments").or_else(|| item.get("input")),
                    ev,
                ),
                "thinking" => ev.has_thinking = true,
                _ => {}
            }
        }
    }
}

fn push_named_tool(name: Option<&Value>, input: Option<&Value>, ev: &mut NormalizedEvent) {
    push_named_tool_str(name.and_then(|n| n.as_str()), input, ev);
}

fn push_named_tool_str(name: Option<&str>, input: Option<&Value>, ev: &mut NormalizedEvent) {
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        ev.tools.push(tool_call_from_input(name, input));
    }
}

/// The one shared, vendor-agnostic tool-call builder. Every adapter routes its
/// tool-construction sites through this so skill capture works for **all**
/// vendors, not just Claude: it builds the `ToolCall` (reclassifying Bash→Test
/// from the command, as before) and, when the tool is a `Skill`, fills
/// `ToolCall::detail` with the skill name parsed from the same input.
///
/// The JSON layer owns input parsing, so `model.rs` stays pure (no "skill"
/// concept). The Anthropic tool is literally named `Skill`, and other agents
/// emit a lowercase `skill` tool, so the case-insensitive match suffices — no
/// `normalize_tool_name` is needed in this crate.
pub(crate) fn tool_call_from_input(name: &str, input: Option<&Value>) -> ToolCall {
    let cmd = extract_command(input);
    let mut call = ToolCall::with_command(name, cmd.as_deref());
    if name.eq_ignore_ascii_case("skill") {
        call.detail = extract_skill_name(input);
    }
    call
}

/// Pull the shell command text out of a tool's input, so a Bash call that runs
/// tests uses the Test category. Accepts either the input object
/// (Anthropic `input.command`) or a JSON-encoded arguments string (OpenAI
/// `function.arguments`).
pub(crate) fn extract_command(input: Option<&Value>) -> Option<String> {
    match input? {
        obj @ Value::Object(_) => command_from_obj(obj),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .as_ref()
            .and_then(command_from_obj),
        _ => None,
    }
}

fn command_from_obj(v: &Value) -> Option<String> {
    v.get("command")
        .or_else(|| v.get("cmd"))
        .and_then(|c| c.as_str())
        .map(str::to_string)
}

/// Pull the invoked skill's name out of a `Skill` tool's input, across the same
/// two input shapes [`extract_command`] handles (an object, or a JSON-encoded
/// arguments string). Resolution order: the explicit name fields first
/// (`skill`, `name`, `skill_name`, `skillName`), else the `path` basename of a
/// `.../skills/<name>/SKILL.md`, else the first whitespace token of `command`
/// with a leading `/` stripped. `None` when nothing names a skill.
fn extract_skill_name(input: Option<&Value>) -> Option<String> {
    match input? {
        obj @ Value::Object(_) => extract_skill_name_from_value(obj),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .as_ref()
            .and_then(extract_skill_name_from_value),
        _ => None,
    }
}

fn extract_skill_name_from_value(value: &Value) -> Option<String> {
    let obj = value.as_object()?;
    ["skill", "name", "skill_name", "skillName"]
        .into_iter()
        .filter_map(|key| obj.get(key))
        .find_map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            obj.get("path")
                .and_then(|value| value.as_str())
                .and_then(extract_skill_name_from_path)
        })
        .or_else(|| {
            obj.get("command")
                .and_then(|value| value.as_str())
                .and_then(extract_skill_name_from_command)
        })
}

fn extract_skill_name_from_path(path: &str) -> Option<String> {
    let mut parts = path.rsplit(['/', '\\']).filter(|part| !part.is_empty());
    let file_name = parts.next()?;
    if !file_name.eq_ignore_ascii_case("SKILL.md") {
        return None;
    }
    let skill_name = parts.next()?.trim();
    (!skill_name.is_empty()).then(|| skill_name.to_string())
}

fn extract_skill_name_from_command(command: &str) -> Option<String> {
    let command_name = command.split_whitespace().next()?.trim_start_matches('/');
    (!command_name.is_empty()).then(|| command_name.to_string())
}

/// Parse usage while keeping vendor token buckets disjoint: OpenAI `prompt_tokens`
/// contains `cached_tokens`, while Anthropic `input_tokens` excludes its cache buckets.
pub(crate) fn parse_usage(value: Option<&Value>) -> Usage {
    let Some(obj) = value.and_then(|v| v.as_object()) else {
        return Usage::default();
    };
    let get = |keys: &[&str]| -> u64 {
        for k in keys {
            if let Some(n) = obj.get(*k).and_then(as_u64) {
                return n;
            }
        }
        0
    };
    let input_tokens = match (
        obj.get("input_tokens").and_then(as_u64),
        obj.get("prompt_tokens").and_then(as_u64),
    ) {
        (Some(n), _) => n,
        // OpenAI prompt_tokens includes cached reads; split them (CH-004).
        (None, Some(p)) => p.saturating_sub(get(&["cached_tokens"])),
        // Pi's disjoint camelCase shape; its buckets never overlap, so no subtraction.
        (None, None) => get(&["input"]),
    };
    Usage {
        input_tokens,
        output_tokens: get(&["output_tokens", "completion_tokens", "output"]),
        cache_read_tokens: get(&[
            "cache_read_input_tokens",
            "cached_tokens",
            "cache_read_tokens",
            "cacheRead",
        ]),
        cache_creation_tokens: get(&[
            "cache_creation_input_tokens",
            "cache_creation_tokens",
            "cacheWrite",
        ]),
    }
}

fn as_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
}

/// Parse a timestamp value (RFC 3339 string or epoch number) into epoch millis.
pub(crate) fn parse_ts(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => {
            let raw = n.as_f64()? as i64;
            // Heuristic: values below ~Nov-2286-in-seconds are seconds, else ms.
            if raw.abs() < 100_000_000_000 {
                Some(raw * 1000)
            } else {
                Some(raw)
            }
        }
        Value::String(s) => OffsetDateTime::parse(s, &Rfc3339)
            .ok()
            .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::model::{Role, ToolCategory};
    use serde_json::json;

    #[test]
    fn message_usage_speed_is_parsed() {
        let record = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "usage": {"input_tokens": 10, "output_tokens": 5, "speed": "fast"}
            }
        });
        let ev = parse_record(&record).expect("record should parse");
        assert_eq!(ev.speed.as_deref(), Some("fast"));
    }

    #[test]
    fn top_level_speed_is_parsed_when_usage_carries_none() {
        let record = json!({
            "type": "assistant",
            "message": {"role": "assistant", "usage": {"input_tokens": 10}},
            "speed": "standard"
        });
        let ev = parse_record(&record).expect("record should parse");
        assert_eq!(ev.speed.as_deref(), Some("standard"));
    }

    #[test]
    fn missing_speed_leaves_it_none() {
        let record = json!({
            "type": "assistant",
            "message": {"role": "assistant", "usage": {"input_tokens": 10}}
        });
        let ev = parse_record(&record).expect("record should parse");
        assert_eq!(ev.speed, None);
    }

    #[test]
    fn thinking_content_block_sets_has_thinking() {
        let record = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "let me see"}]
            }
        });
        let ev = parse_record(&record).expect("record should parse");
        assert!(ev.has_thinking);
    }

    #[test]
    fn no_thinking_block_leaves_has_thinking_false() {
        let record = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "hi"}]
            }
        });
        let ev = parse_record(&record).expect("record should parse");
        assert!(!ev.has_thinking);
    }

    #[test]
    fn openai_cached_tokens_are_split_from_prompt_tokens() {
        let usage = json!({
            "prompt_tokens": 500,
            "completion_tokens": 100,
            "cached_tokens": 200
        });

        let parsed = parse_usage(Some(&usage));
        assert_eq!(parsed.input_tokens, 300);
        assert_eq!(parsed.output_tokens, 100);
        assert_eq!(parsed.cache_read_tokens, 200);
        assert_eq!(parsed.cache_creation_tokens, 0);
        assert_eq!(parsed.input_tokens + parsed.cache_read_tokens, 500);
    }

    #[test]
    fn openai_prompt_tokens_without_cache_are_unchanged() {
        let usage = json!({"prompt_tokens": 500, "completion_tokens": 100});

        let parsed = parse_usage(Some(&usage));
        assert_eq!(parsed.input_tokens, 500);
        assert_eq!(parsed.output_tokens, 100);
        assert_eq!(parsed.cache_read_tokens, 0);
    }

    #[test]
    fn openai_cached_tokens_subtraction_saturates() {
        let usage = json!({"prompt_tokens": 100, "cached_tokens": 250});

        let parsed = parse_usage(Some(&usage));
        assert_eq!(parsed.input_tokens, 0);
        assert_eq!(parsed.cache_read_tokens, 250);
    }

    #[test]
    fn anthropic_usage_buckets_are_unchanged() {
        let usage = json!({
            "input_tokens": 1000,
            "output_tokens": 50,
            "cache_read_input_tokens": 5000,
            "cache_creation_input_tokens": 700
        });

        let parsed = parse_usage(Some(&usage));
        assert_eq!(parsed.input_tokens, 1000);
        assert_eq!(parsed.output_tokens, 50);
        assert_eq!(parsed.cache_read_tokens, 5000);
        assert_eq!(parsed.cache_creation_tokens, 700);
    }

    #[test]
    fn input_tokens_key_prevents_cached_tokens_subtraction() {
        let usage = json!({"input_tokens": 1000, "cached_tokens": 400});

        let parsed = parse_usage(Some(&usage));
        assert_eq!(parsed.input_tokens, 1000);
        assert_eq!(parsed.cache_read_tokens, 400);
    }

    #[test]
    fn pi_usage_keys_parse_into_disjoint_buckets() {
        let usage = json!({
            "input": 2,
            "output": 8,
            "cacheRead": 0,
            "cacheWrite": 14792,
            "totalTokens": 14802
        });

        let parsed = parse_usage(Some(&usage));
        assert_eq!(parsed.input_tokens, 2);
        assert_eq!(parsed.output_tokens, 8);
        assert_eq!(parsed.cache_read_tokens, 0);
        assert_eq!(parsed.cache_creation_tokens, 14_792);
        // Effective input is fresh input plus cache writes: 2 + 14,792 = 14,794.
        assert_eq!(parsed.input_tokens + parsed.cache_creation_tokens, 14_794);

        let with_cache_read = json!({
            "input": 3,
            "output": 5,
            "cacheRead": 700,
            "cacheWrite": 11
        });
        let parsed = parse_usage(Some(&with_cache_read));
        assert_eq!(parsed.input_tokens, 3);
        assert_eq!(parsed.output_tokens, 5);
        assert_eq!(parsed.cache_read_tokens, 700);
        assert_eq!(parsed.cache_creation_tokens, 11);
    }

    #[test]
    fn pi_keys_yield_to_standard_usage_keys() {
        let usage = json!({
            "input_tokens": 100,
            "input": 2,
            "output_tokens": 200,
            "output": 8
        });

        let parsed = parse_usage(Some(&usage));
        assert_eq!(parsed.input_tokens, 100);
        assert_eq!(parsed.output_tokens, 200);
    }

    #[test]
    fn pi_tool_call_content_block_yields_named_tool() {
        let record = json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": "call_1",
                    "name": "read",
                    "arguments": {"path": "src/lib.rs"}
                }]
            }
        });

        let ev = parse_record(&record).expect("record should parse");
        assert_eq!(ev.role, Role::Assistant);
        assert_eq!(ev.tools.len(), 1);
        assert_eq!(ev.tools[0].name, "read");
        assert_eq!(ev.tools[0].category, ToolCategory::Read);
    }

    #[test]
    fn pi_tool_call_arguments_feed_bash_test_classification() {
        let record = json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": "call_1",
                    "name": "bash",
                    "arguments": {"command": "cargo test --workspace"}
                }]
            }
        });

        let ev = parse_record(&record).expect("record should parse");
        assert_eq!(ev.tools.len(), 1);
        assert_eq!(ev.tools[0].category, ToolCategory::Test);
    }

    #[test]
    fn claude_tool_result_user_record_is_a_tool_event() {
        let result = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
            }
        });
        let prompt = json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": "go"}]}
        });
        assert_eq!(parse_record(&result).unwrap().role, Role::Tool);
        assert_eq!(parse_record(&prompt).unwrap().role, Role::User);
    }

    #[test]
    fn pi_tool_result_message_role_is_parsed() {
        let errored = json!({
            "type": "message",
            "message": {
                "role": "toolResult",
                "toolCallId": "call_1",
                "toolName": "bash",
                "isError": true,
                "content": [{"type": "text", "text": "boom"}]
            }
        });
        let ok = json!({
            "type": "message",
            "message": {
                "role": "toolResult",
                "toolCallId": "call_2",
                "toolName": "bash",
                "content": [{"type": "text", "text": "ok"}]
            }
        });

        let errored = parse_record(&errored).expect("tool result should parse");
        assert_eq!(errored.role, Role::Tool);

        let ok = parse_record(&ok).expect("tool result should parse");
        assert_eq!(ok.role, Role::Tool);
    }

    #[test]
    fn extract_skill_name_reads_explicit_fields_in_priority_order() {
        assert_eq!(
            extract_skill_name(Some(&json!({"skill": "deep-research"}))),
            Some("deep-research".to_string())
        );
        assert_eq!(
            extract_skill_name(Some(&json!({"name": "checkpoint"}))),
            Some("checkpoint".to_string())
        );
        assert_eq!(
            extract_skill_name(Some(&json!({"skill_name": "verify"}))),
            Some("verify".to_string())
        );
        assert_eq!(
            extract_skill_name(Some(&json!({"skillName": "plan"}))),
            Some("plan".to_string())
        );
        // `skill` wins over `name` when both are present.
        assert_eq!(
            extract_skill_name(Some(&json!({"skill": "a", "name": "b"}))),
            Some("a".to_string())
        );
    }

    #[test]
    fn extract_skill_name_falls_back_to_path_then_command() {
        assert_eq!(
            extract_skill_name(Some(
                &json!({"path": "/home/avery/.claude/skills/aside/SKILL.md"})
            )),
            Some("aside".to_string())
        );
        // Windows separators and a case-insensitive SKILL.md basename.
        assert_eq!(
            extract_skill_name(Some(&json!({"path": "C:\\u\\skills\\foo\\skill.md"}))),
            Some("foo".to_string())
        );
        // A path not ending in SKILL.md contributes nothing from the path branch.
        assert_eq!(
            extract_skill_name(Some(&json!({"path": "/x/y/z.txt"}))),
            None
        );
        // Command's first token, leading slash stripped.
        assert_eq!(
            extract_skill_name(Some(&json!({"command": "/sessions list"}))),
            Some("sessions".to_string())
        );
    }

    #[test]
    fn extract_skill_name_parses_json_encoded_args_string() {
        // OpenAI-style adapters hand arguments over as a JSON-encoded string.
        let args = Value::String(r#"{"skill":"deep-research"}"#.to_string());
        assert_eq!(
            extract_skill_name(Some(&args)),
            Some("deep-research".to_string())
        );
    }

    #[test]
    fn extract_skill_name_is_none_for_empty_or_unrelated_input() {
        assert_eq!(extract_skill_name(None), None);
        assert_eq!(extract_skill_name(Some(&json!({"skill": "   "}))), None);
        assert_eq!(extract_skill_name(Some(&json!({"other": "x"}))), None);
    }

    #[test]
    fn tool_call_from_input_captures_skill_detail_case_insensitively() {
        let call = tool_call_from_input("Skill", Some(&json!({"skill": "deep-research"})));
        assert_eq!(call.name, "Skill");
        // Skills stay in the `Other` tool category.
        assert_eq!(call.category, ToolCategory::Other);
        assert_eq!(call.detail.as_deref(), Some("deep-research"));

        // A lowercase `skill` tool (non-Claude agents) is captured too.
        let lower = tool_call_from_input("skill", Some(&json!({"name": "checkpoint"})));
        assert_eq!(lower.detail.as_deref(), Some("checkpoint"));
    }

    #[test]
    fn skill_base_name_from_path_takes_dir_or_skill_md_parent() {
        assert_eq!(
            skill_base_name_from_path("/home/avery/.claude/skills/grill-me"),
            Some("grill-me".to_string())
        );
        assert_eq!(
            skill_base_name_from_path("/home/avery/.claude/skills/code-review/SKILL.md"),
            Some("code-review".to_string())
        );
        // Windows separators.
        assert_eq!(
            skill_base_name_from_path("C:\\u\\.claude\\skills\\plan"),
            Some("plan".to_string())
        );
    }

    #[test]
    fn command_names_in_text_extracts_and_strips_slash() {
        let text = "<command-message>code-review</command-message>\n\
                    <command-name>/code-review</command-name>\n\
                    <command-args>changelist</command-args>";
        assert_eq!(command_names_in_text(text), vec!["code-review".to_string()]);
        assert_eq!(command_names_in_text("no tags here"), Vec::<String>::new());
    }

    #[test]
    fn command_skill_name_matches_directly_and_via_plugin_namespace() {
        let names: HashSet<String> = ["frontend-design".to_string(), "code-review".to_string()]
            .into_iter()
            .collect();
        // Direct hit.
        assert_eq!(
            command_skill_name("code-review", &names),
            Some("code-review".to_string())
        );
        // `plugin:skill` resolves to its bare segment.
        assert_eq!(
            command_skill_name("frontend-design:frontend-design", &names),
            Some("frontend-design".to_string())
        );
        // A command that didn't run as a skill is rejected (no base-dir marker).
        assert_eq!(command_skill_name("clear", &names), None);
    }

    #[test]
    fn parse_jsonl_synthesizes_skill_from_command_when_it_actually_ran() {
        // A user-typed `/code-review` (a command, not a Skill tool_use) plus the
        // skill's base-directory marker → one synthesized skill tool. A `/clear`
        // command with no base marker stays a non-skill.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /home/avery/.claude/skills/code-review\n\nReview the diff."}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2024-06-01T12:00:00Z","message":{"role":"user","content":"<command-name>/code-review</command-name>"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
        );
        let events = parse_jsonl(content);
        let skills: Vec<&str> = events
            .iter()
            .flat_map(|ev| &ev.tools)
            .filter(|t| t.name.eq_ignore_ascii_case("skill"))
            .filter_map(|t| t.detail.as_deref())
            .collect();
        assert_eq!(skills, vec!["code-review"]);
    }

    #[test]
    fn parse_jsonl_ignores_command_names_when_no_skill_ran() {
        // Without any base-directory marker, command-name tags never synthesize
        // skills — guards against custom slash commands becoming markers.
        let content = r#"{"type":"user","message":{"role":"user","content":"<command-name>/code-review</command-name>"}}"#;
        let events = parse_jsonl(content);
        let any_skill = events
            .iter()
            .flat_map(|ev| &ev.tools)
            .any(|t| t.name.eq_ignore_ascii_case("skill"));
        assert!(!any_skill);
    }

    #[test]
    fn claude_compact_boundary_record_sets_compaction_flag() {
        let record = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "content": "Compacted conversation"
        });

        let ev = parse_record(&record).expect("compact_boundary should parse");
        assert_eq!(ev.role, Role::System);
        assert!(ev.is_compaction_boundary);
    }

    #[test]
    fn claude_compact_boundary_parses_manual_trigger_and_sizes() {
        let record = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "compactMetadata": {
                "trigger": "manual",
                "preTokens": 196_000,
                "postTokens": 11_000,
            }
        });

        let ev = parse_record(&record).expect("compact_boundary should parse");
        assert_eq!(ev.compaction_trigger, Some(CompactionTrigger::Manual));
        assert_eq!(ev.compaction_pre_tokens, Some(196_000));
        assert_eq!(ev.compaction_post_tokens, Some(11_000));
    }

    #[test]
    fn claude_compact_boundary_parses_auto_trigger() {
        let record = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "compactMetadata": {
                "trigger": "auto",
                "preTokens": 200_000,
                "postTokens": 12_000,
            }
        });

        let ev = parse_record(&record).expect("compact_boundary should parse");
        assert_eq!(ev.compaction_trigger, Some(CompactionTrigger::Auto));
    }

    #[test]
    fn claude_compact_boundary_without_metadata_leaves_trigger_and_sizes_none() {
        let record = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
        });

        let ev = parse_record(&record).expect("compact_boundary should parse");
        assert!(ev.is_compaction_boundary);
        assert_eq!(ev.compaction_trigger, None);
        assert_eq!(ev.compaction_pre_tokens, None);
        assert_eq!(ev.compaction_post_tokens, None);
    }

    #[test]
    fn claude_compact_boundary_without_post_tokens_leaves_it_none() {
        // Some older records omit postTokens entirely.
        let record = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "compactMetadata": {
                "trigger": "auto",
                "preTokens": 196_000,
            }
        });

        let ev = parse_record(&record).expect("compact_boundary should parse");
        assert_eq!(ev.compaction_trigger, Some(CompactionTrigger::Auto));
        assert_eq!(ev.compaction_pre_tokens, Some(196_000));
        assert_eq!(ev.compaction_post_tokens, None);
    }

    #[test]
    fn unrelated_system_records_do_not_set_compaction_flag() {
        // No subtype at all.
        let plain = json!({
            "type": "system",
            "timestamp": "2024-06-01T12:05:00Z",
            "content": "hook ran"
        });
        let ev = parse_record(&plain).expect("system record should parse");
        assert!(!ev.is_compaction_boundary);

        // A different subtype.
        let other = json!({
            "type": "system",
            "subtype": "turn_limit_reached",
            "content": "stop"
        });
        let ev = parse_record(&other).expect("system record should parse");
        assert!(!ev.is_compaction_boundary);
    }

    #[test]
    fn tool_call_from_input_leaves_non_skill_detail_none_and_keeps_command_class() {
        // A non-skill tool never gets `detail`, even when its input has a skill key.
        let read = tool_call_from_input("Read", Some(&json!({"skill": "x"})));
        assert_eq!(read.detail, None);
        // Bash→Test reclassification from the command still flows through the builder.
        let test = tool_call_from_input("Bash", Some(&json!({"command": "cargo test"})));
        assert_eq!(test.category, ToolCategory::Test);
        assert_eq!(test.detail, None);
    }
}
