import { describe, expect, it } from "vitest"

import type { AllowanceDayPayload, ProviderUsageDayPayload } from "../providerUsageIpc"
import {
  allowancePointsLabel,
  allowanceSeriesMax,
  axisDayLabel,
  limitHitDayLabel,
  dayLabel,
  niceCeiling,
  percentCeiling,
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

describe("allowance scale", () => {
  function allowanceDay(usedPercent: number | null, blockCount = 0): AllowanceDayPayload {
    return { localDate: "2026-09-14", usedPercent, blockCount }
  }

  it("stops the scale at one whole allowance", () => {
    expect(percentCeiling(0)).toBe(5)
    expect(percentCeiling(4)).toBe(5)
    expect(percentCeiling(6)).toBe(10)
    expect(percentCeiling(11)).toBe(25)
    expect(percentCeiling(80)).toBe(100)
    expect(percentCeiling(140)).toBe(100)
  })

  it("takes the scale from the known days of both series", () => {
    expect(allowanceSeriesMax([allowanceDay(12), allowanceDay(null)], [allowanceDay(30)])).toBe(
      30,
    )
    expect(allowanceSeriesMax([allowanceDay(null)])).toBe(0)
  })

  it("names a day with no reading rather than calling it zero", () => {
    expect(allowancePointsLabel(null)).toBe("no reading")
    expect(allowancePointsLabel(0)).toBe("0.0 points")
    expect(allowancePointsLabel(4.25)).toBe("4.3 points")
    expect(allowancePointsLabel(31.4)).toBe("31 points")
  })

  it("states a day's limit hits and stays silent when there are none", () => {
    expect(limitHitDayLabel(0)).toBeNull()
    expect(limitHitDayLabel(1)).toBe("1 limit hit")
    expect(limitHitDayLabel(3)).toBe("3 limit hits")
  })
})
