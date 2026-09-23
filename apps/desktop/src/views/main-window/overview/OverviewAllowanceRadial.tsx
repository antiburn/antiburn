import type { ReactNode } from "react"

import type {
  AllowanceUsageAccountPayload,
  AllowanceWindowLevelsPayload,
} from "../../../lib/providerUsageIpc"
import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"
import { ChartLegend } from "../../../components/ui/ChartLegend"

// One unit is one pixel at full size, so the axis labels keep the shared tick size.
const SIZE = 320
const CENTER = SIZE / 2
const INNER_RADIUS = 20
const OUTER_RADIUS = 140
const DAYS_PER_WEEK = 7

const LEGEND_ITEMS = [
  { key: "week", label: "This week", swatch: "bg-context-stroke" },
  { key: "past", label: "Past weeks", swatch: "bg-context-stroke/30" },
  { key: "short", label: "5-hour window", swatch: "bg-context-stroke/20", shape: "line" },
  { key: "rolling", label: "Average usage", swatch: "bg-gray-500", shape: "line" },
] as const

type Point = { x: number; y: number }

function radius(percent: number): number {
  return (
    INNER_RADIUS + (Math.min(100, Math.max(0, percent)) / 100) * (OUTER_RADIUS - INNER_RADIUS)
  )
}

/** The angle for a fraction of one week. Zero is the reset, at the top. The
 *  week runs clockwise. */
function polar(fraction: number, r: number): Point {
  const angle = -Math.PI / 2 + fraction * 2 * Math.PI
  return { x: CENTER + r * Math.cos(angle), y: CENTER + r * Math.sin(angle) }
}

function weekFraction(window: AllowanceWindowLevelsPayload, epoch: number): number {
  const span = window.resetsAtEpoch - window.startsAtEpoch
  if (span <= 0) return 0
  return Math.min(1, Math.max(0, (epoch - window.startsAtEpoch) / span))
}

function fmt(point: Point): string {
  return `${point.x.toFixed(2)},${point.y.toFixed(2)}`
}

/** One week as a petal: the rising level out from the inner ring, then back
 *  along the inner ring to the reset. */
function petalPath(
  window: AllowanceWindowLevelsPayload,
): { area: string; edge: string } | null {
  if (window.points.length < 2) return null
  const edgePoints = window.points.map((point) =>
    polar(weekFraction(window, point.atEpoch), radius(point.percent)),
  )
  const edge = edgePoints
    .map((point, index) => `${index === 0 ? "M" : "L"}${fmt(point)}`)
    .join(" ")
  const first = weekFraction(window, window.points[0]!.atEpoch)
  const last = weekFraction(window, window.points[window.points.length - 1]!.atEpoch)
  const innerLast = polar(last, INNER_RADIUS)
  const innerFirst = polar(first, INNER_RADIUS)
  const largeArc = last - first > 0.5 ? 1 : 0
  const area =
    `${edge} L${fmt(innerLast)} ` +
    `A${INNER_RADIUS},${INNER_RADIUS} 0 ${largeArc} 0 ${fmt(innerFirst)} Z`
  return { area, edge }
}

/** The allowance chart drawn around one week, so every week overlaps the
 *  others. The angle is the time since the weekly reset. The radius is the
 *  level. */
