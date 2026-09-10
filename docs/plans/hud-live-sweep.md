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
| Sweep pace     | 4 s cycle, about 1.3 s crossing, from the title shimmer. One `--led-sweep-cycle` variable to tune.                                                                  |
| Band width     | About six dots on the HUD, so a fifth of the bar. The same fraction on the popover's 32-segment meter.                                                              |
| Reduced motion | Steady brand tint on the next unlit segment; the ring's next eighth.                                                                                                |
| Where it lands | E and the actor half of G on `feat/session-lifecycle-bus` (#489); F and the renderer half of G on `feat/hud-session-blink` (#490), which gets a new title and body. |

## Departures while building

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
- **The renderer keeps a local 30 s clock for keyed sessions too.** The plan
  had the bus's `quiet` event end the sweep by itself. A snapshot that lists
  a session written 20 s ago, or a missed event, then needs a timer anyway,
  so each entry carries the instant its write stops counting and one timer
  serves keyed and keyless sessions alike. `quiet` and `idle` still remove
  the session at once.
- **The sweep position runs in bar lengths.** The pseudo-element formula in
  the plan multiplied by the segment count; the built one divides the
  segment index by it instead, so the band is the same share of every bar
  and the row lag is 100 ms on every bar length.

## Out of scope

- Database-backed agents whose store writes touch every session at once.
- Changing `ACTIVE_SESSION_WINDOW_SECS`. The 30 s sweep window is a second
  event on the bus, not a change to the engine's active window.
- The detail window, which does not animate.
