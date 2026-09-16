import { describe, expect, it } from "vitest"

import type { HudSpendRate } from "./ipc"
import {
  blinkPeriod,
  describeSpend,
  LED_PERIOD_RUNGS_MS,
  ledPeriodFromConsumption,
  ledPeriodMs,
  PERIOD_FAST_MS,
  PERIOD_SLOW_MS,
} from "./ledPeriod"
import type { LiveUsageSummaryPayload } from "./providerUsageIpc"

function spend(usdPerMinute: number, pricedShare = 1): HudSpendRate {
  return { usdPerMinute, windowSecs: 300, pricedShare }
}

function usage(consumptionRate: number | null): LiveUsageSummaryPayload {
  return {
    providers: [
      {
        windows: [{ forecast: { consumptionRate } }],
      },
    ],
  } as unknown as LiveUsageSummaryPayload
}

describe("ledPeriodMs", () => {
  it("holds the slow end at and below the floor", () => {
    expect(ledPeriodMs(0)).toBe(PERIOD_SLOW_MS)
    expect(ledPeriodMs(0.05)).toBe(PERIOD_SLOW_MS)
    expect(ledPeriodMs(0.001)).toBe(PERIOD_SLOW_MS)
  })

  it("holds the fast end at and above the ceiling", () => {
    expect(ledPeriodMs(2)).toBe(PERIOD_FAST_MS)
    expect(ledPeriodMs(40)).toBe(PERIOD_FAST_MS)
  })

  it("lands on a middle rung at the geometric midpoint", () => {
    // sqrt(0.05 * 2) is the midpoint of the geometric range.
    const midpoint = Math.sqrt(0.05 * 2)
    const period = ledPeriodMs(midpoint)
    expect([LED_PERIOD_RUNGS_MS[3], LED_PERIOD_RUNGS_MS[4]]).toContain(period)
  })

  it("only ever returns a rung", () => {
    for (const rate of [0.06, 0.1, 0.2, 0.4, 0.8, 1.2, 1.9]) {
      expect(LED_PERIOD_RUNGS_MS).toContain(ledPeriodMs(rate))
    }
  })

  it("gives no period for no rate", () => {
    expect(ledPeriodMs(null)).toBeNull()
    expect(ledPeriodFromConsumption(null)).toBeNull()
  })
})

describe("blinkPeriod", () => {
  it("prefers a priced spend", () => {
    expect(blinkPeriod(spend(2), usage(1))).toEqual({
      periodMs: PERIOD_FAST_MS,
      source: "spend",
    })
  })

  it("steps down to the allowance rate when nothing is priced", () => {
    expect(blinkPeriod(spend(0, 0), usage(100))).toEqual({
      periodMs: PERIOD_FAST_MS,
      source: "usage",
    })
    expect(blinkPeriod(null, usage(1))).toEqual({ periodMs: PERIOD_SLOW_MS, source: "usage" })
  })

  it("falls back to the fixed slow period", () => {
    expect(blinkPeriod(null, null)).toEqual({ periodMs: PERIOD_SLOW_MS, source: "fixed" })
    expect(blinkPeriod(spend(0, 0), usage(null))).toEqual({
      periodMs: PERIOD_SLOW_MS,
      source: "fixed",
    })
  })
})

describe("describeSpend", () => {
  it("states the rate, hedges a partial price, and names an unpriced window", () => {
    expect(describeSpend(null)).toBeNull()
    expect(describeSpend(spend(0.42))).toBe("Spending about $0.420/min.")
    expect(describeSpend(spend(1.5, 0.5))).toBe("Spending about $1.50/min or more.")
    expect(describeSpend(spend(0, 0))).toBe("Spend unknown: no priced model in the window.")
  })
})
