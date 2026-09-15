# Overview view — implementation plan

**Date:** 2026-09-14
**Status:** implemented; Keith requested publication and assignment to Zack on 2026-09-14.
**Branch:** `claude/home-screen-planning-0d134f`, reset onto `origin/main` (`0456d51e`)
**Name:** the design handoff calls this view Home. Keith renamed it **Overview** on
2026-09-14 (review thread u-5). Code, files, and the sidebar label use Overview; the
handoff below is kept as the design record under its original name.
**Design source:** [`home-view-design-handoff.md`](home-view-design-handoff.md) (copied
from the `codex/home-view-design-handoff` worktree, where it is still untracked)

This plan turns the agreed design handoff into stacked, reviewable changes. The
handoff owns the _what_; this document owns the _how_ and the _order_. Where the
code differs from what the handoff assumed, the difference is called out.

## What the code says today

Checked against `origin/main` on 2026-09-14.

| Handoff assumption                                    | Reality                                                                                                                                                                                   | Effect on the plan                                                                                                             |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| Overview section id is a TS change in `ipc.ts`        | `MainWindowSectionId` is mirrored by the Rust `MainWindowSection` enum in `src-tauri/src/main_window.rs:39` (serde camelCase)                                                             | Add `Overview` on both sides in slice 1                                                                                        |
| `getProviderUsage()` is callable from the main window | `src-tauri/capabilities/main.json` grants `allow-get-live-usage` but **not** `allow-get-provider-usage`; only the popover capability set has it                                           | Add the capability entry in slice 2, plus one for the new daily command                                                        |
| Payload types live in `ipc.ts`                        | They live in `src/lib/providerUsageIpc.ts` (`ProviderUsageSummaryPayload`, `LiveProviderUsagePayload`, …); `ipc.ts` re-exports and owns the `invoke` wrappers                             | Types go in `providerUsageIpc.ts`                                                                                              |
| Daily buckets need a new series                       | Confirmed: `provider_usage::summarize` only fills `today / week / month_to_date / last_30_days` buckets from `updated_at_epoch` (`provider_usage/mod.rs:483`)                             | New `days` series in slice 2                                                                                                   |
| Recent sessions can come from `MainActivitySession`   | Its list only loads once a viewer subscribes _actively_ (`subscribeInactive` never starts the load). Subscribing actively from Overview would also start its selection/analysis machinery | Overview loads its own three rows with `listRecentSessions()` (same command the popover uses)                                  |
| Session rows are reusable                             | `SessionRow` is a private function inside `SessionList.tsx:305`; `SessionList` itself is virtualized with grouping and a toolbar                                                          | Export the row from `SessionList.tsx`; do not mount the full list for three rows                                               |
| Burn findings can open "with the check selected"      | `BurnChecksReport` has no external selection API; each `CheckRow` owns its own `open` state (`BurnChecksReport.tsx:67`)                                                                   | v1 navigates to the Burn checks section; a `focusDetector` request on `BurnChecksSession` is a small follow-up (see Decisions) |
| Burn checks summary can share `BurnChecksSession`     | The session only becomes `active` with an active subscriber **and** a visible window, and an active subscriber also fires the `burn_checks` surface-exposure analytics                    | Overview reads the report with its own consumer id and never counts as a Burn checks exposure                                  |
| Chart needs a library decision                        | `recharts` 3.10.1 is already a dependency (`ContextTokensChart.tsx`), but its bars are not keyboard-focusable                                                                             | Plain DOM bars (30 buttons) — see Decisions                                                                                    |

## Decisions (proposed, for Keith to confirm)

1. **Daily series rides on the existing summary.** Add `days: Vec<ProviderUsageDay>`
   to `ProviderUsageSummary` (totals only, not per provider), computed inside
   `summarize` from the same `WindowBounds` and pricing pass. One IPC call, same
   timezone offset, same pricing snapshot, no second aggregation to drift. The
   payload grows by 30 small rows for every caller, including the popover; that is
   cheap. The alternative, a separate `get_provider_usage_days` command, is only
   worth it if the popover must never see the series.
   _Keith (u-1): "cheap vers, then we'll review"._ Decided: the existing summary.
