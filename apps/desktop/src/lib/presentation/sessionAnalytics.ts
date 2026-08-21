// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shaping and formatting for the session-analytics surface.
 *
 * Pure functions over the analysis summary the local engine produces, with no
 * React, no IPC, and no state — the charts consume the output, and these can be
 * tested (and reused) on their own.
 */

import type {
  ActiveSessionsSummary,
  InitialContextBreakdown,
  InitialContextSource,
  SessionBucket,
} from "../types/session"
import { modelShortName } from "./models"

/* -------------------------------------------------------------------------
 * Buckets and chart series
 * ---------------------------------------------------------------------- */

export interface ContextTokenPoint {
  /** Bucket position; the chart's x value, so marks and areas share one scale. */
  index: number
  /** Rounded percentage through the session, for labels. */
  progress: number
  contextTokens: number
  tokensIn: number
  tokensOut: number
  isCompactionBoundary: boolean
  cacheReadTokens: number
  cacheWriteTokens: number
  isCacheRehydration: boolean
  subagentLaunches: number
  /** Model that produced this point, forward-filled from the last bucket that named one. */
  model: string | null
  /** Thinking-effort mode at this point, forward-filled the same way. */
  thinkingMode: string | null
  /** Response speed at this point, forward-filled the same way. */
  speed: string | null
  /** True when this bucket itself carries a thinking block (not forward-filled). */
  hasThinking: boolean
}

/**
 * Merged context-and-token series over session progress, one point per
 * bucket. This keeps every bucket, including empty ones, so the context area
 * and the token areas share one x-axis grid and cannot drift apart.
 *
 * Context is a level, not a rate. The engine records the largest usage it
 * observes in a bucket, so a bucket with only tool events holds zero. The
 * series carries the last observed level across such buckets. A compaction
 * bucket is a reset, but the new level is unknown until the next model call,
 * so the series fills the compaction bucket and the empty buckets after it
 * with the next observed level. The line then falls at the mark, not to zero.
 */
export function contextTokenSeries(buckets: SessionBucket[]): ContextTokenPoint[] {
  const levels: number[] = []
  let held = 0
  let afterCompaction = false
  for (const bucket of buckets) {
    if (bucket.isCompactionBoundary) {
      afterCompaction = true
      held = 0
    } else if (bucket.contextTokens > 0) {
      afterCompaction = false
      held = bucket.contextTokens
    }
    // NaN marks a bucket that waits for the next observation.
    levels.push(afterCompaction ? Number.NaN : held)
  }
  let next = 0
  for (let i = levels.length - 1; i >= 0; i--) {
    if (Number.isNaN(levels[i])) levels[i] = next
    else next = levels[i]!
  }
  const models = forwardFillMode(buckets, (bucket) => bucket.model)
  const thinkingModes = forwardFillMode(buckets, (bucket) => bucket.thinkingMode)
  const speeds = forwardFillMode(buckets, (bucket) => bucket.speed)
  return buckets.map((bucket, index) => ({
    index,
    progress: Math.round((index / Math.max(1, buckets.length - 1)) * 100),
    contextTokens: levels[index]!,
    tokensIn: bucket.tokensIn,
    tokensOut: bucket.tokensOut,
    isCompactionBoundary: bucket.isCompactionBoundary,
    cacheReadTokens: bucket.cacheReadTokens,
    cacheWriteTokens: bucket.cacheWriteTokens,
    isCacheRehydration: bucket.isCacheRehydration,
    subagentLaunches: bucket.subagentLaunches,
    model: models[index]!,
    thinkingMode: thinkingModes[index]!,
    speed: speeds[index]!,
    hasThinking: bucket.hasThinking,
  }))
}

/**
 * Carry a per-bucket mode value forward: a bucket with no value keeps the
 * last one observed, so the mode reads as persisting until it changes. `null`
 * until the first bucket that names a value.
 */
function forwardFillMode(
  buckets: SessionBucket[],
  pick: (bucket: SessionBucket) => string | null,
): (string | null)[] {
  let held: string | null = null
  return buckets.map((bucket) => {
    const value = pick(bucket)
    if (value !== null) held = value
    return held
  })
}

export interface ModeChangeMarker {
  /** Bucket index the change lands on, matching `ContextTokenPoint.index`. */
  index: number
  /** Compact label, e.g. `"opus → sonnet"`, `"effort high"`, `"fast"`. */
  label: string
}

