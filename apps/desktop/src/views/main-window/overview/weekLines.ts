import type { AllowanceWindowLevelsPayload } from "../../../lib/providerUsageIpc"
import type { RadialFocus } from "./radialFocus"
import {
  levelAt,
  spokeLevelAt,
  weekFraction,
  type LimitStretch,
  type Point,
  type Spoke,
} from "./radialGeometry"

/** The plot box of the week lines, in pixels. Time since the weekly reset
 *  runs left to right. The level runs bottom to top. */
export type Plot = { x0: number; x1: number; y0: number; y1: number }

/** The band one week draws in: the full plot while the weeks overlap, or its
 *  own row while they are apart. */
export type Band = { top: number; height: number }

// The pointer snaps to a line this close, in pixels.
const SNAP = 8

// The number of week colours. The tone classes in overview.css match.
const WEEK_TONES = 7

/** The tone class of a week, from its place in the newest-first order. */
export function weekTone(index: number): string {
  return `week-tone-${index % WEEK_TONES}`
}

/** A short name for a week, relative to the week that is on now. */
export function weekName(
  week: AllowanceWindowLevelsPayload,
  clock: AllowanceWindowLevelsPayload,
): string {
  const span = clock.resetsAtEpoch - clock.startsAtEpoch
  const count = span > 0 ? Math.round((clock.startsAtEpoch - week.startsAtEpoch) / span) : 0
  if (count <= 0) return "This week"
  if (count === 1) return "Last week"
  return `${count} weeks ago`
}

export function plotX(plot: Plot, fraction: number): number {
  return plot.x0 + fraction * (plot.x1 - plot.x0)
}

export function bandY(band: Band, percent: number): number {
  return band.top + band.height * (1 - Math.min(100, Math.max(0, percent)) / 100)
}

export function fullBand(plot: Plot): Band {
  return { top: plot.y0, height: plot.y1 - plot.y0 }
}

/** The band of each week, in the order given. Apart, each week takes its own
 *  row with `gap` between rows. */
export function weekBands(plot: Plot, count: number, apart: boolean, gap: number): Band[] {
  const full = fullBand(plot)
  if (!apart || count <= 1) return Array.from({ length: count }, () => full)
  const height = Math.max(0, (full.height - gap * (count - 1)) / count)
  return Array.from({ length: count }, (_, index) => ({
    top: full.top + index * (height + gap),
    height,
  }))
}

/** The CSS transform that moves a shape drawn in the full plot into a band. */
export function bandTransform(plot: Plot, band: Band): string {
  const scale = band.height / Math.max(1, plot.y1 - plot.y0)
  return `translateY(${(band.top - plot.y0 * scale).toFixed(2)}px) scaleY(${scale.toFixed(4)})`
}

function fmt(point: Point): string {
  return `${point.x.toFixed(2)},${point.y.toFixed(2)}`
}

/** A rising level as a line, and as an area down to the baseline. */
function levelPath(
  plot: Plot,
  points: readonly { fraction: number; percent: number }[],
): { area: string; edge: string } | null {
  if (points.length < 2) return null
  const band = fullBand(plot)
  const at = points.map((point) => ({
    x: plotX(plot, point.fraction),
    y: bandY(band, point.percent),
  }))
  const edge = at.map((point, index) => `${index === 0 ? "M" : "L"}${fmt(point)}`).join(" ")
  const last = at[at.length - 1]!
  const area = `${edge} L${fmt({ x: last.x, y: plot.y1 })} L${fmt({ x: at[0]!.x, y: plot.y1 })} Z`
  return { area, edge }
}

export function weekPath(plot: Plot, week: AllowanceWindowLevelsPayload) {
  return levelPath(
    plot,
    week.points.map((point) => ({
      fraction: weekFraction(week, point.atEpoch),
      percent: point.percent,
    })),
  )
}

export function spokePath(plot: Plot, spoke: Spoke) {
  return levelPath(plot, spoke.points)
}

