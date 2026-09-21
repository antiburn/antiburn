import { act, renderHook } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => true,
  invoke: vi.fn(async (command: string, args?: { snooze?: unknown }) =>
    command === "set_burn_check_snooze" ? [args?.snooze] : [],
  ),
}))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }))

import {
  activeChecksReport,
  snoozeUntil,
  snoozeBurnCheck,
  useSnoozedBurnChecks,
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
  it("expires stored snoozes at the scheduled time", async () => {
    vi.useFakeTimers()
    try {
      const { result } = renderHook(() => useSnoozedBurnChecks())
      await act(async () => snoozeBurnCheck("cacheChurn", "week"))
      expect(result.current).toHaveLength(1)
      await act(async () => vi.advanceTimersByTime(7 * 24 * 60 * 60 * 1_000))
      expect(result.current).toEqual([])
    } finally {
      vi.useRealTimers()
    }
  })

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

  it("adds active category estimates instead of selecting the largest one", () => {
    const report = activeChecksReport(
      {
        evidenceSettled: true,
        pendingEvidence: 0,
        estimatedTokenBurnBasisPoints: 900,
        categories: [
          {
            id: "cacheChurn",
            finding: 1,
            clean: 0,
            unavailable: 0,
            estimatedTokenBurnBasisPoints: 300,
          },
          {
            id: "oldModelUsage",
            finding: 1,
            clean: 0,
            unavailable: 0,
            estimatedTokenBurnBasisPoints: 800,
          },
          {
            id: "sessionsOverDepth",
            finding: 1,
            clean: 0,
            unavailable: 0,
            estimatedTokenBurnBasisPoints: 100,
          },
        ],
      },
      new Set(["sessionsOverDepth"]),
    )
    expect(report.estimatedTokenBurnBasisPoints).toBe(1_100)
  })
})
