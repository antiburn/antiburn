// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Normalized, vendor-agnostic session model.
//!
//! Every vendor adapter parses its own transcript format into a stream of
//! [`NormalizedEvent`]s. The analysis engine only ever sees this model, so it
//! stays completely decoupled from any single vendor's on-disk shape.

use serde::{Deserialize, Serialize};

/// Token accounting for a single turn, as reported by the agent's API usage.
///
/// Fields default to zero so adapters can populate only what a given vendor
/// exposes without breaking aggregation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

impl Usage {
    /// Effective input for display: everything that entered the model for the
    /// first time this turn — fresh input plus prompt-cache writes. Cache reads
    /// (re-served context) are deliberately excluded; they surface as occupancy
    /// via [`Usage::context_tokens`] instead.
    pub fn effective_input_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.cache_creation_tokens)
    }

    /// Approximate context-window occupancy at this turn: everything the model
    /// had to read in (fresh input + cached prefix), excluding generated output.
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_creation_tokens)
    }

    pub fn saturating_add(self, other: Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cache_read_tokens: self
                .cache_read_tokens
                .saturating_add(other.cache_read_tokens),
            cache_creation_tokens: self
                .cache_creation_tokens
                .saturating_add(other.cache_creation_tokens),
        }
    }
}

/// Coarse, vendor-neutral bucket for a tool invocation. The engine reasons
/// about categories (not raw tool names) so heuristics work across agents that
/// name the same capability differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolCategory {
    /// Mutates files: Edit, Write, MultiEdit, NotebookEdit, apply_patch, …
    Edit,
    /// Reads file content: Read, cat, view, open, …
    Read,
    /// Searches the codebase: Grep, Glob, ripgrep, find, search, …
    Search,
    /// Runs a test/build/verify command (a shell command whose text is a known
    /// test runner). Detected from the command, not the tool name.
    Test,
    /// Runs a shell command: Bash, run, exec, terminal, …
    Bash,
    /// Anything else (web fetch, task spawn, MCP calls, unknown tools).
    Other,
}

impl ToolCategory {
    /// Map a raw, vendor-specific tool name to a category. Matching is
    /// case-insensitive and substring-based so minor naming drift between
    /// agents (and MCP-prefixed names) still classifies correctly.
    pub fn from_tool_name(name: &str) -> ToolCategory {
        let n = name.to_ascii_lowercase();
        // Codex uses `write_stdin` to continue/poll a running shell process. It
        // is process control, not a file write, so keep this exact name out of
        // the broad edit substring matching below.
        if n == "write_stdin" {
            return ToolCategory::Bash;
        }
        // Order matters: check edit/search before the broad read/bash buckets.
        const EDIT: &[&str] = &[
            "edit",
            "write",
            "apply_patch",
            "applypatch",
            "notebook",
            "create_file",
            "str_replace",
            "update_file",
        ];
        const SEARCH: &[&str] = &["grep", "glob", "ripgrep", "search", "find", "codebase"];
        const READ: &[&str] = &["read", "cat", "view", "open", "fetch_file", "ls", "list"];
        const BASH: &[&str] = &[
            "bash",
            "shell",
            "exec",
            "run_command",
            "terminal",
            "command",
        ];

        if EDIT.iter().any(|k| n.contains(k)) {
            ToolCategory::Edit
        } else if SEARCH.iter().any(|k| n.contains(k)) {
            ToolCategory::Search
        } else if READ.iter().any(|k| n.contains(k)) {
            ToolCategory::Read
        } else if BASH.iter().any(|k| n.contains(k)) {
            ToolCategory::Bash
        } else {
            ToolCategory::Other
        }
    }

    /// True for the specific "grep/glob/search" tools the sleep-card breakdown
    /// surfaces as a standalone "search intensity" signal.
    pub fn is_grep(name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        n.contains("grep") || n.contains("glob") || n.contains("ripgrep") || n.contains("search")
    }
}

