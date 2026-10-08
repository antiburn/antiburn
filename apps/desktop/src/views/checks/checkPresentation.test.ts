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
  it("uses only category activity and preserves verification", () => {
    const check = category({ finding: 0, clean: 0, checking: true })
    expect(checkRowPresentation(check)).toMatchObject({
      summary: "No issues found yet",
      provisional: true,
      checking: true,
      metric: null,
      iconTone: "bg-system-green/10 text-system-green",
    })
    expect(checkRowPresentation({ ...check, checking: false })).toMatchObject({
      summary: "Not assessed",
      provisional: false,
      checking: false,
    })
    expect(checkRowPresentation({ ...check, unavailable: 1 })).toMatchObject({
      summary: "No issues found yet",
      provisional: true,
    })
    expect(checkRowPresentation({ ...check, lifecycle: "awaitingVerification" })).toMatchObject(
      {
        summary: "Awaiting verification",
        provisional: false,
        checking: false,
      },
    )
  })

  it("shows supplied coverage counts and limits continuation to active work", () => {
    const check = category({
      sampled: true,
      partialContext: true,
      reviewCoverage: { reviewed: 4, total: 8, uncertain: 1, pending: 3, continuing: true },
    })
    expect(checkRowPresentation(check).coverage).toBe(
      "4 of 8 reviewed · 1 uncertain · 3 pending",
    )
    expect(checkRowPresentation(check).evidenceLimits).toHaveLength(2)
    expect(checkRowPresentation(check).evidenceLimits[0]!.details).not.toContain(
      "Review is continuing.",
    )
    expect(
      checkRowPresentation({ ...check, checking: true }).evidenceLimits[0]!.details,
    ).toContain("Review is continuing.")
    expect(checkRowPresentation(category()).coverage).toBeNull()
  })
  it("presents Scope Creep with future guidance and no inferred estimate", () => {
    expect(
      checkRowPresentation(category({ id: "scopeCreep", estimatedTokenBurnBasisPoints: null })),
    ).toMatchObject({
      label: "Scope creep",
      summary: "2/5 sessions failed",
      metric: null,
      costLine: null,
    })
    expect(CHECK_UI.scopeCreep.recommendation).toBe(
      "Keep future work within the agreed task. Ask for approval before adding work.",
    )
  })
  it("provides one short recommendation with its reason", () => {
    expect(CHECK_UI.oldModelUsage).toMatchObject({
      recommendation:
        "Use the reviewed replacement for new sessions to support the same work at a lower API-equivalent cost.",
    })
    expect(CHECK_UI.oldModelUsage).not.toHaveProperty("why")
    expect(CHECK_UI.skillOpportunities.recommendation).toBe(
      "Use this current skill for similar future work.",
    )
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

  it("presents Skill Opportunities through the shared check row", () => {
    expect(
      checkRowPresentation(category({ id: "skillOpportunities", finding: 2, clean: 1 })),
    ).toMatchObject({ label: "Skill opportunities", summary: "2/3 sessions failed" })
  })

  it("presents Over-exploring without inventing an estimate", () => {
    expect(
      checkRowPresentation(
        category({ id: "overExploring", estimatedTokenBurnBasisPoints: null }),
      ),
    ).toMatchObject({
      label: "Over-exploring",
      summary: "2/5 sessions failed",
      metric: null,
      costLine: null,
    })
  })

  it.each([
    {
      finding: 1,
      clean: 0,
      unavailable: 0,
      lifecycle: "failing" as const,
      summary: "1/1 session failed",
    },
    { finding: 0, clean: 1, unavailable: 2, lifecycle: "passing" as const, summary: "Passed" },
    { finding: 0, clean: 0, unavailable: 2, lifecycle: null, summary: "Not assessed" },
  ])("keeps skill evidence state separate from savings: $summary", ({ summary, ...state }) => {
    expect(
      checkRowPresentation(
        category({
          id: "skillOpportunities",
          estimatedTokenBurnBasisPoints: null,
          ...state,
        }),
        [target(null)],
      ),
    ).toMatchObject({
      label: "Skill opportunities",
      summary,
      metric: null,
      costLine: null,
    })
  })

  it("uses the ordinary failed-session wording for ignored instructions", () => {
    expect(
      checkRowPresentation(category({ id: "ignoredInstructions", finding: 2, clean: 0 })),
    ).toMatchObject({
      label: "Ignored instructions",
      summary: "2/2 sessions failed",
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

  it("keeps an in-progress instruction row unassessed until evidence is complete", () => {
    expect(
      checkRowPresentation(
        category({
          id: "ignoredInstructions",
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
