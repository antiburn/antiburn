// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { formatCompact, formatCost } from "../../lib/presentation/sessionAnalytics"
import type { LocalCostSubject, LocalSessionCost } from "../../lib/presentation/sessionCosts"

/**
 * What the Tokens card says, as a pure function of the cost figures the view
 * was handed.
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
  /** Info-tooltip copy. */
  info: string
  /** Right-hand hint: token counts, plus the cost once there is one. */
  hint: string
}

const TOKENS_INFO_BASE =
  "In = input tokens sent to the model for the first time (fresh input plus prompt-cache writes; cache reads are excluded); out = tokens it generated. Both accumulate as the session goes on. Cost is an approximate estimate from the model per-token rates."

const TOKENS_INFO_ORCHESTRATOR =
  "The token chart and in/out counts describe the parent agent — in = input tokens sent to the model for the first time (fresh input plus prompt-cache writes; cache reads are excluded); out = tokens it generated. Cost is inclusive of the parent and its sub-agents; the split below shows each part."

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
  /** Fallback total from the metrics summary, when no cost result exists. */
  summaryCostTotalUsd?: number | null
  tokensInTotal: number
  tokensOutTotal: number
}): TokensCardModel {
  const {
    costScope,
    selectedCost,
    selectedParentCost,
    selectedSubagentsCost,
    hasCostSubagents,
    costSubagentCount,
    tokensInTotal,
    tokensOutTotal,
  } = input

  const costTotal = selectedCost?.totalCostUsd ?? input.summaryCostTotalUsd ?? null
  const split =
    costScope === "inclusive" && selectedParentCost && selectedSubagentsCost && hasCostSubagents
      ? {
          parent: selectedParentCost,
          subagents: selectedSubagentsCost,
          subagentCount: costSubagentCount,
        }
      : null

  const tokens = `${formatCompact(tokensInTotal)} in · ${formatCompact(tokensOutTotal)} out`

  return {
    costTotal,
    split,
    info: hasCostSubagents ? TOKENS_INFO_ORCHESTRATOR : TOKENS_INFO_BASE,
    hint:
      costTotal != null
        ? hasCostSubagents
          ? `${tokens} · incl. ${formatCost(costTotal)}`
          : `${tokens} · ${formatCost(costTotal)}`
        : tokens,
  }
}