/// True when a shell command's text invokes a known test/build/verify runner.
///
/// Matching is token-aware (operates on whitespace-split words, not raw
/// substrings) so it doesn't fire on words that merely *contain* "test" like
/// "latest" or "contest". A command counts when any of its tokens is a runner
/// (`pytest`, `jest`, …) or when a build tool token is paired with a `test`
/// subcommand (`cargo test`, `go test`, `npm run test`, …).
pub fn is_test_command(cmd: &str) -> bool {
    // Standalone runners: a single token is enough.
    const RUNNERS: &[&str] = &[
        "pytest",
        "jest",
        "vitest",
        "mocha",
        "rspec",
        "phpunit",
        "tox",
        "unittest",
        "nextest",
        "ava",
        "karma",
        "jasmine",
        "nose",
        "nosetests",
        "ctest",
        "gotestsum",
    ];
    // Build/runner tools that only mean "testing" when followed by a `test`
    // (or `t`, the common alias) subcommand token.
    const BUILD_TOOLS: &[&str] = &[
        "cargo", "go", "npm", "yarn", "pnpm", "bun", "mvn", "gradle", "dotnet", "mix", "make",
        "deno", "rake", "swift", "bazel", "task", "just",
    ];

    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    for (i, tok) in tokens.iter().enumerate() {
        let t = tok.to_ascii_lowercase();
        // Strip a leading path so `./node_modules/.bin/jest` still matches.
        let base = t.rsplit('/').next().unwrap_or(&t);
        if RUNNERS.contains(&base) {
            return true;
        }
        if BUILD_TOOLS.contains(&base) {
            // Look for a `test`/`t` subcommand in the remaining tokens.
            if tokens[i + 1..]
                .iter()
                .any(|s| matches!(s.to_ascii_lowercase().as_str(), "test" | "t"))
            {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub category: ToolCategory,
    /// Optional, generic per-call detail. Today this carries the skill name for a
    /// `Skill` tool call (the JSON layer populates it via
    /// `vendors::jsonl::tool_call_from_input`); `None` for every other tool. Kept
    /// generic so the "skill" concept never leaks into the core tool model — the
    /// engine reads `detail` only when `name` is a skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ToolCall {
    pub fn new(name: impl Into<String>) -> ToolCall {
        ToolCall::with_command(name, None)
    }

    /// Build a tool call, optionally passing the shell command text so a Bash
    /// invocation that runs tests is reclassified from `Bash` to `Test`.
    pub fn with_command(name: impl Into<String>, command: Option<&str>) -> ToolCall {
        let name = name.into();
        let mut category = ToolCategory::from_tool_name(&name);
        if category == ToolCategory::Bash && command.map(is_test_command).unwrap_or(false) {
            category = ToolCategory::Test;
        }
        ToolCall {
            name,
            category,
            detail: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// Whether a compaction boundary was triggered by the user or by the agent's
/// own context-limit auto-compaction. `None` when the transcript names no
/// trigger, or does not distinguish one (Codex today).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionTrigger {
    Manual,
    Auto,
}

/// Which transcript an event comes from, after [`crate::analysis::merge_subagent_events`]
/// concatenates a parent session with its sub-agents into one stream.
///
/// The engine uses this to keep a few computations parent-only even after the
/// merge: context occupancy, compaction boundaries, and cache-rehydration
/// detection. A sub-agent has its own context window, so mixing its turns
/// into those parent-window computations would not mean anything. Token
/// sums, tool mix, and cost stay unconditional over every event, so a
/// sub-agent's spend still counts toward the session's total — the product
/// rule is that a sub-agent is an implementation detail of its parent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventSource {
    /// The event comes from the session's own transcript.
    #[default]
    Parent,
    /// The event comes from a sub-agent transcript, merged into the parent.
    Subagent,
}

/// One normalized turn/record from a transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedEvent {
    /// Unix epoch milliseconds, when the transcript carries a timestamp.
    pub ts_ms: Option<i64>,
    pub role: Role,
    /// Which transcript this event comes from. `Parent` for every event
    /// before a merge; [`crate::analysis::merge_subagent_events`] tags
    /// sub-agent events `Subagent`.
    #[serde(default)]
    pub source: EventSource,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub tools: Vec<ToolCall>,
    /// The model that produced this turn (e.g. `claude-opus-4-6`), when the
    /// transcript records it per-record. Lets the engine attribute this event's
    /// `usage` to the right model so cost prices per-model rather than at one
    /// headline model. `None` when the record carries no model; the engine then
    /// falls back to the session's headline model.
    #[serde(default)]
    pub model: Option<String>,
    /// The model's thinking mode for this turn, when the transcript records it.
    #[serde(default)]
    pub thinking_mode: Option<String>,
    /// The response speed for this turn (e.g. Claude's "standard"/"fast"),
    /// when the transcript records it. `None` when the vendor carries no
    /// speed signal.
    #[serde(default)]
    pub speed: Option<String>,
    /// True when this turn's content includes a `thinking` block (Claude) or
    /// its vendor equivalent (Codex's `reasoning` response item).
    #[serde(default)]
    pub has_thinking: bool,
    /// The provider's message id (Anthropic `message.id`), when present. Claude
    /// transcripts re-log the same assistant message more than once, each copy
    /// carrying the full `usage`; this id lets the Claude adapter de-duplicate
    /// those copies so token counts (and cost) are not multiplied. `None` for
    /// records without an id (user turns, tool results, non-Claude shapes).
    #[serde(default)]
    pub message_id: Option<String>,
    /// True when this event *is* the compaction boundary itself (Claude
    /// `system`/`compact_boundary`, Codex `event_msg`/`context_compacted`).
    /// Vendor-neutral by design. The engine preserves the context drop at the
    /// correct point.
    #[serde(default)]
    pub is_compaction_boundary: bool,
    /// Whether this compaction boundary was manual or automatic, when the
    /// transcript records it. `None` when this event is not a compaction
    /// boundary, or the vendor names no trigger.
    #[serde(default)]
    pub compaction_trigger: Option<CompactionTrigger>,
    /// The context token count right before this compaction, when the
    /// transcript records it.
    #[serde(default)]
    pub compaction_pre_tokens: Option<u64>,
    /// The context token count right after this compaction, when the
    /// transcript records it. Some older Claude records omit this.
    #[serde(default)]
    pub compaction_post_tokens: Option<u64>,
}

impl NormalizedEvent {
    pub fn new(role: Role) -> NormalizedEvent {
        NormalizedEvent {
            ts_ms: None,
            role,
            source: EventSource::default(),
            usage: Usage::default(),
            tools: Vec::new(),
            model: None,
            thinking_mode: None,
            speed: None,
            has_thinking: false,
            message_id: None,
            is_compaction_boundary: false,
            compaction_trigger: None,
            compaction_pre_tokens: None,
            compaction_post_tokens: None,
        }
    }
}

/// One model and thinking-mode pair that produced billable tokens.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRun {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<String>,
}

/// A full session after normalization, ready for the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedSession {
    pub agent: String,
    pub session_id: String,
    pub events: Vec<NormalizedEvent>,
    /// The model's context-window size for this session, when the vendor reports
    /// it (e.g. Codex `model_context_window`). Claude leaves this as `None` for
    /// unknown model ids so context occupancy can be presented as unavailable.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// The model id used for this session (e.g. `claude-opus-4-6`), when an
    /// adapter can extract it. For sessions that mix models, the most expensive
    /// priceable one seen. `None` when unknown — pricing then yields no estimate.
    #[serde(default)]
    pub model: Option<String>,
}
