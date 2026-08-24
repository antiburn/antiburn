<!--
  This Source Code Form is subject to the terms of the Mozilla Public
  License, v. 2.0. If a copy of the MPL was not distributed with this
  file, You can obtain one at https://mozilla.org/MPL/2.0/.
-->

# Floating HUD parity port

## Goal

Port the reviewed floating usage HUD without changing its UI, interaction,
timing, persistence, copy, information architecture, or resource bounds.
Source-governance D-026 authorizes the port. Deviations-register D-30 records
the macOS-only release and accepted costs.

> **Post-port change:** issue #108 replaces the fixed 176×500 frame and hover
> region command with an original public dynamic-frame implementation. The
> historical parity requirements below describe the initial port.

## Authoritative behavior

The observable reference is antiburn PR #40 at commit
`32ce816211bd85e488163f2d7b35cb79bbbfbb46`. The maintained state contract is
[`docs/hud-states.md`](../hud-states.md).

The port keeps these values fixed:

- a 176×500 transparent, undecorated, always-on-top macOS window;
- `antiburn-overlay`, `#/overlay`, and `overlay_hover`;
- 250ms hover dwell and immediate collapse;
- a collapsed manual drag with one move per animation frame;
- a 220px reserve with an equal and opposite window move;
- an 8px screen margin, 20 LED segments, and 150ms transitions;
- a 90-second live window;
- 100ms cursor, 5-second liveness, 60-second usage, and 30-second reset polls;
- the localStorage preference and its per-webview drift;
- the Settings → Usage toggle, Usage header pop-out, startup restore, and close
  behavior;
- the exact user-facing copy, classes, styles, and Bitcount dependency.

## Architecture

### Native mechanism

`apps/desktop/src-tauri/crates/hud` is an unpublished nested Rust workspace. It
owns native window creation and reuse, the fixed geometry, the reported hover
region, the macOS platform gate, the 100ms cursor watcher, and the
`overlay_hover` event. The desktop shell excludes the nested workspace and uses
it as a path dependency.

`apps/desktop/src-tauri/src/hud.rs` owns the engine-specific latest-session
lookup and its 60-second memo. `commands.rs` owns the IPC policy. The command
names remain `open_overlay_window`, `set_overlay_hover_region`, and
`get_latest_session_activity`.

The crate compiles on Linux and Windows without dead-code or deprecated-code
suppressions. Its non-macOS `open` function is a no-op.

### Renderer boundary

`OverlaySession` is the imperative external-system boundary. It owns IPC calls,
native event listening, all four polls and timers, document transparency,
panel observation, native hover-region reports, direction decisions, reserve
swaps, and global drag events. `OverlayWindow` reads immutable snapshots with
`useSyncExternalStore` and renders the PR #40 DOM.

This design replaces the reference component's effects without changing their
event order. `flushSync` commits layout state before the session measures the
panel, following the established `NudgeSession` pattern. Added HUD code contains
no `useEffect` or `useLayoutEffect`.

`HudVisibilitySession` owns the Usage pop-out button's native visibility read
and focus listener. `PopoverSession` restores the preference when its external
session starts. The Settings toggle reads localStorage once when the pane
instance starts, as the reference does.

### Data adaptation

`usageBars.ts` adapts the public `LiveUsageSummaryPayload`. It uses the existing
`liveWindows` visibility and order rules, so the HUD and Usage surface display
the same limit windows. Session liveness uses transcript write times through the
shell command. These are the only forced data-source adaptations.

## Governance and boundaries

The `hud-window-mechanics` allowlist rule names the source slices and every
current destination, including the nested crate and session boundary. The
denylist permits only those slices under D-026. Unrelated demo code remains
denied.

The HUD adds no network surface. It renders existing shell payloads. The
localStorage preference does not migrate to `AppSettings` in this port.

## Verification

Characterization tests pin bar derivation, exact empty and control copy, hover
dwell, native hover, drag collapse, preference clearing, route selection,
startup restore, platform gates, and both entry points. Rust tests pin native
hover edge normalization.

Required checks include focused Vitest suites, frontend type and lint checks,
the nested crate tests, the shell tests or checks, formatting, and a search that
finds no effect hooks or Rust lint suppressions in added HUD code.

Manual parity uses the state table in `docs/hud-states.md`: every state,
transition, poll, and positioning behavior must match the reference build.

## Platform follow-ups

The macOS-only boundary is the scope of this parity port, not the intended end
state. Windows and Linux support must land as separate follow-up PRs. Each port
must preserve the renderer, copy, information architecture, motion, polling,
persistence, and entry points defined by PR #40 and `docs/hud-states.md`. A port
can change only its native platform adapter and the gates that expose it.

Each follow-up must update D-30 and `docs/support.md`, add native CI coverage,
and include screenshots and motion capture from the target platform. Do not
combine the Windows and Linux ports: their window-system risks and acceptance
matrices are independent.

### Windows HUD

Recommended tracking title: **Add the floating usage HUD to Windows**.

The Windows port must:

- replace the Windows no-op with a target-specific native window adapter;
- position against the monitor work area with the taskbar on any edge;
- handle auto-hidden taskbars, multiple monitors, and mixed-DPI scaling;
- keep the window above ordinary application windows without taking focus;
- verify transparent-window hit testing and document any click interception;
- preserve the visible HUD anchor during expansion, collapse, and direction
  changes;
- expose the Settings and Usage entry points only after the native adapter is
  available.

Acceptance requires Windows 11 x86-64 native CI, the complete HUD state table,
taskbars on all four edges, auto-hide, mixed-DPI multi-monitor movement, startup
restore, close-preference clearing, and manual screenshot and motion evidence.
Windows 10 remains outside the claim until its separate support gap is closed.

### Linux HUD

Recommended tracking title: **Add the floating usage HUD to supported Linux desktops**.

The Linux port must:

- define and document the supported X11 and Wayland compositor matrix;
- replace the Linux no-op only where reliable positioning and always-on-top
  behavior are available;
- position against the active monitor's usable work area and panel placement;
- handle multiple monitors, fractional scaling, and runtime display changes;
- verify transparency, focus behavior, dragging, and pointer hit testing on each
  claimed compositor;
- keep the HUD entry points hidden when the active window system cannot meet the
  contract;
- preserve the visible HUD anchor during expansion, collapse, and direction
  changes.

Acceptance requires native Linux CI plus manual coverage for every claimed
desktop and window system. The first port must test at least one mainstream X11
desktop and one mainstream Wayland desktop. Unsupported compositors must fail
closed without exposing a non-working HUD control.
