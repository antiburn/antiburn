//! Session scanning and metadata extraction.
//!
//! Parses session logs to extract lightweight metadata and timestamps
//! without loading unnecessary structure into the ingest pipeline.

use super::Explorers;
use std::io::{BufRead, BufReader, Cursor};
use std::path::Path;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The agent identity discovery works in. Re-exported so scanner callers keep
/// one import for "metadata plus the agent it came from"; the canonical
/// definition lives in [`crate::model`].
pub use crate::model::AgentKind;

/// Minimal metadata parsed from a session log file.
///
/// Not every field is guaranteed to be present -- session logs vary
/// in structure. Fields are `Option` to handle missing data gracefully.
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    /// The session ID (UUID or similar identifier).
    pub session_id: Option<String>,
    /// The working directory where the agent was running.
    pub cwd: Option<String>,
    /// The agent type, if determinable from the log content.
    pub agent_type: Option<AgentKind>,
    // Diverged for Desktop: title surfaced for the recent activity list.
    /// Human-readable session title. Sourced from explicit fields
    /// (`summary` / `title` / `name`) when present, else the first user
    /// message in the transcript.
    pub title: Option<String>,
    /// Provenance of `title`. `Some` whenever `title` is `Some`. Used by
    /// the rename-detection log so AI-title refinements don't masquerade
    /// as user renames.
    pub title_source: Option<TitleSource>,
}

/// Provenance of a resolved session title. Lets the consumer distinguish a
/// real user rename from an AI re-summarisation or a first-message fallback,
/// without re-parsing the transcript. Agents whose stores cannot distinguish
/// these (e.g. OpenCode's single `title` column) default to `Explicit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSource {
    /// User explicitly renamed the session (Claude Tier-5 `custom-title`
    /// record, Codex SQLite `threads.name`).
    UserRename,
    /// AI/agent generated the title (Claude Tier-4 `ai-title` record,
    /// Tier-3 `summary` event, Codex `session_index.jsonl` `thread_name`).
    AiGenerated,
    /// Explicit top-level field in the transcript (`title` / `name`) or a
    /// single-column store with no rename concept (OpenCode).
    Explicit,
    /// Derived from the first user message in the transcript (fallback
    /// when no explicit field is present).
    FirstMessage,
}

// Diverged for Desktop: priority of the source from which a title was set,
// so a later, higher-priority record (e.g. a `summary` event after the first
// user message) can replace an earlier guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
enum TitleTier {
    #[default]
    None,
    FirstUserMessage,
    ExplicitField,
    Summary,
    // Diverged for Desktop: Claude Code now persists an LLM-generated session
    // label as `{type:"ai-title", aiTitle:"..."}` records. These are short,
    // semantically meaningful, and may be refined multiple times over the
    // course of a session — the latest one wins.
    AiTitle,
    // Diverged for Desktop: Claude Code persists user-initiated renames as
    // `{type:"custom-title", customTitle:"..."}` records. A manual rename
    // beats any AI-generated label; latest wins.
    CustomTitle,
}

impl TitleTier {
    fn as_source(self) -> Option<TitleSource> {
        match self {
            TitleTier::None => None,
            TitleTier::FirstUserMessage => Some(TitleSource::FirstMessage),
            TitleTier::ExplicitField => Some(TitleSource::Explicit),
            // Claude's `summary` event is written by the agent's own
            // summarisation step, not by the user — treat it as AI-generated
            // so rename-filtering at the consumer layer doesn't conflate it
            // with user-authored labels.
            TitleTier::Summary => Some(TitleSource::AiGenerated),
            TitleTier::AiTitle => Some(TitleSource::AiGenerated),
            TitleTier::CustomTitle => Some(TitleSource::UserRename),
        }
    }
}

impl TitleSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserRename => "userRename",
            Self::AiGenerated => "aiGenerated",
            Self::Explicit => "explicit",
            Self::FirstMessage => "firstMessage",
        }
    }
}

pub(super) const TITLE_MAX_CHARS: usize = 200;

// The Claude-flavored hyphenated-path decoder lives in `agents::path_codec`.
// Re-exported here so `scanner::…` stays the single import for callers that
// resolve a Claude project directory (notably `claude::session_title`).
pub use super::agents::path_codec::decode_hyphenated_absolute_path;

/// Parse minimal metadata from a session log file.
///
/// Reads the file line-by-line, attempting to parse each line as JSON
/// and extracting known fields:
/// - `session_id` (or `sessionId`)
/// - `cwd` (or `workdir`, `working_directory`)
/// - `type` field that may indicate the agent
///
/// This is best-effort: not every line will be valid JSON, and not
/// every JSON line will contain the fields we need. The function
/// accumulates fields across all lines. `session_id` is first-value-wins,
/// while `cwd` prefers the most recent direct cwd/workdir field so later
/// worktree handoff records can override stale earlier values.
pub async fn parse_session_metadata(file: &Path) -> SessionMetadata {
    parse_session_metadata_with(Explorers::DISK, file).await
}

/// [`parse_session_metadata`] against a specific adapter set, used when the
/// agent must be inferred and the caller has configured adapters.
pub async fn parse_session_metadata_with(explorers: Explorers, file: &Path) -> SessionMetadata {
    let content = match tokio::fs::read_to_string(file).await {
        Ok(c) => c,
        Err(error) => {
            ::tracing::debug!(
                event = "session_metadata_unreadable",
                path = %file.display(),
                error = %error
            );
            return SessionMetadata::default();
        }
    };
    parse_session_metadata_with_content_for(explorers, file, &content).await
}

pub async fn parse_session_metadata_with_content(file: &Path, content: &str) -> SessionMetadata {
    parse_session_metadata_with_content_for(Explorers::DISK, file, content).await
}

/// [`parse_session_metadata_with_content`] against a specific adapter set.
pub async fn parse_session_metadata_with_content_for(
    explorers: Explorers,
    file: &Path,
    content: &str,
) -> SessionMetadata {
    parse_session_metadata_with_agent_for(explorers, file, content, None).await
}

/// Variant of [`parse_session_metadata_with_content`] that accepts a caller-
/// supplied `agent_type` hint. When `Some`, the scanner skips the
/// `infer_agent_type` substring cascade and uses the hint directly — for
/// callers (the daemon hot path, backfill) that already have the agent type
/// on a `SessionLog` upstream and would otherwise pay for inference only to
/// overwrite the result. When `None`, behavior is identical to the wrapper.
pub async fn parse_session_metadata_with_agent(
    file: &Path,
    content: &str,
    agent_type: Option<AgentKind>,
) -> SessionMetadata {
    parse_session_metadata_with_agent_for(Explorers::DISK, file, content, agent_type).await
}

/// [`parse_session_metadata_with_agent`] against a specific adapter set. The
/// adapter set is consulted only for agent inference (when `agent_type` is
/// `None`) and for the filename-based session-id recovery hook.
pub async fn parse_session_metadata_with_agent_for(
    explorers: Explorers,
    file: &Path,
    content: &str,
    agent_type: Option<AgentKind>,
) -> SessionMetadata {
    let mut metadata = parse_session_metadata_str(content);
    let agent_type = agent_type.unwrap_or_else(|| explorers.infer_agent_type(file));

    apply_metadata_from_path(explorers, &mut metadata, file, &agent_type).await;
    metadata.agent_type = Some(agent_type);

    metadata
}

/// Parse minimal metadata from a session log string.
pub fn parse_session_metadata_str(content: &str) -> SessionMetadata {
    let metadata = SessionMetadata::default();
    let metadata = parse_session_metadata_reader(BufReader::new(Cursor::new(content)), metadata);
    let mut metadata = metadata;

    if (metadata.session_id.is_none() || metadata.cwd.is_none() || metadata.title.is_none())
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(content)
    {
        apply_metadata_from_value(&mut metadata, &value);
        // Diverged for Desktop: also try to extract a title from the
        // whole-document JSON when the JSONL pass found nothing.
        if metadata.title.is_none() {
            let mut tier = TitleTier::None;
            apply_title_from_value(&mut metadata, &mut tier, &value);
            if metadata.title.is_some() {
                metadata.title_source = tier.as_source();
            }
        }
    }

    metadata
}

#[cfg(test)]
/// Extract the session time range (start, end) from a session log file.
///
/// Scans each line as JSON and looks for known timestamp keys:
/// - Top-level: `timestamp`, `time`, `created_at`, `createdAt`
/// - Nested under `payload`: `timestamp`, `created_at`, `createdAt`
///
/// Parses RFC3339/ISO8601 timestamps and returns the min/max epoch seconds.
/// Returns `None` if no parseable timestamps are found.
pub async fn session_time_range(file: &Path) -> Option<(i64, i64)> {
    let content = tokio::fs::read_to_string(file).await.ok()?;
    let reader = BufReader::new(Cursor::new(content.as_bytes()));
    let range = session_time_range_reader(reader);

    if range.is_none()
        && let Some(value) = read_json_value(file).await
    {
        return session_time_range_from_value(&value);
    }

    range
}

#[cfg(test)]
/// Extract the session time range (start, end) from a session log string.
pub fn session_time_range_str(content: &str) -> Option<(i64, i64)> {
    let reader = BufReader::new(Cursor::new(content));
    let range = session_time_range_reader(reader);

    if range.is_none()
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(content)
    {
        return session_time_range_from_value(&value);
    }

    range
}

/// Parse a timestamp value into epoch seconds.
fn parse_timestamp(value: &serde_json::Value) -> Option<i64> {
    let s = value.as_str()?;
    let dt = OffsetDateTime::parse(s, &Rfc3339).ok()?;
    Some(dt.unix_timestamp())
}

fn parse_numeric_timestamp(value: &serde_json::Value) -> Option<i64> {
    let num = value.as_i64()?;
    if num <= 0 {
        return None;
    }
    if num > 1_000_000_000_000 {
        Some(num / 1000)
    } else {
        Some(num)
    }
}

#[cfg(test)]
async fn read_json_value(file: &Path) -> Option<serde_json::Value> {
    let content = tokio::fs::read_to_string(file).await.ok()?;
    serde_json::from_str(&content).ok()
}

async fn apply_metadata_from_path(
    explorers: Explorers,
    metadata: &mut SessionMetadata,
    file: &Path,
    agent_type: &AgentKind,
) {
    if metadata.session_id.is_none()
        && let Some(id) = explorers.recover_session_id_from_path(agent_type, file)
    {
        metadata.session_id = Some(id);
    }

    let Some(project_dir) = claude_project_dir(file, agent_type) else {
        return;
    };

    let recovered_session_id = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|id| !id.is_empty());
    if metadata.session_id.is_some() {
        // Keep the first parsed ID when the log already provided one.
    } else if let Some(id) = recovered_session_id {
        ::tracing::trace!(
            recovered_session_id = %id,
            path = %file.display(),
            "inferred session_id from Claude project path"
        );
        metadata.session_id = Some(id.to_string());
    }

    let recovered_cwd = if metadata.cwd.is_none() {
        infer_claude_project_cwd(project_dir).await
    } else {
        None
    };
    if let Some(cwd) = recovered_cwd {
        ::tracing::trace!(
            recovered_cwd = %cwd,
            path = %file.display(),
            "inferred cwd from Claude project directory name"
        );
        metadata.cwd = Some(cwd);
    }
}

