// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shaping and formatting for the session-analysis surface.
 *
 * Pure functions over the analysis summary the local engine produces, with no
 * React, no IPC, and no state — the charts consume the output, and these can be
 * tested (and reused) on their own.
 */

import type {
  ActiveSessionsSummary,
  InitialContextBreakdown,
  SessionBucket,
  SourceOrigin,
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
  subagentTokens: number
  isCompactionBoundary: boolean
  cacheReadTokens: number
  cacheWriteTokens: number
  isCacheRehydration: boolean
  isCacheRoutingMiss: boolean
  secsSincePriorTurn: number | null
  subagentLaunches: number
  /** The name of the last parent tool call in this bucket, when any. */
  lastTool: string | null
  /**
   * Set when no model call landed in this bucket: the slice sits inside the
   * gap between two calls. `tool` names the call the model made before the
   * gap, when the transcript records one. `secs` is the length of the whole
   * gap, when the next call recorded it. `userPrompt` is true when a user
   * prompt ends the gap: the model waited for the user, not for a tool.
   */
  betweenCalls: { secs: number | null; tool: string | null; userPrompt: boolean } | null
  /** Model that produced this point, forward-filled from the last bucket that named one. */
  model: string | null
  /** Thinking-effort mode at this point, forward-filled the same way. */
  thinkingMode: string | null
  /** Response speed at this point, forward-filled the same way. */
  speed: string | null
  /** True when this bucket itself carries a thinking block (not forward-filled). */
  hasThinking: boolean
  /** Whether the compaction in this bucket was manual or automatic, when known. */
  compactionTrigger: "manual" | "auto" | null
  /** The context token count right before the compaction in this bucket, when known. */
  compactionPreTokens: number | null
  /** The context token count right after the compaction in this bucket, when known. */
  compactionPostTokens: number | null
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
    subagentTokens: bucket.subagentTokens,
    isCompactionBoundary: bucket.isCompactionBoundary,
    cacheReadTokens: bucket.cacheReadTokens,
    cacheWriteTokens: bucket.cacheWriteTokens,
    isCacheRehydration: bucket.isCacheRehydration,
    isCacheRoutingMiss: bucket.isCacheRoutingMiss,
    secsSincePriorTurn: bucket.secsSincePriorTurn,
    subagentLaunches: bucket.subagentLaunches,
    lastTool: bucket.lastTool,
    betweenCalls: betweenCalls(buckets, index),
    model: models[index]!,
    thinkingMode: thinkingModes[index]!,
    speed: speeds[index]!,
    hasThinking: bucket.hasThinking,
    compactionTrigger: bucket.compactionTrigger,
    compactionPreTokens: bucket.compactionPreTokens,
    compactionPostTokens: bucket.compactionPostTokens,
  }))
}

/**
 * Mirrors `analysis::engine::IDLE_GAP_MS`. The engine counts each gap between
 * events toward active time up to this cap, so the chart draws a longer gap
 * as a shelf of this width.
 */
export const IDLE_GAP_SECS = 5 * 60

/** True when a model call (parent or sub-agent) landed in the bucket. */
function hasCall(bucket: SessionBucket): boolean {
  return (
    bucket.tokensIn > 0 ||
    bucket.tokensOut > 0 ||
    bucket.subagentTokens > 0 ||
    bucket.secsSincePriorTurn != null
  )
}

/**
 * Describe the gap a call-less bucket sits in. The tool is the last one the
 * parent called at or after the previous call bucket, so it names the call
 * that ran during the gap. The length comes from the next call bucket, which
 * records the seconds since the call before it. Null for a bucket with a call.
 */
function betweenCalls(
  buckets: readonly SessionBucket[],
  index: number,
): ContextTokenPoint["betweenCalls"] {
  const current = buckets[index]!
  if (hasCall(current)) return null
  let tool: string | null = null
  for (let i = index; i >= 0; i--) {
    const bucket = buckets[i]!
    tool ??= bucket.lastTool
    if (hasCall(bucket)) break
  }
  let secs: number | null = null
  let userPrompt = current.userPrompts > 0
  for (let i = index + 1; i < buckets.length; i++) {
    const bucket = buckets[i]!
    userPrompt ||= bucket.userPrompts > 0
    if (hasCall(bucket)) {
      secs = bucket.secsSincePriorTurn
      break
    }
  }
  return { secs, tool, userPrompt }
}

