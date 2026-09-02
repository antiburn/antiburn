---
title: "Continuous session ingest: watch, resume, and replay"
created_at: "2026-09-02"
status: in progress
---

# Continuous session ingest

- **Date:** 2026-09-02
- **Issue:** none yet. Opened from a popover staleness report.

## Problem

Open the popover after a quiet stretch during which new agent sessions
started, and the session list shows the previous list for a moment, then pops
into the current one.

The list cannot already be current, because nothing discovers sessions while
the popover is hidden. The scan scheduler ticks every 60 seconds but skips the
tick unless the popover is visible (`apps/desktop/src-tauri/src/scan.rs`,
`should_run_scheduled_pass`). Showing the popover kicks a pass; the list
refetches only when that pass emits `scan:finished`. The visible gap is one
full pass.

The gate exists because a pass is expensive. Each pass, for every session
modified in the last 14 days, walks the agent's directories, opens the
transcript, reads up to 16 MiB from its head, hashes it, and parses metadata
out of it. There is no memory of the previous pass at the walk or read level.

Downstream is worse. The insights worker re-parses a whole transcript from
byte zero on every change it notices, writes every turn row again under a new
claim fence, and on publish deletes the rows of every other fence. Turn rows
carry no byte offset. An `AppendOnlyGuarantee` type exists but is hard-coded
to `Absent` for every agent, because fixtures cannot prove how third-party
writers update files.

Three more findings shape the design:

- Discovery, ingest, and evidence are fused. The worker's single stream fold
  fans out to turn rows, metrics, and the evidence accumulator at once
  (`apps/desktop/src-tauri/src/analysis.rs`, `stream_vendor_with_hooks`).
  Metrics are replayable from rows with a parity test
  (`crates/antiburn-local/src/analysis/replay.rs`). Evidence is not.
- The HUD runs its own discovery walk every 60 seconds, polled every 5
  seconds by the overlay, to produce one boolean: blink the first usage
  segment when any transcript was written in the last 90 seconds
  (`apps/desktop/src-tauri/src/hud.rs`, `latest_session_activity`).
- Provider-database agents (OpenCode, Antigravity) already have per-session
  fingerprints from max timestamp plus row count, WAL-aware change detection,
  and ordering columns (`idx`, `time_created`, `time_updated`). Incremental
  ingest for them is a query cursor, not a byte offset.

## Target shape

Two layers, split at the database.

**Layer 1: continuous ingest.** One background component watches transcript
sources, keeps the `session` table current, and appends turn rows as
transcripts grow. It runs whether or not any window is visible. Its cost is
proportional to what changed, not to what exists.

- Change detection is tiered by activity: active sessions checked often,
  inactive sessions rarely, new-session detection frequent but touching only
  leaf directories. Filesystem events accelerate all three; polling remains
  as the reconciliation path and the fallback where events are unavailable.
- Description (the head read) runs only when the source changed.
- Parse resumes from a persisted offset, verified each time by re-hashing a
  window of bytes before it. A mismatch falls back to today's full pass.
- Provider-database sources resume from a row cursor keyed on the vendor's
  update column.
- Active-to-idle is a backend timer keyed on the 180-second window, not a
  file event. Every consumer, including the HUD blink, reads the same signal.

**Layer 2: insights.** Detectors and the Insights report read rows, never
files. Trigger is "rows changed for this session", debounced, promoted to
immediate when that session's drilldown is open, plus a report recompute when
the queue drains. Once it reads rows its cost is O(rows), so it needs no
activity tiering of its own.

## Decisions

| Decision | Outcome |
|---|---|
| Interim fix before the full design | Yes, phase 1 ships first |
| Filesystem events | Yes, `notify` crate, all platforms; polling stays as reconciliation |
| CPU budget | Defaults in each phase, tuned by feel afterwards |
| Provider-database agents | Same tiers; row cursor instead of byte offset |
| HUD discovery walker | Folded into the shared liveness signal |

## Phases

Each phase is one pull request, mergeable on its own, in dependency order.

### Phase 1: describe only on change, scan while hidden

Goal: fix the pop-in without the new architecture.

- In the scan pass, skip the head read for a file-backed session whose
  parent and child sizes match the stored activity cursor. Reuse the stored
  record. The cursor is already sizes-only and already trusted to skip the
  semantic-timestamp derivation; this extends the same trust to the whole
  description. Sessions without a stored record, and provider-database
  sources, are described as today.
- Remove the popover-visibility gate from the scheduler. The 60-second tick
  runs while hidden. Showing the popover still kicks a pass as
  reconciliation.
- The popover refetches the session list on `popover:shown`, as a cheap
  defence against a missed event while hidden.
- Test: a second pass over an unchanged transcript performs no head read
  (the discovery crate's tracked-head-read instrumentation exists for this).

Known limit, accepted: a title change in a vendor index while the transcript
size is unchanged is not picked up until the transcript grows. Phase 2
replaces the size gate with a verified offset.

### Phase 2: verified-resume ingest

Goal: an appended 4 KB costs 4 KB.

- Persist per source (parent and each child) the consumed byte offset and a
  hash of the trailing window before it. On the next change, re-hash that
  window; on match, stream from the offset and continue the turn index; on
  mismatch, full pass.
- Provider-database sources persist a row cursor on the vendor's update
  column and ingest rows after it.
- Fence semantics change: the published row set spans passes. The claim fence
  still guards a full pass; an incremental pass appends under the published
  fence inside one transaction. The schema revision and the
  `published_turn_rows` contract are updated together.
- A parser or analyzer revision bump invalidates every cursor and forces a
  full pass.
- Turn the `AppendOnlyGuarantee` from a static assumption into the runtime
  verification above.
- Test: parity. Incremental ingest over a transcript grown in steps produces
  the same rows as one full pass over the final file, for each vendor.

### Phase 3: evidence from rows

Goal: layer 2 never opens a transcript.

- Parse-time facts the evidence accumulator needs (unrecognized record types,
  coverage reasons, diagnostic fields) are written by layer 1 as a small
  per-session coverage record alongside the rows.
- An evidence replay builds `SessionEvidence` from rows plus that record, with
  a parity test against the live fold, mirroring the metrics replay.
- The worker's stream fold stops accumulating evidence. Detectors run from
  rows on a debounced "rows changed" trigger, immediate for an open
  drilldown. The Insights report recomputes when the queue drains, as today.

### Phase 4: watcher tiers, events, and the HUD fold-in

Goal: layer 1 is continuous and cheap, and every consumer reads one signal.

- Add `notify`. Watch each agent's roots. Classify events into "new source
  under a root" and "known source changed". Debounce writes.
- Polling tiers as reconciliation and fallback: active sources stat every
  few seconds, inactive every minute, new-source leaf-directory check every
  few seconds, full walk hourly and on demand. Per-agent leaf strategies
  replace the unwindowed Codex tree walk and the Claude subagent sweep. WSL
  paths get a slower tier.
- A discovery-level per-session event to the webview carries the refreshed
  `ActivityEntry`, so the list patches one row instead of refetching.
- Active-to-idle expiry as a backend timer, emitted as the same event.
- The HUD's `latest_session_activity` becomes a query on the session table
  or a subscription to the expiry signal; its discovery walk is deleted.
- The detail pane's 10-second fingerprint poll is replaced by the row-change
  event.

## Sequencing notes

- Phase 1 stands alone and is reverted by two small edits if it misbehaves.
- Phase 2 is the enabling work; phases 3 and 4 both depend on it and are
  independent of each other.
- Default cadences are set in phase 4 and reviewed after a week of use.
