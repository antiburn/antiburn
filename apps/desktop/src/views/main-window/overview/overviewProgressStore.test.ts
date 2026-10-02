import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import type { ScanStatus } from "../../../lib/ipc"
import {
  INITIAL_FIRST_RUN_LATCH,
  advanceFirstRunLatch,
  resetFirstRunLatch,
  type FirstRunLatch,
} from "./firstRun"
import {
  INITIAL_DOCK,
  INITIAL_LAST_PASS,
  advanceLastPass,
  deriveOverviewProgress,
  type LastPass,
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
    includeNonRepoFolders: false,
    hasScanHistory: null,
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

  it("takes the new find values once every agent's search is done", () => {
    const next = advanceLastPass(
      INITIAL_LAST_PASS,
      status({
        running: true,
        phase: "reading",
        foundByAgent: [{ agent: "claude-code", sessions: 49, done: true }],
      }),
    )
    expect(next.lastFound).toEqual([{ agent: "claude-code", sessions: 49, done: true }])
  })

  it("does not take find values while an agent is still searching", () => {
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
      INITIAL_DOCK,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.mode).toBe("pending")
  })

  it("is firstRun once the latch decides to show the steps block", () => {
    const latch = resetFirstRunLatch()
    const snapshot = deriveOverviewProgress(latch, inputs(), INITIAL_DOCK, INITIAL_LAST_PASS)
    expect(snapshot.mode).toBe("firstRun")
  })

  it("is steady once the latch decides the device has scanned before", () => {
    const latch: FirstRunLatch = { ...INITIAL_FIRST_RUN_LATCH, decided: true, showSteps: false }
    const snapshot = deriveOverviewProgress(latch, inputs(), INITIAL_DOCK, INITIAL_LAST_PASS)
    expect(snapshot.mode).toBe("steady")
  })
})

