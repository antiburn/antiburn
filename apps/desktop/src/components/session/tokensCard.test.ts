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
  it("states the tokens as stat cells and leaves the cost to the Cost card", () => {
    const model = tokensCardModel({ ...base, hasCostSubagents: false, costScope: "topLevel" })
    expect(model.stats).toEqual([
      { label: "In", value: "1.2M", tone: "in", series: "in" },
      { label: "Out", value: "34.0k", tone: "out", series: "out" },
    ])
    expect(model.costTotal).toBe(41.45)
  })

  it("adds compaction and rehydration counts as their own cells", () => {
    expect(
      tokensCardModel({ ...base, compactionCount: 3, cacheRehydrationCount: 1 }).stats,
    ).toEqual([
      { label: "In", value: "1.2M", tone: "in", series: "in" },
      { label: "Out", value: "34.0k", tone: "out", series: "out" },
      { label: "Compactions", value: "3" },
      { label: "Rehydrations", value: "1", tone: "waste", series: "rehydration" },
    ])
  })

  it("keeps a count label plural whatever the count is", () => {
    expect(tokensCardModel({ ...base, cacheRoutingMissCount: 1 }).stats).toContainEqual({
      label: "Provider cache misses",
      value: "1",
      tone: "waste",
      series: "routingMiss",
    })
    expect(tokensCardModel({ ...base, compactionCount: 1 }).stats).toContainEqual({
      label: "Compactions",
      value: "1",
    })
  })

  it("omits a count cell when nothing of that kind happened", () => {
    const labels = tokensCardModel(base).stats.map((stat) => stat.label)
    expect(labels).toEqual(["In", "Out"])
  })

  it("keeps a null cost total when nothing was priced", () => {
    const model = tokensCardModel({
      ...base,
      selectedCost: null,
      summaryCostTotalUsd: null,
      hasCostSubagents: false,
    })
    expect(model.costTotal).toBeNull()
    expect(model.stats).toEqual([
      { label: "In", value: "1.2M", tone: "in", series: "in" },
      { label: "Out", value: "34.0k", tone: "out", series: "out" },
    ])
  })

  it("falls back to the summary total when no cost result exists", () => {
    const model = tokensCardModel({
      ...base,
      selectedCost: null,
      summaryCostTotalUsd: 3.5,
      hasCostSubagents: false,
    })
    expect(model.costTotal).toBe(3.5)
  })

  it("splits only an inclusive subject that actually has sub-agents", () => {
    expect(tokensCardModel(base).split).toEqual({
      parent: base.selectedParentCost,
      subagents: base.selectedSubagentsCost,
      subagentCount: 3,
      members: [],
      sessionStartedAtEpoch: null,
    })
    expect(tokensCardModel({ ...base, costScope: "topLevel" }).split).toBeNull()
    expect(tokensCardModel({ ...base, hasCostSubagents: false }).split).toBeNull()
    expect(tokensCardModel({ ...base, selectedParentCost: null }).split).toBeNull()
    expect(tokensCardModel({ ...base, selectedSubagentsCost: null }).split).toBeNull()
  })

  it("carries the sub-agent roster through to the split", () => {
    const members = [
      {
        agent: "claude-code",
        subagentId: "a",
        label: "Investigate",
        cost: null,
        tokens: null,
        startedAtEpoch: null,
        modelRuns: [],
      },
    ]
    expect(tokensCardModel({ ...base, members }).split).toEqual({
      parent: base.selectedParentCost,
      subagents: base.selectedSubagentsCost,
      subagentCount: 3,
      members,
      sessionStartedAtEpoch: null,
    })
  })

  it("carries the session's own start epoch through to the split", () => {
    expect(tokensCardModel({ ...base, sessionStartedAtEpoch: 1_700_000_000 }).split).toEqual({
      parent: base.selectedParentCost,
      subagents: base.selectedSubagentsCost,
      subagentCount: 3,
      members: [],
      sessionStartedAtEpoch: 1_700_000_000,
    })
  })
})
