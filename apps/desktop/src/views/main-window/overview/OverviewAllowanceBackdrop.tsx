import { Tooltip } from "../../../components/presentation/Tooltip"
import type {
  AllowanceUsageAccountPayload,
  AllowanceWindowLevelsPayload,
} from "../../../lib/providerUsageIpc"
import {
  chartDaySlots,
  dayAxisLabel,
  dayHeadingLabel,
  dayTooltipLines,
  LEGEND_ITEMS,
  rollingAt,
  type AllowanceDaySlot,
} from "./OverviewAllowanceChart"
import { UsageBanner, UsageBannerTip } from "./UsageBanner"

/** Width of the drawing in viewBox units. The height is the percent scale. */
const SPAN = 1000

const GUIDE_PERCENTS = [100, 75, 50, 25, 0]

/** The banner draws no average line, so its key leaves that entry out. */
const KEY_ITEMS = LEGEND_ITEMS.filter((item) => item.key !== "rolling")

/**
 * The allowance chart as a banner behind the Usage card: the weekly fill and
 * the 5-hour peaks, with a key, dates, and a tooltip for each day. The SVG
 * stretches to the card, so it needs no size measurement. The full chart
 * opens from the figures.
 */
export function OverviewAllowanceBackdrop({
  account,
  rangeStartEpoch,
  rangeEndEpoch,
}: {
  account: AllowanceUsageAccountPayload | null
  rangeStartEpoch: number
  rangeEndEpoch: number
}) {
  const range = rangeEndEpoch - rangeStartEpoch
  if (!account || range <= 0) return null
  const x = (epoch: number) => ((epoch - rangeStartEpoch) / range) * SPAN
  const height = (percent: number) => Math.min(100, Math.max(0, percent))
  const y = (percent: number) => 100 - height(percent)
  const slots = chartDaySlots(rangeStartEpoch, rangeEndEpoch)
  const plot = (
    <svg viewBox={`0 0 ${SPAN} 100`} preserveAspectRatio="none" className="size-full">
      {GUIDE_PERCENTS.map((percent) => (
        <line
          key={percent}
          x1={0}
          x2={SPAN}
          y1={y(percent)}
          y2={y(percent)}
          vectorEffect="non-scaling-stroke"
          className="stroke-separator"
        />
      ))}
      {account.chart.shortWindows.map((window) => (
        <rect
          key={`${window.startsAtEpoch}-${window.resetsAtEpoch}`}
          x={x(window.startsAtEpoch)}
          y={y(window.peakPercent)}
          width={Math.max(0, x(window.resetsAtEpoch) - x(window.startsAtEpoch))}
          height={height(window.peakPercent)}
          className="fill-context-stroke/10"
        />
      ))}
      {account.chart.weeklyWindows.map((window) => {
        const top = topLine(window, x, y)
        if (!top) return null
        const first = window.points[0]!
        const last = window.points.at(-1)!
        return (
          <g key={`${window.lane}-${window.startsAtEpoch}`}>
            <path
              d={`${top} L${x(last.atEpoch)},100 L${x(first.atEpoch)},100 Z`}
              className="fill-context-stroke/25"
            />
            <path
              d={top}
              fill="none"
              vectorEffect="non-scaling-stroke"
              className="stroke-context-stroke/70 stroke-[1.5px]"
            />
          </g>
        )
      })}
    </svg>
  )
  return (
    <UsageBanner
      name="allowance"
      plot={plot}
      keyItems={KEY_ITEMS}
      valueAxis={GUIDE_PERCENTS.filter((percent) => percent < 100).map((percent) => ({
        text: `${percent}%`,
        at: y(percent) / 100,
      }))}
      dates={slots
        .flatMap((slot) => {
          const text = dayAxisLabel(slot)
          if (!text) return []
          return [{ key: String(slot.index), text, at: x(slot.startEpoch) / SPAN }]
        })
        .map((date, _, all) => (date === all.at(-1) ? { ...date, at: 1 } : date))}
      hover={(className) => (
        <div role="group" aria-label="Allowance for the past 30 days" className={className}>
          {slots.map((slot) => {
            const lines = dayTooltipLines(account, slot)
            const left = (x(slot.startEpoch) / SPAN) * 100
            const right = (x(slot.endEpoch) / SPAN) * 100
            return (
              <Tooltip key={slot.index} label={dayTip(account, slot)} delayMs={0}>
                <button
                  type="button"
                  aria-label={lines.join(", ")}
                  tabIndex={-1}
                  className="group absolute inset-y-0 border-0 bg-transparent p-0"
                  style={{ left: `${left}%`, width: `${Math.max(0, right - left)}%` }}
                >
                  <span
                    aria-hidden="true"
                    className="pointer-events-none absolute inset-0 rounded-control bg-label/0 group-hover:bg-label/5"
                  />
                </button>
              </Tooltip>
            )
          })}
        </div>
      )}
    />
  )
}

/** The day's tooltip: the average, then the week's level and the busiest
 * 5-hour window on that day. */
function dayTip(account: AllowanceUsageAccountPayload, slot: AllowanceDaySlot) {
  const rolling = rollingAt(account.chart.rolling, slot.endEpoch)
  const weeklyLevels = new Map<string, { percent: number; atEpoch: number } | null>([
    ["weekly", null],
  ])
  for (const window of account.chart.weeklyWindows) {
    for (const point of window.points) {
      if (point.atEpoch > slot.endEpoch || point.atEpoch < slot.startEpoch) continue
      const previous = weeklyLevels.get(window.lane)
      if (!previous || point.atEpoch >= previous.atEpoch) {
        weeklyLevels.set(window.lane, point)
      }
    }
  }
  const peaks = account.chart.shortWindows
    .filter(
      (window) =>
        window.startsAtEpoch < slot.endEpoch && window.resetsAtEpoch > slot.startEpoch,
    )
    .map((window) => window.peakPercent)
  const busiest = peaks.length ? Math.max(...peaks) : null
  const percent = (value: number | null) =>
    value == null ? "No reading" : `${Math.round(value)}%`
  return (
    <UsageBannerTip
      title={dayHeadingLabel(slot)}
      figure={rolling == null ? "No average yet" : `${Math.round(rolling)}% average`}
      rows={[
        ...[...weeklyLevels].map(([lane, point]) => ({
          key: lane,
          label:
            lane === "weekly"
              ? "Overall week used by day end"
              : `${lane.replace(/^model:/, "")} week used by day end`,
          value: percent(point?.percent ?? null),
          swatch: LEGEND_ITEMS[1].swatch,
          ...(point ? { share: point.percent / 100 } : {}),
        })),
        ...(busiest == null
          ? []
          : [
              {
                key: "short",
                label: "Busiest 5-hour window",
                value: percent(busiest),
                swatch: LEGEND_ITEMS[0].swatch,
                share: busiest / 100,
              },
            ]),
      ]}
      footnote={peaks.length > 1 ? `${peaks.length} 5-hour windows this day` : undefined}
    />
  )
}

function topLine(
  window: AllowanceWindowLevelsPayload,
  x: (epoch: number) => number,
  y: (percent: number) => number,
): string {
  if (window.points.length < 2) return ""
  return window.points
    .map((point, index) => `${index === 0 ? "M" : "L"}${x(point.atEpoch)},${y(point.percent)}`)
    .join(" ")
}
