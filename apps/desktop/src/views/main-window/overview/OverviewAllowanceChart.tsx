import { useState, type CSSProperties, type KeyboardEvent } from "react"

import type {
  AllowanceDayPayload,
  AllowanceUsageAccountPayload,
} from "../../../lib/providerUsageIpc"
import {
  allowancePointsLabel,
  allowanceSeriesMax,
  limitHitDayLabel,
  dayLabel,
  percentCeiling,
} from "../../../lib/presentation/overviewChart"

import { Tooltip } from "../../../components/presentation/Tooltip"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"
import {
  Bar,
  ChartAxis,
  GUIDE_FRACTIONS,
  GuideLabels,
  Guides,
  DAY_TOOLTIP_DELAY_MS,
} from "./overviewChartParts"

import "./overview.css"

/**
 * The colors one chart gives its accounts, in the order it hands them out.
 *
 * Every weight is the deep blue of a reading, so nothing on the chart comes
 * near the red that marks a block. The first two sit at opposite ends of
 * the ramp, which is the widest separation one hue can give the common case
 * of two accounts. A fifth account repeats the first weight, because a
 * repeat is honest and a fifth step does not stay separable.
 */
const SERIES_COLORS = [
  "bg-series-1 text-series-1",
  "bg-series-2 text-series-2",
  "bg-series-3 text-series-3",
  "bg-series-4 text-series-4",
]

/** The color for one account, by its place in the chart. */
function seriesColor(index: number): string {
  return SERIES_COLORS[index % SERIES_COLORS.length]!
}

/** The figures beside the guides: points of the allowance, in percent. */
function guideLabels(ceiling: number): Map<number, string> {
  return new Map(
    GUIDE_FRACTIONS.map((fraction) => [fraction, `${Math.round(ceiling * fraction)}%`]),
  )
}

/** The bar for one day: its height on the scale, and whether it has a figure. */
function barGeometry(day: AllowanceDayPayload | undefined, ceiling: number) {
  const percent = day?.usedPercent ?? null
  return {
    outline: percent == null,
    fraction: percent == null ? 0 : Math.min(1, percent / ceiling),
  }
}

/**
 * Every date any account charts, oldest first.
 *
 * Two accounts can start metering on different days. The chart draws one
 * column for each date either account knows, so a column always holds the
 * same date in every series.
 */
function chartDates(accounts: ReadonlyArray<AllowanceUsageAccountPayload>): string[] {
  const dates = new Set<string>()
  for (const account of accounts) {
    for (const day of account.days) dates.add(day.localDate)
  }
  return [...dates].sort()
}

/** One account's days, addressed by date. */
function daysByDate(account: AllowanceUsageAccountPayload): Map<string, AllowanceDayPayload> {
  return new Map(account.days.map((day) => [day.localDate, day]))
}

/** The one-line reading in a day's tooltip, across every account. */
function dayDetail(
  localDate: string,
  isToday: boolean,
  accounts: ReadonlyArray<AllowanceUsageAccountPayload>,
  series: ReadonlyArray<Map<string, AllowanceDayPayload>>,
): string {
  const parts = [isToday ? "Today" : dayLabel(localDate)]
  accounts.forEach((account, index) => {
    const day = series[index]?.get(localDate)
    const reading = allowancePointsLabel(day?.usedPercent ?? null)
    const hits = day ? limitHitDayLabel(day.blockCount) : null
    parts.push(`${account.displayName} ${reading}${hits ? `, ${hits}` : ""}`)
  })
  return parts.join(" · ")
}

/**
 * Thirty days of allowance, every provider account on one scale.
 *
 * The bars read the provider's own meter, not the local dollar estimate. A
 * session's whole usage lands on the date of its last activity, so the dollar
 * series spikes one day for work spread over several. A meter reading carries
 * the time the provider stated it, so it does not.
 *
 * Both accounts read in percent of their own plan, so one scale holds them
 * both. Color names the account, and the key is the only place that says
 * which color is which.
 *
 * A day no reading speaks for draws an outlined dot and says "no reading". A
 * gap is unknown, never zero.
 */
