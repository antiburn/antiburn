# Home view design handoff

**Date:** 2026-09-14

**Status:** design direction agreed; implementation not started

**Target:** antiburn desktop main window on macOS, Windows, and Linux

## Handoff objective

Add a default **Home** section to the desktop main window. Home gives the reader
one calm, useful answer to “what is happening?” before they enter Burn checks or
Sessions. It combines local cost activity, provider-reported allowance limits,
a compact burn-check summary, and recent sessions.

The view must feel like the existing menubar and Session detail surfaces. It is
not a new dashboard design language. Reuse their components, density, typography,
semantic colors, hover behavior, and honest data states.

## Start here

Read these sources before changing code:

1. [`apps/desktop/design.md`](../../apps/desktop/design.md) is the design contract.
   Its YAML tokens and listed stylesheets are authoritative.
2. [`docs/plans/session-detail-style-guide.md`](session-detail-style-guide.md)
   records the density, type, meter, and color decisions developed in Session
   detail.
3. [`apps/desktop/src/views/MainWindowView.tsx`](../../apps/desktop/src/views/MainWindowView.tsx)
   owns the current main-window navigation and retained panes.
4. [`apps/desktop/src/components/providerUsage/UsageLimitsBar.tsx`](../../apps/desktop/src/components/providerUsage/UsageLimitsBar.tsx),
   [`UsageRing.tsx`](../../apps/desktop/src/components/providerUsage/UsageRing.tsx),
   and [`LiveUsageWindowRows.tsx`](../../apps/desktop/src/components/providerUsage/LiveUsageWindowRows.tsx)
   define the menubar limit language to reuse.
5. [`apps/desktop/src/components/session/SessionList.tsx`](../../apps/desktop/src/components/session/SessionList.tsx)
   and the Session detail components define the recent-session row language.

Design lineage:

- [PR #492](https://github.com/antiburn/antiburn/pull/492)
- [PR #493](https://github.com/antiburn/antiburn/pull/493)
- The agreed interactive mock is currently at
  `/Users/keithlang/.codex/visualizations/2026/09/11/01a08ec0-5d0d-71b0-b8f1-5869a4e23c2b/antiburn-home.html`.
  Treat its numbers as illustrative and its structure as direction. Production
  code and `design.md` remain the source of truth.

## Agreed product decisions

1. **Home is the first and default main-window section.** Navigation order is
   Home, Burn checks, Sessions, then Settings in the existing footer position.
2. **Cost leads the page.** Remove the earlier circular “estimated burn” hero.
   The top area is an open, chart-led cost view.
3. **Show a trailing 30-day daily bar chart.** Spend is the primary measure.
   One quiet cool-colored bar represents one local calendar day; today has a
   stronger treatment.
4. **Put Today, 7 days, and 30 days directly under the chart.** Estimated spend
   is primary. Tokens and session count are secondary context.
5. **Provider limits echo the menubar.** Each provider has its mark inside a
   usage ring and its reported windows in the familiar stack, such as 5-hour
   and weekly, with percentage, reset metadata, and elapsed-period notch.
6. **Burn checks are compact.** Show the rollup and at most the two most useful
   findings. The full report remains the destination for detail.
7. **Recent sessions close the page.** Use the existing session-row silhouette
   and navigate into Session detail.
8. **Do not add a visible spend/tokens switch in version one.** The mock's
   measure control is a design-review aid supplied outside the depicted app.
   If estimated cost is unavailable, the product may fall back to tokens with
   an explicit label.

## Information hierarchy

The workspace reads from broad state to action:

1. **Cost activity:** “How much local agent work happened?”
2. **Provider limits:** “How close am I to the allowances providers report?”
3. **Burn checks:** “Is any of that work worth reviewing?”
4. **Recent sessions:** “Where should I continue or investigate?”

Keep the first screen useful without scrolling at the normal main-window size.
Do not make every section a card. The cost chart sits on the open workspace;
the provider and burn summaries use quiet `surface-card` grouping; sessions use
the established list rows.

## Layout anatomy

### App shell

- Keep `SidebarNav` and the shared `--sidebar-width` of 220px.
- Add a `House`-icon Home row before Burn checks.
- Preserve the platform titlebar behavior in `MainWindowLayout`.
- Use the existing 4px spacing rhythm and main-workspace padding. Do not copy
  raw spacing values from the mock.

### Cost overview

- Top-left: “Estimated spend” eyebrow, then the 30-day figure at
  `type-large-title` or the closest existing hero style.
- Top-right: the selected or hovered day's date, estimated spend, tokens, and
  session count. Keep this to one quiet line when space allows.
- Chart: 30 daily columns, subtle horizontal guides, sparse date labels, and a
  distinct today mark. Bars use a single cool data color. They do not use a
  warning ramp.
- Hover and keyboard focus expose the same day detail. Essential totals remain
  visible without hover.
- Totals row: three equal columns for Today, 7 days, and 30 days. Use tabular
  numerals. Separate columns with hairlines rather than individual cards.

### Provider limits

- Use a quiet panel headed “Provider limits” with a small live/freshness note.
- Present providers as peers. Two can sit side by side at normal width and
  stack when the pane narrows.
- The provider header uses the real vendor mark inside `UsageRing`; use the
  existing fallback initial when no mark exists.
- Under the header, render the provider's own window rows with
  `LiveUsageWindowRows` semantics: label and percent, thin neutral track,
  `accent-fill` progress, and the elapsed-period notch.
- The ring represents the provider's highest current window. Its arc remains
  brand orange, matching the menubar. The arc is a quantity and provider
  identity, not a warning verdict.
- The Home composition may show a ring header and window rows together. Reuse
  the underlying primitives rather than embedding `UsageLimitsBar`, because
  that menubar component intentionally swaps its collapsed and expanded states.
- Keep provider-reported limits visually and semantically separate from local
  cost estimates. They have different denominators and different provenance.

### Burn checks

- Use one compact panel. Do not restore the old burn dial.
- Default finding state: “2 findings · 7 passed” with the estimated burn and
  period beneath it.
- List no more than two findings. Each row has the finding name and affected
  session count, then opens that finding in the full report.
- All-passed state replaces finding rows with one positive line.
- Assessing or unavailable state explains that results are pending. Never show
  missing results as zero findings.

### Recent sessions

- Show two or three recent sessions, then “All sessions”.
- Reuse the existing two-line session hierarchy: verdict, title, and muted
  model/repository context, with cost and recency aligned at the trailing edge.
- Rows are destinations. Use the existing selected, hover, focus, active-title,
  cost-badge, and verdict behavior rather than creating a Home-only variant.

## Color and visual semantics

Use semantic utilities from `design.md`; never paste colors from the mock.

| Meaning | Treatment |
|---|---|
| Normal quantitative activity | A cool cyan/blue treatment. Use the existing accent/data language unless a recurring Home-specific value justifies a documented token. |
| Explicit pass or success | `system-green`, matching current checks and status components. |
| Cost and antiburn identity | `brand` for small text/glyphs and `brand-tint` for fills and usage-ring arcs. |
| Attention | `system-orange` or `system-yellow`, according to the existing warning scale. |
| Failure or critical state | `system-red` / `system-red-text`. |
| Structure | `surface-window`, `surface-sidebar`, `surface-card`, `surface-hover`, `surface-selected`, `separator`, and label inks. |

The earlier shorthand “cyan good, orange alert, red bad” is useful for the
overall temperature, but production semantics are more precise: green already
means an explicit pass, orange also carries antiburn's brand and cost identity,
and provider ring arcs are orange without claiming danger. Pair every semantic
color with a label, icon, value, or shape.

## Typography and density

- Follow the Session detail rule: hierarchy comes mainly from ink, weight, and
  placement, not many type sizes.
- Use `type-large-title` once for the top figure.
- Use `type-body` for primary rows and values.
- Use `type-callout`, `type-footnote`, or `type-caption` only for concise
  secondary context already supported by the design contract.
- Use `font-mono` and `tabular-nums` for aligned spend, token, percentage, and
  time figures.
- Avoid headings that only repeat obvious content. “Provider limits”, “Burn
  checks”, and “Recent sessions” are useful because they separate different
  data sources and actions. The chart label is necessary because its date
  semantics are unusual.
- Use `rounded-control` for quiet panels and controls. Avoid large radii,
  floating dashboard cards, gradients, decorative shadows, and oversized icons.

## Data contract and truthfulness

### Totals already available

`getProviderUsage()` in [`apps/desktop/src/lib/ipc.ts`](../../apps/desktop/src/lib/ipc.ts)
calls the local `get_provider_usage` Tauri command. The current payload already
contains independent local-calendar windows for:

- `today`
- `week` — trailing seven calendar days
- `monthToDate`
- `last30Days` — trailing 30 calendar days, including today

Each window carries input, output, and cache-read tokens, `estimatedUsd`,
`costComplete`, and `sessionCount`. Use `today`, `week`, and `last30Days` for
the three displayed totals. These are API-equivalent estimates from local
session evidence, not provider invoices.

Honor these states:

- `estimatedUsd === null`: no defensible estimate is available.
- `costComplete === false`: some activity could not be priced. Mark the figure
  as partial; do not silently present it as complete.
- No providers or sessions: show an honest empty state, not zero activity
  presented as a measured result.

### Daily chart data still needs a contract

The summary payload does not expose 30 daily buckets. Add a typed daily series
at the aggregation boundary rather than rebuilding it from rendered session
rows. A suggested shape is:

```ts
interface ProviderUsageDayPayload {
  localDate: string
  tokensIn: number
  tokensOut: number
  cacheRead: number
  estimatedUsd: number | null
  costComplete: boolean
  sessionCount: number
}
```

Return exactly 30 ordered local-calendar buckets, including zero-activity days,
so chart spacing and date labels remain stable. Reuse the same timezone offset
and pricing snapshot as `get_provider_usage`.

Current aggregation assigns a session's complete token and price breakdown to
the window containing `UsageEvidenceRecord.updated_at_epoch`. Therefore the
honest chart title is:

> Estimated spend by session activity date

A session spanning several days contributes its entire aggregate to its latest
meaningful activity day. Do not label this “daily spend” or imply per-turn cost
accrual unless the backend later gains timestamped turn-level evidence.

### Live provider limits

Provider allowance limits come from the separate `getLiveUsage()` payload. Do
not derive them from local token or cost totals. Preserve the existing observed,
stale, unavailable, and indeterminate states. A missing percentage is not 0%.

## Interaction and navigation

- Extend `MainWindowSectionId` in `ipc.ts` with `home`.
- Change `MainWindowNavigationSession`'s initial `selected` and `visited` values
  from `burnChecks` to `home`.
- Add Home to `MainWindowView` before the existing sections.
- Preserve retained-pane behavior: first visit starts the Home data session;
  revisits reuse its snapshot.
- Follow the existing external-store boundary, such as a `MainHomeSession`, for
  subscriptions and refresh. Do not add `useEffect`.
- Burn findings navigate to Burn checks with the relevant check selected when
  practical. “All sessions” and recent rows navigate through the existing
  Sessions and Session detail paths.
- Chart hover or focus changes only the local selected-day line. It does not
  trigger navigation.

## Responsive and platform behavior

- At normal width, provider limits and Burn checks can share one row.
- Stack those panels when their provider names, reset text, or finding labels
  would truncate materially.
- Keep all 30 bars visible; reduce date ticks before introducing horizontal
  scrolling.
- Let Today/7 days/30 days stack at very narrow widths.
- Keep macOS titlebar clearance and native window controls. Windows and Linux
  use the same content hierarchy inside their existing decorated shell.
- Test light, dark, increased text size, reduced motion, keyboard navigation,
  and unavailable data.

## Suggested implementation slices

Keep each reviewable change small.

1. **Navigation shell:** add the Home section and default route with a truthful
   loading/empty placeholder.
2. **Local totals and daily series:** extend the provider-usage DTO, Rust
   aggregation, IPC typing, and tests; render the cost chart and three totals.
3. **Provider limits:** connect live usage and reuse the ring/window primitives.
4. **Burn checks and sessions:** add compact summaries and destination actions.
5. **Polish:** responsive states, accessibility, motion, and cross-theme review.

Do not open a pull request until Keith has tested the finished view and
explicitly asks for a PR afterward.

## Acceptance criteria

- Home is the selected section when a normal main window first opens.
- The top of Home is cost-led and contains no burn dial.
- The chart has 30 stable local-date columns and an honest activity-date label.
- Today, trailing 7-day, and trailing 30-day totals agree with the backend
  summary for the same timezone offset.
- Partial, missing, stale, indeterminate, assessing, empty, and error states do
  not masquerade as zeros or completed results.
- Provider marks, rings, window meters, elapsed notches, and reset labels match
  the menubar primitives.
- Burn checks remain a summary; Sessions and Burn checks remain the detail
  destinations.
- The view uses only documented semantic tokens, `type-*` utilities,
  `rounded-control`, and `duration-*` motion.
- Keyboard users can reach navigation, chart-day details, finding rows, and
  session rows with a visible focus state.
- Layout remains legible in light and dark themes on macOS, Windows, and Linux.

## Verification

Run the checks required by [`CONTRIBUTING.md`](../../CONTRIBUTING.md) and
[`apps/desktop/README.md`](../../apps/desktop/README.md):

```sh
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

If Rust changes, also run from `apps/desktop/src-tauri`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Run `node scripts/check-design-drift.mjs` when tokens or stylesheets change.
Use the repository's `design-review` skill on the implemented Home window in
every supported theme and state before asking Keith to test it.

## Non-goals

- Provider billing or invoice reconciliation
- Budgets, quotas inferred from spend, or a cost “full ring” with no denominator
- Predictive burn scoring in the Home hero
- A menubar redesign
- A second theme or token system
- A generic analytics dashboard or a grid of filler KPI cards

## Status

| Step | State |
|---|---|
| PR, Session detail, menubar, and design-system references reviewed | done |
| Home hierarchy discussed and agreed | done |
| Interactive design mock updated | done |
| Implementation handoff written | done |
| Production implementation | not started |
| Cross-platform design review | not started |
| Keith's hands-on test | not started |
