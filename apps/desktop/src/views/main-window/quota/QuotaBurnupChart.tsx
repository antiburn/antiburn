import { memo, useId, useRef } from "react"

import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"
import type { QuotaPeriodPayload } from "../../../lib/providerUsageIpc"
import { axisDayLabel } from "../../../lib/presentation/chartDates"
import { traceEvent } from "../../../lib/perfTrace"
import { useElementHeight, useElementWidth } from "../../../lib/useElementWidth"
import {
  rowsInWindow,
  slotPaceEnd,
  windowSlots,
  WINDOW_GAP_PX,
  type QuotaWindowSlot,
} from "./quotaLayout"
import { quotaBandPaths, quotaBandSpecs, quotaStackTopPath } from "./quotaPaths"
import type { QuotaSeries, QuotaSeriesRow } from "./quotaSeries"

const DAY_SECS = 24 * 60 * 60
const DAILY_TICKS_MAX_SPAN_SECS = 8 * DAY_SECS
const SHORT_WINDOW_MAX_SECS = 2 * DAY_SECS
const CHART_MIN_HEIGHT = 200
const MARGIN_TOP = 6
const TIME_AXIS_HEIGHT = 16
const VALUE_AXIS_WIDTH = 32
const WINDOW_TICK_MIN_GAP_PX = 64
const HOUR_SECS = 60 * 60
const Y_TICKS = [0, 25, 50, 75, 100]
const GRID_TICKS = [25, 50, 75, 100]

type QuotaChartSeries = string

export interface QuotaBurnupChartProps {
  rangeStartEpoch: number
  rangeEndEpoch: number
  nowEpoch: number
  // The caller selects the periods and supplies them in start order.
  periods: readonly QuotaPeriodPayload[]
  showPace: boolean
  series: QuotaSeries
  onHighlight: (series: QuotaChartSeries | null) => void
}

export function formatQuotaPercent(value: number | null | undefined): string {
  return value == null ? "—" : `${value.toFixed(1)}%`
}

function localDateOf(epoch: number): string {
  const date = new Date(epoch * 1000)
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, "0")
  const day = String(date.getDate()).padStart(2, "0")
  return `${year}-${month}-${day}`
}

function localMidnight(epoch: number): number {
  const date = new Date(epoch * 1000)
  date.setHours(0, 0, 0, 0)
  return Math.floor(date.getTime() / 1000)
}

// Use calendar days to keep ticks at midnight across daylight-saving changes.
export function xAxisTicks(rangeStart: number, rangeEnd: number): number[] {
  const stepDays = rangeEnd - rangeStart <= DAILY_TICKS_MAX_SPAN_SECS ? 1 : 7
  const ticks: number[] = []
  const date = new Date(localMidnight(rangeStart) * 1000)
  let t = Math.floor(date.getTime() / 1000)
  while (t <= rangeEnd) {
    if (t >= rangeStart) ticks.push(t)
    date.setDate(date.getDate() + stepDays)
    t = Math.floor(date.getTime() / 1000)
  }
  return ticks
}

function localHourLabel(epoch: number): string {
  return new Intl.DateTimeFormat(undefined, { hour: "numeric" })
    .format(new Date(epoch * 1000))
    .toLowerCase()
    .replace(/\s+/g, "")
}

function windowTickLabel(period: QuotaPeriodPayload): string {
  const start = period.startsAtEpoch
  const day = axisDayLabel(localDateOf(start))
  const long = period.resetsAtEpoch - start > SHORT_WINDOW_MAX_SECS
  return long ? day : `${day} ${localHourLabel(start)}`
}

interface WindowAxisTick {
  x: number
  label: string
  anchor: "start" | "middle" | "end"
}

