import { useRef, useState, type CSSProperties, type PointerEvent, type ReactNode } from "react"

import type {
  AllowanceUsageAccountPayload,
  AllowanceWindowLevelsPayload,
} from "../../../lib/providerUsageIpc"
import { cn } from "../../../lib/cn"
import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"
import { ChartLegend, type ChartLegendItem } from "../../../components/ui/ChartLegend"
import { useElementHeight, useElementWidth } from "../../../lib/useElementWidth"
import { useEntranceProps } from "./overviewEntrance"
import { OverviewRadialTooltip } from "./OverviewRadialTooltip"
import {
  dimsData,
  legendLayer,
  pinEmphasis,
  pointerFocus,
  showsFlag,
  type RadialFocus,
  type RadialLayer,
} from "./radialFocus"
import { DataLayer, FocusMark, GridLayer, type Petal, type SpokeLine } from "./radialLayers"
import {
  DAYS_PER_WEEK,
  PIN_GAP,
  PIN_STACK_DEGREES,
  LIMIT_PERCENT,
  PIN_STEP,
  arcPath,
  buildSpokes,
  geometry,
  layoutPins,
  levelAt,
  limitStretches,
  petalPath,
  polar,
  radius,
  wedgePath,
  weekFraction,
  type PlacedPin,
  type Point,
} from "./radialGeometry"
import type { ConfigShare, WasteMarks } from "./wasteMarks"

// The outer ring stays this far inside the square, so the labels fit.
const EDGE_GAP = 16
// With waste marks, the ring leaves this much room for the pin stacks.
const PIN_ROOM = 64
const PAST_PIN_OPACITY = 0.35
// Parts out of focus fade to this share.
const DIM_SHARE = 0.25
// Pins inside one span share a label.
const CLUSTER_DEGREES = 20
// The flag at the reset: a header line, then one row per config check.
const FLAG_HEAD = 18
const FLAG_ROW = 17
const FLAG_SHARE_WIDTH = 30
// An estimate of caption text width, so labels can keep clear of each other.
const CHAR_WIDTH = 6.2

const LEGEND_ITEMS: readonly ChartLegendItem[] = [
  { key: "week", label: "This week", swatch: "bg-context-stroke" },
  { key: "past", label: "Past weeks", swatch: "bg-context-stroke/30" },
  { key: "short", label: "5-hour window", swatch: "bg-context-stroke/20" },
  { key: "rolling", label: "Average usage", swatch: "bg-gray-500", shape: "line" },
]
const LIMIT_LEGEND: ChartLegendItem = {
  key: "limit",
  label: "Limit hit",
  swatch: "bg-system-red",
  shape: "line",
}
const WASTE_LEGEND: ChartLegendItem = {
  key: "waste",
  label: "Wasteful session",
  swatch: "bg-brand",
  shape: "line",
}

type Box = { x0: number; x1: number; y0: number; y1: number }
function flagHeight(config: readonly ConfigShare[]): number {
  return config.length ? FLAG_HEAD + config.length * FLAG_ROW : 0
}

function emphasisOpacity(emphasis: "full" | "normal" | "dim", base: number): number {
  if (emphasis === "full") return 1
  return emphasis === "dim" ? base * DIM_SHARE : base
}

