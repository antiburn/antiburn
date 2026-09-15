import type { ProviderUsageDayPayload } from "../providerUsageIpc"
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
