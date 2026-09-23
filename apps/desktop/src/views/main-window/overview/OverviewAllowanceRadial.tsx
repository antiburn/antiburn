import { useRef, type CSSProperties, type ReactNode } from "react"

import type {
  AllowanceUsageAccountPayload,
  AllowanceWindowLevelsPayload,
} from "../../../lib/providerUsageIpc"
import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"
import { ChartLegend } from "../../../components/ui/ChartLegend"
import { useElementHeight, useElementWidth } from "../../../lib/useElementWidth"

const DAYS_PER_WEEK = 7
// The outer ring stays this far inside the square, so the labels fit.
const EDGE_GAP = 16
// The hole in the middle holds the round Optimise button. These set its share
// of the chart and the gap between the button and the inner ring.
const HOLE_SHARE = 0.15
// The smallest hole that still fits the button label.
const MIN_HOLE = 88
const HOLE_GAP = 8

const LEGEND_ITEMS = [
  { key: "week", label: "This week", swatch: "bg-context-stroke" },
  { key: "past", label: "Past weeks", swatch: "bg-context-stroke/30" },
  { key: "short", label: "5-hour window", swatch: "bg-context-stroke/20", shape: "line" },
  { key: "rolling", label: "Average usage", swatch: "bg-gray-500", shape: "line" },
] as const

type Point = { x: number; y: number }

/** The chart geometry in pixels, from the side of the square. */
type Geometry = {
  center: number
  inner: number
  outer: number
  hole: number
}

function geometry(side: number): Geometry {
  const center = side / 2
  const outer = Math.max(0, center - EDGE_GAP)
  const hole = Math.max(MIN_HOLE, side * HOLE_SHARE)
  return { center, outer, hole, inner: Math.min(outer, hole / 2 + HOLE_GAP) }
}

function radius(g: Geometry, percent: number): number {
  return g.inner + (Math.min(100, Math.max(0, percent)) / 100) * (g.outer - g.inner)
}

/** The point for a fraction of one week. Zero is the reset, at the top. The
 *  week runs clockwise. */
function polar(g: Geometry, fraction: number, r: number): Point {
  const angle = -Math.PI / 2 + fraction * 2 * Math.PI
  return { x: g.center + r * Math.cos(angle), y: g.center + r * Math.sin(angle) }
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
  g: Geometry,
  window: AllowanceWindowLevelsPayload,
): { area: string; edge: string } | null {
  if (window.points.length < 2) return null
  const edge = window.points
    .map((point, index) => {
      const at = polar(g, weekFraction(window, point.atEpoch), radius(g, point.percent))
      return `${index === 0 ? "M" : "L"}${fmt(at)}`
    })
    .join(" ")
  const first = weekFraction(window, window.points[0]!.atEpoch)
  const last = weekFraction(window, window.points[window.points.length - 1]!.atEpoch)
  const largeArc = last - first > 0.5 ? 1 : 0
  const area =
    `${edge} L${fmt(polar(g, last, g.inner))} ` +
    `A${g.inner},${g.inner} 0 ${largeArc} 0 ${fmt(polar(g, first, g.inner))} Z`
  return { area, edge }
}

/** The allowance chart drawn around one week, so every week overlaps the
 *  others. The angle is the time since the weekly reset. The radius is the
 *  level. The chart fills the space it gets, as a square. */
export function OverviewAllowanceRadial({
  account,
  rangeEndEpoch,
  controls,
  center,
}: {
  account: AllowanceUsageAccountPayload
  rangeEndEpoch: number
  controls?: ReactNode
  /** The content in the hole of the chart. */
  center?: ReactNode
}) {
  const frameRef = useRef<HTMLDivElement | null>(null)
  const side = Math.min(useElementWidth(frameRef), useElementHeight(frameRef))
  const g = geometry(side)

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
        from: polar(g, fraction, g.inner),
        to: polar(g, fraction, radius(g, short.peakPercent)),
      },
    ]
  })

  const currentPetal = current ? petalPath(g, current) : null
  const currentLast = current?.points[current.points.length - 1]
  const currentTip =
    current && currentLast
      ? polar(g, weekFraction(current, currentLast.atEpoch), radius(g, currentLast.percent))
      : null

  const summary =
    `${account.displayName}: ${weeks.length} weekly windows drawn on one week.` +
    (latestRolling != null ? ` Average usage is ${Math.round(latestRolling)} percent.` : "")

  return (
    <section className="overview-chart" aria-label="Allowance chart">
      <p className="sr-only">{summary}</p>
      <div className="overview-chart-legend mb-(--space-sm) grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-(--space-md)">
        <ChartLegend ariaLabel="Layers" items={LEGEND_ITEMS} />
        {controls}
      </div>
      <div ref={frameRef} className="relative min-h-0 flex-1">
        {side > 0 && (
          <div
            className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2"
            style={{ inlineSize: side, blockSize: side }}
          >
            <svg width={side} height={side} className="block" aria-hidden="true">
              {[50, 100].map((percent) => (
                <circle
                  key={percent}
                  cx={g.center}
                  cy={g.center}
                  r={radius(g, percent)}
                  fill="none"
                  stroke="var(--color-separator)"
                  strokeWidth={1}
                />
              ))}
              {Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
                const from = polar(g, day / DAYS_PER_WEEK, g.inner)
                const to = polar(g, day / DAYS_PER_WEEK, g.outer)
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
                const petal = petalPath(g, window)
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
                  cx={g.center}
                  cy={g.center}
                  r={radius(g, latestRolling)}
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
                <circle
                  cx={currentTip.x}
                  cy={currentTip.y}
                  r={3.5}
                  className="fill-context-stroke"
                />
              )}

              <text
                x={g.center + 5}
                y={g.center - radius(g, 100) - 5}
                textAnchor="start"
                {...AXIS_TICK}
              >
                100% · reset
              </text>
              <text
                x={g.center + 5}
                y={g.center - radius(g, 50) - 4}
                textAnchor="start"
                {...AXIS_TICK}
              >
                50%
              </text>
            </svg>
            {center && (
              <div
                className="absolute inset-0 flex items-center justify-center"
                style={{ "--overview-hole-size": `${g.hole}px` } as CSSProperties}
              >
                {center}
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  )
}
