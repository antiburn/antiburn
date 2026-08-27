//! Codex adapter — Codex Desktop / CLI "rollout" transcripts.
//!
//! Codex writes one JSONL file per session
//! (`~/.codex/sessions/<date>/rollout-*.jsonl`). Every line is an envelope
//! `{timestamp, type, payload}`:
//!
//! - `response_item` carries the model-API conversation — `message`
//!   (role + `content[].text`), direct `function_call` / `custom_tool_call`
//!   invocations, their outputs, and `reasoning` (thinking). Current Codex
//!   Desktop sessions wrap real tool calls in a `custom_tool_call` named `exec`
//!   whose input is JavaScript (`tools.apply_patch(...)`,
//!   `tools.exec_command(...)`, …); this adapter lexes those calls as data and
//!   never evaluates the script.
//! - `event_msg` carries UI-layer events; the only one we need is `token_count`,
//!   whose `info.last_token_usage` is the latest turn's usage (its `input_tokens`
//!   is the live prompt size = context-window occupancy) and whose
//!   `info.model_context_window` gives the model's real window. (The duplicate
//!   `user_message` / `agent_message` echoes of `response_item` turns are
//!   skipped so turns aren't double-counted.)
//! - `compacted` is a top-level envelope (not an `event_msg`) that newer Codex
//!   rollouts write when a compaction finishes. Older rollouts instead (or
//!   also) emit `{"type":"event_msg","payload":{"type":"context_compacted"}}`.
//!   Both mark the same event; see `compaction_event` for how the parser
//!   avoids double-counting when a rollout emits both.
//!
//! The shared `parse_record` only understands `role`/`content` at the top level
//! or under `message`, so it drops every Codex line — the data is nested under
//! `payload` and the top-level `type` is `response_item`. This adapter unwraps
//! the envelope.

use anyhow::Context;
use serde_json::{Map, Value};

use super::jsonl::{parse_ts, tool_call_from_input};
use super::read_source;
use crate::analysis::interface::{SessionInput, VendorAdapter};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role, ToolCall, Usage};

pub struct CodexAdapter;

impl VendorAdapter for CodexAdapter {
    fn agent(&self) -> &'static str {
        "codex"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading codex session {}", input.session_id))?;
        let (events, context_window, model) = parse_codex(&content);
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events,
            cache_write_tokens_available: false,
            context_window,
            model,
        })
    }
}

