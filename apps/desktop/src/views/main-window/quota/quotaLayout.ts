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
  /** The time at `right`: the window's reset, or `now` for the open window. */
  endsAtEpoch: number
  /** Maps a time inside this window linearly onto `[left, right]`. */
  x: (t: number) => number
}

/**
 * One equal-width slot per window, back to back along the plot with `gapPx`
 * of space between slots, in the same order as `periods`. Each slot's own
 * `x` stretches its window's span across the slot, so a short five-hour
 * window and a long weekly window still draw at the same width. A closed
 * window spans start to reset. The open window (the one `nowEpoch` falls
 * inside) spans start to now, so its usage so far fills the whole slot
 * instead of the leading fraction of it.
 */
export function windowSlots(
  periods: readonly QuotaPeriodPayload[],
  plotLeft: number,
  plotWidth: number,
  gapPx: number = WINDOW_GAP_PX,
  nowEpoch: number = Number.POSITIVE_INFINITY,
): QuotaWindowSlot[] {
  const count = periods.length
  if (count === 0) return []
  const slotWidth = Math.max(1, (plotWidth - gapPx * (count - 1)) / count)
  return periods.map((period, index) => {
    const left = plotLeft + index * (slotWidth + gapPx)
    const right = left + slotWidth
    const open = nowEpoch >= period.startsAtEpoch && nowEpoch < period.resetsAtEpoch
    const endsAtEpoch = open ? nowEpoch : period.resetsAtEpoch
    const span = endsAtEpoch - period.startsAtEpoch || 1
    const x = (t: number) => left + ((t - period.startsAtEpoch) / span) * slotWidth
    return { period, left, right, endsAtEpoch, x }
  })
}

/** The pace line's height at a slot's right edge: 100 at a closed window's
 *  reset, the elapsed share of the window at the open window's now. */
export function slotPaceEnd(slot: QuotaWindowSlot): number {
  const { startsAtEpoch, resetsAtEpoch } = slot.period
  const span = resetsAtEpoch - startsAtEpoch
  if (span <= 0) return 100
  return Math.min(100, (100 * (slot.endsAtEpoch - startsAtEpoch)) / span)
}

/** Rows that fall inside one window: `startsAt <= t < resetsAt`. */
export function rowsInWindow(
  rows: readonly QuotaSeriesRow[],
  period: QuotaPeriodPayload,
): QuotaSeriesRow[] {
  return rows.filter((row) => row.t >= period.startsAtEpoch && row.t < period.resetsAtEpoch)
}
