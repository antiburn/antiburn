import type { CSSProperties, ReactNode } from "react"

import { formatCost } from "../../../lib/presentation/sessionAnalysis"
import { formatTokenBurnPercent } from "../../../lib/presentation/checks"
import type { AllowanceWindowLevelsPayload } from "../../../lib/providerUsageIpc"
import { topSessions, type UsageSession } from "./usageSessions"
import type { CheckFacts, ConfigShare } from "./wasteMarks"
import type { RadialFocus } from "./radialFocus"
import {
  DAYS_PER_WEEK,
  LIMIT_PERCENT,
  PIN_STACK_DEGREES,
  levelAt,
  turnDistance,
  type LimitStretch,
  type PlacedPin,
  type Spoke,
} from "./radialGeometry"

// The hover card lists this many nearby pins, checks or sessions.
const NEARBY_LIMIT = 3
const HOUR = 3600
// A limit hit names the sessions last active from this long before it to
// this long after it.
const BEFORE_HIT = 5 * HOUR
const AFTER_HIT = HOUR

/** The chart data that the hover card reads. */
export type RadialData = {
  clock: AllowanceWindowLevelsPayload
  current: AllowanceWindowLevelsPayload | undefined
  past: readonly AllowanceWindowLevelsPayload[]
  weeks: readonly AllowanceWindowLevelsPayload[]
  spokes: readonly Spoke[]
  rolling: number | null
  limits: readonly LimitStretch[]
  placed: readonly PlacedPin[]
  config: readonly ConfigShare[]
  checks?: readonly CheckFacts[]
  /** The sessions with an estimated limit share, at their last activity. */
  sessions?: readonly UsageSession[]
}

function formatWhen(epoch: number): string {
  return new Date(epoch * 1000).toLocaleString(undefined, {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit",
  })
}

function formatTime(epoch: number): string {
  return new Date(epoch * 1000).toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  })
}

function formatDate(epoch: number): string {
  return new Date(epoch * 1000).toLocaleDateString(undefined, {
    weekday: "short",
    day: "numeric",
    month: "short",
  })
}

function weeksAgo(data: RadialData, weekStart: number): string {
  const span = data.clock.resetsAtEpoch - data.clock.startsAtEpoch
  const count = span > 0 ? Math.round((data.clock.startsAtEpoch - weekStart) / span) : 0
  if (count <= 0) return "This week"
  if (count === 1) return "Last week"
  return `${count} weeks ago`
}

function duration(seconds: number): string {
  const hours = Math.round(seconds / 3600)
  if (hours < 1) return "under an hour"
  if (hours < 24) return `${hours}h`
  const days = Math.floor(hours / 24)
  return hours % 24 ? `${days}d ${hours % 24}h` : `${days}d`
}

function percent(value: number): string {
  return `${Math.round(value)}%`
}

function average(values: readonly number[]): number | null {
  return values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null
}

function sessions(count: number): string {
  return `${count} wasteful ${count === 1 ? "session" : "sessions"}`
}

function share(value: number): string {
  return value > 0 && value < 1 ? "<1%" : `~${Math.round(value)}%`
}

/** The sessions with the biggest estimated share of a span. Before the
 *  oldest known session, it says that there is no detail. */
function TopSessions({
  data,
  from,
  to,
  metric = "weekly",
  heading,
}: {
  data: RadialData
  from: number
  to: number
  metric?: "weekly" | "fiveHour"
  heading?: string
}) {
  const known = data.sessions ?? []
  if (!known.length) return null
  const list = topSessions(known, from, to, metric)
  if (!list.length) {
    const oldest = Math.min(...known.map((session) => session.atEpoch))
    return to <= oldest ? <Hint>No session detail this far back</Hint> : null
  }
  return (
    <>
      <p className="mt-(--space-xs) text-label-secondary">
        {heading ?? "Top sessions"}, est. share of{" "}
        {metric === "weekly" ? "the week" : "5 hours"}
      </p>
      {list.slice(0, NEARBY_LIMIT).map((session) => (
        <p key={session.key} className="flex justify-between gap-(--space-sm)">
          <span className="min-w-0 truncate">{session.title}</span>
          <span className="shrink-0 text-label-secondary tabular-nums">
            {share(
              (metric === "weekly" ? session.weeklyPercent : session.fiveHourPercent) ?? 0,
            )}
          </span>
        </p>
      ))}
      {list.length > NEARBY_LIMIT && (
        <p className="text-label-tertiary">{list.length - NEARBY_LIMIT} more</p>
      )}
    </>
  )
}

