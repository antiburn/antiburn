//! Shared JSON-record parsing used by the JSONL-family adapters and, for cells
//! that contain embedded JSON, by the generic SQLite adapter.
//!
//! Handles the Anthropic transcript shape (`message.content[]` with `text` /
//! `thinking` / `tool_use` parts, `message.usage`), the OpenAI shape (`role` +
//! string `content`, `tool_calls[].function.name`, `usage.prompt_tokens` /
//! `completion_tokens`), and Pi's generic JSONL shape (`toolCall` content blocks
//! with `arguments`, `toolResult` messages, and `input` / `output` /
//! `cacheRead` / `cacheWrite` usage keys). Unrecognized records are skipped.
//!
//! [`parse_record`] takes an explicit [`RecordShape`] naming the key locations its
//! caller's records actually use, so a bespoke adapter reads only its own vendor's
//! layout. [`RecordShape::Generic`] keeps the full historical fallback set for the
//! vendors without a bespoke adapter (see [`super::vendors::generic_jsonl`]).

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::analysis::interface::{
    ContentKind, ContentPart, ContextSourceKind, EvidenceObservation, RelationProvenance,
};
use crate::analysis::model::{
    CompactionTrigger, EventSource, NormalizedEvent, Role, ToolCall, Usage, is_subagent_launch_tool,
};

/// The record layout a caller expects. Each variant names the key locations that
/// carry the role, usage, model, effort, speed, timestamp, and thread identity for
/// that caller's records. [`parse_record`] reads only the locations its shape
/// names — no fallback to another shape's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordShape {
    /// Claude Code's transcript: role/usage/model/message-id under `message`;
    /// effort, timestamp, `isSidechain`, and thread identity (`uuid`/
    /// `parentUuid`) at the top level.
    Claude,
    /// Pi v3's transcript: role/usage/model always under `message`, with Pi's
    /// disjoint usage buckets (`input`/`output`/`cacheRead`/`cacheWrite`). The
    /// caller stamps its own timestamp and speed onto the event afterward, so
    /// this shape never reads either.
    Pi,
    /// Cursor's transcript: role is always top-level; content and model can be
    /// top-level or nested under `message`, depending on the source tier.
    /// Cursor never reports usage, effort, or thread identity.
    Cursor,
    /// Any vendor without a bespoke adapter: every key location `parse_record`
    /// has ever supported, tried together per record — the Anthropic, OpenAI,
    /// and Pi transcript conventions all at once.
    Generic,
}

/// Parse line-delimited JSON into normalized events, skipping blank/malformed
/// lines. Used only by the generic fallback adapter, so every record is parsed
/// with [`RecordShape::Generic`].
pub fn parse_jsonl(content: &str) -> Vec<NormalizedEvent> {
    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(ev) = parse_record(&value, RecordShape::Generic)
        {
            events.push(ev);
        }
    }
    events
}

/// Every evidence observation a full (non-inherited) Claude record carries:
/// [`context_observations`]'s context-source evidence, then
/// [`work_observations`]'s turn-attribution evidence. A skipped fork-replay
/// record calls [`context_observations`] alone — see `claude::visit_reader`.
pub(crate) fn evidence_observations(value: &Value) -> Vec<EvidenceObservation> {
    let mut observations = context_observations(value);
    observations.extend(work_observations(value));
    observations
}

/// The observations a record contributes to the fork's own context window,
/// even when the record itself is inherited (replayed) rather than the
/// fork's own work: named context sources (MCP servers, deferred tools) and
/// the harness version. A fork's first own request still carries an
/// inherited record's context contribution, so this half stays true for a
/// skipped record.
pub(crate) fn context_observations(value: &Value) -> Vec<EvidenceObservation> {
    let mut observations = Vec::new();
    if let Some(attachment) = value.get("attachment") {
        let attachment_type = attachment.get("type").and_then(Value::as_str);
        if attachment_type == Some("mcp_instructions_delta") {
            let names = attachment
                .get("addedNames")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            observations.extend(names.filter_map(|name| {
                let name = name.as_str()?.trim();
                if name.is_empty() {
                    return None;
                }
                // Only the server name is kept (#228, Option B). The
                // paired `addedBlocks` entry is the server's full injected
                // instruction text, not a description: persisting a prefix
                // of it stretched the "names and descriptions" evidence
                // invariant, and nothing downstream reads MCP descriptions
                // (the Unused MCP Servers detector only needs `invoked`).
                Some(EvidenceObservation::ContextSource {
                    kind: ContextSourceKind::McpServer,
                    name: name.to_owned(),
                    description: None,
                })
            }));
        } else if attachment_type == Some("deferred_tools_delta") {
            let names = attachment
                .get("addedNames")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            observations.extend(names.filter_map(|name| {
                let name = name.as_str()?.trim();
                if name.is_empty() {
                    return None;
                }
                Some(EvidenceObservation::DeferredTool {
                    name: name.to_owned(),
                })
            }));
        }
    }

    // The harness's own version, Claude's top-level `version` field. The
    // sink keeps only the first-seen value, mirroring
    // `initial_context::ClaudeContextAccumulator::observe`.
    if let Some(version) = value
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
    {
        observations.push(EvidenceObservation::HarnessVersion {
            version: version.to_owned(),
        });
    }
    observations
}

