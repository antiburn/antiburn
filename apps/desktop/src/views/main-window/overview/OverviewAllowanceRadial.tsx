import { useRef, useState, type CSSProperties, type PointerEvent, type ReactNode } from "react"

import type {
  AllowanceUsageAccountPayload,
  AllowanceWindowLevelsPayload,
} from "../../../lib/providerUsageIpc"
import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import { cn } from "../../../lib/cn"
import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"
import { ChartLegend, type ChartLegendItem } from "../../../components/ui/ChartLegend"
import { useElementHeight, useElementWidth } from "../../../lib/useElementWidth"
import { useEntranceProps } from "./overviewEntrance"
import type { ConfigShare, WasteMarks, WastePin } from "./wasteMarks"

const DAYS_PER_WEEK = 7
// The outer ring stays this far inside the square, so the labels fit.
const EDGE_GAP = 16
// With waste marks, the ring leaves this much room for the pin stacks.
const PIN_ROOM = 64
// The hole in the middle holds the round Optimise button. These set its share
// of the chart and the gap between the button and the inner ring.
const HOLE_SHARE = 0.15
// The smallest hole that still fits the button label.
const MIN_HOLE = 88
const HOLE_GAP = 16

// Pins in the same few degrees stack outward, one step per session.
const PIN_STACK_DEGREES = 3
const PIN_STEP = 7
const PIN_GAP = 6
const PAST_PIN_OPACITY = 0.35
// Pins of other checks fade to this share while one check is in focus.
const DIM_SHARE = 0.25
// Pins inside one span share a label.
const CLUSTER_DEGREES = 20
// The flag at the reset: a header line, then one row per config check.
const FLAG_HEAD = 18
const FLAG_ROW = 17
const FLAG_SHARE_WIDTH = 30
// An estimate of caption text width, so labels can keep clear of each other.
const CHAR_WIDTH = 6.2
// The hover card lists this many nearby pins.
const NEARBY_LIMIT = 3

const LEGEND_ITEMS: readonly ChartLegendItem[] = [
  { key: "week", label: "This week", swatch: "bg-context-stroke" },
  { key: "past", label: "Past weeks", swatch: "bg-context-stroke/30" },
  { key: "short", label: "5-hour window", swatch: "bg-context-stroke/20", shape: "line" },
  { key: "rolling", label: "Average usage", swatch: "bg-gray-500", shape: "line" },
]
const WASTE_LEGEND: ChartLegendItem = {
  key: "waste",
  label: "Wasteful session",
  swatch: "bg-brand",
  shape: "line",
}

type Point = { x: number; y: number }
type Box = { x0: number; x1: number; y0: number; y1: number }

/** The chart geometry in pixels. The ring is a square of `side`, `top`
 *  pixels down, so the flag at the reset has room above it. */
type Geometry = {
  cx: number
  cy: number
  inner: number
  outer: number
  hole: number
}

function geometry(side: number, edge: number, top: number): Geometry {
  const half = side / 2
  const outer = Math.max(0, half - edge)
  const hole = Math.max(MIN_HOLE, side * HOLE_SHARE)
  return { cx: half, cy: top + half, outer, hole, inner: Math.min(outer, hole / 2 + HOLE_GAP) }
}

function radius(g: Geometry, percent: number): number {
  return g.inner + (Math.min(100, Math.max(0, percent)) / 100) * (g.outer - g.inner)
}

/** The point for a fraction of one week. Zero is the reset, at the top. The
 *  week runs clockwise. */
function polar(g: Geometry, fraction: number, r: number): Point {
  const angle = -Math.PI / 2 + fraction * 2 * Math.PI
  return { x: g.cx + r * Math.cos(angle), y: g.cy + r * Math.sin(angle) }
}

function weekFraction(window: AllowanceWindowLevelsPayload, epoch: number): number {
  const span = window.resetsAtEpoch - window.startsAtEpoch
  if (span <= 0) return 0
  return Math.min(1, Math.max(0, (epoch - window.startsAtEpoch) / span))
}

