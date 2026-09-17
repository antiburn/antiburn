# HUD v2: frame, live LEDs, per-agent boxes, edge dock

_Plan. Branch `feat/hud-v2` (off `origin/main`, token-map commits replayed on top). 2026-09-16. Built; draft PR open._

Eight asks from Keith, grouped into four PRs that each stand alone. The token
map (`hud-token-map.md`, shipped on this branch) and the LED spend-rate design
(`hud-led-spend-rate.md`, designed, not built) are the two inputs. The observer
loop handoff (`token-usage-breakdown-implementation-handoff.md`) supplies the
signals that item 8 needs and is planned separately.

## Status

| Step                                                  | State                                                                                                                    |
| ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| 0. Plan reviewed, open questions decided              | done                                                                                                                     |
| 0b. Token-map commits replayed onto `feat/hud-v2`     | done                                                                                                                     |
| A. Frame + round dots + mode palette retune (1, 2, 7) | built; Keith round 2 fixes applied (40% frame, no hover swap, close ✕ off, launch restore, click guard)                  |
| B. LED blink follows spend, in mode colour (3, 7)     | built; on-screen tested by Keith 2026-09-16                                                                              |
| C. Agent boxes at LED scale, per-box detail (4, 5, 6) | built; tested by Keith 2026-09-16 (LED scale shipped in A; this adds per-box detail and the single-session rule)         |
| D. Edge dock: off-screen, wake on edge/activity/burn  | built; tested by Keith 2026-09-16 (gesture model: drop against an edge docks with a 6pt tab; drag tears off; no setting) |
| E. Restack on PR #489 (session lifecycle bus)         | done 2026-09-17; PR #565 base is now `feat/session-lifecycle-bus`. The LED bar carries both the sweep (#489) and the spend blink (#565); the overlap needs Keith's eye. |

## What Keith asked for

1. An always-on rounded rect around the HUD window, 50% opacity (30% asked
   first, raised in review), white in light and dark in dark.
2. Purely round dots everywhere.
3. LEDs flash faster or slower with the speed of spend (`hud-led-spend-rate.md`).
4. The near-time agent display sits above the VU meter. Each active agent and
   its sub-agents sit in a stroked box, as today, but the LEDs are the same
   size as the VU meter's, and sub-agent LEDs are smaller.
5. Hovering a box opens its own detail window, like today's, one for the usage
   meter and one per agent box.
6. A single running session shows no boxed agents.
7. A colour labelling system: every active (animated) LED takes the colour of
   what the agent is doing. No orange, coral, black or white; those stay
   reserved for the non-animated usage LEDs.
8. The HUD can live off-screen and come back when (a) the mouse hits a screen
   edge, setting, default right; (b) new activity lands after more than one
   hour of quiet; (c) burn rate is high.

## Vocabulary

- **VU meter**: the LED usage bars (`LedBar`, 20 segments, 6 px dots).
- **Agent box**: one stroked frame per live session holding that session's
  dots, today drawn by `TokenMap`.
- **Mode**: `WorkMode`, the seven kinds of work a turn did. Already shipped;
  do not add an eighth vocabulary.

---

## PR A — frame, round dots, mode palette (items 1, 2, 7 palette)

### The frame

The HUD panel (`OverlayWindow.tsx`, the `rounded-xl border-transparent` div)
gets a visible material: `bg-hud-frame` at 60% alpha with a 1 px `separator`
hairline. New tokens in `hud.css` and `design.md`:

| Token          | Light                  | Dark                 |
| -------------- | ---------------------- | -------------------- |
| `bg-hud-frame` | `hsl(0 0% 100% / 0.7)` | `hsl(0 0% 0% / 0.7)` |

First white in both themes (Keith, 2026-09-16, after seeing a dark frame vanish
on a dark desktop), then 70% black in dark once the frame went to 70% alpha
(0d8a9b42). The frame replaces the LED rings and the HUD's full-strength
off grey: with a constant backing they are noise.

The frame is always on, not hover-only. The close ✕ is commented out for now
(Keith, 2026-09-16); when it returns it keeps its own opaque
`bg-hud` disc so it still reads on the translucent frame. The webview stays
transparent outside the panel, so the drop area is unchanged.

`prefers-reduced-transparency` makes the frame opaque `bg-hud`, matching the
rule the popover already follows.

### Round dots

Dots draw in HUD pixels on the LED grid: 20 columns, a 6px dot (4px for a
sub-agent), corner radius that hugs a dot. A session leaves the map 90 s after
its last turn, the same window that drives the live LED (Keith, 2026-09-16:
dots lingered for the full five-minute rate window).

