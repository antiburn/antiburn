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
  QuotaSamplePayload,
  QuotaUnattributedPayload,
  QuotaUsagePayload,
} from "../../../lib/providerUsageIpc"

/** One 15-minute bucket, in seconds. Matches the backend's contribution grain. */
const QUOTA_BUCKET_SECS = 15 * 60
const DAY_SECS = 24 * 60 * 60
const WEEK_SECS = 7 * DAY_SECS
/** The request never spans more than this many days, whatever the preset
 *  computes: enough for ten weekly windows. */
const MAX_RANGE_DAYS = 70
/**
 * The longest gap between two authoritative meter samples that still draws a
 * line between them. A wider gap leaves the meter line null instead of
 * guessing across a long silence.
 */
export const QUOTA_METER_INTERPOLATION_GAP_SECS = 3 * 60 * 60

/** A preset that shows one or more of a lane's own reset-to-reset windows:
 *  the open one, the one before it, or the latest N. */
export type QuotaRangePreset =
  "thisWindow" | "lastWindow" | "last3Windows" | "last5Windows" | "last10Windows"

type QuotaWindowPreset = QuotaRangePreset

/** How many windows each `lastNWindows` preset names. */
const WINDOW_PRESET_COUNT: Record<"last3Windows" | "last5Windows" | "last10Windows", number> = {
  last3Windows: 3,
  last5Windows: 5,
  last10Windows: 10,
}

/** An explicit range a caller picked, outside the fixed presets. */
export interface QuotaCustomRange {
  kind: "custom"
  startEpoch: number
  endEpoch: number
}

/** The range a Quota session shows: one of the fixed presets, or a custom range. */
export type QuotaRangeSelection = QuotaRangePreset | QuotaCustomRange

export interface QuotaRange {
  startEpoch: number
  endEpoch: number
}

/** True when `range` is a custom range rather than a named preset. */
export function isCustomRange(range: QuotaRangeSelection): range is QuotaCustomRange {
  return typeof range === "object"
}

const WINDOW_PRESETS: ReadonlySet<QuotaRangePreset> = new Set<QuotaRangePreset>([
  "thisWindow",
  "lastWindow",
  "last3Windows",
  "last5Windows",
  "last10Windows",
])

/** True when `range` names a lane's own windows rather than a custom span. */
export function isWindowPreset(range: QuotaRangeSelection): boolean {
  return !isCustomRange(range)
}

/** True when `value` is one of the `QuotaRangePreset` literals, for a value
 *  read out of untyped storage (persisted view prefs). A preset an earlier
 *  build saved and this one no longer knows fails here and falls back. */
