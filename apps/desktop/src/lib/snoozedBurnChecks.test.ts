import { describe, expect, it } from "vitest"

import { snoozeUntil, visibleSessionHygieneChecks } from "./snoozedBurnChecks"
import { sessionHygieneChecks } from "./presentation/sessionHygiene"

const payload = {
  evidenceState: "ready" as const,
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
})
