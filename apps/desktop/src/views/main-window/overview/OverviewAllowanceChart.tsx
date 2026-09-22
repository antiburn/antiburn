import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react"

import type {
  AllowanceRollingPointPayload,
  AllowanceUsageAccountPayload,
  AllowanceWindowLevelsPayload,
  AllowanceWindowPeakPayload,
} from "../../../lib/providerUsageIpc"
import { axisDayLabel, dayLabel } from "../../../lib/presentation/chartDates"
import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"

import { Tooltip } from "../../../components/presentation/Tooltip"
import { ChartLegend } from "../../../components/ui/ChartLegend"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { useElementHeight, useElementWidth } from "../../../lib/useElementWidth"

import "./overview.css"

const MARGIN_TOP = 8
const TIME_AXIS_HEIGHT = 16
const VALUE_AXIS_WIDTH = 44
const DAY_TOOLTIP_DELAY_MS = 100

const DAY_SECS = 24 * 60 * 60
const CHART_DAYS = 30
const AXIS_LABEL_STEP = 7
const AXIS_LABEL_CLEARANCE = 3

const GUIDE_PERCENTS = [100, 75, 50, 25]

const LEGEND_ITEMS = [
  { key: "short", label: "5-hour window", swatch: "bg-context-stroke/20" },
  { key: "weekly", label: "Week", swatch: "bg-context-stroke/60" },
  { key: "rolling", label: "Average usage", swatch: "bg-gray-500", shape: "line" },
] as const

export function OverviewAllowanceChart({
  account,
  rangeStartEpoch,
  rangeEndEpoch,
  loading = false,
  controls,
}: {
  account: AllowanceUsageAccountPayload | null
  rangeStartEpoch: number
  rangeEndEpoch: number
  loading?: boolean
  controls?: ReactNode
}) {
  if (!account) {
    return (
      <section className="overview-chart" aria-label="Allowance chart" aria-busy={loading}>
        {loading ? (
          <>
            {/* The legend the plot draws above itself, held open so the rest of
                the page does not shift down when the plot replaces this. */}
            <div aria-hidden="true" className="invisible mb-(--space-sm)">
              <ChartLegend ariaLabel="Layers" items={LEGEND_ITEMS} />
            </div>
            <div aria-hidden="true" className="overview-chart-placeholder" />
          </>
        ) : (
          <p className="type-body text-label-secondary">No allowance history to chart yet.</p>
        )}
      </section>
    )
  }

  return (
    <AllowancePlot
      account={account}
      rangeStartEpoch={rangeStartEpoch}
      rangeEndEpoch={rangeEndEpoch}
      controls={controls}
    />
  )
}

