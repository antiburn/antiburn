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
  PhaseDistribution,
  SessionBucket,
  SessionPhase,
} from "../types/session"

/* -------------------------------------------------------------------------
 * Phases
 * ---------------------------------------------------------------------- */

export interface PhaseMeta {
  key: SessionPhase
  label: string
  /** CSS variable for the band color (see styles/session-analytics-colors.css). */
  colorVar: string
  /** One-line explanation of what the mode captures. */
  tip: string
  /** Healthy reference band, as fractions 0..1. */
  reference: { lo: number; hi: number }
}

/** Ordered deepest → shallowest, matching the hypnogram lanes (bottom → top). */
export const PHASES: readonly PhaseMeta[] = [
  {
    key: "implementing",
    label: "Implement",
    colorVar: "var(--color-mode-implementing)",
    tip: "Editing or writing files — the deep-work core of a session.",
    reference: { lo: 0.3, hi: 0.7 },
  },
  {
    key: "testing",
    label: "Test",
    colorVar: "var(--color-mode-testing)",
    tip: "Running tests, builds, or checks to verify behavior.",
    reference: { lo: 0.05, hi: 0.25 },
  },
  {
    key: "exploring",
    label: "Explore",
    colorVar: "var(--color-mode-exploring)",
    tip: "Reading, searching, and running commands to gather context.",
    reference: { lo: 0.1, hi: 0.4 },
  },
  {
    key: "thinking",
    label: "Plan",
    colorVar: "var(--color-mode-thinking)",
    tip: "Reasoning with no tool calls — mostly planning, but also any thinking between actions.",
    reference: { lo: 0.05, hi: 0.3 },
  },
  {
    key: "disruption",
    label: "Errors",
    colorVar: "var(--color-mode-disruption)",
    tip: "Failed tools, rejected edits, and corrections — the session's wake-ups.",
    reference: { lo: 0.0, hi: 0.1 },
  },
]

/** Band color for a phase, falling back to a muted label color. */
export function phaseColor(phase: SessionPhase): string {
  return PHASES.find((p) => p.key === phase)?.colorVar ?? "var(--color-label-tertiary)"
}

/** Display label for a phase, falling back to the raw key. */
export function phaseLabel(phase: SessionPhase): string {
  return PHASES.find((p) => p.key === phase)?.label ?? phase
}

/* -------------------------------------------------------------------------
 * Pattern score
 * ---------------------------------------------------------------------- */

/** Score at or above which a session reads as "Healthy". */
const HEALTHY_SCORE_THRESHOLD = 75
/** Score at or above which a session reads as "Drifting"; below it, "Thrashing". */
const DRIFTING_SCORE_THRESHOLD = 50

/**
 * A 0–100 session-health score to its swatch color: green healthy, amber
 * drifting, red disrupted. Shared by the score stat and the sub-agent roster
 * rows so both color the same score identically.
 */
export function scoreColor(score: number): string {
  if (score >= HEALTHY_SCORE_THRESHOLD) return "var(--color-system-green)"
  if (score >= DRIFTING_SCORE_THRESHOLD) return "var(--color-pattern-drifting)"
  return "var(--color-mode-disruption)"
}

/** The health-score band label paired with {@link scoreColor}. */
export function scoreLabel(score: number): string {
  if (score >= HEALTHY_SCORE_THRESHOLD) return "Healthy"
  if (score >= DRIFTING_SCORE_THRESHOLD) return "Drifting"
  return "Thrashing"
}

/* -------------------------------------------------------------------------
 * Phase breakdown
 * ---------------------------------------------------------------------- */

/** Whether a phase sits outside its healthy reference band. */
type ReferenceFlag = "high" | "low" | null

export interface PhaseBreakdownRow extends PhaseMeta {
  fraction: number
  minutes: number
  flag: ReferenceFlag
}

/** Per-phase rows for the donut breakdown list (percent, minutes, High/Low). */
export function phaseBreakdown(summary: ActiveSessionsSummary): PhaseBreakdownRow[] {
  // Distribute active (working) time, not wall-clock: the distribution is
  // event-weighted, so multiplying by wall-clock would spread idle gaps across
  // the phases. Active time excludes those gaps.
  const totalMinutes = summary.avgActiveSecs / 60
  return PHASES.map((meta) => {
    const fraction = summary.phaseDistribution[meta.key]
    let flag: ReferenceFlag = null
    if (fraction > meta.reference.hi) flag = "high"
    else if (fraction < meta.reference.lo) flag = "low"
    return { ...meta, fraction, minutes: Math.round(totalMinutes * fraction), flag }
  })
}

/* -------------------------------------------------------------------------
 * Buckets and chart series
 * ---------------------------------------------------------------------- */

export interface BucketDetail {
  /** Progress fraction × durationSecs (approximate; buckets are resampled). */
  elapsedSecs: number
  /** 0..100. */
  progressPct: number
  /** In {@link PHASES} order, only modes with a non-zero share. */
  modes: { label: string; colorVar: string; pct: number }[]
  tokensIn: number
  tokensOut: number
  /** 0..100, clamped; null when the context window is unknown. */
  contextPct: number | null
  /** True when the bucket holds no classifiable activity. */
  empty: boolean
}

