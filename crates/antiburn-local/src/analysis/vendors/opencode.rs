// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! OpenCode adapter.
//!
//! OpenCode stores each session in `opencode.db` across two tables: `message`
//! (one row per turn — role + token usage) and `part` (the turn's content, split
//! into `text` / `reasoning` / `tool` / `patch` / `file` fragments linked to
//! their message by `message_id`). The discovery layer reads both tables and
//! emits a JSONL stream where each line is tagged
//! `{"type":"message"|"part", …}` with the row's raw `data` under `payload`.
//!
//! The shared `parse_record` can't reconstruct a turn from these split records:
//! a `type:"part"` line has no role, and a `type:"message"` line carries usage
//! under `payload.tokens` (not `usage`) with no inline content. This adapter
//! joins the parts back onto their message by `messageID`, producing one
//! [`NormalizedEvent`] per turn — matching Claude's one-event-per-message
//! granularity.
//!
//! A raw [`RawSource::Sqlite`] (a database handed straight in, rather than the
//! discovery layer's synthesized JSONL) falls back to the generic table walk.

use std::collections::HashMap;

use anyhow::Context;
use serde_json::{Map, Value};

use super::jsonl::{parse_ts, tool_call_from_input};
use super::read_source;
use crate::analysis::interface::{RawSource, SessionInput, VendorAdapter};
use crate::analysis::model::{
    NormalizedEvent, NormalizedSession, Role, ToolCall, ToolCategory, Usage,
};

pub struct OpenCodeAdapter;

impl VendorAdapter for OpenCodeAdapter {
    fn agent(&self) -> &'static str {
        "opencode"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        // A database handed straight in (not the discovery layer's synthesized
        // message/part JSONL) falls back to the generic SQLite table walk.
        if matches!(input.source, RawSource::Sqlite(_)) {
            return super::sqlite::SqliteAdapter { agent: "opencode" }.normalize(input);
        }
        let content = read_source(&input.source)
            .with_context(|| format!("reading opencode session {}", input.session_id))?;
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events: parse_opencode(&content),
            cache_write_tokens_available: true,
            context_window: None,
            model: None,
        })
    }
}

/// Join the discovery layer's `message` + `part` JSONL stream into one event per
/// message. Parts attach to their message by `messageID`; a part seen before its
/// message lazily creates the event (its role is corrected when the message
/// line arrives).
fn parse_opencode(content: &str) -> Vec<NormalizedEvent> {
    let mut order: Vec<String> = Vec::new();
    let mut by_msg: HashMap<String, NormalizedEvent> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(obj) = value.as_object() else {
            continue;
        };
        match obj.get("type").and_then(|t| t.as_str()) {
            Some("message") => apply_message(obj, &mut by_msg, &mut order),
            Some("part") => apply_part(obj, &mut by_msg, &mut order),
            _ => {}
        }
    }

    order
        .into_iter()
        .filter_map(|id| by_msg.remove(&id))
        .collect()
}

/// Get the event for `id`, creating it (and recording first-seen order) if new.
fn event_for<'a>(
    id: &str,
    default_role: Role,
    by_msg: &'a mut HashMap<String, NormalizedEvent>,
    order: &mut Vec<String>,
) -> &'a mut NormalizedEvent {
    if !by_msg.contains_key(id) {
        order.push(id.to_string());
        by_msg.insert(id.to_string(), NormalizedEvent::new(default_role));
    }
    by_msg.get_mut(id).expect("just inserted")
}

fn apply_message(
    obj: &Map<String, Value>,
    by_msg: &mut HashMap<String, NormalizedEvent>,
    order: &mut Vec<String>,
) {
    let Some(id) = obj.get("messageID").and_then(|v| v.as_str()) else {
        return;
    };
    let role = role_of(obj.get("role").and_then(|r| r.as_str()));
    let ts = obj
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(parse_ts);
    let usage = obj
        .get("payload")
        .and_then(|p| p.get("tokens"))
        .and_then(|t| t.as_object())
        .map(opencode_usage)
        .unwrap_or_default();

    let ev = event_for(id, role, by_msg, order);
    if ev.ts_ms.is_none() {
        ev.ts_ms = ts;
    }
    ev.usage = ev.usage.saturating_add(usage);
}

fn apply_part(
    obj: &Map<String, Value>,
    by_msg: &mut HashMap<String, NormalizedEvent>,
    order: &mut Vec<String>,
) {
    let Some(id) = obj.get("messageID").and_then(|v| v.as_str()) else {
        return;
    };
    let part_type = obj
        .get("partType")
        .and_then(|t| t.as_str())
        .or_else(|| {
            obj.get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
        })
        .unwrap_or("");
    let payload = obj.get("payload");
    let ts = obj
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(parse_ts);

    // Parts belong to an assistant turn unless their message says otherwise.
    let ev = event_for(id, Role::Assistant, by_msg, order);
    if ev.ts_ms.is_none() {
        ev.ts_ms = ts;
    }

    match part_type {
        "text" | "reasoning" => {}
        "tool" => apply_tool_part(payload, ev),
        // A patch part is a committed file edit (it lists the files it touched).
        // No tool input to parse, so it stays a literal with `detail: None`.
        "patch" => ev.tools.push(ToolCall {
            name: "patch".to_string(),
            category: ToolCategory::Edit,
            detail: None,
        }),
        // step-start / step-finish (its tokens duplicate the message row) and
        // file attachments carry no extra signal.
        _ => {}
    }
}

fn apply_tool_part(payload: Option<&Value>, ev: &mut NormalizedEvent) {
    let Some(p) = payload.and_then(|p| p.as_object()) else {
        return;
    };
    let name = p
        .get("tool")
        .and_then(|t| t.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("tool");
    let state = p.get("state");
    // OpenCode nests the tool args under `state.input` (e.g. `{command, …}`); the
    // shared builder pulls both the command and (for a `skill` tool) the skill name.
    let input = state.and_then(|s| s.get("input"));
    ev.tools.push(tool_call_from_input(name, input));
}

fn role_of(role: Option<&str>) -> Role {
    match role {
        Some("user") => Role::User,
        Some("system") => Role::System,
        Some("tool") => Role::Tool,
        _ => Role::Assistant,
    }
}

fn opencode_usage(tokens: &Map<String, Value>) -> Usage {
    let n = |k: &str| tokens.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cache = tokens.get("cache").and_then(|c| c.as_object());
    let cn = |k: &str| {
        cache
            .and_then(|c| c.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        // OpenCode's `input` already excludes cached tokens (input + cache.write
        // + output == total), so the fields map straight across.
        input_tokens: n("input"),
        output_tokens: n("output"),
        cache_read_tokens: cn("read"),
        cache_creation_tokens: cn("write"),
    }
}
