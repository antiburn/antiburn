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
  /** Right-hand hint: token counts, then compactions and rehydrations when any. */
  hint: string
}

/** "3 compactions", "1 compaction", "2 routing misses". */
function countLabel(count: number, noun: string): string {
  if (count === 1) return `${count} ${noun}`
  return `${count} ${noun}${noun.endsWith("s") ? "es" : "s"}`
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

  const parts = [`${formatCompact(tokensInTotal)} in`, `${formatCompact(tokensOutTotal)} out`]
  if (compactionCount > 0) parts.push(countLabel(compactionCount, "compaction"))
  if (cacheRehydrationCount > 0) parts.push(countLabel(cacheRehydrationCount, "rehydration"))
  if (cacheRoutingMissCount > 0) parts.push(countLabel(cacheRoutingMissCount, "routing miss"))

  return { costTotal, split, hint: parts.join(" · ") }
}
