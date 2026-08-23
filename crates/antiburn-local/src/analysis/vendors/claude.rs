// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Claude Code adapter — the richest source of token + tool data.
//!
//! Claude stores one JSONL transcript per session at
//! `~/.claude/projects/<encoded>/<session_id>.jsonl`. Each line is a JSON
//! record; assistant lines carry `message.usage` (input/output/cache tokens)
//! and `message.content[]` with `text` / `thinking` / `tool_use` parts, while
//! user lines carry `tool_result` blocks that flag errors.

use std::collections::HashMap;

use anyhow::Context;

use super::jsonl::parse_jsonl;
use super::read_source;
use crate::analysis::interface::{SessionInput, VendorAdapter};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Usage};

pub struct ClaudeAdapter;

impl VendorAdapter for ClaudeAdapter {
    fn agent(&self) -> &'static str {
        "claude"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading claude session {}", input.session_id))?;
        // Single parse pass: `parse_jsonl` already records each turn's model, so
        // the window + headline model are derived from the events rather than a
        // second `serde_json` pass over the whole transcript.
        let mut events = parse_jsonl(&content);
        let (context_window, model) = detect_window_and_model(&events);
        dedup_assistant_usage(&mut events);
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events,
            cache_write_tokens_available: true,
            context_window,
            model,
        })
    }
}

/// Collapse Claude's duplicate assistant records before metrics.
///
/// Claude Code writes the same assistant message (same `message.id`) to the
/// transcript more than once — re-emitted on resume/compaction and once per
/// content block — and every copy carries the *full* `usage`. Summing every line
/// multiplies token counts (and therefore cost), dominated by the repeated
/// cache-read figure. Mirror the backend's de-duplication
/// (`crates/analysis/src/note_parser.rs::record_claude_assistant_usage`): per
/// `message.id`, keep the running per-field maximum and attribute only the
/// positive delta to each event, so each message's usage is counted exactly once.
/// Exact duplicates collapse to zero after the first copy; a streaming message
/// whose usage grows across copies still sums to its final total. Events with no
/// `message.id` (user turns, tool results) are left untouched.
fn dedup_assistant_usage(events: &mut [NormalizedEvent]) {
    let mut max_by_id: HashMap<String, Usage> = HashMap::new();
    for ev in events.iter_mut() {
        let Some(id) = ev.message_id.clone() else {
            continue;
        };
        let cur = ev.usage;
        let prev = max_by_id.get(&id).copied().unwrap_or_default();
        ev.usage = Usage {
            input_tokens: cur.input_tokens.saturating_sub(prev.input_tokens),
            output_tokens: cur.output_tokens.saturating_sub(prev.output_tokens),
            cache_read_tokens: cur.cache_read_tokens.saturating_sub(prev.cache_read_tokens),
            cache_creation_tokens: cur
                .cache_creation_tokens
                .saturating_sub(prev.cache_creation_tokens),
        };
        max_by_id.insert(
            id,
            Usage {
                input_tokens: cur.input_tokens.max(prev.input_tokens),
                output_tokens: cur.output_tokens.max(prev.output_tokens),
                cache_read_tokens: cur.cache_read_tokens.max(prev.cache_read_tokens),
                cache_creation_tokens: cur.cache_creation_tokens.max(prev.cache_creation_tokens),
            },
        );
    }
}

/// The context window for a recognized Claude model family. Unknown model ids
/// stay unavailable rather than inheriting a misleading 200k guess.
fn model_context_window(model: &str) -> Option<u64> {
    let m = model.to_ascii_lowercase();
    let is_1m = m.contains("opus-4")
        || m.contains("fable-5")
        || m.contains("sonnet-5")
        || m.contains("sonnet-4-5")
        || m.contains("sonnet-4-6")
        || m.contains("sonnet-4-7")
        || m.contains("sonnet-4-8")
        || m.contains("sonnet-4-9");
    if is_1m {
        Some(1_000_000)
    } else if m.contains("haiku")
        || m.contains("claude-3")
        || m.contains("sonnet-4-0")
        || m.contains("sonnet-4-202")
    {
        Some(200_000)
    } else {
        None
    }
}

/// Infer the session's context window and headline model from the per-turn
/// models `parse_jsonl` already extracted — no second parse of the transcript.
/// A session mixes models (e.g. Opus for the main thread, Haiku for background
/// tasks): the **window** is set by the largest (max across every turn's model),
/// and the **model** we attribute cost to is the most expensive *priceable* one
/// seen (else the first model seen, for display). Both are `None` when no model
/// is recorded; empty / `<synthetic>` models are already filtered out upstream.
fn detect_window_and_model(events: &[NormalizedEvent]) -> (Option<u64>, Option<String>) {
    let mut window: Option<u64> = None;
    let mut first_model: Option<String> = None;
    let mut best_priceable: Option<(String, f64)> = None;
    // Consecutive turns almost always repeat the same model; skip the repeats so
    // the window + pricing lookups run once per distinct run, not once per turn.
    let mut last_seen_model: Option<&str> = None;
    for ev in events {
        let Some(model) = ev.model.as_deref() else {
            continue;
        };
        if last_seen_model == Some(model) {
            continue; // same model as the previous turn — already accounted for.
        }
        last_seen_model = Some(model);
        if let Some(w) = model_context_window(model) {
            window = Some(window.map_or(w, |cur| cur.max(w)));
        }
        if first_model.is_none() {
            first_model = Some(model.to_string());
        }
        if let Some(p) = crate::analysis::pricing::lookup_pricing(model) {
            let rank = p.input_cost_per_token + p.output_cost_per_token;
            if best_priceable.as_ref().is_none_or(|(_, r)| rank > *r) {
                best_priceable = Some((model.to_string(), rank));
            }
        }
    }
    let model = best_priceable.map(|(m, _)| m).or(first_model);
    (window, model)
}
