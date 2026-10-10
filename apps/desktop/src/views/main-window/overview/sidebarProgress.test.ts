import { describe, expect, it } from "vitest"
import type { AllowanceUsageSummaryPayload } from "../../../lib/providerUsageIpc"
import {
  deriveOverviewProgress,
  INITIAL_LAST_PASS,
  type FlowStep,
} from "./overviewProgressStore"
import { INITIAL_FIRST_RUN_LATCH } from "./firstRun"
import { sidebarProgress } from "./sidebarProgress"

function progress(flow: FlowStep) {
  return deriveOverviewProgress(
    { ...INITIAL_FIRST_RUN_LATCH, decided: true, showSteps: true },
    {
      scanStatus: null,
      checksReport: null,
      deferred: [],
      onboardingCompleted: false,
      checksReportCurrent: false,
    },
    flow,
    true,
    INITIAL_LAST_PASS,
  )
}

describe("sidebar summaries", () => {
  it("uses the same account order and rounding as the subscription headlines", () => {
    const allowance = {
      accounts: [
        { utilization: { utilizationPercent: 28.7 } },
        { utilization: null },
        { utilization: { utilizationPercent: 19.1 } },
        { utilization: { utilizationPercent: 0 } },
      ],
    } as AllowanceUsageSummaryPayload
    expect(sidebarProgress("quota", progress("done"), allowance).status).toBe("29% / 19% / 0%")
    expect(sidebarProgress("quota", progress("done"), null).status).toBe("")
  })

  it("moves each transition name to its row only after the step ends", () => {
    for (const [view, current, next, name] of [
      ["agents", "agents", "limits", "progress-step-agents"],
      ["quota", "limits", "sessions", "progress-live-limits"],
      ["activity", "sessions", "checks", "progress-step-sessions"],
      ["burnChecks", "checks", "fixes", "progress-step-checks"],
    ] as const) {
      expect(sidebarProgress(view, progress(current), null).transitionName).toBeUndefined()
      expect(sidebarProgress(view, progress(next), null).transitionName).toBe(name)
    }
    expect(sidebarProgress("burnChecks", progress("done"), null).transitionName).toBe(
      "progress-step-fixes",
    )
  })

  it("shows discovered agents, combined sessions, and checks needing fixes", () => {
    const state = progress("done")
    state.agents.rows = [
      { agent: "codex", label: "Codex", sessions: 5, done: true },
      { agent: "cursor", label: "Cursor", sessions: 0, done: true },
    ]
    state.sessions.displayCompleted = 440
    state.failingCount = 4
    expect(sidebarProgress("agents", state, null).status).toBe("1")
    expect(sidebarProgress("activity", state, null).status).toBe("440")
    expect(sidebarProgress("burnChecks", state, null).status).toBe("4 to fix")
  })
})
