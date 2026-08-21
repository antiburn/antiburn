# The usage meter, in the menubar popover

Port chunk 1a of `docs/plans/popover-overview-rollout.md` — the horizontal
limits bar and the segmented meters under it — from `proto/popover-overview`
onto `main`. One PR, roughly 730 lines added and 200 removed.

The prototype branch is 29 commits of whole-window redesign. This plan takes
one slice of it: the meter UI Keith named first, and nothing that depends on a
window that does not exist yet.

## What ships

The vertical `UsageLimitsSection` at the top of the popover becomes a
horizontal bar: one pill per provider with a mini radial and its worst
window's percentage, and a chart-icon button on the right that drops
per-provider segmented meters below.

```
 ( ✳ 84% )  ( ◎ 0% )                                    [chart icon]

 CLAUDE
 5-hour limit                                                   46%
 ●●●●●●●●●●●●●●○○○|○○○○○○○○○○○○○○
 Weekly limit                                                   50%
 ●●●●●●●●●●●●●●●●○○○|○○○○○○○○○○○○
 Fable weekly limit                                             84%
 ●●●●●●●●●●●●●●●●●●●●●●●●●|●●●●○○

 CODEX
 5-hour limit                                                    0%
 ○○○○○○○○○○○○○○○○○○○○○○○○○○○○○○○○
```

## What does not ship, and why

The prototype's `UsageLimitsBar` reaches into two later chunks. Both come out
in this port:

| Prototype behaviour | Chunk | Decision here |
| --- | --- | --- |
| Hovering a pill opens a floating peek window | 1b | **Removed.** `showUsagePeek`/`hideUsagePeek` do not exist in `lib/ipc.ts` on main, and the Rust window behind them is 266 lines of its own chunk. The rollout doc already says the meters work without a peek — the hover just does nothing. |
| Clicking a pill opens a standalone Usage window | 4 | **Kept as today's behaviour.** `onViewAll` stays wired to `session.setShowUsage(true)`, main's in-popover usage surface. When chunk 4 lands it swaps one call. |

Neither removal changes the file's shape, so 1b and 4 add their line back
rather than rewriting the component.

## Where main has already moved

Main is 58 commits past the prototype's fork point. Two of those matter:

1. **The degraded-provider work already landed** (`9eb3bdf`, chunk 5). Every
   `liveUnavailableProviders` / `liveUnavailableReason` / `liveErrorNote`
   import in `UsageLimitsBar.tsx` resolves against main unchanged. Nothing to do.
2. **`UsageLimitsSection.tsx` is byte-identical** between main and the
   prototype. It is not modified by this port — it is deleted at the end of it
   (see step 6), except for the `UsageLimitsSectionProps` type the bar reuses.

The three-dot diff (`main...proto`) overstates this port because it measures
from the fork point. The true surface is the two-dot diff.

## Steps

### 1. The brand orange, as a token

Ground rule 4 of the rollout doc. `#FF6A2C` appears as a bare Tailwind
arbitrary value six times across the three files this port touches, and eleven
more times in chunks not yet written. It becomes a token before the second
chunk copies the literal again.

Main has no brand-orange token. Its accent tokens are macOS blue
(`--color-accent-fill-val: rgb(0 122 255)`, `styles/tokens.css:122`) and its
orange tokens are macOS system orange, which is a different colour with
different semantics. So this is a new token in `styles/tokens.css`, with a
light and a dark branch — the prototype defines one literal and no dark
variant, which is a gap this port closes rather than carries.

**Decided (Keith, 2026-08-21): the orange changes between modes.** The split
is not the one the question assumed, though. Measured against the surfaces:

- `#FF6A2C` on white is **2.86:1** — below the 4.5:1 a caption needs and below
  the 3:1 a small icon needs. The prototype's `text-[#FF6A2C]` disclosure icon
  fails in light mode today.
- `#FF6A2C` on near-black is **7.35:1** — comfortable.

So it is **light mode** that needs a different value, not dark. Main already
solved this shape for system orange and the pattern is worth copying rather
than inventing: `--color-system-orange-val: rgb(179 81 0)` is the text-safe
one, `--color-system-orange-tint-val: rgb(255 149 0)` is the fill, and
`tokens.css:127-129` comments the second as "standard orange, for
fills/backgrounds".

Two brand tokens, then, each with a light and a dark branch:

