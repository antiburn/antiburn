import { describe, expect, it } from "vitest"

import { devSpendRate, withDevBlock } from "./hudDev"
import type { UsageBarItem } from "./usageBars"

const bar = (key: string): UsageBarItem => ({
  key,
  label: key,
  providerName: "Claude",
  percent: 40,
  resetsAt: null,
  color: "#000",
  expectedFraction: null,
})

describe("withDevBlock", () => {
  it("holds the first bar at its limit until the block time", () => {
    const blocked = withDevBlock([bar("a"), bar("b")], 20_000, 5_000)
    expect(blocked[0]).toMatchObject({ percent: 100, resetsAt: new Date(20_000) })
    expect(blocked[1]).toEqual(bar("b"))
  })

  it("returns the bars unchanged once the block time passed", () => {
    expect(withDevBlock([bar("a")], 20_000, 20_000)).toEqual([bar("a")])
    expect(withDevBlock([], 20_000, 0)).toEqual([])
  })
})

describe("devSpendRate", () => {
  it("prices the whole window at the given rate", () => {
    expect(devSpendRate(2, 300)).toEqual({ usdPerMinute: 2, windowSecs: 300, pricedShare: 1 })
    expect(devSpendRate(null, 300)).toBeNull()
  })
})