/// The observations that count a record as attributable turn work:
/// [`EvidenceObservation::DelegatedTurn`], this record's
/// [`thread_link_observation`], and any [`EvidenceObservation::SubagentSpawn`]
/// its content declares. An inherited (replayed) record is context, not
/// work, so `claude::visit_reader` skips this half for it — except the
/// thread link, which it emits directly so a later record's parent link
/// still resolves.
pub(crate) fn work_observations(value: &Value) -> Vec<EvidenceObservation> {
    let mut observations = Vec::new();
    let is_sidechain = value
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let message = value.get("message");
    let is_assistant = message
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("assistant");
    let delegated_model = (is_sidechain && is_assistant)
        .then(|| {
            message
                .and_then(|message| message.get("model"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty() && *model != "<synthetic>")
                .map(str::to_owned)
        })
        .flatten();
    observations.push(EvidenceObservation::DelegatedTurn {
        is_sidechain,
        is_assistant,
        model: delegated_model,
    });

    if let Some(link) = thread_link_observation(value) {
        observations.push(link);
    }

    let parent_model = value
        .get("message")
        .and_then(|message| message.get("model"))
        .or_else(|| value.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty() && *model != "<synthetic>")
        .map(str::to_owned);
    let ts_ms = value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .or_else(|| value.get("created_at"))
        .or_else(|| value.get("createdAt"))
        .and_then(parse_ts);
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"));
    if let Some(items) = content.and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("tool_use")
                && item
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(is_subagent_launch_tool)
            {
                observations.push(EvidenceObservation::SubagentSpawn {
                    ts_ms,
                    parent_model: parent_model.clone(),
                    provenance: RelationProvenance::TaskToolUse,
                });
            }
        }
    }
    observations
}

/// This record's [`EvidenceObservation::ThreadLink`] (Claude's top-level
/// `uuid` / `parentUuid`), when either field is present. `None` for a
/// record with neither.
///
/// Emitted for every record that carries either field, so the evidence
/// sink can verify parent links even through eventless records. Read
/// `parentUuid` only, with no fallback to `logicalParentUuid`: a
/// compaction boundary's logical parent can sit in another file or later
/// in this file, so it is not a link this source can verify, and the
/// sink checks only `parentUuid`.
pub(crate) fn thread_link_observation(value: &Value) -> Option<EvidenceObservation> {
    let uuid = thread_identity_field(value, "uuid");
    let parent_uuid = thread_identity_field(value, "parentUuid");
    (uuid.is_some() || parent_uuid.is_some())
        .then_some(EvidenceObservation::ThreadLink { uuid, parent_uuid })
}

/// A top-level thread-identity field (Claude's `uuid` / `parentUuid`, Pi's
/// `id` / `parentId`), when present and non-empty. A JSON `null` (a vendor's
/// explicit thread-root marker) reads as `None`, exactly like an absent
/// field.
pub(super) fn thread_identity_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Returns true for known eventless record names.
///
/// The Claude adapter permits unread nested scalar keys and command markers for these records.
/// Shallow evidence shapes still fail closed.
pub(super) fn is_recognized_eventless(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some(
            "attachment"
                | "summary"
                | "file-history-snapshot"
                // Harness housekeeping records: session bookkeeping lines that
                // carry no usage data (no tokens, models, tools, or timing).
                | "permission-mode"
                | "mode"
                | "last-prompt"
                | "ai-title"
                | "custom-title"
                | "bridge-session"
                | "artifact-comment-monitor"
                | "queue-operation"
                | "file-history-delta"
                | "pr-link"
                | "atis-latch"
                | "worktree-state"
                | "relocated"
                | "frame-link"
                | "cost-state"
                | "agent-name"
                | "history-suppression"
                | "artifact-autoreact-ledger"
        )
    )
}

/// Returns true when an unknown object cannot carry evidence that `parse_record` reads.
///
/// `INERTNESS_MIRROR_CASES` lists the parser readers and deliberate exemptions.
/// The scan rejects evidence keys and tool shapes at any depth.
/// The framing limit bounds the scan to one record of at most `MAX_RECORD_BYTES`.
/// A non-object record fails closed.
/// Timestamps and thread fields are inert because separate passes read them first.
/// `isSidechain`, attachments, message IDs, and free text also produce no skipped event evidence.
/// An empty tool name is inert because `push_named_tool_str` rejects it.
/// The Claude adapter checks command markers separately because they can create late tool calls.
/// This proof keeps complete coverage without weakening the clean-status evidence rule.
pub(super) fn is_inert_unrecognized(value: &Value) -> bool {
    is_inert_record(value, true)
}

/// Returns true when a known eventless record carries no parser-readable evidence.
///
/// Known eventless records can contain nested configuration with scalar evidence-key names.
/// `parse_record` reads those scalar keys only at the root or in the root `message` object.
pub(super) fn is_inert_recognized_eventless(value: &Value) -> bool {
    is_inert_record(value, false)
}

