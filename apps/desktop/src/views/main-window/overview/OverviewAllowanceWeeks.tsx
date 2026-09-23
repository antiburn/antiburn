import { useRef, useState, type CSSProperties, type PointerEvent, type ReactNode } from "react"

import type { AllowanceUsageAccountPayload } from "../../../lib/providerUsageIpc"
import { cn } from "../../../lib/cn"
import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"
import { ChartLegend, type ChartLegendItem } from "../../../components/ui/ChartLegend"
import { useElementHeight, useElementWidth } from "../../../lib/useElementWidth"
import { useEntranceProps } from "./overviewEntrance"
import { OverviewRadialTooltip } from "./OverviewRadialTooltip"
import { legendLayer, pinEmphasis, type RadialFocus, type RadialLayer } from "./radialFocus"
import {
  DAYS_PER_WEEK,
  LIMIT_PERCENT,
  buildSpokes,
  layoutPins,
  levelAt,
  limitStretches,
  spokeLevelAt,
  weekFraction,
  type Geometry,
  type Point,
  type Spoke,
} from "./radialGeometry"
import {
  bandTransform,
  bandY,
  fullBand,
  plotX,
  spokePath,
  spreadLabels,
  weekBands,
  weekName,
  weekPath,
  weekPointerFocus,
  weekTone,
  type Band,
  type Plot,
} from "./weekLines"
import type { WasteMarks } from "./wasteMarks"

// Room around the plot: the level ticks on the left, the week labels on the
// right, the day names below.
const AXIS_LEFT = 34
const LABEL_ROOM = 152
const AXIS_BOTTOM = 22
const TOP_GAP = 8
// Apart, the rows keep this gap.
const ROW_GAP = 16
// Pins stack up from the top of the plot, one step per session.
const PIN_STEP = 6
const PIN_LENGTH = 4
const PIN_GAP = 4
const MAX_PIN_ROWS = 5
const PAST_PIN_OPACITY = 0.35
// Week labels on the right keep this far apart.
const LABEL_GAP = 15
// Parts out of focus fade to this share.
const DIM_SHARE = 0.25
// The Optimise button's size, as a share of the plot height, and its limits.
const ORB_SHARE = 0.34
const ORB_MIN = 64
const ORB_MAX = 104
// `layoutPins` places pins on the flower's ring. This chart reads only the
// stack and the time of week, so the ring has no size.
const NO_RING: Geometry = { cx: 0, cy: 0, inner: 0, outer: 0, hole: 0 }

const LEGEND_ITEMS: readonly ChartLegendItem[] = [
  { key: "week", label: "This week", swatch: "bg-context-stroke" },
  { key: "past", label: "Past weeks", swatch: "bg-context-stroke/30" },
]
const SHORT_LEGEND: ChartLegendItem = {
  key: "short",
  label: "5-hour window",
  swatch: "bg-gray-500/30",
}
const ROLLING_LEGEND: ChartLegendItem = {
  key: "rolling",
  label: "Average usage",
  swatch: "bg-gray-500",
  shape: "line",
}
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
const WEEK_KEY = "w:"

type Strength = "full" | "normal" | "dim"

function opacityOf(strength: Strength, base = 1): number {
  if (strength === "full") return 1
  return strength === "dim" ? base * DIM_SHARE : base
}

/** How strong a week draws for a focus. `focusWeek` is the week the focus
 *  belongs to, for a week, a limit hit or a 5-hour window. */
function weekStrength(
  start: number,
  current: boolean,
  hasLimit: boolean,
  focus: RadialFocus | null,
  focusWeek: number | null,
): Strength {
  switch (focus?.kind) {
    case undefined:
    case "time":
    case "day":
      return "normal"
    case "week":
      return focusWeek === start ? "full" : "dim"
    case "limit":
    case "short":
      return focusWeek === start ? "normal" : "dim"
    case "layer":
      if (focus.layer === "week") return current ? "full" : "dim"
      if (focus.layer === "past") return current ? "dim" : "full"
      if (focus.layer === "short") return "normal"
      if (focus.layer === "limit") return hasLimit ? "normal" : "dim"
      return "dim"
    default:
      return "dim"
  }
}

/** True when a 5-hour curve draws at full strength: it is in focus, its
 *  week is, or its key entry is. */
