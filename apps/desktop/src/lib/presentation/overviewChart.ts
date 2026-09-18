import type { AllowanceDayPayload, ProviderUsageDayPayload } from "../providerUsageIpc"
import { formatSpendFigure } from "./providerUsage"

/** Parse a `YYYY-MM-DD` reader-local date as a local `Date` at midnight. */
function localDateOf(localDate: string): Date {
  const [year, month, day] = localDate.split("-").map(Number)
  return new Date(year ?? 1970, (month ?? 1) - 1, day ?? 1)
}

/** "Mon 14 Sep" for a reader-local date. */
export function dayLabel(localDate: string): string {
  const date = localDateOf(localDate)
  const weekday = date.toLocaleDateString("en-US", { weekday: "short" })
  const month = date.toLocaleDateString("en-US", { month: "short" })
  return `${weekday} ${date.getDate()} ${month}`
}

/** "14 Sep" for an axis label. */
export function axisDayLabel(localDate: string): string {
  const date = localDateOf(localDate)
  return `${date.getDate()} ${date.toLocaleDateString("en-US", { month: "short" })}`
}

/**
 * A round ceiling for a chart scale: 1, 2, or 5 times a power of ten, the
 * smallest one at or above `max`. A zero or invalid max gives a $1 scale so
 * an empty chart still draws its guides.
 */
export function niceCeiling(max: number): number {
  if (!Number.isFinite(max) || max <= 0) return 1
  const power = 10 ** Math.floor(Math.log10(max))
  for (const step of [1, 2, 5, 10]) {
    if (step * power >= max) return step * power
  }
  return 10 * power
}

/** The tallest priced day across both series. */
export function seriesMax(
  ...series: ReadonlyArray<ReadonlyArray<ProviderUsageDayPayload>>
): number {
  let max = 0
  for (const days of series) {
    for (const day of days) {
      if (day.estimatedUsd != null && day.estimatedUsd > max) max = day.estimatedUsd
    }
  }
  return max
}

/**
 * The change from the comparison day to the day, as "+$1.20" or "−$0.40".
 * Null when either day has no priced figure, so the caption never compares
 * a figure with a gap.
 */
export function spendDeltaLabel(
  day: ProviderUsageDayPayload,
  previous: ProviderUsageDayPayload | undefined,
): string | null {
  if (day.estimatedUsd == null || previous?.estimatedUsd == null) return null
  const delta = day.estimatedUsd - previous.estimatedUsd
  if (Math.abs(delta) < 0.005) return "no change"
  return `${delta < 0 ? "−" : "+"}${formatSpendFigure(Math.abs(delta))}`
}

/** The percent ceilings an allowance chart may use, smallest first. */
const PERCENT_CEILINGS = [5, 10, 25, 50, 100]

/**
 * A round percent ceiling for an allowance scale.
 *
 * The scale stops at 100%, because one period holds one whole allowance and
 * a day cannot consume more of it than exists.
 */
export function percentCeiling(max: number): number {
  if (!Number.isFinite(max) || max <= 0) return PERCENT_CEILINGS[0] as number
  for (const step of PERCENT_CEILINGS) {
    if (step >= max) return step
  }
  return 100
}

/** The largest known daily figure across the series given. */
export function allowanceSeriesMax(
  ...series: ReadonlyArray<ReadonlyArray<AllowanceDayPayload>>
): number {
  let max = 0
  for (const days of series) {
    for (const day of days) {
      if (day.usedPercent != null && day.usedPercent > max) max = day.usedPercent
    }
  }
  return max
}

/** A percentage-point figure, with one decimal only while it is small. */
function pointsFigure(points: number): string {
  return points < 10 ? `${points.toFixed(1)}` : `${Math.round(points)}`
}

/** Points of the allowance one day consumed, or the word for no reading. */
export function allowancePointsLabel(usedPercent: number | null): string {
  if (usedPercent == null) return "no reading"
  return `${pointsFigure(usedPercent)} points`
}

/**
 * How many limit hits a day carried, in the reader's words.
 *
 * A limit hit is a request the provider refused. The word names what the
 * reader met, where "block" named what the provider did.
 */
export function limitHitDayLabel(limitHitCount: number): string | null {
  if (limitHitCount <= 0) return null
  return limitHitCount === 1 ? "1 limit hit" : `${limitHitCount} limit hits`
}
