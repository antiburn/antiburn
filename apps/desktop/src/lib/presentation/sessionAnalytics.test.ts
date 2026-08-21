// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from "vitest"

import type { InitialContextBreakdown, SessionBucket } from "../types/session"
import {
  contextSeries,
  costBreakdownRows,
  costFigureLabel,
  costOutlierThreshold,
  formatCost,
  formatDuration,
  HIGH_COST_FLOOR_USD,
  HIGH_COST_MEDIAN_MULTIPLE,
  HIGH_COST_MIN_SAMPLE,
  initialContextNamedRows,
  initialContextTotal,
  median,
  tokenSeries,
} from "./sessionAnalytics"

function bucket(over: Partial<SessionBucket> = {}): SessionBucket {
  return {
    tokensIn: 0,
    tokensOut: 0,
    contextTokens: 0,
    isCompactionBoundary: false,
    ...over,
  }
}

describe("tokenSeries", () => {
  it("keeps each observed token bucket at its measured progress", () => {
    const buckets = [
      bucket(),
      bucket({ tokensIn: 100, tokensOut: 10 }),
      bucket(),
      bucket({ tokensIn: 300, tokensOut: 30 }),
      bucket({ tokensIn: 500, tokensOut: 50 }),
      bucket(),
    ]
    const series = tokenSeries(buckets)
    expect(series).toHaveLength(3)
    expect(series.map((p) => p.tokensIn)).toEqual([100, 300, 500])
    expect(series.map((p) => p.progress)).toEqual([20, 60, 80])
  })

  it("returns an empty series when no bucket has activity", () => {
    expect(tokenSeries([bucket(), bucket()])).toEqual([])
  })

  it("drops a compaction-only bucket instead of rendering a spurious zero dip", () => {
    const buckets = [
      bucket({ tokensIn: 100, tokensOut: 10 }),
      bucket({ isCompactionBoundary: true }),
      bucket({ tokensIn: 300, tokensOut: 30 }),
    ]
    const series = tokenSeries(buckets)
    expect(series).toHaveLength(2)
    expect(series.map((p) => p.tokensIn)).toEqual([100, 300])
  })
})

describe("contextSeries", () => {
  it("keeps each observed context bucket at its measured progress", () => {
    const series = contextSeries(
      [
        bucket(),
        bucket({ contextTokens: 50_000 }),
        bucket(),
        bucket({ contextTokens: 150_000 }),
      ],
      200_000,
    )
    expect(series).toHaveLength(2)
    expect(series.map((p) => p.contextPct)).toEqual([25, 75])
    expect(series.map((p) => p.progress)).toEqual([33, 100])
  })

  it("carries the compaction-boundary flag onto the matching point", () => {
    const series = contextSeries(
      [
        bucket({ contextTokens: 180_000 }),
        bucket({ contextTokens: 20_000, isCompactionBoundary: true }),
        bucket({ contextTokens: 40_000 }),
      ],
      200_000,
    )
    expect(series.map((p) => p.isCompactionBoundary)).toEqual([false, true, false])
  })

  it("keeps a compaction-only bucket", () => {
    const series = contextSeries(
      [
        bucket({ contextTokens: 180_000 }),
        bucket({ contextTokens: 0, isCompactionBoundary: true }),
        bucket({ contextTokens: 20_000 }),
      ],
      200_000,
    )
    expect(series).toHaveLength(3)
    expect(series.map((p) => p.isCompactionBoundary)).toEqual([false, true, false])
    expect(series.map((p) => p.contextPct)).toEqual([90, 0, 10])
  })
})

describe("formatDuration", () => {
  it("carries the minute remainder into the hour instead of printing 60m", () => {
    expect(formatDuration(7170)).toBe("2h")
    expect(formatDuration(3570)).toBe("1h")
  })

  it("shows seconds under a minute instead of collapsing to 0m", () => {
    expect(formatDuration(0)).toBe("0s")
    expect(formatDuration(30)).toBe("30s")
    expect(formatDuration(59)).toBe("59s")
  })

  it("formats hours and minutes compactly", () => {
    expect(formatDuration(60)).toBe("1m")
    expect(formatDuration(3600)).toBe("1h")
    expect(formatDuration(3660)).toBe("1h 1m")
    expect(formatDuration(8220)).toBe("2h 17m")
  })
})

