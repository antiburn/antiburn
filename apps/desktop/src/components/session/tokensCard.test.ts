// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from "vitest"

import {
  inclusiveCostSubject,
  subagentsCostSubject,
  topLevelCostSubject,
  type LocalCostSubject,
  type LocalSessionCost,
} from "../../lib/presentation/sessionCosts"
import { tokensCardModel } from "./tokensCard"

function cost(subject: LocalCostSubject, totalCostUsd: number): LocalSessionCost {
  return {
    subject,
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    totalTokens: 0,
    inputCostUsd: 0,
    outputCostUsd: 0,
    cacheReadCostUsd: 0,
    cacheWriteCostUsd: 0,
    totalCostUsd,
    isActive: false,
  }
}

const inclusive = inclusiveCostSubject("claude-code", "parent")
const parent = topLevelCostSubject("claude-code", "parent")
const subagents = subagentsCostSubject("claude-code", "parent")

const base = {
  costScope: "inclusive" as const,
  selectedCost: cost(inclusive, 41.45),
  selectedParentCost: cost(parent, 32.95),
  selectedSubagentsCost: cost(subagents, 8.5),
  hasCostSubagents: true,
  costSubagentCount: 3,
  tokensInTotal: 1_200_000,
  tokensOutTotal: 34_000,
}

describe("tokensCardModel", () => {
  it("states the tokens and the cost in the hint", () => {
    const model = tokensCardModel({ ...base, hasCostSubagents: false, costScope: "topLevel" })
    expect(model.hint).toBe("1.2M in · 34.0k out · $41.45")
  })

  it("marks an orchestrator total as inclusive", () => {
    expect(tokensCardModel(base).hint).toBe("1.2M in · 34.0k out · incl. $41.45")
    expect(tokensCardModel(base).info).toContain("inclusive of the parent and its sub-agents")
  })

  it("drops the cost from the hint when nothing was priced", () => {
    const model = tokensCardModel({
      ...base,
      selectedCost: null,
      summaryCostTotalUsd: null,
      hasCostSubagents: false,
    })
    expect(model.costTotal).toBeNull()
    expect(model.hint).toBe("1.2M in · 34.0k out")
  })

  it("falls back to the summary total when no cost result exists", () => {
    const model = tokensCardModel({
      ...base,
      selectedCost: null,
      summaryCostTotalUsd: 3.5,
      hasCostSubagents: false,
    })
    expect(model.costTotal).toBe(3.5)
    expect(model.hint).toContain("$3.50")
  })

  it("splits only an inclusive subject that actually has sub-agents", () => {
    expect(tokensCardModel(base).split).toEqual({
      parent: base.selectedParentCost,
      subagents: base.selectedSubagentsCost,
      subagentCount: 3,
    })
    expect(tokensCardModel({ ...base, costScope: "topLevel" }).split).toBeNull()
    expect(tokensCardModel({ ...base, hasCostSubagents: false }).split).toBeNull()
    expect(tokensCardModel({ ...base, selectedParentCost: null }).split).toBeNull()
    expect(tokensCardModel({ ...base, selectedSubagentsCost: null }).split).toBeNull()
  })

  it("describes a plain session without orchestration language", () => {
    const model = tokensCardModel({ ...base, hasCostSubagents: false, costScope: "topLevel" })
    expect(model.info).not.toContain("sub-agent")
    expect(model.info).toContain("cache reads are excluded")
  })
})
