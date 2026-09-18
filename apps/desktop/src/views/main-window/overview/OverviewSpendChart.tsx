import { useState, type CSSProperties, type KeyboardEvent } from "react"

import type { ProviderUsageDayPayload } from "../../../lib/providerUsageIpc"
import {
  dayLabel,
  niceCeiling,
  seriesMax,
  spendDeltaLabel,
} from "../../../lib/presentation/overviewChart"
import {
  formatSpendFigure,
  formatTokenFigure,
  sessionCountLabel,
  windowTokens,
} from "../../../lib/presentation/providerUsage"

import { Tooltip } from "../../../components/presentation/Tooltip"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"
import {
  Bar,
  ChartAxis,
  ChartLegend,
  GUIDE_FRACTIONS,
  GuideLabels,
  Guides,
  DAY_TOOLTIP_DELAY_MS,
} from "./overviewChartParts"

import "./overview.css"

/**
 * The figures beside the guides, one precision for the whole scale: whole
 * dollars when every guide lands on one, else dollars and cents.
 */
function guideLabels(ceiling: number): Map<number, string> {
  const values = GUIDE_FRACTIONS.map((fraction) => [fraction, ceiling * fraction] as const)
  const whole = values.every(([, usd]) => Number.isInteger(usd))
  return new Map(
    values.map(([fraction, usd]) => [
      fraction,
      whole ? `$${usd.toLocaleString("en-US")}` : formatSpendFigure(usd),
    ]),
  )
}

/** The bar for one day: its height on the scale, and whether it has a figure. */
function barGeometry(day: ProviderUsageDayPayload | undefined, ceiling: number) {
  const usd = day?.estimatedUsd ?? null
  const tokens = day ? windowTokens(day) : 0
  return {
    outline: usd == null && tokens > 0,
    fraction: usd == null ? 0 : Math.min(1, usd / ceiling),
  }
}

/** The one-line reading in a day's tooltip. */
function dayDetail(
  day: ProviderUsageDayPayload,
  previous: ProviderUsageDayPayload | undefined,
  isToday: boolean,
): string {
  const tokens = windowTokens(day)
  const figure =
    day.estimatedUsd != null
      ? formatSpendFigure(day.estimatedUsd)
      : tokens > 0
        ? "not priced"
        : "no sessions"
  const parts = [isToday ? "Today" : dayLabel(day.localDate), figure]
  if (tokens > 0) parts.push(formatTokenFigure(tokens), sessionCountLabel(day.sessionCount))
  const delta = spendDeltaLabel(day, previous)
  if (delta) parts.push(`${delta} vs 30 days before`)
  return parts.join(" · ")
}

/**
 * Thirty days of estimated local spend as paired pill bars: this period in
 * front, the thirty days before it behind in a quiet neutral. A day under
 * the pointer, or the day with keyboard focus, writes its reading on the
 * line under the chart. Each day is a button, and the arrow keys walk them.
 *
 * A day with tokens but no price draws an outlined dot and says "not priced",
 * so it is never mistaken for a day at zero.
 */
export function OverviewSpendChart({
  days,
  previousDays,
  loading = false,
}: {
  days: ReadonlyArray<ProviderUsageDayPayload>
  previousDays: ReadonlyArray<ProviderUsageDayPayload>
  loading?: boolean
}) {
  // The keyboard's place in the row follows the date, not the index, so a
  // refresh that adds a day keeps the reader's day. A date the series no
  // longer holds falls back to today.
  const [focusDate, setFocusDate] = useState<string | null>(null)
  const lastIndex = days.length - 1
  const foundFocus =
    focusDate == null ? -1 : days.findIndex((day) => day.localDate === focusDate)
  const focusIndex = foundFocus >= 0 ? foundFocus : lastIndex
  const ceiling = niceCeiling(seriesMax(days, previousDays))

  function focusDay(index: number, list: HTMLElement | null): void {
    const clamped = Math.max(0, Math.min(lastIndex, index))
    const day = days[clamped]
    if (!day) return
    setFocusDate(day.localDate)
    const button = list?.querySelector<HTMLButtonElement>(`[data-day="${day.localDate}"]`)
    button?.focus()
  }

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number): void {
    const list = event.currentTarget.parentElement
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
    focusDay(target, list)
  }

  return (
    <section
      className="overview-chart"
      aria-label="Estimated spend by day"
      aria-busy={loading || undefined}
    >
      {loading || days.length === 0 ? (
        <Skeleton className="block min-h-[var(--overview-chart-height)] w-full flex-1" />
      ) : (
        <div className="overview-chart-scroll">
          <div className="overview-chart-body">
            <div className="overview-plot">
              <div className="relative h-full">
                <ChartLegend nowClassName="bg-token-in" />
                <Guides />
                <div
                  role="group"
                  aria-label="Estimated spend for the past 30 days"
                  className="overview-days relative"
                >
                  {days.map((day, index) => {
                    const previous = previousDays[index]
                    const now = barGeometry(day, ceiling)
                    const before = barGeometry(previous, ceiling)
                    const isToday = index === lastIndex
                    const detail = dayDetail(day, previous, isToday)
                    return (
                      <Tooltip
                        key={day.localDate}
                        label={<SegmentFigure>{detail}</SegmentFigure>}
                        delayMs={DAY_TOOLTIP_DELAY_MS}
                      >
                        <button
                          type="button"
                          data-day={day.localDate}
                          aria-label={detail}
                          tabIndex={index === focusIndex ? 0 : -1}
                          className="overview-day"
                          onFocus={() => setFocusDate(day.localDate)}
                          onKeyDown={(event) => onKeyDown(event, index)}
                          style={{ "--overview-bar-index": index } as CSSProperties}
                        >
                          <Bar
                            fraction={before.fraction}
                            outline={before.outline}
                            className="bg-label-tertiary/30 text-label-tertiary/30"
                          />
                          <Bar
                            fraction={now.fraction}
                            outline={now.outline}
                            className={
                              isToday
                                ? "bg-token-in text-token-in"
                                : "bg-token-in text-token-in opacity-70"
                            }
                          />
                        </button>
                      </Tooltip>
                    )
                  })}
                </div>
              </div>
              <GuideLabels labels={guideLabels(ceiling)} />
            </div>
            <ChartAxis dates={days.map((day) => day.localDate)} />
          </div>
        </div>
      )}
    </section>
  )
}
