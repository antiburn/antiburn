import { describe, expect, it } from "vitest"

import type { BurnCheckTargetPayload, ChecksCategoryPayload } from "../../lib/insightsIpc"
import { CHECK_UI, checkRowPresentation } from "./checkUi"

function category(overrides: Partial<ChecksCategoryPayload> = {}): ChecksCategoryPayload {
  const finding = overrides.finding ?? 2
  const clean = overrides.clean ?? 3
  return {
    id: "oldModelUsage",
    finding,
    clean,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: 800,
    lifecycle: finding > 0 ? "failing" : clean > 0 ? "passing" : null,
    ...overrides,
  }
}

function target(
  estimatedOpportunity: BurnCheckTargetPayload["display"]["estimatedOpportunity"],
) {
  return {
    display: { estimatedOpportunity },
  } as BurnCheckTargetPayload
}

describe("check row presentation", () => {
  it("provides one short recommendation with its reason", () => {
    expect(CHECK_UI.oldModelUsage).toMatchObject({
      recommendation:
        "Use the reviewed replacement for new sessions to support the same work at a lower API-equivalent cost.",
    })
    expect(CHECK_UI.oldModelUsage).not.toHaveProperty("why")
  })

  it("provides the shared failed row content and status colors", () => {
    expect(checkRowPresentation(category())).toMatchObject({
      label: "Old model usage",
      summary: "2/5 sessions failed",
      metric: "8% estimated burn",
      iconTone: "bg-system-red/10 text-system-red-text",
      metricTone: "text-system-red-text",
    })
  })

  it("asks users to review possible instruction conflicts without calling them failures", () => {
    expect(
      checkRowPresentation(category({ id: "ignoredInstructions", finding: 2, clean: 0 })),
    ).toMatchObject({
      label: "Ignored Instructions",
      summary: "2 sessions need review",
      metric: null,
    })
  })

  it("provides the shared passed row content and status colors", () => {
    expect(
      checkRowPresentation(
        category({ finding: 0, clean: 5, estimatedTokenBurnBasisPoints: 0 }),
      ),
    ).toMatchObject({
      label: "Old model usage",
      summary: "Passed",
      metric: "0% estimated burn",
      iconTone: "bg-system-green/10 text-system-green",
      metricTone: "text-system-green",
    })
  })

  it("keeps an independent token burn metric for every failed check", () => {
    const estimates = [
      ["sessionsOverDepth", 800, "8% estimated burn"],
      ["modelOverthinking", 350, "3% estimated burn"],
      ["overpoweredSubagents", 880, "8% estimated burn"],
      ["unusedMcpServers", 100, "1% estimated burn"],
      ["unusedBuiltInTools", 1, "<1% estimated burn"],
      ["unusedSkills", 100, "1% estimated burn"],
      ["oldModelUsage", 400, "4% estimated burn"],
      ["overuseOfFastMode", 333, "3% estimated burn"],
      ["cacheChurn", 700, "7% estimated burn"],
    ] as const

    for (const [id, estimatedTokenBurnBasisPoints, metric] of estimates) {
      expect(checkRowPresentation(category({ id, estimatedTokenBurnBasisPoints })).metric).toBe(
        metric,
      )
    }
  })

  it("sums the priced opportunity across every loaded target", () => {
    const targets = [
      target({ value: 8.2, unit: "apiEquivalentUsd" }),
      target({ value: 1.8, unit: "apiEquivalentUsd" }),
    ]
    expect(checkRowPresentation(category(), targets).costLine).toBe("~$10.00")
  })

  it("has no cost line when the target list has not loaded", () => {
    expect(checkRowPresentation(category()).costLine).toBeNull()
  })

  it("has no cost line when the check has no findings, even with priced targets", () => {
    const targets = [target({ value: 8.2, unit: "apiEquivalentUsd" })]
    expect(
      checkRowPresentation(category({ finding: 0, clean: 5 }), targets).costLine,
    ).toBeNull()
  })

  it("has no cost line when any target's estimate did not price to dollars", () => {
    const targets = [
      target({ value: 8.2, unit: "apiEquivalentUsd" }),
      target({ value: 400, unit: "literalInputTokens" }),
    ]
    expect(checkRowPresentation(category(), targets).costLine).toBeNull()
  })

  it("has no cost line when a target has no estimate at all", () => {
    const targets = [target({ value: 8.2, unit: "apiEquivalentUsd" }), target(null)]
    expect(checkRowPresentation(category(), targets).costLine).toBeNull()
  })
})