/** The focus under the pointer, and the time of week it reads. Null outside
 *  the plot. The nearest line wins: a limit hit, a week's line, the average
 *  line or a 5-hour curve. Inside a 5-hour area with no line near, the
 *  pointer takes the window. Apart, the pointer reads only the week of its
 *  row, and takes that week when no line is near. */
export function weekPointerFocus(
  plot: Plot,
  pointer: Point,
  weeks: readonly AllowanceWindowLevelsPayload[],
  bandOf: (weekStart: number) => Band,
  spokes: readonly Spoke[],
  rolling: number | null,
  limits: readonly LimitStretch[],
  apart: boolean,
): { focus: RadialFocus; fraction: number } | null {
  if (pointer.x < plot.x0 || pointer.x > plot.x1) return null
  if (pointer.y < plot.y0 - SNAP || pointer.y > plot.y1 + SNAP) return null
  const fraction = (pointer.x - plot.x0) / Math.max(1, plot.x1 - plot.x0)

  // Apart, only the week whose row is nearest the pointer counts.
  let row: AllowanceWindowLevelsPayload | undefined
  if (apart) {
    let nearest = Infinity
    for (const week of weeks) {
      const band = bandOf(week.startsAtEpoch)
      const distance = Math.max(0, band.top - pointer.y, pointer.y - band.top - band.height)
      if (distance < nearest) {
        nearest = distance
        row = week
      }
    }
  }
  const inRow = (weekStart: number) => !row || row.startsAtEpoch === weekStart

  let best = null as { focus: RadialFocus; distance: number } | null
  const offer = (focus: RadialFocus, distance: number) => {
    if (distance <= SNAP && (!best || distance < best.distance)) best = { focus, distance }
  }
  // A limit hit runs on the 100% line, over its week's line. It comes first,
  // so it wins the tie.
  for (const limit of limits) {
    if (!inRow(limit.weekStart)) continue
    const end = Math.max(limit.to, limit.from + 0.004)
    if (fraction >= limit.from - 0.005 && fraction <= end + 0.005)
      offer(
        { kind: "limit", weekStart: limit.weekStart },
        Math.abs(pointer.y - bandY(bandOf(limit.weekStart), 100)),
      )
  }
  for (const week of weeks) {
    if (!inRow(week.startsAtEpoch)) continue
    const level = levelAt(week, fraction)
    if (level != null)
      offer(
        { kind: "week", start: week.startsAtEpoch },
        Math.abs(pointer.y - bandY(bandOf(week.startsAtEpoch), level)),
      )
  }
  // Prefer a week's line where it runs on the average line.
  if (!apart && rolling != null)
    offer({ kind: "rolling" }, Math.abs(pointer.y - bandY(fullBand(plot), rolling)) + 2)
  for (const spoke of spokes) {
    if (!inRow(spoke.weekStart)) continue
    const level = spokeLevelAt(spoke, fraction)
    if (level == null) continue
    const band = bandOf(spoke.weekStart)
    const top = bandY(band, level)
    if (pointer.y < top - SNAP || pointer.y > band.top + band.height) continue
    offer({ kind: "short", key: spoke.key }, Math.min(SNAP, Math.abs(pointer.y - top)) + 1)
  }
  const fallback: RadialFocus = row
    ? { kind: "week", start: row.startsAtEpoch }
    : { kind: "time" }
  return { focus: best?.focus ?? fallback, fraction }
}

/** Spread label centres so no two sit closer than `gap`, inside `[top,
 *  bottom]`, and keep their order. */
export function spreadLabels(
  wanted: readonly number[],
  gap: number,
  top: number,
  bottom: number,
): number[] {
  const order = wanted.map((y, index) => ({ y, index })).sort((left, right) => left.y - right.y)
  let floor = top
  for (const item of order) {
    item.y = Math.max(item.y, floor)
    floor = item.y + gap
  }
  let ceiling = bottom
  for (let index = order.length - 1; index >= 0; index--) {
    const item = order[index]!
    item.y = Math.min(item.y, ceiling)
    ceiling = item.y - gap
  }
  const out = new Array<number>(wanted.length).fill(0)
  for (const item of order) out[item.index] = item.y
  return out
}
