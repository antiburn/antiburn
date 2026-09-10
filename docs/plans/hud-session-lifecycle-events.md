---
title: "HUD sees a session write within two seconds, and blinks even at zero usage"
created_at: "2026-09-08"
status: draft
---

# HUD sees a session write within two seconds, and blinks even at zero usage

- **Date:** 2026-09-08
- **Branch:** `feat/session-lifecycle-bus` (the bus), with
  `feat/hud-session-blink` (the surfaces) stacked on it. The original build
  branch `claude/hud-active-session-notification-cbf342` holds the same work
  as three phase commits.
- **Origin:** Keith's HUD note plus Chris's Slack idea (2026-09-08): fire
  `session_active` / `session_cold` style events from the session watching
  code through a single-producer, multi-consumer channel, so the HUD and the
  session list stop working out "active" for themselves.
- **Status:** draft, awaiting review.

## Status

| Phase | What | Size | State |
|---|---|---|---|
| A | Leftmost LED blinks when nothing is lit | ~60 lines | built 2026-09-08, in review |
| B | `SessionLifecycle` actor and broadcast bus in the Tauri shell | ~400 lines | built 2026-09-08, in review |
| C | HUD and popover meter read the bus; the idle task folds into the actor | ~300 lines | built 2026-09-08, in review |
| D | Session list and other consumers move onto the bus | follow-up | not planned here |

Two pull requests (agreed 2026-09-08). The first, `feat/session-lifecycle-bus`,
is the wiring: phase B and the shell half of phase C (the bridge to the
webview, `get_live_sessions`, the idle task folded into the actor). It keeps
`get_latest_session_activity` so the HUD on `main` still works against it.
The second, `feat/hud-session-blink`, stacks on the first and holds the
surfaces: phase A, the renderer half of phase C, and the ring blink. It
removes `get_latest_session_activity`.

## Two problems

**1. Low usage hides the live blink.** The HUD blinks the last lit LED
while a session is live. (Phase C later moves the blink to the next unlit
LED; see its departures.) With 20 segments, the first segment lights only
when usage passes 2.5 percent. Below that `litCount` is zero, no segment
gets `led-blink`, and a fresh window or a light day shows nothing at all.
The empty-bars branch (no usage snapshot yet, or every meter turned off)
never passes `blinkLast` either.

Source: `apps/desktop/src/components/ui/LedBar.tsx` (`litCount`,
`blinkIndex`) and `apps/desktop/src/views/OverlayWindow.tsx` (the
`state.bars.length === 0` branch).

**2. "Live" is derived in three places, and lands late.** There is no
in-process event bus. Every producer emits the Tauri event
`sessions:entry-changed` straight to the webview, and the same announce
closure is copied at five sites (`scan/mod.rs`, `scan/idle.rs`,
`scan/scoped.rs` twice, `insights_worker.rs`). The HUD's `OverlaySession`
takes each entry's timestamp, applies its own 90 second window, and runs its
own expiry timer. The popover applies a 180 second window at read time. The
backend idle task in `scan/idle.rs` applies 180 seconds again.

Latency today, from a transcript write to the HUD blink:

| Step | Cost |
|---|---|
| `notify` event to debounced burst | 1.5 s quiet, 5 s max under a steady stream |
| Targeted refresh per session floor | up to 10 s (`TARGETED_MIN_INTERVAL`) |
| Re-describe (stat, head read) and upsert | tens of ms |
| `sessions:entry-changed` to `sessionLive` | immediate |

So the first write of a burst reaches the HUD in about two seconds, but every
later write in a busy session waits on the 10 second floor plus a describe.
The liveness signal is riding on the row-refresh pipeline when it only needs
to know "this file was just written".

## What we keep

The continuous-ingest work (`docs/plans/continuous-session-ingest.md`,
done) already built most of the machinery:

- A `notify` watcher with a debounced burst per agent root (`scan/watch.rs`).
- Burst classification into known session, new session, title-only, and
  database-agent lanes (`scan/scoped.rs`, `classify_burst`).
- A backend idle-expiry task keyed on `ACTIVE_SESSION_WINDOW_SECS` = 180
  (`scan/idle.rs`).
- A push path into the HUD (`OverlaySession.listenForActivity`).

This plan adds one thing in the middle: a typed lifecycle bus. It does not
touch discovery, describe, the store schema, or the popover.

## Design

### Phase A: the leftmost LED blinks when nothing is lit

In `LedBar.tsx`, when `blinkLast` is true and `litCount` is zero, blink
segment 0 instead of nothing.

The blink also changes colour. Today the last lit segment alternates between
the provider colour and the unlit grey, so it reads as the bar being one
segment shorter half the time. "Live" is a different fact from "used", so
the blink takes the `brand-tint` token (antiburn orange, one value in both
themes, from `apps/desktop/design.md`) as its on state, and the segment's resting colour
as its off state: the provider colour when lit, `led-off` grey when not. One
keyframe rule covers every case, and no new token enters the contract.

