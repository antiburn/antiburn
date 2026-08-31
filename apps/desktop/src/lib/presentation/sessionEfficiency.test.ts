import { describe, expect, it } from "vitest"

import type { SessionEfficiency } from "../types/session"
import {
  efficiencyBandWord,
  efficiencyMetrics,
  efficiencyProfile,
  efficiencyThresholdGuidance,
  formatCostPerMTok,
  formatSharePercent,
} from "./sessionEfficiency"

function totals(over: Partial<SessionEfficiency> = {}): SessionEfficiency {
  return {
    totalUsd: 10,
    newWorkUsd: 3.4,
    carryUsd: 5.4,
    rewriteUsd: 1.2,
    growthTokens: 200_000,
    outputTokens: 50_000,
    pricedTurns: 12,
    unpricedTurns: 0,
    ...over,
  }
}

describe("efficiencyMetrics", () => {
  it("derives the headline cost and three spend shares", () => {
    const m = efficiencyMetrics(totals(), "claude-code")
    expect(m.costPerMTok?.value).toBeCloseTo(40)
    expect(m.realWorkShare?.value).toBeCloseTo(0.34)
    expect(m.rewriteShare?.value).toBeCloseTo(0.12)
    expect(m.carryShare?.value).toBeCloseTo(0.54)
    expect(m.unpricedTurns).toBe(0)
    expect(m.profile).toBe("claude")
  })

  it("bands an ok Claude session as ok on every metric", () => {
    const m = efficiencyMetrics(totals(), "claude-code")
    expect(m.costPerMTok?.band).toBe("ok")
    expect(m.realWorkShare?.band).toBe("ok")
    expect(m.rewriteShare?.band).toBe("ok")
    expect(m.carryShare?.band).toBe("ok")
  })

  it("bands the Claude edges for each metric", () => {
    const cheap = efficiencyMetrics(
      totals({ totalUsd: 5, newWorkUsd: 4, carryUsd: 0.8, rewriteUsd: 0.2 }),
      "claude-code",
    )
    expect(cheap.costPerMTok?.band).toBe("good")
    expect(cheap.realWorkShare?.band).toBe("good")
    expect(cheap.rewriteShare?.band).toBe("good")
    expect(cheap.carryShare?.band).toBe("good")

    const dear = efficiencyMetrics(
      totals({ totalUsd: 25, newWorkUsd: 4, carryUsd: 14, rewriteUsd: 7 }),
      "claude-code",
    )
    expect(dear.costPerMTok?.band).toBe("bad")
    expect(dear.realWorkShare?.band).toBe("bad")
    expect(dear.rewriteShare?.band).toBe("bad")

    const highCarry = efficiencyMetrics(
      totals({ totalUsd: 10, newWorkUsd: 2, carryUsd: 6, rewriteUsd: 2 }),
      "claude-code",
    )
    expect(highCarry.carryShare?.band).toBe("bad")
  })

  it("treats the Claude gaps between ok and bad as ok", () => {
    // Rewrite 22% sits between the 20% ok top and the 25% bad edge.
    const rewrite = efficiencyMetrics(totals({ rewriteUsd: 2.2 }), "claude-code")
    expect(rewrite.rewriteShare?.band).toBe("ok")
    // Real work at 19% sits between the bad edge and the good edge.
    const realWork = efficiencyMetrics(totals({ newWorkUsd: 1.9 }), "claude-code")
    expect(realWork.realWorkShare?.band).toBe("ok")
  })

  it("reads a Codex session against the Codex bands", () => {
    // $40/MTok is ok for Claude and high for Codex.
    const m = efficiencyMetrics(totals(), "codex")
    expect(m.profile).toBe("codex")
    expect(m.costPerMTok?.band).toBe("ok")
    const dear = efficiencyMetrics(totals({ totalUsd: 12, rewriteUsd: 0.6 }), "codex")
    expect(dear.costPerMTok?.band).toBe("bad")
    expect(dear.rewriteShare?.band).toBe("good")
    const good = efficiencyMetrics(
      totals({ totalUsd: 4, newWorkUsd: 1.4, carryUsd: 2.4, rewriteUsd: 0.2 }),
      "codex",
    )
    expect(good.costPerMTok?.band).toBe("good")
    expect(good.realWorkShare?.band).toBe("good")
  })

  it("returns null metrics when there is no spend", () => {
    const m = efficiencyMetrics(totals({ totalUsd: 0, newWorkUsd: 0, rewriteUsd: 0 }), "codex")
    expect(m.costPerMTok).toBeNull()
    expect(m.realWorkShare).toBeNull()
    expect(m.rewriteShare).toBeNull()
    expect(m.carryShare).toBeNull()
  })

  it("returns a null cost per MTok when the token denominator is zero", () => {
    const m = efficiencyMetrics(totals({ growthTokens: 0, outputTokens: 0 }), "claude-code")
    expect(m.costPerMTok).toBeNull()
    expect(m.realWorkShare).not.toBeNull()
  })

  it("carries the unpriced turn count through", () => {
    expect(efficiencyMetrics(totals({ unpricedTurns: 3 }), "claude-code").unpricedTurns).toBe(3)
  })
})

describe("efficiencyProfile", () => {
  it("uses the Claude bands for every agent but Codex", () => {
    expect(efficiencyProfile("codex")).toBe("codex")
    expect(efficiencyProfile("claude-code")).toBe("claude")
    expect(efficiencyProfile("cursor")).toBe("claude")
  })
})

describe("formatting", () => {
  it("shows cost per MTok to two decimals and shares to two significant figures", () => {
    expect(formatCostPerMTok(41.4)).toBe("$41.40")
    expect(formatCostPerMTok(3.14159)).toBe("$3.14")
    expect(formatSharePercent(0.336)).toBe("34%")
    expect(formatSharePercent(0.0712)).toBe("7.1%")
    expect(formatSharePercent(0.00684)).toBe("0.68%")
    expect(formatSharePercent(1)).toBe("100%")
    expect(formatSharePercent(0)).toBe("0%")
  })

  it("names the direction of a bad reading", () => {
    expect(efficiencyBandWord("good", "costPerMTok")).toBe("good")
    expect(efficiencyBandWord("ok", "rewriteShare")).toBe("ok")
    expect(efficiencyBandWord("bad", "costPerMTok")).toBe("high")
    expect(efficiencyBandWord("bad", "rewriteShare")).toBe("high")
    expect(efficiencyBandWord("bad", "realWorkShare")).toBe("low")
    expect(efficiencyBandWord("bad", "carryShare")).toBe("high")
  })

  it("spells the thresholds for the agent's profile", () => {
    expect(efficiencyThresholdGuidance("costPerMTok", "claude")).toEqual([
      "For Claude, aim for below $33. Above $80 is too high.",
    ])
    expect(efficiencyThresholdGuidance("realWorkShare", "codex")).toEqual([
      "For Codex, aim for above 33%. Below 17% is too low.",
    ])
    expect(efficiencyThresholdGuidance("rewriteShare", "codex")).toEqual([
      "For Codex, aim for below 8%. Above 14% is too high.",
    ])
    expect(efficiencyThresholdGuidance("carryShare", "codex")).toEqual([
      "For Codex, aim for below 59%. Above 69% is too high.",
    ])
  })
})
