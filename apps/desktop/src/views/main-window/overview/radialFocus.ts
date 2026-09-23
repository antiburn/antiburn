import type { AllowanceWindowLevelsPayload } from "../../../lib/providerUsageIpc"
import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import {
  DAYS_PER_WEEK,
  fractionAt,
  levelAt,
  radius,
  type Geometry,
  type LimitStretch,
  type PlacedPin,
  type Point,
  type Spoke,
} from "./radialGeometry"

/** The layers the key names. */
export type RadialLayer = "week" | "past" | "short" | "rolling" | "limit" | "waste"

/** The part of the week flower in focus. The pointer sets a week, a limit
 *  hit, a 5-hour window, the average ring or a plain time from where it is. A pin, a label,
 *  a day, a config row, a key entry or the button set it directly. */
export type RadialFocus =
  | { kind: "time" }
  | { kind: "week"; start: number }
  | { kind: "short"; key: string }
  | { kind: "rolling" }
  | { kind: "limit"; weekStart: number }
  | { kind: "day"; day: number }
  | { kind: "pin"; key: string; detector: BurnCheckDetectorId }
  | { kind: "check"; detector: BurnCheckDetectorId }
  | { kind: "config"; detector: BurnCheckDetectorId }
  | { kind: "layer"; layer: RadialLayer }

// The pointer snaps to a line this close, in pixels.
const SNAP = 8

/** The focus under the pointer, and the time of week it reads. Null outside
 *  the ring and the pins. The nearest line wins: a limit hit, a week's edge,
 *  the average ring, or the top of a 5-hour segment. Inside a segment with no
 *  line near, the pointer takes the segment. Otherwise it reads the time. */
export function pointerFocus(
  g: Geometry,
  pointer: Point,
  reach: number,
  weeks: readonly AllowanceWindowLevelsPayload[],
  spokes: readonly Spoke[],
  rolling: number | null,
  limits: readonly LimitStretch[],
): { focus: RadialFocus; fraction: number } | null {
  const r = Math.hypot(pointer.x - g.cx, pointer.y - g.cy)
  if (r < g.inner || r > reach) return null
  const fraction = fractionAt(g, pointer)
  let best = null as { focus: RadialFocus; distance: number } | null
  const offer = (focus: RadialFocus, distance: number) => {
    if (distance <= SNAP && (!best || distance < best.distance)) best = { focus, distance }
  }
  // A limit hit runs on the rim, over its week's edge. It comes first, so it
  // wins the tie.
  for (const limit of limits) {
    if (
      fraction >= limit.from - 0.005 &&
      fraction <= Math.max(limit.to, limit.from + 0.004) + 0.005
    )
      offer({ kind: "limit", weekStart: limit.weekStart }, Math.abs(r - g.outer))
  }
  for (const week of weeks) {
    const level = levelAt(week, fraction)
    if (level != null)
      offer({ kind: "week", start: week.startsAtEpoch }, Math.abs(r - radius(g, level)))
  }
  // Prefer a week's edge where it runs on the average ring.
  if (rolling != null) offer({ kind: "rolling" }, Math.abs(r - radius(g, rolling)) + 2)
  // Where segments of past weeks overlap, the one whose top is nearest wins.
  for (const spoke of spokes) {
    const top = radius(g, spoke.peakPercent)
    if (fraction < spoke.from || fraction > spoke.to || r > top + 4) continue
    offer({ kind: "short", key: spoke.key }, Math.min(SNAP, Math.abs(r - top)) + 1)
  }
  return { focus: best?.focus ?? { kind: "time" }, fraction }
}

export type Emphasis = "full" | "normal" | "dim"

/** How strong a pin draws for a focus. */
export function pinEmphasis(item: PlacedPin, focus: RadialFocus | null): Emphasis {
  if (!focus || focus.kind === "time") return "normal"
  switch (focus.kind) {
    case "week":
      return item.weekStart === focus.start ? "full" : "dim"
    case "limit":
      return item.weekStart === focus.weekStart ? "normal" : "dim"
    case "day":
      return Math.floor(item.fraction * DAYS_PER_WEEK) === focus.day ? "full" : "dim"
    case "pin":
      if (item.key === focus.key) return "full"
      return item.pin.detector === focus.detector ? "normal" : "dim"
    case "check":
      return item.pin.detector === focus.detector ? "full" : "dim"
    case "layer":
      return focus.layer === "waste" ? "full" : "dim"
    default:
      return "dim"
  }
}

/** True while the focus sits on something other than the usage data, so the
 *  data layer steps back. */
export function dimsData(focus: RadialFocus | null): boolean {
  if (!focus) return false
  return focus.kind !== "time" && focus.kind !== "day"
}

/** The key entry that names the focus, so the key lights up with the chart. */
export function legendLayer(
  focus: RadialFocus | null,
  currentStart: number | undefined,
): RadialLayer | null {
  switch (focus?.kind) {
    case "layer":
      return focus.layer
    case "week":
      return focus.start === currentStart ? "week" : "past"
    case "short":
      return "short"
    case "rolling":
      return "rolling"
    case "limit":
      return "limit"
    case "pin":
    case "check":
      return "waste"
    default:
      return null
  }
}

/** True when the focus leaves the config flag in view. */
export function showsFlag(focus: RadialFocus | null): boolean {
  return (
    !focus ||
    focus.kind === "time" ||
    focus.kind === "config" ||
    (focus.kind === "layer" && focus.layer === "waste")
  )
}