export function OverviewAllowanceChart({
  accounts,
  loading = false,
}: {
  accounts: ReadonlyArray<AllowanceUsageAccountPayload>
  loading?: boolean
}) {
  // The keyboard's place in the row follows the date, not the index, so a
  // refresh that adds a day keeps the reader's day. A date the series no
  // longer holds falls back to today.
  const [focusDate, setFocusDate] = useState<string | null>(null)
  const charted = accounts.filter((account) => account.days.length > 0)
  const dates = chartDates(charted)
  if (loading && dates.length === 0) {
    return (
      <section className="overview-chart" aria-label="Allowance by day" aria-busy>
        <Skeleton className="block min-h-[var(--overview-chart-height)] w-full flex-1" />
      </section>
    )
  }
  if (dates.length === 0) {
    return (
      <section className="overview-chart" aria-label="Allowance by day">
        <p className="type-body text-label-secondary">
          antiburn has no meter readings to chart yet. A reading arrives the next time an agent
          states an account&apos;s allowance.
        </p>
      </section>
    )
  }

  const series = charted.map(daysByDate)
  const lastIndex = dates.length - 1
  const foundFocus = focusDate == null ? -1 : dates.indexOf(focusDate)
  const focusIndex = foundFocus >= 0 ? foundFocus : lastIndex
  const ceiling = percentCeiling(allowanceSeriesMax(...charted.map((account) => account.days)))

  function focusDay(index: number, list: HTMLElement | null): void {
    const localDate = dates[Math.max(0, Math.min(lastIndex, index))]
    if (!localDate) return
    setFocusDate(localDate)
    list?.querySelector<HTMLButtonElement>(`[data-day="${localDate}"]`)?.focus()
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
      aria-label="Allowance by day"
      style={{ "--overview-series-count": charted.length } as CSSProperties}
    >
      <div className="overview-chart-scroll">
        <div className="overview-chart-body">
          <div className="overview-plot">
            <div className="relative h-full">
              <AccountLegend accounts={charted} />
              <Guides />
              <div
                role="group"
                aria-label="Allowance for the past 30 days"
                className="overview-days relative"
              >
                {dates.map((localDate, index) => {
                  const isToday = index === lastIndex
                  const detail = dayDetail(localDate, isToday, charted, series)
                  return (
                    <Tooltip
                      key={localDate}
                      label={<SegmentFigure>{detail}</SegmentFigure>}
                      delayMs={DAY_TOOLTIP_DELAY_MS}
                    >
                      <button
                        type="button"
                        data-day={localDate}
                        aria-label={detail}
                        tabIndex={index === focusIndex ? 0 : -1}
                        className="overview-day"
                        onFocus={() => setFocusDate(localDate)}
                        onKeyDown={(event) => onKeyDown(event, index)}
                        style={{ "--overview-bar-index": index } as CSSProperties}
                      >
                        {charted.map((account, accountIndex) => {
                          const day = series[accountIndex]?.get(localDate)
                          const geometry = barGeometry(day, ceiling)
                          return (
                            <span
                              key={`${account.provider}:${account.accountKey}`}
                              className="overview-series"
                            >
                              {(day?.blockCount ?? 0) > 0 && (
                                <span
                                  aria-hidden="true"
                                  className="overview-block-mark bg-system-red-tint"
                                />
                              )}
                              <Bar
                                fraction={geometry.fraction}
                                outline={geometry.outline}
                                className={seriesColor(accountIndex)}
                              />
                            </span>
                          )
                        })}
                      </button>
                    </Tooltip>
                  )
                })}
              </div>
            </div>
            <GuideLabels labels={guideLabels(ceiling)} />
          </div>
          <ChartAxis dates={dates} />
        </div>
      </div>
    </section>
  )
}

/** The key: which color draws which account, and what a red mark means. */
function AccountLegend({
  accounts,
}: {
  accounts: ReadonlyArray<AllowanceUsageAccountPayload>
}) {
  return (
    <p className="overview-legend type-caption flex items-center gap-[var(--space-md)] text-label-secondary">
      {accounts.map((account, index) => (
        <span
          key={`${account.provider}:${account.accountKey}`}
          className="inline-flex items-center gap-[var(--space-xs)]"
        >
          <span
            aria-hidden="true"
            className={`h-2 w-2 rounded-small ${seriesColor(index).split(" ")[0]}`}
          />
          {account.displayName}
        </span>
      ))}
      <span className="inline-flex items-center gap-[var(--space-xs)]">
        <span aria-hidden="true" className="h-2 w-2 rounded-full bg-system-red-tint" />
        Limit hit
      </span>
    </p>
  )
}