/** The mode the session starts in, as the detail header reports it. */
export interface SessionModeBaseline {
  model: string | null
  thinkingMode: string | null
  speed: string | null
  hasThinking: boolean
}

/**
 * The first observed model, effort, and speed, and whether the first bucket
 * with a model signal carries a thinking block. The tooltip hides a mode row
 * that matches this baseline, because the session header already names it.
 */
export function sessionModeBaseline(points: readonly ContextTokenPoint[]): SessionModeBaseline {
  const first = points.find((point) => point.model != null)
  return {
    model: first?.model ?? null,
    thinkingMode: points.find((point) => point.thinkingMode != null)?.thinkingMode ?? null,
    speed: points.find((point) => point.speed != null)?.speed ?? null,
    hasThinking: first?.hasThinking ?? false,
  }
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

/** Candidate spacings for the time axis, in seconds. */
const TIME_AXIS_STEPS_SECS = [
  60, 120, 300, 600, 900, 1800, 3600, 7200, 10_800, 14_400, 21_600, 28_800, 43_200, 86_400,
]

export interface TimeAxisTick {
  /** Bucket index, possibly fractional, on the numeric x axis. */
  index: number
  label: string
}

/**
 * Elapsed active-time marks for the x axis. Buckets span active time evenly,
 * so a mark at `t` seconds sits at `t / activeSecs` of the way across. Marks
 * use the coarsest step that still gives at most `maxTicks` marks.
 */
export function timeAxisTicks(
  activeSecs: number,
  bucketCount: number,
  maxTicks: number,
): TimeAxisTick[] {
  if (activeSecs <= 0 || bucketCount < 2) return []
  const step =
    TIME_AXIS_STEPS_SECS.find((s) => activeSecs / s <= maxTicks) ??
    TIME_AXIS_STEPS_SECS[TIME_AXIS_STEPS_SECS.length - 1]!
  const ticks: TimeAxisTick[] = []
  for (let t = step; t < activeSecs; t += step) {
    ticks.push({ index: (t / activeSecs) * (bucketCount - 1), label: formatDuration(t) })
  }
  return ticks
}

/* -------------------------------------------------------------------------
 * Skills and MCPs
 * ---------------------------------------------------------------------- */

export interface SkillMcpRow {
  key: string
  /** Which kind of source this is. */
  kind: "skill" | "mcp" | "tool"
  /** The skill, MCP server, or tool name, without the kind prefix. */
  name: string
  tokenCount: number
  /** How many times the session used this source after it loaded. */
  useCount: number
  /** Where the skill or MCP server is installed. */
  origin: SourceOrigin
  /**
   * True for a tool row the harness deferred this session: it sent only the
   * tool's name, not its definition, so `tokenCount` is a small estimate.
   */
  deferred?: boolean
}

export interface SkillMcpUsage {
  /** Total tokens skills and MCPs occupied in the initial context. */
  totalTokens: number
  /** Tokens spent on skills and MCPs the session never used. */
  wastedTokens: number
  /** Every skill and MCP row, sorted by token contribution descending. */
  rows: SkillMcpRow[]
}

/**
 * Display word for a row's status: `"Unused"`, `"Deferred"` for an unused
 * deferred tool, `"Used"`, or `"Used ×N"` when the session used it more than
 * once.
 */
export function skillMcpStatusLabel(row: Pick<SkillMcpRow, "useCount" | "deferred">): string {
  if (row.useCount > 0) return row.useCount > 1 ? `Used ×${row.useCount}` : "Used"
  if (row.deferred) return "Deferred"
  return "Unused"
}

/** Display word for a row's install origin, or `null` when the origin is unknown. */
export function skillMcpOriginLabel(origin: SourceOrigin): string | null {
  switch (origin) {
    case "bundled":
      return "Bundled"
    case "plugin":
      return "Plugin"
    case "user":
      return "User"
    case "project":
      return "Project"
    case "unknown":
      return null
  }
}

/** Row kind for each engine source dimension. */
function skillMcpKind(source: InitialContextBreakdown["sources"][number]["source"]) {
  if (source === "skill_instructions") return "skill" as const
  if (source === "mcp_instructions") return "mcp" as const
  return "tool" as const
}

/**
 * Skill, MCP, and built-in tool rows from the initial-context breakdown,
 * sorted by token contribution descending, then by name. A skill or an MCP
 * server loads its instructions before the first response, and a harness
 * tool definition loads the same way; a row with `useCount` 0 spent that
 * space without the session ever calling on it. A deferred tool's estimate
 * still counts as wasted when unused — the harness paid for the name even
 * though the model never asked for the full definition.
 */
export function skillMcpUsage(breakdown: InitialContextBreakdown): SkillMcpUsage {
  const rows = breakdown.sources
    .filter((row): row is typeof row & { sourceName: string } => row.sourceName != null)
    .map((row) => ({
      key: `${row.source}:${row.sourceName}`,
      kind: skillMcpKind(row.source),
      name: row.sourceName,
      tokenCount: row.tokenCount,
      useCount: row.useCount ?? 0,
      origin: row.origin ?? "unknown",
      ...(row.deferred !== undefined ? { deferred: row.deferred } : {}),
    }))
    .sort((a, b) => b.tokenCount - a.tokenCount || a.name.localeCompare(b.name))

  return {
    totalTokens: rows.reduce((acc, r) => acc + r.tokenCount, 0),
    wastedTokens: rows
      .filter((r) => r.useCount === 0)
      .reduce((acc, r) => acc + r.tokenCount, 0),
    rows,
  }
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

/** Whole seconds as a zero-padded `HH:MM:SS` clock offset. */
export function formatTime(totalSecs: number): string {
  const whole = Math.max(0, Math.floor(totalSecs))
  const hrs = Math.floor(whole / 3600)
  const mins = Math.floor((whole % 3600) / 60)
  const secs = whole % 60
  return `${String(hrs).padStart(2, "0")}:${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`
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
 * Compact token count capped at three significant digits, for a column too
 * narrow for a full number — `"950"`, `"1.2k"`, `"14k"`, `"143M"`, `"2.1B"`.
 * Below 1000 tokens shows the plain count. At or above 1000, the count shows
 * in the largest unit (k/M/B) that keeps it under three digits: one decimal
 * while the scaled value reads under 10, a whole number from 10 up, and never
 * a bare ".0". Rounding that would push a value to 1000 in its own unit (for
 * example 999.6k) instead rolls over to the next unit up (1M).
 */
export function formatTokensShort(n: number): string {
  const value = Number.isFinite(n) ? n : 0
  const abs = Math.abs(value)
  if (abs < 1000) return `${Math.round(value)}`

  const tiers = [
    { factor: 1_000, suffix: "k" },
    { factor: 1_000_000, suffix: "M" },
    { factor: 1_000_000_000, suffix: "B" },
  ] as const

  let tier = tiers.length - 1
  while (tier > 0 && abs < tiers[tier]!.factor) tier--

  for (; tier < tiers.length; tier++) {
    const { factor, suffix } = tiers[tier]!
    const scaled = value / factor
    const text =
      Math.abs(scaled) < 10 ? scaled.toFixed(1).replace(/\.0$/, "") : `${Math.round(scaled)}`
    const isLastTier = tier === tiers.length - 1
    if (Math.abs(Number.parseFloat(text)) < 1000 || isLastTier) return `${text}${suffix}`
    // Rounding filled this tier (e.g. 999.6k rounded to "1000"); the next
    // iteration retries one unit up, where it reads under 10 with a decimal.
  }
  return `${Math.round(value)}`
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
  /** Billable tokens behind this row, when the caller has them. */
  tokens?: number | undefined
}

/**
 * The four billable components, in display order. Token counts are optional:
 * a caller pricing from `resultComponentCost` has them, but the activity
 * list's `SessionCostComponents` payload carries USD only.
 */
export function costBreakdownRows(cost: {
  inputUsd: number
  outputUsd: number
  cacheReadUsd: number
  cacheWriteUsd: number
  inputTokens?: number
  outputTokens?: number
  cacheReadTokens?: number
  cacheWriteTokens?: number
}): CostRow[] {
  return [
    { label: "Input", usd: cost.inputUsd, tokens: cost.inputTokens },
    { label: "Output", usd: cost.outputUsd, tokens: cost.outputTokens },
    { label: "Cache read", usd: cost.cacheReadUsd, tokens: cost.cacheReadTokens },
    { label: "Cache write", usd: cost.cacheWriteUsd, tokens: cost.cacheWriteTokens },
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