In `OverlayWindow.tsx`, the empty-bars branch passes
`blinkLast={state.sessionLive}` so a live session with no usage snapshot
still shows.

`HudDetailView` stays as it is: `docs/hud-states.md` says only the HUD
blinks.

Tests, in `OverlayWindow.test.tsx` next to the existing blink cases:

- live session, usage below one segment: exactly one `.led-blink`, at index 0
- live session, no bars: exactly one `.led-blink`, at index 0
- not live, usage below one segment: no `.led-blink`

### Phase B: one `SessionLifecycle` actor, one broadcast bus

A new module, `apps/desktop/src-tauri/src/session_lifecycle.rs`, owns the
"which sessions are live" question for the whole shell.

**The bus.** One `tokio::sync::broadcast::Sender<SessionEvent>` held in
Tauri managed state as `SessionEvents`. Any part of the shell can
`subscribe()`. Chris's link (mpsc, broadcast, watch) lands on broadcast: many
readers, each gets every event, a slow reader lags and is told so instead of
blocking the producer.

```rust
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// The store indexed this session for the first time.
    Started { session: SessionKey, agent: AgentKind, at: i64 },
    /// A write to this session's source was observed.
    /// `session` is None when the path is under an agent root but the
    /// store has not indexed the session yet.
    Activity { session: Option<SessionKey>, agent: AgentKind, at: i64 },
    /// The session crossed ACTIVE_SESSION_WINDOW_SECS without a write.
    Idle { session: SessionKey, agent: AgentKind, at: i64 },
}
```

**Single producer.** Only the actor task sends on the bus. Everyone else
reports observations to the actor over an `mpsc` channel:

```rust
pub enum Observation {
    /// A watcher burst touched a path that maps to this session or agent.
    Touched { session: Option<SessionKey>, agent: AgentKind, at: i64 },
    /// A scan pass upserted these sessions; `new` marks first-time keys.
    Indexed { sessions: Vec<(SessionKey, AgentKind, i64)>, new: Vec<SessionKey> },
}
```

The actor keeps a map of live session key to last-activity epoch. On
`Touched` it publishes `Activity` and re-arms its expiry timer. On `Indexed`
it publishes `Started` for each new key and `Activity` for the rest. When
the soonest deadline passes it publishes `Idle` for each expired key. This
is the same loop `scan/idle.rs` runs today, moved into the actor and fed by
observations instead of a store re-read on every wake. The store query
`sessions_active_since` seeds the map once at launch.

**Where observations come from.**

- The scheduler loop in `scan/mod.rs` runs `classify_burst` on each burst
  it takes from `ScanController`. Right after classification, before any
  floor or describe, it reports one `Touched` per
  classified session (the T1 lane) and one `Touched { session: None }` per
  agent in the new-session and database-agent lanes (T3, T5). This is the
  realtime path: a write reaches the bus about 1.5 seconds after it happens,
  or every 5 seconds under a steady stream, with no describe in between.
- `scan::pass` and the scoped refresh report `Indexed` after their upsert,
  which is where `Started` comes from.

**Tests** for the actor, driven under `tokio::time::pause()` the way
`scan/idle/tests.rs` does:

- `Touched` publishes `Activity` with the same key and time
- a first `Indexed` publishes `Started` once, a second does not
- a key with no touch for 180 s publishes `Idle`, and a touch at 170 s
  moves that deadline
- a subscriber that joins late gets the seeded live set through a snapshot
  command, not a replay

Phase B ships with the bus wired and tested but no consumer changed. The
existing `sessions:entry-changed` emits stay exactly where they are.

Built 2026-09-08, three small departures from the sketch above:

- Events carry a `SessionRef` (`environmentKey`, `agent`, `sessionId`) rather
  than the store's `SessionKey`. `store/model.rs` keeps storage shapes off the
  wire, so the wire shape lives in `session_lifecycle.rs`.
- `Started` is published only when the key is new to the store and not yet
  in the live map, so a repeated `Indexed` for the same rows says nothing.
  A pass reports only its changed rows inside the 180 s window, and the
  actor ignores an epoch older than the one it holds, so a tick's full pass
  does not replay activity.
- One subscriber ships now: a debug-level log of every event
  (`session_lifecycle_event`), which also shows the lag handling. Without a
  subscriber the repo's no-dead-code rule fails the build, and the log is
  how the running app can be checked before phase C.

### Phase C: the HUD reads the bus, the idle task folds in

**Bridge to the webview.** One task subscribes to the bus and emits every
event as `session:lifecycle` to the overlay window. It also emits
`sessions:entry-changed` for `Idle` events using `completion_entry`, which
is what `scan/idle.rs` does today. Then `scan/idle.rs` is deleted and
`IdleWake` becomes `Observation::Touched`. The popover sees the same events
it saw before.