export function isQuotaRangePreset(value: unknown): value is QuotaRangePreset {
  return typeof value === "string" && WINDOW_PRESETS.has(value as QuotaRangePreset)
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

/** A day, for the five-hour lane's calendar-day fetch: its windows are
 *  irregular (one starts at the first turn after idle), so a window preset
 *  fetches by calendar days and lets `selectQuotaPeriods` pick the windows
 *  the fetch turned up. */
const FIVE_HOUR_FETCH_DAYS = 2

/**
 * The wall-clock range a window preset covers on the five-hour lane (or any
 * other non-weekly lane): enough calendar days to contain the windows it
 * names, since the lane's own windows do not line up with a fixed span.
 */
function irregularLaneWindowRange(
  preset: QuotaWindowPreset,
  lane: QuotaLanePayload | null,
  now: number,
): QuotaRange {
  if (preset === "thisWindow") {
    const current = lane?.currentPeriod
    if (current) {
      return capRange({ startEpoch: current.startsAtEpoch, endEpoch: current.resetsAtEpoch })
    }
    return capRange({ startEpoch: now - FIVE_HOUR_FETCH_DAYS * DAY_SECS, endEpoch: now })
  }
  if (preset === "lastWindow") {
    return capRange({ startEpoch: now - FIVE_HOUR_FETCH_DAYS * DAY_SECS, endEpoch: now })
  }
  const days = Math.max(FIVE_HOUR_FETCH_DAYS, WINDOW_PRESET_COUNT[preset])
  return capRange({ startEpoch: now - days * DAY_SECS, endEpoch: now })
}

/**
 * The wall-clock range a window preset covers on a weekly lane: the exact
 * span of the windows it names when the lane's currently open window is
 * known, else a trailing multiple of a week ending now.
 */
function weeklyLaneWindowRange(
  preset: QuotaWindowPreset,
  currentWeekly: QuotaLanePayload["currentPeriod"],
  now: number,
): QuotaRange {
  if (!currentWeekly) {
    const weeks =
      preset === "thisWindow" || preset === "lastWindow" ? 1 : WINDOW_PRESET_COUNT[preset]
    return capRange({ startEpoch: now - weeks * WEEK_SECS, endEpoch: now })
  }
  if (preset === "thisWindow") {
    return capRange({
      startEpoch: currentWeekly.startsAtEpoch,
      endEpoch: currentWeekly.resetsAtEpoch,
    })
  }
  if (preset === "lastWindow") {
    return capRange({
      startEpoch: currentWeekly.startsAtEpoch - WEEK_SECS,
      endEpoch: currentWeekly.startsAtEpoch,
    })
  }
  return capRange({
    startEpoch: currentWeekly.resetsAtEpoch - WINDOW_PRESET_COUNT[preset] * WEEK_SECS,
    endEpoch: currentWeekly.resetsAtEpoch,
  })
}

/**
 * The wall-clock range a preset covers for the selected lane.
 *
 * A preset (`thisWindow`, `lastWindow`, `lastNWindows`) fetches enough
 * history to contain the windows it names; `selectQuotaPeriods` then picks
 * those windows out of the fetched payload. A weekly lane fetches the
 * windows' own exact span when its current window is known. The five-hour
 * lane's windows are irregular, so it always fetches by calendar days and
 * lets selection pick the windows out of what came back.
 */
export function rangeForPreset(
  preset: QuotaRangePreset,
  lane: QuotaLanePayload | null,
  now: number,
): QuotaRange {
  if (lane && isWeeklyLane(lane.lane)) {
    return weeklyLaneWindowRange(preset, lane.currentPeriod, now)
  }
  return irregularLaneWindowRange(preset, lane, now)
}

/** False when the preset's whole range ends before the lane's first reading. */
export function presetHasReadings(
  preset: QuotaRangePreset,
  lane: QuotaLanePayload | null,
  now: number,
): boolean {
  if (!lane) return true
  return rangeForPreset(preset, lane, now).endEpoch > lane.firstObservedEpoch
}

/**
 * The wall-clock range a selection covers: a custom range applies its own
 * bounds directly, capped the same way a preset is, so a session-detail
 * deep link cannot request an unbounded query.
 */
export function resolveQuotaRange(
  range: QuotaRangeSelection,
  lane: QuotaLanePayload | null,
  now: number,
): QuotaRange {
  if (isCustomRange(range)) {
    return capRange({ startEpoch: range.startEpoch, endEpoch: range.endEpoch })
  }
  return rangeForPreset(range, lane, now)
}

/**
 * The windows a selection shows, in start order.
 *
 * A custom range keeps every fetched period that overlaps
 * the fetched span. A window preset instead names specific windows out of
 * the fetched payload: `thisWindow` is the latest window at or before `now`
 * (the last one at all, once every window is still in the future);
 * `lastWindow` is the one immediately before it; `lastNWindows` is that
 * window and the `N - 1` before it, however many of those the fetch
 * actually turned up.
 */
export function selectQuotaPeriods(
  range: QuotaRangeSelection,
  periods: readonly QuotaPeriodPayload[],
  fetched: QuotaRange,
  now: number,
): QuotaPeriodPayload[] {
  if (!isWindowPreset(range)) {
    return periods
      .filter(
        (period) =>
          period.resetsAtEpoch > fetched.startEpoch && period.startsAtEpoch < fetched.endEpoch,
      )
      .sort((left, right) => left.startsAtEpoch - right.startsAtEpoch)
  }
  const sorted = [...periods].sort((left, right) => left.startsAtEpoch - right.startsAtEpoch)
  if (sorted.length === 0) return []
  const dueByNow = sorted.filter((period) => period.startsAtEpoch <= now)
  const latest =
    dueByNow.length > 0 ? dueByNow[dueByNow.length - 1]! : sorted[sorted.length - 1]!
  const latestIndex = sorted.indexOf(latest)
  const preset = range as QuotaWindowPreset
  if (preset === "thisWindow") return [latest]
  if (preset === "lastWindow") return latestIndex > 0 ? [sorted[latestIndex - 1]!] : []
  const count = WINDOW_PRESET_COUNT[preset]
  const startIndex = Math.max(0, latestIndex - count + 1)
  return sorted.slice(startIndex, latestIndex + 1)
}

/**
 * The x range the chart and series cover for a selection: the fetched range
 * for a custom range, the selected windows' own span for window presets.
 * `periods` here is the selection's own output — `selectQuotaPeriods`'s
 * result, not every fetched period — so a window preset's display range
 * hugs just the windows it shows.
 */
export function quotaDisplayRange(
  range: QuotaRangeSelection,
  periods: readonly QuotaPeriodPayload[],
  fetched: QuotaRange,
): QuotaRange {
  if (!isWindowPreset(range) || periods.length === 0) return fetched
  const sorted = [...periods].sort((left, right) => left.startsAtEpoch - right.startsAtEpoch)
  return {
    startEpoch: sorted[0]!.startsAtEpoch,
    endEpoch: sorted[sorted.length - 1]!.resetsAtEpoch,
  }
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
  /** Index into `QUOTA_SESSION_SWATCHES`: the session's dollar rank across
   *  the range, so the biggest spender takes the darkest step. */
  hue: number
}

/** A session earns its own series when some period's percent exceeds this
 *  share of the limit: the smallest band a reader can see. */
export const QUOTA_OWN_SERIES_MIN_PERCENT = 1

/** No more than this many sessions ever carry their own series. Matches the
 *  `quota-area-s0`..`quota-area-s4` highlight rules in quota.css and the
 *  five steps of the session ramp; change all three together. */
export const QUOTA_OWN_SERIES_CAP = 5

/** The five steps of the session ramp, darkest first. A session's `hue` is
 *  its dollar rank, so the biggest spender takes the darkest step. Kept
 *  module-private: the burnup chart draws its own fills from
 *  `quotaBandSpecs` in quotaPaths.ts, and `quotaSwatchClasses` below is the
 *  only other caller that needs these Tailwind classes. */
const QUOTA_SESSION_SWATCHES = [
  "bg-quota-session-1",
  "bg-quota-session-2",
  "bg-quota-session-3",
  "bg-quota-session-4",
  "bg-quota-session-5",
] as const

/** How many steps the session ramp carries. `quotaPaths.ts` uses this
 *  instead of a literal, so the band fill and the swatch list can never
 *  fall out of step. */
export const QUOTA_HUE_COUNT = QUOTA_SESSION_SWATCHES.length

/**
 * The swatch class for every layer the burnup chart and the top-sessions
 * list can name: the meter, each top session by its ramp step, and the two
 * grouped bands in the resting greys the session-analysis charts use. One
 * source, so the chart and the list always agree on a series' color.
 */
export function quotaSwatchClasses(
  topSessions: readonly QuotaTopSession[],
): Record<string, string> {
  const classes: Record<string, string> = {
    meter: "bg-quota-meter",
    other: "bg-chart-rest-strong",
    unattributed: "bg-chart-rest-faint",
  }
  topSessions.forEach((session) => {
    classes[session.key] = QUOTA_SESSION_SWATCHES[session.hue % QUOTA_SESSION_SWATCHES.length]!
  })
  return classes
}

interface QualifyingSession {
  key: string
  agent: string
  sessionId: string
  wslDistro: string | null
  title: string | null
  usd: number
  percent: number | null
  periodCount: number
  /** Exceeded `QUOTA_OWN_SERIES_MIN_PERCENT` in at least one period. */
  qualifies: boolean
  /** The earliest `bucketStartEpoch` this session contributes to, across
   *  every given period. `POSITIVE_INFINITY` for a session with no
   *  contribution rows, so it sorts last in stack order. */
  firstBucketEpoch: number
}

/** The earliest `bucketStartEpoch` each session's contributions reach,
 *  across every given period, keyed the same way as `qualifyingSessionsAcross`. */
function firstBucketEpochsAcross(periods: readonly QuotaPeriodPayload[]): Map<string, number> {
  const firstBucketEpochs = new Map<string, number>()
  for (const period of periods) {
    for (const contribution of period.contributions) {
      const key = quotaSessionKey(
        contribution.agent,
        contribution.sessionId,
        contribution.wslDistro,
      )
      const existing = firstBucketEpochs.get(key)
      if (existing == null || contribution.bucketStartEpoch < existing) {
        firstBucketEpochs.set(key, contribution.bucketStartEpoch)
      }
    }
  }
  return firstBucketEpochs
}

/** Every bound session merged across periods, with its own-series
 *  eligibility, and whether the lane carries any percent data at all. */
function qualifyingSessionsAcross(periods: readonly QuotaPeriodPayload[]): {
  sessions: QualifyingSession[]
  anyPercent: boolean
} {
  const totals = new Map<string, QualifyingSession>()
  let anyPercent = false
  for (const period of periods) {
    for (const session of period.sessions) {
      const key = quotaSessionKey(session.agent, session.sessionId, session.wslDistro)
      if (session.percent != null) anyPercent = true
      const qualifies =
        session.percent != null && session.percent > QUOTA_OWN_SERIES_MIN_PERCENT
      const existing = totals.get(key)
      if (existing) {
        existing.usd += session.usd
        existing.percent =
          existing.percent == null || session.percent == null
            ? null
            : existing.percent + session.percent
        existing.periodCount += 1
        if (session.title) existing.title = session.title
        if (qualifies) existing.qualifies = true
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
          qualifies,
          firstBucketEpoch: Number.POSITIVE_INFINITY,
        })
      }
    }
  }
  const firstBucketEpochs = firstBucketEpochsAcross(periods)
  for (const session of totals.values()) {
    session.firstBucketEpoch = firstBucketEpochs.get(session.key) ?? Number.POSITIVE_INFINITY
  }
  return {
    sessions: [...totals.values()].sort((left, right) => right.usd - left.usd),
    anyPercent,
  }
}

