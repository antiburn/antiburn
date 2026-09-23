import { describe, expect, it } from "vitest"

import type { Spoke } from "./radialGeometry"
import { weekBands, weekPointerFocus, spreadLabels, type Plot } from "./weekLines"

const PLOT: Plot = { x0: 0, x1: 700, y0: 0, y1: 100 }
const WEEK = 7 * 86400

const week = (start: number, top: number) => ({
  lane: "weekly",
  startsAtEpoch: start,
  resetsAtEpoch: start + WEEK,
  points: [
    { atEpoch: start, percent: 0 },
    { atEpoch: start + WEEK, percent: top },
  ],
})

const spoke: Spoke = {
  key: "s",
  weekStart: 0,
  current: false,
  from: 0,
  to: 0.1,
  startsAtEpoch: 0,
  resetsAtEpoch: 0.1 * WEEK,
  peakPercent: 80,
  points: [
    { fraction: 0, percent: 0 },
    { fraction: 0.1, percent: 80 },
  ],
}

describe("weekLines", () => {
  it("splits the plot into one row per week when apart", () => {
    expect(weekBands(PLOT, 2, false, 10)).toEqual([
      { top: 0, height: 100 },
      { top: 0, height: 100 },
    ])
    expect(weekBands(PLOT, 2, true, 10)).toEqual([
      { top: 0, height: 45 },
      { top: 55, height: 45 },
    ])
  })

  it("snaps to a 5-hour curve at its level, not at its peak", () => {
    const full = () => ({ top: 0, height: 100 })
    // Halfway through the window the curve is at 40%, so y = 60.
    const hit = weekPointerFocus(
      PLOT,
      { x: 35, y: 61 },
      [week(0, 50)],
      full,
      [spoke],
      null,
      [],
      false,
    )
    expect(hit?.focus).toEqual({ kind: "short", key: "s" })
    // At the window's peak height, but halfway through, the curve is far away.
    const miss = weekPointerFocus(
      PLOT,
      { x: 35, y: 21 },
      [week(0, 50)],
      full,
      [spoke],
      null,
      [],
      false,
    )
    expect(miss?.focus).toEqual({ kind: "time" })
  })

  it("reads only the week of the pointer's row when apart", () => {
    const weeks = [week(WEEK, 50), week(0, 50)]
    const bands = weekBands(PLOT, 2, true, 10)
    const bandOf = (start: number) => (start === WEEK ? bands[0]! : bands[1]!)
    const focus = weekPointerFocus(PLOT, { x: 600, y: 80 }, weeks, bandOf, [], null, [], true)
    expect(focus?.focus).toEqual({ kind: "week", start: 0 })
  })

  it("spreads labels apart and keeps them inside the plot", () => {
    expect(spreadLabels([50, 52, 51], 10, 0, 100)).toEqual([50, 70, 60])
    expect(spreadLabels([99, 100], 10, 0, 100)).toEqual([90, 100])
  })
})
