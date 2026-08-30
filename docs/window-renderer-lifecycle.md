# Desktop window renderer lifecycle

_Contributor reference for when desktop webview renderers are created, reused,
hidden, and destroyed._

The desktop shell stays resident because antiburn is a menu-bar application.
Its webview renderers do not need the same lifetime. The shell keeps a renderer
only while it is visible, is likely to be used again soon, or is completing a
bounded handoff.

This document covers the popover, onboarding, and Settings renderers. The HUD
and nudge windows have separate ownership rules. See [HUD states](hud-states.md)
for the HUD and its detail window.

## The shared lifecycle

`WindowReadiness` in
[`window_readiness.rs`](../apps/desktop/src-tauri/src/window_readiness.rs)
owns the common state machine:

```text
Idle --request--> Loading --matching readiness--> Ready
 ^                    |                              |
 |                    +--failed or destroyed--------+
 +------------------------destroyed-----------------+
```

A native build receives a monotonically increasing renderer generation. The
shell injects that generation before the page loads. After React commits the
window shell, `WindowReadyBoundary` reports the generation through
`window_ready`. The native window appears only when the report matches the
active generation and a reveal is pending.

This handshake gives the lifecycle these properties:

- A hidden window never appears before React has committed its shell.
- Repeated open requests share one active load instead of creating renderers.
- A click during a hidden prewarm changes the same load to reveal on readiness.
- A readiness report from a destroyed or replaced renderer has no effect.
- An open request can replace one stale load per load cycle. It does not create
  an unbounded retry loop.

The shared Tauri adapters in
[`window_lifecycle.rs`](../apps/desktop/src-tauri/src/window_lifecycle.rs) record
load timing, warn about the current stale generation, and reset failed loads.
The popover, Settings, and onboarding modules own their window-specific reveal
and destruction policies.

## Onboarding handoff and popover prewarm

Completing onboarding performs a deliberate handoff from the first-run window
to the menu-bar surface:

1. The shell hides onboarding immediately, changes macOS to accessory mode,
   and shows the menu-bar-location notification.
2. On the next main-loop turn, the shell requests one hidden popover renderer.
   This moves renderer startup out of the first menu-bar click.
3. It waits one second before destroying onboarding. This lets the final
   settings IPC response leave the renderer that sent it.
4. The popover remains hidden after it reports readiness. Readiness starts a
   one-minute handoff lease instead of revealing it.

`prewarm` is a handoff optimization, not a permanent resident window. It does
nothing while onboarding is pending, when a popover window already exists, or
when a popover load is already active. Its lease ends on the first reveal,
onboarding restart, application shutdown, or the one-minute timeout.
If readiness never arrives, a 65-second loading fail-safe destroys the hidden
renderer instead of leaving an unbounded WebContent process.
Clicks, cancellations, Pin, and stale replacement do not restart either
deadline. A replacement generation inherits the original absolute deadline.

### First click after onboarding

The first tray click reuses the prewarmed generation when it is ready or still
loading within the stale threshold:

- If the renderer is ready, the shell cancels eviction, places the existing
  window, and reveals it.
- If the renderer is still loading, the shell records a pending reveal. The
  matching readiness report reveals that same renderer when React commits.

If the loading prewarm has crossed the five-second stale threshold, the click
requests the lifecycle's one permitted replacement for that load cycle. The
shell destroys the old window and defers the replacement build until Tauri
releases the old window label. The replacement carries the pending reveal, so
no two renderers load in parallel.

A second toggle while the active renderer still loads cancels the pending
reveal. An onboarding prewarm keeps its remaining absolute lease; other loads
use the normal grace period.

Restarting onboarding cancels and destroys a renderer that still belongs only
to the onboarding prewarm. This prevents a hidden post-onboarding surface from
surviving when onboarding becomes the active application surface again.

## Popover idle eviction

Hiding is useful for a quick reopen, but it is not a long-term ownership model.
The popover keeps a hidden renderer for 15 seconds after:

- an ordinary dismissal, including focus loss, Escape, or a tray toggle; or
- cancellation of a non-prewarm reveal while the renderer is still loading.