/**
 * The sessions that earn their own series: any session whose percent
 * exceeded `QUOTA_OWN_SERIES_MIN_PERCENT` in some period, ranked by dollars
 * across the range and capped at `QUOTA_OWN_SERIES_CAP`. Falls back to the
 * top five sessions by dollars when the lane carries no percent data at
 * all, so the chart is not left without a single named band.
 */
function selectOwnSeriesSessions(periods: readonly QuotaPeriodPayload[]): QualifyingSession[] {
  const { sessions, anyPercent } = qualifyingSessionsAcross(periods)
  if (!anyPercent) return sessions.slice(0, QUOTA_OWN_SERIES_CAP)
  return sessions.filter((session) => session.qualifies).slice(0, QUOTA_OWN_SERIES_CAP)
}

/**
 * The sessions selected for their own series, in stack order: first
 * appearance at the bottom. The session whose contributions start earliest
 * across the displayed range sorts first (bottom of the stack), and the
 * session that starts last sorts last (top, just under "other" /
 * "unattributed" / "unexplained"). Ties break by dollars descending, then
 * by key, so the order stays deterministic.
 */
function topSessionsAcross(periods: readonly QuotaPeriodPayload[]): QuotaTopSession[] {
  return selectOwnSeriesSessions(periods)
    .slice()
    .sort(
      (left, right) =>
        left.firstBucketEpoch - right.firstBucketEpoch ||
        right.usd - left.usd ||
        (left.key < right.key ? -1 : left.key > right.key ? 1 : 0),
    )
    .map((session) => ({
      key: session.key,
      agent: session.agent,
      sessionId: session.sessionId,
      wslDistro: session.wslDistro,
      title: session.title,
      usd: session.usd,
      hue: 0,
    }))
}

