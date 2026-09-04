import { useId, useState, type CSSProperties, type ReactElement } from "react"
import {
  Area,
  AreaChart,
  ReferenceLine,
  ResponsiveContainer,
  Text,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts"

import { prefersReducedMotion, slowAnimationDurationMs } from "../../../lib/popoverHeight"
import { modelShortName } from "../../../lib/presentation/models"
import {
  axisScale,
  contextTokenSeries,
  formatCompact,
  formatDuration,
  formatPct,
  formatTokenBand,
  IDLE_GAP_SECS,
  timeAxisTicks,
  modeChangeMarkers,
  sessionModeBaseline,
  type ContextTokenPoint,
  type ModeChangeMarker,
  type SessionModeBaseline,
} from "../../../lib/presentation/sessionAnalysis"
import type { SessionBucket } from "../../../lib/types/session"
import { GLASS_TOOLTIP_STYLE } from "./tooltip"

/**
 * A layer of the plot that a key entry can name.
 *
 * The chart draws grey and takes one layer's color at a time. A reader who
 * points at a key entry sees which part of the plot the figure counts, which
 * a static legend beside a colored chart never shows.
 */
export type ChartSeries =
  "context" | "in" | "out" | "rehydration" | "routingMiss" | "compaction"

export interface ContextTokensChartProps {
  buckets: SessionBucket[]
  /** Null when context occupancy is unavailable for this model. */
  contextWindow: number | null
  /** Active seconds the buckets span; null hides the time marks. */
  activeSecs?: number | null
  /**
   * The layer to draw in its own color. Null draws the context area in its
   * blue and every other layer in grey; a named layer lights that one alone.
   */
  highlight?: ChartSeries | null
}

/** Absolute token level where the context fill turns from calm to warm. */
const WARM_FLOOR_TOKENS = 400_000
/** Token level the warm ramp reaches full red at. */
const CRITICAL_TOKENS = 1_000_000
/** Horizontal padding inside a label pill. */
const PILL_PAD_X = 5
/** Vertical padding above and below the label text inside its pill. */
const PILL_PAD_Y = 2
/** Mean glyph width at the label size, for sizing a pill to its text. */
const PILL_CHAR_WIDTH = 6.1

/**
 * Where the text sits relative to the point recharts computes for each label
 * position. Recharts gives a custom label the point but not the anchors, so
 * the chart states the same anchors the built-in label uses.
 */
const LABEL_ANCHORS: Record<
  string,
  { textAnchor: "start" | "middle"; verticalAnchor: "start" | "end" }
> = {
  insideTopLeft: { textAnchor: "start", verticalAnchor: "start" },
  insideTop: { textAnchor: "middle", verticalAnchor: "start" },
  insideBottom: { textAnchor: "middle", verticalAnchor: "end" },
  top: { textAnchor: "middle", verticalAnchor: "end" },
}

/* Recharts states a label's geometry as string-or-number, so the pill takes
   the same shape and converts once. */
interface PillLabelProps {
  x?: string | number | undefined
  y?: string | number | undefined
  dy?: string | number | undefined
  fontSize?: string | number | undefined
  fill?: string | undefined
  value?: string | number | boolean | null | undefined
  position?: unknown
}

/**
 * A label drawn inside the plot, on a translucent pill. The pill is the
 * opposite of the surface, so the text stays legible over the fill, the
 * line, and the marker bars it can land on.
 */
function PillLabel({
  x,
  y,
  dy = 0,
  fontSize = 11,
  fill,
  value,
  position = "insideTop",
}: PillLabelProps) {
  const originX = Number(x)
  const originY = Number(y)
  const offsetY = Number(dy)
  const size = Number(fontSize)
  if (
    value == null ||
    value === false ||
    !Number.isFinite(originX) ||
    !Number.isFinite(originY)
  ) {
    return null
  }
  const text = String(value)
  const anchors =
    (typeof position === "string" ? LABEL_ANCHORS[position] : undefined) ??
    LABEL_ANCHORS.insideTop!
  const width = text.length * PILL_CHAR_WIDTH + PILL_PAD_X * 2
  const height = size + PILL_PAD_Y * 2
  const left = anchors.textAnchor === "start" ? originX - PILL_PAD_X : originX - width / 2
  // A "start" anchor puts the text's top edge on the point, an "end" anchor
  // puts its bottom edge there.
  const top =
    anchors.verticalAnchor === "start" ? originY - PILL_PAD_Y : originY + PILL_PAD_Y - height
  return (
    <g>
      <rect
        x={left}
        y={top + offsetY}
        width={width}
        height={height}
        rx={height / 2}
        fill="var(--color-chart-label-pill)"
      />
      <Text
        x={originX}
        y={originY + offsetY}
        textAnchor={anchors.textAnchor}
        verticalAnchor={anchors.verticalAnchor}
        fontSize={size}
        fill={fill}
      >
        {text}
      </Text>
    </g>
  )
}

/* Band label text, drawn inside the plot. The size matches the caption
   step of the type scale, which is the legibility floor. */
const AXIS_LABEL = {
  fontSize: 11,
  fill: "var(--color-label-tertiary)",
  content: PillLabel,
}
/** A cache event keeps the established prominent marker. */
const CACHE_EVENT_BAR_WIDTH = 7
/** An ordinary rewrite uses a quiet line, because the user usually cannot prevent it. */
const REWRITE_MARKER_WIDTH = 2
/** Small rewrites stay in the tooltip and do not add chart noise. */
const MATERIAL_REWRITE_TOKENS = 20_000
/** Default opacity for a material rewrite. */
const REWRITE_BAR_OPACITY = 0.9
/** Bar opacity for a routing miss: the same cost, but not avoidable, so the mark draws lighter. */
const ROUTING_MISS_BAR_OPACITY = 0.4
/** The lit stroke of each mark drawn over the plot. Each event has its own hue. */
const MARK_STROKE: Record<"rehydration" | "routingMiss" | "compaction", string> = {
  rehydration: "var(--color-mark-rehydration)",
  routingMiss: "var(--color-mark-routing-miss)",
  compaction: "var(--color-mark-compaction)",
}
/** A compaction mark at rest and lit. It is heavier than a hairline, because it traces the drop. */
const COMPACTION_STROKE_WIDTH = 2.5
const COMPACTION_LIT_STROKE_WIDTH = 3.5
/** Every mark at rest. A mark is a hairline, so it takes the denser grey. */
const REST_MARK_STROKE = "var(--color-chart-rest-mark)"
/** Vertical step between stacked mode-label rows, in pixels. */
const MODE_LABEL_ROW_HEIGHT = 13
/** Nearer than this fraction of the x-domain, two mode labels would collide. */
const MODE_LABEL_MIN_GAP_FRACTION = 0.18

/**
 * Give each mode label a row, so labels close on the x-axis stack instead of
 * overlapping. A label joins the first row with enough room, and opens a new
 * row (up to three) when none has it.
 */
function staggeredModeMarkers(
  markers: ModeChangeMarker[],
  domainMax: number,
): Array<ModeChangeMarker & { row: number }> {
  const minGap = Math.max(1, domainMax) * MODE_LABEL_MIN_GAP_FRACTION
  const lastOnRow: number[] = []
  return markers.map((marker) => {
    let row = lastOnRow.findIndex((last) => marker.index - last >= minGap)
    if (row === -1) row = lastOnRow.length < 3 ? lastOnRow.length : 0
    lastOnRow[row] = marker.index
    return { ...marker, row }
  })
}

/**
 * The rewrite-family points, with a flag for which bars carry a label. Only a
 * cache rehydration is labeled: an ordinary rewrite and a provider cache miss
 * draw a quiet line, because the user usually cannot prevent them. A bar
 * nearer than the mode-label gap to the last labeled bar shares that label, so
 * the labels do not overlap each other.
 */
function labeledRewritePoints(
  data: ContextTokenPoint[],
): Array<{ point: ContextTokenPoint; showLabel: boolean }> {
  const points = data.filter(
    (point) =>
      point.rewriteTokens >= MATERIAL_REWRITE_TOKENS ||
      point.isCacheRehydration ||
      point.isCacheRoutingMiss,
  )
  const minGap = Math.max(1, data.length - 1) * MODE_LABEL_MIN_GAP_FRACTION
  let lastLabeled = Number.NEGATIVE_INFINITY
  return points.map((point) => {
    const showLabel = point.isCacheRehydration && point.index - lastLabeled >= minGap
    if (showLabel) lastLabeled = point.index
    return { point, showLabel }
  })
}

export interface ContextTokensTooltipProps {
  active?: boolean
  contextWindow: number | null
  activeSecs?: number | null
  bucketCount?: number
  /** The session's starting mode. A mode row that matches it is not shown. */
  baseline?: SessionModeBaseline
  payload?: Array<{ payload?: ContextTokenPoint }>
}

const NO_BASELINE: SessionModeBaseline = {
  model: null,
  thinkingMode: null,
  speed: null,
  hasThinking: false,
}

/**
 * Row swatches for the token-series lines. `colorVar` is the lit color, which
 * the tooltip and the chart key always show: the swatch states what a series
 * looks like, and lighting the layer proves it. `restVar` is the grey the
 * layer draws in the rest of the time. The three greys differ by value, so
 * the stack keeps its steps with no color at all. `series` is null for a row
 * the key does not name, which therefore never lights.
 */
const TOKEN_ROWS: Array<{
  key: "tokensIn" | "tokensOut" | "subagentTokens"
  label: string
  colorVar: string
  restVar: string
  series: ChartSeries | null
}> = [
  {
    key: "tokensIn",
    label: "Parent in",
    colorVar: "var(--color-token-in)",
    restVar: "var(--color-chart-rest)",
    series: "in",
  },
  {
    key: "tokensOut",
    label: "Parent out",
    colorVar: "var(--color-token-out)",
    restVar: "var(--color-chart-rest-strong)",
    series: "out",
  },
  {
    key: "subagentTokens",
    label: "Subagents",
    colorVar: "var(--color-token-subagent)",
    restVar: "var(--color-chart-rest-faint)",
    series: null,
  },
]

/**
 * Cache rows are not drawn on the chart, so they get a hollow swatch in the
 * input color: the same family as "Parent in", but not a plotted series.
 * Some vendors report a known zero cache write, so that row hides when it
 * has nothing to say.
 */
const CACHE_ROWS: Array<{
  key: "cacheReadTokens" | "cacheWriteTokens"
  label: string
  colorVar: string
  hideWhenZero: boolean
}> = [
  {
    key: "cacheReadTokens",
    label: "Cache read",
    colorVar: "var(--color-token-in)",
    hideWhenZero: false,
  },
  {
    key: "cacheWriteTokens",
    label: "Cache write",
    colorVar: "var(--color-token-in)",
    hideWhenZero: true,
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
 * The line for a bucket with no model call: the slice sits inside a gap
 * between two calls. The label names what the session waited on: a tool
 * that ran during the gap, or the user when a prompt ended it. A gap at or
 * past the idle cap also shows how much of it the axis draws.
 */
function betweenCallsLabel(gap: NonNullable<ContextTokenPoint["betweenCalls"]>): string {
  let secs = ""
  if (gap.secs != null) {
    secs = ` · ${formatDuration(gap.secs)}`
    if (gap.secs >= IDLE_GAP_SECS) secs += ` (${formatDuration(IDLE_GAP_SECS)} shown)`
  }
  if (gap.tool != null && !gap.userPrompt) return `During ${gap.tool} call${secs}`
  if (gap.userPrompt) return `Waiting for user${secs}`
  return `Between model calls${secs}`
}

/** Describe a rehydration from an analysis that predates its exact breakdown. */
function legacyRehydrationLabel(point: ContextTokenPoint): string {
  if (point.cacheWriteTokens > 0) {
    return `Cache rehydrated · ${formatCompact(point.cacheWriteTokens)} written`
  }
  return `Cache rehydrated · ${formatCompact(point.tokensIn)} re-sent uncached`
}

/**
 * The transport keeps the old routing-miss field name for stored analyses.
 */
function routingMissLabel(point: ContextTokenPoint): string {
  if (point.cacheRehydration != null) {
    return `Provider cache miss · ${formatCompact(point.cacheRehydration.rewrittenTokens)} old context uncached`
  }
  if (point.cacheWriteTokens > 0) {
    return `Provider cache miss · ${formatCompact(point.cacheWriteTokens)} written`
  }
  return `Provider cache miss · ${formatCompact(point.tokensIn)} re-sent uncached`
}

function rewriteLabel(point: ContextTokenPoint): string {
  return `Context rewrite · ${formatCompact(point.rewriteTokens)} re-sent`
}

/**
 * A material rewrite occupies its part of the current context. A legacy cache
 * marker reaches the context level when no rewrite quantity exists.
 *
 * The bar carries no label. SVG paints in document order, so a label drawn
 * with its own bar goes under the token areas that follow it. The labels
 * render last, through `rewriteBarLabel`.
 */
function rewriteBar(
  point: ContextTokenPoint,
  keyPrefix: string,
  hasContextAxis: boolean,
  opacity: number,
  strokeWidth: number,
  stroke: string,
): ReactElement {
  const cacheEvent = point.cacheRehydration
  const start = cacheEvent?.stillCachedTokens ?? 0
  const end = cacheEvent
    ? cacheEvent.stillCachedTokens + cacheEvent.rewrittenTokens
    : point.rewriteTokens || point.contextTokens
  return hasContextAxis ? (
    <ReferenceLine
      key={`${keyPrefix}-${point.index}`}
      className="animate-chart-mark"
      yAxisId="context"
      segment={[
        { x: point.index, y: start },
        { x: point.index, y: end },
      ]}
      stroke={stroke}
      strokeWidth={strokeWidth}
      strokeOpacity={opacity}
    />
  ) : (
    <ReferenceLine
      key={`${keyPrefix}-${point.index}`}
      className="animate-chart-mark"
      yAxisId="tokens"
      x={point.index}
      stroke={stroke}
      strokeWidth={strokeWidth}
      strokeOpacity={opacity}
    />
  )
}

/**
 * The label of a rewrite bar, on the same anchor the bar uses but with no
 * line of its own. The chart renders these after the plot layers, so every
 * label stays legible over the areas.
 */
function rewriteBarLabel(point: ContextTokenPoint, hasContextAxis: boolean): ReactElement {
  const cacheEvent = point.cacheRehydration
  const label = { ...AXIS_LABEL, value: "rehydration", position: "top" as const }
  const end = cacheEvent
    ? cacheEvent.stillCachedTokens + cacheEvent.rewrittenTokens
    : point.rewriteTokens || point.contextTokens
  return hasContextAxis ? (
    <ReferenceLine
      key={`rewrite-label-${point.index}`}
      className="animate-chart-mark"
      yAxisId="context"
      segment={[
        { x: point.index, y: 0 },
        { x: point.index, y: end },
      ]}
      stroke="none"
      label={label}
    />
  ) : (
    <ReferenceLine
      key={`rewrite-label-${point.index}`}
      className="animate-chart-mark"
      yAxisId="tokens"
      x={point.index}
      stroke="none"
      label={label}
    />
  )
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
  baseline = NO_BASELINE,
}: ContextTokensTooltipProps) {
  const point = payload?.[0]?.payload
  if (!active || !point) return null
  const elapsed =
    activeSecs != null && bucketCount > 1
      ? formatDuration((point.index / (bucketCount - 1)) * activeSecs)
      : `${point.progress}% through`
  const cacheEvent = point.cacheRehydration
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
      <div className="mb-1">{elapsed} into session</div>
      <div className="flex flex-col gap-1 text-label-secondary">
        {pct != null && (
          <span>
            Context · {formatCompact(point.contextTokens)} ({formatPct(pct)})
          </span>
        )}
        {point.betweenCalls != null && (
          <span className="mt-1">{betweenCallsLabel(point.betweenCalls)}</span>
        )}
        {/* The token rows always show. Hiding them on a bucket that also had
            an idle gap or a cache event made the reader hover other buckets
            to learn what this one spent. */}
        <span className="mt-1 type-caption text-label-tertiary">Tokens</span>
        {TOKEN_ROWS.map((row) => (
          <span key={row.key} className="flex items-center gap-1.5">
            <span
              className="h-2 w-2 shrink-0 rounded-full"
              style={{ backgroundColor: row.colorVar }}
            />
            {row.label} · {formatCompact(point[row.key])}
          </span>
        ))}
        {CACHE_ROWS.filter((row) => !row.hideWhenZero || point[row.key] > 0).map((row) => (
          <span key={row.key} className="flex items-center gap-1.5">
            <span
              className="h-2 w-2 shrink-0 rounded-full border"
              style={{ borderColor: row.colorVar }}
            />
            {row.label} · {formatCompact(point[row.key])}
          </span>
        ))}
        {point.isCacheRehydration && cacheEvent != null && (
          <>
            <span style={{ color: MARK_STROKE.rehydration }}>
              Cache rehydration · {formatCompact(cacheEvent.contextTokens)} context
            </span>
            <span className="pl-3">
              Still cached · {formatCompact(cacheEvent.stillCachedTokens)}
            </span>
            <span className="pl-3">
              Old context rewritten · {formatCompact(cacheEvent.rewrittenTokens)}
            </span>
            <span className="pl-3">
              Context growth · {formatCompact(cacheEvent.growthTokens)}
            </span>
          </>
        )}
        {point.isCacheRehydration && cacheEvent == null && (
          <span style={{ color: MARK_STROKE.rehydration }}>
            {legacyRehydrationLabel(point)}
          </span>
        )}
        {point.isCacheRoutingMiss && (
          <span style={{ color: MARK_STROKE.routingMiss }}>{routingMissLabel(point)}</span>
        )}
        {point.rewriteTokens > 0 && cacheEvent == null && (
          <span style={{ color: "var(--color-context-warning)" }}>{rewriteLabel(point)}</span>
        )}
        {cacheEvent?.userInactiveSecs != null && (
          <span>User inactive · {formatDuration(cacheEvent.userInactiveSecs)}</span>
        )}
        {point.secsSincePriorTurn != null && cacheEvent?.userInactiveSecs == null && (
          <span>Since prior turn · {formatDuration(point.secsSincePriorTurn)}</span>
        )}
        {point.isCompactionBoundary && (
          <span style={{ color: MARK_STROKE.compaction }}>{compactionLabel(point)}</span>
        )}
        {point.subagentLaunches > 0 && (
          <span>Subagents launched · {point.subagentLaunches}</span>
        )}
        {point.model != null && point.model !== baseline.model && (
          <span>Model · {modelShortName(point.model)}</span>
        )}
        {point.thinkingMode != null && point.thinkingMode !== baseline.thinkingMode && (
          <span>Effort · {point.thinkingMode}</span>
        )}
        {point.speed != null && point.speed !== baseline.speed && (
          <span>Speed · {point.speed}</span>
        )}
        {point.hasThinking && !baseline.hasThinking && <span>Thinking</span>}
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
  highlight = null,
}: ContextTokensChartProps) {
  const data = contextTokenSeries(buckets)
  const fillId = `context-tokens-fill-${useId().replace(/:/g, "")}`
  const [initialBuckets] = useState(() => buckets)
  const animate = !prefersReducedMotion()
  const animationDurationMs = slowAnimationDurationMs()
  // The first paint plays the session back in order: the context fill grows,
  // the token spikes follow it, and the rewrite marks land last, on top of a
  // chart that has finished drawing. A later bucket set comes from the live
  // poll, where a staggered replay would read as the panel redrawing itself,
  // so those updates animate together.
  const entranceStepMs = buckets === initialBuckets ? Math.round(animationDurationMs / 2) : 0
  const tokenRowStepMs = Math.round(entranceStepMs / 2)
  const markDelayMs =
    entranceStepMs === 0
      ? 0
      : entranceStepMs + TOKEN_ROWS.length * tokenRowStepMs + animationDurationMs

  const peak = data.reduce((m, d) => Math.max(m, d.contextTokens), 0)
  const tokenPeak = data.reduce(
    (m, d) => Math.max(m, d.tokensIn + d.tokensOut + d.subagentTokens),
    0,
  )
  // The largest spike reaches the top of the plot, so the token layer keeps
  // its full range of variation. Its low alpha keeps it secondary.
  const tokenCeiling = Math.max(1, tokenPeak)
  // The context area draws whenever the session has context to show. The
  // window only places the band marks. A model with an unknown window still
  // has a context curve, and dropping that curve left the plot with the token
  // spikes alone and no main shape.
  const hasContextData = peak > 0
  const contextAxis = contextWindow != null ? axisScale(peak, contextWindow, 5) : null
  // With no window to scale against, the peak is the top of the plot, so the
  // curve fills the same height it would inside a known window.
  const contextCeiling = contextAxis?.ceiling ?? Math.max(1, peak)
  // Every vertical `ReferenceLine` needs a `yAxisId` that names an axis the
  // chart renders — recharts falls back to an axis id of "0", which does not
  // exist here. The "tokens" axis always renders, so it is the fallback.
  const markerAxisId = hasContextData ? "context" : "tokens"

  // The fill gradient is an SVG `objectBoundingBox` gradient, so its [0,1]
  // offsets map over the *area path's* bounding box, which spans 0..peak
  // tokens, not the fixed context window. Offset f sits at the absolute
  // token value peak·(1−f). The warm ramp is in absolute tokens, not a
  // fraction of the window, so a 1M-window session and a 200k-window session
  // both turn warm at the same 400k mark. Below 400k the fill stays the calm
  // grey; from 400k up it ramps from amber to red, reaching red at 1M tokens
  // regardless of the window size.
  // The context area is the main shape, so it keeps its blue at rest and
  // gives it up only while another layer is lit. Pointing at the context key
  // entry adds the warm ramp, which says how deep the session ran into its
  // window.
  const contextLit = highlight === "context"
  const contextGrey = highlight != null && !contextLit
  const stops: ReactElement[] = []
  if (contextGrey) {
    stops.push(
      <stop key="rest-top" offset={0} stopColor="var(--color-context-rest-top)" />,
      <stop key="rest-base" offset={1} stopColor="var(--color-context-rest-base)" />,
    )
  } else {
    if (contextLit && peak > WARM_FLOOR_TOKENS) {
      const kinkOffset = (peak - WARM_FLOOR_TOKENS) / peak
      const t = Math.min(
        1,
        Math.max(0, (peak - WARM_FLOOR_TOKENS) / (CRITICAL_TOKENS - WARM_FLOOR_TOKENS)),
      )
      const topColor = `color-mix(in oklch, var(--color-context-warning), var(--color-context-warning) ${Math.round(t * 100)}%)`
      stops.push(
        <stop key="warm-top" offset={0} stopColor={topColor} stopOpacity={0.55} />,
        <stop
          key="warm-edge"
          offset={kinkOffset}
          stopColor="var(--color-context-warning)"
          stopOpacity={0.55}
        />,
        <stop
          key="healthy-edge"
          offset={kinkOffset}
          stopColor="var(--color-context-fill-top)"
        />,
      )
    } else {
      stops.push(
        <stop key="healthy-edge" offset={0} stopColor="var(--color-context-fill-top)" />,
      )
    }
    stops.push(
      <stop key="healthy-base" offset={1} stopColor="var(--color-context-fill-base)" />,
    )
  }

  return (
    <ResponsiveContainer
      width="100%"
      height={220}
      style={{ "--chart-mark-delay": `${markDelayMs}ms` } as CSSProperties}
    >
      <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
        <defs>
          {hasContextData && (
            <linearGradient id={fillId} x1={0} y1={0} x2={0} y2={1}>
              {stops}
            </linearGradient>
          )}
        </defs>
        {/* A numeric axis on the bucket index. A category axis on the rounded
            `progress` value placed each vertical mark by its index instead of
            by its value, so marks drifted left of the points they belong to. */}
        <XAxis dataKey="index" type="number" domain={[0, Math.max(1, data.length - 1)]} hide />
        {hasContextData && <YAxis yAxisId="context" hide domain={[0, contextCeiling]} />}
        <YAxis yAxisId="tokens" hide orientation="right" domain={[0, tokenCeiling]} />
        {/* The band lines only. Their labels draw after the plot layers. */}
        {contextAxis?.ticks.map((value) => (
          <ReferenceLine
            key={`band-${value}`}
            yAxisId="context"
            y={value}
            stroke="var(--color-separator)"
            strokeDasharray="2 4"
          />
        ))}
        {/* A material rewrite draws a quiet line over the part of the
            context it re-sent. A cache event keeps the wider marker, and a
            routing miss draws lighter. */}
        {labeledRewritePoints(data).map(({ point }) => {
          const opacity = point.isCacheRoutingMiss
            ? ROUTING_MISS_BAR_OPACITY
            : REWRITE_BAR_OPACITY
          const strokeWidth = point.isCacheRehydration
            ? CACHE_EVENT_BAR_WIDTH
            : REWRITE_MARKER_WIDTH
          const markSeries: ChartSeries | null = point.isCacheRoutingMiss
            ? "routingMiss"
            : point.isCacheRehydration
              ? "rehydration"
              : null
          const stroke =
            markSeries != null && highlight === markSeries
              ? MARK_STROKE[markSeries]
              : REST_MARK_STROKE
          return rewriteBar(point, "rewrite", hasContextData, opacity, strokeWidth, stroke)
        })}
        <Tooltip
          cursor={{ stroke: "var(--color-separator)" }}
          /* No animation: the tooltip re-enters on every bucket the pointer
             crosses, and the replayed entry reads as the panel jumping. */
          isAnimationActive={false}
          content={
            <ContextTokensTooltip
              contextWindow={contextWindow}
              activeSecs={activeSecs}
              bucketCount={data.length}
              baseline={sessionModeBaseline(data)}
            />
          }
        />
        {hasContextData && (
          <Area
            yAxisId="context"
            type="monotone"
            dataKey="contextTokens"
            stroke={
              contextGrey ? "var(--color-chart-rest-strong)" : "var(--color-context-stroke)"
            }
            /* A fine line: the color carries the mark without weight. */
            strokeWidth={1.5}
            fill={`url(#${fillId})`}
            isAnimationActive={animate}
            animationDuration={animationDurationMs}
            animationBegin={0}
            animationEasing="ease-out"
          />
        )}
        {/* The token spikes draw after the context fill, so their color sits
            on top of it and does not dull under the grey. */}
        {TOKEN_ROWS.map((row, index) => (
          <Area
            key={row.key}
            yAxisId="tokens"
            type="monotone"
            dataKey={row.key}
            stackId="t"
            /* A solid block. A gradient made each layer fade into the one
               below it, so the stack lost its steps. */
            stroke="none"
            fill={row.series != null && highlight === row.series ? row.colorVar : row.restVar}
            isAnimationActive={animate}
            animationDuration={animationDurationMs}
            animationBegin={entranceStepMs + index * tokenRowStepMs}
            animationEasing="ease-out"
          />
        ))}
        {/* A compaction traces the drop in the context area, from the last
            level before the boundary to the first level after it. It draws
            over the fills, so the stroke stays whole where the area thins. */}
        {hasContextData &&
          data
            .filter((point) => point.isCompactionBoundary && point.index > 0)
            .map((point) => {
              const lit = highlight === "compaction"
              return (
                <ReferenceLine
                  key={`compaction-${point.index}`}
                  className="animate-chart-mark"
                  data-series="compaction"
                  yAxisId="context"
                  segment={[
                    { x: point.index - 1, y: data[point.index - 1]!.contextTokens },
                    { x: point.index, y: point.contextTokens },
                  ]}
                  stroke={lit ? MARK_STROKE.compaction : REST_MARK_STROKE}
                  strokeWidth={lit ? COMPACTION_LIT_STROKE_WIDTH : COMPACTION_STROKE_WIDTH}
                  strokeLinecap="round"
                />
              )
            })}
        {/* Every text label draws last. SVG has no z-index, so document order
            is the only way to keep a label over the areas it annotates. Each
            of these marks carries a label and no line of its own; the lines
            they belong to draw before the plot layers. */}
        {contextAxis?.ticks.map((value) => (
          <ReferenceLine
            key={`band-label-${value}`}
            yAxisId="context"
            y={value}
            stroke="none"
            label={{ ...AXIS_LABEL, value: formatTokenBand(value), position: "insideTopLeft" }}
          />
        ))}
        {/* Bars close on the x-axis share one "rewrite" label, so the labels
            do not overlap each other. */}
        {labeledRewritePoints(data)
          .filter(({ showLabel }) => showLabel)
          .map(({ point }) => rewriteBarLabel(point, hasContextData))}
        {/* Elapsed active time along the bottom, as labels only. */}
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
        {/* A mode change (model, thinking effort, or speed) draws no line at
            all — only its label, at the top of the plot — so it stays a
            calm annotation rather than another vertical mark competing with
            compaction and rewrite markers. */}
        {staggeredModeMarkers(modeChangeMarkers(data), data.length - 1).map((marker) => (
          <ReferenceLine
            key={`mode-${marker.index}`}
            yAxisId={markerAxisId}
            x={marker.index}
            stroke="none"
            label={{
              ...AXIS_LABEL,
              value: marker.label,
              position: "insideTop",
              dy: marker.row * MODE_LABEL_ROW_HEIGHT,
            }}
          />
        ))}
      </AreaChart>
    </ResponsiveContainer>
  )
}