/** Per-bucket detail for the hypnogram hover tooltip. */
export function bucketDetail(
  bucket: SessionBucket,
  index: number,
  bucketCount: number,
  durationSecs: number,
  contextWindow: number,
): BucketDetail {
  const fraction = bucketCount > 1 ? index / (bucketCount - 1) : 0
  const total = distributionTotal(bucket.distribution)
  const modes = PHASES.map((meta) => ({
    label: meta.label,
    colorVar: meta.colorVar,
    pct: pct(bucket.distribution[meta.key]),
  })).filter((m) => m.pct > 0)
  return {
    elapsedSecs: Math.round(fraction * durationSecs),
    progressPct: Math.round(fraction * 100),
    modes,
    tokensIn: bucket.tokensIn,
    tokensOut: bucket.tokensOut,
    contextPct:
      contextWindow > 0
        ? Math.min(100, Math.round((bucket.contextTokens / contextWindow) * 100))
        : null,
    empty: total <= 0,
  }
}

export interface TokenPoint {
  progress: number
  tokensIn: number
  tokensOut: number
}

/** Input/output token throughput over session progress. */
export function tokenSeries(buckets: SessionBucket[]): TokenPoint[] {
  const active = activeBuckets(buckets)
  return active.map((b, i) => ({
    progress: Math.round((i / Math.max(1, active.length - 1)) * 100),
    tokensIn: b.tokensIn,
    tokensOut: b.tokensOut,
  }))
}

export interface ContextPoint {
  progress: number
  contextPct: number
  isCompactionBoundary: boolean
}

/** Context-window occupancy (0..100%) over progress. */
export function contextSeries(buckets: SessionBucket[], contextWindow: number): ContextPoint[] {
  const window = contextWindow || 1
  const active = activeBuckets(buckets, true)
  return active.map((b, i) => ({
    progress: Math.round((i / Math.max(1, active.length - 1)) * 100),
    contextPct: Math.min(100, Math.round((b.contextTokens / window) * 100)),
    isCompactionBoundary: b.isCompactionBoundary,
  }))
}

export interface ToolSlice {
  key: string
  label: string
  value: number
  colorVar: string
}

/** Tool-mix donut slices, ordered to match the hypnogram phase colors. */
export function toolMixSlices(summary: ActiveSessionsSummary): ToolSlice[] {
  const m = summary.toolMix
  return [
    { key: "edit", label: "Edits", value: m.edit, colorVar: "var(--color-mode-implementing)" },
    { key: "test", label: "Tests", value: m.test, colorVar: "var(--color-mode-testing)" },
    { key: "read", label: "Reads", value: m.read, colorVar: "var(--color-mode-exploring)" },
    {
      key: "search",
      label: "Searches",
      value: m.search,
      colorVar: "var(--color-mode-thinking)",
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
 * a session "mode" hue, so the largest slice reads as inert baseline rather
 * than as an alarm.
 */
const INITIAL_CONTEXT_SOURCES: readonly InitialContextSourceMeta[] = [
  {
    key: "skill_instructions",
    label: "Skills",
    colorVar: "var(--color-mode-thinking)",
    tip: "Instructions loaded from installed skills before the first response.",
  },
  {
    key: "mcp_instructions",
    label: "MCP",
    colorVar: "var(--color-mode-exploring)",
    tip: "Tool definitions and instructions from connected MCP servers.",
  },
  {
    key: "agent_instructions",
    label: "Agent files",
    colorVar: "var(--color-mode-implementing)",
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

/** File size from a byte count — `"4.2 KiB"`, `"1.1 MiB"`. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B"
  if (bytes < 1024) return `${Math.round(bytes)} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GiB`
}

/**
 * Coarse "time ago" from an epoch-ms timestamp: `"just now"`, `"5m ago"`,
 * `"3h ago"`, `"6d ago"`, falling back to an absolute short date once it is
 * older than about a month. Used for on-disk file times.
 */
export function formatRelativeTime(epochMs: number): string {
  const deltaSec = Math.max(0, (Date.now() - epochMs) / 1000)
  if (deltaSec < 60) return "just now"
  const min = Math.floor(deltaSec / 60)
  if (min < 60) return `${min}m ago`
  const hr = Math.floor(min / 60)
  if (hr < 24) return `${hr}h ago`
  const day = Math.floor(hr / 24)
  if (day < 30) return `${day}d ago`
  return new Date(epochMs).toLocaleDateString(undefined, { month: "short", day: "numeric" })
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

/* -------------------------------------------------------------------------
 * Internals
 * ---------------------------------------------------------------------- */

function distributionTotal(d: PhaseDistribution): number {
  return d.implementing + d.testing + d.exploring + d.thinking + d.disruption
}

/**
 * Buckets that hold classifiable activity — drops idle gaps so charts read on
 * the active-time axis (matching the hypnogram), not wall-clock.
 *
 * `includeCompactionBoundaries` keeps a compaction marker's bucket even with no
 * classified activity: the marker record is never classified into a phase, so
 * it would otherwise be dropped here and the compaction line would never reach
 * the chart. Only {@link contextSeries} opts in — the marker bucket's token
 * counts are genuinely zero, so including it in {@link tokenSeries} would draw
 * a spurious dip rather than a meaningful data point.
 */
function activeBuckets(
  buckets: SessionBucket[],
  includeCompactionBoundaries = false,
): SessionBucket[] {
  return buckets.filter(
    (b) =>
      (b.dominantPhase != null && distributionTotal(b.distribution) > 0) ||
      (includeCompactionBoundaries && b.isCompactionBoundary),
  )
}

/** True when no analyzed session produced any classifiable activity. */
export function isEmptySummary(summary: ActiveSessionsSummary): boolean {
  return summary.sessionCount === 0 || distributionTotal(summary.phaseDistribution) <= 0
}

function pct(fraction: number): number {
  return Math.round(fraction * 100)
}
