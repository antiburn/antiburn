/**
 * The headline efficiency metric and the three spend shares the card shows.
 *
 * Every session keeps the legacy visualization scale.
 * Agent-specific threshold guidance appears only for explicitly supported agents.
 */

import type { SessionEfficiency } from "../types/session"

/** How a metric reads against its band thresholds. */
export type EfficiencyBand = "good" | "ok" | "bad"

/** A reference family used for visualization or agent-specific guidance. */
export type EfficiencyProfile = "claude" | "codex"

/** One metric with its reading against the visualization scale. */
export interface EfficiencyMetric {
  value: number
  band: EfficiencyBand
}

export interface EfficiencyMetrics {
  /** Dollars per million tokens of context growth plus output. */
  costPerMTok: EfficiencyMetric | null
  /** Share of the spend that was real work, in the range 0 to 1. */
  realWorkShare: EfficiencyMetric | null
  /** Share of the spend that was rewrite, in the range 0 to 1. */
  rewriteShare: EfficiencyMetric | null
  /** Share of the spend that was carry, in the range 0 to 1. */
  carryShare: EfficiencyMetric | null
  unpricedTurns: number
  /** The non-null profile preserves the legacy scale and verdict visualization. */
  profile: EfficiencyProfile
  /** The nullable profile controls agent-specific tooltip and inline guidance. */
  guidanceProfile: EfficiencyProfile | null
}

/** Band edges and direction for one metric. */
interface BandEdges {
  good: number
  bad: number
  higherIsBetter: boolean
}

interface ProfileEdges {
  costPerMTok: BandEdges
  rewriteShare: BandEdges
  realWorkShare: BandEdges
  carryShare: BandEdges
}

const EDGES: Record<EfficiencyProfile, ProfileEdges> = {
  claude: {
    costPerMTok: { good: 33, bad: 80, higherIsBetter: false },
    rewriteShare: { good: 0.1, bad: 0.25, higherIsBetter: false },
    realWorkShare: { good: 0.36, bad: 0.18, higherIsBetter: true },
    // Carry uses the overhead left when Real Work and Rewrite reach the same band.
    carryShare: { good: 0.54, bad: 0.57, higherIsBetter: false },
  },
  codex: {
    costPerMTok: { good: 20, bad: 46, higherIsBetter: false },
    rewriteShare: { good: 0.08, bad: 0.14, higherIsBetter: false },
    realWorkShare: { good: 0.33, bad: 0.17, higherIsBetter: true },
    // Carry uses the overhead left when Real Work and Rewrite reach the same band.
    carryShare: { good: 0.59, bad: 0.69, higherIsBetter: false },
  },
}

/** Return the guidance profile only for an exact supported agent slug. */
export function efficiencyProfile(agent: string): EfficiencyProfile | null {
  if (agent === "claude-code") return "claude"
  if (agent === "codex") return "codex"
  return null
}

/** Preserve the pre-existing reference scale. This fallback does not validate agent-specific guidance. */
function visualizationProfile(agent: string): EfficiencyProfile {
  return agent === "codex" ? "codex" : "claude"
}

function bandFor(value: number, edges: BandEdges): EfficiencyBand {
  if (edges.higherIsBetter) {
    if (value > edges.good) return "good"
    if (value < edges.bad) return "bad"
    return "ok"
  }
  if (value < edges.good) return "good"
  if (value > edges.bad) return "bad"
  return "ok"
}

/**
 * The three bands and marker position for one efficiency thermometer, with
 * the four tick values that bound them: the start, the two band edges, and
 * the top of the scale.
 */
export interface EfficiencyThermometer {
  segments: [EfficiencyBand, EfficiencyBand, EfficiencyBand]
  position: number
  ticks: [string, string, string, string]
}

