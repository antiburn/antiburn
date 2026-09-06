//! Initial context token source attribution.
//!
//! Computes, locally and live, where a session's *initial* context window went
//! — the tokens loaded before the first model response — broken down by source
//! dimension.
//!
//! This runs as a separate pass over the **raw transcript** (not the normalized
//! [`crate::analysis::model`] stream, which discards the per-source text this needs). It
//! is best-effort: agent-specific parsing stays isolated.
//!
//! Supported agents today: Claude Code and Codex (both delivered as JSONL by the
//! app). Everything else returns `None` ("unavailable"). A supported agent whose
//! session has no skills or MCP servers still returns a breakdown, with empty
//! `sources`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::tool_catalog::{self, ToolCatalog};
use crate::model::skill::SkillUse;

/// Estimated token cost of the short name line the harness sends for a
/// deferred tool, in place of its full definition. This is an estimate, not a
/// measured value — a deferred tool never sends its real definition, so
/// there is nothing to measure per session.
const DEFERRED_TOOL_TOKEN_ESTIMATE: u32 = 5;

/// Source dimension for tokens loaded before a coding agent's first response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InitialContextTokenSource {
    /// Skill catalog entries or loaded skill instructions.
    Skill,
    /// MCP server/tool instructions.
    Mcp,
    /// A built-in tool's definition (`Bash`, `Read`, …), from the embedded
    /// tool catalogue.
    BuiltinTool,
}

impl InitialContextTokenSource {
    /// Stable string key, used verbatim in [`InitialContextSourceCount::source`].
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill_instructions",
            Self::Mcp => "mcp_instructions",
            Self::BuiltinTool => "builtin_tool",
        }
    }
}

/// Where a skill or MCP server is installed. `Unknown` when the transcript
/// and the local file system give no evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    /// Ships with the coding agent itself.
    Bundled,
    /// Installed through a plugin.
    Plugin,
    /// Installed for the current user, outside any project.
    User,
    /// Installed inside the current project (the session's working directory).
    Project,
    /// No evidence tells us where the source came from. For a Claude skill,
    /// this happens when the name matches neither the project nor the user
    /// directory and one of two things is true: the name carries a `:`, or
    /// the project probe could not run because the transcript's `cwd` does
    /// not exist on this machine. A bare name that both probes checked and
    /// found in neither directory resolves to `Bundled` instead: the harness
    /// only lists skills it found, and a plugin skill always carries a
    /// `<plugin>:` prefix, so a bare, unmatched name can only ship with the
    /// agent.
    #[default]
    Unknown,
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
    /// How many times the session used this source after it loaded. Skills count
    /// `Skill` tool calls with this name. MCP servers count tool calls whose name
    /// starts with `mcp__<server>__`. Always 0 for other sources.
    #[serde(default)]
    pub use_count: u32,
    /// Where this source is installed. Always [`SourceOrigin::Unknown`] for
    /// an MCP row, for now.
    #[serde(default)]
    pub origin: SourceOrigin,
    /// True for a `builtin_tool` row when the session deferred this tool
    /// (the harness sent only its name, not its full definition). Always
    /// `false` for a skill or MCP row. Omitted from JSON when `false`, so
    /// existing skill and MCP rows serialize unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deferred: bool,
    /// Extra raw names `fill_use_counts` also matches for a `builtin_tool`
    /// row's `use_count`, beyond `source_name` itself: the catalogue's
    /// canonical name, each alias, and each alias's last dot-segment. A
    /// harness can call one tool by more than one of these spellings across
    /// versions (Codex namespaces some tools, e.g. `functions.exec`, but
    /// calls them by their bare last segment, `exec`, in the transcript).
    /// Never serialized — this is an implementation detail of `use_count`,
    /// not a UI-facing value. Always empty for a skill or MCP row.
    #[serde(skip)]
    pub(crate) match_names: Vec<String>,
}

/// The per-session initial-context breakdown surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialContextBreakdown {
    pub sources: Vec<InitialContextSourceCount>,
}

/// Parse a raw transcript into a public initial-context breakdown, or `None`
/// when the agent/session has no reliable signal ("unavailable").
pub fn parse_initial_context(agent: &str, payload: &str) -> Option<InitialContextBreakdown> {
    parse_initial_context_with_catalog(agent, payload, tool_catalog::embedded())
}

/// Same as [`parse_initial_context`], but takes the built-in tool catalogue as
/// an argument so a test can supply a fixture catalogue instead of the one
/// embedded in the binary.
fn parse_initial_context_with_catalog(
    agent: &str,
    payload: &str,
    catalog: &ToolCatalog,
) -> Option<InitialContextBreakdown> {
    if agent.eq_ignore_ascii_case("claude") {
        return parse_claude(payload, catalog);
    }
    let result = match agent.to_ascii_lowercase().as_str() {
        "codex" => parse_codex(payload, catalog),
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
    let (_, descriptions) = accumulator.finish(tool_catalog::embedded());
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
        if let Some((name, description, _file_path)) = parse_markdown_bullet(line)
            && !description.is_empty()
        {
            out.entry(name).or_insert(description);
        }
    }
}

fn to_output(breakdown: InitialContextTokenBreakdown) -> InitialContextBreakdown {
    let sources = breakdown
        .rows
        .into_iter()
        .filter(|row| row.token_count > 0)
        .map(|row| InitialContextSourceCount {
            source: row.source.as_str().to_string(),
            source_name: row.source_name,
            token_count: row.token_count.max(0) as u64,
            // `analyze_sources` fills this from session tool-call metrics
            // after the breakdown is grafted onto `SessionMetrics`.
            use_count: 0,
            origin: row.origin,
            deferred: row.deferred,
            match_names: row.match_names,
        })
        .collect();
    InitialContextBreakdown { sources }
}

