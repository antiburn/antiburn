# Desktop window renderer lifecycle

_Contributor reference for when desktop webview renderers are created, reused,
hidden, and destroyed._

The desktop shell stays resident to support monitoring and the menu-bar companion.
The main popover renderer also stays resident after its first use, so the
application's primary surface can reopen immediately. Other renderers remain
bounded by their interaction or handoff.

This document covers the main window, popover, its peek companion, onboarding,
and Settings renderers. The HUD and nudge windows have separate ownership rules.
See [HUD states](hud-states.md) for the HUD and its detail window.

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
active generation and a reveal is pending. The main-window policy extends this
machine with generation-preserving `Ready` state, hidden verification, and a
`Terminal` recovery gate. Other windows do not enter that terminal phase.

This handshake gives the lifecycle these properties:

- A hidden window never appears before React has committed its shell.
- Repeated open requests share one active load instead of creating renderers.
- A click during a hidden prewarm changes the same load to reveal on readiness.
- A readiness report from a destroyed or replaced renderer has no effect.
- An open request can replace one stale load per load cycle. It does not create
  an unbounded retry loop.

The shared Tauri adapters in
[`window_lifecycle.rs`](../apps/desktop/src-tauri/src/window_lifecycle.rs) record
load timing and warn about the current stale generation. They reset failed
loads only for windows whose labels are free. Main-window failures use their
ownership-aware resolver instead. The main-window, popover, Settings, and
onboarding modules own their window-specific reveal and destruction policies.

## macOS overlay presentation

The HUD, HUD detail, popover, and nudge start hidden and unfocused. Their
native presentation runs on the main thread: configure stacking and Space
behavior, then use `orderFrontRegardless()` for visual reveal. All use
`CanJoinAllSpaces | FullScreenAuxiliary`. Tauri-backed surfaces reapply their
native policy before each reveal; popover refocus reapplies it too.

| Surface | Native level | Keyboard interaction |
| --- | --- | --- |
| HUD and detail | Screen saver (1000) | Non-focusable; detail also passes through clicks |
| Popover | Floating (3) | Takes key and gives its webview first responder separately from visual reveal |
| Nudge | Status (25) | Passive on arrival; takes key only after hover |
| Native popover preview | Matches the anchor before presentation | Cannot become key or main |

HUD and detail use passive nonactivating panels, retaining their existing
screen-saver level while the fullscreen fix is validated. Level alone did not
make their previous ordinary windows visible over fullscreen apps. The
nonactivating popover panel stays below system menus and status-level nudges.
Nudges remain above floating windows. HUD and detail additionally use
`Stationary | IgnoresCycle`.

HUD, detail, popover, and nudge resolve or convert their panel in the same
main-thread callback that configures and reveals it. Failed conversion does not fall back
to an activating Tauri show or focus operation. The popover records reveal
completion only after native presentation, not after queueing a callback.
Nudge key release returns focus through the popover's nonactivating path.

Native previews initialize their nonactivating style and fullscreen collection
behavior before presentation. Frame placement reapplies the anchor's level.
They bypass Tauri window conversion, so they do not need repeated collection
configuration to defend against toolkit changes.

