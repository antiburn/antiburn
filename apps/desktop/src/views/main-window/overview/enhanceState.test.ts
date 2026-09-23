import { describe, expect, it } from "vitest"

import type {
  BurnCheckDetectorId,
  BurnCheckTargetPayload,
  ChecksReportPayload,
} from "../../../lib/insightsIpc"
import {
  enhanceButtonState,
  isAppliedFix,
  predictSavings,
  projectSavings,
} from "./enhanceState"

function target(
  unit: string | null,
  value = 0,
  watch: Partial<NonNullable<BurnCheckTargetPayload["watch"]>> | null = null,
): BurnCheckTargetPayload {
  return {
    display: { estimatedOpportunity: unit ? { value, unit } : null },
    watch,
  } as unknown as BurnCheckTargetPayload
}

describe("enhanceButtonState", () => {
  it("offers a fresh run until the reader starts", () => {
    expect(enhanceButtonState(null, 0, {})).toEqual({ kind: "loading" })
    expect(enhanceButtonState(["a", "b"], 3, {})).toEqual({ kind: "fresh", count: 2 })
    expect(enhanceButtonState([], 0, {})).toEqual({ kind: "clear" })
  })

  it("resumes an unfinished run, even while the report loads", () => {
    const prefs = { enhanceStartedAt: 20, enhanceCompletedAt: 10, enhanceStep: 3 as const }
    expect(enhanceButtonState(null, 0, prefs)).toEqual({ kind: "resume", step: 3 })
    expect(enhanceButtonState(["a"], 2, prefs)).toEqual({ kind: "resume", step: 3 })
  })

  it("puts new failures first, then watching", () => {
    const prefs = { enhanceStartedAt: 10, enhanceCompletedAt: 20, enhanceSeenFailing: ["a"] }
    expect(enhanceButtonState(["a", "b"], 1, prefs)).toEqual({ kind: "new", count: 1 })
    expect(enhanceButtonState(["a"], 1, prefs)).toEqual({ kind: "watching", count: 1 })
    expect(enhanceButtonState(["a"], 0, prefs)).toEqual({ kind: "fresh", count: 1 })
  })
})

describe("projectSavings", () => {
  it("keeps tokens and dollars apart over three months", () => {
    expect(
      projectSavings([
        target("literalInputTokens", 1000),
        target("cacheClassTokens", 500),
        target("apiEquivalentUsd", 2),
        target("improvements", 4),
        target(null),
      ]),
    ).toEqual({ tokens: 4500, usd: 6, estimated: 3 })
  })
})

describe("predictSavings", () => {
  // Mask 0b10 is modelOverthinking, 0b11 adds sessionsOverDepth.
  const byMask: Array<number | null> = Array.from({ length: 512 }, () => null)
  byMask[0b10] = 250
  byMask[0b11] = 400
  function report(tokenBurnDenominator: number | null): ChecksReportPayload {
    return {
      tokenBurnDenominator,
      estimatedTokenBurnBasisPointsByDetectorMask: byMask,
    } as unknown as ChecksReportPayload
  }
  const checks = (...ids: BurnCheckDetectorId[]) => new Set(ids)

  it("projects the combined burn of the fixed checks over three months", () => {
    expect(predictSavings(report(1_000_000), checks("modelOverthinking"))).toEqual({
      tokens: 75_000,
      basisPoints: 250,
    })
    expect(
      predictSavings(report(1_000_000), checks("modelOverthinking", "sessionsOverDepth")),
    ).toEqual({ tokens: 120_000, basisPoints: 400 })
  })

  it("predicts nothing without fixes, a denominator, or a measured burn", () => {
    expect(predictSavings(report(1_000_000), checks())).toBeNull()
    expect(predictSavings(report(null), checks("modelOverthinking"))).toBeNull()
    expect(predictSavings(report(1_000_000), checks("cacheChurn"))).toBeNull()
  })
})

describe("isAppliedFix", () => {
  it("counts action watches that did not recur", () => {
    expect(isAppliedFix(target(null, 0, { origin: "action", lifecycle: "watching" }))).toBe(
      true,
    )
    expect(isAppliedFix(target(null, 0, { origin: "action", lifecycle: "recurred" }))).toBe(
      false,
    )
    expect(isAppliedFix(target(null, 0, { origin: "passive", lifecycle: "fixed" }))).toBe(false)
    expect(isAppliedFix(target(null))).toBe(false)
  })
})
