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

import {
  contextBandValues,
  contextTokenSeries,
  formatCompact,
  formatPct,
  formatTokenBand,
  type ContextTokenPoint,
} from "../../../lib/presentation/sessionAnalytics"
import type { SessionBucket } from "../../../lib/types/session"
import { GLASS_TOOLTIP_STYLE } from "./tooltip"

export interface ContextTokensChartProps {
  buckets: SessionBucket[]
  /** Null when context occupancy is unavailable for this model. */
  contextWindow: number | null
}

/** Absolute token level where the context fill turns from calm to warm. */
const WARM_FLOOR_TOKENS = 400_000
/** Token level the warm ramp reaches full red at. */
const CRITICAL_TOKENS = 1_000_000

interface ContextTokensTooltipProps {
  active?: boolean
  contextWindow: number | null
  payload?: Array<{ payload?: ContextTokenPoint }>
}

/** Row swatches for the token-series lines, matching the chart's fill colors. */
const TOKEN_ROWS: Array<{ key: "tokensIn" | "tokensOut"; label: string; colorVar: string }> = [
  { key: "tokensIn", label: "In", colorVar: "var(--color-token-in)" },
  { key: "tokensOut", label: "Out", colorVar: "var(--color-token-out)" },
]

/** Custom tooltip: progress, context depth, token in/out, and compaction. */
function ContextTokensTooltip({ active, payload, contextWindow }: ContextTokensTooltipProps) {
  const point = payload?.[0]?.payload
  if (!active || !point) return null
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
      <div className="mb-1">{point.progress}% through</div>
      <div className="flex flex-col gap-1 text-label-secondary">
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
        {point.isCompactionBoundary && <span>Compaction</span>}
      </div>
    </div>
  )
}

/**
 * Merged context-and-token chart: context-window fullness is the primary
 * area, input/output tokens per slice sit behind it as a faint secondary
 * layer, on one shared progress axis.
 */
export function ContextTokensChart({ buckets, contextWindow }: ContextTokensChartProps) {
  const data = contextTokenSeries(buckets)
  const fillId = `context-tokens-fill-${useId().replace(/:/g, "")}`
  const hasContext = contextWindow != null

  const tokenSum = data.reduce((m, d) => Math.max(m, d.tokensIn + d.tokensOut), 0)
  const tokenCeiling = Math.max(1, tokenSum * 3)
  const bands = hasContext ? contextBandValues(contextWindow) : []
  // Every `ReferenceLine`, including the vertical compaction markers, needs a
  // `yAxisId` that names an axis the chart actually renders — recharts falls
  // back to an axis id of "0", which does not exist here. The chart always
  // renders the "tokens" axis, so that is the fallback when there is no
  // "context" axis to use instead.
  const compactionAxisId = hasContext ? "context" : "tokens"

  // The fill gradient is an SVG `objectBoundingBox` gradient, so its [0,1]
  // offsets map over the *area path's* bounding box, which spans 0..peak
  // tokens, not the fixed context window. Offset f sits at the absolute
  // token value peak·(1−f). The warm ramp is in absolute tokens, not a
  // fraction of the window, so a 1M-window session and a 200k-window session
  // both turn warm at the same 400k mark. Below 400k the fill stays the calm
  // blue; from 400k up it ramps from amber to red, reaching red at 1M tokens
  // regardless of the window size.
  const peak = data.reduce((m, d) => Math.max(m, d.contextTokens), 0)
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
        <XAxis dataKey="progress" hide />
        {hasContext && <YAxis yAxisId="context" hide domain={[0, contextWindow]} />}
        <YAxis yAxisId="tokens" hide orientation="right" domain={[0, tokenCeiling]} />
        {hasContext &&
          bands.map((value) => (
            <ReferenceLine
              key={`band-${value}`}
              yAxisId="context"
              y={value}
              stroke="var(--color-separator)"
              strokeDasharray="2 4"
              label={{
                value: formatTokenBand(value),
                position: "insideTopRight",
                fill: "var(--color-label-tertiary)",
                fontSize: 9,
              }}
            />
          ))}
        {data
          .filter((point) => point.isCompactionBoundary)
          .map((point, i) => (
            <ReferenceLine
              // `progress` is a rounded percentage, so two compactions can
              // land on the same value in a long session; the index keeps
              // sibling keys unique regardless.
              key={`compaction-${i}-${point.progress}`}
              yAxisId={compactionAxisId}
              x={point.progress}
              stroke="var(--color-label-tertiary)"
              strokeDasharray="2 2"
              strokeOpacity={0.8}
            />
          ))}
        <Tooltip
          cursor={{ stroke: "var(--color-separator)" }}
          content={<ContextTokensTooltip contextWindow={contextWindow} />}
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
