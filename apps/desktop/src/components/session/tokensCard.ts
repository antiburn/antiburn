import type { ChartSeries } from "./analysis/ContextTokensChart"
import { formatCompact } from "../../lib/presentation/sessionAnalysis"
import type { LocalCostSubject, LocalSessionCost } from "../../lib/presentation/sessionCosts"
import type { SubagentMember } from "../../lib/types/session"

/**
 * What the Context and Tokens card says, and the cost split the Cost card
 * shows, as a pure function of the figures the view was handed.
 *
 * A function rather than a branch buried in a render body: the caller resolves
 * which cost subject the card describes, passes the results in, and gets the
 * copy back. That makes the parent/sub-agent split testable on its own, and
 * keeps the render body to layout.
 */

/** The parent/sub-agents split shown above the Tokens breakdown. */
export interface TokensCostSplit {
  parent: LocalSessionCost
  subagents: LocalSessionCost
  subagentCount: number
  /** One entry per sub-agent, for the Cost card's expandable detail rows. */
  members: SubagentMember[]
  /** Unix seconds of the session's own first transcript event, or null when
   * unknown. Each detail row shows its sub-agent's start relative to this. */
  sessionStartedAtEpoch: number | null
}

/** Everything the Tokens card needs beyond its chart. */
export interface TokensCardModel {
  /** The headline cost, or null when nothing priced this session. */
  costTotal: number | null
  /**
   * Parent/sub-agents split, only when both sides describe the same session as
   * the headline. Mixing subjects would produce rows that do not add up.
   */
  split: TokensCostSplit | null
  /**
   * The Context section's stat cells: token counts, then compactions and
   * rehydrations when any. Each cell carries a category label and its value,
   * so the section reads in the same label-over-value grid as the hero.
   *
   * `series` names the layer of the plot the cell counts, and the color the
   * cell takes, so the strip doubles as the chart's key. The key lights that
   * layer while the pointer rests on the cell. A cell with no series counts
   * something the chart draws no mark for, so it never lights.
   */
  stats: Array<{
    label: string
    value: string
    series?: ChartSeries
  }>
}

/** Decide what the Tokens card says, given the already-selected cost figures. */
export function tokensCardModel(input: {
  /**
   * Scope of the subject the headline figure describes. Only an `inclusive`
   * subject can break into parent plus sub-agents.
   */
  costScope?: LocalCostSubject["scope"] | null
  /** The headline result, or null when nothing was priced. */
  selectedCost: LocalSessionCost | null
  /** The parent-only result. */
  selectedParentCost: LocalSessionCost | null
  /** The all-sub-agents result. */
  selectedSubagentsCost: LocalSessionCost | null
  /** Whether this session has priced sub-agents at all. */
  hasCostSubagents: boolean
  /** How many sub-agents the split names. */
  costSubagentCount: number
  /** One entry per sub-agent, for the Cost card's expandable detail rows. */
  members?: SubagentMember[]
  /** Unix seconds of the session's own first transcript event, or null when
   * unknown. Passed through to the split's detail rows. */
  sessionStartedAtEpoch?: number | null
  /** Fallback total from the metrics summary, when no cost result exists. */
  summaryCostTotalUsd?: number | null
  tokensInTotal: number
  tokensOutTotal: number
  compactionCount?: number
  cacheRehydrationCount?: number
  /** Only carried on `SessionMetrics`, not on the aggregate summary. */
  cacheRoutingMissCount?: number
}): TokensCardModel {
  const {
    costScope,
    selectedCost,
    selectedParentCost,
    selectedSubagentsCost,
    hasCostSubagents,
    costSubagentCount,
    members = [],
    sessionStartedAtEpoch = null,
    tokensInTotal,
    tokensOutTotal,
    compactionCount = 0,
    cacheRehydrationCount = 0,
    cacheRoutingMissCount = 0,
  } = input

  const costTotal = selectedCost?.totalCostUsd ?? input.summaryCostTotalUsd ?? null
  const split =
    costScope === "inclusive" && selectedParentCost && selectedSubagentsCost && hasCostSubagents
      ? {
          parent: selectedParentCost,
          subagents: selectedSubagentsCost,
          subagentCount: costSubagentCount,
          members,
          sessionStartedAtEpoch,
        }
      : null

  // A label names the category, so it stays plural whatever the count is.
  const stats: TokensCardModel["stats"] = [
    { label: "In", value: formatCompact(tokensInTotal), series: "in" },
    { label: "Out", value: formatCompact(tokensOutTotal), series: "out" },
  ]
  if (compactionCount > 0) {
    stats.push({ label: "Compactions", value: String(compactionCount), series: "compaction" })
  }
  if (cacheRehydrationCount > 0) {
    stats.push({
      label: "Rehydrations",
      value: String(cacheRehydrationCount),
      series: "rehydration",
    })
  }
  if (cacheRoutingMissCount > 0) {
    stats.push({
      label: "Provider cache misses",
      value: String(cacheRoutingMissCount),
      series: "routingMiss",
    })
  }

  return { costTotal, split, stats }
}
