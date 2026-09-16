import { describe, expect, it } from "vitest"

import type {
  QuotaLanePayload,
  QuotaPeriodPayload,
  QuotaUsagePayload,
} from "../../../lib/providerUsageIpc"
import {
  isWeeklyLane,
  quotaBurnupSeries,
  quotaLatestPeriod,
  quotaLatestSampleEpoch,
  quotaSessionKey,
  quotaTopSessionRows,
  quotaUnattributedTotal,
  rangeForPreset,
} from "./quotaSeries"

const DAY = 24 * 60 * 60
const WEEK = 7 * DAY
const BUCKET = 15 * 60

function lane(over: Partial<QuotaLanePayload> = {}): QuotaLanePayload {
  return { lane: "weekly", label: "Weekly", hasFactor: true, currentPeriod: null, ...over }
}

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
    estimatedPercent: null,
    ...over,
  }
}

function usage(periods: QuotaPeriodPayload[], hasFactor = true): QuotaUsagePayload {
  return {
    provider: "anthropic",
    accountKey: "acct",
    lane: "weekly",
    laneLabel: "Weekly",
    rangeStartEpoch: 0,
    rangeEndEpoch: WEEK,
    factor: hasFactor ? { usdPerPercent: 1, confidence: "learned" } : null,
    periods,
    generatedAt: "now",
  }
}

describe("isWeeklyLane", () => {
  it("is true for weekly and model-scoped lanes, false for five-hour", () => {
    expect(isWeeklyLane("weekly")).toBe(true)
    expect(isWeeklyLane("model:claude-fable")).toBe(true)
    expect(isWeeklyLane("fiveHour")).toBe(false)
  })
})

describe("rangeForPreset", () => {
  const now = 10 * WEEK

  it("thisWeek uses the weekly lane's current period when present", () => {
    const weekly = lane({ currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 } })
    const range = rangeForPreset("thisWeek", weekly, now)
    expect(range).toEqual({ startEpoch: now - 1000, endEpoch: now + 5000 })
  })

  it("thisWeek falls back to a trailing week with no current period", () => {
    const range = rangeForPreset("thisWeek", lane(), now)
    expect(range).toEqual({ startEpoch: now - WEEK, endEpoch: now })
  })

  it("thisWeek for the five-hour lane borrows the account's weekly current period", () => {
    const fiveHour = lane({ lane: "fiveHour", label: "5-hour", currentPeriod: null })
    const weekly = lane({ currentPeriod: { startsAtEpoch: now - 2000, resetsAtEpoch: now + 3000 } })
    const range = rangeForPreset("thisWeek", fiveHour, now, weekly)
    expect(range).toEqual({ startEpoch: now - 2000, endEpoch: now + 3000 })
  })

  it("thisWeek for the five-hour lane falls back with no weekly lane current period", () => {
    const fiveHour = lane({ lane: "fiveHour", label: "5-hour", currentPeriod: null })
    const range = rangeForPreset("thisWeek", fiveHour, now, lane())
    expect(range).toEqual({ startEpoch: now - WEEK, endEpoch: now })
  })

  it("lastWeek is the week before thisWeek's start", () => {
    const weekly = lane({ currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 } })
    const range = rangeForPreset("lastWeek", weekly, now)
    expect(range).toEqual({ startEpoch: now - 1000 - WEEK, endEpoch: now - 1000 })
  })

  it("last30Days is a trailing thirty days regardless of lane", () => {
    const range = rangeForPreset("last30Days", lane(), now)
    expect(range).toEqual({ startEpoch: now - 30 * DAY, endEpoch: now })
  })

  it("never spans more than 35 days", () => {
    const weekly = lane({ currentPeriod: { startsAtEpoch: now - 40 * DAY, resetsAtEpoch: now } })
    const range = rangeForPreset("thisWeek", weekly, now)
    expect(range.endEpoch - range.startEpoch).toBe(35 * DAY)
    expect(range.endEpoch).toBe(now)
  })
})