fn parse_codex(content: &str) -> (Vec<NormalizedEvent>, Option<u64>, Option<String>) {
    let mut events = Vec::new();
    // A forked rollout starts by replaying its parent's history. Keep those
    // records available to the desktop analysis view, but do not attribute
    // their already-billed token_count events to the child. The first task
    // addressed to the child's agent path marks the owned usage boundary.
    let owned_usage_start = codex_fork_owned_offset(content);
    // The model's context-window size, reported on each `token_count` event's
    // `info.model_context_window`. Constant per model; take the first seen.
    let mut context_window = None;
    // The model id, reported on the `session_meta` / `turn_context` envelope.
    // Best-effort across the pointers Codex has used; first non-empty wins. `None`
    // is fine — cost then has no local estimate for the session.
    let mut model: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_thinking_mode: Option<String> = None;
    // Dedupe state for compaction boundaries: some rollouts write a
    // `context_compacted` event_msg and a top-level `compacted` record
    // back-to-back for the same compaction (see `compaction_event`).
    let mut previous_event_was_boundary = false;
    let mut previous_boundary_ts: Option<i64> = None;
    // Dedupe state for `token_count` rows (see `token_count_key`).
    let mut previous_token_count_key: Option<TokenCountKey> = None;
    let mut offset = 0;
    for line_with_ending in content.split_inclusive('\n') {
        let line_offset = offset;
        offset += line_with_ending.len();
        let line = line_with_ending.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            let usage_is_owned = owned_usage_start.is_none_or(|start| line_offset >= start);
            if usage_is_owned && context_window.is_none() {
                context_window = value
                    .pointer("/payload/info/model_context_window")
                    .and_then(Value::as_u64)
                    .filter(|&w| w > 0);
            }
            if usage_is_owned {
                if let Some(next_model) = [
                    "/payload/model",
                    "/payload/info/model",
                    "/payload/turn_context/model",
                ]
                .iter()
                .find_map(|p| value.pointer(p).and_then(Value::as_str))
                .map(str::trim)
                .filter(|model| !model.is_empty())
                {
                    current_model = Some(next_model.to_string());
                    if model.is_none() {
                        model = current_model.clone();
                    }
                }
                if let Some(next_mode) = [
                    "/payload/effort",
                    "/payload/reasoning_effort",
                    "/payload/turn_context/effort",
                    "/payload/turn_context/reasoning_effort",
                    "/payload/thread_settings/reasoning_effort",
                    "/payload/collaboration_mode/settings/reasoning_effort",
                ]
                .iter()
                .find_map(|p| value.pointer(p).and_then(Value::as_str))
                .map(str::trim)
                .filter(|mode| !mode.is_empty())
                {
                    current_thinking_mode = Some(next_mode.to_string());
                }
                // Codex rollouts carry no speed/fast-mode signal like Claude's
                // `usage.speed`, so `NormalizedEvent.speed` stays `None` here.
            }
            let inherited_token_count = !usage_is_owned
                && value.get("type").and_then(Value::as_str) == Some("event_msg")
                && value.pointer("/payload/type").and_then(Value::as_str) == Some("token_count");
            if let Some(key) = token_count_key(&value) {
                let duplicate_token_count = previous_token_count_key.as_ref() == Some(&key);
                previous_token_count_key = Some(key);
                if duplicate_token_count {
                    continue;
                }
            }
            if !inherited_token_count && let Some(mut ev) = record_to_event(&value) {
                if usage_is_owned {
                    ev.model = ev.model.or_else(|| current_model.clone());
                    ev.thinking_mode = current_thinking_mode.clone();
                }
                let duplicate_boundary = ev.is_compaction_boundary
                    && previous_event_was_boundary
                    && ev
                        .ts_ms
                        .zip(previous_boundary_ts)
                        .is_none_or(|(cur, prev)| {
                            (cur - prev).abs() <= COMPACTION_DEDUPE_WINDOW_MS
                        });
                if !duplicate_boundary {
                    previous_event_was_boundary = ev.is_compaction_boundary;
                    if ev.is_compaction_boundary {
                        previous_boundary_ts = ev.ts_ms;
                    }
                    events.push(ev);
                }
            }
        }
    }
    (events, context_window, model)
}

/// Locate the first row whose token usage belongs to a Codex fork itself.
///
/// Codex rehydrates the parent's rollout into a child file before the child's
/// first task. Those inherited rows are useful context, but their token_count
/// events describe requests already made by the parent. A task addressed to the
/// child's agent path ends the replay; include its preceding developer message
/// when Codex emits one immediately before task_started.
fn codex_fork_owned_offset(content: &str) -> Option<usize> {
    let mut agent_path: Option<String> = None;
    let mut last_task_started_offset: Option<usize> = None;
    let mut previous_row: Option<(usize, bool)> = None;
    let mut offset = 0;

    for line_with_ending in content.split_inclusive('\n') {
        let line = line_with_ending.trim();
        let value = serde_json::from_str::<Value>(line).ok();

        if let Some(value) = value.as_ref() {
            let row_type = value.get("type").and_then(Value::as_str);
            let payload_type = value.pointer("/payload/type").and_then(Value::as_str);

            if row_type == Some("session_meta")
                && value
                    .pointer("/payload/thread_source")
                    .and_then(Value::as_str)
                    == Some("subagent")
            {
                agent_path = value
                    .pointer("/payload/agent_path")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }

            if row_type == Some("event_msg") && payload_type == Some("task_started") {
                last_task_started_offset = Some(
                    previous_row
                        .filter(|(_, is_developer_message)| *is_developer_message)
                        .map(|(previous_offset, _)| previous_offset)
                        .unwrap_or(offset),
                );
            }

            let addressed_to_child = row_type == Some("response_item")
                && payload_type == Some("agent_message")
                && value
                    .pointer("/payload/recipient")
                    .and_then(Value::as_str)
                    .zip(agent_path.as_deref())
                    .is_some_and(|(recipient, path)| recipient == path);
            if addressed_to_child {
                return Some(last_task_started_offset.unwrap_or(offset));
            }

            let is_developer_message = row_type == Some("response_item")
                && payload_type == Some("message")
                && value.pointer("/payload/role").and_then(Value::as_str) == Some("developer");
            previous_row = Some((offset, is_developer_message));
        }

        offset += line_with_ending.len();
    }

    None
}