describe("deriveOverviewProgress in steady mode", () => {
  const steadyLatch: FirstRunLatch = {
    ...INITIAL_FIRST_RUN_LATCH,
    decided: true,
    showSteps: false,
    step1Done: true,
    step2Done: true,
    step2Read: { completed: 1, total: 1 },
    step3Done: true,
  }

  it("shows the last finished pass's Find rows, not the live reset ones", () => {
    const lastPass: LastPass = {
      lastFound: [{ agent: "claude-code", sessions: 49, done: true }],
      lastRead: { completed: 49, total: 49 },
    }
    // A routine pass has just reset the live status to empty.
    const liveReset = inputs({
      scanStatus: status({ running: true, phase: "finding", foundByAgent: [] }),
    })
    const snapshot = deriveOverviewProgress(steadyLatch, liveReset, INITIAL_DOCK, lastPass)
    expect(snapshot.find.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])
  })

  it("falls back to the live Find status when no pass has finished yet", () => {
    const live = inputs({
      scanStatus: status({ foundByAgent: [{ agent: "codex", sessions: 3, done: true }] }),
    })
    const snapshot = deriveOverviewProgress(steadyLatch, live, INITIAL_DOCK, INITIAL_LAST_PASS)
    expect(snapshot.find.rows).toEqual([
      { agent: "codex", label: "Codex", sessions: 3, done: true },
    ])
  })

  it("uses the live checks report for Check, so a still-running check shows", () => {
    const live = inputs({
      checksReport: report({ windowSessions: 42, pendingEvidence: 1, deferredEvidence: 0 }),
    })
    const snapshot = deriveOverviewProgress(steadyLatch, live, INITIAL_DOCK, INITIAL_LAST_PASS)
    expect(snapshot.check).toEqual({
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
    step1Done: true,
    step1Rows: [{ agent: "claude-code", sessions: 49, done: true }],
    step2Done: true,
    step2Read: { completed: 49, total: 49 },
    step2Gate: { kept: 49, outsideRepository: 0, excluded: 0, unreadable: 0 },
    step3Done: true,
    step3Check: { windowSessions: 49, deferredEvidence: 0 },
  }

  it("uses the latched values while the steps have not all docked", () => {
    const snapshot = deriveOverviewProgress(
      firstRunLatch,
      inputs(),
      { ...INITIAL_DOCK, stepsDocked: 2 },
      INITIAL_LAST_PASS,
    )
    expect(snapshot.find.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])
    expect(snapshot.check).toEqual({
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
    const snapshot = deriveOverviewProgress(
      firstRunLatch,
      live,
      { ...INITIAL_DOCK, stepsDocked: 3 },
      lastPass,
    )
    // The numbers are the same at the moment of the switch, so no jump.
    expect(snapshot.check).toEqual({
      done: true,
      windowSessions: 49,
      pendingEvidence: 1,
      deferredEvidence: 0,
    })
  })

  it("keeps find.done, read.done and check.done at the latch's own meaning", () => {
    const snapshot = deriveOverviewProgress(
      firstRunLatch,
      inputs(),
      { ...INITIAL_DOCK, stepsDocked: 3 },
      INITIAL_LAST_PASS,
    )
    expect(snapshot.find.done).toBe(true)
    expect(snapshot.read.done).toBe(true)
    expect(snapshot.check.done).toBe(true)
  })

  it("finishes the check step when every pending session is deferred, such as a live session", () => {
    const latch: FirstRunLatch = {
      ...firstRunLatch,
      step3Done: true,
      step3Check: { windowSessions: 44, deferredEvidence: 1 },
    }
    const snapshot = deriveOverviewProgress(
      latch,
      inputs(),
      { ...INITIAL_DOCK, stepsDocked: 2 },
      INITIAL_LAST_PASS,
    )
    expect(snapshot.check).toEqual({
      done: true,
      windowSessions: 44,
      pendingEvidence: 1,
      deferredEvidence: 1,
    })
  })

  it("stays done when a live session goes pending again after a turn", () => {
    const latch: FirstRunLatch = {
      ...firstRunLatch,
      step3Done: true,
      step3Check: { windowSessions: 44, deferredEvidence: 0 },
    }
    const requeued = inputs({
      checksReport: report({ evidenceSettled: false, windowSessions: 44, pendingEvidence: 1 }),
      checksReportCurrent: true,
    })
    const snapshot = deriveOverviewProgress(
      latch,
      requeued,
      { ...INITIAL_DOCK, stepsDocked: 2 },
      INITIAL_LAST_PASS,
    )
    expect(snapshot.check.done).toBe(true)
  })
})

describe("deriveOverviewProgress", () => {
  it("shows a plain empty state's numbers when the window has no current sessions", () => {
    const emptyInputs = inputs({
      checksReport: report({ evidenceSettled: true, windowSessions: 0, categories: [] }),
      hasScanHistory: true,
    })
    const latch = advanceFirstRunLatch(INITIAL_FIRST_RUN_LATCH, emptyInputs)
    const snapshot = deriveOverviewProgress(latch, emptyInputs, INITIAL_DOCK, INITIAL_LAST_PASS)
    expect(snapshot.check).toEqual({
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
      INITIAL_DOCK,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.check.windowSessions).toBe(40)
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
      INITIAL_DOCK,
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

  it("shows the history line only while the background pass is pending or running", () => {
    const withoutHistory = deriveOverviewProgress(
      INITIAL_FIRST_RUN_LATCH,
      inputs(),
      INITIAL_DOCK,
      INITIAL_LAST_PASS,
    )
    expect(withoutHistory.history).toBeNull()

    const running = deriveOverviewProgress(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        scanStatus: status({ history: { state: "running", completed: 1_204, total: 6_300 } }),
      }),
      INITIAL_DOCK,
      INITIAL_LAST_PASS,
    )
    expect(running.history).toEqual({ completed: 1_204, total: 6_300 })

    const done = deriveOverviewProgress(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        scanStatus: status({ history: { state: "done", completed: 6_300, total: 6_300 } }),
      }),
      INITIAL_DOCK,
      INITIAL_LAST_PASS,
    )
    expect(done.history).toBeNull()
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
      INITIAL_DOCK,
      INITIAL_LAST_PASS,
    )
    expect(snapshot.find.rows).toEqual([
      { agent: "codex", label: "Codex", sessions: 88, done: true },
    ])
  })
})