describe("quotaBurnupSeries", () => {
  it("produces a zero row at the period start", () => {
    const p = period({ startsAtEpoch: 1000, resetsAtEpoch: 1000 + WEEK })
    const series = quotaBurnupSeries(usage([p]), 1000, 1000 + WEEK)
    const first = series.rows[0]!
    expect(first.t).toBe(1000)
    expect(first.other).toBe(0)
    expect(first.unattributed).toBe(0)
  })

  it("emits a final-total row just before the reset that matches the last bucket", () => {
    const start = 0
    const reset = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      contributions: [
        { agent: "claude", sessionId: "s1", wslDistro: null, bucketStartEpoch: 0, usd: 1, percent: 10 },
        {
          agent: "claude",
          sessionId: "s1",
          wslDistro: null,
          bucketStartEpoch: BUCKET,
          usd: 1,
          percent: 5,
        },
      ],
      sessions: [
        { agent: "claude", sessionId: "s1", wslDistro: null, title: "Fix bug", usd: 2, percent: 15 },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset)
    const key = quotaSessionKey("claude", "s1", null)
    const finalRow = series.rows.find((row) => row.t === reset - 1)
    expect(finalRow).toBeDefined()
    expect(finalRow![key]).toBe(15)
  })

  it("keeps the top five sessions by dollars and absorbs the rest into other", () => {
    const start = 0
    const reset = WEEK
    const sessions = Array.from({ length: 7 }, (_, i) => ({
      agent: "claude",
      sessionId: `s${i}`,
      wslDistro: null,
      title: null,
      usd: 7 - i,
      percent: 7 - i,
    }))
    const contributions = sessions.map((s) => ({
      agent: s.agent,
      sessionId: s.sessionId,
      wslDistro: null,
      bucketStartEpoch: 0,
      usd: s.usd,
      percent: s.percent,
    }))
    const p = period({ startsAtEpoch: start, resetsAtEpoch: reset, sessions, contributions })
    const series = quotaBurnupSeries(usage([p]), start, reset)
    expect(series.topSessions).toHaveLength(5)
    expect(series.topSessions.map((s) => s.sessionId)).toEqual(["s0", "s1", "s2", "s3", "s4"])
    const row = series.rows.find((r) => r.t === 0)!
    // s5 (2) and s6 (1) are absorbed into "other".
    expect(row.other).toBe(3)
  })

  it("ramps the unattributed column across the period from its total", () => {
    const start = 0
    const reset = 4 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      unattributed: { usd: 4, percent: 8, sessionCount: 1 },
    })
    const series = quotaBurnupSeries(usage([p]), start, reset)
    const midRow = series.rows.find((row) => row.t === 2 * BUCKET)!
    expect(midRow.unattributed).toBeCloseTo(4, 5)
    const finalRow = series.rows.find((row) => row.t === reset - 1)!
    expect(finalRow.unattributed).toBeGreaterThan(7.9)
  })

  it("breaks the meter line where there is no reading", () => {
    const start = 0
    const reset = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      samples: [{ observedAtEpoch: BUCKET, usedPercent: 42, fresh: true, authoritative: true }],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset)
    const withReading = series.rows.find((row) => row.t === BUCKET)!
    expect(withReading.meter).toBe(42)
    const withoutReading = series.rows.filter((row) => row.t !== BUCKET)
    expect(withoutReading.every((row) => row.meter === null)).toBe(true)
  })

  it("ignores a non-authoritative sample for the meter line", () => {
    const start = 0
    const reset = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      samples: [{ observedAtEpoch: BUCKET, usedPercent: 42, fresh: true, authoritative: false }],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset)
    expect(series.rows.every((row) => row.meter === null)).toBe(true)
  })

  it("nulls every percent column when the lane has no factor", () => {
    const start = 0
    const reset = WEEK
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      contributions: [
        { agent: "claude", sessionId: "s1", wslDistro: null, bucketStartEpoch: 0, usd: 1, percent: null },
      ],
      sessions: [
        { agent: "claude", sessionId: "s1", wslDistro: null, title: null, usd: 1, percent: null },
      ],
      unattributed: { usd: 0, percent: null, sessionCount: 0 },
    })
    const series = quotaBurnupSeries(usage([p], false), start, reset)
    const key = quotaSessionKey("claude", "s1", null)
    for (const row of series.rows) {
      expect(row.other).toBeNull()
      expect(row.unattributed).toBeNull()
      expect(row[key]).toBeNull()
    }
  })

  it("fills a gap between periods with a null row so the line breaks", () => {
    const first = period({ startsAtEpoch: 0, resetsAtEpoch: BUCKET })
    const second = period({ startsAtEpoch: 3 * BUCKET, resetsAtEpoch: 4 * BUCKET })
    const series = quotaBurnupSeries(usage([first, second]), 0, 4 * BUCKET)
    const gapRow = series.rows.find((row) => row.t === 2 * BUCKET)
    expect(gapRow).toBeDefined()
    expect(gapRow!.other).toBeNull()
    expect(gapRow!.meter).toBeNull()
  })
})

