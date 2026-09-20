import { describe, expect, it } from "vitest"

import type {
  QuotaLanePayload,
  QuotaPeriodPayload,
  QuotaUsagePayload,
} from "../../../lib/providerUsageIpc"
import {
  assignQuotaHues,
  isQuotaRangePreset,
  isWeeklyLane,
  isWindowPreset,
  quotaBurnupSeries,
  quotaDisplayRange,
  quotaLatestPeriod,
  quotaLatestSampleEpoch,
  QUOTA_HUE_COUNT,
  QUOTA_METER_INTERPOLATION_GAP_SECS,
  QUOTA_OWN_SERIES_CAP,
  QUOTA_OWN_SERIES_MIN_PERCENT,
  quotaSessionKey,
  quotaSwatchClasses,
  quotaTopSessionRows,
  quotaUnattributedTotal,
  rangeForPreset,
  selectQuotaPeriods,
  type QuotaRange,
  type QuotaSeriesRow,
  type QuotaTopSession,
} from "./quotaSeries"

const DAY = 24 * 60 * 60
const WEEK = 7 * DAY
const BUCKET = 15 * 60
/** A `nowEpoch` well past any range these fixtures use, so clamping never trims them. */
const FAR_FUTURE = Number.MAX_SAFE_INTEGER

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
    unattributedBuckets: [],
    estimatedPercent: null,
    unexplainedBuckets: [],
    unexplainedPercent: null,
    meterCoverageUntil: null,
    meterRegressions: 0,
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
    const weekly = lane({
      currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 },
    })
    const range = rangeForPreset("thisWeek", weekly, now)
    expect(range).toEqual({ startEpoch: now - 1000, endEpoch: now + 5000 })
  })

  it("thisWeek falls back to a trailing week with no current period", () => {
    const range = rangeForPreset("thisWeek", lane(), now)
    expect(range).toEqual({ startEpoch: now - WEEK, endEpoch: now })
  })

  it("thisWeek for the five-hour lane borrows the account's weekly current period", () => {
    const fiveHour = lane({ lane: "fiveHour", label: "5-hour", currentPeriod: null })
    const weekly = lane({
      currentPeriod: { startsAtEpoch: now - 2000, resetsAtEpoch: now + 3000 },
    })
    const range = rangeForPreset("thisWeek", fiveHour, now, weekly)
    expect(range).toEqual({ startEpoch: now - 2000, endEpoch: now + 3000 })
  })

  it("thisWeek for the five-hour lane falls back with no weekly lane current period", () => {
    const fiveHour = lane({ lane: "fiveHour", label: "5-hour", currentPeriod: null })
    const range = rangeForPreset("thisWeek", fiveHour, now, lane())
    expect(range).toEqual({ startEpoch: now - WEEK, endEpoch: now })
  })

  it("lastWeek is the week before thisWeek's start", () => {
    const weekly = lane({
      currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 },
    })
    const range = rangeForPreset("lastWeek", weekly, now)
    expect(range).toEqual({ startEpoch: now - 1000 - WEEK, endEpoch: now - 1000 })
  })

  it("last30Days is a trailing thirty days regardless of lane", () => {
    const range = rangeForPreset("last30Days", lane(), now)
    expect(range).toEqual({ startEpoch: now - 30 * DAY, endEpoch: now })
  })

  it("never spans more than 70 days", () => {
    const weekly = lane({
      currentPeriod: { startsAtEpoch: now - 80 * DAY, resetsAtEpoch: now },
    })
    const range = rangeForPreset("thisWeek", weekly, now)
    expect(range.endEpoch - range.startEpoch).toBe(70 * DAY)
    expect(range.endEpoch).toBe(now)
  })

  describe("window presets on a weekly lane", () => {
    it("thisWindow is the current window's own span", () => {
      const weekly = lane({
        currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 },
      })
      expect(rangeForPreset("thisWindow", weekly, now)).toEqual({
        startEpoch: now - 1000,
        endEpoch: now + 5000,
      })
    })

    it("lastWindow is the week before the current window's own start", () => {
      const weekly = lane({
        currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 },
      })
      expect(rangeForPreset("lastWindow", weekly, now)).toEqual({
        startEpoch: now - 1000 - WEEK,
        endEpoch: now - 1000,
      })
    })

    it("last3Windows spans three weeks back from the current window's reset", () => {
      const weekly = lane({
        currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 },
      })
      expect(rangeForPreset("last3Windows", weekly, now)).toEqual({
        startEpoch: now + 5000 - 3 * WEEK,
        endEpoch: now + 5000,
      })
    })

    it("last10Windows spans ten weeks back from the current window's reset", () => {
      const weekly = lane({
        currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 5000 },
      })
      expect(rangeForPreset("last10Windows", weekly, now)).toEqual({
        startEpoch: now + 5000 - 10 * WEEK,
        endEpoch: now + 5000,
      })
    })

    it("falls back to a trailing week for this/lastWindow with no current period", () => {
      expect(rangeForPreset("thisWindow", lane(), now)).toEqual({
        startEpoch: now - WEEK,
        endEpoch: now,
      })
      expect(rangeForPreset("lastWindow", lane(), now)).toEqual({
        startEpoch: now - WEEK,
        endEpoch: now,
      })
    })

    it("falls back to a trailing N weeks for lastNWindows with no current period", () => {
      expect(rangeForPreset("last5Windows", lane(), now)).toEqual({
        startEpoch: now - 5 * WEEK,
        endEpoch: now,
      })
    })
  })

  describe("window presets on the five-hour lane", () => {
    function fiveHour(over: Partial<QuotaLanePayload> = {}): QuotaLanePayload {
      return lane({ lane: "fiveHour", label: "5-hour", ...over })
    }

    it("thisWindow is the lane's own current period when present", () => {
      const current = fiveHour({
        currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 3000 },
      })
      expect(rangeForPreset("thisWindow", current, now)).toEqual({
        startEpoch: now - 1000,
        endEpoch: now + 3000,
      })
    })

    it("thisWindow falls back to a trailing two days with no current period", () => {
      expect(rangeForPreset("thisWindow", fiveHour(), now)).toEqual({
        startEpoch: now - 2 * DAY,
        endEpoch: now,
      })
    })

    it("lastWindow always fetches a trailing two days, ignoring any current period", () => {
      const current = fiveHour({
        currentPeriod: { startsAtEpoch: now - 1000, resetsAtEpoch: now + 3000 },
      })
      expect(rangeForPreset("lastWindow", current, now)).toEqual({
        startEpoch: now - 2 * DAY,
        endEpoch: now,
      })
    })

    it("lastNWindows fetches max(2, N) trailing calendar days", () => {
      expect(rangeForPreset("last3Windows", fiveHour(), now)).toEqual({
        startEpoch: now - 3 * DAY,
        endEpoch: now,
      })
      expect(rangeForPreset("last10Windows", fiveHour(), now)).toEqual({
        startEpoch: now - 10 * DAY,
        endEpoch: now,
      })
    })

    it("a null lane selection is treated the same as the five-hour lane", () => {
      expect(rangeForPreset("thisWindow", null, now)).toEqual({
        startEpoch: now - 2 * DAY,
        endEpoch: now,
      })
    })
  })
})