// Keep loading states outside this component. The size hooks must find the element when they subscribe.
function AllowancePlot({
  account,
  rangeStartEpoch,
  rangeEndEpoch,
  controls,
}: {
  account: AllowanceUsageAccountPayload
  rangeStartEpoch: number
  rangeEndEpoch: number
  controls?: ReactNode
}) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const width = useElementWidth(containerRef)
  const height = useElementHeight(containerRef)
  const [focusedIndex, setFocusedIndex] = useState<number | null>(null)
  const rawId = useId()
  const clipId = `overview-allowance-clip-${rawId.replace(/:/g, "")}`
  const fillId = `overview-allowance-fill-${rawId.replace(/:/g, "")}`

  const slots = chartDaySlots(rangeStartEpoch, rangeEndEpoch)
  const lastIndex = CHART_DAYS - 1
  const activeIndex = focusedIndex ?? lastIndex

  const plotLeft = 0
  const plotRight = Math.max(plotLeft, width - VALUE_AXIS_WIDTH)
  const plotTop = MARGIN_TOP
  const plotBottom = Math.max(plotTop, height - TIME_AXIS_HEIGHT)
  const plotWidth = plotRight - plotLeft
  const plotHeight = plotBottom - plotTop
  const x = timeScale(rangeStartEpoch, rangeEndEpoch, plotLeft, plotWidth)
  const y = percentScale(plotTop, plotHeight)

  function focusDay(index: number): void {
    const target = Math.max(0, Math.min(lastIndex, index))
    setFocusedIndex(target)
    containerRef.current
      ?.querySelector<HTMLButtonElement>(`[data-day-index="${target}"]`)
      ?.focus()
  }

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number): void {
    const target =
      event.key === "ArrowLeft"
        ? index - 1
        : event.key === "ArrowRight"
          ? index + 1
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? lastIndex
              : null
    if (target == null) return
    event.preventDefault()
    focusDay(target)
  }

  const lastRolling = rollingAt(account.chart.rolling, rangeEndEpoch)
  const summary =
    lastRolling == null
      ? `${account.displayName}: not enough window history yet for a rolling utilization line.`
      : `${account.displayName}: rolling subscription utilization ends this range at ` +
        `${Math.round(lastRolling)} percent.`

  return (
    <section className="overview-chart overview-chart-in" aria-label="Allowance chart">
      <p className="sr-only">{summary}</p>
      <div className="mb-(--space-sm) grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-(--space-md)">
        <ChartLegend ariaLabel="Layers" items={LEGEND_ITEMS} />
        {controls}
      </div>
      <div ref={containerRef} className="relative min-h-(--overview-chart-height) flex-1">
        {width > 0 && height > 0 && (
          <>
            {/* Keep the SVG outside normal layout flow. Its measured height otherwise prevents the container from shrinking. */}
            <svg className="absolute inset-0" width={width} height={height} aria-hidden="true">
              <defs>
                <clipPath id={clipId}>
                  <rect x={plotLeft} y={plotTop} width={plotWidth} height={plotHeight} />
                </clipPath>
                {/* The session Context chart's fill: the blue is strongest under
                    the line and fades out toward the baseline. */}
                <linearGradient id={fillId} x1={0} y1={0} x2={0} y2={1}>
                  <stop offset={0} stopColor="var(--color-context-fill-top)" />
                  <stop offset={1} stopColor="var(--color-context-fill-base)" />
                </linearGradient>
              </defs>

              {GUIDE_PERCENTS.map((percent) => (
                <line
                  key={percent}
                  x1={plotLeft}
                  x2={plotRight}
                  y1={y(percent)}
                  y2={y(percent)}
                  stroke="var(--color-separator)"
                  strokeOpacity={0.6}
                />
              ))}
              {GUIDE_PERCENTS.map((percent) => (
                <text
                  key={`label-${percent}`}
                  x={plotRight + 6}
                  y={y(percent)}
                  textAnchor="start"
                  dominantBaseline="middle"
                  {...AXIS_TICK}
                >
                  {percent}%
                </text>
              ))}

              {slots.map((slot) => {
                const label = dayAxisLabel(slot)
                if (!label) return null
                return (
                  <text
                    key={slot.index}
                    x={slot.isToday ? x(slot.endEpoch) : x(slot.startEpoch)}
                    y={plotBottom + 4}
                    textAnchor={slot.isToday ? "end" : "start"}
                    dominantBaseline="hanging"
                    {...AXIS_TICK}
                  >
                    {label}
                  </text>
                )
              })}

              <g clipPath={`url(#${clipId})`}>
                {account.chart.shortWindows.map((window) => {
                  const rect = shortWindowRect(window, x, y, plotBottom)
                  return (
                    <rect
                      key={`${window.startsAtEpoch}-${window.resetsAtEpoch}`}
                      x={rect.x}
                      y={rect.y}
                      width={rect.width}
                      height={rect.height}
                      className="fill-context-stroke/[0.18]"
                    />
                  )
                })}

                {/* The gradient carries its own alpha, so the group draws at
                    full strength: a solid blue line over a fading fill. */}
                <g>
                  {account.chart.weeklyWindows.map((window) => {
                    const area = weeklyAreaPath(window, x, y)
                    if (!area) return null
                    return (
                      <g key={`${window.lane}-${window.startsAtEpoch}`}>
                        <path d={area} fill={`url(#${fillId})`} stroke="none" />
                        <path
                          d={weeklyTopLinePath(window, x, y)}
                          fill="none"
                          className="stroke-context-stroke stroke-1"
                        />
                      </g>
                    )
                  })}
                </g>

                {account.chart.rolling.length > 0 && (
                  <path
                    d={rollingLinePath(account.chart.rolling, x, y, rangeEndEpoch)}
                    fill="none"
                    className="stroke-gray-500 stroke-[3.5px]"
                    strokeLinejoin="round"
                  />
                )}
              </g>
            </svg>

            <div
              role="group"
              aria-label="Allowance for the past 30 days"
              className="absolute inset-0"
            >
              {slots.map((slot) => {
                const left = x(slot.startEpoch)
                const right = x(slot.endEpoch)
                const lines = dayTooltipLines(account, slot)
                return (
                  <Tooltip
                    key={slot.index}
                    label={<SegmentFigure>{lines.join(" · ")}</SegmentFigure>}
                    delayMs={DAY_TOOLTIP_DELAY_MS}
                  >
                    <button
                      type="button"
                      data-day-index={slot.index}
                      aria-label={lines.join(", ")}
                      tabIndex={slot.index === activeIndex ? 0 : -1}
                      className="group absolute inset-y-0 border-0 bg-transparent p-0"
                      style={{ left, width: Math.max(0, right - left) }}
                      onFocus={() => setFocusedIndex(slot.index)}
                      onKeyDown={(event) => onKeyDown(event, slot.index)}
                    >
                      <span
                        aria-hidden="true"
                        className="pointer-events-none absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-label/0 group-hover:bg-label/25 group-focus-visible:bg-label/25"
                      />
                    </button>
                  </Tooltip>
                )
              })}
            </div>
          </>
        )}
      </div>
    </section>
  )
}

