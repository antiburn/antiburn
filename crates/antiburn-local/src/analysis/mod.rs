// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Vendor-agnostic analysis of AI coding-agent session transcripts.
//!
//! The caller decides *which* sessions to analyze and hands them in as
//! [`SessionInput`]s; this module owns everything after that: per-vendor
//! normalization (the [`interface`] layer) and the vendor-neutral [`engine`]
//! that turns the normalized stream into the sleep-style [`ActiveSessionsSummary`].
//!
//! A second, independent pass ([`initial_context`]) attributes each session's
//! *initial* context window (tokens loaded before the first response) by source
//! — skills, MCP servers, agent instruction files, system instructions, and an
//! unattributed remainder. It reads the raw transcript directly because the
//! normalized stream drops the per-source text it needs.
//!
//! Everything here is local and read-only: transcripts are parsed from bytes the
//! caller supplies (or from a path/SQLite file on this machine), and no result
//! ever leaves the process.
//!
//! ```no_run
//! use antiburn_local::analysis::{analyze_sources, SessionInput, RawSource};
//!
//! let inputs = vec![SessionInput {
//!     agent: "claude".into(),
//!     session_id: "abc".into(),
//!     source: RawSource::File("/path/to/abc.jsonl".into()),
//! }];
//! let summary = analyze_sources(inputs);
//! println!("{} live sessions, health {}", summary.session_count, summary.avg_pattern_score);
//! ```

mod engine;
mod initial_context;
mod interface;
mod model;
mod pricing;
mod vendors;

pub use engine::{
    ActiveSessionsSummary, BUCKETS, Bucket, CONTEXT_WINDOW, Phase, PhaseDistribution, PhaseSegment,
    SessionCost, SessionMetrics, SkillUse, ToolMix, active_time_fraction, aggregate_metrics,
};
pub use initial_context::{InitialContextBreakdown, InitialContextSourceCount, TrackingStatus};
pub use interface::{RawSource, SessionInput, VendorAdapter};
pub use model::{NormalizedEvent, NormalizedSession, Role, ToolCall, ToolCategory, Usage};
pub use pricing::{install_runtime_pricing, price_breakdown, pricing_generation};
pub use vendors::{adapter_for, has_dedicated_adapter};

/// Normalize and analyze a batch of live sessions into one averaged summary.
///
/// Sources that fail to read (missing file, unopenable DB) are skipped so one
/// bad session never sinks the whole view. Returns an empty summary when no
/// session yields any analyzable events.
pub fn analyze_sources(inputs: Vec<SessionInput>) -> ActiveSessionsSummary {
    analyze_sources_with(inputs, true)
}

/// Like [`analyze_sources`], but lets the caller skip the initial-context pass.
///
/// The aggregate (multi-session) view never renders `initialContext` — only the
/// single-session view does — so the polled aggregate command passes `false` to
/// avoid an extra raw read + JSON parse per live session on every 8s tick.
pub fn analyze_sources_with(
    inputs: Vec<SessionInput>,
    want_initial_context: bool,
) -> ActiveSessionsSummary {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    // Normalize each source in isolation. A panic in one vendor adapter (an
    // arithmetic overflow on a malformed transcript, an unexpected shape) must
    // drop only that session — not fail the whole command for every live session.
    let sessions: Vec<NormalizedSession> = inputs
        .iter()
        .filter_map(|input| {
            match catch_unwind(AssertUnwindSafe(|| {
                adapter_for(&input.agent).normalize(input)
            })) {
                Ok(Ok(session)) => Some(session),
                // Unreadable source (missing file, unopenable DB): expected, skip quietly.
                Ok(Err(_)) => None,
                Err(_) => {
                    eprintln!(
                        "antiburn-local/analysis: adapter for agent={} session_id={} panicked; skipping",
                        input.agent, input.session_id
                    );
                    None
                }
            }
        })
        .collect();
    let mut summary = engine::aggregate(&sessions);

    if !want_initial_context {
        return summary;
    }

    // The initial-context breakdown is a separate pass over the *raw* payload:
    // the normalized stream drops the per-source text (skill listings, MCP
    // deltas, base/developer prompts) it needs. Compute it per input and graft
    // it onto the matching session metrics. Sources we can't read as text here
    // (SQLite-backed agents) simply leave the field `None` ("unavailable").
    for input in &inputs {
        let Ok(payload) = vendors::read_source(&input.source) else {
            continue;
        };

        // Skill one-liners grafted onto each `SkillUse::description` by name. This
        // rides the same raw-payload read as the initial-context pass (so the
        // aggregate tick, which skips this whole block, pays nothing) but is
        // independent of it: a session with skills but no initial-context
        // breakdown still gets its descriptions. Isolated behind `catch_unwind`
        // like the breakdown — a malformed transcript must not sink the command.
        let descriptions = catch_unwind(AssertUnwindSafe(|| {
            initial_context::parse_skill_descriptions(&input.agent, &payload)
        }))
        .unwrap_or_default();

        let breakdown = match catch_unwind(AssertUnwindSafe(|| {
            initial_context::parse_initial_context(&input.agent, &payload)
        })) {
            Ok(breakdown) => breakdown,
            Err(_) => {
                eprintln!(
                    "antiburn-local/analysis: initial-context parse for agent={} session_id={} panicked; skipping breakdown",
                    input.agent, input.session_id
                );
                None
            }
        };

        if let Some(metrics) = summary
            .sessions
            .iter_mut()
            .find(|m| m.agent == input.agent && m.session_id == input.session_id)
        {
            if let Some(breakdown) = breakdown {
                metrics.initial_context = Some(breakdown);
            }
            for skill_use in &mut metrics.skill_uses {
                if skill_use.description.is_none()
                    && let Some(description) = descriptions.get(&skill_use.name)
                {
                    skill_use.description = Some(description.clone());
                }
            }
        }
    }

    summary
}

/// Normalize a single source without aggregation (handy for tests/tools).
pub fn normalize_source(input: &SessionInput) -> anyhow::Result<NormalizedSession> {
    adapter_for(&input.agent).normalize(input)
}

#[cfg(test)]
mod tests;
