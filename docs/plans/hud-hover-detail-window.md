# HUD hover detail: a separate tooltip window

_Plan. Branch `claude/hud-hover-tooltip-0214cd`. 2026-08-24._

> **Post-plan change:** issue #108 replaces the fixed HUD frame and reported
> hover region with a content-sized frame. The separate detail-window design
> stays unchanged; historical frame references below describe its starting
> point.

## Status

| Step                                            | State                           |
| ----------------------------------------------- | ------------------------------- |
| 1. Rust: detail window + placement + commands   | done                            |
| 2. Frontend: HudDetailView + route              | done                            |
| 3. Frontend: strip expansion from HUD, ✕ on HUD | done                            |
| 4. Docs + tests + slop pass                     | done — PR waits on a screenshot |

Reviewed with Keith via discuss 2026-08-24; all three open questions decided
(see Decisions).

## The problem

The HUD today is one fixed 176×500 transparent frame. Hover doesn't resize the
window — it expands the panel _inside_ the frame, and a pile of machinery keeps
that expansion from misbehaving at screen edges:

- `reserveAbove` / `flipUp` / `swapping` in `OverlaySession.ts` — when the HUD
  sits low, the window moves up 220px while the content pads down by the same
  amount, hidden for a frame during the swap.
- `panelHeights` + ResizeObserver + chrome/row estimates to guess the expanded
  height before it exists.
- `decideDirection()` re-running after drags, bar-count changes, and expansions.
- A whole "Rejected positioning designs" table in `docs/hud-states.md`
  documenting five failed variants of this.

It's annoying for the user (content shifts under the cursor) and for us
(every new HUD feature has to thread through the reserve/flip logic).

## The proposal

The HUD window only ever shows the collapsed bars. After a 400ms hover intent,
a **second, separate always-on-top window** — a big tooltip — appears next to
the HUD, showing what the expanded panel shows today: the wordmark (plain
text), and per-limit label, percentage, bar, and reset time.

The HUD never changes size, layout, or position on hover. The tooltip window is
sized to its content _before_ it is shown, so it never resizes on screen either.
All the reserve/flip/swap machinery is deleted.

The tooltip is **pure display** — no controls, and the cursor never needs to
enter it. It shows while the pointer rests on the HUD panel and hides when the
pointer leaves. The ✕ moves onto the HUD itself: small, top right, fading in on
pointer entry, absolutely positioned so it adds no height. Settings stays
reachable through the tray (the tooltip wordmark is no longer a button).

## Design

### Windows

|           | HUD (`antiburn-overlay`)                   | Detail (`antiburn-hud-detail`, new)                                                                                                                                                                                                                          |
| --------- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Size      | unchanged 176×500 transparent frame for v1 | 176 wide, sized to content before each show                                                                                                                                                                                                                  |
| Focus     | never focused                              | `focused(false)`, cursor events ignored — the tooltip is display only                                                                                                                                                                                        |
| Lifecycle | unchanged                                  | created lazily on first hover, then kept warm hidden (popover pattern)                                                                                                                                                                                       |
| Chrome    | unchanged                                  | transparent window; content card paints `bg-hud`, `rounded-xl`, `bevel`, border, plus a tooltip treatment: soft CSS drop shadow (a few px of transparent padding carries it) and a 120–150ms fade-in on show, instant hide; reduced-motion disables the fade |

Keeping the HUD's oversized transparent frame for v1 limits scope: the hover
region reporting (`set_overlay_hover_region`) already tells Rust where the
drawn bars are, and dragging is untouched. Shrinking the frame to fit the bars
is a separate cleanup we can do later.

### Placement (Rust, `crates/hud`)

A pure `compute_detail_position()` mirroring the popover's `compute_position`,
with unit tests:

- Anchor: the HUD panel's drawn rect (window position + the hover region edges
  already reported).
- Prefer **below** the panel, 8px gap, left-aligned with the panel.
- Flip **above** when below would cross the screen's bottom margin.
- Clamp horizontally to the monitor with the usual 8px margin, using the
  monitor that contains the anchor (mixed-DPI safe, same as the popover).

Because the tooltip is its own window sized to content, placement is one
position calculation at show time — no reserves, no equal-and-opposite moves.

### Hover state machine