function windowAxisTicks(slot: QuotaWindowSlot, minGapPx: number): WindowAxisTick[] {
  const { period, endsAtEpoch } = slot
  const start = period.startsAtEpoch
  const open = endsAtEpoch !== period.resetsAtEpoch
  const long = period.resetsAtEpoch - start > SHORT_WINDOW_MAX_SECS
  const inner: WindowAxisTick[] = []
  if (long) {
    for (const t of xAxisTicks(start, endsAtEpoch)) {
      if (t > start)
        inner.push({ x: slot.x(t), label: axisDayLabel(localDateOf(t)), anchor: "middle" })
    }
  } else {
    for (let t = Math.ceil(start / HOUR_SECS) * HOUR_SECS; t <= endsAtEpoch; t += HOUR_SECS) {
      if (t > start) inner.push({ x: slot.x(t), label: localHourLabel(t), anchor: "middle" })
    }
  }
  const ticks: WindowAxisTick[] = [
    { x: slot.left, label: windowTickLabel(period), anchor: "start" },
  ]
  const rightLimit = open ? slot.right - minGapPx : Number.POSITIVE_INFINITY
  // Allow more space after the first label because it extends to the right.
  let last = slot.left + minGapPx / 2
  for (const tick of inner) {
    if (tick.x - last < minGapPx || tick.x > rightLimit) continue
    ticks.push(tick)
    last = tick.x
  }
  if (open) ticks.push({ x: slot.right, label: "now", anchor: "end" })
  return ticks
}

interface BandLayerPath {
  layerKey: string
  d: string
  vertices: number
}