function Row({ label, value }: { label: ReactNode; value: ReactNode }) {
  return (
    <p className="text-label-secondary">
      {label} <span className="text-label tabular-nums">{value}</span>
    </p>
  )
}

/** The checks with the most pins in a set, as "label ×N". */
function topChecks(items: readonly PlacedPin[]): string[] {
  const counts = new Map<string, number>()
  for (const item of items) counts.set(item.pin.label, (counts.get(item.pin.label) ?? 0) + 1)
  return [...counts.entries()]
    .sort((left, right) => right[1] - left[1])
    .slice(0, NEARBY_LIMIT)
    .map(([label, count]) => (count > 1 ? `${label} ×${count}` : label))
}

function timeCard(fraction: number, data: RadialData): ReactNode {
  const clockSpan = data.clock.resetsAtEpoch - data.clock.startsAtEpoch
  const epoch = data.clock.startsAtEpoch + fraction * clockSpan
  const thisWeek = data.current ? levelAt(data.current, fraction) : null
  const pastAverage = average(
    data.past
      .map((window) => levelAt(window, fraction))
      .filter((level): level is number => level != null),
  )
  const short = data.current
    ? data.spokes.find((spoke) => spoke.startsAtEpoch <= epoch && epoch < spoke.resetsAtEpoch)
    : undefined
  const nearby = data.placed.filter(
    (item) => turnDistance(item.fraction, fraction) <= (PIN_STACK_DEGREES * 1.5) / 360,
  )
  return (
    <>
      <p className="font-semibold">{formatWhen(epoch)}</p>
      <Row label="This week" value={thisWeek != null ? percent(thisWeek) : "–"} />
      {pastAverage != null && <Row label="Past weeks, average" value={percent(pastAverage)} />}
      {short && <Row label="5-hour window peak" value={percent(short.peakPercent)} />}
      {nearby.slice(0, NEARBY_LIMIT).map((item) => (
        <p key={item.key} className="truncate">
          <span className="text-brand">{item.pin.label}</span> · {item.pin.title}
        </p>
      ))}
      {nearby.length > NEARBY_LIMIT && (
        <p className="text-label-tertiary">{sessions(nearby.length - NEARBY_LIMIT)} more</p>
      )}
    </>
  )
}

function weekCard(start: number, fraction: number | null, data: RadialData): ReactNode {
  const week = data.weeks.find((window) => window.startsAtEpoch === start)
  if (!week) return null
  const isCurrent = week === data.current
  const span = week.resetsAtEpoch - week.startsAtEpoch
  const level = fraction != null ? levelAt(week, fraction) : null
  const peak = Math.max(...week.points.map((point) => point.percent))
  const last = week.points[week.points.length - 1]
  const waste = data.placed.filter((item) => item.weekStart === week.startsAtEpoch)
  return (
    <>
      <p className="font-semibold">
        {isCurrent ? "This week" : `Week of ${formatDate(week.startsAtEpoch)}`}
      </p>
      {level != null && fraction != null && (
        <Row label={formatWhen(week.startsAtEpoch + fraction * span)} value={percent(level)} />
      )}
      {isCurrent && last && (
        <Row
          label="Now"
          value={`${percent(last.percent)} · resets ${formatDate(week.resetsAtEpoch)}`}
        />
      )}
      {!isCurrent && last && <Row label="Ended at" value={percent(last.percent)} />}
      {!isCurrent && Number.isFinite(peak) && <Row label="Peak" value={percent(peak)} />}
      {waste.length > 0 && <p className="text-brand">{sessions(waste.length)}</p>}
      <TopSessions data={data} from={week.startsAtEpoch} to={week.resetsAtEpoch} />
    </>
  )
}

