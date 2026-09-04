# Session Detail feedback — Dave + Keith, 3 Sep 2026

Review of PR #346 (`feat/session-details-visual-polish`). Keith: "I don't think
anything there I'd disagree with." Treat everything below as accepted unless
marked **Open**.

Code lives in `apps/desktop/src/components/session/`.

---

## 1. Efficiency meters — go back to the thermometer

`analysis/EfficiencyBreakdown.tsx`, `ui/SegmentedMeter.tsx`

This is the biggest change in the round, and the one Dave felt most strongly
about:

> Still not completely sure about this visualisation, I liked it the other way
> where it was easy to see good and bad and medium. With this visualisation
> you'll actually see less if you're doing a good job — more efficient is a
> shorter bar.

He is right, and commit `2df5ae79` states the cause outright: "the zones run
left to right along each metric's own scale, so orange always marks the good
end." A good reading therefore fills less of the track, so the better the
session, the less there is to look at.

### 1.1 `CostScaleBar` goes back to a banded track with a needle

`CostScaleBar` is the offender. It renders `percent={scale.position * 100}`
through `SegmentedMeter`, so the track fills up to the reading — and a low
`$/MTok` is a good result that draws almost nothing.

Go back to the `1bb192b7` form: a full-width track split into good / ok / bad
bands (`system-green` / `separator` / `system-orange`) with a needle marking
where this session sits. The track never shrinks; the needle moves. That is
exactly "good and bad and medium".

This is a render swap, not a data-layer job. `efficiencyThermometer()` in
`lib/presentation/sessionEfficiency.ts` survived the restyle, is still called
today, and already supplies `scale.position` — the component even passes it as
`expectedFraction`.

### 1.2 `CompositionTrack` keeps its form

The three shares stay as one track (Keith: "keep as single thermometer"). It
was never part of the problem: three runs summing to 100% means it is always
full width, so it does not shrink when a session goes well.

The two tracks will then read differently side by side, which is right. A
banded track with a needle answers "where does this reading sit between good
and bad". A composition answers "how did the whole divide". Different
questions, different forms.

### 1.3 Restyle both

Keith: same form, better style — more saturated colours, bolder and larger
curves, better fit with the rest of the UI.

The old thermometer was genuinely weak, and the specifics line up: band fills
at `/50` opacity over an `h-1.5` track, with a 1.5px needle. Saturate the
bands, thicken the track, give the needle real weight. `CompositionTrack` takes
the same treatment: saturated runs, thicker track.

---

## 2. Context tokens chart

`analysis/ContextTokensChart.tsx`

With §1 pulled out, this section is ordinary polish.

### 2.1 Rehydration marker is too light

The rehydration mark now reads as barely-there. The previous version was maybe
too heavy, but it caught the eye, which is what a rehydration should do. Land
between the two.

### 2.2 Y axis should stretch to the next tick

The plot should run up to the next tick mark rather than stopping short — in
the screenshot, to 400k. Tidier.

`axisScale()` in `lib/presentation/sessionAnalysis.ts` already rounds the
ceiling up to a step multiple, but clamps to `cap`, and the chart passes
`summary.contextWindow`. That clamp is the likely cause — inferred from the
code, not a confirmed repro.

### 2.3 One hover, all the numbers

A bucket currently surfaces several small separate tooltips. Hovering the bar
should show one panel covering everything about that bucket.

### 2.4 Out tokens should read stronger

Out tokens are the most expensive thing and the best proxy for work actually
getting done. They should be the strongest series, not the quietest.

### 2.5 "rehydration" label placement — **Open**

Dave: "I wonder if we should put 'rehydration' at the bottom of the chart
rather than on the chart itself, not sure (obviously I just dumped it there)."
Worth trying both.

### 2.6 Settled: rehydration is not all-or-nothing

Keith asked whether rehydration is "all the cache or nothing". It isn't — which
is why bars stop short of the top. On Dave's session about 26.4k tokens never
expire, presumably the system prompt, which gets its own caching treatment.

No code change, but the visualisation should not imply all-or-nothing.

---

## 3. Header hierarchy

`SessionDetailPresentation.tsx`, the "Session summary" block

Repo goes to the very top of the section, then the session name, then the
models / time / cost row. Dave's reasoning:

> You're "inside" the repo, that's the container, and then the session is what
> you're doing in it. And then you're doing it in this way, with these models,
> this amount of time/dollars.

Zack raised it too. Repo currently sits second and eats a lot of vertical
space. This is the top section only — no change to the session list.

Also: "this is good without the arrows." Just don't reintroduce them.

---

## 4. "Usage" tab → "Cost"

`SessionDetailPresentation.tsx` (`DETAIL_TABS`, `UsageHelperFooter`),
`analysis/CostBreakdown.tsx`, `analysis/HygieneBreakdown.tsx`

### 4.1 Rename the tab to "Cost"

> "Usage" is a weird thing to call this, it doesn't actually represent anything
> on this page. And if on the first page I want to see something about the cost
> of the session, I don't actually know where to click because there's nothing
> that tells me this is the cost drilldown.

### 4.2 Checks above the cost rows

Checks are more important and more interesting much of the time. They currently
sit under `CostBreakdown`; flip the order.

### 4.3 Add subheadings

With both on one tab, the screen probably needs "Cost" and "Checks"
subheadings.

### 4.4 Drop the static footer note

Delete the "Cache reads bill at about 10% of the fresh-input price…" copy. Far
too much text for something 99.99% of people will not care about.

### 4.5 Replace the hover-driven explainer

Dave did not work out that the footer text was driven by hovering a check:
"yeah that wasn't clear to me at all." The failure is that hover left no trace.

An accordion was the obvious fix, but it does not read as a desktop Mac app.

**Decision: an info button that appears on row hover, with a tooltip.** The
button is the affordance the old version lacked, and the tooltip keeps the
rows at a fixed height.

There is precedent in the codebase: `EfficiencyBreakdown.tsx` at `1bb192b7`
had exactly this — a `RowInfo` component wrapping an `Info` icon in a
`Tooltip`, one per row.

### 4.6 Explainer text repeats the check name

The dynamic text opens by restating the name of the check being hovered. Drop
it, wasted words. (`HygieneBreakdown.tsx` passes `{ title: entry.name, body:
entry.explainer }`; the title prefix renders in `UsageHelperFooter`.)

---

## 5. Tools tab

`analysis/SkillsMcpChart.tsx`, plus the tools panel in
`SessionDetailPresentation.tsx`

Tools is generally good, but the most important line — how much was burned by
the unused items — sits at the bottom. Wrong hierarchy. Move it up.

---

## 6. Sub-agents

`orchestration/SubagentRosterRow.tsx`, `analysis/CostBreakdown.tsx`

### 6.1 The list needs work — **not this PR**

Dave on his own earlier work: "subagents list was bad before when I did it, no
need to do it as part of this PR but it would be good to make it nicer at some
point (but it's not super important)."

### 6.2 It is not discoverable

Keith: "I don't even know how to get to that subagents view." You get there by
clicking the "N sub-agents" row. Needs an affordance.

### 6.3 Test with a real sub-agent session

Dave: "probably important to test on a session with subagents because it
stretches the layout a fair bit." Do this before merging.

---

## 7. Tab nav

`SessionDetailPresentation.tsx`, the `SegmentedControl` block

The nav is unclear. Dave suggested left-hand-side tabs; Keith ruled that out —
side tabs feel odd and eat horizontal space, which is in short supply.

**Decision: nav goes to the top.** It currently renders below the tab panel.

Window width: Dave is fine with a wider window, Keith and Zack are not. No
change.
