import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../insightsIpc"
import { checksPresentation, formatTokenBurnPercent, tokenBurnTone } from "./checks"

function category(overrides: Partial<ChecksCategoryPayload> = {}): ChecksCategoryPayload {
  return {
    id: "cacheChurn",
    finding: 5,
    clean: 5,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: 1_250,
    ...overrides,
  }
}

function report(categories: ChecksCategoryPayload[]): ChecksReportPayload {
  return {
    evidenceSettled: true,
    estimatedTokenBurnBasisPoints: 1_625,
    categories,
  }
}

describe("Checks presentation", () => {
  it("uses the report-owned cohort token estimate", () => {
    const presentation = checksPresentation(report([category()]))
    expect(presentation.estimate.tokenBurnBasisPoints).toBe(1_625)
  })

  it("sorts findings by their cohort token estimates", () => {
    const presentation = checksPresentation(
      report([
        category({ id: "cacheChurn", estimatedTokenBurnBasisPoints: 500 }),
        category({ id: "modelOverthinking", estimatedTokenBurnBasisPoints: 1_250 }),
      ]),
    )
    expect(presentation.failures.map((item) => item.id)).toEqual([
      "modelOverthinking",
      "cacheChurn",
    ])
  })

  it("keeps confirmed clean results from partially covered categories", () => {
    const presentation = checksPresentation(
      report([
        category({
          finding: 0,
          clean: 0,
          unavailable: 10,
        }),
        category({
          id: "sessionsOverDepth",
          finding: 0,
          clean: 8,
          unavailable: 2,
        }),
        category({
          id: "oldModelUsage",
          finding: 0,
          clean: 10,
        }),
      ]),
    )
    expect(presentation.failures).toEqual([])
    expect(presentation.wins.map((item) => item.id)).toEqual([
      "sessionsOverDepth",
      "oldModelUsage",
    ])
    expect(presentation.unavailable).toHaveLength(1)
  })

  it("does not synthesize an estimate when cohort token totals are incomplete", () => {
    const presentation = checksPresentation({
      ...report([category()]),
      estimatedTokenBurnBasisPoints: null,
    })
    expect(presentation.estimate.tokenBurnBasisPoints).toBeNull()
  })

  it("rounds token burn estimates up to whole percentages", () => {
    expect(formatTokenBurnPercent(1_625)).toBe("17%")
    expect(formatTokenBurnPercent(5)).toBe("1%")
    expect(formatTokenBurnPercent(1_800)).toBe("18%")
    expect(formatTokenBurnPercent(0)).toBe("0%")
  })

  it("assigns the requested token burn color thresholds", () => {
    expect(tokenBurnTone(0)).toBe("text-system-yellow")
    expect(tokenBurnTone(99)).toBe("text-system-yellow")
    expect(tokenBurnTone(100)).toBe("text-system-yellow")
    expect(tokenBurnTone(499)).toBe("text-system-yellow")
    expect(tokenBurnTone(500)).toBe("text-system-yellow")
    expect(tokenBurnTone(1_499)).toBe("text-system-yellow")
    expect(tokenBurnTone(1_500)).toBe("text-system-yellow")
    expect(tokenBurnTone(1_501)).toBe("text-system-red-text")
  })
})
