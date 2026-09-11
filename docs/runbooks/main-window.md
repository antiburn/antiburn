# Main-window validation

Use this runbook for changes to the main window and for each subsequent view
migration. The first main-window change contains no product views. Preserve
this baseline as real content is added.

## Build and isolate

Build with `pnpm --filter @antiburn/desktop dev:bundle` for native smoke tests.
Use a separate debug profile as described in [debugging](../debugging.md).
For performance measurements, use an optimized packaged build with the debug
bundle identifier and without the `distribution` feature:

```bash
pnpm --filter @antiburn/desktop tauri build --config src-tauri/tauri.debug.conf.json
```

Record the commit, OS build, CPU, RAM, display scale and usable work area, build
profile, and fixture size. Never compare debug timings with optimized timings.
Use synthetic sessions and retain reports outside the repository. Do not
combine logs from different machines, builds, or benchmark intervals.

## Native smoke matrix

Test a fresh profile separately from saved placement. The default content size
is 1100×600 logical pixels; the normal minimum is 1000×560. The initial outer frame,
including native chrome, occupies at most 85% of each usable display dimension.
Small work areas may reduce the effective minimum. A valid saved user size can
exceed this initial cap. Do not reset a user's placement to test the default.

Check a Retina display, a non-Retina display, and movement between displays with
different scale factors. On a 2× display, the uncapped default content size is
2200×1200 physical pixels and still appears as 1100×600 logical pixels. Test both
fresh placement on the secondary display and restoration after relaunch.

For macOS minimize/restore tests, confirm that the main window leaves the window
server's on-screen list before activation and returns afterward. Match the app
PID and the main window's layer and bounds. System Events can retain a stale
`AXMinimized` value after programmatic restoration, and an accessibility window
list can include minimized windows. Neither alone establishes on-screen state.

Run the following on macOS, Windows, and Linux. On Linux record the desktop
environment and X11/XWayland/Wayland backend; compositor policy can control
position and focus. Test an installed build for taskbar icon grouping.

| Scenario                                     | Expected result                                                                      |
| -------------------------------------------- | ------------------------------------------------------------------------------------ |
| Explicit launch after onboarding             | One main window opens with an opaque themed canvas                                   |
| First launch                                 | Onboarding appears; completing it opens the main window                              |
| Background/login launch                      | Monitoring and tray start without creating the main webview or taking focus          |
| Launch again                                 | The existing process opens or restores its main window; no second scan scheduler     |
| Close then Open antiburn                     | The same renderer reappears; navigation state will remain when views exist           |
| macOS close, switch away, Command-Tab back   | The retained main window reappears; closing alone never immediately reopens it       |
| macOS close, then open Settings or popover   | The requested surface appears without reopening the main window                      |
| Close during initial load                    | Readiness does not unexpectedly reopen the closed window                             |
| Visible warm open                            | Native focus is immediate and emits no health check                                  |
| Hidden warm open                             | One generation/request check acknowledges before reveal                              |
| Close during forced recovery, then reopen    | The watched replacement completes or reaches the terminal dialog                     |
| Hidden recovery exhausts its budget          | No dialog appears until the next open                                                 |
| Dead or absent main WebContent               | The native terminal dialog remains usable                                             |
| Terminal Try Again                           | One fresh watched generation starts; no label overlap occurs                          |
| Terminal Dismiss, then open                  | The terminal dialog returns with a fresh token                                        |
| Delayed or duplicate destruction             | Terminal state and its failure budget remain intact                                   |
| Fallback Reload                              | The fallback's generation starts one watched replacement                             |
| Commit-phase descendant failure              | Fallback reports without an earlier healthy status                                    |
| Targeted open during replacement             | The replacement peeks, applies, and acknowledges the requested session                |
| Navigate after target, then recover           | Recovery does not replay the retired target                                           |
| Minimize then open                           | The window restores and becomes usable                                               |
| macOS minimize, switch away, activate        | The main window restores through native unminimize; minimizing alone stays minimized |
| Switch applications                          | Normal Dock/taskbar switching works; the main window does not hide on blur           |
| Resize, maximize, tile                       | Native controls and OS window management work                                        |
| Reopen after monitor removal or scale change | Usable bounds are restored with title controls reachable                             |
| macOS chrome                                 | Traffic lights work; the empty title strip drags; native title text stays hidden     |
| macOS title-strip double-click               | First double-click expands the window; the second restores its previous size         |
| Windows/Linux chrome                         | Native title bar remains; no duplicate webview drag strip                            |
| Light/dark and accessibility preferences     | No white startup flash; readable opaque surface; no opening animation                |
| Explicit Quit from each menu                 | Process and monitoring stop, including with no visible windows                       |
| Existing tray primary/secondary clicks       | Popover toggle and existing menu actions retain their behavior                       |
| Select a popover session                     | Main window opens or focuses on that exact native or WSL session                     |
| Select while the main window is loading      | The requested session appears after startup without briefly replacing it             |
| Select while the popover is pinned           | Main window focuses; the pinned popover remains on its activity list                 |
| Select while the popover is unpinned         | Main window focuses; normal focus-loss policy dismisses the popover                  |
| Existing settings, HUD, and notifications    | Each supported surface opens and behaves as before                                   |

