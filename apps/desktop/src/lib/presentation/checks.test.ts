import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../insightsIpc"
import {
  checksHeroPresentation,
  checksPresentation,
  formatTokenBurnPercent,
  tokenBurnTone,
} from "./checks"

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

  it("presents the same concise failed hero on every checks surface", () => {
    expect(checksHeroPresentation(checksPresentation(report([category()])))).toEqual({
      result: "16% token burn",
      summary: "1 check failed",
      state: "failed",
      tone: "text-system-red-text",
    })
  })

  it("states when clean assessed results still need more evidence", () => {
    const presentation = checksPresentation(
      report([
        category({ finding: 0, clean: 8, unavailable: 2 }),
        category({ id: "oldModelUsage", finding: 0, clean: 10, unavailable: 0 }),
      ]),
    )

    expect(checksHeroPresentation(presentation)).toEqual({
      result: "No issues found where assessed",
      summary: "More evidence is needed",
      state: "pending",
      tone: "text-label",
    })
  })

  it("keeps passed hero text neutral", () => {
    const presentation = checksPresentation(
      report([category({ finding: 0, clean: 10, unavailable: 0 })]),
    )

    expect(checksHeroPresentation(presentation).tone).toBe("text-label")
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

  it("floors basis-point estimates to whole percentages", () => {
    expect(formatTokenBurnPercent(1_625)).toBe("16%")
    expect(formatTokenBurnPercent(1_250)).toBe("12%")
    expect(formatTokenBurnPercent(99)).toBe("<1%")
    expect(formatTokenBurnPercent(1)).toBe("<1%")
    expect(formatTokenBurnPercent(1_800)).toBe("18%")
    expect(formatTokenBurnPercent(0)).toBe("0%")
  })

  it("uses green for zero, yellow below five percent, and red from five percent", () => {
    expect(tokenBurnTone(0)).toBe("text-system-green")
    expect(tokenBurnTone(1)).toBe("text-system-yellow")
    expect(tokenBurnTone(499)).toBe("text-system-yellow")
    expect(tokenBurnTone(500)).toBe("text-system-red-text")
    expect(tokenBurnTone(1_500)).toBe("text-system-red-text")
  })
})
