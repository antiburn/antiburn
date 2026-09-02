# Session Detail: visual clean-up and polish

**Branch:** `feat/session-details-visual-polish` · **Status:** built 2026-09-01, then revised the same day against Keith's six in-app pins (see [In-app review round 1](#in-app-review-round-1--2026-09-01)); all checks green; awaiting his second look, then split into the stacked PRs

## What's wrong today

Keith's read, confirmed by a code survey of `SessionDetailPresentation.tsx` (805 lines) and its five analysis cards:

1. **The top section has no grid.** Nine different kinds of item (vendor icon, repo path, cost pill, prev/next chevrons, model list, active duration, relative time, WSL badge, multi-line title in a dashed box) sit across three ad-hoc flex rows. Nothing aligns with anything.
2. **Density is wrong for a menubar app.** Five cards (Efficiency, Burn Checks, Context, Cost, Skills/MCPs) in one long scroll, every card open all the time. A menubar surface should answer "how is this session doing?" in one glance, with depth on demand.
3. **Text is too small.** Card metadata sits at `type-footnote` (11px) and `type-caption` (11px); the chart axis labels are 9px inline (`ContextTokensChart.tsx:44`), below the design system's own 11px floor. The title — the most human piece of content — renders at an off-token `text-sm` inside a dashed placeholder box.
4. **Colors are muted and off-brand.** The chart runs blue/violet/emerald at 22% fill opacity; Efficiency uses a _different_ palette again (`system-blue/indigo/gold/green/orange` at 50%). The brand orange `#FF6A2C` (tokens `brand`, `brand-tint` already exist) appears only on the high-cost flame pill. Nothing looks like the fire-poster brand.

## Goals

- One glance answers "is this session healthy and what did it cost" — bigger type, fewer simultaneous elements.
- A real grid in the header: things of the same kind align; things of different kinds separate.
- One palette, led by brand orange, shared between the chart, efficiency bars, and cost rows.
- Everything on-token. The redesign clears the existing off-token debt instead of adding to it.

## Proposed IA

### Header becomes a hero (always visible)

Replace rows A/B/C (`SessionDetailPresentation.tsx:633-742`) with a structured hero:

- **Line 1 — identity:** session title, promoted to `type-title-3` (15/600), 2-line clamp, flush left. No dashed box, and **no vendor/model logo mark** (decided in proto v5 review — "model logo not needed").
- **Line 2 — place:** repo path + branch + WSL badge in **monospace** (`ui-monospace` stack, 11px, `text-label-secondary`) — decided in proto v5 review.
- **Line 3 — stat strip:** a **4-cell** grid: **cost**, **active time**, **models**, **last activity**. Caption label over value, shared baseline. All four values render at the **same size** (13.5px in the proto scale); cost differentiates by **mono face + brand color only**, never by size (decided in proto v5 review — "dont make cost a different size").
- Prev/next chevrons and actions (reveal/export/delete/fork) stay in the sticky title bar where they already live — they are navigation chrome, not session facts.

### Body becomes tabs

Adopt the suspicion: tab the five cards. `SegmentedControl` already ships `semantics="tabs"` + `variant="text-tabs"` with an animated indicator and reduced-motion fallback — built, documented in design.md (`segmented-indicator` recipe), and used by no caller yet. This is the reuse-not-build option.

**The tab bar lives at the bottom of the popover** as a segmented control (decided 2026-09-01), pinned below the scrolling content — iOS tab-bar placement, not web-page top tabs.

**The popover does not change size when a tab is selected.** It sizes once to the largest tab's content and holds that height across tab switches (decided 2026-09-01). Shorter tabs get breathing room, not a jumping window.

Tabs (4, not 5) — Burn Checks moved out of Overview in the in-app review:

| Tab          | Contents                                | Why together                                                  |
| ------------ | --------------------------------------- | ------------------------------------------------------------- |
| **Overview** | Context chart + Efficiency thermometer  | The health story: one chart, one score                        |
| **Checks**   | Burn Checks, every check shown          | A list that deserves its own room, not a rollup in a corner   |
| **Cost**     | Cost table + sub-agent split            | The money story                                               |
| **Tools**    | Skills, MCPs and tools table            | The "what burned tokens for nothing" story                    |

Default tab: Overview. Keyboard ←/→ stays bound to prev/next _session_ (existing behavior); tabs switch by click and by the SegmentedControl's own arrow-key handling when focused.

**Decided in review (2026-09-01): tabs.** The no-tabs collapse alternative is off the table; the proto still lets us tune tab count and what lives in Overview.

### Color: one brand-led analysis palette

Rework `src/styles/session-analysis-colors.css` so the chart tells the brand story. **Settled palette: "ember"** (proto v6) — brand orange leads, companions stay quiet:

