// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useId, type ReactElement } from "react"
import {
  Area,
  AreaChart,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts"

import { modelShortName } from "../../../lib/presentation/models"
import {
  axisScale,
  contextTokenSeries,
  formatCompact,
  formatDuration,
  formatPct,
  formatTokenBand,
  timeAxisTicks,
  modeChangeMarkers,
  type ContextTokenPoint,
} from "../../../lib/presentation/sessionAnalytics"
import type { SessionBucket } from "../../../lib/types/session"
import { GLASS_TOOLTIP_STYLE } from "./tooltip"

export interface ContextTokensChartProps {
  buckets: SessionBucket[]
  /** Null when context occupancy is unavailable for this model. */
  contextWindow: number | null
  /** Active seconds the buckets span; null hides the time marks. */
  activeSecs?: number | null
}

/** Absolute token level where the context fill turns from calm to warm. */
const WARM_FLOOR_TOKENS = 400_000
/** Token level the warm ramp reaches full red at. */
const CRITICAL_TOKENS = 1_000_000
/** Band label text, drawn inside the plot. */
const AXIS_LABEL = { fontSize: 9, fill: "var(--color-label-tertiary)" }
// A rehydration bar is a few pixels wide on each side of its bucket so it
// reads as a block of cost, not a hairline.
const REHYDRATION_BAR_WIDTH = 6

export interface ContextTokensTooltipProps {
  active?: boolean
  contextWindow: number | null
  activeSecs?: number | null
  bucketCount?: number
  payload?: Array<{ payload?: ContextTokenPoint }>
}

/** Row swatches for the token-series lines, matching the chart's fill colors. */
const TOKEN_ROWS: Array<{
  key: "tokensIn" | "tokensOut" | "subagentTokens"
  label: string
  colorVar: string
}> = [
  { key: "tokensIn", label: "Parent in", colorVar: "var(--color-token-in)" },
  { key: "tokensOut", label: "Parent out", colorVar: "var(--color-token-out)" },
  {
    key: "subagentTokens",
    label: "Subagents",
    colorVar: "var(--color-token-subagent)",
  },
]

/**
 * The compaction tooltip line: names the trigger when known, and the
 * before/after size when the transcript records it. `postTokens` is absent
 * on some older Claude records, so that half falls back to "before" only.
 */
function compactionLabel(point: ContextTokenPoint): string {
  const trigger =
    point.compactionTrigger === "manual"
      ? "Compaction (manual)"
      : point.compactionTrigger === "auto"
        ? "Compaction (auto)"
        : "Compaction"
  if (point.compactionPreTokens == null) return trigger
  const pre = formatCompact(point.compactionPreTokens)
  if (point.compactionPostTokens == null) return `${trigger} · ${pre} before`
  return `${trigger} · ${pre} → ${formatCompact(point.compactionPostTokens)}`
}

/**
 * The custom tooltip shows active time, context depth, token throughput,
 * compaction, and sub-agent launches. Tests can render it with a fixed payload.
 */
export function ContextTokensTooltip({
  active,
  payload,
  contextWindow,
  activeSecs = null,
  bucketCount = 0,
}: ContextTokensTooltipProps) {
  const point = payload?.[0]?.payload
  if (!active || !point) return null
  const elapsed =
    activeSecs != null && bucketCount > 1
      ? formatDuration((point.index / (bucketCount - 1)) * activeSecs)
      : `${point.progress}% through`
  const pct =
    contextWindow != null && contextWindow > 0
      ? Math.min(1, point.contextTokens / contextWindow)
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
      <div className="mb-1">{elapsed}</div>
      <div className="flex flex-col gap-1 text-label-secondary">
        {point.secsSincePriorTurn != null && (
          <span>Since prior turn · {formatDuration(point.secsSincePriorTurn)}</span>
        )}
        {pct != null && (
          <span>
            Context · {formatCompact(point.contextTokens)} ({formatPct(pct)})
          </span>
        )}
        {TOKEN_ROWS.map((row) => (
          <span key={row.key} className="flex items-center gap-1.5">
            <span
              className="h-2 w-2 shrink-0 rounded-full"
              style={{ backgroundColor: row.colorVar }}
            />
            {row.label} · {formatCompact(point[row.key])}
          </span>
        ))}
        <span>Cache read · {formatCompact(point.cacheReadTokens)}</span>
        <span>Cache write · {formatCompact(point.cacheWriteTokens)}</span>
        {point.isCacheRehydration && (
          <span style={{ color: "var(--color-context-critical)" }}>
            Cache rehydrated · {formatCompact(point.cacheWriteTokens)} written
          </span>
        )}
        {point.isCompactionBoundary && <span>{compactionLabel(point)}</span>}
        {point.subagentLaunches > 0 && (
          <span>Subagents launched · {point.subagentLaunches}</span>
        )}
        {point.model != null && <span>Model · {modelShortName(point.model)}</span>}
        {point.thinkingMode != null && <span>Effort · {point.thinkingMode}</span>}
        {point.speed != null && <span>Speed · {point.speed}</span>}
        {point.hasThinking && <span>Thinking</span>}
      </div>
    </div>
  )
}