`TokenMap` already draws circles. The change is the blob frame: `rx` goes from
`UNIT / 2` to a full pill radius so nothing on the HUD has a square corner.
`LedBar` segments are already `rounded-full`. Nothing else to do; this item
mostly confirms the current state and pins it with a test.

### Mode palette retune

Three of the seven mode tokens break rule 7 today:

| Mode       | Today                         | Problem            | Proposed                              |
| ---------- | ----------------------------- | ------------------ | ------------------------------------- |
| `changing` | brand-tint (coral)            | coral is reserved  | `system-purple` (light/dark variants) |
| `other`    | `system-orange`               | orange is reserved | `system-teal`                         |
| `talking`  | black / white at 35–40% alpha | black/white banned | `system-mint`, dimmed to 60%          |

`looking` (blue), `running` (green), `delegating` (indigo), `thinking` (gold)
stay. All seven then sit away from the burn orange and from each other. The
tokens are semantic (`--color-mode-*`), so the retune touches `hud.css` and
`design.md` only; nothing in TSX changes.

### Size

~120 lines. Tests: frame class present, reduced-transparency swap, mode
tokens present in the drift check.

---

## PR B — LED blink follows spend, in mode colour (items 3, 7 behaviour)

Build `hud-led-spend-rate.md` as designed, with one addition from item 7.

### From the spend-rate plan, unchanged

- Widen `ModeSample` with `model` and unsplit `usage`.
- `HudSpendRate { usd_per_minute, window_secs, priced_share }` on the token map
  payload, summed across sessions and sub-agents.
- `ledPeriod.ts`: geometric map $0.05–$2.00/min → 3000–300 ms, quantised to
  eight rungs. Reduced motion: no blink, rate stated in words in the detail
  window.
- Degradation ladder: priced → `consumptionRate` → fixed 3 s → none.

### Addition: the blinking LED takes the mode colour

Today the last lit VU segment blinks in the bar's own colour (burn orange).
Under rule 7 an animated LED carries meaning through colour, so the blinking
segment takes `--color-mode-<mode>` of the newest live turn across all
sessions, and the static segments stay orange. `LedBar` gains an optional
`blinkColor` next to the planned `blinkPeriodMs`. When no mode is known the
segment blinks in its bar colour, as today.

The token map payload already carries per-session modes and `lastTurnEpoch`;
`deriveTokenMap` already finds the newest live session. Expose that session's
top mode as `layout.liveMode` and the HUD reads it from the snapshot. No new
timer, no new command.

### Check first

The spend-rate plan flags that `get_hud_token_map` reads sessions from the
store, which only the scan writes, and the scan pauses with the popover.

**Checked 2026-09-16, by reading `scan/mod.rs` on main.** The concern is out
of date. The scan now runs an OS filesystem watcher over every agent's roots
plus an unconditional 5 minute reconciliation tick, popover or not; only the
`discovery_paused` setting stops it. The liveness path
(`latest_session_activity`) reads the same store, so the LED and the map see
the same sessions. No seeding needed.

### Size

~350 lines including Rust tests. Stacked on the token-map PRs.

---

## PR C — agent boxes at LED scale, per-box detail (items 4, 5, 6)

### LED scale

`TokenMap` dots are 3.6 SVG units in a 10-unit cell, which renders around
8 px on the 136 px content width. The VU dots are 6 px with a 6.8 px pitch
(20 across 136 px). Change the map to the VU grid:

- Cell pitch = the VU pitch, so a full-width row holds 20 dots, like a bar.
- Parent dot radius = 3 px, the VU radius. Sub-agent dot radius = 2 px.
- `cells` in `deriveTokenMap` goes from 12 to 20 wide. The height stays
  cropped to the rows in use (already shipped in `9e7488ce`).

The dot ladder does not change. More cells per row means a finer ladder step
fits more often, which is a small gain.

### Per-box detail windows

Today one detail window shows the usage stats and the map summary together.
Split it by hover target:

- Hovering the VU meter shows the usage card, as today, without the map rows.
- Hovering an agent box shows that session's card: title, agent, rate, mode
  split as a small LED row, sub-agents each on a line, and the dot value.

Keep **one** detail webview and one Rust window. The HUD passes a `target`
field in `HudDetailState` (`"usage"` or a session key) and the card renders
the matching content. Moving between boxes re-sends state with
`reason: "show"`, which restarts the fade. One window is enough because only
one pointer exists; spawning a window per box would multiply the placement
code and the warm-window trick for nothing visible.

The HUD needs to know which box is under the pointer. `TokenMap` gets
`onHoverBlob(key | null)` driven by `onMouseEnter`/`onMouseLeave` on the blob
`<rect>` and its dots (the wrapper drops `pointer-events-none`). The 400 ms
intent timer is reused; switching boxes inside an open detail is immediate.