function thermometerFor(
  value: number,
  edges: BandEdges,
  metricKey: keyof ProfileEdges,
): EfficiencyThermometer {
  const low = Math.min(edges.good, edges.bad)
  const high = Math.max(edges.good, edges.bad)
  const top = high * 2
  let position: number
  if (value < low) {
    position = Math.max(0, value / low) / 3
  } else if (value <= high) {
    position = (1 + (value - low) / (high - low)) / 3
  } else {
    position = (2 + Math.min(1, (value - high) / (top - high))) / 3
  }
  return {
    segments: edges.higherIsBetter ? ["bad", "ok", "good"] : ["good", "ok", "bad"],
    position,
    ticks: [
      formatEdge(metricKey, 0),
      formatEdge(metricKey, low),
      formatEdge(metricKey, high),
      formatEdge(metricKey, top),
    ],
  }
}

/** Build the thermometer for one metric and agent profile. */
export function efficiencyThermometer(
  value: number,
  metricKey: keyof ProfileEdges,
  profile: EfficiencyProfile,
): EfficiencyThermometer {
  return thermometerFor(value, EDGES[profile][metricKey], metricKey)
}

function metric(value: number, edges: BandEdges): EfficiencyMetric {
  return { value, band: bandFor(value, edges) }
}

/** Build metrics with the legacy scale and separate agent-specific guidance applicability. */
export function efficiencyMetrics(totals: SessionEfficiency, agent: string): EfficiencyMetrics {
  const profile = visualizationProfile(agent)
  const guidanceProfile = efficiencyProfile(agent)
  const edges = EDGES[profile]
  const denominatorTokens = totals.growthTokens + totals.outputTokens
  const hasSpend = totals.totalUsd > 0
  return {
    costPerMTok:
      hasSpend && denominatorTokens > 0
        ? metric((totals.totalUsd / denominatorTokens) * 1e6, edges.costPerMTok)
        : null,
    realWorkShare: hasSpend
      ? metric(totals.newWorkUsd / totals.totalUsd, edges.realWorkShare)
      : null,
    rewriteShare: hasSpend
      ? metric(totals.rewriteUsd / totals.totalUsd, edges.rewriteShare)
      : null,
    carryShare: hasSpend ? metric(totals.carryUsd / totals.totalUsd, edges.carryShare) : null,
    unpricedTurns: totals.unpricedTurns,
    profile,
    guidanceProfile,
  }
}

/** `$41.40` — a figure to two decimal places. */
export function formatCostPerMTok(value: number): string {
  return `$${value.toFixed(2)}`
}

/** `34%`, `7.1%`, `0.68%` — a 0 to 1 share to two significant figures. */
export function formatSharePercent(share: number): string {
  const percent = share * 100
  if (percent === 0) return "0%"
  return `${Number(percent.toPrecision(2))}%`
}

/** `$33` or `10%` — a band edge, with no more digits than the edge has. */
function formatEdge(metricKey: keyof ProfileEdges, value: number): string {
  return metricKey === "costPerMTok" ? `$${value}` : `${Math.round(value * 100)}%`
}

const readableProfile: Record<EfficiencyProfile, string> = {
  claude: "Claude",
  codex: "Codex",
}

/**
 * The band word after a value. A bad reading names its direction: a high
 * cost or rewrite share, or a low real-work share.
 */
export function efficiencyBandWord(
  band: EfficiencyBand,
  metricKey: keyof ProfileEdges,
): string {
  if (band !== "bad") return band
  return EDGES.claude[metricKey].higherIsBetter ? "low" : "high"
}

/** Describe the good, bad, and neutral ranges for one metric. */
export function efficiencyThresholdGuidance(
  metricKey: keyof ProfileEdges,
  profile: EfficiencyProfile | null,
): string[] {
  if (profile === null) return []

  const edges = EDGES[profile][metricKey]
  const fmt = (value: number) => formatEdge(metricKey, value)
  if (edges.higherIsBetter) {
    return [
      `For ${readableProfile[profile]}, aim for above ${fmt(edges.good)}. Below ${fmt(edges.bad)} is too low.`,
    ]
  }
  return [
    `For ${readableProfile[profile]}, aim for below ${fmt(edges.good)}. Above ${fmt(edges.bad)} is too high.`,
  ]
}
