# Session Detail: build the playground decisions

Source of truth: playground v11 (2026-09-04, eleven rounds in discuss). This plan
turns each decision Keith made there into a change in the desktop app, then runs a
general design pass over the whole Session Detail view.

## Status

| # | Change | Where | Status |
|---|---|---|---|
| 1 | New colour tokens, both themes, design.md in sync | `session-analysis-colors.css`, `design.md` | done |
| 2 | Remove the tab meter row ("mistake. remove") | `SessionDetailPresentation.tsx`, delete `SessionTabMeter.tsx` | done |
| 3 | Native segmented control for Context / Cost / Tools | `SessionDetailPresentation.tsx` | done |
| 4 | Context line and fill lit at rest, in blue | `ContextTokensChart.tsx` | done |
| 5 | Compaction marks on the chart, lit brand orange | `ContextTokensChart.tsx` | done |
| 6 | Rehydration yellow, routing miss pink | `ContextTokensChart.tsx` | done |
| 7 | Key entries as chips: hover tint, press, click to pin | `SessionDetailPresentation.tsx` | done |
| 8 | Compaction icon: chevrons, not the plus-looking fold | `SessionDetailPresentation.tsx` | done |
| 9 | Efficiency: 4px hairlines, grey at rest, colour on hover | `EfficiencyBreakdown.tsx` | done |
| 10 | Work = teal, waste = hot red, with darker text inks on light | efficiency, hygiene, tools | done |
| 11 | Tools tab: headline "N tokens burned" in brand orange, no meter | `SessionDetailPresentation.tsx` | done |
| 12 | Tests, verify set, laundry-list status | tests, `docs/plans/session-detail-laundry-list.md` | done |
| 13 | General design pass (ask on big decisions) | whole view | done (2026-09-04) |

## 1. Tokens

Every colour is a token in `session-analysis-colors.css` and `design.md`. Fill
inks and text inks are separate, like `brand` and `brand-tint`: the fill is the
playground value, the text ink is that value darkened 45% on light and the fill
value on dark. Dark fills lift 8% toward white.

| Token | Light | Dark | Was |
|---|---|---|---|
| `context-stroke` | blue `hsl(221.2 83% 53.3%)` | `hsl(221 89% 59.8%)` | brand orange |
| `context-fill-top` / `-base` | blue at 0.5 / 0.12 | blue at 0.42 / 0.1 | orange |
| `token-in` | cyan `hsl(189 86% 53.3%)` | `hsl(188.7 87% 58.2%)` | dusty cyan |
| `token-out` | violet `hsl(258.3 89% 66.2%)` | `hsl(258 89% 69.8%)` | deep blue |
| `mark-rehydration` (new) | yellow `hsl(45.5 96% 56.2%)` | `hsl(45.2 96% 60.1%)` | context-warning |
| `mark-routing-miss` (new) | pink `hsl(330.3 81% 60.3%)` | `hsl(330.6 82% 64.5%)` | context-warning |
| `mark-compaction` (new) | brand `hsl(17.6 100% 58.6%)` | same | not drawn |
| `share-work` | teal `hsl(173.4 80% 40%)` | `hsl(173.5 78% 46.4%)` | system green |
| `share-work-text` (new) | `hsl(174 80% 25.4%)` | same as dark fill | system green |
| `share-waste` | hot red `hsl(347.9 100% 56%)` | `hsl(348 100% 60.5%)` | warning yellow |
| `share-waste-text` (new) | `hsl(348 100% 34.5%)` | same as dark fill | system red text |

`context-warning` and `context-critical` stay for the heat ramp and the HUD. The
chart no longer uses them for marks.

## 2. Chart

- The context line and fill are lit at rest, in blue. Hover or pin on Context
  shows the warm ramp instead, so the depth reading is one interaction away.
- Compaction draws as a heavy stroke down the drop between the bucket before the
  boundary and the boundary bucket. Rest grey, lit brand orange. New series
  `"compaction"`.
- Rehydration marks light yellow, routing misses light pink. Tooltip lines take
  the same inks.

## 3. Key chips

Each key entry is a pill chip. Hover tints it in its own colour and lights its
layer. Click pins it, so the layer stays lit when the pointer leaves. Clicking a
pinned chip unpins it. Compactions get `series: "compaction"` and the
`ChevronsDownUp` icon.

## 4. Efficiency block

Track and composition are 4px. At rest, every run draws in the chart rest greys
and the band words in `text-label`. Hovering anywhere in the block restores the
colours. Band words use the text inks (teal good, red bad, label for ok).

## 5. Verdict inks elsewhere

Hygiene check words and icons: passed = teal text ink, failing = red text ink.
Tools status words: the same pair. The heat steps for unused tools collapse to
one red text ink, because red is the verdict and the token count beside it
carries the size.

## 6. Tools tab

The finding becomes a headline: the wasted-token count in `type-large-title`,
brand orange, tabular, with the caption "tokens burned / by items the session
never used" beside it. No meter, because the number has no ceiling.

## 7. Design pass

After the build, walk the whole view in both themes at 380px and fix what reads
badly: alignment, spacing, ink hierarchy, empty states. Big decisions (anything
that removes a section, changes what a tab holds, or touches the HUD) go to
Keith as a question first.

Done in the pass (2026-09-04):

- The detail tabs use a new `native-tabs` `SegmentedControl` variant: the
  macOS segmented look, a recessed neutral track with the selected segment
  raised on the surface. The default `segmented` variant paints accent blue
  and is shared with the settings panes, so it stays as it is.
- The chart key is a stat grid: three equal columns, each cell a colour
  swatch, the figure in the label ink, and a caption. It replaced the
  coloured pills (solid, then outlined), which read as six badges and left
  the yellow rehydration ink low on contrast in light.
- The chart axes moved outside the plot: token bands on the left, elapsed
  time under the plot, both in caption grey. The "rehydration" tag only
  draws while that layer is lit. The resting greys are fainter.
- The efficiency block carries its colours at rest (teal work, red waste,
  neutral carry) and the band word is a caption tag.
- Spacing rhythm: the hero flows into the tab strip with no hairline, the
  title steps down to body-large semibold, the repo line and the models
  line take one weight and one face.
- No section was removed and no tab changed what it holds, so nothing needed
  a question.
- Notate round two (2026-09-04): the chart is lit in full colour at rest and the key isolates one layer; the $/MTok bar is a hatched ruler with $0/$33/$80/$160 ticks; check words read "Passed"/"Failed" with a neutral passing word; Checks→Cost gap widened; Tools headline gets breathing room and states its period ("loaded but never called in this session").
- The $/MTok scale moved to the Cost tab under an Efficiency heading, redrawn as a bullet graph per the proto pick (grey bands, brand measure, target at the good edge, word and dollar range under each band). The work/rewrite/carry composition stays under the context chart, which now fills the tab's height (2026-09-04).
- The $/MTok reading prints its guidance inline under the scale on the Cost tab (what it measures, the profile thresholds, what to change) instead of a tooltip, and the Efficiency section sits at the foot of the tab, so a short tab keeps its slack between the cost rows and the scale rather than after it. The composition rows keep their tooltips (2026-09-04).
