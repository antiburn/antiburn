import type { AllowanceWindowLevelsPayload } from "../../../lib/providerUsageIpc"
import type { WastePin } from "./wasteMarks"

export const DAYS_PER_WEEK = 7
// The hole in the middle holds the round Optimise button. These set its share
// of the chart and the gap between the button and the inner ring.
const HOLE_SHARE = 0.15
// The smallest hole that still fits the button label.
const MIN_HOLE = 88
const HOLE_GAP = 16

// Pins in the same few degrees stack outward, one step per session.
export const PIN_STACK_DEGREES = 3
export const PIN_STEP = 7
export const PIN_GAP = 6

export type Point = { x: number; y: number }

/** The chart geometry in pixels. The ring is a square of `side`, `top`
 *  pixels down, so the flag at the reset has room above it. */
export type Geometry = {
  cx: number
  cy: number
  inner: number
  outer: number
  hole: number
}

export function geometry(side: number, edge: number, top: number): Geometry {
  const half = side / 2
  const outer = Math.max(0, half - edge)
  const hole = Math.max(MIN_HOLE, side * HOLE_SHARE)
  return { cx: half, cy: top + half, outer, hole, inner: Math.min(outer, hole / 2 + HOLE_GAP) }
}

export function radius(g: Geometry, percent: number): number {
  return g.inner + (Math.min(100, Math.max(0, percent)) / 100) * (g.outer - g.inner)
}

/** The point for a fraction of one week. Zero is the reset, at the top. The
 *  week runs clockwise. */
export function polar(g: Geometry, fraction: number, r: number): Point {
  const angle = -Math.PI / 2 + fraction * 2 * Math.PI
  return { x: g.cx + r * Math.cos(angle), y: g.cy + r * Math.sin(angle) }
}

/** The fraction of the week at a point, from the reset, clockwise. */
export function fractionAt(g: Geometry, point: Point): number {
  return (Math.atan2(point.y - g.cy, point.x - g.cx) / (2 * Math.PI) + 1.25) % 1
}

export function weekFraction(window: AllowanceWindowLevelsPayload, epoch: number): number {
  const span = window.resetsAtEpoch - window.startsAtEpoch
  if (span <= 0) return 0
  return Math.min(1, Math.max(0, (epoch - window.startsAtEpoch) / span))
}

/** The level of a week at a fraction of it, or null outside its points. */
export function levelAt(window: AllowanceWindowLevelsPayload, fraction: number): number | null {
  let previous: { fraction: number; percent: number } | null = null
  for (const point of window.points) {
    const at = weekFraction(window, point.atEpoch)
    if (at >= fraction) {
      if (!previous) return at === fraction ? point.percent : null
      const span = at - previous.fraction
      const share = span > 0 ? (fraction - previous.fraction) / span : 1
      return previous.percent + share * (point.percent - previous.percent)
    }
    previous = { fraction: at, percent: point.percent }
  }
  return null
}

/** The distance between two fractions of a week, across the reset too. */
export function turnDistance(left: number, right: number): number {
  const distance = Math.abs(left - right) % 1
  return Math.min(distance, 1 - distance)
}

function fmt(point: Point): string {
  return `${point.x.toFixed(2)},${point.y.toFixed(2)}`
}

/** One week as a petal: the rising level out from the inner ring, then back
 *  along the inner ring to the reset. The return is two half arcs. A single
 *  arc over a full week starts and ends on the same point, and SVG drops an
 *  arc like that, which fills the gap around the button. */
export function petalPath(
  g: Geometry,
  window: AllowanceWindowLevelsPayload,
): { area: string; edge: string } | null {
  if (window.points.length < 2) return null
  const edge = window.points
    .map((point, index) => {
      const at = polar(g, weekFraction(window, point.atEpoch), radius(g, point.percent))
      return `${index === 0 ? "M" : "L"}${fmt(at)}`
    })
    .join(" ")
  const first = weekFraction(window, window.points[0]!.atEpoch)
  const last = weekFraction(window, window.points[window.points.length - 1]!.atEpoch)
  const arc = `A${g.inner},${g.inner} 0 0 0`
  const area =
    `${edge} L${fmt(polar(g, last, g.inner))} ` +
    `${arc} ${fmt(polar(g, (first + last) / 2, g.inner))} ` +
    `${arc} ${fmt(polar(g, first, g.inner))} Z`
  return { area, edge }
}

/** A pie segment from the inner ring out to `r`, clockwise from one fraction
 *  of the week to another. */