// The parent uses CSS to highlight bands. Memoization prevents path calculations
// when only the highlight changes.
function QuotaBurnupChartImpl({
  rangeStartEpoch,
  rangeEndEpoch,
  nowEpoch,
  periods,
  showPace,
  series,
  onHighlight,
}: QuotaBurnupChartProps) {
  const { rows, topSessions } = series
  const containerRef = useRef<HTMLDivElement | null>(null)
  const width = useElementWidth(containerRef)
  const height = useElementHeight(containerRef)
  const rawId = useId()
  const idBase = rawId.replace(/:/g, "")
  const clipId = `quota-clip-${idBase}`
  const hatchId = `quota-hatch-${idBase}`

  const plotLeft = VALUE_AXIS_WIDTH
  const plotRight = Math.max(plotLeft, width)
  const plotTop = MARGIN_TOP
  const plotBottom = Math.max(plotTop, height - TIME_AXIS_HEIGHT)
  const plotWidth = plotRight - plotLeft
  const plotHeight = plotBottom - plotTop
  // Keep values above 100% in the paths. The clipPath limits the visible area.
  const y = (v: number) => plotTop + (1 - v / 100) * plotHeight

  const slots: QuotaWindowSlot[] = windowSlots(
    periods,
    plotLeft,
    plotWidth,
    WINDOW_GAP_PX,
    nowEpoch,
  )

  const bandLayers: ReadonlyArray<{
    rows: readonly QuotaSeriesRow[]
    x: (t: number) => number
    key: string | number
  }> = slots.map((slot) => ({
    rows: rowsInWindow(rows, slot.period),
    x: slot.x,
    key: slot.period.periodId ?? slot.period.startsAtEpoch,
  }))

  const bandSpecs = quotaBandSpecs(topSessions, `url(#${hatchId})`)
  const bandKeys = bandSpecs.map((spec) => spec.key)
  const pathsByLayer = bandLayers.map((layer) =>
    quotaBandPaths(layer.rows, bandKeys, layer.x, y),
  )
  const bandLayerPaths = new Map<string, BandLayerPath[]>()
  let totalBandVertices = 0
  bandSpecs.forEach((spec, index) => {
    const paths: BandLayerPath[] = []
    pathsByLayer.forEach((layerPaths, layerIndex) => {
      const { d, vertices } = layerPaths[index]!
      if (d === "") return
      paths.push({ layerKey: `${spec.key}-${layerIndex}`, d, vertices })
      totalBandVertices += vertices
    })
    bandLayerPaths.set(spec.key, paths)
  })
  const stackTopPaths = bandLayers.map((layer) => ({
    key: layer.key,
    ...quotaStackTopPath(layer.rows, bandKeys, layer.x, y),
  }))
  const lineVertices = stackTopPaths.reduce((total, line) => total + line.vertices, 0)
  const areas = [...bandLayerPaths.values()].reduce((total, paths) => total + paths.length, 0)
  const vertices = totalBandVertices + lineVertices

  traceEvent("quota.chart.render", { rows: rows.length, areas, vertices })

  const windowTicks = slots.flatMap((slot) =>
    windowAxisTicks(slot, WINDOW_TICK_MIN_GAP_PX).map((tick) => ({
      ...tick,
      key: `${slot.period.periodId ?? slot.period.startsAtEpoch}-${tick.x}`,
    })),
  )

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div
        ref={containerRef}
        className="min-h-0 flex-1"
        style={{ minHeight: CHART_MIN_HEIGHT }}
      >
        {width > 0 && height > 0 && (
          <svg width={width} height={height} role="img" aria-label="Limits burnup">
            <defs>
              <clipPath id={clipId}>
                <rect x={plotLeft} y={plotTop} width={plotWidth} height={plotHeight} />
              </clipPath>
              {/* Use a pattern to distinguish unexplained usage without color. */}
              <pattern
                id={hatchId}
                width={6}
                height={6}
                patternUnits="userSpaceOnUse"
                patternTransform="rotate(45)"
              >
                <rect width={6} height={6} fill="transparent" />
                <line
                  x1={0}
                  y1={0}
                  x2={0}
                  y2={6}
                  stroke="var(--color-quota-unexplained)"
                  strokeWidth={2}
                />
              </pattern>
            </defs>

            {windowTicks.map((tick) => (
              <text
                key={tick.key}
                x={tick.x}
                y={plotBottom + 2}
                textAnchor={tick.anchor}
                dominantBaseline="hanging"
                {...AXIS_TICK}
              >
                {tick.label}
              </text>
            ))}

            {Y_TICKS.map((value) => (
              <text
                key={value}
                x={0}
                y={y(value)}
                textAnchor="start"
                dominantBaseline="middle"
                {...AXIS_TICK}
              >
                {value}%
              </text>
            ))}
            {GRID_TICKS.map((value) => (
              <line
                key={`grid-${value}`}
                data-quota-line="grid"
                x1={plotLeft}
                x2={plotRight}
                y1={y(value)}
                y2={y(value)}
                stroke="var(--color-quota-reset)"
              />
            ))}

            {slots.map((slot) => {
              const t = slot.period.resetsAtEpoch
              if (slot.endsAtEpoch !== t) return null
              if (t < rangeStartEpoch || t > rangeEndEpoch) return null
              return (
                <line
                  key={`reset-${slot.period.periodId ?? t}`}
                  data-quota-line="reset"
                  x1={slot.right}
                  x2={slot.right}
                  y1={plotTop}
                  y2={plotBottom}
                  stroke="var(--color-quota-reset)"
                />
              )
            })}
            {bandSpecs.flatMap((spec) =>
              (bandLayerPaths.get(spec.key) ?? []).map(({ layerKey, d }) => (
                <g
                  key={layerKey}
                  onMouseEnter={() => onHighlight(spec.key)}
                  onMouseLeave={() => onHighlight(null)}
                >
                  <path
                    className={spec.className}
                    d={d}
                    fill={spec.fill}
                    clipPath={`url(#${clipId})`}
                  />
                </g>
              )),
            )}
            {stackTopPaths.map((stackTopPath) => (
              <path
                key={stackTopPath.key}
                className="quota-line-top"
                d={stackTopPath.d}
                fill="none"
                stroke="var(--color-quota-meter)"
                strokeWidth={1.5}
                clipPath={`url(#${clipId})`}
              />
            ))}

            {showPace &&
              slots.map((slot) => (
                <line
                  key={slot.period.periodId ?? slot.period.startsAtEpoch}
                  data-quota-line="pace"
                  x1={slot.left}
                  y1={y(0)}
                  x2={slot.right}
                  y2={y(slotPaceEnd(slot))}
                  stroke="var(--color-quota-pace)"
                  strokeWidth={1}
                  strokeDasharray="1 3"
                  strokeLinecap="round"
                  clipPath={`url(#${clipId})`}
                />
              ))}
          </svg>
        )}
      </div>
    </div>
  )
}

export const QuotaBurnupChart = memo(QuotaBurnupChartImpl)