function dayCard(day: number, data: RadialData): ReactNode {
  const clockSpan = data.clock.resetsAtEpoch - data.clock.startsAtEpoch
  const from = day / DAYS_PER_WEEK
  const to = (day + 1) / DAYS_PER_WEEK
  // The level rises through a week, so the rise over a day is the use in it.
  const used = (window: AllowanceWindowLevelsPayload) => {
    const start = levelAt(window, from)
    const end = levelAt(window, to) ?? window.points[window.points.length - 1]?.percent
    return start != null && end != null && end >= start ? { start, end } : null
  }
  const thisWeek = data.current ? used(data.current) : null
  const pastAverage = average(
    data.past.flatMap((window) => {
      const span = used(window)
      return span ? [span.end - span.start] : []
    }),
  )
  const waste = data.placed.filter((item) => Math.floor(item.fraction * DAYS_PER_WEEK) === day)
  return (
    <>
      <p className="font-semibold">{formatDate(data.clock.startsAtEpoch + from * clockSpan)}</p>
      {thisWeek && (
        <Row
          label="Used this week"
          value={`+${percent(thisWeek.end - thisWeek.start)} (${percent(thisWeek.start)} → ${percent(thisWeek.end)})`}
        />
      )}
      {pastAverage != null && (
        <Row label="Past weeks, average" value={`+${percent(pastAverage)}`} />
      )}
      {waste.length > 0 && <p className="text-brand">{sessions(waste.length)}</p>}
      {topChecks(waste).map((line) => (
        <p key={line} className="text-label-secondary">
          {line}
        </p>
      ))}
      <TopSessions
        data={data}
        from={data.clock.startsAtEpoch + from * clockSpan}
        to={data.clock.startsAtEpoch + to * clockSpan}
      />
    </>
  )
}

function Hint({ children }: { children: ReactNode }) {
  return <p className="text-label-tertiary">{children}</p>
}

function pinCard(key: string, data: RadialData): ReactNode {
  const item = data.placed.find((placed) => placed.key === key)
  if (!item) return null
  const { pin } = item
  const facts = [
    pin.repo,
    pin.agent,
    pin.models.join(", "),
    pin.costUsd != null ? formatCost(pin.costUsd) : "",
  ].filter(Boolean)
  return (
    <>
      <p className="font-semibold">{pin.label}</p>
      <p>{pin.title}</p>
      <p className="text-label-secondary">
        {formatWhen(pin.atEpoch)} · {weeksAgo(data, item.weekStart)}
      </p>
      {facts.length > 0 && <p className="text-label-secondary">{facts.join(" · ")}</p>}
      {pin.alsoFailed.length > 0 && (
        <p>
          <span className="text-label-secondary">Also failed </span>
          <span className="text-burn-check-failure-text">{pin.alsoFailed.join(", ")}</span>
        </p>
      )}
      <Hint>Click to open the session</Hint>
    </>
  )
}

function checkCard(detector: string, data: RadialData): ReactNode {
  const items = data.placed.filter((item) => item.pin.detector === detector)
  if (!items.length) return null
  const facts = data.checks?.find((check) => check.detector === detector)
  return (
    <>
      <p className="font-semibold">{items[0]!.pin.label}</p>
      <Row label="Sessions pinned" value={items.length} />
      <Row label="This week" value={items.filter((item) => item.current).length} />
      {facts?.burnBasisPoints != null && (
        <Row
          label="Avoidable, est."
          value={`${formatTokenBurnPercent(facts.burnBasisPoints)} of tokens`}
        />
      )}
      {facts?.change && (
        <Row label="Try" value={<span className="whitespace-nowrap">{facts.change}</span>} />
      )}
      <Hint>Point at a pin to see its session</Hint>
    </>
  )
}

