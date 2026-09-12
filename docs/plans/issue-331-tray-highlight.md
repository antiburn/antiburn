# Issue 331: Keep the menu-bar highlight paired with the popover

## Findings and outcome

[Issue #331](https://github.com/antiburn/antiburn/issues/331) reports flicker on
macOS with antiburn 0.3.1. Revised against `origin/main` at `bd9ad46b` (app 0.5.2),
rebased from `a5eeb5ad`. The current source still contains the documented cause.
The implementation and local native comparison are complete. The validation
record below separates observed behavior from the remaining platform checks.

- The pinned `tray-icon` revision, `0ada43072646fe4454b1f969f4f446dc401fa1aa`,
  calls `highlight(false)` in macOS `mouseUp:` before delivering the click.
  `tray::set_highlight(true)` subsequently lights the button from `note_shown`.
  These competing writes explain an opening blink.
- The visible, pinned branch of `popover::toggle` only anchors and focuses the
  window. It never calls `note_shown`, so native mouse-up can leave the button
  dark while the pinned popover remains visible.
- `note_hidden` clears the highlight unconditionally. Focus loss, outside clicks,
  and reopen suppression are relevant trace points, but source inspection does
  not prove that they cause the reported repeated flicker.

### Changes on main that affect this plan

- PR #484 changes the popover's focus sequence: order the panel front, make it
  key, then make the actual WKWebView first responder. The panel now uses
  `CanJoinAllSpaces | FullScreenAuxiliary`. Preserve this sequence and collection
  behavior; reproduce on this revision before attributing any blur to the old
  focus path.
- PR #484 also moves macOS hover previews to a native `PassivePanel` with its own
  WKWebView. It cannot become key or main. Observe preview presentation through
  the anchored-window manager; do not expect a Tauri `popover-peek` window or its
  window events on macOS.
- PR #487 adds retained main-window health checks and recovery to shared readiness
  code. The popover still uses its existing open/toggle readiness path. Do not
  introduce the main window's health handshake or recovery policy into this fix.
- `tray.rs`, `popover.rs`, `global_click.rs`, and the pinned tray dependency are
  unchanged from the previous plan baseline. The dependency ownership proposal
  therefore remains applicable; the native regression matrix expands below.

Success means the icon lights once when the popover becomes visible, stays lit
throughout its visibility, and clears when it hides. Native context-menu tracking
can also highlight the item while its menu is open.

## Implementation

### Establish the event sequence

Use the isolated debug app through `pnpm --filter @antiburn/desktop dev`.
Temporarily trace native mouse down/up, application highlight requests,
`note_shown`, `note_hidden`, dismissal reasons, and suppression decisions. Include
monotonic elapsed time, actual visibility, pin state, and renderer generation.
Trace panel focus callback execution separately from its scheduling, including
key-window and first-responder changes. Record cold and warm opens, pinned clicks,
a right-click menu interaction, Space transitions, and native preview show/hide.
Keep diagnostics local and content-free; remove temporary instrumentation before
shipping. Do not tune the 250 ms suppression interval without evidence.

### Give the app explicit highlight ownership

- Vendor the current pinned `tray-icon` source at
  `apps/desktop/src-tauri/vendor/tray-icon`. Replace the Git patch with a Cargo
  path patch, exclude the vendor crate from the shell workspace, and update the
  lockfile. Preserve the existing macOS 27 menu attachment fix and upstream
  licenses. Record the base revision and local delta in a vendor README.
- Add a macOS-only `TrayIcon::set_highlight_override(Option<bool>)` method.
  `None` preserves upstream automatic behavior; `Some(bool)` gives the app
  control. Store the override across native status-item recreation, and share
  its current value with the native event target.
- With an override active, skip automatic highlight writes on primary mouse
  down/up. Keep click delivery unchanged. Continue native context-menu tracking
  and restore the latest override after tracking returns. Avoid holding a
  mutable borrow across native menu tracking or event delivery.
- Initialize antiburn's override to `Some(false)` immediately after tray creation.
  Implement the existing `tray::set_highlight` wrapper with the new method through
  `with_inner_tray_icon`; retain its non-macOS no-op. Report bridge failures instead
  of silently ignoring them.
- Keep `note_shown` and `note_hidden` as the visibility owners. The pinned raise
  path requires no synthetic shown event: mouse-up no longer clears its highlight.
  Preserve usage polling and all existing dismissal gates.
- Keep panel focus, Space collection behavior, native preview management, and
  shared renderer readiness unchanged. The tray override must not activate the
  app, make another window key, or synthesize extra visibility events.
- Replace the obsolete accepted-flicker comments with the ownership contract and
  patch retirement condition: remove the vendor patch when a compatible upstream
  release provides equivalent control and retains the macOS 27 fix.

No frontend API, persisted setting, or product analytics event changes are needed.

## Validation and acceptance

- Add dependency-level tests for default automatic behavior, explicit true/false
  overrides, mouse-up preservation, clearing the override, and restoring the
  latest value after menu tracking. Exercise the native event handler where
  feasible; pure state tests alone cannot prove rendering behavior.
- Run shell `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`. Run the vendor crate's relevant formatting and tests separately
  because it is excluded from the workspace. Verify Cargo resolves exactly one
  `tray-icon`, from the intended path, and check third-party notices.
- On macOS 27 and an available supported earlier macOS version, verify cold/warm
  opens, repeated open/close clicks, pinned raises, pin/unpin through the menu,
  right-click menu dismissal, Escape, outside clicks, app switching, native
  dialogs, and nudge focus handoffs. Check both menu-bar appearances and tray
  usage-meter updates while the popover remains open.
- Repeat opening, pinned raising, typing, and Escape on ordinary and full-screen
  Spaces. Confirm the previous app remains active while the popover receives
  keyboard input. Verify the highlight follows actual visibility across Space
  switches rather than assuming that every switch hides the panel.
- Show, retarget, and dismiss provider/check hover previews. Their passive native
  panels must not steal key status, activate antiburn, or clear the popover's
  highlight. Hiding the popover must conceal its preview and clear the highlight.
- Exercise the tray menu's Open antiburn action with a retained main window,
  including its existing unhealthy-renderer recovery. Confirm the tray highlight
  still follows popover visibility during that handoff. Reuse existing recovery
  tests; add no health-check machinery to the popover.
- Check cancelled or failed opens and onboarding redirection: no persistent
  highlight may remain without a visible popover or tracked menu. Preserve
  existing click handling, including drag/release behavior.
- Capture before/after video and correlate it with the event trace. A correct
  final Boolean is insufficient: ordinary opening must have no on/off/on flash.
  Investigate any extra hide/show transition before declaring the issue fixed.
- Use Windows/Linux CI builds to confirm the macOS-only API and path patch do not
  regress the other tray backends. Do not claim native validation on unavailable
  macOS versions; record that gap explicitly.

## Defaults and boundaries

The implementation default remains a vendored patch rather than a separate
repository. Primary-button highlight
follows actual popover visibility, with no temporary pressed highlight before
reveal; cold loading therefore stays unlit until the window appears. This avoids
introducing pending-open highlight state or another source of flicker.

Do not add timers, repeated re-highlighting, Objective-C method swizzling, or a
broad popover state-machine rewrite. Preserve existing pin, onboarding, focus,
Space, preview, and reopen-suppression behavior. If runtime traces establish a separate lifecycle
race, capture that failing sequence and refine this plan before expanding the fix.

## Implementation and validation record — 2026-09-11

The vendored override and app integration are implemented. `note_shown` and
`note_hidden` retain highlight ownership; no popover lifecycle, frontend, or
focus behavior changes were required. The notices generator includes the
vendored path dependency, preserving its existing license attribution without
changing `THIRD_PARTY_NOTICES`. CI checks the excluded vendor crate separately
and enables its native AppKit harness on macOS.

### Native comparison

Isolated baseline and patched debug apps ran on macOS 26.4.1 (25E253). Temporary
diagnostics remained in disposable source copies outside the repository.

- The baseline reproduced a dark tray button after clicking an already-visible,
  pinned popover. Mouse-down/up reached the app while it remained visible and
  pinned. The patched build retained its highlight through the same sequence.
- Cold and warm opening, repeated open/close, native menu pin/unpin, and pinned
  app switching behaved correctly in the patched build. Hiding cleared the
  highlight; reopening restored it after the popover became visible.
- Before/after recordings and event logs were captured locally. The recordings
  did not resolve the baseline's brief opening highlight transition. The patched
  recording showed one dark-to-lit transition at the recording's resolution.
- The maintainer also tested clicking the patched build and reported that the
  release build's flicker was absent.

Local evidence is under `/private/tmp/antiburn331-evidence/`; it is temporary,
not a committed test fixture. The decisive pinned-state images are
`baseline-pinned-dark.png` and `patched-pinned-settled.png`.

### Automated evidence

- Shell formatting and Clippy with warnings denied passed. The shell suite
  passed 1,119 tests; two filesystem-watcher tests failed under the sandbox.
  All 28 watcher tests, including both failures, then passed with normal native
  filesystem-event access.
- Vendor formatting, Clippy, six library tests, and the explicitly enabled native
  AppKit harness passed. The harness exercises actual status-button mouse events,
  event delivery, explicit true/false, automatic mode, and status-item recreation.
  Deterministic tests cover restoring the latest override after menu tracking.
- Desktop lint, type checking, formatting, build, and all 1,317 frontend tests
  passed. Six notices-generator tests and the notices consistency check passed.
- CI classification tests, workflow YAML parsing, and workflow formatting passed.
  Cargo resolves one `tray-icon` instance from the intended vendor path.
- Independent Rust and TypeScript reviews inspected the native implementation
  and notices changes. The Rust reviewer also passed a Windows cross-check.

### Remaining runtime coverage

macOS 27 is unavailable locally. Its dynamic menu-attachment code is retained,
but the macOS 27 runtime matrix remains unverified. Windows and Linux runtime
behavior also requires CI or another host; local Linux cross-validation was
blocked by an uncached dependency and sandbox registry access.

Full-screen and Space transitions, both menu-bar appearances, typing, native
dialogs, nudge handoffs, usage-meter updates, and the complete passive-preview
focus matrix were not independently exercised. Automated Escape injection did
not establish delivery to the nonactivating panel, so Escape is not marked as
passed. These existing paths remain unchanged; the results above do not imply
completion of every runtime scenario in the original acceptance matrix.