fn is_inert_record(value: &Value, reject_nested_scalar_keys: bool) -> bool {
    if !value.is_object() {
        return false;
    }

    let mut pending = vec![(value, true)];
    while let Some((value, reads_scalar_keys)) = pending.pop() {
        match value {
            Value::Object(object) => {
                if (reads_scalar_keys
                    && [
                        "role",
                        "usage",
                        "model",
                        "speed",
                        "effort",
                        "reasoning_effort",
                    ]
                    .into_iter()
                    .any(|key| object.contains_key(key)))
                    || object.contains_key("tool_calls")
                    || object.contains_key("compactMetadata")
                    || object.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
                    || object
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| {
                            matches!(kind, "tool_use" | "toolCall" | "tool_result" | "thinking")
                        })
                {
                    return false;
                }

                let has_arguments =
                    object.contains_key("input") || object.contains_key("arguments");
                let has_named_tool = object
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| !name.is_empty())
                    && has_arguments;
                let has_split_named_tool = has_arguments
                    && object.values().any(|child| {
                        child
                            .as_object()
                            .and_then(|child| child.get("name"))
                            .and_then(Value::as_str)
                            .is_some_and(|name| !name.is_empty())
                    });
                if has_named_tool || has_split_named_tool {
                    return false;
                }

                pending.extend(object.iter().map(|(key, child)| {
                    let reads_scalar_keys =
                        reject_nested_scalar_keys || (reads_scalar_keys && key == "message");
                    (child, reads_scalar_keys)
                }));
            }
            Value::Array(items) => {
                pending.extend(items.iter().map(|item| (item, reject_nested_scalar_keys)))
            }
            _ => {}
        }
    }

    true
}