fn claude_project_dir<'a>(file: &'a Path, agent_type: &AgentKind) -> Option<&'a Path> {
    if *agent_type != AgentKind::Claude {
        return None;
    }

    let project_dir = file.parent()?;
    let projects_dir = project_dir.parent()?;
    let claude_dir = projects_dir.parent()?;
    let encoded = project_dir.file_name()?.to_str()?;

    if projects_dir.file_name().and_then(|name| name.to_str()) != Some("projects")
        || claude_dir.file_name().and_then(|name| name.to_str()) != Some(".claude")
        || !encoded.starts_with('-')
        || encoded.len() <= 1
        || encoded.contains('/')
        || encoded.contains('\\')
    {
        return None;
    }

    Some(project_dir)
}

async fn infer_claude_project_cwd(project_dir: &Path) -> Option<String> {
    let encoded = project_dir.file_name()?.to_str()?;
    // `project_dir` is `<home>/.claude/projects/<encoded>`; walk up three
    // segments to bound the decoder's filesystem probe to the user's home.
    let home = project_dir.parent()?.parent()?.parent()?;
    let decoded = decode_hyphenated_absolute_path(encoded, home).await?;
    Some(decoded.to_string_lossy().to_string())
}

fn parse_session_metadata_reader<R: BufRead>(
    reader: R,
    mut metadata: SessionMetadata,
) -> SessionMetadata {
    // Diverged for Desktop: track which tier the current title came from.
    let mut title_tier = TitleTier::None;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Attempt to parse as JSON
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        apply_metadata_from_value(&mut metadata, &value);
        apply_title_from_value(&mut metadata, &mut title_tier, &value);
    }

    if metadata.title.is_some() {
        metadata.title_source = title_tier.as_source();
    }

    metadata
}

// Codex `response_item` records carry the tool / event name in `payload.name`
// for non-message payloads (`function_call`, `local_shell_call`, etc.). The
// session title extractor must not pick those up as if they were explicit
// titles — `exec_command` is the name of Codex's primary shell tool, not a
// session label. Returns true when the record's `payload.type` identifies a
// tool/event shape whose `name` field is *not* a title.
fn is_non_title_payload(value: &serde_json::Value) -> bool {
    let Some(payload_type) = value.pointer("/payload/type").and_then(|v| v.as_str()) else {
        return false;
    };
    matches!(
        payload_type,
        "function_call"
            | "function_call_output"
            | "local_shell_call"
            | "local_shell_call_output"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "reasoning"
            | "message"
            | "user_message"
            | "agent_message"
            | "task_started"
            | "task_complete"
            | "token_count"
            | "event_msg"
    )
}

// Some agents persist tool invocations as standalone JSONL records whose
// top-level `type` identifies the shape (e.g. `{"type":"tool_use","name":"Bash",
// ...}`). The `name` field on those records is a tool name, not a session
// title, so the explicit-field extractor must skip them.
fn is_non_title_top_level(value: &serde_json::Value) -> bool {
    let Some(top_type) = value.get("type").and_then(|v| v.as_str()) else {
        return false;
    };
    matches!(
        top_type,
        "tool_use" | "tool_result" | "function_call" | "function_call_output"
    )
}

// Diverged for Desktop: extract a title candidate from a single JSON record.
// Higher-tier sources (e.g. an explicit Claude `summary` event) overwrite
// lower-tier ones (e.g. a first-user-message fallback).
fn apply_title_from_value(
    metadata: &mut SessionMetadata,
    tier: &mut TitleTier,
    value: &serde_json::Value,
) {
    // Tier 5: Claude Code user-initiated rename. Beats AI-generated titles.
    // Latest wins (the same session can be renamed multiple times).
    if value.get("type").and_then(|v| v.as_str()) == Some("custom-title")
        && let Some(text) = value.get("customTitle").and_then(|v| v.as_str())
        && !text.trim().is_empty()
        && *tier <= TitleTier::CustomTitle
    {
        metadata.title = Some(normalize_title(text));
        *tier = TitleTier::CustomTitle;
        return;
    }

    // Tier 4: Claude Code AI-generated session title. Latest wins (Claude Code
    // refines the title across multiple ai-title records as the session
    // progresses).
    if value.get("type").and_then(|v| v.as_str()) == Some("ai-title")
        && let Some(text) = value.get("aiTitle").and_then(|v| v.as_str())
        && !text.trim().is_empty()
        && *tier <= TitleTier::AiTitle
    {
        metadata.title = Some(normalize_title(text));
        *tier = TitleTier::AiTitle;
        return;
    }

    // Tier 3: explicit Claude-style `summary` event.
    if let Some(text) = value.get("summary").and_then(|v| v.as_str())
        && !text.trim().is_empty()
        && *tier <= TitleTier::Summary
    {
        metadata.title = Some(normalize_title(text));
        *tier = TitleTier::Summary;
        return;
    }

    // Tier 2: explicit `title` / `name` (top-level or nested under payload).
    // Payload-nested fields are only honoured when the payload doesn't look
    // like a tool/event record — Codex `response_item` carries the tool name
    // in `payload.name` for `function_call`-shaped payloads, and "exec_command"
    // is a tool name, not a session title. The top-level `name` field is
    // similarly skipped on tool-shaped records (e.g. `{"type":"tool_use",
    // "name":"Bash",...}`) so a mid-session tool invocation can't overwrite a
    // user-message-derived title.
    if *tier < TitleTier::ExplicitField {
        let top_level_title = value
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        let top_level_name = if is_non_title_top_level(value) {
            None
        } else {
            value
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
        };
        let payload_nested = if is_non_title_payload(value) {
            None
        } else {
            value
                .pointer("/payload/title")
                .or_else(|| value.pointer("/payload/name"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
        };
        // Cline 2.0+ persists the session title nested under `metadata.title`
        // in `<id>.json`. Top-level `title` is unset on those records, so the
        // earlier candidates miss it. Keep this last in the ExplicitField
        // chain — agents that surface a top-level `title` should still win.
        let metadata_nested = value
            .pointer("/metadata/title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        if let Some(text) = top_level_title
            .or(top_level_name)
            .or(payload_nested)
            .or(metadata_nested)
        {
            metadata.title = Some(normalize_title(text));
            *tier = TitleTier::ExplicitField;
            return;
        }
    }

    // Tier 1: first user message text. Only set if nothing better has been seen.
    // Skip records flagged synthetic by the agent (e.g. Claude Code's
    // `isMeta: true` caveat preamble) so the next real user record gets a turn.
    if *tier == TitleTier::None
        && !value
            .get("isMeta")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        && let Some(text) = first_user_message_text(value)
        && !text.trim().is_empty()
    {
        metadata.title = Some(normalize_title(&text));
        *tier = TitleTier::FirstUserMessage;
    }
}

// Diverged for Desktop: extract user-message text from a record. Handles:
//   - {role:"user", content:"..."}
//   - {role:"user", content:[{type:"text", text:"..."}, ...]}
//   - {type:"user", message:{role:"user", content: ...}}        (Claude)
//   - {type:"response_item", payload:{role:"user", content:...}} (Codex)
//   - Antigravity / Windsurf API cascade payloads, where the prompt lives at
//     /steps/steps/N/userInput/userResponse with no `role:"user"` marker.
fn first_user_message_text(value: &serde_json::Value) -> Option<String> {
    // Codex records are uniquely shaped (`type: "response_item"` or
    // `type: "event_msg"`). Route them through a Codex-specific extractor
    // that strips internal wrapper tags from the resulting text. Every other
    // agent goes through the original keep-or-skip path below — unchanged
    // from before the Codex title work, so their title output is identical.
    let is_codex_shape = matches!(
        value.get("type").and_then(|v| v.as_str()),
        Some("response_item") | Some("event_msg")
    );
    if is_codex_shape {
        let raw = codex_user_message_text(value)?;
        let stripped = strip_synthetic_wrappers(&raw);
        if stripped.is_empty() {
            return None;
        }
        return Some(stripped);
    }

    // Some agents (for example Copilot CLI) wrap the user turn in a typed `data`
    // envelope — `{"type":"user.message","data":{"content":"…"}}` — with no
    // `role:"user"` marker, so the role-keyed candidates below would miss it.
    // Route it through a dedicated extractor, mirroring the Codex arm above.
    if value.get("type").and_then(|v| v.as_str()) == Some("user.message") {
        return copilot_user_message_text(value);
    }

    let candidates = [
        value,
        value.get("message").unwrap_or(&serde_json::Value::Null),
        value.get("payload").unwrap_or(&serde_json::Value::Null),
    ];
    for candidate in candidates {
        if !is_user_role(candidate) {
            continue;
        }
        // Try the candidate's own `content` first, then any nested
        // `message.content` / `payload.content` — Cursor CLI transcripts
        // store the user prompt as `{"role":"user","message":{"content":
        // [{"type":"text","text":"…"}]}}` (top-level `role`, nested
        // `content`), which the original `candidate.get("content")` alone
        // misses.
        let nested_content_sources = [
            candidate.get("content"),
            candidate.get("message").and_then(|m| m.get("content")),
            candidate.get("payload").and_then(|p| p.get("content")),
        ];
        for content in nested_content_sources {
            if let Some(text) = extract_content_text(content) {
                // Skip records whose entire content is internal wrapper tags
                // (slash-command and bash I/O preambles) so the title falls
                // through to the next real user record. Preserve the original
                // text otherwise — callers expect the unmodified prose.
                if is_synthetic_command_text(&text) {
                    return None;
                }
                // Unwrap Cursor CLI's `<user_query>…</user_query>` shell so
                // the title is the user's prose, not the wrapped form.
                return Some(unwrap_user_query(strip_cursor_timestamp(&text)));
            }
        }
    }

    // Antigravity / Windsurf cascade payloads have no `role:"user"` field; the
    // prompt lives inside a nested `steps[].userInput.userResponse` (with
    // `userInput.items[N].text` as a secondary location).
    if let Some(text) = first_cascade_user_text(value) {
        return Some(text);
    }

    None
}

/// Extract normalized user-message candidates in transcript order. Fork-title
/// resolution compares parent and child sequences to identify the first owned
/// divergent prompt without relying on rewritten provider record IDs.
pub fn user_message_titles_from_content(content: &str) -> Vec<String> {
    fn collect(value: &serde_json::Value, output: &mut Vec<String>) {
        if let Some(values) = value.as_array() {
            for value in values {
                collect(value, output);
            }
            return;
        }
        if value
            .get("isMeta")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return;
        }
        if let Some(text) = first_user_message_text(value).filter(|text| !text.trim().is_empty()) {
            output.push(normalize_title(&text));
        }
    }

    let mut output = Vec::new();
    let mut parsed_line = false;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            parsed_line = true;
            collect(&value, &mut output);
        }
    }
    if !parsed_line && let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
        collect(&value, &mut output);
    }
    output
}

/// Codex-only: extract the raw user-prompt text from a `response_item` or
/// `event_msg` record. Two shapes:
///   1. `response_item` with `payload.role == "user"` → `payload.content[]`
///   2. `event_msg` with `payload.type == "user_message"` → `payload.message`
fn codex_user_message_text(value: &serde_json::Value) -> Option<String> {
    let payload = value.get("payload")?;
    if is_user_role(payload)
        && let Some(text) = extract_content_text(payload.get("content"))
    {
        return Some(text);
    }
    if payload.get("type").and_then(|v| v.as_str()) == Some("user_message")
        && let Some(msg) = payload.get("message").and_then(|v| v.as_str())
    {
        return Some(msg.to_string());
    }
    None
}

