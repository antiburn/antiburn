# Interface scale visual QA

## Run the browser checks

Run these commands from the repository root after the desktop build compiles.
Install Chromium once, or after a Playwright version change:

```sh
pnpm --dir apps/desktop exec playwright install chromium
pnpm --dir apps/desktop node scripts/test-interface-scale.mjs
```

The runner checks the fixture's TypeScript before starting Playwright. Append
`--grep "interface-scale smoke"` for the short interaction pass. Use
`--grep "issue 507 targeted journeys"` for Settings and onboarding, or
`--grep "interface-scale error recovery"` for asserted error presentations.

The populated matrix covers eight surfaces at 90, 100, 110, 125, 150, 175, and
200 percent, in light and dark, with normal and constrained layouts. Additional
input-state variants run at constrained 200% in dark. These variants are not a
full state-by-scale-by-theme matrix.

Artifacts are under `apps/desktop/test-results/visual`; the HTML report is under
`apps/desktop/playwright-report/visual`. Later runs can replace these files.

## Fixture scope and geometry

The fixture is served from `tests/visual/` and imports the actual React views.
Its Tauri modules are Vite aliases to deterministic test-only command/event
shims. No production fixture hook is present. The shims supply synthetic data
with fixed timestamps; Playwright fixes the browser clock and color scheme.
Query parameters select the fixture inputs:

```text
/tests/visual/?surface=popover&state=long&scale=200&theme=dark
```

Surfaces: `main`, `settings`, `onboarding`, `popover`, `preview`, `hud`,
`hud-detail`, and `nudge`. Input states: `populated`, `empty`, `loading`, `error`,
and `long`. A state name does not prove that the corresponding UI is visible.
For example, onboarding's generic error input can still show Welcome, and
Settings can show defaults while a read is pending. Do not count these captures
as verified error or loading presentations.

Dedicated fault tests assert actual session-analysis, interface-size save,
onboarding bootstrap, and preview-data errors. They exercise recovery where the
product supplies a retry. The preview supplies a pointer-away instruction, not
a retry button. The fixture does not invent unavailable UI states.

For the populated matrix, each context uses a CSS viewport equal to the target
window size divided by the scale factor, rounded to whole pixels, and a matching
device scale factor. The target sizes are 1280 × 860 and 640 × 480. Targeted 200%
journeys use a 320 × 240 CSS viewport with device scale factor 2. The example URL
alone does not configure this geometry or emulate native zoom.

The harness does not use CSS `zoom`, override `window.innerWidth`, or change
viewport metrics during capture. It checks layout in Chromium, not native
WebView page zoom, rasterisation, materials, device-DPI conversion, persistence,
or window-size negotiation. The canvas around an overlay is not its native
window frame.

## Visual and interaction review

Review the 200% captures in both themes and layouts before release. Check text
overlap, unintended clipping, wrapping, focus-ring appearance, and visible
control bounds. Confirm that intentional truncation remains contained. Inspect
content below the viewport by scrolling; a viewport capture does not cover it.

Browser assertions cover root horizontal overflow and interactive-control
horizontal bounds. They do not prove vertical reachability, pointer hit testing,
or keyboard operation. Exercise those separately: use Tab and activation keys,
open and dismiss navigation, scroll to lower controls, change and reset the
scale, and return from detail with Back. Check focus restoration, including a
repeated external request for the same session.

The targeted journeys cover the seven current Settings panes: General, Privacy,
Notifications, Usage, Sources, Appearance, and About. They also cover all four
onboarding steps: Welcome, Agents, Repos, and Ready. At 200% in both themes,
these journeys generate 22 pane/step captures. Update this inventory when the
navigation changes; historical capture counts are not current coverage.

## Native desktop release checks

### HUD v2 and notch island

Use the current HUD, including its token map, not captures of the earlier
usage-only HUD. Repeat floating, edge-docked, notch-preview, collapsed-island,
and expanded-island checks at 90%, 100%, and 200%, in both themes.

