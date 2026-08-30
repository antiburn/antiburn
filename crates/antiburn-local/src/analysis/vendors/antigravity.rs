//! Antigravity adapter.
//!
//! Antigravity transcripts are step-based and don't fit the generic JSONL
//! parser: every step carries an uppercase `type` (`USER_INPUT`,
//! `PLANNER_RESPONSE`, `CORTEX_STEP_TYPE_*`) and no lowercase `role`, so the
//! generic `resolve_role` drops every record and the session normalizes to
//! empty. This adapter understands the two shapes Antigravity actually writes:
//!
//! 1. **Brain transcript** (`RawSource::Jsonl`): a synthesized one-line JSON
//!    header (`{"sessionId":…,"source":"antigravity_brain",…}`) followed by the
//!    raw `transcript.jsonl`, whose lines are steps like
//!    `{"type":"USER_INPUT","created_at":"…","content":"<USER_REQUEST>…"}` and
//!    `{"type":"PLANNER_RESPONSE","content":"…"}`.
//!
//! 2. **API cascade** (`RawSource::File`): a single JSON document
//!    `{"sessionId":…,"source":"antigravity_api","steps":<LSP response>}` whose
//!    steps live at the nested pointer `/steps/steps`, each shaped like
//!    `{"type":"CORTEX_STEP_TYPE_USER_INPUT","status":"ok","userInput":{…}}`.
//!
//! Token-usage and tool-name field locations are not documented. The adapter
//! reads likely locations and leaves missing values empty.

use anyhow::Context;
use serde_json::Value;

use super::read_source;
use crate::analysis::interface::{SessionInput, VendorAdapter};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role};
use crate::analysis::records::{parse_ts, parse_usage, tool_call_from_input};

pub struct AntigravityAdapter;

impl VendorAdapter for AntigravityAdapter {
    fn agent(&self) -> &'static str {
        "antigravity"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading antigravity session {}", input.session_id))?;
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events: parse_antigravity(&content),
            cache_write_tokens_available: true,
            context_window: None,
            model: None,
        })
    }
}

/// Parse either Antigravity shape into normalized events.
fn parse_antigravity(content: &str) -> Vec<NormalizedEvent> {
    // Cascade path: the whole payload is one JSON document carrying a steps
    // array at `/steps/steps` (or, defensively, a top-level `/steps`).
    if let Ok(doc) = serde_json::from_str::<Value>(content.trim())
        && let Some(steps) = doc
            .pointer("/steps/steps")
            .or_else(|| doc.pointer("/steps"))
            .and_then(|v| v.as_array())
    {
        return steps.iter().filter_map(step_to_event).collect();
    }

    // Brain path: line-delimited steps, preceded by a synthesized metadata
    // header line. Skip blank/malformed lines and the header rather than
    // failing the whole session.
    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && !is_meta_line(&value)
            && let Some(ev) = step_to_event(&value)
        {
            events.push(ev);
        }
    }
    events
}

/// True for the synthesized header / wrapper lines (`{"sessionId":…,
/// "source":"antigravity_brain"…}`) that carry no step payload.
fn is_meta_line(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    if matches!(
        obj.get("source").and_then(|s| s.as_str()),
        Some("antigravity_brain") | Some("antigravity_api")
    ) {
        return true;
    }
    // A session-wrapper object: identifies a session but carries no step shape.
    obj.contains_key("sessionId")
        && !obj.contains_key("type")
        && !obj.contains_key("content")
        && !obj.contains_key("userInput")
}

/// Map one Antigravity step to a normalized event, or `None` when it carries no
/// analyzable signal.
fn step_to_event(step: &Value) -> Option<NormalizedEvent> {
    let obj = step.as_object()?;
    let raw_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let kind = normalize_type(raw_type);

    let has_content = obj.contains_key("content") || obj.contains_key("userInput");
    let role = match role_for(&kind) {
        Some(role) => role,
        None if has_content => Role::Assistant,
        None => return None,
    };

    let mut ev = NormalizedEvent::new(role);

    ev.ts_ms = obj
        .get("created_at")
        .or_else(|| obj.get("timestamp"))
        .or_else(|| obj.get("createdAt"))
        // API-cascade steps (from `GetCascadeTrajectorySteps`) nest the timestamp
        // under `metadata` rather than at the top level — without this every
        // event parses with no time, so the session shows "0 mins active".
        .or_else(|| obj.get("metadata").and_then(|m| m.get("createdAt")))
        .or_else(|| obj.get("metadata").and_then(|m| m.get("startedAt")))
        .and_then(parse_ts);

    // Antigravity attaches tool invocations as a `tool_calls[]` array on the
    // PLANNER_RESPONSE step (each `{name, args}`), with separate action-named
    // result steps (VIEW_FILE, GREP_SEARCH, …). The names live in `tool_calls`,
    // so read them off every step regardless of role.
    push_tool_calls(obj, &mut ev);
    // Fallback: a Tool-role step that names its tool inline (no tool_calls).
    if ev.tools.is_empty()
        && role == Role::Tool
        && let Some(name) = tool_name(obj)
    {
        let input = obj.get("args").or_else(|| obj.get("input"));
        ev.tools.push(tool_call_from_input(&name, input));
    }

    // Best-effort token usage from any of the likely usage containers.
    ev.usage = parse_usage(
        obj.get("usage")
            .or_else(|| obj.get("tokenUsage"))
            .or_else(|| obj.get("tokens")),
    );

    Some(ev)
}

