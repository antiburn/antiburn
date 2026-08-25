// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Initial context token source attribution.
//!
//! Computes, locally and live, where a session's *initial* context window went
//! — the tokens loaded before the first model response — broken down by source
//! dimension.
//!
//! This runs as a separate pass over the **raw transcript** (not the normalized
//! [`crate::analysis::model`] stream, which discards the per-source text this needs). It
//! is best-effort: agent-specific parsing stays isolated, and a shared
//! normalizer assigns any unknown remainder to [`InitialContextTokenSource::Unattributed`].
//!
//! Supported agents today: Claude Code and Codex (both delivered as JSONL by the
//! app). Everything else returns `None` ("unavailable"); no misleading zero rows.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Source dimension for tokens loaded before a coding agent's first response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InitialContextTokenSource {
    /// User or repository-authored instructions, such as AGENTS.md or CLAUDE.md.
    AgentInstructions,
    /// Coding-agent harness instructions: system prompts, internal context, etc.
    SystemInstructions,
    /// Skill catalog entries or loaded skill instructions.
    SkillInstructions,
    /// MCP server/tool instructions.
    McpInstructions,
    /// Known initial-context tokens that cannot be attributed to another source.
    Unattributed,
}

impl InitialContextTokenSource {
    /// Stable string key, used verbatim in [`InitialContextSourceCount::source`].
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentInstructions => "agent_instructions",
            Self::SystemInstructions => "system_instructions",
            Self::SkillInstructions => "skill_instructions",
            Self::McpInstructions => "mcp_instructions",
            Self::Unattributed => "unattributed",
        }
    }
}

/// How completely the initial context could be attributed for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackingStatus {
    /// Every known initial-context token was attributed to a concrete source.
    Tracked,
    /// Some tokens are known to be present but couldn't be attributed
    /// (surfaced as an `unattributed` source).
    TrackedPartial,
}

/// One source/source-name token count in the public breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialContextSourceCount {
    /// Stable source-dimension key (see [`InitialContextTokenSource::as_str`]).
    pub source: String,
    /// Optional source name, such as a skill name or MCP server name.
    pub source_name: Option<String>,
    pub token_count: u64,
}

/// The per-session initial-context breakdown surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialContextBreakdown {
    pub tracking_status: TrackingStatus,
    /// Exact or estimated total initial input context size, when known.
    pub total_tokens: Option<u64>,
    pub sources: Vec<InitialContextSourceCount>,
}

/// Parse a raw transcript into a public initial-context breakdown, or `None`
/// when the agent/session has no reliable signal ("unavailable").
pub fn parse_initial_context(agent: &str, payload: &str) -> Option<InitialContextBreakdown> {
    if agent.eq_ignore_ascii_case("claude") {
        return parse_claude(payload);
    }
    let result = match agent.to_ascii_lowercase().as_str() {
        "codex" => parse_codex(payload),
        _ => InitialContextTokenParseResult::Unsupported,
    };
    match result {
        InitialContextTokenParseResult::Unsupported => None,
        InitialContextTokenParseResult::Supported(breakdown) => Some(to_output(breakdown)),
    }
}

/// Parse a `name → one-line description` map from a raw transcript's skill
/// listing, for grafting onto [`crate::analysis::SkillUse::description`].
///
/// Reuses the exact bullet format the initial-context attribution already walks —
/// Claude's `skill_listing` attachment and Codex's `## Skills` developer-prompt
/// section, both `"- <name>: <description>"`. Best-effort and side-effect-free:
/// returns an empty map for agents/sessions without a skill listing. First-seen
/// description wins for a repeated name.
pub fn parse_skill_descriptions(agent: &str, payload: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    match agent.to_ascii_lowercase().as_str() {
        "claude" => collect_claude_skill_descriptions(payload, &mut out),
        "codex" => collect_codex_skill_descriptions(payload, &mut out),
        _ => {}
    }
    out
}

fn collect_claude_skill_descriptions(payload: &str, out: &mut HashMap<String, String>) {
    let mut accumulator = ClaudeContextAccumulator::default();
    for value in parse_json_lines(payload) {
        accumulator.observe(&value);
    }
    let (_, descriptions) = accumulator.finish();
    out.extend(descriptions);
}

