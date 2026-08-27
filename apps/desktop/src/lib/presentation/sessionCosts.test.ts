import { describe, expect, it } from "vitest"

import type { LocalCostSubject, LocalSessionCost } from "./sessionCosts"
import {
  costForSubject,
  costScopeResults,
  costSubjectKey,
  inclusiveCostSubject,
  resultComponentCost,
  resultModels,
  sameCostSubject,
  subagentCostSubject,
  subagentsCostSubject,
  topLevelCostSubject,
} from "./sessionCosts"

function cost(
  subject: LocalCostSubject,
  totalCostUsd: number,
  over: Partial<LocalSessionCost> = {},
): LocalSessionCost {
  return {
    subject,
    inputTokens: 1,
    outputTokens: 2,
    cacheReadTokens: 3,
    cacheCreationTokens: 4,
    totalTokens: 10,
    inputCostUsd: totalCostUsd / 4,
    outputCostUsd: totalCostUsd / 4,
    cacheReadCostUsd: totalCostUsd / 4,
    cacheWriteCostUsd: totalCostUsd / 4,
    totalCostUsd,
    isActive: false,
    ...over,
  }
}

describe("costSubjectKey", () => {
  it("cannot collide a parent, aggregate, child, or sibling", () => {
    const keys = [
      topLevelCostSubject("claude-code", "parent"),
      subagentsCostSubject("claude-code", "parent"),
      inclusiveCostSubject("claude-code", "parent"),
      subagentCostSubject("claude-code", "parent", "child-a"),
      subagentCostSubject("claude-code", "parent", "child-b"),
    ].map(costSubjectKey)
    expect(new Set(keys).size).toBe(keys.length)
  })

  it("isolates native and WSL identities, case-insensitively", () => {
    const native = costSubjectKey(inclusiveCostSubject("codex", "same"))
    const ubuntu = costSubjectKey(inclusiveCostSubject("codex", "same", "Ubuntu"))
    const debian = costSubjectKey(inclusiveCostSubject("codex", "same", "Debian"))
    expect(new Set([native, ubuntu, debian]).size).toBe(3)
    expect(costSubjectKey(inclusiveCostSubject("codex", "same", "UBUNTU"))).toBe(ubuntu)
  })

  it("separates two agents that reuse one session id", () => {
    expect(costSubjectKey(inclusiveCostSubject("codex", "same"))).not.toBe(
      costSubjectKey(inclusiveCostSubject("claude-code", "same")),
    )
  })

  it("cannot be forged by a separator character inside an id", () => {
    expect(costSubjectKey(inclusiveCostSubject("a", "b|c"))).not.toBe(
      costSubjectKey(inclusiveCostSubject("a|b", "c")),
    )
  })

  it("agrees with sameCostSubject", () => {
    expect(
      sameCostSubject(
        subagentCostSubject("codex", "p", "c", "Ubuntu"),
        subagentCostSubject("codex", "p", "c", "ubuntu"),
      ),
    ).toBe(true)
    expect(
      sameCostSubject(
        subagentCostSubject("codex", "p", "c"),
        subagentCostSubject("codex", "p", "other"),
      ),
    ).toBe(false)
  })
})

describe("costScopeResults / costForSubject", () => {
  const parent = topLevelCostSubject("claude-code", "parent")
  const children = subagentsCostSubject("claude-code", "parent")
  const all = inclusiveCostSubject("claude-code", "parent")
  const childA = subagentCostSubject("claude-code", "parent", "child-a")

  const scopes = {
    topLevel: cost(parent, 32.95),
    subagents: cost(children, 8.5),
    inclusive: cost(all, 41.45),
    children: [cost(childA, 7.54)],
  }

  it("flattens every present scope and drops the absent ones", () => {
    expect(costScopeResults(scopes).map((r) => r.totalCostUsd)).toEqual([
      32.95, 8.5, 41.45, 7.54,
    ])
    expect(costScopeResults({ topLevel: null, children: [] })).toEqual([])
  })

  it("never substitutes a parent result for a child", () => {
    const results = costScopeResults(scopes)
    expect(costForSubject(childA, results)?.totalCostUsd).toBe(7.54)
    expect(costForSubject(all, results)?.totalCostUsd).toBe(41.45)
    expect(
      costForSubject(subagentCostSubject("claude-code", "parent", "missing"), results),
    ).toBeNull()
  })
})

describe("resultComponentCost", () => {
  it("renames the wire fields onto the breakdown vocabulary", () => {
    expect(resultComponentCost(cost(inclusiveCostSubject("codex", "p"), 4))).toEqual({
      totalUsd: 4,
      inputUsd: 1,
      outputUsd: 1,
      cacheReadUsd: 1,
      cacheWriteUsd: 1,
      totalTokens: 10,
      inputTokens: 1,
      outputTokens: 2,
      cacheReadTokens: 3,
      cacheWriteTokens: 4,
    })
  })
})

describe("resultModels", () => {
  const subject = inclusiveCostSubject("codex", "parent")

  it("prefers the per-model breakdown and sorts it", () => {
    expect(
      resultModels(
        cost(subject, 2.4, {
          model: "ignored-when-a-breakdown-exists",
          modelBreakdown: { "model-b": {}, "model-a": {} },
        }),
      ),
    ).toEqual(["model-a", "model-b"])
  })

  it("falls back to the singular model when there is no breakdown", () => {
    expect(resultModels(cost(subject, 2.4, { model: "model-a" }))).toEqual(["model-a"])
    expect(resultModels(cost(subject, 2.4, { model: "model-a", modelBreakdown: {} }))).toEqual([
      "model-a",
    ])
  })

  it("drops blank model keys rather than rendering an empty row", () => {
    expect(resultModels(cost(subject, 2.4, { modelBreakdown: { "  ": {} } }))).toEqual([])
    expect(resultModels(cost(subject, 2.4, { model: "   " }))).toEqual([])
    expect(resultModels(cost(subject, 2.4))).toEqual([])
  })
})