export function wedgePath(g: Geometry, from: number, to: number, r: number): string {
  return (
    `M${fmt(polar(g, from, g.inner))} L${fmt(polar(g, from, r))} ` +
    `A${r},${r} 0 0 1 ${fmt(polar(g, to, r))} ` +
    `L${fmt(polar(g, to, g.inner))} ` +
    `A${g.inner},${g.inner} 0 0 0 ${fmt(polar(g, from, g.inner))} Z`
  )
}

/** The band of one day, from the inner ring to the outer ring. */
export function dayPath(g: Geometry, day: number): string {
  return wedgePath(g, day / DAYS_PER_WEEK, (day + 1) / DAYS_PER_WEEK, g.outer)
}

/** An arc along a ring, clockwise from one fraction of the week to another. */
export function arcPath(g: Geometry, from: number, to: number, r: number): string {
  // Give a stretch that is only one reading long a visible length.
  const end = Math.max(to, from + 0.004)
  const large = end - from > 0.5 ? 1 : 0
  return `M${fmt(polar(g, from, r))} A${r},${r} 0 ${large} 1 ${fmt(polar(g, end, r))}`
}

// A reading at this level or above is a limit hit. The meters report whole
// percents, so this is 100%.
export const LIMIT_PERCENT = 99.5

/** The stretch of a week at the weekly limit. */
export type LimitStretch = {
  weekStart: number
  current: boolean
  from: number
  to: number
  hitAtEpoch: number
  untilEpoch: number
}

/** Where each week reached its limit, and how long it stayed there. A past
 *  week that ends at the limit stays there until its reset. */
export function limitStretches(
  weeks: readonly AllowanceWindowLevelsPayload[],
  current: AllowanceWindowLevelsPayload | undefined,
): LimitStretch[] {
  return weeks.flatMap((week) => {
    const hit = week.points.find((point) => point.percent >= LIMIT_PERCENT)
    if (!hit) return []
    const capped = week.points.filter(
      (point) => point.atEpoch >= hit.atEpoch && point.percent >= LIMIT_PERCENT,
    )
    const last = capped[capped.length - 1]!
    const isCurrent = week === current
    const endsCapped = week.points[week.points.length - 1] === last
    const untilEpoch = !isCurrent && endsCapped ? week.resetsAtEpoch : last.atEpoch
    return [
      {
        weekStart: week.startsAtEpoch,
        current: isCurrent,
        from: weekFraction(week, hit.atEpoch),
        to: weekFraction(week, untilEpoch),
        hitAtEpoch: hit.atEpoch,
        untilEpoch,
      },
    ]
  })
}

export type PlacedPin = {
  key: string
  pin: WastePin
  current: boolean
  weekStart: number
  bin: number
  stack: number
  fraction: number
  r: number
}

/** Put each pin at its time since the weekly reset. Pins in one bin stack
 *  outward: this week next to the ring, older sessions further out. */
export function layoutPins(
  g: Geometry,
  weeks: readonly AllowanceWindowLevelsPayload[],
  current: AllowanceWindowLevelsPayload | undefined,
  pins: readonly WastePin[],
): { placed: PlacedPin[]; binTop: Map<number, number> } {
  const bins = new Map<number, PlacedPin[]>()
  for (const pin of pins) {
    const week = weeks.find(
      (window) => window.startsAtEpoch <= pin.atEpoch && pin.atEpoch < window.resetsAtEpoch,
    )
    if (!week) continue
    const bin = Math.floor((weekFraction(week, pin.atEpoch) * 360) / PIN_STACK_DEGREES)
    const list = bins.get(bin) ?? []
    list.push({
      key: `${pin.detector}-${pin.navigationHandle}`,
      pin,
      current: week === current,
      weekStart: week.startsAtEpoch,
      bin,
      stack: 0,
      fraction: 0,
      r: 0,
    })
    bins.set(bin, list)
  }
  const placed: PlacedPin[] = []
  const binTop = new Map<number, number>()
  for (const [bin, list] of bins) {
    list.sort(
      (left, right) =>
        Number(right.current) - Number(left.current) || right.pin.atEpoch - left.pin.atEpoch,
    )
    const fraction = ((bin + 0.5) * PIN_STACK_DEGREES) / 360
    list.forEach((item, index) => {
      item.stack = index
      item.fraction = fraction
      item.r = g.outer + PIN_GAP + index * PIN_STEP
      placed.push(item)
    })
    binTop.set(bin, g.outer + PIN_GAP + list.length * PIN_STEP)
  }
  return { placed, binTop }
}
