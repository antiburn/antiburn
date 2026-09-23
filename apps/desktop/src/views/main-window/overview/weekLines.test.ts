import { describe, expect, it } from "vitest"

import type { PlacedPin, Spoke } from "./radialGeometry"
import {
  groupPins,
  placeCallouts,
  weekBands,
  weekPointerFocus,
  spreadLabels,
  type Plot,
} from "./weekLines"

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

  it("groups close pins of one check and keeps other checks apart", () => {
    const pin = (key: string, detector: string, fraction: number) =>
      ({
        key,
        fraction,
        pin: { detector, label: detector, atEpoch: 0, title: key, navigationHandle: key },
      }) as unknown as PlacedPin
    const groups = groupPins(
      [
        pin("a", "cacheChurn", 0.1),
        pin("b", "cacheChurn", 0.12),
        pin("c", "cacheChurn", 0.5),
      ].concat(pin("d", "modelOverthinking", 0.11)),
      (item) => item.fraction * 1000,
      36,
    )
    expect(groups.map((group) => [group.detector, group.pins.length, group.x])).toEqual([
      ["cacheChurn", 2, 110],
      ["modelOverthinking", 1, 110],
      ["cacheChurn", 1, 500],
    ])
  })

  it("steps close callouts up to the left and never crosses a leader", () => {
    const spots = placeCallouts(
      [
        { x: 10, width: 80 },
        { x: 40, width: 80 },
        { x: 300, width: 80 },
      ],
      0,
      400,
      2,
      10,
      6,
    )
    expect(spots).toEqual([
      { row: 1, flip: false, left: 10, right: 96 },
      { row: 0, flip: false, left: 40, right: 126 },
      { row: 0, flip: false, left: 300, right: 386 },
    ])
  })

  it("flips a callout at the right edge and drops one that fits nowhere", () => {
    expect(placeCallouts([{ x: 380, width: 80 }], 0, 400, 2, 10, 6)).toEqual([
      { row: 0, flip: true, left: 294, right: 380 },
    ])
    expect(
      placeCallouts(
        [
          { x: 10, width: 80 },
          { x: 20, width: 80 },
          { x: 30, width: 80 },
        ],
        0,
        400,
        2,
        10,
        6,
      )[0],
    ).toBeNull()
  })
})
