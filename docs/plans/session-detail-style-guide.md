# Session Detail style guide — phase 3

Round 4 feedback (annotated screenshot, 2026-09-01): across the four tabs there
are too many text styles and too many labels. The home screen — the usage bar
and the session list — is the density of styles to match. This plan defines the
style rules, then applies them to the Session Detail view.

## Audit: what each surface actually uses

**Home screen (the reference).** The usage meters use one type size
(`type-footnote`) for everything: provider name, window label, reset time, and
figure. Hierarchy comes from ink (`text-label` / `-secondary` / `-tertiary`),
weight, and an uppercase eyebrow — never from a size change. There is no
"Usage" heading; the bars explain themselves. One bar primitive
(`SegmentedMeter`: grey track, orange fill, elapsed notch) draws every meter.
Session rows: one title size, one metadata size, meaning carried by icons with
tooltips (fork glyphs) rather than text labels.

**Session Detail (current).** Six type styles (`title-3`, `headline`, `body`,
`callout`, `caption`, `footnote`), thirteen ink utilities, three unrelated bar
treatments (green–amber–red thermometer, dotted share segments, the chart), a
section heading over every block, and a caption label over every figure.

## The rules (to be added to `apps/desktop/design.md`)

1. **One data size per surface.** Every figure, row, and label of data in a tab
   uses `type-body`. Hierarchy comes from ink and weight, not from a smaller
   size. Size changes are reserved for the hero title (`type-title-3`).
2. **No heading that restates its content.** A token chart does not need
   "Context" above it; meters do not need "Efficiency". A heading survives only
   when the content is ambiguous without it.
3. **No label over a self-evident value.** An orange `$70.20` is a cost;
   `3h 41m` is a duration; `2m ago` is a time. Where identification is genuinely
   needed, use an icon with a tooltip (the session-row fork-glyph pattern), not
   a caption.
4. **One meter style.** Every horizontal bar uses the usage-meter silhouette:
   full-width grey track, single solid fill, optional notch. Judgment (good /
   ok / high) is carried by the band word's ink, never by a multi-color bar.
5. **Color only where it means something.** Brand orange = cost and real work;
   cyan/blue = the In/Out series; red = bad. Everything else greyscale.

Resulting type ladder for the detail view: `type-title-3` (hero title, once) →
`type-body` (all data) → `type-callout` (guidance prose only) → `type-caption`
(table column headers only). Four styles, down from six.

## Screen changes (from the annotated screenshot)

| # | Feedback | Change |
|---|---|---|
| 1 | "move prev, next controls down" | Move the newer/older session chevrons out of the top toolbar into the hero: flanking the title block, left and right. Toolbar keeps back, relations, reveal, export, delete. |
| 2 | "no need for titles" (hero stat labels) | Drop the caption labels over the hero stats. Cost / duration / last-activity values stand alone; labels move into each cell's tooltip and aria-label. |
| 3 | "give own line" (Models) | Models leaves the 4-column grid and takes its own full-width line under the stat row, in the session-row model style (`font-medium` names · muted thinking mode). Hero grid becomes 3 columns. |
| 4 | "no need" (Context title) | Remove the "Context" section heading. The stat strip and chart open the tab directly. |
| 5 | "do with icons for these 4, with tool tips" | Replace the In / Out / Compactions / Rehydrations caption labels with icons + tooltips: `ArrowDownToLine` (In), `ArrowUpFromLine` (Out), `FoldVertical` (Compactions), `RotateCcw` (Rehydrations, and `Repeat2` for routing misses if shown). Icon takes the cell's tone ink; value sits beside it at `type-body`. |
| 6 | "no need" (Efficiency title) | Remove the "Efficiency" heading and its subtitle. The per-metric explainer already lives in each row's expandable guidance. |
| 7 | "no need — use the single style like the usage bar" | One bar style for all four efficiency rows, matching `SegmentedMeter`: grey full-width track, single solid fill. Share rows fill by their share with their tone (brand / red / cyan). `$/MTok` shows the same track with a position notch instead of the green–amber–red ramp — the band word alone carries the judgment. |

Plus the systemic pass rules 1–5 imply:

- Burn Checks, Cost, and Tools keep `type-body` rows (done in round 3) but drop
  any remaining caption labels that restate content.
- **Tools becomes two-line cells (review round 4).** The 4-column table was
  cramming too much into one row. Each source gets the session-row silhouette:
  line 1 = name (`text-label`) with the tokens figure right-aligned; line 2,
  muted = kind · origin · status ("Skill · Project · Used ×2"), unused/deferred
  keeping its bold mark. Column headers go away with the columns. Proposed as
  plain rows with hover wash rather than boxed cards — cards mark tappable
  destinations on the home screen; these are readings. (Awaiting Keith's call
  on plain vs boxed.)
- Ink census target after the pass: `label` / `-secondary` / `-tertiary`, the
  three meaning colors (brand, in/out, red), and green + amber only in band
  words. Nothing else.

## Open questions — all decided 2026-09-02

- **Q1 — $/MTok bar**: keep the bar — grey track with a position notch, no
  color ramp.
- **Q2 — Efficiency subtitle**: cut entirely; explainers live in each row's
  expandable guidance.
- **Q3 — hero stat order**: Cost · Active · Last.
- **Tools cells**: plain two-line rows, and **no hover wash** — they are not
  interactive.
- **Q4 — checks/cost/tools headings**: ~~"Burn Checks", "Cost", and "Skills,
  MCPs and tools" headings also restate their tab's name. Drop them too?~~
  **Decided (review round 4): drop them.** On Checks, also remove the rollup
  words ("0/4 passing", "Not assessed") — each row already carries its own
  pass/fail mark. The wasted-tokens subtitle on Tools moves to a footnote under
  the table. Checks gains a quiet explainer block at the bottom of the tab
  (below a hairline): one or two `type-callout` sentences per check saying what
  it tests and its token impact — cliff-notes length, no heading. Cost gets the
  same treatment: one short bottom line on cache-costing subtleties (cache
  reads ≈10% of fresh-input price, writes ≈25% more), absorbing the existing
  "On subscription…" subtitle. Copy drafts land in this plan before
  implementation.

## Explainer copy drafts (round-4 asks — for review before build)

Checks tab, bottom block. One entry per check, bold lead word, `type-callout`
muted:

- **Session overdepth.** Past ~200k tokens, every turn resends the whole
  history as cache reads. Deep sessions cost more per message than fresh ones.
- **Model overthinking.** High reasoning effort spends extra output tokens on
  every reply. Most tasks do fine on a lower setting.
- **Overpowered subagents.** Subagents inherit the big model for fetch-and-carry
  work. Routine lookups on a smaller model cost a fraction.
- **Obsolete model.** Newer models do the same work better, usually at the same
  or lower price.
- **Fast mode overuse.** Fast mode trades a higher token rate for speed. Keep
  it for bursts, not as the default.
- **Excess cache rehydration.** When the cache expires mid-session, the next
  turn rewrites the whole context at full price. Long idle gaps are the usual
  cause.

Cost tab, bottom line:

> Cache reads bill at about 10% of fresh input; cache writes about 25% more.
> On subscription these figures are estimates at API prices.

## Status