#[cfg(test)]
const INERTNESS_MIRROR_CASES: &[(&str, bool)] = &[
    (r#"{"type":"new","role":"agent"}"#, false),
    (r#"{"type":"new","message":{"role":"agent"}}"#, false),
    (r#"{"type":"new","usage":{}}"#, false),
    (r#"{"type":"new","message":{"usage":null}}"#, false),
    (r#"{"type":"new","payload":{"usage":{}}}"#, false),
    (r#"{"type":"new","model":"m"}"#, false),
    (r#"{"type":"new","speed":"fast"}"#, false),
    (r#"{"type":"new","effort":"high"}"#, false),
    (r#"{"type":"new","reasoning_effort":"high"}"#, false),
    (r#"{"type":"new","tool_calls":[]}"#, false),
    (r#"{"type":"new","content":[{"type":"tool_use"}]}"#, false),
    (r#"{"type":"new","content":[{"type":"toolCall"}]}"#, false),
    (
        r#"{"type":"new","content":[{"type":"tool_result"}]}"#,
        false,
    ),
    (r#"{"type":"new","content":[{"type":"thinking"}]}"#, false),
    (r#"{"type":"new","compactMetadata":{}}"#, false),
    (r#"{"type":"new","subtype":"compact_boundary"}"#, false),
    (r#"{"type":"new","name":"Bash","input":{}}"#, false),
    (
        r#"{"type":"new","payload":{"name":"Bash"},"arguments":{}}"#,
        false,
    ),
    (
        r#"{"type":"new","timestamp":1,"ts":1,"created_at":1,"createdAt":1}"#,
        true,
    ),
    (r#"{"type":"new","uuid":"u","parentUuid":"p"}"#, true),
    (r#"{"type":"new","isSidechain":true}"#, true),
    (r#"{"type":"new","attachment":{"name":"guide"}}"#, true),
    (r#"{"type":"new","message":{"id":"m"}}"#, true),
    (r#"{"type":"new","text":"free","content":"free"}"#, true),
    (r#"{"type":"new","name":"","input":{}}"#, true),
];

pub(super) fn record_discriminator(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
        .to_owned()
}

/// Extract this record's message content as [`ContentPart`]s, for the
/// `turn_content` capture. Reads the same `message.content[]` / top-level
/// `content` shape [`parse_record`] reads, but never retains it on
/// `NormalizedEvent` — the caller emits the result as a separate
/// `TurnContent` record, right after the record's `MetricsEvent`.
///
/// `role` is the event's *resolved* role (after the Claude tool-result
/// reclassification from `User` to `Tool`), so a `tool_result` block is
/// captured as `ToolResult` even though the record's JSON role is `user`.
pub(super) fn extract_content_parts(value: &Value, role: Role) -> Vec<ContentPart> {
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    extract_content_parts_from_container(obj, role)
}

/// Like [`extract_content_parts`], reading a `message`-shaped JSON object
/// directly rather than a whole transcript record. Lets a caller that only
/// has the inner object (Codex's `payload`) skip re-wrapping it into a full
/// record [`Value`].
pub(super) fn extract_content_parts_from_container(
    container: &serde_json::Map<String, Value>,
    role: Role,
) -> Vec<ContentPart> {
    let content = container
        .get("message")
        .and_then(Value::as_object)
        .and_then(|m| m.get("content"))
        .or_else(|| container.get("content"));
    if role == Role::Tool {
        return tool_result_parts(content);
    }
    let mut parts = Vec::new();
    match content {
        Some(Value::String(text)) => push_text(text, role, &mut parts),
        Some(Value::Array(items)) => {
            for item in items {
                push_content_block(item, role, &mut parts);
            }
        }
        _ => {}
    }
    parts
}

fn text_kind(role: Role) -> ContentKind {
    if role == Role::Assistant {
        ContentKind::AssistantText
    } else {
        ContentKind::UserText
    }
}

fn push_text(text: &str, role: Role, parts: &mut Vec<ContentPart>) {
    if !text.is_empty() {
        parts.push(ContentPart::new(text_kind(role), text));
    }
}

fn push_content_block(item: &Value, role: Role, parts: &mut Vec<ContentPart>) {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        // "input_text" / "output_text" are Codex's (OpenAI-shaped) content
        // block types, captured through this same helper.
        "text" | "input_text" | "output_text" => {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                push_text(text, role, parts);
            }
        }
        "thinking" => {
            if let Some(text) = item
                .get("thinking")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                parts.push(ContentPart::new(ContentKind::Thinking, text));
            }
        }
        "tool_use" | "toolCall" => {
            let input = item.get("input").or_else(|| item.get("arguments"));
            if let Some(text) = input.and_then(compact_json_text) {
                parts.push(ContentPart::new(ContentKind::ToolInput, text));
            }
        }
        "tool_result" | "toolResult" | "function_call_output" => {
            if let Some(text) = tool_result_text(item) {
                parts.push(ContentPart::new(ContentKind::ToolResult, text));
            }
        }
        _ => {}
    }
}

/// A tool-result-shaped item's own content: a plain string, or the
/// concatenated text of a content-block array (non-text parts skipped).
fn tool_result_text(item: &Value) -> Option<String> {
    concatenated_text(item.get("content"))
}

/// A content value's text: a plain string, or the concatenated `text` field
/// of each item in a content-block array (non-text items skipped).
pub(super) fn concatenated_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => (!text.is_empty()).then(|| text.clone()),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

/// A tool-role record's whole content as one `ToolResult` part: the
/// top-level content string or array (Pi's `toolResult` message shape), or
/// the nested tool-result items within a mixed array (Claude's `tool_result`
/// content block, embedded in an otherwise `user`-shaped message). Non-text
/// items are skipped.
fn tool_result_parts(content: Option<&Value>) -> Vec<ContentPart> {
    let text = match content {
        Some(Value::Array(items)) => {
            let mut out = String::new();
            for item in items {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                let text = if matches!(
                    item_type,
                    "tool_result" | "toolResult" | "function_call_output"
                ) {
                    tool_result_text(item)
                } else {
                    item.get("text").and_then(Value::as_str).map(str::to_owned)
                };
                if let Some(text) = text {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&text);
                }
            }
            (!out.is_empty()).then_some(out)
        }
        _ => concatenated_text(content),
    };
    text.into_iter()
        .map(|text| ContentPart::new(ContentKind::ToolResult, text))
        .collect()
}

/// A tool call's input, JSON-serialized compactly. A JSON-encoded string
/// (OpenAI-style `function.arguments`) is parsed and re-serialized so the
/// stored text is canonical JSON either way; a non-JSON string (Codex's raw
/// `exec` script) is kept as-is.
pub(super) fn compact_json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(parsed) => serde_json::to_string(&parsed).ok(),
            Err(_) => Some(raw.clone()),
        },
        other => serde_json::to_string(other).ok(),
    }
}

/// Parse a single JSON record into a normalized event, or `None` if the record
/// carries no analyzable signal (titles, summaries, metadata lines, …).
///
/// `shape` names the key locations this call site's records actually use; see
/// [`RecordShape`]. For [`RecordShape::Generic`], every read here is a rejected
/// key in `is_inert_unrecognized` or an exempt row in `INERTNESS_MIRROR_CASES`
/// (that table only has to hold for the generic fallback, since it alone reads
/// every location). Add one of the two before you update the fingerprint.
pub(crate) fn parse_record(value: &Value, shape: RecordShape) -> Option<NormalizedEvent> {
    let obj = value.as_object()?;
    let msg = obj.get("message").and_then(|m| m.as_object());
    let top_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");

    let role = resolve_role(msg, obj, top_type, shape)?;
    let mut ev = NormalizedEvent::new(role);
    if matches!(shape, RecordShape::Claude | RecordShape::Generic)
        && obj
            .get("isSidechain")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        ev.source = EventSource::Subagent;
    }

    // The record's timestamp. Claude and Cursor always write "timestamp";
    // Pi's caller stamps its own timestamp onto the event afterward, so this
    // shape never reads one. Generic tries every spelling any vendor has used.
    let ts_value = match shape {
        RecordShape::Claude | RecordShape::Cursor => obj.get("timestamp"),
        RecordShape::Pi => None,
        RecordShape::Generic => obj
            .get("timestamp")
            .or_else(|| obj.get("ts"))
            .or_else(|| obj.get("created_at"))
            .or_else(|| obj.get("createdAt")),
    };
    ev.ts_ms = ts_value.and_then(parse_ts);

    // Usage lives under message.usage for Claude and Pi; Cursor never reports
    // it. Generic tries message.usage, then top-level usage (OpenAI's
    // location).
    let usage_value = match shape {
        RecordShape::Claude | RecordShape::Pi => msg.and_then(|m| m.get("usage")),
        RecordShape::Cursor => None,
        RecordShape::Generic => msg
            .and_then(|m| m.get("usage"))
            .or_else(|| obj.get("usage")),
    };
    ev.usage = parse_usage(usage_value);

    // The response speed (Claude's "standard"/"fast" fast-mode signal): only
    // Claude and the generic fallback ever carry it, as message.usage.speed
    // or a bare top-level speed field.
    ev.speed = match shape {
        RecordShape::Claude | RecordShape::Generic => usage_value
            .and_then(|u| u.get("speed"))
            .or_else(|| obj.get("speed")),
        RecordShape::Pi | RecordShape::Cursor => None,
    }
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|speed| !speed.is_empty())
    .map(str::to_string);

    // Model that produced this turn, when recorded. Claude and Pi always nest
    // it under message.model; Cursor's synthesized records place it top-level
    // (its own source tiers disagree, so both locations are tried) and
    // generic mirrors OpenAI's top-level location too. `<synthetic>` is
    // Claude's sentinel for injected, unbilled turns — skip it so it never
    // becomes a pricing key.
    let model_value = match shape {
        RecordShape::Claude | RecordShape::Pi => msg.and_then(|m| m.get("model")),
        RecordShape::Cursor | RecordShape::Generic => msg
            .and_then(|m| m.get("model"))
            .or_else(|| obj.get("model")),
    };
    ev.model = model_value
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty() && *m != "<synthetic>")
        .map(str::to_string);

    // The reasoning-effort tier: only Claude and the generic fallback carry
    // it. Claude always writes it top-level; generic also tries
    // message.effort for a vendor that nests it.
    ev.thinking_mode = match shape {
        RecordShape::Claude => obj.get("effort").or_else(|| obj.get("reasoning_effort")),
        RecordShape::Generic => obj
            .get("effort")
            .or_else(|| obj.get("reasoning_effort"))
            .or_else(|| msg.and_then(|m| m.get("effort")))
            .or_else(|| msg.and_then(|m| m.get("reasoning_effort"))),
        RecordShape::Pi | RecordShape::Cursor => None,
    }
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|mode| !mode.is_empty())
    .map(str::to_string);

    // Per-record thread identity (Claude's top-level `uuid` / `parentUuid`).
    // `parentUuid: null` marks a thread root and stays `None`.
    if matches!(shape, RecordShape::Claude | RecordShape::Generic) {
        ev.uuid = thread_identity_field(value, "uuid");
        ev.parent_uuid = thread_identity_field(value, "parentUuid");
    }

    // Pi's own per-record thread identity (`id` / `parentId`). `parentId:
    // null` marks a thread root and stays `None`. Pi never writes
    // `logicalParentUuid` or nests an id under `message`.
    if shape == RecordShape::Pi {
        ev.uuid = thread_identity_field(value, "id");
        ev.parent_uuid = thread_identity_field(value, "parentId");
    }

    // The link across a compaction boundary (Claude's `logicalParentUuid`),
    // read only for Claude's own shape: no other vendor writes this key.
    if shape == RecordShape::Claude {
        ev.logical_parent_uuid = thread_identity_field(value, "logicalParentUuid");
    }

    // Provider message id (Anthropic `message.id`), used by the Claude adapter to
    // de-duplicate re-logged copies of the same assistant message.
    if matches!(shape, RecordShape::Claude | RecordShape::Generic) {
        ev.message_id = msg
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
    }

    // Claude marks a compaction boundary with a top-level system record, and
    // (on most records) names the trigger and before/after size in
    // compactMetadata.
    if matches!(shape, RecordShape::Claude | RecordShape::Generic)
        && top_type == "system"
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

    // Standalone, top-level tool-shaped records — a generic-vendor
    // convention; Claude, Pi, and Cursor never write a bare tool record.
    if shape == RecordShape::Generic {
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
    }

    // Content block: Claude and Cursor accept it nested under message.content
    // or top-level (Claude's compact-boundary system records, Cursor's
    // flat-JSONL tier); Pi always nests it. Generic tries both locations.
    let content = match shape {
        RecordShape::Pi => msg.and_then(|m| m.get("content")),
        RecordShape::Claude | RecordShape::Cursor | RecordShape::Generic => msg
            .and_then(|m| m.get("content"))
            .or_else(|| obj.get("content")),
    };
    process_content(content, &mut ev);
    // Claude Code writes a tool result as a `user` record whose content is a
    // `tool_result` block. That is the tool's turn, not a user prompt.
    if matches!(shape, RecordShape::Claude | RecordShape::Generic)
        && ev.role == Role::User
        && has_tool_result_block(content)
    {
        ev.role = Role::Tool;
    }

    // OpenAI-style tool calls: Cursor's flat-JSONL tier and the generic
    // fallback are the only shapes that carry them.
    if matches!(shape, RecordShape::Cursor | RecordShape::Generic) {
        let calls = if shape == RecordShape::Cursor {
            obj.get("tool_calls")
        } else {
            msg.and_then(|m| m.get("tool_calls"))
                .or_else(|| obj.get("tool_calls"))
        };
        if let Some(calls) = calls.and_then(|c| c.as_array()) {
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
    }

    Some(ev)
}

fn resolve_role(
    msg: Option<&serde_json::Map<String, Value>>,
    obj: &serde_json::Map<String, Value>,
    top_type: &str,
    shape: RecordShape,
) -> Option<Role> {
    match shape {
        RecordShape::Claude => {
            role_from_str(msg.and_then(|m| m.get("role")).and_then(Value::as_str))
                .or_else(|| role_from_claude_type(top_type))
        }
        RecordShape::Pi => role_from_str(msg.and_then(|m| m.get("role")).and_then(Value::as_str)),
        RecordShape::Cursor => role_from_str(obj.get("role").and_then(Value::as_str)),
        RecordShape::Generic => {
            let role_str = msg
                .and_then(|m| m.get("role"))
                .or_else(|| obj.get("role"))
                .and_then(|r| r.as_str());
            role_from_str(role_str).or_else(|| role_from_generic_type(top_type))
        }
    }
}

fn role_from_str(role: Option<&str>) -> Option<Role> {
    match role? {
        "assistant" => Some(Role::Assistant),
        "user" => Some(Role::User),
        "system" => Some(Role::System),
        "tool" | "toolResult" => Some(Role::Tool),
        _ => None,
    }
}

/// Claude's top-level `type` also names the role for records with no
/// `message` wrapper (e.g. a `system` compact-boundary record).
fn role_from_claude_type(top_type: &str) -> Option<Role> {
    match top_type {
        "assistant" => Some(Role::Assistant),
        "user" => Some(Role::User),
        "system" => Some(Role::System),
        _ => None,
    }
}

/// A generic-vendor convention: some non-bespoke vendors write a bare
/// top-level tool record (`{"type":"tool_use",...}` or the OpenAI-flavored
/// `function_call` / `function_call_output`) with no `role` field at all.
fn role_from_generic_type(top_type: &str) -> Option<Role> {
    match top_type {
        "assistant" => Some(Role::Assistant),
        "user" => Some(Role::User),
        "system" => Some(Role::System),
        "tool_use" | "function_call" => Some(Role::Assistant),
        "tool_result" | "toolResult" | "function_call_output" => Some(Role::Tool),
        _ => None,
    }
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
    use crate::analysis::model::ToolCategory;
    use serde_json::json;

    #[test]
    fn every_field_parse_record_reads_appears_in_the_inertness_table() {
        for (record, expected_inert) in INERTNESS_MIRROR_CASES {
            let value: Value = serde_json::from_str(record).unwrap();
            assert_eq!(
                is_inert_unrecognized(&value),
                *expected_inert,
                "unexpected classification for {record}"
            );
        }
    }

    #[test]
    fn parse_record_changes_require_an_inertness_review() {
        // Seam 4f: `parse_record` now reads Pi's own `id` / `parentId` pair
        // into `ev.uuid` / `ev.parent_uuid` for `RecordShape::Pi`, mirroring
        // Claude's `uuid` / `parentUuid` read. This is an addition, not a
        // change to any inertness-reviewed key, so `INERTNESS_MIRROR_CASES`
        // needs no update.
        const EXPECTED_FINGERPRINT: u64 = 12_239_640_525_636_906_098;
        let source = include_str!("records.rs").replace("\r\n", "\n");
        let start = source.find("pub(crate) fn parse_record").unwrap();
        let end = source[start..].find("\n#[cfg(test)]\nmod tests").unwrap() + start;
        let fingerprint = source.as_bytes()[start..end]
            .iter()
            .fold(0xcbf29ce484222325_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });

        assert_eq!(fingerprint, EXPECTED_FINGERPRINT);
    }

    #[test]
    fn a_bare_housekeeping_record_is_inert() {
        assert!(is_inert_unrecognized(&json!({
            "type": "telemetry_ping",
            "timestamp": 1,
            "payload": {"ok": true}
        })));
    }

    #[test]
    fn roles_fail_closed() {
        for record in [
            json!({"type": "new", "role": "agent"}),
            json!({"type": "new", "message": {"role": "agent"}}),
            json!({"type": "new", "role": 7}),
            json!({"type": "new", "role": null}),
            json!({"type": "new", "role": {}}),
        ] {
            assert!(!is_inert_unrecognized(&record));
        }
    }

    #[test]
    fn non_object_records_fail_closed() {
        for record in [json!([]), json!(7), json!("text"), Value::Null] {
            assert!(!is_inert_unrecognized(&record));
        }
    }

    #[test]
    fn usage_model_speed_or_effort_fails_closed() {
        for record in [
            json!({"type": "new", "usage": {}}),
            json!({"type": "new", "usage": null}),
            json!({"type": "new", "message": {"usage": {"speed": "fast"}}}),
            json!({"type": "new", "model": "m"}),
            json!({"type": "new", "message": {"model": "m"}}),
            json!({"type": "new", "speed": "fast"}),
            json!({"type": "new", "effort": "high"}),
            json!({"type": "new", "message": {"reasoning_effort": "high"}}),
        ] {
            assert!(!is_inert_unrecognized(&record));
        }
    }

    #[test]
    fn tool_calls_and_content_blocks_fail_closed() {
        for record in [
            json!({"type": "new", "tool_calls": []}),
            json!({"type": "new", "message": {"tool_calls": []}}),
            json!({"type": "new", "content": [{"type": "tool_use"}]}),
            json!({"type": "new", "message": {"content": [{"nested": {"type": "toolCall"}}]}}),
            json!({"type": "new", "payload": {"type": "tool_result"}}),
            json!({"type": "new", "payload": [{"type": "thinking"}]}),
        ] {
            assert!(!is_inert_unrecognized(&record));
        }
    }

    #[test]
    fn named_tool_shapes_fail_closed() {
        for record in [
            json!({"type": "new", "name": "Bash", "input": {}}),
            json!({"type": "new", "message": {"name": "Bash", "arguments": {}}}),
            json!({"type": "new", "payload": {"name": "Bash", "input": {}}}),
            json!({"type": "new", "payload": {"name": "Bash"}, "arguments": {}}),
        ] {
            assert!(!is_inert_unrecognized(&record));
        }
        assert!(is_inert_unrecognized(
            &json!({"type": "new", "name": "", "input": {}})
        ));
    }

    #[test]
    fn wrong_message_or_content_types_are_inert() {
        for record in [
            json!({"type": "new", "message": []}),
            json!({"type": "new", "message": "text"}),
            json!({"type": "new", "content": {"text": "free"}}),
            json!({"type": "new", "content": "free"}),
        ] {
            assert!(is_inert_unrecognized(&record));
        }
    }

    #[test]
    fn compaction_metadata_fails_closed() {
        assert!(!is_inert_unrecognized(
            &json!({"type": "new", "compactMetadata": {}})
        ));
        assert!(!is_inert_unrecognized(
            &json!({"type": "new", "subtype": "compact_boundary"})
        ));
    }

    #[test]
    fn allowlisted_names_use_the_recognized_eventless_predicate() {
        for kind in [
            "attachment",
            "summary",
            "file-history-snapshot",
            "permission-mode",
            "mode",
            "last-prompt",
            "ai-title",
            "queue-operation",
            "file-history-delta",
            "pr-link",
            "atis-latch",
            "worktree-state",
            "relocated",
            "frame-link",
            "cost-state",
            "agent-name",
            "history-suppression",
            "artifact-autoreact-ledger",
        ] {
            let inert = json!({"type": kind, "timestamp": 1});
            assert!(is_recognized_eventless(&inert));
            assert!(is_inert_recognized_eventless(&inert));
            assert!(!is_inert_recognized_eventless(
                &json!({"type": kind, "message": {"usage": {}}})
            ));
        }
    }

    #[test]
    fn recognized_eventless_records_fail_closed_on_every_parser_readable_shape() {
        for key in [
            "role",
            "usage",
            "model",
            "speed",
            "effort",
            "reasoning_effort",
        ] {
            let mut root = json!({"type": "cost-state"});
            root.as_object_mut()
                .unwrap()
                .insert(key.to_owned(), Value::Null);
            assert!(!is_inert_recognized_eventless(&root), "root {key}");

            let mut message = json!({"type": "cost-state", "message": {}});
            message["message"]
                .as_object_mut()
                .unwrap()
                .insert(key.to_owned(), Value::Null);
            assert!(!is_inert_recognized_eventless(&message), "message {key}");
        }

        for record in [
            json!({"type": "cost-state", "payload": {"tool_calls": []}}),
            json!({"type": "cost-state", "payload": {"content": [{"type": "tool_use"}]}}),
            json!({"type": "cost-state", "payload": {"content": [{"type": "toolCall"}]}}),
            json!({"type": "cost-state", "payload": {"content": [{"type": "tool_result"}]}}),
            json!({"type": "cost-state", "payload": {"content": [{"type": "thinking"}]}}),
            json!({"type": "cost-state", "payload": {"compactMetadata": {}}}),
            json!({"type": "cost-state", "payload": {"subtype": "compact_boundary"}}),
            json!({"type": "cost-state", "payload": {"name": "Bash", "input": {}}}),
            json!({
                "type": "cost-state",
                "payload": {"arguments": {}, "call": {"name": "Bash"}}
            }),
        ] {
            assert!(!is_inert_recognized_eventless(&record), "{record}");
        }

        for record in [
            json!({"type": "attachment", "attachment": {"config": {"model": "display-only"}}}),
            json!({"type": "last-prompt", "prompt": "<command-name>/review</command-name>"}),
        ] {
            assert!(is_inert_recognized_eventless(&record), "{record}");
        }
    }

    #[test]
    fn large_and_deep_allowlisted_records_stay_inert() {
        let large = json!({
            "type": "file-history-snapshot",
            "files": (0..500).map(|index| json!({"path": index})).collect::<Vec<_>>()
        });
        let deep = json!({
            "type": "summary",
            "payload": {"a": {"b": {"c": {"d": {"e": {"f": {"g": {"h": {}}}}}}}}}
        });

        assert!(is_inert_unrecognized(&large));
        assert!(is_inert_unrecognized(&deep));
    }

    #[test]
    fn nested_evidence_keys_fail_closed() {
        for record in [
            json!({"type": "new", "payload": {"role": "agent"}}),
            json!({"type": "new", "payload": {"usage": {}}}),
            json!({"type": "new", "payload": {"model": "m"}}),
        ] {
            assert!(!is_inert_unrecognized(&record));
        }
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
        let ev = parse_record(&record, RecordShape::Generic).expect("record should parse");
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
        let ev = parse_record(&record, RecordShape::Generic).expect("record should parse");
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
    fn tool_call_from_input_leaves_non_skill_detail_none_and_keeps_command_class() {
        // A non-skill tool never gets `detail`, even when its input has a skill key.
        let read = tool_call_from_input("Read", Some(&json!({"skill": "x"})));
        assert_eq!(read.detail, None);
        // Bash→Test reclassification from the command still flows through the builder.
        let test = tool_call_from_input("Bash", Some(&json!({"command": "cargo test"})));
        assert_eq!(test.category, ToolCategory::Test);
        assert_eq!(test.detail, None);
    }

    fn claude_assistant_record_with_tool(tool_name: &str) -> Value {
        json!({
            "type": "assistant",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": {
                "role": "assistant",
                "model": "claude-opus-4-6",
                "content": [
                    {"type": "tool_use", "name": tool_name, "input": {}}
                ]
            }
        })
    }

    #[test]
    fn evidence_observations_emits_subagent_spawn_for_an_agent_tool_use() {
        let record = claude_assistant_record_with_tool("Agent");
        let observations = evidence_observations(&record);
        assert!(observations.iter().any(|observation| matches!(
            observation,
            EvidenceObservation::SubagentSpawn {
                provenance: RelationProvenance::TaskToolUse,
                ..
            }
        )));
    }

    #[test]
    fn evidence_observations_emits_subagent_spawn_for_a_lowercase_agent_tool_use() {
        let record = claude_assistant_record_with_tool("agent");
        let observations = evidence_observations(&record);
        assert!(observations.iter().any(|observation| matches!(
            observation,
            EvidenceObservation::SubagentSpawn {
                provenance: RelationProvenance::TaskToolUse,
                ..
            }
        )));
    }

    #[test]
    fn evidence_observations_does_not_emit_subagent_spawn_for_an_unrelated_tool_use() {
        let record = claude_assistant_record_with_tool("Read");
        let observations = evidence_observations(&record);
        assert!(
            !observations.iter().any(|observation| matches!(
                observation,
                EvidenceObservation::SubagentSpawn { .. }
            ))
        );
    }

    /// A record's context contribution ([`context_observations`]) never
    /// carries the turn-attribution half's variants — the split
    /// `claude::visit_reader` relies on to skip a fork's inherited
    /// records without also skipping their context.
    #[test]
    fn context_observations_excludes_thread_link_and_delegated_turn() {
        let record = json!({
            "type": "user",
            "uuid": "u1",
            "parentUuid": "u0",
            "version": "1.0.0",
            "isSidechain": true,
            "message": {"role": "assistant", "model": "claude-opus-4-6"},
        });
        let observations = context_observations(&record);
        assert!(observations.iter().any(|observation| matches!(
            observation,
            EvidenceObservation::HarnessVersion { version } if version == "1.0.0"
        )));
        assert!(
            !observations
                .iter()
                .any(|observation| matches!(observation, EvidenceObservation::ThreadLink { .. }))
        );
        assert!(
            !observations.iter().any(|observation| matches!(
                observation,
                EvidenceObservation::DelegatedTurn { .. }
            ))
        );
    }

    #[test]
    fn evidence_observations_is_context_observations_then_work_observations() {
        let record = claude_assistant_record_with_tool("Agent");
        let mut expected = context_observations(&record);
        expected.extend(work_observations(&record));
        assert_eq!(evidence_observations(&record), expected);
    }

    #[test]
    fn thread_link_observation_is_none_without_uuid_or_parent_uuid() {
        let record = json!({"type": "user"});
        assert_eq!(thread_link_observation(&record), None);
    }

    #[test]
    fn thread_link_observation_carries_uuid_and_parent_uuid() {
        let record = json!({"type": "assistant", "uuid": "u2", "parentUuid": "u1"});
        assert_eq!(
            thread_link_observation(&record),
            Some(EvidenceObservation::ThreadLink {
                uuid: Some("u2".to_owned()),
                parent_uuid: Some("u1".to_owned()),
            })
        );
    }
}