describe("formatCost", () => {
  it("always renders two decimals", () => {
    expect(formatCost(0.003)).toBe("<$0.01")
    expect(formatCost(0.42)).toBe("$0.42")
    expect(formatCost(2.4)).toBe("$2.40")
    expect(formatCost(20.7)).toBe("$20.70")
    expect(formatCost(240)).toBe("$240.00")
  })

  it("renders an exact zero as $0.00, not <$0.01", () => {
    // A structurally-zero component must read as a true zero, so the breakdown
    // rows still sum to the total.
    expect(formatCost(0)).toBe("$0.00")
  })

  it("clamps non-finite or negative (malformed) input to $0.00", () => {
    expect(formatCost(Number.NaN)).toBe("$0.00")
    expect(formatCost(Number.POSITIVE_INFINITY)).toBe("$0.00")
    expect(formatCost(-1)).toBe("$0.00")
  })
})

describe("costBreakdownRows", () => {
  it("maps the four billable components in display order", () => {
    const rows = costBreakdownRows({
      inputUsd: 0.3,
      outputUsd: 0.8,
      cacheReadUsd: 1.1,
      cacheWriteUsd: 0.2,
    })
    expect(rows.map((r) => r.label)).toEqual(["Input", "Output", "Cache read", "Cache write"])
    expect(rows.map((r) => r.usd)).toEqual([0.3, 0.8, 1.1, 0.2])
  })
})

describe("costFigureLabel", () => {
  it("labels a live figure a projection and a settled one an estimate", () => {
    expect(costFigureLabel(true)).toBe("Projected cost")
    expect(costFigureLabel(false)).toBe("Estimated cost")
  })
})

describe("initialContextTotal / initialContextNamedRows", () => {
  const breakdown = (
    totalTokens: number | null,
    sources: InitialContextBreakdown["sources"],
  ): InitialContextBreakdown => ({ trackingStatus: "trackedPartial", totalTokens, sources })

  it("uses the slice sum when per-source estimates overshoot the reported total", () => {
    const ic = breakdown(1000, [
      { source: "skill_instructions", sourceName: "a", tokenCount: 800 },
      { source: "system_instructions", sourceName: null, tokenCount: 700 },
    ])
    expect(initialContextTotal(ic)).toBe(1500)
  })

  it("uses the reported total when the slices agree", () => {
    const ic = breakdown(1000, [
      { source: "system_instructions", sourceName: null, tokenCount: 600 },
      { source: "unattributed", sourceName: null, tokenCount: 400 },
    ])
    expect(initialContextTotal(ic)).toBe(1000)
  })

  it("sorts named rows by contribution and rolls the tail into one Other row", () => {
    const ic = breakdown(
      null,
      ["a", "b", "c", "d", "e", "f", "g"].map((name, i) => ({
        source: "skill_instructions" as const,
        sourceName: name,
        tokenCount: (i + 1) * 100,
      })),
    )
    const rows = initialContextNamedRows(ic)
    expect(rows).toHaveLength(6)
    expect(rows[0]?.label).toBe("Skill: g")
    expect(rows[5]?.label).toBe("Other (2)")
    expect(rows[5]?.tokenCount).toBe(300) // a (100) + b (200)
  })

  it("drops unnamed and unprefixed sources from the named drill-down", () => {
    const ic = breakdown(null, [
      { source: "skill_instructions", sourceName: null, tokenCount: 900 },
      { source: "unattributed", sourceName: "baseline", tokenCount: 900 },
    ])
    expect(initialContextNamedRows(ic)).toEqual([])
  })
})

describe("median / costOutlierThreshold", () => {
  it("returns the middle value, averaging the two middles for an even list", () => {
    expect(median([3, 1, 2])).toBe(2)
    expect(median([1, 2, 3, 4])).toBe(2.5)
    expect(median([])).toBe(0)
  })

  it("does not mutate its input", () => {
    const xs = [3, 1, 2]
    median(xs)
    expect(xs).toEqual([3, 1, 2])
  })

  it("returns null below the minimum sample size", () => {
    expect(costOutlierThreshold(Array(HIGH_COST_MIN_SAMPLE - 1).fill(5))).toBeNull()
  })

  it("uses the median multiple once the median clears the floor", () => {
    expect(costOutlierThreshold(Array(HIGH_COST_MIN_SAMPLE).fill(2))).toBe(
      HIGH_COST_MEDIAN_MULTIPLE * 2,
    )
  })

  it("falls back to the absolute floor for a cheap cohort", () => {
    expect(costOutlierThreshold(Array(HIGH_COST_MIN_SAMPLE).fill(0.1))).toBe(
      HIGH_COST_FLOOR_USD,
    )
  })
})