| Step | State |
|---|---|
| Audit + rules drafted | done |
| Round-4 review decisions folded in (headings, explainers, Tools cells) | done |
| Explainer copy drafted for review | done |
| design.md rules section | done |
| Hero: chevrons down, labels dropped, Models own line | done |
| Context: heading gone, icon stats | done |
| Efficiency: heading gone, single bar style | done |
| Systemic ink/label pass over Checks/Cost/Tools | done |
| Verify: prettier, type-check, lint, tests, drift check | done (1009/1009 tests, drift in sync) |
| Round 5: Tools cells, Checks hover helper, padding, hero circles, chart stagger + gradients | done |
| Round 6: rewrite-label dedup, token series over context fill | done |
| Round 6: efficiency LEDs per band + tinted off state, guidance to tooltips, typo fixes | done |
| Round 6: Tools unused-status severity colors | done |
| Round 6: Cost + Checks merged into Usage tab with shared hover helper | done |
| Round 6: peek (anchor) window usage cards restyled (radius, padding) | done |
| Verify round 6: prettier, type-check, lint, tests, drift check | done (1011/1011 tests, drift in sync) |
| Round 7: meters redrawn as VU meters — zones by position, not one band color | done |
| Round 7: usage meters orange / yellow at 80% / red at 90%; new vivid fill tokens | done |
| Round 7: efficiency meters on their own band edges, in blue / purple / red | done |
| Round 7: chart token series lifted to 0.9→0.18 with a hairline per layer | done |
| Round 7: context line finer (2 → 1.5) | done |
| Round 7: hero price on the hot brand orange | done |
| Round 7: Tools "Used ×20" and above reads red | done |
| Round 7: tab strip white on hot orange, full-round, same in both themes | done |
| Verify round 7: prettier, type-check, lint, tests, drift check | done (1012/1012 tests, drift in sync) |
| Round 8: efficiency meters back on the shared orange / yellow / red palette | done |
| Round 8: chart token series as solid fills, no gradient, no stroke | done |
| Round 8: chart tooltip animation off, so it stops replaying its entry | done |
| Verify round 8: prettier, type-check, lint, tests, drift check | done (1012/1012 tests, drift in sync) |
| Round 9: light-mode context fill in solid brand orange, dark stays grey | done |
| Round 9: chart labels sit on a half-strength pill, opposite the surface | done |
| Round 9: VU meter unlit segments halved again (tint/25 → tint/12) | done |
| Round 9: session rows and usage-meter hover on a half-strength neutral wash | done |
| Verify round 9: prettier, type-check, lint, tests, drift check | done (1012/1012 tests, drift in sync) |
| Round 10: Real Work meter fills from the right down to the reading | done |
| Round 10: zones always left to right — orange at the good end of every meter | done |
| Round 10: unlit LEDs a quarter less saturated, on their own tokens | done |
| Round 10: chart rewrite bars in the warning yellow, not red | done |
| Verify round 10: prettier, type-check, lint, tests, drift check | done (1013/1013 tests, drift in sync) |
| Round 11: chart plays in as a sequence — context, tokens, then rewrite marks | done |
| Round 11: rewrite bars white in both themes, on their own token | done |
| Round 11: meter warning zone tried in magenta, reverted to yellow | done |
| Verify round 11: prettier, type-check, lint, tests, drift check | done (1013/1013 tests, drift in sync) |
| Round 12: rewrite bars at full opacity, routing misses at 0.4 | done |
| Round 12: Tools name in semibold, so the row leads with its subject | done |
| Verify round 12: prettier, type-check, lint, tests, drift check | done (1013/1013 tests, drift in sync) |
| Round 13: one point less between a row's status line and its title | done |
| Verify round 13: prettier, type-check, lint, tests | done (1013/1013 tests) |
| Round 14: burn-check verdict ink mixes the vivid tints, not the text tones | done |
| Round 14: efficiency tooltips on the chart tooltip's type and ink | done |
| Round 14: platform tooltips on the surface token, not two named greys | done |
| Verify round 14: prettier, type-check, lint, tests, drift check | done (1013/1013 tests, drift in sync) |
| Round 15: chart text labels render after the plot layers, so none sits under an area | done |
| Round 15: rewrite bars at 0.9 opacity | done |
| Verify round 15: prettier, type-check, lint, tests, drift check | done (1014/1014 tests, drift in sync) |
| Round 16: the three shares redrawn as one composition track (proto variant D) | done |
| Round 16: each share keeps an identity color; the band word carries the judgment | done |
| Round 16: $/MTok keeps its LED VU meter, outside the composition | done |
| Verify round 16: prettier, type-check, lint, tests, drift check | done (1014/1014 tests, drift in sync) |
| Round 16 revision: composition runs in a very light grey, the warning yellow, and a mid grey | done |
| Round 16 revision: guidance goes back into tooltips, so the block keeps the height of its readings | done |
| Verify round 16 revision: prettier, type-check, lint, tests, drift check | done (1015/1015 tests, drift in sync) |
| Round 17: hero newer/older session chevrons removed; arrow-key traversal kept | done |
| Verify round 17: type-check, lint, tests, drift check | done (1054/1054 tests, drift in sync) |