/// Fill `InitialContextSourceCount::use_count` on a breakdown's skill, MCP, and
/// built-in-tool rows, from the session's own tool-call counts. A skill row
/// counts entries in `skill_uses` whose name matches (case-insensitive); an MCP
/// row counts `mcp_tool_calls` for the matching server name (also
/// case-insensitive); a built-in-tool row sums `tool_calls_by_name` entries
/// whose raw name matches the row's displayed name or any of its
/// `match_names` (also case-insensitive) — a harness can call one tool by
/// more than one spelling across versions (its canonical catalogue name, an
/// alias, or an alias's bare last segment).
///
/// Callers run this once per session, over whichever `skill_uses`,
/// `mcp_tool_calls`, and `tool_calls_by_name` the same session's
/// `SessionMetrics` already computed — the streaming path
/// (`SessionMetricsAccumulator::metrics`) and the batch path
/// (`analyze_sources_with`) each call it so their results stay identical.
pub(crate) fn fill_use_counts(
    breakdown: &mut InitialContextBreakdown,
    skill_uses: &[SkillUse],
    mcp_tool_calls: &HashMap<String, u32>,
    tool_calls_by_name: &HashMap<String, u32>,
) {
    for row in &mut breakdown.sources {
        let Some(name) = row.source_name.as_deref() else {
            continue;
        };
        row.use_count = if row.source == InitialContextTokenSource::Skill.as_str() {
            skill_uses
                .iter()
                .filter(|skill_use| skill_use.name.eq_ignore_ascii_case(name))
                .count() as u32
        } else if row.source == InitialContextTokenSource::Mcp.as_str() {
            mcp_tool_calls
                .iter()
                .find(|(server, _)| server.eq_ignore_ascii_case(name))
                .map(|(_, count)| *count)
                .unwrap_or(0)
        } else if row.source == InitialContextTokenSource::BuiltinTool.as_str() {
            tool_calls_by_name
                .iter()
                .filter(|(call_name, _)| {
                    call_name.eq_ignore_ascii_case(name)
                        || row
                            .match_names
                            .iter()
                            .any(|candidate| candidate.eq_ignore_ascii_case(call_name))
                })
                .map(|(_, count)| *count)
                .sum()
        } else {
            0
        };
    }
}

enum InitialContextTokenParseResult {
    /// No reliable initial-context signal for this agent/session.
    Unsupported,
    /// Some reliable initial-context signal.
    Supported(InitialContextTokenBreakdown),
}

