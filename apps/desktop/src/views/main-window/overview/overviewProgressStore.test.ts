import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import type { ScanHistoryProgress, ScanStatus } from "../../../lib/ipc"
import { installedCheckFixture } from "../burn-checks/tests/installedCheckFixture"
import { checkResultStatus } from "../../../lib/presentation/checkRowStatus"
import {
  INITIAL_LAST_PASS,
  advanceLastPass,
  deriveOverviewProgress,
  type ProgressInputs,
  type LastPass,
} from "./overviewProgressStore"

function status(overrides: Partial<ScanStatus> = {}): ScanStatus {
  return {
    running: false,
    completedAgents: 0,
    totalAgents: 0,
    sessions: 0,
    finishedAt: null,
    cancelled: false,
    error: null,
    agents: [],
    listChanged: false,
    reDescribed: 0,
    phase: "idle",
    foundByAgent: [],
    read: { completed: 0, total: 0 },
    gate: null,
    ...overrides,
  }
}

function category(overrides: Partial<ChecksCategoryPayload> = {}): ChecksCategoryPayload {
  return {
    id: "modelOverthinking",
    lifecycle: "passing",
    finding: 0,
    clean: 10,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: null,
    ...overrides,
  }
}

function report(overrides: Partial<ChecksReportPayload> = {}): ChecksReportPayload {
  return {
    evidenceSettled: true,
    windowSessions: 0,
    pendingEvidence: 0,
    deferredEvidence: 0,
    estimatedTokenBurnBasisPoints: null,
    categories: [],
    ...overrides,
  }
}

function inputs(overrides: Partial<ProgressInputs> = {}): ProgressInputs {
  return {
    scanStatus: null,
    checksReport: null,
    deferred: [],
    checksReportCurrent: false,
    ...overrides,
  }
}

it("keeps installed results, work, errors, and waiting states in the Overview projection", () => {
  const checksReport = report({
    installedChecks: [
      {
        ...installedCheckFixture,
        finding: 0,
        clean: 1,
        checkingCount: 2,
        reviewFailed: 1,
        reviewedWithoutStatus: 1,
        waitingForDataCount: 1,
        waitingForResultsCount: 1,
      },
      {
        ...installedCheckFixture,
        effectiveId: "installed:first-run",
        finding: 0,
        clean: 0,
        checkingCount: 1,
      },
    ],
  })
  const progress = deriveOverviewProgress(inputs({ checksReport }), INITIAL_LAST_PASS)
  expect(progress.categories).toHaveLength(2)
  expect(checkResultStatus(progress.categories[0]!).text).toBe("Passed · Checking")
  expect(progress.categories[1]).toMatchObject({ status: "checking", checking: true, clean: 0 })
  expect(checkResultStatus(progress.categories[1]!).text).toBe("Not assessed · Checking")
})
describe("advanceLastPass", () => {
  it("keeps the previous values while a pass runs", () => {
    const previous: LastPass = {
      lastFound: [{ agent: "codex", sessions: 88, done: true }],
      lastRead: { completed: 88, total: 88 },
    }
    const midPass = status({
      running: true,
      phase: "finding",
      foundByAgent: [],
      read: { completed: 0, total: 0 },
    })
    expect(advanceLastPass(previous, midPass)).toEqual(previous)
  })

  it("takes the new Agents values once the pass reaches saving", () => {
    const next = advanceLastPass(
      INITIAL_LAST_PASS,
      status({
        running: true,
        phase: "saving",
        foundByAgent: [{ agent: "claude-code", sessions: 49, done: true }],
      }),
    )
    expect(next.lastFound).toEqual([{ agent: "claude-code", sessions: 49, done: true }])
  })

  it("keeps discovery's candidate counts out of lastFound", () => {
    // Discovery's counts are higher than the read stage's admitted counts.
    // Taking them would make the docked row jump up and back down on each pass.
    const previous: LastPass = {
      lastFound: [{ agent: "claude-code", sessions: 43, done: true }],
      lastRead: null,
    }
    const afterDiscovery = advanceLastPass(
      previous,
      status({
        running: true,
        phase: "reading",
        foundByAgent: [{ agent: "claude-code", sessions: 47, done: true }],
      }),
    )
    expect(afterDiscovery.lastFound).toEqual(previous.lastFound)
    const afterReadStage = advanceLastPass(
      afterDiscovery,
      status({
        running: true,
        phase: "saving",
        foundByAgent: [{ agent: "claude-code", sessions: 42, done: true }],
      }),
    )
    expect(afterReadStage.lastFound).toEqual([
      { agent: "claude-code", sessions: 42, done: true },
    ])
  })

  it("does not take Agents values while an agent is still searching", () => {
    const next = advanceLastPass(
      INITIAL_LAST_PASS,
      status({
        running: true,
        phase: "finding",
        foundByAgent: [{ agent: "claude-code", sessions: 49, done: false }],
      }),
    )
    expect(next.lastFound).toBeNull()
  })

  it("takes the new read values once the pass has left the read stage", () => {
    const next = advanceLastPass(
      INITIAL_LAST_PASS,
      status({
        running: false,
        phase: "saving",
        read: { completed: 49, total: 49 },
      }),
    )
    expect(next.lastRead).toEqual({ completed: 49, total: 49 })
  })

  it("does not take read values while idle or still finding", () => {
    expect(advanceLastPass(INITIAL_LAST_PASS, status({ phase: "idle" })).lastRead).toBeNull()
    expect(
      advanceLastPass(INITIAL_LAST_PASS, status({ running: true, phase: "finding" })).lastRead,
    ).toBeNull()
  })
})

it("shows current data and history without a setup decision", () => {
  const progress = deriveOverviewProgress(
    inputs({
      scanStatus: status({
        foundByAgent: [{ agent: "codex", sessions: 3, done: true }],
        read: { completed: 2, total: 3 },
        history: { state: "running", completed: 4, total: 6 } as ScanHistoryProgress,
      }),
      checksReport: report({ categories: [category({ lifecycle: "failing", finding: 1 })] }),
    }),
    INITIAL_LAST_PASS,
  )
  expect(progress.agents.rows[0]).toMatchObject({ label: "Codex", sessions: 3 })
  expect(progress.sessions.displayCompleted).toBe(6)
  expect(progress.sessions.displayTotal).toBe(9)
  expect(progress.history?.state).toBe("reading")
  expect(progress.failingCount).toBe(1)
})