/**
 * One marker per bucket where the model, thinking mode, or speed changes from
 * the previous non-null value of that same field. The first bucket that names
 * a field's value is the session's starting mode, not a change, so it draws
 * no marker for that field — the tooltip shows it instead. The one exception:
 * "fast" speed at the very start still gets a marker, since fast mode is easy
 * to miss otherwise. Several fields changing in the same bucket join into one
 * label with " · ".
 */
export function modeChangeMarkers(points: ContextTokenPoint[]): ModeChangeMarker[] {
  const markers: ModeChangeMarker[] = []
  let seenModel = false
  let seenThinkingMode = false
  let seenSpeed = false
  let prevModel: string | null = null
  let prevThinkingMode: string | null = null
  let prevSpeed: string | null = null

  for (const point of points) {
    const parts: string[] = []

    if (point.model !== null) {
      if (seenModel && point.model !== prevModel) {
        parts.push(`${modelShortName(prevModel!)} → ${modelShortName(point.model)}`)
      }
      prevModel = point.model
      seenModel = true
    }

    if (point.thinkingMode !== null) {
      if (seenThinkingMode && point.thinkingMode !== prevThinkingMode) {
        parts.push(`effort ${point.thinkingMode}`)
      }
      prevThinkingMode = point.thinkingMode
      seenThinkingMode = true
    }

    if (point.speed !== null) {
      if (seenSpeed && point.speed !== prevSpeed) {
        parts.push(point.speed)
      } else if (!seenSpeed && point.speed === "fast") {
        parts.push("fast")
      }
      prevSpeed = point.speed
      seenSpeed = true
    }

    if (parts.length > 0) markers.push({ index: point.index, label: parts.join(" · ") })
  }

  return markers
}

/** Candidate axis steps, from fine to coarse. */
const AXIS_STEPS = [
  1_000, 2_000, 5_000, 10_000, 20_000, 25_000, 50_000, 100_000, 200_000, 250_000, 500_000,
  1_000_000,
]

/** One chart axis: its top value and the clean values it is marked at. */
export interface AxisScale {
  /** Top of the axis, in tokens. */
  ceiling: number
  /** Marked values strictly between zero and the ceiling. */
  ticks: number[]
}

/**
 * Scale an axis to the data, not to a fixed range. The ceiling is `peak`
 * plus headroom, rounded up to the finest clean step that yields at most
 * `maxTicks` marks, and never above `cap`. A session that used 130k of a 1M
 * window then fills the chart instead of a thin strip at the bottom.
 */
export function axisScale(peak: number, cap: number, maxTicks: number): AxisScale {
  const target = Math.min(cap, Math.max(1, peak) * 1.1)
  const step =
    AXIS_STEPS.find((s) => target / s <= maxTicks) ?? AXIS_STEPS[AXIS_STEPS.length - 1]!
  const ceiling = Math.min(cap, Math.ceil(target / step) * step)
  const ticks: number[] = []
  for (let v = step; v < ceiling; v += step) ticks.push(v)
  return { ceiling, ticks }
}

export interface ToolSlice {
  key: string
  label: string
  value: number
  colorVar: string
}

/** Tool-mix donut slices. */
export function toolMixSlices(summary: ActiveSessionsSummary): ToolSlice[] {
  const m = summary.toolMix
  return [
    {
      key: "edit",
      label: "Edits",
      value: m.edit,
      colorVar: "var(--color-analytics-blue-strong)",
    },
    { key: "test", label: "Tests", value: m.test, colorVar: "var(--color-analytics-green)" },
    { key: "read", label: "Reads", value: m.read, colorVar: "var(--color-analytics-blue)" },
    {
      key: "search",
      label: "Searches",
      value: m.search,
      colorVar: "var(--color-analytics-cyan)",
    },
    { key: "bash", label: "Commands", value: m.bash, colorVar: "var(--color-label-secondary)" },
    { key: "other", label: "Other", value: m.other, colorVar: "var(--color-label-tertiary)" },
  ].filter((s) => s.value > 0)
}

/* -------------------------------------------------------------------------
 * Initial context
 * ---------------------------------------------------------------------- */

interface InitialContextSourceMeta {
  key: InitialContextSource
  label: string
  colorVar: string
  /** One-line explanation of the source (legend-item tooltip). */
  tip: string
}

/**
 * Broad initial-context source dimensions, in display order. The
 * `unattributed` remainder is the platform's fixed baseline overhead — always
 * loaded and not user-editable. It is last and takes a neutral role color, not
 * a saturated hue, so the largest slice reads as inert baseline rather
 * than as an alarm.
 */