/**
 * Merged context-and-token chart: context-window fullness is the primary
 * area, input/output tokens per slice sit behind it as a faint secondary
 * layer, on one shared progress axis.
 */
export function ContextTokensChart({
  buckets,
  contextWindow,
  activeSecs = null,
}: ContextTokensChartProps) {
  const data = contextTokenSeries(buckets)
  const fillId = `context-tokens-fill-${useId().replace(/:/g, "")}`
  const hasContext = contextWindow != null

  const peak = data.reduce((m, d) => Math.max(m, d.contextTokens), 0)
  const tokenPeak = data.reduce(
    (m, d) => Math.max(m, d.tokensIn + d.tokensOut + d.subagentTokens),
    0,
  )
  // The largest spike reaches the top of the plot, so the token layer keeps
  // its full range of variation. Its low alpha keeps it secondary.
  const tokenCeiling = Math.max(1, tokenPeak)
  const contextAxis = hasContext ? axisScale(peak, contextWindow, 5) : null
  // Every vertical `ReferenceLine` needs a `yAxisId` that names an axis the
  // chart renders — recharts falls back to an axis id of "0", which does not
  // exist here. The "tokens" axis always renders, so it is the fallback.
  const markerAxisId = hasContext ? "context" : "tokens"

  // The fill gradient is an SVG `objectBoundingBox` gradient, so its [0,1]
  // offsets map over the *area path's* bounding box, which spans 0..peak
  // tokens, not the fixed context window. Offset f sits at the absolute
  // token value peak·(1−f). The warm ramp is in absolute tokens, not a
  // fraction of the window, so a 1M-window session and a 200k-window session
  // both turn warm at the same 400k mark. Below 400k the fill stays the calm
  // blue; from 400k up it ramps from amber to red, reaching red at 1M tokens
  // regardless of the window size.
  const stops: ReactElement[] = []
  if (peak > WARM_FLOOR_TOKENS) {
    const kinkOffset = (peak - WARM_FLOOR_TOKENS) / peak
    const t = Math.min(
      1,
      Math.max(0, (peak - WARM_FLOOR_TOKENS) / (CRITICAL_TOKENS - WARM_FLOOR_TOKENS)),
    )
    const topColor = `color-mix(in oklch, var(--color-context-warning), var(--color-context-critical) ${Math.round(t * 100)}%)`
    stops.push(
      <stop key="warm-top" offset={0} stopColor={topColor} stopOpacity={0.55} />,
      <stop
        key="warm-edge"
        offset={kinkOffset}
        stopColor="var(--color-context-warning)"
        stopOpacity={0.55}
      />,
      <stop key="healthy-edge" offset={kinkOffset} stopColor="var(--color-context-fill-top)" />,
    )
  } else {
    stops.push(<stop key="healthy-edge" offset={0} stopColor="var(--color-context-fill-top)" />)
  }
  stops.push(<stop key="healthy-base" offset={1} stopColor="var(--color-context-fill-base)" />)

  return (
    <ResponsiveContainer width="100%" height={160}>
      <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
        {hasContext && (
          <defs>
            <linearGradient id={fillId} x1={0} y1={0} x2={0} y2={1}>
              {stops}
            </linearGradient>
          </defs>
        )}
        {/* A numeric axis on the bucket index. A category axis on the rounded
            `progress` value placed each vertical mark by its index instead of
            by its value, so marks drifted left of the points they belong to. */}
        <XAxis dataKey="index" type="number" domain={[0, Math.max(1, data.length - 1)]} hide />
        {contextAxis && <YAxis yAxisId="context" hide domain={[0, contextAxis.ceiling]} />}
        <YAxis yAxisId="tokens" hide orientation="right" domain={[0, tokenCeiling]} />
        {/* Elapsed active time along the bottom, as labels only. The token
            spikes sit behind them, so no line is drawn. */}
        {activeSecs != null &&
          timeAxisTicks(activeSecs, data.length, 6).map((tick) => (
            <ReferenceLine
              key={`time-${tick.label}`}
              yAxisId={markerAxisId}
              x={tick.index}
              stroke="none"
              label={{ ...AXIS_LABEL, value: tick.label, position: "insideBottom" }}
            />
          ))}
        {contextAxis?.ticks.map((value) => (
          <ReferenceLine
            key={`band-${value}`}
            yAxisId="context"
            y={value}
            stroke="var(--color-separator)"
            strokeDasharray="2 4"
            label={{ ...AXIS_LABEL, value: formatTokenBand(value), position: "insideTopLeft" }}
          />
        ))}
        {/* A compaction draws no mark: the drop in the context area shows it,
            and the tooltip names it. A cache rehydration draws a wide red bar
            from the baseline up to the context level at that time, because
            the area shows no change for it. The bar height scales its weight:
            a rehydration of a small context costs little and shows little. */}
        {data
          .filter((point) => point.isCacheRehydration)
          .map((point) =>
            contextAxis ? (
              <ReferenceLine
                key={`cache-rehydration-${point.index}`}
                yAxisId="context"
                segment={[
                  { x: point.index, y: 0 },
                  { x: point.index, y: point.contextTokens },
                ]}
                stroke="var(--color-context-critical)"
                strokeWidth={REHYDRATION_BAR_WIDTH}
                strokeOpacity={0.6}
                label={{ ...AXIS_LABEL, value: "cache", position: "top" }}
              />
            ) : (
              <ReferenceLine
                key={`cache-rehydration-${point.index}`}
                yAxisId="tokens"
                x={point.index}
                stroke="var(--color-context-critical)"
                strokeWidth={REHYDRATION_BAR_WIDTH}
                strokeOpacity={0.6}
                label={{ ...AXIS_LABEL, value: "cache", position: "top" }}
              />
            ),
          )}
        {/* A mode change (model, thinking effort, or speed) draws no line at
            all — only its label, at the top of the plot — so it stays a
            calm annotation rather than another vertical mark competing with
            compaction and cache-rehydration. */}
        {modeChangeMarkers(data).map((marker) => (
          <ReferenceLine
            key={`mode-${marker.index}`}
            yAxisId={markerAxisId}
            x={marker.index}
            stroke="none"
            label={{ ...AXIS_LABEL, value: marker.label, position: "insideTop" }}
          />
        ))}
        <Tooltip
          cursor={{ stroke: "var(--color-separator)" }}
          content={
            <ContextTokensTooltip
              contextWindow={contextWindow}
              activeSecs={activeSecs}
              bucketCount={data.length}
            />
          }
        />
        <Area
          yAxisId="tokens"
          type="monotone"
          dataKey="tokensIn"
          stackId="t"
          stroke="none"
          fill="var(--color-token-in)"
          fillOpacity={0.22}
          isAnimationActive={false}
        />
        <Area
          yAxisId="tokens"
          type="monotone"
          dataKey="tokensOut"
          stackId="t"
          stroke="none"
          fill="var(--color-token-out)"
          fillOpacity={0.22}
          isAnimationActive={false}
        />
        <Area
          yAxisId="tokens"
          type="monotone"
          dataKey="subagentTokens"
          stackId="t"
          stroke="none"
          fill="var(--color-token-subagent)"
          fillOpacity={0.22}
          isAnimationActive={false}
        />
        {hasContext && (
          <Area
            yAxisId="context"
            type="monotone"
            dataKey="contextTokens"
            stroke="var(--color-token-in)"
            strokeWidth={1.5}
            fill={`url(#${fillId})`}
            isAnimationActive={false}
          />
        )}
      </AreaChart>
    </ResponsiveContainer>
  )
}
