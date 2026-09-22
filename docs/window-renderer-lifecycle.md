# Desktop window renderer lifecycle

_Contributor reference for when desktop webview renderers are created, reused,
hidden, and destroyed._

The shell stays resident for monitoring and the menu-bar companion. The main
window and popover keep their renderers after first use so repeat opens are
immediate. Onboarding, Settings, and the peek companion have bounded
interaction lifetimes. The HUD and nudge have separate owners; see
[HUD states](hud-states.md) for HUD behavior.

## The shared lifecycle

[`WindowReadiness`](../apps/desktop/src-tauri/src/window_readiness.rs) tracks
`Idle → Loading → Ready` by a monotonically increasing renderer generation.
The shell injects that generation before page load. After React commits the
window shell, `WindowReadyBoundary` reports it to native code. Only readiness
from the active generation can satisfy a pending reveal. Repeated requests
share one load, and a stale load can be replaced once per load cycle. Destruction
must release the native window label before a replacement build uses it.

The main window extends this policy with a health check for hidden ready
renderers and a terminal recovery gate. The shell module for each surface owns
its reveal, hide, and destruction decisions; the shared
[readiness adapter](../apps/desktop/src-tauri/src/window_lifecycle.rs) provides
timing and stale-load warnings. A readiness report from a destroyed generation
cannot reveal a new one.

For new window policies, create renderers on demand by default. Prewarm only
at a handoff with a likely near-term interaction and a bounded lease. Reuse an
active generation rather than starting parallel loads. Keep durable state in
native services, the local database, or persisted preferences so a fresh
renderer can reconstruct the surface. Cancel work when its final visible owner
leaves; frontend cleanup helps, but native teardown is the final ownership
gate. Every delayed reveal or destruction must prove that its request and
renderer generation are still current.

## macOS overlay presentation

HUD, HUD detail, popover, and nudge start hidden and unfocused. Their native
presentation runs on the main thread, sets stacking and Space behavior, and
reveals with `orderFrontRegardless()`. Tauri-backed panels reapply that policy
before each reveal; popover refocus reapplies it too.

| Surface        | Stacking and input                                                                                                |
| -------------- | ----------------------------------------------------------------------------------------------------------------- |
| HUD and detail | Passive nonactivating panels at screen-saver level. Neither can become key or main; detail passes clicks through. |
| Popover        | Nonactivating floating panel below system menus and nudges. Visual reveal and keyboard focus are separate steps.  |
| Nudge          | Status-level panel, passive on arrival; it takes key only after hover.                                            |
| Native preview | Matches its anchor's level and cannot become key or main.                                                         |

All four overlays use `CanJoinAllSpaces | FullScreenAuxiliary`; HUD and detail
also use `Stationary | IgnoresCycle`. Level alone did not make the previous
ordinary HUD window visible over fullscreen apps. Panel conversion and reveal
must happen in one main-thread callback. Failed conversion leaves the surface
hidden; it must not fall back to an activating show or focus operation. Native
previews set their nonactivating style before presentation and bypass Tauri
window conversion.

