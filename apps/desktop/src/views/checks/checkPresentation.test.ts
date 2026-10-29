import { describe, expect, it } from "vitest"

import type { BurnCheckTargetPayload, ChecksCategoryPayload } from "../../lib/insightsIpc"
import { CHECK_UI, checkRowPresentation } from "./checkPresentation"

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
  it("keeps unassessed categories neutral and preserves verification", () => {
    const check = category({ finding: 0, clean: 0 })
    expect(checkRowPresentation(check)).toMatchObject({
      summary: "Not assessed",
      checking: false,
      metric: null,
      iconTone: "bg-surface-card text-label-secondary",
    })
    expect(checkRowPresentation({ ...check, unavailable: 1 })).toMatchObject({
      summary: "Not assessed",
    })
    expect(checkRowPresentation({ ...check, lifecycle: "awaitingVerification" })).toMatchObject(
      {
        summary: "Verifying",
        checking: false,
      },
    )
  })

  it("provides one short recommendation with its reason", () => {
    expect(CHECK_UI.oldModelUsage).toMatchObject({
      recommendation:
        "Use the reviewed replacement for new sessions to do similar work at a lower estimated API cost.",
    })
    expect(CHECK_UI.oldModelUsage).not.toHaveProperty("why")
  })

  it("provides the shared failed row content and status colors", () => {
    expect(checkRowPresentation(category())).toMatchObject({
      label: "Old model usage",
      summary: "2 failed · 3 passed",
      metric: "8% estimated burn",
      iconTone: "bg-system-red/10 text-system-red-text",
      metricTone: "text-system-red-text",
    })
  })

  it.each([
    {
      finding: 1,
      clean: 0,
      unavailable: 0,
      lifecycle: "failing" as const,
      summary: "1 failed",
    },
    { finding: 0, clean: 1, unavailable: 2, lifecycle: "passing" as const, summary: "Passed" },
    { finding: 0, clean: 0, unavailable: 2, lifecycle: null, summary: "Not assessed" },
  ])("keeps skill evidence state separate from savings: $summary", ({ summary, ...state }) => {
    expect(
      checkRowPresentation(
        category({
          id: "unusedSkills",
          estimatedTokenBurnBasisPoints: null,
          ...state,
        }),
        [target(null)],
      ),
    ).toMatchObject({
      label: "Unused skills",
      summary,
      metric: null,
      costLine: null,
    })
  })

  it("provides the shared passed row content without a burn estimate", () => {
    expect(
      checkRowPresentation(
        category({ finding: 0, clean: 5, estimatedTokenBurnBasisPoints: 0 }),
      ),
    ).toMatchObject({
      label: "Old model usage",
      summary: "Passed",
      metric: null,
      iconTone: "bg-system-green/10 text-system-green",
      metricTone: null,
    })
  })

  it("keeps an incomplete row unassessed until evidence is complete", () => {
    expect(
      checkRowPresentation(
        category({
          id: "sessionsOverDepth",
          finding: 0,
          clean: 0,
          unavailable: 7,
          lifecycle: null,
        }),
      ),
    ).toMatchObject({ summary: "Not assessed", metric: null })
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