This policy does not change Main, Settings, or Onboarding activation. It also
does not remove the upstream Wry cold-webview creation limitation documented
in the [desktop README](../apps/desktop/README.md#known-gaps). Creation-time
activation and reveal-time activation require separate macOS QA.

## Main window

The main window uses a dedicated frontend entry and a retained renderer. An
explicit launch creates it after onboarding. Background startup does not
construct its webview. After first use, closing hides the window and preserves
its renderer for the next open. Quit stops the application and all renderers.

The native mechanism crate owns creation, reveal, and placement geometry. The
shell owns readiness, persisted placement, launch intent, bounded recovery, and
the close policy. Repeated opens join the same load. A matching initial
readiness report permits the first reveal without waiting for data or network
requests.

A hidden or minimized ready renderer must answer one event-driven health check
before native reveal. The check names both its renderer generation and request
ID. Concurrent opens share that request. The renderer answers only after React
commits either the application tree or its independent error fallback. A
healthy application commit permits reveal. A fallback commit starts bounded
replacement. A dropped event is recovered by one generation-scoped pull after
the responder installs. There is no heartbeat, poll, extra webview, network
wait, data wait, or hidden animation-frame wait.

The 300 ms health timeout is provisional. It is the failure detector when a
WebContent process cannot answer. Emitting an event does not establish health.
A healthy acknowledgement establishes JavaScript response and a committed
application tree, but not painted pixels. Native reveal completion also does
not prove a frame was presented.

Automatic recovery permits two consecutive failures. A healthy status report
from the active generation is the only automatic budget reset. Renderer
readiness, fallback commit, acknowledgement, reveal, and destruction do not
reset it. Every destroy-and-replace generation has a 10-second provisional
watchdog. The watchdog also covers ordinary stale replacement, fallback Reload,
build and destroy retries, and native-dialog Try Again.

Recovery preserves native label ownership:

- A failed destroy remains a pending-destroy load or becomes terminal with the
  native label still owned.
- A failed build receives a fresh generation. If a native handle exists, the
  shell destroys it before rebuilding. Native build and failure resolution run
  after the readiness guard is released.
- A watchdog replaces a hung load without using reveal intent as a gate. Closing
  during recovery keeps the hidden recovery and watchdog active.
- A delayed or duplicate `Destroyed` event cannot reset terminal state. It can
  only record that the terminal window no longer owns the label.
- The state machine never returns to `Idle` while a native window may own the
  `main` label.

Budget exhaustion enters a terminal phase and opens a native **Try Again** or
**Dismiss** dialog. A hidden recovery defers that dialog until the next open.
Each dialog has a token, and its worker callback returns to the main thread
before token validation or any state change. Try Again grants one fresh budget
only after both the token and terminal transition match. Dismiss keeps the
terminal gate, so a later open presents a new informed choice. This surface
works without a responsive WebContent process.

The session target is a retained revision, not a destructive mount-time take.
An active loading or ready generation peeks, applies, and acknowledges the
latest revision. An acknowledgement received while loading remains recorded.
The shell retires it only after that same generation becomes ready and is also
presented. Acknowledgement and presentation can happen in either order.
Recovery removes a doomed generation's application acknowledgement but retains the pending
target. Accepted destroy or build failures, terminal entry, Dismiss, and Try
Again also retain it. Only reconciliation or a revision-matched genuine open
failure clears it. A recovered renderer therefore does not replay an old target
after the user has navigated elsewhere.

The root `MainWindowErrorBoundary` is inside `WindowReadyBoundary`. Its static
fallback can commit the initial readiness marker and remain revealable. The
boundary defers healthy classification until commit error handling settles.
A descendant callback-ref failure therefore commits fallback without first
reporting a healthy application. Global errors and unhandled rejections send
bounded closed-schema diagnostics only. They do not declare the renderer fatal
or trigger recovery. Reports contain no
message, URL, path, stack, rejection payload, session identity, or free-form
text. Native accepts at most five reports per generation.

The main window adds no scans or provider polling. Views gate presentation work
on visibility rather than focus and consume existing native state. An
unfocused window can still be visible beside another application.

Opening timing is local diagnostic evidence. Hidden-warm `elapsed_ms` now
includes the health handshake. See the
[validation runbook](runbooks/main-window.md) for separate cold, first-open,
warm, acknowledgement, and visible-content measurements.

## Onboarding handoff and popover prewarm

Completing onboarding performs a deliberate handoff from the first-run window
to the menu-bar surface:

1. The shell hides onboarding immediately, opens the main window, and shows
   the menu-bar-location notification. macOS retains regular application mode.
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
reveal. An onboarding prewarm keeps its remaining absolute lease. A normal
popover load stays available for the next open request.

Restarting onboarding cancels and destroys a renderer that still belongs only
to the onboarding prewarm. This prevents a hidden post-onboarding surface from
surviving when onboarding becomes the active application surface again.

## Popover residency and prewarm eviction

After a normal popover renderer is created, ordinary dismissal only hides it.
Focus loss, Escape, a tray toggle, reveal cancellation, and unpinning do not
schedule destruction. The next open reuses the renderer and its loaded state.
The renderer remains available until application shutdown or an explicit
lifecycle reset.

The onboarding prewarm keeps its renderer for 60 seconds after readiness. A
prewarm that never becomes ready has a 65-second loading fail-safe. Cancelling
a pending reveal does not extend or replace either absolute deadline. The
first successful reveal consumes the one-shot lease and makes the renderer
resident.

At a prewarm deadline, the shell destroys the renderer only when all of these
conditions still hold:

- the eviction request is the current request;
- it still names the current renderer generation;
- the window is hidden.

An unrevealed onboarding prewarm expires at its absolute deadline even when
Pin is enabled. After the first successful reveal, there is no eviction
deadline for Pin to change.

An open request cancels the eviction before it places or reveals the window.
Starting a newer prewarm schedule also invalidates the older timer. A stale
timer therefore cannot destroy a revealed or replaced renderer. After prewarm
eviction, the next open starts from `Idle` and creates a fresh renderer from
native and persisted state.

## Peek companion interaction lifetime

The passive peek companion is created on the first provider or checks hover.
Starting the application and revealing the main popover do not create its
renderer. A request made while the renderer loads remains attached to its
target generation and reveals after the matching readiness and presentation
reports.

Pointer exit starts the existing bridge and outside delays. The renderer first
clears the current target and hides, then the shell destroys its native window.
The concealment generation also owns the delayed destruction, so a new hover or
retarget makes an older destruction callback stale. If native destruction wins
a race with a new target, the destroyed-window handler rebuilds only while that
target is still active. Renderer readiness then redelivers the retained request.

Hiding or destroying the main popover starts the same concealment path. The
companion therefore exists only for an active peek interaction and its bounded
pointer-exit transition. A renderer that does not acknowledge concealment is
hidden and destroyed after the configured 80-millisecond fallback.

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
| 300 ms     | Provisional main-window hidden health acknowledgement timeout                                       | Hidden retained open             |
| 10 seconds | Provisional main-window recovery watchdog for each replacement generation                           | Recovery generation start        |
| 60 seconds | Keep the one post-onboarding handoff renderer available for the first menu-bar click                 | Onboarding prewarm readiness     |
| 65 seconds | Destroy an onboarding prewarm that never reports renderer readiness                                  | Onboarding prewarm build         |
| 5 seconds  | Refresh Insights processing status only while the pane has subscribers and remains visible           | Insights session start or resume |

These values serve different purposes. The stale threshold is a recovery
boundary, not an eviction deadline. The main-window 300 ms and 10-second values
require post-implementation calibration before merge. No distribution is
recorded here yet. The prewarm eviction delays are bounded handoff windows, not
guarantees that a renderer will remain alive. A lifecycle reset, onboarding
restart, application shutdown, or build failure can end one earlier.

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

1. **Keep repeated primary surfaces ready.** The main popover remains resident
   after first use; one-shot and explicitly closed renderers do not.
2. **Create on demand by default.** Prewarm only at a clear handoff where a
   near-term interaction is likely and the work has a bounded lifetime.
3. **Reuse one generation.** Coalesce repeated requests and turn an existing
   hidden load into a reveal rather than creating parallel renderers.
4. **Bound one-shot ownership.** Destroy handoff renderers when their lease
   ends, and close ordinary windows when their interaction ends.
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
| Main ownership, recovery, dialogs, targets, and native effects   | [`main_window.rs`](../apps/desktop/src-tauri/src/main_window.rs)                |
| Popover facade, first-click reuse, and Tauri window effects      | [`popover.rs`](../apps/desktop/src-tauri/src/popover.rs)                        |
| Popover prewarm leases, eviction tokens, and deadline ownership  | [`retention.rs`](../apps/desktop/src-tauri/src/popover/retention.rs)            |
| Peek target policy and shell hooks                               | [`popover_peek.rs`](../apps/desktop/src-tauri/src/popover_peek.rs)              |
| Peek generations, concealment, and renderer destruction          | [`anchored-window`](../apps/desktop/src-tauri/crates/anchored-window/src/lib.rs) |
| Popover latency milestones and structured timing                 | [`timing.rs`](../apps/desktop/src-tauri/src/popover/timing.rs)                  |
| Onboarding completion and delayed teardown                       | [`onboarding.rs`](../apps/desktop/src-tauri/src/onboarding.rs)                  |
| Settings creation, destruction, and native Insights cancellation | [`settings.rs`](../apps/desktop/src-tauri/src/settings.rs)                      |
| Global close and destroyed-window routing                        | [`lib.rs`](../apps/desktop/src-tauri/src/lib.rs)                                |
| React readiness marker                                           | [`WindowReadyMarker.tsx`](../apps/desktop/src/components/WindowReadyMarker.tsx) |
| Main application/fallback boundary                               | [`MainWindowErrorBoundary.tsx`](../apps/desktop/src/components/MainWindowErrorBoundary.tsx) |
| Main committed-health store                                      | [`rendererHealth.ts`](../apps/desktop/src/lib/rendererHealth.ts)                |
| Main hidden-open responder                                       | [`mainWindowHealth.ts`](../apps/desktop/src/lib/mainWindowHealth.ts)            |
| Main typed health and target IPC                                 | [`mainWindowIpc.ts`](../apps/desktop/src/lib/mainWindowIpc.ts)                  |
| Main bounded bootstrap diagnostics                               | [`bootstrapDiagnostics.ts`](../apps/desktop/src/lib/bootstrapDiagnostics.ts)    |
| Popover content-ready boundary                                   | [`PopoverSession.ts`](../apps/desktop/src/views/popover/PopoverSession.ts)      |
| Insights visibility and subscriber ownership                     | [`InsightsSession.ts`](../apps/desktop/src/views/settings/InsightsSession.ts)   |
| Native Insights cancellation and request sharing                 | [`insights_ipc.rs`](../apps/desktop/src-tauri/src/insights_ipc.rs)              |

## Change checklist

When a window lifecycle changes, verify all of these together:

- opening from `Idle`, `Loading`, and `Ready`;
- repeated opens and toggles during a load;
- a stale generation reporting readiness after replacement;
- close or hide behavior before and after readiness;
- resident reuse after dismissal, reveal cancellation, and unpinning;
- prewarm expiry after readiness, reopening, Pin, or renderer replacement;
- destruction resetting readiness before the next build;
- cancellation of native work when its final visible owner leaves; and
- fresh reconstruction without relying on the previous renderer's memory;
- a cancelled or replaced verification cannot destroy or reveal;
- a failed destroy never yields `Idle` while its label can remain owned;
- closing a recovery cannot strand its loading generation;
- delayed destruction cannot reset a terminal recovery ledger;
- only a generation-scoped healthy application commit resets recovery budget;
- a loading generation's session-target acknowledgement remains until
  readiness and presentation;
- a doomed generation's session-target acknowledgement retires nothing;
- an accepted recovery failure never clears the pending session target; and
- a commit-phase callback-ref failure cannot report the application as healthy.

Keep timings next to the native or frontend owner that enforces them. Tests
should assert both the duration and the condition that makes delayed work safe.
