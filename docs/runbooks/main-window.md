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
is 900×600 logical pixels; the normal minimum is 800×560. The initial outer frame,
including native chrome, occupies at most 85% of each usable display dimension.
Small work areas may reduce the effective minimum. A valid saved user size can
exceed this initial cap. Do not reset a user's placement to test the default.

Check a Retina display, a non-Retina display, and movement between displays with
different scale factors. On a 2× display, the uncapped default content size is
1800×1200 physical pixels and still appears as 900×600 logical pixels. Test both
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
| Existing settings, HUD, and notifications    | Each supported surface opens and behaves as before                                   |

Windows/Linux login registration now adds `--background`. Existing registrations
are rewritten when the updated app reconciles the enabled preference. An old
registration that launches the new binary before reconciliation still has no
argument, so that first launch opens the main window. The old command is
indistinguishable from a manual launch; the app does not guess from login
preferences or elapsed boot time. Launch the updated app once before testing
quiet login on an upgraded installation. macOS uses the native login event.

## Navigation shell

Check the 900×600 default and 800×560 minimum with light and dark themes.

- The sidebar remains visible throughout resizing; no compact navigation mode appears.
- Activity is the only selected main section; no Settings or Quit sidebar action appears.
- The existing Settings window remains accessible through the tray and native menus.
- Check readable 28px rows and independent vertical content scrolling without horizontal overflow.
- Restore an older saved 560×420 window; it expands to at least 800×560 when the display allows.
- Close and reopen the main window; the selected section persists.
- The first macOS row starts below the 40px drag strip. Buttons must not drag the window.
- The title strip still drags and toggles maximize on double-click.

## Opening measurements

Measure three separate paths:

1. **Cold launch:** quit the app, then explicitly launch it with onboarding done.
2. **First open:** launch in the background, then select **Open antiburn** once.
3. **Warm reopen:** close the main window, then select **Open antiburn** at least
   30 times. Wait for each reveal before closing it again.

The shell writes `main_window_revealed` JSON events containing `open_kind`,
`elapsed_ms`, and `window: main`. Summarize one benchmark interval with:

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

Record the generated frontend entry and imported chunk sizes from the build.
The navigation-only main entry must not load session analysis or chart code.

## Hidden resources

Record process-tree memory and idle CPU before opening, after first open,
after hiding, and after 30 hide/show cycles. Include associated webview
processes; the shell process alone misses renderer cost. Use the same settling
interval and sample count in each state. Use five samples after 10 seconds of
settling as the initial protocol, with one second between samples.

Confirm one resident main renderer, no growing renderer count, no accumulating
listeners or timers, and no additional scan or provider polling loop. Hidden
CPU should return to its background baseline. Retained memory is expected;
monotonic growth across repeated cycles needs investigation.

The [popover memory report](memory-reporting.md) measures a different surface.
Do not describe its shell-plus-popover measurements as a main-window memory
measurement.

## Evidence to attach to a PR

Include platform smoke results, native timing summaries, observed frame/input
timings, bundle sizes, and resource samples. Mark an unavailable platform or
measurement as unverified. Unit tests and a successful bundle build do not
establish native app-switching behavior or the latency target.