interface AllowanceDaySlot {
  index: number
  startEpoch: number
  endEpoch: number
  isToday: boolean
}

function chartDaySlots(rangeStartEpoch: number, rangeEndEpoch: number): AllowanceDaySlot[] {
  return Array.from({ length: CHART_DAYS }, (_, index) => {
    const isToday = index === CHART_DAYS - 1
    const startEpoch = rangeStartEpoch + index * DAY_SECS
    const endEpoch = isToday ? rangeEndEpoch : Math.min(startEpoch + DAY_SECS, rangeEndEpoch)
    return { index, startEpoch, endEpoch, isToday }
  })
}

function timeScale(
  rangeStartEpoch: number,
  rangeEndEpoch: number,
  plotLeft: number,
  plotWidth: number,
): (epoch: number) => number {
  const span = rangeEndEpoch - rangeStartEpoch || 1
  return (epoch: number) => plotLeft + ((epoch - rangeStartEpoch) / span) * plotWidth
}

function percentScale(plotTop: number, plotHeight: number): (percent: number) => number {
  return (percent: number) => plotTop + (1 - percent / 100) * plotHeight
}

function shortWindowRect(
  window: AllowanceWindowPeakPayload,
  x: (epoch: number) => number,
  y: (percent: number) => number,
  plotBottom: number,
): { x: number; width: number; y: number; height: number } {
  const left = x(window.startsAtEpoch)
  const right = x(window.resetsAtEpoch)
  const top = y(window.peakPercent)
  return {
    x: left,
    width: Math.max(0, right - left),
    y: top,
    height: Math.max(0, plotBottom - top),
  }
}

function weeklyTopLinePath(
  window: AllowanceWindowLevelsPayload,
  x: (epoch: number) => number,
  y: (percent: number) => number,
): string {
  if (window.points.length < 2) return ""
  return window.points
    .map((point, index) => `${index === 0 ? "M" : "L"}${x(point.atEpoch)},${y(point.percent)}`)
    .join(" ")
}

function weeklyAreaPath(
  window: AllowanceWindowLevelsPayload,
  x: (epoch: number) => number,
  y: (percent: number) => number,
  baselinePercent = 0,
): string {
  const top = weeklyTopLinePath(window, x, y)
  if (!top) return ""
  const lastPoint = window.points[window.points.length - 1]!
  const firstPoint = window.points[0]!
  return (
    `${top} L${x(lastPoint.atEpoch)},${y(baselinePercent)} ` +
    `L${x(firstPoint.atEpoch)},${y(baselinePercent)} Z`
  )
}

function rollingLinePath(
  points: readonly AllowanceRollingPointPayload[],
  x: (epoch: number) => number,
  y: (percent: number) => number,
  rangeEndEpoch: number,
): string {
  const segments: string[] = []
  let previous: AllowanceRollingPointPayload | null = null
  for (const point of points) {
    if (previous?.percent != null) {
      segments.push(`L${x(point.atEpoch)},${y(previous.percent)}`)
    }
    if (point.percent != null) {
      segments.push(
        `${previous?.percent == null ? "M" : "L"}${x(point.atEpoch)},${y(point.percent)}`,
      )
    }
    previous = point
  }
  if (previous?.percent != null) {
    segments.push(`L${x(rangeEndEpoch)},${y(previous.percent)}`)
  }
  return segments.join(" ")
}

function localDateOf(epoch: number): string {
  const date = new Date(epoch * 1_000)
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, "0")
  const day = String(date.getDate()).padStart(2, "0")
  return `${year}-${month}-${day}`
}

function dayAxisLabel(slot: AllowanceDaySlot): string | null {
  if (slot.isToday) return "Today"
  if (
    slot.index % AXIS_LABEL_STEP === 0 &&
    slot.index < CHART_DAYS - 1 - AXIS_LABEL_CLEARANCE
  ) {
    return axisDayLabel(localDateOf(slot.startEpoch))
  }
  return null
}

function dayHeadingLabel(slot: AllowanceDaySlot): string {
  return slot.isToday ? "Today" : dayLabel(localDateOf(slot.startEpoch))
}

function rollingAt(
  rolling: readonly AllowanceRollingPointPayload[],
  atEpoch: number,
): number | null {
  let value: number | null = null
  for (const point of rolling) {
    if (point.atEpoch > atEpoch) break
    value = point.percent
  }
  return value
}

function dayTooltipLines(
  account: AllowanceUsageAccountPayload,
  slot: AllowanceDaySlot,
): string[] {
  const lines = [dayHeadingLabel(slot)]
  const rolling = rollingAt(account.chart.rolling, slot.endEpoch)
  lines.push(
    rolling == null
      ? "Average usage: not enough history yet"
      : `Average usage: ${Math.round(rolling)}%`,
  )
  return lines
}
