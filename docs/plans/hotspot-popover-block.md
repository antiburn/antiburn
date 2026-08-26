# Hotspot block on the activity popover

The block at the foot of the activity surface. It names the single most common
Hygiene & Efficiency finding across the last 30 days, says how many sessions
carry it, hands over one pasteable fix, and opens to show the evidence behind
the count.

Design is settled. This plan covers what has to be built, in what order, and
the one thing that blocks the useful half of it.

## What is settled

From the Figma file _antiburn bottom section_, the HTML proto, and the review of
the built block on 2026-08-26:

- **An orange rule across the top of the block, and nothing else is orange.**
  Not a left spine, not a hairline. `border-t border-brand-tint`. The first
  build also filled the fix field with the brand colour and set the count in
  `text-brand`; both were too loud. The brand carries further when it marks one
  thing than when it fills three.
- **One type style and one colour across the claim line.** The count, the
  category name and the saving are all `type-body`. Only the saving steps back,
  to `text-label-tertiary`, because it is an estimate.
- **No section label.** No "TIPS", no "Issues", no eyebrow of any kind.
- **The fix field is a plain stroked field.** `border-separator`, `text-label`
  ink, `font-mono`. No fill and no brand colour, so it needs no new token and it
  reads the same in both themes. This replaces the `brand-field` token an
  earlier draft of this plan added: with no theme-varying ground there is
  nothing for a token to hold.

  Contrast is why the field never went white-on-orange. White on `brand-tint`
  measures 2.86:1 and fails AA at 11px, and it also fails the 3:1 a graphic
  needs, so the copied-state check could not have gone white either. On the
  stroked field `label` measures 6.34:1 and the check is `system-green`.

- **The whole field is the copy target.** A 13px icon beside a wide, obviously
  selectable command is the smaller of the two things a reader aims at. The
  field takes the click; the icon only reports what happened.
- **Activity surface only.** Not session, not usage. `34×` is a claim about the
  whole 30-day cohort; sitting under one session's analysis it would read as a
  claim about _that_ session, and no wording fixes a placement that lies. The
  usage surface is also already at 780px, the one recorded deviation from the
  700 contract.
- **The detail carries as many evidence rows as the detector recorded.** One
  mechanism sentence, then the counters. No alternative line. An earlier draft
  fixed the count at two; the block now takes a list and scrolls past
  `max-h-56` rather than capping what a detector may prove.
- **An info icon at the leading edge of the claim line**, and the whole claim
  line is the hit target. Not a disclosure triangle: what opens is evidence for
  a claim, not a section of the document.
- **The detail opens below the fix field.** The pasteable line never moves, open
  or closed, so the reader can hit copy without waiting for a reflow.
- **The window never resizes.** See _Layout rule_ below.
- **No finding, no block.** Rule, claim, fix and icon go together.

## The blocker: nothing can populate this yet

Checked against live code on this branch:

- There is no `crates/antiburn-local/src/insights/` module. No report
  accumulator, no detectors, no status module.
- There is no `get_local_insights_report` IPC command, and nothing in
  `apps/desktop/src` references an efficiency report.
- In `crates/antiburn-local/src/analysis/evidence.rs:186`, `SessionEvidence`
  carries exactly one real group — `context`. The groups every detector needs
  (`tools`, `models`, `subagents`, `cache`, `compactions`, `quota_incidents`)
  are `EvidenceValue<UnfinishedGroup>` behind `#[cfg(debug_assertions)]`.

So **zero detectors can produce a finding today**, and Unused MCP Servers — the
category the design is drawn around — needs the `tools` group specifically.

This does not block the UI. It blocks the UI being _lit_. The carve below builds
the presentation against a frozen contract and lets it render nothing until the
report exists, which is exactly what it must do on a cold install anyway.

## Layout rule

`POPOVER_HEIGHTS.activity` stays at `DEFAULT_POPOVER_HEIGHT` (700). **Do not
touch `apps/desktop/src/lib/popoverHeight.ts`.** Opening the detail must not
ask the shell for a taller window.

This works for free. In `PopoverView.tsx:345`, the list already sits in
`<div className="min-h-0 flex-1">` above `PopoverFooter`, and `SessionList` owns
a scrolling viewport inside it. A taller block takes its height out of the list,
and the list shows fewer rows. Nothing moves; less is visible.

