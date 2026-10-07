import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import type { ScanHistoryProgress, ScanStatus } from "../../../lib/ipc"
import {
  INITIAL_FIRST_RUN_LATCH,
  advanceFirstRunLatch,
  resetFirstRunLatch,
  type FirstRunLatch,
} from "./firstRun"
import {
  INITIAL_LAST_PASS,
  advanceLastPass,
  deriveOverviewProgress,
  firstFailingCheck,
  type LastPass,
  type OverviewProgress,
  type ProgressInputs,
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
    onboardingCompleted: null,
    checksReportCurrent: false,
    ...overrides,
  }
}

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

describe("deriveOverviewProgress's mode", () => {
  it("is pending until the latch decides", () => {
    const snapshot = deriveOverviewProgress(
      INITIAL_FIRST_RUN_LATCH,
      inputs(),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.mode).toBe("pending")
  })

  it("is firstRun once the latch decides to show the steps block", () => {
    const latch = resetFirstRunLatch()
    const snapshot = deriveOverviewProgress(
      latch,
      inputs(),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.mode).toBe("firstRun")
  })

  it("is steady once the latch decides the device has scanned before", () => {
    const latch: FirstRunLatch = { ...INITIAL_FIRST_RUN_LATCH, decided: true, showSteps: false }
    const snapshot = deriveOverviewProgress(
      latch,
      inputs(),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.mode).toBe("steady")
  })
})

describe("deriveOverviewProgress in steady mode", () => {
  const steadyLatch: FirstRunLatch = {
    ...INITIAL_FIRST_RUN_LATCH,
    decided: true,
    showSteps: false,
    agentsDone: true,
    sessionsDone: true,
    sessionsRead: { completed: 1, total: 1 },
    checksDone: true,
  }

  it("shows the last finished pass's Agents rows, not the live reset ones", () => {
    const lastPass: LastPass = {
      lastFound: [{ agent: "claude-code", sessions: 49, done: true }],
      lastRead: { completed: 49, total: 49 },
    }
    // A routine pass has just reset the live status to empty.
    const liveReset = inputs({
      scanStatus: status({ running: true, phase: "finding", foundByAgent: [] }),
    })
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      liveReset,
      "agents",
      null,
      true,
      lastPass,
    )
    expect(snapshot.agents.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])
  })

  it("falls back to the live Agents status when no pass has finished yet", () => {
    const live = inputs({
      scanStatus: status({ foundByAgent: [{ agent: "codex", sessions: 3, done: true }] }),
    })
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      live,
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.agents.rows).toEqual([
      { agent: "codex", label: "Codex", sessions: 3, done: true },
    ])
  })

  it("uses the live checks report for Check, so a still-running check shows", () => {
    const live = inputs({
      checksReport: report({ windowSessions: 42, pendingEvidence: 1, deferredEvidence: 0 }),
    })
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      live,
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.checks).toEqual({
      done: true,
      windowSessions: 42,
      pendingEvidence: 1,
      deferredEvidence: 0,
    })
  })
})

describe("deriveOverviewProgress in first-run mode", () => {
  const firstRunLatch: FirstRunLatch = {
    ...resetFirstRunLatch(),
    agentsDone: true,
    agentsFound: [{ agent: "claude-code", sessions: 49, done: true }],
    sessionsDone: true,
    sessionsRead: { completed: 49, total: 49 },
    checksDone: true,
    checksResult: { windowSessions: 49, deferredEvidence: 0 },
  }

  it("uses the latched values while the steps have not all docked", () => {
    const snapshot = deriveOverviewProgress(
      firstRunLatch,
      inputs(),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.agents.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])
    expect(snapshot.checks).toEqual({
      done: true,
      windowSessions: 49,
      pendingEvidence: 0,
      deferredEvidence: 0,
    })
  })

  it("switches to the steady values once the third step has docked", () => {
    const lastPass: LastPass = {
      lastFound: [{ agent: "claude-code", sessions: 49, done: true }],
      lastRead: { completed: 49, total: 49 },
    }
    const live = inputs({ checksReport: report({ windowSessions: 49, pendingEvidence: 1 }) })
    const snapshot = deriveOverviewProgress(firstRunLatch, live, "fixes", null, true, lastPass)
    // The numbers are the same at the moment of the switch, so no jump.
    expect(snapshot.checks).toEqual({
      done: true,
      windowSessions: 49,
      pendingEvidence: 1,
      deferredEvidence: 0,
    })
  })

  it("keeps agents.done, read.done and check.done at the latch's own meaning", () => {
    const snapshot = deriveOverviewProgress(
      firstRunLatch,
      inputs(),
      "fixes",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.agents.done).toBe(true)
    expect(snapshot.sessions.done).toBe(true)
    expect(snapshot.checks.done).toBe(true)
  })

  it("finishes the Checks step when every pending session is deferred, such as a live session", () => {
    const latch: FirstRunLatch = {
      ...firstRunLatch,
      checksDone: true,
      checksResult: { windowSessions: 44, deferredEvidence: 1 },
    }
    const snapshot = deriveOverviewProgress(
      latch,
      inputs(),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.checks).toEqual({
      done: true,
      windowSessions: 44,
      pendingEvidence: 1,
      deferredEvidence: 1,
    })
  })
})