Placement: the detail window anchors below the HUD as today. Anchoring under
the hovered box would need per-box screen rects and the window is nearly as
wide as the HUD anyway.

### Single session

When exactly one session is live, the map hides and the blinking VU LED (PR B)
carries the activity alone. The detail card for the usage meter then includes
that session's row, so the information is still one hover away. Two or more
sessions bring the boxes back. Hysteresis: a box that just disappeared does
not reappear within one poll, to avoid flicker when a second session's window
sample flickers around zero.

### Size

~400 lines. Stacked on PR B for the live mode. Screenshot of two boxes plus
the per-box card for the PR.

---

## PR D — edge dock: off-screen HUD that comes back (item 8)

### States

```
Shown ──(dock)──▶ Docked ──(edge hit | wake)──▶ Shown
```

- **Docked**: the window sits fully off the chosen edge, still alive, still
  polling at the slow rate. The token map and usage polls keep running so the
  return is instant. Nothing renders on screen.
- **Edge hit**: the global cursor rests within 2 px of the chosen edge for
  150 ms. The HUD slides in to its last on-screen position and stays until the
  pointer leaves it, then slides back after 3 s.
- **Wake**: the HUD slides in on its own and stays for 5 s or until hovered,
  whichever is longer, then docks again.

### Entry points and setting

Settings → Usage → Floating HUD gains:

- "Dock off-screen" toggle. Off by default; existing users see no change.
- "Wake edge": left / right / top / bottom, default right. Enabled with the
  toggle.
- A dock button beside the close ✕ on the HUD, visible on hover, that docks
  without turning the preference off. The close ✕ keeps hiding the HUD.

Preference keys: `antiburn.hudDock` (`"0"`/`"1"`) and `antiburn.hudDockEdge`.
Mirrored to the shell with the same broadcast pattern the visibility toggle
uses, so the Rust watcher knows the edge without asking the webview.

### Rust

The hover watcher (`spawn_hover_watcher`) polls the cursor every 100 ms only
while the HUD is visible. Docked mode adds an **edge watcher**: 100 ms cursor
poll against the monitor frame, active only while docked. It emits
`overlay_edge_hit` and the shell slides the window in with the existing
`RESIZE_STEPS` easing, reused for x/y.

Wake reasons come from the frontend, which already has both numbers:

- **Activity after one hour of quiet**: `getLatestSessionActivity` returns the
  newest transcript epoch. If the previous reading was more than 3600 s older
  than the new one, wake. Uses `activity_source == "event"` only, because
  mtime lies by weeks.
- **High burn**: `usd_per_minute` from PR B at or above the spend ceiling
  ($2.00/min, or whatever the anchor settles at) for two consecutive polls.
  Below the ceiling the HUD does not wake. Cold start is "measuring", never
  "high", so a fresh dock does not wake on its first sample.

Both call a new `wake_overlay(reason)` command. Reasons are logged so the
5 s auto-dock can be tuned.

### Multi-monitor

The dock edge is the edge of the monitor the HUD was last on. Changing
monitors while docked re-docks to the same edge of the new active monitor.

### Size

~500 lines, mostly Rust. Follows PR B for the burn signal; the edge and the
one-hour wake can ship without it behind the same toggle.

---

## Order and dependencies

```
A (frame, dots, palette)        independent
B (spend LED, mode colour)      needs token-map PRs
C (boxes at LED scale, detail)  needs B for live mode; hover split is independent
D (edge dock)                   needs B for burn wake; edge + 1h wake independent
```

Suggested order: A, B, C, D. A can go first while the spend anchors are
sampled from real transcripts.

## Docs to update

- `docs/hud-states.md`: frame, LED period, agent boxes, single-session rule,
  per-box detail, the Docked state and its transitions.
- `apps/desktop/design.md`: `bg-hud-frame`, the retuned mode tokens, the LED
  period range and flash cap.

## Open questions

1. ~~Single session (item 6).~~ Decided: hide the map. The active VU LED
   alone carries it, flashing at spend speed in the activity colour.
2. ~~Mode colours.~~ Decided: purple / teal / mint for changing / other /
   talking.
3. ~~Wake dwell.~~ Decided: 5 s.
4. ~~Spend anchors.~~ Agreed: keep $0.05 and $2.00/min as the starting
   point and sample a few of Keith's days before B ships.
5. ~~Frame alpha.~~ Decided: 70%, white in light and black in dark.

## Decisions