The cost, measured off the built component in the running app, not the proto —
the proto's padding was looser and its numbers (116px / 190px) were wrong:

| State                    | Block height | List gives up                 |
| ------------------------ | ------------ | ----------------------------- |
| Closed                   | 68px         | —                             |
| Open, five evidence rows | 236px        | 168px, about two session rows |

The open figure grows with the row count until the detail hits `max-h-56`
(224px), which lands at roughly eight rows. Past that the detail scrolls and the
block stops growing, so the list never gives up more than 236px.

The block goes between the list `div` and `<PopoverFooter>`, inside the same
flex column. The footer keeps its own `border-t border-separator` when the block
is absent; when the block is present the block owns the top edge and the footer's
hairline is not drawn twice.

## Component

New file: `apps/desktop/src/views/popover/HotspotBlock.tsx`.

Follow the conventions already set by `components/ui/Disclosure.tsx`:
uncontrolled open state via `useState`, `aria-expanded` + `aria-controls`, and
the body **unmounted** when collapsed so closed evidence stays out of the
accessibility tree and out of find-in-page.

It is a separate component rather than a use of `Disclosure`, because
`Disclosure` is a full-width label button with a trailing chevron for prose in
Settings. This is a three-part claim line with a leading info icon and a fixed
action field that lives outside the collapsible region.

**No `useEffect`.** The one place it would be reached for is reverting the copy
icon from a check back to the clipboard glyph after 2s. That is work caused by
the click, so it belongs in the click handler: set state, `setTimeout`, done. A
timer that fires after unmount sets state on a dead component, which is a no-op
in React 18 and not a warning. If review disagrees, the fallback is to drop the
timed revert and leave the check until the block re-renders — not to add an
effect.

### Contract

```ts
/** One of the nine canonical Hygiene & Efficiency categories. */
type HotspotCategory =
  | "unusedMcpServers"
  | "overpoweredSubagents"
  | "sessionsOverDepth"
  // … and the remaining six. `hotspot.ts` holds the full list, in tie-break
  // order.
  | "oldModelUsage";

type HotspotFinding = {
  category: HotspotCategory;
  /** Sessions in the assessed cohort carrying this finding. */
  sessions: number;
  /** Preformatted, already `≈`-prefixed. Null when pricing is unknown. */
  saving: string | null;
  /** The one pasteable line. Never prose. */
  fix: string;
  /**
   * The counters the detector recorded, preformatted, in the order to show
   * them: the size of the problem first, then the proof it is real. For
   * Unused MCP Servers that is tokens over the window, then the servers that
   * were loaded and never invoked.
   *
   * No ceiling on the count. A long list scrolls inside the opened detail.
   */
  evidence: readonly HotspotEvidenceRow[];
};
```

There is no `cohort` field. An earlier draft carried one for a `34 of 61`
denominator, but the claim line prints `34×` and the denominator, where a
detector records it, is just another evidence row.

**Prose is not in the payload.** The mechanism sentence is a fixed string per
category, held in a TS constant keyed by `HotspotCategory`. It never names a
repo, a path or a number, so shipping it through IPC would be moving a constant
across a process boundary for nothing.
The IPC payload carries counts, slots and the assembled fix line only.

This also keeps the privacy rule in `local-insights-architecture.md` easy to
hold: no raw prompts, tool inputs, transcript content or local paths cross the
boundary.

### Rendering rules

- `finding == null` → return `null`. Not an empty shell, not an "all clear"
  message. This is what satisfies **FR-14**: a half-read cohort renders as
  nothing, so it cannot be misread as clean.
- The claim line is a `<button>` spanning the full width.
- The info icon does not rotate. It steps from `text-label-tertiary` to
  `text-label` over `duration-fast` while the detail is open.
- The fix line is `font-mono`, truncates with an ellipsis, never wraps.
- The fix field is itself a `<button>`. Copy writes `finding.fix` to the
  clipboard and swaps the icon to a check for 2s — the same shape as Cadence's
  `copyText`. A rejected write is caught and shows nothing: the reader can still
  select the line, and a 26px field has no room to say more.

## Selection

