/**
 * Pure builders for the Quota screen: the range presets a reader can choose,
 * and the burnup series a chosen range and lane draw. Nothing here reads the
 * clock or calls IPC; `now` and the payload always come from the caller, so
 * every rule stays a plain, testable function.
 */

import type {
  QuotaContributionPayload,
  QuotaLanePayload,
  QuotaPeriodPayload,
  QuotaUnattributedPayload,
  QuotaUsagePayload,
} from "../../../lib/providerUsageIpc"

/** One 15-minute bucket, in seconds. Matches the backend's contribution grain. */
export const QUOTA_BUCKET_SECS = 15 * 60
const DAY_SECS = 24 * 60 * 60
const WEEK_SECS = 7 * DAY_SECS
/** The request never spans more than this many days, whatever the preset computes. */
const MAX_RANGE_DAYS = 35

export type QuotaRangePreset = "thisWeek" | "lastWeek" | "last30Days"

export interface QuotaRange {
  startEpoch: number
  endEpoch: number
}

/** True for the weekly lane and every model-scoped weekly lane, such as Claude's "Fable" window. */
export function isWeeklyLane(lane: string): boolean {
  return lane === "weekly" || lane.startsWith("model:")
}

/** Keep a range inside the request's hard cap, trimming from the start. */
function capRange(range: QuotaRange): QuotaRange {
  const maxSpan = MAX_RANGE_DAYS * DAY_SECS
  if (range.endEpoch - range.startEpoch <= maxSpan) return range
  return { startEpoch: range.endEpoch - maxSpan, endEpoch: range.endEpoch }
}

/**
 * The wall-clock range a preset covers for the selected lane.
 *
 * `thisWeek` and `lastWeek` anchor to the weekly lane's currently open
 * window when one exists: the selected lane's own window when it is weekly,
 * or `weeklyLane`'s window when the selection is the five-hour lane. With no
 * open weekly window, both fall back to a plain trailing week.
 */
export function rangeForPreset(
  preset: QuotaRangePreset,
  lane: QuotaLanePayload | null,
  now: number,
  weeklyLane?: QuotaLanePayload | null,
): QuotaRange {
  if (preset === "last30Days") {
    return capRange({ startEpoch: now - 30 * DAY_SECS, endEpoch: now })
  }
  const currentWeekly =
    (lane && isWeeklyLane(lane.lane) ? lane.currentPeriod : null) ??
    (weeklyLane ? weeklyLane.currentPeriod : null)
  if (preset === "thisWeek") {
    if (currentWeekly) {
      return capRange({
        startEpoch: currentWeekly.startsAtEpoch,
        endEpoch: currentWeekly.resetsAtEpoch,
      })
    }
    return capRange({ startEpoch: now - WEEK_SECS, endEpoch: now })
  }
  // lastWeek: the week before thisWeek's own start.
  const thisWeek = rangeForPreset("thisWeek", lane, now, weeklyLane)
  return capRange({
    startEpoch: thisWeek.startEpoch - WEEK_SECS,
    endEpoch: thisWeek.startEpoch,
  })
}

/** Stable identity for one session inside the burnup series and the top-sessions list. */
export function quotaSessionKey(
  agent: string,
  sessionId: string,
  wslDistro: string | null,
): string {
  return JSON.stringify([agent, sessionId, wslDistro ?? null])
}

export interface QuotaTopSession {
  key: string
  agent: string
  sessionId: string
  wslDistro: string | null
  title: string | null
  usd: number
}

/** The top five sessions by dollars across every period in the range. */
function topSessionsAcross(periods: readonly QuotaPeriodPayload[]): QuotaTopSession[] {
  const totals = new Map<string, QuotaTopSession>()
  for (const period of periods) {
    for (const session of period.sessions) {
      const key = quotaSessionKey(session.agent, session.sessionId, session.wslDistro)
      const existing = totals.get(key)
      if (existing) {
        existing.usd += session.usd
        if (session.title) existing.title = session.title
      } else {
        totals.set(key, {
          key,
          agent: session.agent,
          sessionId: session.sessionId,
          wslDistro: session.wslDistro,
          title: session.title,
          usd: session.usd,
        })
      }
    }
  }
  return [...totals.values()].sort((left, right) => right.usd - left.usd).slice(0, 5)
}

/** One row of the burnup series. Session columns are added by key, dynamically. */
export interface QuotaSeriesRow {
  t: number
  index: number
  meter: number | null
  other: number | null
  unattributed: number | null
  [sessionKey: string]: number | null
}

export interface QuotaSeries {
  rows: QuotaSeriesRow[]
  topSessions: QuotaTopSession[]
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0
  return Math.min(1, Math.max(0, value))
}