1. **Single session hides the map** (Keith, 2026-09-16). Only the active VU
   LED shows, flashing at spend speed and coloured by activity type. Boxes
   appear from two agents up, where a busy sub-agent counts as an agent
   (Keith, 2026-09-16: "if there's 2 or more agents/subagents running there
   should be the map showing").
2. **Mode palette** (Keith, 2026-09-16): changing → purple, other → teal,
   talking → mint at 60%. The other four stay.
3. **Wake dwell is 5 s** (Keith, 2026-09-16), extended while hovered.
4. **Spend anchors start at $0.05 / $2.00 per minute** (Keith, 2026-09-16),
   to be checked against real transcripts before PR B ships.
5. **Frame alpha is 60%** (Keith, 2026-09-16), after 30%, 50% and 40% on screen; the hover surface swap is gone and the stroke is a vertical gradient.

## Tweaks after the first on-screen test (Keith, 2026-09-16)

1. **The tab is the whole edge.** A docked HUD peeks in when the pointer
   rests anywhere in the tab strip along its edge, not only on the window.
2. **Dock only past the edge.** A drop docks when the frame is off screen at
   mouseup. A HUD near an edge, still on screen, stays free.
3. **Sub-agents always show.** A sub-agent with tokens in the window gets at
   least one small dot, so a short sub-agent no longer rounds to nothing.
4. **Hover a dot for its owner.** A small dot names its sub-agent: the detail
   card highlights that row and says its top mode.
5. **Brand tint deeper.** The lit LED coral drops from 58.6% to 54%
   lightness; the unlit tint follows.

## Tweaks after the second on-screen test (Keith, 2026-09-16)

1. **Wake hold is 3.5s**, not 5s.
2. **Countdown while blocked.** When a limit is at 100%, a line under the bars
   reads "{limit} · resets in {time}", ticking every 5s.
3. **Reset celebration.** When a blocked limit drops below 100%, confetti plays
   over "{provider} usage reset" for 6s and a docked HUD peeks in.
4. **Shared display edges bounce.** A drop past an edge another display touches
   moves the HUD back inside the display instead of docking.
5. **Detail contrast.** Usage-card and legend text moved up one label level.
6. **Sounds.** A Web Audio synth (`hudSounds.ts`): a falling pop on a real
   tear-off, a rising bwoop when a nudge shows.
7. **Reset seen without the menubar.** Once a blocked bar's reset time passes,
   the HUD asks the shell for a fresh read instead of waiting on the cached
   summary.
8. **Dark frame is black.** `hud-frame` dark is 60% black, with a softer top
   stroke.
9. **Model limits show at 0%.** The HUD draws a supplemental per-model limit
   even before it moves. The popover and main window keep hiding idle ones
   (`liveWindows` takes `includeIdleModelLimits`).
10. **Drag reopen diagnostics.** `app_activated` logs the restore decision
    and `main_window_closed` logs a close, so the next repro names the path.
11. **Map dots in colour.** `hud.css` is `@theme static`: Tailwind dropped the
    mode variables no utility used, so the dots drew black.
12. **Live LED dims, never off.** The blink goes from the mode colour to 35%
    of it, so it reads as the lit LED pulsing, not an off LED flashing.
13. **Bolder mode colours.** The seven `mode-*` tokens move to saturated,
    fully opaque hues (blue, green, purple, indigo, yellow, hot pink, cyan).
    `talking` loses its 60% alpha and becomes pink so it no longer reads as a
    dim teal next to `other`.
14. **Holding the edge holds the HUD.** While the pointer rests on the tab
    strip that peeked the HUD in, the auto-dock treats it as on the HUD, so
    the HUD stays until the pointer leaves both.
15. **Dev menu for the HUD.** Debug builds get a "HUD Dev" submenu in the
    tray menu: dock at each edge, wake, a fixed spend rate (off, $0.05,
    $0.50, $2.00 per minute), block the top limit for 20 s so the countdown,
    the fresh read and the confetti all run, and celebrate now. Spend and
    block overrides ride a `hud_dev` event to the overlay webview; dock and
    wake call the hud crate directly. Nothing ships in release builds.
16. **Countdown leads with the time.** The blocked line reads "resets in
    1h 30m · 5-hour limit": the HUD clips the tail, and the limit's name
    was hiding the time.
17. **Wakes stay longer, peeks shorter.** A wake holds 4.8 s (was 2.8 s)
    and lingers 3 s after the pointer leaves. An edge peek lingers 1.5 s
    (was 3 s) once the pointer leaves the HUD and the edge.
18. **No mode key on the detail card.** The card drops the list of mode
    colours. Each session row already carries a dot in its top mode, so
    the key repeated what the rows show. The nudge chime is commented
    out in `NudgeSession`.