describe("isWindowPreset", () => {
  it("is true only for a window preset, not a date preset or a custom range", () => {
    expect(isWindowPreset("thisWindow")).toBe(true)
    expect(isWindowPreset("last10Windows")).toBe(true)
    expect(isWindowPreset("thisWeek")).toBe(false)
    expect(isWindowPreset("last30Days")).toBe(false)
    expect(isWindowPreset({ kind: "custom", startEpoch: 0, endEpoch: 1 })).toBe(false)
  })
})

describe("isQuotaRangePreset", () => {
  it("is true for every window and date preset literal, false for anything else", () => {
    expect(isQuotaRangePreset("thisWindow")).toBe(true)
    expect(isQuotaRangePreset("last10Windows")).toBe(true)
    expect(isQuotaRangePreset("thisWeek")).toBe(true)
    expect(isQuotaRangePreset("lastWeek")).toBe(true)
    expect(isQuotaRangePreset("last30Days")).toBe(true)
    expect(isQuotaRangePreset("notAPreset")).toBe(false)
    expect(isQuotaRangePreset(undefined)).toBe(false)
    expect(isQuotaRangePreset(42)).toBe(false)
  })
})

describe("selectQuotaPeriods", () => {
  const now = 10 * WEEK
  const fetched: QuotaRange = { startEpoch: 0, endEpoch: 4 * WEEK }
  const periods = [
    period({ periodId: 1, startsAtEpoch: 0, resetsAtEpoch: WEEK }),
    period({ periodId: 2, startsAtEpoch: WEEK, resetsAtEpoch: 2 * WEEK }),
    period({ periodId: 3, startsAtEpoch: 2 * WEEK, resetsAtEpoch: 3 * WEEK }),
    period({ periodId: 4, startsAtEpoch: 3 * WEEK, resetsAtEpoch: 4 * WEEK }),
  ]

  it("keeps every period overlapping the fetched range for a date preset, sorted by start", () => {
    const shuffled = [periods[2]!, periods[0]!, periods[3]!, periods[1]!]
    const selected = selectQuotaPeriods("thisWeek", shuffled, fetched, now)
    expect(selected.map((p) => p.periodId)).toEqual([1, 2, 3, 4])
  })

  it("keeps every period overlapping the fetched range for a custom range", () => {
    const selected = selectQuotaPeriods(
      { kind: "custom", startEpoch: 0, endEpoch: 4 * WEEK },
      periods,
      fetched,
      now,
    )
    expect(selected).toHaveLength(4)
  })

  it("drops a period that does not overlap the fetched range at all", () => {
    const outside = period({ periodId: 5, startsAtEpoch: 4 * WEEK, resetsAtEpoch: 5 * WEEK })
    const selected = selectQuotaPeriods("thisWeek", [...periods, outside], fetched, now)
    expect(selected.map((p) => p.periodId)).not.toContain(5)
  })

  it("thisWindow is the latest period at or before now", () => {
    const selected = selectQuotaPeriods("thisWindow", periods, fetched, 2 * WEEK + 100)
    expect(selected.map((p) => p.periodId)).toEqual([3])
  })

  it("thisWindow falls back to the very last period once every one is in the future", () => {
    const selected = selectQuotaPeriods("thisWindow", periods, fetched, -1)
    expect(selected.map((p) => p.periodId)).toEqual([4])
  })

  it("lastWindow is the period immediately before the latest", () => {
    const selected = selectQuotaPeriods("lastWindow", periods, fetched, 2 * WEEK + 100)
    expect(selected.map((p) => p.periodId)).toEqual([2])
  })

  it("lastWindow is empty when the latest period is the first one", () => {
    const selected = selectQuotaPeriods("lastWindow", periods, fetched, WEEK - 1)
    expect(selected).toEqual([])
  })

  it("lastNWindows returns the N periods ending at the latest, inclusive", () => {
    const selected = selectQuotaPeriods("last3Windows", periods, fetched, 3 * WEEK + 100)
    expect(selected.map((p) => p.periodId)).toEqual([2, 3, 4])
  })

  it("lastNWindows returns fewer than N when fewer periods exist", () => {
    const selected = selectQuotaPeriods("last10Windows", periods, fetched, 3 * WEEK + 100)
    expect(selected.map((p) => p.periodId)).toEqual([1, 2, 3, 4])
  })

  it("returns nothing for an empty payload", () => {
    expect(selectQuotaPeriods("thisWindow", [], fetched, now)).toEqual([])
    expect(selectQuotaPeriods("thisWeek", [], fetched, now)).toEqual([])
  })
})

