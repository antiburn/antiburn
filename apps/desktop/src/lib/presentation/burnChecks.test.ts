import { describe, expect, it } from "vitest"

import type { ChecksReportPayload } from "../insightsIpc"
import {
  aggregateBurnCheckPresentation,
  emptyBurnCheckPresentation,
  sessionBurnCheckPresentation,
} from "./burnChecks"

function sessionStatuses(...statuses: Array<"finding" | "clean" | "notAssessed">) {
  return statuses.map((status) => ({ status }))
}

function report(
  categories: ChecksReportPayload["categories"],
  evidenceSettled = true,
): ChecksReportPayload {
  return {
    pendingEvidence: 0,
    evidenceSettled,
    estimatedTokenBurnBasisPoints: null,
    categories,
  }
}

function category(
  finding: number,
  clean: number,
  unavailable = 0,
): ChecksReportPayload["categories"][number] {
  return {
    id: "cacheChurn",
    finding,
    clean,
    unavailable,
    estimatedTokenBurnBasisPoints: null,
  }
}

describe("sessionBurnCheckPresentation", () => {
  it("uses terminal language only for complete nonempty results", () => {
    const passed = sessionBurnCheckPresentation(sessionStatuses("clean", "clean"), "ready")
    expect(passed.state).toBe("allPassed")
    expect(passed.headline).toBe("All Burn Checks passed")
    expect(passed.compactPhrases.map((phrase) => phrase.text)).toEqual(["2 passed"])
    expect(passed.indicator.kind).toBe("pass")

    const failed = sessionBurnCheckPresentation(sessionStatuses("finding", "finding"), "ready")
    expect(failed.state).toBe("allFailed")
    expect(failed.headline).toBe("All Burn Checks failed")
    expect(failed.indicator.kind).toBe("fail")
  })

  it("pluralizes mixed compact phrases without repeating the noun", () => {
    const value = sessionBurnCheckPresentation(
      sessionStatuses("finding", "clean", "clean", "clean"),
      "ready",
    )
    expect(value.headline).toBe("Some Burn Checks failed")
    expect(value.compactPhrases.map((phrase) => phrase.text)).toEqual(["1 failed", "3 passed"])
    expect(value.breakdownPhrases.map((phrase) => phrase.text)).toEqual([
      "1 failed",
      "3 passed",
    ])
  })

  it("uses a pass verdict while keeping unassessed coverage visible", () => {
    const value = sessionBurnCheckPresentation(
      sessionStatuses("clean", "clean", "notAssessed"),
      "ready",
    )
    expect(value.state).toBe("assessedPassed")
    expect(value.headline).toBe("All assessed Burn Checks passed")
    expect(value.compactPhrases.map((phrase) => phrase.text)).toEqual([
      "2 passed",
      "1 not assessed",
    ])
    expect(value.breakdownPhrases.map((phrase) => phrase.text)).toEqual([
      "2 passed",
      "1 not assessed",
    ])
    expect(value.indicator.kind).toBe("pass")
    expect(value.accessibleDescription).toContain("3 session checks")
  })

  it.each([
    ["pending", "Running Burn Checks…"],
    ["processing", "Running Burn Checks…"],
    ["stale", "Refreshing Burn Checks…"],
    ["activelyGrowing", "Burn Checks awaiting session data"],
    ["unsupported", "Burn Checks not supported"],
    ["failed", "Burn Checks unavailable"],
    ["ready", "Burn Checks not assessed"],
  ] as const)("maps %s evidence with no result to %s", (lifecycle, expected) => {
    const value = sessionBurnCheckPresentation([], lifecycle)
    expect(value.headline).toBe(expected)
    expect(value.compactPhrases).toEqual([{ outcome: "status", text: expected }])
  })

  it("preserves results and discloses stale, growing, and failed refreshes", () => {
    const checks = sessionStatuses("finding", "clean")
    expect(sessionBurnCheckPresentation(checks, "stale").contextPhrases).toEqual([
      "Refreshing",
      "Evidence incomplete",
    ])
    expect(sessionBurnCheckPresentation(checks, "activelyGrowing").contextPhrases).toEqual([
      "Session still growing",
      "Evidence incomplete",
    ])
    expect(sessionBurnCheckPresentation(checks, "failed").contextPhrases).toEqual([
      "Refresh unavailable",
      "Evidence incomplete",
    ])
  })

  it("keeps processing separate from unavailable and unassessed counts", () => {
    const value = sessionBurnCheckPresentation(sessionStatuses("clean"), "processing")
    expect(value.compactPhrases.map((phrase) => phrase.text)).toEqual(["1 passed"])
    expect(value.counts.unassessed).toBe(0)
    expect(value.contextPhrases).toEqual(["Running", "Evidence incomplete"])
  })
})

describe("aggregateBurnCheckPresentation", () => {
  it("counts every report category once and preserves mixed classification", () => {
    const value = aggregateBurnCheckPresentation(
      report([category(2, 3, 4), category(0, 3, 2), category(0, 0, 5)]),
    )
    expect(value.counts).toEqual({ failed: 1, passed: 1, unassessed: 1 })
    expect(value.breakdownPhrases.map((phrase) => phrase.text)).toEqual([
      "1 failed",
      "1 passed",
      "1 not assessed",
    ])
    expect(value.contextPhrases).toContain("Evidence incomplete")
    expect(value.accessibleDescription).toContain("3 report categories")
  })

  it("uses evidenceSettled to keep otherwise complete results nonterminal", () => {
    const value = aggregateBurnCheckPresentation(
      report([category(0, 4), category(0, 9)], false),
    )
    expect(value.state).toBe("assessedPassed")
    expect(value.headline).toBe("All assessed Burn Checks passed")
    expect(value.indicator.kind).toBe("pass")
  })

  it("retains results when a refresh fails", () => {
    const value = aggregateBurnCheckPresentation(report([category(2, 3)]), true)
    expect(value.counts.failed).toBe(1)
    expect(value.compactPhrases[0]?.text).toBe("1 failed")
    expect(value.contextPhrases).toContain("Refresh unavailable")
  })

  it("never turns an empty report into a pass", () => {
    const value = aggregateBurnCheckPresentation(report([]))
    expect(value.state).toBe("notAssessed")
    expect(value.indicator.kind).toBe("neutral")
  })
})

describe("emptyBurnCheckPresentation", () => {
  it("distinguishes report loading from unavailability", () => {
    expect(emptyBurnCheckPresentation("pending").headline).toBe("Running Burn Checks…")
    expect(emptyBurnCheckPresentation("unavailable").headline).toBe("Burn Checks unavailable")
  })
})
