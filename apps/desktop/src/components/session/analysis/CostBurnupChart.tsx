import type { CSSProperties } from "react"
import {
  Area,
  AreaChart,
  ReferenceDot,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts"

import { slowAnimationDurationMs } from "../../../lib/popoverHeight"
import { modelShortName } from "../../../lib/presentation/models"
import {
  costAxisScale,
  costBurnupSeries,
  formatCost,
  formatCostTick,
  formatDuration,
  timeAxisTicks,
  type CostBurnupPoint,
} from "../../../lib/presentation/sessionAnalysis"
import type { SessionBucket } from "../../../lib/types/session"
import { AXIS_LABEL, AXIS_TICK, labeledIndices } from "./chartLabels"
import { GLASS_TOOLTIP_STYLE } from "./tooltip"
import { useChartResize } from "./useChartResize"

/**
 * A layer of the burnup plot that a key entry can name.
 *
 * The four billable components stack from the baseline up; the rest are
 * marks the plot draws over the stack, not areas of their own.
 */
export type CostSeries =
  | "input"
  | "output"
  | "cacheRead"
  | "cacheWrite"
  | "rehydration"
  | "compaction"
  | "subagentLaunch"

export interface CostBurnupChartProps {
  buckets: SessionBucket[]
  /** Active seconds the buckets span; null hides the time marks. */
  activeSecs?: number | null
  /**
   * The layer to isolate. Null draws every layer in its own color; a named
   * layer keeps that one in color and rests every other layer in grey.
   */
  highlight?: CostSeries | null
}

/** The plot fills the height its tab gives it, and never draws shorter than this. */
const CHART_MIN_HEIGHT = 180
/** The height the time axis takes under the plot. */
const TIME_AXIS_HEIGHT = 16
/** The width the dollar axis takes on the left of the plot, sized for "$1.2k". */
const VALUE_AXIS_WIDTH = 44
/** An event mark's stroke width at rest and when its own key row is lit. */
const MARK_STROKE_WIDTH = 1
const MARK_LIT_STROKE_WIDTH = 1.5
/** An event mark's dash pattern. Short dashes with a round cap read as dots. */
const MARK_DASH = "1 3"
/** Every mark at rest. A mark is a hairline, so it takes the denser grey. */
const REST_MARK_STROKE = "var(--color-chart-rest-mark)"
/** The lit stroke of each event mark drawn over the plot. */
const MARK_STROKE: Record<"rehydration" | "compaction", string> = {
  rehydration: "var(--color-mark-rehydration)",
  compaction: "var(--color-mark-compaction)",
}
/** A sub-agent launch tick's fixed pixel geometry: a short mark on the baseline. */
const SUBAGENT_TICK_WIDTH = 2
const SUBAGENT_TICK_HEIGHT = 6

/**
 * A vertical mark label is about one glyph height wide, so two labels can
 * sit closer together on the x domain than two horizontal pill labels can.
 */
const VERTICAL_LABEL_MIN_GAP_FRACTION = 0.05

/**
 * Sub-agent launch marks are an experiment: a short tick on the baseline for
 * each bucket that launched one, with no label. Set to `false` to drop them
 * if they read as noise.
 */
const SHOW_SUBAGENT_LAUNCH_MARKS = true

/**
 * The four billable layers, from the baseline up: input, output, cache
 * write, then cache read. Cache write sits on the more stable lower stack,
 * because it is the layer a user can affect most and so needs the clearest
 * shape. `restVar` gives each layer its own resting grey step, so the four
 * stay distinguishable with no color at all: output takes the strongest
 * step, as the Context chart's token layers do, and the two cache layers
 * take the two faintest steps.
 */
const COST_ROWS: Array<{
  key: "inputUsd" | "outputUsd" | "cacheReadUsd" | "cacheWriteUsd"
  label: string
  colorVar: string
  restVar: string
  series: CostSeries
}> = [
  {
    key: "inputUsd",
    label: "Input",
    colorVar: "var(--color-token-in)",
    restVar: "var(--color-chart-rest)",
    series: "input",
  },
  {
    key: "outputUsd",
    label: "Output",
    colorVar: "var(--color-token-out)",
    restVar: "var(--color-chart-rest-strong)",
    series: "output",
  },
  {
    key: "cacheWriteUsd",
    label: "Cache write",
    colorVar: "var(--color-cost-cache-write)",
    restVar: "var(--color-chart-rest-fainter)",
    series: "cacheWrite",
  },
  {
    key: "cacheReadUsd",
    label: "Cache read",
    colorVar: "var(--color-cost-cache-read)",
    restVar: "var(--color-chart-rest-faint)",
    series: "cacheRead",
  },
]

/** The label of a compaction mark: names the trigger when it is manual. */
function compactionMarkLabel(point: CostBurnupPoint): string {
  return point.compactionTrigger === "manual" ? "Manual compaction" : "Compaction"
}

export interface CostBurnupTooltipProps {
  active?: boolean
  activeSecs?: number | null
  bucketCount?: number
  payload?: Array<{ payload?: CostBurnupPoint }>
}

/**
 * The custom tooltip shows elapsed time, the running total, each component's
 * cumulative figure, this bucket's own slice, and any marks it carries.
 */
export function CostBurnupTooltip({
  active,
  payload,
  activeSecs = null,
  bucketCount = 0,
}: CostBurnupTooltipProps) {
  const point = payload?.[0]?.payload
  if (!active || !point) return null
  const elapsed =
    activeSecs != null && bucketCount > 1
      ? formatDuration((point.index / (bucketCount - 1)) * activeSecs)
      : null

  return (
    <div
      className="text-label"
      style={{
        ...GLASS_TOOLTIP_STYLE,
        lineHeight: 1.4,
        padding: "6px 9px",
        whiteSpace: "nowrap",
      }}
    >
      {elapsed != null && <div className="mb-1">{elapsed} into session</div>}
      <div className="flex flex-col gap-1 text-label-secondary">
        <span>
          Total so far · <span className="tabular-nums">{formatCost(point.totalUsd)}</span>
        </span>
        {COST_ROWS.map((row) => (
          <span key={row.key} className="flex items-center gap-1.5">
            <span
              className="h-2 w-2 shrink-0 rounded-full"
              style={{ backgroundColor: row.colorVar }}
            />
            {row.label} · <span className="tabular-nums">{formatCost(point[row.key])}</span>
          </span>
        ))}
        {point.bucketUsd > 0 && (
          <span>
            This slice · <span className="tabular-nums">{formatCost(point.bucketUsd)}</span>
          </span>
        )}
        {point.isCompactionBoundary && (
          <span style={{ color: MARK_STROKE.compaction }}>{compactionMarkLabel(point)}</span>
        )}
        {point.isCacheRehydration && (
          <span style={{ color: MARK_STROKE.rehydration }}>Cache rehydration</span>
        )}
        {point.subagentLaunches > 0 && (
          <span>
            Sub-agents launched · <span className="tabular-nums">{point.subagentLaunches}</span>
          </span>
        )}
        {point.model != null && <span>Model · {modelShortName(point.model)}</span>}
      </div>
    </div>
  )
}

/**
 * The Cost tab's stacked cumulative cost chart: the four billable components
 * build up from the baseline, on the same bucket-index x axis the Context
 * chart uses, so the two charts stay lined up for a reader moving between
 * tabs.
 */
export function CostBurnupChart({
  buckets,
  activeSecs = null,
  highlight = null,
}: CostBurnupChartProps) {
  const data = costBurnupSeries(buckets)
  const { onResize, resizing, animate, initial } = useChartResize(buckets)
  const animationDurationMs = slowAnimationDurationMs()
  // Marks fade in once the stack has finished growing, on the first paint
  // only: a live update animates everything together, so a staggered replay
  // does not read as the panel redrawing itself.
  const markDelayMs = animate && initial ? animationDurationMs : 0

  const peakTotal = data.length > 0 ? data[data.length - 1]!.totalUsd : 0
  const hasCostData = peakTotal > 0
  const costAxis = costAxisScale(peakTotal, 4)

  const timeTicks = activeSecs != null ? timeAxisTicks(activeSecs, data.length, 6) : []
  const timeTickLabels = new Map(timeTicks.map((tick) => [tick.index, tick.label]))

  const rehydrationIndices = data
    .filter((point) => point.isCacheRehydration)
    .map((point) => point.index)
  const labeledRehydration = labeledIndices(
    rehydrationIndices,
    data.length,
    VERTICAL_LABEL_MIN_GAP_FRACTION,
  )
  const compactionIndices = data
    .filter((point) => point.isCompactionBoundary)
    .map((point) => point.index)
  const labeledCompaction = labeledIndices(
    compactionIndices,
    data.length,
    VERTICAL_LABEL_MIN_GAP_FRACTION,
  )

  return (
    <ResponsiveContainer
      width="100%"
      height="100%"
      minHeight={CHART_MIN_HEIGHT}
      onResize={onResize}
      className={resizing ? "[&_.animate-chart-mark]:animate-none" : ""}
      style={{ "--chart-mark-delay": `${markDelayMs}ms` } as CSSProperties}
    >
      <AreaChart data={data} margin={{ top: 6, right: 12, bottom: 0, left: 0 }}>
        {/* A numeric axis on the bucket index, shared with the Context chart,
            so marks land at the index they belong to rather than a rounded
            category value. */}
        <XAxis
          dataKey="index"
          type="number"
          domain={[0, Math.max(1, data.length - 1)]}
          hide={timeTicks.length === 0}
          ticks={timeTicks.map((tick) => tick.index)}
          tickFormatter={(value: number) => timeTickLabels.get(value) ?? ""}
          interval={0}
          axisLine={false}
          tickLine={false}
          tickMargin={2}
          height={TIME_AXIS_HEIGHT}
          tick={AXIS_TICK}
        />
        <YAxis
          yAxisId="cost"
          domain={[0, costAxis.ceiling]}
          ticks={costAxis.ticks}
          tickFormatter={formatCostTick}
          axisLine={false}
          tickLine={false}
          tickMargin={4}
          width={VALUE_AXIS_WIDTH}
          tick={AXIS_TICK}
        />
        <Tooltip
          cursor={{ stroke: "var(--color-separator)" }}
          isAnimationActive={false}
          content={<CostBurnupTooltip activeSecs={activeSecs} bucketCount={data.length} />}
        />
        {hasCostData &&
          COST_ROWS.map((row) => (
            <Area
              key={row.key}
              yAxisId="cost"
              type="monotone"
              dataKey={row.key}
              stackId="cost"
              stroke="none"
              fill={highlight == null || highlight === row.series ? row.colorVar : row.restVar}
              isAnimationActive={animate}
              animationDuration={animationDurationMs}
              animationBegin={0}
              animationEasing="ease-out"
            />
          ))}
        {/* Marks draw after the stack, so they stay visible over its solid
            fill instead of sitting underneath it. */}
        {hasCostData &&
          data
            .filter((point) => point.isCompactionBoundary)
            .map((point) => {
              const lit = highlight == null || highlight === "compaction"
              const emphasised = highlight === "compaction"
              return (
                <ReferenceLine
                  key={`compaction-${point.index}`}
                  className="animate-chart-mark"
                  yAxisId="cost"
                  x={point.index}
                  stroke={lit ? MARK_STROKE.compaction : REST_MARK_STROKE}
                  strokeWidth={emphasised ? MARK_LIT_STROKE_WIDTH : MARK_STROKE_WIDTH}
                  strokeDasharray={MARK_DASH}
                  strokeLinecap="round"
                />
              )
            })}
        {hasCostData &&
          data
            .filter((point) => point.isCacheRehydration)
            .map((point) => {
              const lit = highlight == null || highlight === "rehydration"
              const emphasised = highlight === "rehydration"
              return (
                <ReferenceLine
                  key={`rehydration-${point.index}`}
                  className="animate-chart-mark"
                  yAxisId="cost"
                  x={point.index}
                  stroke={lit ? MARK_STROKE.rehydration : REST_MARK_STROKE}
                  strokeWidth={emphasised ? MARK_LIT_STROKE_WIDTH : MARK_STROKE_WIDTH}
                  strokeDasharray={MARK_DASH}
                  strokeLinecap="round"
                />
              )
            })}
        {hasCostData && SHOW_SUBAGENT_LAUNCH_MARKS && (
          <>
            {data
              .filter((point) => point.subagentLaunches > 0)
              .map((point) => {
                const lit = highlight == null || highlight === "subagentLaunch"
                return (
                  <ReferenceDot
                    key={`subagent-${point.index}`}
                    className="animate-chart-mark"
                    yAxisId="cost"
                    x={point.index}
                    y={0}
                    ifOverflow="visible"
                    shape={(props: { cx?: number; cy?: number }) => {
                      const { cx, cy } = props
                      if (cx == null || cy == null) return <g />
                      return (
                        <rect
                          x={cx - SUBAGENT_TICK_WIDTH / 2}
                          y={cy - SUBAGENT_TICK_HEIGHT}
                          width={SUBAGENT_TICK_WIDTH}
                          height={SUBAGENT_TICK_HEIGHT}
                          fill={lit ? "var(--color-token-subagent)" : REST_MARK_STROKE}
                        />
                      )
                    }}
                  />
                )
              })}
          </>
        )}
        {/* Labels draw last of all, so they stay legible over every mark and
            every area. Every labeled mark keeps its label at all times; a
            bar close to the last labeled one shares that label instead of
            overlapping it. Each label takes its line's lit color, or grey
            when another layer is highlighted, so it still names the line. */}
        {hasCostData &&
          data
            .filter((point) => point.isCompactionBoundary && labeledCompaction.has(point.index))
            .map((point) => {
              const lit = highlight == null || highlight === "compaction"
              return (
                <ReferenceLine
                  key={`compaction-label-${point.index}`}
                  className="animate-chart-mark"
                  yAxisId="cost"
                  x={point.index}
                  stroke="none"
                  label={{
                    ...AXIS_LABEL,
                    value: compactionMarkLabel(point),
                    position: "insideTop" as const,
                    angle: -90,
                    fill: lit ? MARK_STROKE.compaction : REST_MARK_STROKE,
                  }}
                />
              )
            })}
        {hasCostData &&
          data
            .filter((point) => point.isCacheRehydration && labeledRehydration.has(point.index))
            .map((point) => {
              const lit = highlight == null || highlight === "rehydration"
              return (
                <ReferenceLine
                  key={`rehydration-label-${point.index}`}
                  className="animate-chart-mark"
                  yAxisId="cost"
                  x={point.index}
                  stroke="none"
                  label={{
                    ...AXIS_LABEL,
                    value: "Rehydration",
                    position: "insideTop" as const,
                    angle: -90,
                    fill: lit ? MARK_STROKE.rehydration : REST_MARK_STROKE,
                  }}
                />
              )
            })}
      </AreaChart>
    </ResponsiveContainer>
  )
}