/// Typed `data` envelope (for example Copilot CLI): extract the user prompt from
/// `{"type":"user.message","data":{"content":"…"}}`. The caller has already
/// matched the `user.message` type. Returns `None` for synthetic slash-command /
/// I/O preambles so the title falls through to the next real user record, and
/// ignores the sibling `data.transformedContent` (the wrapper-augmented form) in
/// favour of the clean `data.content`.
fn copilot_user_message_text(value: &serde_json::Value) -> Option<String> {
    let text = value.pointer("/data/content").and_then(|v| v.as_str())?;
    if is_synthetic_command_text(text) {
        return None;
    }
    Some(text.to_string())
}

fn first_cascade_user_text(value: &serde_json::Value) -> Option<String> {
    // Real on-disk shape: {steps: {steps: [...]}}. The internal `steps` wrapper
    // mirrors the upstream API response. Be permissive in case Antigravity ever
    // flattens it: accept either `value/steps/steps` or `value/steps` directly.
    let inner = value
        .pointer("/steps/steps")
        .and_then(|v| v.as_array())
        .or_else(|| value.pointer("/steps").and_then(|v| v.as_array()))?;

    for step in inner {
        for ptr in ["/userInput/userResponse", "/userInput/items/0/text"] {
            if let Some(text) = step
                .pointer(ptr)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn is_user_role(value: &serde_json::Value) -> bool {
    value.get("role").and_then(|v| v.as_str()) == Some("user")
}

fn extract_content_text(content: Option<&serde_json::Value>) -> Option<String> {
    let content = content?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        for item in arr {
            if let Some(t) = item
                .get("text")
                .or_else(|| item.get("input_text"))
                .and_then(|v| v.as_str())
                && !t.trim().is_empty()
            {
                return Some(t.to_string());
            }
        }
    }
    None
}

pub(super) fn normalize_title(raw: &str) -> String {
    let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > TITLE_MAX_CHARS {
        collapsed.chars().take(TITLE_MAX_CHARS).collect()
    } else {
        collapsed
    }
}

// Diverged for Desktop: Claude Code (and Codex Desktop) embeds slash-command,
// bash I/O, and IDE-state metadata in user-role records as XML-ish wrapper tags
// (e.g. `<command-name>/foo</command-name>` or
// `<ide_opened_file>...</ide_opened_file>`). When such a wrapper leads the
// first user message, its raw text leaks into the title.
//
// Two helpers below share this tag list:
//   - `is_synthetic_command_text` (bool) — used on the non-Codex path; returns
//     true only when `text` is *entirely* wrapper, so the caller skips the
//     record. If prose follows the wrapper the caller keeps the original
//     text as the title (preserves pre-Codex-work behavior for Claude etc.).
//   - `strip_synthetic_wrappers` (String) — used on the Codex-only path;
//     returns the cleaned prose so a `<ide_opened_file>...</ide_opened_file>
//     real prompt` content yields just `real prompt`.
const CLAUDE_WRAPPER_TAGS: &[&str] = &[
    "local-command-caveat",
    "local-command-stdout",
    "local-command-stderr",
    "command-name",
    "command-message",
    "command-args",
    "bash-input",
    "bash-stdout",
    "bash-stderr",
    // IDE-state preamble Claude Code injects when a file is open at session
    // start (and on later IDE focus changes during a session bootstrap).
    "ide_opened_file",
];

/// Remove every complete wrapper-tag pair (e.g. `<command-name>...</command-name>`)
/// from `text` and return the trimmed remainder. Used by the Codex-only branch
/// of `first_user_message_text` to strip Codex Desktop's `<ide_opened_file>`
/// preamble when prose follows the closing tag.
fn strip_synthetic_wrappers(text: &str) -> String {
    let mut buf = text.to_string();
    let mut changed = true;
    while changed {
        changed = false;
        for tag in CLAUDE_WRAPPER_TAGS {
            let open_prefix = format!("<{tag}");
            let close = format!("</{tag}>");
            let Some(open_at) = buf.find(&open_prefix) else {
                continue;
            };
            // Confirm the open tag actually closes (allows attributes / spaces
            // after the tag name).
            let after_prefix = open_at + open_prefix.len();
            let Some(open_end_rel) = buf[after_prefix..].find('>') else {
                continue;
            };
            let open_end = after_prefix + open_end_rel + 1;
            let Some(close_rel) = buf[open_end..].find(&close) else {
                continue;
            };
            let close_end = open_end + close_rel + close.len();
            buf.replace_range(open_at..close_end, "");
            changed = true;
            break;
        }
    }
    buf.trim().to_string()
}

/// Pre-Codex-work bool predicate: true when `text` is entirely composed of
/// wrapper tags (slash-command, bash I/O, IDE-state preambles). Used by the
/// non-Codex (Claude, etc.) branch of `first_user_message_text` to preserve
/// the exact pre-existing title behavior — when `text` contains prose
/// alongside a wrapper, this returns false and the caller keeps the original
/// (unstripped) text as the title.
fn is_synthetic_command_text(text: &str) -> bool {
    strip_synthetic_wrappers(text).is_empty()
}

/// Cursor CLI (`cursor-agent`) wraps every user prompt in
/// `<user_query>…</user_query>` before persisting it to the transcript. When
/// the wrapper is the entire payload, return just the inner prose so titles
/// render as the user's prompt rather than the literal tagged form. Anything
/// else is returned unchanged.
fn strip_cursor_timestamp(text: &str) -> &str {
    let trimmed = text.trim();
    if !trimmed.starts_with("<timestamp>") {
        return text;
    }
    let Some(close) = trimmed.find("</timestamp>") else {
        return text;
    };
    trimmed[close + "</timestamp>".len()..].trim_start()
}

fn unwrap_user_query(text: &str) -> String {
    let trimmed = text.trim();
    const OPEN: &str = "<user_query>";
    const CLOSE: &str = "</user_query>";
    if trimmed.starts_with(OPEN) && trimmed.ends_with(CLOSE) {
        let inner = &trimmed[OPEN.len()..trimmed.len() - CLOSE.len()];
        let inner = inner.trim();
        if !inner.is_empty() {
            return inner.to_string();
        }
    }
    text.to_string()
}

fn apply_metadata_from_value(metadata: &mut SessionMetadata, value: &serde_json::Value) {
    // Extract session_id (top-level, nested under payload for Codex, or nested
    // under a typed `data` envelope, e.g. Copilot CLI's `session.start`).
    if metadata.session_id.is_none()
        && let Some(id) = value
            .get("session_id")
            .or_else(|| value.get("sessionId"))
            .or_else(|| value.get("sessionID"))
            .or_else(|| value.get("taskId"))
            .or_else(|| value.pointer("/payload/id"))
            .or_else(|| value.pointer("/data/sessionId"))
            .and_then(|v| v.as_str())
    {
        metadata.session_id = Some(id.to_string());
    }

    // Extract cwd / workdir (top-level, nested under payload for Codex, or under
    // a typed `data` envelope, e.g. Copilot CLI's `data.context.cwd`).
    let mut saw_direct_cwd = false;
    if let Some(cwd) = value
        .get("cwd")
        .or_else(|| value.get("workdir"))
        .or_else(|| value.get("working_directory"))
        .or_else(|| value.get("directory"))
        .or_else(|| value.get("workspaceDirectory"))
        .or_else(|| value.get("workspacePath"))
        .or_else(|| value.pointer("/payload/cwd"))
        .or_else(|| value.pointer("/data/context/cwd"))
        .and_then(|v| v.as_str())
    {
        metadata.cwd = Some(cwd.to_string());
        saw_direct_cwd = true;
    }

    if metadata.session_id.is_none()
        && let Some(id) = value.get("sessionId").and_then(|v| v.as_str())
    {
        metadata.session_id = Some(id.to_string());
    }
    if metadata.session_id.is_none()
        && let Some(id) = value.get("id").and_then(|v| v.as_str())
        && (id.starts_with("ses_")
            || (id.starts_with("T-") && value.get("messages").and_then(|v| v.as_array()).is_some()))
    {
        metadata.session_id = Some(id.to_string());
    }

    if !saw_direct_cwd && let Some(path) = extract_cwd_from_value(value) {
        metadata.cwd = Some(path);
    }
}

#[cfg(test)]
fn session_time_range_reader<R: BufRead>(reader: R) -> Option<(i64, i64)> {
    let mut min_ts: Option<i64> = None;
    let mut max_ts: Option<i64> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let candidates = [
            value.get("timestamp"),
            value.get("time"),
            value.get("created_at"),
            value.get("createdAt"),
            value.get("creationDate"),
            value.get("lastMessageDate"),
            value.pointer("/payload/timestamp"),
            value.pointer("/payload/created_at"),
            value.pointer("/payload/createdAt"),
        ];

        for candidate in candidates.iter().flatten() {
            if let Some(ts) =
                parse_timestamp(candidate).or_else(|| parse_numeric_timestamp(candidate))
            {
                min_ts = Some(min_ts.map_or(ts, |min| min.min(ts)));
                max_ts = Some(max_ts.map_or(ts, |max| max.max(ts)));
            }
        }
    }

    match (min_ts, max_ts) {
        (Some(min), Some(max)) => Some((min, max)),
        _ => None,
    }
}

#[cfg(test)]
fn session_time_range_from_value(value: &serde_json::Value) -> Option<(i64, i64)> {
    let mut all = Vec::new();
    collect_timestamp_candidates(value, &mut all);
    let mut min_ts: Option<i64> = None;
    let mut max_ts: Option<i64> = None;
    for candidate in all {
        if let Some(ts) =
            parse_timestamp(&candidate).or_else(|| parse_numeric_timestamp(&candidate))
        {
            min_ts = Some(min_ts.map_or(ts, |min| min.min(ts)));
            max_ts = Some(max_ts.map_or(ts, |max| max.max(ts)));
        }
    }
    match (min_ts, max_ts) {
        (Some(min), Some(max)) => Some((min, max)),
        _ => None,
    }
}

fn extract_cwd_from_value(value: &serde_json::Value) -> Option<String> {
    if let Some(path) = value.pointer("/baseUri/fsPath").and_then(|v| v.as_str()) {
        return Some(normalize_cwd_path(path));
    }
    if let Some(path) = value.pointer("/baseUri/path").and_then(|v| v.as_str()) {
        return Some(normalize_cwd_path(path));
    }

    if let Some(requests) = value.get("requests").and_then(|v| v.as_array()) {
        for request in requests {
            if let Some(path) = request
                .pointer("/variableData/variables")
                .and_then(|v| v.as_array())
                .and_then(|vars| {
                    vars.iter().find_map(|var| {
                        var.pointer("/value/uri/fsPath")
                            .or_else(|| var.pointer("/value/uri/path"))
                            .and_then(|v| v.as_str())
                    })
                })
            {
                return Some(normalize_cwd_path(path));
            }

            if let Some(path) = request
                .pointer("/response")
                .and_then(|v| v.as_array())
                .and_then(|responses| {
                    responses.iter().find_map(|resp| {
                        resp.pointer("/baseUri/fsPath")
                            .or_else(|| resp.pointer("/baseUri/path"))
                            .and_then(|v| v.as_str())
                    })
                })
            {
                return Some(normalize_cwd_path(path));
            }
        }
    }

    if let Some(trees) = value
        .pointer("/env/initial/trees")
        .and_then(|v| v.as_array())
    {
        for tree in trees {
            if let Some(uri) = tree.get("uri").and_then(|v| v.as_str()) {
                return Some(normalize_cwd_path(uri));
            }
        }
    }

    None
}

fn normalize_cwd_path(path: &str) -> String {
    let trimmed = strip_file_uri_prefix(path);
    let candidate = Path::new(trimmed);
    if looks_like_file(candidate) {
        candidate
            .parent()
            .unwrap_or(candidate)
            .to_string_lossy()
            .to_string()
    } else {
        candidate.to_string_lossy().to_string()
    }
}

fn strip_file_uri_prefix(path: &str) -> &str {
    let Some(mut trimmed) = path.strip_prefix("file://") else {
        return path;
    };

    if let Some(local_path) = trimmed.strip_prefix("localhost/") {
        trimmed = local_path;
    }

    if let Some(without_leading_slash) = trimmed.strip_prefix('/')
        && is_windows_drive_path(without_leading_slash)
    {
        return without_leading_slash;
    }

    trimmed
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

fn looks_like_file(path: &Path) -> bool {
    if path.extension().is_some() {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        return name.contains('.');
    }
    false
}

/// Extract candidate working directories from a session transcript.
///
/// Scans JSONL content line-by-line for absolute file paths in tool-call
/// inputs (e.g. `file_path`, `path`, `command` fields). Returns deduplicated
/// parent directories that could be resolved to git repo roots.
///
/// This is used as a fallback when the session's recorded `cwd` doesn't
/// resolve to a git repository (e.g. Claude desktop app sessions where `cwd`
/// points to a parent directory like `~/Documents/GitHub`).
pub fn extract_candidate_cwds_from_transcript(content: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();

    for line in content.lines() {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        collect_absolute_paths_from_value(&value, &mut seen, &mut candidates);
    }

    candidates
}

/// Maximum recursion depth when walking nested JSON structures.
/// Prevents stack overflow on pathologically deep transcripts.
const TRANSCRIPT_PATH_MAX_DEPTH: usize = 16;

/// Collect absolute file paths from a JSON value, resolving them to their
/// parent directory. Targets tool-call input fields.
///
/// Recurses into nested objects and arrays up to `TRANSCRIPT_PATH_MAX_DEPTH`
/// to handle varied transcript formats.
fn collect_absolute_paths_from_value(
    value: &serde_json::Value,
    seen: &mut std::collections::HashSet<String>,
    candidates: &mut Vec<String>,
) {
    collect_paths_recursive(value, seen, candidates, 0);
}

fn collect_paths_recursive(
    value: &serde_json::Value,
    seen: &mut std::collections::HashSet<String>,
    candidates: &mut Vec<String>,
    depth: usize,
) {
    if depth > TRANSCRIPT_PATH_MAX_DEPTH {
        return;
    }

    let path_keys = ["file_path", "path", "old_path", "new_path", "directory"];
    for key in &path_keys {
        if let Some(path_str) = value.get(key).and_then(|v| v.as_str()) {
            add_absolute_path_candidate(path_str, seen, candidates);
        }
    }

    // Check "command" field for `cd` commands that reveal working directories.
    if let Some(cmd) = value.get("command").and_then(|v| v.as_str()) {
        extract_cd_targets(cmd, seen, candidates);
    }

    // Recurse into nested structures (e.g., content arrays, tool inputs).
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                // Skip large content fields unlikely to contain tool-call paths.
                if key == "text" || key == "output" || key == "result" || key == "stdout" {
                    continue;
                }
                collect_paths_recursive(val, seen, candidates, depth + 1);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_paths_recursive(item, seen, candidates, depth + 1);
            }
        }
        _ => {}
    }
}

