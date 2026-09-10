# Session detail: a blue for the reading, orange for carry

Four tweaks to the Sessions detail view, from three Notate captures on 2026-09-10.
Branch `feat/sessions-detail-view-blue`, off `main` at 28822df9.

## What changes

| # | Where | Now | After |
|---|---|---|---|
| 1 | Context tab, composition bar + legend dots | Real Work black, Rewrite Waste orange, Carry grey | Real Work **blue**, Rewrite Waste **grey**, Carry **orange** |
| 2 | Cost tab, `$/MTOK` measure bar | brand orange | the same **blue** |
| 3 | Tools tab, "N tokens burned" figure | always brand orange | orange, or **red** when the wasted share is high |
| 4 | Cost tab layout | `gap-x-6` (no vertical gap) | `gap-6` |

Item 4 is the missing padding between the hero and Checks, spotted earlier and still
unfixed on `main`.

## Decisions already taken

- **Recolour in place, no reorder.** The row order stays Real Work / Rewrite Waste / Carry.
- **A new token,** rather than retuning `share-work`. `share-work` stays teal, so the pass
  wording in Checks is untouched.
- **Severity by share of startup context wasted,** not by absolute tokens.
- **The gap fix rides along** in this branch.

## The blue

`hsl(191.5 83% 36.8%)` light, `hsl(192 63% 47.6%)` dark — the value Marty defined as
`burn-check-pass-fill` on `feat/desktop-pr4-burn-check-design`. That branch's PR (#441) is
closed unmerged, so the token does not exist on `main` and we add our own.

Proposed name: **`measure`**, in the session-analysis sub-palette. It covers both uses — the
Real Work slice and the cost measure bar are each "the reading", drawn calmly, with the verdict
left to the words beside them. Alternative if `measure` reads too abstract: `share-work-blue`,
though that invites confusion with the teal `share-work`.

## File by file

**`src/styles/session-analysis-colors.css`** — a token needs all four blocks: the `@theme`
declaration, the light `:root`, the `@media (prefers-color-scheme: dark)` branch, and the
explicit `:root[data-theme="dark"]` branch.

**`design.md`** — add the `measure:` entry, and amend the Session Detail colour sentence at
line 473. See the risk below.

**`src/components/session/analysis/EfficiencyBreakdown.tsx`**

- `SHARE_ROWS` (line 80): `bg-label` → `bg-measure`, `bg-brand-tint` → `bg-share-carry`,
  `bg-share-carry` → `bg-brand-tint`. One `inkClassName` drives both the bar run and the
  legend dot, so each row changes once.
- The comment above `SHARE_ROWS` explains the current colour reasoning and needs rewriting.
- `CostScaleBar` (line 187): the `cost-measure` span goes `bg-brand-tint` → `bg-measure`.

**`src/components/session/SessionDetailPresentation.tsx`**

- Line 900: `gap-x-6` → `gap-6`.
- Line 925: the figure's `text-brand` becomes conditional. `skillMcpUsage` already returns
  both `wastedTokens` and `totalTokens`, so the share is a division, no new derivation.
  Red uses `text-system-red-text`, the palette's red for words.

**Tests** — `EfficiencyBreakdown.test.tsx` asserts the three run colours at lines 109-111 and
needs updating. Worth adding: one case for the cost measure's colour, one for the Tools figure
crossing the red threshold.

## Where red starts

Settled in review on 2026-09-10. The figure turns red when **both** hold:

- at least **50%** of the startup context went unused, and
- at least **10k tokens** were burned.

Either one alone leaves it brand orange. The share stops a large context from going red on a
small proportion; the floor stops a tiny context from going red on a handful of tokens. Both
numbers live in named constants beside the Tools hero, so retuning is one edit.

The 10k floor is my pick, not Keith's — he asked for a floor without naming a figure. It sits
at roughly a third of the 23.5k in the capture that prompted this, so a session like that still
reaches red on merit. Raise it if red turns out to be common.

## The one risk

`design.md` line 473 currently reads: "brand orange for a compaction, teal for real work, red
for waste." After this change orange means both a compaction mark and the Carry slice, and
waste is no longer the red end of the composition. The doc sentence has to change to match, and
CI's `check-design-drift.mjs` enforces that it does. Worth a look before I build, since it
loosens a rule the rest of the view leans on.

## Verify

Run the app, open a session with all three tabs populated, check light and dark. Then
`pnpm --filter @antiburn/desktop lint`, `type-check`, and `test`.

## Status

| Step | State |
|---|---|
| Plan agreed | done |
| Token added | done — `measure`, all four blocks, plus the `design.md` entry |
| Composition + cost bar recoloured | done |
| Tools severity | done — `WASTED_RED_SHARE` 0.5, `WASTED_RED_TOKENS` 10k |
| Gap fix | done |
| Tests updated | done — 1288 tests pass; lint, type-check, design-drift clean |
| Checked in the running app | done — harness render, light and dark |

## What the render showed

The blue lands in both themes: `rgb(16 142 172)` light, `rgb(45 167 198)` dark, on the cost
measure and the Real Work run alike. Rewrite Waste reads as a mid neutral, Carry as brand
orange.

One finding worth a decision. In **light** mode the two verdict inks are nearly the same
colour: `text-system-red-text` is `rgb(190 0 20)` and `text-brand` resolves to `rgb(191 63 8)`,
a dark rust. Identical lightness, 23° of hue apart. Side by side you can tell them apart; in
the app you only ever see one figure, with nothing to compare it to, so "orange vs red" may not
read as a signal at all. Dark mode is fine — `rgb(255 138 128)` against `rgb(255 106 44)` is a
clearer split.

Options if the light case matters: use `system-red` (`hsl(354 100% 42.1%)`) instead of
`system-red-text` for more saturation, or carry the severity in a word beside the figure
rather than in the ink alone.
