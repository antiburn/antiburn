---
title: "HUD stops when the session does, and sweeps instead of blinking"
created_at: "2026-09-11"
status: built
---

# HUD stops when the session does, and sweeps instead of blinking

- **Date:** 2026-09-11
- **Branches:** `feat/session-lifecycle-bus` (#489) for the stop fix,
  `feat/hud-session-blink` (#490) for the sweep. Both are drafts, so the
  sweep replaces the blink on its own branch instead of stacking a third.
- **Follows:** `hud-session-lifecycle-events.md` (phases A to C, built
  2026-09-08). This plan changes two things that plan shipped.
- **Status:** approved 2026-09-11 ("commence plan"); E, F, and G built the same day.

## Status

| Phase | What                                                                  | Size           | State                                                                                                   |
| ----- | --------------------------------------------------------------------- | -------------- | ------------------------------------------------------------------------------------------------------- |
| E     | The Claude desktop manifest no longer keeps the HUD live              | ~80 lines Rust | built 2026-09-11 on `feat/session-lifecycle-bus`                                                        |
| F     | A brand-tint sweep across the live provider's bars replaces the blink | ~250 lines     | built 2026-09-11 on `feat/hud-session-blink`                                                            |
| G     | The sweep ends 30 s after the last write; `idle` stays at 180 s       | ~60 lines      | built 2026-09-11: actor half on `feat/session-lifecycle-bus`, renderer half on `feat/hud-session-blink` |
| H     | A model-scoped meter sweeps only for the model that is running        | ~350 lines     | built 2026-09-11 on `feat/hud-session-blink`                                                            |

## Problem 1: the HUD never stops

**The logic today.** A provider's meters animate while any session of that
provider is live. A session is live from its first transcript write until
180 seconds pass with no write (`ACTIVE_SESSION_WINDOW_SECS`, the same
window the session list uses for its active pill). The shell's lifecycle
actor keeps that clock and publishes `idle`. One more path exists: a write
under an agent's watch root that matches no indexed session is reported as
_keyless_ activity, and the renderer counts the agent as live for 180
seconds after it. That path exists so a brand-new session shows within
about 1.5 seconds, before the next discovery pass indexes it.

**What happens.** The Claude desktop app rewrites its session manifest
every 30 seconds while a Code tab is open, working or not:

```
~/Library/Application Support/Claude/claude-code-sessions/<tab>/<pane>/local_<id>.json.tmp
~/Library/Application Support/Claude/claude-code-sessions/<tab>/<pane>/local_<id>.json
```

That tree is a Claude watch root (`ClaudeExplorer::watch_roots`). The
manifest is not a transcript, so it matches no indexed session, and the
classifier files it under the keyless lane. Every 30 seconds the renderer
gets a keyless `activity`, pushes the agent's expiry 180 seconds out, and the
sweep never ends. From the debug build's log, one burst per 30 seconds:

```
21:52:14 scan_burst_classified sessions:0 agents:1 paths: .../local_990b88ca-....json.tmp, .../local_990b88ca-....json
21:52:14 session_lifecycle_event Activity { session: None, agent: Claude, at: 1789077134 }
21:52:44 scan_burst_classified sessions:0 agents:1 paths: .../local_990b88ca-....json.tmp, ...
21:52:44 session_lifecycle_event Activity { session: None, agent: Claude, at: 1789077164 }
```

Source: `~/Library/Logs/antiburn-debug/antiburn.2026-09-10-21.log`. The
keyed path is fine: the same log shows `Idle` for this session's transcript
180 seconds after its last write.

The other agents watch transcript trees only (Codex watches
`~/.codex/sessions/`, for example), so the Claude manifest is the one quiet
writer under a watch root today. The database-backed agents (Cursor,
Windsurf) are a separate case: a write to their store touches every session
in it. They have no meter on the HUD, so they are out of scope here.

## Problem 2: a blink is not the right shape

The blink turns one segment on and off. Keith wants a _swish_: a highlight
that runs left to right across the whole bar, the way the session list's
title shimmer runs across a running session's title
(`activity-row-title-shimmer` in `session-rows.css`). The sweep says
"working" without pointing at one segment, and it reads on every bar length.

## Design

### Phase E: a quiet path rediscovers but is not activity

The scanner's classifier (`scan/scoped.rs`) sorts each path in a burst into
a lane. A path under a watch root with no indexed session becomes
`ClassifiedPath::Agent`, which does two things: it schedules a rediscovery
for that agent, and it reports keyless activity to the bus. The manifest
needs the first (a new desktop session starts with a new manifest) and must
not do the second.

- `Explorer` gets `fn is_quiet_path(&self, path: &Path, home: &Path) -> bool`,
  default `false`. `ClaudeExplorer` returns `true` for any path under
  `claude-code-sessions`.
- `classify_path` returns a new `ClassifiedPath::QuietAgent(agent)` for a
  quiet path; `ScopedWork` gets `quiet_agents`. The scheduler admits
  `quiet_agents` exactly as it admits `agents` (same floor, same
  `rediscover_agents` call). `report_touched` reports `agents` and
  `db_agents` and skips `quiet_agents`.
- No change to the bus, the actor, or the renderers.

Tests: `classify_burst` files a manifest write as quiet; a burst with the
manifest and a transcript reports the transcript only; `report_touched`
sends nothing for a quiet-only burst. Run the session-lifecycle tests.

Alternative, if a smaller cut is preferred: drop keyless activity from the
renderers (`anonymousUntil` and its timer in `sessionLiveness.ts`,
`OverlaySession.ts`, `PopoverSession.ts`, about 40 lines removed). That
closes the whole class but a new session then waits for its first discovery
pass, up to 20 seconds, before the HUD shows it. Phase E keeps the 1.5
second path for real transcripts.

### Phase F: the sweep

**What it looks like.** A soft band of the brand tint, about six dots wide
on the HUD's 20-dot bar, slides from the left end to the right end across
every dot, lit and unlit, then the bar rests. The band is the sweep's only
mark: no dot is on or off by itself. One pass takes about 1.3 seconds and
the cycle is 4 seconds, the shimmer's numbers, so the HUD and the session
list move at one pace. The rows of one provider run 100 milliseconds apart
from the top, as now, so three Anthropic bars show one diagonal wave. The
band travels in the fill direction, which is left to right on the HUD and
`fillFrom` on the popover's meters.

On the closed bar a provider's ring gets the same band as an arc that runs
one lap clockwise around the track each cycle. The popover's usage meters
get the same treatment as the HUD bars.

**Mechanics.** One shared clock, as the blink has today, so a row that
starts late is in phase. The `.led-clock` ancestor animates one registered
custom property, `--led-sweep`, from before the left end to past the right
end of the bar and then holds it there for the rest phase. Each dot knows
its index and its bar's segment count, and paints the band on a pseudo
element whose opacity falls off with its distance from the sweep position:

```css
.led-sweep-dot::after {
  /* Distance from the band's centre, in dots. */
  --led-pos: calc(
    (var(--led-sweep) - var(--led-row) * var(--led-row-lag)) *
      var(--led-segments) - var(--led-index) - 0.5
  );
  opacity: clamp(0, 1 - max(var(--led-pos), calc(-1 * var(--led-pos))) / 3, 1);
  background: var(--color-brand-tint);
}
```

The ring reads the same `--led-sweep` and rotates its arc by it. `max(x, -x)`
stands in for `abs()`, which older WebKit lacks. `@property` makes
`--led-sweep` interpolate smoothly; without it (WKWebView before Safari
16.4) the value flips discretely and the band jumps instead of gliding,
which is the same floor the blink has today. Dropped: one gradient overlay
masked to the dots, because the dot pitch under `justify-between` depends on
the bar width and a mask cannot follow it.

**Reduced motion.** The loop stops (`design.md`: no duration makes a loop
acceptable). In its place the next unlit segment holds the brand tint
steadily, and the ring holds its next eighth. That is a colour, not motion,
and it keeps a live mark on screen.

**API.** `LedBar` and `SegmentedMeter`: `blinkNext` becomes `live`,
`blinkStep` becomes `row`. `UsageRing`: `blink` becomes `live`. The row is
no longer capped at five, so `ledBlink.ts` goes. The `.led-blink` and
`--led-flash-N` rules in `hud.css` go with it.

Tests: the existing blink tests move to the sweep classes and attributes
(every dot carries `led-sweep-dot` and its index while live; none without;
the ring's arc is present only while live; the clock class follows
`liveProviders`). Sweep timing is verified in the Browser pane by sampling
computed opacity, as the blink was.

Docs: `docs/hud-states.md` (the blink bullets become sweep bullets),
`apps/desktop/design.md` (the `motion:` list gets `led-sweep`),
`hud-session-lifecycle-events.md` (status table notes that F replaced the
blink), and the two PR bodies.

### Phase G: the sweep ends at 30 seconds, the session stays active for 180

The actor keeps one deadline per session today, at 180 seconds, and
publishes `idle` when it passes. Phase G adds a second, earlier deadline:
30 seconds after a session's last write the actor publishes `quiet` for it.
A later write publishes `activity` again and re-arms both deadlines. The
`idle` event and `ACTIVE_SESSION_WINDOW_SECS` do not move, so the session
list's active pill, the store's seed query, and the engine's definition of
active are unchanged.

- `SessionEvent::Quiet { session, agent, at }` joins the bus, and
  `LiveEntry` records whether `quiet` was already published for its current
  write. `soonest_deadline` takes the earlier of the two clocks.
- `get_live_sessions` reports each session's `last_activity_at` already, so
  the renderers can tell a quiet session from a working one on a snapshot.
- The renderers' live set for the sweep is the sessions with a write in the
  last 30 seconds: on at `activity`, off at `quiet` or `idle`. `isLive` and
  `liveProviders` read that set. Keyless activity (phase E's remaining
  case, a brand-new transcript) uses the same 30 seconds locally.

Tests: the actor publishes `quiet` at 30 s and `idle` at 180 s for one
session; a write between them re-arms both; a write after `quiet` publishes
`activity` and a second `quiet` 30 s later. Renderer tests: `quiet` clears
the sweep; a snapshot with a session written 45 s ago starts with no sweep.

Agreed 2026-09-11 in review ("30"). Thirty seconds ends the sweep during a
long tool run, such as a build, which is the intended reading: the meters
sweep while tokens flow, not while the agent waits.

## Decisions to confirm

| Decision       | Proposal                                                                                                                                                            |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Stop fix       | Phase E, the quiet-path lane. The renderer-side cut is the fallback.                                                                                                |
| Off delay      | 30 s after the last transcript write, through a `quiet` event on the bus (phase G). `idle` and the session list keep 180 s. Agreed 2026-09-11.                      |
| Sweep pace     | 4000 ms cycle, about a 2 s crossing: the session list shimmer's cycle and phase on every surface, set on 2026-09-11. One `--led-sweep-cycle` variable to tune.      |
| Band width     | About three dots on the HUD, so a seventh of the bar. The same fraction on the popover's 32-segment meter.                                                          |
| Reduced motion | Steady brand tint on the next unlit segment; the ring's next eighth.                                                                                                |
| Where it lands | E and the actor half of G on `feat/session-lifecycle-bus` (#489); F and the renderer half of G on `feat/hud-session-blink` (#490), which gets a new title and body. |

## Departures while building

- **The closed bar's ring stays provider-level.** Phase H scopes each meter
  to the model it measures, but the ring on the closed popover bar shows
  `maxLiveUsedPercent`, the provider's highest meter rather than one window.
  There is no single model to match it against, so the ring keeps the
  provider rule and sweeps whenever a session draws on the provider.
- **Only lit segments move, and the band on them is the shimmer white.**
  Keith, 2026-09-11, on the first build: "dont animate unlit LEDs". The tint
  over an Anthropic bar's lit segments paints nothing, so a lit segment takes
  `--color-shimmer`, the highlight the session list already runs across a
  running title, and an unlit segment carries no sweep at all. The one
  exception is phase A's case: a bar with nothing lit flashes its first
  segment in the brand tint as the sweep passes. The ring follows the same
  rule: its gleam runs from twelve o'clock to the end of the reading's arc
  and fades there, and a ring under an eighth flashes its first eighth in
  the tint. The reduced-motion mark is unchanged.
- **The gleam peaks well under full, and lower on the HUD.** Keith,
  2026-09-11, on the lit-only build: "animation colour should be more
  subtle, like 50% less change", then on the half-strength build: "even a
  little more subtle in HUD, less subtle in the menubar view".
  `--led-gleam-peak` on `.led-clock` caps the gleam and the ring's gleam at
  0.56 in the popover, after Keith, on the two-level build: "make VU in
  menubar color change a bit more subtle". The HUD's clock hosts add
  `led-clock-soft`, which sets 0.245, after Keith on the four-level build:
  "make HUD color change 30% more subtle". The reduced-motion mark stays at
  full, because it is a steady colour and not a change.
- **Two brightness levels, not a ramp.** Keith, the same day: "rather
  than a perfect gradient, lets try instead giving the LEDs 4 brightness
  options, and applying the gradient to that", then, on the four-level
  build: "Try 2 brightness levels only". `round(up, …, 1/1)` on the band's
  profile puts each segment at off or at the peak, so the band hops a
  segment at a time. `--led-gleam-steps` on `.led-clock` holds the count.
  The band narrowed from about six segments to about three with the second
  step, because six segments at the peak together read as a block and not
  as a gleam. A webview without `round()` keeps the ramp. The ring's fade
  stays smooth; it is an arc, not a lamp.
- **The cycle is the session list shimmer's, on both surfaces.** Keith,
  the same day, moved the pace three times: "slow animation down by 1
  second", then "slow down animation by 1.5 second for the menubar VU
  meter", then "Set timing of VU meter in menubar and HUD to be the same as
  the animation timings for the active session in the session list". The
  shimmer runs a 4000 ms linear cycle with no rest and crosses its title in
  half of one, so `--led-sweep-cycle` is 4000 ms on `.led-clock` and the
  keyframes run the travel across the complete cycle. A bar crossing takes
  about 2 s, and `--led-row-lag` is 0.05 bar lengths, which is 100 ms. The
  cycle is a literal, not a shared token: `--activity-row-shimmer-cycle`
  sits on `.activity-row-active` and does not reach the HUD. A comment in
  each stylesheet names the other.
- **The shimmer and the sweep share a phase, not only a cycle.** Keith, the
  same day: "align timing of sessions list and the VU meter". A CSS
  animation starts when the browser applies it, so two animations of one
  cycle still sit at different points of it. The travel range copies the
  shimmer's as well: the band runs from half a bar before the left end to
  half a bar past the right end, as the shimmer's band runs from half a
  title before the text to half a title past it. One cycle alone was not
  enough, because the sweep then crossed its bar 500 ms before the shimmer
  crossed its title. Measured in Chrome: the sweep position then equalled
  the shimmer's band centre at each eighth of the cycle.
- **The sweep holds back 0.2 s behind the shimmer.** Keith, on the running
  app: "VU meter seems to start animation about 0.2 seconds before session
  list item". The centres were equal, but the onsets are not: the shimmer's
  band is soft and almost a title wide, so it fades in, and the meter's band
  is sharp and three segments wide, so it snaps on. The travel starts a
  tenth of a bar earlier and ends a tenth earlier, which delays the meter's
  onset by 0.2 s and leaves the pace and the crossing unchanged.
- **The phase lives on the running animation, not in a render.** Keith, on
  the running app: "issue; timing of active sessions and VU meters are
  getting out of sync. Build some mechanism to force to always be in sync".
  The first build gave each element a negative `animation-delay` of the wall
  clock. A delay only corrects the start. Each later render wrote a new delay
  while the animation kept its first start time, so the phase moved by the
  time between the mount and that render. The session rows render on every
  scan and every usage poll, so their shimmer wandered. A window that stops
  painting holds its animations as well, while the clock runs on.
  `installLivePhase` in `src/lib/livePhase.ts` therefore owns the phase. It
  sets `Animation.startTime` of each live animation from the wall clock, and
  sets it again when an animation starts, when the window comes back, and
  once each cycle. It writes nothing when the error is under one frame. The
  stylesheets declare no delay, and no component passes one, so a render
  cannot move an animation. `mountWindow` installs it for every window, next
  to `installFocusModality`.

  The anchor runs inside `requestAnimationFrame`, because a timeline reports
  the time of the last frame while `Date.now` reports now. A first build
  compared the two directly and a window that painted rarely put them 853 ms
  apart. Measured in Chrome with the real stylesheets and the real module:
  two surfaces that join 2.5 s late land 5.7 ms from the two already
  running, a forced 900 ms drift is gone one cycle later, and every
  animation tracks the wall clock within 8.2 ms. The effect delay of each is 0.

- **The gleam takes a shade of the segment's own colour.** Keith, the same
  day: the sweep must "accommodate dark mode for the various models, which
  might use white (for openAI)". The OpenAI bar takes `--color-label`, near
  white in dark mode, and the white gleam vanished on it. `LedBar` hands each
  lit segment's colour to the stylesheet in `--led-color`, and relative
  colour syntax turns it into the gleam: white above a segment under oklch
  lightness 0.85, a dark shade of the same hue above one over it. A webview
  without the syntax keeps the white gleam. `SegmentedMeter` and the ring
  paint only the brand and red tints, so they stay on the shimmer white.
- **The renderer keeps a local 30 s clock for keyed sessions too.** The plan
  had the bus's `quiet` event end the sweep by itself. A snapshot that lists
  a session written 20 s ago, or a missed event, then needs a timer anyway,
  so each entry carries the instant its write stops counting and one timer
  serves keyed and keyless sessions alike. `quiet` and `idle` still remove
  the session at once.
- **The sweep position runs in bar lengths.** The pseudo-element formula in
  the plan multiplied by the segment count; the built one divides the
  segment index by it instead, so the band and the row lag are the same
  share of every bar, whatever its length.

## Out of scope

- Database-backed agents whose store writes touch every session at once.
- Changing `ACTIVE_SESSION_WINDOW_SECS`. The 30 s sweep window is a second
  event on the bus, not a change to the engine's active window.
- The detail window, which does not animate.

## Problem 3: a model-scoped meter sweeps for a model that is not running

Keith, 2026-09-11: "Issue: if user is not running fable, the fable line
should not be shimmering". Then, on the routes: "If a fable session is
active, then also sweep fable. If not, do not include it in the sweep".

**The logic today.** The sweep is chosen once per provider and every meter in
that provider's group inherits it:

```
src/components/providerUsage/UsageLimitsBar.tsx:142  live={liveProviders.includes(reading.provider)}
src/components/providerUsage/UsageLimitsBar.tsx:176  live={liveProviders.includes(reading.provider)}
src/components/providerUsage/UsageLimitsBar.tsx:329  live={live}            (each WindowMeterRow)
src/views/OverlayWindow.tsx:83                       live={state.liveProviders.includes(bar.provider)}
```

Anthropic publishes three windows: the account-wide `five-hour` and
`seven-day`, and one model-scoped `weekly-<model>` per model the plan meters
separately. A model-scoped window carries `scopeModel`, the provider's
display name, such as `"Fable"`. Any live Claude Code session therefore
sweeps the Fable line, whatever model that session runs.

**Why the renderer cannot tell.** The lifecycle bus carries the agent only:

```
src-tauri/src/session_lifecycle.rs:118  pub struct LiveSession { session, agent, last_activity_at }
src/lib/sessionLiveness.ts:29           interface LiveEntry { agent, until }
```

`SessionRecord` holds no model either, so the model is not available where
the scan feeds the bus. The model lives in the evidence tables that the
analysis worker writes after a pass indexes a session.

## Phase H: a scoped meter sweeps only for the model that is running

**The rule.** A window with no `scopeModel` keeps today's behaviour: it
sweeps while the provider has a live session. A window with a `scopeModel`
sweeps only while a live session runs a model that matches that scope. An
unknown model does not sweep a scoped window, so the meter stays still until
the app can show the model is running.

**Where the model comes from.** The newest turn the analyzer published for
that session. The `turn` table already holds `model` and `ts_ms` per turn
(`crates/antiburn-local/src/analysis/evidence_query.rs:358`), so the newest
row answers "the model this session runs now". The whole-session model set
in `analysis.model_breakdown_json` does not: a session that ran Fable an
hour ago and Opus now still lists Fable, which is the report Keith made.

A new store read returns one value per session:

```sql
SELECT model FROM turn
 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
   AND claim_fence = ?4 AND model IS NOT NULL AND model <> ''
 ORDER BY ts_ms DESC, turn_index DESC
 LIMIT 1
```

`?4` is the session's `published_fence`, the same fence
`Store::published_turn_rows` reads, so a pass in flight cannot leak a
half-written claim.

**Where it is carried.** The live snapshot, not the event stream.
`get_live_sessions` already runs with an `AppHandle`, so it reads the store
and fills a new `LiveSession.model` field. The events on `session:lifecycle`
stay as they are: they answer "is anything live", which must be fast, and
the model only exists after an analysis pass anyway. The renderer already
re-reads the snapshot on `scan:finished`, which is when a new model can
first appear.

**Matching a model id to a scope name.** `scopeModel` is a display name
(`"Fable"`) and a session model is a raw id (`"claude-fable-5"`). A new
helper beside `modelShortName` in `src/lib/presentation/models.ts` compares
them by token: it slugs the scope name, drops the vendor and numeric tokens
(`claude`, `gpt`, `anthropic`, `openai`, and any all-digit token), and
reports a match when every token that is left appears as a token of the
model id.

| Scope name          | Model id                     | Match |
| ------------------- | ---------------------------- | ----- |
| `Fable`             | `claude-fable-5`             | yes   |
| `Fable`             | `claude-opus-4-6`            | no    |
| `Claude Sonnet 4.5` | `claude-sonnet-4-5-20250929` | yes   |
| `Fable`             | `gpt-5.6-sol`                | no    |

**The changes.**

| File                                              | Change                                                                            |
| ------------------------------------------------- | --------------------------------------------------------------------------------- |
| `src-tauri/src/store/mod.rs`                      | `latest_turn_model(&SessionKey) -> Result<Option<String>>`, at the published fence |
| `src-tauri/src/session_lifecycle.rs`              | `LiveSession` gains `model: Option<String>`                                       |
| `src-tauri/src/commands.rs`                       | `get_live_sessions` fills `model` from the store                                   |
| `src/lib/sessionLiveness.ts`                      | `LiveEntry` gains `model`; new `liveModels(state, now)`                            |
| `src/lib/presentation/models.ts`                  | `modelMatchesScope(modelId, scopeName)`                                            |
| `src/lib/presentation/liveUsage.ts`               | `windowSweeps(window, providerLive, liveModels)`                                   |
| `src/lib/usageBars.ts`                            | `UsageBarItem` gains `scopeModel`                                                  |
| `src/components/providerUsage/UsageLimitsBar.tsx` | each `WindowMeterRow` decides its own sweep                                        |
| `src/views/OverlayWindow.tsx`                     | each bar decides its own sweep; the `led-clock` host needs any bar to sweep        |
| `src/views/overlay/OverlaySession.ts`             | carry `liveModels` in the HUD snapshot                                             |
| `src/views/popover/PopoverSession.ts`             | carry `liveModels` in the popover snapshot                                         |

About 350 lines with tests.

**Where it lands.** All of phase H on `feat/hud-session-blink` (#490). The
model is read only by the sweep, and #490 already owns `sessionLiveness.ts`,
`usageBars.ts`, and its own additions to `commands.rs` and `store/mod.rs`.
#489 stays reviewable as the bus alone, and no merge up is needed.

**What this does not fix.** A brand-new session shows no model until the
analyzer publishes its first turn, so its scoped meter starts sweeping a
pass late. The account-wide meters still sweep at once. Keyless activity, a
write with no indexed session, carries no model and sweeps no scoped meter.