- **Context area** → hot-orange stroke (`--color-context-stroke`, the brand tint) over a **grey** area fill, not an orange one (revised in the in-app review). The line carries the heat; the fill stays out of the way of what sits under it.
- **Token in / out / sub-agent series** → a **cool violet-to-indigo** secondary, at 0.3 fill opacity so the hue survives the grey wash painted over it (revised in the in-app review).
- **Efficiency's share bars run the hot ramp instead** — brand orange, red, amber, all at full strength. The chart is cool and the bars are hot, so the two surfaces never trade meanings. The parallel `system-*` palette is still dead.
- **Orange loudness: hero** (proto Q — decided). Brand orange appears in the hero cost stat _and_ the chart. Everything else is neutral.
- **Cost and Tools tabs are greyscale only** — no red, no brand accents in their tables (decided 2026-09-01). Waste/cache rows differentiate by `label-tertiary`, weight, and indentation.
- **Warning/critical heat ramp** stays amber→red but ramps _from_ the brand hue so it reads as one system.
- Both light and dark values defined per token, per design.md contract; `design.md` updated in the same change.

### Chart and Efficiency: content is frozen

- The Context chart keeps **every measurement the shipping chart has**: token-in, token-out, and sub-agent series; the 200k/100k context bands; rewrite bars; plan-mode bands; mode labels; compaction markers. This pass restyles, it does not remove.
- The Efficiency card keeps **exactly the shipping metrics — no more, no less, no change**: $/MTok thermometer, Real Work %, Rewrite Waste %, Carry %. No invented categories, no added explainer copy.
- The Tools table must handle a **realistically long list** (design for the long case, not the 3-row demo case).

### Typography pass

- Nothing interactive or data-bearing below `type-callout` (12px); captions only for labels.
- Chart axis labels move from 9px inline to `type-caption` via CSS var.
- Weight overrides always paired with a `type-*` class (per design.md), replacing loose `font-bold` / `font-semibold!` / `text-sm`.

### Density (decided in review: in scope)

Keith's call: the whole view is crammed; fix the base density in this pass, not separately. Tabs make this affordable — with one tab of content visible at a time, looser spacing doesn't make the popover feel endless.

- **Padding settled at 12px** (proto v6 `pad: 12`): the win comes from tabs and flatter chrome, not fatter cards. Keep `p-3`-scale padding; remove nested card-in-card chrome instead ("less web page, more typography" — proto review). Sections separate by type hierarchy and hairline separators, not stacked boxes.
- **Type scale: bumped** (proto v6 `typescale: bumped`) — values at 13.5px, nothing data-bearing below 12px.
- Rows inside Efficiency, Burn Checks, and Cost get taller line boxes instead of the current magic-pixel heights.
- The hero gets clear vertical rhythm on the 4px spacing scale — separation between identity, place, and stat strip.

### Token hygiene (in scope, same PR series)

Fix the survey's off-token list while touching these files: raw `--color-bg-secondary`/`--color-border` vars, `border-1`, `text-sm`, `rounded-xl/lg/md` → `rounded-popover`/`rounded-control`, magic `h-[7px]`/`leading-[13px]`, the stray `dark:bg-separator`, and fold the local `Card` shell in `SessionDetailPresentation.tsx:246-270` into `src/components/ui/Card.tsx` if they can share.

## Process

1. **This doc** reviewed via /discuss — settle IA (tabs vs. collapse), hero contents, palette direction.
2. **/proto** — one HTML playground with the hero + tabbed body, controls for: tab grouping (what lives in Overview), stat-strip cell count, chart palette variants (2-3 orange treatments), type sizes, and a padding-scale slider (12/16/20) for the density decision.
3. **Implement** in stacked PRs, each ≲1,000 lines:
   - PR 1: palette + token hygiene (mechanical, low-risk, unblocks screenshots)
   - PR 2: hero header
   - PR 3: tabs + card regrouping + density
4. Tests updated per PR (`SessionDetailPresentation.test.tsx` is 657 lines and asserts structure; expect meaningful churn in PR 2-3).

## Open questions — all closed

1. ~~Tabs or collapsed cards?~~ **Decided: tabs**, bottom-of-window segmented control (review 2026-09-01).
2. ~~How loud should the orange be?~~ **Decided: hero** — orange in the hero cost stat and the chart; Cost/Tools tabs stay greyscale (proto v6, 2026-09-01).
3. ~~3 or 4 stat cells?~~ **Decided: 4** — cost, active time, models, last activity, all the same value size (proto v6, 2026-09-01).
4. ~~Raise the whole popover's base density in this pass?~~ **Decided: yes, in scope** — but via flatter chrome at 12px padding, not fatter cards (proto v6).

## Proto record

Settled in `playground-v6.html` (scratchpad, session 2026-09-01): `palette: ember · loudness: hero · chrome: flat · cells: 4 · pad: 12 · typescale: bumped`. Review closed with zero open threads.

## In-app review round 1 — 2026-09-01

Keith ran the built app and pinned six items (`notate-2026-09-01-18.20.43`). All six are implemented:

| # | Pin | Decision |
| - | --- | -------- |
| 1 | Hero "all too squished in" | More vertical rhythm: hero gap `1.5` → `2`, bottom padding `3` → `4`, stat-strip top margin `1` → `2`, cell label-to-value gap `0.5` → `1`. |
| 2 | Chart "go a hot orange, with grey inner fill" | New `--color-context-stroke` token carries the hot orange line. The area fill becomes a neutral from the label family. The stroke no longer borrows the token-series color, so the line and the series move independently. |
| 3 | Chart's token layer "choose a cool secondary…purple?" | Token in / out become violet and system-indigo; the sub-agent series stays a neutral. Fill opacity rises to 0.3 because the grey context fill paints over them. |
| 4 | Efficiency bars "these colours are bland" | The half-alpha fill was the cause: it flattened four different hues into one pastel. Bands and share bars now paint at full strength on a hot ramp — brand orange for real work, red for rewrite waste, amber for carry. |
| 5 | Burn Checks "don't put in compacted view. move checks to another tab" | Read as two instructions. Checks get their own tab, and `HygieneBreakdown` gains `collapsePassing` (default true) so that tab shows every check with the count as a plain line, not a toggle. |
| 6 | "use the same formatting as the Models etc… an appropriate number of text styles" | The Context section's hint string becomes stat cells in the hero's own label-over-value grid (`tokensCard` returns `stats`, `HeroStat` becomes the shared `StatCell`). The type ladder drops to four steps: `type-title-3` for the session title, `type-headline` for section titles and stat values, `type-callout` for every data row and prose, `type-caption` for labels only. This also lifts Efficiency, Burn Checks and Cost rows off 11px, which was the original "text is too small" complaint. |

Still open from the plan: taller line boxes in the remaining rows. (The long Tools list is settled in round 3: every row renders.)

## Refinement pass 2 — 2026-09-01

Keith asked for a second pass: lead with the brand orange, keep the other colors bold. Six refinements:

1. **Active tab inks brand orange.** The `raised-tabs` selected label goes `text-brand`, the way a platform tab bar marks the active view with the accent. The one always-visible piece of brand in the chrome. `design.md` gains `selectedInk` on the tab-bar entry.
2. **Context line at full weight.** The brand stroke goes 1.5px → 2px.
3. **Thermometer runs green → amber → red.** The middle band was `bg-separator` grey, which made the ramp read as two colors with a gap. It is now `bg-context-warning`.
4. **Red means bad, everywhere.** The "high" band word was `text-system-orange`; with orange as the brand it cannot also mean trouble. Bad → `text-system-red-text`, ok → `text-context-warning`.
5. **Carry recolors to the chart's violet.** `bg-system-orange-tint` sat 17° of hue from the brand orange, so Real Work and Carry read as the same bar. Carry is the cost of resending the context tokens the chart draws violet, so it takes `bg-token-in` — three shares, three distinct bold hues, each meaning what the chart says.
6. **The Context stat strip becomes the chart's legend.** Stat cells gain a `tone`: "In" inks the violet series color, "Out" the indigo, and the Rehydrations / Routing-misses counts ink `context-critical`, matching the red bars the chart draws for them. Compactions stay neutral. The hero cost keeps the brand tone through the same mechanism.

## In-app review round 3 — 2026-09-01 (`notate-2026-09-01-23.05.40`, 8 pins)

| Pin | Comment | Change |
| --- | --- | --- |
| 1 | Efficiency: "Take the layout from the usage meter" | `EfficiencyRowLine` adopts the `WindowMeterRow` silhouette: label left and figure right on one baseline, full-width bar underneath. The grid container becomes a flex column; the thermometer gets a fixed `h-2.5`. |
| 2 | In/Out colors: "try a variant of this, but keep the brand orange" | The cool series moves off violet/indigo to cyan/blue (`token-in` hsl 199°, `token-out` hsl 235°) — the complement of the flame, with 36° of internal separation. Carry's share bar follows automatically via `bg-token-in`. The orange chart line and hero cost stay. |
| 3 | Active tab: "orange is a bit too buzzy" | Selected raised-tab label dims to `text-brand/80`. Still the brand, no longer vibrating against the raised fill. |
| 4 | Burn Checks rows: "text too small" | Check rows and the rollup line go `type-callout` (12px) → `type-body` (13px). Guidance prose stays callout. |
| 5 | Cost rows: "text too small" | All Cost rows, member rows, the footer and the pill go `type-body`. |
| 6 | Tools: "just list them all" | `SkillsMcpChart` loses the collapse entirely — no `Show N more`, no expanded store. Every row always renders. `useSkillsMcpExpanded.ts` and its test are deleted as dead code; the collapse tests are replaced by one all-rows assertion. |
| 7 | Tools rows: "text too small" | Table rows go `type-caption` (11px) → `type-body` (13px). The header row stays caption. |
| 8 | Models hero stat: "find a way to not truncate this" | `StatCell` gains a `wrap` option (`break-words` instead of `truncate` on the value); the Models cell uses it, so long model lists wrap onto more lines. |

The type ladder settles at: `type-title-3` session title, `type-headline` section titles and stat values, `type-body` every data row, `type-callout` guidance prose and subtitles, `type-caption` labels and table headers.