/// Map one rollout envelope record to a normalized event, or `None` for framing
/// / bookkeeping records that carry no analyzable signal (`session_meta`,
/// `turn_context`, `task_started`, and the `user_message` / `agent_message` UI
/// echoes of `response_item` turns).
fn record_to_event(record: &Value) -> Option<NormalizedEvent> {
    let obj = record.as_object()?;
    let ts = obj.get("timestamp").and_then(parse_ts);
    let rec_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = obj.get("payload").and_then(|p| p.as_object())?;
    let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match (rec_type, payload_type) {
        ("response_item", "message") => message_event(payload, ts),
        ("response_item", "reasoning") => Some(reasoning_event(payload, ts)),
        ("response_item", "function_call_output")
        | ("response_item", "custom_tool_call_output") => Some(tool_output_event(payload, ts)),
        ("response_item", "custom_tool_call") => custom_tool_call_event(payload, ts),
        // Any other response_item that names a tool is an invocation
        // (function_call, local_shell_call, …).
        ("response_item", _) if payload.contains_key("name") => function_call_event(payload, ts),
        ("event_msg", "token_count") => token_count_event(payload, ts),
        ("event_msg", "context_compacted") => Some(compaction_event(ts)),
        ("compacted", _) => Some(compaction_event(ts)),
        _ => None,
    }
}

fn message_event(payload: &Map<String, Value>, ts: Option<i64>) -> Option<NormalizedEvent> {
    let role = match payload.get("role").and_then(|r| r.as_str()) {
        Some("assistant") => Role::Assistant,
        Some("user") => Role::User,
        // Codex injects instructions as `developer` / `system` turns.
        Some("system") | Some("developer") => Role::System,
        _ => return None,
    };
    let mut ev = NormalizedEvent::new(role);
    ev.ts_ms = ts;
    Some(ev)
}

fn reasoning_event(_payload: &Map<String, Value>, ts: Option<i64>) -> NormalizedEvent {
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;
    // A `reasoning` response_item is Codex's chain-of-thought turn, the
    // vendor equivalent of a Claude `thinking` content block.
    ev.has_thinking = true;
    ev
}

fn function_call_event(payload: &Map<String, Value>, ts: Option<i64>) -> Option<NormalizedEvent> {
    let name = payload
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())?;
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;
    // `arguments` is a JSON-encoded string; the shared builder digs out the shell
    // command (so a Bash-class call that runs tests reclassifies to Testing) and
    // the skill name (when this is a `Skill` call) from the same input.
    let input = payload.get("arguments").or_else(|| payload.get("input"));
    ev.tools.push(tool_call_from_input(name, input));
    Some(ev)
}

/// Normalize a Codex custom tool call.
///
/// Current Codex Desktop wraps one or more actual tool calls in an outer `exec`
/// script. When that bounded shape is recognized, expose the nested tools and
/// omit the wrapper from `tools` so tool-mix accounting reflects the work
/// itself; `wrapper_tool` still names the wrapper, so its own use as a
/// built-in tool is not lost. Unknown/malformed scripts retain the outer
/// `exec` Bash fallback (in `tools`, with no `wrapper_tool`).
fn custom_tool_call_event(
    payload: &Map<String, Value>,
    ts: Option<i64>,
) -> Option<NormalizedEvent> {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    let input = payload.get("input").or_else(|| payload.get("arguments"));
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;

    if name == "exec"
        && let Some(script) = input.and_then(Value::as_str)
    {
        ev.tools = nested_exec_tool_calls(script);
    }
    if ev.tools.is_empty() {
        ev.tools.push(tool_call_from_input(name, input));
    } else {
        // The wrapper itself is the real built-in tool whose definition costs
        // tokens. Record its use separately from `tools`, so tool-mix
        // accounting still reflects only the nested work it did.
        ev.wrapper_tool = Some(name.to_string());
    }
    Some(ev)
}