const INITIAL_CONTEXT_SOURCES: readonly InitialContextSourceMeta[] = [
  {
    key: "skill_instructions",
    label: "Skills",
    colorVar: "var(--color-analytics-cyan)",
    tip: "Instructions loaded from installed skills before the first response.",
  },
  {
    key: "mcp_instructions",
    label: "MCP",
    colorVar: "var(--color-analytics-blue)",
    tip: "Tool definitions and instructions from connected MCP servers.",
  },
  {
    key: "agent_instructions",
    label: "Agent files",
    colorVar: "var(--color-analytics-blue-strong)",
    tip: "Project and personal agent files loaded at startup.",
  },
  {
    key: "system_instructions",
    label: "System",
    colorVar: "var(--color-context-system)",
    tip: "The agent's own system prompt and harness instructions.",
  },
  {
    key: "unattributed",
    label: "Fixed overhead",
    colorVar: "var(--color-context-fixed)",
    tip: "The platform's baseline context that's always loaded and can't be edited — system prompt and built-in tool definitions; exact makeup varies by agent.",
  },
]

/** Broad-source donut slices for the initial-context card. */
export function initialContextSlices(
  breakdown: InitialContextBreakdown,
): (ToolSlice & { tip: string })[] {
  const totals = new Map<InitialContextSource, number>()
  for (const row of breakdown.sources) {
    totals.set(row.source, (totals.get(row.source) ?? 0) + row.tokenCount)
  }
  return INITIAL_CONTEXT_SOURCES.map((meta) => ({
    key: meta.key,
    label: meta.label,
    value: totals.get(meta.key) ?? 0,
    colorVar: meta.colorVar,
    tip: meta.tip,
  })).filter((s) => s.value > 0)
}

/**
 * Donut-center total for the initial-context card: the larger of the reported
 * total and the sum of the displayed slices. Per-source counts are character
 * estimates that can overshoot the reported total, so taking the max
 * guarantees the center is never smaller than its own slices (a no-op when the
 * two agree).
 */
export function initialContextTotal(breakdown: InitialContextBreakdown): number {
  const sliceTotal = initialContextSlices(breakdown).reduce((acc, s) => acc + s.value, 0)
  return Math.max(breakdown.totalTokens ?? 0, sliceTotal)
}

export interface NamedSourceRow {
  key: string
  /** For example `"Skill: imagegen"`, `"MCP: figma"`. */
  label: string
  tokenCount: number
  colorVar: string
}

const NAMED_SOURCE_PREFIX: Partial<Record<InitialContextSource, string>> = {
  skill_instructions: "Skill",
  mcp_instructions: "MCP",
  agent_instructions: "File",
}

/**
 * Named skill / MCP / file rows, sorted by token contribution descending and
 * capped to `limit`, with the remainder rolled into an "Other" row.
 */
export function initialContextNamedRows(
  breakdown: InitialContextBreakdown,
  limit = 5,
): NamedSourceRow[] {
  const named = breakdown.sources
    .filter((row) => row.sourceName && NAMED_SOURCE_PREFIX[row.source])
    .map((row) => ({
      key: `${row.source}:${row.sourceName}`,
      label: `${NAMED_SOURCE_PREFIX[row.source]}: ${row.sourceName}`,
      tokenCount: row.tokenCount,
      colorVar:
        INITIAL_CONTEXT_SOURCES.find((m) => m.key === row.source)?.colorVar ??
        "var(--color-label-tertiary)",
    }))
    .sort((a, b) => b.tokenCount - a.tokenCount)

  if (named.length <= limit) return named
  const top = named.slice(0, limit)
  const rest = named.slice(limit)
  top.push({
    key: "other-named",
    label: `Other (${rest.length})`,
    tokenCount: rest.reduce((acc, r) => acc + r.tokenCount, 0),
    colorVar: "var(--color-label-tertiary)",
  })
  return top
}

/* -------------------------------------------------------------------------
 * Formatting
 * ---------------------------------------------------------------------- */

/** Compact duration in h/m/s — `"6h 37m"`, `"12m"`, `"30s"`. */
export function formatDuration(secs: number): string {
  // Under a minute, show seconds so short spans don't collapse to "0m".
  const totalSecs = Math.round(secs)
  if (totalSecs < 60) return `${totalSecs}s`
  // Round once into total minutes, then split, so the minute remainder cannot
  // round up to 60 independently of the hour (which would print "1h 60m" or a
  // bare "60m"): 7170s → "2h", 3570s → "1h".
  const totalMins = Math.round(secs / 60)
  const hrs = Math.floor(totalMins / 60)
  const mins = totalMins % 60
  if (hrs > 0 && mins > 0) return `${hrs}h ${mins}m`
  if (hrs > 0) return `${hrs}h`
  return `${mins}m`
}

