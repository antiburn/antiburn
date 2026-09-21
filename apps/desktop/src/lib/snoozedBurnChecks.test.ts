import { describe, expect, it } from "vitest"

import {
  activeChecksReport,
  snoozeUntil,
  visibleCheckCategories,
  visibleSessionHygieneChecks,
  visibleUnusedContextRows,
} from "./snoozedBurnChecks"
import { sessionHygieneChecks } from "./presentation/sessionHygiene"

const payload = {
  evidenceState: "ready" as const,
  unusedResources: null,
  badges: [
    { id: "sessionOverdepth" as const, status: "finding" as const, notAssessedReason: null },
    { id: "modelOverthinking" as const, status: "clean" as const, notAssessedReason: null },
    { id: "overpoweredSubagents" as const, status: "clean" as const, notAssessedReason: null },
    { id: "obsoleteModel" as const, status: "clean" as const, notAssessedReason: null },
    { id: "fastModeOveruse" as const, status: "clean" as const, notAssessedReason: null },
    {
      id: "excessCacheRehydration" as const,
      status: "clean" as const,
      notAssessedReason: null,
    },
  ],
}

describe("snoozed burn checks", () => {
  it("clamps a calendar-month snooze at the end of the target month", () => {
    const until = snoozeUntil("month", new Date(2026, 0, 31, 12))
    expect(new Date(until!)).toEqual(new Date(2026, 1, 28, 12))
  })

  it("removes only matching session checks", () => {
    const checks = visibleSessionHygieneChecks(
      sessionHygieneChecks(payload),
      new Set(["sessionsOverDepth", "unusedSkills"]),
    )
    expect(checks.map((check) => check.id)).not.toContain("sessionOverdepth")
    expect(checks.map((check) => check.id)).toContain("modelOverthinking")
  })

  it("removes snoozed categories and resource rows before totals are derived", () => {
    const snoozed = new Set(["oldModelUsage", "unusedSkills"] as const)
    const categories = [
      {
        id: "oldModelUsage" as const,
        finding: 2,
        clean: 0,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: 900,
      },
      {
        id: "unusedSkills" as const,
        finding: 1,
        clean: 0,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: 700,
      },
      {
        id: "cacheChurn" as const,
        finding: 1,
        clean: 0,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: 300,
      },
    ]
    expect(visibleCheckCategories(categories, snoozed).map((category) => category.id)).toEqual([
      "cacheChurn",
    ])
    expect(
      visibleUnusedContextRows(
        [
          { name: "server", kind: "MCP server", costUsd: 1 },
          { name: "skill", kind: "Skill", costUsd: 1 },
        ],
        snoozed,
      ),
    ).toEqual([{ name: "server", kind: "MCP server", costUsd: 1 }])
    expect(
      activeChecksReport(
        {
          evidenceSettled: true,
          pendingEvidence: 0,
          estimatedTokenBurnBasisPoints: 900,
          categories,
        },
        snoozed,
      ).estimatedTokenBurnBasisPoints,
    ).toBe(300)
  })
})
