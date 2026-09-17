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

/** A preset that shows one or more of a lane's own reset-to-reset windows,
 *  rather than a fixed date span. Module-private: callers only ever see it
 *  folded into `QuotaRangePreset`. */
type QuotaWindowPreset =
  "thisWindow" | "lastWindow" | "last3Windows" | "last5Windows" | "last10Windows"

export type QuotaRangePreset = "thisWeek" | "lastWeek" | "last30Days" | QuotaWindowPreset

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

/** True when `range` shows a lane's own windows rather than a fixed date span. */
export function isWindowPreset(range: QuotaRangeSelection): boolean {
  return !isCustomRange(range) && WINDOW_PRESETS.has(range)
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
 * `thisWeek` and `lastWeek` anchor to the weekly lane's currently open
 * window when one exists: the selected lane's own window when it is weekly,
 * or `weeklyLane`'s window when the selection is the five-hour lane. With no
 * open weekly window, both fall back to a plain trailing week.
 *
 * A window preset (`thisWindow`, `lastWindow`, `lastNWindows`) fetches
 * enough history to contain the windows it names; `selectQuotaPeriods` then
 * picks those windows out of the fetched payload. A weekly lane fetches the
 * windows' own exact span when its current window is known. The five-hour
 * lane's windows are irregular, so it always fetches by calendar days and
 * lets selection pick the windows out of what came back.
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
  if (preset === "lastWeek") {
    // lastWeek: the week before thisWeek's own start.
    const thisWeek = rangeForPreset("thisWeek", lane, now, weeklyLane)
    return capRange({
      startEpoch: thisWeek.startEpoch - WEEK_SECS,
      endEpoch: thisWeek.startEpoch,
    })
  }
  if (lane && isWeeklyLane(lane.lane)) {
    return weeklyLaneWindowRange(preset, lane.currentPeriod, now)
  }
  return irregularLaneWindowRange(preset, lane, now)
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
  weeklyLane?: QuotaLanePayload | null,
): QuotaRange {
  if (isCustomRange(range)) {
    return capRange({ startEpoch: range.startEpoch, endEpoch: range.endEpoch })
  }
  return rangeForPreset(range, lane, now, weeklyLane)
}

/**
 * The windows a selection shows, in start order.
 *
 * A date preset or a custom range keeps every fetched period that overlaps
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
 * for date presets, the selected windows' own span for window presets.
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
  /** Index into `QUOTA_SESSION_SWATCHES`, assigned by `assignQuotaHues` so
   *  bands that touch in the stack never share a color. */
  hue: number
}

/** A session earns its own series when some period's percent exceeds this
 *  share of the limit: the smallest band a reader can see. */
export const QUOTA_OWN_SERIES_MIN_PERCENT = 1

/** No more than this many sessions ever carry their own series. Matches the
 *  `quota-area-s0`..`quota-area-s39` highlight rules enumerated in
 *  quota.css; change both together. */
export const QUOTA_OWN_SERIES_CAP = 40

/** The eight session hues, indexed by the assignment `assignQuotaHues` picks.
 *  Kept module-private: the burnup chart now draws its own fills from
 *  `quotaBandSpecs` in quotaPaths.ts, and `quotaSwatchClasses` below is the
 *  only other caller that needs these Tailwind classes. */
const QUOTA_SESSION_SWATCHES = [
  "bg-quota-session-1",
  "bg-quota-session-2",
  "bg-quota-session-3",
  "bg-quota-session-4",
  "bg-quota-session-5",
  "bg-quota-session-6",
  "bg-quota-session-7",
  "bg-quota-session-8",
] as const

/** How many distinct session hues the palette carries. `quotaPaths.ts` uses
 *  this instead of a literal, so the band fill and the swatch list can never
 *  fall out of step. */
export const QUOTA_HUE_COUNT = QUOTA_SESSION_SWATCHES.length

/**
 * The swatch class for every layer the burnup chart and the top-sessions
 * list can name: the meter, each top session by its assigned hue, and the
 * two shared grey bands. One source, so the chart and the list always agree
 * on a series' color.
 */
export function quotaSwatchClasses(
  topSessions: readonly QuotaTopSession[],
): Record<string, string> {
  const classes: Record<string, string> = {
    meter: "bg-quota-meter",
    other: "bg-quota-other",
    unattributed: "bg-quota-unattributed",
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
        })
      }
    }
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
  if (!anyPercent) return sessions.slice(0, 5)
  return sessions.filter((session) => session.qualifies).slice(0, QUOTA_OWN_SERIES_CAP)
}

/** The sessions selected for their own series, in chart order (by dollars). */
function topSessionsAcross(periods: readonly QuotaPeriodPayload[]): QuotaTopSession[] {
  return selectOwnSeriesSessions(periods).map((session) => ({
    key: session.key,
    agent: session.agent,
    sessionId: session.sessionId,
    wslDistro: session.wslDistro,
    title: session.title,
    usd: session.usd,
    hue: 0,
  }))
}

/**
 * The conflict graph for the chart's hues: two top sessions conflict when a
 * row's active bands (value greater than zero) place them next to each
 * other in the stack, since those are the two colors a reader sees touching.
 * Only stack-adjacent pairs conflict — a session two layers away in the
 * stack never clashes with this one, whatever hue it holds. Each edge is
 * weighted by the number of rows the pair touches, so a forced clash can
 * prefer the neighbor it touches for the fewest rows.
 */