**HUD.** `OverlaySession.listenForActivity` subscribes to
`session:lifecycle` instead of `sessions:entry-changed`. `Activity` extends
the deadline, `Idle` for the last live key turns the blink off. A new
command `get_live_sessions` returns the actor's map as the snapshot on
activation, replacing `get_latest_session_activity` for the HUD. The
snapshot also gives the HUD a live count for later.

The HUD drops its own 90 second timer. The blink follows the bus: on at
`Activity`, off at `Idle`, so one 180 second window applies everywhere.
`docs/hud-states.md` is updated in the same change.

The popover's usage meter (`SegmentedMeter`) gets the same blink from the
same events, so the main window and the HUD agree. It subscribes to
`session:lifecycle` through the popover's existing event plumbing and uses
the brand-on, resting-off rule from phase A.

**Tests** in `OverlayWindow.test.tsx` and `OverlaySession` tests:

- an `Activity` event sets `sessionLive` and starts the blink
- an `Idle` event clears it
- activation with a non-empty snapshot starts live

Built 2026-09-08, three small departures:

- A keyless `Activity` (a write under an agent root the store has not
  indexed yet) has no `Idle` counterpart on the bus, so the renderers keep
  one local 180 second timer for that case only. A keyed session never runs
  a renderer timer.
- Both meters blink the *next unlit* segment, not the last lit one: a lit
  segment already carries a colour close to the brand tint, so only the
  segment past the reading can alternate (brand on, the unlit colour off).
  The Claude bar colour is 3 degrees of hue and 1 point of lightness from
  the brand tint, which a 6px HUD dot cannot show, so the last-lit rule from
  phase A made the HUD blink invisible. At zero both rules land on the first
  segment, which keeps phase A's low-usage case. On the closed bar a
  provider's ring blinks the next eighth past the arc's end (agreed
  2026-09-08, "yeah blink them"). A thirty-second of a 26px ring is two
  pixels, so the ring's blink is an eighth.
- Every meter of the provider a live session draws on blinks, not only the
  first (agreed 2026-09-08: "flash all the usage quotas/limits that are
  being effected"). The renderer maps the agent slug to its provider in
  `sessionLiveness.ts`, mirroring the fixed routes in `providers.rs`, and
  both snapshots carry `liveProviders` beside `sessionLive`. Within a
  provider the rows turn on from the top down, 250 ms apart, and turn off
  together: `hud.css` holds one keyframe set per step, because
  `animation-delay` would move the off edge too. A live agent with no
  provider on the limits surfaces (Cursor, Copilot) blinks nothing on the
  bars; the empty HUD bar still blinks for it.
- Both renderers re-read `get_live_sessions` on `scan:finished` and
  `sessions:invalidated` (the popover also on show), so a lagged bus reader
  recovers within one pass.

### Phase D: follow-ups, not in this plan

- The popover's session list reads `Started` and `Idle` from the bus for
  row state, dropping its read-time `is_active` computation.
- The five copied announce closures become one bridge consumer.
- `notifications.rs` and the nudge crate can subscribe for "first session
  of the day" style prompts.
- The Slack thread mentions actor frameworks. The actor here is a plain
  task with an `mpsc` inbox and a `broadcast` outbox. That is the shape an
  actor crate would give us, without the dependency. Revisit only if a
  third actor appears.

## Decisions to confirm

| Decision | Proposal |
|---|---|
| Blink colour | brand orange as the on state in every case, alternating with the segment's resting colour (agreed 2026-09-08) |
| Live window and surfaces | one 180 s window from the bus, no separate 90 s timer; the HUD, the popover's usage meter, and the closed bar's ring all flash (agreed 2026-09-08; "main one" is the popover) |
| Discovery paused | the watcher still runs, so the HUD still blinks while paused; paused means no indexing work, not a dark light (agreed 2026-09-08) |

## How it is built

Engineering choices that need no product input:

- Bus type: `tokio::sync::broadcast`, capacity 64. Every subscriber sees
  every event. A subscriber that falls 64 behind is told it lagged, logs
  it, and re-reads its snapshot.
- Who sends: only the actor. Producers report `Observation`s over `mpsc`, so
  one place decides every transition.
- `Touched` timing: at burst classification, before the per-session floor
  and before any describe. A write reaches the bus in about 1.5 s.
- `scan/idle.rs`: deleted in phase C. Its expiry loop lives in the actor,
  and the popover receives the same `sessions:entry-changed` it does today.

## Out of scope

- Any change to discovery, describe, snapshot resume, or the store schema.
- The popover session list (phase D).
- Notifications for session start or stop.
- Changing `ACTIVE_SESSION_WINDOW_SECS`.
