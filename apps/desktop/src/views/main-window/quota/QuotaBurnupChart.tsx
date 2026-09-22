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
import { quotaBandPath, quotaBandSpecs, quotaStackTopPath } from "./quotaPaths"
import type { QuotaSeries, QuotaSeriesRow } from "./quotaSeries"

const DAY_SECS = 24 * 60 * 60
/** Switch from a daily to a weekly x-axis tick past this span. */
const DAILY_TICKS_MAX_SPAN_SECS = 8 * DAY_SECS
/** A window at or under this span draws a day+hour tick label instead of a
 *  bare day, since several of its own windows can land on the same day. */
const SHORT_WINDOW_MAX_SECS = 2 * DAY_SECS
const CHART_MIN_HEIGHT = 200
const MARGIN_TOP = 6
const MARGIN_RIGHT = 12
const TIME_AXIS_HEIGHT = 16
// Wider than a plain value axis (44px): the extra room keeps the "100%"
// tick clear of the plot's left edge.
const VALUE_AXIS_WIDTH = 56
/** The least a reader needs between two x-axis tick labels to read both. */
const WINDOW_TICK_MIN_GAP_PX = 64
const HOUR_SECS = 60 * 60
const Y_TICKS = [0, 25, 50, 75, 100]
/** Horizontal gridlines: every tick but 0%, which the plot's own bottom
 *  edge already marks. */
const GRID_TICKS = [25, 50, 75, 100]

/** A layer of the burnup chart: `"meter"`, `"other"`, `"unattributed"`,
 *  `"unexplained"`, or a top session's key. */
type QuotaChartSeries = string

export interface QuotaBurnupChartProps {
  rangeStartEpoch: number
  rangeEndEpoch: number
  nowEpoch: number
  /** The windows to draw, in start order. The chart draws only these — it
   *  no longer filters `usage.periods` itself, so the caller (which also
   *  decides which windows a window-preset range shows) stays the single
   *  source of truth for what is on screen. */
  periods: readonly QuotaPeriodPayload[]
  /** False hides the pace lines. */
  showPace: boolean
  /** The chart's rows and top sessions, built once by the caller so the
   *  top-sessions list can share the same top-session ranking. */
  series: QuotaSeries
  onHighlight: (series: QuotaChartSeries | null) => void
}

export function formatQuotaPercent(value: number | null | undefined): string {
  return value == null ? "—" : `${value.toFixed(1)}%`
}

/** The reader-local `YYYY-MM-DD` date an epoch-second instant falls on. */
function localDateOf(epoch: number): string {
  const date = new Date(epoch * 1000)
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, "0")
  const day = String(date.getDate()).padStart(2, "0")
  return `${year}-${month}-${day}`
}

/** Local midnight at or before `epoch`. */
function localMidnight(epoch: number): number {
  const date = new Date(epoch * 1000)
  date.setHours(0, 0, 0, 0)
  return Math.floor(date.getTime() / 1000)
}

/** One tick per day for a range up to eight days, else one tick per week.
 *  Each tick advances by calendar day, not by a fixed number of seconds: a
 *  daylight-saving change shifts local midnight by an hour, and adding
 *  `stepSecs` would carry that shift into every later tick. */
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

/** The reader-local hour a wall-clock instant falls on, lower-cased and
 *  without the space `Intl` puts before "am"/"pm" (`"2pm"`, `"14:00"` for a
 *  24-hour locale). No existing helper in the codebase gives the hour alone
 *  without also stating the minute. */
function localHourLabel(epoch: number): string {
  return new Intl.DateTimeFormat(undefined, { hour: "numeric" })
    .format(new Date(epoch * 1000))
    .toLowerCase()
    .replace(/\s+/g, "")
}

/** A window tick's label: the bare day for a window long enough that only
 *  one of it lands on a day (a weekly window), else the day plus the hour,
 *  since several short windows can share a day. */
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

/** The x-axis labels of one window slot: its start at the left edge, a
 *  label per local midnight inside a long window or per whole hour inside
 *  a short one, and "now" at the right edge of the open window. The edge
 *  labels always draw; an inner label draws only when it clears both the
 *  label before it and the right edge by `minGapPx`. */
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
  // The start label is anchored at its left edge, so it reaches half a gap
  // further right than a centred label would.
  let last = slot.left + minGapPx / 2
  for (const tick of inner) {
    if (tick.x - last < minGapPx || tick.x > rightLimit) continue
    ticks.push(tick)
    last = tick.x
  }
  if (open) ticks.push({ x: slot.right, label: "now", anchor: "end" })
  return ticks
}