This policy does not change activation of Main, Settings, or Onboarding.
Upstream Wry can still activate during cold webview creation; the
[desktop README](../apps/desktop/README.md#known-gaps) tracks that limit.
Creation-time and reveal-time activation need separate macOS QA.

## Main window

Explicit launch creates the main window after onboarding; background startup
does not construct it. Closing normally hides its renderer, while Quit destroys
it. On Windows and Linux, closing exits instead when the tray icon is hidden.
The native mechanism owns window creation, placement, and reveal geometry.
The shell owns readiness, saved placement, launch intent, recovery, and close
policy. Initial matching readiness allows the first reveal without waiting for
data or network work.

A hidden or minimized ready renderer must answer one event-driven health
request before reveal. The request names its generation and request ID;
concurrent opens share it. The renderer answers only after React commits either
the application tree or its independent error fallback. A dropped event gets
one generation-scoped pull after the responder installs. There is no heartbeat
or hidden frame loop. A healthy reply proves a responsive JavaScript path and
committed application tree, not painted pixels; native reveal completion also
does not prove a presented frame.

Automatic recovery permits two consecutive failures. Only a healthy
application report from the active generation resets that budget. Readiness,
fallback commit, acknowledgement, reveal, and destruction do not. Every
replacement generation has a bounded watchdog, including ordinary stale-load
replacement, fallback Reload, build or destroy retry, and Try Again. Closing
during recovery keeps the hidden recovery and watchdog active.

Recovery must retain native label ownership correctly:

- A failed destroy remains a pending-destroy load or enters terminal recovery
  while the label is still owned. It cannot return to `Idle`.
- A failed build uses a fresh generation. If a native handle exists, the shell
  destroys it before rebuilding. Build and failure resolution run outside the
  readiness guard.
- A hung load can be replaced without reveal intent. A late or duplicate
  `Destroyed` event cannot reset a terminal ledger; it only records release of
  the label.

When the budget is exhausted, a native **Try Again** or **Dismiss** dialog works
without a responsive webview. Hidden recovery defers it until the next open.
The dialog callback returns to the main thread and must match both its token
and terminal transition. Try Again grants one fresh budget; Dismiss retains the
terminal gate so a later open presents a new choice.

A requested session target is a retained revision. A loading or ready
generation applies and acknowledges the latest target, but the shell retires
it only after that same generation is both ready and presented. Those two
events may arrive in either order. Recovery removes a doomed generation's
application acknowledgement while retaining the target. Accepted build or
destroy failures, terminal entry, Dismiss, and Try Again also retain it. Only
reconciliation or a revision-matched genuine open failure clears it. This
prevents both lost navigation and replay of an old target after navigation
elsewhere.

The root `MainWindowErrorBoundary` sits inside `WindowReadyBoundary`.
Its static fallback can commit readiness and remain revealable. Healthy
classification waits until commit error handling settles, so a callback-ref
failure does not report a healthy application first. Global errors and
unhandled rejections send only bounded, closed-schema diagnostics; they
neither declare fatal state nor trigger recovery. Reports omit messages,
paths, URLs, stacks, rejection payloads, and session identity, and native
accepts at most five per generation.

The main window uses existing native state, with no new scans or provider
polling. Views gate presentation work on visibility, not focus: an unfocused
window can remain visible beside another app. The shared session list serves
Overview, sidebar, and Sessions through one bounded refresh lifecycle while
the main window is visible. The selected detail loads its additional work only
while Sessions is active; hiding or minimizing suspends both, and returning
reconciles them. See the
[main-window validation runbook](runbooks/main-window.md) for cold, warm,
health-acknowledgement, and visible-content measurements.

## Onboarding handoff and popover prewarm

Onboarding completion hides the first-run window, opens Main, and shows the
menu-bar-location notification. On the next main-loop turn the shell requests
one hidden popover renderer. It delays onboarding destruction long enough for
the final settings IPC response to leave that renderer. This prewarm moves
startup cost away from the first tray click; readiness leaves it hidden.

The first click reuses a ready prewarm, or records reveal intent against its
loading generation. A second toggle while loading cancels that intent. If the
load is stale, one replacement waits for destruction and release of the old
label; the pending reveal follows the replacement. Two popover renderers do not
load in parallel. Starting onboarding again destroys an unrevealed prewarm that
belongs to the completed handoff.

The prewarm is a one-shot lease. It expires one minute after readiness, or
after a bounded loading fail-safe if readiness never arrives. Clicks,
cancellations, Pin, and stale replacement do not extend the original absolute
deadline; a replacement inherits it. The first successful reveal consumes
the lease and makes the popover resident. A deadline destroys only a hidden
window when both its eviction request and renderer generation still match.
An open request invalidates eviction before placement or reveal, so an old
timer cannot destroy a shown or replaced renderer.

After ordinary popover creation, focus loss, Escape, tray toggles, reveal
cancellation, and unpinning hide rather than destroy its renderer. A later
open reuses it. Shutdown or explicit lifecycle reset ends residency. After
prewarm eviction, a later open starts fresh from native and persisted state.

General → Application changes app-presence settings without restarting a
renderer. Hiding the tray icon unpins and hides the tray-owned popover.
On macOS, the store repairs a malformed both-hidden state by restoring the
Dock icon. On Windows and Linux, closing Main exits when the tray icon is
hidden.

## Peek companion interaction lifetime

The passive peek companion is created on first provider or checks hover, not
at startup or when the main popover opens. A request during loading stays
attached to its target generation and reveals only after matching readiness
and presentation.

Pointer exit begins concealment: the renderer clears its target and hides,
then native destroys the window. The concealment generation owns delayed
destruction. A new hover or retarget invalidates old callbacks. If native
destruction wins the race, the destroyed handler rebuilds only while the new
target remains active, and readiness redelivers that request. Hiding or
destroying the popover uses the same concealment path. A bounded native
fallback hides and destroys a renderer that cannot acknowledge concealment.

## Settings teardown

Settings is created on demand and destroyed on close. Controls write through
to persisted settings, so a later renderer can reconstruct them. The title-bar
close button and Command-W use the same native close path; `Destroyed`
resets readiness. A stale loading renderer gets one replacement after its
label is released. Pane requests during loading go to the active renderer;
a replacement takes its pending pane when it mounts.

## Checks report cancellation

The popover and main Burn Checks surfaces can request one native report
reduction. Each request carries a consumer ID. When a surface no longer needs
the report, it releases its ID; the native controller cancels the reduction
only if that ID is still the current Checks consumer. An old release cannot
cancel a newer consumer. Shutdown sends the same cancellation signal. The
reduction is read-only and stops at its next cooperative probe. Concurrent
current requests can share a run, while a new request never joins a run already
marked for cancellation.

## Timing and validation

The main hidden-renderer health timeout and replacement watchdog are failure
detectors, not proof that pixels appeared. A stale-load threshold permits one
replacement on a later open; it is not an eviction timer. The popover prewarm
lease and loading fail-safe are absolute handoff deadlines. Keep numerical
timings with the native or frontend owner that enforces them; validate their
duration and generation guard together. The main-window timeout values still
need release calibration.

Popover timing records content-free boundaries for open, generation, build,
readiness, reveal, and settled activity and cached usage. Content readiness
requires both activity and cached usage to settle; an empty activity list
counts as settled. A hidden prewarm can reach that point before the click, in
which case reveal-to-content time is zero.

Add a popover bootstrap snapshot only if release measurements show a median
reveal-to-content interval of at least 250 ms. The snapshot must stay in
memory, derive from authoritative stores, have a bounded serialized size, and
invalidate with one revision. It must refresh through the existing command
path after reveal. Below that gate, renderer prewarm remains the complete
optimization.

## Owners

| Concern                                      | Source                                                                                                                                                                                                                               |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Main readiness, recovery, and native effects | [`main_window.rs`](../apps/desktop/src-tauri/src/main_window.rs)                                                                                                                                                                     |
| Popover reveal and retention                 | [`popover.rs`](../apps/desktop/src-tauri/src/popover.rs), [`retention.rs`](../apps/desktop/src-tauri/src/popover/retention.rs)                                                                                                       |
| Onboarding and Settings lifetime             | [`onboarding.rs`](../apps/desktop/src-tauri/src/onboarding.rs), [`settings.rs`](../apps/desktop/src-tauri/src/settings.rs)                                                                                                           |
| Peek companion and native preview            | [`popover_peek.rs`](../apps/desktop/src-tauri/src/popover_peek.rs), [`native.rs`](../apps/desktop/src-tauri/crates/anchored-window/src/macos/native.rs)                                                                              |
| Frontend readiness and main health           | [`WindowReadyMarker.tsx`](../apps/desktop/src/components/WindowReadyMarker.tsx), [`mainWindowHealth.ts`](../apps/desktop/src/lib/mainWindowHealth.ts)                                                                                |
| Main list and detail visibility              | [`MainActivitySession.ts`](../apps/desktop/src/views/main-window/MainActivitySession.ts)                                                                                                                                             |
| Checks report cancellation                   | [`BurnChecksSession.ts`](../apps/desktop/src/views/main-window/BurnChecksSession.ts), [`PopoverSession.ts`](../apps/desktop/src/views/popover/PopoverSession.ts), [`insights_ipc.rs`](../apps/desktop/src-tauri/src/insights_ipc.rs) |

When changing a window policy, test requests from `Idle`, `Loading`, and
`Ready`; repeated toggles; stale readiness after replacement; hide and close
before readiness; resident reuse; prewarm expiry; and fresh reconstruction.
Exercise delayed callbacks after reveal, replacement, terminal recovery, and
native destruction. For Main, also test health classification after a
commit-time fallback, budget reset only by an active healthy generation,
label ownership after failed destroy, recovery after close, and target
acknowledgement in either order around readiness and presentation. For
Checks and peek, verify cancellation or concealment when the final visible
owner leaves. Process-tree checks should include renderer count, not only
main-process memory.