Windows/Linux login registration now adds `--background`. Existing registrations
are rewritten when the updated app reconciles the enabled preference. An old
registration that launches the new binary before reconciliation still has no
argument, so that first launch opens the main window. The old command is
indistinguishable from a manual launch; the app does not guess from login
preferences or elapsed boot time. Launch the updated app once before testing
quiet login on an upgraded installation. macOS uses the native login event.

## Navigation shell

Check the 1100×600 default and 1000×560 minimum with light and dark themes.

- The sidebar remains visible throughout resizing; no compact navigation mode appears.
- Sessions is the main section. Settings appears at the bottom; no Quit action appears.
- The Settings sidebar action and Command+, (Control+, on Windows/Linux) open the existing Settings window.
- Check readable 28px rows and independent vertical content scrolling without horizontal overflow.
- Restore an older saved 560×420 window; it expands to at least 1000×560 when the display allows.
- Close and reopen the main window; the selected section persists.
- The first macOS sidebar row starts below the 40px drag strip. Session panes have no top gap. Buttons must not drag the window.
- The sidebar drag strip, session list header, detail toolbar, and empty-detail top area drag and toggle maximize on double-click on macOS.

### Session selection appearance

- In both themes, the selected card is softly distinct from resting and hovered cards.
- Hover the selected card and open its tooltip; its selected fill remains stable.
- Move selection with the keyboard; the focus indicator remains visible.
- Text, badges, and icons keep their normal contrast.
- Verify the menu-bar session list retains its existing appearance and navigation.

## Opening measurements

Measure three separate paths:

1. **Cold launch:** quit the app, then explicitly launch it with onboarding done.
2. **First open:** launch in the background, then select **Open antiburn** once.
3. **Warm reopen:** close the main window, then select **Open antiburn** at least
   30 times. Wait for each reveal before closing it again.

The shell writes `main_window_revealed` JSON events containing `open_kind`,
`elapsed_ms`, and `window: main`. A hidden warm sample includes its health
handshake; a visible warm sample does not run one. Summarize one benchmark
interval with:

```bash
node scripts/window-open-report.mjs /absolute/path/to/benchmark.jsonl
```

The report uses nearest-rank p95 and keeps cold, first, and warm samples apart.
Its warm native target is p95 ≤100ms with at least 30 samples. Fewer samples are
inconclusive. The script returns an analysis report; `status: missed` is a
target miss, even though the report command itself completed successfully.

