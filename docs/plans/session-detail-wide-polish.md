# Session Detail (main window): five pins from 2026-09-09

**Branch:** `feat/session-detail-polish` · **Worktree:** `.claude/worktrees/feat-session-detail-polish` · **Status:** approved and built 2026-09-09; lint, type-check, tests, and design-drift check green; awaiting Keith's in-app look. No PR until asked.

Source: four Notate stills of the wide session detail pane (Context, Cost ×2, Tools tabs) taken 10:41–10:44 AEST, plus one follow-up from the first in-app look (row 6). The Notate stills came from a 1506×861 window; the follow-up is what the default 1100×600 window showed. Everything below is the **wide** layout only. The popover layout does not change.

| # | Pin | Where it lives today | Fix | Status |
|---|-----|----------------------|-----|--------|
| 1 | "Let's go blue for selected state" | `session-detail.css` overrides the tab pill: selected = white surface + raised shadow | Selected segment takes `bg-accent-fill` + white label, the same blue the session list's `$ / % week / % 5h` picker already uses | built |
| 2 | "Hover this centre bottom of the view, in context" | `SessionDetailPresentation.tsx:912` — the Context/Cost/Tools control sits in the toolbar between the title and the actions | Move it out of the toolbar to a floating pill anchored at the bottom‑centre of the tab panel, over the content | built |
| 3 | "Move this text into tooltip" (burn‑check descriptions) | `HygieneBreakdown.tsx` `InlineHygieneRow` prints summary + guidance under every check name | Row shows name + verdict only; the copy moves into a hover tooltip on the row | built |
| 4 | "What is this vert line here?" + "Let's put this in tooltip" | `EfficiencyBreakdown.tsx` `CostScaleBar` draws `cost-target`, a 2px `bg-label` line at the good/ok edge; `CostRowLine` prints the guidance paragraph under the scale | Remove the line. Move the paragraph into a tooltip on the `$16.86 per million tokens` hero | built |
| 5 | "Should margin/inset be the same on the toolbar as the content?" | Toolbar is `px-6` (24px); the tab panel is `px-10` (40px) | Yes, same inset. Toolbar goes to `px-10` so the title's left edge lines up with the content below it | built |
| 6 | Keith, on the first in-app look at 1100×600: "dollars are all low, checks have huge vert space, and line chart at bottom gets pushed below the fold" | `session-detail.css` gates the cost card's two columns and the checks grid's two columns behind `@container (min-width: 40rem)`; the cost pane of the default window is ~32rem, so both fall to one column and push the Efficiency section past the fold | Breakpoint drops to 30rem so the default window gets both two-column layouts (the 1000px minimum still stacks). Check rows tighten to `py-1` + `gap-y-0.5`, the same rhythm as the popover's check rows | built |

## 1 — Blue selected tab

`session-detail.css` has a wide‑only override block for `.session-detail-tabs button[aria-selected="true"]` that paints the selected segment on `--color-surface` with `--shadow-raised`. Change that rule to `--color-accent-fill` with white ink, and set the unselected segments to `text-label-secondary` (already the case). Nothing changes in `SegmentedControl.tsx`; the `native-tabs` variant stays quiet everywhere else.

Result matches the list header's picker exactly: recessed grey track, blue raised segment, white label.

## 2 — Floating switcher at the bottom

Today's toolbar row is `[back?] [summary: repo / title / meta] [Context Cost Tools] [open, delete]`. The switcher competes with the title.

New arrangement:

- **Toolbar:** `[back?] [summary] [open, delete]`. Nothing else changes in it (other than the inset, see §5).
- **Tab panel** becomes `relative`. A new wrapper sits `absolute inset-x-0 bottom-0` with `flex justify-center` and `pointer-events-none`, and the pill inside is `pointer-events-auto`. The pill keeps the same `SegmentedControl` instance (same `idPrefix`, `semantics="tabs"`, `aria-controls`) so keyboard and a11y contracts hold.
- **Pill material:** same recipe as the toolbar — `bg-surface/80` + `backdrop-filter: blur(var(--space-lg)) saturate(120%)`, `rounded-full`, `shadow-raised`. It reads as a floating control, not a stripe. The reduced‑transparency media query falls back to `--color-surface-window` like the toolbar does.
- **Content clearance:** the scroll container's bottom padding grows from `pb-10` to `pb-10 + pill height + var(--space-md)` so the last row on every tab can scroll clear of the pill.
- The popover (`!wide`) keeps its existing top switcher. No change.