The onboarding prewarm instead keeps its renderer for 60 seconds after
readiness. Cancelling a pending reveal does not extend or replace that absolute
deadline. The first reveal consumes the one-shot lease. A later dismissal uses
the normal 15-second grace period.

At the end of the grace period, the shell destroys the renderer only when all
of these conditions still hold:

- the eviction request is the current request;
- it still names the current renderer generation;
- the window is hidden; and
- for the normal 15-second grace period, the popover is not pinned.

An unrevealed onboarding prewarm expires at its absolute deadline even when
Pin is enabled. Pin protects a renderer only after the first successful reveal.

An open request cancels the eviction before it places or reveals the window.
Starting a newer grace period also invalidates the older timer. A stale timer
therefore cannot destroy a reopened or replaced renderer.

Pinned popovers are exempt because pinning is an explicit request to keep the
surface available. Unpinning a hidden popover starts the normal grace period.
After eviction, the next open starts from `Idle` and creates a fresh renderer
from native and persisted state.

## Settings teardown

Settings is created on demand and destroyed on close. It does not use a grace
period because closing an ordinary settings window is an explicit end to that
interaction, while every control has already written through to persisted
settings.

The title-bar close button and Command-W use the same native close path. The
global close policy allows Settings to close, and the `Destroyed` event resets
its readiness state. A later request creates a new renderer and restores its
state from persisted settings and the native services each pane reads.

If an open request finds a Settings renderer that has remained loading beyond
the stale threshold, the lifecycle destroys it and defers one replacement
until Tauri releases the old window label. Pane requests made during an active
load are delivered to that renderer; a newly built renderer takes its pending
pane once when it mounts.

## Insights cancellation at the Settings boundary

The Insights pane can run a native report reduction. Renderer teardown must
not leave that work running for a reader who has closed the pane or Settings.
Cancellation is enforced at two boundaries:

- `InsightsSession` cancels when its last subscriber leaves and when the
  document becomes hidden. It also stops its five-second status poll.
- The Settings `Destroyed` handler calls the native `InsightsController`
  directly. This covers native window destruction even when frontend cleanup
  cannot complete.

The native controller sets a cooperative cancellation flag. The read-only
reduction stops at its next cancellation probe, so cancellation cannot corrupt
stored evidence. Concurrent requests normally share one in-flight reduction.
A new request does not join a run whose cancellation flag is already set; it
starts a new reduction instead.

Application shutdown uses the same native cancellation signal.

## Lifecycle timing

| Timing     | Purpose                                                                                              | Start point                      |
| ---------- | ---------------------------------------------------------------------------------------------------- | -------------------------------- |
| 1 second   | Let onboarding's final IPC response complete before destroying its renderer                          | Onboarding completion            |
| 5 seconds  | Mark an active renderer load stale; log a warning and permit one replacement on a later open request | Renderer build start             |
| 15 seconds | Keep a dismissed popover available for a likely near-term reopen, then make it eligible for eviction | Hidden dismissal                 |
| 60 seconds | Keep the one post-onboarding handoff renderer available for the first menu-bar click                 | Onboarding prewarm readiness     |
| 65 seconds | Destroy an onboarding prewarm that never reports renderer readiness                                  | Onboarding prewarm build         |
| 5 seconds  | Refresh Insights processing status only while the pane has subscribers and remains visible           | Insights session start or resume |

These values serve different purposes. The stale threshold is a recovery
boundary, not an eviction deadline. Each eviction delay is a reuse window, not
a guarantee that a renderer will remain alive. A lifecycle reset, onboarding
restart, application shutdown, or build failure can end it earlier.

## Popover latency evidence

The shell records content-free timing boundaries for each menu-bar open:

- the open request and renderer generation;
- whether the request uses the onboarding prewarm;
- a renderer build that starts behind the request;
- renderer readiness and reveal; and
- the first settled activity and cached usage state.

The frontend reports content readiness only after both activity and cached
usage settle. An empty activity list counts as settled. A hidden prewarm can
reach this point before the first click, so the shell retains the milestone by
renderer generation and reports zero reveal-to-content time after reveal.