/** One row of the burnup series. Session columns are added by key, dynamically. */
export interface QuotaSeriesRow {
  t: number
  index: number
  meter: number | null
  other: number | null
  unattributed: number | null
  /** Cumulative spend the meter's own rise credits to no local session:
   *  a meter-rise segment with no dollars behind it at all. */
  unexplained: number | null
  [sessionKey: string]: number | null
}

export interface QuotaSeries {
  rows: QuotaSeriesRow[]
  topSessions: QuotaTopSession[]
}

/** The next absolute 15-minute mark at or after `t`, on the same grain the backend buckets use. */
function ceilToBucket(t: number): number {
  return Math.ceil(t / QUOTA_BUCKET_SECS) * QUOTA_BUCKET_SECS
}

function zeroRow(t: number, topSessions: readonly QuotaTopSession[]): QuotaSeriesRow {
  const row: QuotaSeriesRow = {
    t,
    index: 0,
    meter: null,
    other: 0,
    unattributed: 0,
    unexplained: 0,
  }
  for (const session of topSessions) row[session.key] = 0
  return row
}

function gapRow(t: number, topSessions: readonly QuotaTopSession[]): QuotaSeriesRow {
  const row: QuotaSeriesRow = {
    t,
    index: 0,
    meter: null,
    other: null,
    unattributed: null,
    unexplained: null,
  }
  for (const session of topSessions) row[session.key] = null
  return row
}

