//! Rebuilds `SessionMetrics` from a session's turn rows alone.
//!
//! [`turn_row_from_event`] (`rows.rs`) turns a stream of [`NormalizedEvent`]s
//! into rows. This module inverts that mapping: [`event_from_row`] turns one
//! row back into an event, and [`metrics_from_rows`] replays a whole row set
//! through [`SessionMetricsAccumulator`] the same way the live pipeline
//! drives it, so the same code computes the same numbers. See
//! `crates/antiburn-local/tests/turn_row_replay_parity.rs` for the proof.

use crate::analysis::engine::SessionMetrics;
use crate::analysis::interface::{NormalizedRecord, RecordSink, SessionSummary};
use crate::analysis::metrics_sink::{SessionMetricsAccumulator, merge_metrics};
use crate::analysis::model::{
    EventSource, NormalizedEvent, Role, ToolCall, Usage, is_subagent_launch_tool,
};
use crate::analysis::rows::{TurnRow, TurnScope};

/// Builds the smallest `Vec<ToolCall>` that reproduces the two reads
/// `SessionMetricsAccumulator::observe_parent_fields` makes over a row's
/// tools: `tools.last().name` (the row's own [`TurnRow::last_tool`]) and the
/// count of tool names that are a `Task` or `Agent` call, case-insensitive
/// (the row's own [`TurnRow::subagent_launches`]).
///
/// A turn's [`TurnRow::last_tool`] that itself reads as a launch already
/// counts toward [`TurnRow::subagent_launches`] — `turn_row_from_event`
/// derives both from the same `event.tools`, and the last tool is one of
/// the tools. So this function reserves that last slot for the row's own
/// `last_tool` string (case preserved, so the reproduced last-tool name
/// matches exactly) and fills every slot before it with a generic `"Task"`
/// call, one per remaining launch.
fn synthesize_tools(row: &TurnRow) -> Vec<ToolCall> {
    let mut tools = Vec::new();
    let last_is_task = row
        .last_tool
        .as_deref()
        .is_some_and(is_subagent_launch_tool);
    let generic_launches = if last_is_task {
        row.subagent_launches.saturating_sub(1)
    } else {
        row.subagent_launches
    };
    for _ in 0..generic_launches {
        tools.push(ToolCall::new("Task"));
    }
    if let Some(name) = &row.last_tool {
        tools.push(ToolCall::new(name.clone()));
    }
    tools
}

/// Rebuilds the [`NormalizedEvent`] one [`TurnRow`] came from, field by
/// field, inverting `turn_row_from_event` (`rows.rs`).
///
/// A row does not carry every field an event can: `wrapper_tool`,
/// `may_resolve_late_tool`, `late_tool_candidate_is_builtin`, and
/// `logical_parent_uuid` have no column, so the rebuilt event leaves them at
/// their default. `SessionMetricsAccumulator` reads none of these to compute
/// `SessionMetrics`, so the rebuilt event still drives it to the same
/// result — see the parity test for the fields this does and does not cover.
///
/// `event.source` mirrors the row's own [`TurnRow::scope`] directly
/// (`Main` → `Parent`, `Delegated` → `Subagent`). This is correct for a row
/// whose accumulator processes it inline, mixed with the rest of its
/// source's rows — every row in the parity test's fixtures. A row from a
/// discovered child transcript (see [`is_parent_group`]'s doc comment for
/// how [`metrics_from_rows`] tells one from the parent's own rows) is
/// different: the live pipeline drives such a source through its own
/// dedicated accumulator, which still sees its own events as `Parent`, and
/// only `merge_metrics` folds it in as a sub-agent stream.
/// [`metrics_from_rows`] applies that override itself, after calling this
/// function, for exactly that case.
pub(crate) fn event_from_row(row: &TurnRow) -> NormalizedEvent {
    let role = match row.role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" => Role::System,
        "tool" => Role::Tool,
        other => unreachable!("turn row carries an unrecognized role {other:?}"),
    };
    let mut event = NormalizedEvent::new(role);
    event.ts_ms = row.ts_ms;
    event.source = match row.scope {
        TurnScope::Main => EventSource::Parent,
        TurnScope::Delegated => EventSource::Subagent,
    };
    event.usage = Usage {
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cache_read_tokens: row.cache_read_tokens,
        cache_creation_tokens: row.cache_write_tokens,
    };
    event.tools = synthesize_tools(row);
    event.model = row.model.clone();
    event.thinking_mode = row.effort.clone();
    event.speed = row.speed.clone();
    event.has_thinking = row.has_thinking;
    event.message_id = row.message_id.clone();
    event.is_compaction_boundary = row.is_compaction_boundary;
    event.compaction_trigger = row.compaction_trigger;
    event.compaction_pre_tokens = row.compaction_pre_tokens;
    event.compaction_post_tokens = row.compaction_post_tokens;
    event.uuid = row.uuid.clone();
    event.parent_uuid = row.parent_uuid.clone();
    event.thread_id = Some(row.thread_id.clone());
    event
}