/** The level of a week at a fraction of it, or null outside its points. */
function levelAt(window: AllowanceWindowLevelsPayload, fraction: number): number | null {
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
function turnDistance(left: number, right: number): number {
  const distance = Math.abs(left - right) % 1
  return Math.min(distance, 1 - distance)
}

function fmt(point: Point): string {
  return `${point.x.toFixed(2)},${point.y.toFixed(2)}`
}

function formatWhen(epoch: number): string {
  return new Date(epoch * 1000).toLocaleString(undefined, {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit",
  })
}

function weeksAgo(count: number): string {
  if (count <= 0) return "This week"
  if (count === 1) return "Last week"
  return `${count} weeks ago`
}

/** One week as a petal: the rising level out from the inner ring, then back
 *  along the inner ring to the reset. The return is two half arcs. A single
 *  arc over a full week starts and ends on the same point, and SVG drops an
 *  arc like that, which fills the gap around the button. */
function petalPath(
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

type PlacedPin = {
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
function layoutPins(
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

function flagHeight(config: readonly ConfigShare[]): number {
  return config.length ? FLAG_HEAD + config.length * FLAG_ROW : 0
}

/** The allowance chart drawn around one week, so every week overlaps the
 *  others. The angle is the time since the weekly reset. The radius is the
 *  level. The chart fills the space it gets, as a square. Wasteful sessions
 *  stick out past the rim as pins; config checks sit once at the reset. The
 *  chart draws itself in clockwise from the reset, and the pointer reads any
 *  time of the week. */
export function OverviewAllowanceRadial({
  account,
  rangeEndEpoch,
  controls,
  center,
  waste,
}: {
  account: AllowanceUsageAccountPayload
  rangeEndEpoch: number
  controls?: ReactNode
  /** The content in the hole of the chart. */
  center?: ReactNode
  waste?: WasteMarks
}) {
  const frameRef = useRef<HTMLDivElement | null>(null)
  const width = useElementWidth(frameRef)
  const height = useElementHeight(frameRef)
  const [pointer, setPointer] = useState<Point | null>(null)
  const [pinKey, setPinKey] = useState<string | null>(null)
  const [labelDetector, setLabelDetector] = useState<BurnCheckDetectorId | null>(null)
  const pins = waste?.pins ?? []
  const config = waste?.config ?? []
  const marked = pins.length > 0 || config.length > 0
  const edge = marked ? PIN_ROOM : EDGE_GAP
  // The flag rises above the ring. Keep room for it above the square.
  const flagRoom = Math.max(0, flagHeight(config) + 16 + PIN_GAP - edge)
  const side = Math.max(0, Math.min(width, height - flagRoom))
  const g = geometry(side, edge, flagRoom)
  const entrance = useEntranceProps("allowance-radial", "overview-radial-in", side > 0)

  const weeks = account.chart.weeklyWindows.filter((window) => window.lane === "weekly")
  const current = weeks.find(
    (window) => window.startsAtEpoch <= rangeEndEpoch && rangeEndEpoch < window.resetsAtEpoch,
  )
  // The week that names the days and times on the chart.
  const clock = current ?? weeks[weeks.length - 1]
  const clockSpan = clock ? clock.resetsAtEpoch - clock.startsAtEpoch : 0
  const past = weeks.filter((window) => window !== current)
  const latestRolling = [...account.chart.rolling]
    .reverse()
    .find((point) => point.atEpoch <= rangeEndEpoch && point.percent != null)?.percent

  const spokes = account.chart.shortWindows.flatMap((short) => {
    const mid = (short.startsAtEpoch + short.resetsAtEpoch) / 2
    const week = weeks.find(
      (window) => window.startsAtEpoch <= mid && mid < window.resetsAtEpoch,
    )
    if (!week) return []
    const fraction = weekFraction(week, mid)
    return [
      {
        key: `${short.startsAtEpoch}-${short.resetsAtEpoch}`,
        from: polar(g, fraction, g.inner),
        to: polar(g, fraction, radius(g, short.peakPercent)),
      },
    ]
  })

  const currentPetal = current ? petalPath(g, current) : null
  const currentLast = current?.points[current.points.length - 1]
  const currentTip =
    current && currentLast
      ? polar(g, weekFraction(current, currentLast.atEpoch), radius(g, currentLast.percent))
      : null

  const { placed, binTop } = layoutPins(g, weeks, current, pins)
  const boxes: Box[] = []
  const overlaps = (box: Box) =>
    boxes.some(
      (other) =>
        box.x0 < other.x1 && box.x1 > other.x0 && box.y0 < other.y1 && box.y1 > other.y0,
    )

  // The flag hangs to the right of its pole. Only the pin stacks under it push it up.
  let flag: { top: number; bottom: number; x: number } | null = null
  if (config.length) {
    const flagWidth =
      FLAG_SHARE_WIDTH + 8 + Math.max(...config.map((item) => item.label.length)) * CHAR_WIDTH
    let bottom = g.cy - g.outer - 16
    for (const [bin, top] of binTop) {
      const angle = (((bin + 0.5) * PIN_STACK_DEGREES) / 360) * 2 * Math.PI
      const near = g.outer * Math.sin(angle)
      const far = top * Math.sin(angle)
      if (Math.max(near, far) < -4 || Math.min(near, far) > flagWidth + 12) continue
      bottom = Math.min(bottom, g.cy - top * Math.cos(angle) - 8)
    }
    const top = bottom - flagHeight(config)
    flag = { top, bottom, x: g.cx + 8 }
    boxes.push({ x0: g.cx - 4, x1: g.cx + 8 + flagWidth + 4, y0: top - 12, y1: bottom })
  }

  // Name the check at each group of its pins, biggest groups first. A label
  // that cannot keep clear of the flag and the other labels is left out; the
  // hover card still names its pins.
  const clusters = new Map<string, PlacedPin[]>()
  for (const item of placed) {
    const key = `${item.pin.detector}|${Math.floor((item.fraction * 360) / CLUSTER_DEGREES)}`
    clusters.set(key, [...(clusters.get(key) ?? []), item])
  }
  const labels = [...clusters.entries()]
    .sort((left, right) => right[1].length - left[1].length)
    .flatMap(([key, group]) => {
      const fraction = group.reduce((sum, item) => sum + item.fraction, 0) / group.length
      const text = group[0]!.pin.label
      const labelWidth = (text.length + (group.length > 1 ? 4 : 0)) * CHAR_WIDTH
      let r = Math.max(...group.map((item) => binTop.get(item.bin) ?? g.outer)) + 8
      for (let step = 0; step < 16; step++, r += 12) {
        const at = polar(g, fraction, r)
        const anchor = at.x > g.cx + 8 ? "start" : at.x < g.cx - 8 ? "end" : "middle"
        const left =
          anchor === "start"
            ? at.x
            : anchor === "end"
              ? at.x - labelWidth
              : at.x - labelWidth / 2
        const box = { x0: left - 3, x1: left + labelWidth + 3, y0: at.y - 8, y1: at.y + 8 }
        if (overlaps(box)) continue
        boxes.push(box)
        const detector = group[0]!.pin.detector
        return [{ key, detector, at, anchor, text, count: group.length, fraction }]
      }
      return []
    })

  // The pointer reads the time of the week under it, inside the ring and
  // among the pins.
  const hoveredPin = placed.find((item) => item.key === pinKey) ?? null
  const focusDetector = hoveredPin?.pin.detector ?? labelDetector
  const reach = Math.max(g.outer, ...binTop.values()) + PIN_STEP
  const pointerRadius = pointer ? Math.hypot(pointer.x - g.cx, pointer.y - g.cy) : 0
  const hoverFraction =
    pointer && !hoveredPin && pointerRadius >= g.inner && pointerRadius <= reach
      ? (Math.atan2(pointer.y - g.cy, pointer.x - g.cx) / (2 * Math.PI) + 1.25) % 1
      : null
  const readout =
    hoverFraction != null && clock
      ? (() => {
          const epoch = clock.startsAtEpoch + hoverFraction * clockSpan
          const thisWeek = current ? levelAt(current, hoverFraction) : null
          const pastLevels = past
            .map((window) => levelAt(window, hoverFraction))
            .filter((level): level is number => level != null)
          const short = current
            ? account.chart.shortWindows.find(
                (window) => window.startsAtEpoch <= epoch && epoch < window.resetsAtEpoch,
              )
            : undefined
          const nearby = placed.filter(
            (item) =>
              turnDistance(item.fraction, hoverFraction) <= (PIN_STACK_DEGREES * 1.5) / 360,
          )
          return {
            when: formatWhen(epoch),
            thisWeek,
            pastAverage: pastLevels.length
              ? pastLevels.reduce((sum, level) => sum + level, 0) / pastLevels.length
              : null,
            shortPeak: short?.peakPercent ?? null,
            nearby,
          }
        })()
      : null

  const summary =
    `${account.displayName}: ${weeks.length} weekly windows drawn on one week.` +
    (latestRolling != null ? ` Average usage is ${Math.round(latestRolling)} percent.` : "") +
    (placed.length ? ` ${placed.length} wasteful sessions are pinned by time of week.` : "") +
    config
      .map(
        (item) => ` ${item.label} fails ${Math.round(item.share * 100)} percent of sessions.`,
      )
      .join("")

  function trackPointer(event: PointerEvent<SVGSVGElement>) {
    const rect = event.currentTarget.getBoundingClientRect()
    setPointer({ x: event.clientX - rect.left, y: event.clientY - rect.top })
  }

  const layerSize = { inlineSize: side, blockSize: side + flagRoom }
  const tooltipStyle: CSSProperties | undefined = pointer
    ? {
        left: pointer.x > g.cx ? pointer.x - 14 : pointer.x + 14,
        top: pointer.y > g.cy ? pointer.y - 14 : pointer.y + 14,
        translate: `${pointer.x > g.cx ? "-100%" : "0"} ${pointer.y > g.cy ? "-100%" : "0"}`,
      }
    : undefined

  return (
    <section className="overview-chart" aria-label="Allowance chart">
      <p className="sr-only">{summary}</p>
      <div className="overview-chart-legend mb-(--space-sm) flex items-center justify-end">
        {controls}
      </div>
      <div ref={frameRef} className="relative min-h-0 flex-1">
        {side > 0 && (
          <div
            className={cn(
              "overview-radial absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2",
              entrance.className,
            )}
            onAnimationEnd={entrance.onAnimationEnd}
            style={{ ...layerSize, "--overview-radial-cy": `${g.cy}px` } as CSSProperties}
          >
            {/* The grid and the axis stay still while the data sweeps in. */}
            <svg
              width={side}
              height={side + flagRoom}
              className="absolute inset-0 overflow-visible"
              aria-hidden="true"
            >
              {[50, 100].map((percent) => (
                <circle
                  key={percent}
                  cx={g.cx}
                  cy={g.cy}
                  r={radius(g, percent)}
                  fill="none"
                  stroke="var(--color-separator)"
                  strokeWidth={1}
                />
              ))}
              {Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
                const from = polar(g, day / DAYS_PER_WEEK, g.inner)
                const to = polar(g, day / DAYS_PER_WEEK, g.outer)
                return (
                  <line
                    key={day}
                    x1={from.x}
                    y1={from.y}
                    x2={to.x}
                    y2={to.y}
                    stroke="var(--color-separator)"
                    strokeWidth={1}
                  />
                )
              })}
              {clock &&
                Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
                  const at = polar(g, (day + 0.5) / DAYS_PER_WEEK, g.outer - 12)
                  const epoch = clock.startsAtEpoch + ((day + 0.5) / DAYS_PER_WEEK) * clockSpan
                  return (
                    <text
                      key={day}
                      x={at.x}
                      y={at.y}
                      textAnchor="middle"
                      dominantBaseline="middle"
                      {...AXIS_TICK}
                    >
                      {new Date(epoch * 1000).toLocaleDateString(undefined, {
                        weekday: "short",
                      })}
                    </text>
                  )
                })}
              <text
                x={g.cx + 5}
                y={g.cy - radius(g, 100) + 12}
                textAnchor="start"
                {...AXIS_TICK}
              >
                100% · reset
              </text>
              <text
                x={g.cx + 5}
                y={g.cy - radius(g, 50) + 12}
                textAnchor="start"
                {...AXIS_TICK}
              >
                50%
              </text>
            </svg>

            <svg
              width={side}
              height={side + flagRoom}
              className="overview-radial-data absolute inset-0 overflow-visible"
              aria-hidden="true"
            >
              {spokes.map((spoke) => (
                <line
                  key={spoke.key}
                  x1={spoke.from.x}
                  y1={spoke.from.y}
                  x2={spoke.to.x}
                  y2={spoke.to.y}
                  className="stroke-context-stroke/20"
                  strokeWidth={2.5}
                  strokeLinecap="round"
                />
              ))}

              {past.map((window) => {
                const petal = petalPath(g, window)
                if (!petal) return null
                return (
                  <g key={window.startsAtEpoch}>
                    <path d={petal.area} className="fill-context-stroke/[0.07]" />
                    <path
                      d={petal.edge}
                      fill="none"
                      className="stroke-context-stroke/35"
                      strokeWidth={1}
                    />
                  </g>
                )
              })}

              {latestRolling != null && (
                <circle
                  cx={g.cx}
                  cy={g.cy}
                  r={radius(g, latestRolling)}
                  fill="none"
                  className="stroke-gray-500"
                  strokeWidth={1}
                />
              )}

              {currentPetal && (
                <g>
                  <path d={currentPetal.area} className="fill-context-stroke/25" />
                  <path
                    d={currentPetal.edge}
                    fill="none"
                    className="stroke-context-stroke"
                    strokeWidth={2}
                    strokeLinejoin="round"
                  />
                </g>
              )}
              {currentTip && (
                <circle
                  cx={currentTip.x}
                  cy={currentTip.y}
                  r={3.5}
                  className="fill-context-stroke"
                />
              )}
            </svg>

            <svg
              width={side}
              height={side + flagRoom}
              className="absolute inset-0 overflow-visible"
              aria-hidden="true"
              onPointerMove={trackPointer}
              onPointerLeave={() => {
                setPointer(null)
                setPinKey(null)
                setLabelDetector(null)
              }}
            >
              {hoverFraction != null && (
                <g className="pointer-events-none">
                  <line
                    x1={polar(g, hoverFraction, g.inner).x}
                    y1={polar(g, hoverFraction, g.inner).y}
                    x2={polar(g, hoverFraction, reach).x}
                    y2={polar(g, hoverFraction, reach).y}
                    className="stroke-label-tertiary"
                    strokeWidth={1}
                    strokeDasharray="2 3"
                  />
                  {readout?.thisWeek != null && (
                    <circle
                      cx={polar(g, hoverFraction, radius(g, readout.thisWeek)).x}
                      cy={polar(g, hoverFraction, radius(g, readout.thisWeek)).y}
                      r={4}
                      className="fill-context-stroke stroke-surface"
                      strokeWidth={1.5}
                    />
                  )}
                </g>
              )}

              {placed.map((item) => {
                const from = polar(g, item.fraction, item.r)
                const to = polar(g, item.fraction, item.r + PIN_STEP - 2)
                const hovered = item.key === pinKey
                const base = item.current ? 1 : PAST_PIN_OPACITY
                const opacity = hovered
                  ? 1
                  : focusDetector && focusDetector !== item.pin.detector
                    ? base * DIM_SHARE
                    : base
                return (
                  <g
                    key={item.key}
                    data-waste-pin={item.pin.detector}
                    className="overview-waste-focus cursor-pointer"
                    style={{ opacity }}
                    onPointerEnter={() => setPinKey(item.key)}
                    onPointerLeave={() => setPinKey(null)}
                    onClick={() => waste?.onOpen?.(item.pin)}
                  >
                    <line
                      x1={from.x}
                      y1={from.y}
                      x2={to.x}
                      y2={to.y}
                      className="overview-waste-pin stroke-brand"
                      strokeWidth={hovered ? 4 : 2.5}
                      style={{ "--at": item.fraction, "--stack": item.stack } as CSSProperties}
                    />
                    <line
                      x1={from.x}
                      y1={from.y}
                      x2={to.x}
                      y2={to.y}
                      className="stroke-transparent"
                      strokeWidth={PIN_STEP + 2}
                    />
                  </g>
                )
              })}
              {labels.map((label) => (
                <text
                  key={label.key}
                  x={label.at.x}
                  y={label.at.y}
                  textAnchor={label.anchor}
                  dominantBaseline="middle"
                  className="overview-waste-label overview-waste-focus type-caption fill-label-secondary"
                  style={
                    {
                      "--at": label.fraction,
                      opacity:
                        focusDetector && focusDetector !== label.detector ? DIM_SHARE : 1,
                    } as CSSProperties
                  }
                  onPointerEnter={() => setLabelDetector(label.detector)}
                  onPointerLeave={() => setLabelDetector(null)}
                >
                  {label.text}
                  {label.count > 1 && (
                    <>
                      {" "}
                      <tspan className="fill-brand font-semibold">×{label.count}</tspan>
                    </>
                  )}
                </text>
              ))}

              {flag && (
                <g data-waste-flag="">
                  <line
                    x1={g.cx}
                    y1={g.cy - g.outer}
                    x2={g.cx}
                    y2={flag.top}
                    className="overview-waste-pole stroke-brand"
                    strokeWidth={1.5}
                  />
                  <circle
                    cx={g.cx}
                    cy={flag.top}
                    r={3}
                    className="overview-waste-flag-row fill-brand"
                  />
                  <text
                    x={flag.x}
                    y={flag.top + 4}
                    dominantBaseline="middle"
                    className="overview-waste-flag-row type-caption fill-label-tertiary"
                  >
                    Config · share of sessions
                  </text>
                  {config.map((item, index) => {
                    const y = flag.top + FLAG_HEAD + index * FLAG_ROW + 4
                    return (
                      <g
                        key={item.detector}
                        className="overview-waste-flag-row"
                        style={{ "--row": index + 1 } as CSSProperties}
                      >
                        <text
                          x={flag.x + FLAG_SHARE_WIDTH}
                          y={y}
                          textAnchor="end"
                          dominantBaseline="middle"
                          className="type-callout font-semibold tabular-nums fill-brand"
                        >
                          {Math.round(item.share * 100)}%
                        </text>
                        <text
                          x={flag.x + FLAG_SHARE_WIDTH + 8}
                          y={y}
                          dominantBaseline="middle"
                          className="type-callout fill-label-secondary"
                        >
                          {item.label}
                        </text>
                      </g>
                    )
                  })}
                </g>
              )}
            </svg>

            {center && (
              <div
                className="absolute inset-x-0 flex items-center justify-center"
                style={
                  {
                    top: flagRoom,
                    blockSize: side,
                    "--overview-hole-size": `${g.hole}px`,
                  } as CSSProperties
                }
              >
                {center}
              </div>
            )}

            {hoveredPin && clock && (
              <div
                className="ui-tooltip pointer-events-none absolute w-max"
                style={tooltipStyle}
              >
                <p className="font-semibold">{hoveredPin.pin.label}</p>
                <p>{hoveredPin.pin.title}</p>
                <p className="text-label-secondary">
                  {formatWhen(hoveredPin.pin.atEpoch)} ·{" "}
                  {weeksAgo(
                    Math.round((clock.startsAtEpoch - hoveredPin.weekStart) / clockSpan),
                  )}
                </p>
                <p className="text-label-tertiary">Click to open the session</p>
              </div>
            )}
            {readout && (
              <div
                className="ui-tooltip pointer-events-none absolute w-max"
                style={tooltipStyle}
              >
                <p className="font-semibold">{readout.when}</p>
                <p className="text-label-secondary">
                  This week{" "}
                  <span className="text-label tabular-nums">
                    {readout.thisWeek != null ? `${Math.round(readout.thisWeek)}%` : "–"}
                  </span>
                </p>
                {readout.pastAverage != null && (
                  <p className="text-label-secondary">
                    Past weeks, average{" "}
                    <span className="text-label tabular-nums">
                      {Math.round(readout.pastAverage)}%
                    </span>
                  </p>
                )}
                {readout.shortPeak != null && (
                  <p className="text-label-secondary">
                    5-hour window peak{" "}
                    <span className="text-label tabular-nums">
                      {Math.round(readout.shortPeak)}%
                    </span>
                  </p>
                )}
                {readout.nearby.slice(0, NEARBY_LIMIT).map((item) => (
                  <p key={item.key} className="truncate">
                    <span className="text-brand">{item.pin.label}</span> · {item.pin.title}
                  </p>
                ))}
                {readout.nearby.length > NEARBY_LIMIT && (
                  <p className="text-label-tertiary">
                    {readout.nearby.length - NEARBY_LIMIT} more wasteful sessions
                  </p>
                )}
              </div>
            )}
          </div>
        )}
      </div>
      <ChartLegend
        ariaLabel="Layers"
        className="mt-(--space-sm) justify-center"
        items={placed.length ? [...LEGEND_ITEMS, WASTE_LEGEND] : LEGEND_ITEMS}
      />
    </section>
  )
}