interface MeterSample {
  observedAtEpoch: number
  usedPercent: number
}

/**
 * The meter's reading at `t`, from the two authoritative samples bracketing
 * it. Interpolates linearly when they are at most
 * `QUOTA_METER_INTERPOLATION_GAP_SECS` apart. Returns null with no bracketing
 * sample on one side, or across a wider gap. The caller advances `prev` and
 * `next` alongside its own ascending walk, so this stays O(1) per row.
 */
function interpolateMeter(
  prev: MeterSample | null,
  next: MeterSample | null,
  t: number,
): number | null {
  if (prev && prev.observedAtEpoch === t) return prev.usedPercent
  if (!prev || !next) return null
  const gap = next.observedAtEpoch - prev.observedAtEpoch
  if (gap > QUOTA_METER_INTERPOLATION_GAP_SECS) return null
  const fraction = (t - prev.observedAtEpoch) / gap
  return prev.usedPercent + (next.usedPercent - prev.usedPercent) * fraction
}

/**
 * One period's rows: a zero row at its visible start, one row per bucket
 * that carries a contribution or unattributed spend, a final-total row just
 * before the reset, and an authoritative meter reading wherever one was
 * observed. Contributions and unattributed spend both accumulate from the
 * period's own start, never the range's, and a bucket still in progress at
 * a row's own time contributes only the fraction of its own 15 minutes
 * already elapsed, so its value ramps in rather than jumping in whole at
 * the bucket's start. No row lands after `nowEpoch`: the chart has no
 * estimate for a time that has not happened yet.
 */
