/**
 * The headline efficiency metric and the three spend shares the card shows.
 *
 * Bands apply only to Claude Code and Codex because their harness behavior differs.
 */

import type { SessionEfficiency } from "../types/session"

/** How a metric reads against its band thresholds. */
export type EfficiencyBand = "good" | "ok" | "bad"

/** The agent family whose thresholds a session reads against. */
export type EfficiencyProfile = "claude" | "codex"

/** One metric with its reading and an optional benchmark band. */
export interface EfficiencyMetric {
  value: number
  band: EfficiencyBand | null
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
  profile: EfficiencyProfile | null
}

/** The good and bad edges for one metric. */
interface BandEdges {
  good: number
  bad: number
}

interface ProfileEdges {
  costPerMTok: BandEdges
  rewriteShare: BandEdges
  realWorkShare: BandEdges
  carryShare: BandEdges
}

const HIGHER_IS_BETTER: Record<keyof ProfileEdges, boolean> = {
  costPerMTok: false,
  rewriteShare: false,
  realWorkShare: true,
  carryShare: false,
}

const EDGES: Record<EfficiencyProfile, ProfileEdges> = {
  claude: {
    costPerMTok: { good: 33, bad: 80 },
    rewriteShare: { good: 0.1, bad: 0.25 },
    realWorkShare: { good: 0.36, bad: 0.18 },
    // Carry uses the overhead left when Real Work and Rewrite reach the same band.
    carryShare: { good: 0.54, bad: 0.57 },
  },
  codex: {
    costPerMTok: { good: 20, bad: 46 },
    rewriteShare: { good: 0.08, bad: 0.14 },
    realWorkShare: { good: 0.33, bad: 0.17 },
    // Carry uses the overhead left when Real Work and Rewrite reach the same band.
    carryShare: { good: 0.59, bad: 0.69 },
  },
}

const PROFILE_BY_AGENT: Partial<Record<string, EfficiencyProfile>> = {
  "claude-code": "claude",
  codex: "codex",
}

/** The optional threshold family for an agent slug. */
export function efficiencyProfile(agent: string): EfficiencyProfile | null {
  if (!Object.prototype.hasOwnProperty.call(PROFILE_BY_AGENT, agent)) return null
  return PROFILE_BY_AGENT[agent] ?? null
}

function bandFor(
  value: number,
  edges: BandEdges,
  metricKey: keyof ProfileEdges,
): EfficiencyBand {
  if (HIGHER_IS_BETTER[metricKey]) {
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
    segments: HIGHER_IS_BETTER[metricKey] ? ["bad", "ok", "good"] : ["good", "ok", "bad"],
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

function metric(
  value: number,
  metricKey: keyof ProfileEdges,
  profile: EfficiencyProfile | null,
): EfficiencyMetric {
  return {
    value,
    band: profile === null ? null : bandFor(value, EDGES[profile][metricKey], metricKey),
  }
}

/** The three metrics for one subject's totals, with applicable benchmark bands. */
export function efficiencyMetrics(totals: SessionEfficiency, agent: string): EfficiencyMetrics {
  const profile = efficiencyProfile(agent)
  const denominatorTokens = totals.growthTokens + totals.outputTokens
  const hasSpend = totals.totalUsd > 0
  return {
    costPerMTok:
      hasSpend && denominatorTokens > 0
        ? metric((totals.totalUsd / denominatorTokens) * 1e6, "costPerMTok", profile)
        : null,
    realWorkShare: hasSpend
      ? metric(totals.newWorkUsd / totals.totalUsd, "realWorkShare", profile)
      : null,
    rewriteShare: hasSpend
      ? metric(totals.rewriteUsd / totals.totalUsd, "rewriteShare", profile)
      : null,
    carryShare: hasSpend
      ? metric(totals.carryUsd / totals.totalUsd, "carryShare", profile)
      : null,
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
  return HIGHER_IS_BETTER[metricKey] ? "low" : "high"
}

/** Describe the good, bad, and neutral ranges for one metric. */
export function efficiencyThresholdGuidance(
  metricKey: keyof ProfileEdges,
  profile: EfficiencyProfile | null,
): string[] {
  if (profile === null) return []

  const edges = EDGES[profile][metricKey]
  const fmt = (value: number) => formatEdge(metricKey, value)
  if (HIGHER_IS_BETTER[metricKey]) {
    return [
      `For ${readableProfile[profile]}, aim for above ${fmt(edges.good)}. Below ${fmt(edges.bad)} is too low.`,
    ]
  }
  return [
    `For ${readableProfile[profile]}, aim for below ${fmt(edges.good)}. Above ${fmt(edges.bad)} is too high.`,
  ]
}
