import { describe, expect, it } from "vitest"

import type {
  AllowanceOveragePayload,
  AllowanceUsageAccountPayload,
  AllowanceUsageSummaryPayload,
  AllowanceUtilizationPayload,
} from "../../../lib/providerUsageIpc"
import {
  allowanceAccounts,
  limitHitsCaption,
  limitHitsFigure,
  limitHitsNote,
  causeLine,
  hasAllowanceFigures,
  limitHitsTooltip,
  utilizationFigure,
  utilizationTooltip,
} from "./overviewAllowance"

function utilization(
  overrides: Partial<AllowanceUtilizationPayload> = {},
): AllowanceUtilizationPayload {
  return {
    typicalPercent: 40,
    peakPercent: 62,
    averagePercent: 41.4,
    periodCount: 9,
    maxedPeriodCount: 0,
    windowKind: "weekly",
    firstPeriodAt: "2026-07-13T00:00:00Z",
    lastPeriodAt: "2026-09-14T00:00:00Z",
    ...overrides,
  }
}

function account(
  overrides: Partial<AllowanceUsageAccountPayload> = {},
): AllowanceUsageAccountPayload {
  return {
    provider: "anthropic",
    displayName: "Claude",
    accountKey: "account",
    utilization: utilization(),
    burst: null,
    overage: {
      blockCount: 0,
      waitedSeconds: 0,
      blocksWithoutWait: 0,
      lastBlockAt: null,
    },
    days: [],
    previousDays: [],
    ...overrides,
  }
}

describe("utilizationFigure", () => {
  it("states the average share of the subscription the meter reports", () => {
    expect(utilizationFigure(utilization())).toBe("41%")
  })

  it("states a figure at any sample size, because a mean needs no sample floor", () => {
    // The median collapses to null below six periods. The average does not,
    // so one period still states what that period used.
    expect(
      utilizationFigure(
        utilization({ typicalPercent: null, averagePercent: 62, periodCount: 1 }),
      ),
    ).toBe("62%")
  })
})

describe("utilizationTooltip", () => {
  it("names the window, the span it covers, and what the meter leaves out", () => {
    expect(utilizationTooltip(utilization())).toBe(
      "The average share of your plan used in one week, read from the " +
        "provider's own meter. It covers all 9 weeks antiburn has readings " +
        "for. The meter stops at 100%, so it never counts the demand the " +
        "provider refused.",
    )
  })

  it("follows the window the store names", () => {
    const rolling = utilizationTooltip(utilization({ windowKind: "rolling", periodCount: 18 }))
    expect(rolling).toContain("used in one window")
    expect(rolling).toContain("all 18 windows")
  })
})

function overage(overrides: Partial<AllowanceOveragePayload> = {}): AllowanceOveragePayload {
  return {
    blockCount: 2,
    waitedSeconds: 8220,
    blocksWithoutWait: 0,
    lastBlockAt: "2026-09-14T03:43:00Z",
    ...overrides,
  }
}

describe("limitHitsFigure", () => {
  it("reads a long wait in hours and a short one in minutes", () => {
    expect(limitHitsFigure(overage())).toBe("2.3h")
    expect(limitHitsFigure(overage({ waitedSeconds: 2700 }))).toBe("45m")
    expect(limitHitsFigure(overage({ blockCount: 12, waitedSeconds: 39_960 }))).toBe("11h")
  })

  it("counts the limit hits when none states a reset", () => {
    // A limit hit that reports no usable reset is counted and adds no time.
    // "0h" would claim the reader waited no time, which is false.
    expect(
      limitHitsFigure(overage({ blockCount: 1, waitedSeconds: 0, blocksWithoutWait: 1 })),
    ).toBe("1")
    expect(limitHitsFigure(overage({ blockCount: 0, waitedSeconds: 0 }))).toBe("0")
  })
})

