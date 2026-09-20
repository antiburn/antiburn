import { memo, useId, useRef } from "react"

import {
  AXIS_TICK,
  labeledIndices,
  pillGeometry,
} from "../../../components/session/analysis/chartLabels"
import type { QuotaPeriodPayload } from "../../../lib/providerUsageIpc"
import { axisDayLabel } from "../../../lib/presentation/overviewChart"
import { traceEvent } from "../../../lib/perfTrace"
import { useElementHeight, useElementWidth } from "../../../lib/useElementWidth"
import { rowsInWindow, windowSlots, type QuotaWindowSlot } from "./quotaLayout"
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
/** The least a reader needs between two window-tick labels to read both. */
const WINDOW_TICK_MIN_GAP_PX = 64
const Y_TICKS = [0, 25, 50, 75, 100]
/** Horizontal gridlines: every tick but 0%, which the plot's own bottom
 *  edge already marks. */
const GRID_TICKS = [25, 50, 75, 100]

/** A layer of the burnup chart: `"meter"`, `"other"`, `"unattributed"`,
 *  `"unexplained"`, or a top session's key. */
type QuotaChartSeries = string

export type QuotaChartAxisMode = "window" | "date"

export interface QuotaBurnupChartProps {
  rangeStartEpoch: number
  rangeEndEpoch: number
  nowEpoch: number
  /** The windows to draw, in start order. The chart draws only these — it
   *  no longer filters `usage.periods` itself, so the caller (which also
   *  decides which windows a window-preset range shows) stays the single
   *  source of truth for what is on screen. */
  periods: readonly QuotaPeriodPayload[]
  /** `"window"` gives each period its own equal-width slot along the plot;
   *  `"date"` draws one shared wall-clock x axis, today's rendering. */
  axisMode: QuotaChartAxisMode
  /** False draws each band once, at full strength, with no pace line and no
   *  above-pace highlight. */
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

/** One tick per day for a range up to eight days, else one tick per week. */
function xAxisTicks(rangeStart: number, rangeEnd: number): number[] {
  const stepSecs = rangeEnd - rangeStart <= DAILY_TICKS_MAX_SPAN_SECS ? DAY_SECS : 7 * DAY_SECS
  const ticks: number[] = []
  for (let t = localMidnight(rangeStart); t <= rangeEnd; t += stepSecs) {
    if (t >= rangeStart) ticks.push(t)
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

/** A pill-backed label drawn inside the plot, for the "now" line. Plain
 *  SVG, not recharts: this chart never renders a recharts `Text`, the cost
 *  the dev trace found in the entrance animation and stacked-area redraw. */
function LinePill({ x, y, text }: { x: number; y: number; text: string }) {
  const rect = pillGeometry(text, x, y, "middle", "start", AXIS_TICK.fontSize)
  return (
    <g>
      <rect
        x={rect.x}
        y={rect.y}
        width={rect.width}
        height={rect.height}
        rx={rect.height / 2}
        fill="var(--color-chart-label-pill)"
      />
      <text x={x} y={y} textAnchor="middle" dominantBaseline="hanging" {...AXIS_TICK}>
        {text}
      </text>
    </g>
  )
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
 * both. `axisMode="date"` draws one shared wall-clock x
 * axis across every window in `periods`; `axisMode="window"` instead gives
 * each window its own equal-width slot, back to back, so a short five-hour
 * window and a long weekly window compare at the same width instead of the
 * short one shrinking to a sliver. The top-sessions list below names each
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
 * at its reset, and each band draws twice, through two clip paths: once
 * faded, clipped to the whole plot, for the usage that stays under its
 * window's pace line, and once at full strength, clipped to the region
 * above the pace line, for the usage that runs ahead of it. Without
 * `showPace`, each band draws once, at full strength, and no pace line or
 * above-pace highlight appears.
 */
function QuotaBurnupChartImpl({
  rangeStartEpoch,
  rangeEndEpoch,
  nowEpoch,
  periods,
  axisMode,
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
  const aboveClipId = `quota-above-${idBase}`
  const hatchId = `quota-hatch-${idBase}`

  const plotLeft = VALUE_AXIS_WIDTH
  const plotRight = Math.max(plotLeft, width - MARGIN_RIGHT)
  const plotTop = MARGIN_TOP
  const plotBottom = Math.max(plotTop, height - TIME_AXIS_HEIGHT)
  const plotWidth = plotRight - plotLeft
  const plotHeight = plotBottom - plotTop
  const span = rangeEndEpoch - rangeStartEpoch || 1
  const sharedX = (t: number) => plotLeft + ((t - rangeStartEpoch) / span) * plotWidth
  // No clamping here: a `<clipPath>` sized to the plot area clips a band or
  // the meter where it runs over 100%, in place of recharts' `allowDataOverflow`.
  const y = (v: number) => plotTop + (1 - v / 100) * plotHeight

  // One slot per window either way: in date mode every slot shares the same
  // linear scale, so a period's own start and reset just read off it; in
  // window mode each slot owns an equal width and its own local scale.
  const slots: QuotaWindowSlot[] =
    axisMode === "window"
      ? windowSlots(periods, plotLeft, plotWidth)
      : periods.map((period) => ({
          period,
          left: sharedX(period.startsAtEpoch),
          right: sharedX(period.resetsAtEpoch),
          x: sharedX,
        }))

  // Date mode draws every band once, over the whole unsliced series, on the
  // shared scale — exactly today's rendering. Window mode draws a band once
  // per slot, over just that window's own rows, on the slot's own scale.
  const bandLayers: ReadonlyArray<{
    rows: readonly QuotaSeriesRow[]
    x: (t: number) => number
  }> =
    axisMode === "window"
      ? slots.map((slot) => ({ rows: rowsInWindow(rows, slot.period), x: slot.x }))
      : [{ rows, x: sharedX }]

  const bandSpecs = quotaBandSpecs(topSessions, `url(#${hatchId})`)
  const bandKeys = bandSpecs.map((spec) => spec.key)
  const bandLayerPaths = new Map<string, BandLayerPath[]>()
  let totalBandVertices = 0
  bandSpecs.forEach((spec, index) => {
    const paths: BandLayerPath[] = []
    bandLayers.forEach((layer, layerIndex) => {
      const { d, vertices } = quotaBandPath(layer.rows, bandKeys, index, layer.x, y)
      // Date mode draws one path per band regardless — exactly today's
      // rendering. Window mode skips a slot where the band held no usage of
      // its own, since an empty path there would otherwise still register a
      // hover group with nothing inside it.
      if (d === "" && axisMode === "window") return
      paths.push({ layerKey: `${spec.key}-${layerIndex}`, d, vertices })
      totalBandVertices += vertices
    })
    bandLayerPaths.set(spec.key, paths)
  })
  const stackTopPaths = bandLayers.map((layer) =>
    quotaStackTopPath(layer.rows, bandKeys, layer.x, y),
  )
  const lineVertices = stackTopPaths.reduce((total, line) => total + line.vertices, 0)
  // Every drawn band draws twice with a pace line (faded under it, full
  // above it); without one it draws once.
  const areas = [...bandLayerPaths.values()].reduce((total, paths) => total + paths.length, 0)
  const vertices = totalBandVertices * (showPace ? 2 : 1) + lineVertices

  traceEvent("quota.chart.render", { rows: rows.length, areas, vertices })

  const windowTickLefts = axisMode === "window" ? slots.map((slot) => slot.left) : []
  const labeledWindowTicks =
    axisMode === "window"
      ? labeledIndices(
          windowTickLefts,
          Math.max(1, plotWidth),
          WINDOW_TICK_MIN_GAP_PX / plotWidth,
        )
      : new Set<number>()

  const dateTicks = axisMode === "date" ? xAxisTicks(rangeStartEpoch, rangeEndEpoch) : []
  const dateTickLabels = new Map(dateTicks.map((t) => [t, axisDayLabel(localDateOf(t))]))

  const showNowLine =
    axisMode === "date"
      ? nowEpoch >= rangeStartEpoch && nowEpoch <= rangeEndEpoch
      : slots.some(
          (slot) =>
            nowEpoch >= slot.period.startsAtEpoch && nowEpoch < slot.period.resetsAtEpoch,
        )
  const nowSlot =
    axisMode === "window"
      ? (slots.find(
          (slot) =>
            nowEpoch >= slot.period.startsAtEpoch && nowEpoch < slot.period.resetsAtEpoch,
        ) ?? null)
      : null
  const nowX = nowSlot ? nowSlot.x(nowEpoch) : sharedX(nowEpoch)

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
              {showPace && (
                // The region on or above each window's pace line, up to the
                // plot top. Nested inside the plot clip, so it also bounds a
                // window that starts before the range or resets after it.
                <clipPath id={aboveClipId} clipPath={`url(#${clipId})`}>
                  {slots.map((slot) => (
                    <polygon
                      key={slot.period.periodId ?? slot.period.startsAtEpoch}
                      points={`${slot.left},${y(0)} ${slot.right},${y(100)} ${slot.right},${plotTop} ${slot.left},${plotTop}`}
                    />
                  ))}
                </clipPath>
              )}
            </defs>

            {axisMode === "date"
              ? dateTicks.map((t) => (
                  <text
                    key={t}
                    x={sharedX(t)}
                    y={plotBottom + 2}
                    textAnchor="middle"
                    dominantBaseline="hanging"
                    {...AXIS_TICK}
                  >
                    {dateTickLabels.get(t) ?? ""}
                  </text>
                ))
              : slots.map(
                  (slot) =>
                    labeledWindowTicks.has(slot.left) && (
                      <text
                        key={slot.period.periodId ?? slot.period.startsAtEpoch}
                        x={slot.left}
                        y={plotBottom + 2}
                        textAnchor="start"
                        dominantBaseline="hanging"
                        {...AXIS_TICK}
                      >
                        {windowTickLabel(slot.period)}
                      </text>
                    ),
                )}

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

            {showNowLine && (
              <g>
                <line
                  data-quota-line="now"
                  x1={nowX}
                  x2={nowX}
                  y1={plotTop}
                  y2={plotBottom}
                  stroke="var(--color-label)"
                />
                <LinePill x={nowX} y={plotTop} text="now" />
              </g>
            )}

            {/* One group per band per slot, each with its own hover
                handlers: a session then carries several groups in window
                mode, one per window it appears in, but every one names the
                same series key, so hovering any of them highlights the same
                row in the list below. */}
            {bandSpecs.flatMap((spec) =>
              (bandLayerPaths.get(spec.key) ?? []).map(({ layerKey, d }) => (
                <g
                  key={layerKey}
                  onMouseEnter={() => onHighlight(spec.key)}
                  onMouseLeave={() => onHighlight(null)}
                >
                  {showPace ? (
                    <>
                      {/* The whole band, faded: usage that stays under the
                          window's pace line. */}
                      <path
                        className={`${spec.className} quota-area-under-pace`}
                        d={d}
                        fill={spec.fill}
                        clipPath={`url(#${clipId})`}
                      />
                      {/* The same band at full strength, clipped to the
                          region above the pace line. */}
                      <path
                        className={spec.className}
                        d={d}
                        fill={spec.fill}
                        clipPath={`url(#${aboveClipId})`}
                      />
                    </>
                  ) : (
                    <path
                      className={spec.className}
                      d={d}
                      fill={spec.fill}
                      clipPath={`url(#${clipId})`}
                    />
                  )}
                </g>
              )),
            )}
            {stackTopPaths.map((stackTopPath, index) => (
              <path
                key={index}
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
                  y2={y(100)}
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