/** The next absolute 15-minute mark at or after `t`, on the same grain the backend buckets use. */
function ceilToBucket(t: number): number {
  return Math.ceil(t / QUOTA_BUCKET_SECS) * QUOTA_BUCKET_SECS
}

function zeroRow(
  t: number,
  hasFactor: boolean,
  topSessions: readonly QuotaTopSession[],
): QuotaSeriesRow {
  const row: QuotaSeriesRow = {
    t,
    index: 0,
    meter: null,
    other: hasFactor ? 0 : null,
    unattributed: hasFactor ? 0 : null,
  }
  for (const session of topSessions) row[session.key] = hasFactor ? 0 : null
  return row
}

function gapRow(t: number, topSessions: readonly QuotaTopSession[]): QuotaSeriesRow {
  const row: QuotaSeriesRow = { t, index: 0, meter: null, other: null, unattributed: null }
  for (const session of topSessions) row[session.key] = null
  return row
}

/**
 * One period's rows: a zero row at its visible start, one row per bucket
 * that carries a contribution, a final-total row just before the reset, and
 * an authoritative meter reading wherever one was observed. Contributions
 * accumulate from the period's own start, never the range's.
 */
function periodRows(
  period: QuotaPeriodPayload,
  rangeStart: number,
  rangeEnd: number,
  hasFactor: boolean,
  topKeys: ReadonlySet<string>,
  topSessions: readonly QuotaTopSession[],
): QuotaSeriesRow[] {
  const start = period.startsAtEpoch
  const reset = period.resetsAtEpoch
  const visibleStart = Math.max(start, rangeStart)
  const visibleEnd = Math.min(reset, rangeEnd)
  if (visibleEnd <= visibleStart) return []

  const byBucket = new Map<number, QuotaContributionPayload[]>()
  for (const contribution of period.contributions) {
    if (contribution.bucketStartEpoch < start || contribution.bucketStartEpoch >= reset)
      continue
    const list = byBucket.get(contribution.bucketStartEpoch) ?? []
    list.push(contribution)
    byBucket.set(contribution.bucketStartEpoch, list)
  }
  const bucketTimes = [...byBucket.keys()].sort((left, right) => left - right)

  const points = new Set<number>()
  if (start >= rangeStart) points.add(start)
  for (let t = ceilToBucket(visibleStart); t <= visibleEnd; t += QUOTA_BUCKET_SECS)
    points.add(t)
  for (const bucketTime of bucketTimes) {
    if (bucketTime >= visibleStart && bucketTime <= visibleEnd) points.add(bucketTime)
  }
  const finalRowTime = reset - 1
  if (finalRowTime >= visibleStart && finalRowTime <= rangeEnd && finalRowTime >= start) {
    points.add(finalRowTime)
  }
  for (const sample of period.samples) {
    if (!sample.authoritative) continue
    if (sample.observedAtEpoch < start || sample.observedAtEpoch >= reset) continue
    if (sample.observedAtEpoch < rangeStart || sample.observedAtEpoch > rangeEnd) continue
    points.add(sample.observedAtEpoch)
  }

  const sortedPoints = [...points].sort((left, right) => left - right)
  const topCumulative = new Map(topSessions.map((session) => [session.key, 0]))
  let otherCumulative = 0
  let bucketPointer = 0
  const unattributedPercent = period.unattributed.percent
  const span = Math.max(1, reset - start)

  const rows: QuotaSeriesRow[] = []
  for (const t of sortedPoints) {
    while (bucketPointer < bucketTimes.length && bucketTimes[bucketPointer]! <= t) {
      const bucketTime = bucketTimes[bucketPointer]!
      for (const contribution of byBucket.get(bucketTime) ?? []) {
        const key = quotaSessionKey(
          contribution.agent,
          contribution.sessionId,
          contribution.wslDistro,
        )
        const percent = contribution.percent ?? 0
        if (topKeys.has(key)) {
          topCumulative.set(key, (topCumulative.get(key) ?? 0) + percent)
        } else {
          otherCumulative += percent
        }
      }
      bucketPointer += 1
    }
    const meterSample = period.samples.find(
      (sample) => sample.authoritative && sample.observedAtEpoch === t,
    )
    const row: QuotaSeriesRow = {
      t,
      index: 0,
      meter: meterSample ? meterSample.usedPercent : null,
      other: hasFactor ? otherCumulative : null,
      unattributed:
        hasFactor && unattributedPercent != null
          ? unattributedPercent * clamp01((t - start) / span)
          : hasFactor
            ? 0
            : null,
    }
    for (const session of topSessions) {
      row[session.key] = hasFactor ? (topCumulative.get(session.key) ?? 0) : null
    }
    rows.push(row)
  }
  return rows
}

/**
 * The burnup series for one lane's usage over a range: every period's rows,
 * a reset-to-zero row at each boundary, and a gap row wherever the range
 * holds no period, so the plot breaks instead of guessing.
 */