export function OverviewAllowanceRadial({
  account,
  rangeEndEpoch,
  controls,
}: {
  account: AllowanceUsageAccountPayload
  rangeEndEpoch: number
  controls?: ReactNode
}) {
  const weeks = account.chart.weeklyWindows.filter((window) => window.lane === "weekly")
  const current = weeks.find(
    (window) => window.startsAtEpoch <= rangeEndEpoch && rangeEndEpoch < window.resetsAtEpoch,
  )
  const past = weeks.filter((window) => window !== current)
  const latestRolling = [...account.chart.rolling]
    .reverse()
    .find((point) => point.atEpoch <= rangeEndEpoch && point.percent != null)?.percent

  const spokes = account.chart.shortWindows.flatMap((short) => {
    const mid = (short.startsAtEpoch + short.resetsAtEpoch) / 2
    const week = weeks.find(
      (window) => window.startsAtEpoch <= mid && mid < window.resetsAtEpoch,
    )
    if (!week) return []
    const fraction = weekFraction(week, mid)
    return [
      {
        key: `${short.startsAtEpoch}-${short.resetsAtEpoch}`,
        from: polar(fraction, INNER_RADIUS),
        to: polar(fraction, radius(short.peakPercent)),
      },
    ]
  })

  const currentPetal = current ? petalPath(current) : null
  const currentTip =
    current && current.points.length > 0
      ? (() => {
          const point = current.points[current.points.length - 1]!
          return polar(weekFraction(current, point.atEpoch), radius(point.percent))
        })()
      : null

  const summary =
    `${account.displayName}: ${weeks.length} weekly windows drawn on one week.` +
    (latestRolling != null ? ` Average usage is ${Math.round(latestRolling)} percent.` : "")

  return (
    <section className="overview-chart" aria-label="Allowance chart by week">
      <p className="sr-only">{summary}</p>
      <div className="overview-chart-legend mb-(--space-sm) grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-(--space-md)">
        <ChartLegend ariaLabel="Layers" items={LEGEND_ITEMS} />
        {controls}
      </div>
      <svg
        viewBox={`0 0 ${SIZE} ${SIZE}`}
        className="mx-auto block aspect-square w-full max-w-80"
        aria-hidden="true"
      >
        {[50, 100].map((percent) => (
          <circle
            key={percent}
            cx={CENTER}
            cy={CENTER}
            r={radius(percent)}
            fill="none"
            stroke="var(--color-separator)"
            strokeWidth={1}
          />
        ))}
        {Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
          const from = polar(day / DAYS_PER_WEEK, INNER_RADIUS)
          const to = polar(day / DAYS_PER_WEEK, OUTER_RADIUS)
          return (
            <line
              key={day}
              x1={from.x}
              y1={from.y}
              x2={to.x}
              y2={to.y}
              stroke="var(--color-separator)"
              strokeWidth={1}
            />
          )
        })}

        {spokes.map((spoke) => (
          <line
            key={spoke.key}
            x1={spoke.from.x}
            y1={spoke.from.y}
            x2={spoke.to.x}
            y2={spoke.to.y}
            className="stroke-context-stroke/20"
            strokeWidth={2.5}
            strokeLinecap="round"
          />
        ))}

        {past.map((window) => {
          const petal = petalPath(window)
          if (!petal) return null
          return (
            <g key={window.startsAtEpoch}>
              <path d={petal.area} className="fill-context-stroke/[0.07]" />
              <path
                d={petal.edge}
                fill="none"
                className="stroke-context-stroke/35"
                strokeWidth={1}
              />
            </g>
          )
        })}

        {latestRolling != null && (
          <circle
            cx={CENTER}
            cy={CENTER}
            r={radius(latestRolling)}
            fill="none"
            className="stroke-gray-500"
            strokeWidth={1}
          />
        )}

        {currentPetal && (
          <g>
            <path d={currentPetal.area} className="fill-context-stroke/25" />
            <path
              d={currentPetal.edge}
              fill="none"
              className="stroke-context-stroke"
              strokeWidth={2}
              strokeLinejoin="round"
            />
          </g>
        )}
        {currentTip && (
          <circle cx={currentTip.x} cy={currentTip.y} r={3.5} className="fill-context-stroke" />
        )}

        <text x={CENTER + 5} y={CENTER - radius(100) - 5} textAnchor="start" {...AXIS_TICK}>
          100% · reset
        </text>
        <text x={CENTER + 5} y={CENTER - radius(50) - 4} textAnchor="start" {...AXIS_TICK}>
          50%
        </text>
      </svg>
    </section>
  )
}
