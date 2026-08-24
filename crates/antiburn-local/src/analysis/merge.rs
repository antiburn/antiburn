// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Merge a parent session's events with its sub-agents' events into one
//! time-aligned stream.
//!
//! A Claude Code sub-agent (a `Task` tool run) writes its own transcript, but
//! the product treats it as an implementation detail of the parent session:
//! 1M tokens in one turn and 1M tokens spread over seven sub-agents read the
//! same. [`merge_subagent_events`] combines every stream into one session.
//! [`crate::analysis::analyze_session`] sums all session tokens but keeps a
//! separate sub-agent bucket series. Context occupancy, compaction, and cache
//! rehydration stay parent-only.

use crate::analysis::model::{EventSource, NormalizedEvent, NormalizedSession};

/// Merge a parent session with every sub-agent it launched into one session.
///
/// The merged session keeps the parent's identity (`agent`, `session_id`,
/// `context_window`, `model`): a sub-agent's own context window does not
/// describe the parent, so it is dropped here. Every event keeps a
/// [`EventSource`] tag so the engine can tell which stream it came from.
///
/// Ordering: events sort by an effective timestamp. An event that carries its
/// own `ts_ms` sorts on it. An event with no timestamp takes the timestamp of
/// the closest earlier event in its *own* stream (the parent, or one
/// sub-agent) — not the merged stream — so a run of untimestamped events
/// (for example tool-result records) stays next to the turn that produced
/// them instead of scattering across the merged timeline. An event before
/// any timestamped event in its own stream sorts first, at the very start of
/// the merge.
///
/// The sort is stable, so when two events land on the same effective
/// timestamp, they keep the order they were pushed in: every parent event
/// before every sub-agent event, and sub-agents in the order `subagents`
/// lists them. This only matters for exact timestamp ties (or two streams
/// that are both still waiting for their first timestamp), which is rare;
/// documented here rather than resolved with an extra ordering key because
/// no vendor's transcript needs finer tie-breaking than "the stream order the
/// caller gave us."
pub fn merge_subagent_events(
    parent: NormalizedSession,
    subagents: Vec<NormalizedSession>,
) -> NormalizedSession {
    let NormalizedSession {
        agent,
        session_id,
        events: parent_events,
        cache_write_tokens_available,
        context_window,
        model,
    } = parent;

    let capacity = parent_events.len() + subagents.iter().map(|s| s.events.len()).sum::<usize>();
    let mut combined: Vec<(i64, NormalizedEvent)> = Vec::with_capacity(capacity);
    push_stream(parent_events, EventSource::Parent, &mut combined);
    for subagent in subagents {
        push_stream(subagent.events, EventSource::Subagent, &mut combined);
    }
    combined.sort_by_key(|(sort_key, _)| *sort_key);

    NormalizedSession {
        agent,
        session_id,
        events: combined.into_iter().map(|(_, event)| event).collect(),
        cache_write_tokens_available,
        context_window,
        model,
    }
}

/// Tag every event in one stream with `source`, compute its sort key (its own
/// timestamp, or the last timestamp seen earlier in this same stream), and
/// push it onto `out`. A stream with no timestamped event yet uses `i64::MIN`,
/// so its events sort before any timestamped event from any stream.
fn push_stream(
    events: Vec<NormalizedEvent>,
    source: EventSource,
    out: &mut Vec<(i64, NormalizedEvent)>,
) {
    let mut last_ts = i64::MIN;
    for mut event in events {
        if let Some(ts) = event.ts_ms {
            last_ts = ts;
        }
        event.source = source;
        out.push((last_ts, event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::model::{Role, Usage};

    fn session(id: &str, events: Vec<NormalizedEvent>) -> NormalizedSession {
        NormalizedSession {
            agent: "claude".into(),
            session_id: id.into(),
            events,
            cache_write_tokens_available: true,
            context_window: None,
            model: None,
        }
    }

    fn ev(ts_ms: Option<i64>) -> NormalizedEvent {
        NormalizedEvent {
            ts_ms,
            ..NormalizedEvent::new(Role::Assistant)
        }
    }

    #[test]
    fn interleaves_streams_by_timestamp_and_tags_source() {
        let parent = session("parent", vec![ev(Some(0)), ev(Some(2000)), ev(Some(4000))]);
        let subagent = session("sub", vec![ev(Some(1000)), ev(Some(3000))]);

        let merged = merge_subagent_events(parent, vec![subagent]);

        assert_eq!(merged.session_id, "parent");
        let stamps: Vec<Option<i64>> = merged.events.iter().map(|e| e.ts_ms).collect();
        assert_eq!(
            stamps,
            vec![Some(0), Some(1000), Some(2000), Some(3000), Some(4000)]
        );
        let sources: Vec<EventSource> = merged.events.iter().map(|e| e.source).collect();
        assert_eq!(
            sources,
            vec![
                EventSource::Parent,
                EventSource::Subagent,
                EventSource::Parent,
                EventSource::Subagent,
                EventSource::Parent
            ]
        );
    }

    #[test]
    fn a_timestampless_event_keeps_its_stream_position() {
        // The sub-agent's second event carries no timestamp; it must land
        // right after the sub-agent's own 1000ms event, not drift to the
        // front of the merge (which `ts_ms: None` sorting first would do).
        let parent = session("parent", vec![ev(Some(0)), ev(Some(5000))]);
        let mut tool_result = ev(None);
        tool_result.usage = Usage {
            output_tokens: 7,
            ..Usage::default()
        };
        let subagent = session("sub", vec![ev(Some(1000)), tool_result]);

        let merged = merge_subagent_events(parent, vec![subagent]);

        let shape: Vec<(EventSource, Option<i64>, u64)> = merged
            .events
            .iter()
            .map(|e| (e.source, e.ts_ms, e.usage.output_tokens))
            .collect();
        assert_eq!(
            shape,
            vec![
                (EventSource::Parent, Some(0), 0),
                (EventSource::Subagent, Some(1000), 0),
                (EventSource::Subagent, None, 7),
                (EventSource::Parent, Some(5000), 0),
            ]
        );
    }
}