export function quotaBurnupSeries(
  usage: QuotaUsagePayload,
  rangeStart: number,
  rangeEnd: number,
): QuotaSeries {
  const hasFactor = usage.factor != null
  const periods = [...usage.periods].sort(
    (left, right) => left.startsAtEpoch - right.startsAtEpoch,
  )
  const topSessions = topSessionsAcross(periods)
  const topKeys = new Set(topSessions.map((session) => session.key))

  const rows: QuotaSeriesRow[] = []
  const covered: Array<[number, number]> = []
  for (const period of periods) {
    if (period.resetsAtEpoch <= rangeStart || period.startsAtEpoch >= rangeEnd) continue
    covered.push([
      Math.max(period.startsAtEpoch, rangeStart),
      Math.min(period.resetsAtEpoch, rangeEnd),
    ])
    rows.push(...periodRows(period, rangeStart, rangeEnd, hasFactor, topKeys, topSessions))
  }

  // A reset row at each boundary still inside the range. A period that
  // starts exactly where another resets already carries its own zero row,
  // so this loop must not draw a second one on top of it.
  for (const period of periods) {
    const t = period.resetsAtEpoch
    if (t < rangeStart || t > rangeEnd) continue
    const opensNextPeriod = periods.some((candidate) => candidate.startsAtEpoch === t)
    if (opensNextPeriod) continue
    rows.push(zeroRow(t, hasFactor, topSessions))
  }

  // Fill every 15-minute mark the range holds that no period covers, so the
  // line breaks there instead of drawing a flat guess across the gap.
  for (let t = ceilToBucket(rangeStart); t <= rangeEnd; t += QUOTA_BUCKET_SECS) {
    if (covered.some(([start, end]) => t >= start && t < end)) continue
    rows.push(gapRow(t, topSessions))
  }

  rows.sort((left, right) => left.t - right.t)
  rows.forEach((row, index) => {
    row.index = index
  })
  return { rows, topSessions }
}

export interface QuotaTopSessionRow {
  key: string
  agent: string
  sessionId: string
  wslDistro: string | null
  title: string | null
  usd: number
  percent: number | null
  periodCount: number
}

/** The top ten sessions by dollars, merged by session key across every period in range. */
export function quotaTopSessionRows(
  periods: readonly QuotaPeriodPayload[],
): QuotaTopSessionRow[] {
  const totals = new Map<string, QuotaTopSessionRow>()
  for (const period of periods) {
    for (const session of period.sessions) {
      const key = quotaSessionKey(session.agent, session.sessionId, session.wslDistro)
      const existing = totals.get(key)
      if (existing) {
        existing.usd += session.usd
        existing.percent =
          existing.percent == null || session.percent == null
            ? null
            : existing.percent + session.percent
        existing.periodCount += 1
        if (session.title) existing.title = session.title
      } else {
        totals.set(key, {
          key,
          agent: session.agent,
          sessionId: session.sessionId,
          wslDistro: session.wslDistro,
          title: session.title,
          usd: session.usd,
          percent: session.percent,
          periodCount: 1,
        })
      }
    }
  }
  return [...totals.values()].sort((left, right) => right.usd - left.usd).slice(0, 10)
}

export interface QuotaUnattributedTotal {
  usd: number
  percent: number | null
  sessionCount: number
}

/** The unattributed spend merged across every period in range. */
export function quotaUnattributedTotal(
  periods: readonly QuotaPeriodPayload[],
): QuotaUnattributedTotal {
  return periods.reduce<QuotaUnattributedTotal>(
    (total, period) => addUnattributed(total, period.unattributed),
    { usd: 0, percent: 0, sessionCount: 0 },
  )
}

function addUnattributed(
  total: QuotaUnattributedTotal,
  next: QuotaUnattributedPayload,
): QuotaUnattributedTotal {
  return {
    usd: total.usd + next.usd,
    percent:
      total.percent == null || next.percent == null ? null : total.percent + next.percent,
    sessionCount: total.sessionCount + next.sessionCount,
  }
}

/** The most recently opened period in range, or null when the range holds none. */
export function quotaLatestPeriod(
  periods: readonly QuotaPeriodPayload[],
): QuotaPeriodPayload | null {
  return periods.reduce<QuotaPeriodPayload | null>(
    (latest, period) =>
      !latest || period.startsAtEpoch > latest.startsAtEpoch ? period : latest,
    null,
  )
}

/** The most recent sample's timestamp across every period in range, or null. */
export function quotaLatestSampleEpoch(periods: readonly QuotaPeriodPayload[]): number | null {
  let latest: number | null = null
  for (const period of periods) {
    for (const sample of period.samples) {
      if (latest == null || sample.observedAtEpoch > latest) latest = sample.observedAtEpoch
    }
  }
  return latest
}