function periodRows(
  period: QuotaPeriodPayload,
  rangeStart: number,
  rangeEnd: number,
  nowEpoch: number,
  topKeys: ReadonlySet<string>,
  topSessions: readonly QuotaTopSession[],
): QuotaSeriesRow[] {
  const start = period.startsAtEpoch
  const reset = period.resetsAtEpoch
  const visibleStart = Math.max(start, rangeStart)
  const visibleEnd = Math.min(reset, rangeEnd)
  if (visibleEnd <= visibleStart) return []
  if (visibleStart > nowEpoch) return []
  // Capped one second short of the reset, the same half-open bound
  // `finalRowTime` below uses: a bucket-aligned reset would otherwise let
  // the grid-fill loop add a point at `t === reset`, and the next period
  // starts at that very instant, so its own slot (`rowsInWindow` keeps
  // `t >= start`) would pick up this period's full closing row one row
  // before its own zero row at the same time, drawing a zero-width
  // full-height spike at the window start.
  const clampedVisibleEnd = Math.min(visibleEnd, nowEpoch, reset - 1)

  const byBucket = new Map<number, QuotaContributionPayload[]>()
  for (const contribution of period.contributions) {
    if (contribution.bucketStartEpoch < start || contribution.bucketStartEpoch >= reset)
      continue
    const list = byBucket.get(contribution.bucketStartEpoch) ?? []
    list.push(contribution)
    byBucket.set(contribution.bucketStartEpoch, list)
  }
  const bucketTimes = [...byBucket.keys()].sort((left, right) => left - right)

  const unattributedBuckets = period.unattributedBuckets
    .filter((bucket) => bucket.bucketStartEpoch >= start && bucket.bucketStartEpoch < reset)
    .sort((left, right) => left.bucketStartEpoch - right.bucketStartEpoch)
  const unexplainedBuckets = period.unexplainedBuckets
    .filter((bucket) => bucket.bucketStartEpoch >= start && bucket.bucketStartEpoch < reset)
    .sort((left, right) => left.bucketStartEpoch - right.bucketStartEpoch)

  const points = new Set<number>()
  if (start >= rangeStart) points.add(start)
  for (let t = ceilToBucket(visibleStart); t <= clampedVisibleEnd; t += QUOTA_BUCKET_SECS)
    points.add(t)
  for (const bucketTime of bucketTimes) {
    if (bucketTime >= visibleStart && bucketTime <= clampedVisibleEnd) points.add(bucketTime)
  }
  for (const bucket of unattributedBuckets) {
    if (
      bucket.bucketStartEpoch >= visibleStart &&
      bucket.bucketStartEpoch <= clampedVisibleEnd
    ) {
      points.add(bucket.bucketStartEpoch)
    }
  }
  for (const bucket of unexplainedBuckets) {
    if (
      bucket.bucketStartEpoch >= visibleStart &&
      bucket.bucketStartEpoch <= clampedVisibleEnd
    ) {
      points.add(bucket.bucketStartEpoch)
    }
  }
  const finalRowTime = Math.min(reset - 1, nowEpoch)
  if (finalRowTime >= visibleStart && finalRowTime <= rangeEnd && finalRowTime >= start) {
    points.add(finalRowTime)
  }
  for (const sample of period.samples) {
    if (!sample.authoritative) continue
    if (sample.observedAtEpoch < start || sample.observedAtEpoch >= reset) continue
    if (sample.observedAtEpoch < rangeStart || sample.observedAtEpoch > rangeEnd) continue
    if (sample.observedAtEpoch > nowEpoch) continue
    points.add(sample.observedAtEpoch)
  }

  const sortedPoints = [...points].sort((left, right) => left - right)
  const topCumulative = new Map(topSessions.map((session) => [session.key, 0]))
  let otherCumulative = 0
  let unattributedCumulative = 0
  let unexplainedCumulative = 0
  let bucketPointer = 0
  let unattributedPointer = 0
  let unexplainedPointer = 0
  let samplePointer = -1
  const authoritativeSamples: MeterSample[] = period.samples
    .filter(
      (sample): sample is QuotaSamplePayload & { usedPercent: number } =>
        sample.authoritative &&
        sample.usedPercent != null &&
        sample.observedAtEpoch >= start &&
        sample.observedAtEpoch < reset,
    )
    .sort((left, right) => left.observedAtEpoch - right.observedAtEpoch)

  const rows: QuotaSeriesRow[] = []
  for (const t of sortedPoints) {
    // A completed bucket (its own 15 minutes fully behind `t`) adds fully
    // and advances past. Buckets are 15-minute aligned, so at most one
    // bucket can still be in progress at `t`; that one ramps in below by
    // the fraction of its own window already elapsed, without advancing
    // the pointer, so the next row can still see it complete.
    while (
      bucketPointer < bucketTimes.length &&
      bucketTimes[bucketPointer]! + QUOTA_BUCKET_SECS <= t
    ) {
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
    let inProgressOther = 0
    const inProgressTop = new Map<string, number>()
    if (bucketPointer < bucketTimes.length && bucketTimes[bucketPointer]! <= t) {
      const bucketTime = bucketTimes[bucketPointer]!
      const fraction = Math.min(1, Math.max(0, (t - bucketTime) / QUOTA_BUCKET_SECS))
      for (const contribution of byBucket.get(bucketTime) ?? []) {
        const key = quotaSessionKey(
          contribution.agent,
          contribution.sessionId,
          contribution.wslDistro,
        )
        const percent = (contribution.percent ?? 0) * fraction
        if (topKeys.has(key)) {
          inProgressTop.set(key, (inProgressTop.get(key) ?? 0) + percent)
        } else {
          inProgressOther += percent
        }
      }
    }
    while (
      unattributedPointer < unattributedBuckets.length &&
      unattributedBuckets[unattributedPointer]!.bucketStartEpoch + QUOTA_BUCKET_SECS <= t
    ) {
      unattributedCumulative += unattributedBuckets[unattributedPointer]!.percent ?? 0
      unattributedPointer += 1
    }
    let inProgressUnattributed = 0
    if (
      unattributedPointer < unattributedBuckets.length &&
      unattributedBuckets[unattributedPointer]!.bucketStartEpoch <= t
    ) {
      const bucket = unattributedBuckets[unattributedPointer]!
      const fraction = Math.min(
        1,
        Math.max(0, (t - bucket.bucketStartEpoch) / QUOTA_BUCKET_SECS),
      )
      inProgressUnattributed = (bucket.percent ?? 0) * fraction
    }
    // Unexplained segments keep the step rule: the backend stamps each one
    // at the segment's own end (the reading that closes it), since its
    // rise is not known any earlier, so the row before that time already
    // ramps toward it via the meter line, and this only needs to add the
    // full amount once that time arrives.
    while (
      unexplainedPointer < unexplainedBuckets.length &&
      unexplainedBuckets[unexplainedPointer]!.bucketStartEpoch <= t
    ) {
      unexplainedCumulative += unexplainedBuckets[unexplainedPointer]!.percent ?? 0
      unexplainedPointer += 1
    }
    while (
      samplePointer + 1 < authoritativeSamples.length &&
      authoritativeSamples[samplePointer + 1]!.observedAtEpoch <= t
    ) {
      samplePointer += 1
    }
    const prevSample = samplePointer >= 0 ? authoritativeSamples[samplePointer]! : null
    const nextSample =
      samplePointer + 1 < authoritativeSamples.length
        ? authoritativeSamples[samplePointer + 1]!
        : null
    const meter = interpolateMeter(prevSample, nextSample, t)
    const row: QuotaSeriesRow = {
      t,
      index: 0,
      meter,
      other: otherCumulative + inProgressOther,
      unattributed: unattributedCumulative + inProgressUnattributed,
      unexplained: unexplainedCumulative,
    }
    for (const session of topSessions) {
      row[session.key] =
        (topCumulative.get(session.key) ?? 0) + (inProgressTop.get(session.key) ?? 0)
    }
    rows.push(row)
  }
  return rows
}

/**
 * The burnup series for one lane's usage over a range: every period's rows,
 * a reset-to-zero row at each boundary, and a gap row wherever the range
 * holds no period, so the plot breaks instead of guessing. No row lands
 * after `nowEpoch`, since the estimate and meter have nothing to report for
 * a time that has not happened yet.
 */
export function quotaBurnupSeries(
  usage: QuotaUsagePayload,
  rangeStart: number,
  rangeEnd: number,
  nowEpoch: number,
): QuotaSeries {
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
    rows.push(...periodRows(period, rangeStart, rangeEnd, nowEpoch, topKeys, topSessions))
  }

  // A reset row at each boundary still inside the range and not after now.
  // A period that starts exactly where another resets already carries its
  // own zero row, so this loop must not draw a second one on top of it.
  for (const period of periods) {
    const t = period.resetsAtEpoch
    if (t < rangeStart || t > rangeEnd || t > nowEpoch) continue
    const opensNextPeriod = periods.some((candidate) => candidate.startsAtEpoch === t)
    if (opensNextPeriod) continue
    rows.push(zeroRow(t, topSessions))
  }

  // Fill every 15-minute mark the range holds that no period covers, so the
  // line breaks there instead of drawing a flat guess across the gap. Stops
  // at now, same as every other row source, so no null mark lands later.
  for (
    let t = ceilToBucket(rangeStart);
    t <= Math.min(rangeEnd, nowEpoch);
    t += QUOTA_BUCKET_SECS
  ) {
    if (covered.some(([start, end]) => t >= start && t < end)) continue
    rows.push(gapRow(t, topSessions))
  }

  rows.sort((left, right) => left.t - right.t)
  rows.forEach((row, index) => {
    row.index = index
  })

  // The ramp step is the session's dollar rank: darkest for the biggest
  // spender, so the chart and the dollar-sorted list read the same way.
  const byDollars = [...topSessions].sort((left, right) => right.usd - left.usd)
  byDollars.forEach((session, rank) => {
    session.hue = rank
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

/**
 * Every session with its own chart series, in dollars-descending order.
 * The chart itself stacks these bands by first appearance instead (see
 * `topSessionsAcross`), so this list sorts independently rather than
 * relying on `selectOwnSeriesSessions`'s incidental order.
 */
export function quotaTopSessionRows(
  periods: readonly QuotaPeriodPayload[],
): QuotaTopSessionRow[] {
  return selectOwnSeriesSessions(periods)
    .slice()
    .sort((left, right) => right.usd - left.usd)
    .map((session) => ({
      key: session.key,
      agent: session.agent,
      sessionId: session.sessionId,
      wslDistro: session.wslDistro,
      title: session.title,
      usd: session.usd,
      percent: session.percent,
      periodCount: session.periodCount,
    }))
}

export interface QuotaOtherSessionsTotal {
  usd: number
  percent: number | null
  count: number
}

/** Every bound session outside its own series, folded into one total:
 *  summed dollars, summed percent (null once any folded session's own
 *  percent is unknown), and how many sessions folded in. */
export function quotaOtherSessionsTotal(
  periods: readonly QuotaPeriodPayload[],
): QuotaOtherSessionsTotal {
  const { sessions } = qualifyingSessionsAcross(periods)
  const ownKeys = new Set(selectOwnSeriesSessions(periods).map((session) => session.key))
  return sessions
    .filter((session) => !ownKeys.has(session.key))
    .reduce<QuotaOtherSessionsTotal>(
      (total, session) => ({
        usd: total.usd + session.usd,
        percent:
          total.percent == null || session.percent == null
            ? null
            : total.percent + session.percent,
        count: total.count + 1,
      }),
      { usd: 0, percent: 0, count: 0 },
    )
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

export interface QuotaUnexplainedTotal {
  percent: number | null
}

/** The "not explained by local sessions" spend merged across every period
 *  in range: null once any period in range carries no meter reading at all,
 *  the same contagion rule `quotaUnattributedTotal` uses for its percent. */
export function quotaUnexplainedTotal(
  periods: readonly QuotaPeriodPayload[],
): QuotaUnexplainedTotal {
  return periods.reduce<QuotaUnexplainedTotal>(
    (total, period) => ({
      percent:
        total.percent == null || period.unexplainedPercent == null
          ? null
          : total.percent + period.unexplainedPercent,
    }),
    { percent: 0 },
  )
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
/** The earliest meter reading across `periods`, or null with no reading.
 *  Names when this app first saw the lane, for an empty range's caption. */
export function quotaEarliestSampleEpoch(
  periods: readonly QuotaPeriodPayload[],
): number | null {
  let earliest: number | null = null
  for (const period of periods) {
    for (const sample of period.samples) {
      if (earliest == null || sample.observedAtEpoch < earliest)
        earliest = sample.observedAtEpoch
    }
  }
  return earliest
}

export function quotaLatestSampleEpoch(periods: readonly QuotaPeriodPayload[]): number | null {
  let latest: number | null = null
  for (const period of periods) {
    for (const sample of period.samples) {
      if (latest == null || sample.observedAtEpoch > latest) latest = sample.observedAtEpoch
    }
  }
  return latest
}
