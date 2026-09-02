/**
 * The headline efficiency metric and the three spend shares the card shows.
 *
 * The bands differ by agent family because pricing and harness behavior differ.
 * Any agent other than Codex uses the Claude Code bands.
 */

import type { SessionEfficiency } from "../types/session"

/** How a metric reads against its band thresholds. */
export type EfficiencyBand = "good" | "ok" | "bad"

/** The agent family whose thresholds a session reads against. */
export type EfficiencyProfile = "claude" | "codex"

/** One metric with its reading, or null when the ratio is undefined. */
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
  profile: EfficiencyProfile
}

/**
 * Band edges for one metric. A reading below `good` is good and one above
 * `bad` is bad; anything between reads as ok. A "higher is better"
 * metric flips the comparison.
 */
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

/** The threshold family for an agent slug. */
export function efficiencyProfile(agent: string): EfficiencyProfile {
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

/** The three bands and marker position for one efficiency thermometer. */
export interface EfficiencyThermometer {
  segments: [EfficiencyBand, EfficiencyBand, EfficiencyBand]
  position: number
}

function thermometerFor(value: number, edges: BandEdges): EfficiencyThermometer {
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
  }
}

/** One zone of a meter track: the band that holds from `from` (0–1) upward. */
export interface EfficiencyShareZone {
  from: number
  band: EfficiencyBand
}

/** Build the thermometer for one metric and agent profile. */
export function efficiencyThermometer(
  value: number,
  metricKey: keyof ProfileEdges,
  profile: EfficiencyProfile,
): EfficiencyThermometer {
  return thermometerFor(value, EDGES[profile][metricKey])
}

function metric(value: number, edges: BandEdges): EfficiencyMetric {
  return { value, band: bandFor(value, edges) }
}

/** The three metrics for one subject's totals, read against `agent`'s bands. */
export function efficiencyMetrics(totals: SessionEfficiency, agent: string): EfficiencyMetrics {
  const profile = efficiencyProfile(agent)
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

function readableProfile(profile: EfficiencyProfile) {
  if (profile === "claude") return "Claude"
  if (profile === "codex") return "Codex"
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
  profile: EfficiencyProfile,
): string[] {
  const edges = EDGES[profile][metricKey]
  const fmt = (value: number) => formatEdge(metricKey, value)
  if (edges.higherIsBetter) {
    return [
      `For ${readableProfile(profile)}, aim for above ${fmt(edges.good)}. Below ${fmt(edges.bad)} is too low.`,
    ]
  }
  return [
    `For ${readableProfile(profile)}, aim for below ${fmt(edges.good)}. Above ${fmt(edges.bad)} is too high.`,
  ]
}