These events measure the request to native reveal completion, not process
launch to first frame. Measure process-launch latency separately with an
external timer. Use screen recording or a platform presentation trace to
measure request-to-visible and verify that controls accept input. Record these
values beside the native timing report; do not equate `show()` completion with
pixels on screen or successful focus activation.

Also measure `main_window_health_check_started` to `main_window_health_ack`
for at least 30 hidden warm opens. Include long-hidden (at least ten minutes),
minimized, and busy-renderer cases. Record median, nearest-rank p95 and p99,
and sample count. The 300 ms timeout is provisional. Raise it if measured p99
exceeds approximately one third of the timeout. Sanity-check the provisional
10-second recovery watchdog against isolated recovery load times. Escalate a
timeout disagreement for review rather than tuning from a single observation.

Keep three separate labels for native reveal, health acknowledgement, and
visible content. Neither acknowledgement nor reveal establishes painted pixels.
The earlier 148 ms first-open and 11 ms warm-open observations have one sample
each. They are not distributions and must not be used as before/after evidence.

Record the generated frontend entry and imported chunk sizes from the build.
The navigation-only main entry must not load session analysis or chart code.

## Recovery and terminal-dialog drill

Use only an isolated debug profile and an app instance whose processes you can
attribute. Never stop another antiburn instance or an unrelated WebContent
process. Never modify a user's database.

1. Confirm a normal hidden check acknowledges and reveals.
2. Hang or stop only the isolated main renderer. Open the hidden window and
   confirm timeout, bounded replacement, and eventual reveal.
3. Force a committed fallback. Confirm the fallback reveals independently and
   **Reload** starts recovery for only its generation.
4. Force three consecutive failures. Confirm the native dialog appears even
   when the webview is absent or hung.
5. Choose **Dismiss**, open again, and confirm a new dialog appears.
6. Choose **Try Again** and confirm a fresh watched generation either becomes
   ready or returns to terminal. Confirm no duplicate `main` label appears.
7. Close during forced recovery. Confirm no surprise dialog while closed and
   confirm reopen still completes or presents the deferred dialog.
8. Delay a destruction callback beyond terminal entry. Confirm it records a
   free label without clearing the dialog gate or budget.
9. Request a session through destroy failure, direct build failure, and deferred
   build failure. Confirm each replacement peeks and acknowledges it. Include
   an acknowledgement that arrives while its generation still loads.
10. Force a descendant callback-ref failure during commit. Confirm fallback is
    reported without an earlier healthy status.
11. After a successful target, navigate elsewhere and recover. Confirm the old
    target does not replay.

A stale dialog callback must not clear a newer dialog, reset its ledger, retry,
or touch a window. Unit tests establish token validation. Mark native stale
callback injection as unverified if the isolated fixture cannot produce it.

## Hidden resources

Record process-tree memory and idle CPU before opening, after first open,
after hiding, and after 30 hide/show cycles. Include associated webview
processes; the shell process alone misses renderer cost. Use the same settling
interval and sample count in each state. Use five samples after 10 seconds of
settling as the initial protocol, with one second between samples.

Confirm one resident main renderer, no growing renderer count, no accumulating
listeners or timers, and no additional scan or provider polling loop. Include
30 hide/show cycles, two forced recoveries, and one terminal Try Again. Record
process count and memory before and after the sequence. Hidden CPU should return
to its background baseline. Retained memory is expected; monotonic growth across
repeated cycles needs investigation. Attribute app-owned WebContent before
publishing combined memory. WebKit helpers can have PPID 1, so shell ancestry
alone is insufficient. Until attribution is validated, label shell RSS as
shell-only.

The [popover memory report](memory-reporting.md) measures a different surface.
Do not describe its shell-plus-popover measurements as a main-window memory
measurement.

## Evidence to attach to a PR

Include platform smoke results, native timing summaries, observed frame/input
timings, bundle sizes, and resource samples. Mark an unavailable platform or
measurement as unverified. Unit tests and a successful bundle build do not
establish native app-switching behavior or the latency target.