/// Extract absolute directory paths from shell command strings.
///
/// Splits on `&&` and `;` to isolate individual commands, then looks for
/// `cd /absolute/path` patterns. Handles quoted paths.
fn extract_cd_targets(
    cmd: &str,
    seen: &mut std::collections::HashSet<String>,
    candidates: &mut Vec<String>,
) {
    // Split on && and ; to get individual command segments.
    let segments: Vec<&str> = cmd.split("&&").flat_map(|part| part.split(';')).collect();

    for segment in segments {
        let trimmed = segment.trim();
        if let Some(rest) = trimmed.strip_prefix("cd ") {
            let dir = rest.trim().trim_matches('"').trim_matches('\'');
            // Guard against picking up flags or relative paths.
            if dir.starts_with('/') || is_windows_drive_path(dir) {
                add_dir_candidate(dir, seen, candidates);
            }
        }
    }
}

fn add_absolute_path_candidate(
    path_str: &str,
    seen: &mut std::collections::HashSet<String>,
    candidates: &mut Vec<String>,
) {
    if !path_str.starts_with('/') && !is_windows_drive_path(path_str) {
        return;
    }

    let path = Path::new(path_str);
    let dir = if looks_like_file(path) {
        path.parent().map(|p| p.to_string_lossy().to_string())
    } else {
        Some(path_str.to_string())
    };

    if let Some(dir) = dir {
        add_dir_candidate(&dir, seen, candidates);
    }
}

fn add_dir_candidate(
    dir: &str,
    seen: &mut std::collections::HashSet<String>,
    candidates: &mut Vec<String>,
) {
    if !dir.is_empty() && seen.insert(dir.to_string()) {
        candidates.push(dir.to_string());
    }
}

fn collect_timestamp_candidates(value: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                match key.as_str() {
                    "timestamp" | "time" | "created_at" | "createdAt" | "creationDate"
                    | "lastMessageDate" => out.push(val.clone()),
                    _ => {}
                }
                collect_timestamp_candidates(val, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_timestamp_candidates(item, out);
            }
        }
        _ => {}
    }
}

/// Best-effort extraction of a session's latest event time (Unix seconds) from
/// its raw transcript content.
///
/// Scans each JSONL record (falling back to a whole-document JSON parse for
/// non-JSONL transcripts) for known timestamp fields — `timestamp`, `time`,
/// `created_at`/`createdAt`, `creationDate`, `lastMessageDate`, at any nesting
/// depth — and returns the maximum. This is the authoritative "when did this
/// session happen" signal for ordering the Recent Activity list: unlike the
/// transcript file mtime, it survives file copies, cloud sync, backup restores,
/// and provider-DB sources that have no meaningful file mtime. Callers fall back
/// to the file mtime when a transcript carries no parseable timestamp.
pub fn max_event_epoch(content: &str) -> Option<i64> {
    fn scan(value: &serde_json::Value, max_ts: &mut Option<i64>) {
        let mut candidates = Vec::new();
        collect_timestamp_candidates(value, &mut candidates);
        for candidate in &candidates {
            if let Some(ts) =
                parse_timestamp(candidate).or_else(|| parse_numeric_timestamp(candidate))
            {
                *max_ts = Some(max_ts.map_or(ts, |current| current.max(ts)));
            }
        }
    }

    let mut max_ts: Option<i64> = None;
    let mut saw_jsonl_record = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            saw_jsonl_record = true;
            scan(&value, &mut max_ts);
        }
    }
    // Whole-document JSON transcript (not line-delimited).
    if !saw_jsonl_record && let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
        scan(&value, &mut max_ts);
    }
    max_ts
}