These boundaries decide whether a bootstrap snapshot is justified. Add one
only when release measurements show a median reveal-to-content interval of at
least 250 milliseconds. A snapshot must remain memory-only, derived from the
authoritative stores, bounded in serialized size, and invalidated with one
revision. It must refresh through the existing command path after reveal. If
the evidence does not meet the gate, renderer prewarm remains the complete
optimization.

## Memory guiding principles

Use these principles when adding or changing desktop windows:

1. **Keep the native shell resident, not every renderer.** A hidden webview has
   a process cost even when it paints nothing.
2. **Create on demand by default.** Prewarm only at a clear handoff where a
   near-term interaction is likely and the work has a bounded lifetime.
3. **Reuse one generation.** Coalesce repeated requests and turn an existing
   hidden load into a reveal rather than creating parallel renderers.
4. **Destroy after the interaction ends.** Use a short grace period only when
   it materially improves the next expected interaction.
5. **Keep durable state outside renderer lifetime.** A fresh renderer must be
   able to reconstruct the surface from native state, the local database, and
   persisted preferences.
6. **Cancel invisible work at the native boundary.** Frontend cleanup improves
   responsiveness, but native teardown must remain the final ownership gate.
7. **Bind delayed work to generations.** A timer or readiness report must prove
   it still belongs to the active renderer before it can reveal or destroy it.
8. **Treat visibility as a work gate.** Polling and scans should run only while
   the visible feature needs them.
9. **Validate the process tree, not only the main process.** Renderer presence
   and count are part of the lifecycle contract even when no benchmark is
   recorded in this document.
10. **Gate caches on measured latency.** A second representation of native
    state is justified only when a bounded snapshot improves a visible delay.

## Code map

| Concern                                                          | Source                                                                          |
| ---------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| Shared phases, generations, stale-load policy                    | [`window_readiness.rs`](../apps/desktop/src-tauri/src/window_readiness.rs)      |
| Tauri readiness, timing, and trace adapters                      | [`window_lifecycle.rs`](../apps/desktop/src-tauri/src/window_lifecycle.rs)      |
| Popover facade, first-click reuse, and Tauri window effects      | [`popover.rs`](../apps/desktop/src-tauri/src/popover.rs)                        |
| Popover leases, eviction tokens, and deadline ownership          | [`retention.rs`](../apps/desktop/src-tauri/src/popover/retention.rs)            |
| Popover latency milestones and structured timing                 | [`timing.rs`](../apps/desktop/src-tauri/src/popover/timing.rs)                  |
| Onboarding completion and delayed teardown                       | [`onboarding.rs`](../apps/desktop/src-tauri/src/onboarding.rs)                  |
| Settings creation, destruction, and native Insights cancellation | [`settings.rs`](../apps/desktop/src-tauri/src/settings.rs)                      |
| Global close and destroyed-window routing                        | [`lib.rs`](../apps/desktop/src-tauri/src/lib.rs)                                |
| React readiness marker                                           | [`WindowReadyMarker.tsx`](../apps/desktop/src/components/WindowReadyMarker.tsx) |
| Popover content-ready boundary                                   | [`PopoverSession.ts`](../apps/desktop/src/views/popover/PopoverSession.ts)      |
| Insights visibility and subscriber ownership                     | [`InsightsSession.ts`](../apps/desktop/src/views/settings/InsightsSession.ts)   |
| Native Insights cancellation and request sharing                 | [`insights_ipc.rs`](../apps/desktop/src-tauri/src/insights_ipc.rs)              |

## Change checklist

When a window lifecycle changes, verify all of these together:

- opening from `Idle`, `Loading`, and `Ready`;
- repeated opens and toggles during a load;
- a stale generation reporting readiness after replacement;
- close or hide behavior before and after readiness;
- delayed eviction after reopen, pin, unpin, or renderer replacement;
- destruction resetting readiness before the next build;
- cancellation of native work when its final visible owner leaves; and
- fresh reconstruction without relying on the previous renderer's memory.

Keep timings next to the native or frontend owner that enforces them. Tests
should assert both the duration and the condition that makes delayed work safe.