/** The allowance chart drawn around one week, so every week overlaps the
 *  others. The angle is the time since the weekly reset. The radius is the
 *  level. The chart fills the space it gets, as a square. Wasteful sessions
 *  stick out past the rim as pins; config checks sit once at the reset. The
 *  chart draws itself in clockwise from the reset. Red marks show where a
 *  week or a 5-hour window reached its limit.
 *
 *  Every part reacts to the pointer. The pointer snaps to a limit hit, a
 *  week's edge, a 5-hour segment or the average ring, and otherwise reads the
 *  time. A pin, a
 *  label, a day, a config row, a key entry or the Optimise button brings its
 *  part forward. The rest fades, and a card tells the numbers. */
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
  const [explicit, setExplicit] = useState<RadialFocus | null>(null)
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
  const rolling =
    [...account.chart.rolling]
      .reverse()
      .find((point) => point.atEpoch <= rangeEndEpoch && point.percent != null)?.percent ?? null

  const spokes: SpokeLine[] = buildSpokes(account.chart.shortWindows, weeks, current).map(
    (spoke) => ({
      ...spoke,
      path: wedgePath(g, spoke.from, spoke.to, radius(g, spoke.peakPercent)),
    }),
  )
  const petals: Petal[] = weeks.flatMap((window: AllowanceWindowLevelsPayload) => {
    const path = petalPath(g, window)
    return path ? [{ start: window.startsAtEpoch, current: window === current, path }] : []
  })
  const limits = limitStretches(weeks, current)
  const shortLimits = spokes.filter((spoke) => spoke.peakPercent >= LIMIT_PERCENT).length
  const currentLast = current?.points[current.points.length - 1]
  const tip =
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
  let flag: { top: number; bottom: number; x: number; width: number } | null = null
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
    flag = { top, bottom, x: g.cx + 8, width: flagWidth }
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
        return [{ key, detector, group, at, anchor, text, fraction }]
      }
      return []
    })

  // A part the pointer is on directly wins. Otherwise the pointer snaps to
  // the nearest line inside the ring and among the pins.
  const reach = Math.max(g.outer, ...binTop.values()) + PIN_STEP
  const snapped =
    !explicit && pointer
      ? pointerFocus(g, pointer, reach, weeks, spokes, rolling, limits)
      : null
  const focus = explicit ?? snapped?.focus ?? null
  const guide = snapped?.fraction ?? null
  const guideWeek =
    snapped?.focus.kind === "week"
      ? weeks.find(
          (window) =>
            snapped.focus.kind === "week" && window.startsAtEpoch === snapped.focus.start,
        )
      : snapped?.focus.kind === "time"
        ? current
        : undefined
  const guideLevel = guideWeek && guide != null ? levelAt(guideWeek, guide) : null
  const flagOpacity = showsFlag(focus) ? 1 : DIM_SHARE

  const summary =
    `${account.displayName}: ${weeks.length} weekly windows drawn on one week.` +
    (rolling != null ? ` Average usage is ${Math.round(rolling)} percent.` : "") +
    (limits.length ? ` The weekly limit was hit in ${limits.length} of these weeks.` : "") +
    (shortLimits ? ` The 5-hour limit was hit in ${shortLimits} of the 5-hour windows.` : "") +
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
  function hold(next: RadialFocus) {
    return {
      onPointerEnter: () => setExplicit(next),
      onPointerLeave: () => setExplicit(null),
    }
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
            <GridLayer g={g} side={side} height={side + flagRoom} />
            <DataLayer
              g={g}
              side={side}
              height={side + flagRoom}
              petals={petals}
              spokes={spokes}
              rolling={rolling}
              limits={limits}
              tip={tip}
              dimmed={dimsData(focus)}
            />

            <svg
              width={side}
              height={side + flagRoom}
              className="absolute inset-0 overflow-visible"
              aria-hidden="true"
              onPointerMove={trackPointer}
              onPointerLeave={() => {
                setPointer(null)
                setExplicit(null)
              }}
            >
              <g className="pointer-events-none">
                <FocusMark
                  g={g}
                  focus={focus}
                  petals={petals}
                  spokes={spokes}
                  rolling={rolling}
                  limits={limits}
                />
                {guide != null && (
                  <line
                    x1={polar(g, guide, g.inner).x}
                    y1={polar(g, guide, g.inner).y}
                    x2={polar(g, guide, reach).x}
                    y2={polar(g, guide, reach).y}
                    className="stroke-label-tertiary"
                    strokeWidth={1}
                    strokeDasharray="2 3"
                  />
                )}
                {guide != null && guideLevel != null && (
                  <circle
                    cx={polar(g, guide, radius(g, guideLevel)).x}
                    cy={polar(g, guide, radius(g, guideLevel)).y}
                    r={4}
                    className="fill-context-stroke stroke-surface"
                    strokeWidth={1.5}
                  />
                )}
              </g>

              {limits.map((limit) => (
                <path
                  key={limit.weekStart}
                  data-radial-limit={limit.weekStart}
                  d={arcPath(g, limit.from, limit.to, g.outer)}
                  fill="none"
                  className="stroke-transparent"
                  strokeWidth={10}
                  {...hold({ kind: "limit", weekStart: limit.weekStart })}
                />
              ))}

              {clock &&
                Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
                  const at = polar(g, (day + 0.5) / DAYS_PER_WEEK, g.outer - 12)
                  const epoch = clock.startsAtEpoch + ((day + 0.5) / DAYS_PER_WEEK) * clockSpan
                  const active = focus?.kind === "day" && focus.day === day
                  return (
                    <g key={day} data-radial-day={day} {...hold({ kind: "day", day })}>
                      <circle cx={at.x} cy={at.y} r={14} className="fill-transparent" />
                      <text
                        x={at.x}
                        y={at.y}
                        textAnchor="middle"
                        dominantBaseline="middle"
                        {...AXIS_TICK}
                        fontWeight={active ? 600 : undefined}
                      >
                        {new Date(epoch * 1000).toLocaleDateString(undefined, {
                          weekday: "short",
                        })}
                      </text>
                    </g>
                  )
                })}

              {placed.map((item) => {
                const from = polar(g, item.fraction, item.r)
                const to = polar(g, item.fraction, item.r + PIN_STEP - 2)
                const hovered = focus?.kind === "pin" && focus.key === item.key
                const opacity = emphasisOpacity(
                  pinEmphasis(item, focus),
                  item.current ? 1 : PAST_PIN_OPACITY,
                )
                return (
                  <g
                    key={item.key}
                    data-waste-pin={item.pin.detector}
                    className="overview-radial-focus cursor-pointer"
                    style={{ opacity }}
                    {...hold({ kind: "pin", key: item.key, detector: item.pin.detector })}
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
              {labels.map((label) => {
                const lit =
                  !focus ||
                  focus.kind === "time" ||
                  label.group.some((item) => pinEmphasis(item, focus) !== "dim")
                return (
                  <text
                    key={label.key}
                    x={label.at.x}
                    y={label.at.y}
                    textAnchor={label.anchor}
                    dominantBaseline="middle"
                    className="overview-waste-label overview-radial-focus type-caption fill-label-secondary"
                    style={
                      { "--at": label.fraction, opacity: lit ? 1 : DIM_SHARE } as CSSProperties
                    }
                    {...hold({ kind: "check", detector: label.detector })}
                  >
                    {label.text}
                    {label.group.length > 1 && (
                      <>
                        {" "}
                        <tspan className="fill-brand font-semibold">
                          ×{label.group.length}
                        </tspan>
                      </>
                    )}
                  </text>
                )
              })}

              {flag && (
                <g
                  data-waste-flag=""
                  className="overview-radial-focus"
                  style={{ opacity: flagOpacity }}
                >
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
                    const lit = focus?.kind !== "config" || focus.detector === item.detector
                    return (
                      <g
                        key={item.detector}
                        data-radial-config={item.detector}
                        className="overview-waste-flag-row overview-radial-focus"
                        style={
                          { "--row": index + 1, opacity: lit ? 1 : DIM_SHARE } as CSSProperties
                        }
                        {...hold({ kind: "config", detector: item.detector })}
                      >
                        <rect
                          x={flag.x - 4}
                          y={y - FLAG_ROW / 2}
                          width={flag.width + 8}
                          height={FLAG_ROW}
                          className="fill-transparent"
                        />
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
                className="pointer-events-none absolute inset-x-0 flex items-center justify-center"
                style={
                  {
                    top: flagRoom,
                    blockSize: side,
                    "--overview-hole-size": `${g.hole}px`,
                  } as CSSProperties
                }
              >
                {/* The button brings the waste forward: the pins and the flag. */}
                <div
                  className="pointer-events-auto"
                  onPointerEnter={() =>
                    marked && setExplicit({ kind: "layer", layer: "waste" })
                  }
                  onPointerLeave={() => setExplicit(null)}
                >
                  {center}
                </div>
              </div>
            )}

            {focus && clock && tooltipStyle && (
              <OverviewRadialTooltip
                focus={focus}
                fraction={guide}
                data={{ clock, current, past, weeks, spokes, rolling, limits, placed, config }}
                style={tooltipStyle}
              />
            )}
          </div>
        )}
      </div>
      <ChartLegend
        ariaLabel="Layers"
        size="large"
        className="mt-(--space-md) justify-center"
        items={[
          ...LEGEND_ITEMS,
          ...(limits.length || shortLimits ? [LIMIT_LEGEND] : []),
          ...(placed.length ? [WASTE_LEGEND] : []),
        ]}
        activeKey={legendLayer(focus, current?.startsAtEpoch)}
        onActiveChange={(key) =>
          setExplicit(key ? { kind: "layer", layer: key as RadialLayer } : null)
        }
      />
    </section>
  )
}