One winner, chosen deterministically:

1. Rank by session count, descending.
2. Ties broken by estimated tokens, descending.
3. Ties broken by fixed category order.

Open state is **not persisted**. Every time the popover opens, the block is
closed. Reopening on the same finding does not restore an expansion, because a
tray popover is glanced at, and a reader who left it open a week ago did not ask
for a 236px block on next launch.

## Sequence

**Seam 1 — presentation, wired but dark. Done.**
`HotspotBlock.tsx`, the category prose constants, the contract types, unit
tests. Rendered from `PopoverView` with a hard-coded `null` finding, so the call
site exists and nothing is dead code. Ships invisible. No Rust.

| File                                                   | State                                                      |
| ------------------------------------------------------ | ---------------------------------------------------------- |
| `apps/desktop/src/views/popover/hotspot.ts`            | new — types, nine categories, per-category copy            |
| `apps/desktop/src/views/popover/HotspotBlock.tsx`      | new — the block                                            |
| `apps/desktop/src/views/popover/HotspotBlock.test.tsx` | new — 12 tests                                             |
| `apps/desktop/src/views/PopoverView.tsx`               | renders `<HotspotBlock finding={null} />` above the footer |

No stylesheet changes and no new tokens. The block uses `brand-tint`,
`separator`, `label`, `label-secondary`, `label-tertiary` and `system-green`,
all of which the contract already carries.

Green on `origin/main` at `684b0a4`: `type-check`, `lint`, `format`, `knip`,
849 unit tests, `check-design-drift.mjs`, `pnpm run slop`.

**Seam 2 — the report reducer and detectors.**
Blocked on the `tools` evidence group landing for real. Owned by GH-70's CH
ladder, not by this plan. This plan's only requirement is that the reducer emits
enough to fill `HotspotFinding`.

**Seam 3 — IPC and go-live.**
`get_local_insights_report`, the winner selection, and swapping the hard-coded
`null` for the real value.

Seam 1 is worth doing now and on its own: it is the part under design review,
it needs no Rust, and it is the part that would otherwise be rewritten twice.

## Verification

- `HotspotBlock.test.tsx`: a null finding renders nothing at all; the claim line
  exposes `aria-expanded` and toggles it; the evidence body is absent from the
  DOM when closed; copy writes the fix string to the clipboard; the command text
  sits inside the copy button, so the whole field is the target; every evidence
  row a finding carries is rendered.
- A test asserting `POPOVER_HEIGHTS.activity === DEFAULT_POPOVER_HEIGHT`, so a
  later change cannot quietly make the block resize the window.
- A rejected clipboard write leaves the block in its uncopied state. The other
  half of that test — that the rejection is _handled_ — is enforced by vitest,
  which reports an unhandled rejection and exits non-zero. Drop the `.catch` in
  `onCopy` and the file fails on that route, not on an `expect`.
- `scripts/check-design-drift.mjs` — no new tokens are needed, so this passes
  untouched. If it does not, the block is hard-coding something.
- `pnpm run slop` before finishing.

## Decisions taken

All three open questions were settled at review on 2026-08-26 and are folded
into _What is settled_ above: no alternative line, `label` ink, and activity
surface only.

Reviewing the built block the same day settled five more, also folded in above:
drop the brand fill for a plain stroked field, make the whole field the copy
target, drop `text-brand` from the count, use an info icon rather than a
disclosure triangle, and let the detail carry as many evidence rows as the
detector recorded.

One consequence of dropping the alternative line is worth recording rather than
rediscovering. It means the block offers exactly one route. That is right for
Unused MCP Servers, where `claude mcp remove` covers the common case. It is
thinner for Sessions Over Depth, where the honest advice is often "hand off"
rather than a threshold change, and for a claude.ai connector, which
`claude mcp remove` cannot touch at all. Those still get a correct fix line;
they just lose the caveat beside it. The caveat belongs in Settings → Insights,
which reads the same report and has room for it.

## Out of scope

- The nine detectors and the evidence groups they need (GH-70).
- The report accumulator and its read-only snapshot connection.
- Settings → Insights, which the architecture plan names as the first insights
  surface. This block is a second, smaller reader of the same report.
- Any second finding, a list of findings, or a way to page between them.
