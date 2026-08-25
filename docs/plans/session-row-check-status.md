# Session-row check status — one line, counted, hover-expanded

Replaces the V6 hygiene glyph fan (`docs/plans/session-row-v6.md`, phase 2) with the check-status slot Keith picked on 2026-08-25. Target: `apps/desktop`.

## Status

| Phase | Status |
| --- | --- |
| 1. Short check names in the mock data | Done |
| 2. `SessionHygieneStatus` component | Done |
| 3. Row wiring + fan removal | Done |
| 4. Stylesheet + `design.md` | Done |
| 5. Tests, slop, screenshot, PR | Done — screenshot captured from the running popover |
| 6. Review pass 1 (Keith, 2026-08-25) | Done — copy trimmed, meta line re-ordered, hot cost de-pilled |

## The design

The slot sits at the **start of the models meta line**, left of the model names.

- **All checks passed** — a lone green check glyph (11px, stroke 3). On row hover the
  glyph gains the text "6/6 passed" in the same green, and the model names shift right to
  make room.
- **Any check failed** — no glyph. The slot shows "2/6 failed" in red, always visible at
  rest. The fraction is semibold with `tabular-nums`; the word stays medium. On row hover
  one more red line opens below the meta line, listing the failed checks' short names,
  comma-joined, truncating with an ellipsis on overflow.

The fraction counts **failures**, not passes, on a failing row: "2/6 failed" means two of
the six checks failed. The real hygiene set has six checks today, so the mocks' "7/7"
reads "6/6" in the app.

The visible label drops the word "checks" (Keith's call on review — the row has no space
for it). The `aria-label` keeps the full sentence, because a listener has no row to read
the meaning from.

## Decisions

### Colors: use `system-green` and `system-red`, do not add new tokens

The brief assumed no green or red token existed. Both do, and the light-mode
`system-green` (`#248A3D`) is the exact green the mock used. `system-red` is within a
hair of the mock's red in both themes (`#D70015` / `#FF6961` against the mock's
`#D02F1F` / `#FF6157`). Adding `success`/`danger` tokens would put a near-duplicate pair
in the palette, which is the drift `design.md` exists to prevent, so this change uses the
existing tokens and adds none.

`system-red-text` is the wrong variant here: `tokens.css` documents it as the red for
text sitting **on the red tint**, where the plain system red dips below AA. This text sits
on the popover surface, where `system-red` measures 7.7:1 light and 6.0:1 dark.

`system-green` as text measures 4.4:1 on a white card, just under AA. That is the token's
standing value and it is already used as text in `UsageView`, `LiveUsageDetail`, and
`EfficiencyBreakdown`; this change does not re-open it. The resting state is a glyph,
which needs 3:1, and the text only appears over the hover wash, which is darker.

### Hover is pure CSS

No `useEffect`, no JS hover state. Both reveals key off `.session-row:hover` and animate a
`0fr → 1fr` grid track over `--duration-fast`: a column for the passed label (so the model
names slide right rather than jumping), a row for the failure line. The gap and the top
inset live on an inner span, so they collapse with the track instead of leaving a stub of
padding at rest.

### Accessibility

The status slot carries the whole sentence as an `aria-label` and hides its visual parts
from assistive tech, so the count and the failed names are readable at rest even though
the text is visually hidden until hover. The failure line is `aria-hidden`; its text is
already inside that label.

## Steps

1. **`mockSessionHygiene.ts`** — add a `shortTitle` to the check definitions and to
   `MockSessionHygieneCheck`. It is the check's plain name, with no "detected" suffix and
   no "No " prefix: "Session overdepth", "Model overthinking", "Overpowered subagents",
   "Obsolete model", "Fast mode overuse", "Excess cache rehydration".
2. **`SessionHygieneStatus.tsx`** — new component, replacing `SessionHygieneBadges.tsx`.
   Renders the slot and, for a failing row, the failure line. Delete
   `SessionHygieneBadges.tsx` and its test.
3. **`SessionList.tsx`** — drop `SessionHygieneBadges` from the right rail on the repo
   line; wrap the models line so the failure line can sit under it; put the status slot at
   the start of that line.
4. **`session-rows.css`** — delete the whole fan block (`.session-hygiene-pass`,
   `.session-hygiene-mark`, the `nth-last-child` offsets and delays, the repo-name fade,
   and the fan's reduced-motion rules). Add the two reveal rules.
5. **`design.md`** — replace the `session-hygiene-fan` and `session-hygiene-mark` motion
   recipes with one `session-hygiene-status` recipe. No `sources:` change: the stylesheet
   is already listed and no token moves.
6. **Tests** — rewrite the `SessionList` hygiene test against the new copy, add a
   `SessionHygieneStatus` test, extend the `mockSessionHygiene` test for `shortTitle`.
   Then `pnpm run slop`, the repo checks, `git commit -s`, and a screenshot for the PR.

## Review pass 1 — 2026-08-25

Keith reviewed the running app and asked for four changes. All are in this change.

1. **Drop "checks" from the visible label** — "1/6 failed", not "1/6 checks failed".
2. **Show the time on hover only** — the `<time>` fades in over `--duration-fast`. This is
   visual only: the element and its `aria-label` stay in the accessibility tree at rest.
3. **Cost moves down to the models line**, right-aligned. **Time moves up to the repo
   line**, top right. Together these land points 1-3 of the chosen layout in
   `session-list-chosen-layout`, apart from the branch line, which still has no data.
4. **The hot cost loses its pill** — a flame glyph on the left plus semibold `brand`
   orange type. `.type-caption` sets its own weight, so the weight utility needs `!`.
   The calm cost keeps the grey pill, which now reads as the quiet state against the
   loud one.

## Out of scope

- Real hygiene data — the slot still reads `mockSessionHygiene`.
- The branch line of the chosen row layout. `branch` is still not populated by the shell,
  so that line waits for its own change.
- The detail pane's hygiene dots (`SessionDetailPresentation`), which are a separate
  treatment and keep their own markup.
