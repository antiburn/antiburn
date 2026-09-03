---
title: "Continuous session ingest: watch, resume, and replay"
created_at: "2026-09-02"
status: done
---

# Continuous session ingest

- **Date:** 2026-09-02
- **Issue:** none. Opened from a popover staleness report.
- **Status:** every phase merged on 2026-09-03 (#343, #345, #350, #353, #354,
  #355, #356). Open follow-ups are listed at the end.

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

### Phase 1: describe only on change, scan while hidden (done)

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

### Phase 2: evidence from rows (done)

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

### Phase 3: verified-resume ingest (done)

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

#### Phase 3a: snapshot resume, crate level (done)

- Snapshot types, an offset reader with tail verification, a
  `VendorAdapter::visit_claimed_resumed` seam, and a Claude adapter
  snapshot.
- Test: parity. Incremental ingest over a transcript grown in steps produces
  the same rows, metrics, summary, and evidence as one full pass over the
  final file.
- Codex, Pi, and the provider-database adapters report resume unsupported;
  a caller falls back to a full pass for them.

#### Phase 3b: snapshot resume, desktop level (done)

- A `source_resume` table persists each source's snapshot. The worker calls
  the new seam and falls back to a full pass when it returns unsupported or
  the offset reader detects a change.
- Fence semantics change: the published row set spans passes. The claim
  fence still guards a full pass; an incremental pass appends under the
  published fence inside one transaction. The schema revision and the
  `published_turn_rows` contract are updated together.
- A parser, analyzer, metrics, evidence, or coverage revision bump
  invalidates every snapshot and forces a full pass.
- Not done: `AppendOnlyGuarantee` is still the static `Absent` stub. The
  resumed path verifies its offset with the tail hash from phase 3a and
  never consults the guarantee, so it now only decides how a full read
  re-checks its prefix. Listed as a follow-up below.

Design rules the 3b code comments cite by number:

- **R1. Per source.** A snapshot is stored per `(session key, source_key)`,
  where `source_key` is the parent's session id or a child's own id, the
  same value the turn rows carry. Each source independently resumes
  (appends its new rows) or reads fully (replaces its rows).
- **R2. Resume conditions.** A source resumes only when a stored snapshot
  exists, its six revision columns all equal the current constants, it
  decodes, the adapter supports resume, and `visit_claimed_resumed` accepts
  it. Otherwise the source reads fully. A `SourceChanged` outcome from the
  resumed visit falls back to a full read of that source in the same pass.
- **R3. Evidence fold is rebuilt every pass.** One evidence accumulator per
  input. At the end of the pass the parent's residual is cloned and every
  child's residual folded into the clone, in input order, to produce the
  coverage record. The per-source snapshot stores each source's own
  residual, never the folded one.
- **R4. Fence semantics.** A resumed source's new rows are written under the
  claim fence, then re-stamped onto the session's existing published fence
  inside the publish transaction. A source that read fully has its old rows
  under the published fence deleted first, then its new rows re-stamped the
  same way. `published_fence` stays put once set and becomes the claim fence
  only the first time a session publishes. On a lost race the claim fence
  still identifies exactly this pass's rows. The coverage record is
  upserted under the published fence.
- **R5. Snapshot storage.** The `source_resume` table (migration v30) holds
  one row per source: the snapshot blob, the six revisions it was captured
  under, a descriptive fingerprint, and a write time, with the same cascade
  from `session` that `turn` has. A row is written only inside a winning
  publish transaction. A full read that yields no resume state deletes the
  source's row.
- **R6. Invalidation.** A revision mismatch is decided at read time, so a
  build with a bumped revision never resumes from old rows. A startup sweep
  next to `reconcile_evidence_revisions` deletes every stale row.
- **R7. Queueing is unchanged.** The scan pass still marks evidence pending
  on a fingerprint or cursor change; the worker claims as before.

#### Phase 3c: Codex and Pi snapshots (done)

- Extended `visit_claimed_resumed` to `CodexAdapter` and `PiAdapter`,
  covering Codex's pending fork-row buffer. The resume parity harness now
  sweeps every Codex and Pi characterization fixture, not just Claude's.
- Codex unsettled rule: a fork sub-agent rollout can still have
  `ForkOwnership::Pending` at end of stream. `finish` still flushes those
  buffered rows as owned, so a settled read publishes every row, but a
  later full pass could resolve the same rows differently. So
  `visit_claimed_resumed` reports `resume: None` whenever ownership is
  still `Pending` right before `finish`, even though the read still
  settles. The next change to that source costs one full pass instead of
  a resume.

### Phase 4: watcher tiers, events, and the HUD fold-in (done)

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

### Phase 5: scoped passes (5a and 5c merged 2026-09-03; 5b in review; 5d planned)

Goal: a watcher event costs work proportional to what changed, not a
whole-machine pass. Found on 2026-09-03 after phase 4 had run for a day:
the log showed a full pass every 1.5 s for over an hour, and antiburn sat at
55 to 80 percent of a core.

Two causes, one bug and one design gap:

- Bug: every pass opens Cursor's `state.vscdb` databases read-only. They are
  in WAL mode, and a WAL reader takes read marks in the `state.vscdb-shm`
  sidecar, which bumps its mtime. That sidecar sits under a recursive Cursor
  watch root, `is_relevant` only drops `Access` events, so the pass kicked
  itself: modify, 1.5 s quiet, pass, modify. The gap histogram made it
  unambiguous: about 2240 of roughly 2550 consecutive gaps were exactly the
  quiet window, and one gap in the hour exceeded 6 s.
- Design gap: a watcher kick ran the same pass as the 60 s tick. That pass
  walks all 11 agents, loads every stored record, stats every source, upserts
  every row, refreshes the repository list, and its `scan:finished` makes the
  popover refetch the list, recompute 30-day usage totals, and resolve both
  live provider accounts. An active Claude Code session writing sub-agent
  transcripts every couple of seconds keeps that going at the 1.5 s to 5 s
  cadence with no bug involved.

The wanted behaviour, from the original planning discussion:

- An active session's row updates often: every 5 to 20 s, not every 1.5 s.
- An inactive session costs nothing until it becomes active again.
- A new session is detected when it is created.

None of that needs full discovery. The event path names the session or the
agent, and the pieces to act on it narrowly already exist: per-row patching
through `sessions:entry-changed`, fingerprint reuse in
`reuse_unchanged_record`, per-agent `discover_recent`, `list_changed`, and
`Explorers::infer_agent_and_surface` to map a path to its agent.

#### Phase 5a: stop the self-trigger, name every trigger

- W1. `is_relevant` in `scan/watch.rs` drops an event whose every path ends
  in `-shm`. That sidecar is the WAL index, which readers modify; `-wal` is
  written only by a real writer and stays relevant, because a WAL database's
  committed rows live there until a checkpoint. Provider database opens are
  left as they are: `immutable=1` would hide un-checkpointed rows and
  `locking_mode=EXCLUSIVE` would hold a shared lock against the vendor.
- W2. The debounce loop collects the relevant paths of each burst and hands
  them to the kick, deduplicated, instead of a bare `()`. Phase 5b consumes
  them; 5a logs them.
- W3. Every pass request carries a `ScanTrigger`: launch, tick, watcher
  (with the burst's path count and a bounded sample of paths), popover shown,
  settings transition, insights pane, repository toggle, scan root added,
  folder access granted, index cleared, manual rescan. The scheduler logs
  `scan_pass_requested` with the trigger at debug, and `run_pass` logs
  `scan_pass_started` and `scan_pass_finished` with the trigger, duration in
  milliseconds, sessions persisted, rows re-described, and `list_changed`.
  This is the instrumentation that was missing when the loop was diagnosed
  from `repo_discovery_agent_done` lines alone.
- W4. A request that arrives while a pass is running is still dropped, but
  the drop is logged with its trigger, so a hidden feedback loop shows up as
  a stream of drops rather than silence.

#### Phase 5b: targeted refresh and per-agent rediscovery

The watcher's burst is classified path by path, and each class runs the
narrowest pass that answers it. All passes stay serialized through the
scheduler task, so passes never overlap and the store sees one writer.

- T1. Known session. A path that equals a stored native file session's
  `source_label`, or whose ancestor directory `D` has `D.jsonl` as a stored
  `source_label` (the sub-agent layout), names that session. The targeted
  pass builds the `SessionLog` from the stored record, runs
  `describe_with_states` over just those logs with just their previous
  records, upserts the resulting rows, announces the re-described ones
  through `sessions:entry-changed`, and wakes the insights worker and the
  idle task. It does not run discovery, does not refresh repositories, does
  not rewrite per-agent scan bookkeeping, and does not emit `scan:started`
  or `scan:finished`.
- T2. Per-session floor. A session re-described at `t` is not re-described
  again before `t + TARGETED_MIN_INTERVAL` (10 s). Events inside the floor
  are coalesced into one deferred refresh at the floor's end, never dropped,
  so a session that keeps writing is refreshed every 10 s and a session that
  writes once is refreshed once.
- T3. New session. A path under an agent's watch root that matches no stored
  session means that agent has a session the store has never seen. The
  agent-scoped pass runs `discover_recent` for that one agent, describes the
  result against the stored records, upserts, and goes through the normal
  `run_pass` status machinery, so `scan:finished` with `list_changed` makes
  the popover refetch the list. Discovery for the other ten agents does not
  run.
- T4. Per-agent floor. Agent-scoped rediscovery for one agent runs at most
  once per `AGENT_REDISCOVER_MIN_INTERVAL` (20 s), coalesced the same way as
  T2.
- T5. Database-backed agents. For Cursor, OpenCode, Antigravity, Windsurf,
  and Kiro the changed path is a database, not a session, so every event
  under their roots is a T3 rediscovery of that agent, with a longer floor,
  `DB_AGENT_REDISCOVER_MIN_INTERVAL` (30 s).
- T6. Unclassified paths (no agent root claims them) are ignored; the 60 s
  tick reconciles. The tick, the popover-shown kick, the manual rescan, and
  every command kick keep running the full pass exactly as before.
- T7. A burst that arrives while a pass is in flight is held, merged with any
  later bursts, and processed when the scheduler is free, rather than dropped.

#### Phase 5c: refresh scaling and the Cursor walks

- F1. The popover's `scan:finished` handler refreshes usage at most once per
  `USAGE_REFRESH_MIN_MS` (30 s) unless the pass reported `list_changed`.
  Row patches from `sessions:entry-changed` never trigger a usage refresh.
- F2. Cursor's `collect_agent_transcript_dirs` and
  `collect_cursor_chat_metadata` bound their walk by depth to the documented
  layout (`projects/<project>/agent-transcripts`,
  `chats/<workspace>/<chat>/meta.json`) instead of the mtime-gated pruning
  the other agents got in 4a, so a T5 rediscovery of Cursor does not pay for
  a full walk. Mtime gating is wrong here: a new session file bumps only its
  immediate parent's mtime, so an old project directory would hide every
  session added to it since.

#### Phase 5d: the tick and the popover stop being the freshness path

Found on 2026-09-03 from a live log of the 5b build: watcher bursts were
cheap (a targeted refresh took about 90 ms), but the 60 s tick and every
popover open still ran a full pass. Three popover opens in 13 s ran three
full passes of 1.8 to 2.1 s each, contending with the popover's own render
and usage recompute. Both were insurance from before the watcher existed.
What the tick uniquely covers now is WSL sessions (never watched) and a rare
FSEvents drop under load; a degraded watcher already falls back to the 15 s
tick, and the watcher re-registers new agent roots itself.

- R1. Opening the popover does not request a scan. `note_shown` still emits
  `popover:shown`; the `PopoverShown` trigger is removed as dead code.
- R2. `TICK` is 5 minutes. `FALLBACK_TICK` stays 15 s for a degraded watcher.
- R3. A full pass writes only the rows the describe step changed (new, or
  cursor moved), plus any reused row whose evidence is `failed` with
  `source-missing`, so a returned source still re-queues. An idle tick is
  stat calls only. Per-agent scan bookkeeping still counts every record.
- R4. A full pass refreshes repositories only when `list_changed`, or when
  the trigger is one that can change the repository set: launch, settings
  transition, insights pane, repository toggle, scan root added, folder
  access granted, index cleared, manual rescan. The tick and the watcher
  triggers do not.
- R5. `ScanStatus` carries `re_described`. The popover's `scan:finished`
  handler refreshes usage only when `list_changed`, or when the pass
  re-described at least one session and `USAGE_REFRESH_MIN_MS` has elapsed.
  A pass that changed nothing triggers no usage refresh.
- R6. Usage freshness while the popover is visible no longer rides on the
  scan. The shell emits `popover:hidden` from `note_hidden`. The popover
  refreshes usage on a 60 s interval while visible (started on shown,
  stopped on hidden, matching `POPOVER_LIVE_USAGE_MAX_AGE`), and on a
  `sessions:entry-changed` event under the same 30 s floor, so an active
  session's totals stay current without a scan pass.

## Sequencing notes

- Phase 1 stands alone and is reverted by two small edits if it misbehaves.
- Phase 2 depends only on phase 1.
- Phase 3 is the enabling work for phase 4.
- Default cadences are set in phase 4 and reviewed after a week of use.
- Phase 5a stands alone. 5b stacks on 5a (it consumes the burst paths 5a
  passes through). 5c's two items are independent of each other and of 5b.
- 5d stacks on 5b: it changes the same scheduler and the popover's scan
  handler.
- 5b keeps the tick as a fixed deadline that only a full pass resets, so
  scoped wakes every few seconds cannot starve reconciliation. A burst at
  the watcher's path bound forces a full pass, since paths were dropped.

## Follow-ups

- Cursor's `collect_agent_transcript_dirs` and `collect_cursor_chat_metadata`
  were unbounded recursive walks. Done in phase 5c (F2) by bounding each
  walk's depth to the documented layout, not by mtime, since a new session
  file bumps only its immediate parent's mtime.
- `AppendOnlyGuarantee` is still hard-coded to `Absent`. The resumed path no
  longer needs it; either evidence it per agent or remove it and let a full
  read always re-check its whole prefix.
- Provider-database agents (OpenCode, Antigravity) persist no row cursor. A
  change still costs a full re-stream of the session's rows.
- The phase 4 cadences were reviewed after a day, not a week; the result is
  phase 5. The 15 s fallback, 1.5 s / 5 s burst coalescing, and 60 s list
  reconcile stay; the per-session and per-agent floors are new, and 5d moved
  the tick to 5 minutes once the watcher carried freshness.
- While discovery is paused the scan does not run, so the HUD's live signal
  now follows the pause. Decide whether that is the wanted behaviour.
