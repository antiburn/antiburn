/**
 * Pure layout geometry for the burnup chart's window-relative x axis: the
 * equal-width slot each window draws in, and the rows that fall inside one
 * window. No React and no chart-wide scale: each slot carries its own `x`,
 * so a caller draws a window's bands and lines without touching any other
 * window's geometry.
 */

import type { QuotaPeriodPayload } from "../../../lib/providerUsageIpc"
import type { QuotaSeriesRow } from "./quotaSeries"

/** The gap between two adjacent window slots, in pixels. */
export const WINDOW_GAP_PX = 6

export interface QuotaWindowSlot {
  period: QuotaPeriodPayload
  left: number
  right: number
  /** Maps a time inside this window linearly onto `[left, right]`. */
  x: (t: number) => number
}

/**
 * One equal-width slot per window, back to back along the plot with `gapPx`
 * of space between slots, in the same order as `periods`. Each slot's own
 * `x` stretches its window's own start-to-reset span across the slot, so a
 * short five-hour window and a long weekly window still draw at the same
 * width.
 */
export function windowSlots(
  periods: readonly QuotaPeriodPayload[],
  plotLeft: number,
  plotWidth: number,
  gapPx: number = WINDOW_GAP_PX,
): QuotaWindowSlot[] {
  const count = periods.length
  if (count === 0) return []
  const slotWidth = Math.max(1, (plotWidth - gapPx * (count - 1)) / count)
  return periods.map((period, index) => {
    const left = plotLeft + index * (slotWidth + gapPx)
    const right = left + slotWidth
    const span = period.resetsAtEpoch - period.startsAtEpoch || 1
    const x = (t: number) => left + ((t - period.startsAtEpoch) / span) * slotWidth
    return { period, left, right, x }
  })
}

/** Rows that fall inside one window: `startsAt <= t < resetsAt`. */
export function rowsInWindow(
  rows: readonly QuotaSeriesRow[],
  period: QuotaPeriodPayload,
): QuotaSeriesRow[] {
  return rows.filter((row) => row.t >= period.startsAtEpoch && row.t < period.resetsAtEpoch)
}