/** One band's path across one slot, keyed for React and skipped when the
 *  slot holds none of the band's own usage. */
interface BandLayerPath {
  layerKey: string
  d: string
  vertices: number
}

/**
 * The Quota screen's burnup chart: a heavy line across the top of the
 * device's stacked estimate, tracing the stack's own sum. Under the
 * shared-meter model this sum equals the provider's own meter at every row
 * with a reading, and the device's estimate elsewhere, so one line carries
 * both. Each window in `periods` gets its own equal-width slot, back to
 * back, so a short five-hour window and a long weekly window compare at the
 * same width instead of the short one shrinking to a sliver. The open
 * window's slot maps its start to "now" across the full width, so a window
 * one day in still fills the plot. The top-sessions list below names each
 * layer; hovering a layer here or a row there highlights the same series in
 * both places.
 *
 * Hand-drawn SVG, not recharts: a 30-day range can carry thousands of rows
 * across dozens of stacked bands, and recharts forces every band to walk
 * every row and re-mount an entrance animation on each data refresh. This
 * chart instead draws each band only across the buckets where it has usage,
 * with no animation and no tooltip.
 *
 * Highlighting never touches this component's own render: each band carries
 * a stable `className`, and the caller dims the rested ones through a CSS
 * attribute on a wrapper outside this chart. Wrapped in `memo` so a hover
 * change, which does not alter any prop here, skips this chart's own
 * re-render and the path recompute that would follow across every band.
 *
 * With `showPace`, the chart draws one dotted pace line per window, the
 * constant rate that spends the window evenly from 0% at its start to 100%
 * at its reset. In the open window the line stops where "now" falls on
 * that rate, at the slot's right edge.
 */
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
  const plotRight = Math.max(plotLeft, width - MARGIN_RIGHT)
  const plotTop = MARGIN_TOP
  const plotBottom = Math.max(plotTop, height - TIME_AXIS_HEIGHT)
  const plotWidth = plotRight - plotLeft
  const plotHeight = plotBottom - plotTop
  // No clamping here: a `<clipPath>` sized to the plot area clips a band or
  // the meter where it runs over 100%, in place of recharts' `allowDataOverflow`.
  const y = (v: number) => plotTop + (1 - v / 100) * plotHeight

  // One slot per window: each owns an equal width and its own local scale.
  const slots: QuotaWindowSlot[] = windowSlots(
    periods,
    plotLeft,
    plotWidth,
    WINDOW_GAP_PX,
    nowEpoch,
  )

  // A band draws once per slot, over just that window's own rows, on the
  // slot's own scale. Each layer carries the same key as its slot, for the
  // stack-top line below.
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
  const bandLayerPaths = new Map<string, BandLayerPath[]>()
  let totalBandVertices = 0
  bandSpecs.forEach((spec, index) => {
    const paths: BandLayerPath[] = []
    bandLayers.forEach((layer, layerIndex) => {
      const { d, vertices } = quotaBandPath(layer.rows, bandKeys, index, layer.x, y)
      // Skip a slot where the band held no usage of its own: an empty path
      // there would still register a hover group with nothing inside it.
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

  // Each slot labels itself: its window's start first, then the days or
  // hours inside it, and "now" at the open window's right edge. The start
  // always labels, so a slot narrower than the gap still names its window.
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
              {/* A diagonal hatch, not a new hue, marks the "unexplained"
                  band: the maintainer has a colour-vision deficiency, so the
                  band must read apart from "unattributed" by shape, not
                  color alone. */}
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
                x={plotLeft - 4}
                y={y(value)}
                textAnchor="end"
                dominantBaseline="middle"
                {...AXIS_TICK}
              >
                {value}%
              </text>
            ))}

            {/* Gridlines draw under the bands: this block sits before every
                band path below it in document order. Reuses the reset
                line's own grey — the same light, CVD-safe tone reads right
                for a structural gridline too, so the chart needs no second
                grey token. */}
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
              // The open window's slot ends at now, not at its reset.
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

            {/* One group per band per slot, each with its own hover
                handlers: a session carries one group per window it appears
                in, but every one names the same series key, so hovering any
                of them highlights the same row in the list below. */}
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
