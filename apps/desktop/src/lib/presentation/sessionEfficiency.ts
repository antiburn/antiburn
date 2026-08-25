// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * The three efficiency metrics the Efficiency card shows, and the band each
 * one falls in.
 *
 * The bands are per agent family. Codex bills a cheaper cache and a leaner
 * prompt, so its "ok" range sits lower on every scale. Any agent that is not
 * Codex reads against the Claude Code bands.
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
}

const EDGES: Record<EfficiencyProfile, ProfileEdges> = {
  claude: {
    costPerMTok: { good: 33, bad: 80, higherIsBetter: false },
    rewriteShare: { good: 0.1, bad: 0.25, higherIsBetter: false },
    realWorkShare: { good: 0.36, bad: 0.18, higherIsBetter: true },
  },
  codex: {
    costPerMTok: { good: 20, bad: 46, higherIsBetter: false },
    rewriteShare: { good: 0.08, bad: 0.14, higherIsBetter: false },
    realWorkShare: { good: 0.33, bad: 0.17, higherIsBetter: true },
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

/**
 * One row's thermometer. `segments` lists the three bands in numeric order,
 * left to right. `position` is where the value sits on the track, in the
 * range 0 to 1. Each band takes one third of the track. The value maps
 * linearly inside its band: the low band runs from 0 to the first edge, the
 * high band from the second edge to twice that edge.
 */
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

/** The thermometer for one metric's value, read against `profile`'s bands. */
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

/** What one metric measures, for a row tooltip. */
export function efficiencyMetricDescription(metricKey: keyof ProfileEdges): string {
  switch (metricKey) {
    case "costPerMTok":
      return "Cost for real work. Waste increases this cost, so high efficiency is a low number here."
    case "realWorkShare":
      return "How much you spent on real work. The rest of your spend was just re-reading or re-sending context."
    case "rewriteShare":
      return "How much you spent rewriting the cache: usually after compaction, a cache miss, or a model switch."
  }
}

/** One sentence of thresholds for a row tooltip. */
export function efficiencyThresholdsText(
  metricKey: keyof ProfileEdges,
  profile: EfficiencyProfile,
): string {
  const edges = EDGES[profile][metricKey]
  const fmt = (value: number) => formatEdge(metricKey, value)
  if (edges.higherIsBetter) {
    return `[Good = over ${fmt(edges.good)}; Low = below ${fmt(edges.bad)}; otherwise OK]`
  }
  return `[Good = below ${fmt(edges.good)}; High = over ${fmt(edges.bad)}; otherwise OK]`
}
