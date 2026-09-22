import { act, renderHook } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const invoke = vi.hoisted(() => vi.fn())
const listen = vi.hoisted(() => vi.fn().mockResolvedValue(() => undefined))

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => true,
  invoke,
}))
vi.mock("@tauri-apps/api/event", () => ({ listen }))

import {
  activeChecksReport,
  refreshSnoozedBurnChecks,
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

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

describe("snoozed burn checks", () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockImplementation(async (command: string, args?: { snooze?: unknown }) =>
      command === "set_burn_check_snooze" ? [args?.snooze] : [],
    )
    listen.mockClear()
  })

  it("does not expose an empty ready state while the initial read is pending", async () => {
    let resolve!: (records: unknown[]) => void
    invoke.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      }),
    )
    const { result } = renderHook(() => useSnoozedBurnChecks())

    expect(result.current).toEqual({ status: "loading", records: [] })
    await act(async () => resolve([{ detector: "oldModelUsage", scope: "check", until: null }]))
    expect(result.current).toMatchObject({
      status: "ready",
      records: [{ detector: "oldModelUsage" }],
    })
  })

  it("handles rejected reads and retains the last known records", async () => {
    const { result } = renderHook(() => useSnoozedBurnChecks())
    await act(async () => undefined)
    await act(async () => snoozeBurnCheck("cacheChurn", "forever"))
    invoke.mockRejectedValueOnce(new Error("Unavailable"))

    await act(refreshSnoozedBurnChecks)

    expect(result.current.status).toBe("error")
    expect(result.current.records.map((record) => record.detector)).toContain("cacheChurn")
  })

  it("ignores an older read that resolves after a newer read", async () => {
    const older = deferred<unknown[]>()
    const newerRead = deferred<unknown[]>()
    invoke.mockReturnValueOnce(older.promise).mockReturnValueOnce(newerRead.promise)
    const { result } = renderHook(() => useSnoozedBurnChecks())
    const newer = refreshSnoozedBurnChecks()
    newerRead.resolve([{ detector: "unusedSkills", scope: "check", until: null }])
    await act(async () => newer)
    older.resolve([{ detector: "oldModelUsage", scope: "check", until: null }])
    await act(async () => older.promise)

    expect(result.current.records.map((record) => record.detector)).toEqual(["unusedSkills"])
  })

  it("uses one event listener for concurrent subscribers", async () => {
    const first = renderHook(() => useSnoozedBurnChecks())
    const second = renderHook(() => useSnoozedBurnChecks())
    await act(async () => undefined)
    expect(listen).toHaveBeenCalledOnce()
    first.unmount()
    second.unmount()
  })

  it("expires stored snoozes at the scheduled time", async () => {
    vi.useFakeTimers()
    try {
      const { result } = renderHook(() => useSnoozedBurnChecks())
      await act(async () => snoozeBurnCheck("cacheChurn", "week"))
      expect(result.current.records).toHaveLength(1)
      await act(async () => vi.advanceTimersByTime(7 * 24 * 60 * 60 * 1_000))
      expect(result.current).toEqual({ status: "ready", records: [] })
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
          estimatedTokenBurnBasisPointsByDetectorMask: Array.from({ length: 512 }, (_, mask) =>
            mask === 1 << 8 ? 300 : null,
          ),
          categories,
        },
        snoozed,
      ).estimatedTokenBurnBasisPoints,
    ).toBe(300)
  })

  it("selects the exact overlapping aggregate after filtering", () => {
    const report = activeChecksReport(
      {
        evidenceSettled: true,
        pendingEvidence: 0,
        estimatedTokenBurnBasisPoints: 900,
        estimatedTokenBurnBasisPointsByDetectorMask: Array.from({ length: 512 }, (_, mask) =>
          mask === ((1 << 6) | (1 << 8)) ? 850 : null,
        ),
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
    expect(report.estimatedTokenBurnBasisPoints).toBe(850)
  })

  it("preserves the estimate when snoozing an absent category", () => {
    const report = {
      evidenceSettled: true,
      pendingEvidence: 0,
      estimatedTokenBurnBasisPoints: 900,
      categories: [
        {
          id: "cacheChurn" as const,
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 900,
        },
      ],
    }

    expect(
      activeChecksReport(report, new Set(["unusedSkills"])).estimatedTokenBurnBasisPoints,
    ).toBe(900)
  })
})
