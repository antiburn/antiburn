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
size is unchanged is not picked up until the transcript grows. Phase 3
replaces the size gate with a verified offset.

### Phase 2: evidence from rows

Goal: layer 2 never opens a transcript.

Phase 2 and phase 3 swap order from the original plan. Verified-resume ingest
cannot skip the transcript prefix while `SessionEvidence` is still folded from
the full record stream, so evidence must come from rows first.

- Parse-time facts the evidence accumulator needs (unrecognized record types,
  coverage reasons, diagnostic fields) are written by layer 1 as a small
  per-session coverage record alongside the rows.
- An evidence replay builds `SessionEvidence` from rows plus that record, with
  a parity test against the live fold, mirroring the metrics replay.
- The worker's stream fold stops accumulating evidence. Detectors run from
  rows on a debounced "rows changed" trigger, immediate for an open
  drilldown. The Insights report recomputes when the queue drains, as today.

### Phase 3: verified-resume ingest

Goal: an appended 4 KB costs 4 KB.

- The unit of resume is a snapshot, not an offset. State lives in adapters
  (per-stream buffers like `ClaudeStreamState`), sinks (a reorder window,
  deferred cache patches, duration heaps), and rows (which cannot rebuild
  every field). A snapshot bundles the adapter and sink state with the
  verified byte offset and a hash of the trailing window before it.
  Restoring it and streaming from the offset must equal a full pass.

Deferred: provider-database sources persist no row cursor here. Their
fingerprint already gates re-streaming, the stream is a query, and the
Antigravity blob hash is a discovery cost that phase 4 tiers address.

#### Phase 3a: snapshot resume, crate level

- Snapshot types, an offset reader with tail verification, a
  `VendorAdapter::visit_claimed_resumed` seam, and a Claude adapter
  snapshot.
- Test: parity. Incremental ingest over a transcript grown in steps produces
  the same rows, metrics, summary, and evidence as one full pass over the
  final file.
- Codex, Pi, and the provider-database adapters report resume unsupported;
  a caller falls back to a full pass for them.

#### Phase 3b: snapshot resume, desktop level

- A `source_resume` table persists each source's snapshot. The worker calls
  the new seam and falls back to a full pass when it returns unsupported or
  the offset reader detects a change.
- Fence semantics change: the published row set spans passes. The claim
  fence still guards a full pass; an incremental pass appends under the
  published fence inside one transaction. The schema revision and the
  `published_turn_rows` contract are updated together.
- A parser, analyzer, metrics, evidence, or coverage revision bump
  invalidates every snapshot and forces a full pass.
- Turn the `AppendOnlyGuarantee` from a static assumption into the runtime
  verification phase 3a adds.

#### Phase 3c: Codex and Pi snapshots

- Extend `visit_claimed_resumed` to `CodexAdapter` and `PiAdapter`, covering
  Codex's pending fork-row buffer.

### Phase 4: watcher tiers, events, and the HUD fold-in

Goal: layer 1 is continuous and cheap, and every consumer reads one signal.

#### Phase 4a: filesystem watchers and discovery pruning (done)

- Added `notify` and a watcher over each agent's `watch_roots` (the
  `AgentExplorer` trait's per-agent recursive or shallow root set). A
  changed path debounces into a `ScanController::request()` kick: quiet
  window 1.5 s, maximum wait 5 s, so a burst of writes collapses into one
  follow-up pass and a continuous stream still kicks at least every 5 s.
  `Access`-only events and WSL-mounted paths are dropped as noise.
- The scheduler's own tick drops from 60 s to a 15 s fallback only while
  the watcher is unhealthy (it never started, or a root failed to watch);
  a healthy watcher keeps the 60 s tick as pure reconciliation. Watched
  roots are re-listed and any new one picked up every 60 s, so an agent
  installed after startup is watched without a restart.
  WSL paths stay on the tick; they are not watched.
- Discovery pruning replaced the three unwindowed walks a frequent pass
  would otherwise repeat: Codex now walks only `YYYY/MM/DD` date
  directories inside the recency window (full-walk fallback for an
  unparseable name), Claude's sub-agent sweep skips a session's
  `subagents/` directory unless its parent is already in the recent set
  or the directory's own mtime is inside the window, and Antigravity
  checks the mtime window before opening a conversation database instead
  of after.
- Follow-up, not done here: Cursor's `collect_agent_transcript_dirs` and
  `collect_cursor_chat_metadata` are still unwindowed recursive walks: a
  Cursor watch delivers change notifications, but a kicked pass still
  pays for the full walk underneath. Needs the same date- or mtime-gated
  pruning the other three agents got in 4a.

#### Phase 4b: per-session event, idle expiry, HUD fold-in (done)

- A scan pass now tracks which sessions it actually re-described versus
  reused, in `scan::describe_with_states`, and announces each changed one
  through the existing `sessions:entry-changed` event
  (`SESSION_ENTRY_CHANGED_EVENT`) instead of leaving the popover to find out
  on its next full refetch. `ScanStatus` gained `list_changed`, true when a
  pass indexed a session the list has never shown or evicted a rejected one,
  so a consumer can tell "the set of rows changed" from "a row's fields
  changed" without diffing the list itself.
- Active-to-idle expiry is a backend task (`scan/idle.rs`): it sleeps until
  the soonest active session's window ends, then announces every session
  that crossed it through the same `sessions:entry-changed` event. An
  `IdleWake` notify lets `scan::pass` re-arm the task's deadline immediately
  after a write, rather than waiting for a stale wake.
  `Store::sessions_active_since` backs the task's read of the active set.
- `hud::latest_session_activity` is now one call to
  `Store::latest_session_activity` — a single `SELECT MAX(updated_at_epoch)`
  query — replacing the discovery walk and its in-memory memoization.
  `get_latest_session_activity` is a sync command now that it no longer
  awaits a walk.
- The popover's detail pane no longer polls a fingerprint. It refreshes,
  coalesced to one extra run per burst, when `sessions:entry-changed` names
  its open subject (or, for a sub-agent, its parent). The activity list
  patches the one row an event describes in place and re-sorts it; a row not
  already on screen triggers a coalesced full refetch instead. `scan:finished`
  drives the list too: `list_changed` refetches immediately, and a pass that
  never sets it still gets reconciled every `LIST_RECONCILE_MS` (60 s, the
  scheduler's own healthy-watcher tick) as a backstop. The
  `get_session_analysis_fingerprint` command, `poll_fingerprint_with_subagents`,
  and their tests are deleted along with the poll.
- Left out: Cursor's `collect_agent_transcript_dirs` and
  `collect_cursor_chat_metadata` are still unwindowed recursive walks — the
  4a follow-up noted above, unrelated to 4b's event and expiry work, and
  still open.

## Sequencing notes

- Phase 1 stands alone and is reverted by two small edits if it misbehaves.
- Phase 2 depends only on phase 1.
- Phase 3 is the enabling work for phase 4.
- Default cadences are set in phase 4 and reviewed after a week of use.