/// One `source_key` group of rows, in the row order `query_turn_rows`
/// already returns them in (by `turn_index`, ascending).
struct SourceGroup<'a> {
    source_key: &'a str,
    rows: Vec<&'a TurnRow>,
}

/// Splits `rows` into consecutive runs sharing one `source_key`, preserving
/// `rows`' own order. Correct only when `rows` is already sorted by
/// `(source_key, turn_index)` — [`crate::analysis::query_turn_rows`]'s own
/// order.
fn group_by_source<'a>(rows: &'a [TurnRow]) -> Vec<SourceGroup<'a>> {
    let mut groups: Vec<SourceGroup<'a>> = Vec::new();
    for row in rows {
        match groups.last_mut() {
            Some(group) if group.source_key == row.source_key => group.rows.push(row),
            _ => groups.push(SourceGroup {
                source_key: &row.source_key,
                rows: vec![row],
            }),
        }
    }
    groups
}

/// True when `group` is the parent transcript's own rows, among possibly
/// several sources.
///
/// The live pipeline gives every source's [`TurnRowSink`] the same
/// [`crate::analysis::TurnRowStore`], fenced to one fixed session key for
/// the whole pass (`FencedTurnRowStore`, `apps/desktop/src-tauri/src/store/
/// mod.rs`), but a distinct `source_key` per input
/// (`TurnRowSink::new(store, input.session_id.clone(), scope)`,
/// `stream_vendor_with_hooks`). For the parent input (index 0),
/// `input.session_id` is that same session's own id — the parent
/// transcript *is* the session — so its rows' `source_key` equals
/// `session_id`. A discovered child transcript's `input.session_id` is that
/// child's own, distinct id, so its rows' `source_key` never does. This is
/// the same rule `query_model_runs` (`evidence_query.rs`) already uses to
/// split parent runs from child runs. It needs no row shape (`scope`,
/// `child_id`): those still round-trip through [`event_from_row`], but
/// `metrics_from_rows` never inspects them to tell sources apart.
///
/// [`TurnRowSink`]: crate::analysis::rows::TurnRowSink
fn is_parent_group(group: &SourceGroup<'_>, session_id: &str) -> bool {
    group.source_key == session_id
}

/// Builds one [`SessionMetricsAccumulator`] from one source's rows.
/// `force_parent_source` overrides every row's own `event.source` to
/// `EventSource::Parent` — see [`event_from_row`]'s doc comment for when the
/// caller sets it.
fn accumulator_from_group(
    agent: &str,
    session_id: &str,
    group: &SourceGroup<'_>,
    force_parent_source: bool,
    summary: SessionSummary,
) -> SessionMetricsAccumulator {
    let mut accumulator = SessionMetricsAccumulator::new(agent, session_id);
    for &row in &group.rows {
        let mut event = event_from_row(row);
        if force_parent_source {
            event.source = EventSource::Parent;
        }
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    accumulator.finish(summary);
    accumulator
}

/// [`metrics_from_rows`] found no source group whose `source_key` equals
/// the session's own id, so it could not tell which source is the parent
/// transcript — see [`is_parent_group`]'s doc comment for that rule. A row
/// set should never actually have this shape; the caller should fall back
/// to the live parse path rather than trust a rebuild that cannot find its
/// own parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingParentRows;

impl std::fmt::Display for MissingParentRows {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "metrics_from_rows: no source group's source_key equals the session id"
        )
    }
}

impl std::error::Error for MissingParentRows {}