/// Return the latest timestamp on a record that represents actual agent work.
///
/// Transcript files also contain records for titles, mode changes, token
/// accounting, permissions, and other bookkeeping. Those records may be
/// written by a harness while an otherwise idle TUI is open, so their
/// timestamps must not move the Recent Activity row. This deliberately keeps
/// the provider-specific allowlist small and fail-closed: callers can fall
/// back to the source heartbeat when a format has no recognised event time.
pub fn max_activity_event_epoch(content: &str, agent: AgentKind) -> Option<i64> {
    fn timestamp(value: &serde_json::Value) -> Option<i64> {
        [
            value.get("timestamp"),
            value.get("time"),
            value.get("created_at"),
            value.get("createdAt"),
            value.get("creationDate"),
            value.pointer("/payload/timestamp"),
        ]
        .into_iter()
        .flatten()
        .find_map(|candidate| {
            parse_timestamp(candidate).or_else(|| parse_numeric_timestamp(candidate))
        })
    }

    fn claude_activity(value: &serde_json::Value) -> bool {
        matches!(
            value.get("type").and_then(serde_json::Value::as_str),
            Some("user")
                | Some("assistant")
                | Some("tool_use")
                | Some("tool_result")
                | Some("function_call")
                | Some("function_call_output")
        )
    }

    fn codex_activity(value: &serde_json::Value) -> bool {
        let top_type = value.get("type").and_then(serde_json::Value::as_str);
        match top_type {
            Some("function_call") | Some("function_call_output") => true,
            Some("response_item") => {
                let payload_type = value
                    .pointer("/payload/type")
                    .and_then(serde_json::Value::as_str);
                matches!(
                    payload_type,
                    Some("message")
                        | Some("reasoning")
                        | Some("function_call")
                        | Some("function_call_output")
                        | Some("custom_tool_call")
                        | Some("custom_tool_call_output")
                        | Some("local_shell_call")
                        | Some("local_shell_call_output")
                )
            }
            Some("event_msg") => matches!(
                value
                    .pointer("/payload/type")
                    .and_then(serde_json::Value::as_str),
                Some("agent_message") | Some("task_started") | Some("task_complete")
            ),
            _ => false,
        }
    }

    let is_activity = |value: &serde_json::Value| match agent {
        AgentKind::Claude => claude_activity(value),
        AgentKind::Codex => codex_activity(value),
        _ => false,
    };

    content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .filter(is_activity)
        .filter_map(|value| timestamp(&value))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    #[test]
    fn max_event_epoch_returns_latest_rfc3339_across_jsonl_records() {
        let content = concat!(
            r#"{"type":"user","timestamp":"2026-05-01T10:00:00Z"}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-05-01T10:05:30Z"}"#,
            "\n",
            r#"not json — ignored"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-05-01T09:00:00Z"}"#,
        );
        // Latest of the three parseable timestamps (10:05:30Z).
        let expected = OffsetDateTime::parse("2026-05-01T10:05:30Z", &Rfc3339)
            .unwrap()
            .unix_timestamp();
        assert_eq!(max_event_epoch(content), Some(expected));
    }

    #[test]
    fn max_event_epoch_reads_numeric_and_nested_and_whole_document() {
        // Numeric epoch-millis nested under `payload`, whole-document JSON.
        let content = r#"{"payload":{"createdAt":1746093930000}}"#;
        assert_eq!(max_event_epoch(content), Some(1_746_093_930));

        // No parseable timestamp anywhere → None so callers fall back to mtime.
        assert_eq!(max_event_epoch(r#"{"type":"user","text":"hi"}"#), None);
        assert_eq!(max_event_epoch(""), None);
    }

    #[test]
    fn max_activity_event_epoch_ignores_claude_housekeeping_records() {
        let content = concat!(
            r#"{"type":"user","timestamp":"2026-06-26T21:20:00Z"}"#,
            "\n",
            r#"{"type":"custom-title","timestamp":"2026-08-19T17:07:32Z"}"#,
            "\n",
            r#"{"type":"permission-mode","timestamp":"2026-08-19T17:07:33Z"}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-06-26T21:30:15Z"}"#,
        );
        let expected = OffsetDateTime::parse("2026-06-26T21:30:15Z", &Rfc3339)
            .unwrap()
            .unix_timestamp();
        assert_eq!(
            max_activity_event_epoch(content, AgentKind::Claude),
            Some(expected)
        );
    }

    #[test]
    fn max_activity_event_epoch_ignores_codex_metadata_and_token_counts() {
        let content = concat!(
            r#"{"timestamp":"2026-06-26T21:20:00Z","type":"session_meta","payload":{"id":"x"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-26T21:30:15Z","type":"response_item","payload":{"type":"function_call","name":"exec_command"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-19T17:07:32Z","type":"event_msg","payload":{"type":"token_count"}}"#,
        );
        let expected = OffsetDateTime::parse("2026-06-26T21:30:15Z", &Rfc3339)
            .unwrap()
            .unix_timestamp();
        assert_eq!(
            max_activity_event_epoch(content, AgentKind::Codex),
            Some(expected)
        );
    }

    async fn write_temp_file(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        tokio::fs::write(&path, content)
            .await
            .expect("failed to write temp file");
        path
    }

    fn encode_claude_project_path(path: &Path) -> String {
        let raw = path.to_string_lossy();
        let normalized = raw.replace('\\', "/");
        let encoded = if let Some(without_drive_root) = normalized
            .strip_prefix(|c: char| c.is_ascii_alphabetic())
            .and_then(|rest| rest.strip_prefix(":/"))
        {
            format!(
                "{}--{}",
                normalized.chars().next().unwrap_or_default(),
                without_drive_root.replace('/', "-")
            )
        } else {
            normalized.replace('/', "-")
        };
        if encoded.starts_with('-') {
            encoded
        } else {
            format!("-{encoded}")
        }
    }

    use super::super::Explorers;
    use super::super::agents;

    const FAKE_HOME: &str = "/Users/foo";

    #[tokio::test]
    async fn test_infer_agent_type_claude() {
        let path = agents::claude::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Claude);
    }

    #[tokio::test]
    async fn test_infer_agent_type_codex() {
        let path = agents::codex::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Codex);
    }

    #[tokio::test]
    async fn test_infer_agent_type_cline() {
        let path = agents::cline::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Cline);
    }

    #[tokio::test]
    async fn test_infer_agent_type_opencode() {
        let path = agents::opencode::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::OpenCode);
    }

    #[tokio::test]
    async fn test_infer_agent_type_kiro() {
        let path = agents::kiro::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Kiro);
    }

    #[tokio::test]
    async fn test_infer_agent_type_amp_code() {
        let path = agents::amp_code::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::AmpCode);
    }

    #[tokio::test]
    async fn test_infer_agent_type_cursor() {
        let path = agents::cursor::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Cursor);
    }

    /// Cross-agent precedence: a Cursor `globalStorage` path that also matches
    /// Cline's `saoudrizwan.claude-dev` substring must resolve to Cline. Path
    /// is intentionally literal — this is a dispatcher-ordering test.
    #[tokio::test]
    async fn test_infer_agent_type_cline_wins_over_cursor_for_ambiguous_path() {
        let path = Path::new(
            "/Users/foo/Library/Application Support/Cursor/User/globalStorage/saoudrizwan.claude-dev/tasks/x/task.json",
        );
        assert_eq!(Explorers::DISK.infer_agent_type(path), AgentKind::Cline);
    }

    #[tokio::test]
    async fn test_infer_agent_type_copilot() {
        let path = agents::copilot::sample_vs_code_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Copilot);
    }

    #[tokio::test]
    async fn test_infer_agent_type_copilot_cli_home() {
        let path = agents::copilot::sample_cli_home_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Copilot);
    }

    #[tokio::test]
    async fn test_infer_agent_type_copilot_cli_xdg() {
        let path = agents::copilot::sample_cli_xdg_log_path(Path::new("/home/foo"));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Copilot);
    }

    #[tokio::test]
    async fn test_infer_agent_type_copilot_cli_windows() {
        let path =
            agents::copilot::sample_cli_windows_log_path(Path::new(r"C:\Users\foo\AppData\Local"));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Copilot);
    }

    #[tokio::test]
    async fn test_infer_agent_type_antigravity() {
        let path = agents::antigravity::sample_workspace_log_path(Path::new(FAKE_HOME));
        assert_eq!(
            Explorers::DISK.infer_agent_type(&path),
            AgentKind::Antigravity
        );
    }

    #[tokio::test]
    async fn test_infer_agent_type_antigravity_cli_brain() {
        let path = agents::antigravity::sample_cli_brain_log_path(Path::new(FAKE_HOME));
        assert_eq!(
            Explorers::DISK.infer_agent_type(&path),
            AgentKind::Antigravity
        );
    }

    #[tokio::test]
    async fn test_infer_agent_type_antigravity_ide_brain() {
        let path = agents::antigravity::sample_ide_brain_log_path(Path::new(FAKE_HOME));
        assert_eq!(
            Explorers::DISK.infer_agent_type(&path),
            AgentKind::Antigravity
        );
    }

    /// Defensive regression guard: an Antigravity CLI brain path whose
    /// session directory is named `cursor`. Cursor's matcher is bounded to
    /// `/.cursor/` and `/cursor/user/` so this already won't be misattributed,
    /// but the dispatcher also runs Antigravity before Cursor — if anyone
    /// loosens the Cursor matcher again, this test catches it.
    #[tokio::test]
    async fn test_infer_agent_type_antigravity_cli_brain_wins_over_cursor() {
        let path = Path::new("/Users/foo/.gemini/antigravity-cli/brain/cursor/trace.jsonl");
        assert_eq!(
            Explorers::DISK.infer_agent_type(path),
            AgentKind::Antigravity
        );
    }

    /// Defensive regression guard: a Copilot CLI session-state path whose
    /// session directory is named `cursor`. Same reasoning as the Antigravity
    /// case above — Cursor's bounded matcher won't claim this today, but the
    /// test pins the contract against future loosening.
    #[tokio::test]
    async fn test_infer_agent_type_copilot_cli_wins_over_cursor() {
        let path = Path::new("/Users/foo/.copilot/session-state/cursor/transcript.jsonl");
        assert_eq!(Explorers::DISK.infer_agent_type(path), AgentKind::Copilot);
    }

    /// Cross-agent precedence: a Windsurf workspace path that incidentally
    /// contains the substring `cursor` (e.g. a workspace folder named
    /// `~/code/cursor-experiments/`) must resolve to Windsurf. The old
    /// `infer_agent_type` (before `AgentKind::ALL`) classified Cursor before
    /// Windsurf with a broad `.cursor` match — this test pins the new
    /// ordering so a regression that loosens Cursor's matcher again is caught.
    #[tokio::test]
    async fn test_infer_agent_type_windsurf_wins_over_cursor_for_ambiguous_substring() {
        let path = Path::new(
            "/Users/foo/Library/Application Support/Windsurf/User/workspaceStorage/abc/Codeium.windsurf/cursor-experiments/session.jsonl",
        );
        assert_eq!(Explorers::DISK.infer_agent_type(path), AgentKind::Windsurf);
    }

    /// Cross-agent precedence: a genuine Cursor path must still resolve to
    /// Cursor even though Windsurf is now ordered before it in
    /// `AgentKind::ALL`. Bookend to the test above — both must hold.
    #[tokio::test]
    async fn test_infer_agent_type_cursor_still_wins_for_canonical_cursor_path() {
        let path = Path::new(
            "/Users/foo/Library/Application Support/Cursor/User/workspaceStorage/abc/state.vscdb",
        );
        assert_eq!(Explorers::DISK.infer_agent_type(path), AgentKind::Cursor);
    }

    #[tokio::test]
    async fn test_infer_agent_type_windsurf() {
        let path = agents::windsurf::sample_codeium_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Windsurf);
    }

    #[tokio::test]
    async fn test_infer_agent_type_windsurf_linux_workspace_storage() {
        let path = agents::windsurf::sample_linux_workspace_log_path(Path::new("/home/foo"));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Windsurf);
    }

    #[tokio::test]
    async fn test_infer_agent_type_windsurf_windows_workspace_storage() {
        let path = agents::windsurf::sample_windows_workspace_log_path(
            Path::new(r"C:\Users\foo"),
            Path::new(r"C:\Users\foo\AppData\Roaming"),
        );
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Windsurf);
    }

    #[tokio::test]
    async fn test_infer_agent_type_pi() {
        let path = agents::pi::sample_log_path(Path::new(FAKE_HOME));
        assert_eq!(Explorers::DISK.infer_agent_type(&path), AgentKind::Pi);
    }

    #[tokio::test]
    async fn test_infer_agent_type_unknown_defaults_to_claude() {
        let path = Path::new("/tmp/some/random/session.jsonl");
        assert_eq!(Explorers::DISK.infer_agent_type(path), AgentKind::Claude);
    }

    #[tokio::test]
    async fn test_agent_type_display_claude() {
        assert_eq!(AgentKind::Claude.to_string(), "claude-code");
    }

    #[tokio::test]
    async fn test_agent_type_display_codex() {
        assert_eq!(AgentKind::Codex.to_string(), "codex");
    }

    #[tokio::test]
    async fn test_agent_type_display_cursor() {
        assert_eq!(AgentKind::Cursor.to_string(), "cursor");
    }

    #[tokio::test]
    async fn test_agent_type_display_copilot() {
        assert_eq!(AgentKind::Copilot.to_string(), "copilot");
    }

    #[tokio::test]
    async fn test_agent_type_display_cline() {
        assert_eq!(AgentKind::Cline.to_string(), "cline");
    }

    #[tokio::test]
    async fn test_agent_type_display_opencode() {
        assert_eq!(AgentKind::OpenCode.to_string(), "opencode");
    }

    #[tokio::test]
    async fn test_agent_type_display_kiro() {
        assert_eq!(AgentKind::Kiro.to_string(), "kiro");
    }

    #[tokio::test]
    async fn test_agent_type_display_amp_code() {
        assert_eq!(AgentKind::AmpCode.to_string(), "amp-code");
    }

    #[tokio::test]
    async fn test_agent_type_display_antigravity() {
        assert_eq!(AgentKind::Antigravity.to_string(), "antigravity");
    }

    #[tokio::test]
    async fn test_agent_type_display_windsurf() {
        assert_eq!(AgentKind::Windsurf.to_string(), "windsurf");
    }

    #[tokio::test]
    async fn test_agent_type_display_pi() {
        assert_eq!(AgentKind::Pi.to_string(), "pi");
    }

    #[tokio::test]
    async fn test_parse_metadata_session_id_and_cwd() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"session_id":"abc-123","cwd":"/Users/foo/bar"}
{"type":"message","content":"hello"}
"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("abc-123".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/bar".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_camel_case_session_id() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"sessionId":"def-456","cwd":"/Users/baz"}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("def-456".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_workdir_field() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"workdir":"/home/user/project"}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.cwd, Some("/home/user/project".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_working_directory_field() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"working_directory":"/opt/app"}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.cwd, Some("/opt/app".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_opencode_fields() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"id":"ses_123","sessionID":"ses_123","directory":"/Users/foo/dev/repo"}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("ses_123".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/repo".to_string()));
    }

    /// Copilot CLI `events.jsonl` nests session id + cwd under `data` on the
    /// `session.start` line, and the user prompt under `data.content` on a
    /// `user.message` line. Without these the daemon can't resolve a repo and
    /// silently drops every Copilot CLI session. Shapes captured verbatim from
    /// a real `~/.copilot/session-state/<id>/events.jsonl`.
    #[tokio::test]
    async fn test_parse_metadata_copilot_cli_session_state() {
        let dir = TempDir::new().unwrap();
        let content = concat!(
            r#"{"type":"session.start","data":{"sessionId":"525396d0-810b-44cc-99c3-ff60f07b39e1","version":1,"producer":"copilot-agent","context":{"cwd":"/Users/foo/dev/demo-app"}},"id":"e0566314-21f0-454b-9afb-4b6b2ebe3f90","timestamp":"2026-06-01T03:45:05.224Z","parentId":null}"#,
            "\n",
            r#"{"type":"system.message","data":{"role":"system","content":"You are the GitHub Copilot CLI."},"id":"sys-1","timestamp":"2026-06-01T03:45:05.300Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"hello world test from copilot cli","transformedContent":"<current_datetime>2026-06-01T14:22:04+10:00</current_datetime>\n\nhello world test from copilot cli"},"id":"usr-1","timestamp":"2026-06-01T03:45:10.000Z"}"#,
            "\n",
        );
        let file = write_temp_file(dir.path(), "events.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(
            metadata.session_id,
            Some("525396d0-810b-44cc-99c3-ff60f07b39e1".to_string())
        );
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/demo-app".to_string()));
        assert_eq!(
            metadata.title,
            Some("hello world test from copilot cli".to_string())
        );
    }

    /// A Copilot CLI `user.message` whose prompt is entirely a synthetic
    /// slash-command wrapper must be skipped so the title falls through to the
    /// next real user message — exercising `copilot_user_message_text`'s
    /// synthetic-skip (`None`) branch.
    #[tokio::test]
    async fn test_parse_metadata_copilot_cli_skips_synthetic_user_message() {
        let dir = TempDir::new().unwrap();
        let content = concat!(
            r#"{"type":"user.message","data":{"content":"<command-name>/clear</command-name>"},"id":"usr-1","timestamp":"2026-06-01T03:45:05.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"the real first prompt"},"id":"usr-2","timestamp":"2026-06-01T03:45:10.000Z"}"#,
            "\n",
        );
        let file = write_temp_file(dir.path(), "events.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.title, Some("the real first prompt".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_kiro_fields() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"sessionId":"kiro-1","workspaceDirectory":"/Users/foo/dev/kiro-repo"}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("kiro-1".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/kiro-repo".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_amp_fields() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"id":"T-123","messages":[],"env":{"initial":{"trees":[{"uri":"file:///Users/foo/dev/demo-cli"}]}}}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("T-123".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/demo-cli".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_amp_fields_windows_file_uri() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"id":"T-456","messages":[],"env":{"initial":{"trees":[{"uri":"file:///C:/Users/foo/dev/demo-cli"}]}}}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("T-456".to_string()));
        assert_eq!(metadata.cwd, Some("C:/Users/foo/dev/demo-cli".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_cline_task_id() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"taskId":"task-123","workspacePath":"/Users/foo/dev/repo"}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("task-123".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/repo".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_chat_session_json() {
        let dir = TempDir::new().unwrap();
        let content = r#"{
  "sessionId": "chat-123",
  "requests": [
    {
      "variableData": {
        "variables": [
          {
            "value": {
              "uri": {
                "fsPath": "/Users/foo/dev/my-repo/src/main.rs"
              }
            }
          }
        ]
      }
    }
  ]
}"#;
        let file = write_temp_file(dir.path(), "session.json", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("chat-123".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/my-repo/src".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_fields_across_multiple_lines() {
        let dir = TempDir::new().unwrap();
        // session_id on one line, cwd on another
        let content = r#"{"session_id":"multi-line-id"}
{"type":"other"}
{"cwd":"/Users/foo/repo"}
"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("multi-line-id".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/repo".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_no_fields() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"type":"message","content":"no metadata here"}
{"type":"tool_result","content":"more stuff"}
"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, None);
        assert_eq!(metadata.cwd, None);
    }

    #[tokio::test]
    async fn test_parse_metadata_invalid_json_lines_skipped() {
        let dir = TempDir::new().expect("temporary session directory should be created");
        let content = r#"this is not json
{"session_id":"valid-id"}
also not json {{{{
{"cwd":"/valid/path"}
"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("valid-id".to_string()));
        assert_eq!(metadata.cwd, Some("/valid/path".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_empty_file() {
        let dir = TempDir::new().unwrap();
        let file = write_temp_file(dir.path(), "empty.jsonl", "").await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, None);
        assert_eq!(metadata.cwd, None);
    }

    #[tokio::test]
    async fn test_parse_metadata_nonexistent_file() {
        let path = Path::new("/nonexistent/file.jsonl");
        let metadata = parse_session_metadata(path).await;

        assert_eq!(metadata.session_id, None);
        assert_eq!(metadata.cwd, None);
    }

    #[tokio::test]
    async fn test_parse_metadata_agent_type_from_claude_path() {
        let dir = TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude").join("projects");
        tokio::fs::create_dir_all(&claude_dir).await.unwrap();
        let file = write_temp_file(&claude_dir, "session.jsonl", "{}").await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.agent_type, Some(AgentKind::Claude));
    }

    #[tokio::test]
    async fn test_parse_metadata_agent_type_from_codex_path() {
        let dir = TempDir::new().unwrap();
        let codex_dir = dir.path().join(".codex").join("sessions");
        tokio::fs::create_dir_all(&codex_dir).await.unwrap();
        let file = write_temp_file(&codex_dir, "session.jsonl", "{}").await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.agent_type, Some(AgentKind::Codex));
    }

    #[tokio::test]
    async fn test_parse_metadata_agent_type_from_opencode_path() {
        let dir = TempDir::new().unwrap();
        let opencode_dir = dir.path().join(".local").join("share").join("opencode");
        tokio::fs::create_dir_all(&opencode_dir).await.unwrap();
        let file = write_temp_file(&opencode_dir, "session.json", "{}").await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.agent_type, Some(AgentKind::OpenCode));
    }

    #[tokio::test]
    async fn test_parse_metadata_agent_type_from_opencode_custom_xdg_path() {
        let dir = TempDir::new().unwrap();
        let opencode_dir = dir
            .path()
            .join("custom-data")
            .join("opencode")
            .join("storage")
            .join("session");
        tokio::fs::create_dir_all(&opencode_dir).await.unwrap();
        let file = write_temp_file(&opencode_dir, "session.json", "{}").await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.agent_type, Some(AgentKind::OpenCode));
    }

    #[tokio::test]
    async fn test_parse_metadata_infers_claude_session_id_and_cwd_from_path() {
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("work").join("git-context-explorer");
        tokio::fs::create_dir_all(&repo_root).await.unwrap();

        let encoded = encode_claude_project_path(&repo_root);
        let claude_dir = dir.path().join(".claude").join("projects").join(encoded);
        tokio::fs::create_dir_all(&claude_dir).await.unwrap();

        let file = write_temp_file(
            &claude_dir,
            "042afd87-a800-4248-8234-5e9222dd6f22.jsonl",
            "{}",
        )
        .await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(
            metadata.session_id,
            Some("042afd87-a800-4248-8234-5e9222dd6f22".to_string())
        );
        assert_eq!(metadata.cwd, Some(repo_root.to_string_lossy().to_string()));
    }

    #[tokio::test]
    async fn test_decode_hyphenated_absolute_path_handles_hyphenated_segments() {
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("work").join("git--context-explorer");
        tokio::fs::create_dir_all(&repo_root).await.unwrap();

        let decoded =
            decode_hyphenated_absolute_path(&encode_claude_project_path(&repo_root), dir.path())
                .await;

        assert_eq!(decoded, Some(repo_root));
    }

    // Note: `initial_hyphenated_path_state` unit tests live alongside the
    // helper in `agents::path_codec` — keeps the implementation invariant
    // and its tests colocated.

    #[tokio::test]
    async fn test_parse_metadata_first_value_wins() {
        let dir = TempDir::new().unwrap();
        // Two lines with session_id -- first should win
        let content = r#"{"session_id":"first-id"}
{"session_id":"second-id"}
"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("first-id".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_last_cwd_wins() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"session_id":"session-1","cwd":"/tmp/first"}
{"cwd":"/tmp/second"}
"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("session-1".to_string()));
        assert_eq!(metadata.cwd, Some("/tmp/second".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_direct_cwd_beats_nested_payload_path_in_same_event() {
        let dir = TempDir::new().unwrap();
        let content =
            r#"{"sessionId":"session-1","cwd":"/top/level","payload":{"cwd":"/payload/path"}}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("session-1".to_string()));
        assert_eq!(metadata.cwd, Some("/top/level".to_string()));
    }

    #[test]
    fn test_parse_metadata_str_extracts_fields() {
        let content = r#"{"session_id":"str-1","cwd":"/tmp/repo"}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.session_id, Some("str-1".to_string()));
        assert_eq!(metadata.cwd, Some("/tmp/repo".to_string()));
    }

    #[test]
    fn test_parse_metadata_str_falls_back_to_full_json() {
        let content = r#"{"sessionId":"chat-999","requests":[{"response":[{"baseUri":{"path":"file:///Users/foo/dev/repo"}}]}]}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.session_id, Some("chat-999".to_string()));
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/repo".to_string()));
    }

    #[test]
    fn test_normalize_cwd_path_handles_localhost_windows_file_uri() {
        let normalized = normalize_cwd_path("file://localhost/C:/Users/foo/dev/repo");
        assert_eq!(normalized, "C:/Users/foo/dev/repo".to_string());
    }

    #[test]
    fn test_title_from_claude_summary_event() {
        // Claude Code emits a `summary` line at the end of a session.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"first prompt text"}}"#,
            "\n",
            r#"{"type":"summary","summary":"Refactor auth module","leafUuid":"abc"}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("Refactor auth module"));
    }

    #[test]
    fn test_title_falls_back_to_first_user_message_when_no_summary() {
        let content =
            r#"{"type":"user","message":{"role":"user","content":"investigate the flaky test"}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("investigate the flaky test")
        );
    }

    #[test]
    fn test_title_from_codex_first_user_message() {
        // Codex stores user messages with content as an array of input_text parts.
        let content = concat!(
            r#"{"timestamp":"2026-02-10T01:52:21Z","type":"session_meta","payload":{"id":"sid","cwd":"/repo"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add a session title interface"}]}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("add a session title interface")
        );
    }

    #[test]
    fn test_title_from_explicit_title_field_full_document() {
        // VS Code chatSessions JSON: single document, top-level `name` field.
        let content = r#"{"sessionId":"chat-1","name":"Fix login bug","requests":[]}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("Fix login bug"));
    }

    #[test]
    fn test_title_from_cline_metadata_nested_title() {
        // Cline 2.0+ `<id>.json`: title lives at `metadata.title`, not at the
        // top level. The whole-document fallback path must extract it via
        // the ExplicitField tier so Recent Activity shows a human-readable
        // title instead of the session id.
        let content = r#"{
            "version": 1,
            "session_id": "1779760221048_s43gt",
            "cwd": "/Users/x/repo",
            "provider": "cline",
            "prompt": "<user_input mode=\"act\">hello world from cline CLI</user_input>",
            "metadata": {
                "title": "hello world from cline CLI",
                "totalCost": 0.024
            }
        }"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("hello world from cline CLI"),
        );
        assert_eq!(metadata.title_source, Some(TitleSource::Explicit));
    }

    #[test]
    fn test_title_top_level_title_still_wins_over_metadata_nested() {
        // A top-level `title` must still beat `metadata.title` — order in the
        // ExplicitField chain matters for any agent that populates both.
        let content = r#"{"title":"top","metadata":{"title":"nested"}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("top"));
    }

    #[test]
    fn test_title_summary_overrides_user_message_regardless_of_order() {
        // Summary line appears BEFORE the first user message; still wins.
        let content = concat!(
            r#"{"type":"summary","summary":"Authoritative summary"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"earlier user prompt"}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("Authoritative summary"));
    }

    #[test]
    fn test_title_explicit_field_beats_user_message_fallback() {
        // First line: user message (tier 1). Second line: explicit title (tier 2).
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"earlier prompt"}}"#,
            "\n",
            r#"{"title":"Explicit title here"}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("Explicit title here"));
    }

    #[test]
    fn test_title_normalizes_whitespace_and_truncates() {
        let long = "a ".repeat(150); // 300 chars including spaces
        let content = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"{}"}}}}"#,
            long.trim()
        );
        let metadata = parse_session_metadata_str(&content);
        let title = metadata.title.expect("title should be set");
        assert!(title.chars().count() <= TITLE_MAX_CHARS);
        assert!(!title.contains("  "));
    }

    #[test]
    fn test_title_none_when_nothing_recoverable() {
        let content = r#"{"unrelated":"value"}"#;
        let metadata = parse_session_metadata_str(content);
        assert!(metadata.title.is_none());
    }

    #[test]
    fn test_title_skips_blank_summary() {
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"real prompt"}}"#,
            "\n",
            r#"{"type":"summary","summary":"   "}"#,
        );
        let metadata = parse_session_metadata_str(content);
        // Blank summary is ignored; user-message fallback wins.
        assert_eq!(metadata.title.as_deref(), Some("real prompt"));
    }

    #[test]
    fn test_title_skips_claude_ismeta_caveat_record() {
        // Claude Code injects an `isMeta:true` <local-command-caveat> record
        // before any real user prompt when a session is started via slash command.
        let content = concat!(
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat: The messages below were generated by the user while running local commands. DO NOT respond to these messages or otherwise consider them in your response unless the user explicitly asks you to.</local-command-caveat>"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"the current branch supports Zed editor AI sessions. Review now."}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("the current branch supports Zed editor AI sessions. Review now.")
        );
    }

    #[test]
    fn test_title_skips_claude_slash_command_wrapper_record() {
        // The first user record after the caveat is a slash-command echo with
        // <command-name>/<command-message>/<command-args> wrappers and no real
        // text. The title should fall through to the next record.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/model</command-name>\n            <command-message>model</command-message>\n            <command-args>default</command-args>"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to claude-opus-4-7[1m]</local-command-stdout>"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"actually do the work"}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("actually do the work"));
    }

    #[test]
    fn test_title_keeps_user_text_alongside_wrapper_tags() {
        // A real user message that happens to contain a complete <command-name>
        // pair *plus* additional prose should keep the original content (the
        // wrapper-only check fails because non-empty text remains after strip).
        let content = r#"{"type":"user","message":{"role":"user","content":"<command-name>/foo</command-name>\n\nbut also explain X"}}"#;
        let metadata = parse_session_metadata_str(content);
        let title = metadata.title.expect("title should be set");
        assert!(
            title.contains("but also explain X"),
            "expected the real prose to survive, got: {title:?}"
        );
    }

    #[test]
    fn test_title_claude_wrapper_alongside_prose_keeps_original_text() {
        // Cross-agent invariant: when a Claude (or any non-Codex) record carries
        // a complete wrapper pair plus prose, the title must be the *original*
        // unstripped text. The Codex-only branch of `first_user_message_text`
        // strips wrappers; this test pins the non-Codex branch's preserved
        // behavior so a future shared-helper refactor cannot silently change it.
        let content = r#"{"type":"user","message":{"role":"user","content":"<command-name>/foo</command-name>\n\nbut also explain X"}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            // `normalize_title` collapses whitespace, so the `\n\n` becomes a single space.
            Some("<command-name>/foo</command-name> but also explain X"),
        );
    }

    #[test]
    fn test_title_keeps_user_text_with_incomplete_wrapper_mention() {
        // User legitimately mentions a tag name without a closing pair —
        // the helper must not strip anything, and the prose passes through.
        let content = r#"{"type":"user","message":{"role":"user","content":"rename <command-name> to <commandName>"}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("rename <command-name> to <commandName>")
        );
    }

    #[test]
    fn test_title_codex_user_message_unchanged_by_synthetic_filter() {
        // Cross-agent regression guard: plain Codex user content must pass
        // through. Wrapper-tag patterns are Claude-only.
        let content = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add a session title interface"}]}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("add a session title interface")
        );
    }

    #[test]
    fn test_title_codex_session_meta_still_ignored() {
        // Cross-agent regression guard: Codex session_meta has no `role:"user"`,
        // so it must continue to be skipped (not even reach the synthetic check).
        let content = concat!(
            r#"{"type":"session_meta","payload":{"id":"sid","cwd":"/repo"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first real prompt"}]}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("first real prompt"));
    }

    #[test]
    fn test_title_codex_function_call_does_not_override_user_message() {
        // Codex emits `response_item` records with `payload.type="function_call"`
        // and `payload.name="exec_command"` (the shell tool). The title must
        // remain the user's first prompt, not the tool name.
        let content = concat!(
            r#"{"type":"session_meta","payload":{"id":"sid","cwd":"/repo"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"refactor the auth middleware"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"command\":[\"ls\"]}","call_id":"call_1"}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("refactor the auth middleware")
        );
    }

    #[test]
    fn test_title_top_level_tool_use_does_not_override_user_message() {
        // Defense-in-depth: agents that persist tool invocations as standalone
        // JSONL records ({"type":"tool_use","name":"Bash",...}) must not have
        // the tool name promoted to a session title.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"refactor the auth middleware"}}"#,
            "\n",
            r#"{"type":"tool_use","name":"Bash","input":{"command":"ls"}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("refactor the auth middleware")
        );
    }

    #[test]
    fn test_title_strips_ide_opened_file_wrapper_when_prose_follows() {
        // Codex Desktop emits the IDE-state preamble + the real user prompt
        // in a single user `content` string. The wrapper must be stripped
        // away so the title shows just the prompt.
        let content = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<ide_opened_file>The user opened the file /Users/avery/config.toml in the IDE. This may or may not be related to the current task.</ide_opened_file>hide the \"Open Dashboard\" button"}]}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("hide the \"Open Dashboard\" button")
        );
    }

    #[test]
    fn test_title_codex_event_msg_user_message_extracted() {
        // Codex `event_msg` records carry the user prompt as a flat string in
        // `payload.message` with no `role:"user"` marker. The title must
        // still extract from these.
        let content = concat!(
            r#"{"type":"session_meta","payload":{"id":"sid","cwd":"/repo"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"refactor the auth middleware","local_images":[],"text_elements":[]}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("refactor the auth middleware")
        );
    }

    #[test]
    fn test_title_codex_event_msg_user_message_strips_wrappers() {
        // event_msg.user_message also routes through wrapper stripping so a
        // leading `<ide_opened_file>...</ide_opened_file>` preamble disappears.
        let content = r#"{"type":"event_msg","payload":{"type":"user_message","message":"<ide_opened_file>The user opened /Users/avery/x.toml.</ide_opened_file>hide the Open Dashboard button"}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("hide the Open Dashboard button")
        );
    }

    #[test]
    fn test_title_codex_function_call_alone_yields_no_title() {
        // A Codex session whose transcript contains only tool calls (no user
        // message yet) must not synthesise a title from `payload.name`. The
        // frontend then falls back to `Session <id>`.
        let content = concat!(
            r#"{"type":"session_meta","payload":{"id":"sid","cwd":"/repo"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"call_1"}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert!(
            metadata.title.is_none(),
            "expected no title, got: {:?}",
            metadata.title
        );
    }

    #[test]
    fn test_title_skips_claude_ide_opened_file_wrapper() {
        // Claude Code injects an <ide_opened_file>...</ide_opened_file> preamble
        // as the first user record when a file is open in the IDE at session
        // start. Title should fall through to the next real user message.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"<ide_opened_file>The user opened the file /path/to/SessionDetailView.tsx in the IDE. This may or may not be related to the current task.</ide_opened_file>"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"introduce the session title into SessionDetailView"}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("introduce the session title into SessionDetailView")
        );
    }

    #[test]
    fn test_title_uses_claude_ai_title_record() {
        // Claude Code now persists `{type:"ai-title", aiTitle:"..."}` records.
        // ai-title beats first-user-message and beats `summary`.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"the user's first prompt"}}"#,
            "\n",
            r#"{"type":"summary","summary":"older summary text"}"#,
            "\n",
            r#"{"type":"ai-title","sessionId":"abc","aiTitle":"Display organization name instead of UUID"}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("Display organization name instead of UUID")
        );
    }

    #[test]
    fn test_title_extracts_cursor_cli_user_query_through_nested_message_content() {
        // Real on-disk shape from
        // ~/.cursor/projects/<ws>/agent-transcripts/<id>/<id>.jsonl —
        // `role:"user"` lives on the outer object while `content` is nested
        // under `message`. The `<user_query>…</user_query>` wrapper must be
        // stripped so the title is the prose, not the tagged form.
        let content = concat!(
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nhello world (test III) from Cursor CLI\n</user_query>"}]}}"#,
            "\n",
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"reply"}]}}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("hello world (test III) from Cursor CLI")
        );
        assert_eq!(metadata.title_source, Some(TitleSource::FirstMessage));
    }

    #[test]
    fn test_title_strips_cursor_leading_timestamp_before_user_query() {
        let content = r#"{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>2026-07-29T12:00:00Z</timestamp>\n<user_query>Fix the upload retry</user_query>"}]}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("Fix the upload retry"));
    }

    #[test]
    fn test_title_preserves_meaningful_date_text() {
        let content = r#"{"role":"user","message":{"content":[{"type":"text","text":"Plan the July 29, 2026 release"}]}}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("Plan the July 29, 2026 release")
        );
    }

    #[test]
    fn test_title_custom_title_beats_ai_title_and_summary() {
        // Tier 5 (UserRename): a `custom-title` record must overwrite both
        // earlier `summary` (Tier 3) and `ai-title` (Tier 4) records, and
        // the provenance must be `UserRename` so rename-detection logs fire.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"first prompt"}}"#,
            "\n",
            r#"{"type":"summary","summary":"older summary"}"#,
            "\n",
            r#"{"type":"ai-title","aiTitle":"AI guess"}"#,
            "\n",
            r#"{"type":"custom-title","customTitle":"Q2 planning notes"}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("Q2 planning notes"));
        assert_eq!(metadata.title_source, Some(TitleSource::UserRename));
    }

    #[test]
    fn test_title_ai_title_provenance_is_ai_generated() {
        // Tier 4 → AiGenerated, not UserRename — rename-detection log must
        // ignore this transition even when the title text changes.
        let content =
            r#"{"type":"ai-title","aiTitle":"Display organization name instead of UUID"}"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title_source, Some(TitleSource::AiGenerated));
    }

    #[test]
    fn test_title_latest_ai_title_wins() {
        // Claude Code refines ai-title across multiple records. The latest one
        // is the most accurate; later records must overwrite earlier ones.
        let content = concat!(
            r#"{"type":"ai-title","sessionId":"abc","aiTitle":"first draft title"}"#,
            "\n",
            r#"{"type":"ai-title","sessionId":"abc","aiTitle":"refined final title"}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("refined final title"));
    }

    #[test]
    fn test_title_from_antigravity_cascade_user_response() {
        // Real Antigravity cascade shape: a single
        // multi-line JSON document with the user prompt buried under
        // /steps/steps/0/userInput/userResponse. Title should be that text.
        let content = r#"{
          "sessionId": "e3a18420-3212-4df4-a730-0f3a4471e96a",
          "cascadeId": "e3a18420-3212-4df4-a730-0f3a4471e96a",
          "source": "antigravity_api",
          "baseUri": {"path": "file:///Users/x/repo"},
          "steps": {
            "steps": [
              {
                "type": "CORTEX_STEP_TYPE_USER_INPUT",
                "status": "ok",
                "userInput": {
                  "userResponse": "antigravity gemini sessions are not showing in list of sessions",
                  "items": [{"text": "antigravity gemini sessions are not showing in list of sessions"}]
                }
              }
            ]
          }
        }"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(
            metadata.title.as_deref(),
            Some("antigravity gemini sessions are not showing in list of sessions")
        );
    }

    #[test]
    fn test_title_from_cascade_skips_steps_without_user_input() {
        // Earlier steps may carry no user input (e.g. a pure tool step).
        // Walk forward to find the first step whose userInput has text.
        let content = r#"{
          "sessionId": "abc",
          "steps": {
            "steps": [
              {"type": "CORTEX_STEP_TYPE_TOOL", "status": "ok"},
              {"type": "CORTEX_STEP_TYPE_USER_INPUT", "userInput": {"userResponse": "the real first prompt"}}
            ]
          }
        }"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("the real first prompt"));
    }

    #[test]
    fn test_title_from_cascade_falls_back_to_items_text() {
        // Some steps only carry items[].text without a userResponse field.
        let content = r#"{
          "sessionId": "abc",
          "steps": {
            "steps": [
              {"userInput": {"items": [{"text": "items-only prompt"}]}}
            ]
          }
        }"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("items-only prompt"));
    }

    #[test]
    fn test_title_from_flattened_cascade_steps() {
        // Defensive: if Antigravity ever flattens to a single `steps` array
        // (no inner `steps` wrapper), the same probe still finds the prompt.
        let content = r#"{
          "sessionId": "abc",
          "steps": [
            {"userInput": {"userResponse": "flattened prompt"}}
          ]
        }"#;
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("flattened prompt"));
    }

    #[test]
    fn test_title_ai_title_with_blank_value_is_ignored() {
        // Defensive: an empty/whitespace aiTitle string must not clobber a
        // useful lower-tier title.
        let content = concat!(
            r#"{"type":"user","message":{"role":"user","content":"the real prompt"}}"#,
            "\n",
            r#"{"type":"ai-title","sessionId":"abc","aiTitle":"   "}"#,
        );
        let metadata = parse_session_metadata_str(content);
        assert_eq!(metadata.title.as_deref(), Some("the real prompt"));
    }

    #[tokio::test]
    async fn test_parse_metadata_codex_session_meta() {
        let dir = TempDir::new().unwrap();
        // Real Codex session_meta format: id and cwd nested under payload
        let content = r#"{"timestamp":"2026-02-10T01:52:21.832Z","type":"session_meta","payload":{"id":"019c453f-e731-7bd1-9d05-c571ec59ca6b","cwd":"/Users/foo/dev/my-project","originator":"codex_cli_rs","cli_version":"0.98.0"}}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(
            metadata.session_id,
            Some("019c453f-e731-7bd1-9d05-c571ec59ca6b".to_string())
        );
        assert_eq!(metadata.cwd, Some("/Users/foo/dev/my-project".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_codex_payload_id_only() {
        let dir = TempDir::new().unwrap();
        // Only payload.id present, no cwd
        let content = r#"{"type":"session_meta","payload":{"id":"abc-123"}}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("abc-123".to_string()));
        assert_eq!(metadata.cwd, None);
    }

    #[tokio::test]
    async fn test_parse_metadata_codex_payload_cwd_only() {
        let dir = TempDir::new().unwrap();
        // Only payload.cwd present, no id
        let content = r#"{"type":"session_meta","payload":{"cwd":"/Users/foo/bar"}}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, None);
        assert_eq!(metadata.cwd, Some("/Users/foo/bar".to_string()));
    }

    #[tokio::test]
    async fn test_parse_metadata_last_cwd_across_lines_wins_over_earlier_top_level() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"session_id":"top-level-id","cwd":"/top/level"}
{"type":"session_meta","payload":{"id":"payload-id","cwd":"/payload/path"}}
"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let metadata = parse_session_metadata(&file).await;

        assert_eq!(metadata.session_id, Some("top-level-id".to_string()));
        assert_eq!(metadata.cwd, Some("/payload/path".to_string()));
    }

    #[tokio::test]
    async fn test_session_time_range_parses_rfc3339() {
        let dir = TempDir::new().unwrap();
        let t1 = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let t2 = OffsetDateTime::from_unix_timestamp(1_700_000_120).unwrap();
        let s1 = t1.format(&Rfc3339).unwrap();
        let s2 = t2.format(&Rfc3339).unwrap();

        let content = format!(
            r#"{{"timestamp":"{s1}"}}
{{"payload":{{"createdAt":"{s2}"}}}}"#,
        );
        let file = write_temp_file(dir.path(), "session.jsonl", &content).await;

        let range = session_time_range(&file).await.unwrap();
        assert_eq!(range.0, t1.unix_timestamp());
        assert_eq!(range.1, t2.unix_timestamp());
    }

    #[tokio::test]
    async fn test_session_time_range_numeric_ms() {
        let dir = TempDir::new().unwrap();
        let content = r#"{
  "creationDate": 1749509938455,
  "lastMessageDate": 1749509971642
}"#;
        let file = write_temp_file(dir.path(), "session.json", content).await;

        let range = session_time_range(&file).await.unwrap();
        assert_eq!(range.0, 1_749_509_938);
        assert_eq!(range.1, 1_749_509_971);
    }

    #[test]
    fn test_session_time_range_str_parses() {
        let content = r#"{"timestamp":"2026-02-10T01:52:21Z"}"#;
        let range = session_time_range_str(content).unwrap();
        assert_eq!(range.0, range.1);
    }

    #[tokio::test]
    async fn test_session_time_range_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"type":"message","content":"no timestamps"}"#;
        let file = write_temp_file(dir.path(), "session.jsonl", content).await;

        let range = session_time_range(&file).await;
        assert!(range.is_none());
    }

    #[test]
    fn test_extract_candidate_cwds_from_tool_use_file_paths() {
        let content = [
            r#"{"type":"tool_use","name":"Read","input":{"file_path":"/Users/foo/code/my-repo/src/main.rs"}}"#,
            r#"{"type":"tool_use","name":"Write","input":{"file_path":"/Users/foo/code/my-repo/README.md"}}"#,
            r#"{"type":"tool_use","name":"Edit","input":{"file_path":"/Users/foo/code/other-repo/lib.rs"}}"#,
        ]
        .join("\n");

        let candidates = extract_candidate_cwds_from_transcript(&content);

        assert!(candidates.contains(&"/Users/foo/code/my-repo/src".to_string()));
        assert!(candidates.contains(&"/Users/foo/code/my-repo".to_string()));
        assert!(candidates.contains(&"/Users/foo/code/other-repo".to_string()));
    }

    #[test]
    fn test_extract_candidate_cwds_from_cd_commands() {
        let content = r#"{"type":"tool_use","name":"Bash","input":{"command":"cd /Users/foo/code/my-repo && cargo build"}}"#;

        let candidates = extract_candidate_cwds_from_transcript(content);

        assert!(candidates.contains(&"/Users/foo/code/my-repo".to_string()));
    }

    #[test]
    fn test_extract_candidate_cwds_deduplicates() {
        let content = [
            r#"{"type":"tool_use","name":"Read","input":{"file_path":"/Users/foo/repo/src/a.rs"}}"#,
            r#"{"type":"tool_use","name":"Read","input":{"file_path":"/Users/foo/repo/src/b.rs"}}"#,
        ]
        .join("\n");

        let candidates = extract_candidate_cwds_from_transcript(&content);

        // /Users/foo/repo/src should appear only once.
        let count = candidates
            .iter()
            .filter(|c| *c == "/Users/foo/repo/src")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_extract_candidate_cwds_ignores_relative_paths() {
        let content = r#"{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}"#;

        let candidates = extract_candidate_cwds_from_transcript(content);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_extract_candidate_cwds_skips_output_fields() {
        // Paths in "output" or "result" fields should not be extracted.
        let content =
            r#"{"type":"tool_result","output":"/Users/foo/code/some-repo/build/output.log"}"#;

        let candidates = extract_candidate_cwds_from_transcript(content);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_extract_candidate_cwds_handles_directory_paths() {
        let content =
            r#"{"type":"tool_use","name":"Glob","input":{"directory":"/Users/foo/code/my-repo"}}"#;

        let candidates = extract_candidate_cwds_from_transcript(content);
        assert!(candidates.contains(&"/Users/foo/code/my-repo".to_string()));
    }

    #[test]
    fn test_extract_candidate_cwds_cd_with_semicolons_and_chained_commands() {
        // Ensure cd targets are extracted cleanly from complex shell commands
        // without picking up non-path fragments.
        let content = r#"{"type":"tool_use","name":"Bash","input":{"command":"cd /Users/foo/repo1 && cargo test; cd /Users/foo/repo2 && make"}}"#;

        let candidates = extract_candidate_cwds_from_transcript(content);

        assert!(candidates.contains(&"/Users/foo/repo1".to_string()));
        assert!(candidates.contains(&"/Users/foo/repo2".to_string()));
        // Should NOT contain fragments like "/Users/foo/repo1 && cargo test"
        assert!(!candidates.iter().any(|c| c.contains("&&")));
        assert!(!candidates.iter().any(|c| c.contains(";")));
    }

    #[test]
    fn test_extract_candidate_cwds_empty_transcript() {
        let candidates = extract_candidate_cwds_from_transcript("");
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_extract_candidate_cwds_malformed_json_lines() {
        let content = "not json at all\n{invalid json}\n{\"valid\":\"but no paths\"}";
        let candidates = extract_candidate_cwds_from_transcript(content);
        assert!(candidates.is_empty());
    }
}