| Token | Light | Dark | Used by |
| --- | --- | --- | --- |
| `--color-brand-tint-val` | `#FF6A2C` | `#FF6A2C` | Filled segments, the ring arc, hover washes |
| `--color-brand-val` | darkened, ~`rgb(199 66 10)` | `#FF6A2C` | The expanded chart icon, any figure or label |

Large filled shapes carry the brand mark unchanged in both modes — a segment
row and a ring arc are not text, and the 3:1 rule for graphical objects is met
against both surfaces. Only the small-and-inked uses take the light-mode
darkening.

The exact light-mode value is a design call, not an arithmetic one — the table
gives a starting point that clears 4.5:1, and it should be eyeballed against
the popover before it lands.

Then the six literals become the tokens:

| File:line | Today | Takes |
| --- | --- | --- |
| `UsageLimitsBar.tsx:94,140,178` | `hover:bg-[#FF6A2C]/[0.08]` | tint |
| `UsageLimitsBar.tsx:95` | `text-[#FF6A2C]` when expanded | brand |
| `UsageRing.tsx:114` | `stroke="#FF6A2C"` | tint |
| `SegmentedMeter.tsx:55` | `bg-[#FF6A2C]` for a filled segment | tint |

### 2. The two new primitives

`components/ui/SegmentedMeter.tsx` (70 lines) and
`components/ui/SegmentFigure.tsx` (30 lines). Both are new files with no
dependency on anything outside main.

Three rules in `SegmentedMeter` that survive review and must survive the port:

- **A `null` percent renders every segment empty at half strength.** That is
  visibly a meter with no reading, not a meter at zero — which would be a
  claim nobody made.
- **The `expectedFraction` notch** marks how far through the window's period
  the clock has travelled, so 60% used at 30% elapsed and 60% used at 90%
  elapsed stop looking alike. No fraction, no notch — never drawn from an
  assumption.
- **Segments span the full row width**, so the notch's percent offset and the
  track measure the same width. That is what keeps the notch honest.

`SegmentFigure` is a deliberate placeholder: every numeric readout funnels
through it so the LED/segment-display treatment can be applied in one file
when the visual reference turns up. It carries `tabular-nums` and no font
family — the 2026-08-21 annotation pass removed the monospace, because a
second typeface for a two-character percentage read as a different kind of
thing from its own label. Do not scatter `tabular-nums` around; keep the funnel.

Neither file has a test on the prototype. Both get one here (see step 5).

### 3. The bar

`components/providerUsage/UsageLimitsBar.tsx` (290 lines, new) plus
`LiveMetricRows.tsx` (44 lines, an extraction out of `LiveUsageDetail`, not
new behaviour), and small edits to `UsageRing.tsx`, `LiveUsageDetail.tsx`, and
`index.ts`.

Decisions worth keeping, all argued once already:

- The disclosure is a **chart icon with a pressed state**, not a rotating
  chevron. The control shows what it reveals; its fill shows whether the
  meters are open.
- The provider name survives as an **uppercase eyebrow** above its meters. It
  was removed in review pass 5 and put back in pass 6, because two providers
  with the same window label are otherwise indistinguishable.
- **The reset time is hover-only.** `WindowMeterRow` takes an opt-in
  `resetOnHover`; the row is a named Tailwind group (`group/meter`) so each
  meter reveals its own reset. The label fades with `opacity-0` and does not
  unmount — it stays in the accessibility tree, and the row keeps the space so
  the percentage does not jump.
- `UsageRing` loses its grey remainder track for a stated percentage: the
  caption beside the ring already says the number. The dashed indeterminate
  track stays, or the ring vanishes.

`UsageLimitsBar` imports `UsageLimitsSectionProps` and takes the same props as
the section it replaces, so the two stay swappable through the port.

### 4. The wording changes in `liveUsage.ts`

Two changes, +32/−13. Small, and each is a decision:

- **`liveResetLabel` reports a wall-clock time** (`resets 4pm`), not a
  countdown. A clock time stays true for as long as it is on screen; "in 2h"
  is wrong a minute after it renders.
- **The Today row says a share of the limit**, not "points". The shell reports
  percentage points of the window's allowance; "points" is the unit, not a
  word a reader knows. Below 10 it keeps a decimal so a small real share does
  not round away to `0%`.

**This breaks a test outside the file list.** `liveResetLabel` has a second
consumer main's chunk list does not mention: `LiveUsageWindowRows.tsx:56`,
inside the existing usage view. So
`views/popover/UsageView.test.tsx:258` — `getByText(/resets in 2h 30m/)` —
must move to `/resets 2:30pm/`. That is the only assertion affected;
`OverlayWindow.test.tsx` and `usageBars.test.ts` go through a separate
`resetsLabel` in `lib/usageBars.ts` that this port does not touch.