describe("quotaTopSessionRows", () => {
  it("merges the same session across periods and counts periods touched", () => {
    const session = {
      agent: "claude",
      sessionId: "s1",
      wslDistro: null,
      title: "Fix bug",
      usd: 2,
      percent: 4,
    }
    const rows = quotaTopSessionRows([
      period({ sessions: [session] }),
      period({ sessions: [{ ...session, usd: 1, percent: 2 }] }),
    ])
    expect(rows).toHaveLength(1)
    expect(rows[0]!.usd).toBe(3)
    expect(rows[0]!.percent).toBe(6)
    expect(rows[0]!.periodCount).toBe(2)
  })

  it("caps at ten rows, sorted by dollars", () => {
    const sessions = Array.from({ length: 12 }, (_, i) => ({
      agent: "claude",
      sessionId: `s${i}`,
      wslDistro: null,
      title: null,
      usd: i,
      percent: i,
    }))
    const rows = quotaTopSessionRows([period({ sessions })])
    expect(rows).toHaveLength(10)
    expect(rows[0]!.sessionId).toBe("s11")
  })
})

describe("quotaUnattributedTotal", () => {
  it("sums usd, percent, and session count across periods", () => {
    const total = quotaUnattributedTotal([
      period({ unattributed: { usd: 1, percent: 2, sessionCount: 1 } }),
      period({ unattributed: { usd: 3, percent: 4, sessionCount: 2 } }),
    ])
    expect(total).toEqual({ usd: 4, percent: 6, sessionCount: 3 })
  })

  it("goes null when any period's percent is unknown", () => {
    const total = quotaUnattributedTotal([
      period({ unattributed: { usd: 1, percent: null, sessionCount: 0 } }),
    ])
    expect(total.percent).toBeNull()
  })
})

describe("quotaLatestPeriod and quotaLatestSampleEpoch", () => {
  it("picks the period with the latest start and the latest sample overall", () => {
    const earlier = period({ startsAtEpoch: 0, resetsAtEpoch: WEEK, samples: [
      { observedAtEpoch: 10, usedPercent: 1, fresh: true, authoritative: true },
    ] })
    const later = period({ startsAtEpoch: WEEK, resetsAtEpoch: 2 * WEEK, samples: [
      { observedAtEpoch: WEEK + 20, usedPercent: 2, fresh: true, authoritative: true },
    ] })
    expect(quotaLatestPeriod([earlier, later])).toBe(later)
    expect(quotaLatestSampleEpoch([earlier, later])).toBe(WEEK + 20)
  })

  it("returns null for an empty period list", () => {
    expect(quotaLatestPeriod([])).toBeNull()
    expect(quotaLatestSampleEpoch([])).toBeNull()
  })
})