describe("quotaDisplayRange", () => {
  const fetched: QuotaRange = { startEpoch: 0, endEpoch: 4 * WEEK }
  const selected = [
    period({ periodId: 2, startsAtEpoch: WEEK, resetsAtEpoch: 2 * WEEK }),
    period({ periodId: 3, startsAtEpoch: 2 * WEEK, resetsAtEpoch: 3 * WEEK }),
  ]

  it("is the fetched range for a date preset", () => {
    expect(quotaDisplayRange("thisWeek", selected, fetched)).toEqual(fetched)
  })

  it("is the fetched range for a custom range", () => {
    const custom = { kind: "custom" as const, startEpoch: 0, endEpoch: WEEK }
    expect(quotaDisplayRange(custom, selected, fetched)).toEqual(fetched)
  })

  it("hugs the selected windows' own span for a window preset", () => {
    expect(quotaDisplayRange("last3Windows", selected, fetched)).toEqual({
      startEpoch: WEEK,
      endEpoch: 3 * WEEK,
    })
  })

  it("falls back to the fetched range when a window preset selected nothing", () => {
    expect(quotaDisplayRange("lastWindow", [], fetched)).toEqual(fetched)
  })

  it("sorts the selected periods before taking their span", () => {
    const reversed = [selected[1]!, selected[0]!]
    expect(quotaDisplayRange("last3Windows", reversed, fetched)).toEqual({
      startEpoch: WEEK,
      endEpoch: 3 * WEEK,
    })
  })
})

