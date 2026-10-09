// The Overview's first-run latch. Decides, once per session, whether to show
// the Agents, Sessions and Checks steps block, and remembers each step's own numbers so
// a later routine pass does not reset a checklist the reader already saw.
// Pure code, with no module state, so a test drives it without any IPC
// mocking.

import type { ChecksReportPayload } from "../../../lib/insightsIpc"
import type { AgentFoundCount, ScanStatus } from "../../../lib/ipc"

export interface FirstRunInputs {
  scanStatus: ScanStatus | null
  checksReport: ChecksReportPayload | null
  /**
   * `settings.onboardingCompleted`, at the first settings read of the
   * session — the durable "this device is not a first run" signal. Null
   * until that first read resolves.
   */
  onboardingCompleted: boolean | null
  /**
   * Whether `checksReport` was requested after the last scan pass finished.
   * A report from before that point can miss the sessions the pass saved,
   * so the Checks step does not latch on it.
   */
  checksReportCurrent: boolean
}

export interface FirstRunLatch {
  /** Whether the "show steps this session" question has been answered. */
  decided: boolean
  showSteps: boolean
  agentsDone: boolean
  agentsFound: AgentFoundCount[]
  sessionsDone: boolean
  sessionsRead: { completed: number; total: number }
  checksDone: boolean
  checksResult: { windowSessions: number; deferredEvidence: number }
}

export const INITIAL_FIRST_RUN_LATCH: FirstRunLatch = {
  decided: false,
  // Not a guess: showing first-run UI before `onboardingCompleted` is known
  // would be as wrong as hiding it would be. Stays hidden until
  // {@link advanceFirstRunLatch} decides, or {@link resetFirstRunLatch} forces
  // it on.
  showSteps: false,
  agentsDone: false,
  agentsFound: [],
  sessionsDone: false,
  sessionsRead: { completed: 0, total: 0 },
  checksDone: false,
  checksResult: { windowSessions: 0, deferredEvidence: 0 },
}

// Decide once from the saved setting. Keep completed step results across routine scans.
// A report must follow the finished read pass before Checks can complete.
// Deferred evidence does not block progress: live sessions can requeue after every turn.
export function advanceFirstRunLatch(
  latch: FirstRunLatch,
  inputs: FirstRunInputs,
): FirstRunLatch {
  let next = latch
  if (!next.decided && inputs.onboardingCompleted != null) {
    next = {
      ...next,
      decided: true,
      showSteps: !inputs.onboardingCompleted,
    }
  }
  const found = inputs.scanStatus?.foundByAgent ?? []
  if (!next.agentsDone && found.length > 0 && found.every((row) => row.done)) {
    next = { ...next, agentsDone: true, agentsFound: found }
  }
  const phase = inputs.scanStatus?.phase
  if (!next.sessionsDone && phase === "saving" && inputs.scanStatus?.gate) {
    next = {
      ...next,
      sessionsDone: true,
      sessionsRead: inputs.scanStatus.read,
    }
  }
  const report = inputs.checksReport
  const readSettled = !next.showSteps || (next.sessionsDone && inputs.checksReportCurrent)
  if (
    !next.checksDone &&
    next.decided &&
    readSettled &&
    report != null &&
    report.pendingEvidence <= report.deferredEvidence
  ) {
    next = {
      ...next,
      checksDone: true,
      checksResult: {
        windowSessions: report.windowSessions,
        deferredEvidence: report.deferredEvidence,
      },
    }
  }
  return next
}

/** `ftue:reset` wipes the real data, so the pass it triggers must be tracked
 *  from the start. Brings the steps block back at once, rather than waiting
 *  to re-decide against whatever stale status is still on hand. */
export function resetFirstRunLatch(): FirstRunLatch {
  return { ...INITIAL_FIRST_RUN_LATCH, decided: true, showSteps: true }
}