/// Lex `tools.<identifier>(...)` calls from a Codex `exec` script without
/// executing or fully parsing JavaScript. String/template contents and comments
/// are skipped so examples or command output cannot masquerade as invocations.
/// A JSON-compatible object first argument is retained for command-aware
/// classification. For the JavaScript-object form Codex also emits
/// (`exec_command({cmd:"cargo test"})`), the string-valued `cmd` property is
/// extracted lexically.
fn nested_exec_tool_calls(script: &str) -> Vec<ToolCall> {
    let bytes = script.as_bytes();
    let mut tools = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = skip_javascript_string(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index + 2);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
                continue;
            }
            _ => {}
        }

        let Some(mut cursor) = match_identifier(bytes, index, b"tools") else {
            index += 1;
            continue;
        };
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b'.') {
            index += 1;
            continue;
        }
        cursor = skip_ascii_whitespace(bytes, cursor + 1);
        let name_start = cursor;
        while bytes
            .get(cursor)
            .is_some_and(|byte| is_javascript_identifier_continue(*byte))
        {
            cursor += 1;
        }
        if cursor == name_start {
            index += 1;
            continue;
        }
        let name = &script[name_start..cursor];
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b'(') {
            index += 1;
            continue;
        }

        let argument_start = skip_ascii_whitespace(bytes, cursor + 1);
        let parsed_argument = (bytes.get(argument_start) == Some(&b'{'))
            .then(|| balanced_object_end(bytes, argument_start))
            .flatten()
            .and_then(|end| parse_object_argument(&script[argument_start..end]));
        tools.push(tool_call_from_input(name, parsed_argument.as_ref()));
        index = cursor + 1;
    }

    tools
}

fn parse_object_argument(argument: &str) -> Option<Value> {
    serde_json::from_str(argument).ok().or_else(|| {
        let command = javascript_object_string_property(argument, b"cmd")?;
        let mut object = Map::new();
        object.insert("cmd".to_string(), Value::String(command));
        Some(Value::Object(object))
    })
}