fn collect_codex_skill_descriptions(payload: &str, out: &mut HashMap<String, String>) {
    for value in parse_json_lines(payload) {
        if value.get("type").and_then(Value::as_str) != Some("response_item")
            || value.pointer("/payload/type").and_then(Value::as_str) != Some("message")
        {
            continue;
        }
        let role = value
            .pointer("/payload/role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if role != "developer" && role != "system" {
            continue;
        }
        let text = extract_codex_message_text(&value);
        if let Some((start, end)) = section_bounds(&text, "## Skills") {
            insert_bullet_descriptions(&text[start..end], out);
        }
    }
}

fn insert_bullet_descriptions(text: &str, out: &mut HashMap<String, String>) {
    for line in text.lines() {
        if let Some((name, description)) = parse_markdown_bullet(line)
            && !description.is_empty()
        {
            out.entry(name).or_insert(description);
        }
    }
}

fn to_output(breakdown: InitialContextTokenBreakdown) -> InitialContextBreakdown {
    let mut partial = false;
    let sources = breakdown
        .rows
        .into_iter()
        .filter(|row| row.token_count > 0)
        .map(|row| {
            if row.source == InitialContextTokenSource::Unattributed {
                partial = true;
            }
            InitialContextSourceCount {
                source: row.source.as_str().to_string(),
                source_name: row.source_name,
                token_count: row.token_count.max(0) as u64,
            }
        })
        .collect();
    InitialContextBreakdown {
        tracking_status: if partial {
            TrackingStatus::TrackedPartial
        } else {
            TrackingStatus::Tracked
        },
        total_tokens: breakdown.total_tokens.map(|t| t.max(0) as u64),
        sources,
    }
}

enum InitialContextTokenParseResult {
    /// No reliable initial-context signal for this agent/session.
    Unsupported,
    /// Some reliable initial-context signal.
    Supported(InitialContextTokenBreakdown),
}

struct InitialContextTokenBreakdown {
    total_tokens: Option<i64>,
    rows: Vec<InitialContextTokenSourceCount>,
}

struct InitialContextTokenSourceCount {
    source: InitialContextTokenSource,
    source_name: Option<String>,
    token_count: i64,
}

fn parse_codex(payload: &str) -> InitialContextTokenParseResult {
    let mut total_tokens = None;
    let mut system_instruction_texts = Vec::new();
    let mut source_rows = Vec::new();

    for value in parse_json_lines(payload) {
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "session_meta" => {
                if let Some(text) = value
                    .pointer("/payload/base_instructions/text")
                    .and_then(Value::as_str)
                {
                    system_instruction_texts.push(text.to_string());
                }
            }
            "response_item" => {
                if value.pointer("/payload/type").and_then(Value::as_str) != Some("message") {
                    continue;
                }
                let role = value
                    .pointer("/payload/role")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if role != "developer" && role != "system" {
                    continue;
                }
                let text = extract_codex_message_text(&value);
                if text.is_empty() {
                    continue;
                }
                let (without_skills, skill_rows) = parse_codex_developer_prompt(&text);
                let (without_agent_files, agent_rows) =
                    parse_instruction_file_mentions(&without_skills);
                source_rows.extend(agent_rows);
                if !without_agent_files.trim().is_empty() {
                    system_instruction_texts.push(without_agent_files);
                }
                source_rows.extend(skill_rows);
            }
            "event_msg"
                if total_tokens.is_none()
                    && value.pointer("/payload/type").and_then(Value::as_str)
                        == Some("token_count") =>
            {
                total_tokens = value
                    .pointer("/payload/info/total_token_usage/input_tokens")
                    .and_then(Value::as_i64);
            }
            _ => {}
        }
    }

    if system_instruction_texts.is_empty() && source_rows.is_empty() && total_tokens.is_none() {
        return InitialContextTokenParseResult::Unsupported;
    }

    let system_instruction_tokens = estimate_tokens(&system_instruction_texts.join("\n\n"));
    if system_instruction_tokens > 0 {
        source_rows.push(InitialContextTokenSourceCount {
            source: InitialContextTokenSource::SystemInstructions,
            source_name: None,
            token_count: system_instruction_tokens,
        });
    }

    InitialContextTokenParseResult::Supported(normalize_breakdown(total_tokens, source_rows))
}

#[derive(Default)]
pub(crate) struct ClaudeContextAccumulator {
    total_tokens: Option<i64>,
    system_instruction_texts: Vec<String>,
    source_rows: Vec<InitialContextTokenSourceCount>,
    seen_first_assistant: bool,
    skill_descriptions: HashMap<String, String>,
}