Simpler than today, because the tooltip is display-only and the cursor never
travels into it:

- Hover detection is unchanged: DOM mouse edges plus the Rust 100ms cursor
  poll against the HUD panel region, emitting `overlay_hover`. No union with
  the tooltip rect, no hide grace.
- Pointer enters the panel → the ✕ fades in at once, and a 400ms intent timer
  starts. Timer fires → `show_hud_detail`.
- Pointer leaves the panel → pending timer cleared, tooltip hidden, ✕ fades
  out.
- Mouse down on the panel clears the pending timer and suppresses it until
  mouse up. After mouse up, a fresh 400ms count starts only if the pointer
  still rests on the panel. This covers drags: a drag never shows the tooltip
  (today's rule: dragging always renders collapsed).

### Data flow

The detail view is a small external-store view (no `useEffect`), route
`#/hud-detail`. The HUD session pushes state to it rather than the detail
window polling on its own:

- On show, and whenever the HUD's usage poll refreshes while the detail window
  is visible, the HUD emits a `hud-detail:state` event carrying the derived
  bars, `now`, and `sessionLive`.
- The detail view renders from the last received payload. It reuses
  `deriveUsageBars` types and `resetsIn` for labels.

One data owner (the HUD session), no second polling loop.

### New commands

| Command                          | Does                                                                   |
| -------------------------------- | ---------------------------------------------------------------------- |
| `show_hud_detail(width, height)` | create-or-reuse, size, place against the HUD panel, show without focus |
| `hide_hud_detail`                | hide                                                                   |

The detail webview measures its content and passes the size with the show call,
so the window appears at final size. `capabilities/default.json` lists the new
label (events only — it needs no window-control permissions).

## What gets deleted

From `OverlaySession.ts`: `reserveAbove`, `flipUp`, `swapping`, `panelHeights`,
`measurePanel`, `decideDirection`, `applyReserve`, and the
`ESTIMATED_*` / `RESERVE_ABOVE` constants. From `OverlayWindow.tsx`: the entire
expanded branch (header row, labels, percentages, reset lines) and the
`lift`/`marginTop` maths. The settings button goes with it — Settings stays
reachable through the tray. The ✕ survives as a small overlay control at the
HUD's top right. `docs/hud-states.md` gets rewritten: Expanded becomes "Detail
window shown", the Positioning section shrinks to the anchor rules above, and
the rejected-designs table gains the entry that closes it out.

`set_overlay_hover_region` and the drag path stay. The ResizeObserver also
stays, against the first draft of this list: it drives the hover-region report
when the bar count or the fonts change the panel height, and the shell needs
those edges for both the cursor watcher and detail placement.

## Steps

1. **Rust window + placement.** Add detail-window creation, sizing, and
   `compute_detail_position` with tests to `crates/hud`. The window ignores
   cursor events. Wire the two new commands through `commands.rs` / `lib.rs`;
   add the capability entry.
2. **Frontend detail view.** `#/hud-detail` route, `HudDetailView` rendering
   the detail content from pushed state, measure-then-show, fade-in.
3. **Frontend HUD slimming.** Strip the expanded branch and the reserve/flip
   machinery; move the ✕ to the HUD top right; 400ms intent timer with
   mousedown cancel/suppress. Update `OverlayWindow.test.tsx`, add detail-view
   tests.
4. **Docs + hygiene.** Rewrite `docs/hud-states.md`, run `pnpm run slop`,
   DCO-signed commits, screenshot for the PR (I'll try capturing the HUD +
   tooltip on a dev build; if the HUD needs real usage data I'll ask Keith).

Roughly 400–600 lines net, one PR.

## Decisions (discuss review, 2026-08-24)

1. **Tooltip width:** keep 176, but with a more tooltip-like treatment — drop
   shadow and a short fade-in.
2. **Controls:** the ✕ moves to the HUD's top right, small, shown on hover.
   The tooltip is non-interactive; the settings entry point moves to the tray
   alone.
3. **Show delay:** 400ms. Any mouse down on the HUD cancels the pending timer
   completely until mouse up.

## Non-goals

- Shrinking the HUD's 500px transparent frame (follow-up).
- Windows/Linux support (the HUD is macOS-only; the detail window inherits
  that boundary).
- Any change to bars, blinking, polling rates, or the flinch work (#57).
