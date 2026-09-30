import { describe, expect, it } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import type { ScanStatus } from "../../../lib/ipc"
import {
  INITIAL_FTUE_LATCH,
  advanceFtueLatch,
  deriveFtueSnapshot,
  hasScanHistory,
  resetFtueLatch,
  unlatchReadOutcome,
  type FtueInputs,
  type FtueLatch,
} from "./ftueStore"

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
    estimatedTokenBurnBasisPoints: null,
    categories: [],
    ...overrides,
  }
}

function inputs(overrides: Partial<FtueInputs> = {}): FtueInputs {
  return {
    scanStatus: null,
    checksReport: null,
    includeNonRepoFolders: false,
    hasScanHistory: null,
    ...overrides,
  }
}

describe("advanceFtueLatch", () => {
  it("stays undecided until both the checks report and the scan history signal are known", () => {
    let latch = advanceFtueLatch(INITIAL_FTUE_LATCH, inputs({ hasScanHistory: true }))
    expect(latch.decided).toBe(false)
    latch = advanceFtueLatch(latch, inputs({ checksReport: report() }))
    expect(latch.decided).toBe(false)
    latch = advanceFtueLatch(latch, inputs({ checksReport: report(), hasScanHistory: true }))
    expect(latch.decided).toBe(true)
  })

  it("decides to show the steps block when the checks report is not settled", () => {
    const latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: false }),
        hasScanHistory: true,
      }),
    )
    expect(latch.showSteps).toBe(true)
  })

  it("decides to show the steps block when the device has no persisted scan history", () => {
    const latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: true }),
        hasScanHistory: false,
      }),
    )
    expect(latch.showSteps).toBe(true)
  })

  it("decides to skip the steps block once everything is already settled", () => {
    const latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: true }),
        hasScanHistory: true,
      }),
    )
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(false)
  })

  it("holds the decision for the rest of the session even if the answer would change", () => {
    let latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: true }),
        hasScanHistory: true,
      }),
    )
    expect(latch.showSteps).toBe(false)
    // A later routine pass makes the report look unsettled again; the
    // decision must not flip back to showing the steps block.
    latch = advanceFtueLatch(
      latch,
      inputs({
        scanStatus: status({ phase: "finding" }),
        checksReport: report({ evidenceSettled: false }),
        hasScanHistory: true,
      }),
    )
    expect(latch.showSteps).toBe(false)
  })

  it("ordinary launch mid-pass, with persisted scan state, shows no steps", () => {
    // A launch runs a full pass, same as a first run: discovery has reset
    // and is under way. The persisted `scan_state` table (unlike the
    // in-memory `finishedAt`) still shows this device has scanned before.
    const midLaunchPass = status({
      running: true,
      phase: "finding",
      foundByAgent: [],
      agents: [
        { agent: "claude-code", lastCompletedAt: "2026-09-30T12:00:00Z", sessionsSeen: 412 },
      ],
    })
    const latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        scanStatus: midLaunchPass,
        checksReport: report({ evidenceSettled: true }),
        hasScanHistory: hasScanHistory(midLaunchPass),
      }),
    )
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(false)
  })

  it("a genuinely empty scan_state table shows the steps block", () => {
    const firstRunPass = status({ running: true, phase: "finding", agents: [] })
    const latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        scanStatus: firstRunPass,
        checksReport: report({ evidenceSettled: false }),
        hasScanHistory: hasScanHistory(firstRunPass),
      }),
    )
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(true)
  })

  it("picks up a pass already at the read outcome as done, mid-way", () => {
    // Onboarding already ran a pass before the Overview opened. Discovery
    // and the read stage are both finished; only the checks report is not
    // settled yet.
    const latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        scanStatus: status({
          phase: "saving",
          foundByAgent: [{ agent: "claude-code", sessions: 412 }],
          read: { completed: 500, total: 500 },
          gate: { kept: 480, outsideRepository: 20, excluded: 0, unreadable: 0 },
        }),
        checksReport: report({ evidenceSettled: false }),
        hasScanHistory: false,
      }),
    )
    expect(latch.showSteps).toBe(true)
    expect(latch.step1Done).toBe(true)
    expect(latch.step1Rows).toEqual([{ agent: "claude-code", sessions: 412 }])
    expect(latch.step2Done).toBe(true)
    expect(latch.step2Read).toEqual({ completed: 500, total: 500 })
    expect(latch.step2Gate).toEqual({
      kept: 480,
      outsideRepository: 20,
      excluded: 0,
      unreadable: 0,
    })
  })

  it("latches step 1 and 2 once, and ignores a later routine pass resetting them", () => {
    let latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        scanStatus: status({
          phase: "saving",
          foundByAgent: [{ agent: "codex", sessions: 88 }],
          read: { completed: 88, total: 88 },
          gate: { kept: 88, outsideRepository: 0, excluded: 0, unreadable: 0 },
        }),
        checksReport: report({ evidenceSettled: true }),
      }),
    )
    expect(latch.step1Done).toBe(true)
    expect(latch.step2Done).toBe(true)

    // A routine 5-minute tick starts a fresh pass: discovery resets to
    // empty and the phase moves back to "finding".
    latch = advanceFtueLatch(
      latch,
      inputs({
        scanStatus: status({
          phase: "finding",
          foundByAgent: [],
          read: { completed: 0, total: 0 },
        }),
        checksReport: report({ evidenceSettled: true }),
      }),
    )
    expect(latch.step1Rows).toEqual([{ agent: "codex", sessions: 88 }])
    expect(latch.step2Read).toEqual({ completed: 88, total: 88 })
  })
})