impl ClaudeContextAccumulator {
    pub(crate) fn observe(&mut self, value: &Value) {
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "assistant" if !self.seen_first_assistant => {
                self.total_tokens = first_claude_usage_total(value);
                self.seen_first_assistant = true;
            }
            "user" | "human" => {
                let is_meta = value
                    .get("isMeta")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !is_meta {
                    return;
                }
                let text = extract_claude_message_text(value);
                if text.contains("Base directory for this skill:") {
                    self.source_rows.push(InitialContextTokenSourceCount {
                        source: InitialContextTokenSource::SkillInstructions,
                        source_name: parse_claude_loaded_skill_name(&text),
                        token_count: estimate_tokens(&text),
                    });
                } else if !text.trim().is_empty() {
                    let (without_agent_files, agent_rows) = parse_instruction_file_mentions(&text);
                    self.source_rows.extend(agent_rows);
                    if !self.seen_first_assistant && !without_agent_files.trim().is_empty() {
                        self.system_instruction_texts.push(without_agent_files);
                    }
                }
            }
            "attachment" => {
                let Some(attachment) = value.get("attachment") else {
                    return;
                };
                match attachment
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "skill_listing" => {
                        if let Some(content) = attachment.get("content").and_then(Value::as_str) {
                            self.source_rows.extend(parse_named_markdown_bullets(
                                content,
                                InitialContextTokenSource::SkillInstructions,
                            ));
                            insert_bullet_descriptions(content, &mut self.skill_descriptions);
                        }
                    }
                    "mcp_instructions_delta" => {
                        let names = attachment
                            .get("addedNames")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let blocks = attachment
                            .get("addedBlocks")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        for (index, block) in blocks.iter().enumerate() {
                            let Some(text) = block.as_str() else {
                                continue;
                            };
                            self.source_rows.push(InitialContextTokenSourceCount {
                                source: InitialContextTokenSource::McpInstructions,
                                source_name: names
                                    .get(index)
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                token_count: estimate_tokens(text),
                            });
                        }
                    }
                    "deferred_tools_delta"
                    | "command_permissions"
                    | "auto_mode"
                    | "diagnostics" => {
                        let text = serde_json::to_string(attachment).unwrap_or_default();
                        if !self.seen_first_assistant && !text.is_empty() {
                            self.system_instruction_texts.push(text);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub(crate) fn finish(mut self) -> (Option<InitialContextBreakdown>, HashMap<String, String>) {
        if self.system_instruction_texts.is_empty()
            && self.source_rows.is_empty()
            && self.total_tokens.is_none()
        {
            return (None, self.skill_descriptions);
        }
        let system_instruction_tokens =
            estimate_tokens(&self.system_instruction_texts.join("\n\n"));
        if system_instruction_tokens > 0 {
            self.source_rows.push(InitialContextTokenSourceCount {
                source: InitialContextTokenSource::SystemInstructions,
                source_name: None,
                token_count: system_instruction_tokens,
            });
        }
        let breakdown = normalize_breakdown(self.total_tokens, self.source_rows);
        (Some(to_output(breakdown)), self.skill_descriptions)
    }
}

fn parse_claude(payload: &str) -> Option<InitialContextBreakdown> {
    let mut accumulator = ClaudeContextAccumulator::default();
    for value in parse_json_lines(payload) {
        accumulator.observe(&value);
    }
    accumulator.finish().0
}

fn normalize_breakdown(
    total_tokens: Option<i64>,
    mut rows: Vec<InitialContextTokenSourceCount>,
) -> InitialContextTokenBreakdown {
    rows.retain(|row| row.token_count >= 0);
    merge_rows(&mut rows);

    if let Some(total_tokens) = total_tokens {
        // `known_tokens` is a sum of chars/4 *estimates*, so it can exceed the
        // reported `total_tokens` and clamp `unattributed` to 0. That's intentional:
        // we don't shrink the per-source estimates to fit. The frontend donut shows
        // a center of `max(total, sliceTotal)` so the displayed center is never less
        // than the slices even when the estimates overshoot.
        let known_tokens = rows.iter().map(|row| row.token_count).sum::<i64>();
        let unattributed = (total_tokens - known_tokens).max(0);
        if unattributed > 0 {
            rows.push(InitialContextTokenSourceCount {
                source: InitialContextTokenSource::Unattributed,
                source_name: None,
                token_count: unattributed,
            });
        }
    }

    InitialContextTokenBreakdown { total_tokens, rows }
}

/// Sum rows that share a `(source, source_name)` key, preserving first-seen
/// order. An index map keeps this O(n) rather than the prior per-row linear scan.
fn merge_rows(rows: &mut Vec<InitialContextTokenSourceCount>) {
    let mut merged: Vec<InitialContextTokenSourceCount> = Vec::new();
    let mut index: HashMap<(InitialContextTokenSource, Option<String>), usize> = HashMap::new();
    for row in rows.drain(..) {
        let key = (row.source, row.source_name.clone());
        match index.get(&key) {
            Some(&i) => merged[i].token_count += row.token_count,
            None => {
                index.insert(key, merged.len());
                merged.push(row);
            }
        }
    }
    *rows = merged;
}

fn parse_json_lines(payload: &str) -> Vec<Value> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Array(values)) => return values,
        Ok(value) => return vec![value],
        Err(_) => {}
    }

    payload
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

fn extract_codex_message_text(value: &Value) -> String {
    value
        .pointer("/payload/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_claude_message_text(value: &Value) -> String {
    let Some(content) = value.pointer("/message/content") else {
        return String::new();
    };
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn first_claude_usage_total(value: &Value) -> Option<i64> {
    let usage = value
        .get("usage")
        .or_else(|| value.pointer("/message/usage"))?;
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total = input + cache_creation + cache_read;
    (total > 0).then_some(total)
}

fn parse_codex_developer_prompt(text: &str) -> (String, Vec<InitialContextTokenSourceCount>) {
    let Some((skills_start, skills_end)) = section_bounds(text, "## Skills") else {
        return (text.to_string(), Vec::new());
    };
    let skills_section = &text[skills_start..skills_end];
    let mut without_skills = String::with_capacity(text.len() - skills_section.len());
    without_skills.push_str(&text[..skills_start]);
    without_skills.push_str(&text[skills_end..]);
    (
        without_skills,
        parse_named_markdown_bullets(skills_section, InitialContextTokenSource::SkillInstructions),
    )
}

fn parse_named_markdown_bullets(
    text: &str,
    source: InitialContextTokenSource,
) -> Vec<InitialContextTokenSourceCount> {
    let mut rows = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_text = String::new();

    for line in text.lines() {
        if let Some(name) = parse_markdown_bullet_name(line) {
            push_named_source(&mut rows, source, current_name.take(), &current_text);
            current_name = Some(name);
            current_text.clear();
        }
        if current_name.is_some() {
            current_text.push_str(line);
            current_text.push('\n');
        }
    }
    push_named_source(&mut rows, source, current_name, &current_text);
    rows
}

fn parse_markdown_bullet_name(line: &str) -> Option<String> {
    parse_markdown_bullet(line).map(|(name, _)| name)
}

/// Parse a `"- <name>: <description>"` skill-listing bullet into its name and the
/// post-colon description text (trimmed). The name half keeps the original
/// validation (single token, backtick-stripped); the description is everything
/// after the first colon, so it may itself contain colons. `None` for non-bullet
/// lines or a multi-word/empty name.
fn parse_markdown_bullet(line: &str) -> Option<(String, String)> {
    let line = line.trim_start();
    let rest = line.strip_prefix("- ")?;
    let (name, description) = rest.split_once(':')?;
    let name = name.trim().trim_matches('`');
    if name.is_empty() || name.contains(' ') {
        None
    } else {
        Some((name.to_string(), description.trim().to_string()))
    }
}

fn push_named_source(
    rows: &mut Vec<InitialContextTokenSourceCount>,
    source: InitialContextTokenSource,
    source_name: Option<String>,
    text: &str,
) {
    let token_count = estimate_tokens(text);
    if token_count == 0 {
        return;
    }
    rows.push(InitialContextTokenSourceCount {
        source,
        source_name,
        token_count,
    });
}

fn parse_claude_loaded_skill_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("Base directory for this skill: "))
        .and_then(|path| path.rsplit('/').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn parse_instruction_file_mentions(text: &str) -> (String, Vec<InitialContextTokenSourceCount>) {
    let mut remainder = Vec::new();
    let mut rows = Vec::new();

    for line in text.lines() {
        if let Some(file_name) = instruction_file_name(line) {
            push_named_source(
                &mut rows,
                InitialContextTokenSource::AgentInstructions,
                Some(file_name.to_string()),
                line,
            );
        } else {
            remainder.push(line);
        }
    }

    (remainder.join("\n"), rows)
}

fn instruction_file_name(text: &str) -> Option<&'static str> {
    const INSTRUCTION_FILES: [&str; 4] = ["AGENTS.md", "CLAUDE.md", "GEMINI.md", ".cursorrules"];

    INSTRUCTION_FILES.into_iter().find(|file_name| {
        // Only attribute when the filename appears as a path basename — at line
        // start or immediately after a `/` (the `Instructions from /workspace/
        // AGENTS.md:` framing) — not as a space-preceded word in prose like
        // "edit CLAUDE.md", which would otherwise reclassify a System mention as
        // an Agent-file instruction.
        text.match_indices(file_name)
            .any(|(idx, _)| idx == 0 || text.as_bytes().get(idx - 1) == Some(&b'/'))
    })
}

fn section_bounds(text: &str, heading: &str) -> Option<(usize, usize)> {
    let start = text.find(heading)?;
    let after_heading = start + heading.len();
    let rest = &text[after_heading..];
    // A section runs until the next `## ` heading. XML-wrapped blocks (Codex's
    // `<skills_instructions>…</skills_instructions>`) have no trailing heading, so
    // also stop at a line-start closing tag — taking whichever terminator comes
    // first — to avoid swallowing the tag and the prose after it.
    let next_heading = rest.find("\n## ").map(|offset| after_heading + offset + 1);
    let closing_tag = rest.find("\n</").map(|offset| after_heading + offset + 1);
    let end = [next_heading, closing_tag]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(text.len());
    Some((start, end))
}

/// Rough token estimate (chars / 4). Deliberately tokenizer-free: the breakdown
/// is a proportional attribution, not an exact count, and every source is
/// estimated the same way so the slices stay comparable to one another.
fn estimate_tokens(text: &str) -> i64 {
    let chars = text.chars().count() as i64;
    if chars == 0 { 0 } else { (chars + 3) / 4 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_tokens(
        breakdown: &InitialContextBreakdown,
        source: InitialContextTokenSource,
        source_name: Option<&str>,
    ) -> u64 {
        breakdown
            .sources
            .iter()
            .filter(|row| {
                row.source == source.as_str() && row.source_name.as_deref() == source_name
            })
            .map(|row| row.token_count)
            .sum()
    }

    #[test]
    fn codex_extracts_system_instructions_named_skills_and_remainder() {
        let payload = include_str!("../../tests/fixtures/initial_context/codex_realistic.jsonl");
        let breakdown =
            parse_initial_context("codex", payload).expect("expected supported Codex breakdown");

        assert_eq!(breakdown.total_tokens, Some(18_764));
        assert_eq!(breakdown.tracking_status, TrackingStatus::TrackedPartial);
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SystemInstructions,
                None
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SkillInstructions,
                Some("orbit-tracker")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SkillInstructions,
                Some("atlas-notes")
            ) > 0
        );
        assert!(source_tokens(&breakdown, InitialContextTokenSource::Unattributed, None) > 0);
    }

    #[test]
    fn codex_reserves_agent_instructions_for_instruction_files() {
        let payload = r#"{"type":"session_meta","payload":{"base_instructions":{"text":"You are Codex, a coding agent."}}}
{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>Sandbox details omitted.</permissions instructions>\nInstructions from /workspace/AGENTS.md: follow repo guidance.\nInstructions from /workspace/CLAUDE.md: follow Claude guidance."}]}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000}}}}"#;

        let breakdown =
            parse_initial_context("codex", payload).expect("expected supported Codex breakdown");

        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SystemInstructions,
                None
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::AgentInstructions,
                Some("AGENTS.md")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::AgentInstructions,
                Some("CLAUDE.md")
            ) > 0
        );
    }

    #[test]
    fn claude_extracts_named_loaded_skill_mcp_and_remainder() {
        let payload = include_str!("../../tests/fixtures/initial_context/claude_realistic.jsonl");
        let breakdown =
            parse_initial_context("claude", payload).expect("expected supported Claude breakdown");

        assert_eq!(breakdown.total_tokens, Some(84_321));
        assert_eq!(breakdown.tracking_status, TrackingStatus::TrackedPartial);
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SkillInstructions,
                Some("orbit-tracker")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SkillInstructions,
                Some("atlas-notes")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SkillInstructions,
                Some("ledger-sync")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::McpInstructions,
                Some("nebula-docs")
            ) > 0
        );
        assert!(source_tokens(&breakdown, InitialContextTokenSource::Unattributed, None) > 0);
    }

    #[test]
    fn instruction_file_name_requires_a_path_basename() {
        // Path basenames (line start or after `/`) are instruction files…
        assert_eq!(
            instruction_file_name("Instructions from /workspace/AGENTS.md: follow guidance."),
            Some("AGENTS.md")
        );
        assert_eq!(instruction_file_name("CLAUDE.md"), Some("CLAUDE.md"));
        // …but a space-preceded prose mention is not, so it stays System, not
        // Agent-file (F4a regression: `contains` reclassified prose like this).
        assert_eq!(
            instruction_file_name("Remember to edit CLAUDE.md first."),
            None
        );
        assert_eq!(instruction_file_name("see the AGENTS.md for details"), None);
    }

    #[test]
    fn section_bounds_stops_at_xml_closing_tag() {
        // A `## Skills` block closed by `</skills_instructions>` with no following
        // `## ` heading must end at the closing tag, not run to end-of-text and
        // swallow the tag + trailing prose (F4b).
        let text = "## Skills\n- a: one\n- b: two\n</skills_instructions>\ntrailing prose";
        let (start, end) = section_bounds(text, "## Skills").unwrap();
        let section = &text[start..end];
        assert!(section.contains("- a: one"));
        assert!(!section.contains("</skills_instructions>"));
        assert!(!section.contains("trailing prose"));
    }

    #[test]
    fn codex_skills_section_excludes_closing_tag() {
        // End-to-end against the realistic Codex fixture: the closing tag prose is
        // no longer absorbed into the skills section, so the named skill bullets
        // stay clean (orbit-tracker / atlas-notes) and nothing spurious is attributed.
        let payload = include_str!("../../tests/fixtures/initial_context/codex_realistic.jsonl");
        let breakdown =
            parse_initial_context("codex", payload).expect("expected supported Codex breakdown");
        let names: Vec<_> = breakdown
            .sources
            .iter()
            .filter(|r| r.source == InitialContextTokenSource::SkillInstructions.as_str())
            .filter_map(|r| r.source_name.as_deref())
            .collect();
        assert!(names.contains(&"orbit-tracker"));
        assert!(names.contains(&"atlas-notes"));
        // The closing tag is not a skill bullet name.
        assert!(!names.iter().any(|n| n.contains('<') || n.contains('>')));
    }

    #[test]
    fn claude_attributes_attachment_after_early_assistant_ack() {
        // An early assistant ack (carrying the initial-context usage) used to hard-
        // `break` parsing, dropping a skill-listing attachment that arrives right
        // after it. It must now still be attributed (F4c).
        let payload = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":5000}}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- deferred-skill: A skill listed after an early assistant ack."}}"#,
        );
        let breakdown =
            parse_initial_context("claude", payload).expect("expected supported Claude breakdown");
        assert_eq!(breakdown.total_tokens, Some(5000));
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::SkillInstructions,
                Some("deferred-skill")
            ) > 0
        );
    }

    #[test]
    fn parse_skill_descriptions_reads_claude_listing_and_codex_section() {
        // Claude `skill_listing` attachment bullets.
        let claude = concat!(
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- aside: Quick side question.\n- verify: Verification command."}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#,
        );
        let map = parse_skill_descriptions("claude", claude);
        assert_eq!(
            map.get("aside").map(String::as_str),
            Some("Quick side question.")
        );
        assert_eq!(
            map.get("verify").map(String::as_str),
            Some("Verification command.")
        );

        // Codex `## Skills` developer-prompt section, same bullet format.
        let codex = r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"intro\n## Skills\n- orbit-tracker: Compute satellite passes.\n- atlas-notes: Outline a notes directory.\n## Next"}]}}"#;
        let map = parse_skill_descriptions("codex", codex);
        assert_eq!(
            map.get("orbit-tracker").map(String::as_str),
            Some("Compute satellite passes.")
        );
        assert_eq!(
            map.get("atlas-notes").map(String::as_str),
            Some("Outline a notes directory.")
        );

        // Unknown agents and listing-free transcripts yield an empty map.
        assert!(parse_skill_descriptions("cursor", claude).is_empty());
        assert!(parse_skill_descriptions("claude", "{}").is_empty());
    }

    #[test]
    fn unsupported_agents_return_none() {
        let cursor = include_str!("../../tests/fixtures/initial_context/cursor_unsupported.jsonl");
        assert!(parse_initial_context("cursor", cursor).is_none());
        assert!(parse_initial_context("copilot", cursor).is_none());
        // Even a Claude-shaped payload is "unavailable" under an unknown label.
        assert!(parse_initial_context("windsurf", cursor).is_none());
    }
}
