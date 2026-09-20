import { describe, expect, it } from "vitest"

import type { QuotaPeriodPayload } from "../../../lib/providerUsageIpc"
import { rowsInWindow, windowSlots, WINDOW_GAP_PX } from "./quotaLayout"
import type { QuotaSeriesRow } from "./quotaSeries"

const DAY = 24 * 60 * 60
const WEEK = 7 * DAY

function period(over: Partial<QuotaPeriodPayload> = {}): QuotaPeriodPayload {
  return {
    periodId: 1,
    startsAtEpoch: 0,
    resetsAtEpoch: WEEK,
    startSource: "reported",
    resetSource: "reported",
    samples: [],
    peakPercent: null,
    contributions: [],
    sessions: [],
    unattributed: { usd: 0, percent: 0, sessionCount: 0 },
    unattributedBuckets: [],
    estimatedPercent: null,
    unexplainedBuckets: [],
    unexplainedPercent: null,
    meterCoverageUntil: null,
    meterRegressions: 0,
    ...over,
  }
}

function row(t: number): QuotaSeriesRow {
  return {
    t,
    index: 0,
    meter: null,
    other: null,
    unattributed: null,
    unexplained: null,
  }
}

describe("windowSlots", () => {
  it("gives an equal-width slot to each window, back to back with the gap between them", () => {
    const periods = [
      period({ periodId: 1, startsAtEpoch: 0, resetsAtEpoch: WEEK }),
      period({ periodId: 2, startsAtEpoch: WEEK, resetsAtEpoch: 2 * WEEK }),
      period({ periodId: 3, startsAtEpoch: 2 * WEEK, resetsAtEpoch: 3 * WEEK }),
    ]
    const slots = windowSlots(periods, 10, 100, 6)
    expect(slots).toHaveLength(3)
    const width = (100 - 6 * 2) / 3
    expect(slots[0]!.left).toBe(10)
    expect(slots[0]!.right).toBeCloseTo(10 + width, 10)
    expect(slots[1]!.left).toBeCloseTo(10 + width + 6, 10)
    expect(slots[1]!.right).toBeCloseTo(10 + 2 * width + 6, 10)
    expect(slots[2]!.left).toBeCloseTo(10 + 2 * width + 2 * 6, 10)
    expect(slots[2]!.right).toBeCloseTo(100 + 10, 10)
  })

  it("uses the default gap when none is given", () => {
    const periods = [period()]
    const slots = windowSlots(periods, 0, 100)
    expect(slots).toHaveLength(1)
    expect(slots[0]!.left).toBe(0)
    expect(slots[0]!.right).toBe(100)
    expect(WINDOW_GAP_PX).toBeGreaterThan(0)
  })

  it("floors the slot width at 1px rather than going negative when there is no room", () => {
    const periods = [period({ periodId: 1 }), period({ periodId: 2, startsAtEpoch: WEEK })]
    const slots = windowSlots(periods, 0, 1, 6)
    expect(slots[0]!.right - slots[0]!.left).toBe(1)
    expect(slots[1]!.right - slots[1]!.left).toBe(1)
  })

  it("returns no slots for no periods", () => {
    expect(windowSlots([], 0, 100)).toEqual([])
  })

  it("maps a window's own start and reset to the slot's own left and right", () => {
    const p = period({ startsAtEpoch: 1000, resetsAtEpoch: 1000 + WEEK })
    const [slot] = windowSlots([p], 20, 200)
    expect(slot!.x(1000)).toBeCloseTo(20, 10)
    expect(slot!.x(1000 + WEEK)).toBeCloseTo(220, 10)
    expect(slot!.x(1000 + WEEK / 2)).toBeCloseTo(120, 10)
  })

  it("gives a short window and a long window the same slot width", () => {
    const short = period({ periodId: 1, startsAtEpoch: 0, resetsAtEpoch: 5 * 60 * 60 })
    const long = period({ periodId: 2, startsAtEpoch: 5 * 60 * 60, resetsAtEpoch: WEEK })
    const [shortSlot, longSlot] = windowSlots([short, long], 0, 100, 6)
    expect(shortSlot!.right - shortSlot!.left).toBeCloseTo(longSlot!.right - longSlot!.left, 10)
  })
})

describe("rowsInWindow", () => {
  it("keeps a row at the window's own start, drops one at its reset", () => {
    const p = period({ startsAtEpoch: 100, resetsAtEpoch: 200 })
    const rows = [row(99), row(100), row(150), row(199), row(200), row(201)]
    const kept = rowsInWindow(rows, p).map((r) => r.t)
    expect(kept).toEqual([100, 150, 199])
  })

  it("keeps a row at reset minus one", () => {
    const p = period({ startsAtEpoch: 100, resetsAtEpoch: 200 })
    expect(rowsInWindow([row(199)], p)).toHaveLength(1)
  })

  it("returns nothing for a window with no matching rows", () => {
    const p = period({ startsAtEpoch: 1000, resetsAtEpoch: 2000 })
    expect(rowsInWindow([row(0), row(3000)], p)).toEqual([])
  })
})