/// Extract a top-level, string-valued property from a JavaScript object literal.
/// This intentionally supports only the bounded shape needed for command
/// classification; expressions and nested properties are ignored.
fn javascript_object_string_property(argument: &str, property: &[u8]) -> Option<String> {
    let bytes = argument.as_bytes();
    let mut object_depth = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = skip_javascript_string(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index + 2);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
                continue;
            }
            b'{' => object_depth += 1,
            b'}' => object_depth = object_depth.checked_sub(1)?,
            _ if object_depth == 1 => {
                let Some(mut cursor) = match_identifier(bytes, index, property) else {
                    index += 1;
                    continue;
                };
                cursor = skip_ascii_whitespace(bytes, cursor);
                if bytes.get(cursor) != Some(&b':') {
                    index += 1;
                    continue;
                }
                cursor = skip_ascii_whitespace(bytes, cursor + 1);
                let quote = *bytes.get(cursor)?;
                if !matches!(quote, b'\'' | b'"') {
                    index += 1;
                    continue;
                }
                let end = skip_javascript_string(bytes, cursor);
                if end <= cursor + 1 || bytes.get(end - 1) != Some(&quote) {
                    return None;
                }
                return Some(argument[cursor + 1..end - 1].to_string());
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn match_identifier(bytes: &[u8], start: usize, expected: &[u8]) -> Option<usize> {
    let end = start.checked_add(expected.len())?;
    if bytes.get(start..end) != Some(expected)
        || start
            .checked_sub(1)
            .and_then(|index| bytes.get(index))
            .is_some_and(|byte| is_javascript_identifier_continue(*byte))
        || bytes
            .get(end)
            .is_some_and(|byte| is_javascript_identifier_continue(*byte))
    {
        return None;
    }
    Some(end)
}

fn is_javascript_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn skip_javascript_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while let Some(byte) = bytes.get(index) {
        if *byte == b'\\' {
            index = (index + 2).min(bytes.len());
        } else if *byte == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    bytes.len()
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(|byte| *byte != b'\n') {
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> usize {
    while index + 1 < bytes.len() {
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }
        index += 1;
    }
    bytes.len()
}

/// Return the exclusive end of a balanced object beginning at `start`.
fn balanced_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = skip_javascript_string(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index + 2);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn tool_output_event(_payload: &Map<String, Value>, ts: Option<i64>) -> NormalizedEvent {
    let mut ev = NormalizedEvent::new(Role::Tool);
    ev.ts_ms = ts;
    ev
}

fn token_count_event(payload: &Map<String, Value>, ts: Option<i64>) -> Option<NormalizedEvent> {
    let info = payload.get("info").and_then(|i| i.as_object())?;
    // `last_token_usage` is the latest turn's usage; its `input_tokens` is the
    // full prompt that turn — i.e. the live context-window occupancy (it climbs
    // as history accumulates and drops on compaction). `total_token_usage` is the
    // lifetime cumulative and must not be used for occupancy. It grows beyond
    // the context window and would peg the chart at 100%.
    let usage_obj = info
        .get("last_token_usage")
        .or_else(|| info.get("total_token_usage"))
        .and_then(|u| u.as_object())?;
    let usage = codex_usage(usage_obj);
    if usage == Usage::default() {
        return None;
    }
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;
    ev.usage = usage;
    Some(ev)
}

/// The usage pair that identifies one `token_count` row.
type TokenCountKey = (Value, Value);

/// Codex writes the last `token_count` row again when it resumes a rollout.
/// The copy repeats both `last_token_usage` and `total_token_usage` exactly.
/// A real turn always moves `total_token_usage`, so `parse_codex` drops a row
/// whose pair equals the previous `token_count` row. This keeps a resume from
/// adding a ghost turn with a second copy of the same context occupancy.
fn token_count_key(value: &Value) -> Option<TokenCountKey> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg")
        || value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count")
    {
        return None;
    }
    let info = value.pointer("/payload/info")?;
    Some((
        info.get("last_token_usage").cloned().unwrap_or(Value::Null),
        info.get("total_token_usage")
            .cloned()
            .unwrap_or(Value::Null),
    ))
}

/// The largest gap between two compaction records that `parse_codex` still
/// treats as one compaction, not two. Real, distinct compactions in the wild
/// are separated by turns and are minutes to hours apart; the sibling
/// records for one compaction land in the same instant. Five seconds gives a
/// wide safety margin without risking a merge of two real compactions.
const COMPACTION_DEDUPE_WINDOW_MS: i64 = 5_000;

/// Codex marks a completed compaction two different ways, depending on
/// rollout version: a top-level `{"type":"compacted","payload":{...}}`
/// record (current rollouts), or `{"type":"event_msg","payload":
/// {"type":"context_compacted"}}` (older rollouts). Some older rollouts write
/// both, back-to-back, for the same compaction. `parse_codex` matches both
/// shapes to this function and drops a second boundary event when the
/// previous event it emitted was also a compaction boundary at (about) the
/// same timestamp, so one compaction still produces exactly one boundary.
fn compaction_event(ts: Option<i64>) -> NormalizedEvent {
    let mut ev = NormalizedEvent::new(Role::System);
    ev.ts_ms = ts;
    ev.is_compaction_boundary = true;
    ev
}

fn codex_usage(u: &Map<String, Value>) -> Usage {
    let get = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let input = get("input_tokens");
    let cached = get("cached_input_tokens");
    Usage {
        // Codex reports cached tokens *inside* `input_tokens`; split them out so
        // context occupancy (input + cache_read) isn't double-counted.
        input_tokens: input.saturating_sub(cached),
        output_tokens: get("output_tokens"),
        cache_read_tokens: cached,
        cache_creation_tokens: 0,
    }
}