/// Rebuilds `SessionMetrics` by replaying `rows` through
/// [`SessionMetricsAccumulator`], the same way the live pipeline drives it
/// from a normalized event stream.
///
/// `rows` must already carry `query_turn_rows`'s own order — sorted by
/// `(source_key, turn_index)` — since a row's position within its own
/// source is the only order this function has to go on.
///
/// The live pipeline builds one accumulator per source file (the parent
/// transcript, plus one per discovered child transcript) and combines them
/// with [`merge_metrics`]; each accumulator finishes with its own source's
/// `SessionSummary`, built from that source's own raw bytes. Rows carry no
/// column for a `SessionSummary`'s fields (`model`, `context_window`,
/// `cache_write_tokens_available`, `started_at_ms`, and the rest), so the
/// caller supplies one per `source_key` through `summary_for` — the pure
/// row-based part of this function is everything else: grouping rows by
/// source, rebuilding each source's events, and driving and merging the
/// accumulators exactly as the live pipeline does.
///
/// Every parity-tested fixture streams through one source, so `summary_for`
/// runs once; a session with discovered child transcripts calls it once per
/// `source_key`, parent first, each call's row group passed for context.
///
/// Returns [`MissingParentRows`] when no source group's `source_key` equals
/// `session_id` — this rebuild is meant to back a live production command,
/// so an unexpected row shape returns an error the caller can fall back on
/// instead of panicking. Because [`group_by_source`] partitions rows into
/// groups with distinct `source_key`s, at most one group can ever match, so
/// there is no corresponding "more than one parent" case to handle.
pub fn metrics_from_rows(
    agent: impl Into<String>,
    session_id: impl Into<String>,
    rows: &[TurnRow],
    mut summary_for: impl FnMut(&str) -> SessionSummary,
) -> Result<SessionMetrics, MissingParentRows> {
    let agent = agent.into();
    let session_id = session_id.into();
    let groups = group_by_source(rows);
    if groups.is_empty() {
        let mut accumulator = SessionMetricsAccumulator::new(agent, session_id);
        accumulator.finish(summary_for(""));
        return Ok(accumulator.metrics());
    }
    // A single source needs no parent/child call: there is no other group
    // to fold it in as a sub-agent stream of, so it is the parent
    // regardless of its own `source_key`.
    if groups.len() == 1 {
        let group = &groups[0];
        let summary = summary_for(group.source_key);
        let parent = accumulator_from_group(&agent, &session_id, group, false, summary);
        return Ok(parent.metrics());
    }
    let parent_index = groups
        .iter()
        .position(|group| is_parent_group(group, &session_id))
        .ok_or(MissingParentRows)?;
    let parent_group = &groups[parent_index];
    let parent_summary = summary_for(parent_group.source_key);
    let parent = accumulator_from_group(&agent, &session_id, parent_group, false, parent_summary);
    let children: Vec<SessionMetricsAccumulator> = groups
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != parent_index)
        .map(|(_, group)| {
            let summary = summary_for(group.source_key);
            accumulator_from_group(&agent, &session_id, group, true, summary)
        })
        .collect();
    Ok(merge_metrics(&parent, &children))
}