function configCard(detector: string, data: RadialData): ReactNode {
  const share = data.config.find((item) => item.detector === detector)
  if (!share) return null
  return (
    <>
      <p className="font-semibold">{share.label}</p>
      <Row
        label="Fails in"
        value={`${share.finding} of ${share.sessions} sessions (${percent(share.share * 100)})`}
      />
      <Hint>A setup check: fix it once, every session gains</Hint>
    </>
  )
}

function shortCard(key: string, data: RadialData): ReactNode {
  const spoke = data.spokes.find((item) => item.key === key)
  if (!spoke) return null
  return (
    <>
      <p className="font-semibold">5-hour window</p>
      <p className="text-label-secondary">
        {formatWhen(spoke.startsAtEpoch)} – {formatTime(spoke.resetsAtEpoch)}
      </p>
      <Row label="Peak" value={percent(spoke.peakPercent)} />
      {spoke.peakPercent >= LIMIT_PERCENT && (
        <p className="text-system-red-text">Hit the 5-hour limit</p>
      )}
      <TopSessions
        data={data}
        from={spoke.startsAtEpoch}
        to={spoke.resetsAtEpoch}
        metric="fiveHour"
      />
    </>
  )
}

function limitCard(weekStart: number, data: RadialData): ReactNode {
  const limit = data.limits.find((item) => item.weekStart === weekStart)
  if (!limit) return null
  const week = data.weeks.find((window) => window.startsAtEpoch === weekStart)
  return (
    <>
      <p className="font-semibold text-system-red-text">Hit the weekly limit</p>
      <Row
        label="At"
        value={`${formatDate(limit.hitAtEpoch)}, ${formatTime(limit.hitAtEpoch)}`}
      />
      <Row
        label={limit.current ? "At 100% for" : "At 100% until the reset, for"}
        value={duration(limit.untilEpoch - limit.hitAtEpoch)}
      />
      {limit.current && week && <Row label="Resets" value={formatDate(week.resetsAtEpoch)} />}
      {!limit.current && <Hint>{weeksAgo(data, weekStart)}</Hint>}
      <TopSessions
        data={data}
        from={limit.hitAtEpoch - BEFORE_HIT}
        to={limit.hitAtEpoch + AFTER_HIT}
        heading="Active around the hit"
      />
    </>
  )
}

function rollingCard(rolling: number | null): ReactNode {
  if (rolling == null) return null
  return (
    <>
      <p className="font-semibold">
        Average usage <span className="tabular-nums">{percent(rolling)}</span>
      </p>
      <p className="text-label-secondary">Typical level over the last 28 days</p>
    </>
  )
}

function body(focus: RadialFocus, fraction: number | null, data: RadialData): ReactNode {
  switch (focus.kind) {
    case "time":
      return fraction == null ? null : timeCard(fraction, data)
    case "week":
      return weekCard(focus.start, fraction, data)
    case "day":
      return dayCard(focus.day, data)
    case "short":
      return shortCard(focus.key, data)
    case "rolling":
      return rollingCard(data.rolling)
    case "limit":
      return limitCard(focus.weekStart, data)
    case "pin":
      return pinCard(focus.key, data)
    case "check":
      return checkCard(focus.detector, data)
    case "config":
      return configCard(focus.detector, data)
    case "layer":
      return null
  }
}

/** The hover card for the part of the week flower in focus. */
export function OverviewRadialTooltip({
  focus,
  fraction,
  data,
  style,
}: {
  focus: RadialFocus
  fraction: number | null
  data: RadialData
  style: CSSProperties
}) {
  const content = body(focus, fraction, data)
  if (!content) return null
  return (
    <div className="ui-tooltip pointer-events-none absolute w-max max-w-80" style={style}>
      {content}
    </div>
  )
}