- The camera exclusion, header height, and fillets stay fixed in native logical
  points. Both wings widen with interface scale without shifting the camera gap;
  indicators grow uniformly but fit inside the fixed height. Check hover near the
  outer ends of the enlarged wings. Collapsed wings remain compact; expanded
  wings meet the body's side edges, inside the fixed fillet gutters. Check that
  the camera gap does not shift when the panel is clamped against a display edge.
  Body padding stays based on the compact wings. The expanded body scales below it, widens within the notch
  display's available bounds, and scrolls when its content exceeds the space.
  Check the last usage row and token-map target, not only the first viewport.
- Dock at each edge; hover to peek, leave to park, wake, and drag free. Change
  scale during each transition. The visible tab stays six native logical points,
  the wake hold is preserved, and no old frame restores an obsolete position.
- Change scale during a drag. The HUD keeps its applied scale until settlement,
  then takes the latest requested scale without jumping back to the previous
  saved position. Other windows may scale immediately.
- Test Hide during startup/reopen and during a drag; reopen afterward. A queued
  restore or size measurement must not resurrect a hidden HUD or leave drag
  handling stuck. Repeat quit/relaunch with HUD off, floating, docked, and islanded.
- Test on the actual notch display when available. The debug fake-notch control
  is useful on an external display, but record it as simulated—not evidence of
  physical camera alignment. Move/disconnect/reconnect displays separately.

The native HUD owns its applied zoom and revisioned geometry. Browser fixtures
convert hardware points to CSS pixels by dividing by that applied scale; they
must not add CSS zoom. Native hit testing, panel activation, display conversion,
and OS accessibility preference handling still require native checks.

### Shared surfaces and platform gates

Use an isolated test profile. Repeat these checks after relevant native changes,
at 90% and 200%, in light and dark where appearance matters:

- Exercise the scale picker, reset, native View menu, and platform shortcuts
  from main, Settings, and popover. Check all presets, newly opened surfaces,
  and persistence after quitting and relaunching the app.
- Check all Settings panes and onboarding steps. Change scale while onboarding
  is open; verify that both the window and content adapt, navigation stays
  reachable, and long lists scroll. Confirm that main-window resizing remains
  user-controlled.
- Drag Settings smaller at each scale. Verify that its scale-aware minimum keeps
  sidebar navigation visible when the display has room. Check the smaller-display
  fallback, then move back to a larger display and verify that the minimum updates.
- Check popover scrolling, pinning, and session-to-main navigation. Check
  provider-preview hit testing and placement, HUD/detail bounds, and nudge
  hover, expansion, and dismissal. Change scale during HUD animation. Inspect
  material corners, transparency, and titlebar/traffic-light clearance.
- With HUD detail visible, repeatedly alternate scale shortcuts while entering
  and leaving the HUD during its resize animation. Also hide and reopen the HUD.
  Verify that Settings and main-window controls remain responsive throughout.
  A successful static capture does not establish freedom from native lock contention.
- Move windows between displays with different backing factors at both endpoint
  presets. Check retained scale, work-area fit, pointer alignment, and overlay
  anchoring. Record the backing factors and geometry when validating numerical
  conversion; browser device-scale emulation is not physical display evidence.
- With permission, test Reduce motion and Reduce transparency. Check focus,
  animation interruption, readability, and overlay materials. Record and restore
  the original system preferences.
- Run native Windows/WebView2 and Linux/WebKitGTK checks as well as macOS checks.
  A Chromium pass does not certify these native implementations.

Record the tested revision, platform, scale, theme, surface, and interaction.
Separate automated passes, generated captures, manual inspection, and remaining
checks. Do not count an overlay action as a visual pass if the capture shows a
different window. Record console warnings separately from layout/focus results.

Keep review reports and captures outside committed product documentation.
This runbook defines the procedure; it is not a record of a completed release
validation pass.