/// Rebuilds each source's own `SessionMetrics` from `rows`, keyed by
/// `source_key`, without merging them together.
///
/// The drilldown needs the parent's metrics and each child's separately (for
/// example `top_level_cost`, `subagents_cost`, and each sub-agent's own
/// figures in the desktop app's orchestration status), which
/// [`metrics_from_rows`] cannot give it — that function only returns the one
/// merged view. This function reuses [`is_parent_group`]'s rule
/// (`source_key == session_id`) to tell the parent's rows from a child's:
/// the parent's own group replays with its rows' own `event.source`; every
/// other group replays as an external child, with `event.source` forced to
/// [`EventSource::Parent`] within its own accumulator (see
/// [`event_from_row`]'s doc comment for why).
///
/// `rows` must already carry `query_turn_rows`'s own order — sorted by
/// `(source_key, turn_index)`. `summary_for` supplies the `SessionSummary`
/// for one `source_key` at a time, the same contract [`metrics_from_rows`]
/// uses.
///
/// To rebuild the same merged view the live pipeline computes, call
/// [`metrics_from_rows`] on the same `rows` and `summary_for` — it reuses
/// [`merge_metrics`] internally, the merge the live pipeline itself uses.
/// Never fails: an empty `rows`, or a `rows` with no group whose
/// `source_key` equals `session_id`, simply replays every group as an
/// external child (no group is the parent) rather than erroring — unlike
/// [`metrics_from_rows`], nothing here depends on finding the parent among
/// several groups.
pub fn metrics_by_source(
    agent: impl Into<String>,
    session_id: impl Into<String>,
    rows: &[TurnRow],
    mut summary_for: impl FnMut(&str) -> SessionSummary,
) -> std::collections::BTreeMap<String, SessionMetrics> {
    let agent = agent.into();
    let session_id = session_id.into();
    let mut by_source = std::collections::BTreeMap::new();
    for group in group_by_source(rows) {
        let force_parent_source = !is_parent_group(&group, &session_id);
        let summary = summary_for(group.source_key);
        let accumulator =
            accumulator_from_group(&agent, &session_id, &group, force_parent_source, summary);
        by_source.insert(group.source_key.to_string(), accumulator.metrics());
    }
    by_source
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::rows::turn_row_from_event;

    fn base_row() -> TurnRow {
        let event = NormalizedEvent::new(Role::Assistant);
        turn_row_from_event(&event, "s1", 0)
    }

    #[test]
    fn synthesize_tools_reproduces_last_tool_and_subagent_count_with_no_task_tool() {
        let mut row = base_row();
        row.last_tool = Some("Read".to_owned());
        row.subagent_launches = 0;

        let tools = synthesize_tools(&row);
        assert_eq!(tools.last().map(|tool| tool.name.as_str()), Some("Read"));
        assert_eq!(
            tools
                .iter()
                .filter(|tool| is_subagent_launch_tool(&tool.name))
                .count(),
            0
        );
    }

    #[test]
    fn synthesize_tools_reproduces_several_launches_with_an_unrelated_last_tool() {
        let mut row = base_row();
        row.last_tool = Some("Read".to_owned());
        row.subagent_launches = 3;

        let tools = synthesize_tools(&row);
        assert_eq!(tools.len(), 4);
        assert_eq!(tools.last().map(|tool| tool.name.as_str()), Some("Read"));
        assert_eq!(
            tools
                .iter()
                .filter(|tool| is_subagent_launch_tool(&tool.name))
                .count(),
            3
        );
    }

    /// The edge case `synthesize_tools`'s doc comment reasons through: the
    /// last tool call itself is a `"task"` launch, so it already counts
    /// toward `subagent_launches`. A naive implementation that always
    /// appends one generic `"Task"` call per launch, then the row's own
    /// `last_tool`, would over-count by one.
    #[test]
    fn synthesize_tools_does_not_double_count_when_the_last_tool_is_itself_a_task_launch() {
        let mut row = base_row();
        row.last_tool = Some("Task".to_owned());
        row.subagent_launches = 1;

        let tools = synthesize_tools(&row);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools.last().map(|tool| tool.name.as_str()), Some("Task"));
        assert_eq!(
            tools
                .iter()
                .filter(|tool| is_subagent_launch_tool(&tool.name))
                .count(),
            1
        );
    }

    /// Mirrors `synthesize_tools_does_not_double_count_when_the_last_tool_is_itself_a_task_launch`
    /// for Claude Code's renamed `Agent` tool: the last tool call is an
    /// `Agent` launch, so it already counts toward `subagent_launches`.
    #[test]
    fn synthesize_tools_does_not_double_count_when_the_last_tool_is_itself_an_agent_launch() {
        let mut row = base_row();
        row.last_tool = Some("Agent".to_owned());
        row.subagent_launches = 1;

        let tools = synthesize_tools(&row);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools.last().map(|tool| tool.name.as_str()), Some("Agent"));
        assert_eq!(
            tools
                .iter()
                .filter(|tool| is_subagent_launch_tool(&tool.name))
                .count(),
            1
        );
    }

    /// The row's `last_tool` can carry a different case than the generic
    /// synthesized calls (e.g. a vendor that logs `"task"` lowercase); the
    /// exact string still must survive so the accumulator's interned
    /// `bucket.last_tool` matches byte for byte.
    #[test]
    fn synthesize_tools_keeps_the_last_tools_own_case_when_it_is_a_task_launch() {
        let mut row = base_row();
        row.last_tool = Some("task".to_owned());
        row.subagent_launches = 2;

        let tools = synthesize_tools(&row);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "Task");
        assert_eq!(tools[1].name, "task");
    }

    #[test]
    fn synthesize_tools_is_empty_without_any_tool_call() {
        let row = base_row();
        assert!(synthesize_tools(&row).is_empty());
    }

    #[test]
    fn event_from_row_inverts_scope_into_source() {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.source = EventSource::Subagent;
        event.thread_id = Some("sidechain-1".to_owned());
        let row = turn_row_from_event(&event, "parent-1", 0);

        let rebuilt = event_from_row(&row);
        assert_eq!(rebuilt.source, EventSource::Subagent);
    }

    #[test]
    fn event_from_row_round_trips_every_scalar_field() {
        let mut event = NormalizedEvent::new(Role::Tool);
        event.ts_ms = Some(42);
        event.model = Some("claude-opus-4-6".to_owned());
        event.thinking_mode = Some("high".to_owned());
        event.speed = Some("fast".to_owned());
        event.has_thinking = true;
        event.message_id = Some("msg-1".to_owned());
        event.is_compaction_boundary = true;
        event.compaction_trigger = Some(crate::analysis::model::CompactionTrigger::Manual);
        event.compaction_pre_tokens = Some(100);
        event.compaction_post_tokens = Some(20);
        event.uuid = Some("uuid-1".to_owned());
        event.parent_uuid = Some("uuid-0".to_owned());
        event.usage = Usage {
            input_tokens: 5,
            output_tokens: 6,
            cache_read_tokens: 7,
            cache_creation_tokens: 8,
        };
        let row = turn_row_from_event(&event, "parent-1", 0);

        let rebuilt = event_from_row(&row);
        assert_eq!(rebuilt.role, Role::Tool);
        assert_eq!(rebuilt.ts_ms, Some(42));
        assert_eq!(rebuilt.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(rebuilt.thinking_mode.as_deref(), Some("high"));
        assert_eq!(rebuilt.speed.as_deref(), Some("fast"));
        assert!(rebuilt.has_thinking);
        assert_eq!(rebuilt.message_id.as_deref(), Some("msg-1"));
        assert!(rebuilt.is_compaction_boundary);
        assert_eq!(
            rebuilt.compaction_trigger,
            Some(crate::analysis::model::CompactionTrigger::Manual)
        );
        assert_eq!(rebuilt.compaction_pre_tokens, Some(100));
        assert_eq!(rebuilt.compaction_post_tokens, Some(20));
        assert_eq!(rebuilt.uuid.as_deref(), Some("uuid-1"));
        assert_eq!(rebuilt.parent_uuid.as_deref(), Some("uuid-0"));
        assert_eq!(rebuilt.usage.input_tokens, 5);
        assert_eq!(rebuilt.usage.output_tokens, 6);
        assert_eq!(rebuilt.usage.cache_read_tokens, 7);
        assert_eq!(rebuilt.usage.cache_creation_tokens, 8);
    }

    #[test]
    fn metrics_from_rows_matches_a_direct_accumulator_for_one_source() {
        let mut live = SessionMetricsAccumulator::new("claude", "s1");
        let mut rows = Vec::new();
        for index in 0..3u64 {
            let mut event = NormalizedEvent::new(Role::Assistant);
            event.ts_ms = Some(1_000 + index as i64 * 1_000);
            event.model = Some("claude-opus-4-6".to_owned());
            event.usage = Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            };
            rows.push(turn_row_from_event(&event, "s1", index));
            live.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        }
        let summary = SessionSummary {
            cache_write_tokens_available: true,
            model: Some("claude-opus-4-6".to_owned()),
            ..SessionSummary::default()
        };
        live.finish(SessionSummary {
            cache_write_tokens_available: true,
            model: Some("claude-opus-4-6".to_owned()),
            ..SessionSummary::default()
        });

        let replayed = metrics_from_rows("claude", "s1", &rows, |_| SessionSummary {
            cache_write_tokens_available: summary.cache_write_tokens_available,
            model: summary.model.clone(),
            ..SessionSummary::default()
        })
        .expect("a single source group needs no parent lookup");

        assert_eq!(replayed.tokens_in, live.metrics().tokens_in);
        assert_eq!(replayed.tokens_out, live.metrics().tokens_out);
        assert_eq!(replayed.model_breakdown, live.metrics().model_breakdown);
        assert_eq!(replayed.buckets, live.metrics().buckets);
    }

    #[test]
    fn metrics_from_rows_drives_an_external_child_source_as_parent_then_merges() {
        // The parent file's own rows carry `source_key == session_id`
        // ("parent-1" — the live pipeline's own `SessionInput.session_id`
        // for the parent input is the session's own id). The child file's
        // rows carry a distinct `source_key` ("child-1"), with `Delegated`
        // scope and `child_id` equal to that same key — the shape
        // `TurnRowSink` writes for a discovered sub-agent transcript. Only
        // the `source_key` difference decides which group is the parent;
        // the `scope`/`child_id` shape here just matches what production
        // actually writes.
        let mut parent_event = NormalizedEvent::new(Role::Assistant);
        parent_event.ts_ms = Some(1_000);
        parent_event.model = Some("claude-opus-4-6".to_owned());
        parent_event.usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Usage::default()
        };
        let parent_row = turn_row_from_event(&parent_event, "parent-1", 0);

        let mut child_event = NormalizedEvent::new(Role::Assistant);
        child_event.ts_ms = Some(2_000);
        child_event.model = Some("claude-haiku-4-6".to_owned());
        child_event.usage = Usage {
            input_tokens: 3,
            output_tokens: 1,
            ..Usage::default()
        };
        let mut child_row = turn_row_from_event(&child_event, "child-1", 0);
        child_row.scope = TurnScope::Delegated;
        child_row.child_id = Some("child-1".to_owned());

        let rows = vec![parent_row, child_row];

        // Build the same two accumulators by hand, from the original
        // events (not rows), and merge them the way the live pipeline
        // does: this is the reference this test compares against.
        let mut expected_parent = SessionMetricsAccumulator::new("claude", "parent-1");
        expected_parent.record(NormalizedRecord::MetricsEvent(Box::new(parent_event)));
        expected_parent.finish(SessionSummary {
            model: Some("claude-opus-4-6".to_owned()),
            ..SessionSummary::default()
        });
        let mut expected_child = SessionMetricsAccumulator::new("claude", "parent-1");
        expected_child.record(NormalizedRecord::MetricsEvent(Box::new(child_event)));
        expected_child.finish(SessionSummary {
            model: Some("claude-haiku-4-6".to_owned()),
            ..SessionSummary::default()
        });
        let expected = merge_metrics(&expected_parent, &[expected_child]);

        let replayed =
            metrics_from_rows("claude", "parent-1", &rows, |source_key| SessionSummary {
                model: Some(if source_key == "parent-1" {
                    "claude-opus-4-6".to_owned()
                } else {
                    "claude-haiku-4-6".to_owned()
                }),
                ..SessionSummary::default()
            })
            .expect("the parent group's source_key equals the session id");

        assert_eq!(replayed.tokens_in, expected.tokens_in);
        assert_eq!(replayed.tokens_out, expected.tokens_out);
        assert_eq!(replayed.model_breakdown, expected.model_breakdown);
        assert_eq!(replayed.buckets, expected.buckets);
    }

    #[test]
    fn metrics_from_rows_reports_a_missing_parent_instead_of_panicking() {
        // Neither source's `source_key` equals the session id passed in, so
        // there is no row group `metrics_from_rows` can call the parent.
        let mut event_a = NormalizedEvent::new(Role::Assistant);
        event_a.usage = Usage {
            input_tokens: 1,
            output_tokens: 1,
            ..Usage::default()
        };
        let row_a = turn_row_from_event(&event_a, "source-a", 0);
        let mut event_b = NormalizedEvent::new(Role::Assistant);
        event_b.usage = Usage {
            input_tokens: 1,
            output_tokens: 1,
            ..Usage::default()
        };
        let row_b = turn_row_from_event(&event_b, "source-b", 0);

        let result = metrics_from_rows("claude", "neither-source-key", &[row_a, row_b], |_| {
            SessionSummary::default()
        });

        assert_eq!(result, Err(MissingParentRows));
    }

    /// One parent turn plus two child turns, each in its own source. Every
    /// group's own `metrics_by_source` entry must match a direct
    /// accumulator built from that group's own event alone, and the three
    /// entries summed must equal `metrics_from_rows`'s own merged view over
    /// the same rows — the split the drilldown and `get_subagent_analysis`
    /// both need.
    #[test]
    fn metrics_by_source_splits_a_parent_and_two_children_without_merging() {
        let mut parent_event = NormalizedEvent::new(Role::Assistant);
        parent_event.ts_ms = Some(1_000);
        parent_event.model = Some("claude-opus-4-6".to_owned());
        parent_event.usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Usage::default()
        };
        let parent_row = turn_row_from_event(&parent_event, "parent-1", 0);

        let mut child_a_event = NormalizedEvent::new(Role::Assistant);
        child_a_event.ts_ms = Some(2_000);
        child_a_event.model = Some("claude-haiku-4-6".to_owned());
        child_a_event.usage = Usage {
            input_tokens: 3,
            output_tokens: 1,
            ..Usage::default()
        };
        let mut child_a_row = turn_row_from_event(&child_a_event, "child-a", 0);
        child_a_row.scope = TurnScope::Delegated;
        child_a_row.child_id = Some("child-a".to_owned());

        let mut child_b_event = NormalizedEvent::new(Role::Assistant);
        child_b_event.ts_ms = Some(3_000);
        child_b_event.model = Some("claude-sonnet-4-6".to_owned());
        child_b_event.usage = Usage {
            input_tokens: 7,
            output_tokens: 2,
            ..Usage::default()
        };
        let mut child_b_row = turn_row_from_event(&child_b_event, "child-b", 0);
        child_b_row.scope = TurnScope::Delegated;
        child_b_row.child_id = Some("child-b".to_owned());

        let rows = vec![parent_row, child_a_row, child_b_row];

        let summary_for = |source_key: &str| SessionSummary {
            model: Some(
                match source_key {
                    "parent-1" => "claude-opus-4-6",
                    "child-a" => "claude-haiku-4-6",
                    "child-b" => "claude-sonnet-4-6",
                    other => panic!("unexpected source_key {other:?}"),
                }
                .to_owned(),
            ),
            ..SessionSummary::default()
        };

        let by_source = metrics_by_source("claude", "parent-1", &rows, summary_for);
        assert_eq!(
            by_source.keys().collect::<Vec<_>>(),
            vec!["child-a", "child-b", "parent-1"]
        );

        let mut expected_parent = SessionMetricsAccumulator::new("claude", "parent-1");
        expected_parent.record(NormalizedRecord::MetricsEvent(Box::new(parent_event)));
        expected_parent.finish(summary_for("parent-1"));
        assert_eq!(
            by_source["parent-1"].tokens_in,
            expected_parent.metrics().tokens_in
        );
        assert_eq!(
            by_source["parent-1"].model_breakdown,
            expected_parent.metrics().model_breakdown
        );

        let mut expected_child_a = SessionMetricsAccumulator::new("claude", "parent-1");
        expected_child_a.record(NormalizedRecord::MetricsEvent(Box::new(child_a_event)));
        expected_child_a.finish(summary_for("child-a"));
        assert_eq!(
            by_source["child-a"].tokens_in,
            expected_child_a.metrics().tokens_in
        );
        assert_eq!(
            by_source["child-a"].model_breakdown,
            expected_child_a.metrics().model_breakdown
        );

        let mut expected_child_b = SessionMetricsAccumulator::new("claude", "parent-1");
        expected_child_b.record(NormalizedRecord::MetricsEvent(Box::new(child_b_event)));
        expected_child_b.finish(summary_for("child-b"));
        assert_eq!(
            by_source["child-b"].tokens_in,
            expected_child_b.metrics().tokens_in
        );
        assert_eq!(
            by_source["child-b"].model_breakdown,
            expected_child_b.metrics().model_breakdown
        );

        let merged = metrics_from_rows("claude", "parent-1", &rows, summary_for)
            .expect("the parent group's source_key equals the session id");
        assert_eq!(
            merged.tokens_in,
            by_source
                .values()
                .map(|metrics| metrics.tokens_in)
                .sum::<u64>()
        );
    }

    /// No group's `source_key` equals `session_id`, unlike
    /// `metrics_from_rows`'s own version of this case: this function never
    /// errors, it just has no group it treats as the parent.
    #[test]
    fn metrics_by_source_never_errors_when_no_group_is_the_parent() {
        let event = NormalizedEvent::new(Role::Assistant);
        let row = turn_row_from_event(&event, "source-a", 0);

        let by_source = metrics_by_source("claude", "neither-source-key", &[row], |_| {
            SessionSummary::default()
        });

        assert_eq!(by_source.keys().collect::<Vec<_>>(), vec!["source-a"]);
    }
}