describe("limitHitsCaption and limitHitsNote", () => {
  it("names the wait, and the limit hits it covers, over the span", () => {
    expect(limitHitsCaption(overage(), 30)).toBe("waiting on 2 limit hits in 30 days")
    expect(limitHitsCaption(overage({ blockCount: 1 }), 30)).toBe(
      "waiting on 1 limit hit in 30 days",
    )
  })

  it("leaves the count to the figure when the figure is the count", () => {
    const counted = overage({ blockCount: 1, waitedSeconds: 0, blocksWithoutWait: 1 })
    expect(limitHitsCaption(counted, 30)).toBe("limit hit in 30 days")
  })

  it("says nothing when every limit hit states a reset", () => {
    expect(limitHitsNote(overage())).toBeNull()
  })

  it("names the limit hits the wait leaves out", () => {
    expect(limitHitsNote(overage({ blockCount: 3, blocksWithoutWait: 1 }))).toBe(
      "1 with no stated reset",
    )
    expect(
      limitHitsNote(overage({ blockCount: 1, waitedSeconds: 0, blocksWithoutWait: 1 })),
    ).toBe("no stated reset")
  })
})

describe("limitHitsTooltip", () => {
  it("says the figure is a wait, and what the wait leaves out", () => {
    expect(limitHitsTooltip(overage(), 30)).toBe(
      "How long you waited for the limit to reset, across 2 limit hits in " +
        "the last 30 days. A run of retries counts as one. A limit hit that " +
        "states no reset adds no time to this figure.",
    )
  })

  it("says the figure is a count when no limit hit states a reset", () => {
    const counted = overage({ blockCount: 1, waitedSeconds: 0, blocksWithoutWait: 1 })
    expect(limitHitsTooltip(counted, 30)).toBe(
      "1 limit hit in the last 30 days. The figure counts them instead of " +
        "timing them, because none of them stated when the limit resets. A " +
        "run of retries counts as one.",
    )
  })

  it("says the provider refused nothing rather than describing a wait", () => {
    expect(limitHitsTooltip(overage({ blockCount: 0, waitedSeconds: 0 }), 30)).toBe(
      "How many times the provider refused a request in the last 30 days. It refused none.",
    )
  })
})

describe("causeLine", () => {
  it("counts only the windows that reached the ceiling", () => {
    const burst = utilization({ windowKind: "rolling", periodCount: 18, maxedPeriodCount: 1 })
    expect(causeLine(burst)).toBe("1 of 18 windows reached 100%")
  })

  it("says nothing when no window reached the ceiling", () => {
    // A refusal happens at 100% and at nothing less. A window at 99% refused
    // nothing, so there is no threshold below the ceiling to report.
    const burst = utilization({ windowKind: "rolling", peakPercent: 99, maxedPeriodCount: 0 })
    expect(causeLine(burst)).toBeNull()
  })

  it("says nothing when no short window was recorded", () => {
    expect(causeLine(null)).toBeNull()
  })
})

describe("allowanceAccounts", () => {
  it("drops an account with neither figure and keeps one with either", () => {
    const summary: AllowanceUsageSummaryPayload = {
      accounts: [
        account({ accountKey: "empty", utilization: null }),
        account({ accountKey: "metered" }),
        account({
          accountKey: "blocked",
          utilization: null,
          overage: {
            blockCount: 3,
            waitedSeconds: 100,
            blocksWithoutWait: 0,
            lastBlockAt: "2026-09-14T03:43:00Z",
          },
        }),
      ],
      overageSpanDays: 30,
      generatedAt: "2026-09-15T00:00:00Z",
    }

    expect(allowanceAccounts(summary).map((entry) => entry.accountKey)).toEqual([
      "metered",
      "blocked",
    ])
  })

  it("has nothing to show before the first read", () => {
    expect(allowanceAccounts(null)).toEqual([])
  })

  it("counts an account with a meter but no limit hit", () => {
    expect(hasAllowanceFigures(account())).toBe(true)
  })
})
