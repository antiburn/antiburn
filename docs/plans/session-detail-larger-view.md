# Session Detail: wide layout, plus a resizable dev window to see it in

**Branch:** `claude/session-details-larger-view-12b08a` (worktree `session-details-larger-view-12b08a`, reset onto `origin/main` at `88ec278a` on 2026-09-08) · **Status:** PR 1 built 2026-09-08, awaiting review; PR 2 not started

## Status

| Step | State | Notes |
| --- | --- | --- |
| 0. Plan | approved | Keith approved PR 1 in chat on 2026-09-08 after the pins review closed with no comments |
| 1. Dev session window (PR 1) | built, awaiting review | Debug-only tray item opens a resizable window: list on the left, detail on the right. The detail still renders the popover layout until PR 2 adds `layout="wide"` |
| 2. Wide layout for Session Detail (PR 2, stacked on PR 1) | not started | The ten pins below, gated by a `layout="wide"` prop |
| 3. Verify in the dev window, screenshots for both PRs | not started | Keith uploads the images |

## What this is

Two things, shipped as two stacked PRs:

1. **A dev window.** A debug-build-only tray item, **Session Window…**, that opens a normal, resizable, decorated window with the session list down the left and the selected session's detail filling the rest. It stays open while the app runs and reopens from the tray. It exists so the wide layout can be seen and tuned at any size without waiting for the upcoming larger view. It is compiled out of release builds.
2. **A wide layout for `SessionDetailPresentation`.** The three tabs (Context, Cost, Tools) get a second layout that suits a pane 700px and wider. The popover layout does not change at all.

The upcoming larger view (the three-column window in the screenshots, being built elsewhere) adopts the wide layout by passing one prop. Nothing in this plan depends on that window's code.

## How the wide layout is selected

**A `layout` prop, not a container query.** `SessionDetailPresentation` gains `layout?: "popover" | "wide"` (default `"popover"`). `SessionPane` passes it through. The dev window passes `"wide"`; the popover passes nothing.

Why a prop:

- It is an explicit contract the upcoming host can read in one line.
- Half the pins change markup (the key on one line, the composition rows as cells, two-column tools), not just classes. Markup switches need a value in React, and a width observer is the kind of external-system sync `AGENTS.md` asks us to avoid unless nothing simpler works.
- It is testable in jsdom. Container queries never evaluate there.

The cost is that a wide pane that is dragged narrow does not fall back to the popover layout. The dev window gets a minimum width (760px) so that never happens there, and the upcoming host is expected to have one too.

## The ten pins, and what each becomes

All of these apply to `layout="wide"` only. Pin numbers are per screenshot.

### Tools tab (screenshot 1)

| Pin | Keith | Change |
| --- | --- | --- |
| 1 | Make this break into two columns when in this larger view | `SkillsMcpChart` takes a `columns` prop (1 or 2). Wide renders a `grid-cols-2` grid with the same row cells. Rank order stays token-descending, left to right then down. The "9.4k tokens burned" headline stays full width above the grid. |
| 2 | This shouldn't be full width, right?? | The Context / Cost / Tools `SegmentedControl` stops stretching. `SegmentedControl` gets `equalWidth` honoured for `native-tabs` (today that variant is always a full-width grid). Wide renders it as `inline-grid` with content-sized columns and a small minimum per segment, left-aligned under the title. |

### Cost tab (screenshot 2)

| Pin | Keith | Change |
| --- | --- | --- |
| 1 | Some healthy margins on the side would be nice | Side padding goes from `px-4` (16px) to `px-8` (32px) on the header block, the tab bar, and every tab panel, so the left edges of repo, title, tabs, and rows stay on one line. Long-form text (the Efficiency paragraph) gets `max-w-prose`. |
| 2 | Don't line break these. Make more subtle colour/weight. | The cost-section sentences under the $/MTok scale (`METRIC_SUMMARY` + `efficiencyThresholdGuidance` + `METRIC_GUIDANCE`) render as one running paragraph, joined with spaces, in `text-label-tertiary` at regular weight. Today each sentence is its own line in `text-label-secondary`. This one applies to both layouts: the popover wraps the same paragraph at its own width. |

### Context tab (screenshot 3)

