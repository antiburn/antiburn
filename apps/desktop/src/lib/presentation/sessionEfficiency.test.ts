// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from "vitest"

import type { SessionEfficiency } from "../types/session"
import {
  efficiencyBandWord,
  efficiencyMetrics,
  efficiencyProfile,
  efficiencyThresholdsText,
  efficiencyMetricDescription,
  efficiencyThermometer,
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
  it("derives the three ratios from the totals", () => {
    const m = efficiencyMetrics(totals(), "claude-code")
    expect(m.costPerMTok?.value).toBeCloseTo(40)
    expect(m.realWorkShare?.value).toBeCloseTo(0.34)
    expect(m.rewriteShare?.value).toBeCloseTo(0.12)
    expect(m.unpricedTurns).toBe(0)
    expect(m.profile).toBe("claude")
  })

  it("bands a ok Claude session as ok on every metric", () => {
    const m = efficiencyMetrics(totals(), "claude-code")
    expect(m.costPerMTok?.band).toBe("ok")
    expect(m.realWorkShare?.band).toBe("ok")
    expect(m.rewriteShare?.band).toBe("ok")
  })

  it("bands the Claude edges: cost, rewrite, and real work", () => {
    const cheap = efficiencyMetrics(
      totals({ totalUsd: 5, newWorkUsd: 4, rewriteUsd: 0.2 }),
      "claude-code",
    )
    expect(cheap.costPerMTok?.band).toBe("good")
    expect(cheap.realWorkShare?.band).toBe("good")
    expect(cheap.rewriteShare?.band).toBe("good")

    const dear = efficiencyMetrics(
      totals({ totalUsd: 25, newWorkUsd: 4, rewriteUsd: 7 }),
      "claude-code",
    )
    expect(dear.costPerMTok?.band).toBe("bad")
    expect(dear.realWorkShare?.band).toBe("bad")
    expect(dear.rewriteShare?.band).toBe("bad")
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
      totals({ totalUsd: 4, newWorkUsd: 1.4, rewriteUsd: 0.2 }),
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

  it("places a value on its thermometer by band thirds", () => {
    // Lower is better: good | ok | bad, edges at $33 and $80, top at $160.
    expect(efficiencyThermometer(0, "costPerMTok", "claude")).toEqual({
      segments: ["good", "ok", "bad"],
      position: 0,
    })
    expect(efficiencyThermometer(33, "costPerMTok", "claude").position).toBeCloseTo(1 / 3)
    expect(efficiencyThermometer(80, "costPerMTok", "claude").position).toBeCloseTo(2 / 3)
    expect(efficiencyThermometer(120, "costPerMTok", "claude").position).toBeCloseTo(5 / 6)
    expect(efficiencyThermometer(999, "costPerMTok", "claude").position).toBe(1)
    // Higher is better: bad | ok | good, edges at 18% and 36%.
    const realWork = efficiencyThermometer(0.27, "realWorkShare", "claude")
    expect(realWork.segments).toEqual(["bad", "ok", "good"])
    expect(realWork.position).toBeCloseTo(0.5)
  })

  it("describes each metric in a sentence", () => {
    expect(efficiencyMetricDescription("costPerMTok")).toMatch(/Cost for real work/)
    expect(efficiencyMetricDescription("realWorkShare")).toMatch(/spent on real work/)
    expect(efficiencyMetricDescription("rewriteShare")).toMatch(/rewriting the cache/)
  })

  it("names the direction of a bad reading", () => {
    expect(efficiencyBandWord("good", "costPerMTok")).toBe("good")
    expect(efficiencyBandWord("ok", "rewriteShare")).toBe("ok")
    expect(efficiencyBandWord("bad", "costPerMTok")).toBe("high")
    expect(efficiencyBandWord("bad", "rewriteShare")).toBe("high")
    expect(efficiencyBandWord("bad", "realWorkShare")).toBe("low")
  })

  it("spells the thresholds for the agent's profile", () => {
    expect(efficiencyThresholdsText("costPerMTok", "claude")).toBe(
      "[Good = below $33; High = over $80; otherwise OK]",
    )
    expect(efficiencyThresholdsText("realWorkShare", "codex")).toBe(
      "[Good = over 33%; Low = below 17%; otherwise OK]",
    )
    expect(efficiencyThresholdsText("rewriteShare", "codex")).toBe(
      "[Good = below 8%; High = over 14%; otherwise OK]",
    )
  })
})