/** A 0..1 fraction as a whole percent. */
export function formatPct(fraction: number): string {
  return `${Math.round(fraction * 100)}%`
}

/** Compact integer for token counts — `"1.2k"`, `"3.4M"`. */
export function formatCompact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`
  return `${n}`
}

/**
 * Compact integer for a context-band value, with a trailing ".0" dropped —
 * `"200k"`, `"1.5M"`, `"50k"`. Only for band labels; other callers keep
 * {@link formatCompact}'s fixed one decimal place.
 */
export function formatTokenBand(n: number): string {
  return formatCompact(n).replace(/\.0(?=[kM])/, "")
}

/**
 * USD cost, always to two decimals (`$XX.XX`) so a trailing zero never drops
 * (`$20.70`, not `$20.7`). Every figure is an on-device estimate; surrounding
 * labels say so. A real `$0` — and any non-finite or negative input, which
 * would be malformed — reads `$0.00`; only a positive value below half a cent
 * reads `<$0.01`. That keeps a structurally-zero component row (cache writes
 * on a model that has none) from misreading as `<$0.01`.
 */
export function formatCost(usd: number): string {
  if (!Number.isFinite(usd) || usd <= 0) return "$0.00"
  if (usd < 0.005) return "<$0.01"
  return `$${usd.toFixed(2)}`
}

/* -------------------------------------------------------------------------
 * Cost presentation
 * ---------------------------------------------------------------------- */

export interface CostRow {
  label: string
  usd: number
}

/** The four billable components, in display order. */
export function costBreakdownRows(cost: {
  inputUsd: number
  outputUsd: number
  cacheReadUsd: number
  cacheWriteUsd: number
}): CostRow[] {
  return [
    { label: "Input", usd: cost.inputUsd },
    { label: "Output", usd: cost.outputUsd },
    { label: "Cache read", usd: cost.cacheReadUsd },
    { label: "Cache write", usd: cost.cacheWriteUsd },
  ]
}

/**
 * Headline label for a cost figure. A live session's number is a moving target
 * that only climbs — a *projection* — so it reads "Projected cost"; once the
 * session settles it is a fixed "Estimated cost". Shared by the badge tooltip
 * and the breakdown headline so their wording cannot drift.
 */
export function costFigureLabel(isActive: boolean): "Projected cost" | "Estimated cost" {
  return isActive ? "Projected cost" : "Estimated cost"
}

/**
 * Outlier constants. A session is "high cost" when its total is *strictly
 * greater* than `max(HIGH_COST_FLOOR_USD, HIGH_COST_MEDIAN_MULTIPLE × median)`,
 * and only once the cohort holds at least {@link HIGH_COST_MIN_SAMPLE} priced
 * sessions — below that the median is too noisy to trust.
 */
export const HIGH_COST_MEDIAN_MULTIPLE = 3
/**
 * Absolute floor: never flag a session under this, even well above the median.
 * Stops a cheap cohort (median a few cents) from screaming about a $0.40
 * session.
 */
export const HIGH_COST_FLOOR_USD = 2
/** Minimum priced sessions before any outlier shows. */
export const HIGH_COST_MIN_SAMPLE = 8

/**
 * Median of a list. Returns 0 for an empty list — callers gate on sample size
 * before relying on the value. Does not mutate the input.
 */
export function median(values: number[]): number {
  if (values.length === 0) return 0
  const sorted = [...values].sort((a, b) => a - b)
  const mid = Math.floor(sorted.length / 2)
  if (sorted.length % 2 !== 0) return sorted[mid] ?? 0
  return ((sorted[mid - 1] ?? 0) + (sorted[mid] ?? 0)) / 2
}

/**
 * The dollar threshold above which a session is a cost outlier, or null when
 * the cohort is too small to judge. The floor suppresses flags on small
 * sessions; the multiple scales the bar with the cohort. A session is high-cost
 * when its own total is *strictly greater* than this.
 */
export function costOutlierThreshold(costs: number[]): number | null {
  if (costs.length < HIGH_COST_MIN_SAMPLE) return null
  return Math.max(HIGH_COST_FLOOR_USD, HIGH_COST_MEDIAN_MULTIPLE * median(costs))
}

/** True when no session produced analyzed events. */
export function isEmptySummary(summary: ActiveSessionsSummary): boolean {
  return summary.sessionCount === 0
}
