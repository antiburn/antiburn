import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../insightsIpc"
import {
  checksHeroPresentation,
  checksPresentation,
  formatTokenBurnPercent,
  tokenBurnTone,
} from "./checkReport"

function category(overrides: Partial<ChecksCategoryPayload> = {}): ChecksCategoryPayload {
  const finding = overrides.finding ?? 5
  const clean = overrides.clean ?? 5
  return {
    id: "cacheChurn",
    finding,
    clean,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: 1_250,
    lifecycle: finding > 0 ? "failing" : clean > 0 ? "passing" : null,
    ...overrides,
  }
}

function report(categories: ChecksCategoryPayload[]): ChecksReportPayload {
  return {
    evidenceSettled: true,
    windowSessions: 0,
    pendingEvidence: 0,
    deferredEvidence: 0,
    estimatedTokenBurnBasisPoints: 1_625,
    categories,
  }
}

describe("Checks presentation", () => {
  it("does not count a zero-candidate smart check as a passed result", () => {
    const check = category({
      id: "skillOpportunities",
      lifecycle: "passing",
      checking: true,
      finding: 0,
      clean: 0,
      reviewCoverage: { reviewed: 0, total: 0, uncertain: 0, pending: 0, continuing: false },
    })
    const presentation = checksPresentation(report([check]))
    expect(presentation.wins).toEqual([])
    expect(presentation.activeUnavailable).toEqual([check])
    expect(presentation.burnChecks.counts).toEqual({ failed: 0, passed: 0, unassessed: 1 })
    expect(checksHeroPresentation(presentation).result).toBe("No checks assessed")
  })
  it("keeps provisional checks outside durable pass counts and excludes snoozed checks", () => {
    const check = category({ finding: 0, clean: 0, checking: true })
    const source = report([check])
    const before = structuredClone(source)
    const presented = checksPresentation(source)
    expect(presented.wins).toEqual([check])
    expect(presented.activeUnavailable).toEqual([])
    expect(presented.burnChecks.counts).toEqual({ failed: 0, passed: 0, unassessed: 1 })
    expect(checksHeroPresentation(presented).summary).toBeNull()
    expect(checksPresentation(source, false, new Set([check.id])).wins).toEqual([])
    expect(source).toEqual(before)
  })
  it("counts confirmed passes without counting checking rows as passed results", () => {
    const presentation = checksPresentation(
      report([
        category({ id: "ignoredInstructions", finding: 0, clean: 0, checking: true }),
        category({ id: "oldModelUsage", finding: 0, clean: 2 }),
      ]),
    )
    expect(presentation.wins).toHaveLength(2)
    expect(checksHeroPresentation(presentation).summary).toBe("1 check passed")
    expect(presentation.burnChecks.counts).toEqual({ failed: 0, passed: 1, unassessed: 1 })
  })
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
      result: "16% estimated token burn",
      summary: "1 check failed",
      state: "failed",
      tone: "text-system-red-text",
    })
  })

  it("keeps clean assessed results conclusive", () => {
    const presentation = checksPresentation(
      report([
        category({ finding: 0, clean: 8, unavailable: 2 }),
        category({ id: "oldModelUsage", finding: 0, clean: 10, unavailable: 0 }),
      ]),
    )

    expect(checksHeroPresentation(presentation)).toEqual({
      result: "No issues found",
      summary: "2 checks passed",
      state: "passed",
      tone: "text-label",
    })
  })

  it("keeps an in-progress instruction check in the passed group until a finding exists", () => {
    const ongoing = category({
      id: "ignoredInstructions",
      finding: 0,
      clean: 0,
      unavailable: 7,
      lifecycle: "passing",
    })
    const presentation = checksPresentation(report([ongoing]))

    expect(presentation.wins.map((check) => check.id)).toEqual(["ignoredInstructions"])
    expect(presentation.failures).toEqual([])
    expect(checksHeroPresentation(presentation).result).toBe("No checks assessed")
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

  it("keeps a historically failing category in failures even when it also has clean evidence", () => {
    const presentation = checksPresentation(
      report([category({ finding: 1, clean: 8, unavailable: 0 })]),
    )

    expect(presentation.failures.map((item) => item.id)).toEqual(["cacheChurn"])
    expect(presentation.wins).toEqual([])
  })

  it("uses the report lifecycle instead of historical counts for check groups", () => {
    const presentation = checksPresentation(
      report([
        category({ id: "cacheChurn", finding: 0, clean: 8, lifecycle: "failing" }),
        category({
          id: "modelOverthinking",
          finding: 4,
          clean: 0,
          lifecycle: "awaitingVerification",
        }),
        category({ id: "oldModelUsage", finding: 2, clean: 0, lifecycle: "passing" }),
      ]),
    )

    expect(presentation.failures.map((item) => item.id)).toEqual(["cacheChurn"])
    expect(presentation.awaiting?.map((item) => item.id)).toEqual(["modelOverthinking"])
    expect(presentation.wins.map((item) => item.id)).toEqual(["oldModelUsage"])
  })

  it("does not synthesize an estimate when cohort token totals are incomplete", () => {
    const presentation = checksPresentation({
      ...report([category()]),
      estimatedTokenBurnBasisPoints: null,
    })
    expect(presentation.estimate.tokenBurnBasisPoints).toBeNull()
  })

  it("separates active assessed, unavailable, and stored snoozed categories", () => {
    const unavailable = category({
      id: "unusedSkills",
      finding: 0,
      clean: 0,
      unavailable: 4,
      lifecycle: null,
    })
    const passed = category({ id: "oldModelUsage", finding: 0, clean: 4 })
    const presentation = checksPresentation(
      report([category(), passed, unavailable]),
      false,
      new Set(["unusedSkills"]),
    )

    expect(presentation.activeAssessed.map((item) => item.id)).toEqual([
      "cacheChurn",
      "oldModelUsage",
    ])
    expect(presentation.activeUnavailable).toEqual([])
    expect(presentation.snoozed.map((item) => item.id)).toEqual(["unusedSkills"])
    expect(presentation.noActiveChecks).toBe(false)
  })

  it("keeps unavailable active categories distinct from an empty report", () => {
    const presentation = checksPresentation(
      report([
        category({
          id: "unusedSkills",
          finding: 0,
          clean: 0,
          unavailable: 4,
          lifecycle: null,
        }),
      ]),
    )

    expect(presentation.activeAssessed).toEqual([])
    expect(presentation.activeUnavailable.map((item) => item.id)).toEqual(["unusedSkills"])
    expect(presentation.noActiveChecks).toBe(false)
    expect(checksHeroPresentation(presentation).result).toBe("No checks assessed")
    expect(checksPresentation(report([])).noActiveChecks).toBe(true)
    expect(checksPresentation(report([])).noEnabledChecks).toBe(true)
    expect(checksHeroPresentation(checksPresentation(report([]))).result).toBe(
      "No checks enabled",
    )
  })

  it("keeps the running presentation while evidence is unsettled", () => {
    const presentation = checksPresentation({
      ...report([
        category({
          finding: 0,
          clean: 0,
          unavailable: 4,
          lifecycle: null,
        }),
      ]),
      evidenceSettled: false,
      pendingEvidence: 1,
      deferredEvidence: 0,
    })

    expect(presentation.noActiveChecks).toBe(false)
    expect(checksHeroPresentation(presentation).result).toBe("No checks assessed")
    expect(presentation.burnChecks.headline).toBe("Running Burn Checks…")
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