function stackConflicts(
  topSessions: readonly QuotaTopSession[],
  rows: readonly QuotaSeriesRow[],
): Map<string, Map<string, number>> {
  const conflicts = new Map<string, Map<string, number>>()
  const addEdge = (left: string, right: string) => {
    const leftEdges = conflicts.get(left) ?? new Map<string, number>()
    leftEdges.set(right, (leftEdges.get(right) ?? 0) + 1)
    conflicts.set(left, leftEdges)
    const rightEdges = conflicts.get(right) ?? new Map<string, number>()
    rightEdges.set(left, (rightEdges.get(left) ?? 0) + 1)
    conflicts.set(right, rightEdges)
  }
  for (const row of rows) {
    const active = topSessions.filter((session) => (row[session.key] ?? 0) > 0)
    for (let i = 0; i + 1 < active.length; i++) addEdge(active[i]!.key, active[i + 1]!.key)
  }
  return conflicts
}

/**
 * Greedy graph coloring for the chart's hues, over the conflict graph built
 * from the series rows: two bands that never touch in the stack may share a
 * hue, however long each is visible for on its own.
 *
 * Sessions are visited in stack order (`topSessions`, roughly biggest to
 * smallest), so a session's already-colored neighbors are the ones above it
 * in the stack. Each session takes the lowest hue none of those neighbors
 * hold. When every hue is taken, the session touches more distinct
 * neighbors than there are hues and a clash is unavoidable; it then takes
 * the hue whose neighbors' edge weight (rows touched) is smallest, so the
 * visible clash spans the fewest rows, ties going to the lowest hue.
 */
export function assignQuotaHues(
  topSessions: readonly QuotaTopSession[],
  rows: readonly QuotaSeriesRow[],
  hueCount: number = QUOTA_SESSION_SWATCHES.length,
): Map<string, number> {
  const conflicts = stackConflicts(topSessions, rows)
  const hues = new Map<string, number>()
  for (const session of topSessions) {
    const neighborEdges = conflicts.get(session.key) ?? new Map<string, number>()
    const weightByHue = new Map<number, number>()
    for (const [neighborKey, weight] of neighborEdges) {
      const neighborHue = hues.get(neighborKey)
      if (neighborHue == null) continue // not yet colored: no constraint from it
      weightByHue.set(neighborHue, (weightByHue.get(neighborHue) ?? 0) + weight)
    }
    let chosen = -1
    for (let hue = 0; hue < hueCount; hue++) {
      if (!weightByHue.has(hue)) {
        chosen = hue
        break
      }
    }
    if (chosen === -1) {
      let bestWeight = Infinity
      for (let hue = 0; hue < hueCount; hue++) {
        const weight = weightByHue.get(hue) ?? 0
        if (weight < bestWeight) {
          bestWeight = weight
          chosen = hue
        }
      }
    }
    hues.set(session.key, chosen)
  }
  return hues
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
 * period's own start, never the range's. No row lands after `nowEpoch`: the
 * chart has no estimate for a time that has not happened yet.
 */
function periodRows(
  period: QuotaPeriodPayload,
  rangeStart: number,
  rangeEnd: number,
  nowEpoch: number,
  hasFactor: boolean,
  topKeys: ReadonlySet<string>,
  topSessions: readonly QuotaTopSession[],
): QuotaSeriesRow[] {
  const start = period.startsAtEpoch
  const reset = period.resetsAtEpoch
  const visibleStart = Math.max(start, rangeStart)
  const visibleEnd = Math.min(reset, rangeEnd)
  if (visibleEnd <= visibleStart) return []
  if (visibleStart > nowEpoch) return []
  const clampedVisibleEnd = Math.min(visibleEnd, nowEpoch)

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
  let bucketPointer = 0
  let unattributedPointer = 0
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
    while (
      unattributedPointer < unattributedBuckets.length &&
      unattributedBuckets[unattributedPointer]!.bucketStartEpoch <= t
    ) {
      unattributedCumulative += unattributedBuckets[unattributedPointer]!.percent ?? 0
      unattributedPointer += 1
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
    const row: QuotaSeriesRow = {
      t,
      index: 0,
      meter: interpolateMeter(prevSample, nextSample, t),
      other: hasFactor ? otherCumulative : null,
      unattributed: hasFactor ? unattributedCumulative : null,
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
    rows.push(
      ...periodRows(period, rangeStart, rangeEnd, nowEpoch, hasFactor, topKeys, topSessions),
    )
  }

  // A reset row at each boundary still inside the range and not after now.
  // A period that starts exactly where another resets already carries its
  // own zero row, so this loop must not draw a second one on top of it.
  for (const period of periods) {
    const t = period.resetsAtEpoch
    if (t < rangeStart || t > rangeEnd || t > nowEpoch) continue
    const opensNextPeriod = periods.some((candidate) => candidate.startsAtEpoch === t)
    if (opensNextPeriod) continue
    rows.push(zeroRow(t, hasFactor, topSessions))
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

  // Hue only affects rendering, so it is assigned last, once every row's
  // stack-adjacency is known.
  const hues = assignQuotaHues(topSessions, rows)
  for (const session of topSessions) session.hue = hues.get(session.key) ?? 0

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

/** Every session with its own chart series, in chart order (by dollars). */
export function quotaTopSessionRows(
  periods: readonly QuotaPeriodPayload[],
): QuotaTopSessionRow[] {
  return selectOwnSeriesSessions(periods).map((session) => ({
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
