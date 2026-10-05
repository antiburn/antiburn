// The Overview's first-run latch. Decides, once per session, whether to show
// the Agents, Sessions and Checks steps block, and remembers each step's own numbers so
// a later routine pass does not reset a checklist the reader already saw.
// Pure code, with no module state, so a test drives it without any IPC
// mocking.

import type { ChecksReportPayload } from "../../../lib/insightsIpc"
import type { AgentFoundCount, ReadGateCounts, ScanStatus } from "../../../lib/ipc"

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
  sessionsGate: ReadGateCounts | null
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
  sessionsGate: null,
  checksDone: false,
  checksResult: { windowSessions: 0, deferredEvidence: 0 },
}

/**
 * Advance the latch from one set of inputs.
 *
 * Decides "show the steps block" once, the first time
 * {@link FirstRunInputs.onboardingCompleted} is known: true (show the steps)
 * when the stored setting says the first run has not finished, false
 * otherwise. The answer then holds for the rest of the session (see
 * {@link resetFirstRunLatch} for `ftue:reset`).
 *
 * `onboardingCompleted` — not `ScanStatus.finishedAt` and not the checks
 * report's `evidenceSettled` — is the signal, because both of those are
 * ordinarily unsettled for a few seconds after every launch: `finished_at`
 * lives only in the in-memory `ScanController` and is cleared every time a
 * pass starts (`scan/mod.rs`), and `evidenceSettled` goes false while the
 * evidence worker catches up with whatever a live agent session wrote since
 * the last launch. An Overview that read either signal during that window
 * would misread an ordinary launch as a first run and show the steps block
 * every time. The stored setting survives across launches — cleared only by
 * an explicit reset — so it tells "never finished a first run" from
 * "finished one already" correctly. One accepted consequence: a
 * revision-bump re-ingest does not bring the steps block back, even though
 * it marks evidence unsettled again — intended, since the device already
 * finished a first run.
 *
 * The Agents and Sessions steps each latch their own numbers the first time they finish, so
 * a later routine pass — every 5 minutes, and every launch, per the scan
 * design — does not reset a checklist the reader already saw.
 *
 * The Agents step finishes once discovery has found every agent's sessions, read
 * straight from `foundByAgent`: non-empty, and every entry done. A full pass
 * now waits at the backend's first-run sessions gate before it moves past the
 * "finding" phase — until the reader presses Agents' Next — so `phase` alone
 * can stay `"finding"` long after discovery itself is done. Reading the
 * phase instead, as before, would never finish the Agents step during that wait.
 *
 * The Checks step latches when the worker has no work it can claim: every pending
 * session is deferred by a retry backoff. A live session changes during its
 * check, backs off, and goes pending again after each turn, so neither
 * `evidenceSettled` nor a live pending count can tell when the first check
 * is done. In the steps block, the Checks step also waits for the Sessions step and for a report
 * requested after the pass finished, so a report from before the pass saved
 * its sessions cannot latch an empty result.
 */
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
      sessionsGate: inputs.scanStatus.gate,
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

/**
 * Un-latch the Sessions step, so the pass a reader's own "Include them" click
 * triggers replaces the read outcome they just asked to change — unlike a
 * routine tick, this pass has a reader waiting to see its result. The Checks step
 * un-latches too, because the pass adds sessions to check. The Agents step is
 * untouched: discovery does not depend on this setting.
 */
export function unlatchSessionsOutcome(latch: FirstRunLatch): FirstRunLatch {
  return {
    ...latch,
    sessionsDone: false,
    sessionsGate: null,
    checksDone: false,
    checksResult: INITIAL_FIRST_RUN_LATCH.checksResult,
  }
}