describe("hasScanHistory", () => {
  it("is null when the status itself is unknown", () => {
    expect(hasScanHistory(null)).toBeNull()
  })

  it("is false for a genuinely empty scan_state table", () => {
    expect(hasScanHistory(status({ agents: [] }))).toBe(false)
  })

  it("is false when an agent is registered but has never completed a pass", () => {
    expect(
      hasScanHistory(
        status({ agents: [{ agent: "claude-code", lastCompletedAt: null, sessionsSeen: 0 }] }),
      ),
    ).toBe(false)
  })

  it("is true when any agent has a persisted completed pass, even mid-launch", () => {
    expect(
      hasScanHistory(
        status({
          running: true,
          phase: "finding",
          agents: [
            {
              agent: "claude-code",
              lastCompletedAt: "2026-09-30T12:00:00Z",
              sessionsSeen: 412,
            },
          ],
        }),
      ),
    ).toBe(true)
  })
})

describe("resetFtueLatch", () => {
  it("brings the steps block back and clears the latched step 1 and 2 numbers", () => {
    const settled: FtueLatch = {
      decided: true,
      showSteps: false,
      step1Done: true,
      step1Rows: [{ agent: "codex", sessions: 88 }],
      step2Done: true,
      step2Read: { completed: 88, total: 88 },
      step2Gate: { kept: 88, outsideRepository: 0, excluded: 0, unreadable: 0 },
    }
    const latch = resetFtueLatch()
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(true)
    expect(latch.step1Done).toBe(false)
    expect(latch.step2Done).toBe(false)
    expect(latch).not.toEqual(settled)
  })
})

describe("unlatchReadOutcome", () => {
  it("un-latches step 2 alone, so the pass an 'Include them' click triggers replaces it", () => {
    const done: FtueLatch = {
      decided: true,
      showSteps: true,
      step1Done: true,
      step1Rows: [{ agent: "codex", sessions: 88 }],
      step2Done: true,
      step2Read: { completed: 88, total: 88 },
      step2Gate: { kept: 68, outsideRepository: 20, excluded: 0, unreadable: 0 },
    }
    const latch = unlatchReadOutcome(done)
    expect(latch.step2Done).toBe(false)
    expect(latch.step2Gate).toBeNull()
    // Discovery is unaffected by this setting.
    expect(latch.step1Done).toBe(true)
    expect(latch.step1Rows).toEqual([{ agent: "codex", sessions: 88 }])
  })
})

describe("deriveFtueSnapshot", () => {
  it("shows a plain empty state's numbers when the window has no current sessions", () => {
    const latch = advanceFtueLatch(
      INITIAL_FTUE_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: true, windowSessions: 0, categories: [] }),
        hasScanHistory: true,
      }),
    )
    const snapshot = deriveFtueSnapshot(
      latch,
      inputs({
        checksReport: report({ evidenceSettled: true, windowSessions: 0, categories: [] }),
        hasScanHistory: true,
      }),
      false,
    )
    expect(snapshot.check).toEqual({ done: true, windowSessions: 0, pendingEvidence: 0 })
    expect(snapshot.failingCount).toBe(0)
  })

  it("reports zero failing categories as a clean result, not an empty one", () => {
    const checksReport = report({
      evidenceSettled: true,
      windowSessions: 40,
      categories: [category({ lifecycle: "passing" }), category({ id: "cacheChurn" })],
    })
    const snapshot = deriveFtueSnapshot(INITIAL_FTUE_LATCH, inputs({ checksReport }), false)
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
    const snapshot = deriveFtueSnapshot(INITIAL_FTUE_LATCH, inputs({ checksReport }), false)
    expect(snapshot.failingCount).toBe(1)
    expect(snapshot.categories.map((c) => c.status)).toEqual([
      "needsFix",
      "awaitingVerification",
      "passing",
      "notChecked",
    ])
  })

  it("shows the history line only while the background pass is pending or running", () => {
    const withoutHistory = deriveFtueSnapshot(INITIAL_FTUE_LATCH, inputs(), false)
    expect(withoutHistory.history).toBeNull()

    const running = deriveFtueSnapshot(
      INITIAL_FTUE_LATCH,
      inputs({
        scanStatus: status({ history: { state: "running", completed: 1_204, total: 6_300 } }),
      }),
      false,
    )
    expect(running.history).toEqual({ completed: 1_204, total: 6_300 })

    const done = deriveFtueSnapshot(
      INITIAL_FTUE_LATCH,
      inputs({
        scanStatus: status({ history: { state: "done", completed: 6_300, total: 6_300 } }),
      }),
      false,
    )
    expect(done.history).toBeNull()
  })

  it("maps the discovery slug to a display label", () => {
    const snapshot = deriveFtueSnapshot(
      INITIAL_FTUE_LATCH,
      inputs({
        scanStatus: status({
          phase: "finding",
          foundByAgent: [{ agent: "codex", sessions: 88 }],
        }),
      }),
      false,
    )
    expect(snapshot.find.rows).toEqual([{ agent: "Codex", sessions: 88 }])
  })
})