| Pin | Keith | Change |
| --- | --- | --- |
| 1 | Key could be tighter. Single line? | `ChartKey` gets a `layout` prop. Wide renders the five stats as one `flex` row of content-sized cells with `gap-x-6`, left-aligned. Hover, pin, and tooltips unchanged. Popover keeps the 3-column grid. |
| 2 | Chart feels a little too tall. Bit more padding would be good | Wide caps the plot at `max-h-[360px]` (a local geometry value, documented in `design.md` as a session-detail value) instead of filling the panel, and adds `pt-6` above the plot and `gap-y-6` between plot, key, and composition. Popover keeps fill-the-panel. |
| 3 | None of these horizontal rules | Wide drops every `border-b` / `border-t border-separator` in the detail: under the title row, under the tab bar, above the composition block, and the three section rules in the Cost tab. Section spacing goes to `gap-y-6` and the small-caps `TabSectionHeading` carries the boundary. |
| 4 | Remove the hr | Same rule as pin 3, the one above Real Work / Rewrite / Carry. |
| 5 | This bar could be taller. Think desktop app | `CompositionTrack` gets a `height` prop (`"hairline"` today, `"bar"` for wide). Wide renders `h-2.5` (10px) with `rounded-control` ends. Exact height tuned in the dev window against the chart's stroke. |
| 6 | Key could probably fit on one line? | `EfficiencyBreakdown` composition section gets the same `layout` prop. Wide renders the three share rows as one `flex` row of cells in the same shape as the chart key (swatch, value, caption, band word), each keeping its tooltip. Popover keeps the stacked rows. |
| 7 | Remove this surplus line (the title row at the top of the detail) | Wide drops the title row entirely: no "Session Detail" / session title text and no back chevron, since the hero already carries the title and the host owns navigation. `onBack` becomes optional. The reveal and delete actions move to the right end of the hero's title line. Popover keeps the row. |
| 8 | Remove hr (under the title row) | Goes with pin 7. With the row gone, the wide detail has no rule from the top of the pane down. |
| 9 | These (reveal, delete) could be on the same line as the session title? | Same as pin 7: the two icons sit at the right end of the hero's title line, centred on its first line, in the tertiary ink they use now. The title keeps its 2-line clamp and leaves room for them. |

The pins were placed on 2026-09-08 and are recorded here so the review does not have to be replayed. Two pins (Cost 2, Context 7) also touch the popover: the efficiency paragraph and the optional `onBack`; neither changes what the popover shows.

## The dev window

### Shell (Rust, debug builds only)

- New module `apps/desktop/src-tauri/src/session_window.rs`, `#[cfg(debug_assertions)]`. Label `session-window`. Builds `index.html#/session-window` as a standard decorated window: title "antiburn Session (dev)", `inner_size(1180, 820)`, `min_inner_size(760, 600)`, resizable and maximizable, `visible(true)`. No readiness dance, no non-activating panel, no popover material. A second open refocuses the existing window.
- Tray: `MENU_SESSION_WINDOW` / "Session Window…" added to the debug items in `tray.rs`, next to Reset Onboarding.
- `capabilities/default.json` lists `session-window` so it can call the same commands the popover does.
- `commands::window_ready` keeps ignoring the new label; the window is visible from the start.

### Frontend

- `route.ts`: `"session-window"` joins `ShellRoute`; `App.tsx` lazy-loads `views/SessionWindowView.tsx` for it.
- `SessionWindowView`: two columns in `bg-surface-window`. Left is `PopoverView` at `w-[380px]` with a right border. Right is `SessionPane layout="wide"` for the session on top of the stack, or a quiet "Select a session" placeholder.
- `PopoverView` gains two optional props: `session` (an injected `PopoverSession`, default creates its own) and `detail: "inline" | "aside"` (default `"inline"`). With `"aside"`, `body()` always renders the activity list, and the session-pane derivation (`sessionPayload`, `sessionLoading`, `sessionError`, `sessionRefreshing`, `displaySubject`, neighbours) moves into a pure `sessionPaneState(state)` helper in `PopoverSession.ts` so `SessionWindowView` derives the same props without copying them.
- `PopoverSession` gains a constructor option `shell: "popover" | "window"`. In `"window"` mode `syncHeight` skips `setPopoverHeight` and Escape does not call `hidePopover`; both act on the real popover window today, which is the wrong window here. Everything else (loading, refresh, delete, open sub-agent) is shared.
- The selected row in the left list is not highlighted in v1. A follow-up if it matters.

### What the dev window does not do

It is not the upcoming larger view and does not try to look like it. It has no sidebar, no host title bar, and no polish beyond what the wide layout needs to be judged.

## PR split

1. **PR 1: dev session window.** `session_window.rs`, tray item, capability, route, `SessionWindowView`, the `PopoverView` / `PopoverSession` props. Roughly 350 lines. Debug-only, so it ships dark. Screenshot: the window open with a session selected.
2. **PR 2: wide layout for Session Detail** (based on PR 1). The `layout` prop through `SessionPane` → `SessionDetailPresentation` → `ChartKey` / `EfficiencyBreakdown` / `SkillsMcpChart` / `SegmentedControl`, the paragraph change, `design.md` note for the 360px chart cap. Roughly 400 lines with tests. Screenshot: each of the three tabs in the dev window.

## Verification

- `pnpm --filter @antiburn/desktop lint`, `type-check`, `test`; `scripts/check-design-drift.mjs` after the `design.md` edit.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` in `src-tauri`.
- `pnpm --filter @antiburn/desktop dev`, open the tray's Session Window…, walk the three tabs at 760px, 1180px, and full screen; confirm the popover is pixel-identical to main.
- Tests added: `SessionDetailPresentation.test.tsx` renders wide and asserts no separators, one-row key, `columns=2` on the tools grid; `EfficiencyBreakdown` asserts the single paragraph; `route.test.ts` for the new fragment; `PopoverSession` window-mode skips the popover IPC.