describe("quotaBurnupSeries", () => {
  it("produces a zero row at the period start", () => {
    const p = period({ startsAtEpoch: 1000, resetsAtEpoch: 1000 + WEEK })
    const series = quotaBurnupSeries(usage([p]), 1000, 1000 + WEEK, FAR_FUTURE)
    const first = series.rows[0]!
    expect(first.t).toBe(1000)
    expect(first.other).toBe(0)
    expect(first.unattributed).toBe(0)
  })

  it("emits a final-total row just before the reset that matches the last bucket", () => {
    const start = 0
    // One second past the last bucket's own end, so the final row lands
    // where that bucket has finished ramping in, not partway through it.
    const reset = 2 * BUCKET + 1
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      contributions: [
        {
          agent: "claude",
          sessionId: "s1",
          wslDistro: null,
          bucketStartEpoch: 0,
          usd: 1,
          percent: 10,
        },
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
        {
          agent: "claude",
          sessionId: "s1",
          wslDistro: null,
          title: "Fix bug",
          usd: 2,
          percent: 15,
        },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    const key = quotaSessionKey("claude", "s1", null)
    const finalRow = series.rows.find((row) => row.t === reset - 1)
    expect(finalRow).toBeDefined()
    expect(finalRow![key]).toBe(15)
  })

  it("gives a session its own series only when some period's percent exceeds the minimum", () => {
    const start = 0
    const reset = WEEK
    const sessions = [
      {
        agent: "claude",
        sessionId: "above",
        wslDistro: null,
        title: null,
        usd: 1,
        percent: QUOTA_OWN_SERIES_MIN_PERCENT + 0.1,
      },
      {
        agent: "claude",
        sessionId: "at-threshold",
        wslDistro: null,
        title: null,
        usd: 5,
        percent: QUOTA_OWN_SERIES_MIN_PERCENT,
      },
    ]
    const contributions = sessions.map((s) => ({
      agent: s.agent,
      sessionId: s.sessionId,
      wslDistro: null,
      bucketStartEpoch: 0,
      usd: s.usd,
      percent: s.percent,
    }))
    const p = period({ startsAtEpoch: start, resetsAtEpoch: reset, sessions, contributions })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    // "at-threshold" has more dollars but never exceeds the minimum, so it
    // folds into "other" despite outranking "above" by spend.
    expect(series.topSessions.map((s) => s.sessionId)).toEqual(["above"])
    // At the bucket's own end, once it has fully ramped in: "other"
    // accumulates the folded session's contributed percent, not its dollars.
    const row = series.rows.find((r) => r.t === BUCKET)!
    expect(row.other).toBe(QUOTA_OWN_SERIES_MIN_PERCENT)
  })

  it("gives a session its own series when only one of several periods exceeds the minimum", () => {
    const low = {
      agent: "claude",
      sessionId: "s1",
      wslDistro: null,
      title: null,
      usd: 1,
      percent: 0.5,
    }
    const high = { ...low, usd: 1, percent: QUOTA_OWN_SERIES_MIN_PERCENT + 5 }
    const first = period({
      startsAtEpoch: 0,
      resetsAtEpoch: WEEK,
      sessions: [low],
      contributions: [{ ...low, bucketStartEpoch: 0 }],
    })
    const second = period({
      startsAtEpoch: WEEK,
      resetsAtEpoch: 2 * WEEK,
      sessions: [high],
      contributions: [{ ...high, bucketStartEpoch: WEEK }],
    })
    const series = quotaBurnupSeries(usage([first, second]), 0, 2 * WEEK, FAR_FUTURE)
    expect(series.topSessions.map((s) => s.sessionId)).toEqual(["s1"])
  })

  it("caps own-series sessions at QUOTA_OWN_SERIES_CAP, ranked by dollars", () => {
    const start = 0
    const reset = WEEK
    const count = QUOTA_OWN_SERIES_CAP + 1
    const sessions = Array.from({ length: count }, (_, i) => ({
      agent: "claude",
      sessionId: `s${i}`,
      wslDistro: null,
      title: null,
      usd: count - i,
      percent: QUOTA_OWN_SERIES_MIN_PERCENT + 1,
    }))
    const p = period({ startsAtEpoch: start, resetsAtEpoch: reset, sessions })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    expect(series.topSessions).toHaveLength(QUOTA_OWN_SERIES_CAP)
    // The lowest-dollar qualifying session (s[count - 1], the smallest usd)
    // falls outside the cap and folds into "other".
    expect(series.topSessions.some((s) => s.sessionId === `s${count - 1}`)).toBe(false)
  })

  it("falls back to the top five sessions by dollars when the lane has no factor", () => {
    const start = 0
    const reset = WEEK
    const sessions = Array.from({ length: 7 }, (_, i) => ({
      agent: "claude",
      sessionId: `s${i}`,
      wslDistro: null,
      title: null,
      usd: 7 - i,
      percent: null,
    }))
    const p = period({ startsAtEpoch: start, resetsAtEpoch: reset, sessions })
    const series = quotaBurnupSeries(usage([p], false), start, reset, FAR_FUTURE)
    expect(series.topSessions.map((s) => s.sessionId)).toEqual(["s0", "s1", "s2", "s3", "s4"])
  })

  it("stacks own-series sessions by first appearance, bottom to top, not by dollars", () => {
    const start = 0
    const reset = 4 * BUCKET
    // Dollars run opposite to start order: "late" earns the most but starts
    // last, "early" earns the least but starts first.
    const early = {
      agent: "claude",
      sessionId: "early",
      wslDistro: null,
      title: null,
      usd: 1,
      percent: QUOTA_OWN_SERIES_MIN_PERCENT + 1,
    }
    const mid = { ...early, sessionId: "mid", usd: 2 }
    const late = { ...early, sessionId: "late", usd: 3 }
    const sessions = [early, mid, late]
    const contributions = [
      { ...early, bucketStartEpoch: 0 },
      { ...mid, bucketStartEpoch: BUCKET },
      { ...late, bucketStartEpoch: 2 * BUCKET },
    ]
    const p = period({ startsAtEpoch: start, resetsAtEpoch: reset, sessions, contributions })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    expect(series.topSessions.map((s) => s.sessionId)).toEqual(["early", "mid", "late"])
    // The list under the chart keeps dollars-descending order regardless.
    const rows = quotaTopSessionRows([p])
    expect(rows.map((r) => r.sessionId)).toEqual(["late", "mid", "early"])
  })

  it("keeps a session's earlier window in place once it reappears in a later window", () => {
    // "returning" first appears in window 1. "newcomer" appears only in
    // window 2, but at a bucket earlier than "returning"'s window-2 bucket.
    // "returning" still stacks first: its window-1 appearance decides.
    const returning = {
      agent: "claude",
      sessionId: "returning",
      wslDistro: null,
      title: null,
      usd: 1,
      percent: QUOTA_OWN_SERIES_MIN_PERCENT + 1,
    }
    const newcomer = { ...returning, sessionId: "newcomer", usd: 100 }
    const window1 = period({
      startsAtEpoch: 0,
      resetsAtEpoch: WEEK,
      sessions: [returning],
      contributions: [{ ...returning, bucketStartEpoch: 0 }],
    })
    const window2 = period({
      startsAtEpoch: WEEK,
      resetsAtEpoch: 2 * WEEK,
      sessions: [returning, newcomer],
      contributions: [
        { ...returning, bucketStartEpoch: WEEK + 2 * BUCKET },
        { ...newcomer, bucketStartEpoch: WEEK },
      ],
    })
    const series = quotaBurnupSeries(usage([window1, window2]), 0, 2 * WEEK, FAR_FUTURE)
    expect(series.topSessions.map((s) => s.sessionId)).toEqual(["returning", "newcomer"])
  })

  it("accumulates the unattributed column from its per-bucket buckets", () => {
    const start = 0
    const reset = 4 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      unattributed: { usd: 4, percent: 8, sessionCount: 1 },
      unattributedBuckets: [
        { bucketStartEpoch: 0, usd: 1, percent: 2 },
        { bucketStartEpoch: 2 * BUCKET, usd: 3, percent: 6 },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    const beforeSecondBucket = series.rows.find((row) => row.t === BUCKET)!
    expect(beforeSecondBucket.unattributed).toBe(2)
    const finalRow = series.rows.find((row) => row.t === reset - 1)!
    expect(finalRow.unattributed).toBe(8)
  })

  it("ramps a contribution's own bucket in linearly across its own 15 minutes", () => {
    const start = 0
    const reset = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      contributions: [
        {
          agent: "claude",
          sessionId: "s1",
          wslDistro: null,
          bucketStartEpoch: 0,
          usd: 1,
          percent: 30,
        },
      ],
      // Forces a row five minutes into the bucket; the bucket's own start
      // and end already land as rows on their own.
      samples: [{ observedAtEpoch: 300, usedPercent: 1, fresh: true, authoritative: true }],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    // At the bucket's own start, none of it has elapsed: the row excludes it.
    const atStart = series.rows.find((row) => row.t === 0)!
    expect(atStart.other).toBe(0)
    // Five minutes into the 15-minute bucket, a third of it has elapsed.
    const fiveMinutesIn = series.rows.find((row) => row.t === 300)!
    expect(fiveMinutesIn.other).toBeCloseTo(10, 5)
    // At the bucket's own end, the whole contribution has landed.
    const atBucketEnd = series.rows.find((row) => row.t === BUCKET)!
    expect(atBucketEnd.other).toBe(30)
  })

  it("keeps the step rule for unexplained buckets: the whole rise lands at its own stamped time", () => {
    const start = 0
    const reset = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      // The backend stamps an unexplained segment at its own end, so this
      // row's time is where the segment's reading closed it, not a bucket
      // start; the frontend still adds it in full there, unramped.
      unexplainedBuckets: [{ bucketStartEpoch: 300, usd: 0, percent: 12 }],
      samples: [{ observedAtEpoch: 299, usedPercent: 1, fresh: true, authoritative: true }],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    const justBefore = series.rows.find((row) => row.t === 299)!
    expect(justBefore.unexplained).toBe(0)
    const atStamp = series.rows.find((row) => row.t === 300)!
    expect(atStamp.unexplained).toBe(12)
  })

  it("interpolates the meter between two readings ten minutes apart", () => {
    const start = 0
    const reset = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      samples: [
        { observedAtEpoch: 600, usedPercent: 20, fresh: true, authoritative: true },
        { observedAtEpoch: 1200, usedPercent: 40, fresh: true, authoritative: true },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    const gridRow = series.rows.find((row) => row.t === BUCKET)!
    expect(gridRow.meter).toBeCloseTo(30, 5)
  })

  it("leaves the meter null across a gap wider than three hours", () => {
    const start = 0
    const gap = 2 * QUOTA_METER_INTERPOLATION_GAP_SECS
    const reset = BUCKET + gap + BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      samples: [
        { observedAtEpoch: BUCKET, usedPercent: 10, fresh: true, authoritative: true },
        { observedAtEpoch: BUCKET + gap, usedPercent: 90, fresh: true, authoritative: true },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    const midRow = series.rows.find((row) => row.t === BUCKET + gap / 2)!
    expect(midRow.meter).toBeNull()
  })

  it("ignores a non-authoritative sample for the meter line", () => {
    const start = 0
    const reset = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      samples: [
        { observedAtEpoch: BUCKET, usedPercent: 42, fresh: true, authoritative: false },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    expect(series.rows.every((row) => row.meter === null)).toBe(true)
  })

  it("still accumulates band values from a no-factor payload that carries percents", () => {
    const start = 0
    const reset = WEEK
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      contributions: [
        {
          agent: "claude",
          sessionId: "s1",
          wslDistro: null,
          bucketStartEpoch: 0,
          usd: 1,
          percent: 30,
        },
      ],
      sessions: [
        {
          agent: "claude",
          sessionId: "s1",
          wslDistro: null,
          title: null,
          usd: 1,
          percent: 30,
        },
      ],
      unattributed: { usd: 0.2, percent: 5, sessionCount: 1 },
      unattributedBuckets: [{ bucketStartEpoch: 0, usd: 0.2, percent: 5 }],
    })
    // A lane with meter readings but no learned factor still carries shared
    // percents from the backend: a band's value is the accumulated percent,
    // not gated on `usage.factor`.
    const series = quotaBurnupSeries(usage([p], false), start, reset, FAR_FUTURE)
    const key = quotaSessionKey("claude", "s1", null)
    const finalRow = series.rows.find((row) => row.t === reset - 1)!
    expect(finalRow[key]).toBe(30)
    expect(finalRow.unattributed).toBe(5)
  })

  it("fills a gap between periods with a null row so the line breaks", () => {
    const first = period({ startsAtEpoch: 0, resetsAtEpoch: BUCKET })
    const second = period({ startsAtEpoch: 3 * BUCKET, resetsAtEpoch: 4 * BUCKET })
    const series = quotaBurnupSeries(usage([first, second]), 0, 4 * BUCKET, FAR_FUTURE)
    const gapRow = series.rows.find((row) => row.t === 2 * BUCKET)
    expect(gapRow).toBeDefined()
    expect(gapRow!.other).toBeNull()
    expect(gapRow!.meter).toBeNull()
  })

  it("keeps a bucket-aligned reset from spiking into the next period's opening row", () => {
    const start = 0
    const mid = 2 * BUCKET
    const end = 4 * BUCKET
    const first = period({
      startsAtEpoch: start,
      resetsAtEpoch: mid,
      contributions: [
        {
          agent: "claude",
          sessionId: "s1",
          wslDistro: null,
          bucketStartEpoch: 0,
          usd: 1,
          percent: 50,
        },
      ],
    })
    const second = period({ startsAtEpoch: mid, resetsAtEpoch: end })
    const series = quotaBurnupSeries(usage([first, second]), start, end, FAR_FUTURE)
    // The boundary is bucket-aligned: with no fix, the first period's own
    // grid-fill would still add a row at `t === mid` (its own `reset`),
    // carrying its full accumulated total, one row before the second
    // period's own zero row at that same time.
    const rowsAtBoundary = series.rows.filter((row) => row.t === mid)
    expect(rowsAtBoundary).toHaveLength(1)
    expect(rowsAtBoundary[0]!.other).toBe(0)
    expect(series.rows.filter((row) => row.t < mid).some((row) => row.other === 50)).toBe(true)
  })

  it("stops a period's rows at now and draws nothing between now and the reset", () => {
    const start = 0
    const reset = 4 * BUCKET
    const now = 2 * BUCKET + 100
    const p = period({ startsAtEpoch: start, resetsAtEpoch: reset })
    const series = quotaBurnupSeries(usage([p]), start, reset, now)
    const lastRow = series.rows[series.rows.length - 1]!
    expect(lastRow.t).toBe(now)
    expect(series.rows.every((row) => row.t <= now)).toBe(true)
  })

  it("emits no rows for a period that starts after now", () => {
    const start = 5 * BUCKET
    const reset = 9 * BUCKET
    const now = 2 * BUCKET
    const p = period({ startsAtEpoch: start, resetsAtEpoch: reset })
    const series = quotaBurnupSeries(usage([p]), start, reset, now)
    expect(series.rows).toHaveLength(0)
  })

  it("confines every row to the given range even when the payload carries periods outside it", () => {
    // Simulates a window preset: the fetch brought back several weeks of
    // periods, but the display range narrows to just the latest one.
    const p1 = period({ periodId: 1, startsAtEpoch: 0, resetsAtEpoch: WEEK })
    const p2 = period({ periodId: 2, startsAtEpoch: WEEK, resetsAtEpoch: 2 * WEEK })
    const p3 = period({ periodId: 3, startsAtEpoch: 2 * WEEK, resetsAtEpoch: 3 * WEEK })
    const series = quotaBurnupSeries(usage([p1, p2, p3]), 2 * WEEK, 3 * WEEK, FAR_FUTURE)
    expect(series.rows.every((row) => row.t >= 2 * WEEK && row.t <= 3 * WEEK)).toBe(true)
    expect(series.rows[0]!.t).toBe(2 * WEEK)
    // The range's own end is p3's reset: a boundary zero row lands there,
    // but nothing from p1 or p2 (both entirely before the range) appears.
    expect(series.rows[series.rows.length - 1]!.t).toBe(3 * WEEK)
  })

  it("keeps the meter null at now when the only sample lands later", () => {
    const start = 0
    const reset = 4 * BUCKET
    const now = 2 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      samples: [
        { observedAtEpoch: 3 * BUCKET, usedPercent: 90, fresh: true, authoritative: true },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, now)
    const lastRow = series.rows[series.rows.length - 1]!
    expect(lastRow.t).toBe(now)
    expect(lastRow.meter).toBeNull()
  })

  it("accumulates the unexplained column from its per-bucket buckets", () => {
    const start = 0
    const reset = 4 * BUCKET
    const p = period({
      startsAtEpoch: start,
      resetsAtEpoch: reset,
      unexplainedBuckets: [
        { bucketStartEpoch: 0, usd: 0, percent: 3 },
        { bucketStartEpoch: 2 * BUCKET, usd: 0, percent: 5 },
      ],
    })
    const series = quotaBurnupSeries(usage([p]), start, reset, FAR_FUTURE)
    const beforeSecondBucket = series.rows.find((row) => row.t === BUCKET)!
    expect(beforeSecondBucket.unexplained).toBe(3)
    const finalRow = series.rows.find((row) => row.t === reset - 1)!
    expect(finalRow.unexplained).toBe(8)
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

  it("returns every own-series session, sorted by dollars, with no ten-row cap", () => {
    const sessions = Array.from({ length: 12 }, (_, i) => ({
      agent: "claude",
      sessionId: `s${i}`,
      wslDistro: null,
      title: null,
      usd: i,
      percent: i + 2,
    }))
    const rows = quotaTopSessionRows([period({ sessions })])
    expect(rows).toHaveLength(12)
    expect(rows[0]!.sessionId).toBe("s11")
    expect(rows.map((r) => r.sessionId)).toEqual(
      [...sessions].sort((a, b) => b.usd - a.usd).map((s) => s.sessionId),
    )
  })

  it("folds a non-qualifying session out of the rows entirely", () => {
    const qualifying = {
      agent: "claude",
      sessionId: "big",
      wslDistro: null,
      title: null,
      usd: 1,
      percent: QUOTA_OWN_SERIES_MIN_PERCENT + 1,
    }
    const nonQualifying = {
      agent: "claude",
      sessionId: "small",
      wslDistro: null,
      title: null,
      usd: 100,
      percent: QUOTA_OWN_SERIES_MIN_PERCENT,
    }
    const rows = quotaTopSessionRows([period({ sessions: [qualifying, nonQualifying] })])
    expect(rows.map((r) => r.sessionId)).toEqual(["big"])
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
    const earlier = period({
      startsAtEpoch: 0,
      resetsAtEpoch: WEEK,
      samples: [{ observedAtEpoch: 10, usedPercent: 1, fresh: true, authoritative: true }],
    })
    const later = period({
      startsAtEpoch: WEEK,
      resetsAtEpoch: 2 * WEEK,
      samples: [
        { observedAtEpoch: WEEK + 20, usedPercent: 2, fresh: true, authoritative: true },
      ],
    })
    expect(quotaLatestPeriod([earlier, later])).toBe(later)
    expect(quotaLatestSampleEpoch([earlier, later])).toBe(WEEK + 20)
  })

  it("returns null for an empty period list", () => {
    expect(quotaLatestPeriod([])).toBeNull()
    expect(quotaLatestSampleEpoch([])).toBeNull()
  })
})

describe("assignQuotaHues", () => {
  function topSession(key: string): QuotaTopSession {
    return {
      key,
      agent: "claude",
      sessionId: key,
      wslDistro: null,
      title: null,
      usd: 0,
      hue: 0,
    }
  }

  /** A minimal series row: `t` and any session values the test needs, with
   *  every other field defaulting to null. */
  function seriesRow(t: number, values: Record<string, number | null> = {}): QuotaSeriesRow {
    return {
      t,
      index: 0,
      meter: null,
      other: null,
      unattributed: null,
      unexplained: null,
      ...values,
    }
  }

  it("gives two sessions different hues once the first is still cumulative when the second starts, even though their own contributions never overlap in time", () => {
    // "a" contributes only at t=0, then stays cumulative (and so visible)
    // for the rest of the window. "b" contributes only much later. Their
    // own contribution buckets never overlap, but the row where "b" starts
    // still shows "a" active, so the bands touch in the stack.
    const sessions = [topSession("a"), topSession("b")]
    const rows = [
      seriesRow(0, { a: 10, b: null }),
      seriesRow(BUCKET, { a: 10, b: null }),
      seriesRow(10 * BUCKET, { a: 10, b: 5 }),
      seriesRow(11 * BUCKET, { a: 10, b: 5 }),
    ]
    const hues = assignQuotaHues(sessions, rows)
    expect(hues.get("a")).not.toBe(hues.get("b"))
  })

  it("lets two sessions that are never stack-adjacent share a hue, when a session between them is active at every row where both are", () => {
    const sessions = [topSession("a"), topSession("m"), topSession("b")]
    const rows = [
      seriesRow(0, { a: 10, m: null, b: null }),
      seriesRow(BUCKET, { a: 10, m: 5, b: null }),
      seriesRow(2 * BUCKET, { a: 10, m: 5, b: 3 }),
      seriesRow(3 * BUCKET, { a: 10, m: 5, b: 3 }),
    ]
    const hues = assignQuotaHues(sessions, rows)
    // "a" and "b" never sit next to each other in the active list: "m" is
    // always between them, so only a-m and m-b are conflict edges.
    expect(hues.get("a")).toBe(hues.get("b"))
    expect(hues.get("m")).not.toBe(hues.get("a"))
  })

  it("sends a forced clash to the hue whose neighbor touches for the fewest rows", () => {
    // "a" and "b" touch directly (never coexisting with "x"), so they take
    // different hues. "x" then touches both, more rows against "a" than
    // against "b". With only two hues, "x" must clash with one of them —
    // it should pick "b", the neighbor it touches for fewer rows.
    const sessions = [topSession("a"), topSession("b"), topSession("x")]
    const rows = [
      seriesRow(0, { a: 10, b: 5, x: null }),
      seriesRow(BUCKET, { a: 10, b: 5, x: null }),
      seriesRow(2 * BUCKET, { a: 10, b: null, x: 3 }),
      seriesRow(3 * BUCKET, { a: 10, b: null, x: 3 }),
      seriesRow(4 * BUCKET, { a: 10, b: null, x: 3 }),
      seriesRow(5 * BUCKET, { a: 10, b: null, x: 3 }),
      seriesRow(6 * BUCKET, { a: 10, b: null, x: 3 }),
      seriesRow(7 * BUCKET, { a: null, b: 5, x: 3 }),
      seriesRow(8 * BUCKET, { a: null, b: 5, x: 3 }),
    ]
    const hues = assignQuotaHues(sessions, rows, 2)
    expect(hues.get("a")).not.toBe(hues.get("b"))
    expect(hues.get("x")).toBe(hues.get("b"))
  })

  it("keeps every hue in range, and quotaSwatchClasses maps eight hues to eight distinct classes", () => {
    // A full conflict clique across eight sessions: every pair touches in
    // its own row, so the greedy assignment needs every one of the eight
    // hues, processed in `sessions` order.
    const sessions = Array.from({ length: 8 }, (_, i) => topSession(`s${i}`))
    const rows: QuotaSeriesRow[] = []
    let t = 0
    for (let i = 0; i < sessions.length; i++) {
      for (let j = i + 1; j < sessions.length; j++) {
        const values: Record<string, number | null> = {}
        sessions.forEach((session, k) => {
          values[session.key] = k === i || k === j ? 10 : null
        })
        rows.push(seriesRow(t, values))
        t += BUCKET
      }
    }
    const hues = assignQuotaHues(sessions, rows)
    for (const session of sessions) {
      const hue = hues.get(session.key)!
      expect(hue).toBeGreaterThanOrEqual(0)
      expect(hue).toBeLessThan(QUOTA_HUE_COUNT)
      session.hue = hue
    }
    const classes = quotaSwatchClasses(sessions)
    const distinctClasses = new Set(sessions.map((session) => classes[session.key]))
    expect(distinctClasses.size).toBe(8)
  })

  it("gives a session with no rows above zero a hue, without throwing", () => {
    const sessions = [topSession("a")]
    expect(() => assignQuotaHues(sessions, [])).not.toThrow()
    const hues = assignQuotaHues(sessions, [seriesRow(0, { a: null })])
    expect(hues.get("a")).toBe(0)
  })
})
