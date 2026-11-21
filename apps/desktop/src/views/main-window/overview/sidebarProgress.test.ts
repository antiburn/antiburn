import { describe, expect, it } from "vitest"
import type { AllowanceUsageSummaryPayload } from "../../../lib/providerUsageIpc"
import { deriveOverviewProgress, INITIAL_LAST_PASS } from "./overviewProgressStore"
import { sidebarProgress } from "./sidebarProgress"

function progress() {
  return deriveOverviewProgress(
    {
      scanStatus: null,
      checksReport: null,
      deferred: [],
      checksReportCurrent: false,
    },
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
    expect(sidebarProgress("quota", progress(), allowance).status).toBe("29% / 19% / 0%")
    expect(sidebarProgress("quota", progress(), null).status).toBe("")
  })

  it("shows discovered agents, combined sessions, and checks needing fixes", () => {
    const state = progress()
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
