import { describe, expect, it } from "vitest"

import type { ProviderUsageDayPayload } from "../providerUsageIpc"
import {
  axisDayLabel,
  dayLabel,
  niceCeiling,
  seriesMax,
  spendDeltaLabel,
} from "./overviewChart"

const day = (usd: number | null): ProviderUsageDayPayload => ({
  localDate: "2026-09-14",
  tokensIn: 10,
  tokensOut: 0,
  cacheRead: 0,
  estimatedUsd: usd,
  costComplete: usd != null,
  sessionCount: 1,
})

describe("overviewChart", () => {
  it("labels a reader-local date without a timezone shift", () => {
    expect(dayLabel("2026-09-14")).toBe("Mon 14 Sep")
    expect(axisDayLabel("2026-01-02")).toBe("2 Jan")
  })

  it("picks a 1-2-5 ceiling at or above the max", () => {
    expect(niceCeiling(0)).toBe(1)
    expect(niceCeiling(0.7)).toBe(1)
    expect(niceCeiling(1)).toBe(1)
    expect(niceCeiling(14.5)).toBe(20)
    expect(niceCeiling(42)).toBe(50)
    expect(niceCeiling(50)).toBe(50)
    expect(niceCeiling(51)).toBe(100)
    expect(niceCeiling(864.2)).toBe(1000)
  })

  it("ignores unpriced days when finding the tallest", () => {
    expect(seriesMax([day(3), day(null)], [day(7)])).toBe(7)
    expect(seriesMax([day(null)], [])).toBe(0)
  })

  it("compares two priced days and nothing else", () => {
    expect(spendDeltaLabel(day(3), day(1))).toBe("+$2.00")
    expect(spendDeltaLabel(day(1), day(3))).toBe("−$2.00")
    expect(spendDeltaLabel(day(1), day(1.001))).toBe("no change")
    expect(spendDeltaLabel(day(null), day(1))).toBeNull()
    expect(spendDeltaLabel(day(1), day(null))).toBeNull()
    expect(spendDeltaLabel(day(1), undefined)).toBeNull()
  })
})