### 5. Tests

Port `UsageLimitsBar.test.tsx` (147 lines, five tests) as-is. It covers the
degraded-provider path and one happy-path accessible name.

It does not cover the meter itself, so this port adds what the prototype never
wrote:

- `SegmentedMeter.test.tsx` — the null-percent half-strength state, the notch
  position, and no notch without an `expectedFraction`. These are the three
  rules from step 2; a rule with no test is a rule that quietly dies.
- `SegmentFigure.test.tsx` — thin, but it pins the funnel.
- A test for the disclosure toggle's pressed state in `UsageLimitsBar`.

`UsageRing.test.tsx` needs no change: its zero-arc assertion still passes with
the remainder track gone, and the `percent={null}` path keeps its dashed track.

### 6. The swap, and the delete

`views/PopoverView.tsx` renders `<UsageLimitsSection>` at line 288. The 1a
edit is about six lines — the import, the JSX tag, and leaving `onViewAll` on
main's `session.setShowUsage(true)`. The prototype's 215-line `PopoverView`
diff is almost entirely chunks 2 and 3 (hotspots footer, session peek, the
removed session pane) and is not applied.

Then delete `UsageLimitsSection.tsx` and its test, moving
`UsageLimitsSectionProps` into `UsageLimitsBar.tsx` under its own name. The
rollout doc says the section goes when the bar is real. Leaving both is how a
codebase ends up with two of everything.

The delete is safe: `PopoverView.tsx:288` is the only render site in the app.
The settings `UsagePane` reaches `liveErrorNote` directly and never touches the
section. Beyond `PopoverView`, only the barrel export at
`components/providerUsage/index.ts:6` and a comment in `PopoverView.test.tsx:69`
name it.

### 7. Type scale

**Decided (Keith, 2026-08-21): 11px.** Chunk 0's one-line bump comes into this
PR — `.type-caption` goes from 10px to 11px at `styles/typography.css:65`. The
earlier review pass on exactly this text was blunt: "we should never be using
tiny text like this."

The blast radius is real and belongs in the PR description: **80 uses of
`type-caption` across 25 files**, including the overlay window, onboarding, the
repository list, the scan status bar, and every session-analytics chart. All of
them get a pixel taller. None should break — the class carries no layout
assumption — but the reviewer deserves to know the change is not local to the
meters.

**The consequence, flagged not fixed.** At 11px, `type-caption` and
`type-footnote` (`typography.css:59`) are the *same size*, separated only by
letter-spacing (0.06px against 0.12px). That is not a scale step, it is a
duplicate. Known-debt item 3 in the rollout doc says the scale may have one
level too many, and this bump is what proves it.

Collapsing the two is out of scope here — it touches every surface in the app
and needs a design pass, not a find-and-replace. This PR ships the 11px and
leaves both classes standing. Whoever takes the scale next has the evidence.

## Verification

From `apps/desktop`:

```bash
pnpm exec tsc --noEmit && pnpm exec vitest run && pnpm exec eslint src && pnpm exec prettier --check src
```

No Rust changes in this port, so no `cargo` run is needed. Never stage
`default.profraw`.

Then a real look: build the app, open the popover, and check the bar against
the reference screenshot — pills, the chart icon's pressed state, the eyebrows,
the notch positions, and a hover on a meter revealing its reset.

## Ground rules carried from the rollout doc

1. **`AGENTS.md` applies.** No `useEffect` — the prototype 1a files have none,
   and the port must not add one. All comments in ASD-STE100. Every commit
   signed off (`git commit -s`); the DCO check fails the whole PR on one
   unsigned commit.
2. **No prototype markers.** Nothing in this chunk touches
   `lib/prototypeData.ts`, and no `PROTOTYPE` doc comment may survive into the PR.
3. **One PR.** ~730 added, ~200 removed, under the 1,000-line cap.

## Decisions, settled 2026-08-21

Both open questions are closed and folded into the steps above.

1. **The brand orange changes between modes** — but as a text/fill split, and
   it is light mode that needs the darker value. Two tokens, step 1.
2. **`type-caption` goes to 11px** in this PR, carrying chunk 0's one-line
   bump and its 80-site blast radius. Step 7.

Nothing else blocks a start.