describe("deriveOverviewProgress", () => {
  it("shows a plain empty state's numbers when the window has no current sessions", () => {
    const emptyInputs = inputs({
      checksReport: report({ evidenceSettled: true, windowSessions: 0, categories: [] }),
      onboardingCompleted: true,
    })
    const latch = advanceFirstRunLatch(INITIAL_FIRST_RUN_LATCH, emptyInputs)
    const snapshot = deriveOverviewProgress(
      latch,
      emptyInputs,
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.checks).toEqual({
      done: true,
      windowSessions: 0,
      pendingEvidence: 0,
      deferredEvidence: 0,
    })
    expect(snapshot.failingCount).toBe(0)
  })

  it("reports zero failing categories as a clean result, not an empty one", () => {
    const checksReport = report({
      evidenceSettled: true,
      windowSessions: 40,
      categories: [category({ lifecycle: "passing" }), category({ id: "cacheChurn" })],
    })
    const snapshot = deriveOverviewProgress(
      INITIAL_FIRST_RUN_LATCH,
      inputs({ checksReport }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.checks.windowSessions).toBe(40)
    expect(snapshot.failingCount).toBe(0)
    expect(snapshot.categories).toHaveLength(2)
  })

  it("counts only failing categories, and maps every lifecycle to a status", () => {
    const checksReport = report({
      categories: [
        category({ id: "modelOverthinking", lifecycle: "failing" }),
        category({ id: "cacheChurn", lifecycle: "awaitingVerification" }),
        category({ id: "oldModelUsage", lifecycle: "passing" }),
        category({ id: "overuseOfFastMode", lifecycle: null }),
      ],
    })
    const snapshot = deriveOverviewProgress(
      INITIAL_FIRST_RUN_LATCH,
      inputs({ checksReport }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.failingCount).toBe(1)
    expect(snapshot.categories.map((c) => c.status)).toEqual([
      "needsFix",
      "awaitingVerification",
      "passing",
      "notChecked",
    ])
  })

  it("maps the discovery slug to a display label", () => {
    const snapshot = deriveOverviewProgress(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        scanStatus: status({
          phase: "finding",
          foundByAgent: [{ agent: "codex", sessions: 88, done: true }],
        }),
      }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.agents.rows).toEqual([
      { agent: "codex", label: "Codex", sessions: 88, done: true },
    ])
  })
})

describe("deriveOverviewProgress's history", () => {
  const steadyLatch: FirstRunLatch = {
    ...INITIAL_FIRST_RUN_LATCH,
    decided: true,
    showSteps: false,
  }

  it("stays null throughout the first-run steps, even while the backend reports a running pass", () => {
    const midFirstRun = deriveOverviewProgress(
      resetFirstRunLatch(),
      inputs({
        scanStatus: status({
          history: { state: "running", completed: 1_204, total: 6_300, passRunning: false },
        }),
      }),
      "sessions",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(midFirstRun.history).toBeNull()
  })

  it("is exposed once a first run reaches its last step", () => {
    const atDone = deriveOverviewProgress(
      resetFirstRunLatch(),
      inputs({
        scanStatus: status({
          history: { state: "running", completed: 1_204, total: 6_300, passRunning: false },
        }),
      }),
      "done",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(atDone.history).toEqual({ state: "reading", completed: 1_204, total: 6_300 })
  })

  it("is exposed in steady mode", () => {
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      inputs({
        scanStatus: status({
          history: { state: "done", completed: 6_300, total: 6_300, passRunning: false },
        }),
      }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.history).toEqual({ state: "done", completed: 6_300, total: 6_300 })
  })

  function historySnapshot(history: ScanHistoryProgress) {
    return deriveOverviewProgress(
      steadyLatch,
      inputs({ scanStatus: status({ history }) }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    ).history
  }

  it("maps a 'none' pass to null", () => {
    expect(
      historySnapshot({ state: "none", completed: 0, total: 0, passRunning: false }),
    ).toBeNull()
  })

  it("maps a 'pending' pass as-is", () => {
    expect(
      historySnapshot({ state: "pending", completed: 0, total: 0, passRunning: false }),
    ).toEqual({
      state: "pending",
      completed: 0,
      total: 0,
    })
  })

  it("maps a running pass with no sessions found yet to 'looking'", () => {
    expect(
      historySnapshot({ state: "running", completed: 0, total: 0, passRunning: false }),
    ).toEqual({
      state: "looking",
      completed: 0,
      total: 0,
    })
  })

  it("maps a running pass with sessions found to 'reading'", () => {
    expect(
      historySnapshot({ state: "running", completed: 1_204, total: 6_300, passRunning: false }),
    ).toEqual({
      state: "reading",
      completed: 1_204,
      total: 6_300,
    })
  })

  it("maps a finished pass to 'done'", () => {
    expect(
      historySnapshot({ state: "done", completed: 6_300, total: 6_300, passRunning: false }),
    ).toEqual({
      state: "done",
      completed: 6_300,
      total: 6_300,
    })
  })

  it("is null when the backend reports no history field at all", () => {
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      inputs({ scanStatus: status() }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.history).toBeNull()
  })
})

describe("deriveOverviewProgress's Sessions display numbers", () => {
  const steadyLatch: FirstRunLatch = {
    ...INITIAL_FIRST_RUN_LATCH,
    decided: true,
    showSteps: false,
  }

  it("equals the 30-day numbers while the history pass has nothing to add", () => {
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      inputs({
        scanStatus: status({
          read: { completed: 114, total: 114 },
          history: { state: "running", completed: 0, total: 0, passRunning: false },
        }),
      }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.sessions.displayCompleted).toBe(114)
    expect(snapshot.sessions.displayTotal).toBe(114)
  })

  it("adds the history pass's own completed/total once it is reading", () => {
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      inputs({
        scanStatus: status({
          read: { completed: 114, total: 114 },
          history: { state: "running", completed: 200, total: 318, passRunning: false },
        }),
      }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.sessions.displayCompleted).toBe(314)
    expect(snapshot.sessions.displayTotal).toBe(432)
  })

  it("keeps adding once the history pass is done", () => {
    const snapshot = deriveOverviewProgress(
      steadyLatch,
      inputs({
        scanStatus: status({
          read: { completed: 114, total: 114 },
          history: { state: "done", completed: 318, total: 318, passRunning: false },
        }),
      }),
      "agents",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.sessions.displayCompleted).toBe(432)
    expect(snapshot.sessions.displayTotal).toBe(432)
  })

  it("keeps the first-run Sessions step's own numbers unchanged by the history pass", () => {
    const snapshot = deriveOverviewProgress(
      resetFirstRunLatch(),
      inputs({
        scanStatus: status({
          read: { completed: 1, total: 10 },
          history: { state: "running", completed: 200, total: 318, passRunning: false },
        }),
      }),
      "sessions",
      null,
      true,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.sessions.completed).toBe(1)
    expect(snapshot.sessions.total).toBe(10)
    // History is hidden during the first-run steps, so there is nothing to add.
    expect(snapshot.sessions.displayCompleted).toBe(1)
    expect(snapshot.sessions.displayTotal).toBe(10)
    expect(snapshot.history).toBeNull()
  })
})

describe("firstFailingCheck", () => {
  it("picks the failing check with the highest estimated burn, the Checks list's top row", () => {
    const progress = {
      categories: [
        { id: "unusedMcpServers", label: "", status: "needsFix", estimatedBurnBasisPoints: 10 },
        {
          id: "modelOverthinking",
          label: "",
          status: "passing",
          estimatedBurnBasisPoints: 900,
        },
        { id: "unusedSkills", label: "", status: "needsFix", estimatedBurnBasisPoints: 300 },
        {
          id: "unusedBuiltInTools",
          label: "",
          status: "needsFix",
          estimatedBurnBasisPoints: null,
        },
      ],
    } as unknown as OverviewProgress
    expect(firstFailingCheck(progress)).toBe("unusedSkills")
  })
})
