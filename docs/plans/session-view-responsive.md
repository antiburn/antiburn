# Session view: use wide windows, two tabs

_Plan. Branch `feat/session-view-responsive`. 2026-09-07._

Two asks: let the session detail view use a larger window instead of the 380px
popover column, and cut its tabs from three to two. A playground settles the
layout questions first; the build follows the playground's settings block.

## Status

| # | Step | Where | Status |
|---|---|---|---|
| 0 | Playground at 380 → 1280 wide, tab schemes, wide layouts | scratchpad `proto/session-view-responsive/playground.html` | v1 built |
| 1 | Decisions A and B | this doc | decided 2026-09-07, see below |
| 2 | Two tabs: merge panels, default tab, tests | `SessionDetailPresentation.tsx`, tests | done (21f3fb9b) |
| 3 | Container-query layout for the detail view, tab bar kept at every width | `SessionDetailPresentation.tsx`, `--container-session-wide` in `tokens.css`, `design.md` sizes | done (21f3fb9b): Tailwind `@session-wide:` variants, no new stylesheet |
| 4 | Test wide in a plain window (sandbox in the browser), no Rust change | | done: verified at 380 and 1200 with the sandbox branch applied locally |
| 5 | Drift check, tests, screenshots, PR (Keith uploads the image) | | checks pass; screenshots captured; PR waits for Keith |

## Where things are today

- The view is `apps/desktop/src/components/session/SessionDetailPresentation.tsx`.
  It renders header, summary, a native-tabs segmented control, and one panel.
- Tabs are **Context** (chart, key chips, composition), **Cost** (checks, cost
  rows, efficiency), **Tools** ("N tokens burned" headline plus the skills/MCP
  table). Defined at lines 240-247, rendered at 803-915.
- The popover window is fixed at 380 logical px wide and `.resizable(false)`
  (`popover.rs:120`, `:661`). Height is the only degree of freedom, animated in
  Rust, driven by `lib/popoverHeight.ts`.
- There are **no** size breakpoints or container queries anywhere in
  `apps/desktop/src`. This is the first size-responsive layout in the app.

## Decision A: where does the wide view live?

The layout can be responsive on its own (step 3). Something still has to give
it a wide box. Three ways:

| Option | What changes | Cost | Feel |
|---|---|---|---|
| A1. Popover grows for the session view | `WIDTH` becomes per-view; `set_size` animates width too; `compute_position` re-clamps to the monitor | small Rust change, `popoverHeight.ts` gains width | a 900px menu-bar popover is unusual but fast to reach |
| A2. Separate resizable session window | new window module like `settings.rs` (960×680 precedent); "Open in window" affordance in the popover header; same React component behind a `#/session/<id>` route | medium: window, route, IPC for the session id | standard macOS window, remembers size, can sit beside the terminal |
| A3. Popover becomes resizable by drag | `.resizable(true)` and drop the height animation on user resize | small but fights the popover's own height logic | menu-bar popovers do not resize by drag on macOS |

**Decided: out of scope here** (Keith, 2026-09-07). The session view will get
its own window, and Marty is building that. This branch makes the view
responsive and tests it wide in a plain window. No `popover.rs` change.

## Decision B: which two tabs?

The playground has these schemes as a control. The composition and efficiency
rows are the swing content.

| Scheme | Tab 1 | Tab 2 | Why |
|---|---|---|---|
| B1 | Context: chart, key, composition | Cost: checks, cost, efficiency, tools burned | Tools is a cost finding, so it joins the money story |
| B2 | Context: chart, key | Breakdown: composition, checks, cost, efficiency, tools | the chart stands alone, everything tabular in one scroll |
| B3 | Overview: chart, key, checks | Detail: composition, cost, efficiency, tools | the verdict up front, the numbers behind |

**Decided: B1, tab bar kept at every width** (Keith, 2026-09-07: "combine
cost and tools"; "view is to be with a tab bar Context: Cost"). The wide
layout applies inside each tab panel; the tabs never disappear.

## Decision C: the wide layout

Playground knobs, each a CSS variable or data attribute on the stage:

- `width` 380 → 1280
- `layout`: stretch (one column) · two columns (chart left, breakdown right) · hero row plus columns
- `breakpoint` 480 → 960: the container width where the wide layout starts
- `max-content` 0 → 1280: clamp the content width inside a very wide window
- `chart-height` 160 → 480
- `tabs`: three (today) · B1 · B2 · B3, plus `wide-tabs` on/off

## Build notes

- Tailwind v4 ships container queries. Put `@container` on the detail root and
  use `@md:` / `@lg:` variants, or named sizes in a small
  `session-detail-layout.css` added to `design.md` `sources:`. No `useEffect`
  and no JS width measurement: the layout is CSS only.
- Two tabs touches `DETAIL_TABS`, the `SessionDetailTab` union, the three
  `tab ===` blocks, the default `useState`, and the tests in
  `SessionDetailPresentation.test.tsx` and `SessionDetailChartKey.test.tsx`.
- `scripts/check-design-drift.mjs` reads Rust constants; run it after any
  window geometry change.
- Every commit signed (`git commit -s`). PR needs a screenshot of the wide
  window, which Keith uploads.

## Non-goals

- Changing the session list.
- Reworking the chart internals; it already fills its container.
- Windows or Linux window chrome specifics beyond what Settings already does.
