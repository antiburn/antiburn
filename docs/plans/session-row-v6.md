# Session row V6 — simpler card, hygiene glyphs, brand-orange alerts

Implements the design settled in the HTML proto (session-row-proto.html, iterated to V6 in the 2026-08-24 session). Target: `apps/desktop/src/components/session/SessionList.tsx` and friends.

## Status

| Phase | Status |
| --- | --- |
| 1. Icon availability check | Done — all six exports verified in installed `lucide-react@1.33.0` |
| 2. `SessionHygieneBadges` component | Done — `SessionHygieneBadges.tsx` + fan CSS in `session-rows.css` + `design.md` recipe |
| 3. Row layout restructure | Done — models top, `type-title-3` title, repo line, right rail, hover time |
| 4. Cost pill goes brand orange | Done — solid `bg-brand-tint text-white` (Keith picked solid over tint) |
| 5. Tests + checks + screenshot | Done — tests updated/added, all checks green, Keith supplied a demo recording for the PR |

Post-review refinements folded in during Keith's testing (2026-08-24): titles settled on a new `type-body-large` step (14px/400); non-hot cost pill went grey; hygiene glyphs lightened to 1.75 stroke and fail glyphs use `brand-tint`; model names fade while the fan is open; the model line always renders to keep row rhythm; the agent icon centers on the title line; the active-row background pulse was removed (shimmer stays); and check/cross marks rise in above the glyphs after a 2s hover, left to right.

## Locked design decisions (from the proto)

- **Row layout**: model names move to the top meta line (above the title); repo/project hangs below the title on its own line. Relative time is hidden at rest and fades in bottom-right, on the same line as the repo name, on card hover.
- **Hygiene marks are bare glyphs** — no circles, no pills, no shadows. One icon per check, 12px glyph in a 20px-tall hit target (matches the cost pill height).
- **Failing checks are always visible**, sitting directly left of the cost pill, in brand orange.
- **Passing checks are hidden at rest.** On card hover they fan out leftward from the leftmost alert glyph (or from the cost pill when all pass), each chip translating from a stacked position with a slight outward stagger. On hover-out they **fade in place** — no slide-back (the transform snap-back is delayed past the fade).
- **Alert colour**: brand orange, via the existing `brand` token — not raw `#FF6A2C`, since `brand` is already contrast-adjusted per theme. Pass glyphs use `text-label-tertiary`.
- **High-cost pill goes solid brand orange**: white text and flame on a full `brand-tint` fill (`bg-brand-tint text-white`), replacing the red tint treatment (`bg-system-red/15 text-system-red-text`). Keith picked "solid" from four proto variants after flagging that pure orange *text* suffers on a light card. Check white-on-`brand-tint` contrast in both themes; if it falls short, deepen the fill with the `brand` (text-grade) colour rather than reverting to tinted text.
- **Session title size up one notch.** Keith's call: antiburn's current text runs small against best-practice Mac apps. The proto moved the title from 14px to 15px. Implementation-wise this means adjusting the `type-callout` step (or moving the title to a larger existing `type-*` step) — a token change, so update `design.md` in the same change per the drift check. Decide scope in review: title only, or the whole `type-*` scale.
- **Icons** (owner-picked): all six checks exist in lucide.

  | Check | lucide import |
  | --- | --- |
  | sessionOverdepth | `LifeBuoy` |
  | modelOverthinking | `Brain` |
  | overpoweredSubagents | `Hammer` |
  | obsoleteModel | `Sunset` |
  | fastModeOveruse | `Rabbit` |
  | excessCacheRehydration | `Droplet` |

  The proto used hand-drawn approximations; the real lucide glyphs differ slightly (lucide's Rabbit is a full side profile, its Brain has more interior detail). Sanity-check legibility at 12px once wired in. Verify the imports compile against the pinned `lucide-react@1.33.0` as step one of implementation.

## Implementation steps

### 1. Icon check (5 min)
`pnpm --filter desktop exec node -e "…"` or simply import the six icons and typecheck. If any name differs in 1.33.0 (e.g. `LifeBuoy` vs `Lifebuoy`), fix the import — the glyphs themselves are all in the catalog.

### 2. `SessionHygieneBadges` component
New file `apps/desktop/src/components/session/SessionHygieneBadges.tsx`.

- Props: `checks: MockSessionHygieneCheck[]` (keep consuming `mockSessionHygiene` — real data is a separate feature).
- Renders two groups: failed checks (always visible, `text-brand`), passed checks (the fan-out group, `text-label-tertiary`).
- Each glyph wrapped in the existing Radix `Tooltip` (short delay ~150ms) with the check title; `aria-label` on each glyph. This replaces the proto's CSS `data-tip` tooltips.
- Fan-out mechanics are pure CSS, keyed off the row's `group` hover (`group-hover:` variants). No `useEffect`, no JS state.
- The per-chip stacked offsets and stagger delays (`nth-last-child(n) → translateX(n·23px)`, `transition-delay: n·15ms`) don't express well as Tailwind utilities. Put them in a small stylesheet block in an existing session stylesheet or a new `session-hygiene.css`; **if a new stylesheet is added, register it in `design.md` `sources:` in the same change** (CI runs `scripts/check-design-drift.mjs`). Use `duration-*` tokens for timings — no hard-coded ms values outside the stylesheet's documented ones.
- Reduced motion: gate the transform animation behind `motion-safe:`; with reduced motion the pass glyphs just fade.

### 3. Row layout restructure in `SessionRow`
Current structure (top line: title + fork icons; middle: repo · WSL · branch · cost · time; bottom: models + hygiene dots) becomes:

- **Top meta line**: model names (existing `modelRunShortNames` text, `type-footnote text-label-tertiary`).
- **Title line**: unchanged (title + fork icons, 2-line clamp, shimmer when active).
- **Bottom line**: repo (+ additional-repos tooltip), WSL badge, branch — and the relative `<time>` right-aligned, `opacity-0 group-hover:opacity-100`. Keep the `<time>` element and its `aria-label` so the timestamp stays available to screen readers at all times; hover-hiding is visual only.
- **Right rail** (top-aligned with the model line): `SessionHygieneBadges` fails + `SessionCostBadge`. The pass-glyph fan-out overlays the title area when it needs the width (absolutely positioned, anchored `right: 100% + gap` of the rail).
- Hygiene dots block (`hygieneChecks.map` with ✓/× spans) is deleted, replaced by the new component.

Open question for review: the proto never showed **branch**. Plan keeps it on the bottom line next to repo; flag in review if it should be hover-only like the time.

### 4. `SessionCostBadge` hot state
One-line class swap: `bg-system-red/15 text-system-red-text` → `bg-brand-tint/15 text-brand`. Update the comment that says "red plus a flame" (WCAG note still applies — the accessible name already carries the meaning). The tooltip's "Higher than usual" line (`text-system-red-text`) switches to `text-brand` to match.

### 5. Tests, checks, PR
- Update `SessionList` tests for the new structure (hygiene ✓/× spans are gone; time visibility class; model line position).
- Add a render test for `SessionHygieneBadges`: fails render with titles, passes render, orders fails after passes in DOM (rightmost).
- `pnpm run slop` before finishing; fix any aislop findings this change causes.
- All commits `git commit -s` (DCO).
- PR needs a screenshot — capture the row at rest and mid-hover from the running app, or ask Keith for one before `gh pr create`.

## Out of scope

- Changes to the underlying evidence collection beyond the existing real-data wiring.
- Further check categories beyond the six shipped checks.
- Dark mode polish beyond what the `brand`/`label-tertiary` tokens give for free — check it in the screenshot pass, don't redesign.
