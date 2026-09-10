import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload } from "../../lib/insightsIpc"
import { CHECK_UI, checkRowPresentation } from "./checkUi"

function category(overrides: Partial<ChecksCategoryPayload> = {}): ChecksCategoryPayload {
  return {
    id: "oldModelUsage",
    finding: 2,
    clean: 3,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: 800,
    ...overrides,
  }
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
      metric: "8% token burn",
      iconTone: "bg-system-red/10 text-system-red-text",
      metricTone: "text-system-red-text",
    })
  })

  it("provides the shared passed row content and status colors", () => {
    expect(
      checkRowPresentation(
        category({ finding: 0, clean: 5, estimatedTokenBurnBasisPoints: 0 }),
      ),
    ).toMatchObject({
      label: "Old model usage",
      summary: "5 passed",
      metric: null,
      iconTone: "bg-system-green/10 text-system-green",
      metricTone: null,
    })
  })

  it("keeps an independent token burn metric for every failed check", () => {
    const estimates = [
      ["sessionsOverDepth", 800, "8% token burn"],
      ["modelOverthinking", 350, "3% token burn"],
      ["overpoweredSubagents", 880, "8% token burn"],
      ["unusedMcpServers", 100, "1% token burn"],
      ["unusedBuiltInTools", 1, "<1% token burn"],
      ["unusedSkills", 100, "1% token burn"],
      ["oldModelUsage", 400, "4% token burn"],
      ["overuseOfFastMode", 333, "3% token burn"],
      ["cacheChurn", 700, "7% token burn"],
    ] as const

    for (const [id, estimatedTokenBurnBasisPoints, metric] of estimates) {
      expect(checkRowPresentation(category({ id, estimatedTokenBurnBasisPoints })).metric).toBe(
        metric,
      )
    }
  })
})