2. **Bars are plain DOM, not recharts.** Thirty `<button>` elements in a flex row,
   heights from the day's `estimatedUsd` as a percentage of the y-axis max, roving
   `tabindex` with arrow keys. This gives hover and keyboard focus the same
   selected-day detail line for free, renders identically in tests, and keeps the
   chart on semantic utilities. `ContextTokensChart` stays on recharts; the two
   charts do different jobs.
   _Keith (u-2): "some subtle growth animation in would be optimal"._ So the bars
   grow in once on first paint: each bar transitions `height` from 0 over
   `--duration-slow` (300ms, the token `design.md` reserves for "a meter or bar
   that fills"), with a small per-bar stagger left to right so the chart reads as
   filling in rather than popping. Later data refreshes transition the height
   change only, no replay. `motion.css` already clamps every transition under
   `prefers-reduced-motion: reduce`, so nothing extra is needed there. Still no
   recharts: a CSS transition on a plain element does this job.
3. **Chart colours come from the prototype round, not from `measure`.** This
   period's bars are `token-in` (the bright cyan); the previous period's bars are
   `label-tertiary` at 30% opacity; past days of this period sit at 70% opacity and
   today at 100%. See "Settled design" below. No new token.
4. **Overview owns its data through one external store, `MainOverviewSession`**, built on
   the same `attach / start / syncActive / dispose` shape as `BurnChecksSession`.
   It reads local usage, live usage, the checks report, and recent sessions; it
   refreshes on `onSessionsInvalidated`, `onScanEvent` (finished phases),
   `onLiveUsageChanged`, `onChecksReportChanged`, and window visibility. No
   `useEffect` anywhere in the view.
5. **Finding rows navigate to Burn checks, not to a specific check, in v1.**
   Deep-linking would need a `focusDetector(id)` request on `BurnChecksSession`
   that `CheckRow` honours by opening and scrolling.
   _Keith (u-4): "I think we may need to re think burn checks a later day"._ So
   slice 4b is dropped from this stack. Finding rows land on the Burn checks
   section and nothing more; the Overview side of the checks panel stays thin
   (title, count, verified savings, top three findings) so a later rethink of
   Burn checks does not have to unpick Overview.
6. **No Overview analytics surface in v1.** Adding `"overview"` to the `Surface` union
   touches the Rust analytics enums. Not needed to ship the view.
7. **No new stylesheet.** Tailwind semantic utilities cover the layout. If an
   `overview.css` becomes necessary, it is added to `design.md` `sources:` in the same
   change, per `AGENTS.md`.

## Settled design (prototype round, 2026-09-14)

Seven HTML prototype versions were reviewed in `discuss` (scratchpad
`proto/overview/playground-v7.html` is the latest; v1–v6 threads are all resolved).
The values below are Keith's pasted `overview-v6` settings plus his v6 notes, and
override the handoff where they differ. v7 is under review; anything that changes
there gets folded in here.

**Principle: reuse existing components wherever one exists.** Keith: "the design
needs to use as many existing components as possible. For example, the existing
Session list view." Only the bar chart is new drawing. Everything else composes
`SessionRow`, `SegmentedMeter`, `SegmentFigure`, and the presentation helpers.

Layout, top to bottom, at 1040px comfortable density, 16px panel padding:

1. **Hero totals above the chart, no headline.** Kicker "Estimated" (`type-callout`),
   then three cells with hairline dividers: Today, 7 days, 30 days. The figure is
   32px, weight 800, letter-spacing −0.03em, monospace, colour `measure`, rendered
   through `SegmentFigure` for the tabular digits (the mono family is a deliberate
   hero exception, so it is a class on the hero, not a change to `SegmentFigure`).
   Under each figure: "71.2M tokens · 4 sessions" in `type-footnote text-label-tertiary`.
   The right edge carries "Local sessions" as a quiet caption; "Updated just now" sits
   on the page header line.
2. **Daily chart, 90px tall, paired bars.** Title "Estimated spend by session activity
   date" with a key on the right ("Last 30 days" / "30 days before"). For each of 30
   days a pair: this period in `token-in`, the previous period in `label-tertiary` at
   30% opacity, 2px inside the pair, 3px between days. Bars are 7px wide, the same
   diameter as the `SegmentedMeter` dot, and fully rounded (pill), so a zero day is a
   single dot and a bar is a stretched dot. Past days at 70% opacity, today at 100%.
   Y axis with $0/$40/$80 guides; x labels weekly plus "Today" in `token-in`. Grow-in
   on first paint only: 300ms (`--duration-slow`), 12ms stagger per bar, ease-out.
   Below the axis, the selected-day line: "Today · $42.80 · 71.2M tokens · 4 sessions"
   and, once the previous series exists, "±$x vs 30 days before".
   **The previous-period series is new data.** `summarize` must add a second 30-day
   window (`previous_30_days_start = last_30_days_start − 30 × 86 400`) and emit
   `previous_days` alongside `days`; both are 30 buckets, oldest first. The
   `WindowBounds::earliest` query bound moves back to the previous window's start.
3. **Provider limits: dot meters only, no heading, no rings.** Two provider cards side
   by side (Claude Max, Codex Plus), each with the provider name and plan on top and a
   `WindowMeterRow` per window: label left, percent right through `SegmentFigure`,
   `SegmentedMeter` with 16 segments and the elapsed notch, and the reset time under
   the meter as a caption (always shown, not on hover: this surface shows one
   provider per card). The Live/Stale freshness tag floats in the panel's top-right
   corner. `UsageRing` is not used on Overview. `WindowMeterRow` is private to
   `UsageLimitsBar.tsx`; export it (or lift it with `SegmentedMeter` into
   `components/ui`) and give it a `segments` prop rather than duplicating it.
4. **Burn checks: no heading.** One summary line with the burn mark, "2 findings ·
   7 passed", "$27.60 verified savings" under it, and a "More →" link at the right
   edge of that line (not "View report"). Then up to two finding rows, label left and
   "N sessions" right in `brand`. All-passed state: mark, "All 9 checks passed",
   "184 sessions assessed".
5. **Recent sessions: no heading.** An "All sessions →" link above the rows, then three
   real `SessionRow`s with `badgeMetric="cost"`, exactly as the Sessions list renders
   them (status line, title, models, repo · time, fail wash).
6. **Both themes** verified at every version; no new token was needed.

## Round 2 feedback (2026-09-14, in-app via notate)

Keith tested the built page in the running app and pinned thirteen notes with
`notate` (captures `notate-2026-09-14-12.42.20` and `notate-2026-09-14-12.43.45`).
Each note and the change it produced, applied on top of the settled design above:

1. "Lets try with chart on the top" → the chart is first; the totals sit under it.
2. "This 'estimated spend' label seems redundant" (chart heading) → heading removed.
   The section keeps its accessible name.
3. "This is redundant. Remove." (selected-day line under the chart) → the line shows
   only when a day other than today is selected; today's reading is the totals.
4. "remove" ("Local sessions" caption) → removed, with the "Estimated" kicker row.
   The totals cells carry their own labels.
5. "Key needs better home" (legend collided with the top guide figure) → the key
   moves to a footer row under the axis, right-aligned, on the same line as the
   selected-day reading.
6. "These are spaced out way too much" (meter dots) → 32 segments, the popover's
   count, and each provider group is capped at 300px so the dots pack the same.
7. "Remove this" ("More →") → removed. The summary header is the button to the
   report instead.
8. "Use the actual new radial display here" → the flame mark is the report hero's
   `SegmentedRadialDial` (butt caps, no gap, burn in `brand-tint` over `measure`)
   at 44px, with the same minimum-arc rule.
9. "Is there a compact version of these?" (session rows) → `SessionRow` gains a
   `compact` prop: one line with the status, title, first model, time and cost.
   Overview uses it; the Sessions list is unchanged.
10. "How to make this view more prominent." (Burn checks) → the burn percent is the
    headline in `type-title-2`; "N findings · M passed" is the detail line.
11. "This space is stupid" (empty area under the checks card) → the panel row is
    now Burn checks beside Recent sessions, two blocks of about the same height,
    stretched to one height. Provider limits moves to a full-width panel under
    them, with the provider groups side by side. The limits card was the tall
    one; beside anything shorter it always left a hole.
12. "More lines" (chart guides) → guides at 100/75/50/25 percent, each with a figure.
13. "Recent sessions is totally below fold. We'll need to be more compact vertically"
    → page gap `space-xl` instead of `space-2xl`, no chart heading, no totals
    kicker row, compact session rows, the day reading only on demand, and the
    sessions panel in the third row instead of the fourth.

Layout after this round, top to bottom: chart, totals, Burn checks beside Recent
sessions, Provider limits.

## Round 3 feedback (2026-09-14, in chat and via notate)

Keith's second look, in chat and one more `notate` capture
(`notate-2026-09-14-13.10.18`). Each note and the change it produced:

1. "Lets run chart along bottom. That can be what expands when window sizes
   taller." → the chart is the last block and grows with the window. The page is
   a flex column that fills the scroll viewport; the chart keeps its minimum
   height in a short window and takes every spare pixel in a tall one. The
   Radix viewport wrapper has no set height, so the page's percent height could
   not resolve through it; `overview.css` makes that wrapper a flex column.
2. "Instead of click on chart, make hover for info" → the day reading follows the
   pointer. No day is selected any more; keyboard users get the same reading by
   focusing a bar and moving with the arrow keys. The reading line under the axis
   holds a "Hover a day for its reading" hint when nothing is under the pointer,
   so the page does not jump.
3. "Use the segmented radial chart that marty made for the token burn. Also say
   'less than' not the symbol" → the Burn checks hero is `BurnCheckIndicator`,
   the segmented dial from the session list, at 44px. The headline reads
   "Less than 1% estimated burn" instead of "<1%".
4. "Remove all the word 'tokens' and 'token' from this screen. It's redundant.
   Its an app about tokens." → the totals captions read "900k · 2 sessions", the
   day reading reads "Today · $14.50 · 900 · 2 sessions · …", and the checks
   headline reads "estimated burn".
5. "Dont show 'live'" → the limits card shows a "Stale" tag only when a reading
   is stale. Fresh readings carry no tag.
6. "move chart key to top left of chart, on top of chart, with some subtle
   white container around it" → the key sits over the top-left corner of the
   plot in a translucent `surface-card` pill with the card outline. It ignores
   the pointer so the bars under it still hover.
7. "this view is messy - needs more alignment. propose some solutions" (the Burn
   checks card beside the compact session rows) → proposals only, no change
   yet. See the note in chat; the decision goes here once Keith picks one.

Layout after this round, top to bottom: totals, Burn checks beside Recent
sessions, Provider limits, chart.

## Round 4 feedback (2026-09-14, in chat and via notate)

Keith's third look, in chat and one more `notate` capture
(`notate-2026-09-14-13.21.28`). Each note and the change it produced:

1. "Remove the 'hover a day for its reading' bar/line. Instead, use tooltip,
   with a strong hover effect (fade out all others) for the various bars." →
   the reading line is gone. Each day is a `Tooltip` trigger that opens after
   100ms with the day's reading. While one day is under the pointer, or has
   keyboard focus, every other day fades to a quarter. The arrow keys still
   walk the days, and the tooltip follows focus.
2. "make the container for the key white, 20%" → a new `surface-key` token,
   white at 20% in both themes, on the key pill. The token lives in
   `tokens.css` and `design.md`.
3. "Make the VU meters responsive, ie extend them horizontally by drawing more
   LEDs and updating relative LIT leds etc" → each provider group measures its
   own width (`useElementWidth`, a `ResizeObserver` behind
   `useSyncExternalStore`) and draws one dot per 9px, never fewer than 16. The
   lit count follows the percent, so a wider card gets a longer meter with the
   same lit share. The 300px cap on a group is gone.
4. Notate pins "Combine these two into a single vertical, stacked" (Burn checks
   and Recent sessions) and "Stack there, and 2-up to the right of the
   Burn/session stack" (Provider limits), plus "mockup this in proto as well as
   your 3 suggestions above" → a variants proto in the scratchpad
   (`proto/overview-panels/variants-v1.html`, opened in discuss): A is Keith's
   2-up, B same card and header, C one table, D stacked full width. No layout
   change until he picks one.

## Slices

Each slice is one PR of roughly a few hundred lines, stacked on the one before
(per the PR-size habit). No PR opens until Keith has tested and asks for it.

### Slice 1 — Navigation shell

Goal: Overview exists, is the default, and shows a truthful placeholder.

- `apps/desktop/src-tauri/src/main_window.rs:39` — add `Overview` to `MainWindowSection`.
  Extend the `section_target_keeps_only_the_latest_request` test's neighbours with
  a `overview` round-trip.
- `apps/desktop/src/lib/ipc.ts:223` — `MainWindowSectionId = "overview" | "activity" | "burnChecks"`.
- `apps/desktop/src/views/main-window/MainWindowNavigationSession.ts:16` —
  initial `selected: "overview"`, `visited: ["overview"]`. Update
  `MainWindowNavigationSession.test.ts`.
- `apps/desktop/src/views/MainWindowView.tsx` — add the Overview section first
  (`House` icon from lucide per the handoff; open to a better glyph for an Overview label), route `selectSection("overview")` through
  `navigationSession.select`, keep Burn checks and Sessions after it.
- New `apps/desktop/src/views/main-window/OverviewView.tsx` — for this slice only: the
  macOS titlebar spacer (copy the pattern in `BurnChecksView.tsx:24`), an `sr-only`
  `<h1>Overview</h1>`, and a `Skeleton`-based loading block. No fake numbers.
- `apps/desktop/src/views/MainWindowView.test.tsx` — default section is Overview; the
  popover's `openMainWindowSection("burnChecks")` still lands on Burn checks.
- Verify: `pnpm --filter @antiburn/desktop lint|type-check|test`, plus
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
  from `apps/desktop/src-tauri`.

### Slice 2 — Local totals and the daily chart

Goal: the cost-led top of the page, on real data.

Rust:

- `apps/desktop/src-tauri/src/dto.rs` — add `ProviderUsageDay { local_date: String,
tokens_in, tokens_out, cache_read, estimated_usd: Option<f64>, cost_complete: bool,
session_count: u32 }` and `days: Vec<ProviderUsageDay>` on `ProviderUsageSummary`.
- `apps/desktop/src-tauri/src/provider_usage/mod.rs` — add
  `previous_30_days_start` to `WindowBounds` (and to `earliest()`), and a
  `previous_30_days` membership flag. In `summarize`, keep two `[Bucket; 30]` arrays
  indexed by `(updated_at_epoch - <window start>) / 86_400`; emit exactly 30 ordered
  buckets per series, oldest first, zero days included, `local_date` as `YYYY-MM-DD`
  in the reader's offset. Reuse `window_of` for the conversion. The previous window
  feeds only `previous_days`, never the existing totals.
- `apps/desktop/src-tauri/src/provider_usage/tests.rs` — 30 buckets always; a
  session lands in the bucket of its `updated_at_epoch` local date; a boundary at
  local midnight with a non-zero offset; `cost_complete` false propagates to its day;
  sum of `days` equals `totals.last_30_days`; a session 31 days old lands in
  `previous_days` and in no total.
- `apps/desktop/src-tauri/capabilities/main.json` — add `allow-get-provider-usage`.

TypeScript:

- `apps/desktop/src/lib/providerUsageIpc.ts` — `ProviderUsageDayPayload` and
  `days?: ProviderUsageDayPayload[]` on `ProviderUsageSummaryPayload` (optional so
  the popover-peek fixtures keep compiling). `EMPTY_PROVIDER_USAGE` in `ipc.ts`
  gains `days: []`.
- New `apps/desktop/src/views/main-window/MainOverviewSession.ts` — snapshot
  `{ active, usage, usageError, liveUsage, generatedAt, loading, refreshing, … }`.
  This slice loads `getProviderUsage()` and `getLiveUsage()`; later slices add
  checks and sessions. Adapter interface for tests, like `BurnChecksAdapter`.
- New `apps/desktop/src/views/main-window/overview/OverviewSpendChart.tsx` — eyebrow
  "Estimated spend", the 30-day figure at `type-large-title` `font-mono tabular-nums`,
  the selected-day line top-right, the chart with the title "Estimated spend by
  session activity date", subtle guides, sparse date ticks, and the today mark.
  Y-axis max is the next step above the largest day (same rule as the Context
  chart). A day with `estimatedUsd === null` and tokens > 0 draws a hatched or
  outlined bar with "not priced" in its detail line; it is not drawn as zero.
- New `apps/desktop/src/views/main-window/overview/OverviewSpendTotals.tsx` — Today / 7 days
  / 30 days from `totals.today`, `totals.week`, `totals.last30Days`, hairline
  separators, `font-mono tabular-nums`. Reuse `formatSpendFigure`,
  `formatTokenFigure`, `windowTokens`, `sessionCountLabel` from
  `lib/presentation/providerUsage.ts`. `costComplete === false` appends "partial";
  `estimatedUsd === null` falls back to the token figure with a "tokens" label, as
  the handoff allows.
- `OverviewView.tsx` — replace the placeholder; wire the session with
  `useSyncExternalStore(active ? session.subscribe : session.subscribeInactive, …)`.
- Tests: `MainOverviewSession.test.ts`, `OverviewSpendChart.test.tsx` (30 bars, keyboard
  arrow selection updates the detail line, null-cost day is not zero),
  `OverviewSpendTotals.test.tsx` (partial and null states).

### Slice 3 — Provider limits panel

- `apps/desktop/src/components/providerUsage/UsageLimitsBar.tsx` — export
  `WindowMeterRow` with a `segments` prop (default 32) and a `resetPlacement`
  option so the reset can sit as a caption under the meter instead of beside the
  label. No visual change to the popover.
- New `apps/desktop/src/views/main-window/overview/OverviewProviderLimits.tsx` — a
  `bg-surface-card rounded-control` panel with no heading; the freshness tag
  (Live / Stale from `liveUsage.generatedAt`) absolutely positioned top-right.
  Providers from `orderedLiveAccounts(liveDisplayableProviders(live))`, each a
  column: display name, `livePlanAccountLabel`, then one `WindowMeterRow` per window
  at 16 segments with the reset caption always visible. Unavailable providers via
  `liveUnavailableProviders` and `liveUnavailableReason`. Two abreast with
  `grid-cols-[repeat(auto-fit,minmax(…))]` so it stacks when narrow. No `UsageRing`.
- Empty state when no provider reports anything: one quiet line pointing at Settings.
- `MainOverviewSession` — subscribe `onLiveUsageChanged`; no `refreshLiveUsage()` from
  Overview (the shell owns the polling cadence).
- Tests: `OverviewProviderLimits.test.tsx` — determinate meter with notch, `null`
  percent at half strength, red zone above 90%, unavailable seat, empty state; the
  local-cost figures never appear in this panel.

### Slice 4 — Burn checks summary and recent sessions

- `MainOverviewSession` — read `getChecksReport("main-home-<n>")` with
  `cancelChecksReport` on dispose, refresh on `onChecksReportChanged`; load
  `listRecentSessions()` and keep the newest three, refresh on
  `onSessionsInvalidated` and `onSessionEntryChanged`.
- New `overview/OverviewBurnChecks.tsx` — rollup via `checksPresentation` /
  `checksHeroPresentation` from `lib/presentation/checks.ts`; up to two categories
  with `finding > 0`, ranked by `estimatedTokenBurnBasisPoints` then `finding`,
  labelled through `checkRowPresentation` in `views/checks/checkUi.ts`; row trailing
  text "N sessions". All-passed and assessing/unavailable states per the handoff.
  No panel heading; the "More" link sits at the right of the summary line. "More" and
  each row call `onOpenBurnChecks()`.
- `apps/desktop/src/components/session/SessionList.tsx` — export `SessionRow`
  (and its props type) so Overview renders the same silhouette and states. No visual
  change to the list.
- New `overview/OverviewRecentSessions.tsx` — three `SessionRow`s with `badgeMetric="cost"`,
  `hygieneBySession` from `useSessionHygiene(sessionHygieneIdentities(rows))`, under
  an "All sessions" link (no heading). Row click → `navigationSession.select("activity")` then
  `activitySession.selectEntry(entry)`. Confirm during implementation that
  `selectEntry` before the list has loaded selects correctly; if not, route through
  `openRelated(subjectForEntry(entry))`.
- `MainWindowView.tsx` — pass the two navigation callbacks into `OverviewView`.
- Tests: `OverviewBurnChecks.test.tsx` (findings, all passed, pending never shows zero),
  `OverviewRecentSessions.test.tsx` (three rows, navigation callbacks),
  `MainWindowView.test.tsx` (Overview row click lands in Sessions with that session).

### Slice 5 — Polish and review

- Responsive: provider and checks panels stack below a measured width; totals
  stack at very narrow widths; date ticks thin before the chart ever scrolls.
  Built with CSS container queries on the page (`overview.css`), the same
  pattern as `session-detail.css`, because no `useElementWidth` hook exists.
  Panels stack below 700px, totals below 540px. The axis labels sit at a fixed
  pitch per day, so they never collide and need no thinning; below the chart's
  natural width the chart scrolls sideways instead of shrinking the bars.
- Light, dark, increased text size, reduced motion (the bar grow-in is clamped
  by `motion.css`; confirm nothing else moves), keyboard pass through nav →
  bars → finding rows → session rows with visible focus.
- `node scripts/check-design-drift.mjs` if any token or stylesheet changed.
- Design review of the built window in both themes, then Keith's hands-on test.
  Only then: ask whether to open PRs.

## Verification per slice

```sh
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

Rust slices, from `apps/desktop/src-tauri`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Every commit signed off with `git commit -s`.

## Open questions for Keith

- ✋ Handoff step 5 names a repository `design-review` skill. There is none in this
  repo's `.claude/`. Is there one elsewhere, or does "design review" mean a manual
  pass in both themes?

## Non-goals (unchanged from the handoff)

Billing reconciliation, budgets or invented denominators, predictive burn in the
hero, a menubar redesign, a second token system, a filler KPI grid, a spend/tokens
switch.

## Status

| Step                                          | State                                                                                                                                                                          |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Worktree reset onto `origin/main`             | done (2026-09-14)                                                                                                                                                              |
| Code audited against the handoff              | done                                                                                                                                                                           |
| Implementation plan written                   | done                                                                                                                                                                           |
| Plan reviewed by Keith                        | done (2026-09-14, discuss, six threads resolved)                                                                                                                               |
| Design prototypes v1–v7 reviewed by Keith     | v1–v6 done (2026-09-14, discuss); v7 open                                                                                                                                      |
| Slice 1 — navigation shell                    | done (2026-09-14)                                                                                                                                                              |
| Slice 2 — totals and daily chart              | done (2026-09-14)                                                                                                                                                              |
| Slice 3 — provider limits                     | done (2026-09-14)                                                                                                                                                              |
| Slice 4 — burn checks and recent sessions     | done (2026-09-14)                                                                                                                                                              |
| Slice 5 — polish, design review, Keith's test | polish done (2026-09-14); Keith's first two hands-on tests done (2026-09-14, see Round 2 and Round 3); selected layout implemented: checks and recent sessions share a card beside fixed-width provider limits; `/design-review` on a live instance still to run |
| Overview follow-up — fixed 180pt usage card and Recent header | Done (2026-09-14). Formatting, lint, type-check, all 1,558 desktop tests, frontend build, design drift, and diff checks pass. Native visual review pending. |
| Publication — five planned PR slices | Published 2026-09-14: [#529](https://github.com/antiburn/antiburn/pull/529) → [#530](https://github.com/antiburn/antiburn/pull/530) → [#531](https://github.com/antiburn/antiburn/pull/531) → [#532](https://github.com/antiburn/antiburn/pull/532) → [#533](https://github.com/antiburn/antiburn/pull/533). All assigned to Zack (`z0w0`); screenshot placeholders await Keith’s upload. Completed-stack Rust formatting, Clippy, and 1,241 tests passed; slop and secret scans passed. |
| Overview follow-up — usage-only card | Replaces the background-free usage variation (2026-09-14): restore usage card fill and outline; remove card chrome from the checks and sessions section. Keep widths and padding. Formatting, ESLint, type-check, 12 panel tests, design drift, and diff checks pass. Keith approved the result and requested pushing it to PR #533 (2026-09-14). |
| Overview follow-up — burn finding cards | Implemented locally: match the recent session cards with shared fill, radius, padding, gaps, and hover treatment. Formatting, ESLint, type-check, six panel tests, design drift, and diff checks pass. Keith approved the result and requested pushing it to PR #533 (2026-09-14). |

## CI export fix (2026-09-14)

Make `localDateOf`, `overviewRecentSessions`, and `overviewChecksSummary` private to their modules. Apply each fix at its first affected PR, merge the fixes forward without rewriting published commits, run Knip on each affected slice and the frontend checks on the completed stack, then push and verify CI.

| Step | Status |
| --- | --- |
| Identify CI failures | Done: unused exports in Linux Knip on #530–#533; #529 passes. |
| Fix and propagate exports | Done on #530–#533 with signed commits and forward merges. |
| Local verification | Passed: Knip on all four affected slices; focused tests and type checks on intermediate slices; final clean checkout formatting, lint, type-check, all 1,558 tests, and build. Slop, secrets, design drift, and diff checks passed. |
| Push and CI verification | Fixes prepared for #530–#533. Live CI results are recorded on the linked PR checks. |