/// Strip a `CORTEX_STEP_TYPE_` prefix and uppercase, so the brain
/// (`USER_INPUT`) and cascade (`CORTEX_STEP_TYPE_USER_INPUT`) namings collapse
/// to one key.
fn normalize_type(raw: &str) -> String {
    raw.trim()
        .strip_prefix("CORTEX_STEP_TYPE_")
        .unwrap_or(raw)
        .to_ascii_uppercase()
}

/// Resolve a normalized type key to its role.
fn role_for(kind: &str) -> Option<Role> {
    if kind == "USER_INPUT" {
        return Some(Role::User);
    }
    if kind.contains("PLAN") || kind.contains("THINK") || kind.contains("REASON") {
        return Some(Role::Assistant);
    }
    // Action / tool-result step types: the generic `TOOL`, plus the concrete
    // action names Antigravity uses for tool *results* (VIEW_FILE,
    // LIST_DIRECTORY, GREP_SEARCH, RUN_COMMAND, CODE_ACTION, …). Errors are
    // tool-shaped too. The tool *names* come from `tool_calls[]` on the
    // preceding planner step, so these result steps do not count tools again.
    const TOOL_MARKERS: &[&str] = &[
        "TOOL",
        "ACTION",
        "FILE",
        "DIRECTORY",
        "SEARCH",
        "GREP",
        "VIEW",
        "LIST",
        "COMMAND",
        "RUN",
        "EDIT",
        "WRITE",
        "TERMINAL",
        "BROWSER",
        "PERMISSION",
        "ERROR",
    ];
    if TOOL_MARKERS.iter().any(|m| kind.contains(m)) {
        return Some(Role::Tool);
    }
    if kind.contains("RESPONSE") {
        return Some(Role::Assistant);
    }
    // Framing / bookkeeping steps (CONVERSATION_HISTORY, KNOWLEDGE_ARTIFACTS,
    // EPHEMERAL_MESSAGE, GENERIC, settings, …): keep as System so they're not
    // misread as assistant work, but still carry timestamps for duration.
    if kind.contains("SETTINGS")
        || kind.contains("SYSTEM")
        || kind.contains("META")
        || kind.contains("HISTORY")
        || kind.contains("KNOWLEDGE")
        || kind.contains("ARTIFACT")
        || kind.contains("EPHEMERAL")
        || kind.contains("MESSAGE")
        || kind == "GENERIC"
    {
        return Some(Role::System);
    }
    None
}

/// Push a `ToolCall` for each entry in a step's `tool_calls[]` array. Each entry
/// names the tool at `.name` (Antigravity) or `.function.name` (OpenAI-style),
/// or is a bare string.
fn push_tool_calls(obj: &serde_json::Map<String, Value>, ev: &mut NormalizedEvent) {
    let Some(calls) = obj.get("tool_calls").and_then(|c| c.as_array()) else {
        return;
    };
    for call in calls {
        let name = match call {
            Value::String(s) => Some(s.as_str()),
            _ => call
                .get("name")
                .or_else(|| call.get("function").and_then(|f| f.get("name")))
                .and_then(|n| n.as_str()),
        };
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            let input = call.get("args").or_else(|| call.get("input"));
            ev.tools.push(tool_call_from_input(name, input));
        }
    }
}

/// Pull a tool name from the first present of the likely field locations.
fn tool_name(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let direct = obj
        .get("toolName")
        .or_else(|| obj.get("tool_name"))
        .or_else(|| obj.get("name"))
        .and_then(|v| v.as_str());
    if let Some(name) = direct.filter(|n| !n.is_empty()) {
        return Some(name.to_string());
    }
    for container in ["toolCall", "tool", "action"] {
        if let Some(name) = obj
            .get(container)
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
        {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_for_classifies_known_step_types() {
        // Direct user input.
        assert_eq!(role_for("USER_INPUT"), Some(Role::User));

        // Planning and reasoning steps use the assistant role.
        assert_eq!(role_for("PLAN"), Some(Role::Assistant));
        assert_eq!(role_for("CHAIN_OF_THINKING"), Some(Role::Assistant));
        assert_eq!(role_for("REASONING"), Some(Role::Assistant));

        // Tool / action / result step types map to Tool.
        assert_eq!(role_for("TOOL"), Some(Role::Tool));
        assert_eq!(role_for("VIEW_FILE"), Some(Role::Tool));
        assert_eq!(role_for("RUN_COMMAND"), Some(Role::Tool));
        assert_eq!(role_for("GREP_SEARCH"), Some(Role::Tool));
        assert_eq!(role_for("ERROR"), Some(Role::Tool));

        // Plain assistant prose (not a tool or a plan).
        assert_eq!(role_for("MODEL_RESPONSE"), Some(Role::Assistant));

        // Framing / bookkeeping steps stay System.
        assert_eq!(role_for("GENERIC"), Some(Role::System));
        assert_eq!(role_for("CONVERSATION_HISTORY"), Some(Role::System));
        assert_eq!(role_for("EPHEMERAL_MESSAGE"), Some(Role::System));
        assert_eq!(role_for("KNOWLEDGE_ARTIFACTS"), Some(Role::System));

        // Unknown step types are left for the caller to decide.
        assert_eq!(role_for("FLOOP"), None);
    }
}
