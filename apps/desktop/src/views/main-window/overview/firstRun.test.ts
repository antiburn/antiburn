import { describe, expect, it } from "vitest"

import type { ChecksReportPayload } from "../../../lib/insightsIpc"
import type { ScanStatus } from "../../../lib/ipc"
import {
  INITIAL_FIRST_RUN_LATCH,
  advanceFirstRunLatch,
  resetFirstRunLatch,
  unlatchReadOutcome,
  type FirstRunInputs,
  type FirstRunLatch,
} from "./firstRun"

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

function inputs(overrides: Partial<FirstRunInputs> = {}): FirstRunInputs {
  return {
    scanStatus: null,
    checksReport: null,
    onboardingCompleted: null,
    checksReportCurrent: false,
    ...overrides,
  }
}

describe("advanceFirstRunLatch", () => {
  it("stays undecided until settings answer onboardingCompleted", () => {
    let latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({ checksReport: report() }),
    )
    expect(latch.decided).toBe(false)
    latch = advanceFirstRunLatch(
      latch,
      inputs({ checksReport: report(), onboardingCompleted: true }),
    )
    expect(latch.decided).toBe(true)
  })

  it("does not show the steps block for an ordinary launch whose evidence is briefly unsettled", () => {
    // A live agent session almost always leaves the checks report briefly
    // unsettled right after launch. `evidenceSettled` must not factor into
    // this decision, or every ordinary launch would show the steps block.
    const latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: false }),
        onboardingCompleted: true,
      }),
    )
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(false)
  })

  it("decides to show the steps block when the first run has not finished", () => {
    const latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: true }),
        onboardingCompleted: false,
      }),
    )
    expect(latch.showSteps).toBe(true)
  })

  it("decides to skip the steps block once the first run has already finished", () => {
    const latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: true }),
        onboardingCompleted: true,
      }),
    )
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(false)
  })

  it("holds the decision for the rest of the session even if the answer would change", () => {
    let latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        checksReport: report({ evidenceSettled: true }),
        onboardingCompleted: true,
      }),
    )
    expect(latch.showSteps).toBe(false)
    // A later routine pass makes the report look unsettled again; the
    // decision must not flip back to showing the steps block.
    latch = advanceFirstRunLatch(
      latch,
      inputs({
        scanStatus: status({ phase: "finding" }),
        checksReport: report({ evidenceSettled: false }),
        onboardingCompleted: true,
      }),
    )
    expect(latch.showSteps).toBe(false)
  })

  it("ordinary launch mid-pass, with a finished first run, shows no steps", () => {
    // A launch runs a full pass, same as a first run: discovery has reset
    // and is under way. The stored setting still shows this device already
    // finished a first run.
    const midLaunchPass = status({
      running: true,
      phase: "finding",
      foundByAgent: [],
      agents: [
        { agent: "claude-code", lastCompletedAt: "2026-09-30T12:00:00Z", sessionsSeen: 412 },
      ],
    })
    const latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        scanStatus: midLaunchPass,
        checksReport: report({ evidenceSettled: false }),
        onboardingCompleted: true,
      }),
    )
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(false)
  })

  it("a genuinely new install shows the steps block", () => {
    const firstRunPass = status({ running: true, phase: "finding", agents: [] })
    const latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        scanStatus: firstRunPass,
        checksReport: report({ evidenceSettled: false }),
        onboardingCompleted: false,
      }),
    )
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(true)
  })

  it("picks up a pass already at the read outcome as done, mid-way", () => {
    // A pass already ran before the Overview opened. Discovery and the read
    // stage are both finished; only the checks report is not settled yet.
    const latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        scanStatus: status({
          phase: "saving",
          foundByAgent: [{ agent: "claude-code", sessions: 412, done: true }],
          read: { completed: 500, total: 500 },
          gate: { kept: 480, outsideRepository: 20, excluded: 0, unreadable: 0 },
        }),
        checksReport: report({ evidenceSettled: false }),
        onboardingCompleted: false,
      }),
    )
    expect(latch.showSteps).toBe(true)
    expect(latch.step1Done).toBe(true)
    expect(latch.step1Rows).toEqual([{ agent: "claude-code", sessions: 412, done: true }])
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
    let latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({
        scanStatus: status({
          phase: "saving",
          foundByAgent: [{ agent: "codex", sessions: 88, done: true }],
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
    latch = advanceFirstRunLatch(
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
    expect(latch.step1Rows).toEqual([{ agent: "codex", sessions: 88, done: true }])
    expect(latch.step2Read).toEqual({ completed: 88, total: 88 })
  })
})

describe("advanceFirstRunLatch step 3", () => {
  const readDone: FirstRunLatch = {
    ...resetFirstRunLatch(),
    step1Done: true,
    step2Done: true,
    step2Read: { completed: 44, total: 44 },
    step2Gate: { kept: 44, outsideRepository: 0, excluded: 0, unreadable: 0 },
  }

  it("finishes when every pending session is deferred, such as a live session", () => {
    const latch = advanceFirstRunLatch(
      readDone,
      inputs({
        checksReport: report({
          evidenceSettled: false,
          windowSessions: 44,
          pendingEvidence: 1,
          deferredEvidence: 1,
        }),
        checksReportCurrent: true,
      }),
    )
    expect(latch.step3Done).toBe(true)
    expect(latch.step3Check).toEqual({ windowSessions: 44, deferredEvidence: 1 })
  })

  it("keeps running while a pending session can still be claimed", () => {
    const latch = advanceFirstRunLatch(
      readDone,
      inputs({
        checksReport: report({
          evidenceSettled: false,
          windowSessions: 44,
          pendingEvidence: 2,
          deferredEvidence: 1,
        }),
        checksReportCurrent: true,
      }),
    )
    expect(latch.step3Done).toBe(false)
  })

  it("does not latch on a report requested before the pass finished", () => {
    // The report from before the pass saved its sessions is empty and
    // settled. Latching on it would show the empty state for good.
    const latch = advanceFirstRunLatch(
      readDone,
      inputs({ checksReport: report({ windowSessions: 0 }), checksReportCurrent: false }),
    )
    expect(latch.step3Done).toBe(false)
  })

  it("stays done when a live session goes pending again after a turn", () => {
    let latch = advanceFirstRunLatch(
      readDone,
      inputs({ checksReport: report({ windowSessions: 44 }), checksReportCurrent: true }),
    )
    latch = advanceFirstRunLatch(
      latch,
      inputs({
        checksReport: report({
          evidenceSettled: false,
          windowSessions: 44,
          pendingEvidence: 1,
        }),
        checksReportCurrent: true,
      }),
    )
    expect(latch.step3Done).toBe(true)
  })

  it("does not wait for the read step when the steps block is hidden", () => {
    const latch = advanceFirstRunLatch(
      INITIAL_FIRST_RUN_LATCH,
      inputs({ checksReport: report({ windowSessions: 44 }), onboardingCompleted: true }),
    )
    expect(latch.showSteps).toBe(false)
    expect(latch.step3Done).toBe(true)
  })
})

describe("resetFirstRunLatch", () => {
  it("brings the steps block back and clears the latched step 1 and 2 numbers", () => {
    const settled: FirstRunLatch = {
      decided: true,
      showSteps: false,
      step1Done: true,
      step1Rows: [{ agent: "codex", sessions: 88, done: true }],
      step2Done: true,
      step2Read: { completed: 88, total: 88 },
      step2Gate: { kept: 88, outsideRepository: 0, excluded: 0, unreadable: 0 },
      step3Done: true,
      step3Check: { windowSessions: 88, deferredEvidence: 0 },
    }
    const latch = resetFirstRunLatch()
    expect(latch.decided).toBe(true)
    expect(latch.showSteps).toBe(true)
    expect(latch.step1Done).toBe(false)
    expect(latch.step2Done).toBe(false)
    expect(latch.step3Done).toBe(false)
    expect(latch).not.toEqual(settled)
  })
})

describe("unlatchReadOutcome", () => {
  it("un-latches steps 2 and 3, so the pass an 'Include them' click triggers replaces them", () => {
    const done: FirstRunLatch = {
      decided: true,
      showSteps: true,
      step1Done: true,
      step1Rows: [{ agent: "codex", sessions: 88, done: true }],
      step2Done: true,
      step2Read: { completed: 88, total: 88 },
      step2Gate: { kept: 68, outsideRepository: 20, excluded: 0, unreadable: 0 },
      step3Done: true,
      step3Check: { windowSessions: 68, deferredEvidence: 0 },
    }
    const latch = unlatchReadOutcome(done)
    expect(latch.step2Done).toBe(false)
    expect(latch.step2Gate).toBeNull()
    expect(latch.step3Done).toBe(false)
    // Discovery is unaffected by this setting.
    expect(latch.step1Done).toBe(true)
    expect(latch.step1Rows).toEqual([{ agent: "codex", sessions: 88, done: true }])
  })
})