struct InitialContextTokenBreakdown {
    rows: Vec<InitialContextTokenSourceCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InitialContextTokenSourceCount {
    source: InitialContextTokenSource,
    source_name: Option<String>,
    token_count: i64,
    origin: SourceOrigin,
    /// See [`InitialContextSourceCount::deferred`]. Always `false` outside
    /// [`builtin_tool_rows`].
    deferred: bool,
    /// See [`InitialContextSourceCount::match_names`]. Always empty outside
    /// [`builtin_tool_rows`].
    match_names: Vec<String>,
}

fn parse_codex(payload: &str, catalog: &ToolCatalog) -> InitialContextTokenParseResult {
    let values = parse_json_lines(payload);
    // The session's cwd tells a project-scoped skill from a user-scoped one.
    // Read it up front so it is available no matter where a `## Skills`
    // bullet turns up relative to the `session_meta` record.
    let cwd = values.iter().find_map(|value| {
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            return None;
        }
        value
            .pointer("/payload/cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    // The Codex CLI version, from `session_meta`, and the model, from
    // `turn_context`. First-seen value wins for each — a session runs one
    // harness build throughout, and a mid-session model switch is rare
    // enough that "first" is as defensible a pick as any other.
    let cli_version = values.iter().find_map(|value| {
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            return None;
        }
        value
            .pointer("/payload/cli_version")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let model = values.iter().find_map(|value| {
        if value.get("type").and_then(Value::as_str) != Some("turn_context") {
            return None;
        }
        value
            .pointer("/payload/model")
            .and_then(Value::as_str)
            .map(str::to_string)
    });

    let mut source_rows = Vec::new();

    for value in values {
        if value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            != "response_item"
        {
            continue;
        }
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
        source_rows.extend(parse_codex_developer_prompt(&text, cwd.as_deref()));
    }

    // Codex carries no known deferred-tool marker, so every catalogued tool
    // counts as loaded in full.
    source_rows.extend(builtin_tool_rows(
        "codex",
        cli_version.as_deref(),
        model.as_deref(),
        &HashSet::new(),
        catalog,
    ));

    InitialContextTokenParseResult::Supported(normalize_breakdown(source_rows))
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CodexContextAccumulator {
    source_rows: Vec<InitialContextTokenSourceCount>,
    skill_descriptions: HashMap<String, String>,
    cwd: Option<String>,
    cli_version: Option<String>,
    model: Option<String>,
}

impl CodexContextAccumulator {
    pub(crate) fn observe(&mut self, value: &Value) {
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if self.cwd.is_none() {
                    self.cwd = value
                        .pointer("/payload/cwd")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                if self.cli_version.is_none() {
                    self.cli_version = value
                        .pointer("/payload/cli_version")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            Some("turn_context") if self.model.is_none() => {
                self.model = value
                    .pointer("/payload/model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("response_item")
                if value.pointer("/payload/type").and_then(Value::as_str) == Some("message") =>
            {
                let role = value.pointer("/payload/role").and_then(Value::as_str);
                if matches!(role, Some("developer" | "system")) {
                    let text = extract_codex_message_text(value);
                    self.source_rows
                        .extend(parse_codex_developer_prompt(&text, self.cwd.as_deref()));
                    if let Some((start, end)) = section_bounds(&text, "## Skills") {
                        insert_bullet_descriptions(&text[start..end], &mut self.skill_descriptions);
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn finish(mut self) -> (Option<InitialContextBreakdown>, HashMap<String, String>) {
        self.source_rows.extend(builtin_tool_rows(
            "codex",
            self.cli_version.as_deref(),
            self.model.as_deref(),
            &HashSet::new(),
            tool_catalog::embedded(),
        ));
        let breakdown = to_output(normalize_breakdown(self.source_rows));
        (Some(breakdown), self.skill_descriptions)
    }
}

/// How strongly a piece of evidence pins down a Claude skill's origin, lowest
/// number wins. Matches the priority order in the module doc: an
/// `invoked_skills` attachment beats a `dynamic_skill` attachment, which beats
/// the skill-expansion preamble path, which beats a plugin-qualified listing
/// name, which beats the filesystem probe (applied only when nothing else
/// answered, in [`ClaudeContextAccumulator::finish_with_probe`]).
mod claude_origin_rank {
    pub(super) const INVOKED_SKILLS: u8 = 0;
    pub(super) const DYNAMIC_SKILL: u8 = 1;
    pub(super) const PREAMBLE_PATH: u8 = 2;
    pub(super) const LISTING_NAME_SHAPE: u8 = 3;
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClaudeContextAccumulator {
    source_rows: Vec<InitialContextTokenSourceCount>,
    skill_descriptions: HashMap<String, String>,
    /// The session's own working directory, read from the `cwd` Claude
    /// records on every line. Used to tell a project-scoped skill from a
    /// user-scoped one.
    cwd: Option<String>,
    /// Best origin evidence seen so far for each skill name, applied in
    /// [`Self::finish_with_probe`] once every record has been observed —
    /// evidence for a listing row can arrive after the row itself.
    skill_origin_evidence: HashMap<String, (u8, SourceOrigin)>,
    /// The harness's own version, from the top-level `version` field Claude
    /// stamps on every record. First-seen value wins.
    harness_version: Option<String>,
    /// Frequency of each full model id seen on `message.model`, in first-seen
    /// order. A bare alias such as `sonnet` never enters this list — see
    /// [`Self::observe_model_id`] — so the catalogue lookup always resolves
    /// against a real model id when the transcript names one at all.
    model_frequency: Vec<(String, u32)>,
    /// Tool names the harness has deferred at least once this session,
    /// unioned from every `deferred_tools_delta` attachment.
    deferred_tools: HashSet<String>,
}

impl ClaudeContextAccumulator {
    pub(crate) fn observe(&mut self, value: &Value) {
        if self.cwd.is_none()
            && let Some(cwd) = value
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.is_empty())
        {
            self.cwd = Some(cwd.to_string());
        }
        if self.harness_version.is_none()
            && let Some(version) = value
                .get("version")
                .and_then(Value::as_str)
                .filter(|version| !version.is_empty())
        {
            self.harness_version = Some(version.to_string());
        }
        if let Some(model) = value.pointer("/message/model").and_then(Value::as_str) {
            self.observe_model_id(model);
        }
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
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
                    let name = parse_claude_loaded_skill_name(&text);
                    if let Some((name, path)) =
                        name.as_deref().zip(parse_claude_loaded_skill_path(&text))
                    {
                        let origin = classify_claude_filesystem_path(path, self.cwd.as_deref());
                        self.record_skill_origin(name, claude_origin_rank::PREAMBLE_PATH, origin);
                    }
                    self.source_rows.push(InitialContextTokenSourceCount {
                        source: InitialContextTokenSource::Skill,
                        source_name: name,
                        token_count: estimate_tokens(&text),
                        origin: SourceOrigin::Unknown,
                        deferred: false,
                        match_names: Vec::new(),
                    });
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
                            let rows = parse_named_markdown_bullets(
                                content,
                                InitialContextTokenSource::Skill,
                                None,
                            );
                            for row in &rows {
                                // A `<plugin>:<skill>` listing name is plugin
                                // evidence. A directory-scoped project skill
                                // also uses `<dir>:<skill>`, but a
                                // `dynamic_skill` attachment (Project, a
                                // stronger rank) always wins when both are
                                // present for the same name.
                                if let Some(name) = &row.source_name
                                    && name.contains(':')
                                {
                                    self.record_skill_origin(
                                        name,
                                        claude_origin_rank::LISTING_NAME_SHAPE,
                                        SourceOrigin::Plugin,
                                    );
                                }
                            }
                            self.source_rows.extend(rows);
                            insert_bullet_descriptions(content, &mut self.skill_descriptions);
                        }
                    }
                    "invoked_skills" => {
                        let skills = attachment
                            .get("skills")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        for skill in &skills {
                            let (Some(name), Some(path)) = (
                                skill.get("name").and_then(Value::as_str),
                                skill.get("path").and_then(Value::as_str),
                            ) else {
                                continue;
                            };
                            let origin =
                                classify_claude_invoked_skill_path(path, self.cwd.as_deref());
                            self.record_skill_origin(
                                name,
                                claude_origin_rank::INVOKED_SKILLS,
                                origin,
                            );
                        }
                    }
                    "dynamic_skill" => {
                        let names = attachment
                            .get("skillNames")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        for name in names.iter().filter_map(Value::as_str) {
                            self.record_skill_origin(
                                name,
                                claude_origin_rank::DYNAMIC_SKILL,
                                SourceOrigin::Project,
                            );
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
                                source: InitialContextTokenSource::Mcp,
                                source_name: names
                                    .get(index)
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                token_count: estimate_tokens(text),
                                origin: SourceOrigin::Unknown,
                                deferred: false,
                                match_names: Vec::new(),
                            });
                        }
                    }
                    "deferred_tools_delta" => {
                        let names = attachment
                            .get("addedNames")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        for name in names.iter().filter_map(Value::as_str) {
                            self.deferred_tools.insert(name.to_string());
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Record `origin` for `name` only if it out-ranks (or is the first) the
    /// evidence already on file — a lower `rank` wins. See
    /// [`claude_origin_rank`] for the priority order.
    ///
    /// A [`SourceOrigin::Unknown`] origin is not evidence: the caller could
    /// not classify its input. It is dropped here so that weaker evidence and
    /// the filesystem probe still run for this name.
    fn record_skill_origin(&mut self, name: &str, rank: u8, origin: SourceOrigin) {
        if origin == SourceOrigin::Unknown {
            return;
        }
        match self.skill_origin_evidence.get(name) {
            Some((existing_rank, _)) if *existing_rank <= rank => {}
            _ => {
                self.skill_origin_evidence
                    .insert(name.to_string(), (rank, origin));
            }
        }
    }

    /// Count one sighting of a `message.model` value. A bare alias (`sonnet`,
    /// `opus`, `haiku`) carries no hyphen and never resolves against the tool
    /// catalogue, so it is dropped here rather than competing with a full id
    /// for "most frequent".
    fn observe_model_id(&mut self, model: &str) {
        let model = model.trim();
        if model.is_empty() || !model.contains('-') {
            return;
        }
        match self
            .model_frequency
            .iter_mut()
            .find(|(seen, _)| seen == model)
        {
            Some((_, count)) => *count += 1,
            None => self.model_frequency.push((model.to_string(), 1)),
        }
    }

    /// The most frequently seen full model id, or `None` when the transcript
    /// never named one.
    fn resolved_model(&self) -> Option<String> {
        self.model_frequency
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(model, _)| model.clone())
    }

    pub(crate) fn finish(
        self,
        catalog: &ToolCatalog,
    ) -> (Option<InitialContextBreakdown>, HashMap<String, String>) {
        self.finish_with_probe(&real_claude_filesystem_probe, catalog)
    }

    /// Same as [`Self::finish`], but takes the filesystem-existence check as an
    /// argument so a test can stub it instead of touching the real disk.
    fn finish_with_probe(
        mut self,
        probe: &dyn Fn(&str) -> bool,
        catalog: &ToolCatalog,
    ) -> (Option<InitialContextBreakdown>, HashMap<String, String>) {
        let home = crate::paths::home_dir().map(|path| path.to_string_lossy().into_owned());
        for row in &mut self.source_rows {
            if row.source != InitialContextTokenSource::Skill {
                continue;
            }
            let Some(name) = row.source_name.as_deref() else {
                continue;
            };
            row.origin = resolve_claude_skill_origin(
                name,
                &self.skill_origin_evidence,
                self.cwd.as_deref(),
                home.as_deref(),
                probe,
            );
        }
        let resolved_model = self.resolved_model();
        self.source_rows.extend(builtin_tool_rows(
            "claude",
            self.harness_version.as_deref(),
            resolved_model.as_deref(),
            &self.deferred_tools,
            catalog,
        ));
        let breakdown = normalize_breakdown(self.source_rows);
        (Some(to_output(breakdown)), self.skill_descriptions)
    }
}

/// Classify an absolute Claude skill path into a [`SourceOrigin`]. `cwd` is the
/// session's own working directory, the only way to tell a project-scoped
/// skill from a user-scoped one at the same `.claude/skills/` path shape.
fn classify_claude_filesystem_path(path: &str, cwd: Option<&str>) -> SourceOrigin {
    if path.contains("/bundled-skills/") {
        return SourceOrigin::Bundled;
    }
    if path.contains("/.claude/plugins/") {
        return SourceOrigin::Plugin;
    }
    if path.contains("/.claude/skills/") {
        return match cwd {
            Some(cwd) if !cwd.is_empty() && path.starts_with(cwd) => SourceOrigin::Project,
            _ => SourceOrigin::User,
        };
    }
    SourceOrigin::Unknown
}

/// Classify the `path` of one `invoked_skills` entry into a [`SourceOrigin`].
///
/// The value is a scheme string, not a filesystem path: Claude writes
/// `bundled:<name>` for a skill that ships with the agent and
/// `userSettings:<name>` for a skill in the user's own directory. An
/// unrecognized scheme returns [`SourceOrigin::Unknown`], which
/// [`ClaudeContextAccumulator::record_skill_origin`] drops, so the filesystem
/// probe still runs for that name.
fn classify_claude_invoked_skill_path(path: &str, cwd: Option<&str>) -> SourceOrigin {
    if path.starts_with('/') {
        return classify_claude_filesystem_path(path, cwd);
    }
    match path.split_once(':') {
        Some(("bundled", _)) => SourceOrigin::Bundled,
        Some(("userSettings", _)) => SourceOrigin::User,
        Some(("projectSettings" | "localSettings", _)) => SourceOrigin::Project,
        Some(("plugin", _)) => SourceOrigin::Plugin,
        _ => SourceOrigin::Unknown,
    }
}

/// Resolve one skill's origin: transcript evidence wins outright; otherwise,
/// probe the filesystem for a `SKILL.md` under the project or the user's home.
///
/// The project probe needs the session's `cwd` to exist on this machine. The
/// user probe does not: a user skill lives under the home directory, so the
/// answer stays correct after the session's directory is deleted. When both
/// probes miss and the `cwd` exists, a bare name (no `:`) resolves to
/// `Bundled` — see [`SourceOrigin::Unknown`] for the reasoning.
fn resolve_claude_skill_origin(
    name: &str,
    evidence: &HashMap<String, (u8, SourceOrigin)>,
    cwd: Option<&str>,
    home: Option<&str>,
    probe: &dyn Fn(&str) -> bool,
) -> SourceOrigin {
    if let Some(&(_, origin)) = evidence.get(name) {
        return origin;
    }
    // An absent `cwd` means the transcript came from another machine, or the
    // session ran in a directory that is now deleted, such as a removed git
    // worktree. Only the project probe depends on it.
    let cwd = cwd
        .filter(|cwd| !cwd.is_empty() && probe(cwd))
        .unwrap_or_default();
    if !cwd.is_empty() && probe(&format!("{cwd}/.claude/skills/{name}/SKILL.md")) {
        return SourceOrigin::Project;
    }
    if let Some(home) = home.filter(|home| !home.is_empty())
        && probe(&format!("{home}/.claude/skills/{name}/SKILL.md"))
    {
        return SourceOrigin::User;
    }
    // Both probes ran and found the skill in neither directory. The harness
    // only lists skills it actually found, and a plugin skill always carries
    // a `<plugin>:` prefix, so a bare name (no `:`) with no project or user
    // hit can only have shipped with the agent itself.
    if !cwd.is_empty() && !name.contains(':') {
        return SourceOrigin::Bundled;
    }
    SourceOrigin::Unknown
}

fn real_claude_filesystem_probe(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

fn parse_claude(payload: &str, catalog: &ToolCatalog) -> Option<InitialContextBreakdown> {
    let mut accumulator = ClaudeContextAccumulator::default();
    for value in parse_json_lines(payload) {
        accumulator.observe(&value);
    }
    accumulator.finish(catalog).0
}

fn normalize_breakdown(
    mut rows: Vec<InitialContextTokenSourceCount>,
) -> InitialContextTokenBreakdown {
    rows.retain(|row| row.token_count >= 0);
    merge_rows(&mut rows);
    InitialContextTokenBreakdown { rows }
}

/// Build one `builtin_tool` row per tool the catalogue lists for `agent` at
/// `version`/`model`. A tool named in `deferred` (case-insensitive, against
/// its catalogue name or any alias) gets [`DEFERRED_TOOL_TOKEN_ESTIMATE`]
/// instead of its measured cost. Empty when `version` or `model` is unknown,
/// or when the catalogue cannot resolve them.
fn builtin_tool_rows(
    agent: &str,
    version: Option<&str>,
    model: Option<&str>,
    deferred: &HashSet<String>,
    catalog: &ToolCatalog,
) -> Vec<InitialContextTokenSourceCount> {
    let (Some(version), Some(model)) = (version, model) else {
        return Vec::new();
    };
    let Some(tools) = catalog.lookup(agent, version, model) else {
        return Vec::new();
    };
    tools
        .into_iter()
        .map(|tool| {
            let is_deferred = tool.is_deferred(deferred);
            InitialContextTokenSourceCount {
                source: InitialContextTokenSource::BuiltinTool,
                source_name: Some(tool.display_name()),
                token_count: if is_deferred {
                    DEFERRED_TOOL_TOKEN_ESTIMATE as i64
                } else {
                    tool.tokens as i64
                },
                origin: SourceOrigin::Bundled,
                deferred: is_deferred,
                match_names: tool.match_names(),
            }
        })
        .collect()
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

fn parse_codex_developer_prompt(
    text: &str,
    cwd: Option<&str>,
) -> Vec<InitialContextTokenSourceCount> {
    let Some((skills_start, skills_end)) = section_bounds(text, "## Skills") else {
        return Vec::new();
    };
    let skills_section = &text[skills_start..skills_end];
    parse_named_markdown_bullets(skills_section, InitialContextTokenSource::Skill, cwd)
}

/// Classify a Codex skill's `(file: <path>)` locator into a [`SourceOrigin`].
/// `cwd` is the session's own working directory (from `session_meta`), the
/// only way to tell a project-scoped skill from a user-scoped one at the same
/// `skills/` path shape.
fn classify_codex_skill_origin(path: &str, cwd: Option<&str>) -> SourceOrigin {
    if path.contains("/.codex/skills/.system/")
        || path.contains("/.codex/plugins/cache/openai-bundled/")
        || path.contains("/.codex/plugins/cache/openai-primary-runtime/")
    {
        return SourceOrigin::Bundled;
    }
    if path.contains("/plugins/cache/") {
        return SourceOrigin::Plugin;
    }
    let is_skill_path = path.contains("/.codex/skills/") || path.contains("/.agents/skills/");
    if !is_skill_path {
        return SourceOrigin::Unknown;
    }
    match cwd {
        Some(cwd) if !cwd.is_empty() && path.starts_with(cwd) => SourceOrigin::Project,
        _ => SourceOrigin::User,
    }
}

fn parse_named_markdown_bullets(
    text: &str,
    source: InitialContextTokenSource,
    cwd: Option<&str>,
) -> Vec<InitialContextTokenSourceCount> {
    let mut rows = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_origin = SourceOrigin::Unknown;
    let mut current_text = String::new();

    for line in text.lines() {
        if let Some((name, _, file_path)) = parse_markdown_bullet(line) {
            push_named_source(
                &mut rows,
                source,
                current_name.take(),
                current_origin,
                &current_text,
            );
            current_name = Some(name);
            current_origin = file_path
                .map(|path| classify_codex_skill_origin(&path, cwd))
                .unwrap_or_default();
            current_text.clear();
        }
        if current_name.is_some() {
            current_text.push_str(line);
            current_text.push('\n');
        }
    }
    push_named_source(
        &mut rows,
        source,
        current_name,
        current_origin,
        &current_text,
    );
    rows
}

/// Parse a `"- <name>: <description>"` skill-listing bullet into its name, the
/// post-colon description text (trimmed, with any trailing Codex `(file: <path>)`
/// locator removed), and that locator's path when present. The name half keeps
/// the original validation (no internal space, backtick-stripped); splitting on
/// the first `": "` first (falling back to a bare `:`) lets a plugin-qualified
/// name such as `browser:control-in-app-browser` survive, since a valid name
/// never contains a space and so never contains `": "` itself. `None` for a
/// non-bullet line or a multi-word/empty name.
fn parse_markdown_bullet(line: &str) -> Option<(String, String, Option<String>)> {
    let line = line.trim_start();
    let rest = line.strip_prefix("- ")?;
    let (name, description) = rest.split_once(": ").or_else(|| rest.split_once(':'))?;
    let name = name.trim().trim_matches('`');
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    let (description, file_path) = split_file_locator(description.trim());
    Some((name.to_string(), description, file_path))
}

/// Split a bullet description from a trailing Codex `(file: <path>)` source
/// locator, when present. The token estimate still runs over the raw line (the
/// locator is real context), but a stored skill description must not carry it.
fn split_file_locator(description: &str) -> (String, Option<String>) {
    const LOCATOR_OPEN: &str = "(file: ";
    let trimmed = description.trim_end();
    if trimmed.ends_with(')')
        && let Some(open) = trimmed.rfind(LOCATOR_OPEN)
    {
        let path = trimmed[open + LOCATOR_OPEN.len()..trimmed.len() - 1].trim();
        if !path.is_empty() {
            return (
                trimmed[..open].trim_end().to_string(),
                Some(path.to_string()),
            );
        }
    }
    (trimmed.to_string(), None)
}

fn push_named_source(
    rows: &mut Vec<InitialContextTokenSourceCount>,
    source: InitialContextTokenSource,
    source_name: Option<String>,
    origin: SourceOrigin,
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
        origin,
        deferred: false,
        match_names: Vec::new(),
    });
}

fn parse_claude_loaded_skill_path(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix("Base directory for this skill: "))
}

fn parse_claude_loaded_skill_name(text: &str) -> Option<String> {
    parse_claude_loaded_skill_path(text)
        .and_then(|path| path.rsplit('/').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
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

    /// The committed fixture catalogue, not the embedded production one — so
    /// these tests stay deterministic regardless of what a local build or CI
    /// run happens to regenerate at `tool_catalog.json`.
    fn test_catalog() -> ToolCatalog {
        ToolCatalog::from_json(include_str!("../../tests/fixtures/tool_catalog.json"))
            .expect("fixture catalog must parse")
    }

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

    /// The `origin` of the one skill row named `source_name`, or `None` when no
    /// such row exists.
    fn skill_origin(
        breakdown: &InitialContextBreakdown,
        source_name: &str,
    ) -> Option<SourceOrigin> {
        breakdown
            .sources
            .iter()
            .find(|row| {
                row.source == InitialContextTokenSource::Skill.as_str()
                    && row.source_name.as_deref() == Some(source_name)
            })
            .map(|row| row.origin)
    }

    #[test]
    fn codex_extracts_named_skills() {
        let payload = include_str!("../../tests/fixtures/initial_context/codex_realistic.jsonl");
        let breakdown =
            parse_initial_context("codex", payload).expect("expected supported Codex breakdown");

        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::Skill,
                Some("orbit-tracker")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::Skill,
                Some("atlas-notes")
            ) > 0
        );
    }

    #[test]
    fn codex_classifies_skill_origin_from_the_file_locator_path() {
        let payload = concat!(
            r#"{"type":"session_meta","payload":{"cwd":"/home/avery/projects/demo-app","base_instructions":{"text":"You are Codex, a coding agent."}}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<skills_instructions>\n"#,
            r#"## Skills\n"#,
            r#"- imagegen: Generate images. (file: /home/avery/.codex/skills/.system/imagegen/SKILL.md)\n"#,
            r#"- browser:control-in-app-browser: Control the browser. (file: /home/avery/.codex/plugins/cache/openai-bundled/browser/1.0.0/skills/control-in-app-browser/SKILL.md)\n"#,
            r#"- documents:documents: Edit docs. (file: /home/avery/.codex/plugins/cache/openai-primary-runtime/documents/1.0.0/skills/documents/SKILL.md)\n"#,
            r#"- deep-research-work:deep-research: Research things. (file: /home/avery/.codex/plugins/cache/openai-curated-remote/deep-research-work/0.1.14/skills/deep-research/SKILL.md)\n"#,
            r#"- design-review: Review designs. (file: /home/avery/projects/demo-app/.agents/skills/design-review/SKILL.md)\n"#,
            r#"- discuss: Discuss things. (file: /home/avery/.codex/skills/discuss/SKILL.md)\n"#,
            r#"- changelog-cli: Changelog help. (file: /home/avery/.agents/skills/changelog-cli/SKILL.md)\n"#,
            r#"- mystery-skill: No locator here.\n"#,
            r#"</skills_instructions>"}]}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5000}}}}"#,
        );

        let breakdown =
            parse_initial_context("codex", payload).expect("expected supported Codex breakdown");

        assert_eq!(
            skill_origin(&breakdown, "imagegen"),
            Some(SourceOrigin::Bundled)
        );
        assert_eq!(
            skill_origin(&breakdown, "browser:control-in-app-browser"),
            Some(SourceOrigin::Bundled)
        );
        assert_eq!(
            skill_origin(&breakdown, "documents:documents"),
            Some(SourceOrigin::Bundled)
        );
        assert_eq!(
            skill_origin(&breakdown, "deep-research-work:deep-research"),
            Some(SourceOrigin::Plugin)
        );
        assert_eq!(
            skill_origin(&breakdown, "design-review"),
            Some(SourceOrigin::Project)
        );
        assert_eq!(
            skill_origin(&breakdown, "discuss"),
            Some(SourceOrigin::User)
        );
        assert_eq!(
            skill_origin(&breakdown, "changelog-cli"),
            Some(SourceOrigin::User)
        );
        assert_eq!(
            skill_origin(&breakdown, "mystery-skill"),
            Some(SourceOrigin::Unknown)
        );

        // The `(file: …)` locator counts toward the token estimate but must not
        // leak into the stored description.
        let descriptions = parse_skill_descriptions("codex", payload);
        assert_eq!(
            descriptions.get("imagegen").map(String::as_str),
            Some("Generate images.")
        );
        assert!(!descriptions["imagegen"].contains("(file:"));
        assert_eq!(
            descriptions
                .get("browser:control-in-app-browser")
                .map(String::as_str),
            Some("Control the browser.")
        );
    }

    #[test]
    fn claude_classifies_skill_origin_from_transcript_evidence() {
        let payload = concat!(
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- bundled-evidence-skill: Comes with the agent.\n- project-evidence-skill: Loaded for this project.\n- browser:control-in-app-browser: Control the browser.\n- no-evidence-skill: No signal at all."}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"invoked_skills","skills":[{"name":"bundled-evidence-skill","path":"bundled:bundled-evidence-skill","content":"..."}]}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"dynamic_skill","skillDir":"/home/avery/projects/demo-app/.claude/skills","skillNames":["project-evidence-skill"],"displayPath":".claude/skills"}}"#,
        );

        let breakdown =
            parse_initial_context("claude", payload).expect("expected supported Claude breakdown");

        assert_eq!(
            skill_origin(&breakdown, "bundled-evidence-skill"),
            Some(SourceOrigin::Bundled)
        );
        assert_eq!(
            skill_origin(&breakdown, "project-evidence-skill"),
            Some(SourceOrigin::Project)
        );
        assert_eq!(
            skill_origin(&breakdown, "browser:control-in-app-browser"),
            Some(SourceOrigin::Plugin)
        );
        assert_eq!(
            skill_origin(&breakdown, "no-evidence-skill"),
            Some(SourceOrigin::Unknown)
        );
    }

    #[test]
    fn claude_classifies_an_invoked_skill_from_its_scheme_path() {
        // An `invoked_skills` path is a scheme string, not a filesystem path.
        // A user skill must not lose its origin because the session used it.
        let payload = concat!(
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- user-invoked-skill: Runs for the user.\n- bundled-invoked-skill: Comes with the agent."}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"invoked_skills","skills":[{"name":"user-invoked-skill","path":"userSettings:user-invoked-skill","content":"..."},{"name":"bundled-invoked-skill","path":"bundled:bundled-invoked-skill","content":"..."}]}}"#,
        );

        let breakdown =
            parse_initial_context("claude", payload).expect("expected supported Claude breakdown");

        assert_eq!(
            skill_origin(&breakdown, "user-invoked-skill"),
            Some(SourceOrigin::User)
        );
        assert_eq!(
            skill_origin(&breakdown, "bundled-invoked-skill"),
            Some(SourceOrigin::Bundled)
        );
    }

    #[test]
    fn claude_unreadable_invoked_skill_path_leaves_the_probe_to_answer() {
        // An unknown scheme is not evidence. It must not suppress the
        // filesystem probe, which knows this name as a project skill.
        let cwd = "/home/avery/projects/demo-app";
        let project_skill_path = format!("{cwd}/.claude/skills/probe-me/SKILL.md");
        let payload = format!(
            concat!(
                r#"{{"type":"attachment","cwd":"{cwd}","attachment":{{"type":"skill_listing","content":"- probe-me: Loaded from the project."}}}}"#,
                "\n",
                r#"{{"type":"attachment","attachment":{{"type":"invoked_skills","skills":[{{"name":"probe-me","path":"someFutureScheme:probe-me","content":"..."}}]}}}}"#,
            ),
            cwd = cwd
        );

        let probe = move |path: &str| -> bool { path == cwd || path == project_skill_path };

        let mut accumulator = ClaudeContextAccumulator::default();
        for value in parse_json_lines(&payload) {
            accumulator.observe(&value);
        }
        let (breakdown, _) = accumulator.finish_with_probe(&probe, &test_catalog());
        let breakdown = breakdown.expect("expected supported Claude breakdown");

        assert_eq!(
            skill_origin(&breakdown, "probe-me"),
            Some(SourceOrigin::Project)
        );
    }

    #[test]
    fn claude_probes_the_user_directory_when_the_cwd_is_gone() {
        // The session ran in a git worktree that is now deleted. A user skill
        // lives under the home directory, so its origin is still knowable.
        let cwd = "/home/avery/projects/demo-app/.claude/worktrees/gone";
        let home = crate::paths::home_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/home/avery".to_string());
        let user_skill_path = format!("{home}/.claude/skills/user-skill/SKILL.md");

        let payload = format!(
            r#"{{"type":"attachment","cwd":"{cwd}","attachment":{{"type":"skill_listing","content":"- user-skill: Loaded for the user.\n- bare-skill: No evidence, no `:` in the name."}}}}"#
        );

        // The stub probe reports the cwd as absent, and knows only the user
        // skill.
        let probe = move |path: &str| -> bool { path == user_skill_path };

        let mut accumulator = ClaudeContextAccumulator::default();
        for value in parse_json_lines(&payload) {
            accumulator.observe(&value);
        }
        let (breakdown, _) = accumulator.finish_with_probe(&probe, &test_catalog());
        let breakdown = breakdown.expect("expected supported Claude breakdown");

        assert_eq!(
            skill_origin(&breakdown, "user-skill"),
            Some(SourceOrigin::User)
        );
        // The project directory is gone, so the resolver cannot rule it out
        // and must not infer Bundled for the name it did not find.
        assert_eq!(
            skill_origin(&breakdown, "bare-skill"),
            Some(SourceOrigin::Unknown)
        );
    }

    #[test]
    fn claude_dynamic_skill_evidence_outranks_the_listing_name_shape() {
        // A directory-scoped project skill can *also* use a `<dir>:<skill>`
        // listing name (Plugin's shape), so a `dynamic_skill` attachment must
        // still win as Project for that same name (contract priority order).
        let payload = concat!(
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- project-dir:scoped-skill: A directory-scoped project skill."}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"dynamic_skill","skillDir":"/home/avery/projects/demo-app/.claude/skills/project-dir","skillNames":["project-dir:scoped-skill"],"displayPath":".claude/skills/project-dir"}}"#,
        );

        let breakdown =
            parse_initial_context("claude", payload).expect("expected supported Claude breakdown");

        assert_eq!(
            skill_origin(&breakdown, "project-dir:scoped-skill"),
            Some(SourceOrigin::Project)
        );
    }

    #[test]
    fn claude_probes_filesystem_for_project_and_user_skills() {
        let cwd = "/home/avery/projects/demo-app";
        let home = crate::paths::home_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/home/avery".to_string());
        let project_skill_path = format!("{cwd}/.claude/skills/project-skill/SKILL.md");
        let user_skill_path = format!("{home}/.claude/skills/user-skill/SKILL.md");

        let payload = format!(
            r#"{{"type":"attachment","cwd":"{cwd}","attachment":{{"type":"skill_listing","content":"- project-skill: Loaded from the project.\n- user-skill: Loaded for the user.\n- bare-skill: No evidence, no `:` in the name."}}}}"#
        );

        // A stub probe: never touches the real disk. Only the two exact paths
        // the resolver is expected to build report as present.
        let probe = move |path: &str| -> bool {
            path == cwd || path == project_skill_path || path == user_skill_path
        };

        let mut accumulator = ClaudeContextAccumulator::default();
        for value in parse_json_lines(&payload) {
            accumulator.observe(&value);
        }
        let (breakdown, _) = accumulator.finish_with_probe(&probe, &test_catalog());
        let breakdown = breakdown.expect("expected supported Claude breakdown");

        assert_eq!(
            skill_origin(&breakdown, "project-skill"),
            Some(SourceOrigin::Project)
        );
        assert_eq!(
            skill_origin(&breakdown, "user-skill"),
            Some(SourceOrigin::User)
        );
        // The probe ran (cwd exists) and found the bare name in neither
        // directory, so it can only have shipped with the agent.
        assert_eq!(
            skill_origin(&breakdown, "bare-skill"),
            Some(SourceOrigin::Bundled)
        );
    }

    #[test]
    fn resolve_claude_skill_origin_keeps_a_colon_qualified_name_unknown_without_a_hit() {
        // A `:`-qualified name with no transcript evidence and no filesystem
        // hit is a plugin skill missing its evidence, not a bundled one, so
        // it must stay Unknown rather than being misclassified as Bundled.
        // (A listing bullet with this shape actually carries its own
        // Plugin evidence before this function ever runs — see
        // `claude_origin_rank::LISTING_NAME_SHAPE` — so this exercises the
        // resolver directly, as if that evidence were absent.)
        let cwd = "/home/avery/projects/demo-app";
        let evidence = HashMap::new();
        let probe = |path: &str| -> bool { path == cwd };

        assert_eq!(
            resolve_claude_skill_origin(
                "plugin:bare-skill",
                &evidence,
                Some(cwd),
                Some("/home/avery"),
                &probe
            ),
            SourceOrigin::Unknown
        );
    }

    #[test]
    fn claude_leaves_bare_skill_unknown_when_the_project_probe_cannot_run() {
        // The transcript's `cwd` does not exist on this machine (a fixture, or
        // a session recorded elsewhere), so the project probe cannot run. A
        // bare name with no other evidence and no user-directory hit must stay
        // Unknown, not be misread as Bundled.
        let cwd = "/home/avery/projects/demo-app";
        let payload = format!(
            r#"{{"type":"attachment","cwd":"{cwd}","attachment":{{"type":"skill_listing","content":"- bare-skill: No evidence at all."}}}}"#
        );

        // The stub probe reports every path as absent, including the cwd.
        let probe = |_path: &str| -> bool { false };

        let mut accumulator = ClaudeContextAccumulator::default();
        for value in parse_json_lines(&payload) {
            accumulator.observe(&value);
        }
        let (breakdown, _) = accumulator.finish_with_probe(&probe, &test_catalog());
        let breakdown = breakdown.expect("expected supported Claude breakdown");

        assert_eq!(
            skill_origin(&breakdown, "bare-skill"),
            Some(SourceOrigin::Unknown)
        );
    }

    #[test]
    fn claude_extracts_named_loaded_skill_mcp_and_remainder() {
        let payload = include_str!("../../tests/fixtures/initial_context/claude_realistic.jsonl");
        let breakdown =
            parse_initial_context("claude", payload).expect("expected supported Claude breakdown");

        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::Skill,
                Some("orbit-tracker")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::Skill,
                Some("atlas-notes")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::Skill,
                Some("ledger-sync")
            ) > 0
        );
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::Mcp,
                Some("nebula-docs")
            ) > 0
        );
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
            .filter(|r| r.source == InitialContextTokenSource::Skill.as_str())
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
        assert!(
            source_tokens(
                &breakdown,
                InitialContextTokenSource::Skill,
                Some("deferred-skill")
            ) > 0
        );
    }

    #[test]
    fn claude_session_with_no_skills_or_mcp_still_returns_a_breakdown() {
        // A supported agent whose transcript carries no skill or MCP evidence still
        // gets a breakdown, so the UI can show its empty state instead of
        // disappearing the card entirely.
        let payload = concat!(
            r#"{"type":"user","message":{"role":"user","content":"hello"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":"hi"}}"#,
        );
        let breakdown = parse_initial_context("claude", payload)
            .expect("a supported agent always returns a breakdown");
        assert!(breakdown.sources.is_empty());
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

    fn builtin_tool_row<'a>(
        breakdown: &'a InitialContextBreakdown,
        name: &str,
    ) -> Option<&'a InitialContextSourceCount> {
        breakdown.sources.iter().find(|row| {
            row.source == InitialContextTokenSource::BuiltinTool.as_str()
                && row.source_name.as_deref() == Some(name)
        })
    }

    /// End-to-end against a Claude fixture carrying a `version`, a
    /// `message.model` (with a bare-alias distractor), and a
    /// `deferred_tools_delta` attachment. The fixture catalogue, not the
    /// embedded production one, keeps the expected token counts stable.
    #[test]
    fn claude_builtin_tool_rows_reflect_version_model_and_deferral() {
        let payload =
            include_str!("../../tests/fixtures/initial_context/claude_builtin_tools.jsonl");
        let breakdown = parse_initial_context_with_catalog("claude", payload, &test_catalog())
            .expect("expected supported Claude breakdown");

        // `claude-fable-5` is the most frequent full model id (2 sightings);
        // the single bare `sonnet` alias never competes for that slot.
        let bash = builtin_tool_row(&breakdown, "Bash").expect("expected a Bash row");
        assert!(bash.deferred);
        assert_eq!(bash.token_count, DEFERRED_TOOL_TOKEN_ESTIMATE as u64);

        let read = builtin_tool_row(&breakdown, "Read").expect("expected a Read row");
        assert!(!read.deferred);
        assert_eq!(read.token_count, 625);

        let cron_create =
            builtin_tool_row(&breakdown, "CronCreate").expect("expected a CronCreate row");
        assert!(cron_create.deferred);
        assert_eq!(cron_create.token_count, DEFERRED_TOOL_TOKEN_ESTIMATE as u64);

        // claude-fable-5 carries no task_* tool at 2.1.246.
        assert!(builtin_tool_row(&breakdown, "TaskCreate").is_none());

        // `fill_use_counts` is independent of the catalogue: it only matches a
        // row's displayed name against the session's own raw tool-call counts.
        let mut breakdown = breakdown;
        let tool_calls_by_name =
            HashMap::from([("Bash".to_string(), 2u32), ("Read".to_string(), 1u32)]);
        fill_use_counts(&mut breakdown, &[], &HashMap::new(), &tool_calls_by_name);
        assert_eq!(builtin_tool_row(&breakdown, "Bash").unwrap().use_count, 2);
        assert_eq!(builtin_tool_row(&breakdown, "Read").unwrap().use_count, 1);
        // Deferred and never called: still a row, still zero use.
        assert_eq!(
            builtin_tool_row(&breakdown, "CronCreate")
                .unwrap()
                .use_count,
            0
        );
    }

    /// End-to-end against a Codex fixture carrying `session_meta.cli_version`
    /// and `turn_context.model`. Codex has no deferred-tool marker, so every
    /// catalogued tool loads at its measured cost.
    #[test]
    fn codex_builtin_tool_rows_use_cli_version_and_turn_context_model() {
        let payload =
            include_str!("../../tests/fixtures/initial_context/codex_builtin_tools.jsonl");
        let breakdown = parse_initial_context_with_catalog("codex", payload, &test_catalog())
            .expect("expected supported Codex breakdown");

        let apply_patch =
            builtin_tool_row(&breakdown, "apply_patch").expect("expected an apply_patch row");
        assert!(!apply_patch.deferred);
        assert_eq!(apply_patch.token_count, 270);

        let web_search =
            builtin_tool_row(&breakdown, "web_search").expect("expected a web_search row");
        assert_eq!(web_search.token_count, 4436);

        let mut breakdown = breakdown;
        let tool_calls_by_name = HashMap::from([("apply_patch".to_string(), 2u32)]);
        fill_use_counts(&mut breakdown, &[], &HashMap::new(), &tool_calls_by_name);
        assert_eq!(
            builtin_tool_row(&breakdown, "apply_patch")
                .unwrap()
                .use_count,
            2
        );
    }

    /// End-to-end against a real Codex namespacing shape: 0.149.1's catalogue
    /// carries `functions.exec` and `collaboration.spawn_agent` aliases, but a
    /// session calls the wrapper `exec` (unwrapped into a nested `read` call)
    /// and the short `spawn_agent` name — never the dotted alias itself.
    /// Proves the display name folds to the alias's last segment, and that
    /// `tool_calls_by_name` (computed by the real Codex adapter + metrics
    /// pipeline, exercising the `exec`-wrapper fix) drives `use_count` for
    /// both rows.
    #[test]
    fn codex_builtin_tool_rows_match_namespaced_aliases_by_last_segment() {
        let payload =
            include_str!("../../tests/fixtures/initial_context/codex_namespaced_tools.jsonl");

        let input = crate::analysis::SessionInput {
            agent: "codex".to_string(),
            session_id: "namespaced-tools".to_string(),
            source: crate::analysis::RawSource::Jsonl(payload.to_string()),
            fork_parent_session_id: None,
        };
        let session = crate::analysis::normalize_source(&input).expect("normalize codex session");
        let metrics = crate::analysis::analyze_session(&session);
        // The `exec` wrapper is not lost: it unwraps into a nested `read` call
        // for tool-mix accounting, but its own use still counts.
        assert_eq!(metrics.tool_calls_by_name.get("exec").copied(), Some(1));
        assert_eq!(metrics.tool_calls_by_name.get("read").copied(), Some(1));
        assert_eq!(
            metrics.tool_calls_by_name.get("spawn_agent").copied(),
            Some(1)
        );

        let mut breakdown = parse_initial_context_with_catalog("codex", payload, &test_catalog())
            .expect("expected supported Codex breakdown");

        // Display names fold the namespaced alias to its bare last segment.
        assert!(builtin_tool_row(&breakdown, "exec").is_some());
        assert!(builtin_tool_row(&breakdown, "spawn_agent").is_some());
        assert!(builtin_tool_row(&breakdown, "functions.exec").is_none());
        assert!(builtin_tool_row(&breakdown, "collaboration.spawn_agent").is_none());

        fill_use_counts(
            &mut breakdown,
            &metrics.skill_uses,
            &metrics.mcp_tool_calls,
            &metrics.tool_calls_by_name,
        );
        assert_eq!(builtin_tool_row(&breakdown, "exec").unwrap().use_count, 1);
        assert_eq!(
            builtin_tool_row(&breakdown, "spawn_agent")
                .unwrap()
                .use_count,
            1
        );
    }
}