function spokeStrong(spoke: Spoke, focus: RadialFocus | null): boolean {
  switch (focus?.kind) {
    case "short":
      return focus.key === spoke.key
    case "week":
      return focus.start === spoke.weekStart
    case "layer":
      if (focus.layer === "short") return true
      if (focus.layer === "week") return spoke.current
      if (focus.layer === "past") return !spoke.current
      return focus.layer === "limit" && spoke.peakPercent >= LIMIT_PERCENT
    default:
      return false
  }
}

/** The allowance chart as week lines. Every weekly window draws from its
 *  reset on one shared week, so the weeks overlap. Each 5-hour window draws
 *  as its own rising curve under its week. Red on the 100% line shows where
 *  a week sat at its limit. "Break apart" moves each week into its own row.
 *
 *  Every part reacts to the pointer. The pointer snaps to a limit hit, a
 *  week's line, a 5-hour curve or the average line, and otherwise reads the
 *  time. A week label, a key entry, a day, a pin or the Optimise button
 *  brings its part forward. The rest fades, and a card tells the numbers. */
export function OverviewAllowanceWeeks({
  account,
  rangeEndEpoch,
  controls,
  center,
  waste,
}: {
  account: AllowanceUsageAccountPayload
  rangeEndEpoch: number
  controls?: ReactNode
  /** The content in the empty top left of the plot. */
  center?: ReactNode
  waste?: WasteMarks | undefined
}) {
  const frameRef = useRef<HTMLDivElement | null>(null)
  const width = useElementWidth(frameRef)
  const height = useElementHeight(frameRef)
  const [pointer, setPointer] = useState<Point | null>(null)
  const [explicit, setExplicit] = useState<RadialFocus | null>(null)
  const [apart, setApart] = useState(false)
  const [colours, setColours] = useState(true)

  const weeks = account.chart.weeklyWindows.filter((window) => window.lane === "weekly")
  const current = weeks.find(
    (window) => window.startsAtEpoch <= rangeEndEpoch && rangeEndEpoch < window.resetsAtEpoch,
  )
  // The week that names the days and times on the chart.
  const clock = current ?? weeks[weeks.length - 1]
  const clockSpan = clock ? clock.resetsAtEpoch - clock.startsAtEpoch : 0
  const past = weeks.filter((window) => window !== current)
  // Newest first: the order of the colours, the rows and the key.
  const newest = [...weeks].reverse()
  const rolling =
    [...account.chart.rolling]
      .reverse()
      .find((point) => point.atEpoch <= rangeEndEpoch && point.percent != null)?.percent ?? null
  const spokes = buildSpokes(account.chart.shortWindows, weeks, current)
  const limits = limitStretches(weeks, current)
  const shortLimits = spokes.filter((spoke) => spoke.peakPercent >= LIMIT_PERCENT).length
  const pins = waste?.pins ?? []
  const { placed } = layoutPins(NO_RING, weeks, current, pins)
  const pinRows = Math.min(MAX_PIN_ROWS, Math.max(0, ...placed.map((item) => item.stack + 1)))

  const plot: Plot = {
    x0: AXIS_LEFT,
    x1: Math.max(AXIS_LEFT, width - LABEL_ROOM),
    y0: TOP_GAP + (pinRows ? PIN_GAP + pinRows * PIN_STEP : 0),
    y1: Math.max(0, height - AXIS_BOTTOM),
  }
  const ready = clock != null && plot.x1 - plot.x0 > 40 && plot.y1 - plot.y0 > 40
  const entrance = useEntranceProps("allowance-weeks", "overview-chart-in", ready)
  const full = fullBand(plot)
  const bands = weekBands(plot, newest.length, apart, ROW_GAP)
  const bandByStart = new Map(newest.map((week, index) => [week.startsAtEpoch, bands[index]!]))
  const bandOf = (start: number): Band => bandByStart.get(start) ?? full
  const toneOf = (start: number): string | undefined => {
    const index = newest.findIndex((week) => week.startsAtEpoch === start)
    return colours && index >= 0 ? weekTone(index) : undefined
  }

  const snapped =
    !explicit && pointer && ready
      ? weekPointerFocus(plot, pointer, weeks, bandOf, spokes, rolling, limits, apart)
      : null
  const focus = explicit ?? snapped?.focus ?? null
  const guide = snapped?.fraction ?? null
  const focusWeek =
    focus?.kind === "week"
      ? focus.start
      : focus?.kind === "limit"
        ? focus.weekStart
        : focus?.kind === "short"
          ? (spokes.find((spoke) => spoke.key === focus.key)?.weekStart ?? null)
          : null

  // The dot on the guide: the level of the part in focus at the guide's time.
  const guideDot = (() => {
    if (guide == null || !snapped) return null
    if (snapped.focus.kind === "short") {
      const key = snapped.focus.key
      const spoke = spokes.find((item) => item.key === key)
      const level = spoke ? spokeLevelAt(spoke, guide) : null
      return spoke && level != null ? { start: spoke.weekStart, level } : null
    }
    const week =
      snapped.focus.kind === "week"
        ? weeks.find((window) => window.startsAtEpoch === focusWeek)
        : snapped.focus.kind === "time"
          ? current
          : undefined
    const level = week ? levelAt(week, guide) : null
    return week && level != null ? { start: week.startsAtEpoch, level } : null
  })()

  const rollingFocused =
    focus?.kind === "rolling" || (focus?.kind === "layer" && focus.layer === "rolling")
  const rollingLit = rollingFocused || !focus || focus.kind === "time" || focus.kind === "day"

  // Each week's label sits at the right end of its line while the weeks
  // overlap, and at the top left of its row while they are apart.
  const lastLevel = (week: (typeof weeks)[number]) =>
    week.points[week.points.length - 1]?.percent ?? 0
  const spread = spreadLabels(
    newest.map((week) => bandY(full, lastLevel(week))),
    LABEL_GAP,
    plot.y0,
    plot.y1,
  )

  const summary =
    `${account.displayName}: ${weeks.length} weekly windows drawn over one week.` +
    (rolling != null ? ` Average usage is ${Math.round(rolling)} percent.` : "") +
    (limits.length ? ` The weekly limit was hit in ${limits.length} of these weeks.` : "") +
    (shortLimits ? ` The 5-hour limit was hit in ${shortLimits} of the 5-hour windows.` : "") +
    (placed.length ? ` ${placed.length} wasteful sessions are pinned by time of week.` : "")

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

  const legendItems: ChartLegendItem[] = [
    ...(colours
      ? newest.map((week, index) => ({
          key: `${WEEK_KEY}${week.startsAtEpoch}`,
          label: clock ? weekName(week, clock) : "",
          swatch: `${weekTone(index)} bg-(--week)`,
        }))
      : LEGEND_ITEMS),
    ...(spokes.length ? [SHORT_LEGEND] : []),
    ...(rolling != null && !apart ? [ROLLING_LEGEND] : []),
    ...(limits.length || shortLimits ? [LIMIT_LEGEND] : []),
    ...(placed.length ? [WASTE_LEGEND] : []),
  ]
  const activeKey =
    colours && focus?.kind === "week"
      ? `${WEEK_KEY}${focus.start}`
      : legendLayer(focus, current?.startsAtEpoch)

  const tooltipStyle: CSSProperties | undefined = pointer
    ? {
        left: pointer.x > width / 2 ? pointer.x - 14 : pointer.x + 14,
        top: pointer.y > height / 2 ? pointer.y - 14 : pointer.y + 14,
        translate: `${pointer.x > width / 2 ? "-100%" : "0"} ${pointer.y > height / 2 ? "-100%" : "0"}`,
      }
    : undefined
  const orbSize = Math.round(
    Math.min(ORB_MAX, Math.max(ORB_MIN, (plot.y1 - plot.y0) * ORB_SHARE)),
  )

  return (
    <section className="overview-chart overview-weeks" aria-label="Allowance chart">
      <p className="sr-only">{summary}</p>
      <div className="overview-chart-legend mb-(--space-sm) flex items-center justify-between gap-(--space-sm)">
        <div className="flex items-center gap-(--space-xs)">
          <button
            type="button"
            className="ui-push-button"
            aria-pressed={colours}
            onClick={() => setColours(!colours)}
          >
            {colours ? "One colour" : "Colour each week"}
          </button>
          {weeks.length > 1 && (
            <button
              type="button"
              className="ui-push-button"
              aria-pressed={apart}
              onClick={() => {
                setApart(!apart)
                setExplicit(null)
              }}
            >
              {apart ? "Put together" : "Break apart"}
            </button>
          )}
        </div>
        {controls}
      </div>
      <div
        ref={frameRef}
        className={cn("relative min-h-0 flex-1", entrance.className)}
        onAnimationEnd={entrance.onAnimationEnd}
      >
        {ready && clock && (
          <>
            <svg
              width={width}
              height={height}
              className="absolute inset-0 overflow-visible"
              aria-hidden="true"
              data-apart={apart ? "" : undefined}
              onPointerMove={trackPointer}
              onPointerLeave={() => {
                setPointer(null)
                setExplicit(null)
              }}
            >
              {focus?.kind === "day" && (
                <rect
                  x={plotX(plot, focus.day / DAYS_PER_WEEK)}
                  y={plot.y0}
                  width={(plot.x1 - plot.x0) / DAYS_PER_WEEK}
                  height={plot.y1 - plot.y0}
                  className="fill-context-stroke/[0.06]"
                />
              )}

              {/* The shared level grid. It steps back while the weeks are
                  apart, and each row draws its own. */}
              <g className="week-grid" style={{ opacity: apart ? 0 : 1 }}>
                {[0, 50, 100].map((percent) => (
                  <g key={percent}>
                    <line
                      x1={plot.x0}
                      x2={plot.x1}
                      y1={bandY(full, percent)}
                      y2={bandY(full, percent)}
                      stroke="var(--color-separator)"
                      strokeWidth={1}
                    />
                    <text
                      x={plot.x0 - 6}
                      y={bandY(full, percent)}
                      textAnchor="end"
                      dominantBaseline="middle"
                      {...AXIS_TICK}
                    >
                      {percent}%
                    </text>
                  </g>
                ))}
              </g>
              {Array.from({ length: DAYS_PER_WEEK + 1 }, (_, day) => (
                <line
                  key={day}
                  x1={plotX(plot, day / DAYS_PER_WEEK)}
                  x2={plotX(plot, day / DAYS_PER_WEEK)}
                  y1={plot.y0}
                  y2={plot.y1}
                  stroke="var(--color-separator)"
                  strokeWidth={1}
                />
              ))}

              {/* Oldest first, so this week draws on top. */}
              {weeks.map((week) => {
                const isCurrent = week === current
                const path = weekPath(plot, week)
                const limit = limits.find((item) => item.weekStart === week.startsAtEpoch)
                const strength = weekStrength(
                  week.startsAtEpoch,
                  isCurrent,
                  limit != null,
                  focus,
                  focusWeek,
                )
                const bold = apart || isCurrent || strength === "full"
                const weekSpokes = spokes.filter(
                  (spoke) => spoke.weekStart === week.startsAtEpoch,
                )
                return (
                  <g
                    key={week.startsAtEpoch}
                    data-week={week.startsAtEpoch}
                    className={cn("week-band", toneOf(week.startsAtEpoch))}
                    style={
                      {
                        transform: bandTransform(plot, bandOf(week.startsAtEpoch)),
                        opacity: opacityOf(strength),
                        "--row": newest.indexOf(week),
                      } as CSSProperties
                    }
                  >
                    <g className="week-grid" style={{ opacity: apart ? 1 : 0 }}>
                      {[0, 100].map((percent) => (
                        <line
                          key={percent}
                          x1={plot.x0}
                          x2={plot.x1}
                          y1={bandY(full, percent)}
                          y2={bandY(full, percent)}
                          stroke="var(--color-separator)"
                          strokeWidth={1}
                          vectorEffect="non-scaling-stroke"
                        />
                      ))}
                    </g>
                    {weekSpokes.map((spoke) => {
                      const shape = spokePath(plot, spoke)
                      if (!shape) return null
                      const strong = spokeStrong(spoke, focus)
                      const hit = spoke.peakPercent >= LIMIT_PERCENT
                      return (
                        <g key={spoke.key} data-short={spoke.key}>
                          <path
                            d={shape.area}
                            className={
                              hit
                                ? strong
                                  ? "fill-system-red/35"
                                  : "fill-system-red/15"
                                : strong
                                  ? "fill-(--week)/25"
                                  : "fill-(--week)/[0.07]"
                            }
                          />
                          <path
                            d={shape.edge}
                            fill="none"
                            className={
                              hit
                                ? strong
                                  ? "stroke-system-red"
                                  : "stroke-system-red/50"
                                : strong
                                  ? "stroke-(--week)"
                                  : "stroke-(--week)/25"
                            }
                            strokeWidth={strong ? 1.5 : 1}
                            strokeLinejoin="round"
                            vectorEffect="non-scaling-stroke"
                          />
                        </g>
                      )
                    })}
                    {path && (
                      <>
                        <path
                          d={path.area}
                          className={
                            strength === "full"
                              ? "fill-(--week)/25"
                              : bold
                                ? "fill-(--week)/15"
                                : colours
                                  ? "fill-(--week)/[0.07]"
                                  : "fill-(--week)/[0.05]"
                          }
                        />
                        <path
                          d={path.edge}
                          fill="none"
                          className={
                            bold
                              ? "stroke-(--week)"
                              : colours
                                ? "stroke-(--week)/80"
                                : "stroke-(--week)/35"
                          }
                          strokeWidth={strength === "full" ? 2.5 : bold ? 2 : colours ? 1.5 : 1}
                          strokeLinejoin="round"
                          vectorEffect="non-scaling-stroke"
                        />
                      </>
                    )}
                    {limit && (
                      <line
                        x1={plotX(plot, limit.from)}
                        x2={plotX(plot, Math.max(limit.to, limit.from + 0.004))}
                        y1={bandY(full, 100)}
                        y2={bandY(full, 100)}
                        className="stroke-system-red"
                        strokeWidth={
                          focus?.kind === "limit" && focusWeek === week.startsAtEpoch ? 6 : 4
                        }
                        strokeLinecap="round"
                        vectorEffect="non-scaling-stroke"
                        {...hold({ kind: "limit", weekStart: week.startsAtEpoch })}
                      />
                    )}
                  </g>
                )
              })}

              {rolling != null && (
                <line
                  x1={plot.x0}
                  x2={plot.x1}
                  y1={bandY(full, rolling)}
                  y2={bandY(full, rolling)}
                  className={cn(
                    "week-grid",
                    rollingFocused ? "stroke-label" : "stroke-gray-500",
                  )}
                  strokeWidth={rollingFocused ? 2 : 1}
                  strokeDasharray="4 3"
                  style={{ opacity: apart ? 0 : rollingLit ? 1 : DIM_SHARE }}
                />
              )}

              {current && (
                <g
                  className={cn("week-move", toneOf(current.startsAtEpoch))}
                  style={{
                    transform: `translate(${plotX(
                      plot,
                      weekFraction(
                        current,
                        current.points[current.points.length - 1]?.atEpoch ?? 0,
                      ),
                    )}px, ${bandY(bandOf(current.startsAtEpoch), lastLevel(current))}px)`,
                  }}
                >
                  <circle r={3.5} className="fill-(--week)" />
                </g>
              )}

              {guide != null && (
                <line
                  x1={plotX(plot, guide)}
                  x2={plotX(plot, guide)}
                  y1={plot.y0}
                  y2={plot.y1}
                  className="pointer-events-none stroke-label-tertiary"
                  strokeWidth={1}
                  strokeDasharray="2 3"
                />
              )}
              {guide != null && guideDot && (
                <circle
                  cx={plotX(plot, guide)}
                  cy={bandY(bandOf(guideDot.start), guideDot.level)}
                  r={4}
                  className={cn(
                    "pointer-events-none fill-(--week) stroke-surface",
                    toneOf(guideDot.start),
                  )}
                  strokeWidth={1.5}
                />
              )}

              {placed.map((item) => {
                const x = plotX(plot, item.fraction)
                const band = bandOf(item.weekStart)
                const y = apart
                  ? band.top + PIN_LENGTH + 1 + item.stack * PIN_STEP
                  : plot.y0 - PIN_GAP - item.stack * PIN_STEP
                const hovered = focus?.kind === "pin" && focus.key === item.key
                const opacity = opacityOf(
                  pinEmphasis(item, focus),
                  item.current ? 1 : PAST_PIN_OPACITY,
                )
                return (
                  <g
                    key={item.key}
                    data-waste-pin={item.pin.detector}
                    className="week-move cursor-pointer"
                    style={{ transform: `translate(${x}px, ${y}px)`, opacity }}
                    {...hold({ kind: "pin", key: item.key, detector: item.pin.detector })}
                    onClick={() => waste?.onOpen?.(item.pin)}
                  >
                    <line
                      y1={0}
                      y2={-PIN_LENGTH}
                      className="stroke-brand"
                      strokeWidth={hovered ? 4 : 2.5}
                    />
                    <line
                      y1={2}
                      y2={-PIN_LENGTH - 2}
                      className="stroke-transparent"
                      strokeWidth={PIN_STEP + 2}
                    />
                  </g>
                )
              })}

              {newest.map((week, index) => {
                const band = bandOf(week.startsAtEpoch)
                const x = apart ? plot.x0 + 6 : plot.x1 + 8
                const y = apart ? band.top + 10 : spread[index]!
                const limit = limits.some((item) => item.weekStart === week.startsAtEpoch)
                const strength = weekStrength(
                  week.startsAtEpoch,
                  week === current,
                  limit,
                  focus,
                  focusWeek,
                )
                return (
                  <g
                    key={week.startsAtEpoch}
                    data-week-label={week.startsAtEpoch}
                    className={cn("week-move", toneOf(week.startsAtEpoch))}
                    style={
                      {
                        transform: `translate(${x}px, ${y}px)`,
                        opacity: opacityOf(strength),
                        "--row": index,
                      } as CSSProperties
                    }
                    {...hold({ kind: "week", start: week.startsAtEpoch })}
                  >
                    <rect
                      x={-4}
                      y={-LABEL_GAP / 2}
                      width={LABEL_ROOM - 8}
                      height={LABEL_GAP}
                      className="fill-transparent"
                    />
                    <text dominantBaseline="middle" className="type-caption">
                      <tspan
                        className={cn(
                          "font-semibold",
                          colours ? "fill-(--week)" : "fill-label-secondary",
                        )}
                      >
                        {weekName(week, clock)}
                      </tspan>{" "}
                      <tspan className="fill-label tabular-nums">
                        {Math.round(lastLevel(week))}%
                      </tspan>
                      {limit && (
                        <tspan className="fill-system-red-text font-semibold"> · limit</tspan>
                      )}
                    </text>
                  </g>
                )
              })}

              {Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
                const epoch = clock.startsAtEpoch + ((day + 0.5) / DAYS_PER_WEEK) * clockSpan
                const x = plotX(plot, (day + 0.5) / DAYS_PER_WEEK)
                const active = focus?.kind === "day" && focus.day === day
                return (
                  <g key={day} data-week-day={day} {...hold({ kind: "day", day })}>
                    <rect
                      x={x - (plot.x1 - plot.x0) / DAYS_PER_WEEK / 2}
                      y={plot.y1}
                      width={(plot.x1 - plot.x0) / DAYS_PER_WEEK}
                      height={AXIS_BOTTOM}
                      className="fill-transparent"
                    />
                    <text
                      x={x}
                      y={plot.y1 + AXIS_BOTTOM / 2 + 2}
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
            </svg>

            {center && !apart && (
              // The weeks rise from the bottom left, so the top left stays empty.
              <div
                className="absolute"
                style={
                  {
                    left: plot.x0 + 16,
                    top: plot.y0 + 12,
                    "--overview-hole-size": `${orbSize}px`,
                  } as CSSProperties
                }
                onPointerEnter={() =>
                  placed.length > 0 && setExplicit({ kind: "layer", layer: "waste" })
                }
                onPointerLeave={() => setExplicit(null)}
              >
                {center}
              </div>
            )}

            {focus && tooltipStyle && (
              <OverviewRadialTooltip
                focus={focus}
                fraction={guide}
                data={{
                  clock,
                  current,
                  past,
                  weeks,
                  spokes,
                  rolling,
                  limits,
                  placed,
                  config: [],
                }}
                style={tooltipStyle}
              />
            )}
          </>
        )}
      </div>
      <ChartLegend
        ariaLabel="Layers"
        size="large"
        className="mt-(--space-md) justify-center"
        items={legendItems}
        activeKey={activeKey}
        onActiveChange={(key) =>
          setExplicit(
            key == null
              ? null
              : key.startsWith(WEEK_KEY)
                ? { kind: "week", start: Number(key.slice(WEEK_KEY.length)) }
                : { kind: "layer", layer: key as RadialLayer },
          )
        }
      />
    </section>
  )
}
