# Session Detail: the laundry list

The polish work has spread across three rounds and two docs. This is the single
running list of what is still wrong, so we can cull it, order it, and stop
rediscovering the same items.

Comment on any item. Kill the ones you disagree with. Add yours at the bottom.

Each item is tagged with who raised it — **[K]** Keith, **[C]** Claude — so it is
obvious which ones are real complaints and which are my guesses.

## Status

| # | Item | Raised | Size | Status |
|---|---|---|---|---|
| A1 | Four colour families fight on one surface | K | L | done (playground v11, 2026-09-04) |
| A2 | The chart body is 18% grey in dark mode | C | S | done (round 6) |
| A3 | Orange means both "brand" and "alert" | C | M | partly (round 6) |
| A4 | Efficiency green/yellow is borrowed from nowhere | C | M | done (greyscale at rest, teal/red on hover) |
| A5 | Stat cells are tinted to match chart series | C | S | done (round 6) |
| B1 | The chart lost its main body — put the fill back | K | S | done (round 4) |
| B2 | The token stack and the context area fight | C | M | done (round 6) |
| B3 | Too many annotation layers on one plot | C | M | partly (round 6) |
| C1 | Tabs should borrow the $/% meter's language | K | L | done (round 5) |
| C2 | The stat strip and the tab bar do the same job twice | C | L | done (round 5) |
| D1 | Header stacks four unrelated kinds of thing | K | M | done (round 4) |
| D2 | Stat figures carry no visible label | C | S | done (round 4) |

---

## A. Colour

### A1. Four colour families fight on one surface — [K]

Right now `session-analysis-colors.css` carries all of these at once:

- hot brand orange (the context line)
- cool cyan and deep blue (the token stack)
- green, yellow, and grey (the efficiency shares)
- amber and red (the context heat ramp, and alerts)

Nothing ties them together. The surface reads as four charts from four apps.

**Proposal:** pick one family and derive everything from it. Brand orange leads,
one cool complement for the secondary layer, red reserved for alerts only, and
nothing else. Every hue that is not one of those three gets deleted or mapped
onto them.

### A2. The chart body is 18% grey in dark mode — [C]

This is the likely cause of B1. Before the restyle the context area was a solid
blue: `hsl(217.2 91% 59.8% / 0.6)` fading to `0.2`. Now dark mode fills with
neutral grey at `0.18` fading to `0.03` — near-invisible on a dark card.

Source: `apps/desktop/src/styles/session-analysis-colors.css`, and the diff at
`git diff 1bb192b7 HEAD -- apps/desktop/src/styles/session-analysis-colors.css`.

**Proposal:** give the fill back real weight in both themes. Whether it goes
back to blue or becomes a dimmed orange depends on A1.

### A3. Orange means both "brand" and "alert" — [C]

`--color-context-stroke` is brand orange. `--color-context-warning` is amber.
`--color-system-orange` is the error text in the empty state. Three oranges,
three meanings, one screen.

**Proposal:** brand orange is never a warning. Move the warning step to amber or
yellow and keep red for critical, so the heat ramp reads
brand → amber → red and never doubles back.

### A4. Efficiency green/yellow is borrowed from nowhere — [C]

`--color-share-work` is green, `--color-share-waste` is yellow, `--color-share-carry`
is grey. The comment claims this matches "the cost track beside it", but the cost
track is orange. Nothing else on the surface is green.

**Proposal:** re-cut the efficiency composition in the A1 family — brand for
work, a desaturated step for carry, and the alert hue for waste.

### A5. Stat cells are tinted to match chart series — [C]

`STAT_TONE_CLASS` in `SessionDetailPresentation.tsx` paints hero figures in
`text-token-in`, `text-token-out`, and `text-context-critical` so they read as
legend entries. In practice a header of four differently-coloured numbers reads
as four alert states.

**Proposal:** one ink for hero figures. If a figure needs to point at a chart
layer, use a swatch, not coloured type.

---

## B. The chart

### B1. The chart lost its main body — put the fill back — [K]

In the current build the chart does not show the main body it used to. The
context area was the thing you read first, and now it is not there.

Almost certainly A2 — the dark-mode fill went to 18% grey. Worth confirming
that's what you're seeing before we chase anything else.

**Proposal:** restore a solid, readable context area fill, then re-judge the
rest of the plot against it.

**Open:** is "main body" the context area fill, or the token stack underneath
it? They are different fixes.

### B2. The token stack and the context area fight — [C]

Two plot layers on two independent Y axes, drawn over each other. The context
area owns the left axis and the whole vertical range; the stacked token series
own a hidden right axis. The token blocks are drawn *after* the fill so they sit
on top of it.

The result is that neither layer has a clean silhouette.

**Proposal:** decide which layer is primary and give the other one less room —
either a fixed bottom band for the token stack, or drop it to a sparkline.

### B3. Too many annotation layers on one plot — [C]

The chart currently draws: band lines, band labels, rewrite bars, rewrite
labels, cache-event bars, routing-miss bars, time ticks, mode-change labels
(staggered across up to N rows), plus the tooltip. That is eight annotation
types over two data layers, in 220px.

**Proposal:** cut to the two or three annotations that change a decision. Move
the rest into the tooltip.

---

## C. Navigation

### C1. Tabs should borrow the $/% meter's language — [K]

The main view's usage bar is the good pattern in this app: a ring, a figure, and
a segmented meter that expands. It is informative *and* it is the control.

The Session Detail tab bar is the opposite — three plain text labels
(`Overview`, `Cost`, `Tools`) that tell you nothing until you click them.

**Proposal:** rebuild the tab strip as a row of meter cells, in the language of
`UsageLimitsBar` / `SegmentedMeter`. Each cell carries its own figure and its
own small meter, and selecting it opens that tab:

| Cell | Figure | Meter |
|---|---|---|
| Overview | context % of window | segmented, orange→yellow→red |
| Cost | `$4.12` | share of the session's spend |
| Tools | call count | share of calls that are tool calls |

The nav then answers "where should I look?" before you click anything.

**Open:** three cells or four? And does the selected cell keep its meter, or
does the meter belong only to the unselected ones?

### C2. The stat strip and the tab bar do the same job twice — [C]

The header already carries a three-cell stat grid (cost, active time, last
activity) directly above the tab bar. If C1 lands, those two rows merge into
one.

**Proposal:** treat C1 and C2 as one change. The meter-cell nav replaces the
stat strip rather than sitting under it.

---

## D. Header

### D1. The header stacks four unrelated kinds of thing — [K]

Top to bottom: repo path (mono, caption), title (title-3, up to two lines),
a three-cell stat grid, then model pills. Four kinds, four type treatments, no
shared grid.

**Proposal:** two zones. Identity (repo + title) and figures (the C1 meter
row). Model pills move into Overview, or onto the title line as a suffix.

### D2. Stat figures carry no visible label — [C]

`StatCell` puts the label in a tooltip and a screen-reader prefix only. A number
with no caption is only self-evident if you already know the layout — and `2m
14s` next to `4h ago` is not.

**Proposal:** if the meter-cell nav lands, each cell gets a visible name anyway,
and this resolves itself.

---

## E. Unsorted — add yours

Drop anything here and I'll sort it into the list above.

- 
- 
- 

---

## Open questions

1. **B1:** which layer is "the main body" — the context fill, or the token stack?
2. **C1:** what figure does each nav cell carry, and what does the meter measure?
3. **A1:** does brand orange lead the whole palette, or does it stay for the
   context line only while the data layers go cool?
4. Is this a fourth round on the existing branch, or does the meter-cell nav
   warrant its own branch and PR?