Tests: `SessionDetailPresentation.test.tsx` finds the tabs by role, not position, so the existing assertions hold. Add one check that the tablist is a descendant of the tab panel's wrapper in the wide layout.

## 3 — Burn‑check copy into tooltips

`InlineHygieneRow` currently prints `summary + guidance` as a paragraph under the name. Change it to:

- Row: `[name] [✓ Passed]` in one line, hoverable (`ROW_CLASS`‑style hover wash so it looks touchable).
- Tooltip (`<Tooltip label={…} delayMs={150}>`) wrapping the row: summary in `text-label`, guidance sentences in `text-label-secondary`, finding details in `text-share-waste-text` when present. This is the same shape `ShareRowLine` already uses for the composition rows, so the pattern is proven at this text length.
- The `session-checks-grid` two‑column layout stays; the rows are now ~1 line each so the block drops from ~600px to ~150px on the Cost tab.

**Keith's question — "Is there a popover in Tauri/react?"** Tauri's window is a normal webview (WebKit on macOS), so any React overlay works. The app already ships `@radix-ui/react-tooltip` and wraps it in `components/presentation/Tooltip.tsx`; that's what everything above uses. Radix also has `react-popover` (click‑to‑open, stays open, can hold interactive content) if we ever want richer bodies, but it isn't in `package.json` yet and tooltips cover this case, so this plan does not add it.

Tests: `HygieneBreakdown.test.tsx:46` renders `inlineGuidance` and asserts on the paragraph text. Update it to hover the row and assert the tooltip body.

## 4 — The vertical line, and the cost paragraph

**What the line is.** `CostScaleBar` draws three grey bands at fixed thirds (good / ok / high), the orange measure from zero to this session's reading, and then a 2px dark line at the edge of the good band (`data-testid="cost-target"`). The code comment calls it the target marker. But the band step and the `under $33 / $33 – $80 / over $80` labels already mark that edge, so the line restates it and reads as a stray tick. Remove it and its test (`EfficiencyBreakdown.test.tsx:67`).

**The paragraph.** `CostRowLine` prints `MetricGuidance inline` under the scale. Move it into a `Tooltip` on the `$16.86 per million tokens / of context growth and output` hero block, using the non‑inline `MetricGuidance` rendering (which already exists for the popover's share rows). Delete the `cost-guidance` block and the `mt-3` gap. Update the comment on `CostRowLine` that says this metric "has the room on its own tab to explain itself without a tooltip" — that reasoning no longer holds.

Tests: `EfficiencyBreakdown.test.tsx:170–208` assert on `cost-guidance`. Replace with a hover‑and‑read‑tooltip assertion.

## 5 — Toolbar inset

Toolbar `px-6` → `px-10`. That's the whole change. The title, meta line, and repo path then share a left edge with the `$1.85` cost card, the checks grid, and the `21.1k` hero on Tools. The actions on the right also gain 16px of breathing room from the window edge, which they could use.

I'm not taking the "make it look more like a toolbar" branch. After §2 removes the switcher, the toolbar is just title + two icon buttons on a blurred material with a hairline. That already reads as a toolbar; the inset was the only thing making it look mis‑set.

## Out of scope

- Popover (`!wide`) layout. Untouched.
- Any change to `SegmentedControl.tsx` variants or `design.md` tokens. §1 is a stylesheet‑only change to a rule that already exists in `session-detail.css`, so no new token, no new stylesheet, no `design.md` edit.
- The stale 900×600 numbers in `docs/runbooks/main-window.md` (separate task chip already raised).

## Order of work

1. §5 inset and §1 blue (small, independent, both CSS/className only).
2. §4 line removal + cost tooltip.
3. §3 checks tooltips.
4. §2 floating switcher (largest; touches layout and a test).
5. `pnpm --filter @antiburn/desktop lint && type-check && test && format`, then `scripts/check-design-drift.mjs`.
6. Commit with `-s`, push branch, screenshot each of the three tabs for Keith. **No PR** until Keith asks.
